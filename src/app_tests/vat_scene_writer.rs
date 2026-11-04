use crate::*;
// Writes two test scenes for VAT: PFX_VAT_SCENES=<dir> cargo test --release -- --ignored write_vat_scenes
#[test]
#[ignore]
fn write_vat_scenes() {
    let Some(dir) = std::env::var_os("PFX_VAT_SCENES").map(PathBuf::from) else { return };
    let kind = |name: &str| NodeKind::templates().into_iter().flat_map(|(_, k)| k).find(|k| k.name() == name).unwrap_or_else(|| panic!("{name}"));
    // Rigid: a fractured box dropped onto the ground.
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let b = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    let up = g.add(NodeKind::Transform { t: [0.0, 1.5, 0.0], r: [20.0, 30.0, 0.0], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    let frac = g.add(kind("material_fracture"), egui::pos2(0.0, 0.0));
    let rbd = g.add(kind("rbd_solver"), egui::pos2(0.0, 0.0));
    for n in [b, up, frac, rbd] { g.nodes.get_mut(&n).unwrap().parent = Some(geo); }
    g.connect(b, up, 0); g.connect(up, frac, 0); g.connect(frac, rbd, 0);
    g.displays.insert(geo, rbd);
    g.nodes.get_mut(&geo).unwrap().name = "rubble".into();
    std::fs::write(dir.join("rubble.pfs"), g.serialize()).unwrap();
    // Fluid: a POP Network's particles meshed by Particle Fluid Surface.
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let s = app.graph.add(NodeKind::Sphere { radius: 0.3, rows: 8, cols: 12 }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&s).unwrap().parent = Some(geo);
    app.current_network = Some(geo);
    app.apply(Action::Add(NodeKind::PopNet { substeps: 1 }, egui::pos2(0.0, 100.0)));
    let net = app.selected.unwrap();
    app.graph.connect(s, net, 0);
    let surface = app.graph.add(kind("particle_surface"), egui::pos2(0.0, 200.0));
    app.graph.nodes.get_mut(&surface).unwrap().parent = Some(geo);
    app.graph.connect(net, surface, 0);
    app.graph.displays.insert(geo, surface);
    app.graph.nodes.get_mut(&geo).unwrap().name = "splash".into();
    std::fs::write(dir.join("splash.pfs"), app.graph.serialize()).unwrap();
}
