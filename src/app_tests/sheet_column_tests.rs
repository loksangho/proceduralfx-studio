use crate::*;

// Stored attributes come straight after P; normals worked out from the surface
// come last, and only when there is a surface.
#[test]
fn particle_attributes_come_first_in_the_spreadsheet() {
    let headers = |mesh: &Mesh| sheet_columns(mesh, SheetDomain::Points).into_iter().map(|c| c.header).collect::<Vec<_>>();
    // What the Particles solver outputs: points with v, age, life and id, no faces.
    let mut particles = Mesh { vertices: vec![Vec3::zero(), Vec3::new(1.0, 0.0, 0.0)], ..Mesh::default() };
    for (name, values) in [("v", vec![0.0; 6]), ("age", vec![0.5, 1.0]), ("life", vec![4.0; 2]), ("id", vec![0.0, 1.0])] {
        particles.point_attributes.insert(name.into(), values);
    }
    assert_eq!(&headers(&particles)[..6], ["#", "P.x", "P.y", "P.z", "age", "id"]);
    assert!(!headers(&particles).iter().any(|h| h.starts_with("N.")), "particles have no surface to take normals from");
    let mut sphere = node_sphere(1.0, 4, 6);
    sphere.point_attributes.insert("age".into(), vec![0.0; sphere.vertices.len()]);
    let columns = headers(&sphere);
    assert_eq!(&columns[4..5], ["age"]);
    assert_eq!(&columns[columns.len() - 3..], ["N.x", "N.y", "N.z"]);
}
