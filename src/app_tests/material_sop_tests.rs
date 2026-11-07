use crate::*;

#[test]
fn material_nodes_assign_materials_to_groups() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let red = app.graph.add(NodeKind::Material { kind: 0, color: [1.0, 0.0, 0.0], roughness: 0.5, value: 0.5 }, egui::pos2(0.0, 0.0));
    let gold = app.graph.add(NodeKind::Shader(shading::Values::preset(shading::Table::Principled, 8)), egui::pos2(0.0, 0.0));
    for m in [red, gold] {
        app.graph.nodes.get_mut(&m).unwrap().context = AppContext::Mat;
    }
    let geo = app.graph.add(NodeKind::Geometry { material: Some(red) }, egui::pos2(0.0, 0.0));
    let child = |app: &mut ProceduralFxStudioApp, kind: NodeKind| {
        let id = app.graph.add(kind, egui::pos2(0.0, 0.0));
        app.graph.nodes.get_mut(&id).unwrap().parent = Some(geo);
        id
    };
    let grid = child(&mut app, NodeKind::Grid { size: 2.0, res: 4 });
    let assign = child(&mut app, NodeKind::MaterialAssign { slots: vec![("0-5".into(), Some(gold))] });
    app.graph.connect(grid, assign, 0);
    app.graph.set_display(assign);
    let cooked = app.graph.cook(assign, 0.0, &mut HashMap::new());
    let ids = &cooked.point_attributes[PRIM_MATERIAL];
    // "0-5" is the first six quads: twelve triangles.
    assert_eq!(ids.len(), 18);
    assert!(ids[..12].iter().all(|v| *v == gold as f32 + 1.0) && ids[12..].iter().all(|v| *v == 0.0));
    // Render: two parts, gold on the group and the object's red on the rest.
    app.render_at = Some(0.0);
    let scene = app.render_scene().unwrap();
    let mut parts: Vec<(usize, Option<NodeId>)> = scene.parts.iter().map(|(m, pm)| (m.indices.len(), match pm { PartMaterial::Node(n) => Some(*n), _ => None })).collect();
    parts.sort();
    assert_eq!(parts, vec![(6, Some(red)), (12, Some(gold))]);
    // Viewport: each primitive shows its own material's colour.
    let shown = app.graph.cook_display(&[geo], 0.0, &mut HashMap::new());
    let cd = &shown.point_attributes["prim:Cd"];
    assert!(cd[0] > 0.9 && cd[1] > 0.6, "gold first: {:?}", &cd[..3]);
    assert_eq!(&cd[17 * 3..], &[1.0, 0.0, 0.0], "the rest is the object's red");
    // USD: one Mesh child per material.
    let import = app.graph.add(NodeKind::LopSopImport { geometry: Some(geo), path: "/geo".into() }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&import).unwrap().context = AppContext::Stage;
    let stage = app.graph.cook_stage(import, 0.0, &mut HashMap::new());
    let meshes: Vec<String> = stage.flatten().into_iter().map(|(path, ..)| path).collect();
    assert_eq!(meshes.len(), 2, "{meshes:?}");
    assert!(meshes.iter().any(|p| p == &format!("/geo/mat{gold}")));
    // Saved and loaded with its groups, materials and node names.
    app.graph.nodes.get_mut(&assign).unwrap().name = "shirt_materials".into();
    let loaded = Graph::deserialize(&app.graph.serialize());
    assert!(matches!(&loaded.nodes[&assign].kind, NodeKind::MaterialAssign { slots } if slots == &vec![("0-5".to_owned(), Some(gold))]));
    assert_eq!(loaded.nodes[&assign].title(assign), "shirt_materials");
    assert_eq!(loaded.nodes[&grid].title(grid), format!("grid{grid}"));
    // A later Material node with an empty group overrides everything.
    let all = child(&mut app, NodeKind::MaterialAssign { slots: vec![(String::new(), Some(red))] });
    app.graph.connect(assign, all, 0);
    let every = app.graph.cook(all, 0.0, &mut HashMap::new());
    assert!(every.point_attributes[PRIM_MATERIAL].iter().all(|v| *v == red as f32 + 1.0));
}

