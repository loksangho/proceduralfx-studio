use crate::*;

#[test]
fn fracture_outputs_feed_the_bullet_solver() {
    let mut g = Graph::default();
    g.next_id += 10_100;
    let block = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    let lift = g.add(NodeKind::Transform { t: [0.0, 2.0, 0.0], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    g.connect(block, lift, 0);
    let frac = g.add(NodeKind::MaterialFracture(fracture::FractureSettings { pieces: 6, ..fracture::FractureSettings::defaults(0) }), egui::pos2(0.0, 0.0));
    g.connect(lift, frac, 0);
    let config = g.add(NodeKind::RbdConfigure(bullet::ConfigureSettings::defaults()), egui::pos2(0.0, 0.0));
    let solver = g.add(NodeKind::RbdBullet(bullet::BulletSettings::defaults()), egui::pos2(0.0, 0.0));
    for out in 0..3 {
        assert!(g.connect_out(frac, out, config, out));
        assert!(g.connect_out(config, out, solver, out));
    }
    // Each output is its own geometry.
    let glue = g.cook_output(frac, 1, 0.0, &mut HashMap::new());
    let proxy = g.cook_output(frac, 2, 0.0, &mut HashMap::new());
    assert!(!glue.vertices.is_empty() && glue.point_attributes.contains_key(fracture::CONSTRAINT_PIECE));
    assert!(!proxy.indices.is_empty());
    assert_eq!(g.cook_output(config, 1, 0.0, &mut HashMap::new()).vertices.len(), glue.vertices.len(), "configure passes constraints through");
    assert!(g.time_dependent(solver));
    sims::clear_all();
    let landed = g.cook(solver, 2.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&solver).cloned()).is_none());
    let (lo, _) = landed.bounds().unwrap();
    assert!(lo.y < 0.3 && lo.y > -0.05, "fell onto the ground: {}", lo.y);
    let moved_proxy = g.cook_output(solver, 2, 2.0, &mut HashMap::new());
    assert_eq!(moved_proxy.vertices.len(), proxy.vertices.len());
    // Links remember their outputs through save and load.
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(loaded.input_link(solver, 2), Some((config, 2)));
    assert_eq!(loaded.input_link(config, 1), Some((frac, 1)));
    // Merge takes any number of inputs, and closes gaps when one is removed.
    let merge = g.add(NodeKind::Merge, egui::pos2(0.0, 0.0));
    for i in 0..3 {
        let b = g.add(NodeKind::Box { size: 1.0 + i as f32, div: 1 }, egui::pos2(0.0, 0.0));
        assert_eq!(g.input_slots(merge), i + 1, "always exactly one free spot");
        g.connect(b, merge, i);
    }
    assert_eq!(g.cook(merge, 0.0, &mut HashMap::new()).vertices.len(), 24);
    assert_eq!(g.input_slots(merge), 4);
    g.disconnect(merge, 0);
    assert_eq!(g.input_slots(merge), 3, "the gap closed: two inputs and a free port");
    // A wire dropped on a Merge (body or a used port) always adds an input.
    assert_eq!(g.drop_slot(merge), Some(2));
    let xf = g.add(NodeKind::Transform { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    assert_eq!(g.drop_slot(xf), Some(0), "other nodes: the first free input");
    let boolean = g.add(NodeKind::Boolean(boolean::BooleanSettings { operation: 2, mode: 0, resolution: 128 }), egui::pos2(0.0, 0.0));
    let a = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    g.connect(a, boolean, 0);
    assert_eq!(g.drop_slot(boolean), Some(1));

    // The STAGE's Merge takes any number of inputs too, layering them in order.
    let lops: Vec<NodeId> = ["/a", "/b", "/c"]
        .iter()
        .map(|path| {
            let id = g.add(NodeKind::LopPrimitive { shape: 0, path: (*path).into(), size: 1.0, t: [0.0; 3] }, egui::pos2(0.0, 0.0));
            g.nodes.get_mut(&id).unwrap().context = AppContext::Stage;
            id
        })
        .collect();
    let lop_merge = g.add(NodeKind::LopMerge, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&lop_merge).unwrap().context = AppContext::Stage;
    for (i, lop) in lops.iter().enumerate() {
        assert_eq!(g.drop_slot(lop_merge), Some(i));
        g.connect(*lop, lop_merge, i);
    }
    assert_eq!(g.input_slots(lop_merge), 4);
    let stage = g.cook_stage(lop_merge, 0.0, &mut HashMap::new());
    assert!(["/a", "/b", "/c"].iter().all(|p| stage.prims.contains_key(*p)), "every input's prims");
    assert!(g.input_link(merge, 0).is_some() && g.input_link(merge, 2).is_none());
    // Pack, move, unpack: the pieces come back turned with their points.
    let pack = g.add(NodeKind::Pack(packed::PackSettings::defaults()), egui::pos2(0.0, 0.0));
    g.connect(frac, pack, 0);
    let spin = g.add(NodeKind::Transform { t: [0.0; 3], r: [0.0, 90.0, 0.0], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    g.connect(pack, spin, 0);
    let unpack = g.add(NodeKind::Unpack { transfer: true }, egui::pos2(0.0, 0.0));
    g.connect(spin, unpack, 0);
    let packed_points = g.cook(pack, 0.0, &mut HashMap::new());
    assert!(packed::is_packed(&packed_points) && packed_points.indices.is_empty());
    let direct = g.cook(frac, 0.0, &mut HashMap::new());
    let turned = node_transform(direct.clone(), [0.0; 3], [0.0, 90.0, 0.0], [1.0; 3]);
    let unpacked = g.cook(unpack, 0.0, &mut HashMap::new());
    let key = |m: &Mesh| { let mut v: Vec<(i32, i32, i32)> = m.vertices.iter().map(|p| ((p.x * 1000.0).round() as i32, (p.y * 1000.0).round() as i32, (p.z * 1000.0).round() as i32)).collect(); v.sort(); v };
    assert_eq!(key(&unpacked), key(&turned));
}
