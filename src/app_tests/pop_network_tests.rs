use crate::*;

// A Geometry object holding a new POP Network, made from the menus as a user would;
// returns the app (inside the object) and the network.
fn new_network() -> (ProceduralFxStudioApp, NodeId) {
    sims::clear_all();
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    app.context = AppContext::Obj;
    app.apply(Action::Add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0)));
    let geo = app.selected.unwrap();
    app.apply(Action::Enter(geo));
    // Particles is hidden in favour of the POP Network; POPs only appear inside one.
    let menu: Vec<NodeKind> = app.templates_for_view().into_iter().flat_map(|(_, k)| k).collect();
    assert!(menu.iter().any(|k| matches!(k, NodeKind::PopNet { .. })));
    assert!(!menu.iter().any(|k| matches!(k, NodeKind::Particles { .. } | NodeKind::Pop(_))));
    app.apply(Action::Add(NodeKind::PopNet { substeps: 1 }, egui::pos2(0.0, 0.0)));
    let net = app.selected.unwrap();
    (app, net)
}

fn pop_child(app: &mut ProceduralFxStudioApp, net: NodeId, op: usize) -> NodeId {
    let id = app.graph.add(NodeKind::Pop(pop::PopNode::new(op)), egui::pos2(0.0, 0.0));
    let n = app.graph.nodes.get_mut(&id).unwrap();
    n.context = AppContext::Obj;
    n.parent = Some(net);
    id
}

fn child(app: &ProceduralFxStudioApp, net: NodeId, op: usize) -> NodeId {
    app.graph.children(net).into_iter().find(|c| matches!(&app.graph.nodes[c].kind, NodeKind::Pop(p) if p.op == op)).unwrap()
}

fn set(app: &mut ProceduralFxStudioApp, id: NodeId, key: &str, values: &[f32]) {
    let Some(NodeKind::Pop(p)) = app.graph.nodes.get_mut(&id).map(|n| &mut n.kind) else { panic!() };
    for (param, slot) in p.param_mut() {
        if param.key == key {
            slot.copy_from_slice(values);
        }
    }
}

fn cook(app: &ProceduralFxStudioApp, id: NodeId, time: f32) -> Mesh {
    app.graph.cook(id, time, &mut HashMap::new())
}

#[test]
fn a_new_pop_network_births_and_falls() {
    let (mut app, net) = new_network();
    // Source, object, solver and gravity, with gravity displayed.
    assert_eq!(app.graph.children(net).len(), 4);
    let gravity = child(&app, net, pop::GRAVITY);
    assert_eq!(app.graph.display_of(net), Some(gravity));
    app.apply(Action::Enter(net));
    let menu: Vec<NodeKind> = app.templates_for_view().into_iter().flat_map(|(_, k)| k).collect();
    assert_eq!(menu.iter().filter(|k| matches!(k, NodeKind::Pop(_))).count(), pop::OPS.len());
    assert!(menu.iter().all(|k| matches!(k, NodeKind::Pop(_) | NodeKind::Merge)));

    // 200 a second from the origin, living 1.5-2.5 s: by 1 s (frame 25) 25 steps have run,
    // the first on frame 1 as in Houdini, so 25/24 of 200, all alive.
    let out = cook(&app, net, 1.0);
    assert_eq!(out.vertices.len(), 208);
    // Frame 1 already has the first step's births.
    assert_eq!(cook(&app, net, 0.0).vertices.len(), 8);
    for name in ["v", "age", "life", "id"] {
        assert!(out.point_attributes.contains_key(name), "{name}");
    }
    assert!(!out.point_attributes.contains_key("detail:pop_next_id"));
    // Launched up at about 1, pulled down by gravity: the oldest are falling now.
    let oldest = (0..208).max_by(|&a, &b| out.point_attributes["age"][a].total_cmp(&out.point_attributes["age"][b])).unwrap();
    assert!(out.point_attributes["v"][oldest * 3 + 1] < -5.0);
    // Viewing a POP inside shows the network's particles.
    assert_eq!(cook(&app, gravity, 1.0).vertices.len(), 208);
    // Scrubbing back gives the same frame.
    let again = cook(&app, net, 0.5);
    let _later = cook(&app, net, 1.5);
    assert_eq!(cook(&app, net, 0.5).vertices, again.vertices);
    // By 3 s the first ones are past their life.
    assert!(cook(&app, net, 3.0).point_attributes["age"].iter().all(|a| *a <= 2.5 + 1e-3));
}

