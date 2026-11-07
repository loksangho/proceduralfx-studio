use crate::*;

fn vop(op: usize, f: impl FnOnce(&mut vop::VopNode)) -> NodeKind {
    let mut v = vop::VopNode::new(op);
    f(&mut v);
    NodeKind::Vop(v)
}

fn op(label: &str) -> usize {
    vop::OPS.iter().position(|o| o.label == label).unwrap()
}

// grid -> Attribute VOP { P + (0, 1, 0) * ch("amp") -> P }
fn lift_network() -> (Graph, NodeId, NodeId) {
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 3 }, egui::pos2(0.0, 0.0));
    let net = g.add(NodeKind::AttributeVop { domain: 2 }, egui::pos2(0.0, 0.0));
    g.connect(grid, net, 0);
    let child = |g: &mut Graph, kind: NodeKind| {
        let id = g.add(kind, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&id).unwrap().parent = Some(net);
        id
    };
    let p = child(&mut g, vop(vop::BIND, |_| {}));
    let up = child(&mut g, vop(vop::CONSTANT, |v| {
        v.ty = 1;
        v.value = [0.0, 1.0, 0.0];
    }));
    let amp = child(&mut g, vop(vop::PARAMETER, |v| v.name = "amp".into()));
    let mul = child(&mut g, vop(op("Multiply"), |_| {}));
    let add = child(&mut g, vop(op("Add"), |_| {}));
    let out = child(&mut g, vop(vop::EXPORT, |_| {}));
    g.connect(up, mul, 0);
    g.connect(amp, mul, 1);
    g.connect(p, add, 0);
    g.connect(mul, add, 1);
    g.connect(add, out, 0);
    g.nodes.get_mut(&net).unwrap().channels.insert("amp".into(), [2.0, 0.0, 0.0]);
    (g, grid, net)
}

#[test]
fn attribute_vop_runs_its_network() {
    let (mut g, grid, net) = lift_network();
    let before = g.cook(grid, 0.0, &mut HashMap::new());
    let after = g.cook(net, 0.0, &mut HashMap::new());
    for (a, b) in before.vertices.iter().zip(&after.vertices) {
        assert!((b.y - a.y - 2.0).abs() < 1e-6 && b.x == a.x);
    }
    let code = g.vop_code(net).unwrap();
    assert!(code.contains("chf(\"amp\")") && code.contains("v@P = vector("), "{code}");
    assert!(!g.time_dependent(net));
    // The group limits it like a wrangle's.
    g.nodes.get_mut(&net).unwrap().group = groups::NodeGroup { pattern: "0-3".into(), kind: 1 };
    let grouped = g.cook(net, 0.0, &mut HashMap::new());
    assert!(grouped.vertices.iter().enumerate().all(|(i, v)| (v.y - if i <= 3 { 2.0 } else { 0.0 }).abs() < 1e-6));
    // Saved and loaded intact, parameter value included.
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(loaded.vop_code(net).unwrap(), code);
    assert_eq!(loaded.nodes[&net].channels["amp"], [2.0, 0.0, 0.0]);
    assert!(loaded.cook(net, 0.0, &mut HashMap::new()).vertices[0].y > 1.9);
}

#[test]
fn attribute_vops_read_their_other_inputs() {
    // The lift network, but its Bind reads P from a raised copy wired into input 1.
    let (mut g, grid, net) = lift_network();
    assert_eq!(g.nodes[&net].kind.num_inputs(), 4);
    let raised = g.add(NodeKind::Transform { t: [0.0, 5.0, 0.0], r: [0.0; 3], s: [1.0; 3], p: [0.0; 3] }, egui::pos2(0.0, 0.0));
    g.connect(grid, raised, 0);
    g.connect(raised, net, 1);
    let bind = g.children(net).into_iter().find(|c| matches!(&g.nodes[c].kind, NodeKind::Vop(v) if v.op == vop::BIND)).unwrap();
    if let NodeKind::Vop(v) = &mut g.nodes.get_mut(&bind).unwrap().kind {
        v.input = 1;
    }
    let after = g.cook(net, 0.0, &mut HashMap::new());
    assert!(after.vertices.iter().all(|p| (p.y - 7.0).abs() < 1e-5), "input 1's P (5) plus the lift (2)");
    // The choice is saved with the scene.
    let loaded = Graph::deserialize(&g.serialize());
    assert!(matches!(&loaded.nodes[&bind].kind, NodeKind::Vop(v) if v.input == 1));
}

