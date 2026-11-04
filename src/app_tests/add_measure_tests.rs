use crate::*;

#[test]
fn add_places_points_and_can_strip_geometry() {
    let mut g = Graph::default();
    g.next_id += 9900;
    let add = g.add(NodeKind::Add { count: 0, spacing: 1.0, layout: 0, points: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]], delete_geometry: false }, egui::pos2(0.0, 0.0));
    let out = g.cook(add, 0.0, &mut HashMap::new());
    assert_eq!(out.vertices, vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)]);
    // Onto a box, keeping only its points.
    let cube = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    let strip = g.add(NodeKind::Add { count: 0, spacing: 1.0, layout: 0, points: vec![[9.0, 9.0, 9.0]], delete_geometry: true }, egui::pos2(0.0, 0.0));
    g.connect(cube, strip, 0);
    let out = g.cook(strip, 0.0, &mut HashMap::new());
    assert!(out.indices.is_empty());
    assert_eq!(out.vertices.len(), 9);
    assert_eq!(*out.vertices.last().unwrap(), Vec3::new(9.0, 9.0, 9.0));
    // A pattern still works, and old saves (count, spacing, layout) load as before.
    let old = NodeKind::from_params("add", &[5.0, 0.5, 0.0]).unwrap();
    assert!(matches!(&old, NodeKind::Add { count: 5, points, delete_geometry: false, .. } if points.is_empty()));
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(format!("{:?}", loaded.nodes[&strip].kind), format!("{:?}", g.nodes[&strip].kind));
    // Measure in a network: a box's area.
    let measure = g.add(NodeKind::Measure(measure::MeasureSettings::new(0)), egui::pos2(0.0, 0.0));
    g.connect(cube, measure, 0);
    let m = g.cook(measure, 0.0, &mut HashMap::new());
    assert!((m.point_attributes["detail:area"][0] - 6.0).abs() < 1e-4);
}
