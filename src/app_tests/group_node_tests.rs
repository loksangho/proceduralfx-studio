use crate::*;

fn group_create(name: &str, domain: groups::GroupDomain, expression: &str) -> NodeKind {
    NodeKind::GroupCreate(groups::GroupCreateSettings {
        name: name.into(),
        domain,
        method: 3,
        bbox_min: [0.0; 3],
        bbox_max: [0.0; 3],
        percent: 0.0,
        seed: 0,
        direction: [0.0, 1.0, 0.0],
        spread: 0.0,
        expression: expression.into(),
        combine: 0,
        invert: false,
    })
}

// Grid -> Group Create (points with x > 0 as "right") -> `node`, with `pattern` as its group.
fn chain(node: NodeKind, pattern: &str) -> (Graph, NodeId, NodeId) {
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 4 }, egui::pos2(0.0, 0.0));
    let make = g.add(group_create("right", groups::GroupDomain::Points, "@P.x>0.01"), egui::pos2(0.0, 0.0));
    let target = g.add(node, egui::pos2(0.0, 0.0));
    g.connect(grid, make, 0);
    g.connect(make, target, 0);
    g.nodes.get_mut(&target).unwrap().group = groups::NodeGroup { pattern: pattern.into(), kind: 0 };
    (g, make, target)
}

#[test]
fn deformers_and_wrangles_only_touch_their_group() {
    let (g, make, transform) = chain(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [1.0; 3] }, "right");
    let before = g.cook(make, 0.0, &mut HashMap::new());
    let after = g.cook(transform, 0.0, &mut HashMap::new());
    for (a, b) in before.vertices.iter().zip(&after.vertices) {
        assert_eq!(b.y, if a.x > 0.01 { 1.0 } else { 0.0 }, "{a:?}");
    }
    // Wrangle: runs on the group's points only.
    let (g, make, wrangle) = chain(NodeKind::Wrangle { code: "f@hit = 1; @P.y = 2;".into(), domain: 2 }, "^right");
    let before = g.cook(make, 0.0, &mut HashMap::new());
    let after = g.cook(wrangle, 0.0, &mut HashMap::new());
    for (i, (a, b)) in before.vertices.iter().zip(&after.vertices).enumerate() {
        let inside = a.x <= 0.01;
        assert_eq!(b.y, if inside { 2.0 } else { 0.0 });
        assert_eq!(after.point_attributes["hit"][i], if inside { 1.0 } else { 0.0 });
    }
    // VEX group functions.
    let (g, _, tag) = chain(NodeKind::Wrangle { code: "i@r = inpointgroup(0, \"right\", @ptnum); setpointgroup(0, \"tagged\", @ptnum, @P.z > 0);".into(), domain: 2 }, "");
    let out = g.cook(tag, 0.0, &mut HashMap::new());
    for (i, p) in out.vertices.iter().enumerate() {
        assert_eq!(out.point_attributes["r"][i], (p.x > 0.01) as u8 as f32);
        assert_eq!(out.point_attributes["group:tagged"][i], (p.z > 0.0) as u8 as f32);
    }
    // A bad group is an error on the node.
    let (g, _, bad) = chain(NodeKind::Transform { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] }, "nosuchgroup");
    assert!(g.cook(bad, 0.0, &mut HashMap::new()).vertices.is_empty());
    assert!(node_errors(|e| e.get(&bad).cloned()).is_some_and(|e| e.contains("nosuchgroup")));
}