#[test]
fn vop_menus_name_each_operation() {
    let mut app = ProceduralFxStudioApp::default();
    let (g, _, net) = lift_network();
    app.graph = g;
    let geometry = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    // Inside the Attribute VOP: only VOP categories, every entry named after its operation.
    app.current_network = Some(net);
    let inside = app.templates_for_view();
    let categories: Vec<&str> = inside.iter().map(|(c, _)| *c).collect();
    assert_eq!(categories, vop::CATEGORIES.to_vec());
    let names: Vec<String> = inside.iter().flat_map(|(_, kinds)| kinds.iter().map(NodeKind::menu_name)).collect();
    assert_eq!(names.len(), vop::OPS.len());
    let unique: HashSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "no two menu entries look the same");
    assert!(names.contains(&"Get Attribute".to_owned()) && names.contains(&"Noise".to_owned()));
    assert!(inside.iter().flat_map(|(_, k)| k).all(|k| matches!(k, NodeKind::Vop(_))));
    // In an ordinary geometry network: no VOPs.
    app.current_network = Some(geometry);
    assert!(app.templates_for_view().iter().flat_map(|(_, k)| k).all(|k| !matches!(k, NodeKind::Vop(_))));
}

// Every operation, wired into an Export with its inputs at their defaults, compiles and runs.
#[test]
fn every_vop_operation_runs() {
    let grid = node_grid(2.0, 2.0, 3, 3);
    for op in 0..vop::OPS.len() {
        if matches!(op, vop::EXPORT) {
            continue;
        }
        let mut nodes = HashMap::new();
        let mut node = vop::VopNode::new(op);
        if op == vop::BIND || op == vop::GET_ATTRIB || op == vop::PC_AVERAGE {
            node.name = "P".into();
        }
        nodes.insert(1, node);
        nodes.insert(2, vop::VopNode { name: "result".into(), ty: 0, ..vop::VopNode::new(vop::EXPORT) });
        let code = vop::compile(&nodes, |id, slot| (id == 2 && slot == 0).then_some(1)).unwrap();
        let _ = vex::run_wrangle(grid.clone(), &[], &code, 0.0, 1.0 / 24.0, 2, None, None);
        let report = vex::last_report(&code, 2).unwrap();
        assert_eq!(report.error, None, "{}:\n{code}", vop::OPS[op].label);
    }
}

#[test]
fn attribute_vop_errors_and_time() {
    let (mut g, _, net) = lift_network();
    // Binding Time makes it animate.
    let t = g.add(vop(vop::BIND, |v| {
        v.name = "Time".into();
        v.ty = 0;
    }), egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&t).unwrap().parent = Some(net);
    let export = g.children(net).into_iter().find(|c| matches!(&g.nodes[c].kind, NodeKind::Vop(v) if v.op == vop::EXPORT)).unwrap();
    let mul = g.children(net).into_iter().find(|c| matches!(&g.nodes[c].kind, NodeKind::Vop(v) if v.def().label == "Multiply")).unwrap();
    g.connect(t, mul, 1);
    assert!(g.time_dependent(net));
    let moved = g.cook(net, 3.0, &mut HashMap::new());
    assert!((moved.vertices[0].y - 3.0).abs() < 1e-5);
    // No export: the node reports why.
    g.nodes.remove(&export);
    assert!(g.cook(net, 0.0, &mut HashMap::new()).vertices.is_empty());
    assert!(node_errors(|e| e.get(&net).cloned()).is_some_and(|e| e.contains("no Export")));
}
