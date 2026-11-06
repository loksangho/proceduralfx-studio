use crate::*;

fn flip() -> NodeKind {
    NodeKind::Flip { particle_count: 3000, grid_resolution: 24, flip_ratio: 0.95, viscosity: 0.1, gravity: -9.8, max_dt: 0.02, substeps: 1, seed: 3, domain: [0.0; 6] }
}

#[test]
fn flip_dam_break_keeps_its_volume_and_flows() {
    // A metre cube of water in the corner of a closed 4 x 2 x 2 tank.
    let mut emitter = node_box(1.0, 1);
    for v in &mut emitter.vertices {
        *v = v.add(Vec3::new(-1.4, 0.5, 0.0));
    }
    let res = 48;
    let h = 4.0 / res as f32;
    sims::clear_all();
    let out = node_flip(9401, emitter, Mesh::default, 20000, res, 0.95, 0.1, -9.8, 0.02, 1, 3, [0.0, 1.0, 0.0, 4.0, 2.0, 2.0], 1.0);
    let cells: HashSet<(i32, i32, i32)> = out.vertices.iter().map(|v| (((v.x + 2.0) / h) as i32, (v.y / h) as i32, ((v.z + 1.0) / h) as i32)).collect();
    let volume = cells.len() as f32 * h.powi(3);
    // Neither squashed flat nor blown apart.
    assert!((0.7..1.6).contains(&volume), "volume {volume}");
    // The water has run across the floor to the far half of the tank...
    assert!(out.vertices.iter().filter(|v| v.x > 0.5).count() > 1000);
    // ...and the particles still sit in a layer, not spread through the air.
    let high = out.vertices.iter().filter(|v| v.y > 0.6).count();
    assert!(high < out.vertices.len() / 20, "{high} particles high up");
}

#[test]
fn flip_steps_from_its_cache_and_feeds_whitewater() {
    let mut g = Graph::default();
    g.next_id += 9300;
    let emitter = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    let lift = g.add(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [1.0, 1.0, 1.0] }, egui::pos2(0.0, 0.0));
    let fluid = g.add(flip(), egui::pos2(0.0, 0.0));
    let white = g.add(NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings { emission_rate: 3000.0, min_speed: 0.2, ..whitewater::WhitewaterSettings::defaults() }), egui::pos2(0.0, 0.0));
    g.connect(emitter, lift, 0);
    g.connect(lift, fluid, 0);
    g.connect(fluid, white, 0);
    sims::clear_all();
    let a = g.cook(fluid, 0.5, &mut HashMap::new());
    assert_eq!(a.vertices.len(), 3000);
    assert_eq!(a.point_attributes["v"].len(), 3000 * 3);
    // The fluid (which sits on its domain floor, the emitter's bottom) is moving.
    let speed: f32 = a.point_attributes["v"].chunks(3).map(|v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()).sum::<f32>() / 3000.0;
    assert!(speed.is_finite() && speed > 0.1, "mean speed {speed}");
    let b = g.cook(fluid, 1.0, &mut HashMap::new());
    sims::clear_all();
    let fresh = g.cook(fluid, 1.0, &mut HashMap::new());
    assert_eq!(b.vertices, fresh.vertices, "cached stepping matches a fresh run");
    assert!(g.time_dependent(white));
    let out = g.cook(white, 1.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&white).cloned()).is_none());
    assert!(!out.vertices.is_empty(), "the splash should make whitewater");
    assert!(renders_points(&out));
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(format!("{:?}", loaded.nodes[&white].kind), format!("{:?}", g.nodes[&white].kind));
    // Without a fluid input it says what's missing.
    let lonely = g.add(NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings::defaults()), egui::pos2(0.0, 0.0));
    g.cook(lonely, 1.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&lonely).cloned()).unwrap().contains("FLIP"));
}

#[test]
#[ignore]
fn whitewater_points_render_in_moonray() {
    let mut mesh = Mesh::default();
    for i in 0..200 {
        mesh.vertices.push(Vec3::new(hash01(i, 1) - 0.5, hash01(i, 2) - 0.5, hash01(i, 3) - 0.5));
    }
    mesh.point_attributes.insert("pscale".into(), vec![0.03; 200]);
    mesh.point_attributes.insert(whitewater::RENDER_POINTS.into(), vec![1.0]);
    let mut app = ProceduralFxStudioApp::default();
    app.render_camera = RenderCamera::Viewport;
    let scene = RenderScene { parts: vec![(mesh, PartMaterial::Default)], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
    let dir = std::env::temp_dir();
    let (rdla, exr) = (dir.join("pfx_points.rdla"), dir.join("pfx_points.exr"));
    app.export_scene_rdla(&rdla, &scene, 64, 48, 4).unwrap();
    let run = Command::new("native/moonray/macos/moonray")
        .env("RDL2_DSO_PATH", "/Users/loksangho/moonray/build/rdl2dso")
        .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
        .output()
        .unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(run.status.success() && !log.contains("Error"), "{log}");
    let render = image::open(&exr).unwrap().to_rgb32f();
    let lit = render.pixels().filter(|p| p[0] + p[1] + p[2] > 0.05).count();
    assert!(lit > 50, "{lit} lit pixels");
}

// Particle presets render in MoonRay: their points become glowing spheres, coloured and
// faded by their attributes. `cargo test --release -- --ignored particles_render`.
#[test]
#[ignore]
fn particles_render_in_moonray() {
    let preset = presets::PRESETS.iter().find(|p| p.name == "Impact").unwrap();
    let mut app = ProceduralFxStudioApp::default();
    app.load_preset(preset);
    app.render_at = Some(0.15);
    let scene = app.render_scene().unwrap();
    assert!(scene.parts.iter().any(|(mesh, _)| renders_points(mesh)), "the sparks are in the scene");
    let dir = std::env::temp_dir();
    let (rdla, exr) = (dir.join("pfx_particles.rdla"), dir.join("pfx_particles.exr"));
    app.export_scene_rdla(&rdla, &scene, 128, 96, 4).unwrap();
    let run = Command::new("native/moonray/macos/moonray")
        .env("RDL2_DSO_PATH", "/Users/loksangho/moonray/build/rdl2dso")
        .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
        .output()
        .unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(run.status.success() && !log.contains("Error"), "{log}");
    let render = image::open(&exr).unwrap().to_rgb32f();
    // Sparks glow orange: red above blue.
    let glowing = render.pixels().filter(|p| p[0] > 0.5 && p[0] > p[2] * 1.5).count();
    assert!(glowing > 20, "{glowing} glowing pixels");
    if let Ok(dir) = std::env::var("PFX_KEEP_RENDER") {
        let _ = std::fs::copy(&exr, std::path::Path::new(&dir).join("particles.exr"));
    }
}
