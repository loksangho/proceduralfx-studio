// Network editor helpers: moved out of main.rs.

use crate::*;

pub(crate) const NODE_W: f32 = 140.0;
pub(crate) const NODE_H: f32 = 40.0;
// Scene files are .pfs (ProceduralFX Studio); .mhip is the old name, still opened.
pub(crate) const SCENE_EXTENSION: &str = "pfs";
pub(crate) const SCENE_FILE: &str = "scene.pfs";

// A live sprite sheet: after an export with "Keep updating" on, the same files are
// exported again whenever the scene, camera or sheet settings change, once they've been
// still for a moment, so a game watching them (bevy_procedural_fx with Bevy's
// file_watcher, or embedded_watcher for embedded assets) updates as you work.
pub(crate) struct LiveSheet {
    pub(crate) path: PathBuf,
    // What the files on disk (or the export under way) show.
    pub(crate) exported: u64,
    // A change not exported yet, and when it was first seen.
    pub(crate) pending: Option<(u64, Instant)>,
}

// How long the scene has to stay unchanged before a live sheet is exported again.
// Whether the bake window has an object to bake.
pub(crate) fn chosen_ok(objects: &[(NodeId, String)], default: Option<NodeId>, chosen: Option<NodeId>) -> bool {
    chosen.filter(|o| objects.iter().any(|(id, _)| id == o)).or(default).is_some()
}

// The Export Vertex Animation window's choices.
#[derive(Default)]
pub(crate) struct VatWindow {
    pub(crate) open: bool,
    // The geometry object to bake (None: the one being viewed, or the first).
    pub(crate) object: Option<NodeId>,
    pub(crate) every: u32,
    // None: the kind that suits the motion.
    pub(crate) mode: Option<vat::Mode>,
    pub(crate) godot: bool,
    pub(crate) unity: bool,
}

pub(crate) enum VatMessage {
    Progress(usize, usize),
    Done(Result<String, String>),
}

pub(crate) const LIVE_SHEET_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

// A scene's content regardless of line order (some parts are written from hash maps).
// The scene file format this version writes, on the first line ("pfs 2"). Bump it when
// a change means an older version would misread a file (not for new node kinds or
// parameters added at the end, which older versions skip or default), so older
// versions refuse such a file instead of opening part of it.
pub(crate) const SCENE_FORMAT: u32 = 2;

// A scene file's format number: "pfs N", or "mhip N" from before the rename.
pub(crate) fn scene_format(text: &str) -> Option<u32> {
    match text.lines().next()?.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["pfs" | "mhip", n] => n.parse().ok(),
        _ => None,
    }
}

// Why a scene can't be opened, if it can't.
pub(crate) fn scene_problem(text: &str) -> Option<String> {
    match scene_format(text) {
        Some(n) if n > SCENE_FORMAT => Some(format!(
            "it was saved by a newer version of ProceduralFX Studio (scene format {n}; this version reads up to {SCENE_FORMAT}). Update the app to open it."
        )),
        _ => None,
    }
}

// How many of a scene's nodes didn't load: kinds this version doesn't know, say, from
// a newer version, or lines damaged in the file.
pub(crate) fn unread_nodes(text: &str, graph: &Graph) -> usize {
    let written = text.lines().filter(|l| l.starts_with("node ") && l.split_whitespace().count() >= 5).count();
    written.saturating_sub(graph.nodes.len())
}

pub(crate) fn scene_fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lines.hash(&mut hasher);
    hasher.finish()
}
pub(crate) const LEGACY_SCENE_FILE: &str = "scene.mhip";

// The timeline's playback range and frame rate, saved with the scene as one line after the
// graph. Older versions of the app skip the line, so files still open in them.
pub(crate) fn timeline_line(start: f32, end: f32, fps: f32) -> String {
    format!("timeline {start} {end} {fps}\n")
}

// Which preset a scene was started from (for its simple controls), by name, in hex.
pub(crate) fn preset_line(name: &str) -> String {
    let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
    format!("preset {hex}\n")
}

