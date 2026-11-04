use crate::*;

// `cargo test --release -- --ignored fetch_renders_frames_with_moonray`
#[test]
#[ignore]
fn fetch_renders_frames_with_moonray() {
    // The packaged app finds MoonRay's shader DSOs itself; a test run needs telling.
    unsafe { std::env::set_var("RDL2_DSO_PATH", "/Users/loksangho/moonray/build/rdl2dso") };
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    app.render_camera = RenderCamera::Viewport;
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let sphere = app.graph.add(NodeKind::Sphere { radius: 1.0, rows: 12, cols: 16 }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&sphere).unwrap().parent = Some(geo);
    app.graph.set_display(sphere);
    let dir = std::env::temp_dir().join(format!("pfx_fetch_{}", std::process::id()));
    let render = app.graph.add(
        NodeKind::MoonRayRender {
            executable: "native/moonray/macos/moonray".into(),
            arguments: "--scene {scene} --output {output}".into(),
            output: dir.join("frame.exr").to_string_lossy().into_owned(),
            width: 64,
            height: 48,
            samples: 4,
            opts: RenderOpts::default(),
        },
        egui::pos2(0.0, 0.0),
    );
    app.graph.nodes.get_mut(&render).unwrap().context = AppContext::Out;
    let mut range = tops::TopNode::new(tops::FRAME_RANGE);
    range.set("end", 2.0);
    let range = app.graph.add(NodeKind::Top(range), egui::pos2(0.0, 0.0));
    let mut fetch = tops::TopNode::new(tops::FETCH);
    fetch.set("node", render as f32);
    fetch.texts[0] = dir.join("beauty.$F4.exr").to_string_lossy().into_owned();
    let fetch = app.graph.add(NodeKind::Top(fetch), egui::pos2(0.0, 0.0));
    app.graph.connect(range, fetch, 0);
    app.start_top_cook(fetch);
    for _ in 0..6000 {
        if app.top_run.is_none() {
            break;
        }
        app.poll_moonray_render();
        app.advance_top_run();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(app.top_results[&fetch].iter().all(|s| s.0 == tops::State::Done), "{:?} / {}", app.top_results[&fetch], app.status);
    for f in ["beauty.0001.exr", "beauty.0002.exr"] {
        assert!(dir.join(f).exists(), "{f} missing");
    }
    // A Render COP reads the latest one.
    assert!(cops::last_render().is_some_and(|p| p.ends_with("beauty.0002.exr")));
}
