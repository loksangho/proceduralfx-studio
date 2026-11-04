use crate::*;

#[test]
fn point_group_names_select_their_primitives() {
    let mut grid = node_grid(2.0, 2.0, 3, 3);
    // Points of the first row of quads.
    grid.point_attributes.insert("group:bottom".into(), (0..9).map(|i| (i < 6) as u8 as f32).collect());
    let out = material_assign(&grid, &[("bottom".into(), Some(4))]).unwrap();
    let ids = &out.point_attributes[PRIM_MATERIAL];
    // The first row of quads: four triangles.
    assert_eq!(ids, &vec![5.0, 5.0, 5.0, 5.0, 0.0, 0.0, 0.0, 0.0]);
    // Primitive groups by name work directly.
    let mut tagged = grid.clone();
    tagged.point_attributes.insert("prim:group:roof".into(), vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    let out = material_assign(&tagged, &[("roof".into(), Some(1))]).unwrap();
    assert_eq!(out.point_attributes[PRIM_MATERIAL], vec![0.0, 0.0, 2.0, 2.0, 0.0, 0.0, 0.0, 0.0]);
}
