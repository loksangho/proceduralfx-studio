// ProceduralFX Studio — node-based procedural modeller
// On Windows, a release build is a GUI program (no console window behind it).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Cargo.toml:  eframe = "0.27"   (same egui generation as your original code)

use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc,
};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
#[cfg(feature = "native-openvdb")]
use std::slice;
use std::time::Instant;
use rfd::FileDialog;

mod render_viewer;
mod abc;
mod ai;
mod anim;
mod attrib;
mod assistant;
mod boolean;
mod assets;
mod bullet;
mod cli;
mod color;
mod cops;
mod crowd;
mod deform;
mod docs;
mod envlight;
mod extrude;
mod fields;
mod fileio;
mod flipbook;
mod gpu_viewport;
mod gpu_volume;
mod gltfio;
mod flow;
mod fracture;
mod groom;
mod groups;
mod kinefx;
mod lsystem;
mod looping;
mod measure;
mod motion;
mod normals;
mod muscle;
mod nodeinfo;
mod packed;
mod perf;
mod applog;
mod pixelart;
mod platform;
mod paint;
mod poly;
mod presets;
mod ramp;
mod randomize;
mod curve;
mod remap;
mod remesh;
mod recovery;
mod raster;
mod sims;
mod providers;
mod snapshot;
mod shading;
mod sopkit;
mod sops;
mod strings;
mod spritesheet;
mod terrain;
mod testgeo;
mod textures;
mod tops;
mod transfer;
mod enumerate;
mod blur;
mod pop;
mod promote;
mod fluidsurface;
mod updater;
mod vat;
mod vellum;
mod usd;
mod vex;
mod vexsyntax;
mod wslrender;
mod vexmath;
mod volume;
mod vop;
mod whitewater;

// The app, split out of this file: its state and types, its methods by area (app/), the
// node graph and what it cooks, and helpers. Each is re-exported here, so the rest of the
// crate names them as before (crate::Graph, crate::Mesh...).
mod app;
mod app_state;
pub(crate) use app_state::*;
mod editor_helpers;
mod engines;
pub(crate) use editor_helpers::*;
mod graph;
pub(crate) use graph::*;
mod mesh;
pub(crate) use mesh::*;
mod node_types;
pub(crate) use node_types::*;
mod sop_ops;
pub(crate) use sop_ops::*;
mod support;
pub(crate) use support::*;
use render_viewer::{reveal_in_file_browser, RenderViewer};

fn main() -> eframe::Result<()> {
    // Windows: USD finds its own plugins in usd\ beside its DLLs, but not the plugin
    // libraries in plugin\usd, which the app ships next to itself.
    if cfg!(windows) {
        if let Some(plugins) = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("plugin").join("usd"))).filter(|dir| dir.is_dir()) {
            let mut path = plugins.into_os_string();
            if let Some(existing) = std::env::var_os("PXR_PLUGINPATH_NAME").filter(|p| !p.is_empty()) {
                path.push(";");
                path.push(existing);
            }
            std::env::set_var("PXR_PLUGINPATH_NAME", path);
        }
    }
    // `--usd-info <file>` lists a USD file's prims and exits (checks USD works in a packaged app).
    let args: Vec<String> = std::env::args().collect();
    if let Some(at) = args.iter().position(|a| a == "--usd-info") {
        let Some(file) = args.get(at + 1) else {
            eprintln!("usage: --usd-info <file.usd>");
            std::process::exit(2);
        };
        match usd::load_file(file, 0.0) {
            Ok(stage) => {
                for (path, prim) in &stage.prims {
                    println!("{path} {}", prim.type_name);
                }
                println!("{} prims", stage.prims.len());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    // `render` and `sprite-sheet` render a scene without a window, then exit.
    if let Some(code) = cli::main(&args) {
        std::process::exit(code);
    }
    recovery::install_panic_hook();
    applog::start(app_config_dir().as_deref());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("ProceduralFX Studio")
            .with_icon(app_icon()),
        // macOS: winit's default app menu quits on Cmd+Q without asking the window,
        // which would skip "Save changes before quitting?"; the app handles Cmd+Q itself.
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_default_menu(false);
        })),
        ..Default::default()
    };
    // wgpu (Metal, DirectX 12, Vulkan) unless PFX_RENDERER=opengl. If wgpu can't
    // start (no usable GPU or driver) before the app opens, run again with OpenGL.
    let opengl = std::env::var("PFX_RENDERER").is_ok_and(|r| r.eq_ignore_ascii_case("opengl"));
    let opened = Arc::new(AtomicBool::new(false));
    let run = |renderer: eframe::Renderer, options: eframe::NativeOptions| {
        let opened = opened.clone();
        eframe::run_native(
            "ProceduralFX Studio",
            eframe::NativeOptions { renderer, ..options },
            Box::new(move |cc| {
                opened.store(true, Ordering::SeqCst);
                install_fonts(&cc.egui_ctx);
                let mut app = ProceduralFxStudioApp::default();
                if let Some(dir) = recovery_dir() {
                    let (session, leftovers) = recovery::Session::start(dir);
                    app.recovery = Some(session);
                    app.recovery_offer = leftovers.into_iter().next();
                }
                // New here: start from an effect (not while offering to recover work).
                app.welcome_open = app.recovery_offer.is_none() && app::welcome_at_startup();
                // PFX_VIEWPORT=cpu keeps the viewport on raster.rs (to compare the two).
                let cpu = std::env::var("PFX_VIEWPORT").is_ok_and(|v| v.eq_ignore_ascii_case("cpu"));
                match &cc.wgpu_render_state {
                    Some(state) => {
                        let info = state.adapter.get_info();
                        let gpu = format!("{} ({:?}{})", info.name, info.backend, [info.driver.as_str(), info.driver_info.as_str()].iter().filter(|s| !s.is_empty()).fold(String::new(), |out, s| out + ", " + s));
                        log::info!("GPU: {gpu}{}", if cpu { "; viewport on the CPU (PFX_VIEWPORT=cpu)" } else { "" });
                        app.gpu_info = Some(gpu);
                    }
                    None => log::info!("Renderer: OpenGL, without the GPU viewport"),
                }
                app.gpu_viewport = match cc.wgpu_render_state.clone().filter(|_| !cpu).map(gpu_viewport::GpuViewport::new) {
                    Some(Ok(viewport)) => Some(viewport),
                    // The GPU refused something: draw the viewport on the CPU instead.
                    Some(Err(error)) => {
                        app.gpu_fallback(error);
                        None
                    }
                    None => None,
                };
                Ok(Box::new(app))
            }),
        )
    };
    if opengl {
        return run(eframe::Renderer::Glow, options);
    }
    match run(eframe::Renderer::Wgpu, options.clone()) {
        Err(eframe::Error::Wgpu(error)) if !opened.load(Ordering::SeqCst) => {
            eprintln!("wgpu couldn't start ({error}); using OpenGL");
            run(eframe::Renderer::Glow, options)
        }
        result => result,
    }
}

#[cfg(test)]
mod app_tests;
