// Node types: moved out of main.rs.

use crate::*;

pub(crate) type NodeId = usize;

pub(crate) fn prim_path_ui(ui: &mut egui::Ui, label: &str, path: &mut String) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        let response = ui.add(egui::TextEdit::singleline(path).desired_width(200.0));
        if response.lost_focus() {
            *path = usd::sanitize_path(path);
        }
        response.changed()
    })
    .inner
}

// Prim pattern field with a button that fills in the prim picked in the scene graph.
pub(crate) fn prim_pattern_ui(ui: &mut egui::Ui, pattern: &mut String, picked: &Option<String>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Primitives");
        changed |= ui.add(egui::TextEdit::singleline(pattern).hint_text("/geo/* /lights/**").desired_width(190.0)).changed();
        if ui.add_enabled(picked.is_some(), egui::Button::new("⇐ picked")).on_hover_text("Use the prim selected in the scene graph").clicked() {
            *pattern = picked.clone().unwrap_or_default();
            changed = true;
        }
    });
    ui.weak("Paths with * (one level) and ** (any depth); ^ excludes.");
    changed
}

pub(crate) fn render_camera_combo(ui: &mut egui::Ui, choice: &mut RenderCamera, cameras: &[(NodeId, String)]) {
    let label = match *choice {
        RenderCamera::Auto => "Auto (selected camera, else viewport)".to_string(),
        RenderCamera::Viewport => "Viewport".to_string(),
        RenderCamera::Node(id) => cameras.iter().find(|(c, _)| *c == id).map_or_else(|| format!("camera{id} (deleted)"), |(_, name)| name.clone()),
    };
    egui::ComboBox::from_id_salt("render_camera")
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.selectable_value(choice, RenderCamera::Auto, "Auto (selected camera, else viewport)");
            ui.selectable_value(choice, RenderCamera::Viewport, "Viewport");
            for (id, name) in cameras {
                ui.selectable_value(choice, RenderCamera::Node(*id), name);
            }
        });
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RenderCamera {
    // Selected camera node if any, otherwise the viewport.
    Auto,
    Viewport,
    Node(NodeId),
}

pub(crate) struct CamRig {
    pub(crate) target: Vec3,
    pub(crate) rot_x: f32,
    pub(crate) rot_y: f32,
    pub(crate) zoom: f32,
    pub(crate) focal: f32,
    pub(crate) orthographic: bool,
    pub(crate) pan: [f32; 2],
}

impl CamRig {
    // The GPU renderer's camera for an image of `size` pixels, framed as MoonRay renders
    // frame it (see export_scene_rdla): the viewport's pan moves the camera, and the focal
    // length applies along the image's smaller side.
    pub(crate) fn realtime_camera(&self, size: [u32; 2], near: f32, viewport_focal_px: f32) -> gpu_viewport::Camera {
        let (sx, cx) = self.rot_x.sin_cos();
        let (sy, cy) = self.rot_y.sin_cos();
        let right = Vec3::new(cy, 0.0, -sy);
        let up = Vec3::new(-sx * sy, cx, -sx * cy);
        let pan_scale = self.zoom / viewport_focal_px.max(1.0);
        let target = self.target.sub(right.scale(self.pan[0] * pan_scale)).add(up.scale(self.pan[1] * pan_scale));
        let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
        let focal = w.min(h) * self.focal;
        gpu_viewport::Camera {
            target: [target.x, target.y, target.z],
            rotation: [sx, cx, sy, cy],
            zoom: self.zoom,
            center: egui::vec2(w * 0.5, h * 0.5),
            size: egui::vec2(w, h),
            focal,
            ortho_scale: focal / self.zoom.max(MIN_ZOOM),
            orthographic: self.orthographic,
            near,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenderOpts {
    pub(crate) camera: RenderCamera,
    pub(crate) max_depth: usize,
    pub(crate) light_samples: usize,
    pub(crate) bsdf_samples: usize,
    pub(crate) background: [f32; 3],
    // Timeline range rendered frame by frame when `sequence` is on.
    pub(crate) sequence: bool,
    pub(crate) range_start: f32,
    pub(crate) range_end: f32,
    pub(crate) range_step: f32,
    // Render the STAGE network's displayed USD stage instead of the OBJ objects.
    pub(crate) stage: bool,
    // Render that stage through Hydra (hd_render with MoonRay's delegate, hdMoonray).
    pub(crate) hydra: bool,
    // In-scattering light samples per volume step: the main cost of smoke and fire renders.
    pub(crate) volume_samples: usize,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            camera: RenderCamera::Auto,
            max_depth: 5,
            light_samples: 2,
            bsdf_samples: 2,
            background: [0.0; 3],
            sequence: false,
            range_start: 0.0,
            range_end: 1.0,
            range_step: 0.1,
            stage: false,
            hydra: false,
            volume_samples: 2,
        }
    }
}

// "Through Hydra" for a STAGE render; true when it changed.
pub(crate) fn hydra_checkbox(ui: &mut egui::Ui, opts: &mut RenderOpts) -> bool {
    ui.add_enabled(opts.stage, egui::Checkbox::new(&mut opts.hydra, "Through Hydra (hdMoonray)"))
        .on_hover_text("Renders the stage with hd_render and MoonRay's Hydra render delegate, as USD-native apps do: the stage is written as USD and read by Hydra, instead of being translated by ProceduralFX Studio")
        .changed()
}

// Timeline times covered by a frame range, capped to keep runaway ranges bounded.
pub(crate) fn frame_times(start: f32, end: f32, step: f32) -> Vec<f32> {
    let step = step.max(1e-4);
    let count = (((end - start) / step) + 1e-4).floor().max(0.0) as usize + 1;
    (0..count.min(10_000)).map(|i| start + i as f32 * step).collect()
}

// Inserts a frame number into an output path: `name.####.exr` style, or `name.0001.exr` when no `#` is present.
pub(crate) fn frame_output(output: &str, frame: usize) -> String {
    if let Some(start) = output.find('#') {
        let width = output[start..].chars().take_while(|c| *c == '#').count();
        return format!("{}{:0width$}{}", &output[..start], frame, &output[start + width..], width = width);
    }
    let path = std::path::Path::new(output);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
    let name = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.{frame:04}.{ext}"),
        None => format!("{stem}.{frame:04}"),
    };
    path.with_file_name(name).to_string_lossy().into_owned()
}

// MoonRay inside the running app bundle (Contents/Resources or Contents/MacOS), if any.
pub(crate) fn bundled_moonray() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    // A Linux install (e.g. the Flatpak) puts it next to the app in <prefix>/bin.
    [contents.join("Resources").join("moonray"), contents.join("MacOS").join("moonray"), contents.join("bin").join("moonray")]
        .into_iter()
        .find(|p| p.exists())
}

// Detect a bundled or system MoonRay executable in common locations. For macOS
// packaging the binary lives in the app bundle; during development it can be
// placed in `native/moonray/macos/moonray` before packaging.
pub(crate) fn detect_moonray_executable() -> String {
    // Windows: MoonRay runs in a WSL container through the helper next to the app.
    if let Some(helper) = wslrender::helper() {
        return helper.to_string_lossy().into_owned();
    }
    if let Some(bundled) = bundled_moonray() {
        return bundled.to_string_lossy().into_owned();
    }
    let candidates = [
        "./moonray",
        "./Resources/moonray",
        "./Contents/Resources/moonray",
        "./Contents/MacOS/moonray",
        "./native/moonray/macos/moonray",
        "/usr/local/bin/moonray",
        "/opt/homebrew/bin/moonray",
    ];
    candidates
        .into_iter()
        .find(|c| std::path::Path::new(c).exists())
        .map(str::to_owned)
        .or_else(|| resolve_executable("moonray").map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "./moonray".into())
}

// The file a render would actually run for `executable`, resolved the way
// spawn_moonray_render does: an existing path, the bundled copy, then PATH.
pub(crate) fn resolve_executable(executable: &str) -> Option<PathBuf> {
    let path = PathBuf::from(executable.trim());
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.exists() {
        return Some(path);
    }
    if let Some(bundled) = bundled_moonray() {
        return Some(bundled);
    }
    if path.components().count() == 1 {
        let paths = std::env::var_os("PATH")?;
        return std::env::split_paths(&paths).map(|dir| dir.join(&path)).find(|p| p.is_file());
    }
    None
}

// Sliders for the ch("name") parameters `code` uses (like Houdini's spare
// parameters, created automatically); values unused by the code are dropped.
// One group of toolbar controls that wraps to the next line as a whole when it doesn't
// fit (its width is the one measured on the frame before).
pub(crate) fn toolbar_group<R>(ui: &mut egui::Ui, name: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let key = egui::Id::new(("toolbar_group", name));
    let width: f32 = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(0.0);
    let at_row_start = ui.cursor().min.x <= ui.max_rect().left() + 1.0;
    // What's left of this row (available_width() is a whole row's in a wrapping layout).
    if width > 0.0 && ui.available_size_before_wrap().x < width && !at_row_start {
        ui.end_row();
    }
    let response = ui.horizontal(add);
    ui.ctx().data_mut(|d| d.insert_temp(key, response.response.rect.width()));
    response.inner
}

// The searchable add-node list the right-click and Tab menus share. Type to search
// every category; Down/Up move through the matches and Enter adds the highlighted
// one (`pick`). With nothing typed, the categories are submenus.
pub(crate) fn node_search_ui(ui: &mut egui::Ui, query: &mut String, pick: &mut usize, templates: &[(&'static str, Vec<NodeKind>)]) -> Option<NodeKind> {
    // Taken before the field sees them (a one-line field would move its cursor).
    let (down, up) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)));
    let before = query.clone();
    ui.add(egui::TextEdit::singleline(query).hint_text("Search nodes…").desired_width(f32::INFINITY)).request_focus();
    if *query != before {
        *pick = 0;
    }
    let mut chosen = None;
    if !query.trim().is_empty() {
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let found: Vec<(&str, &NodeKind)> = templates
            .iter()
            .flat_map(|(cat, kinds)| kinds.iter().map(move |k| (*cat, k)))
            .filter(|(cat, k)| node_matches(k, cat, query))
            .collect();
        if found.is_empty() {
            ui.weak("No matching nodes");
            return None;
        }
        *pick = search_step(*pick, found.len(), down, up);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (i, (cat, kind)) in found.iter().enumerate() {
                let row = ui.selectable_label(i == *pick, format!("{}   ({cat})", kind.menu_display()));
                if i == *pick && (down || up) {
                    row.scroll_to_me(None);
                }
                let row = match kind.menu_help() {
                    Some(help) => row.on_hover_text(help),
                    None => row,
                };
                if row.clicked() || (enter && i == *pick) {
                    chosen = Some((*kind).clone());
                }
            }
        });
        return chosen;
    }
    ui.separator();
    for (cat, kinds) in templates {
        ui.menu_button(*cat, |ui| {
            for kind in kinds {
                let button = ui.button(kind.menu_display());
                let button = match kind.menu_help() {
                    Some(help) => button.on_hover_text(help),
                    None => button,
                };
                if button.clicked() {
                    chosen = Some(kind.clone());
                    ui.close_kind(egui::UiKind::Menu);
                }
            }
        });
    }
    chosen
}

// The highlighted result after Down/Up, kept within `count` results.
pub(crate) fn search_step(pick: usize, count: usize, down: bool, up: bool) -> usize {
    let last = count.saturating_sub(1);
    let pick = pick.min(last);
    if down {
        (pick + 1).min(last)
    } else if up {
        pick.saturating_sub(1)
    } else {
        pick
    }
}

// Where the focused VEX editor's id is kept, so Tab over the network doesn't steal its indent.
pub(crate) const CODE_EDITOR_FOCUS: &str = "vex_code_editor_focus";

// What new Volume and Heightfield Wrangles start with.
pub(crate) const VOLUME_WRANGLE_EXAMPLE: &str = "// Thin the density towards the top of the volume.\n@density *= fit(@P.y, 0, 2, 1, 0);";
pub(crate) const HEIGHTFIELD_WRANGLE_EXAMPLE: &str = "// Add ripples, stronger where the mask is.\n@height += sin(@P.x * 2) * 0.3 * (1 + @mask);";

// (menu label, code) examples for the Volume and Heightfield Wrangles.
pub(crate) const VOLUME_WRANGLE_EXAMPLES: [(&str, &str); 4] = [
    ("Fade towards the top", VOLUME_WRANGLE_EXAMPLE),
    ("Break up with noise", "@density *= fit(noise(@P * 2), 0.35, 0.65, 0, 1);"),
    ("Blur (average the neighbours)", "vector i = volumepostoindex(0, \"density\", @P);\nfloat sum = @density;\nforeach (vector d; array(set(1,0,0), set(-1,0,0), set(0,1,0), set(0,-1,0), set(0,0,1), set(0,0,-1)))\n    sum += volumeindex(0, \"density\", i + d);\n@density = sum / 7;"),
    ("Keep a sphere", "@density *= length(@P) < 1;"),
];
pub(crate) const HEIGHTFIELD_WRANGLE_EXAMPLES: [(&str, &str); 4] = [
    ("Ripples", HEIGHTFIELD_WRANGLE_EXAMPLE),
    ("Flatten above a height", "@height = min(@height, 2);"),
    ("Mask from height", "@mask = fit(@height, 0, 3, 0, 1);"),
    ("Smooth (average the neighbours)", "vector i = volumepostoindex(0, \"height\", @P);\n@height = (volumeindex(0, \"height\", i + set(1, 0, 0)) + volumeindex(0, \"height\", i - set(1, 0, 0))\n         + volumeindex(0, \"height\", i + set(0, 1, 0)) + volumeindex(0, \"height\", i - set(0, 1, 0))) / 4;"),
];

pub(crate) fn examples_menu(ui: &mut egui::Ui, code: &mut String, examples: &[(&str, &str)]) -> bool {
    let mut changed = false;
    ui.menu_button("Insert example ▾", |ui| {
        for (label, example) in examples {
            if ui.button(*label).on_hover_text("Replaces the current code").clicked() {
                *code = example.to_string();
                changed = true;
                ui.close_kind(egui::UiKind::Menu);
            }
        }
    });
    changed
}

// The VEX editor the wrangles share: the code, its ch() parameters, errors, the last
// run's report and output, and the language reference. `noun` names what it runs over.
pub(crate) fn vex_editor_ui(ui: &mut egui::Ui, code: &mut String, domain: usize, noun: &str, channels: &mut vex::Channels, hint: &str) -> bool {
    let mut c = false;
    let dark = ui.visuals().dark_mode;
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = vexsyntax::highlight(text.as_str(), font.clone(), dark);
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let editor = egui::TextEdit::multiline(code)
        .code_editor()
        .desired_rows(14)
        .desired_width(f32::INFINITY)
        .hint_text(hint)
        .layouter(&mut layouter)
        .show(ui);
    c |= editor.response.changed();
    if editor.response.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(CODE_EDITOR_FOCUS), editor.response.id));
    }
    // F1 on a function opens its help.
    if editor.response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::F1)) {
        let word = editor.cursor_range.and_then(|range| vexsyntax::word_at(code, range.primary.index));
        let anchor = match word.as_deref() {
            Some(name) if vexsyntax::is_builtin(name) => docs::vex_anchor(name),
            // Keywords, types and anything else: the language guide.
            _ => "vex".to_string(),
        };
        if let Err(e) = open_documentation_at(Some(&anchor)) {
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("vex_help_error"), e));
        }
    }
    if let Some(e) = ui.ctx().data(|d| d.get_temp::<String>(egui::Id::new("vex_help_error"))) {
        ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("Couldn't open the help: {e}"));
    }
    ui.weak("Put the text cursor on a function and press F1 for its help.");
    c |= channel_params_ui(ui, code, channels);

    let report = vex::last_report(code, domain);
    let error = match &report {
        Some(report) => report.error.clone(),
        None => vex::check(code).err(),
    };
    match error {
        Some(error) => {
            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {error}"));
            // Show the offending source line under the message.
            let line = error
                .strip_prefix("line ")
                .and_then(|rest| rest.split(|ch: char| !ch.is_ascii_digit()).next())
                .and_then(|n| n.parse::<usize>().ok());
            if let Some(text) = line.and_then(|n| code.lines().nth(n.saturating_sub(1))) {
                ui.monospace(format!("{:>4} | {}", line.unwrap_or(0), text.trim_end()));
            }
            if report.is_some() {
                ui.small("The node passes its input through unchanged until the error is fixed.");
            }
        }
        None => match &report {
            Some(report) => {
                ui.colored_label(
                    egui::Color32::LIGHT_GREEN,
                    format!("✔ Ran over {} {noun} in {:.2} ms", report.elements, report.millis),
                );
            }
            None => {
                ui.weak("✔ Compiles. It runs when the node is part of the displayed network.");
            }
        },
    }
    if let Some(report) = report.as_ref().filter(|r| !r.output.is_empty()) {
        egui::CollapsingHeader::new(format!("Output ({} lines)", report.output.len()))
            .id_salt("wrangle_output")
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                    for line in &report.output {
                        ui.monospace(line);
                    }
                });
            });
    }
    egui::CollapsingHeader::new("Language reference").id_salt("wrangle_reference").show(ui, |ui| {
        ui.label(egui::RichText::new(vex::SYNTAX_HELP).monospace().small());
        let mut group = "";
        for (_, _, _, function_group, signature) in vex::BUILTINS {
            if *function_group != group {
                group = function_group;
                ui.add_space(4.0);
                ui.strong(group);
            }
            ui.label(egui::RichText::new(*signature).monospace().small());
        }
    });
    c
}

pub(crate) fn channel_params_ui(ui: &mut egui::Ui, code: &str, channels: &mut vex::Channels) -> bool {
    let refs = vex::channel_refs(code);
    let mut changed = false;
    let before = channels.len();
    // Values the code no longer reads go; a chramp() ramp is several entries.
    channels.retain(|key, _| refs.iter().any(|(n, kind)| if *kind == 'r' { vex::is_ramp_entry(key, n) } else { n == key }));
    changed |= channels.len() != before;
    if refs.is_empty() {
        return changed;
    }
    ui.separator();
    ui.strong("Parameters");
    for (name, kind) in refs {
        if kind == 'r' {
            let (ramp, mut colour) = vex::channel_ramp(channels, &name).unwrap_or_else(|| (vex::default_ramp(), false));
            if vex::channel_ramp(channels, &name).is_none() {
                vex::set_channel_ramp(channels, &name, &ramp, colour);
            }
            let mut ramp_changed = false;
            ui.horizontal(|ui| {
                ui.label(&name);
                ramp_changed |= ui.checkbox(&mut colour, "Colour").on_hover_text("A colour ramp: chramp() gives a vector (a colour) instead of a float").changed();
            });
            let width = if colour { 3 } else { 1 };
            let mut text = ramp.format(width);
            ramp_changed |= ramp_editor(ui, ("chramp", &name), "", &mut text, width, vex::default_ramp);
            if ramp_changed {
                let ramp = ramp::Ramp::from_text(&text, width, vex::default_ramp);
                vex::set_channel_ramp(channels, &name, &ramp, colour);
                changed = true;
            }
            continue;
        }
        let value = channels.entry(name.clone()).or_insert([0.0; 3]);
        ui.horizontal(|ui| {
            if kind == 'v' {
                changed |= group_label(ui, &name, 0.01, &mut value[..]);
            } else {
                ui.label(&name);
            }
            match kind {
                'i' => {
                    let mut v = value[0].round() as i32;
                    if pfield(ui, pdrag(&mut v).speed(0.1)).changed() {
                        value[0] = v as f32;
                        changed = true;
                    }
                }
                'v' => {
                    for c in value.iter_mut() {
                        changed |= pfield(ui, pdrag(c).speed(0.01)).changed();
                    }
                }
                _ => {
                    changed |= pfield(ui, pslider(&mut value[0], 0.0..=1.0).clamping(egui::SliderClamping::Never)).changed();
                }
            }
        });
    }
    ui.small("From ch(\"name\") / chi / chv / chramp in the code. Drag past the slider's end for larger values.");
    changed
}

// The window and Dock icon, from packaging/logo.
pub(crate) fn app_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../packaging/logo/icon-256.png")).expect("bundled icon").to_rgba8();
    egui::IconData { width: image.width(), height: image.height(), rgba: image.into_raw() }
}

// The image a UV Quick Shade file shows ("" = the built-in checker), loaded once.
pub(crate) fn quickshade_texture(file: &str) -> Arc<raster::Texture> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<raster::Texture>>>> = std::sync::OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(file.to_owned())
        .or_insert_with(|| {
            let loaded = (!file.is_empty()).then(|| image::open(file).ok()).flatten().map(|img| {
                let img = img.thumbnail(1024, 1024).to_rgba8();
                raster::Texture { width: img.width() as usize, height: img.height() as usize, pixels: img.pixels().map(|p| p.0).collect() }
            });
            Arc::new(loaded.unwrap_or_else(|| {
                let size = 256;
                raster::Texture { width: size, height: size, pixels: sops::checker_image(size) }
            }))
        })
        .clone()
}

// The viewport's rasterized surfaces, uploaded into one reused texture.
pub(crate) fn viewport_texture(ctx: &egui::Context, image: egui::ColorImage) -> egui::TextureHandle {
    static TEXTURE: std::sync::OnceLock<std::sync::Mutex<Option<egui::TextureHandle>>> = std::sync::OnceLock::new();
    let mut slot = TEXTURE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_mut() {
        Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
        None => *slot = Some(ctx.load_texture("viewport-surfaces", image, egui::TextureOptions::LINEAR)),
    }
    slot.clone().expect("set above")
}

// Per-user settings folder (recent files, updater preferences).
pub(crate) fn app_config_dir() -> Option<PathBuf> {
    // Tests click through the real UI (recent scenes, layouts, assets): keep what they
    // write out of the user's own settings.
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("pfx_test_config_{}", std::process::id())));
    }
    if cfg!(windows) {
        // %APPDATA%\ProceduralFX Studio (Windows doesn't set HOME).
        return std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("ProceduralFX Studio"));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        Some(home?.join("Library/Application Support/ProceduralFX Studio"))
    } else {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config")))?;
        Some(base.join("proceduralfx-studio"))
    }
}

pub(crate) const MAX_RECENT_SCENES: usize = 10;

