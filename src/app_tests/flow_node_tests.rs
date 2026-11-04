use crate::*;

fn add_in(app: &mut ProceduralFxStudioApp, geo: NodeId, kind: NodeKind) -> NodeId {
    app.current_network = Some(geo);
    app.context = AppContext::Obj;
    app.apply(Action::Add(kind, egui::pos2(0.0, 0.0)));
    app.selected.unwrap()
}

#[test]
fn foreach_loops_run_their_body_per_piece() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    // Two boxes, merged: two connected pieces.
    let a = add_in(&mut app, geo, NodeKind::Box { size: 1.0, div: 1 });
    let b = add_in(&mut app, geo, NodeKind::Box { size: 1.0, div: 1 });
    let shift = add_in(&mut app, geo, NodeKind::Transform { t: [5.0, 0.0, 0.0], r: [0.0; 3], s: [1.0; 3] });
    app.graph.connect(b, shift, 0);
    let merge = add_in(&mut app, geo, NodeKind::Merge);
    app.graph.connect(a, merge, 0);
    app.graph.connect(shift, merge, 1);
    // TAB-add a For-Each Connected Piece below the merge: merge -> Begin -> End.
    app.current_network = Some(geo);
    app.apply(Action::AddBelow(NodeKind::ForEachEnd(flow::ForEachSettings::new(flow::CONNECTED)), merge));
    let end = app.selected.unwrap();
    let NodeKind::ForEachEnd(set) = app.graph.nodes[&end].kind.clone() else { panic!() };
    let begin = set.begin.unwrap();
    assert_eq!(app.graph.input_of(begin, 0), Some(merge));
    assert_eq!(app.graph.input_of(end, 0), Some(begin));
    // Body: lift each piece by its iteration number (detail iteration).
    let lift = app.graph.add(NodeKind::Wrangle { code: "@P.y += detail(0, \"iteration\") * 10;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&lift).unwrap().parent = Some(geo);
    app.graph.connect(begin, lift, 0);
    app.graph.connect(lift, end, 0);
    let out = app.graph.cook(end, 0.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&end).cloned()).is_none(), "{:?}", node_errors(|e| e.get(&end).cloned()));
    assert_eq!(out.vertices.len(), 16);
    let high = out.vertices.iter().filter(|v| v.y > 5.0).count();
    assert_eq!(high, 8, "the second piece is lifted by 10");
    // Single pass: only the second piece.
    if let NodeKind::ForEachEnd(s) = &mut app.graph.nodes.get_mut(&end).unwrap().kind {
        s.single_pass = 1;
    }
    let one = app.graph.cook(end, 0.0, &mut HashMap::new());
    assert_eq!(one.vertices.len(), 8);
    assert!(one.vertices.iter().all(|v| v.y > 5.0));
    // The same loop inside a Compile Block, multithreaded, gives the same result.
    if let NodeKind::ForEachEnd(s) = &mut app.graph.nodes.get_mut(&end).unwrap().kind {
        s.single_pass = -1;
    }
    let compile_end = add_in(&mut app, geo, NodeKind::CompileEnd { begin: None });
    let NodeKind::CompileEnd { begin: Some(compile_begin) } = app.graph.nodes[&compile_end].kind else { panic!() };
    app.graph.disconnect(begin, 0);
    app.graph.connect(merge, compile_begin, 0);
    app.graph.connect(compile_begin, begin, 0);
    app.graph.disconnect(compile_end, 0);
    app.graph.connect(end, compile_end, 0);
    assert!(app.graph.in_compile_block(begin));
    let compiled = app.graph.cook(compile_end, 0.0, &mut HashMap::new());
    let sorted = |m: &Mesh| { let mut v: Vec<(i32, i32, i32)> = m.vertices.iter().map(|p| ((p.x * 100.0) as i32, (p.y * 100.0) as i32, (p.z * 100.0) as i32)).collect(); v.sort(); v };
    assert_eq!(sorted(&compiled), sorted(&out));
    // Feedback: each pass adds 1.
    let fb = add_in(&mut app, geo, NodeKind::ForEachEnd(flow::ForEachSettings { iterations: 5, ..flow::ForEachSettings::new(flow::FEEDBACK) }));
    let NodeKind::ForEachEnd(fset) = app.graph.nodes[&fb].kind.clone() else { panic!() };
    let fbegin = fset.begin.unwrap();
    app.graph.connect(a, fbegin, 0);
    let step = app.graph.add(NodeKind::Wrangle { code: "@P.y += 1;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&step).unwrap().parent = Some(geo);
    app.graph.connect(fbegin, step, 0);
    app.graph.connect(step, fb, 0);
    let fed = app.graph.cook(fb, 0.0, &mut HashMap::new());
    let low = fed.vertices.iter().map(|v| v.y).fold(f32::MAX, f32::min);
    assert!((low - 4.5).abs() < 1e-4, "five passes of +1 from -0.5: {low}");
    // Saved and loaded with the pairing intact.
    let loaded = Graph::deserialize(&app.graph.serialize());
    assert!(matches!(&loaded.nodes[&end].kind, NodeKind::ForEachEnd(s) if s.begin == Some(begin)));
}

#[test]
fn object_merge_brings_other_objects_in() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let source = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&source).unwrap().xform.t = [10.0, 0.0, 0.0];
    let cube = add_in(&mut app, source, NodeKind::Box { size: 1.0, div: 1 });
    app.graph.set_display(cube);
    let target = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&target).unwrap().xform.t = [2.0, 0.0, 0.0];
    let merge = add_in(&mut app, target, NodeKind::ObjectMerge { sources: vec![Some(source)], into_this: true });
    let into = app.graph.cook(merge, 0.0, &mut HashMap::new());
    let cx = |m: &Mesh| m.vertices.iter().map(|v| v.x).sum::<f32>() / m.vertices.len() as f32;
    assert!((cx(&into) - 8.0).abs() < 1e-4, "at x=10 in the world, x=8 inside an object at x=2: {}", cx(&into));
    if let NodeKind::ObjectMerge { into_this, .. } = &mut app.graph.nodes.get_mut(&merge).unwrap().kind {
        *into_this = false;
    }
    assert!(cx(&app.graph.cook(merge, 0.0, &mut HashMap::new())).abs() < 1e-4);
    // Merging its own object is refused.
    let own = add_in(&mut app, target, NodeKind::ObjectMerge { sources: vec![Some(target)], into_this: true });
    app.graph.cook(own, 0.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&own).cloned()).is_some());
    let loaded = Graph::deserialize(&app.graph.serialize());
    assert!(matches!(&loaded.nodes[&merge].kind, NodeKind::ObjectMerge { sources, into_this: false } if sources == &vec![Some(source)]));

    // A node in its own network: in place, as built (the object's transform cancels out).
    let sphere = add_in(&mut app, target, NodeKind::Sphere { radius: 1.0, rows: 4, cols: 6 });
    let local = add_in(&mut app, target, NodeKind::ObjectMerge { sources: vec![Some(sphere)], into_this: true });
    let merged = app.graph.cook(local, 0.0, &mut HashMap::new());
    let original = app.graph.cook(sphere, 0.0, &mut HashMap::new());
    assert_eq!(merged.vertices.len(), original.vertices.len());
    assert!(merged.vertices.iter().zip(&original.vertices).all(|(a, b)| a.sub(*b).length() < 1e-5));
    // What's downstream of a merge can't feed it; nested nodes are named by their path.
    let after = add_in(&mut app, target, NodeKind::Transform { t: [1.0, 0.0, 0.0], r: [0.0; 3], s: [1.0; 3] });
    app.graph.connect(local, after, 0);
    let downstream = app.graph.downstream(local);
    assert!(downstream.contains(&after) && !downstream.contains(&sphere));
    assert_eq!(app.graph.path_in_object(sphere), app.graph.nodes[&sphere].title(sphere));
    app.graph.nodes.get_mut(&sphere).unwrap().parent = Some(after);
    assert_eq!(app.graph.path_in_object(sphere), format!("{} / {}", app.graph.nodes[&after].title(after), app.graph.nodes[&sphere].title(sphere)));
    assert_eq!(app.graph.enclosing_geometry(sphere), Some(target));
}