#[test]
fn blast_scatter_and_vellum_use_groups() {
    // Blast a primitive group.
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 4 }, egui::pos2(0.0, 0.0));
    let make = g.add(group_create("top", groups::GroupDomain::Prims, "@P.z>0.1"), egui::pos2(0.0, 0.0));
    let blast = g.add(NodeKind::Blast { component: 0, indices: Vec::new(), invert: false }, egui::pos2(0.0, 0.0));
    g.connect(grid, make, 0);
    g.connect(make, blast, 0);
    g.nodes.get_mut(&blast).unwrap().group = groups::NodeGroup { pattern: "top".into(), kind: 0 };
    let full = g.cook(make, 0.0, &mut HashMap::new());
    let blasted = g.cook(blast, 0.0, &mut HashMap::new());
    // The 3×3 grid's quads: only the top row's centres have z > 0.
    assert_eq!((poly::count(&full), poly::count(&blasted)), (9, 6));
    assert_eq!(blasted.indices.len(), full.indices.len() - 6);
    // Scatter only on that group.
    let scatter = g.add(NodeKind::Scatter(sops::ScatterSettings { count: 200, seed: 3, relax_iterations: 0, volume: false }), egui::pos2(0.0, 0.0));
    g.connect(make, scatter, 0);
    g.nodes.get_mut(&scatter).unwrap().group = groups::NodeGroup { pattern: "top".into(), kind: 0 };
    let points = g.cook(scatter, 0.0, &mut HashMap::new());
    assert_eq!(points.vertices.len(), 200);
    // Every point lies within the group's primitives (lowest z among their corners).
    let top = &full.point_attributes["prim:group:top"];
    let lowest = full.indices.iter().zip(top).filter(|(_, t)| **t > 0.5).flat_map(|(tri, _)| tri.map(|i| full.vertices[i].z)).fold(f32::INFINITY, f32::min);
    assert!(lowest > -0.5);
    assert!(points.vertices.iter().all(|p| p.z >= lowest - 1e-5));
    // Vellum pins its group: the right half stays, the rest falls.
    let (mut g, _, cloth) = chain(NodeKind::VellumSolver(vellum::VellumSettings { ground: false, ..vellum::VellumSettings::defaults(0) }), "right");
    g.nodes.get_mut(&cloth).unwrap().group.kind = 1;
    sims::clear_all();
    let out = g.cook(cloth, 1.0, &mut HashMap::new());
    let rest = g.cook(cloth, 0.0, &mut HashMap::new());
    for (a, b) in rest.vertices.iter().zip(&out.vertices) {
        if a.x > 0.01 {
            assert!(a.sub(*b).length() < 1e-5, "pinned point moved");
        }
    }
    assert!(out.vertices.iter().any(|p| p.y < -0.2), "the free half falls");
}

#[test]
fn picking_a_group_in_the_viewport() {
    let mut app = ProceduralFxStudioApp::default();
    let (g, make, transform) = chain(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [1.0; 3] }, "right");
    app.graph = g;
    app.selected = Some(transform);
    app.start_group_pick(transform, PickTarget::NodeGroup);
    let input = app.graph.cook(make, 0.0, &mut HashMap::new());
    // The viewport shows the node's input, with its current group selected.
    assert_eq!(app.current_mesh.vertices, input.vertices);
    let right: Vec<usize> = (0..input.vertices.len()).filter(|&i| input.vertices[i].x > 0.01).collect();
    assert_eq!(app.selected_vertices, right);
    assert!(app.selection_mode == SelectionMode::Vertex);
    app.rebuild_graph();
    assert_eq!(app.current_mesh.vertices, input.vertices, "the display is paused while picking");
    // Pick points 0-3 and accept.
    app.selected_vertices = vec![3, 1, 0, 2];
    app.finish_group_pick(true);
    assert!(app.group_pick.is_none());
    let group = &app.graph.nodes[&transform].group;
    assert_eq!(group.pattern, "0-3");
    assert_eq!(group.kind, 1);
    let out = app.graph.cook(transform, 0.0, &mut HashMap::new());
    for (i, p) in out.vertices.iter().enumerate() {
        assert_eq!(p.y, if i <= 3 { 1.0 } else { 0.0 });
    }
    // Primitives, then cancel: the group is left as it was.
    app.start_group_pick(transform, PickTarget::NodeGroup);
    app.set_group_pick_domain(groups::GroupDomain::Prims);
    assert!(app.selection_mode == SelectionMode::Triangle);
    app.selected_triangles = vec![0, 1];
    app.finish_group_pick(false);
    assert_eq!(app.graph.nodes[&transform].group.pattern, "0-3");
    // Accepting primitives writes a primitive group.
    app.start_group_pick(transform, PickTarget::NodeGroup);
    app.set_group_pick_domain(groups::GroupDomain::Prims);
    app.selected_triangles = vec![4, 5];
    app.finish_group_pick(true);
    // Triangles 4 and 5 are the grid's third quad.
    assert_eq!(app.graph.nodes[&transform].group, groups::NodeGroup { pattern: "2".into(), kind: 2 });
}