pub(crate) fn recent_scenes_file() -> Option<PathBuf> {
    // Tests save and open scenes too; they mustn't add to the user's list.
    if cfg!(test) {
        return None;
    }
    Some(app_config_dir()?.join("recent_scenes.txt"))
}

// Runs the app without a window until its flipbook, render or sprite sheet recording is
// done (or for at most `steps` frames): each frame cooks (simulations in the background)
// and the GPU viewport renders the frame to capture. Reports frames done and the total;
// returns what the recording said when it finished (what it saved, or why it couldn't).
pub(crate) fn drive_recording(app: &mut ProceduralFxStudioApp, steps: Option<usize>, progress: &mut dyn FnMut(usize, usize)) -> Option<String> {
    let ctx = egui::Context::default();
    let screen = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 400.0))), ..Default::default() };
    let mut step = 0;
    let mut finished = None;
    while steps.is_none_or(|limit| step < limit) {
        step += 1;
        if app.simulation_rx.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = ctx.run(screen.clone(), |ctx| {
            app.flipbook_step(ctx);
            // Cooking again afterwards (back at the resume time) replaces the status.
            if app.flipbook_recording.is_none() && finished.is_none() {
                finished = Some(app.status.clone());
            }
            if app.dirty || app.simulation_rx.is_some() {
                app.rebuild_graph();
                app.dirty = false;
            }
            app.flipbook_request_capture(ctx);
            egui::CentralPanel::default().show(ctx, |ui| app.viewport(ui, ctx));
        });
        match &app.flipbook_recording {
            Some(recording) => progress(recording.next, recording.times.len()),
            None => break,
        }
    }
    finished
}

// The assets folder. Tests use their own.
pub(crate) fn default_asset_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(app_config_dir()?.join("assets"))
}

// Where autosaves and crash reports go. Tests mustn't touch the user's.
pub(crate) fn recovery_dir() -> Option<PathBuf> {
    // In tests this is in their own temporary settings folder (app_config_dir).
    Some(app_config_dir()?.join("recovery"))
}

pub(crate) fn load_recent_scenes() -> Vec<PathBuf> {
    recent_scenes_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| parse_recent_scenes(&text))
        .unwrap_or_default()
}

pub(crate) fn parse_recent_scenes(text: &str) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = PathBuf::from(line);
        if !list.contains(&path) && list.len() < MAX_RECENT_SCENES {
            list.push(path);
        }
    }
    list
}

// Moves `path` to the front of the recent list.
pub(crate) fn push_recent_scene(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(MAX_RECENT_SCENES);
}

pub(crate) fn save_recent_scenes(list: &[PathBuf]) {
    let Some(file) = recent_scenes_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = std::fs::write(file, text);
}

// Writes via a temporary file so a failed save never leaves a half-written scene.
pub(crate) fn write_file_atomically(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".saving");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub(crate) const SHORTCUT_NEW: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::N);
pub(crate) const SHORTCUT_OPEN: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::O);
pub(crate) const SHORTCUT_SAVE: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
pub(crate) const SHORTCUT_QUIT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q);
pub(crate) const SHORTCUT_SAVE_AS: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT), egui::Key::S);

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

// Runs `cmd` with stdout and stderr written to `log_path`, killing it once `cancel` is set.
// hd_render: next to the MoonRay binary, in the app bundle, in a MoonRay build
// tree (PFX_HD_RENDER), or on PATH.
pub(crate) fn find_hd_render(moonray: &std::path::Path) -> Option<PathBuf> {
    let mut candidates = vec![moonray.with_file_name("hd_render")];
    if let Some(contents) = std::env::current_exe().ok().and_then(|exe| exe.parent()?.parent().map(|p| p.to_path_buf())) {
        candidates.extend([contents.join("Resources/hd_render"), contents.join("bin/hd_render")]);
    }
    if let Some(path) = std::env::var_os("PFX_HD_RENDER") {
        candidates.insert(0, path.into());
    }
    if let Some(dirs) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&dirs).map(|dir| dir.join("hd_render")));
    }
    candidates.into_iter().find(|path| path.is_file())
}

// Where hdMoonray's USD plugin (plugInfo.json) is, when hd_render runs from a
// MoonRay install or build tree; the app bundles it with USD's own plugins.
pub(crate) fn hydra_plugin_path(hd_render: &std::path::Path) -> Option<String> {
    if let Some(path) = std::env::var_os("PFX_HYDRA_PLUGIN_PATH") {
        return Some(path.to_string_lossy().into_owned());
    }
    let dirs: Vec<PathBuf> = hd_render.ancestors().skip(1).take(8).flat_map(|dir| [dir.join("plugin/pxr"), dir.join("plugin/usd")]).collect();
    dirs.into_iter().find(|dir| dir.join("hd_moonray_debug/plugInfo.json").is_file()).map(|dir| dir.to_string_lossy().into_owned())
}

// What was last typed into a parameter field (see pdrag and parameters_pane).
pub(crate) enum Typed {
    Number,
    Expression(String),
    Error(String),
    // Delete Channel, from the menu of a field showing this value.
    Delete(f32),
}

thread_local! {
    pub(crate) static TYPED: std::cell::RefCell<Option<Typed>> = const { std::cell::RefCell::new(None) };
    pub(crate) static EXPRESSION_TIME: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
    // The value of the field pdrag/pslider made last, for pfield's menu.
    pub(crate) static FIELD_VALUE: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

// Reads typed text: a number, or an expression ($F, $FF, $T, VEX) evaluated at the
// current time. An expression whose value equals the field's still changes the
// field (by a step), so parameters_pane can tell which parameter it was typed into.
pub(crate) fn typed_parser(current: f64, integral: bool) -> impl Fn(&str) -> Option<f64> {
    typed_parser_in(current, integral, f64::MAX)
}

// The same, for a slider that stops at `max`: the step goes down when up would pass it (an
// expression giving 1 typed into a 0-1 slider at 1 would otherwise be clamped back to 1, and
// look like no change).
pub(crate) fn typed_parser_in(current: f64, integral: bool, max: f64) -> impl Fn(&str) -> Option<f64> {
    move |text: &str| {
        let text = text.trim();
        if let Ok(v) = text.parse::<f64>() {
            TYPED.with(|t| *t.borrow_mut() = Some(Typed::Number));
            return Some(v);
        }
        match vex::eval_expression(text, EXPRESSION_TIME.with(|t| t.get())) {
            Ok(v) => {
                TYPED.with(|t| *t.borrow_mut() = Some(Typed::Expression(text.to_owned())));
                let v = v as f64;
                let step = if integral { 1.0 } else { (current.abs() * 1e-4).max(1e-4) };
                Some(if v != current { v } else if v + step <= max { v + step } else { v - step })
            }
            Err(error) => {
                TYPED.with(|t| *t.borrow_mut() = Some(Typed::Error(error)));
                None
            }
        }
    }
}

// Number fields that also take expressions.
pub(crate) fn pdrag<'a, N: egui::emath::Numeric>(value: &'a mut N) -> egui::DragValue<'a> {
    let current = value.to_f64();
    FIELD_VALUE.with(|v| v.set(current));
    egui::DragValue::new(value).custom_parser(typed_parser(current, N::INTEGRAL))
}

// The closest the view gets to what it looks at, so tiny things (a 0.001 sphere) can fill it.
pub(crate) const MIN_ZOOM: f32 = 0.001;

pub(crate) fn pslider<'a, N: egui::emath::Numeric>(value: &'a mut N, range: std::ops::RangeInclusive<N>) -> PSlider<'a> {
    let current = value.to_f64();
    FIELD_VALUE.with(|v| v.set(current));
    let max = range.end().to_f64();
    PSlider { slider: egui::Slider::new(value, range).custom_parser(typed_parser_in(current, N::INTEGRAL, max)), label: String::new(), value: current }
}

// The selected node's parameters that follow an expression, for the sliders showing them:
// set by the Parameters pane before it draws (see parameters_pane), matched to a slider by
// its value. A slider's name, clicked, switches it between its value and its expression.
pub(crate) struct ExpressionField {
    pub(crate) channel: ChannelId,
    pub(crate) value: f32,
    pub(crate) text: String,
    pub(crate) shown: bool,
    pub(crate) taken: bool,
    // The parameter's name, as letters and digits ("birthrate"), to tell sliders showing the
    // same value apart; and whether a slider carried that name last frame (then only it may
    // show the expression).
    pub(crate) key: String,
    pub(crate) named: bool,
}

pub(crate) fn name_key(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

pub(crate) enum ExpressionEdit {
    Toggle(ChannelId),
    Set(ChannelId, String),
}

thread_local! {
    pub(crate) static EXPRESSION_FIELDS: std::cell::RefCell<Vec<ExpressionField>> = const { std::cell::RefCell::new(Vec::new()) };
    pub(crate) static EXPRESSION_EDITS: std::cell::RefCell<Vec<ExpressionEdit>> = const { std::cell::RefCell::new(Vec::new()) };
    // Expressions a slider carried the name of this frame.
    pub(crate) static EXPRESSION_NAMED: std::cell::RefCell<Vec<ChannelId>> = const { std::cell::RefCell::new(Vec::new()) };
}

// A parameter slider (see pslider): an egui Slider that knows its name, so the name can
// switch it to its expression.
pub(crate) struct PSlider<'a> {
    slider: egui::Slider<'a>,
    label: String,
    value: f64,
}

impl<'a> PSlider<'a> {
    pub(crate) fn text(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    pub(crate) fn logarithmic(mut self, on: bool) -> Self {
        self.slider = self.slider.logarithmic(on);
        self
    }
    pub(crate) fn smallest_positive(mut self, v: f64) -> Self {
        self.slider = self.slider.smallest_positive(v);
        self
    }
    pub(crate) fn clamping(mut self, c: egui::SliderClamping) -> Self {
        self.slider = self.slider.clamping(c);
        self
    }
    pub(crate) fn suffix(mut self, s: impl ToString) -> Self {
        self.slider = self.slider.suffix(s);
        self
    }
    pub(crate) fn show_value(mut self, on: bool) -> Self {
        self.slider = self.slider.show_value(on);
        self
    }
}

impl egui::Widget for PSlider<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        // The expression this slider shows, if any: the first not yet claimed with its value.
        // Matched by value and name; by value alone only for a parameter no slider names.
        let field = EXPRESSION_FIELDS.with(|f| {
            let mut fields = f.borrow_mut();
            let v = self.value as f32;
            let label = name_key(&self.label);
            let same = |e: &ExpressionField| (e.value - v).abs() <= 1e-4 * v.abs().max(1.0);
            let named = |e: &ExpressionField| e.key.len() >= 2 && !label.is_empty() && (label.contains(&e.key) || e.key.contains(&label));
            for e in fields.iter().filter(|e| same(e) && named(e)) {
                EXPRESSION_NAMED.with(|n| n.borrow_mut().push(e.channel));
            }
            let at = fields.iter().position(|e| !e.taken && same(e) && named(e)).or_else(|| fields.iter().position(|e| !e.taken && same(e) && !e.named))?;
            let hit = &mut fields[at];
            hit.taken = true;
            Some((hit.channel, hit.text.clone(), hit.shown))
        });
        let Some((channel, text, shown)) = field.filter(|_| !self.label.is_empty()) else {
            return ui.add(if self.label.is_empty() { self.slider } else { self.slider.text(self.label) });
        };
        let tint = egui::Color32::from_rgb(120, 210, 140);
        let name = |ui: &mut egui::Ui, hint: &str| {
            let r = ui.add(egui::Label::new(egui::RichText::new(&self.label).color(tint)).sense(egui::Sense::click())).on_hover_text(hint);
            if r.clicked() {
                EXPRESSION_EDITS.with(|e| e.borrow_mut().push(ExpressionEdit::Toggle(channel)));
            }
            r
        };
        ui.horizontal(|ui| {
            if shown {
                // What's being typed, kept while the field has focus.
                let key = ui.make_persistent_id(("expression_edit", channel));
                let mut edited = ui.data_mut(|d| d.get_temp::<String>(key)).unwrap_or_else(|| text.clone());
                let width = ui.spacing().slider_width + ui.spacing().interact_size.x + ui.spacing().item_spacing.x;
                let field = ui.add(egui::TextEdit::singleline(&mut edited).id(key.with("field")).font(egui::TextStyle::Monospace).text_color(tint).desired_width(width));
                if field.lost_focus() {
                    if edited != text {
                        EXPRESSION_EDITS.with(|e| e.borrow_mut().push(ExpressionEdit::Set(channel, edited)));
                    }
                    ui.data_mut(|d| d.remove::<String>(key));
                } else if field.has_focus() {
                    ui.data_mut(|d| d.insert_temp(key, edited));
                } else {
                    ui.data_mut(|d| d.remove::<String>(key));
                }
                name(ui, "Its expression: click the name to show its value instead. A plain number replaces the expression.")
            } else {
                let slider = ui.add(self.slider);
                name(ui, &format!("Follows the expression {text}: click the name to show it"));
                slider
            }
        })
        .inner
    }
}

// A parameter field (pdrag/pslider) with its right-click menu.
pub(crate) fn pfield(ui: &mut egui::Ui, field: impl egui::Widget) -> egui::Response {
    let value = FIELD_VALUE.with(|v| v.get()) as f32;
    let response = ui.add(field);
    response.context_menu(|ui| {
        if ui.button("Delete Channel").on_hover_text("Remove the parameter's expression and keyframes; it keeps its current value").clicked() {
            TYPED.with(|t| *t.borrow_mut() = Some(Typed::Delete(value)));
            ui.close();
        }
    });
    response
}

pub(crate) fn run_logged(cmd: &mut Command, log_path: &std::path::Path, cancel: &AtomicBool) -> Result<std::process::ExitStatus, String> {
    let log = File::create(log_path).map_err(|e| format!("Could not create render log: {e}"))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    // Standard input stays open while rendering: closing it asks the Windows helper to
    // stop its container before it's killed.
    let mut child = cmd.stdin(std::process::Stdio::piped()).stdout(log).stderr(log_err).spawn().map_err(|e| e.to_string())?;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status);
        }
        if cancel.load(Ordering::Relaxed) {
            // The Windows helper stops its container when its input closes, given a moment;
            // MoonRay itself is stopped at once, mid-frame.
            drop(child.stdin.take());
            if cfg!(windows) {
                for _ in 0..50 {
                    if child.try_wait().ok().flatten().is_some() {
                        return Err("Render cancelled".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err("Render cancelled".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// Bucket rectangles (x0, y0, x1, y1; exclusive, top-down) ordered from the centre outward.
pub(crate) fn bucket_layout(width: usize, height: usize) -> Vec<[usize; 4]> {
    let tile = (((width.max(height) as f32 / 6.0).ceil() as usize).div_ceil(16) * 16).clamp(32, 256);
    let mut buckets = Vec::new();
    for y in (0..height).step_by(tile) {
        for x in (0..width).step_by(tile) {
            buckets.push([x, y, (x + tile).min(width), (y + tile).min(height)]);
        }
    }
    let centre = (width as f32 * 0.5, height as f32 * 0.5);
    let distance = |b: &[usize; 4]| {
        let (cx, cy) = ((b[0] + b[2]) as f32 * 0.5, (b[1] + b[3]) as f32 * 0.5);
        (cx - centre.0).powi(2) + (cy - centre.1).powi(2)
    };
    buckets.sort_by(|a, b| distance(a).partial_cmp(&distance(b)).unwrap_or(std::cmp::Ordering::Equal));
    buckets
}

// Renders the frame bucket by bucket with MoonRay's -sub_viewport, two processes at a time,
// streaming finished buckets into `progress`, then writes the assembled image to `output`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_buckets(
    make_cmd: &(dyn Fn(&[String]) -> Command + Sync),
    args: &[String],
    output: &str,
    width: usize,
    height: usize,
    log_path: &std::path::Path,
    cancel: &AtomicBool,
    progress: &render_viewer::SharedProgress,
) -> Result<(), String> {
    let buckets = bucket_layout(width, height);
    if let Ok(mut state) = progress.lock() {
        state.total = buckets.len();
    }
    let workers = buckets.len().clamp(1, 2);
    let threads = (std::thread::available_parallelism().map_or(4, |n| n.get()) / workers).max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failure: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let _ = std::fs::write(log_path, "");
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) || failure.lock().map_or(true, |f| f.is_some()) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(&bucket) = buckets.get(index) else { break };
                let [x0, y0, x1, y1] = bucket;
                if let Ok(mut state) = progress.lock() {
                    state.active.push(bucket);
                    state.version += 1;
                }
                let tmp = std::env::temp_dir();
                let bucket_out = tmp.join(format!("proceduralfx_bucket_{stamp}_{index}.exr"));
                let bucket_log = tmp.join(format!("proceduralfx_bucket_{stamp}_{index}.log"));
                let mut bucket_args: Vec<String> = args.to_vec();
                match bucket_args.iter().position(|a| a == "-out") {
                    Some(at) if at + 1 < bucket_args.len() => bucket_args[at + 1] = bucket_out.to_string_lossy().into_owned(),
                    _ => bucket_args.extend(["-out".to_string(), bucket_out.to_string_lossy().into_owned()]),
                }
                // MoonRay's region is left/bottom inclusive, right/top exclusive, rows counted from the bottom.
                bucket_args.extend(
                    ["-sub_viewport".to_string(), x0.to_string(), (height - y1).to_string(), x1.to_string(), (height - y0).to_string()],
                );
                bucket_args.extend(["-threads".to_string(), threads.to_string()]);
                let status = run_logged(&mut make_cmd(&bucket_args), &bucket_log, cancel);
                if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
                    let _ = writeln!(log, "--- bucket {} of {} ({x0},{y0})-({x1},{y1}) ---", index + 1, buckets.len());
                    let _ = log.write_all(&std::fs::read(&bucket_log).unwrap_or_default());
                }
                let result = match status {
                    Ok(s) if s.success() => image::open(&bucket_out).map(|img| img.to_rgba32f()).map_err(|e| format!("could not read bucket: {e}")),
                    Ok(s) => Err(format!("MoonRay exited with {s}: {}", moonray_message(&read_log_tail(&bucket_log, 600)).trim())),
                    Err(e) => Err(e),
                };
                let _ = std::fs::remove_file(&bucket_out);
                let _ = std::fs::remove_file(&bucket_log);
                if let Ok(mut state) = progress.lock() {
                    if let Ok(img) = &result {
                        for y in y0..y1.min(img.height() as usize) {
                            for x in x0..x1.min(img.width() as usize) {
                                state.pixels[y * width + x] = img.get_pixel(x as u32, y as u32).0;
                            }
                        }
                    }
                    state.active.retain(|b| *b != bucket);
                    state.done += 1;
                    state.version += 1;
                }
                if let Err(e) = result {
                    if let Ok(mut f) = failure.lock() {
                        f.get_or_insert(e);
                    }
                }
            });
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return Err("Render cancelled".into());
    }
    if let Some(error) = failure.lock().ok().and_then(|mut f| f.take()) {
        return Err(error);
    }
    let pixels = progress.lock().map_err(|_| "render state poisoned".to_string())?.pixels.clone();
    write_render_image(&pixels, width, height, output)
}

// Writes linear RGBA pixels: EXR as floats, other formats sRGB-encoded 8-bit.
pub(crate) fn write_render_image(pixels: &[[f32; 4]], width: usize, height: usize, output: &str) -> Result<(), String> {
    let (w, h) = (width as u32, height as u32);
    if fileio::extension(output) == "exr" {
        let raw: Vec<f32> = pixels.iter().flatten().copied().collect();
        let buffer = image::Rgba32FImage::from_raw(w, h, raw).ok_or("image size mismatch")?;
        return image::DynamicImage::ImageRgba32F(buffer).save(output).map_err(|e| e.to_string());
    }
    let encode = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0 + 0.5) as u8
    };
    let raw: Vec<u8> = pixels.iter().flat_map(|p| [encode(p[0]), encode(p[1]), encode(p[2]), (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8]).collect();
    let buffer = image::RgbaImage::from_raw(w, h, raw).ok_or("image size mismatch")?;
    image::DynamicImage::ImageRgba8(buffer).to_rgb8().save(output).map_err(|e| e.to_string())
}

// Last `max` bytes of a render log.
pub(crate) fn read_log_tail(path: &std::path::Path, max: usize) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

// What the render worker thread reports back: a status message and the finished image, plus its log.
pub(crate) struct RenderOutcome {
    pub(crate) result: Result<(String, PathBuf), String>,
    pub(crate) log: String,
}

// (name, samples per pixel, max depth, light samples, BSDF samples)
pub(crate) const QUALITY_PRESETS: [(&str, usize, usize, usize, usize); 4] = [
    ("Draft", 4, 2, 1, 1),
    ("Preview", 16, 4, 1, 1),
    ("Production", 64, 5, 2, 2),
    ("Final", 256, 8, 4, 4),
];

pub(crate) const RESOLUTION_PRESETS: [(&str, usize, usize); 7] = [
    ("HD 720p", 1280, 720),
    ("Full HD 1080p", 1920, 1080),
    ("2K DCI", 2048, 1080),
    ("UHD 4K", 3840, 2160),
    ("Square 1K", 1024, 1024),
    ("Square 2K", 2048, 2048),
    ("Portrait 4:5", 1080, 1350),
];

// How a render part is shaded: a MAT network node, a USD preview surface, or the default.
#[derive(Clone, Debug)]
pub(crate) enum PartMaterial {
    Default,
    Node(NodeId),
    Preview { diffuse: [f32; 3], roughness: f32, metallic: f32, opacity: f32, emissive: [f32; 3], ior: f32 },
}

// Everything a MoonRay render needs, from the OBJ objects or from the USD stage.
pub(crate) struct RenderScene {
    pub(crate) parts: Vec<(Mesh, PartMaterial)>,
    // (position, travel direction, color, intensity, kind) as scene_lights().
    pub(crate) lights: Vec<(Vec3, Vec3, [f32; 3], f32, usize)>,
    // Dome light colour, added to the environment.
    pub(crate) environment: Option<[f32; 3]>,
    // Stage camera in app space: eye, right, up, forward, focal length and horizontal aperture (mm).
    pub(crate) camera: Option<(Vec3, Vec3, Vec3, Vec3, f32, f32)>,
    // Fog volumes written to .vdb files for MoonRay.
    pub(crate) volumes: Vec<RenderVolume>,
}

