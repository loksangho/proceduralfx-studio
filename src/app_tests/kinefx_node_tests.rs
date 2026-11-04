use crate::*;

#[test]
fn a_walking_skeleton_deforms_a_captured_mesh() {
    let mut g = Graph::default();
    g.next_id += 9700;
    let rest = g.add(NodeKind::Skeleton(kinefx::SkeletonSettings::defaults()), egui::pos2(0.0, 0.0));
    let walk = g.add(NodeKind::ProceduralMotion(kinefx::MotionSettings { travel: 0.0, ..kinefx::MotionSettings::defaults() }), egui::pos2(0.0, 0.0));
    let ik = g.add(NodeKind::TwoBoneIk(kinefx::IkSettings { blend: 0.0, ..kinefx::IkSettings::defaults() }), egui::pos2(0.0, 0.0));
    g.connect(rest, walk, 0);
    g.connect(walk, ik, 0);
    assert!(g.time_dependent(ik));
    // A "body": points down the left leg.
    let body = g.add(NodeKind::Add { count: 20, spacing: 0.045, layout: 1, points: Vec::new(), delete_geometry: false }, egui::pos2(0.0, 0.0));
    let place = g.add(NodeKind::Transform { t: [0.13, 0.08, 0.0], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    g.connect(body, place, 0);
    let capture = g.add(NodeKind::JointCapture(kinefx::CaptureSettings::defaults()), egui::pos2(0.0, 0.0));
    g.connect(place, capture, 0);
    g.connect(rest, capture, 1);
    let deform = g.add(NodeKind::BoneDeform, egui::pos2(0.0, 0.0));
    g.connect(capture, deform, 0);
    g.connect(rest, deform, 1);
    g.connect(ik, deform, 2);
    let a = g.cook(deform, 0.25, &mut HashMap::new());
    let b = g.cook(deform, 0.75, &mut HashMap::new());
    for id in [rest, walk, ik, capture, deform] {
        assert!(node_errors(|e| e.get(&id).cloned()).is_none(), "{:?}: {:?}", g.nodes[&id].kind, node_errors(|e| e.get(&id).cloned()));
    }
    // The leg swings, so the skinned points move between the two times.
    let moved = a.vertices.iter().zip(&b.vertices).map(|(p, q)| p.sub(*q).length()).fold(0.0f32, f32::max);
    assert!(moved > 0.1, "skinned points moved {moved}");
    assert!(kinefx::Skeleton::is_skeleton(&g.cook(ik, 0.25, &mut HashMap::new())));
    // A posing node on something that isn't a skeleton says so.
    let not = g.add(NodeKind::RigPose(kinefx::RigPose::new()), egui::pos2(0.0, 0.0));
    g.connect(body, not, 0);
    g.cook(not, 0.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&not).cloned()).unwrap().contains("skeleton"));
}