#[test]
fn geometry_menus_have_no_shading_networks_and_search_matches_words() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    app.apply(Action::Enter(geo));
    let menus = app.templates_for_view();
    let kinds: Vec<&NodeKind> = menus.iter().flat_map(|(_, k)| k).collect();
    assert!(kinds.iter().all(|k| !matches!(k, NodeKind::ShaderNode(_) | NodeKind::Shader(_) | NodeKind::MaterialBuilder | NodeKind::Material { .. })));
    assert!(kinds.iter().any(|k| matches!(k, NodeKind::MaterialAssign { .. })));
    assert!(!menus.iter().any(|(c, _)| *c == shading::CATEGORY));
    let find = |q: &str| menus.iter().flat_map(|(c, ks)| ks.iter().map(move |k| (*c, k))).filter(|(c, k)| node_matches(k, c, q)).map(|(_, k)| k.menu_name()).collect::<Vec<_>>();
    assert!(find("noise").iter().any(|n| n == "noise"));
    assert!(find("terrain erode").iter().any(|n| n == "terrain_erode"));
    assert!(find("terrainerode").iter().any(|n| n == "terrain_erode"));
    assert!(find("rigging ik").iter().any(|n| n == "two_bone_ik"), "category words match too");
    assert!(find("zzzz").is_empty());
}

// A material that doesn't use Cd shows its own colour over a model's corner colours (as a
// USDZ's texture brings them); one that does keeps them.
#[test]
fn an_objects_material_colours_over_a_models_corner_colours() {
    let mut g = Graph::default();
    // A Principled Shader, blue, not using point colour.
    let mut shader = shading::Values::new(shading::Table::Principled);
    shader.set("base_color", [0.0, 0.3, 1.0]);
    let blue = g.add(NodeKind::Shader(shader), egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&blue).unwrap().context = AppContext::Mat;
    let geo = g.add(NodeKind::Geometry { material: Some(blue) }, egui::pos2(0.0, 0.0));
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 3 }, egui::pos2(0.0, 0.0));
    let gold = g.add(NodeKind::Wrangle { code: "v@Cd = set(1, 0.8, 0.2);".into(), domain: 3 }, egui::pos2(0.0, 80.0));
    for id in [grid, gold] {
        g.nodes.get_mut(&id).unwrap().parent = Some(geo);
    }
    g.connect(grid, gold, 0);
    g.set_display(gold);
    let raw = g.cook(gold, 0.0, &mut HashMap::new());
    assert!(raw.point_attributes.contains_key("vertex:Cd"), "the corners have their own colour");
    let shown = g.cook_display(&[geo], 0.0, &mut HashMap::new());
    let corners = &shown.point_attributes["vertex:Cd"];
    assert!(corners.chunks(3).all(|c| c[2] > 0.5 && c[0] < 0.2), "the material's colour wins on every corner: {:?}", &corners[..3]);
    // Loose points beside the surface (glow splats) keep their own colour.
    let mut g2 = g.clone();
    if let Some(NodeKind::Wrangle { code, .. }) = g2.nodes.get_mut(&gold).map(|n| &mut n.kind) {
        *code = "v@Cd = set(1, 0.8, 0.2); int p = addpoint(0, {0, 2, 0}); setpointattrib(0, \"Cd\", p, {1, 0.8, 0.2});".into();
    }
    let with_points = g2.cook_display(&[geo], 0.0, &mut HashMap::new());
    let cd = &with_points.point_attributes["Cd"];
    assert_eq!(&cd[cd.len() - 3..], &[1.0, 0.8, 0.2], "the loose point keeps its gold");
}