#[derive(Clone, Debug)]
pub(crate) struct RenderVolume {
    pub(crate) file: String,
    // Extinction per world unit for density 1, albedo, and emission scale (None: no emission grid).
    pub(crate) extinction: f32,
    pub(crate) albedo: [f32; 3],
    pub(crate) emission: Option<f32>,
    // Pyro Shader settings: emission colour multiplier and scattering anisotropy.
    pub(crate) emission_tint: [f32; 3],
    pub(crate) anisotropy: f32,
}

// A TOP network cook in progress: items still to run (last first), and the one running.
pub(crate) struct TopRun {
    pub(crate) queue: Vec<(NodeId, usize)>,
    pub(crate) items: HashMap<NodeId, Vec<tops::WorkItem>>,
    pub(crate) current: Option<TopJob>,
    pub(crate) total: usize,
    pub(crate) failed: usize,
}

pub(crate) struct TopJob {
    pub(crate) node: NodeId,
    pub(crate) index: usize,
    pub(crate) output: String,
    pub(crate) wait: TopWait,
}

pub(crate) enum TopWait {
    Render,
    Shell(Receiver<Result<String, String>>),
}

pub(crate) struct SequenceJob {
    pub(crate) executable: String,
    pub(crate) arguments: String,
    pub(crate) output: String,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) samples: usize,
    pub(crate) opts: RenderOpts,
    pub(crate) times: Vec<f32>,
    pub(crate) next: usize,
}

pub(crate) const MATERIAL_NAMES: [&str; 4] = ["diffuse", "metal", "glass", "emissive"];
// USD gprim types made by the Primitive LOP, and light types made by the Light LOP.
pub(crate) const LOP_SHAPES: [&str; 5] = ["Sphere", "Cube", "Cylinder", "Cone", "Capsule"];
pub(crate) const LOP_LIGHTS: [&str; 4] = ["DistantLight", "SphereLight", "DomeLight", "RectLight"];
// Key of the STAGE network's display flag in Graph::displays (top-level LOPs have no container).
pub(crate) const STAGE_NETWORK: NodeId = usize::MAX;

#[derive(Clone, Debug)]
pub(crate) enum NodeKind {
    // Points: a pattern of `count` points (0: none) plus explicit positions, added to the
    // optional input; `delete_geometry` drops the input's primitives but keeps its points.
    Add { count: usize, spacing: f32, layout: usize, points: Vec<[f32; 3]>, delete_geometry: bool },
    Grid { size: f32, res: usize },
    Plane { size: f32, res: usize },
    Cone { radius: f32, height: f32, rows: usize, cols: usize, caps: bool },
    Capsule { radius: f32, length: f32, rows: usize, cols: usize },
    Box { size: f32, div: usize },
    Sphere { radius: f32, rows: usize, cols: usize },
    Torus { major: f32, minor: f32, rows: usize, cols: usize },
    // Scale and rotate about the pivot p, then translate by t.
    Transform { t: [f32; 3], r: [f32; 3], s: [f32; 3], p: [f32; 3] },
    Noise { freq: f32, amp: f32 },
    Taper { factor: f32 },
    Twist { angle: f32 },
    Bend { angle: f32 },
    Smooth { iterations: usize, strength: f32 },
    Bevel { amount: f32, segments: usize },
    Fuse { tol: f32 },
    Subdivide { levels: usize },
    // Even triangles of one size: adds or removes triangles (see remesh.rs).
    Remesh(remesh::RemeshSettings),
    Mirror { axis: usize },
    Copy { count: usize, t: [f32; 3], r: [f32; 3], s: f32 },
    // Optional piece attribute: each point copies the piece with its value of it.
    CopyToPoints { scale: f32, rotation: [f32; 3], piece_attribute: String },
    AttributeCreate(attrib::CreateSettings),
    AttributeRandomize(randomize::RandomizeSettings),
    AttributeDelete(attrib::DeleteSettings),
    Wrangle { code: String, domain: usize },
    // VEX over a volume's voxels (@density / @surface) or a terrain's points (@height, @mask).
    VolumeWrangle { code: String },
    HeightfieldWrangle { code: String },
    // An L-system drawn as curves.
    LSystem(lsystem::LSystemSettings),
    // Ready-made shapes to start from (built-in models, Platonic solids, a torus knot).
    TestGeometry(testgeo::TestGeoSettings),
    Merge,
    Null,
    Tube { r_bot: f32, r_top: f32, height: f32, rows: usize, cols: usize, caps: bool },
    Peak { dist: f32 },
    Ripple { freq: f32, amp: f32, speed: f32 },
    // Point Jitter: random offsets, scaled per axis.
    Jitter { amp: f32, seed: usize, scale: [f32; 3] },
    // Point particles. Input 1: optional source geometry (emits from its
    // surface or points; the origin when unconnected), input 2: optional static collider.
    Particles {
        count: usize,
        emission_rate: f32,
        lifetime: f32,
        speed: f32,
        spread: f32,
        gravity: f32,
        seed: usize,
        drag: f32,
        bounce: f32,
        friction: f32,
        radius: f32,
        substeps: usize,
        along_normals: bool,
    },
    Flip {
        particle_count: usize,
        grid_resolution: usize,
        flip_ratio: f32,
        viscosity: f32,
        gravity: f32,
        max_dt: f32,
        substeps: usize,
        seed: usize,
        // Domain box: centre xyz, size xyz (zero size: the emitter's bounds).
        domain: [f32; 6],
    },
    // Spray, foam and bubbles from a FLIP fluid. Input 1: fluid particles (with v), input 2: optional collider.
    WhitewaterSolver(whitewater::WhitewaterSettings),
    ParticleSurface { radius: f32, join_distance: f32, resolution: usize, smoothing: usize },
    Extrude(extrude::ExtrudeSettings),
    Clip { axis: usize, offset: f32, invert: bool },
    Blast { component: usize, indices: Vec<usize>, invert: bool },
    // A frame cache; `range` (start, end, step frames) is what Fetch writes on its own.
    Cache { cache_id: usize, mode: usize, range: [f32; 3] },
    // VDB from Polygons. output: 0 polygons (the surface directly), 1 VDB (a level set volume).
    Vdb { voxel_size: f32, padding: usize, operation: usize, iterations: usize, output: usize },
    // Converts a VDB or volume (VDB, Pyro, .vdb files). mode: 0 polygons at `iso`, 1 fog volume.
    ConvertVdb { mode: usize, iso: f32 },
    // Reads geometry from disk: .obj, .fbx, .glb/.gltf, .stl, .usd/.usda/.usdc/.usdz and .vdb; $F is the frame.
    // A file to read; for FBX, the take to play (by name, empty: the first) and whether it loops.
    FileImport { file: String, take: String, looping: bool },
    // Passes its input through and writes it to disk on demand (same formats; $F for sequences).
    FileExport { file: String },
    MoonRayRender {
        executable: String,
        arguments: String,
        output: String,
        width: usize,
        height: usize,
        samples: usize,
        opts: RenderOpts,
    },
    Camera {
        position: [f32; 3],
        rotation: [f32; 3],
        focal: f32,
        near: f32,
        orthographic: bool,
        pan: [f32; 2],
        zoom: f32,
    },
    Light {
        position: [f32; 3],
        rotation: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        kind: usize,
    },
    Reverse,
    Switch { index: usize },
    // Smoke and fire on a voxel grid. Input 1: source geometry, input 2: optional static collider.
    Pyro(sims::PyroSettings),
    // Seamless loops of a simulation, from `start` for `length` seconds (see looping.rs):
    // particles exactly, anything else by blending its next loop into the start over `blend`.
    LoopParticles { start: f32, length: f32 },
    // Input 1 at another time: `offset` seconds later, or held at `at` seconds.
    TimeShift { offset: f32, hold: bool, at: f32 },
    // Input 1 retimed: time runs `speed` times as fast from `anchor` (where it's unchanged).
    TimeWarp { speed: f32, anchor: f32 },
    // Input 1 between whole frames: the frames either side mixed, for smooth slow motion.
    TimeBlend,
    // Input 1 over its last `samples` moments, `step` frames apart: copies, curves or velocity.
    Trail { samples: usize, step: f32, mode: usize, fade: bool },
    MakeLoop { start: f32, length: f32, blend: f32 },
    // Groom. Hair Guides: input 1 is the skin mesh.
    HairGuides(groom::GuideSettings),
    // Shapes guide (or hair) curves from input 1.
    GuideProcess(groom::ProcessSettings),
    // Dense hair. Input 1: skin, input 2: guides.
    HairGenerate(groom::HairSettings),
    // Moves hair with an animated skin. Input 1: hair or guides, input 2: skin.
    HairDeform(groom::DeformSettings),
    // Rigid bodies (rapier, Bullet-style). Input 1: pieces, input 2: optional static collider.
    RbdSolver(sims::RbdSettings),
    // XPBD cloth, hair, softbody or grains. Input 1: geometry, input 2: optional static collider.
    VellumSolver(vellum::VellumSettings),
    // Writes N on points or vertices.
    Normal(sops::NormalSettings),
    // Points on (or inside) the input's surface.
    Scatter(sops::ScatterSettings),
    // Input 1 follows the motion of points: rest (input 2) to deformed (input 3).
    PointDeform(deform::PointDeformSettings),
    // Moves each piece of input 1 rigidly with its template point (input 2), optionally from rest (input 3).
    TransformPieces,
    // Area, perimeter, curvature, gradient, volume or curve length as attributes.
    Measure(measure::MeasureSettings),
    ExtractCentroid(sopkit::CentroidSettings),
    // Input 2 (optional): the reference whose bounds are the target.
    MatchSize(sopkit::MatchSizeSettings),
    ConvertLine,
    Metaball(sopkit::MetaballSettings),
    Connectivity(sopkit::ConnectivitySettings),
    Resample(sopkit::ResampleSettings),
    // A curve through (or near) points you place: polyline, smooth or B-spline, open or closed.
    Curve(curve::CurveSettings),
    // A circle, ellipse, arc or slice: one polygon or a curve (see circle.rs).
    Circle(circle::CircleSettings),
    // Reshapes an attribute's values through a ramp, from an input range to an output range.
    AttributeRemap(remap::RemapSettings),
    // Scales a point attribute in, holds it and scales it out over time (see fade.rs).
    // Input 2: optional start frames; input 3: optional hold scales.
    AttributeFade(fade::FadeSettings),
    // Flow control (see flow.rs): For-Each and Compile blocks are Begin/End pairs.
    ForEachBegin,
    ForEachEnd(flow::ForEachSettings),
    CompileBegin,
    CompileEnd { begin: Option<NodeId> },
    // Geometry of other objects (or nodes in other networks), optionally moved into this object's space.
    ObjectMerge { sources: Vec<Option<NodeId>>, into_this: bool },
    // Assigns MAT materials to primitive groups, overriding the object's material (see PRIM_MATERIAL).
    MaterialAssign { slots: Vec<(String, Option<NodeId>)> },
    // Union / intersection / subtraction of two closed meshes.
    Boolean(boolean::BooleanSettings),
    // Rigid pieces with breakable glue. Inputs: geometry, constraints, proxy, collision; outputs the first three.
    RbdBullet(bullet::BulletSettings),
    // Per-piece density, friction, bounce, active and velocity for the RBD Bullet Solver.
    RbdConfigure(bullet::ConfigureSettings),
    // Packs pieces into one point each (geometry stored once); Unpack turns them back.
    Pack(packed::PackSettings),
    Unpack { transfer: bool },
    // Breaks a closed mesh into concrete, glass or wood pieces (with `piece` for RBD).
    MaterialFracture(fracture::FractureSettings),
    // Agents walking with steering. Inputs: start points, optional ground, optional obstacles.
    CrowdSolver(crowd::CrowdSettings),
    // A muscle body from origin to insertion (or along input curve), with animatable activation.
    Muscle(muscle::MuscleSettings),
    // Jiggle (secondary motion) on animated muscles; tendon tips stay pinned.
    MuscleSolver(muscle::JiggleSettings),
    // Moves skin (input 1) with the muscles under it (input 2) relative to frame 0.
    MuscleSkin(muscle::SkinSettings),
    // KineFX: skeletons as points with parent and transform (see kinefx.rs).
    Skeleton(kinefx::SkeletonSettings),
    RigPose(kinefx::RigPose),
    TwoBoneIk(kinefx::IkSettings),
    ProceduralMotion(kinefx::MotionSettings),
    SkeletonBlend { weight: f32 },
    // Input 1: mesh, input 2: skeleton in the capture (rest) pose.
    JointCapture(kinefx::CaptureSettings),
    // Input 1: captured mesh, input 2: rest skeleton, input 3: animated skeleton.
    BoneDeform,
    // Heightfield terrain: a grid plus nodes that shape its heights.
    Terrain(terrain::TerrainSettings),
    TerrainNoise(terrain::NoiseSettings),
    TerrainErode(terrain::ErodeSettings),
    TerrainTerrace(terrain::TerraceSettings),
    TerrainMask(terrain::MaskSettings),
    TerrainColorize(terrain::ColorizeSettings),
    // UV projection, transform and viewport texture display.
    UvTexture(sops::UvTextureSettings),
    UvTransform(sops::UvTransformSettings),
    UvQuickShade { file: String },
    // Object-level HDRI environment light (MoonRay EnvLight); also lights and backs the viewport.
    EnvironmentLight { file: String, color: [f32; 3], intensity: f32, rotation: f32, visible: bool },
    // Makes a named point or primitive group.
    GroupCreate(groups::GroupCreateSettings),
    // Sets Cd on points or primitives: one colour, random colours or a ramp.
    Color(color::ColorSettings),
    // Numbers points or primitives (or pieces) in an attribute.
    Enumerate(enumerate::EnumerateSettings),
    // Evens out point attributes (or P) toward their neighbours' average.
    AttributeBlur(blur::BlurSettings),
    // Moves attributes between points, vertices, primitives and the detail.
    AttributePromote(promote::PromoteSettings),
    // Copies attributes from the nearest points or primitives of its second input.
    AttributeTransfer(transfer::TransferSettings),
    // Node-based VEX: a container of VOP nodes compiled to VEX and run over `domain` (like Wrangle).
    AttributeVop { domain: usize },
    // One operation inside an Attribute VOP network.
    Vop(vop::VopNode),
    // A float point attribute painted in the viewport (one stored value per input point).
    AttributePaint { attribute: String, values: Vec<f32>, settings: paint::PaintSettings },
    // Container whose network is cooked once per step on the previous step's output (Prev_Frame).
    SopSolver { substeps: usize },
    // A POP Network: a particle simulation built from POP nodes inside it (see pop.rs).
    // Inputs 0-3: geometry its POP Sources birth from, collide with, or its VEX reads.
    PopNet { substeps: usize },
    // A node inside a POP Network.
    Pop(pop::PopNode),
    // Inside a SOP Solver network: 0 = Prev_Frame, 1 = Input_1 (the solver's input at the step time).
    SolverInput { index: usize },
    // A network packaged as one node (an asset when saved to the library): its output is
    // its display node inside; `values` are its promoted parameters' values.
    Subnet { values: Vec<f32>, spec: assets::Spec },
    // Inside a Subnet: one of the Subnet's inputs (0-3).
    SubnetInput { index: usize },
    // STAGE (LOP) nodes: each builds on the USD stage from its first input.
    // Imports an OBJ geometry object (None: all of them) as Mesh prims under `path`.
    LopSopImport { geometry: Option<NodeId>, path: String },
    // References a USD file under `path`, or sublayers it at the root.
    LopReference { file: String, path: String, sublayer: bool },
    // A USD gprim; shape indexes LOP_SHAPES.
    LopPrimitive { shape: usize, path: String, size: f32, t: [f32; 3] },
    // Applies a transform to prims matching `pattern`, in their parent's space.
    LopTransform { pattern: String, t: [f32; 3], r: [f32; 3], s: [f32; 3] },
    // A UsdLux light aimed from `position` at `target`; kind indexes LOP_LIGHTS.
    LopLight { kind: usize, path: String, position: [f32; 3], target: [f32; 3], color: [f32; 3], intensity: f32, size: f32 },
    LopCamera { path: String, position: [f32; 3], target: [f32; 3], focal: f32, aperture: f32 },
    // Binds a MAT network material (as a UsdPreviewSurface) to prims matching `pattern`.
    LopMaterial { pattern: String, material: Option<NodeId> },
    // Deactivates (or only hides) prims matching `pattern`.
    LopPrune { pattern: String, hide: bool },
    LopMerge,
    // Writes its input stage to a USD file (format from the extension).
    LopExport { file: String },
    // OBJ-level container; its SOP network lives in nodes whose parent is this node.
    Geometry { material: Option<NodeId> },
    // MAT-level MoonRay material; kind indexes MATERIAL_NAMES. `value` is specular (diffuse), IOR (glass) or intensity (emissive).
    Material { kind: usize, color: [f32; 3], roughness: f32, value: f32 },
    // MAT shaders: Principled, Toon, Hair or Pyro (volume), with presets.
    Shader(shading::Values),
    // MAT container of shader nodes wired into a Surface Output.
    MaterialBuilder,
    // One node inside a Material Builder.
    ShaderNode(shading::ShaderNode),
    // IMG: a compositing network of COP nodes (see cops.rs).
    CopNetwork,
    Cop(cops::CopNode),
    // TASKS: a task graph of TOP nodes (see tops.rs).
    TopNetwork,
    Top(tops::TopNode),
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GroupSupport {
    None,
    // Cooks on everything, then restores what changed outside the group.
    Restrict,
    // The node uses the group itself (wrangle, blast, scatter, groom roots).
    Native,
    // Vellum: the group's points are pinned.
    Pin,
}

impl NodeKind {
    pub(crate) fn group_support(&self) -> GroupSupport {
        match self {
            NodeKind::Transform { .. }
            | NodeKind::Noise { .. }
            | NodeKind::Taper { .. }
            | NodeKind::Twist { .. }
            | NodeKind::Bend { .. }
            | NodeKind::Smooth { .. }
            | NodeKind::Peak { .. }
            | NodeKind::Ripple { .. }
            | NodeKind::Jitter { .. }
            | NodeKind::AttributeCreate(_)
            | NodeKind::AttributeRandomize(_)
            | NodeKind::Normal(_)
            | NodeKind::UvTexture(_)
            | NodeKind::UvTransform(_)
            | NodeKind::Color(_)
            | NodeKind::AttributeTransfer(_)
            | NodeKind::AttributePaint { .. }
            | NodeKind::GuideProcess(_)
            | NodeKind::HairDeform(_)
            | NodeKind::AttributeRemap(_)
            | NodeKind::AttributeFade(_) => GroupSupport::Restrict,
            NodeKind::Wrangle { .. }
            | NodeKind::AttributeVop { .. }
            | NodeKind::Blast { .. }
            | NodeKind::Scatter(_)
            | NodeKind::HairGuides(_)
            | NodeKind::HairGenerate(_)
            | NodeKind::RbdConfigure(_)
            | NodeKind::Extrude(_)
            | NodeKind::AttributeBlur(_)
            | NodeKind::HeightfieldWrangle { .. } => GroupSupport::Native,
            NodeKind::VellumSolver(_) => GroupSupport::Pin,
            // POPs act on the group's particles only.
            NodeKind::Pop(n) if !matches!(n.op, pop::OBJECT | pop::SOLVER | pop::SOURCE | pop::STATIC_OBJECT | pop::GROUND_PLANE) => GroupSupport::Native,
            _ => GroupSupport::None,
        }
    }

    pub(crate) fn is_object(&self) -> bool {
        matches!(self, NodeKind::Geometry { .. } | NodeKind::Camera { .. } | NodeKind::Light { .. } | NodeKind::EnvironmentLight { .. })
    }

    pub(crate) fn is_material(&self) -> bool {
        matches!(self, NodeKind::Material { .. } | NodeKind::Shader(_) | NodeKind::MaterialBuilder)
    }

    pub(crate) fn is_render(&self) -> bool {
        matches!(self, NodeKind::MoonRayRender { .. })
    }

    pub(crate) fn is_lop(&self) -> bool {
        matches!(
            self,
            NodeKind::LopSopImport { .. }
                | NodeKind::LopReference { .. }
                | NodeKind::LopPrimitive { .. }
                | NodeKind::LopTransform { .. }
                | NodeKind::LopLight { .. }
                | NodeKind::LopCamera { .. }
                | NodeKind::LopMaterial { .. }
                | NodeKind::LopPrune { .. }
                | NodeKind::LopMerge
                | NodeKind::LopExport { .. }
        )
    }

