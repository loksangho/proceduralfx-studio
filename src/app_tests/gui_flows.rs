// End-to-end tests of the whole app: the real UI, frame by frame, driven like a user
// (moving the mouse, clicking menus and buttons by their labels, typing, pressing keys)
// through egui_kittest. What they write goes to a temporary settings folder (see
// app_config_dir), not the user's.
use crate::*;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

fn harness(app: ProceduralFxStudioApp) -> Harness<'static, ProceduralFxStudioApp> {
    let mut harness = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_state(|ctx, app: &mut ProceduralFxStudioApp| app.frame(ctx), app);
    harness.run_steps(4);
    harness
}

// An app showing an empty geometry network, not playing.
fn empty_network() -> ProceduralFxStudioApp {
    let mut app = ProceduralFxStudioApp::default();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnext 1\n".into());
    app.current_network = Some(0);
    app.selected = None;
    app.animate = false;
    app
}

fn pointer(h: &mut Harness<ProceduralFxStudioApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(2);
}

fn press(h: &Harness<ProceduralFxStudioApp>, at: egui::Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
}

fn button(h: &mut Harness<ProceduralFxStudioApp>, at: egui::Pos2, pressed: bool) {
    press(h, at, pressed);
    h.run_steps(2);
}

fn click_at(h: &mut Harness<ProceduralFxStudioApp>, at: egui::Pos2) {
    pointer(h, at);
    button(h, at, true);
    button(h, at, false);
}

// Drags with the left button from `from` to `to`, in steps, as a hand would.
fn drag(h: &mut Harness<ProceduralFxStudioApp>, from: egui::Pos2, to: egui::Pos2) {
    pointer(h, from);
    button(h, from, true);
    for k in 1..=8 {
        pointer(h, from + (to - from) * (k as f32 / 8.0));
    }
    button(h, to, false);
}