#[test]
fn duplicating_several_nodes_keeps_their_wiring() {
    let mut g = Graph::default();
    g.next_id += 9500;
    let source = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    let a = g.add(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 80.0));
    let b = g.add(NodeKind::Noise { freq: 1.0, amp: 0.2 }, egui::pos2(0.0, 160.0));
    let begin = g.add(NodeKind::ForEachBegin, egui::pos2(0.0, 240.0));
    let end = g.add(NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(begin), ..flow::ForEachSettings::new(0) }), egui::pos2(0.0, 320.0));
    g.connect(source, a, 0);
    g.connect(a, b, 0);
    g.connect(b, begin, 0);
    g.connect(begin, end, 0);
    let before = g.nodes.len();
    let map = g.duplicate_many(&[a, b, begin, end]);
    assert_eq!(g.nodes.len(), before + 4);
    let fed = |g: &Graph, to: NodeId| g.links.iter().filter(|l| l.to == to).map(|l| l.from).collect::<Vec<_>>();
    // The outside input still feeds the copy; inside wires join the copies.
    assert_eq!(fed(&g, map[&a]), vec![source]);
    assert_eq!(fed(&g, map[&b]), vec![map[&a]]);
    assert_eq!(fed(&g, map[&begin]), vec![map[&b]]);
    // The copied loop's End points at the copied Begin.
    assert!(matches!(&g.nodes[&map[&end]].kind, NodeKind::ForEachEnd(f) if f.begin == Some(map[&begin])));
    // The originals are untouched.
    assert_eq!(fed(&g, b), vec![a]);
    assert_eq!(g.nodes[&map[&b]].pos, egui::pos2(30.0, 220.0));
}

#[test]
fn picking_group_create_and_material_groups() {
    let mut app = ProceduralFxStudioApp::default();
    let bbox = groups::GroupCreateSettings {
        name: "picked".into(),
        domain: groups::GroupDomain::Points,
        method: 0,
        bbox_min: [0.01, -1.0, -1.0],
        bbox_max: [9.0, 1.0, 1.0],
        percent: 50.0,
        seed: 1,
        direction: [0.0, 1.0, 0.0],
        spread: 30.0,
        expression: String::new(),
        combine: 0,
        invert: true,
    };
    let (g, make, create) = chain(NodeKind::GroupCreate(bbox), "");
    app.graph = g;
    app.selected = Some(create);
    let input = app.graph.cook(make, 0.0, &mut HashMap::new());
    // It starts from the bounding box's points (before Invert).
    app.start_group_pick(create, PickTarget::GroupCreate);
    let right: Vec<usize> = (0..input.vertices.len()).filter(|&i| input.vertices[i].x > 0.01).collect();
    assert_eq!(app.selected_vertices, right);
    // Picking primitives turns it into an expression group of primitives.
    app.set_group_pick_domain(groups::GroupDomain::Prims);
    app.selected_triangles = vec![4, 5];
    app.finish_group_pick(true);
    let NodeKind::GroupCreate(set) = &app.graph.nodes[&create].kind else { panic!() };
    assert_eq!((set.method, set.domain, set.expression.as_str(), set.invert), (3, groups::GroupDomain::Prims, "2", true));
    assert!(app.graph.nodes[&create].group.is_empty(), "the node's own Group field is untouched");

    // A Material slot: primitives only.
    let (g, _, assign) = chain(NodeKind::MaterialAssign { slots: vec![(String::new(), None), ("0".into(), None)] }, "");
    app.graph = g;
    app.selected = Some(assign);
    app.start_group_pick(assign, PickTarget::MaterialSlot(1));
    assert_eq!(app.selected_triangles, vec![0, 1], "slot 2's group (polygon 0) is selected");
    app.set_group_pick_domain(groups::GroupDomain::Points);
    assert!(app.selection_mode == SelectionMode::Triangle);
    app.selected_triangles = vec![2, 3, 4, 5];
    app.finish_group_pick(true);
    let NodeKind::MaterialAssign { slots } = &app.graph.nodes[&assign].kind else { panic!() };
    assert_eq!((slots[0].0.as_str(), slots[1].0.as_str()), ("", "1-2"));
}