pub(crate) fn parse_preset(text: &str) -> Option<&'static presets::Preset> {
    text.lines().rev().find_map(|line| {
        let hex = line.strip_prefix("preset ")?.trim();
        let bytes: Option<Vec<u8>> = (0..hex.len() / 2).map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
        presets::find(&String::from_utf8(bytes?).ok()?)
    })
}

pub(crate) fn parse_timeline(text: &str) -> Option<(f32, f32, f32)> {
    text.lines().rev().find_map(|line| match line.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["timeline", start, end, fps] => {
            let (start, end, fps) = (start.parse::<f32>().ok()?, end.parse::<f32>().ok()?, fps.parse::<f32>().ok()?);
            (start.is_finite() && end.is_finite() && fps.is_finite()).then(|| (start, end.max(start), fps.clamp(1.0, 120.0)))
        }
        _ => None,
    })
}

// Returns the MoonRay class name and its RDLA definition for a MAT node's settings.
pub(crate) fn top_state_colour(state: tops::State) -> egui::Color32 {
    match state {
        tops::State::Waiting => egui::Color32::from_gray(150),
        tops::State::Running => egui::Color32::from_rgb(250, 200, 60),
        tops::State::Done => egui::Color32::from_rgb(90, 210, 110),
        tops::State::Failed => egui::Color32::from_rgb(235, 80, 70),
    }
}

// A joint picker: the input skeleton's joint names, or a typed name.
pub(crate) fn joint_menu(ui: &mut egui::Ui, salt: impl std::hash::Hash, label: &str, joint: &mut String, names: &[String], optional: bool) -> bool {
    let mut c = false;
    ui.horizontal(|ui| {
        ui.label(label);
        let shown = if joint.trim().is_empty() { "(none)".to_owned() } else { joint.clone() };
        egui::ComboBox::from_id_salt(salt).selected_text(shown).height(400.0).show_ui(ui, |ui| {
            if optional && ui.selectable_label(joint.trim().is_empty(), "(none)").clicked() {
                joint.clear();
                c = true;
            }
            for name in names {
                if ui.selectable_label(joint == name, name).clicked() {
                    *joint = name.clone();
                    c = true;
                }
            }
        });
        c |= ui.add(egui::TextEdit::singleline(joint).desired_width(90.0)).changed();
    });
    c
}

// One parameter of a shader table: a slider, colour swatch or checkbox.
pub(crate) fn shader_param_ui(ui: &mut egui::Ui, param: &shading::Param, slot: &mut [f32]) -> bool {
    match param.kind {
        shading::Kind::Float(min, max) => {
            let log = min >= 0.0 && max / min.max(0.01) >= 1000.0;
            pfield(ui, pslider(&mut slot[0], min..=max).logarithmic(log).text(param.label)).changed()
        }
        shading::Kind::Color => {
            ui.horizontal(|ui| {
                let mut rgb = [slot[0], slot[1], slot[2]];
                let changed = ui.color_edit_button_rgb(&mut rgb).changed();
                // Only on an edit: the button's colour conversions nudge values otherwise.
                if changed {
                    slot.copy_from_slice(&rgb);
                }
                ui.label(param.label);
                changed
            })
            .inner
        }
        shading::Kind::Toggle => {
            let mut on = slot[0] > 0.5;
            let changed = ui.checkbox(&mut on, param.label).changed();
            slot[0] = on as u8 as f32;
            changed
        }
        shading::Kind::Vector(min, max) => {
            ui.horizontal(|ui| {
                let mut changed = false;
                for value in slot.iter_mut().take(3) {
                    changed |= pfield(ui, pdrag(value).range(min..=max).speed(0.05)).changed();
                }
                ui.label(param.label);
                changed
            })
            .inner
        }
    }
}