    // Parameters that can be keyframed, with their channel names.
    // Each parameter's name, in params() order (empty: not for keyframing, e.g. ids and painted values).
    pub(crate) fn param_names(&self) -> Vec<String> {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Table-driven nodes: an operation number, then each table parameter (colours as .r .g .b).
        let table = |params: &[shading::Param]| -> Vec<String> {
            params
                .iter()
                .flat_map(|p| match p.kind {
                    shading::Kind::Vector(..) => vec![format!("{}.x", p.key), format!("{}.y", p.key), format!("{}.z", p.key)],
                    _ if p.width() == 3 => vec![format!("{}.r", p.key), format!("{}.g", p.key), format!("{}.b", p.key)],
                    _ => vec![p.key.to_owned()],
                })
                .collect()
        };
        match self {
            NodeKind::Grid { .. } | NodeKind::Plane { .. } => names(&["size", "resolution"]),
            NodeKind::FileImport { .. } => names(&["loop"]),
            NodeKind::Cone { .. } => names(&["radius", "height", "rows", "columns", "caps"]),
            NodeKind::Capsule { .. } => names(&["radius", "length", "rows", "columns"]),
            NodeKind::Box { .. } => names(&["size", "divisions"]),
            NodeKind::Sphere { .. } => names(&["radius", "rows", "columns"]),
            NodeKind::Torus { .. } => names(&["major_radius", "minor_radius", "rows", "columns"]),
            NodeKind::Noise { .. } => names(&["frequency", "amplitude"]),
            NodeKind::Taper { .. } => names(&["taper"]),
            NodeKind::Twist { .. } => names(&["twist_angle"]),
            NodeKind::Bend { .. } => names(&["bend_angle"]),
            NodeKind::Smooth { .. } => names(&["iterations", "strength"]),
            NodeKind::Bevel { .. } => names(&["amount", "segments"]),
            NodeKind::Fuse { .. } => names(&["distance"]),
            NodeKind::Mirror { .. } => names(&["axis"]),
            NodeKind::Copy { .. } => names(&["copies", "t.x", "t.y", "t.z", "r.x", "r.y", "r.z", "uniform_scale"]),
            NodeKind::CopyToPoints { .. } => names(&["scale", "r.x", "r.y", "r.z"]),
            // The first number of each is an old setting (which attribute), kept for older versions.
            NodeKind::AttributeCreate(_) => names(&["", "value.x", "class", "type", "size", "value.y", "value.z", "value.w"]),
            NodeKind::AttributeRandomize(_) => names(&[
                "", "amount", "seed", "class", "size", "min.x", "max.x", "mode", "type", "distribution", "min.y", "min.z", "min.w", "max.y", "max.z", "max.w",
            ]),
            NodeKind::AttributeDelete(_) => names(&["", "class"]),
            NodeKind::AttributeVop { .. } => names(&["run_over"]),
            NodeKind::Tube { .. } => names(&["bottom_radius", "top_radius", "height", "rows", "columns", "caps"]),
            NodeKind::Peak { .. } => names(&["distance"]),
            NodeKind::Ripple { .. } => names(&["frequency", "amplitude", "speed"]),
            NodeKind::Jitter { .. } => names(&["amount", "seed", "scale.x", "scale.y", "scale.z"]),
            NodeKind::Particles { .. } => names(&["max_particles", "birth_rate", "lifetime", "speed", "spread", "gravity", "seed", "drag", "bounce", "friction", "radius", "substeps", "along_normals"]),
            NodeKind::Flip { .. } => names(&["particles", "grid_resolution", "flip_blend", "viscosity", "gravity", "max_timestep", "substeps", "seed", "domain_center.x", "domain_center.y", "domain_center.z", "domain_size.x", "domain_size.y", "domain_size.z"]),
            NodeKind::ParticleSurface { .. } => names(&["particle_radius", "join_distance", "resolution", "smoothing"]),
            NodeKind::Extrude(_) => names(&["distance", "inset", "connected", "divisions", "output_front", "output_back", "output_side", "output_groups"]),
            NodeKind::Clip { .. } => names(&["axis", "offset", "invert"]),
            NodeKind::Blast { .. } => names(&["component", "invert"]),
            NodeKind::Cache { .. } => names(&["", "mode", "start", "end", "step"]),
            NodeKind::Vdb { .. } => names(&["voxel_size", "padding", "operation", "iterations", "output"]),
            NodeKind::ConvertVdb { .. } => names(&["mode", "iso"]),
            NodeKind::Switch { .. } => names(&["input"]),
            NodeKind::Camera { .. } => names(&["position.x", "position.y", "position.z", "rotation.x", "rotation.y", "rotation.z", "focal", "near", "orthographic", "pan.x", "pan.y", "zoom"]),
            NodeKind::LoopParticles { .. } => names(&["start", "length"]),
            NodeKind::TimeShift { .. } => names(&["offset", "hold", "at"]),
            NodeKind::TimeWarp { .. } => names(&["speed", "anchor"]),
            NodeKind::Trail { .. } => names(&["samples", "step", "mode", "fade"]),
            NodeKind::MakeLoop { .. } => names(&["start", "length", "blend"]),
            NodeKind::Pyro(_) => names(&["resolution", "center.x", "center.y", "center.z", "size.x", "size.y", "size.z", "substeps", "density", "temperature", "fuel", "buoyancy", "dissipation", "cooling", "vorticity", "turbulence", "burn_rate", "wind.x", "wind.y", "wind.z", "opacity"]),
            NodeKind::RbdSolver(_) => names(&["shape", "density", "friction", "bounce", "gravity", "substeps", "ground", "ground_height", "velocity.x", "velocity.y", "velocity.z", "spin.x", "spin.y", "spin.z"]),
            NodeKind::HairGuides(_) => names(&["count", "length", "length_variance", "segments", "droop", "seed"]),
            NodeKind::GuideProcess(_) => names(&["operation", "amount", "frequency", "seed"]),
            NodeKind::HairDeform(_) => names(&["rest_frame"]),
            NodeKind::HairGenerate(_) => names(&["count", "segments", "clump", "length_variance", "frizz", "root_width", "tip_width", "seed"]),
            NodeKind::Normal(_) => names(&["domain", "cusp_angle", "weighting", "reverse"]),
            NodeKind::GroupCreate(_) => names(&["primitives", "method", "bbox_min.x", "bbox_min.y", "bbox_min.z", "bbox_max.x", "bbox_max.y", "bbox_max.z", "percent", "seed", "direction.x", "direction.y", "direction.z", "spread", "combine", "invert"]),
            NodeKind::AttributePaint { .. } => names(&["paint_value", "default_value", "brush_radius", "brush_strength", "mode", "visualize"]),
            NodeKind::Scatter(_) => names(&["count", "seed", "relax_iterations", "fill_volume"]),
            NodeKind::Boolean(_) => names(&["operation", "method", "voxel_resolution"]),
            NodeKind::Measure(_) => names(&["measure", "curvature", "summary"]),
            NodeKind::ExtractCentroid(_) => names(&["partition", "method"]),
            NodeKind::MatchSize(_) => names(&["justify.x", "justify.y", "justify.z", "scale_to_fit", "uniform", "center.x", "center.y", "center.z", "size.x", "size.y", "size.z"]),
            NodeKind::Metaball(_) => names(&["radius", "threshold", "resolution"]),
            NodeKind::Connectivity(_) => names(&["class"]),
            NodeKind::Resample(_) => names(&["by_length", "length", "segments"]),
            NodeKind::Curve(_) => names(&["type", "closed", "divisions"]),
            NodeKind::Circle(_) => names(&["type", "orientation", "radius.x", "radius.y", "center.x", "center.y", "center.z", "divisions", "arc_type", "arc_begin", "arc_end"]),
            NodeKind::AttributeRemap(_) => names(&["class", "auto_range", "input_min", "input_max", "output_min", "output_max"]),
            NodeKind::AttributeFade(_) => names(&["frame_offset", "fade_in", "hold", "fade_out", "seconds", "visualize"]),
            NodeKind::LSystem(_) => names(&["generations", "angle", "step", "step_scale", "thickness", "thickness_scale", "angle_random", "seed"]),
            NodeKind::TestGeometry(_) => names(&["shape", "size", "on_ground", "colours", "p", "q", "detail", "clip", "look", "looping"]),
            NodeKind::PointDeform(_) => names(&["influences", "fit_neighbours", "follow_rotation"]),
            NodeKind::ForEachEnd(_) => names(&["loop_over", "iterations", "start_value", "increment", "single_pass", "multithread"]),
            NodeKind::ObjectMerge { .. } => names(&["into_this_object"]),
            NodeKind::MaterialFracture(_) => names(&["material", "pieces", "seed", "impact.x", "impact.y", "impact.z", "crack_rings", "radial_cracks", "grain_axis", "splinter_length", "explode", "glue_strength"]),
            NodeKind::WhitewaterSolver(_) => names(&["max_particles", "emission_rate", "trapped_air", "wave_crests", "min_speed", "full_emission_speed", "foam_lifespan", "spray_drag", "bubble_buoyancy", "bubble_drag", "spray_neighbours", "bubble_neighbours", "gravity", "particle_size", "substeps", "seed"]),
            NodeKind::MuscleSolver(_) => names(&["firmness", "damping", "coherence", "substeps"]),
            NodeKind::MuscleSkin(_) => names(&["reach", "influences"]),
            NodeKind::Skeleton(_) => names(&["preset", "scale", "joints", "length"]),
            NodeKind::ProceduralMotion(_) => names(&["motion", "cycles_per_second", "amplitude", "forward_speed", "lag", "axis"]),
            NodeKind::JointCapture(_) => names(&["joints_per_point", "falloff", "colour_by_joint"]),
            NodeKind::Terrain(_) => names(&["size.x", "size.z", "resolution"]),
            NodeKind::TerrainNoise(_) => names(&["noise", "blend", "height", "element_size", "octaves", "roughness", "offset.x", "offset.z", "seed", "positive", "use_mask"]),
            NodeKind::TerrainErode(_) => names(&["raindrops", "erosion_rate", "deposition_rate", "sediment_capacity", "evaporation", "inertia", "erosion_radius", "thermal_iterations", "talus_angle", "seed"]),
            NodeKind::TerrainTerrace(_) => names(&["step_height", "offset", "sharpness", "use_mask"]),
            NodeKind::TerrainMask(_) => names(&["source", "min", "max", "falloff", "blur", "invert", "combine", "visualize"]),
            NodeKind::TerrainColorize(_) => names(&["sand.r", "sand.g", "sand.b", "grass.r", "grass.g", "grass.b", "rock.r", "rock.g", "rock.b", "snow.r", "snow.g", "snow.b", "sand_level", "snow_level", "rock_slope", "blend"]),
            NodeKind::UvTexture(_) => names(&["projection", "axis", "scale.u", "scale.v", "offset.u", "offset.v"]),
            NodeKind::AttributeTransfer(_) => names(&["class", "samples", "distance", "blend_width"]),
            NodeKind::Color(_) => names(&["class", "mode", "color.r", "color.g", "color.b", "color2.r", "color2.g", "color2.b", "seed", "axis", "auto_range", "range_min", "range_max"]),
            NodeKind::Enumerate(_) => names(&["class", "start", "increment"]),
            NodeKind::AttributeBlur(_) => names(&["mode", "weighting", "iterations", "step", "radius", "max_points", "pin_border"]),
            NodeKind::AttributePromote(_) => names(&["from", "to", "method", "delete_original"]),
            NodeKind::UvTransform(_) => names(&["translate.u", "translate.v", "rotate", "scale.u", "scale.v", "pivot.u", "pivot.v"]),
            NodeKind::VellumSolver(_) => names(&["type", "stretch_stiffness", "bend_stiffness", "shape_stiffness", "mass", "thickness", "gravity", "damping", "friction", "substeps", "iterations", "ground", "self_collision", "pin_to_animation"]),
            NodeKind::SopSolver { .. } => names(&["substeps"]),
            NodeKind::LopPrimitive { .. } => names(&["shape", "size", "t.x", "t.y", "t.z"]),
            NodeKind::LopTransform { .. } => names(&["t.x", "t.y", "t.z", "r.x", "r.y", "r.z", "s.x", "s.y", "s.z"]),
            NodeKind::LopLight { .. } => names(&["type", "position.x", "position.y", "position.z", "target.x", "target.y", "target.z", "color.r", "color.g", "color.b", "intensity", "size"]),
            NodeKind::LopCamera { .. } => names(&["position.x", "position.y", "position.z", "target.x", "target.y", "target.z", "focal", "aperture"]),
            NodeKind::LopPrune { .. } => names(&["hide"]),
            NodeKind::LopReference { .. } => names(&["sublayer"]),
            NodeKind::Subdivide { .. } => names(&["levels"]),
            NodeKind::Remesh(_) => names(&["target_triangles", "target_size", "iterations", "smoothing", "sharp_angle"]),
            NodeKind::SolverInput { .. } => names(&[""]),
            NodeKind::Subnet { spec, .. } => spec.params.iter().map(|p| p.label.clone()).collect(),
            NodeKind::SubnetInput { .. } => names(&["input"]),
            NodeKind::Add { .. } => names(&["point_count", "spacing", "layout", "delete_geometry"]),
            NodeKind::Transform { .. } => XFORM_NAMES.iter().chain(&["px", "py", "pz"]).map(|s| s.to_string()).collect(),
            NodeKind::RbdBullet(_) => names(&["density", "friction", "bounce", "gravity", "substeps", "ground", "ground_height", "glue_strength_scale", "velocity.x", "velocity.y", "velocity.z"]),
            NodeKind::RbdConfigure(_) => names(&["density", "friction", "bounce", "active", "velocity.x", "velocity.y", "velocity.z", "", "", "", "", ""]),
            NodeKind::Pack(_) => names(&["pack", "pivot"]),
            NodeKind::Unpack { .. } => names(&["transfer_attributes"]),
            NodeKind::CrowdSolver(_) => names(&["speed", "speed_variance", "turn_rate", "personal_space", "separation", "alignment", "cohesion", "goal.x", "goal.y", "goal.z", "goal_weight", "wander", "obstacle_lookahead", "agent_height", "display", "seed"]),
            NodeKind::Muscle(_) => names(&["origin.x", "origin.y", "origin.z", "insertion.x", "insertion.y", "insertion.z", "radius", "shape", "flatness", "tendon", "activation", "contraction", "segments", "sides", "twist", "muscle_id"]),
            NodeKind::TwoBoneIk(_) => names(&["goal.x", "goal.y", "goal.z", "pole.x", "pole.y", "pole.z", "use_pole", "blend", "keep_tip"]),
            NodeKind::SkeletonBlend { .. } => names(&["weight"]),
            NodeKind::Shader(v) => std::iter::once(String::new()).chain(table(v.table.params())).collect(),
            NodeKind::ShaderNode(n) if n.op == shading::SURFACE_OUTPUT => [String::new(), String::new()].into_iter().chain(table(shading::PRINCIPLED)).collect(),
            NodeKind::ShaderNode(n) => std::iter::once(String::new()).chain(table(n.def().params)).collect(),
            NodeKind::Cop(n) => std::iter::once(String::new()).chain(table(n.def().params)).collect(),
            NodeKind::Pop(n) => std::iter::once(String::new()).chain(table(n.def().params)).collect(),
            NodeKind::PopNet { .. } => names(&["substeps"]),
            NodeKind::Top(n) => std::iter::once(String::new()).chain(n.def().params.iter().map(|p| if matches!(p.key, "node" | "param") { String::new() } else { p.key.to_owned() })).collect(),
            NodeKind::Material { .. } => names(&["", "color.r", "color.g", "color.b", "roughness", "value"]),
            _ => Vec::new(),
        }
    }

    pub(crate) fn animatable(&self) -> Vec<(ChannelTarget, String)> {
        let named = |list: &[(usize, &str)]| list.iter().map(|(i, n)| (ChannelTarget::Param(*i), n.to_string())).collect();
        match self {
            NodeKind::Geometry { .. } => XFORM_NAMES.iter().enumerate().map(|(i, n)| (ChannelTarget::Xform(i), n.to_string())).collect(),
            NodeKind::Light { .. } => named(&[
                (0, "tx"), (1, "ty"), (2, "tz"), (3, "rx"), (4, "ry"), (5, "rz"),
                (6, "color.r"), (7, "color.g"), (8, "color.b"), (9, "intensity"),
            ]),
            NodeKind::EnvironmentLight { .. } => named(&[(0, "color.r"), (1, "color.g"), (2, "color.b"), (3, "intensity"), (4, "ry")]),
            NodeKind::Camera { .. } => named(&[
                (0, "tx"), (1, "ty"), (2, "tz"), (3, "rx"), (4, "ry"), (5, "rz"),
                (6, "focal"), (7, "near"), (9, "pan.x"), (10, "pan.y"), (11, "zoom"),
            ]),
            NodeKind::Transform { .. } => XFORM_NAMES.iter().chain(&["px", "py", "pz"]).enumerate().map(|(i, n)| (ChannelTarget::Param(i), n.to_string())).collect(),
            NodeKind::Material { .. } => named(&[(1, "color.r"), (2, "color.g"), (3, "color.b"), (4, "roughness"), (5, "value")]),
            NodeKind::Muscle(_) => named(&[
                (0, "origin.x"),
                (1, "origin.y"),
                (2, "origin.z"),
                (3, "insertion.x"),
                (4, "insertion.y"),
                (5, "insertion.z"),
                (6, "radius"),
                (10, "activation"),
            ]),
            NodeKind::CrowdSolver(_) => named(&[(0, "speed"), (7, "goal.x"), (8, "goal.y"), (9, "goal.z"), (10, "goal_weight")]),
            NodeKind::RigPose(_) => (0..kinefx::POSE_SLOTS)
                .flat_map(|slot| ["rx", "ry", "rz", "tx", "ty", "tz"].iter().enumerate().map(move |(k, n)| (ChannelTarget::Param(slot * 6 + k), format!("joint{}.{n}", slot + 1))))
                .collect(),
            NodeKind::TwoBoneIk(_) => named(&[(0, "goal.x"), (1, "goal.y"), (2, "goal.z"), (3, "pole.x"), (4, "pole.y"), (5, "pole.z"), (7, "blend")]),
            NodeKind::SkeletonBlend { .. } => named(&[(0, "weight")]),
            NodeKind::Wrangle { .. }
            | NodeKind::VolumeWrangle { .. }
            | NodeKind::HeightfieldWrangle { .. }
            | NodeKind::MoonRayRender { .. }
            | NodeKind::AttributeVop { .. }
            | NodeKind::Vop(_) => Vec::new(),
            // Every other node: its parameters by name (params() order); unnamed ones aren't animatable.
            other => other.param_names().into_iter().enumerate().filter(|(_, n)| !n.is_empty()).map(|(i, n)| (ChannelTarget::Param(i), n)).collect(),
        }
    }

    // Text parameters, saved hex-encoded next to the numeric ones.
    pub(crate) fn texts(&self) -> Vec<String> {
        match self {
            NodeKind::LopSopImport { path, .. }
            | NodeKind::LopPrimitive { path, .. }
            | NodeKind::LopLight { path, .. }
            | NodeKind::LopCamera { path, .. } => vec![path.clone()],
            NodeKind::LopReference { file, path, .. } => vec![file.clone(), path.clone()],
            NodeKind::LopTransform { pattern, .. } | NodeKind::LopMaterial { pattern, .. } | NodeKind::LopPrune { pattern, .. } => {
                vec![pattern.clone()]
            }
            NodeKind::LopExport { file } | NodeKind::FileExport { file } | NodeKind::UvQuickShade { file } => {
                vec![file.clone()]
            }
            NodeKind::FileImport { file, take, .. } => vec![file.clone(), take.clone()],
            NodeKind::AttributePaint { attribute, .. } => vec![attribute.clone()],
            NodeKind::VolumeWrangle { code } | NodeKind::HeightfieldWrangle { code } => vec![code.clone()],
            NodeKind::LSystem(l) => vec![l.premise.clone(), l.rules.clone()],
            NodeKind::MaterialAssign { slots } => slots.iter().map(|(g, _)| g.clone()).collect(),
            NodeKind::ForEachEnd(f) => vec![f.attribute.clone()],
            NodeKind::Measure(m) => vec![m.name.clone(), m.source.clone()],
            NodeKind::Connectivity(c) => vec![c.attribute.clone()],
            NodeKind::AttributeRandomize(r) => vec![r.name.clone()],
            NodeKind::AttributeCreate(s) => vec![s.name.clone()],
            NodeKind::AttributeDelete(s) => vec![s.pattern.clone()],
            NodeKind::Extrude(e) => vec![e.front_group.clone(), e.side_group.clone(), e.back_group.clone()],
            NodeKind::EnvironmentLight { file, .. } => vec![file.clone()],
            NodeKind::GroupCreate(g) => vec![g.name.clone(), g.expression.clone()],
            NodeKind::Color(s) => vec![s.random_by.clone(), s.ramp.clone(), s.ramp_attribute.clone()],
            NodeKind::CopyToPoints { piece_attribute, .. } => vec![piece_attribute.clone()],
            NodeKind::Enumerate(s) => vec![s.attribute.clone(), s.piece_attribute.clone()],
            NodeKind::AttributeBlur(s) => vec![s.pattern.clone()],
            NodeKind::AttributeRemap(s) => vec![s.attribute.clone(), s.ramp.clone()],
            NodeKind::AttributeFade(s) => s.texts(),
            NodeKind::AttributePromote(s) => vec![s.pattern.clone(), s.rename.clone()],
            NodeKind::AttributeTransfer(s) => vec![s.pattern.clone()],
            NodeKind::Vop(v) => vec![v.name.clone()],
            NodeKind::Shader(v) => v.textures.clone(),
            NodeKind::ShaderNode(n) => n.texts.clone(),
            NodeKind::Cop(n) => n.texts.clone(),
            NodeKind::Pop(n) => n.texts.clone(),
            NodeKind::Top(n) => n.texts.clone(),
            NodeKind::RigPose(p) => p.joints.clone(),
            NodeKind::TwoBoneIk(k) => vec![k.root.clone(), k.mid.clone(), k.tip.clone()],
            NodeKind::Subnet { spec, .. } => vec![spec.format()],
            _ => Vec::new(),
        }
    }

