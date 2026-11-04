use crate::*;

// A lit quad with UVs and point colour, facing the default camera.
fn panel() -> Mesh {
    let grid = node_grid(2.0, 2.0, 6, 6);
    let mut mesh = sops::uv_texture(&grid, &sops::UvTextureSettings { projection: 0, axis: 1, scale: [1.0, 1.0], offset: [0.0, 0.0] });
    let cd: Vec<f32> = mesh.vertices.iter().flat_map(|v| [0.5 + v.x * 0.25, 0.3, 0.5 - v.z * 0.25]).collect();
    mesh.point_attributes.insert("Cd".into(), cd);
    mesh
}

// An app whose graph holds a Principled shader, a Toon shader and a builder network.
fn shaded_app() -> (ProceduralFxStudioApp, NodeId, NodeId, NodeId) {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let mut gold = shading::Values::preset(shading::Table::Principled, shading::PRINCIPLED_PRESETS.iter().position(|p| p.0 == "Gold").unwrap());
    gold.set("use_point_color", [1.0, 0.0, 0.0]);
    let principled = app.graph.add(NodeKind::Shader(gold), egui::pos2(0.0, 0.0));
    let toon = app.graph.add(NodeKind::Shader(shading::Values::preset(shading::Table::Toon, 1)), egui::pos2(0.0, 0.0));
    let builder = app.graph.add(NodeKind::MaterialBuilder, egui::pos2(0.0, 0.0));
    let child = |app: &mut ProceduralFxStudioApp, op: usize| {
        let id = app.graph.add(NodeKind::ShaderNode(shading::ShaderNode::new(op)), egui::pos2(0.0, 0.0));
        let n = app.graph.nodes.get_mut(&id).unwrap();
        n.context = AppContext::Mat;
        n.parent = Some(builder);
        id
    };
    let output = child(&mut app, shading::SURFACE_OUTPUT);
    let noise = child(&mut app, 4);
    let cells = child(&mut app, 5);
    let mix = child(&mut app, 8);
    let hsv = child(&mut app, 9);
    app.graph.connect(noise, mix, 0);
    app.graph.connect(cells, mix, 2);
    app.graph.connect(mix, hsv, 0);
    app.graph.connect(hsv, output, 0);
    app.graph.connect(cells, output, 1);
    (app, principled, toon, builder)
}

#[test]
fn principled_opacity_follows_point_alpha() {
    let (mut app, principled, _, _) = shaded_app();
    if let NodeKind::Shader(v) = &mut app.graph.nodes.get_mut(&principled).unwrap().kind {
        v.set("use_point_alpha", [1.0, 0.0, 0.0]);
        v.set("opacity", [0.8, 0.0, 0.0]);
    }
    let mut faded = panel();
    let n = faded.vertices.len();
    faded.point_attributes.insert("Alpha".into(), (0..n).map(|i| i as f32 / n as f32).collect());
    let scene = RenderScene { parts: vec![(faded, PartMaterial::Node(principled))], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
    let path = std::env::temp_dir().join(format!("pfx_alpha_{}.rdla", std::process::id()));
    app.export_scene_rdla(&path, &scene, 64, 48, 4).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let map = format!("/scene/materials/mat{principled}_point_alpha");
    assert!(text.contains(&format!("[\"presence\"] = bind(AttributeMap(\"{map}\"))")), "{text}");
    // The map reads the float Alpha, with Opacity where there's none.
    assert!(text.contains(&format!("AttributeMap(\"{map}\") {{\n    [\"primitive_attribute_name\"] = \"Alpha\",\n    [\"primitive_attribute_type\"] = 0,")));
    assert!(text.contains("Rgb(0.8000, 0.8000, 0.8000)") || text.contains("Rgb(0.8, 0.8, 0.8)"));
    // The mesh carries Alpha (and Cd) as user data.
    assert!(text.contains("[\"primitive_attributes\"] = { UserData(\"/scene/objects/mesh0_Cd\"), UserData(\"/scene/objects/mesh0_Alpha\") }") || text.contains("[\"primitive_attributes\"] = { UserData(\"/scene/objects/mesh0_Alpha\") }"), "{text}");
    assert!(text.contains("UserData(\"/scene/objects/mesh0_Alpha\") {\n    [\"float_key\"] = \"Alpha\""));
    // Off, a constant presence as before.
    if let NodeKind::Shader(v) = &mut app.graph.nodes.get_mut(&principled).unwrap().kind {
        v.set("use_point_alpha", [0.0, 0.0, 0.0]);
    }
    app.export_scene_rdla(&path, &scene, 64, 48, 4).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("[\"presence\"] = 0.8000"));
}