#[test]
fn forces_vex_groups_and_kills_in_the_network() {
    let (mut app, net) = new_network();
    let (solver, gravity) = (child(&app, net, pop::SOLVER), child(&app, net, pop::GRAVITY));
    // A source from a grid on input 0, a wind and a wrangle merged into the pre-solve.
    let grid = app.graph.add(NodeKind::Grid { size: 2.0, res: 4 }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&grid).unwrap().parent = app.graph.nodes[&net].parent;
    app.graph.connect(grid, net, 0);
    let source = child(&app, net, pop::SOURCE);
    set(&mut app, source, "velocity", &[0.0, 0.0, 0.0]);
    set(&mut app, source, "variance", &[0.0, 0.0, 0.0]);
    let wind = pop_child(&mut app, net, pop::WIND);
    set(&mut app, wind, "wind", &[3.0, 0.0, 0.0]);
    set(&mut app, wind, "air_resistance", &[50.0]);
    let wrangle = pop_child(&mut app, net, pop::WRANGLE);
    if let Some(NodeKind::Pop(p)) = app.graph.nodes.get_mut(&wrangle).map(|n| &mut n.kind) {
        p.texts[0] = "@Cd = set(1, 0, 0);".into();
    }
    let merge = app.graph.add(NodeKind::Merge, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&merge).unwrap().parent = Some(net);
    app.graph.links.retain(|l| !(l.to == solver && l.from == source));
    app.graph.connect(source, merge, 0);
    app.graph.connect(wind, merge, 1);
    app.graph.connect(wrangle, merge, 2);
    app.graph.connect(merge, solver, 1);
    // After gravity: a group of the left half, and a kill of everything below y = -0.5.
    let group = pop_child(&mut app, net, pop::GROUP);
    set(&mut app, group, "rule", &[3.0]);
    set(&mut app, group, "births_only", &[0.0]);
    if let Some(NodeKind::Pop(p)) = app.graph.nodes.get_mut(&group).map(|n| &mut n.kind) {
        p.texts = vec!["left".into(), "@P.x < 0".into()];
    }
    let kill = pop_child(&mut app, net, pop::KILL);
    if let Some(NodeKind::Pop(p)) = app.graph.nodes.get_mut(&kill).map(|n| &mut n.kind) {
        p.texts[0] = "@P.y < -0.5".into();
    }
    app.graph.connect(gravity, group, 0);
    app.graph.connect(group, kill, 0);
    app.graph.set_display(kill);

    let out = cook(&app, net, 0.5);
    let n = out.vertices.len();
    assert!(n > 50, "{n} particles");
    // Born on the grid (y = 0, |x| <= 1), blown along +x toward 3, red, none below -0.5.
    assert!(out.vertices.iter().all(|p| p.y >= -0.5 && p.z.abs() <= 1.0 + 1e-4));
    assert!(out.point_attributes["v"].chunks(3).all(|v| v[0] > 2.0), "the wind sets the pace");
    assert!(out.point_attributes["Cd"].chunks(3).all(|c| c == [1.0, 0.0, 0.0]));
    let left = &out.point_attributes["group:left"];
    assert!((0..n).all(|i| (left[i] > 0.5) == (out.vertices[i].x < 0.0)));
    // Gravity after the solver acts on the next step: they fall, as fast as the wind's
    // air resistance lets them (about -9.8 / 50).
    // Those a step old or more; the newest haven't had a step of gravity yet.
    let age = &out.point_attributes["age"];
    let vy: Vec<f32> = (0..n).filter(|&i| age[i] > 0.1).map(|i| out.point_attributes["v"][i * 3 + 1]).collect();
    assert!(!vy.is_empty() && vy.iter().all(|v| (v + 9.8 / 50.0).abs() < 0.02), "{vy:?}");
}

