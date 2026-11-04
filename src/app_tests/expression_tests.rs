use crate::*;

#[test]
fn expressions_evaluate_with_frame_variables() {
    // Frames count from 1 at time 0, at SIM_FPS.
    let frame_of = |t: f32| t * sims::SIM_FPS + 1.0;
    assert_eq!(vex::eval_expression("$F", 0.0), Ok(1.0));
    assert_eq!(vex::eval_expression("$F", 1.0), Ok(frame_of(1.0)));
    let t = 0.52;
    assert!((vex::eval_expression("$FF * 2", t).unwrap() - frame_of(t) * 2.0).abs() < 1e-4);
    assert!((vex::eval_expression("sin($T) + $FPS", t).unwrap() - (t.sin() + sims::SIM_FPS)).abs() < 1e-4);
    assert_eq!(vex::eval_expression(" 7.5 ", 3.0), Ok(7.5));
    assert!(vex::eval_expression("fit($F, 1, 11, 0, 1)", 0.0).is_ok());
    assert!(vex::eval_expression("nosuchfunction($F)", 0.0).is_err());
    assert!(vex::eval_expression("$F +", 0.0).is_err());
}

#[test]
fn typing_an_expression_into_a_field_makes_the_parameter_follow_it() {
    let parse = typed_parser(3.0, true);
    EXPRESSION_TIME.with(|t| t.set(1.0 / sims::SIM_FPS));
    assert_eq!(parse("$FF"), Some(2.0));
    assert!(matches!(TYPED.with(|t| t.borrow_mut().take()), Some(Typed::Expression(e)) if e == "$FF"));
    // Same value as the field: still a change, so the field can be found.
    assert_eq!(typed_parser(2.0, true)("$F"), Some(3.0));
    assert_eq!(parse("12"), Some(12.0));
    assert!(matches!(TYPED.with(|t| t.borrow_mut().take()), Some(Typed::Number)));
    assert_eq!(parse("$F +"), None);

    // The panel gives the expression to the parameter the typing changed (Scatter's seed).
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let scatter = app.graph.add(NodeKind::Scatter(sops::ScatterSettings { count: 20, seed: 3, relax_iterations: 0, volume: false }), egui::pos2(0.0, 0.0));
    let before = (scatter, app.graph.nodes[&scatter].kind.params(), app.graph.nodes[&scatter].xform);
    if let NodeKind::Scatter(s) = &mut app.graph.nodes.get_mut(&scatter).unwrap().kind {
        s.seed = 2;
    }
    app.apply_typed(before.clone(), Typed::Expression("$FF".into()));
    let seed = ChannelTarget::Param(before.1.iter().position(|v| *v == 3.0).unwrap());
    let channel = ChannelId { node: scatter, target: seed };
    assert_eq!(app.graph.channels[&channel].expression.as_deref(), Some("$FF"));
    let seed_at = |g: &Graph, t: f32| match g.kind_at(scatter, t).as_ref() { NodeKind::Scatter(s) => s.seed, _ => 0 };
    assert_eq!((seed_at(&app.graph, 0.0), seed_at(&app.graph, 2.0 / sims::SIM_FPS)), (1, 3));
    // Typing a number replaces the expression.
    let before = (scatter, app.graph.nodes[&scatter].kind.params(), app.graph.nodes[&scatter].xform);
    if let NodeKind::Scatter(s) = &mut app.graph.nodes.get_mut(&scatter).unwrap().kind {
        s.seed = 7;
    }
    app.apply_typed(before, Typed::Number);
    assert!(!app.graph.channels.contains_key(&channel));
    // Delete Channel on the field showing the seed: the expression goes, the value stays.
    app.graph.channels.entry(channel).or_default().expression = Some("$F * 5".into());
    app.graph.apply_animation(0.0);
    let shown = match &app.graph.nodes[&scatter].kind { NodeKind::Scatter(s) => s.seed as f32, _ => 0.0 };
    assert_eq!(shown, 5.0);
    let before = (scatter, app.graph.nodes[&scatter].kind.params(), app.graph.nodes[&scatter].xform);
    app.apply_typed(before.clone(), Typed::Delete(4.0));
    assert!(app.graph.channels.contains_key(&channel), "a field with another value isn't it");
    app.apply_typed(before, Typed::Delete(shown));
    assert!(!app.graph.channels.contains_key(&channel));
    assert!(matches!(&app.graph.nodes[&scatter].kind, NodeKind::Scatter(s) if s.seed == 5));
}