#[test]
fn shaders_and_builders_export_to_rdla() {
    let (app, principled, toon, builder) = shaded_app();
    let scene = RenderScene {
        parts: vec![(panel(), PartMaterial::Node(principled)), (panel(), PartMaterial::Node(toon)), (panel(), PartMaterial::Node(builder))],
        lights: app.scene_lights(),
        environment: None,
        camera: None,
        volumes: Vec::new(),
    };
    let path = std::env::temp_dir().join(format!("pfx_shaders_{}.rdla", std::process::id()));
    app.export_scene_rdla(&path, &scene, 64, 48, 4).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(&format!("DwaBaseMaterial(\"/scene/materials/mat{principled}\") {{")));
    assert!(text.contains(&format!("[\"albedo\"] = bind(AttributeMap(\"/scene/materials/mat{principled}_point_color\"))")));
    assert!(text.contains(&format!("DwaToonMaterial(\"/scene/materials/mat{toon}\") {{")));
    assert!(text.contains("NoiseMap_v2(") && text.contains("BlendMap(") && text.contains("ColorCorrectHsvMap("));
    assert!(text.contains("[\"uv_list\"] = {") && text.contains("UserData(\"/scene/objects/mesh0_Cd\") {"));
    // Each part's layer entry uses its own material.
    assert!(text.contains(&format!("DwaBaseMaterial(\"/scene/materials/mat{builder}\")}}").replace("}", "")) || text.contains(&format!("DwaBaseMaterial(\"/scene/materials/mat{builder}\"), LightSet")));
    // The viewport shows each material's colour.
    let gold = app.graph.material_preview(principled).unwrap();
    assert!(gold.0[0] > 0.9 && gold.2, "gold uses Cd: {gold:?}");
    assert!(app.graph.material_preview(builder).is_some());
    // Saving keeps shader nodes and the builder's network.
    let loaded = Graph::deserialize(&app.graph.serialize());
    for (id, node) in &app.graph.nodes {
        assert_eq!(format!("{:?}", loaded.nodes[id].kind), format!("{:?}", node.kind));
        assert_eq!(loaded.nodes[id].parent, node.parent);
    }
}

// Renders every shader in MoonRay: `cargo test --release -- --ignored every_shader_renders`.
#[test]
#[ignore]
fn every_shader_renders_in_moonray() {
    let dir = std::env::temp_dir();
    let texture = dir.join("pfx_shader_texture.png");
    image::RgbImage::from_fn(32, 32, |x, y| if (x / 8 + y / 8) % 2 == 0 { image::Rgb([230, 60, 40]) } else { image::Rgb([40, 60, 230]) }).save(&texture).unwrap();
    let (mut app, ..) = shaded_app();
    let mut materials: Vec<NodeId> = app.graph.nodes.iter().filter(|(_, n)| n.kind.is_material()).map(|(id, _)| *id).collect();
    for table in [shading::Table::Principled, shading::Table::Toon, shading::Table::Hair] {
        for i in 0..table.presets().len() {
            materials.push(app.graph.add(NodeKind::Shader(shading::Values::preset(table, i)), egui::pos2(0.0, 0.0)));
        }
    }
    let mut textured = shading::Values::new(shading::Table::Principled);
    textured.textures = vec![texture.to_string_lossy().into_owned(), texture.to_string_lossy().into_owned(), String::new()];
    materials.push(app.graph.add(NodeKind::Shader(textured), egui::pos2(0.0, 0.0)));
    // A builder using every other node type, including an image and a normal map.
    let builder = app.graph.add(NodeKind::MaterialBuilder, egui::pos2(0.0, 0.0));
    let file = texture.to_string_lossy().into_owned();
    let mut child = |op: usize| {
        let mut node = shading::ShaderNode::new(op);
        node.texts.iter_mut().for_each(|t| *t = file.clone());
        if op == 3 {
            node.texts[0] = "Cd".into();
        }
        let id = app.graph.add(NodeKind::ShaderNode(node), egui::pos2(0.0, 0.0));
        let n = app.graph.nodes.get_mut(&id).unwrap();
        n.context = AppContext::Mat;
        n.parent = Some(builder);
        id
    };
    let (output, image, normal, attribute, checker, ramp, mix) = (child(0), child(1), child(2), child(3), child(6), child(7), child(8));
    app.graph.connect(image, mix, 0);
    app.graph.connect(attribute, mix, 1);
    app.graph.connect(checker, ramp, 0);
    app.graph.connect(ramp, mix, 2);
    app.graph.connect(mix, output, 0);
    app.graph.connect(normal, output, 10);
    materials.push(builder);
    app.render_camera = RenderCamera::Viewport;
    for material in materials {
        let scene = RenderScene { parts: vec![(panel(), PartMaterial::Node(material))], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
        let rdla = dir.join("pfx_shader.rdla");
        let exr = dir.join("pfx_shader.exr");
        let _ = std::fs::remove_file(&exr);
        app.export_scene_rdla(&rdla, &scene, 48, 36, 4).unwrap();
        let run = Command::new("native/moonray/macos/moonray")
            .env("RDL2_DSO_PATH", "/Users/loksangho/moonray/build/rdl2dso")
            .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
            .output()
            .unwrap();
        let label = app.graph.nodes[&material].title(material);
        let log = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
        assert!(run.status.success(), "{label}: {log}");
        for bad in ["Error", "ERROR", "does not exist", "no attribute", "Unknown"] {
            assert!(!log.contains(bad), "{label}: {log}");
        }
        let render = image::open(&exr).unwrap().to_rgb32f();
        let lit = render.pixels().map(|p| p[0] + p[1] + p[2]).fold(0.0f32, f32::max);
        println!("{label}: brightest {lit:.3}");
        assert!(lit > 0.01, "{label} rendered black");
    }
}
