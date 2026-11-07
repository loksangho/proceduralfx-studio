use crate::*;

fn drag(app: &mut ProceduralFxStudioApp, handle: GizmoHandle, axis_screen: egui::Vec2, moved: egui::Vec2) {
    let target = app.gizmo_target().expect("the selected node has a gizmo");
    let part = GizmoPart { handle, points: vec![egui::pos2(100.0, 100.0)], axis_screen, ring_sign: 1.0 };
    let frame = GizmoFrame {
        node: target.node,
        pivot: egui::pos2(100.0, 100.0),
        pixels_per_unit: 10.0,
        cam_right: Vec3::new(1.0, 0.0, 0.0),
        cam_up: Vec3::new(0.0, 1.0, 0.0),
        handle_px: 85.0,
        parts: vec![part.clone()],
    };
    let start = egui::pos2(100.0, 100.0);
    let d = GizmoDrag { part, frame, start_pointer: start, target };
    app.apply_gizmo_drag(&d, start + moved);
}

#[test]
fn domain_boxes_resize_from_their_faces() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    app.context = AppContext::Obj;
    let pyro = app.graph.add(NodeKind::Pyro(sims::PyroSettings::default()), egui::pos2(0.0, 0.0));
    app.selected = Some(pyro);
    let t = app.gizmo_target().unwrap();
    assert_eq!(t.style, GizmoStyle::Box);
    assert_eq!(t.value.s, [4.0, 5.0, 4.0]);
    // Drag the top face up 20 px at 10 px per unit: 2 units taller, centre up 1.
    drag(&mut app, GizmoHandle::Face(1, true), egui::vec2(0.0, -10.0), egui::vec2(0.0, -20.0));
    let NodeKind::Pyro(p) = &app.graph.nodes[&pyro].kind else { panic!() };
    assert!((p.size[1] - 7.0).abs() < 1e-4 && (p.center[1] - 3.0).abs() < 1e-4, "{:?} {:?}", p.size, p.center);
    // The bottom face: dragging it down grows the box downwards.
    drag(&mut app, GizmoHandle::Face(1, false), egui::vec2(0.0, 10.0), egui::vec2(0.0, 10.0));
    let NodeKind::Pyro(p) = &app.graph.nodes[&pyro].kind else { panic!() };
    assert!((p.size[1] - 8.0).abs() < 1e-4 && (p.center[1] - 2.5).abs() < 1e-4, "{:?} {:?}", p.size, p.center);

    // FLIP's automatic domain shows the emitter's bounds; dragging makes it explicit.
    let emitter = app.graph.add(NodeKind::Box { size: 2.0, div: 1 }, egui::pos2(0.0, 0.0));
    let flip = app.graph.add(
        NodeKind::Flip { particle_count: 100, grid_resolution: 16, flip_ratio: 0.9, viscosity: 0.0, gravity: -9.8, max_dt: 0.02, substeps: 1, seed: 1, domain: [0.0; 6] },
        egui::pos2(0.0, 0.0),
    );
    app.graph.connect(emitter, flip, 0);
    app.selected = Some(flip);
    let t = app.gizmo_target().unwrap();
    assert!((t.value.s[0] - 2.0).abs() < 1e-4);
    drag(&mut app, GizmoHandle::Face(0, true), egui::vec2(10.0, 0.0), egui::vec2(30.0, 0.0));
    let NodeKind::Flip { domain, .. } = app.graph.nodes[&flip].kind else { panic!() };
    assert!((domain[3] - 5.0).abs() < 1e-4 && (domain[0] - 1.5).abs() < 1e-4, "{domain:?}");
}

#[test]
fn transform_sops_get_a_gizmo_in_their_objects_space() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    app.context = AppContext::Obj;
    let object = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&object).unwrap().xform = ObjectXform { t: [5.0, 0.0, 0.0], r: [0.0, 90.0, 0.0], s: [2.0, 2.0, 2.0] };
    let xf = app.graph.add(NodeKind::Transform { t: [1.0, 0.0, 0.0], r: [0.0; 3], s: [1.0; 3], p: [0.0; 3] }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&xf).unwrap().parent = Some(object);
    app.selected = Some(xf);
    let t = app.gizmo_target().unwrap();
    assert_eq!(t.style, GizmoStyle::Xform);
    // The handle sits where the object puts the SOP's translate: (1,0,0) * 2, turned 90° about y, + (5,0,0).
    let pivot = t.point(Vec3::from_arr(t.value.t));
    let expected = Vec3::new(1.0, 0.0, 0.0).scale(2.0).rotate_deg([0.0, 90.0, 0.0]).add(Vec3::new(5.0, 0.0, 0.0));
    assert!(pivot.sub(expected).length() < 1e-4, "{pivot:?}");
    // local_dir undoes dir.
    let d = Vec3::new(0.3, -0.7, 1.1);
    assert!(t.local_dir(t.dir(d)).sub(d).length() < 1e-4);
    // A screen-plane drag of 20 px right (2 world units along x) moves the SOP by the local equivalent.
    app.gizmo_mode = GizmoMode::Translate;
    drag(&mut app, GizmoHandle::Center, egui::Vec2::ZERO, egui::vec2(20.0, 0.0));
    let NodeKind::Transform { t: moved, .. } = app.graph.nodes[&xf].kind else { panic!() };
    let world = t.point(Vec3::from_arr(moved)).sub(pivot);
    assert!(world.sub(Vec3::new(2.0, 0.0, 0.0)).length() < 1e-3, "moved {world:?} in world");
    // Nodes without a gizmo, and other contexts, have none.
    app.selected = Some(app.graph.add(NodeKind::Merge, egui::pos2(0.0, 0.0)));
    assert!(app.gizmo_target().is_none());
}