#[test]
fn static_objects_and_ground_planes_catch_particles() {
    let (mut app, net) = new_network();
    let (object, solver) = (child(&app, net, pop::OBJECT), child(&app, net, pop::SOLVER));
    let outside = app.graph.nodes[&net].parent;
    // Particles from a small grid at y = 2 fall onto a box (top at y = 1) on input 1.
    let grid = app.graph.add(NodeKind::Grid { size: 0.5, res: 2 }, egui::pos2(0.0, 0.0));
    let lift = app.graph.add(NodeKind::Wrangle { code: "@P.y += 2;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    let block = app.graph.add(NodeKind::Box { size: 2.0, div: 1 }, egui::pos2(0.0, 0.0));
    for id in [grid, lift, block] {
        app.graph.nodes.get_mut(&id).unwrap().parent = outside;
    }
    app.graph.connect(grid, lift, 0);
    app.graph.connect(lift, net, 0);
    app.graph.connect(block, net, 1);
    let source = child(&app, net, pop::SOURCE);
    set(&mut app, source, "velocity", &[0.0, 0.0, 0.0]);
    set(&mut app, source, "variance", &[0.0, 0.0, 0.0]);
    set(&mut app, source, "life", &[10.0]);
    // Merged with the POP Object into the solver, as in Houdini.
    let still = pop_child(&mut app, net, pop::STATIC_OBJECT);
    let merge = app.graph.add(NodeKind::Merge, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&merge).unwrap().parent = Some(net);
    app.graph.links.retain(|l| !(l.to == solver && l.slot == 0));
    app.graph.connect(object, merge, 0);
    app.graph.connect(still, merge, 1);
    app.graph.connect(merge, solver, 0);
    let out = cook(&app, net, 2.0);
    assert!(out.vertices.len() > 100);
    assert!(out.vertices.iter().all(|p| p.y > 0.95 && p.y < 2.0 + 1e-4), "they rest on the box");
    assert!(out.point_attributes["hit"].iter().any(|h| *h > 0.5));

    // Without it they fall through, until a Ground Plane at y = -1 catches them.
    app.graph.nodes.get_mut(&still).unwrap().bypass = true;
    let ground = pop_child(&mut app, net, pop::GROUND_PLANE);
    set(&mut app, ground, "height", &[-1.0]);
    app.graph.connect(ground, merge, 2);
    let out = cook(&app, net, 2.0);
    assert!(out.vertices.iter().any(|p| p.y < 0.0));
    assert!(out.vertices.iter().all(|p| p.y > -1.0));
}

#[test]
fn viewing_inside_a_network_plays_its_simulation() {
    let (mut app, net) = new_network();
    let (source, solver, gravity) = (child(&app, net, pop::SOURCE), child(&app, net, pop::SOLVER), child(&app, net, pop::GRAVITY));
    // Inside, the viewport shows the network's display node: it must recook as time moves.
    for id in app.graph.children(net) {
        assert!(app.graph.time_dependent(id), "{}", app.graph.nodes[&id].kind.menu_name());
    }
    // The source works before or after the solver, and moved back.
    for slot in [2, 1] {
        app.graph.links.retain(|l| !(l.to == solver && l.from == source));
        app.graph.connect(source, solver, slot);
        let (early, late) = (cook(&app, gravity, 0.5), cook(&app, gravity, 1.0));
        let lowest = |m: &Mesh| m.vertices.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        assert!(late.vertices.len() > early.vertices.len() && lowest(&late) < lowest(&early) - 1.0, "slot {slot}");
    }
}

#[test]
fn pop_vops_and_wrangles_read_the_networks_inputs() {
    let (mut app, net) = new_network();
    let gravity = child(&app, net, pop::GRAVITY);
    // Input 0 of the POP Network: a red grid. Particles are born on it.
    let grid = app.graph.add(NodeKind::Grid { size: 1.0, res: 20 }, egui::pos2(0.0, 0.0));
    let red = app.graph.add(NodeKind::Wrangle { code: "@Cd = set(1, 0, 0);".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    for id in [grid, red] {
        app.graph.nodes.get_mut(&id).unwrap().parent = app.graph.nodes[&net].parent;
    }
    app.graph.connect(grid, red, 0);
    app.graph.connect(red, net, 0);
    // A POP VOP whose Bind reads Cd from VOP input 1 (the network's input 0), exported as tint.
    app.apply(Action::Enter(net));
    app.apply(Action::Add(NodeKind::Pop(pop::PopNode::new(pop::VOP)), egui::pos2(0.0, 0.0)));
    let vop_net = app.selected.unwrap();
    for child in app.graph.children(vop_net) {
        if let Some(NodeKind::Vop(v)) = app.graph.nodes.get_mut(&child).map(|n| &mut n.kind) {
            if v.op == vop::BIND {
                v.input = 1;
            }
            v.name = if v.op == vop::BIND { "Cd".into() } else { "tint".into() };
        }
    }
    app.graph.connect(gravity, vop_net, 0);
    app.graph.set_display(vop_net);
    let out = cook(&app, net, 0.25);
    assert!(!out.vertices.is_empty());
    assert!(out.point_attributes["tint"].chunks(3).all(|c| c == [1.0, 0.0, 0.0]), "read from the network's input");
    // The same from a POP Wrangle.
    let wrangle = pop_child(&mut app, net, pop::WRANGLE);
    if let Some(NodeKind::Pop(p)) = app.graph.nodes.get_mut(&wrangle).map(|n| &mut n.kind) {
        p.texts[0] = "v@tint2 = point(1, \"Cd\", 0); f@count = npoints(1);".into();
    }
    app.graph.connect(vop_net, wrangle, 0);
    app.graph.set_display(wrangle);
    let out = cook(&app, net, 0.25);
    assert!(out.point_attributes["tint2"].chunks(3).all(|c| c == [1.0, 0.0, 0.0]));
    assert!(out.point_attributes["count"].iter().all(|n| *n == 400.0));
}

#[test]
fn every_pop_panel_draws() {
    let (mut app, net) = new_network();
    let ids: Vec<NodeId> = std::iter::once(net).chain((0..pop::OPS.len()).map(|op| pop_child(&mut app, net, op))).collect();
    let ctx = egui::Context::default();
    for id in ids {
        app.selected = Some(id);
        let before = app.graph.nodes[&id].kind.clone();
        let mut changed = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| changed = app.params_ui(ui));
        });
        assert!(!changed, "drawing {} changed it", app.graph.nodes[&id].kind.menu_name());
        assert_eq!(format!("{:?}", app.graph.nodes[&id].kind), format!("{before:?}"));
    }
}

#[test]
fn pop_vops_run_and_networks_survive_save_and_load() {
    let (mut app, net) = new_network();
    let (solver, gravity) = (child(&app, net, pop::SOLVER), child(&app, net, pop::GRAVITY));
    // A POP VOP made from the menu has Bind P -> Export P inside, and runs.
    app.apply(Action::Enter(net));
    app.apply(Action::Add(NodeKind::Pop(pop::PopNode::new(pop::VOP)), egui::pos2(0.0, 0.0)));
    let vop = app.selected.unwrap();
    assert!(app.graph.nodes[&vop].kind.is_container());
    assert_eq!(app.graph.children(vop).len(), 2);
    assert!(app.graph.vop_code(vop).is_ok());
    app.graph.connect(gravity, vop, 0);
    app.graph.set_display(vop);
    // Collisions after the source, in the post-solve input.
    let source = child(&app, net, pop::SOURCE);
    let collide = pop_child(&mut app, net, pop::COLLIDE);
    app.graph.connect(source, collide, 0);
    app.graph.connect(collide, solver, 2);
    let before = cook(&app, net, 2.0);
    assert!(!before.vertices.is_empty());
    // The ground at y = 0 holds them up.
    assert!(before.vertices.iter().all(|p| p.y >= 0.0));
    assert!(before.point_attributes["hit"].iter().any(|h| *h > 0.5));

    let loaded = Graph::deserialize(&app.graph.serialize());
    sims::clear_all();
    let after = loaded.cook(net, 2.0, &mut HashMap::new());
    assert_eq!(after.vertices, before.vertices);
    let Some(NodeKind::Pop(p)) = loaded.nodes.get(&collide).map(|n| &n.kind) else { panic!() };
    assert_eq!(p.op, pop::COLLIDE);
}

#[test]
fn a_stream_in_the_network_pushes_only_its_particles() {
    let (mut app, net) = new_network();
    let (solver, gravity) = (child(&app, net, pop::SOLVER), child(&app, net, pop::GRAVITY));
    let source = child(&app, net, pop::SOURCE);
    set(&mut app, source, "velocity", &[0.0, 0.0, 0.0]);
    set(&mut app, source, "variance", &[0.0, 0.0, 0.0]);
    set(&mut app, gravity, "force", &[0.0, 0.0, 0.0]);
    // Pre-solve: half the particles in a stream, pushed along +x; the stream ends at the merge,
    // so the drag merged after it slows everyone.
    let stream = pop_child(&mut app, net, pop::STREAM);
    let force = pop_child(&mut app, net, pop::FORCE);
    set(&mut app, force, "force", &[20.0, 0.0, 0.0]);
    let drag = pop_child(&mut app, net, pop::DRAG);
    set(&mut app, drag, "wind", &[0.0, 0.0, 3.0]);
    let merge = app.graph.add(NodeKind::Merge, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&merge).unwrap().parent = Some(net);
    app.graph.connect(stream, force, 0);
    app.graph.connect(force, merge, 0);
    app.graph.connect(drag, merge, 1);
    app.graph.connect(merge, solver, 1);

    let out = cook(&app, net, 0.5);
    let n = out.vertices.len();
    let member = &out.point_attributes[&pop::stream_group(&app.graph.nodes[&stream].title(stream))];
    let joined = member.iter().filter(|m| **m > 0.5).count();
    assert!(joined > n / 4 && joined < n * 3 / 4, "{joined} of {n} in the stream");
    let (v, age) = (&out.point_attributes["v"], &out.point_attributes["age"]);
    for i in (0..n).filter(|&i| age[i] > 0.1) {
        let pushed = v[i * 3] > 0.5;
        assert_eq!(pushed, member[i] > 0.5, "particle {i}: only the stream is pushed");
        assert!(v[i * 3 + 2] > 0.1, "particle {i}: the drag after the merge reaches everyone");
    }
}

#[test]
fn a_curve_force_carries_particles_along_a_curve_node() {
    let (mut app, net) = new_network();
    let (solver, gravity) = (child(&app, net, pop::SOLVER), child(&app, net, pop::GRAVITY));
    let source = child(&app, net, pop::SOURCE);
    set(&mut app, source, "velocity", &[0.0, 0.0, 0.0]);
    set(&mut app, source, "variance", &[0.0, 0.0, 0.0]);
    set(&mut app, gravity, "force", &[0.0, 0.0, 0.0]);
    // A curve rising from the origin, into the network's input 1.
    let mut settings = curve::CurveSettings::defaults();
    settings.points = vec![[0.0, 0.0, 0.0], [0.0, 2.0, 0.0], [2.0, 4.0, 0.0]];
    let curve = app.graph.add(NodeKind::Curve(settings), egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&curve).unwrap().parent = app.graph.nodes[&net].parent;
    app.graph.connect(curve, net, 1);
    let flow = pop_child(&mut app, net, pop::CURVE_FORCE);
    set(&mut app, flow, "mode", &[1.0]);
    set(&mut app, flow, "air_resistance", &[20.0]);
    app.graph.connect(flow, solver, 1);

    let out = cook(&app, net, 1.0);
    let (v, age) = (&out.point_attributes["v"], &out.point_attributes["age"]);
    let old: Vec<usize> = (0..out.vertices.len()).filter(|&i| age[i] > 0.5).collect();
    assert!(!old.is_empty());
    // Up the curve and staying near it.
    for &i in &old {
        let p = out.vertices[i];
        assert!(p.y > 0.5 && v[i * 3 + 1] > 0.5, "particle {i} at {p:?} rides up the curve");
        assert!(p.z.abs() < 0.05, "{p:?}");
    }
}

// As in Houdini: a POP Source's particles are a stream that the POPs below it act on; a
// source wired into two streams births once; a stream below a stream takes from it only.
#[test]
fn sources_are_streams_and_branches_share_one_set_of_particles() {
    let (mut app, net) = new_network();
    let (solver, gravity) = (child(&app, net, pop::SOLVER), child(&app, net, pop::GRAVITY));
    set(&mut app, gravity, "force", &[0.0, 0.0, 0.0]);
    let first = child(&app, net, pop::SOURCE);
    set(&mut app, first, "birth_rate", &[240.0]);
    // A second source, merged with the first's chain into the post-solve input.
    let second = pop_child(&mut app, net, pop::SOURCE);
    set(&mut app, second, "birth_rate", &[240.0]);
    // Below the first source: two streams (a half and the rest), one of them with a stream
    // below it, each colouring its particles.
    let colour = |app: &mut ProceduralFxStudioApp, c: [f32; 3]| {
        let id = pop_child(app, net, pop::COLOR);
        set(app, id, "color", &c);
        id
    };
    let half = pop_child(&mut app, net, pop::STREAM);
    set(&mut app, half, "ratio", &[0.5]);
    let all_of_it = pop_child(&mut app, net, pop::STREAM);
    set(&mut app, all_of_it, "rule", &[0.0]);
    let inner = pop_child(&mut app, net, pop::STREAM);
    set(&mut app, inner, "rule", &[0.0]);
    let (red, green, blue) = (colour(&mut app, [1.0, 0.0, 0.0]), colour(&mut app, [0.0, 1.0, 0.0]), colour(&mut app, [0.0, 0.0, 1.0]));
    let merge = app.graph.add(NodeKind::Merge, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&merge).unwrap().parent = Some(net);
    app.graph.links.retain(|l| !(l.to == solver && l.from == first));
    for (from, to, slot) in [(first, all_of_it, 0), (all_of_it, green, 0), (first, half, 0), (half, inner, 0), (inner, red, 0), (green, merge, 0), (red, merge, 1), (second, blue, 0), (blue, merge, 2), (merge, solver, 2)] {
        app.graph.connect(from, to, slot);
    }
    let out = cook(&app, net, 0.5);
    // Two sources' worth of births (240 a second each, 13 steps), not one per branch.
    assert_eq!(out.vertices.len(), 2 * 130);
    let group = |id: NodeId| out.point_attributes[&pop::stream_group(&app.graph.nodes[&id].title(id))].iter().map(|g| *g > 0.5).collect::<Vec<bool>>();
    let (from_first, from_second, in_half) = (group(first), group(second), group(half));
    assert_eq!(from_first.iter().filter(|g| **g).count(), 130, "each source's births are its stream");
    assert!(from_first.iter().zip(&from_second).all(|(a, b)| a != b));
    let cd = &out.point_attributes["Cd"];
    for i in 0..out.vertices.len() {
        let c = &cd[i * 3..i * 3 + 3];
        let expected = if from_second[i] { [0.0, 0.0, 1.0] } else if in_half[i] { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        assert_eq!(c, expected, "particle {i}: only its own streams colour it");
    }
    assert!(in_half.iter().zip(&group(inner)).all(|(h, i)| h == i), "the stream below takes all of the half, and only it");
    let halves = in_half.iter().filter(|g| **g).count();
    assert!(halves > 30 && halves < 100, "{halves}");
}
