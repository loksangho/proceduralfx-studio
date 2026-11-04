use crate::*;

#[test]
fn fractured_pieces_feed_rbd_and_crowds_and_muscles_cook() {
    let mut g = Graph::default();
    g.next_id += 9500;
    let cube = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    let frac = g.add(NodeKind::MaterialFracture(fracture::FractureSettings { pieces: 8, ..fracture::FractureSettings::defaults(0) }), egui::pos2(0.0, 0.0));
    g.connect(cube, frac, 0);
    let pieces = g.cook(frac, 0.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&frac).cloned()).is_none());
    let count = pieces.point_attributes["piece"].iter().fold(0.0f32, |m, p| m.max(*p)) as usize + 1;
    assert!(count >= 4, "{count} pieces");

    // Crowd: agents from a few points, walking.
    let starts = g.add(NodeKind::Add { count: 6, spacing: 1.0, layout: 2, points: Vec::new(), delete_geometry: false }, egui::pos2(0.0, 0.0));
    let crowd_node = g.add(NodeKind::CrowdSolver(crowd::CrowdSettings::defaults()), egui::pos2(0.0, 0.0));
    g.connect(starts, crowd_node, 0);
    assert!(g.time_dependent(crowd_node));
    sims::clear_all();
    let a = g.cook(crowd_node, 1.0, &mut HashMap::new());
    let b = g.cook(crowd_node, 2.0, &mut HashMap::new());
    assert!(!a.indices.is_empty() && a.vertices != b.vertices);
    assert_eq!(a.point_attributes["prim:agent"].len(), a.indices.len());

    // Muscles: flexing (activation keyframed) drives the jiggle solver and the skin.
    let m = g.add(NodeKind::Muscle(muscle::MuscleSettings::defaults()), egui::pos2(0.0, 0.0));
    let target = ChannelId { node: m, target: ChannelTarget::Param(10) };
    let mut channel = anim::Channel::default();
    channel.set_key(0.0, 0.0);
    channel.set_key(1.0, 1.0);
    g.channels.insert(target, channel);
    let solver = g.add(NodeKind::MuscleSolver(muscle::JiggleSettings::defaults()), egui::pos2(0.0, 0.0));
    g.connect(m, solver, 0);
    assert!(g.time_dependent(solver));
    // A skin tube around the muscle (y 0..1), however the Tube node centres itself.
    let tube = g.add(NodeKind::Tube { r_bot: 0.16, r_top: 0.16, height: 1.0, rows: 8, cols: 16, caps: false }, egui::pos2(0.0, 0.0));
    let tube_mesh = g.cook(tube, 0.0, &mut HashMap::new());
    let (lo, _) = tube_mesh.bounds().unwrap();
    let skin_src = g.add(NodeKind::Transform { t: [0.0, -lo.y, 0.0], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    g.connect(tube, skin_src, 0);
    let skin = g.add(NodeKind::MuscleSkin(muscle::SkinSettings::defaults()), egui::pos2(0.0, 0.0));
    g.connect(skin_src, skin, 0);
    g.connect(solver, skin, 1);
    let rest = g.cook(skin, 0.0, &mut HashMap::new());
    let flexed = g.cook(skin, 1.5, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&skin).cloned()).is_none(), "{:?}", node_errors(|e| e.get(&skin).cloned()));
    assert_eq!(rest.vertices.len(), flexed.vertices.len());
    assert!(rest.vertices.iter().zip(&flexed.vertices).any(|(a, b)| a.sub(*b).length() > 1e-3), "skin should move as the muscle flexes");
}
