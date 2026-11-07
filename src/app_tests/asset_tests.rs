use crate::*;

// Geometry ⊃ sphere → Subnet (Subnet Input → Transform → OUT), the Subnet displayed,
// with the Transform's translate x promoted as "Offset". Returns (graph, geometry, subnet, transform).
fn subnet_scene() -> (ProceduralFxStudioApp, NodeId, NodeId, NodeId) {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let sphere = app.graph.add(NodeKind::Sphere { radius: 0.5, rows: 8, cols: 12 }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&sphere).unwrap().parent = Some(geo);
    app.current_network = Some(geo);
    app.apply(Action::Add(NodeKind::Subnet { values: Vec::new(), spec: assets::Spec::default() }, egui::pos2(0.0, 100.0)));
    let subnet = app.selected.expect("added");
    assert_eq!(app.graph.nodes[&subnet].parent, Some(geo));
    app.graph.connect(sphere, subnet, 0);
    app.graph.displays.insert(geo, subnet);
    // Put a Transform between the Subnet's input and OUT.
    let out = app.graph.display_of(subnet).expect("a new Subnet shows OUT");
    let input = app.graph.input_of(out, 0).expect("OUT is fed by Input_1");
    let xform = app.graph.add(NodeKind::Transform { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3], p: [0.0; 3] }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&xform).unwrap().parent = Some(subnet);
    app.graph.connect(input, xform, 0);
    app.graph.connect(xform, out, 0);
    if let Some(NodeKind::Subnet { values, spec }) = app.graph.nodes.get_mut(&subnet).map(|n| &mut n.kind) {
        spec.params.push(assets::Promoted { label: "Offset".into(), node: xform, index: 0, min: -5.0, max: 5.0 });
        values.push(0.0);
    }
    (app, geo, subnet, xform)
}

fn centre_x(g: &Graph, id: NodeId, time: f32) -> f32 {
    let (lo, hi) = g.cook(id, time, &mut HashMap::new()).bounds().expect("geometry");
    (lo.x + hi.x) / 2.0
}

fn set_offset(g: &mut Graph, subnet: NodeId, value: f32) {
    if let Some(NodeKind::Subnet { values, .. }) = g.nodes.get_mut(&subnet).map(|n| &mut n.kind) {
        values[0] = value;
    }
}

#[test]
fn a_subnet_cooks_its_network_with_its_promoted_parameters() {
    let (mut app, geo, subnet, xform) = subnet_scene();
    assert!(centre_x(&app.graph, subnet, 0.0).abs() < 1e-4, "the input comes through");
    let before = app.graph.cook_fingerprints(0);
    set_offset(&mut app.graph, subnet, 2.0);
    assert!((centre_x(&app.graph, subnet, 0.0) - 2.0).abs() < 1e-4, "the slider moves it");
    assert!((centre_x(&app.graph, geo, 0.0) - 2.0).abs() < 1e-4, "and the object shows it");
    let after = app.graph.cook_fingerprints(0);
    assert_ne!(before[&xform], after[&xform], "the node it drives cooks again");
    // The Transform's own value is untouched; the Subnet's is what counts.
    assert!(matches!(app.graph.nodes[&xform].kind, NodeKind::Transform { t: [0.0, _, _], .. }));
    assert_eq!(app.graph.nodes[&subnet].kind.param_names(), vec!["Offset".to_owned()]);

    // Keyed, the slider animates the network.
    assert!(!app.graph.time_dependent(xform));
    let channel = ChannelId { node: subnet, target: ChannelTarget::Param(0) };
    let keys = app.graph.channels.entry(channel).or_default();
    keys.set_key(0.0, 0.0);
    keys.set_key(1.0, 4.0);
    assert!(app.graph.time_dependent(xform) && app.graph.time_dependent(subnet));
    assert!((centre_x(&app.graph, subnet, 0.5) - 2.0).abs() < 0.05);
}

#[test]
fn subnets_save_and_duplicate_with_their_promoted_parameters() {
    let (mut app, _, subnet, xform) = subnet_scene();
    set_offset(&mut app.graph, subnet, 1.5);
    let loaded = Graph::deserialize(&app.graph.serialize());
    assert_eq!(format!("{:?}", loaded.nodes[&subnet].kind), format!("{:?}", app.graph.nodes[&subnet].kind), "saved and loaded");
    assert!((centre_x(&loaded, subnet, 0.0) - 1.5).abs() < 1e-4);

    let copy = app.graph.duplicate(subnet).unwrap();
    let NodeKind::Subnet { spec, .. } = &app.graph.nodes[&copy].kind else { panic!() };
    let target = spec.params[0].node;
    assert_ne!(target, xform, "the copy's slider drives the copy's Transform");
    assert_eq!(app.graph.nodes[&target].parent, Some(copy));
    set_offset(&mut app.graph, copy, -3.0);
    assert!((centre_x(&app.graph, subnet, 0.0) - 1.5).abs() < 1e-4, "the original is unchanged");
    app.graph.connect(app.graph.input_of(subnet, 0).unwrap(), copy, 0);
    assert!((centre_x(&app.graph, copy, 0.0) + 3.0).abs() < 1e-4);
}

