use crate::*;

#[test]
fn scenes_with_the_old_solver_name_still_open() {
    let old = NodeKind::from_params("vellum_solver", &[]).expect("the old name still loads");
    assert_eq!(old.name(), "cloth_solver");
    assert!(NodeKind::templates().iter().flat_map(|(cat, _)| cat.split_whitespace()).all(|w| !w.eq_ignore_ascii_case("kinefx")));
}

#[test]
fn node_info_shows_the_node_and_its_geometry() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let make = |app: &mut ProceduralFxStudioApp, kind: NodeKind| {
        let id = app.graph.add(kind, egui::pos2(0.0, 0.0));
        app.graph.nodes.get_mut(&id).unwrap().parent = Some(geo);
        id
    };
    let cube = make(&mut app, NodeKind::Box { size: 1.0, div: 1 });
    let tag = make(&mut app, NodeKind::Wrangle { code: "v@vel = set(0, 1, 0); i@up = @P.y > 0; setpointgroup(0, \"top\", @ptnum, @P.y > 0);".into(), domain: 2 });
    app.graph.connect(cube, tag, 0);
    let ctx = egui::Context::default();
    let mut texts = Vec::new();
    for _ in 0..2 {
        let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 900.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.node_info_ui(ui, tag));
        });
        texts.clear();
        for clipped in output.shapes {
            fn flatten(shape: egui::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Vec(v) => v.into_iter().for_each(|s| flatten(s, out)),
                    egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                    _ => {}
                }
            }
            flatten(clipped.shape, &mut texts);
        }
    }
    let has = |s: &str| texts.iter().any(|t| t.contains(s));
    for expected in ["Type", "Wrangle", "Inputs", "1 of 4 connected", "Points", "Primitives", "6 (12 triangles)", "Bounds", "Attributes", "vel (3)", "up (1)", "Groups", "top (4)"] {
        assert!(has(expected), "'{expected}' missing from {texts:?}");
    }
}