// Preset menu, textures and the grouped parameters of a shader.
pub(crate) fn shader_values_ui(ui: &mut egui::Ui, values: &mut shading::Values) -> bool {
    let mut c = false;
    let presets = values.table.presets();
    let mut preset = values.preset;
    egui::ComboBox::from_label("Preset").selected_text(presets[preset.min(presets.len() - 1)].0).height(400.0).show_ui(ui, |ui| {
        for (i, (name, _)) in presets.iter().enumerate() {
            ui.selectable_value(&mut preset, i, *name);
        }
    });
    if preset != values.preset {
        values.apply_preset(preset);
        c = true;
    }
    if values.table == shading::Table::Principled {
        for (label, text) in shading::PRINCIPLED_TEXTURES.iter().zip(values.textures.iter_mut()) {
            ui.horizontal(|ui| {
                ui.label(*label);
                c |= ui.add(egui::TextEdit::singleline(text).desired_width(160.0)).changed();
                if ui.button("…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "exr", "tif", "tiff", "hdr", "tx"]).pick_file() {
                        *text = path.to_string_lossy().into_owned();
                        c = true;
                    }
                }
            });
        }
    }
    let mut at = 0;
    let mut group = "";
    for param in values.table.params() {
        if param.group != group {
            group = param.group;
            ui.separator();
            ui.label(egui::RichText::new(group).strong());
        }
        let width = param.width();
        c |= shader_param_ui(ui, param, &mut values.values[at..at + width]);
        at += width;
    }
    c
}

// Add-menu search: every word typed must appear in the node's name, menu name or category.
pub(crate) fn node_matches(kind: &NodeKind, category: &str, query: &str) -> bool {
    let haystack = format!("{} {} {}", kind.name(), kind.menu_name(), category).to_lowercase().replace('_', " ");
    query.to_lowercase().split_whitespace().all(|word| haystack.contains(word) || haystack.replace(' ', "").contains(word))
}

// Undoes an object transform (scale, then rotate, then translate) on a mesh's points.
pub(crate) fn inverse_xform(x: &ObjectXform, mut mesh: Mesh) -> Mesh {
    if x.is_identity() {
        return mesh;
    }
    let s = x.s.map(|v| if v.abs() < 1e-6 { 1e-6 } else { v });
    for v in mesh.vertices.iter_mut() {
        let p = v.sub(Vec3::from_arr(x.t));
        let p = p.rotate_deg([0.0, 0.0, -x.r[2]]).rotate_deg([0.0, -x.r[1], 0.0]).rotate_deg([-x.r[0], 0.0, 0.0]);
        *v = Vec3::new(p.x / s[0], p.y / s[1], p.z / s[2]);
    }
    if s[0] * s[1] * s[2] < 0.0 {
        mesh.indices.iter_mut().for_each(|t| t.swap(1, 2));
    }
    mesh.point_attributes.remove("N");
    mesh
}

// Writes the documentation page next to the app's settings and opens it in the browser.
// What an administrator can run once, when this account can't install WSL itself.
pub(crate) const WSL_ADMIN_COMMAND: &str = "wsl --install --no-distribution";

// Windows rendering set-up, sending each line the helper prints. It checks first (no
// administrator needed); if WSL is missing it stops with "[pfx-render] needs-admin"
// unless `install` is set, in which case it installs (Windows asks an administrator).
// Then it downloads the MoonRay image.
pub(crate) fn start_render_setup(helper: PathBuf, install: bool) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Runs one helper step, passing on what it prints; its exit code.
        let step = |arg: &str| -> Result<Option<i32>, String> {
            let mut command = Command::new(&helper);
            platform::no_window(&mut command);
            let mut child = command
                .arg(arg)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("couldn't run {}: {e}", helper.display()))?;
            if let Some(err) = child.stderr.take() {
                use std::io::BufRead;
                for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            }
            child.wait().map(|s| s.code()).map_err(|e| e.to_string())
        };
        let fail = |why: String| {
            let _ = tx.send(format!("[pfx-render] error set-up stopped: {why}"));
        };
        let first = if install { "--install" } else { "--check" };
        let _ = tx.send(format!("[pfx-render] status {}", if install { "installing WSL containers (Windows asks for an administrator)" } else { "checking for WSL containers" }));
        match step(first) {
            Ok(Some(0)) => {}
            // Missing (3), or the administrator prompt was declined (5).
            Ok(Some(3)) | Ok(Some(5)) => {
                let _ = tx.send("[pfx-render] needs-admin".into());
                return;
            }
            Ok(code) => return fail(setup_failure(code)),
            Err(e) => return fail(e),
        }
        let _ = tx.send("[pfx-render] status downloading MoonRay if it isn't here yet".into());
        match step("--pull") {
            Ok(Some(0)) => {
                let _ = tx.send("[pfx-render] status rendering on Windows is ready".into());
            }
            Ok(code) => fail(setup_failure(code)),
            Err(e) => fail(e),
        }
    });
    rx
}