// Tab over the network at `at`, type the node's name, Enter: returns the node added.
fn add_with_tab(h: &mut Harness<ProceduralFxStudioApp>, at: egui::Pos2, name: &str) -> NodeId {
    let before: std::collections::HashSet<NodeId> = h.state().graph.nodes.keys().copied().collect();
    pointer(h, at);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    h.event(egui::Event::Text(name.into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let added: Vec<NodeId> = h.state().graph.nodes.keys().copied().filter(|id| !before.contains(id)).collect();
    assert_eq!(added.len(), 1, "Tab, {name}, Enter adds one node: {added:?}");
    added[0]
}

// A point in the network canvas, as fractions of its width and height.
// Steps until a background cook has finished (it can take a while on a busy CI machine),
// for at most 30 seconds.
fn settle(h: &mut Harness<ProceduralFxStudioApp>) {
    h.run_steps(3);
    for _ in 0..3000 {
        if h.state().simulation_rx.is_none() && !h.state().status.ends_with("press Escape to cancel") {
            break;
        }
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(2);
}

fn in_canvas(h: &Harness<ProceduralFxStudioApp>, x: f32, y: f32) -> egui::Pos2 {
    let c = h.state().network_canvas;
    egui::pos2(c.left() + c.width() * x, c.top() + c.height() * y)
}

// Where node `id` is on screen.
fn node_on_screen(h: &Harness<ProceduralFxStudioApp>, id: NodeId) -> egui::Rect {
    let app = h.state();
    node_rect(app.graph.nodes[&id].pos, app.network_canvas.min.to_vec2() + app.graph_pan, app.graph_zoom)
}

#[test]
fn a_network_is_built_from_the_tab_menu_and_wired_by_dragging() {
    let mut h = harness(empty_network());
    let canvas = h.state().network_canvas;
    assert!(canvas.width() > 200.0 && canvas.height() > 150.0, "the network editor shows: {canvas:?}");

    let at = in_canvas(&h, 0.15, 0.15);
    let grid = add_with_tab(&mut h, at, "grid");
    assert_eq!(h.state().graph.nodes[&grid].kind.name(), "grid");
    assert_eq!(h.state().graph.nodes[&grid].parent, Some(0), "inside the network being viewed");
    // Click empty canvas so the next node isn't wired below this one automatically.
    let empty = in_canvas(&h, 0.8, 0.9);
    click_at(&mut h, empty);
    assert_eq!(h.state().selected, None, "clicking empty space deselects");
    let at = in_canvas(&h, 0.3, 0.45);
    let noise = add_with_tab(&mut h, at, "noise");
    assert!(!h.state().graph.links.iter().any(|l| l.to == noise), "not wired yet");

    // Drag from the grid's output to the noise's input.
    let (from, to) = (out_port(node_on_screen(&h, grid)), in_port(node_on_screen(&h, noise), 0, h.state().graph.input_slots(noise)));
    drag(&mut h, from, to);
    let wired = |h: &Harness<ProceduralFxStudioApp>| h.state().graph.links.iter().any(|l| l.from == grid && l.to == noise && l.slot == 0);
    assert!(wired(&h), "dragging between the ports wires them: {:?}", h.state().graph.links.iter().map(|l| (l.from, l.to, l.slot)).collect::<Vec<_>>());

    // The toolbar's undo takes the wire away, and redo puts it back.
    h.get_by_label("↶").click();
    h.run_steps(3);
    assert!(!wired(&h), "undone");
    h.get_by_label("↷").click();
    h.run_steps(3);
    assert!(wired(&h), "redone");

    // Double-clicking the noise shows it: the viewport cooks the grid through it.
    let centre = node_on_screen(&h, noise).center();
    pointer(&mut h, centre);
    // A moment after the last click, or egui counts a triple-click.
    h.run_steps(60);
    // One frame each, so the two clicks come close enough together.
    for pressed in [true, false, true, false] {
        press(&h, centre, pressed);
        h.step();
    }
    settle(&mut h);
    assert_eq!(h.state().graph.display_of(0), Some(noise), "the display flag moved to the noise");
    assert!(!h.state().current_mesh.vertices.is_empty(), "and the viewport shows it");
}

#[test]
fn a_preset_opens_from_the_file_menu() {
    let mut h = harness(empty_network());
    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label_contains("New from Preset").click();
    h.run_steps(2);
    h.get_by_label("Smoke (loop)").click_accesskit();
    h.run_steps(6);
    // Its first frame cooks in the background; then the preset's message comes back.
    settle(&mut h);
    let app = h.state();
    assert!(app.status.starts_with("New scene from the Smoke preset"), "{}", app.status);
    assert_eq!(app.graph.nodes.values().filter(|n| n.name == "sheet_camera").count(), 1, "the preset's scene, with its camera");
    assert_eq!((app.playback_start, app.playback_end), presets::find("Smoke").unwrap().range);
}


// A long status message stays beside the timeline's buttons, cut short, never over them.
#[test]
fn a_long_status_never_covers_the_play_buttons() {
    let mut h = harness(empty_network());
    h.state_mut().status = format!("Sprite sheet: saved to {} and every file each engine needs beside it", "/a/very/long/folder/name".repeat(12));
    h.run_steps(3);
    let status = h.get_by_label_contains("Sprite sheet: saved to").rect();
    for button in ["⏮ Start", "▶ Play", "⏭ End", "🎞 Flipbook"] {
        let rect = h.get_by_label(button).rect();
        assert!(!status.intersects(rect), "the status ({status:?}) covers {button} ({rect:?})");
    }
    let row = h.get_by_label("⏮ Start").rect();
    assert!((status.center().y - row.center().y).abs() < row.height(), "still on the buttons' row");
}

// A geometry network holding one Grid (size 4), selected and shown.
fn one_grid() -> ProceduralFxStudioApp {
    let mut app = empty_network();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nparent 1 0\ndisplay 0 1\nnext 2\n".into());
    app.current_network = Some(0);
    app.selected = Some(1);
    app
}

fn width_of(mesh: &Mesh) -> f32 {
    let xs = mesh.vertices.iter().map(|v| v.x);
    xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
}

// Typing a new value into a parameter changes the node, and the viewport cooks it.
#[test]
fn typing_a_parameter_changes_the_node_and_the_viewport() {
    let mut h = harness(one_grid());
    assert!((width_of(&h.state().current_mesh) - 4.0).abs() < 1e-3, "a 4-wide grid shows");
    let field = h.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Size");
    field.click();
    h.run_steps(2);
    // Editing: replace what's there with 6.5, and press Enter.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text("6.5".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    let app = h.state();
    assert!(matches!(app.graph.nodes[&1].kind, NodeKind::Grid { size, .. } if (size - 6.5).abs() < 1e-4), "the grid's size is 6.5");
    assert!((width_of(&app.current_mesh) - 6.5).abs() < 1e-3, "and the viewport shows it 6.5 wide: {}", width_of(&app.current_mesh));
    assert!(app.has_unsaved_changes(), "an unsaved change");
}

// Save, change something, then open the saved scene again from File → Open Recent.
#[test]
fn a_scene_saves_and_reopens_from_open_recent() {
    let dir = std::env::temp_dir().join(format!("pfx_gui_save_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("terrain.pfs");
    let mut app = one_grid();
    app.scene_path = Some(path.clone());
    app.recent_scenes = vec![path.clone()];
    let mut h = harness(app);
    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label_contains("Save Ctrl").click_accesskit();
    h.run_steps(3);
    let saved = std::fs::read_to_string(&path).expect("File → Save wrote the scene");
    assert!(saved.contains(" grid 4 8"), "with the grid in it");
    assert!(!h.state().has_unsaved_changes());

    // A change: a second node from the Tab menu.
    let at = in_canvas(&h, 0.4, 0.5);
    add_with_tab(&mut h, at, "box");
    assert_eq!(h.state().graph.nodes.len(), 3);

    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label_contains("Open Recent").click();
    h.run_steps(2);
    h.get_by_label("terrain.pfs").click_accesskit();
    settle(&mut h);
    let app = h.state();
    assert_eq!(app.graph.nodes.len(), 2, "back to what was saved");
    assert!(app.status.starts_with("Loaded "), "{}", app.status);
    assert!(!app.has_unsaved_changes());
    let _ = std::fs::remove_dir_all(&dir);
}

// After a crash, the prompt's Recover brings the autosaved scene back as unsaved changes;
// Discard deletes it.
#[test]
fn the_recovery_prompt_recovers_or_discards_an_autosave() {
    for choice in ["Recover", "Discard"] {
        let dir = recovery_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let autosave = dir.join(format!("gui-{choice}.pfs"));
        std::fs::write(&autosave, "pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj torus 1 0.25 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n").unwrap();
        let mut app = empty_network();
        app.recovery_offer = Some(recovery::Leftover { id: format!("gui-{choice}"), autosave: Some(autosave.clone()), scene_path: None, when: None, crash_report: None });
        let mut h = harness(app);
        assert!(h.query_by_label_contains("didn't close properly").is_some(), "the prompt shows");
        h.get_by_label(choice).click();
        h.run_steps(4);
        let app = h.state();
        assert!(app.recovery_offer.is_none(), "the prompt is gone");
        let has_torus = app.graph.nodes.values().any(|n| n.kind.name() == "torus");
        if choice == "Recover" {
            assert!(has_torus && app.has_unsaved_changes(), "the autosave is back, unsaved");
        } else {
            assert!(!has_torus && !autosave.exists(), "the autosave is gone");
        }
        let _ = std::fs::remove_file(&autosave);
    }
}

// A stand-in for the server: answers one POST with 201 and hands over its body.
pub(crate) fn one_report_server() -> (String, std::sync::mpsc::Receiver<serde_json::Value>) {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let (mut line, mut length, mut path) = (String::new(), 0usize, String::new());
        while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
            if path.is_empty() {
                path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
            line.clear();
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let _ = (&stream).write_all(b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}");
        let mut json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        json["path"] = path.into();
        let _ = tx.send(json);
    });
    (url, rx)
}

// After a crash, the prompt shows exactly what would be sent (home folder as ~), takes a
// note, and sends only when asked.
#[test]
fn a_crash_report_is_sent_only_when_asked_and_as_shown() {
    let dir = recovery_dir().unwrap().join("crashes");
    std::fs::create_dir_all(&dir).unwrap();
    let home = recovery::home_folder().expect("a home folder");
    let report_path = dir.join("crash-gui-test.txt");
    std::fs::write(&report_path, format!("ProceduralFX Studio crash report\nVersion: 0.22.0\nPanic: couldn't read {home}/Desktop/scene.pfs\n")).unwrap();
    let (server, received) = one_report_server();
    let mut app = empty_network();
    app.crash_report_server = Some(server);
    app.recovery_offer = Some(recovery::Leftover { id: "gui-crash".into(), autosave: None, scene_path: None, when: None, crash_report: Some(report_path.clone()) });
    let mut h = harness(app);
    assert!(h.query_by_label_contains("nothing has been sent").is_some());
    h.get_by_label("What would be sent").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("~/Desktop/scene.pfs").is_some(), "the preview shows the home folder as ~");
    assert!(h.query_by_label_contains(&format!("{home}/Desktop")).is_none(), "and not the real one");
    // A note; Enter while typing it doesn't close the prompt.
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).click();
    h.run_steps(2);
    h.event(egui::Event::Text("baking a splash".into()));
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().recovery_offer.is_some(), "still open");
    assert!(received.try_recv().is_err(), "nothing sent yet");
    h.get_by_label("Send the report").click();
    let mut sent = None;
    for _ in 0..200 {
        h.step();
        if let Ok(body) = received.try_recv() {
            sent = Some(body);
        }
        if h.query_by_label_contains("you sent it").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let sent = sent.unwrap_or_else(|| received.recv_timeout(std::time::Duration::from_secs(5)).expect("the report reached the server"));
    assert_eq!(sent["path"], "/v1/crash-reports");
    assert!(sent["report"].as_str().unwrap().contains("~/Desktop/scene.pfs") && !sent["report"].as_str().unwrap().contains(&format!("{home}/")), "sent as shown");
    assert_eq!(sent["note"].as_str().unwrap().trim(), "baking a splash");
    assert_eq!(sent["app_version"], env!("CARGO_PKG_VERSION"));
    assert!(h.query_by_label_contains("you sent it").is_some(), "the prompt says so: {}", h.state().crash_sent.as_ref().map_or("not yet".into(), |r| format!("{r:?}")));
    // The prompt shrinks once the form goes: let it settle where it'll be clicked.
    h.run_steps(5);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(h.state().recovery_offer.is_none());
    let _ = std::fs::remove_file(&report_path);
}

// Help → Copy diagnostics copies the version, system, viewport and recent log, with the
// home folder as ~, and sends nothing.
#[test]
fn copy_diagnostics_copies_what_support_needs() {
    applog::start(None);
    let mut app = empty_network();
    let home = recovery::home_folder().expect("a home folder");
    app.status = format!("Sprite sheet: saved to {home}/fx/fire.png");
    app.gpu_failure = Some("Validation Error\n  label = 'viewport volumes'".into());
    let mut h = harness(app);
    h.get_by_label("Help").click();
    h.run_steps(2);
    h.get_by_label_contains("Copy diagnostics").click();
    h.run_steps(2);
    // The button ran (the copy itself goes to the system clipboard, outside the test).
    assert!(h.state().status.starts_with("Copied the diagnostics"), "{}", h.state().status);
    let copied = h.state().diagnostics();
    assert!(copied.contains(&format!("ProceduralFX Studio {} diagnostics", env!("CARGO_PKG_VERSION"))), "{copied}");
    assert!(copied.contains("System: ") && copied.contains("Viewport: CPU, after a GPU error: Validation Error / label = 'viewport volumes'"), "{copied}");
    assert!(copied.contains("Sprite sheet: saved to ~/fx/fire.png"), "the status line was logged, home as ~: {copied}");
    assert!(!copied.contains(&format!("{home}/")), "no home folder");
}

// The welcome window starts from a preset, playing, with its simple controls at the top of
// Parameters; a saved scene keeps its preset (and so its controls).
#[test]
fn the_welcome_window_starts_from_a_preset_with_simple_controls() {
    let mut app = empty_network();
    app.welcome_open = true;
    let mut h = harness(app);
    assert!(h.query_by_label_contains("Welcome to ProceduralFX Studio").is_some());
    h.get_by_label("Start from Fire").click();
    settle(&mut h);
    let app = h.state();
    assert!(!app.welcome_open, "closed");
    assert_eq!(app.preset.map(|p| p.name), Some("Fire"));
    assert!(app.graph.nodes.values().any(|n| n.name == "fire_sim"), "the Fire preset's scene");
    assert!(h.query_by_label_contains("Fire controls").is_some(), "its controls show in Parameters");
    for label in ["Size", "Height", "Flicker", "Smoke", "Wind"] {
        assert!(h.query_by_label(label).is_some(), "the {label} slider");
    }
    // Saved and opened again, the scene still knows its preset.
    let text = h.state().scene_text();
    assert!(text.contains(&editor_helpers::preset_line("Fire")));
    let mut other = empty_network();
    other.use_saved_timeline(&text);
    assert_eq!(other.preset.map(|p| p.name), Some("Fire"));
    other.use_saved_timeline("pfs 2\nnext 0\n");
    assert!(other.preset.is_none(), "a scene from no preset has no controls");
}

// A new app starts with nothing in the scene and the timeline stopped.
#[test]
fn the_app_starts_empty_and_still() {
    let app = ProceduralFxStudioApp::default();
    assert!(app.graph.nodes.is_empty(), "no demo scene");
    assert!(!app.animate, "not playing");
    assert!(app.selected.is_none());
}

// Double-clicking a camera looks through it without changing it; again, back to the free view.
#[test]
fn double_clicking_a_camera_looks_through_it() {
    let mut app = ProceduralFxStudioApp::default();
    app.load_state("pfs 2\nnode 0 120 120 0 obj camera 0 1 0 0 0 0 1.2 0.5 0 0 0 6\nnext 1\n".into());
    app.current_network = None;
    let mut h = harness(app);
    let before = h.state().graph.nodes[&0].kind.params();
    let centre = node_on_screen(&h, 0).center();
    let double_click = |h: &mut Harness<ProceduralFxStudioApp>| {
        pointer(h, centre);
        h.run_steps(60);
        for pressed in [true, false, true, false] {
            press(h, centre, pressed);
            h.step();
        }
        h.run_steps(3);
    };
    double_click(&mut h);
    assert!(h.state().looking_through && h.state().active_camera == Some(0), "looking through it: {}", h.state().status);
    assert_eq!(h.state().graph.nodes[&0].kind.params(), before, "the camera itself is unchanged");
    double_click(&mut h);
    assert!(!h.state().looking_through, "back to the free view: {}", h.state().status);
}

// Export Sprite Sheet says how to use the sheet in the chosen engine: for Bevy, the crate
// and code to copy; choosing Godot writes Godot's files and says how to use them.
#[test]
fn the_export_window_says_how_to_use_the_sheet_in_each_engine() {
    let mut app = empty_network();
    app.sprite_sheet.open = true;
    let mut h = harness(app);
    h.get_by_label_contains("Use in Bevy");
    h.get_by_label("bevy_procedural_fx on crates.io");
    h.get_by_label("Copy").click();
    h.run_steps(2);
    h.get_all_by_role(egui::accesskit::Role::ComboBox).find(|n| n.value().as_deref() == Some("Bevy")).expect("the engine menu").click();
    h.run_steps(2);
    h.get_by_label("Godot 4").click();
    h.run_steps(2);
    h.get_by_label_contains("Use in Godot 4");
    assert!(h.query_by_label("Copy").is_none(), "no Bevy code for Godot");
    assert!(h.state().sprite_sheet.godot && !h.state().sprite_sheet.unity, "Godot's files are written");
}

// A preset slider's Open shows the node it changes, inside its network, selected.
#[test]
fn a_preset_sliders_open_shows_the_node_it_changes() {
    let mut app = ProceduralFxStudioApp::default();
    app.welcome_open = false;
    app.load_preset(presets::PRESETS.iter().find(|p| p.name == "Fire").unwrap());
    app.animate = false;
    let mut h = harness(app);
    h.get_all_by_label("Open").next().expect("an Open beside the Size slider").click();
    h.run_steps(3);
    let app = h.state();
    let source = app.graph.nodes.iter().find(|(_, n)| n.name == "source").map(|(id, _)| *id).unwrap();
    assert_eq!(app.selected, Some(source));
    assert_eq!(app.current_network, app.graph.nodes[&source].parent, "inside its network");
}

// A Curve from the Tab menu draws a curve; clicking on a Color node's ramp adds a key.
#[test]
fn a_curve_from_the_tab_menu_and_a_key_clicked_onto_a_ramp() {
    let mut h = harness(one_grid());
    let at = in_canvas(&h, 0.6, 0.5);
    let curve = add_with_tab(&mut h, at, "curve");
    assert!(matches!(h.state().graph.nodes[&curve].kind, NodeKind::Curve(_)));
    let mesh = h.state().graph.cook(curve, 0.0, &mut HashMap::new());
    assert_eq!(crate::groom::curves(&mesh).len(), 1, "one curve");
    h.get_by_label("➕ Add point").click();
    h.run_steps(2);
    assert!(matches!(&h.state().graph.nodes[&curve].kind, NodeKind::Curve(c) if c.points.len() == 5));

    // A Color node on its Ramp mode, selected: its ramp starts as From → To, two keys.
    let mut app = one_grid();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj color 0 2 1 0.45 0.1 0.1 0.3 1 1 1\nparent 1 0\ndisplay 0 1\nnext 2\n".into());
    app.current_network = Some(0);
    app.selected = Some(1);
    let mut h = harness(app);
    let label = h.get_by_label("Color ramp (low → high along the axis)").rect();
    // The ramp's gradient is just below its label: click a third of the way along it.
    click_at(&mut h, egui::pos2(label.left() + 110.0, label.bottom() + 16.0));
    let NodeKind::Color(set) = &h.state().graph.nodes[&1].kind else { panic!() };
    let ramp = ramp::Ramp::parse(&set.ramp, 3).expect("the ramp was written");
    assert_eq!(ramp.keys.len(), 3, "a key was added: {}", set.ramp);
    assert!(h.state().has_unsaved_changes());
}

// With Draw in the viewport on, a click adds a Curve point on the ground, a drag moves one
// and a right-click removes one.
#[test]
fn curve_points_are_placed_moved_and_removed_in_the_viewport() {
    let mut app = empty_network();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj curve 0 0 12 -1 0 0 1 0 0\nparent 1 0\ndisplay 0 1\nnext 2\n".into());
    app.current_network = Some(0);
    app.selected = Some(1);
    let mut h = harness(app);
    h.get_by_label("✏ Draw in the viewport").click();
    h.run_steps(3);
    assert!(h.state().curve_draw);
    let points = |h: &Harness<ProceduralFxStudioApp>| match &h.state().graph.nodes[&1].kind {
        NodeKind::Curve(c) => c.points.clone(),
        _ => panic!(),
    };
    // A click below the middle of the viewport lands on the ground in front of the origin.
    let view = h.state().viewport_rect.unwrap();
    click_at(&mut h, view.center() + egui::vec2(60.0, 40.0));
    let after = points(&h);
    assert_eq!(after.len(), 3, "a point was added: {after:?}");
    assert!(after[2][1].abs() < 1e-3, "on the ground: {:?}", after[2]);
    assert_eq!(h.state().selected, Some(1), "the curve stays selected");

    // Drag point 0 to the right: it moves on the ground.
    let handle = h.state().curve_handles.iter().find(|(_, i, _)| *i == 0).unwrap().2;
    drag(&mut h, handle, handle + egui::vec2(80.0, 0.0));
    let moved = points(&h);
    assert!(moved[0][0] > -0.9 && moved[0][1].abs() < 1e-3, "{:?}", moved[0]);
    assert_eq!(moved.len(), 3, "dragging a point doesn't add one");

    // A click on a point (not a drag) doesn't add another.
    let handle = h.state().curve_handles.iter().find(|(_, i, _)| *i == 1).unwrap().2;
    click_at(&mut h, handle);
    assert_eq!(points(&h).len(), 3);
    // Right-click point 2: removed.
    let handle = h.state().curve_handles.iter().find(|(_, i, _)| *i == 2).unwrap().2;
    pointer(&mut h, handle);
    h.event(egui::Event::PointerButton { pos: handle, button: egui::PointerButton::Secondary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    h.event(egui::Event::PointerButton { pos: handle, button: egui::PointerButton::Secondary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(points(&h).len(), 2);
    assert!(h.state().has_unsaved_changes());
}

// A parameter with an expression: its name, clicked, shows the expression in place of the
// value; typing a new one changes it, and a plain number replaces it.
#[test]
fn clicking_a_parameter_name_switches_between_its_value_and_expression() {
    let mut app = one_grid();
    let channel = ChannelId { node: 1, target: ChannelTarget::Param(0) };
    app.graph.channels.entry(channel).or_default().expression = Some("1 + 2".into());
    app.graph.apply_animation(0.0);
    let mut h = harness(app);
    assert!(matches!(h.state().graph.nodes[&1].kind, NodeKind::Grid { size, .. } if (size - 3.0).abs() < 1e-5));
    h.get_by_label("Size").click();
    h.run_steps(3);
    assert!(h.state().shown_expressions.contains(&channel), "the name shows the expression");
    // Edit it: 2 * 3.
    let field = h.get_all_by_role(egui::accesskit::Role::TextInput).find(|n| n.value().is_some_and(|v| v.contains('+') || v.contains('*'))).expect("the expression's field");
    field.click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("2 * 3".into()));
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    assert_eq!(h.state().graph.channels[&channel].expression.as_deref(), Some("2 * 3"));
    assert!(matches!(h.state().graph.nodes[&1].kind, NodeKind::Grid { size, .. } if (size - 6.0).abs() < 1e-5));
    // Click the name again: the value is back; then a plain number replaces the expression.
    h.get_by_label("Size").click();
    h.run_steps(3);
    assert!(!h.state().shown_expressions.contains(&channel));
    h.get_by_label("Size").click();
    h.run_steps(3);
    let field = h.get_all_by_role(egui::accesskit::Role::TextInput).find(|n| n.value().is_some_and(|v| v.contains('+') || v.contains('*'))).expect("the expression's field");
    field.click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("4".into()));
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    assert!(!h.state().graph.channels.contains_key(&channel));
    assert!(matches!(h.state().graph.nodes[&1].kind, NodeKind::Grid { size, .. } if (size - 4.0).abs() < 1e-5));
}

// A slider showing the same value as an expression doesn't take it over: POP Source's Seed
// is 1, as is its Impulse activation's expression on the first frame.
#[test]
fn an_expression_stays_with_its_own_parameter() {
    let mut app = empty_network();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 2 40 20 0 obj popnet 1\nparent 2 0\nnode 1 40 20 0 obj pop 2\nparent 1 2\ndisplay 0 2\nnext 3\n".into());
    app.current_network = Some(2);
    app.selected = Some(1);
    let index = app.graph.nodes[&1].kind.param_names().iter().position(|n| n == "impulse_activation").unwrap();
    let channel = ChannelId { node: 1, target: ChannelTarget::Param(index) };
    app.graph.channels.entry(channel).or_default().expression = Some("$F == 1".into());
    app.graph.apply_animation(0.0);
    let mut h = harness(app);
    h.run_steps(3);
    // Seed is still an ordinary slider; Impulse activation's name switches to its expression.
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Slider, "Seed").is_some(), "Seed keeps its slider");
    h.get_by_label("Impulse activation").click();
    h.run_steps(3);
    assert!(h.state().shown_expressions.contains(&channel));
    assert!(h.get_all_by_role(egui::accesskit::Role::TextInput).any(|n| n.value().as_deref() == Some("$F == 1")));
}

// Inside one object, the viewport still shows the other objects; picking and the info stay
// with the open one.
#[test]
fn the_other_objects_show_while_inside_one() {
    let mut app = empty_network();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nparent 1 0\ndisplay 0 1\nnode 2 200 20 0 obj geometry -1\nnode 3 40 20 0 obj box 1 1\nparent 3 2\ndisplay 2 3\nnext 4\n".into());
    app.enter_network(Some(2));
    app.animate = false;
    let mut h = harness(app);
    settle(&mut h);
    let (grid, cube) = (crate::node_grid(4.0, 4.0, 8, 8), crate::node_box(1.0, 1));
    let app = h.state();
    assert_eq!(app.current_mesh.vertices.len(), cube.vertices.len(), "the open object is what's picked");
    assert_eq!(app.context_parts.len(), 1, "the other object is shown");
    assert_eq!(app.context_parts[0].vertices.len(), grid.vertices.len());
    // A bypassed object doesn't show.
    h.state_mut().graph.nodes.get_mut(&0).unwrap().bypass = true;
    h.state_mut().dirty = true;
    settle(&mut h);
    assert!(h.state().context_parts.is_empty());
}

// An expression typed into POP Source's Impulse activation makes it follow it, even when
// the expression gives the value it already has (1, at the top of its slider).
#[test]
fn an_expression_typed_into_impulse_activation_sticks() {
    let mut app = empty_network();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 2 40 20 0 obj popnet 1\nparent 2 0\nnode 1 40 20 0 obj pop 2\nparent 1 2\ndisplay 0 2\nnext 3\n".into());
    app.current_network = Some(2);
    app.selected = Some(1);
    let mut h = harness(app);
    for (label, expression) in [("Impulse activation", "$F == 1"), ("Constant activation", "$T < 0.5")] {
        h.get_by_role_and_label(egui::accesskit::Role::SpinButton, label).click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text(expression.into()));
        h.key_press(egui::Key::Enter);
        settle(&mut h);
        let names = h.state().graph.nodes[&1].kind.param_names();
        let key = name_key(label);
        let index = names.iter().position(|n| name_key(n) == key).unwrap();
        let channel = ChannelId { node: 1, target: ChannelTarget::Param(index) };
        assert_eq!(h.state().graph.channels.get(&channel).and_then(|c| c.expression.as_deref()), Some(expression), "{label}: {}", h.state().status);
    }
}

// Objects are drawn each from their own geometry: one coloured per corner beside one coloured
// per face (or see-through) doesn't change the other's look.
#[test]
fn each_object_is_shown_from_its_own_attributes() {
    let mut app = empty_network();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nparent 1 0\ndisplay 0 1\nnode 2 200 20 0 obj geometry -1\nnode 3 40 20 0 obj box 1 1\nparent 3 2\ndisplay 2 3\nnext 4\n".into());
    app.enter_network(None);
    app.animate = false;
    let mut h = harness(app);
    settle(&mut h);
    let app = h.state();
    assert_eq!(app.shown_parts.len(), 2, "at the top, both objects, each its own mesh");
    let total: usize = app.shown_parts.iter().map(|p| p.vertices.len()).sum();
    assert_eq!(total, app.current_mesh.vertices.len(), "and the picked mesh is them merged");
}

// A long build's notes scroll inside the assistant's window, the newest in view.
#[test]
fn the_assistants_notes_scroll_instead_of_running_off_the_bottom() {
    let mut app = one_grid();
    app.assistant.open = true;
    app.assistant.log = (0..80).map(|i| format!("Step {i}: adding nodes and checking the result")).collect();
    let mut h = harness(app);
    h.run_steps(6);
    let last = h.get_by_label("• Step 79: adding nodes and checking the result").rect();
    assert!(last.bottom() <= 900.0 && last.top() >= 0.0, "the newest note is on screen: {last:?}");
    assert!(h.query_by_label("• Step 0: adding nodes and checking the result").is_none_or(|n| n.rect().bottom() < last.top()), "the oldest has scrolled up");
}

// Dragging on empty network space selects the nodes in the box; Shift adds, Ctrl takes away,
// and Alt-dragging pans instead.
#[test]
fn dragging_a_box_selects_nodes() {
    let mut app = empty_network();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 40 0 obj box 1 1\nparent 1 0\nnode 2 40 140 0 obj box 1 1\nparent 2 0\nnode 3 40 240 0 obj box 1 1\nparent 3 0\nnode 4 40 400 0 obj box 1 1\nparent 4 0\nnext 5\n".into());
    app.current_network = Some(0);
    app.selected = None;
    let mut h = harness(app);
    let corner = |h: &Harness<ProceduralFxStudioApp>, id: NodeId, dx: f32, dy: f32| {
        let r = node_on_screen(h, id);
        if dx < 0.0 { r.left_top() + egui::vec2(dx, dy) } else { r.right_bottom() + egui::vec2(dx, dy) }
    };
    // A drag with modifier keys held throughout.
    let drag_with = |h: &mut Harness<ProceduralFxStudioApp>, from: egui::Pos2, to: egui::Pos2, m: egui::Modifiers| {
        let step = |h: &mut Harness<ProceduralFxStudioApp>, n: usize| {
            for _ in 0..n {
                h.input_mut().modifiers = m;
                h.step();
            }
        };
        let button = |h: &mut Harness<ProceduralFxStudioApp>, at: egui::Pos2, pressed: bool| {
            h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: m });
        };
        h.hover_at(from);
        step(h, 2);
        button(h, from, true);
        step(h, 2);
        for k in 1..=8 {
            h.hover_at(from + (to - from) * (k as f32 / 8.0));
            step(h, 2);
        }
        button(h, to, false);
        step(h, 2);
    };
    let selection = |h: &Harness<ProceduralFxStudioApp>| {
        let mut s = h.state().selected_nodes();
        s.sort_unstable();
        s
    };
    // A box round the left column: nodes 1, 2 and 3.
    let (from, to) = (corner(&h, 1, -15.0, -15.0), corner(&h, 3, 15.0, 15.0));
    drag(&mut h, from, to);
    assert_eq!(selection(&h), vec![1, 2, 3]);
    // Shift: node 4 joins them.
    let (from, to) = (corner(&h, 4, -15.0, -15.0), corner(&h, 4, 15.0, 15.0));
    drag_with(&mut h, from, to, egui::Modifiers::SHIFT);
    assert_eq!(selection(&h), vec![1, 2, 3, 4]);
    // Ctrl: node 2 leaves.
    let (from, to) = (corner(&h, 2, -15.0, -15.0), corner(&h, 2, 15.0, 15.0));
    drag_with(&mut h, from, to, egui::Modifiers::COMMAND);
    assert_eq!(selection(&h), vec![1, 3, 4]);
    // A plain box elsewhere replaces the selection.
    let (from, to) = (corner(&h, 4, -15.0, -15.0), corner(&h, 4, 15.0, 15.0));
    drag_with(&mut h, from, to, egui::Modifiers::NONE);
    assert_eq!(selection(&h), vec![4]);
    // Alt-drag pans, selecting nothing new.
    let pan = h.state().graph_pan;
    let start = corner(&h, 1, -15.0, -30.0);
    drag_with(&mut h, start, start + egui::vec2(120.0, 60.0), egui::Modifiers::ALT);
    assert!((h.state().graph_pan - pan).length() > 50.0, "the view moved");
    assert_eq!(selection(&h), vec![4]);
}