#[test]
fn groups_are_saved_with_the_scene() {
    let (g, make, target) = chain(NodeKind::Noise { freq: 1.0, amp: 0.2 }, "right ^0-3");
    let mut g = g;
    g.nodes.get_mut(&target).unwrap().group.kind = 1;
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(loaded.nodes[&target].group, groups::NodeGroup { pattern: "right ^0-3".into(), kind: 1 });
    assert_eq!(format!("{:?}", loaded.nodes[&make].kind), format!("{:?}", g.nodes[&make].kind));
}

#[test]
fn the_timeline_is_saved_with_the_scene() {
    let dir = std::env::temp_dir().join(format!("pfx_timeline_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("range.pfs");
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n");
    (app.playback_start, app.playback_end, app.playback_fps) = (2.0, 2.958, 30.0);
    app.write_scene(path.clone());
    assert!(!app.has_unsaved_changes());
    // Changing only the range is a change to save.
    app.playback_end = 4.0;
    assert!(app.has_unsaved_changes());

    let mut other = ProceduralFxStudioApp::default();
    other.open_scene(path.clone());
    assert_eq!((other.playback_start, other.playback_end, other.playback_fps), (2.0, 2.958, 30.0));
    assert!(other.time >= 2.0 && other.time <= 2.958);
    assert!(!other.has_unsaved_changes(), "just opened: nothing to save");
    assert_eq!(other.graph.nodes.len(), 2, "the graph still loads around the timeline line");

    // A scene saved before the timeline was saved keeps whatever range is set.
    std::fs::write(&path, "pfs 2\nnode 0 40 20 0 obj geometry -1\nnext 1\n").unwrap();
    other.open_scene(path.clone());
    assert_eq!((other.playback_start, other.playback_end), (2.0, 2.958));
    assert_eq!(parse_timeline("timeline 5 1 999\n"), Some((5.0, 5.0, 120.0)), "end before start, and too high a rate, are fixed");
    assert_eq!(parse_timeline("timeline nan 1 24\n"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

// A fingerprint of each starter effect's cooked geometry at a few frames, to check
// that a speed-up changes nothing. cargo test --release -- --ignored cooked_fingerprints --nocapture
#[test]
#[ignore]
fn cooked_fingerprints() {
    use std::hash::{Hash, Hasher};
    for preset in presets::PRESETS {
        let mut app = ProceduralFxStudioApp::default();
        app.load_state(preset.scene.to_owned());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for frame in [0, 6, 12, 23] {
            app.time = preset.range.0 + frame as f32 / sims::SIM_FPS;
            app.dirty = true;
            app.rebuild_graph();
            while app.simulation_rx.is_some() {
                std::thread::sleep(std::time::Duration::from_millis(5));
                app.rebuild_graph();
            }
            for v in &app.current_mesh.vertices {
                for c in [v.x, v.y, v.z] {
                    c.to_bits().hash(&mut hasher);
                }
            }
            let mut keys: Vec<_> = app.current_mesh.point_attributes.keys().collect();
            keys.sort();
            for key in keys {
                key.hash(&mut hasher);
                for value in &app.current_mesh.point_attributes[key] {
                    value.to_bits().hash(&mut hasher);
                }
            }
        }
        eprintln!("fingerprint {:<20} {:016x}", preset.name, hasher.finish());
    }
}

// Where cooking time goes in the starter effects, by node kind (own time, not inputs).
// cargo test --release -- --ignored where_cooking_time_goes --nocapture
#[test]
#[ignore]
fn where_cooking_time_goes() {
    let mut by_kind: HashMap<String, f64> = HashMap::new();
    for preset in presets::PRESETS {
        perf::clear();
        let mut app = ProceduralFxStudioApp::default();
        app.load_state(preset.scene.to_owned());
        let start = Instant::now();
        for frame in 0..24 {
            app.time = preset.range.0 + frame as f32 / sims::SIM_FPS;
            app.dirty = true;
            app.rebuild_graph();
            while app.simulation_rx.is_some() {
                std::thread::sleep(std::time::Duration::from_millis(5));
                app.rebuild_graph();
            }
        }
        eprintln!("{:<28} {:>8.0} ms", preset.name, start.elapsed().as_secs_f64() * 1000.0);
        for (id, stat) in perf::snapshot() {
            let name = app.graph.nodes.get(&id).map_or_else(|| "?".to_owned(), |n| n.kind.menu_name().to_owned());
            *by_kind.entry(name).or_default() += stat.total;
        }
    }
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (name, seconds) in kinds.iter().take(15) {
        eprintln!("  {name:<26} {:>8.0} ms", seconds * 1000.0);
    }
}

#[test]
fn scenes_from_a_newer_format_are_refused_not_half_opened() {
    assert_eq!(scene_format(&Graph::default().serialize()), Some(SCENE_FORMAT));
    assert_eq!(scene_format("mhip 1\n"), Some(1));
    assert_eq!(scene_format("node 0 0 0 0 obj sphere\n"), None);
    assert!(scene_problem(&Graph::default().serialize()).is_none());

    let dir = std::env::temp_dir().join(format!("pfx_format_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("future.pfs");
    let newer = format!("pfs {}\nnode 0 40 20 0 obj geometry -1\nnext 1\n", SCENE_FORMAT + 1);
    std::fs::write(&path, newer).unwrap();
    let mut app = ProceduralFxStudioApp::default();
    let before = app.graph.serialize();
    app.open_scene(path.clone());
    assert!(app.status.contains("newer version"), "{}", app.status);
    assert_eq!(app.graph.serialize(), before, "nothing was loaded");
    assert_ne!(app.scene_path.as_deref(), Some(path.as_path()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nodes_that_cant_be_read_are_reported() {
    // Every starter effect reads in full.
    for preset in presets::PRESETS {
        assert_eq!(unread_nodes(preset.scene, &Graph::deserialize(preset.scene)), 0, "{}", preset.name);
    }
    let dir = std::env::temp_dir().join(format!("pfx_unread_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unknown.pfs");
    std::fs::write(&path, "pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj hyperdrive 1 2 3\nparent 1 0\nnext 2\n").unwrap();
    let mut app = ProceduralFxStudioApp::default();
    app.open_scene(path);
    assert!(app.status.contains("1 node couldn't be read"), "{}", app.status);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovered_work_opens_as_unsaved_changes_to_its_scene() {
    let dir = std::env::temp_dir().join(format!("pfx_recover_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fire.pfs");
    let saved = "pfs 2\nnode 0 40 20 0 obj geometry -1\nnext 1\ntimeline 1 2 24\n";
    std::fs::write(&path, saved).unwrap();
    let autosaved = "pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nnext 2\ntimeline 1 3 24\n";

    let mut app = ProceduralFxStudioApp::default();
    app.recover_scene(autosaved.to_owned(), Some(path.clone()));
    assert_eq!(app.graph.nodes.len(), 2);
    assert_eq!(app.playback_end, 3.0);
    assert_eq!(app.scene_path.as_deref(), Some(path.as_path()));
    assert!(app.has_unsaved_changes());
    // Undo goes back to what was open before.
    app.do_undo();
    assert_eq!(app.graph.nodes.len(), Graph::deserialize(&ProceduralFxStudioApp::default().last_state).nodes.len());
    app.do_redo();
    app.save_scene();
    assert_eq!(Graph::deserialize(&std::fs::read_to_string(&path).unwrap()).nodes.len(), 2, "saving writes it back to its scene");

    // An untitled scene stays untitled; a newer format isn't recovered.
    let mut other = ProceduralFxStudioApp::default();
    other.recover_scene(autosaved.to_owned(), None);
    assert!(other.scene_path.is_none() && other.has_unsaved_changes());
    let mut refused = ProceduralFxStudioApp::default();
    refused.recover_scene(format!("pfs {}\n", SCENE_FORMAT + 1), None);
    assert!(refused.status.contains("newer version"));
    let _ = std::fs::remove_dir_all(&dir);
}

// Damaged scene files (cut short, lines lost or mangled) must load what they can,
// never crash the app.
#[test]
fn networks_inside_themselves_are_refused() {
    let g = Graph::deserialize("pfs 2\nnode 0 0 0 0 obj geometry -1\nnode 1 0 0 0 obj geometry -1\nnode 2 0 0 0 obj box 1 1\nparent 0 0\nparent 0 1\nparent 1 0\nparent 2 1\nnext 3\n");
    // `parent 0 0` is refused; `parent 0 1` is fine; then `parent 1 0` would put 1
    // inside what's inside it, so it's refused too.
    assert_eq!(g.nodes[&0].parent, Some(1));
    assert_eq!(g.nodes[&1].parent, None);
    assert_eq!(g.nodes[&2].parent, Some(1));
    // Saving and loading again terminates and agrees.
    let again = g.serialize();
    assert_eq!(Graph::deserialize(&again).serialize(), again);
}

#[test]
fn damaged_scene_files_never_crash_the_loader() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let junk = ["", "-1", "0", "nan", "inf", "-inf", "1e40", "18446744073709551616", "99999999", "@", "%%", "|||", "::", "é", "\u{0}"];
    let mut failures = Vec::new();
    for preset in presets::PRESETS {
        let lines: Vec<&str> = preset.scene.lines().collect();
        for round in 0..60 {
            let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
            match round % 4 {
                // Cut short, anywhere (on a character boundary).
                0 => {
                    let text = out.join("\n");
                    let mut at = (next() as usize) % text.len().max(1);
                    while !text.is_char_boundary(at) {
                        at -= 1;
                    }
                    out = vec![text[..at].to_owned()];
                }
                // Lines lost.
                1 => out.retain(|_| next() % 4 != 0),
                // Tokens replaced with junk.
                2 => {
                    for line in &mut out {
                        let mut tokens: Vec<String> = line.split(' ').map(str::to_owned).collect();
                        for token in &mut tokens {
                            if next() % 6 == 0 {
                                *token = junk[(next() as usize) % junk.len()].to_owned();
                            }
                        }
                        *line = tokens.join(" ");
                    }
                }
                // Tokens dropped from the ends of lines.
                _ => {
                    for line in &mut out {
                        let mut tokens: Vec<&str> = line.split(' ').collect();
                        let keep = tokens.len().saturating_sub((next() as usize) % 4);
                        tokens.truncate(keep.max(1));
                        *line = tokens.join(" ");
                    }
                }
            }
            let text = out.join("\n");
            let result = std::panic::catch_unwind(|| {
                let graph = Graph::deserialize(&text);
                // What loads saves again, and loads the same way.
                let again = graph.serialize();
                assert_eq!(Graph::deserialize(&again).serialize(), again);
            });
            if result.is_err() {
                failures.push(format!("{} round {round}", preset.name));
            }
        }
    }
    assert!(failures.is_empty(), "the loader panicked on: {failures:?}");
}
