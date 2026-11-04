use crate::*;

fn skin() -> Mesh {
    let mut mesh = Mesh::default();
    mesh.vertices = vec![Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, 1.0)];
    mesh.indices = vec![[0, 2, 1], [0, 3, 2]];
    mesh
}

fn groomed(with_hair: bool) -> Mesh {
    let skin = skin();
    if !with_hair {
        return skin;
    }
    let guides = groom::hair_guides(&skin, &groom::GuideSettings { count: 30, length: 0.8, length_variance: 0.2, segments: 6, droop: 0.4, seed: 2 });
    let hair = groom::hair_generate(
        &skin,
        &guides,
        &groom::HairSettings { count: 3000, segments: 6, clump: 0.3, length_variance: 0.1, frizz: 0.2, width_root: 0.02, width_tip: 0.01, seed: 4 },
    );
    node_merge(skin, hair)
}

fn export(path: &std::path::Path, mesh: Mesh) -> String {
    let app = ProceduralFxStudioApp::default();
    let scene = RenderScene { parts: vec![(mesh, PartMaterial::Default)], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
    app.export_scene_rdla(path, &scene, 160, 120, 4).unwrap();
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn hair_exports_as_curve_geometry() {
    let path = std::env::temp_dir().join(format!("pfx_hair_{}.rdla", std::process::id()));
    let text = export(&path, groomed(true));
    assert!(text.contains("RdlMeshGeometry(\"/scene/objects/mesh0\") {"));
    assert!(text.contains("RdlCurveGeometry(\"/scene/objects/curves0\") {"));
    assert_eq!(text.matches("{RdlCurveGeometry(\"/scene/objects/curves0\")").count(), 1, "curves are in the layer");
    // Hair alone has no triangles: no mesh object is written for it.
    let text = export(&path, groom::hair_guides(&skin(), &groom::GuideSettings { count: 3, length: 1.0, length_variance: 0.0, segments: 2, droop: 0.0, seed: 1 }));
    assert!(!text.contains("RdlMeshGeometry"));
    assert!(text.contains("[\"curves_vertex_count\"] = {\n        3,\n        3,\n        3,\n    }"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn hair_deform_node_follows_an_animated_skin() {
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 6 }, egui::pos2(0.0, 0.0));
    // The skin rises one unit per second.
    let skin = g.add(NodeKind::Wrangle { code: "@P.y += @Time;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    let guides = g.add(
        NodeKind::HairGuides(groom::GuideSettings { count: 20, length: 0.5, length_variance: 0.0, segments: 4, droop: 0.0, seed: 1 }),
        egui::pos2(0.0, 0.0),
    );
    let deform = g.add(NodeKind::HairDeform(groom::DeformSettings { rest_frame: 1.0 }), egui::pos2(0.0, 0.0));
    g.connect(grid, skin, 0);
    g.connect(skin, guides, 0);
    g.connect(guides, deform, 0);
    g.connect(skin, deform, 1);
    assert!(g.time_dependent(deform));
    let rest = g.cook(deform, 0.0, &mut HashMap::new());
    let later = g.cook(deform, 1.5, &mut HashMap::new());
    assert!(!rest.vertices.is_empty());
    assert_eq!(rest.vertices.len(), later.vertices.len());
    for (a, b) in rest.vertices.iter().zip(&later.vertices) {
        assert!((b.y - a.y - 1.5).abs() < 1e-3 && (b.x - a.x).abs() < 1e-3, "{a:?} -> {b:?}");
    }
    // Without a skin the node reports an error instead of cooking.
    let lonely = g.add(NodeKind::HairDeform(groom::DeformSettings { rest_frame: 1.0 }), egui::pos2(0.0, 0.0));
    g.connect(guides, lonely, 0);
    assert!(g.cook(lonely, 0.0, &mut HashMap::new()).vertices.is_empty());
    assert!(node_errors(|e| e.get(&lonely).cloned()).is_some_and(|e| e.contains("input 2")));
}

// Renders with the bundled MoonRay: `cargo test -- --ignored hair_renders_in_moonray`.
#[test]
#[ignore]
fn hair_renders_in_moonray() {
    let moonray = std::path::Path::new("native/moonray/macos/moonray");
    let dir = std::env::temp_dir();
    let render = |hair: bool| {
        let rdla = dir.join(format!("pfx_hair_render_{hair}.rdla"));
        let exr = dir.join(format!("pfx_hair_render_{hair}.exr"));
        export(&rdla, groomed(hair));
        let status = Command::new(moonray)
            .env("RDL2_DSO_PATH", std::env::var("RDL2_DSO_PATH").unwrap_or("/Users/loksangho/moonray/build/rdl2dso".into()))
            .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
        image::open(&exr).unwrap().to_rgb32f()
    };
    let (bare, hairy) = (render(false), render(true));
    let changed = bare.pixels().zip(hairy.pixels()).filter(|(a, b)| (0..3).any(|c| (a[c] - b[c]).abs() > 0.02)).count();
    println!("{changed} of {} pixels changed by the hair", bare.pixels().len());
    assert!(changed > bare.pixels().len() / 50);
}