    pub(crate) fn set_texts(&mut self, texts: &[String]) {
        let mut texts = texts.iter().cloned();
        let mut next = |field: &mut String| {
            if let Some(text) = texts.next() {
                *field = text;
            }
        };
        match self {
            NodeKind::LopSopImport { path, .. }
            | NodeKind::LopPrimitive { path, .. }
            | NodeKind::LopLight { path, .. }
            | NodeKind::LopCamera { path, .. } => next(path),
            NodeKind::LopReference { file, path, .. } => {
                next(file);
                next(path);
            }
            NodeKind::LopTransform { pattern, .. } | NodeKind::LopMaterial { pattern, .. } | NodeKind::LopPrune { pattern, .. } => next(pattern),
            NodeKind::LopExport { file } | NodeKind::FileExport { file } | NodeKind::UvQuickShade { file } => {
                next(file)
            }
            NodeKind::FileImport { file, take, .. } => {
                next(file);
                next(take);
            }
            NodeKind::AttributePaint { attribute, .. } => next(attribute),
            NodeKind::VolumeWrangle { code } | NodeKind::HeightfieldWrangle { code } => next(code),
            NodeKind::LSystem(l) => {
                next(&mut l.premise);
                next(&mut l.rules);
            }
            NodeKind::MaterialAssign { slots } => slots.iter_mut().for_each(|(g, _)| next(g)),
            NodeKind::ForEachEnd(f) => next(&mut f.attribute),
            NodeKind::Measure(m) => {
                next(&mut m.name);
                next(&mut m.source);
            }
            NodeKind::AttributeRandomize(r) => next(&mut r.name),
            NodeKind::AttributeCreate(s) => next(&mut s.name),
            NodeKind::AttributeDelete(s) => next(&mut s.pattern),
            NodeKind::Connectivity(c) => next(&mut c.attribute),
            NodeKind::Extrude(e) => {
                next(&mut e.front_group);
                next(&mut e.side_group);
                next(&mut e.back_group);
            }
            NodeKind::EnvironmentLight { file, .. } => next(file),
            NodeKind::GroupCreate(g) => {
                next(&mut g.name);
                next(&mut g.expression);
            }
            NodeKind::Color(s) => {
                next(&mut s.random_by);
                next(&mut s.ramp);
                next(&mut s.ramp_attribute);
            }
            NodeKind::CopyToPoints { piece_attribute, .. } => next(piece_attribute),
            NodeKind::Enumerate(s) => {
                next(&mut s.attribute);
                next(&mut s.piece_attribute);
            }
            NodeKind::AttributeBlur(s) => next(&mut s.pattern),
            NodeKind::AttributeRemap(s) => {
                next(&mut s.attribute);
                next(&mut s.ramp);
            }
            NodeKind::AttributeFade(s) => {
                for field in [&mut s.attribute, &mut s.start_attribute, &mut s.hold_attribute, &mut s.in_ramp, &mut s.out_ramp] {
                    next(field);
                }
            }
            NodeKind::AttributePromote(s) => {
                next(&mut s.pattern);
                next(&mut s.rename);
            }
            NodeKind::AttributeTransfer(s) => next(&mut s.pattern),
            NodeKind::Vop(v) => next(&mut v.name),
            NodeKind::Shader(v) => v.textures.iter_mut().for_each(|t| next(t)),
            NodeKind::ShaderNode(n) => n.texts.iter_mut().for_each(|t| next(t)),
            NodeKind::Cop(n) => n.texts.iter_mut().for_each(|t| next(t)),
            NodeKind::Pop(n) => n.texts.iter_mut().for_each(|t| next(t)),
            NodeKind::Top(n) => n.texts.iter_mut().for_each(|t| next(t)),
            NodeKind::RigPose(p) => p.joints.iter_mut().for_each(|t| next(t)),
            NodeKind::TwoBoneIk(k) => {
                next(&mut k.root);
                next(&mut k.mid);
                next(&mut k.tip);
            }
            NodeKind::Subnet { values, spec } => {
                let mut text = String::new();
                next(&mut text);
                *spec = assets::Spec::parse(&text);
                values.resize(spec.params.len(), 0.0);
            }
            _ => {}
        }
    }

    // Nodes whose network lives in their children: Geometry objects and SOP Solvers.
    // Nodes with any number of inputs (a free port is always shown).
    pub(crate) fn is_multi_input(&self) -> bool {
        matches!(self, NodeKind::Merge | NodeKind::LopMerge | NodeKind::Switch { .. })
    }

    pub(crate) fn is_container(&self) -> bool {
        matches!(
            self,
            NodeKind::Geometry { .. } | NodeKind::SopSolver { .. } | NodeKind::AttributeVop { .. } | NodeKind::MaterialBuilder | NodeKind::CopNetwork | NodeKind::TopNetwork | NodeKind::PopNet { .. } | NodeKind::Subnet { .. }
        ) || matches!(self, NodeKind::Pop(n) if n.op == pop::VOP)
    }

    pub(crate) fn is_solver(&self) -> bool {
        matches!(
            self,
            NodeKind::Particles { .. }
                | NodeKind::Flip { .. }
                | NodeKind::WhitewaterSolver(_)
                | NodeKind::CrowdSolver(_)
                | NodeKind::MuscleSolver(_)
                | NodeKind::RbdBullet(_)
                | NodeKind::Pyro(_)
                | NodeKind::RbdSolver(_)
                | NodeKind::VellumSolver(_)
                | NodeKind::SopSolver { .. }
                | NodeKind::PopNet { .. }
        )
    }

    // Name in the add-node menus and TAB search ("Add", "Noise"... for VOPs).
    pub(crate) fn menu_name(&self) -> String {
        match self {
            NodeKind::Vop(v) => v.def().label.to_owned(),
            NodeKind::Shader(v) => v.name().to_owned(),
            NodeKind::ShaderNode(n) => n.def().label.to_owned(),
            NodeKind::Cop(n) => n.def().label.to_owned(),
            NodeKind::Top(n) => n.def().label.to_owned(),
            NodeKind::Pop(n) => n.def().label.to_owned(),
            NodeKind::MaterialFracture(f) => format!("material_fracture ({})", fracture::MATERIALS[f.material.min(2)]),
            NodeKind::ForEachEnd(f) => f.menu_name(),
            NodeKind::CompileEnd { .. } => "Compile Block".into(),
            NodeKind::Jitter { .. } => "point_jitter".into(),
            NodeKind::Subnet { spec: assets::Spec { asset: Some((name, _)), .. }, .. } => name.clone(),
            other => other.name().to_owned(),
        }
    }

    // A plain name for a node whose Houdini name means little to someone new to Houdini,
    // and that Houdini name: the Tab menu shows "Particle simulation (POP Network)", and
    // finds it by either.
    pub(crate) fn plain_name(&self) -> Option<(&'static str, &'static str)> {
        let houdini = match self {
            NodeKind::Pop(n) => n.def().label,
            other => other.name(),
        };
        let plain = match houdini {
            "popnet" => ("Particle simulation", "POP Network"),
            "pyro" => ("Fire and smoke simulation", "Pyro"),
            "flip" => ("Liquid simulation", "FLIP"),
            "cloth_solver" => ("Cloth simulation", "Vellum Cloth"),
            "rbd_solver" => ("Rigid body simulation", "RBD Solver"),
            "rbd_bullet_solver" => ("Breaking and bouncing pieces", "RBD Bullet Solver"),
            "whitewater_solver" => ("Foam and spray", "Whitewater"),
            "crowd_solver" => ("Crowd simulation", "Crowd Solver"),
            "sop_solver" => ("Frame-to-frame feedback", "SOP Solver"),
            "particles" => ("Simple particles", "Particles"),
            "particle_surface" => ("Mesh from particles", "Particle Fluid Surface"),
            "wrangle" => ("Code", "Attribute Wrangle, VEX"),
            "volume_wrangle" => ("Volume code", "Volume Wrangle, VEX"),
            "heightfield_wrangle" => ("Terrain code", "Heightfield Wrangle, VEX"),
            "attribute_vop" => ("Code from nodes", "Attribute VOP"),
            "vdb" => ("Volume from a mesh", "VDB from Polygons"),
            "convert_vdb" => ("Convert a volume", "Convert VDB"),
            "blast" => ("Delete parts", "Blast"),
            "fuse" => ("Join close points", "Fuse"),
            "copy_to_points" => ("Copy onto points", "Copy to Points"),
            "xform" => ("Move, rotate, scale", "Transform"),
            "object_merge" => ("Bring in another object", "Object Merge"),
            "pack" => ("Pack into pieces", "Pack"),
            "unpack" => ("Unpack pieces", "Unpack"),
            "attribute_promote" => ("Move an attribute between points and primitives", "Attribute Promote"),
            "attribute_transfer" => ("Copy attributes from nearby", "Attribute Transfer"),
            "connectivity" => ("Number the connected pieces", "Connectivity"),
            "enumerate" => ("Number the elements", "Enumerate"),
            "clip" => ("Cut with a plane", "Clip"),
            "peak" => ("Push along the normals", "Peak"),
            "metaball" => ("Blobs", "Metaball"),
            "lsystem" => ("Branching plants", "L-System"),
            "attribute_remap" => ("Reshape an attribute with a ramp", "Attribute Remap"),
            "attribute_fade" => ("Fade an attribute in and out over time", "Attribute Fade"),
            "POP Curve Force" => ("Flow along a curve", "POP Curve Force"),
            "POP Stream" => ("Act on some of the particles", "POP Stream"),
            "trail" => ("Motion trail", "Trail"),
            "time_shift" => ("Time offset", "Time Shift"),
            "cop_network" => ("Image network", "COP Network"),
            "top_network" => ("Task network", "TOP Network"),
            "material_builder" => ("Material from nodes", "Material Builder"),
            "moonray_render" => ("Render with MoonRay", "ROP"),
            "sop_import" => ("Geometry into USD", "SOP Import"),
            "POP Source" => ("Emit particles", "POP Source"),
            "POP Solver" => ("Move the particles", "POP Solver"),
            "POP Wrangle" => ("Particle code", "POP Wrangle, VEX"),
            "POP VOP" => ("Particle code from nodes", "POP VOP"),
            "POP Replicate" => ("Spawn particles from particles", "POP Replicate"),
            "POP Advect" => ("Carry particles along a flow", "POP Advect"),
            _ => return None,
        };
        Some(plain)
    }

    // What the Tab menu shows: the plain name with the Houdini name after it, if it has one.
    pub(crate) fn menu_display(&self) -> String {
        match self.plain_name() {
            Some((plain, houdini)) => format!("{plain} ({houdini})"),
            None => self.menu_name(),
        }
    }

    pub(crate) fn menu_help(&self) -> Option<&'static str> {
        match self {
            NodeKind::Vop(v) => Some(v.def().help),
            NodeKind::ShaderNode(n) => Some(n.def().help),
            NodeKind::Cop(n) => Some(n.def().help),
            NodeKind::Top(n) => Some(n.def().help),
            NodeKind::Pop(n) => Some(n.def().help),
            _ => None,
        }
    }

    // Title shown on the node in the network editor.
    pub(crate) fn label(&self, id: NodeId) -> String {
        match self {
            NodeKind::SolverInput { index: 0 } => format!("Prev_Frame{id}"),
            NodeKind::SolverInput { .. } => format!("Input_1·{id}"),
            NodeKind::SubnetInput { index } => format!("Input_{}·{id}", index + 1),
            NodeKind::Subnet { spec: assets::Spec { asset: Some((name, _)), .. }, .. } => format!("{}{id}", name.to_lowercase().replace(' ', "_")),
            NodeKind::Vop(v) => v.label(id),
            NodeKind::Shader(v) => format!("{}{id}", v.name().to_lowercase().replace(' ', "_")),
            NodeKind::ShaderNode(n) => n.label(id),
            NodeKind::Cop(n) => n.label(id),
            NodeKind::Top(n) => n.label(id),
            NodeKind::Pop(n) => n.label(id),
            _ => format!("{}{id}", self.name()),
        }
    }