#[test]
fn wrangles_have_four_inputs() {
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 1.0, res: 4 }, egui::pos2(0.0, 0.0));
    let sphere = g.add(NodeKind::Sphere { radius: 2.0, rows: 16, cols: 32 }, egui::pos2(0.0, 0.0));
    let wrangle = g.add(NodeKind::Wrangle { code: "// Onto the sphere in input 1.\n@P = minpos(1, @P);".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    assert_eq!(g.nodes[&wrangle].kind.num_inputs(), 4);
    g.connect(grid, wrangle, 0);
    g.connect(sphere, wrangle, 3);
    // Wired to the fourth input, the sphere is input 3 to VEX; input 1 is empty.
    let out = g.cook(wrangle, 0.0, &mut HashMap::new());
    assert!(out.vertices.iter().all(|p| p.y == 0.0), "input 1 is empty, so the points stay");
    g.disconnect(wrangle, 3);
    g.connect(sphere, wrangle, 1);
    let out = g.cook(wrangle, 0.0, &mut HashMap::new());
    assert!(out.vertices.iter().all(|p| (p.length() - 2.0).abs() < 0.05), "on the sphere");
    // A Volume Wrangle samples a second volume.
    let fog = |size: f32| volume::to_mesh(&volume::to_fog(&volume::from_polygons(&node_box(size, 1), 0.1, 3.0).unwrap()));
    let out = fields::volume_wrangle(fog(1.0), &[fog(3.0)], "@density = volumesample(1, \"density\", @P + set(1.2, 0, 0));", 0.0, 1.0, None).unwrap();
    assert!(out.point_attributes["density"].iter().any(|d| *d > 0.5), "the bigger box reaches 1.2 further");
    let alone = fields::volume_wrangle(fog(1.0), &[], "@density = volumesample(1, \"density\", @P);", 0.0, 1.0, None).unwrap();
    assert!(alone.point_attributes["density"].iter().all(|d| *d == 0.0), "nothing connected: nothing there");
}

#[test]
fn a_switch_expression_chooses_the_input_by_frame() {
    let mut g = Graph::default();
    let switch = g.add(NodeKind::Switch { index: 0 }, egui::pos2(0.0, 0.0));
    let grids: Vec<NodeId> = [2, 4, 8].iter().map(|&res| g.add(NodeKind::Grid { size: 2.0, res }, egui::pos2(0.0, 0.0))).collect();
    for &grid in &grids {
        // Each drop adds an input, as on a Merge.
        let slot = g.drop_slot(switch).unwrap();
        g.connect(grid, switch, slot);
    }
    assert_eq!(g.input_slots(switch), 4, "three inputs and a free one");
    let (input, _) = g.nodes[&switch].kind.animatable().into_iter().find(|(_, n)| n == "input").unwrap();
    g.channels.entry(ChannelId { node: switch, target: input }).or_default().expression = Some("$F - 1".into());
    let count = |g: &Graph, frame: f32| g.cook(switch, (frame - 1.0) / sims::SIM_FPS, &mut HashMap::new()).vertices.len();
    let grid_count: Vec<usize> = grids.iter().map(|&n| g.cook(n, 0.0, &mut HashMap::new()).vertices.len()).collect();
    assert_eq!([count(&g, 1.0), count(&g, 2.0), count(&g, 3.0)], [grid_count[0], grid_count[1], grid_count[2]]);
    assert_eq!(count(&g, 9.0), grid_count[2], "past the last input, the last");
}

#[test]
fn copy_to_points_picks_random_pieces() {
    // Three shapes merged, numbered as pieces, copied onto points with random variants.
    let mut g = Graph::default();
    let shapes = [NodeKind::Box { size: 0.2, div: 1 }, NodeKind::Sphere { radius: 0.1, rows: 6, cols: 8 }, NodeKind::Torus { major: 0.1, minor: 0.03, rows: 6, cols: 8 }];
    let merge = g.add(NodeKind::Merge, egui::pos2(0.0, 0.0));
    let mut sizes = Vec::new();
    for (slot, shape) in shapes.into_iter().enumerate() {
        let n = g.add(shape, egui::pos2(0.0, 0.0));
        sizes.push(g.cook(n, 0.0, &mut HashMap::new()).indices.len());
        g.connect(n, merge, slot);
    }
    let connectivity = g.add(NodeKind::Connectivity(sopkit::ConnectivitySettings::defaults()), egui::pos2(0.0, 0.0));
    g.connect(merge, connectivity, 0);
    let pieces = g.cook(connectivity, 0.0, &mut HashMap::new());
    // Enumerate the pieces by Connectivity's class into `variant`.
    let numbered = enumerate::enumerate(&pieces, &enumerate::EnumerateSettings { class: 1, attribute: "variant".into(), piece_attribute: "class".into(), ..enumerate::EnumerateSettings::defaults() }).unwrap();
    let mut points = Mesh { vertices: (0..6).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect(), ..Default::default() };
    points.point_attributes.insert("variant".into(), vec![2.0, 0.0, 1.0, 1.0, 7.0, 0.0]);
    let copies = node_copy_pieces_to_points(numbered.clone(), points.clone(), 1.0, [0.0; 3], "variant").unwrap();
    // Variant 7 matches no piece: nothing there.
    assert_eq!(copies.indices.len(), sizes[2] + sizes[0] + sizes[1] * 2 + sizes[0]);
    // The torus went to the first point, at x = 0.
    let first_piece: Vec<Vec3> = copies.vertices.iter().copied().filter(|v| v.x.abs() < 0.5).collect();
    assert!(first_piece.iter().any(|v| v.y.abs() < 1e-4 && (v.x.hypot(v.z) - 0.13).abs() < 1e-3), "a torus at the first point");
    assert!(node_copy_pieces_to_points(numbered.clone(), Mesh { vertices: vec![Vec3::zero()], ..Default::default() }, 1.0, [0.0; 3], "variant").is_err());
    // Empty: every point gets everything.
    assert_eq!(node_copy_pieces_to_points(numbered, points, 1.0, [0.0; 3], "").unwrap().indices.len(), 6 * sizes.iter().sum::<usize>());
}

#[test]
fn a_seed_expression_changes_the_scatter_every_frame() {
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 4 }, egui::pos2(0.0, 0.0));
    let scatter = g.add(NodeKind::Scatter(sops::ScatterSettings { count: 20, seed: 3, relax_iterations: 0, volume: false }), egui::pos2(0.0, 0.0));
    g.connect(grid, scatter, 0);
    let (seed, _) = g.nodes[&scatter].kind.animatable().into_iter().find(|(_, n)| n == "seed").unwrap();
    let channel = ChannelId { node: scatter, target: seed };
    g.channels.entry(channel).or_default().expression = Some("$F * 10".into());
    let seed_at = |g: &Graph, t: f32| match g.kind_at(scatter, t).as_ref() {
        NodeKind::Scatter(s) => s.seed,
        _ => unreachable!(),
    };
    assert_eq!((seed_at(&g, 0.0), seed_at(&g, 1.0 / sims::SIM_FPS)), (10, 20));
    let a = g.cook(scatter, 0.0, &mut HashMap::new());
    let b = g.cook(scatter, 1.0 / sims::SIM_FPS, &mut HashMap::new());
    assert_ne!(a.vertices, b.vertices, "a new seed each frame");
    // Saved with the scene, characters and all; edits don't key over it.
    g.channels.get_mut(&channel).unwrap().expression = Some("floor($FF) * 0.5".into());
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(loaded.channels[&channel].expression.as_deref(), Some("floor($FF) * 0.5"));
    let text = anim::Channel { keys: Vec::new(), expression: Some("a; b c".into()) }.to_text();
    assert_eq!(anim::Channel::from_text(&text).expression.as_deref(), Some("a; b c"));
    g.apply_animation(0.0);
    if let NodeKind::Scatter(s) = &mut g.nodes.get_mut(&scatter).unwrap().kind {
        s.seed = 99;
    }
    assert!(!g.commit_animation_edits(0.0, true));
    assert!(g.channels[&channel].keys.is_empty());
}
