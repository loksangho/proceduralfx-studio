use crate::*;

// Renders a STAGE network through Hydra (hd_render + hdMoonray):
// `PFX_HD_RENDER=… PFX_HYDRA_PLUGIN_PATH=… RDL2_DSO_PATH=… cargo test -- --ignored stage_renders_through_hydra`.
#[test]
#[ignore]
fn stage_renders_through_hydra() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let cube = app.graph.add(NodeKind::Box { size: 2.0, div: 1 }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&cube).unwrap().parent = Some(geo);
    app.graph.set_display(cube);
    let lops = [
        NodeKind::LopSopImport { geometry: Some(geo), path: "/geo".into() },
        NodeKind::LopLight { kind: 0, path: "/lights/sun".into(), position: [3.0, 5.0, 4.0], target: [0.0; 3], color: [1.0; 3], intensity: 2.0, size: 0.53 },
        NodeKind::LopCamera { path: "/cam".into(), position: [3.0, 3.0, 6.0], target: [0.0; 3], focal: 35.0, aperture: 36.0 },
    ];
    let ids: Vec<NodeId> = lops.into_iter().map(|kind| app.graph.add(kind, egui::pos2(0.0, 0.0))).collect();
    for pair in ids.windows(2) {
        app.graph.connect(pair[0], pair[1], 0);
    }
    for id in &ids {
        app.graph.nodes.get_mut(id).unwrap().context = AppContext::Stage;
    }
    app.graph.set_display(ids[2]);
    app.render_opts = RenderOpts { stage: true, hydra: true, ..RenderOpts::default() };

    let dir = std::env::temp_dir();
    let usda = dir.join("pfx_hydra_test.usda");
    let exr = dir.join("pfx_hydra_test.exr");
    let camera = app.write_hydra_scene(&usda).unwrap();
    assert_eq!(camera.as_deref(), Some("cam"));
    assert!(std::fs::read_to_string(&usda).unwrap().contains("bool inputs:normalize = 1"));
    let hd_render = find_hd_render(std::path::Path::new("moonray")).expect("set PFX_HD_RENDER");
    let output = Command::new(&hd_render)
        .env("PXR_PLUGINPATH_NAME", hydra_plugin_path(&hd_render).expect("set PFX_HYDRA_PLUGIN_PATH"))
        .args(["-renderer", "HdMoonrayRendererDebugPlugin", "-in", usda.to_str().unwrap(), "-out", exr.to_str().unwrap(), "-size", "160", "120", "-camera", "cam"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let image = image::open(&exr).unwrap().to_rgb32f();
    let lit = image.pixels().filter(|p| p[0] > 0.1).count();
    let brightest = image.pixels().map(|p| p[0]).fold(0.0, f32::max);
    println!("{lit} lit pixels, brightest {brightest}");
    assert!(lit > image.pixels().len() / 20 && brightest > 0.3, "the cube is lit like the MoonRay render");
}