    pub(crate) fn templates() -> Vec<(&'static str, Vec<NodeKind>)> {
        let mut all = vec![
            (
                "USD Create",
                vec![
                    NodeKind::LopSopImport { geometry: None, path: "/geo".into() },
                    NodeKind::LopReference { file: String::new(), path: "/ref".into(), sublayer: false },
                    NodeKind::LopPrimitive { shape: 0, path: "/shapes/sphere".into(), size: 1.0, t: [0.0, 1.0, 0.0] },
                    NodeKind::LopLight {
                        kind: 0,
                        path: "/lights/key".into(),
                        position: [4.0, 6.0, 4.0],
                        target: [0.0; 3],
                        color: [1.0, 0.97, 0.9],
                        intensity: 1.0,
                        size: 1.0,
                    },
                    NodeKind::LopCamera { path: "/cameras/cam".into(), position: [5.0, 4.0, 6.0], target: [0.0; 3], focal: 35.0, aperture: 36.0 },
                ],
            ),
            (
                "USD Edit",
                vec![
                    NodeKind::LopTransform { pattern: "/geo/*".into(), t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] },
                    NodeKind::LopMaterial { pattern: "/geo/**".into(), material: None },
                    NodeKind::LopPrune { pattern: String::new(), hide: false },
                    NodeKind::LopMerge,
                    NodeKind::LopExport { file: String::new() },
                ],
            ),
            (
                "Objects",
                vec![
                    NodeKind::Geometry { material: None },
                    NodeKind::Camera {
                        position: [0.0, 0.0, 0.0],
                        rotation: [23.0, -35.0, 0.0],
                        focal: 1.2,
                        near: 0.5,
                        orthographic: false,
                        pan: [0.0, 0.0],
                        zoom: 6.0,
                    },
                    NodeKind::Light {
                        position: [3.0, 4.0, 3.0],
                        rotation: [-35.0, 35.0, 0.0],
                        color: [1.0, 0.95, 0.85],
                        intensity: 0.9,
                        kind: 0,
                    },
                    NodeKind::EnvironmentLight { file: String::new(), color: [1.0; 3], intensity: 1.0, rotation: 0.0, visible: true },
                ],
            ),
            (
                "Generators",
                vec![
                    NodeKind::Add { count: 0, spacing: 1.0, layout: 0, points: vec![[0.0; 3]], delete_geometry: false },
                    NodeKind::Curve(curve::CurveSettings::defaults()),
                    NodeKind::Circle(circle::CircleSettings::defaults()),
                    NodeKind::Grid { size: 4.0, res: 20 },
                    NodeKind::Plane { size: 4.0, res: 20 },
                    NodeKind::Cone { radius: 1.0, height: 2.0, rows: 1, cols: 32, caps: true },
                    NodeKind::Capsule { radius: 0.6, length: 1.5, rows: 16, cols: 32 },
                    NodeKind::Box { size: 1.5, div: 4 },
                    NodeKind::Sphere { radius: 1.0, rows: 16, cols: 32 },
                    NodeKind::Torus { major: 1.0, minor: 0.35, rows: 16, cols: 32 },
                    NodeKind::Tube { r_bot: 0.5, r_top: 0.5, height: 2.0, rows: 8, cols: 24, caps: true },
                    NodeKind::LSystem(lsystem::LSystemSettings::defaults()),
                    NodeKind::TestGeometry(testgeo::TestGeoSettings::defaults()),
                    NodeKind::ParticleSurface {
                        radius: 0.05,
                        join_distance: 0.0,
                        resolution: 160,
                        smoothing: 1,
                    },
                ],
            ),
            (
                "Groom",
                vec![
                    NodeKind::HairGuides(groom::GuideSettings {
                        count: 200,
                        length: 0.6,
                        length_variance: 0.2,
                        segments: 8,
                        droop: 0.3,
                        seed: 1,
                    }),
                    NodeKind::GuideProcess(groom::ProcessSettings { op: 1, amount: 0.5, frequency: 2.0, seed: 1 }),
                    NodeKind::HairGenerate(groom::HairSettings {
                        count: 5000,
                        segments: 8,
                        clump: 0.3,
                        length_variance: 0.15,
                        frizz: 0.2,
                        width_root: 0.004,
                        width_tip: 0.001,
                        seed: 1,
                    }),
                    NodeKind::HairDeform(groom::DeformSettings { rest_frame: 1.0 }),
                    NodeKind::AttributePaint { attribute: "density".into(), values: Vec::new(), settings: paint::PaintSettings::default() },
                ],
            ),
            (
                "Solvers",
                vec![
                    NodeKind::PopNet { substeps: 1 },
                    NodeKind::Particles {
                        count: 2000,
                        emission_rate: 400.0,
                        lifetime: 4.0,
                        speed: 2.0,
                        spread: 0.35,
                        gravity: -1.5,
                        seed: 1,
                        drag: 0.0,
                        bounce: 0.4,
                        friction: 0.1,
                        radius: 0.02,
                        substeps: 2,
                        along_normals: false,
                    },
                    NodeKind::Flip {
                        particle_count: 20_000,
                        grid_resolution: 64,
                        flip_ratio: 0.95,
                        viscosity: 0.1,
                        gravity: -9.8,
                        max_dt: 0.02,
                        substeps: 2,
                        seed: 11,
                        domain: [0.0; 6],
                    },
                    NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings::defaults()),
                    NodeKind::Pyro(sims::PyroSettings::default()),
                    NodeKind::RbdSolver(sims::RbdSettings::default()),
                    NodeKind::VellumSolver(vellum::VellumSettings::defaults(0)),
                    NodeKind::SopSolver { substeps: 1 },
                    NodeKind::TimeShift { offset: 0.0, hold: false, at: 0.0 },
                    NodeKind::TimeWarp { speed: 0.5, anchor: 0.0 },
                    NodeKind::TimeBlend,
                    NodeKind::Trail { samples: 5, step: 1.0, mode: 0, fade: true },
                    NodeKind::LoopParticles { start: 1.0, length: 1.0 },
                    NodeKind::MakeLoop { start: 2.0, length: 1.0, blend: 0.5 },
                    NodeKind::SolverInput { index: 0 },
                    NodeKind::SolverInput { index: 1 },
                ],
            ),
            (
                "Import/Export",
                vec![
                    NodeKind::FileImport { file: String::new(), take: String::new(), looping: false },
                    NodeKind::FileExport { file: String::new() },
                    NodeKind::ObjectMerge { sources: vec![None], into_this: true },
                ],
            ),
            (
                "Volumes",
                vec![
                    NodeKind::Vdb { voxel_size: 0.1, padding: 0, operation: 0, iterations: 1, output: 1 },
                    NodeKind::ConvertVdb { mode: 0, iso: 0.0 },
                    NodeKind::VolumeWrangle { code: VOLUME_WRANGLE_EXAMPLE.into() },
                ],
            ),
            (
                "Deformers",
                vec![
                    NodeKind::Transform { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3], p: [0.0; 3] },
                    NodeKind::Noise { freq: 2.2, amp: 0.3 },
                    NodeKind::Taper { factor: 0.3 },
                    NodeKind::Twist { angle: 1.0 },
                    NodeKind::Bend { angle: 0.5 },
                    NodeKind::PointDeform(deform::PointDeformSettings::defaults()),
                    NodeKind::TransformPieces,
                    NodeKind::Smooth { iterations: 5, strength: 0.5 },
                    NodeKind::Bevel { amount: 0.25, segments: 2 },
                    NodeKind::Peak { dist: 0.1 },
                    NodeKind::Ripple { freq: 6.0, amp: 0.3, speed: 4.0 },
                    NodeKind::Jitter { amp: 0.05, seed: 1, scale: [1.0; 3] },
                    NodeKind::MatchSize(sopkit::MatchSizeSettings::defaults()),
                ],
            ),
            (
                "Topology",
                vec![
                    NodeKind::Subdivide { levels: 1 },
                    NodeKind::Remesh(remesh::RemeshSettings::defaults()),
                    NodeKind::Fuse { tol: 0.001 },
                    NodeKind::Mirror { axis: 0 },
                    NodeKind::Copy { count: 5, t: [1.5, 0.0, 0.0], r: [0.0; 3], s: 1.0 },
                    NodeKind::CopyToPoints { scale: 1.0, rotation: [0.0; 3], piece_attribute: String::new() },
                    NodeKind::Extrude(extrude::ExtrudeSettings::defaults()),
                    NodeKind::Clip { axis: 1, offset: 0.0, invert: false },
                    NodeKind::Blast { component: 1, indices: Vec::new(), invert: false },
                    NodeKind::Scatter(sops::ScatterSettings { count: 1000, seed: 1, relax_iterations: 5, volume: false }),
                    NodeKind::Boolean(boolean::BooleanSettings { operation: 2, mode: 0, resolution: 128 }),
                    NodeKind::Pack(packed::PackSettings::defaults()),
                    NodeKind::Unpack { transfer: true },
                    NodeKind::Cache { cache_id: 1, mode: 1, range: [1.0, 100.0, 1.0] },
                    NodeKind::Reverse,
                    NodeKind::ExtractCentroid(sopkit::CentroidSettings::defaults()),
                    NodeKind::ConvertLine,
                    NodeKind::Resample(sopkit::ResampleSettings::defaults()),
                    NodeKind::Metaball(sopkit::MetaballSettings::defaults()),
                    NodeKind::Switch { index: 0 },
                    NodeKind::Merge,
                    NodeKind::Null,
                ],
            ),
            (
                "Flow Control",
                (0..flow::MODES.len())
                    .map(|m| NodeKind::ForEachEnd(flow::ForEachSettings::new(m)))
                    .chain(std::iter::once(NodeKind::CompileEnd { begin: None }))
                    .chain([NodeKind::Subnet { values: Vec::new(), spec: assets::Spec::default() }, NodeKind::SubnetInput { index: 0 }])
                    .collect(),
            ),
            (
                "Shading",
                vec![NodeKind::MaterialAssign { slots: vec![(String::new(), None)] }],
            ),
            (
                "Destruction",
                (0..fracture::MATERIALS.len())
                    .map(|m| NodeKind::MaterialFracture(fracture::FractureSettings::defaults(m)))
                    .chain([NodeKind::RbdConfigure(bullet::ConfigureSettings::defaults()), NodeKind::RbdBullet(bullet::BulletSettings::defaults())])
                    .collect(),
            ),
            (
                "Crowds",
                vec![NodeKind::CrowdSolver(crowd::CrowdSettings::defaults())],
            ),
            (
                "Rigging",
                vec![
                    NodeKind::Skeleton(kinefx::SkeletonSettings::defaults()),
                    NodeKind::RigPose(kinefx::RigPose::new()),
                    NodeKind::TwoBoneIk(kinefx::IkSettings::defaults()),
                    NodeKind::ProceduralMotion(kinefx::MotionSettings::defaults()),
                    NodeKind::SkeletonBlend { weight: 0.5 },
                    NodeKind::JointCapture(kinefx::CaptureSettings::defaults()),
                    NodeKind::BoneDeform,
                ],
            ),
            (
                "Muscles",
                vec![
                    NodeKind::Muscle(muscle::MuscleSettings::defaults()),
                    NodeKind::MuscleSolver(muscle::JiggleSettings::defaults()),
                    NodeKind::MuscleSkin(muscle::SkinSettings::defaults()),
                ],
            ),
            (
                "Terrain",
                vec![
                    NodeKind::Terrain(terrain::TerrainSettings { size: [20.0, 20.0], resolution: 256 }),
                    NodeKind::TerrainNoise(terrain::NoiseSettings::defaults()),
                    NodeKind::TerrainErode(terrain::ErodeSettings::defaults()),
                    NodeKind::TerrainTerrace(terrain::TerraceSettings { step: 0.5, offset: 0.0, sharpness: 0.6, use_mask: false }),
                    NodeKind::TerrainMask(terrain::MaskSettings {
                        source: 1,
                        min: 30.0,
                        max: 90.0,
                        falloff: 5.0,
                        blur: 1,
                        invert: false,
                        combine: 0,
                        visualize: true,
                    }),
                    NodeKind::TerrainColorize(terrain::ColorizeSettings::defaults()),
                    NodeKind::HeightfieldWrangle { code: HEIGHTFIELD_WRANGLE_EXAMPLE.into() },
                ],
            ),
            (
                "Attributes",
                vec![
                    NodeKind::AttributeCreate(attrib::CreateSettings::defaults()),
                    NodeKind::AttributeRandomize(randomize::RandomizeSettings::defaults()),
                    NodeKind::AttributeDelete(attrib::DeleteSettings::defaults()),
                    NodeKind::Color(color::ColorSettings::defaults()),
                    NodeKind::AttributeTransfer(transfer::TransferSettings::defaults()),
                    NodeKind::Enumerate(enumerate::EnumerateSettings::defaults()),
                    NodeKind::AttributeBlur(blur::BlurSettings::defaults()),
                    NodeKind::AttributeRemap(remap::RemapSettings::defaults()),
                    NodeKind::AttributeFade(fade::FadeSettings::defaults()),
                    NodeKind::AttributePromote(promote::PromoteSettings::defaults()),
                    NodeKind::Wrangle { code: "@P.y += sin(@P.x * 3.0) * 0.2;".into(), domain: 2 },
                    NodeKind::AttributeVop { domain: 2 },
                    NodeKind::Normal(sops::NormalSettings { domain: 1, cusp_angle: 60.0, weighting: 0, reverse: false }),
                    NodeKind::Measure(measure::MeasureSettings::new(0)),
                    NodeKind::Connectivity(sopkit::ConnectivitySettings::defaults()),
                    NodeKind::AttributePaint { attribute: "mask".into(), values: Vec::new(), settings: paint::PaintSettings::default() },
                ],
            ),
            (
                "Groups",
                vec![NodeKind::GroupCreate(groups::GroupCreateSettings {
                    name: "group1".into(),
                    domain: groups::GroupDomain::Points,
                    method: 0,
                    bbox_min: [-1.0, -1.0, -1.0],
                    bbox_max: [1.0, 1.0, 1.0],
                    percent: 50.0,
                    seed: 1,
                    direction: [0.0, 1.0, 0.0],
                    spread: 30.0,
                    expression: "@P.y>0".into(),
                    combine: 0,
                    invert: false,
                })],
            ),
            (
                "UV",
                vec![
                    NodeKind::UvTexture(sops::UvTextureSettings { projection: 3, axis: 1, scale: [1.0, 1.0], offset: [0.0, 0.0] }),
                    NodeKind::UvTransform(sops::UvTransformSettings { translate: [0.0; 2], rotate: 0.0, scale: [1.0; 2], pivot: [0.5; 2] }),
                    NodeKind::UvQuickShade { file: String::new() },
                ],
            ),
            (
                "Materials",
                vec![
                    NodeKind::Material { kind: 0, color: [0.8, 0.8, 0.8], roughness: 0.5, value: 0.5 },
                    NodeKind::Material { kind: 1, color: [0.9, 0.7, 0.3], roughness: 0.3, value: 0.0 },
                    NodeKind::Material { kind: 2, color: [1.0, 1.0, 1.0], roughness: 0.02, value: 1.5 },
                    NodeKind::Material { kind: 3, color: [1.0, 0.9, 0.7], roughness: 0.0, value: 1.0 },
                ],
            ),
            (
                "Shaders",
                vec![
                    NodeKind::Shader(shading::Values::new(shading::Table::Principled)),
                    NodeKind::Shader(shading::Values::new(shading::Table::Toon)),
                    NodeKind::Shader(shading::Values::new(shading::Table::Hair)),
                    NodeKind::Shader(shading::Values::new(shading::Table::Pyro)),
                    NodeKind::MaterialBuilder,
                ],
            ),
            (
                "Render",
                vec![NodeKind::MoonRayRender {
                    executable: "moonray".into(),
                    arguments: "--scene {scene} --output {output}".into(),
                    output: "moonray_render.exr".into(),
                    width: 1280,
                    height: 720,
                    samples: 64,
                    opts: RenderOpts::default(),
                }],
            ),
        ];
        // Shader presets, one menu per kind of material.
        for (category, range) in shading::PRESET_CATEGORIES {
            all.push((category, range.clone().map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Principled, i))).collect()));
        }
        all.push(("Toon & Hair Presets", (1..shading::TOON_PRESETS.len()).map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Toon, i)))
            .chain((1..shading::HAIR_PRESETS.len()).map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Hair, i)))).collect()));
        all.push(("Volume Presets", (1..shading::PYRO_PRESETS.len()).map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Pyro, i))).collect()));
        // IMG and TASKS: the network nodes, and what goes inside them.
        all.push(("COP Network", vec![NodeKind::CopNetwork]));
        for category in cops::CATEGORIES {
            all.push((category, (0..cops::OPS.len()).filter(|&op| cops::OPS[op].category == category).map(|op| NodeKind::Cop(cops::CopNode::new(op))).collect()));
        }
        all.push(("TOP Network", vec![NodeKind::TopNetwork]));
        all.push((tops::CATEGORY, (0..tops::OPS.len()).map(|op| NodeKind::Top(tops::TopNode::new(op))).collect()));
        // POP nodes (only shown inside a POP Network, with Merge).
        all.push(("POP", (0..pop::OPS.len()).map(|op| NodeKind::Pop(pop::PopNode::new(op))).collect()));
        // Material Builder nodes (only shown inside a Material Builder).
        all.push((shading::CATEGORY, (0..shading::SHADER_OPS.len()).map(|op| NodeKind::ShaderNode(shading::ShaderNode::new(op))).collect()));
        // VOP networks: one menu per category (only shown inside an Attribute VOP).
        all.extend(vop::CATEGORIES.iter().map(|category| {
            (*category, (0..vop::OPS.len()).filter(|&op| vop::OPS[op].category == *category).map(|op| NodeKind::Vop(vop::VopNode::new(op))).collect())
        }));
        all
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            NodeKind::Add { .. } => "add",
            NodeKind::Grid { .. } => "grid",
            NodeKind::Plane { .. } => "plane",
            NodeKind::Cone { .. } => "cone",
            NodeKind::Capsule { .. } => "capsule",
            NodeKind::Box { .. } => "box",
            NodeKind::Sphere { .. } => "sphere",
            NodeKind::Torus { .. } => "torus",
            NodeKind::Transform { .. } => "xform",
            NodeKind::Noise { .. } => "noise",
            NodeKind::Taper { .. } => "taper",
            NodeKind::Twist { .. } => "twist",
            NodeKind::Bend { .. } => "bend",
            NodeKind::Smooth { .. } => "smooth",
            NodeKind::Bevel { .. } => "bevel",
            NodeKind::Fuse { .. } => "fuse",
            NodeKind::Subdivide { .. } => "subdivide",
            NodeKind::Remesh(_) => "remesh",
            NodeKind::Mirror { .. } => "mirror",
            NodeKind::Copy { .. } => "copy",
            NodeKind::CopyToPoints { .. } => "copy_to_points",
            NodeKind::AttributeCreate(_) => "attribute_create",
            NodeKind::AttributeRandomize(_) => "attribute_randomize",
            NodeKind::AttributeDelete(_) => "attribute_delete",
            NodeKind::Wrangle { .. } => "wrangle",
            NodeKind::AttributeVop { .. } => "attribute_vop",
            NodeKind::Vop(_) => "vop",
            NodeKind::Merge => "merge",
            NodeKind::Null => "null",
            NodeKind::Tube { .. } => "tube",
            NodeKind::Peak { .. } => "peak",
            NodeKind::Ripple { .. } => "ripple",
            NodeKind::Jitter { .. } => "jitter",
            NodeKind::Particles { .. } => "particles",
            NodeKind::Flip { .. } => "flip",
            NodeKind::WhitewaterSolver(_) => "whitewater_solver",
            NodeKind::ParticleSurface { .. } => "particle_surface",
            NodeKind::Extrude(_) => "extrude",
            NodeKind::Clip { .. } => "clip",
            NodeKind::Blast { .. } => "blast",
            NodeKind::Cache { .. } => "cache",
            NodeKind::Vdb { .. } => "vdb",
            NodeKind::ConvertVdb { .. } => "convert_vdb",
            NodeKind::FileImport { .. } => "file",
            NodeKind::FileExport { .. } => "file_export",
            NodeKind::MoonRayRender { .. } => "moonray_render",
            NodeKind::Camera { .. } => "camera",
            NodeKind::Light { .. } => "light",
            NodeKind::EnvironmentLight { .. } => "environment_light",
            NodeKind::Reverse => "reverse",
            NodeKind::Pyro(_) => "pyro",
            NodeKind::LoopParticles { .. } => "loop_particles",
            NodeKind::TimeShift { .. } => "time_shift",
            NodeKind::TimeWarp { .. } => "time_warp",
            NodeKind::TimeBlend => "time_blend",
            NodeKind::Trail { .. } => "trail",
            NodeKind::MakeLoop { .. } => "make_loop",
            NodeKind::HairGuides(_) => "hair_guides",
            NodeKind::GuideProcess(_) => "guide_process",
            NodeKind::HairGenerate(_) => "hair_generate",
            NodeKind::HairDeform(_) => "hair_deform",
            NodeKind::LopSopImport { .. } => "sop_import",
            NodeKind::LopReference { .. } => "usd_reference",
            NodeKind::LopPrimitive { .. } => "usd_primitive",
            NodeKind::LopTransform { .. } => "usd_transform",
            NodeKind::LopLight { .. } => "usd_light",
            NodeKind::LopCamera { .. } => "usd_camera",
            NodeKind::LopMaterial { .. } => "assign_material",
            NodeKind::LopPrune { .. } => "usd_prune",
            NodeKind::LopMerge => "usd_merge",
            NodeKind::LopExport { .. } => "usd_export",
            NodeKind::RbdSolver(_) => "rbd_solver",
            NodeKind::VellumSolver(_) => "cloth_solver",
            NodeKind::Normal(_) => "normal",
            NodeKind::Scatter(_) => "scatter",
            NodeKind::Boolean(_) => "boolean",
            NodeKind::MaterialAssign { .. } => "material",
            NodeKind::Measure(_) => "measure",
            NodeKind::ExtractCentroid(_) => "extract_centroid",
            NodeKind::MatchSize(_) => "match_size",
            NodeKind::ConvertLine => "convert_line",
            NodeKind::Metaball(_) => "metaball",
            NodeKind::Connectivity(_) => "connectivity",
            NodeKind::Resample(_) => "resample",
            NodeKind::Curve(_) => "curve",
            NodeKind::Circle(_) => "circle",
            NodeKind::AttributeRemap(_) => "attribute_remap",
            NodeKind::AttributeFade(_) => "attribute_fade",
            NodeKind::VolumeWrangle { .. } => "volume_wrangle",
            NodeKind::HeightfieldWrangle { .. } => "heightfield_wrangle",
            NodeKind::LSystem(_) => "lsystem",
            NodeKind::TestGeometry(_) => "test_geometry",
            NodeKind::PointDeform(_) => "point_deform",
            NodeKind::TransformPieces => "transform_pieces",
            NodeKind::ForEachBegin => "foreach_begin",
            NodeKind::ForEachEnd(_) => "foreach_end",
            NodeKind::CompileBegin => "compile_begin",
            NodeKind::CompileEnd { .. } => "compile_end",
            NodeKind::ObjectMerge { .. } => "object_merge",
            NodeKind::MaterialFracture(_) => "material_fracture",
            NodeKind::RbdBullet(_) => "rbd_bullet_solver",
            NodeKind::RbdConfigure(_) => "rbd_configure",
            NodeKind::Pack(_) => "pack",
            NodeKind::Unpack { .. } => "unpack",
            NodeKind::CrowdSolver(_) => "crowd_solver",
            NodeKind::Muscle(_) => "muscle",
            NodeKind::MuscleSolver(_) => "muscle_solver",
            NodeKind::MuscleSkin(_) => "muscle_skin",
            NodeKind::Skeleton(_) => "skeleton",
            NodeKind::RigPose(_) => "rig_pose",
            NodeKind::TwoBoneIk(_) => "two_bone_ik",
            NodeKind::ProceduralMotion(_) => "procedural_motion",
            NodeKind::SkeletonBlend { .. } => "skeleton_blend",
            NodeKind::JointCapture(_) => "joint_capture",
            NodeKind::BoneDeform => "bone_deform",
            NodeKind::Terrain(_) => "terrain",
            NodeKind::TerrainNoise(_) => "terrain_noise",
            NodeKind::TerrainErode(_) => "terrain_erode",
            NodeKind::TerrainTerrace(_) => "terrain_terrace",
            NodeKind::TerrainMask(_) => "terrain_mask",
            NodeKind::TerrainColorize(_) => "terrain_colorize",
            NodeKind::UvTexture(_) => "uv_texture",
            NodeKind::UvTransform(_) => "uv_transform",
            NodeKind::Color(_) => "color",
            NodeKind::Enumerate(_) => "enumerate",
            NodeKind::AttributeBlur(_) => "attribute_blur",
            NodeKind::AttributePromote(_) => "attribute_promote",
            NodeKind::AttributeTransfer(_) => "attribute_transfer",
            NodeKind::UvQuickShade { .. } => "uv_quickshade",
            NodeKind::AttributePaint { .. } => "attribute_paint",
            NodeKind::GroupCreate(_) => "group_create",
            NodeKind::SopSolver { .. } => "sop_solver",
            NodeKind::SolverInput { .. } => "solver_input",
            NodeKind::Subnet { .. } => "subnet",
            NodeKind::SubnetInput { .. } => "subnet_input",
            NodeKind::Switch { .. } => "switch",
            NodeKind::Geometry { .. } => "geometry",
            NodeKind::Material { kind, .. } => MATERIAL_NAMES[(*kind).min(MATERIAL_NAMES.len() - 1)],
            NodeKind::Shader(v) => match v.table {
                shading::Table::Principled => "principled_shader",
                shading::Table::Toon => "toon_shader",
                shading::Table::Hair => "hair_shader",
                shading::Table::Pyro => "pyro_shader",
            },
            NodeKind::MaterialBuilder => "material_builder",
            NodeKind::ShaderNode(_) => "shader_node",
            NodeKind::CopNetwork => "cop_network",
            NodeKind::Cop(_) => "cop",
            NodeKind::Pop(_) => "pop",
            NodeKind::PopNet { .. } => "popnet",
            NodeKind::TopNetwork => "top_network",
            NodeKind::Top(_) => "top",
        }
    }

    // Output ports: Material Fracture and RBD Bullet have geometry, constraints and proxy.
    pub(crate) fn num_outputs(&self) -> usize {
        match self {
            NodeKind::MaterialFracture(_) | NodeKind::RbdBullet(_) | NodeKind::RbdConfigure(_) => 3,
            _ => 1,
        }
    }

    // Names of the input and output ports, where they have meaning beyond "input N".
    pub(crate) fn port_names(&self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            NodeKind::MaterialFracture(_) | NodeKind::RbdConfigure(_) => (&["Geometry", "Constraints", "Proxy"], &["Geometry", "Constraints", "Proxy"]),
            NodeKind::RbdBullet(_) => (&["Geometry", "Constraints", "Proxy", "Collision"], &["Geometry", "Constraints", "Proxy"]),
            NodeKind::AttributeTransfer(_) => (&["Geometry to change", "Source of the attributes"], &[]),
            // As VEX numbers them: point(1, ...) reads the second.
            NodeKind::Wrangle { .. } | NodeKind::VolumeWrangle { .. } | NodeKind::HeightfieldWrangle { .. } | NodeKind::AttributeVop { .. } => {
                (&["0: geometry to change", "1 (read-only)", "2 (read-only)", "3 (read-only)"], &[])
            }
            NodeKind::AttributeFade(_) => (&["Geometry to fade", "Start frames (optional)", "Hold scales (optional)"], &[]),
            NodeKind::PopNet { .. } => (&["0: POP Source input 0 (VEX input 1)", "1: input 1 (VEX input 2)", "2: input 2 (VEX input 3)", "3: input 3"], &[]),
            NodeKind::Pop(n) if n.op == pop::SOLVER => (&["Objects (POP Object, Static Objects)", "Pre-solve: forces", "Post-solve: POP Sources"], &[]),
            NodeKind::Pop(_) => (&["Particles"], &[]),
            NodeKind::Subnet { .. } => (&["Input 1 (Subnet Input 1 inside)", "Input 2", "Input 3", "Input 4"], &[]),
            _ => (&[], &[]),
        }
    }

    pub(crate) fn num_inputs(&self) -> usize {
        match self {
            NodeKind::Add { .. } => 1,
            NodeKind::Grid { .. }
            | NodeKind::Plane { .. }
            | NodeKind::Cone { .. }
            | NodeKind::Capsule { .. }
            | NodeKind::Box { .. }
            | NodeKind::Sphere { .. }
            | NodeKind::Torus { .. }
            | NodeKind::Tube { .. } => 0,
            NodeKind::Terrain(_) => 0,
            NodeKind::Boolean(_) => 2,
            NodeKind::MatchSize(_) => 2,
            NodeKind::AttributeTransfer(_) => 2,
            NodeKind::ObjectMerge { .. } => 0,
            NodeKind::MaterialFracture(_) => 3,
            NodeKind::RbdBullet(_) => 4,
            NodeKind::RbdConfigure(_) => 3,
            NodeKind::PointDeform(_) | NodeKind::TransformPieces => 3,
            NodeKind::CrowdSolver(_) => 3,
            NodeKind::MuscleSkin(_) => 2,
            NodeKind::Skeleton(_) => 0,
            NodeKind::SkeletonBlend { .. } | NodeKind::JointCapture(_) => 2,
            NodeKind::BoneDeform => 3,
            NodeKind::Particles { .. } => 2,
            NodeKind::Flip { .. } => 2,
            NodeKind::WhitewaterSolver(_) => 2,
            NodeKind::Pyro(_) | NodeKind::RbdSolver(_) | NodeKind::VellumSolver(_) => 2,
            NodeKind::HairGenerate(_) | NodeKind::HairDeform(_) => 2,
            NodeKind::SopSolver { .. } => 1,
            NodeKind::SolverInput { .. } => 0,
            NodeKind::Subnet { .. } => 4,
            NodeKind::SubnetInput { .. } => 0,
            NodeKind::LopMerge => 2,
            NodeKind::FileImport { .. } => 0,
            NodeKind::ParticleSurface { .. } => 1,
            NodeKind::Merge | NodeKind::Switch { .. } | NodeKind::CopyToPoints { .. } => 2,
            NodeKind::AttributeCreate(_)
            | NodeKind::AttributeRandomize(_)
            | NodeKind::AttributeDelete(_)
            | NodeKind::Color(_)
            | NodeKind::Enumerate(_)
            | NodeKind::AttributeBlur(_)
            | NodeKind::AttributeRemap(_)
            | NodeKind::AttributePromote(_) => 1,
            NodeKind::Curve(_) | NodeKind::Circle(_) => 0,
            NodeKind::AttributeFade(_) => 3,
            // Input 0 is the geometry they change; 1-3 are read by point(1, ...), @opinput1_P and the like.
            NodeKind::Wrangle { .. } | NodeKind::VolumeWrangle { .. } | NodeKind::HeightfieldWrangle { .. } | NodeKind::AttributeVop { .. } => vex::INPUTS,
            NodeKind::LSystem(_) | NodeKind::TestGeometry(_) => 0,
            NodeKind::Vop(v) => v.def().inputs.len(),
            NodeKind::MoonRayRender { .. } => 0,
            NodeKind::Camera { .. } | NodeKind::Light { .. } | NodeKind::Geometry { .. } | NodeKind::Material { .. } => 0,
            NodeKind::EnvironmentLight { .. } => 0,
            NodeKind::Shader(_) | NodeKind::MaterialBuilder => 0,
            NodeKind::ShaderNode(n) => n.def().inputs.len(),
            NodeKind::CopNetwork | NodeKind::TopNetwork => 0,
            NodeKind::Cop(n) => n.def().inputs,
            NodeKind::Pop(n) => n.def().inputs,
            NodeKind::PopNet { .. } => 4,
            NodeKind::Top(n) => n.def().inputs,
            _ => 1,
        }
    }

    pub(crate) fn color(&self) -> egui::Color32 {
        if matches!(self, NodeKind::Geometry { .. }) {
            return egui::Color32::from_rgb(110, 200, 150);
        }
        if matches!(self, NodeKind::Material { .. }) {
            return egui::Color32::from_rgb(200, 110, 160);
        }
        if self.is_solver() {
            return egui::Color32::from_rgb(225, 115, 60);
        }
        if self.is_lop() {
            return egui::Color32::from_rgb(70, 175, 190);
        }
        if matches!(self, NodeKind::SolverInput { .. } | NodeKind::SubnetInput { .. }) {
            return egui::Color32::from_rgb(150, 150, 110);
        }
        match self.num_inputs() {
            0 => egui::Color32::from_rgb(90, 170, 90),
            2 => egui::Color32::from_rgb(170, 120, 200),
            _ => match self {
                NodeKind::Subdivide { .. }
                | NodeKind::Remesh(_)
                | NodeKind::Fuse { .. }
                | NodeKind::Mirror { .. }
                | NodeKind::Copy { .. }
                |                 NodeKind::CopyToPoints { .. }
                    | NodeKind::AttributeCreate(_)
                    | NodeKind::AttributeRandomize(_)
                    | NodeKind::AttributeDelete(_)
                    | NodeKind::Wrangle { .. }
                    | NodeKind::VolumeWrangle { .. }
                    | NodeKind::HeightfieldWrangle { .. } => egui::Color32::from_rgb(200, 140, 60),
                NodeKind::Null => egui::Color32::from_gray(150),
                NodeKind::MoonRayRender { .. } => egui::Color32::from_rgb(100, 150, 210),
                NodeKind::Camera { .. } => egui::Color32::from_rgb(80, 180, 220),
                NodeKind::Light { .. } => egui::Color32::from_rgb(240, 180, 70),
                NodeKind::EnvironmentLight { .. } => egui::Color32::from_rgb(235, 200, 110),
                _ => egui::Color32::from_rgb(80, 140, 210),
            },
        }
    }

    // ---- serialization (flat list of numbers) ----
    pub(crate) fn params(&self) -> Vec<f32> {
        match self {
            NodeKind::Add { count, spacing, layout, points, delete_geometry } => {
                let mut p = vec![*count as f32, *spacing, *layout as f32, *delete_geometry as u8 as f32];
                p.extend(points.iter().flatten());
                p
            }
            NodeKind::Grid { size, res } => vec![*size, *res as f32],
            NodeKind::Plane { size, res } => vec![*size, *res as f32],
            NodeKind::Cone { radius, height, rows, cols, caps } => {
                vec![*radius, *height, *rows as f32, *cols as f32, *caps as u8 as f32]
            }
            NodeKind::Capsule { radius, length, rows, cols } => {
                vec![*radius, *length, *rows as f32, *cols as f32]
            }
            NodeKind::Box { size, div } => vec![*size, *div as f32],
            NodeKind::Sphere { radius, rows, cols } => vec![*radius, *rows as f32, *cols as f32],
            NodeKind::Torus { major, minor, rows, cols } => {
                vec![*major, *minor, *rows as f32, *cols as f32]
            }
            NodeKind::Transform { t, r, s, p } => [t.as_slice(), r, s, p].concat(),
            NodeKind::Noise { freq, amp } => vec![*freq, *amp],
            NodeKind::Taper { factor } => vec![*factor],
            NodeKind::Twist { angle } | NodeKind::Bend { angle } => vec![*angle],
            NodeKind::Smooth { iterations, strength } => vec![*iterations as f32, *strength],
            NodeKind::Bevel { amount, segments } => vec![*amount, *segments as f32],
            NodeKind::Fuse { tol } => vec![*tol],
            NodeKind::Mirror { axis } => vec![*axis as f32],
            NodeKind::Copy { count, t, r, s } => {
                [vec![*count as f32], t.to_vec(), r.to_vec(), vec![*s]].concat()
            }
            NodeKind::CopyToPoints { scale, rotation, .. } => {
                vec![*scale, rotation[0], rotation[1], rotation[2]]
            }
            NodeKind::AttributeCreate(s) => s.to_params(),
            NodeKind::AttributeRandomize(r) => r.to_params(),
            NodeKind::AttributeDelete(s) => s.to_params(),
            NodeKind::Wrangle { .. } | NodeKind::VolumeWrangle { .. } | NodeKind::HeightfieldWrangle { .. } => vec![],
            NodeKind::LSystem(l) => l.to_params(),
            NodeKind::TestGeometry(t) => t.to_params(),
            NodeKind::AttributeVop { domain } => vec![*domain as f32],
            NodeKind::Vop(v) => vec![v.op as f32, v.ty as f32, v.value[0], v.value[1], v.value[2], v.input as f32],
            NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                vec![*r_bot, *r_top, *height, *rows as f32, *cols as f32, *caps as u8 as f32]
            }
            NodeKind::Peak { dist } => vec![*dist],
            NodeKind::Ripple { freq, amp, speed } => vec![*freq, *amp, *speed],
            NodeKind::Jitter { amp, seed, scale } => vec![*amp, *seed as f32, scale[0], scale[1], scale[2]],
            NodeKind::Particles {
                count,
                emission_rate,
                lifetime,
                speed,
                spread,
                gravity,
                seed,
                drag,
                bounce,
                friction,
                radius,
                substeps,
                along_normals,
            } => {
                vec![
                    *count as f32,
                    *emission_rate,
                    *lifetime,
                    *speed,
                    *spread,
                    *gravity,
                    *seed as f32,
                    *drag,
                    *bounce,
                    *friction,
                    *radius,
                    *substeps as f32,
                    *along_normals as u8 as f32,
                ]
            }
            NodeKind::WhitewaterSolver(w) => w.to_params(),
            NodeKind::Flip {
                particle_count,
                grid_resolution,
                flip_ratio,
                viscosity,
                gravity,
                max_dt,
                substeps,
                seed,
                domain,
            } => [
                vec![*particle_count as f32, *grid_resolution as f32, *flip_ratio, *viscosity, *gravity, *max_dt, *substeps as f32, *seed as f32],
                domain.to_vec(),
            ]
            .concat(),
            NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                vec![*radius, *join_distance, *resolution as f32, *smoothing as f32]
            }
            NodeKind::Extrude(e) => e.to_params(),
            NodeKind::Clip { axis, offset, invert } => vec![*axis as f32, *offset, *invert as u8 as f32],
            NodeKind::Blast { component, indices, invert } => {
                let mut params = vec![*component as f32, *invert as u8 as f32, indices.len() as f32];
                params.extend(indices.iter().map(|index| *index as f32));
                params
            }
            NodeKind::Cache { cache_id, mode, range } => vec![*cache_id as f32, *mode as f32, range[0], range[1], range[2]],
            NodeKind::Vdb { voxel_size, padding, operation, iterations, output } => {
                vec![*voxel_size, *padding as f32, *operation as f32, *iterations as f32, *output as f32]
            }
            NodeKind::ConvertVdb { mode, iso } => vec![*mode as f32, *iso],
            NodeKind::FileImport { looping, .. } => vec![*looping as u8 as f32],
            NodeKind::FileExport { .. } | NodeKind::UvQuickShade { .. } => vec![],
            NodeKind::MoonRayRender { .. } => vec![],
            NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } => {
                [position.to_vec(), rotation.to_vec(), vec![*focal, *near, *orthographic as u8 as f32, pan[0], pan[1], *zoom]].concat()
            }
            NodeKind::Light { position, rotation, color, intensity, kind } => {
                [position.to_vec(), rotation.to_vec(), color.to_vec(), vec![*intensity, *kind as f32]].concat()
            }
            NodeKind::Switch { index } => vec![*index as f32],
            NodeKind::Reverse => vec![],
            NodeKind::Pyro(settings) => settings.to_params(),
            NodeKind::LoopParticles { start, length } => vec![*start, *length],
            NodeKind::TimeShift { offset, hold, at } => vec![*offset, *hold as u8 as f32, *at],
            NodeKind::TimeWarp { speed, anchor } => vec![*speed, *anchor],
            NodeKind::TimeBlend => vec![],
            NodeKind::Trail { samples, step, mode, fade } => vec![*samples as f32, *step, *mode as f32, *fade as u8 as f32],
            NodeKind::MakeLoop { start, length, blend } => vec![*start, *length, *blend],
            NodeKind::HairGuides(s) => {
                vec![s.count as f32, s.length, s.length_variance, s.segments as f32, s.droop, s.seed as f32]
            }
            NodeKind::GuideProcess(s) => vec![s.op as f32, s.amount, s.frequency, s.seed as f32],
            NodeKind::HairDeform(s) => vec![s.rest_frame],
            NodeKind::HairGenerate(s) => vec![
                s.count as f32,
                s.segments as f32,
                s.clump,
                s.length_variance,
                s.frizz,
                s.width_root,
                s.width_tip,
                s.seed as f32,
            ],
            NodeKind::RbdSolver(settings) => settings.to_params(),
            NodeKind::Normal(s) => vec![s.domain as f32, s.cusp_angle, s.weighting as f32, s.reverse as u8 as f32],
            NodeKind::GroupCreate(g) => {
                let mut p = vec![(g.domain == groups::GroupDomain::Prims) as u8 as f32, g.method as f32];
                p.extend(g.bbox_min);
                p.extend(g.bbox_max);
                p.extend([g.percent, g.seed as f32]);
                p.extend(g.direction);
                p.extend([g.spread, g.combine as f32, g.invert as u8 as f32]);
                p
            }
            NodeKind::EnvironmentLight { color, intensity, rotation, visible, .. } => {
                vec![color[0], color[1], color[2], *intensity, *rotation, *visible as u8 as f32]
            }
            // Brush settings, then every painted value.
            NodeKind::AttributePaint { values, settings: s, .. } => {
                let mut p = vec![s.value, s.default, s.radius, s.strength, s.mode as f32, s.visualize as u8 as f32];
                p.extend_from_slice(values);
                p
            }
            NodeKind::Scatter(s) => vec![s.count as f32, s.seed as f32, s.relax_iterations as f32, s.volume as u8 as f32],
            NodeKind::Boolean(s) => vec![s.operation as f32, s.mode as f32, s.resolution as f32],
            // One value per slot: the material's node id + 1 (0: none).
            NodeKind::MaterialAssign { slots } => slots.iter().map(|(_, m)| m.map_or(0.0, |m| m as f32 + 1.0)).collect(),
            NodeKind::Measure(m) => vec![m.kind as f32, m.curvature as f32, m.summary as f32],
            NodeKind::ExtractCentroid(c) => vec![c.partition as f32, c.method as f32],
            NodeKind::MatchSize(m) => {
                let mut v: Vec<f32> = m.justify.iter().map(|j| *j as f32).collect();
                v.extend([m.scale_to_fit as u8 as f32, m.uniform as u8 as f32]);
                v.extend(m.target_center);
                v.extend(m.target_size);
                v
            }
            NodeKind::ConvertLine => Vec::new(),
            NodeKind::Metaball(m) => vec![m.radius, m.threshold, m.resolution as f32],
            NodeKind::Connectivity(c) => vec![c.class as f32],
            NodeKind::Resample(r) => vec![r.by_length as u8 as f32, r.length, r.segments as f32],
            NodeKind::Curve(c) => c.params(),
            NodeKind::Circle(c) => c.params(),
            NodeKind::AttributeRemap(r) => r.params(),
            NodeKind::AttributeFade(f) => f.params(),
            NodeKind::PointDeform(s) => vec![s.influences as f32, s.fit_neighbours as f32, s.rotate as u8 as f32],
            NodeKind::TransformPieces => vec![],
            NodeKind::ForEachBegin | NodeKind::CompileBegin => vec![],
            NodeKind::ForEachEnd(f) => f.to_params(),
            NodeKind::CompileEnd { begin } => vec![begin.map_or(-1.0, |b| b as f32)],
            // Into-this flag, then each source's node id + 1 (0: empty slot).
            NodeKind::ObjectMerge { sources, into_this } => std::iter::once(*into_this as u8 as f32).chain(sources.iter().map(|s| s.map_or(0.0, |s| s as f32 + 1.0))).collect(),
            NodeKind::MaterialFracture(s) => vec![
                s.material as f32,
                s.pieces as f32,
                s.seed as f32,
                s.impact[0],
                s.impact[1],
                s.impact[2],
                s.rings as f32,
                s.spokes as f32,
                s.grain_axis as f32,
                s.splinter,
                s.separate,
                s.glue_strength,
            ],
            NodeKind::CrowdSolver(s) => s.to_params(),
            NodeKind::RbdBullet(s) => s.to_params(),
            NodeKind::RbdConfigure(s) => s.to_params(),
            NodeKind::Pack(s) => vec![s.mode as f32, s.pivot as f32],
            NodeKind::Unpack { transfer } => vec![*transfer as u8 as f32],
            NodeKind::Muscle(s) => s.to_params(),
            NodeKind::MuscleSolver(s) => vec![s.stiffness, s.damping, s.smoothing as f32, s.substeps as f32],
            NodeKind::MuscleSkin(s) => vec![s.falloff, s.influences as f32],
            NodeKind::Skeleton(s) => vec![s.preset as f32, s.scale, s.joints as f32, s.length],
            NodeKind::RigPose(p) => p.values.iter().flatten().copied().collect(),
            NodeKind::TwoBoneIk(k) => [k.goal.to_vec(), k.pole.to_vec(), vec![k.use_pole as u8 as f32, k.blend, k.keep_tip as u8 as f32]].concat(),
            NodeKind::ProceduralMotion(m) => vec![m.motion as f32, m.rate, m.amplitude, m.travel, m.lag, m.axis as f32],
            NodeKind::SkeletonBlend { weight } => vec![*weight],
            NodeKind::JointCapture(c) => vec![c.influences as f32, c.falloff, c.visualize as u8 as f32],
            NodeKind::BoneDeform => vec![],
            NodeKind::Terrain(s) => vec![s.size[0], s.size[1], s.resolution as f32],
            NodeKind::TerrainNoise(s) => vec![
                s.kind as f32,
                s.blend as f32,
                s.amplitude,
                s.element_size,
                s.octaves as f32,
                s.roughness,
                s.offset[0],
                s.offset[1],
                s.seed as f32,
                s.positive as u8 as f32,
                s.use_mask as u8 as f32,
            ],
            NodeKind::TerrainErode(s) => vec![
                s.droplets as f32,
                s.erosion,
                s.deposition,
                s.capacity,
                s.evaporation,
                s.inertia,
                s.radius as f32,
                s.thermal_iterations as f32,
                s.talus_angle,
                s.seed as f32,
            ],
            NodeKind::TerrainTerrace(s) => vec![s.step, s.offset, s.sharpness, s.use_mask as u8 as f32],
            NodeKind::TerrainMask(s) => vec![
                s.source as f32,
                s.min,
                s.max,
                s.falloff,
                s.blur as f32,
                s.invert as u8 as f32,
                s.combine as f32,
                s.visualize as u8 as f32,
            ],
            NodeKind::TerrainColorize(s) => [
                s.sand.to_vec(),
                s.grass.to_vec(),
                s.rock.to_vec(),
                s.snow.to_vec(),
                vec![s.sand_level, s.snow_level, s.rock_slope, s.blend],
            ]
            .concat(),
            NodeKind::UvTexture(s) => vec![s.projection as f32, s.axis as f32, s.scale[0], s.scale[1], s.offset[0], s.offset[1]],
            NodeKind::Color(s) => s.params(),
            NodeKind::Enumerate(s) => s.params(),
            NodeKind::AttributeBlur(s) => s.params(),
            NodeKind::AttributePromote(s) => s.params(),
            NodeKind::AttributeTransfer(s) => s.params(),
            NodeKind::UvTransform(s) => vec![s.translate[0], s.translate[1], s.rotate, s.scale[0], s.scale[1], s.pivot[0], s.pivot[1]],
            NodeKind::VellumSolver(s) => vec![
                s.kind as f32,
                s.stretch_stiffness,
                s.bend_stiffness,
                s.shape_stiffness,
                s.mass,
                s.thickness,
                s.gravity,
                s.damping,
                s.friction,
                s.substeps as f32,
                s.iterations as f32,
                s.ground as u8 as f32,
                s.self_collision as u8 as f32,
                s.pin_to_animation as u8 as f32,
            ],
            NodeKind::SopSolver { substeps } => vec![*substeps as f32],
            NodeKind::LopSopImport { geometry, .. } => vec![geometry.map_or(-1.0, |g| g as f32)],
            NodeKind::LopReference { sublayer, .. } => vec![*sublayer as u8 as f32],
            NodeKind::LopPrimitive { shape, size, t, .. } => vec![*shape as f32, *size, t[0], t[1], t[2]],
            NodeKind::LopTransform { t, r, s, .. } => [*t, *r, *s].concat(),
            NodeKind::LopLight { kind, position, target, color, intensity, size, .. } => {
                [vec![*kind as f32], position.to_vec(), target.to_vec(), color.to_vec(), vec![*intensity, *size]].concat()
            }
            NodeKind::LopCamera { position, target, focal, aperture, .. } => [position.to_vec(), target.to_vec(), vec![*focal, *aperture]].concat(),
            NodeKind::LopMaterial { material, .. } => vec![material.map_or(-1.0, |m| m as f32)],
            NodeKind::LopPrune { hide, .. } => vec![*hide as u8 as f32],
            NodeKind::LopMerge | NodeKind::LopExport { .. } => vec![],
            NodeKind::SolverInput { index } => vec![*index as f32],
            NodeKind::Subnet { values, .. } => values.clone(),
            NodeKind::SubnetInput { index } => vec![*index as f32],
            NodeKind::Subdivide { levels } => vec![*levels as f32],
            NodeKind::Remesh(r) => r.params(),
            NodeKind::Merge | NodeKind::Null => vec![],
            NodeKind::Geometry { material } => vec![material.map_or(-1.0, |m| m as f32)],
            NodeKind::Material { kind, color, roughness, value } => {
                vec![*kind as f32, color[0], color[1], color[2], *roughness, *value]
            }
            NodeKind::Shader(v) => v.to_params(),
            NodeKind::MaterialBuilder => vec![],
            NodeKind::ShaderNode(n) => n.to_params(),
            NodeKind::CopNetwork | NodeKind::TopNetwork => vec![],
            NodeKind::Cop(n) => n.to_params(),
            NodeKind::Pop(n) => n.to_params(),
            NodeKind::PopNet { substeps } => vec![*substeps as f32],
            NodeKind::Top(n) => n.to_params(),
        }
    }
    pub(crate) fn from_params(name: &str, p: &[f32]) -> Option<NodeKind> {
        let g = |i: usize| p.get(i).copied().unwrap_or(0.0);
        let u = |i: usize| g(i).max(0.0) as usize;
        let v3 = |i: usize| [g(i), g(i + 1), g(i + 2)];
        Some(match name {
            "add" => NodeKind::Add {
                count: u(0).min(100_000),
                spacing: g(1).max(0.001),
                layout: u(2).min(2),
                delete_geometry: g(3) > 0.5,
                points: p.get(4..).unwrap_or(&[]).chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            },
            "grid" => NodeKind::Grid { size: g(0), res: u(1) },
            "plane" => NodeKind::Plane { size: g(0), res: u(1) },
            "cone" => NodeKind::Cone {
                radius: g(0), height: g(1), rows: u(2), cols: u(3), caps: g(4) != 0.0,
            },
            "capsule" => NodeKind::Capsule {
                radius: g(0), length: g(1), rows: u(2), cols: u(3),
            },
            "box" => NodeKind::Box { size: g(0), div: u(1) },
            "sphere" => NodeKind::Sphere { radius: g(0), rows: u(1), cols: u(2) },
            "torus" => NodeKind::Torus { major: g(0), minor: g(1), rows: u(2), cols: u(3) },
            "xform" => NodeKind::Transform { t: v3(0), r: v3(3), s: v3(6), p: v3(9) },
            "noise" => NodeKind::Noise { freq: g(0), amp: g(1) },
            "taper" => NodeKind::Taper { factor: g(0) },
            "twist" => NodeKind::Twist { angle: g(0) },
            "bend" => NodeKind::Bend { angle: g(0) },
            "smooth" => NodeKind::Smooth { iterations: u(0), strength: g(1) },
            "bevel" => NodeKind::Bevel { amount: g(0), segments: u(1).clamp(1, 8) },
            "fuse" => NodeKind::Fuse { tol: g(0) },
            "subdivide" => NodeKind::Subdivide { levels: u(0).clamp(1, 4) },
            "remesh" => NodeKind::Remesh(remesh::RemeshSettings::from_params(p)),
            "mirror" => NodeKind::Mirror { axis: u(0) },
            "copy" => NodeKind::Copy { count: u(0), t: v3(1), r: v3(4), s: g(7) },
            "copy_to_points" => NodeKind::CopyToPoints {
                scale: g(0).max(0.001),
                rotation: v3(1),
                piece_attribute: String::new(),
            },
            "attribute_create" => NodeKind::AttributeCreate(attrib::CreateSettings::from_params(p)),
            "attribute_randomize" => NodeKind::AttributeRandomize(randomize::RandomizeSettings::from_params(p)),
            "attribute_delete" => NodeKind::AttributeDelete(attrib::DeleteSettings::from_params(p)),
            "wrangle" => NodeKind::Wrangle { code: String::new(), domain: u(0).min(3) },
            "volume_wrangle" => NodeKind::VolumeWrangle { code: String::new() },
            "heightfield_wrangle" => NodeKind::HeightfieldWrangle { code: String::new() },
            "lsystem" => NodeKind::LSystem(lsystem::LSystemSettings::from_params(p)),
            "test_geometry" => NodeKind::TestGeometry(testgeo::TestGeoSettings::from_params(p)),
            "attribute_vop" => NodeKind::AttributeVop { domain: u(0).min(3) },
            "vop" => NodeKind::Vop(vop::VopNode {
                op: u(0).min(vop::OPS.len() - 1),
                name: String::new(),
                ty: u(1),
                value: [g(2), g(3), g(4)],
                input: u(5).min(3),
            }),
            "merge" => NodeKind::Merge,
            "null" => NodeKind::Null,
            "tube" => NodeKind::Tube {
                r_bot: g(0),
                r_top: g(1),
                height: g(2),
                rows: u(3),
                cols: u(4),
                caps: g(5) > 0.5,
            },
            "peak" => NodeKind::Peak { dist: g(0) },
            "ripple" => NodeKind::Ripple { freq: g(0), amp: g(1), speed: g(2) },
            // Old saves have no scale: 1 on every axis.
            "jitter" => NodeKind::Jitter { amp: g(0), seed: u(1), scale: [2, 3, 4].map(|i| p.get(i).copied().unwrap_or(1.0)) },
            "particles" => {
                let or = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
                NodeKind::Particles {
                    count: u(0).clamp(1, 50_000),
                    emission_rate: g(1).max(0.01),
                    lifetime: g(2).max(0.01),
                    speed: g(3),
                    spread: g(4).max(0.0),
                    gravity: g(5),
                    seed: u(6),
                    drag: or(7, 0.0).max(0.0),
                    bounce: or(8, 0.4).clamp(0.0, 1.0),
                    friction: or(9, 0.1).clamp(0.0, 1.0),
                    radius: or(10, 0.02).max(0.0),
                    substeps: (or(11, 2.0).max(1.0) as usize).min(8),
                    along_normals: or(12, 0.0) > 0.5,
                }
            }
            "whitewater_solver" => NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings::from_params(p)),
            "flip" => NodeKind::Flip {
                particle_count: u(0).clamp(1, 100_000),
                grid_resolution: u(1).clamp(16, 128),
                flip_ratio: g(2).clamp(0.0, 1.0),
                viscosity: g(3).max(0.0),
                gravity: g(4),
                max_dt: g(5).clamp(0.001, 0.1),
                substeps: u(6).clamp(1, 8),
                seed: u(7),
                domain: [g(8), g(9), g(10), g(11).max(0.0), g(12).max(0.0), g(13).max(0.0)],
            },
            "particle_surface" => NodeKind::ParticleSurface {
                radius: g(0).max(0.001),
                join_distance: g(1).max(0.0),
                resolution: u(2).clamp(8, 512),
                smoothing: u(3).min(8),
            },
            "extrude" => NodeKind::Extrude(extrude::ExtrudeSettings::from_params(p)),
            "clip" => NodeKind::Clip { axis: u(0), offset: g(1), invert: g(2) > 0.5 },
            "blast" => {
                let count = u(2).min(p.len().saturating_sub(3));
                NodeKind::Blast {
                    component: u(0).min(2),
                    indices: p.iter().skip(3).take(count).map(|value| (*value).max(0.0) as usize).collect(),
                    invert: g(1) > 0.5,
                }
            }
            "cache" => NodeKind::Cache {
                cache_id: u(0),
                mode: u(1).min(1),
                // Scenes from before the range was added: frames 1 to 100.
                range: [p.get(2).copied().unwrap_or(1.0), p.get(3).copied().unwrap_or(100.0), p.get(4).copied().unwrap_or(1.0).max(0.001)],
            },
            "vdb" => NodeKind::Vdb {
                voxel_size: g(0).max(0.01),
                padding: u(1).min(4),
                operation: u(2).min(3),
                iterations: u(3).clamp(1, 4),
                output: u(4).min(1),
            },
            "convert_vdb" => NodeKind::ConvertVdb { mode: u(0).min(1), iso: g(1) },
            "file" => NodeKind::FileImport { file: String::new(), take: String::new(), looping: p.first().is_some_and(|l| *l != 0.0) },
            "file_export" => NodeKind::FileExport { file: String::new() },
            "camera" => NodeKind::Camera {
                position: v3(0),
                rotation: v3(3),
                focal: g(6).clamp(0.5, 2.5),
                near: g(7).clamp(0.05, 10.0),
                orthographic: g(8) > 0.5,
                pan: [g(9), g(10)],
                zoom: g(11).clamp(MIN_ZOOM, 200.0),
            },
            "light" => NodeKind::Light {
                position: v3(0),
                rotation: v3(3),
                color: v3(6),
                intensity: g(9).max(0.0),
                kind: u(10).min(2),
            },
            "reverse" => NodeKind::Reverse,
            "pyro" => NodeKind::Pyro(sims::PyroSettings::from_params(p)),
            "time_warp" => NodeKind::TimeWarp { speed: p.first().copied().unwrap_or(1.0), anchor: p.get(1).copied().unwrap_or(0.0) },
            "time_blend" => NodeKind::TimeBlend,
            "trail" => NodeKind::Trail {
                samples: p.first().map_or(5, |v| (*v as usize).clamp(2, 200)),
                step: p.get(1).copied().unwrap_or(1.0).max(0.01),
                mode: p.get(2).map_or(0, |v| (*v as usize).min(2)),
                fade: p.get(3).is_none_or(|v| *v > 0.5),
            },
            "time_shift" => NodeKind::TimeShift { offset: p.first().copied().unwrap_or(0.0), hold: p.get(1).is_some_and(|v| *v > 0.5), at: p.get(2).copied().unwrap_or(0.0) },
            "loop_particles" => NodeKind::LoopParticles { start: p.first().copied().unwrap_or(1.0), length: p.get(1).copied().unwrap_or(1.0).max(1.0 / sims::SIM_FPS) },
            "make_loop" => NodeKind::MakeLoop {
                start: p.first().copied().unwrap_or(2.0),
                length: p.get(1).copied().unwrap_or(1.0).max(1.0 / sims::SIM_FPS),
                blend: p.get(2).copied().unwrap_or(0.5).max(0.0),
            },
            "hair_guides" => NodeKind::HairGuides(groom::GuideSettings {
                count: u(0).clamp(1, 20_000),
                length: g(1).max(0.0),
                length_variance: g(2).clamp(0.0, 1.0),
                segments: u(3).clamp(1, 64),
                droop: g(4).max(0.0),
                seed: u(5).min(u32::MAX as usize) as u32,
            }),
            "guide_process" => NodeKind::GuideProcess(groom::ProcessSettings {
                op: u(0).min(groom::GUIDE_OPS.len() - 1),
                amount: g(1),
                frequency: g(2),
                seed: u(3).min(u32::MAX as usize) as u32,
            }),
            "hair_deform" => NodeKind::HairDeform(groom::DeformSettings { rest_frame: p.first().copied().unwrap_or(1.0) }),
            "hair_generate" => NodeKind::HairGenerate(groom::HairSettings {
                count: u(0).clamp(1, 200_000),
                segments: u(1).clamp(1, 64),
                clump: g(2).clamp(0.0, 1.0),
                length_variance: g(3).clamp(0.0, 1.0),
                frizz: g(4).max(0.0),
                width_root: g(5).max(0.0),
                width_tip: g(6).max(0.0),
                seed: u(7).min(u32::MAX as usize) as u32,
            }),
            "rbd_solver" => NodeKind::RbdSolver(sims::RbdSettings::from_params(p)),
            "group_create" => NodeKind::GroupCreate(groups::GroupCreateSettings {
                name: "group1".into(),
                domain: if g(0) > 0.5 { groups::GroupDomain::Prims } else { groups::GroupDomain::Points },
                method: u(1).min(groups::CREATE_METHODS.len() - 1),
                bbox_min: v3(2),
                bbox_max: v3(5),
                percent: g(8),
                seed: u(9).min(u32::MAX as usize) as u32,
                direction: v3(10),
                spread: g(13),
                expression: String::new(),
                combine: u(14).min(groups::COMBINE_MODES.len() - 1),
                invert: g(15) > 0.5,
            }),
            "environment_light" => NodeKind::EnvironmentLight {
                file: String::new(),
                color: v3(0),
                intensity: g(3).max(0.0),
                rotation: g(4),
                visible: p.get(5).is_none_or(|v| *v > 0.5),
            },
            "attribute_paint" => NodeKind::AttributePaint {
                attribute: "density".into(),
                values: p.get(6..).map(<[f32]>::to_vec).unwrap_or_default(),
                settings: paint::PaintSettings {
                    value: g(0),
                    default: g(1),
                    radius: g(2).clamp(1.0, 2000.0),
                    strength: g(3).clamp(0.0, 1.0),
                    mode: u(4).min(paint::PAINT_MODES.len() - 1),
                    visualize: g(5) > 0.5,
                },
            },
            "normal" => NodeKind::Normal(sops::NormalSettings {
                domain: u(0).min(1),
                cusp_angle: g(1).clamp(0.0, 180.0),
                weighting: u(2).min(sops::NORMAL_WEIGHTS.len() - 1),
                reverse: g(3) > 0.5,
            }),
            "scatter" => NodeKind::Scatter(sops::ScatterSettings {
                count: u(0).clamp(1, 1_000_000),
                seed: u(1).min(u32::MAX as usize) as u32,
                relax_iterations: u(2).min(50),
                volume: g(3) > 0.5,
            }),
            "boolean" => NodeKind::Boolean(boolean::BooleanSettings {
                operation: u(0).min(boolean::OPERATIONS.len() - 1),
                mode: u(1).min(boolean::MODES.len() - 1),
                resolution: u(2).clamp(16, 400),
            }),
            "point_deform" => NodeKind::PointDeform(deform::PointDeformSettings {
                influences: if p.is_empty() { 4 } else { u(0).clamp(1, 32) },
                fit_neighbours: if p.len() > 1 { u(1).clamp(3, 64) } else { 6 },
                rotate: p.get(2).is_none_or(|v| *v > 0.5),
            }),
            "transform_pieces" => NodeKind::TransformPieces,
            "extract_centroid" => NodeKind::ExtractCentroid(sopkit::CentroidSettings { partition: u(0).min(2), method: u(1).min(1) }),
            "match_size" => {
                let d = sopkit::MatchSizeSettings::defaults();
                let f = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
                NodeKind::MatchSize(sopkit::MatchSizeSettings {
                    justify: std::array::from_fn(|a| (f(a, d.justify[a] as f32).max(0.0) as usize).min(3)),
                    scale_to_fit: f(3, 0.0) != 0.0,
                    uniform: f(4, 1.0) != 0.0,
                    target_center: std::array::from_fn(|a| f(5 + a, 0.0)),
                    target_size: std::array::from_fn(|a| f(8 + a, 1.0)),
                })
            }
            "convert_line" => NodeKind::ConvertLine,
            "metaball" => {
                let d = sopkit::MetaballSettings::defaults();
                NodeKind::Metaball(sopkit::MetaballSettings {
                    radius: p.first().copied().unwrap_or(d.radius).max(1e-3),
                    threshold: p.get(1).copied().unwrap_or(d.threshold).clamp(0.01, 0.99),
                    resolution: (p.get(2).copied().unwrap_or(d.resolution as f32) as usize).clamp(8, 256),
                })
            }
            "curve" => NodeKind::Curve(curve::CurveSettings::from_params(p)),
            "circle" => NodeKind::Circle(circle::CircleSettings::from_params(p)),
            "attribute_remap" => NodeKind::AttributeRemap(remap::RemapSettings::from_params(p)),
            "attribute_fade" => NodeKind::AttributeFade(fade::FadeSettings::from_params(p)),
            "connectivity" => NodeKind::Connectivity(sopkit::ConnectivitySettings { class: u(0).min(1), ..sopkit::ConnectivitySettings::defaults() }),
            "resample" => {
                let d = sopkit::ResampleSettings::defaults();
                NodeKind::Resample(sopkit::ResampleSettings {
                    by_length: p.first().copied().unwrap_or(1.0) != 0.0,
                    length: p.get(1).copied().unwrap_or(d.length).max(1e-4),
                    segments: (p.get(2).copied().unwrap_or(d.segments as f32) as usize).max(1),
                })
            }
            "measure" => NodeKind::Measure(measure::MeasureSettings {
                kind: u(0).min(measure::KINDS.len() - 1),
                curvature: u(1).min(1),
                summary: u(2).min(measure::SUMMARIES.len() - 1),
                ..measure::MeasureSettings::new(u(0).min(measure::KINDS.len() - 1))
            }),
            "foreach_begin" => NodeKind::ForEachBegin,
            "foreach_end" => NodeKind::ForEachEnd(flow::ForEachSettings::from_params(p)),
            "compile_begin" => NodeKind::CompileBegin,
            "compile_end" => NodeKind::CompileEnd { begin: p.first().filter(|v| **v >= 0.0).map(|v| *v as usize) },
            "object_merge" => NodeKind::ObjectMerge {
                into_this: p.first().is_none_or(|v| *v > 0.5),
                sources: p.iter().skip(1).map(|v| (*v >= 0.5).then(|| (*v - 1.0).round() as usize)).collect(),
            },
            "material" => NodeKind::MaterialAssign {
                slots: p.iter().map(|v| (String::new(), (*v >= 0.5).then(|| (*v - 1.0).round() as usize))).collect(),
            },
            "material_fracture" => NodeKind::MaterialFracture(fracture::FractureSettings {
                material: u(0).min(fracture::MATERIALS.len() - 1),
                pieces: u(1).clamp(2, 500),
                seed: u(2).min(u32::MAX as usize) as u32,
                impact: v3(3),
                rings: u(6).clamp(1, 20),
                spokes: u(7).clamp(3, 64),
                grain_axis: u(8).min(2),
                splinter: g(9).max(1.0),
                separate: g(10).max(0.0),
                glue_strength: if p.len() > 11 { g(11).max(0.0) } else { 200.0 },
            }),
            "skeleton" => NodeKind::Skeleton(kinefx::SkeletonSettings {
                preset: u(0).min(kinefx::PRESETS.len() - 1),
                scale: if p.len() > 1 { g(1).max(1e-3) } else { 1.0 },
                joints: u(2).clamp(2, 200),
                length: g(3).max(1e-3),
            }),
            "rig_pose" => NodeKind::RigPose(kinefx::RigPose {
                joints: vec![String::new(); kinefx::POSE_SLOTS],
                values: (0..kinefx::POSE_SLOTS).map(|s| std::array::from_fn(|k| g(s * 6 + k))).collect(),
            }),
            "two_bone_ik" => NodeKind::TwoBoneIk(kinefx::IkSettings {
                goal: v3(0),
                pole: v3(3),
                use_pole: g(6) > 0.5,
                blend: if p.len() > 7 { g(7).clamp(0.0, 1.0) } else { 1.0 },
                keep_tip: g(8) > 0.5,
                ..kinefx::IkSettings::defaults()
            }),
            "procedural_motion" => NodeKind::ProceduralMotion(kinefx::MotionSettings {
                motion: u(0).min(kinefx::MOTIONS.len() - 1),
                rate: g(1),
                amplitude: g(2),
                travel: g(3),
                lag: g(4),
                axis: u(5).min(2),
            }),
            "skeleton_blend" => NodeKind::SkeletonBlend { weight: g(0).clamp(0.0, 1.0) },
            "joint_capture" => NodeKind::JointCapture(kinefx::CaptureSettings {
                influences: u(0).clamp(1, kinefx::MAX_INFLUENCES),
                falloff: g(1).max(0.1),
                visualize: g(2) > 0.5,
            }),
            "bone_deform" => NodeKind::BoneDeform,
            "rbd_bullet_solver" => NodeKind::RbdBullet(bullet::BulletSettings::from_params(p)),
            "rbd_configure" => NodeKind::RbdConfigure(bullet::ConfigureSettings::from_params(p)),
            "pack" => NodeKind::Pack(packed::PackSettings { mode: if p.is_empty() { 1 } else { u(0).min(2) }, pivot: u(1).min(1) }),
            "unpack" => NodeKind::Unpack { transfer: p.first().is_none_or(|v| *v > 0.5) },
            "crowd_solver" => NodeKind::CrowdSolver(crowd::CrowdSettings::from_params(p)),
            "muscle" => NodeKind::Muscle(muscle::MuscleSettings::from_params(p)),
            "muscle_solver" => NodeKind::MuscleSolver(muscle::JiggleSettings {
                stiffness: g(0).max(0.0),
                damping: g(1).max(0.0),
                smoothing: u(2).min(50),
                substeps: u(3).clamp(1, 16),
            }),
            "muscle_skin" => NodeKind::MuscleSkin(muscle::SkinSettings { falloff: g(0).max(1e-4), influences: u(1).clamp(1, 16) }),
            "terrain" => NodeKind::Terrain(terrain::TerrainSettings { size: [g(0).max(1e-3), g(1).max(1e-3)], resolution: u(2).clamp(2, 2048) }),
            "terrain_noise" => NodeKind::TerrainNoise(terrain::NoiseSettings {
                kind: u(0).min(terrain::NOISE_TYPES.len() - 1),
                blend: u(1).min(terrain::BLEND_MODES.len() - 1),
                amplitude: g(2),
                element_size: g(3).max(1e-3),
                octaves: u(4).clamp(1, 12),
                roughness: g(5).clamp(0.0, 1.0),
                offset: [g(6), g(7)],
                seed: u(8).min(u32::MAX as usize) as u32,
                positive: g(9) > 0.5,
                use_mask: g(10) > 0.5,
            }),
            "terrain_erode" => NodeKind::TerrainErode(terrain::ErodeSettings {
                droplets: u(0).min(5_000_000),
                erosion: g(1).clamp(0.0, 1.0),
                deposition: g(2).clamp(0.0, 1.0),
                capacity: g(3).max(0.0),
                evaporation: g(4).clamp(0.0, 1.0),
                inertia: g(5).clamp(0.0, 1.0),
                radius: u(6).clamp(1, 8),
                thermal_iterations: u(7).min(500),
                talus_angle: g(8).clamp(1.0, 89.0),
                seed: u(9).min(u32::MAX as usize) as u32,
            }),
            "terrain_terrace" => NodeKind::TerrainTerrace(terrain::TerraceSettings { step: g(0).max(1e-4), offset: g(1), sharpness: g(2).clamp(0.0, 1.0), use_mask: g(3) > 0.5 }),
            "terrain_mask" => NodeKind::TerrainMask(terrain::MaskSettings {
                source: u(0).min(terrain::MASK_SOURCES.len() - 1),
                min: g(1),
                max: g(2),
                falloff: g(3).max(0.0),
                blur: u(4).min(100),
                invert: g(5) > 0.5,
                combine: u(6).min(terrain::MASK_COMBINE.len() - 1),
                visualize: g(7) > 0.5,
            }),
            "terrain_colorize" => NodeKind::TerrainColorize(terrain::ColorizeSettings {
                sand: v3(0),
                grass: v3(3),
                rock: v3(6),
                snow: v3(9),
                sand_level: g(12),
                snow_level: g(13),
                rock_slope: g(14),
                blend: g(15).max(0.0),
            }),
            "uv_texture" => NodeKind::UvTexture(sops::UvTextureSettings {
                projection: u(0).min(sops::UV_PROJECTIONS.len() - 1),
                axis: u(1).min(2),
                scale: [g(2), g(3)],
                offset: [g(4), g(5)],
            }),
            "color" => NodeKind::Color(color::ColorSettings::from_params(p, String::new())),
            "enumerate" => NodeKind::Enumerate(enumerate::EnumerateSettings::from_params(p)),
            "attribute_blur" => NodeKind::AttributeBlur(blur::BlurSettings::from_params(p, "Cd".into())),
            "attribute_promote" => NodeKind::AttributePromote(promote::PromoteSettings::from_params(p, "Cd".into(), String::new())),
            "attribute_transfer" => NodeKind::AttributeTransfer(transfer::TransferSettings::from_params(p, "*".into())),
            "uv_transform" => NodeKind::UvTransform(sops::UvTransformSettings {
                translate: [g(0), g(1)],
                rotate: g(2),
                scale: [g(3), g(4)],
                pivot: [g(5), g(6)],
            }),
            "uv_quickshade" => NodeKind::UvQuickShade { file: String::new() },
            // "vellum_solver" in scenes saved before 0.11.
            "cloth_solver" | "vellum_solver" => {
                let kind = u(0).min(vellum::VELLUM_TYPES.len() - 1);
                let d = vellum::VellumSettings::defaults(kind);
                let or = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
                NodeKind::VellumSolver(vellum::VellumSettings {
                    kind,
                    stretch_stiffness: or(1, d.stretch_stiffness).max(1e-3),
                    bend_stiffness: or(2, d.bend_stiffness).max(1e-3),
                    shape_stiffness: or(3, d.shape_stiffness).clamp(0.0, 1.0),
                    mass: or(4, d.mass).max(1e-4),
                    thickness: or(5, d.thickness).max(1e-4),
                    gravity: or(6, d.gravity),
                    damping: or(7, d.damping).max(0.0),
                    friction: or(8, d.friction).clamp(0.0, 1.0),
                    substeps: (or(9, d.substeps as f32).max(1.0) as usize).min(100),
                    iterations: (or(10, d.iterations as f32).max(1.0) as usize).min(200),
                    ground: or(11, d.ground as u8 as f32) > 0.5,
                    self_collision: or(12, d.self_collision as u8 as f32) > 0.5,
                    pin_to_animation: or(13, d.pin_to_animation as u8 as f32) > 0.5,
                })
            }
            "sop_solver" => NodeKind::SopSolver { substeps: u(0).clamp(1, 16) },
            "sop_import" => NodeKind::LopSopImport { geometry: p.first().filter(|v| **v >= 0.0).map(|v| *v as NodeId), path: "/geo".into() },
            "usd_reference" => NodeKind::LopReference { file: String::new(), path: "/ref".into(), sublayer: g(0) != 0.0 },
            "usd_primitive" => NodeKind::LopPrimitive { shape: u(0).min(LOP_SHAPES.len() - 1), path: "/shapes/shape".into(), size: g(1).max(0.001), t: v3(2) },
            "usd_transform" => NodeKind::LopTransform { pattern: String::new(), t: v3(0), r: v3(3), s: v3(6) },
            "usd_light" => NodeKind::LopLight {
                kind: u(0).min(LOP_LIGHTS.len() - 1),
                path: "/lights/light".into(),
                position: v3(1),
                target: v3(4),
                color: v3(7),
                intensity: g(10).max(0.0),
                size: g(11).max(0.0),
            },
            "usd_camera" => NodeKind::LopCamera { path: "/cameras/cam".into(), position: v3(0), target: v3(3), focal: g(6).max(1.0), aperture: g(7).max(1.0) },
            "assign_material" => NodeKind::LopMaterial { pattern: String::new(), material: p.first().filter(|v| **v >= 0.0).map(|v| *v as NodeId) },
            "usd_prune" => NodeKind::LopPrune { pattern: String::new(), hide: g(0) != 0.0 },
            "usd_merge" => NodeKind::LopMerge,
            "usd_export" => NodeKind::LopExport { file: String::new() },
            "solver_input" => NodeKind::SolverInput { index: u(0).min(1) },
            "subnet" => NodeKind::Subnet { values: p.to_vec(), spec: assets::Spec::default() },
            "subnet_input" => NodeKind::SubnetInput { index: u(0).min(3) },
            "switch" => NodeKind::Switch { index: u(0) },
            "geometry" => NodeKind::Geometry {
                material: p.first().filter(|v| **v >= 0.0).map(|v| *v as usize),
            },
            "diffuse" | "metal" | "glass" | "emissive" => NodeKind::Material {
                kind: MATERIAL_NAMES.iter().position(|n| *n == name).unwrap_or(0),
                color: [g(1).clamp(0.0, 1000.0), g(2).clamp(0.0, 1000.0), g(3).clamp(0.0, 1000.0)],
                roughness: g(4).clamp(0.0, 1.0),
                value: g(5).max(0.0),
            },
            "principled_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Principled, p)),
            "toon_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Toon, p)),
            "hair_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Hair, p)),
            "pyro_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Pyro, p)),
            "material_builder" => NodeKind::MaterialBuilder,
            "shader_node" => NodeKind::ShaderNode(shading::ShaderNode::from_params(p)),
            "cop_network" => NodeKind::CopNetwork,
            "cop" => NodeKind::Cop(cops::CopNode::from_params(p)),
            "pop" => NodeKind::Pop(pop::PopNode::from_params(p)),
            "popnet" => NodeKind::PopNet { substeps: u(0).clamp(1, 16) },
            "top_network" => NodeKind::TopNetwork,
            "top" => NodeKind::Top(tops::TopNode::from_params(p)),
            _ => return None,
        })
    }
}

// MoonRay's output made readable: without its terminal colour codes, and saying plainly
// when it couldn't find its shaders.
pub(crate) fn moonray_message(log: &str) -> String {
    let mut plain = String::with_capacity(log.len());
    let mut chars = log.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... letter
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        plain.push(c);
    }
    if plain.contains("the DSO could not be found") {
        let class = plain.split("global variable '").nth(1).and_then(|r| r.split('\'').next()).unwrap_or("a shader");
        return format!("MoonRay couldn't find its {class} shader: its rdl2dso folder isn't beside it (set RDL2_DSO_PATH to that folder). {}", plain.trim());
    }
    plain
}