#[test]
fn assets_are_saved_placed_from_the_tab_menu_and_updated() {
    let (mut app, geo, subnet, _) = subnet_scene();
    let dir = std::env::temp_dir().join(format!("pfx_asset_lib_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.asset_dir = Some(dir.clone());

    // Version 1: offset 1.
    set_offset(&mut app.graph, subnet, 1.0);
    std::fs::write(dir.join("shifter.pfxasset"), app.graph.asset_text(subnet, "Shifter", 1, "Moves things")).unwrap();
    app.asset_library = assets::library(Some(&dir));
    assert_eq!(app.asset_library.len(), assets::BUILT_IN.len() + 1, "the built-in assets, and this one");

    // It's in the TAB menu inside a geometry network.
    let menu = app.templates_for_view();
    let (_, kinds) = menu.iter().find(|(category, _)| *category == "Assets").expect("an Assets menu");
    let kind = kinds.iter().find(|k| k.menu_name() == "Shifter").expect("the saved asset is listed").clone();
    assert_eq!(kind.menu_name(), "Shifter");

    // Placed: a working copy, fed like any node.
    app.apply(Action::Add(kind.clone(), egui::pos2(200.0, 100.0)));
    let placed = app.selected.unwrap();
    assert_ne!(placed, subnet);
    assert_eq!(app.graph.nodes[&placed].parent, Some(geo));
    app.graph.connect(app.graph.input_of(subnet, 0).unwrap(), placed, 0);
    assert!((centre_x(&app.graph, placed, 0.0) - 1.0).abs() < 1e-4, "{}", app.status);
    let NodeKind::Subnet { spec, .. } = &app.graph.nodes[&placed].kind else { panic!() };
    assert_eq!(spec.asset, Some(("Shifter".into(), 1)));
    assert_eq!(spec.description, "Moves things");
    assert_eq!(app.graph.nodes[&spec.params[0].node].parent, Some(placed), "its slider drives its own nodes");
    set_offset(&mut app.graph, placed, 4.0);

    // Version 2 adds a node: copies update and keep their slider's value.
    let inner = app.graph.display_of(subnet).unwrap();
    let extra = app.graph.add(NodeKind::Transform { t: [0.0, 10.0, 0.0], r: [0.0; 3], s: [1.0; 3], p: [0.0; 3] }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&extra).unwrap().parent = Some(subnet);
    app.graph.connect(inner, extra, 0);
    app.graph.displays.insert(subnet, extra);
    std::fs::write(dir.join("shifter.pfxasset"), app.graph.asset_text(subnet, "Shifter", 2, "Moves things up")).unwrap();
    app.asset_library = assets::library(Some(&dir));
    let text = std::fs::read_to_string(dir.join("shifter.pfxasset")).unwrap();
    let before = app.graph.descendants(placed).len();
    app.graph.fill_from_asset(placed, &text).unwrap();
    assert_eq!(app.graph.descendants(placed).len(), before + 1, "the old network was replaced, not added to");
    let (lo, hi) = app.graph.cook(placed, 0.0, &mut HashMap::new()).bounds().unwrap();
    assert!(((lo.x + hi.x) / 2.0 - 4.0).abs() < 1e-4, "the slider kept its value");
    assert!(((lo.y + hi.y) / 2.0 - 10.0).abs() < 1e-4, "and the new node is there");
    let NodeKind::Subnet { spec, .. } = &app.graph.nodes[&placed].kind else { panic!() };
    assert_eq!(spec.asset, Some(("Shifter".into(), 2)));

    // Every older copy at once: place two more of version 2, make them look like
    // version 1, and update them together.
    for x in [400.0, 600.0] {
        app.apply(Action::Add(kind.clone(), egui::pos2(x, 100.0)));
    }
    let copies: Vec<NodeId> = app.graph.nodes.iter().filter(|(_, n)| matches!(&n.kind, NodeKind::Subnet { spec: assets::Spec { asset: Some((a, _)), .. }, .. } if a == "Shifter")).map(|(id, _)| *id).collect();
    let v1 = app.graph.asset_text(subnet, "Shifter", 1, "Moves things");
    for &c in &copies {
        if c != subnet {
            app.graph.fill_from_asset(c, &v1).unwrap();
        }
    }
    // (The Subnet the asset was saved from isn't a placed copy.)
    assert_eq!(copies.len(), 3);
    assert_eq!(app.graph.asset_copies("Shifter", 2).len(), 3, "all three placed copies are on version 1");
    assert_eq!(app.graph.update_asset_copies("Shifter", 2, &text).unwrap(), 3);
    assert!(app.graph.asset_copies("Shifter", 2).is_empty());

    // A file that isn't an asset says so.
    assert!(app.graph.fill_from_asset(placed, "pfs 2\nnext 0\n").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn subnet_inputs_are_only_offered_inside_subnets() {
    let (mut app, geo, subnet, _) = subnet_scene();
    let offers = |app: &ProceduralFxStudioApp| app.templates_for_view().iter().flat_map(|(_, k)| k.clone()).any(|k| matches!(k, NodeKind::SubnetInput { .. }));
    app.current_network = Some(geo);
    assert!(!offers(&app));
    app.current_network = Some(subnet);
    assert!(offers(&app));
}
