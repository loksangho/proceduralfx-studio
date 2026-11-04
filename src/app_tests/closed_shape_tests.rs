use crate::*;

fn volume(m: &Mesh, tris: impl Iterator<Item = usize>) -> f32 {
    tris.map(|t| { let [a, b, c] = m.indices[t].map(|i| m.vertices[i]); a.dot(b.cross(c)) / 6.0 }).sum()
}

// Directed edges without their opposite: 0 for a closed, consistently wound surface.
fn open_edges(m: &Mesh) -> usize {
    let mut e: HashMap<(usize, usize), i32> = HashMap::new();
    for x in &m.indices {
        for k in 0..3 {
            *e.entry((x[k], x[(k + 1) % 3])).or_default() += 1;
        }
    }
    e.iter().filter(|((a, b), n)| e.get(&(*b, *a)).copied().unwrap_or(0) != **n).count()
}

#[test]
fn closed_generators_are_closed_and_face_outwards() {
    for (name, m) in [
        ("box", node_box(2.0, 2)),
        ("sphere", node_sphere(1.0, 8, 12)),
        ("tube", node_tube(1.0, 0.5, 2.0, 2, 12, true)),
        ("torus", node_torus(1.0, 0.3, 8, 12)),
        ("capsule", node_capsule(0.5, 1.0, 8, 12)),
        ("cone", node_cone(1.0, 2.0, 3, 12, true)),
        ("cone, one row", node_cone(1.0, 2.0, 1, 32, true)),
    ] {
        assert_eq!(open_edges(&m), 0, "{name} has holes or flipped faces");
        assert!(volume(&m, 0..m.indices.len()) > 0.0, "{name} is inside out");
    }
    // The cone narrows to one apex point, and its base is one polygon.
    let cone = node_cone(1.0, 2.0, 1, 32, true);
    assert_eq!(cone.vertices.iter().filter(|v| (v.y - 1.0).abs() < 1e-6).count(), 1);
    assert_eq!(poly::count(&cone), 32 + 1);
}

#[test]
fn a_fractured_cone_keeps_its_volume() {
    let g = Graph::deserialize("pfs 2\nnode 8 0 0 0 obj cone 1 2 1 32 1\nnode 10 0 0 0 obj material_fracture 0 20 1 0 0 0 4 9 1 5 0 200\nlink 8 10 0\nnext 11\n");
    let cone = g.cook(8, 0.0, &mut HashMap::new());
    let pieces = g.cook(10, 0.0, &mut HashMap::new());
    let (whole, cut) = (volume(&cone, 0..cone.indices.len()), volume(&pieces, 0..pieces.indices.len()));
    assert!((whole - cut).abs() < 1e-3 * whole, "the pieces add up to the cone: {cut} vs {whole}");
    let piece = &pieces.point_attributes["prim:piece"];
    let count = piece.iter().map(|p| *p as i64).collect::<HashSet<_>>().len();
    assert_eq!(count, 20);
    for p in 0..20 {
        let v = volume(&pieces, (0..pieces.indices.len()).filter(|&t| piece[t] as i64 == p));
        assert!(v > 0.0, "piece {p} is inside out: {v}");
    }
}

#[test]
fn viewport_shading_keeps_sharp_edges_sharp() {
    // A box's corners shade flat (no N attribute); a sphere stays smooth.
    let cube = node_box(2.0, 1);
    for (t, corners) in sops::corner_normals(&cube).iter().enumerate() {
        let [a, b, c] = cube.indices[t].map(|i| cube.vertices[i]);
        let face = b.sub(a).cross(c.sub(a)).normalize();
        assert!(corners.iter().all(|n| n.dot(face) > 0.999));
    }
    let sphere = node_sphere(1.0, 16, 24);
    let corners = sops::corner_normals(&sphere);
    let smooth = sphere.indices.iter().zip(&corners).all(|(tri, n)| (0..3).all(|k| n[k].dot(sphere.vertices[tri[k]].normalize()) > 0.99));
    assert!(smooth);
}