// What the helper's exit code means, when it stopped without explaining itself.
pub(crate) fn setup_failure(code: Option<i32>) -> String {
    match code {
        Some(3) => "WSL containers aren't installed yet (see above)".into(),
        Some(5) => format!("installing needs administrator approval; an administrator can run `{WSL_ADMIN_COMMAND}` once"),
        Some(4) => "see the message above".into(),
        // Windows' own codes for a program that couldn't start.
        Some(c) if c as u32 == 0xC000_0135 => "pfx-render.exe couldn't load wslcsdk.dll: keep it in the same folder".into(),
        Some(c) if c as u32 == 0xC000_007B => "pfx-render.exe or wslcsdk.dll is for the wrong kind of Windows (needs 64-bit x64)".into(),
        Some(c) if c < 0 => format!("pfx-render.exe crashed (code 0x{:08X})", c as u32),
        Some(c) => format!("pfx-render.exe exited with code {c}"),
        None => "pfx-render.exe was stopped".into(),
    }
}

pub(crate) fn open_documentation() -> Result<String, String> {
    open_documentation_at(None)
}

// Writes the documentation page and opens it, at `anchor` when given (e.g. a VEX
// function's card). Browsers drop a #fragment passed with a file path, so a tiny
// page that redirects to it is opened instead.
pub(crate) fn open_documentation_at(anchor: Option<&str>) -> Result<String, String> {
    let dir = app_config_dir().ok_or("no home folder")?.join("docs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("index.html");
    std::fs::write(&path, docs::html(env!("CARGO_PKG_VERSION"))).map_err(|e| e.to_string())?;
    let target = match anchor {
        Some(anchor) => {
            let anchor: String = anchor.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
            let jump = dir.join("open.html");
            let page = format!(
                "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0; url=index.html#{anchor}\"><script>location.replace('index.html#{anchor}')</script>"
            );
            std::fs::write(&jump, page).map_err(|e| e.to_string())?;
            jump
        }
        None => path.clone(),
    };
    platform::open(&target.to_string_lossy()).map_err(|e| format!("couldn't open the documentation: {e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

// Primitive attribute with each primitive's MAT material: node id + 1, or 0 for the object's
// own material (so primitives merged in without one keep it).
pub(crate) const PRIM_MATERIAL: &str = "prim:material";
// On displayed geometry only: each primitive's Toon Shader (node id + 1; 0: none), so the
// real-time renderer cel-shades and outlines it.
pub(crate) const TOON_ATTR: &str = "prim:__toon";
// Also on displayed geometry only: each primitive's material opacity and whether it
// multiplies by the geometry's Alpha (Principled "Use point alpha"), for the real-time renderer.
pub(crate) const OPACITY_ATTR: &str = "prim:__opacity";

// The Material SOP: later slots (and later Material nodes) override earlier ones.
pub(crate) fn material_assign(input: &Mesh, slots: &[(String, Option<NodeId>)]) -> Result<Mesh, String> {
    let n = input.indices.len();
    let mut ids = input.point_attributes.get(PRIM_MATERIAL).filter(|v| v.len() == n).cloned().unwrap_or_else(|| vec![0.0; n]);
    for (group, material) in slots {
        let value = material.map_or(0.0, |m| m as f32 + 1.0);
        // Primitives, like Houdini's Material SOP (numbers are primitive numbers); a point
        // group name still works, covering the primitives made of its points.
        let mask = groups::prim_mask(input, &groups::NodeGroup { pattern: group.clone(), kind: 2 })
            .or_else(|_| groups::prim_mask(input, &groups::NodeGroup { pattern: group.clone(), kind: 0 }))?;
        for (t, id) in ids.iter_mut().enumerate() {
            if mask.as_ref().is_none_or(|m| m[t]) {
                *id = value;
            }
        }
    }
    let mut out = input.clone();
    out.point_attributes.insert(PRIM_MATERIAL.into(), ids);
    Ok(out)
}

// Splits `mesh` into one part per material: primitives the Material SOP assigned, and the
// rest with `object` (the Geometry's material). Curves and points stay with the object's.
pub(crate) fn split_by_material(mesh: Mesh, object: Option<NodeId>, is_material: impl Fn(NodeId) -> bool) -> Vec<(Mesh, Option<NodeId>)> {
    let Some(ids) = mesh.point_attributes.get(PRIM_MATERIAL).filter(|v| v.len() == mesh.indices.len() && !v.is_empty()).cloned() else {
        return vec![(mesh, object)];
    };
    let effective: Vec<Option<NodeId>> = ids.iter().map(|v| if *v >= 0.5 { Some((*v - 1.0).round() as usize).filter(|m| is_material(*m)) } else { None }.or(object)).collect();
    let mut materials: Vec<Option<NodeId>> = effective.clone();
    // Curves (hair) and render points travel with the object's material, even when every
    // primitive has another one.
    if groom::has_curves(&mesh) || mesh.point_attributes.contains_key(whitewater::RENDER_POINTS) {
        materials.push(object);
    }
    materials.sort_unstable();
    materials.dedup();
    if materials.len() == 1 {
        return vec![(mesh, materials[0])];
    }
    materials
        .into_iter()
        .map(|m| {
            let mask: Vec<bool> = effective.iter().map(|e| *e == m).collect();
            let mut part = groups::only_prims(&mesh, &mask);
            // Curves and loose points go with the object's own material only.
            if m != object {
                part.point_attributes.remove(groom::CURVEU);
                part.point_attributes.remove(whitewater::RENDER_POINTS);
            }
            (part, m)
        })
        .collect()
}

// Points the renderer draws as spheres (Whitewater output), not left out like other point clouds.
// The points a polygon or curve uses (the rest are loose points).
pub(crate) fn used_points(mesh: &Mesh, curves: &[std::ops::Range<usize>]) -> Vec<bool> {
    let mut used = vec![false; mesh.vertices.len()];
    for &p in mesh.indices.iter().flatten().chain(curves.iter().flat_map(|r| r.clone()).collect::<Vec<_>>().iter()) {
        if let Some(u) = used.get_mut(p) {
            *u = true;
        }
    }
    used
}

pub(crate) fn renders_points(mesh: &Mesh) -> bool {
    mesh.indices.is_empty() && !mesh.vertices.is_empty() && mesh.point_attributes.get(whitewater::RENDER_POINTS).is_some_and(|v| v.first() == Some(&1.0))
}

pub(crate) fn rdla_material(name: &str, kind: usize, color: [f32; 3], roughness: f32, value: f32) -> (&'static str, String) {
    let rgb = |c: [f32; 3]| format!("Rgb({:.4}, {:.4}, {:.4})", c[0], c[1], c[2]);
    let (class, attrs) = match kind {
        1 => (
            "DwaMetalMaterial",
            format!("    [\"metallic_color\"] = {},\n    [\"roughness\"] = {:.4},\n", rgb(color), roughness),
        ),
        2 => (
            "DwaSolidDielectricMaterial",
            format!(
                "    [\"albedo\"] = {},\n    [\"roughness\"] = {:.4},\n    [\"refractive_index\"] = {:.4},\n",
                rgb(color),
                roughness,
                value.max(1.0),
            ),
        ),
        3 => (
            "DwaEmissiveMaterial",
            format!("    [\"emission\"] = {},\n", rgb([color[0] * value, color[1] * value, color[2] * value])),
        ),
        _ => (
            "DwaBaseMaterial",
            format!(
                "    [\"albedo\"] = {},\n    [\"roughness\"] = {:.4},\n    [\"specular\"] = {:.4},\n",
                rgb(color),
                roughness,
                value.clamp(0.0, 1.0),
            ),
        ),
    };
    (class, format!("{class}(\"{name}\") {{\n{attrs}}}\n"))
}
// Viewport light intensity -> MoonRay intensity, measured against MoonRay so a render
// shown through the sRGB display transform looks close to the viewport:
// a distant light of intensity 1 head-on renders at the albedo, and a sphere light of
// intensity 1 gives albedo · 251 / distance², which the viewport's falloff matches below.
pub(crate) const DISTANT_LIGHT_SCALE: f32 = 1.0;
pub(crate) const POINT_LIGHT_FALLOFF: f32 = 18.0;
pub(crate) const POINT_LIGHT_SCALE: f32 = POINT_LIGHT_FALLOFF / 251.0;

pub(crate) fn node_rect(pos: egui::Pos2, origin: egui::Vec2, zoom: f32) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(origin.x + pos.x * zoom, origin.y + pos.y * zoom),
        egui::vec2(NODE_W, NODE_H) * zoom,
    )
}
pub(crate) fn in_port(rect: egui::Rect, slot: usize, n: usize) -> egui::Pos2 {
    let x = rect.left() + rect.width() * (slot as f32 + 1.0) / (n as f32 + 1.0);
    egui::pos2(x, rect.top())
}
pub(crate) fn out_port(rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(rect.center().x, rect.bottom())
}
// Output `slot` of `n` along the bottom (one output: the centre).
pub(crate) fn out_port_n(rect: egui::Rect, slot: usize, n: usize) -> egui::Pos2 {
    if n <= 1 {
        return out_port(rect);
    }
    egui::pos2(rect.left() + rect.width() * (slot as f32 + 1.0) / (n as f32 + 1.0), rect.bottom())
}
pub(crate) fn draw_wire(p: &egui::Painter, a: egui::Pos2, b: egui::Pos2, color: egui::Color32) {
    let d = ((b.y - a.y).abs() * 0.5).max(30.0);
    p.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [a, a + egui::vec2(0.0, d), b - egui::vec2(0.0, d), b],
        false,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(2.0_f32, color),
    ));
}
pub(crate) fn vec3_ui(ui: &mut egui::Ui, label: &str, v: &mut [f32; 3], speed: f64) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= group_label(ui, label, speed, v);
        for x in v.iter_mut() {
            changed |= pfield(ui, pdrag(x).speed(speed)).changed();
        }
    });
    changed
}

// A row's label that changes all of the row's values at once: drag the word "Scale" to
// move x, y and z together, each by the same amount (Shift for finer steps).
pub(crate) fn group_label(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, speed: f64, values: &mut [f32]) -> bool {
    let response = ui.add(egui::Label::new(label).sense(egui::Sense::drag())).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let response = if values.len() > 1 { response.on_hover_text("Drag to change them all at once") } else { response };
    if !response.dragged() {
        return false;
    }
    let fine = if ui.input(|i| i.modifiers.shift) { 0.1 } else { 1.0 };
    let delta = (response.drag_delta().x as f64 * speed * fine) as f32;
    if delta == 0.0 {
        return false;
    }
    values.iter_mut().for_each(|v| *v += delta);
    true
}

pub(crate) enum Action {
    Add(NodeKind, egui::Pos2),
    AddBelow(NodeKind, NodeId),
    Display(NodeId),
    Enter(NodeId),
    Bypass(NodeId),
    Duplicate(NodeId),
    DuplicateMany(Vec<NodeId>),
    Delete(NodeId),
}
