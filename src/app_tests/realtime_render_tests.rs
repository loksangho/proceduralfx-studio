use crate::*;

// Runs the app's own frame steps until the recording finishes.
fn run_recording(app: &mut ProceduralFxStudioApp) {
    run_recording_for(app, 400);
}

// `steps` frames of the app, or until the recording is done (simulations cook in the background).
pub(super) fn run_recording_for(app: &mut ProceduralFxStudioApp, steps: usize) {
    let _ = drive_recording(app, Some(steps), &mut |_, _| {});
    assert!(app.flipbook_recording.is_none(), "the recording finished: {}", app.status);
}

#[test]
fn point_alpha_shows_in_real_time_renders() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    // A square facing the camera, Alpha 0.25 on the left half and 1 on the right,
    // under a Principled Shader using point alpha.
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 8 }, egui::pos2(0.0, 0.0));
    let turn = g.add(NodeKind::Transform { t: [0.0; 3], r: [90.0, 0.0, 0.0], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    let alpha = g.add(NodeKind::Wrangle { code: "f@Alpha = @P.x < -0.1 ? 0.25 : 1.0;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    for n in [grid, turn, alpha] {
        g.nodes.get_mut(&n).unwrap().parent = Some(geo);
    }
    g.connect(grid, turn, 0);
    g.connect(turn, alpha, 0);
    g.displays.insert(geo, alpha);
    let mut shader = shading::Values::new(shading::Table::Principled);
    shader.set("base_color", [1.0, 1.0, 1.0]);
    shader.set("use_point_alpha", [1.0, 0.0, 0.0]);
    let material = g.add(NodeKind::Shader(shader), egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&material).unwrap().context = AppContext::Mat;
    g.nodes.get_mut(&geo).unwrap().kind = NodeKind::Geometry { material: Some(material) };
    app.graph = g;
    app.selected = None;
    app.render_camera = RenderCamera::Viewport;
    (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.0, 0.0, 6.0, 0.0, 0.0);
    (app.playback_start, app.playback_end) = (0.0, 0.0);
    app.sprite_sheet = spritesheet::Settings { width: 64, height: 64, ..Default::default() };
    app.dirty = true;
    app.preview_sprite_sheet();
    run_recording(&mut app);
    let frame = &app.flipbook_viewer.as_ref().expect("the preview opens").frames()[0];
    let at = |x: usize| frame.pixels[32 * 64 + x];
    // Left of centre: a quarter there; right: solid.
    let (left, right) = (at(24), at(40));
    assert!((50..=80).contains(&left.a()), "left {left:?}");
    assert_eq!(right.a(), 255, "right {right:?}");
    assert_eq!(at(1).a(), 0, "empty beyond the square");
    // The square's Alpha is its opacity: its points aren't drawn as splats.
    assert_eq!(app.gpu_viewport.as_ref().unwrap().splat_count(), 0);
    // Loose points with Alpha (smoke, particles) still are.
    let mut loose = Mesh { vertices: vec![Vec3::zero(), Vec3::new(1.0, 0.0, 0.0)], ..Mesh::default() };
    loose.point_attributes.insert("Alpha".into(), vec![1.0, 1.0]);
    assert_eq!(used_points(&loose, &[]), vec![false, false]);
    assert!(used_points(&node_grid(1.0, 1.0, 2, 2), &[]).iter().all(|u| *u));
}

#[test]
fn previews_play_the_sheet_as_it_will_export() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    // The ball moves right over 4 frames, then stays at the right.
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nchannel 0 x 0 0,-0.5,1,0,0;0.125,0.5,1,0,0\nnext 2\n");
    app.selected = None;
    app.render_camera = RenderCamera::Viewport;
    (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.0, 0.0, 6.0, 0.0, 0.0);
    (app.playback_start, app.playback_end) = (0.0, 3.0 / sims::SIM_FPS);
    app.sprite_sheet = spritesheet::Settings { width: 64, height: 64, seamless: true, blend: 2, ..Default::default() };
    app.dirty = true;
    app.preview_sprite_sheet();
    run_recording(&mut app);
    let viewer = app.flipbook_viewer.as_ref().expect("the preview opens in the viewer");
    assert_eq!(viewer.frame_count(), 4, "one frame per frame of the range: {}", app.status);
    assert!(app.status.contains("as they'll export, with the seamless blend"), "{}", app.status);
    // Frame 1 is blended with the ball at the right, where the range ends; frame 4 isn't.
    let alpha = |frame: &egui::ColorImage, x: usize| frame.pixels[32 * 64 + x].a();
    let (first, last) = (&viewer.frames()[0], &viewer.frames()[3]);
    assert!(alpha(first, 22) > 0 && alpha(first, 42) > alpha(first, 22), "both ends show in the first frame");
    assert!(alpha(last, 22) == 0 && alpha(last, 42) > 200, "the last frame is as rendered");

    // With Pixel art, the preview shows it: the same size, in hard-edged 4 × 4 blocks.
    app.sprite_sheet = spritesheet::Settings { width: 64, height: 64, pixel_art: true, pixel_scale: 4, pixel_colors: 4, ..Default::default() };
    app.preview_sprite_sheet();
    run_recording(&mut app);
    let viewer = app.flipbook_viewer.as_ref().expect("the preview opens in the viewer");
    assert!(app.status.contains("as pixel art"), "{}", app.status);
    let frame = &viewer.frames()[3];
    assert_eq!(frame.size, [64, 64]);
    assert!(frame.pixels.iter().all(|p| p.a() == 0 || p.a() == 255), "no soft edges");
    assert!(frame.pixels.iter().any(|p| p.a() == 255), "the ball shows");
    for y in (0..64).step_by(4) {
        for x in (0..64).step_by(4) {
            let block = frame.pixels[y * 64 + x];
            assert!((0..4).all(|dy| (0..4).all(|dx| frame.pixels[(y + dy) * 64 + x + dx] == block)), "a solid block at {x},{y}");
        }
    }
}

#[test]
fn seamless_sprite_sheets_fade_the_end_into_the_start() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    // The ball moves right over 4 frames, then stays at the right.
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nchannel 0 x 0 0,-0.5,1,0,0;0.125,0.5,1,0,0\nnext 2\n");
    app.selected = None;
    app.render_camera = RenderCamera::Viewport;
    (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.0, 0.0, 6.0, 0.0, 0.0);
    (app.playback_start, app.playback_end) = (0.0, 3.0 / sims::SIM_FPS);
    app.sprite_sheet = spritesheet::Settings { width: 64, height: 64, seamless: true, blend: 2, ..Default::default() };
    app.dirty = true;
    let dir = std::env::temp_dir().join(format!("pfx_sheet_loop_{}", std::process::id()));
    app.start_sprite_sheet(dir.join("ball.png"), false);
    run_recording(&mut app);
    let sheet = image::open(dir.join("ball.png")).expect("the sheet was saved").to_rgba8();
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ball.pfx.json")).unwrap()).unwrap();
    assert_eq!(meta["frame_count"], 4, "one frame per frame of the range, however many were rendered");
    let [w, h] = [meta["frame_size"][0].as_u64().unwrap() as u32, meta["frame_size"][1].as_u64().unwrap() as u32];
    let alpha = |frame: u32, x: u32| {
        let cell = &meta["frames"][frame as usize];
        sheet.get_pixel(cell[0].as_u64().unwrap() as u32 + x, cell[1].as_u64().unwrap() as u32 + h / 2).0[3]
    };
    // Frame 1 fades from the ball at the right (where frame 4 left it) towards its own,
    // at the left: both show, the right more strongly.
    let (left, right) = (alpha(0, w / 4), alpha(0, w * 3 / 4));
    assert!(left > 0 && right > left, "frame 1 blends both ends: left {left}, right {right}");
    // The last frame is as rendered: only the right.
    assert!(alpha(3, w / 4) == 0 && alpha(3, w * 3 / 4) == 255);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn presets_export_ready_to_use_sheets() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    app.load_preset(presets::find("Magic burst").unwrap());
    assert!(matches!(app.render_camera, RenderCamera::Node(id) if app.graph.nodes[&id].name == "sheet_camera"));
    assert_eq!((app.playback_start, app.playback_end, app.sprite_sheet.width, app.sprite_sheet.seamless), (0.0, 0.75, 256, false));
    let dir = std::env::temp_dir().join(format!("pfx_preset_sheet_{}", std::process::id()));
    app.start_sprite_sheet(dir.join("burst.png"), false);
    run_recording(&mut app);
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("burst.pfx.json")).unwrap()).unwrap();
    assert_eq!(meta["frame_count"], 19, "0 to 0.75 s at 24 fps");
    let sheet = image::open(dir.join("burst.png")).unwrap().to_rgba8();
    assert!(sheet.pixels().filter(|p| p.0[3] > 128).count() > 1000, "it glows");
    // Undo goes back to the scene before.
    app.do_undo();
    assert!(app.graph.nodes.is_empty() || !app.graph.nodes.values().any(|n| n.name == "magic_burst"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_sprite_sheets_follow_the_scene() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n");
    app.selected = None;
    app.render_camera = RenderCamera::Viewport;
    (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.0, 0.0, 6.0, 0.0, 0.0);
    (app.playback_start, app.playback_end) = (0.0, 1.0 / sims::SIM_FPS);
    app.sprite_sheet = spritesheet::Settings { width: 64, height: 64, live: true, ..Default::default() };
    app.dirty = true;
    let dir = std::env::temp_dir().join(format!("pfx_live_sheet_{}", std::process::id()));
    app.start_sprite_sheet(dir.join("ball.png"), false);
    run_recording(&mut app);
    assert!(!app.viewer.open, "the Render Viewer stays shut, so it doesn't cover the game");
    let width = |dir: &std::path::Path| {
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ball.pfx.json")).unwrap()).unwrap();
        meta["frame_size"][0].as_u64().unwrap()
    };
    let small = width(&dir);
    // A bigger ball: once the change settles, the same files are exported again.
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 1 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n");
    app.dirty = true;
    let ctx = egui::Context::default();
    let started = Instant::now();
    // CI machines without a GPU render in software, slowly: allow them a minute.
    while width(&dir) == small && started.elapsed() < std::time::Duration::from_secs(60) {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.live_sheet_step(ctx);
            app.flipbook_step(ctx);
            if app.dirty || app.simulation_rx.is_some() {
                app.rebuild_graph();
                app.dirty = false;
            }
            app.flipbook_request_capture(ctx);
            egui::CentralPanel::default().show(ctx, |ui| app.viewport(ui, ctx));
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(width(&dir) > small, "re-exported with the bigger ball: {}", app.status);
    assert_eq!(app.live_sheet.as_ref().map(|l| l.exported), Some(app.live_sheet_fingerprint()), "and it knows the files are current");
    // Turning it off stops it.
    app.sprite_sheet.live = false;
    let _ = ctx.run(egui::RawInput::default(), |ctx| app.live_sheet_step(ctx));
    assert!(app.live_sheet.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sprite_sheets_pack_the_playback_range_with_its_metadata() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    // A sphere moving 1 unit along x over the first 4 frames.
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nchannel 0 x 0 0,-0.5,1,0,0;0.125,0.5,1,0,0\nnext 2\n");
    app.selected = None;
    app.render_camera = RenderCamera::Viewport;
    (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.0, 0.0, 6.0, 0.0, 0.0);
    (app.playback_start, app.playback_end) = (0.0, 3.0 / sims::SIM_FPS);
    app.sprite_sheet = spritesheet::Settings { width: 64, height: 64, ..Default::default() };
    app.dirty = true;
    let dir = std::env::temp_dir().join(format!("pfx_sheet_app_{}", std::process::id()));
    app.start_sprite_sheet(dir.join("ball.png"), false);
    run_recording(&mut app);
    assert!(app.viewer.open, "without Live, the sheet opens in the Render Viewer");
    let sheet = image::open(dir.join("ball.png")).expect("the sheet was saved").to_rgba8();
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ball.pfx.json")).unwrap()).unwrap();
    assert_eq!(meta["frame_count"], 4);
    assert_eq!(meta["fps"], 24.0);
    let [w, h] = [meta["frame_size"][0].as_u64().unwrap() as u32, meta["frame_size"][1].as_u64().unwrap() as u32];
    assert!(w < 64 && h < 64 && w > h, "trimmed to the moving ball: {w}×{h}");
    assert_eq!(sheet.dimensions(), (w * meta["columns"].as_u64().unwrap() as u32, h * meta["rows"].as_u64().unwrap() as u32));
    // The ball moves right: frame 1's left half is lit, frame 4's isn't.
    let lit = |frame: u32, x: u32| {
        let cell = &meta["frames"][frame as usize];
        sheet.get_pixel(cell[0].as_u64().unwrap() as u32 + x, cell[1].as_u64().unwrap() as u32 + h / 2).0[3] > 0
    };
    assert!(lit(0, w / 4) && !lit(0, w * 3 / 4));
    assert!(!lit(3, w / 4) && lit(3, w * 3 / 4));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn looking_through_a_camera_leaves_it_unchanged() {
    let mut app = ProceduralFxStudioApp::default();
    let camera = app.graph.add(NodeKind::Camera { position: [1.0, 2.0, 3.0], rotation: [-10.0, 30.0, 0.0], focal: 1.0, near: 0.1, orthographic: true, pan: [0.0, 0.0], zoom: 6.0 }, egui::pos2(0.0, 0.0));
    let ctx = egui::Context::default();
    app.look_through(Some(camera));
    assert_eq!((app.cam_target.x, app.cam_zoom, app.orthographic), (1.0, 6.0, true));
    // Edits to the camera show in the view.
    if let NodeKind::Camera { zoom, .. } = &mut app.graph.nodes.get_mut(&camera).unwrap().kind {
        *zoom = 9.0;
    }
    app.begin_animation_frame(&ctx);
    assert_eq!(app.cam_zoom, 9.0);
    assert!(app.looking_through);
    // Orbiting leaves the camera where it is and goes back to the free view.
    app.cam_rot_y += 0.5;
    app.sync_active_camera_from_viewport();
    app.begin_animation_frame(&ctx);
    assert!(!app.looking_through && app.active_camera.is_none());
    assert!(matches!(app.graph.nodes[&camera].kind, NodeKind::Camera { rotation: [-10.0, 30.0, 0.0], zoom: 9.0, .. }));
    assert!((app.cam_rot_y - (30.0f32.to_radians() + 0.5)).abs() < 1e-5, "the view stays where it was moved to");
    // Using it as the viewport camera still moves it with the view.
    app.selected = Some(camera);
    app.use_camera_requested = true;
    app.use_requested_camera();
    app.cam_zoom = 4.0;
    app.sync_active_camera_from_viewport();
    assert!(matches!(app.graph.nodes[&camera].kind, NodeKind::Camera { zoom: 4.0, .. }));
}

#[test]
fn real_time_range_renders_play_in_the_flipbook_viewer() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    app.load_preset(presets::find("Slash").unwrap());
    (app.moonray_default_width, app.moonray_default_height) = (64, 64);
    let frame = 1.0 / sims::SIM_FPS;
    app.moonray_default_opts = RenderOpts { sequence: true, range_start: 0.0, range_end: 4.0 * frame, range_step: frame, ..RenderOpts::default() };
    app.start_realtime_render();
    run_recording(&mut app);
    let viewer = app.flipbook_viewer.as_ref().unwrap_or_else(|| panic!("the frames play: {}", app.status));
    assert_eq!(viewer.frame_count(), 5);
    assert!(app.status.starts_with("Rendered 5 frames in real time") && app.status.contains("flipbook viewer"), "{}", app.status);
    // Cameras are listed by their names.
    let cameras = app.camera_ids();
    assert_eq!(cameras.len(), 1);
    assert_eq!(cameras[0].1, "sheet_camera");
    assert_eq!(app.resolve_render_camera().1, "sheet_camera");
}

#[test]
fn real_time_renders_save_transparent_frames_through_the_render_camera() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 1 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n");
    app.selected = None;
    app.render_camera = RenderCamera::Viewport;
    (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.3, 0.4, 6.0, 0.0, 0.0);
    (app.moonray_default_width, app.moonray_default_height) = (64, 48);
    app.moonray_default_opts.sequence = false;
    app.dirty = true;
    app.start_realtime_render();
    let dir = app.flipbook_recording.as_ref().and_then(|r| r.capture.as_ref()).and_then(|c| c.save_to.clone()).expect("a real-time render started");
    // The app's own frame steps: the flipbook, cooking, asking for the capture, the viewport.
    let ctx = egui::Context::default();
    let screen = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 400.0))), ..Default::default() };
    for _ in 0..10 {
        let _ = ctx.run(screen.clone(), |ctx| {
            app.flipbook_step(ctx);
            if app.dirty || app.simulation_rx.is_some() {
                app.rebuild_graph();
                app.dirty = false;
            }
            app.flipbook_request_capture(ctx);
            egui::CentralPanel::default().show(ctx, |ui| app.viewport(ui, ctx));
        });
        if app.flipbook_recording.is_none() {
            break;
        }
    }
    assert!(app.flipbook_recording.is_none(), "the render finished: {}", app.status);
    let frame = image::open(dir.join("render.0001.png")).expect("the frame was saved").to_rgba8();
    assert_eq!(frame.dimensions(), (64, 48), "at the Render Settings size, not the viewport's");
    assert_eq!(frame.get_pixel(0, 0).0[3], 0, "transparent where there's nothing");
    let middle = frame.get_pixel(32, 24).0;
    assert!(middle[3] == 255 && middle[0] > 40, "the sphere fills the middle: {middle:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// When the GPU refuses the viewport's work, the app carries on with the CPU viewport and
// says why, instead of crashing.
#[test]
fn a_gpu_failure_falls_back_to_the_cpu_viewport() {
    let Some(gpu) = gpu_viewport::headless() else {
        eprintln!("no GPU here; skipping");
        return;
    };
    let mut app = ProceduralFxStudioApp::default();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n".into());
    app.animate = false;
    app.gpu_viewport = Some(gpu);
    let ctx = egui::Context::default();
    let frame = |app: &mut ProceduralFxStudioApp| {
        let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), ..Default::default() }, |ctx| app.frame(ctx));
    };
    frame(&mut app);
    assert!(app.gpu_viewport.is_some());
    app.gpu_viewport.as_ref().unwrap().fail("Validation Error\n\nCaused by:\n  In Device::create_render_pipeline, label = 'viewport volumes'");
    frame(&mut app);
    assert!(app.gpu_viewport.is_none(), "given up");
    assert!(app.status.contains("drawn on the CPU now") && app.status.contains("Validation Error"), "{}", app.status);
    assert!(app.gpu_failure.as_deref().is_some_and(|e| e.contains("viewport volumes")));
    // And the app keeps drawing, the viewport with the CPU.
    frame(&mut app);
    frame(&mut app);
    assert!(!app.current_mesh.vertices.is_empty());
}
