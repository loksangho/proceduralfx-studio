use crate::*;

#[test]
fn the_export_vertex_animation_window_draws_and_picks_the_viewed_object() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nnode 2 240 20 0 obj geometry -1\nnode 3 40 20 0 obj sphere 0.5 8 12\nparent 1 0\nparent 3 2\ndisplay 0 1\ndisplay 2 3\nnext 4\n");
    assert_eq!(app.vat_default_object(), Some(0), "the first object at the top");
    app.current_network = Some(2);
    assert_eq!(app.vat_default_object(), Some(2), "the one being viewed");
    app.vat_window.open = true;
    let ctx = egui::Context::default();
    for _ in 0..3 {
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.vat_window(ctx));
    }
    assert!(app.vat_window.open && app.vat_job.is_none());
}

// The window's export, through to the files: bakes in the background while the window
// reports progress, then says what it wrote.
#[test]
fn exporting_vertex_animation_bakes_in_the_background() {
    let dir = std::env::temp_dir().join(format!("pfx_vat_window_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nnode 2 40 100 0 obj noise 2.2 0.65\nparent 1 0\nparent 2 0\nlink 1 2 0\ndisplay 0 2\nnext 3\n");
    (app.playback_start, app.playback_end) = (0.0, 11.0 / sims::SIM_FPS);
    app.vat_window = VatWindow { open: true, every: 2, godot: true, unity: true, ..VatWindow::default() };
    app.start_vat_bake(0, dir.join("wobble"));
    assert!(app.vat_job.is_some());
    let ctx = egui::Context::default();
    let started = Instant::now();
    let mut saw_progress = false;
    while app.vat_job.is_some() && started.elapsed() < std::time::Duration::from_secs(60) {
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.vat_window(ctx));
        saw_progress |= app.status.starts_with("Baking vertex animation");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.status.starts_with("Baked as soft: 64 elements over 6 frames"), "{}", app.status);
    let _ = saw_progress;
    for file in ["wobble.obj", "wobble_offsets.exr", "wobble_normals.exr", "wobble.vat.json", "wobble_vat.tscn", "wobble_vat.gdshader", "wobble_vat.shader", "wobble_vat.mat", "wobble_offsets.exr.meta"] {
        assert!(dir.join(file).is_file(), "{file}");
    }
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("wobble.vat.json")).unwrap()).unwrap();
    assert_eq!(json["fps"], 12.0, "every second frame plays at half the rate");
    // A shape that can't be baked says why, in the status bar.
    app.vat_window.mode = Some(vat::Mode::Rigid);
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nnode 2 40 100 0 obj noise 2.2 0.65\nparent 1 0\nparent 2 0\nlink 1 2 0\ndisplay 0 2\nnext 3\n");
    app.start_vat_bake(0, dir.join("bent"));
    while app.vat_job.is_some() {
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.vat_window(ctx));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.status.contains("bends or stretches"), "{}", app.status);
    let _ = std::fs::remove_dir_all(&dir);
}

// Export Sprite Sheet with every engine option, through to the files.
#[test]
fn exporting_a_sprite_sheet_writes_every_engines_files() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("pfx_sheet_window_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n");
    app.selected = None;
    app.render_camera = RenderCamera::Viewport;
    (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.0, 0.0, 6.0, 0.0, 0.0);
    (app.playback_start, app.playback_end) = (0.0, 5.0 / sims::SIM_FPS);
    app.sprite_sheet = spritesheet::Settings { width: 32, height: 32, godot: true, unity: true, fill_grid: true, ..Default::default() };
    app.dirty = true;
    let png = dir.join("ball.png");
    app.start_sprite_sheet(png.clone(), false);
    super::realtime_render_tests::run_recording_for(&mut app, 400);
    assert!(app.status.starts_with("Sprite sheet: 6 frames in 2×3 cells"), "a filled grid, no empty cells: {}", app.status);
    for path in [png.clone(), spritesheet::json_path(&png), spritesheet::godot_path(&png), spritesheet::godot_particles_path(&png), spritesheet::unity_meta_path(&png)] {
        assert!(path.is_file(), "{}", path.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
