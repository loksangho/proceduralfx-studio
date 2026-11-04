use crate::*;

fn quads() -> Mesh {
    node_grid(2.0, 2.0, 3, 3)
}

#[test]
fn generators_make_quads_and_ngon_caps() {
    assert_eq!(poly::count(&node_box(1.0, 1)), 6);
    assert_eq!(poly::count(&node_box(1.0, 2)), 24);
    assert_eq!(poly::count(&node_torus(1.0, 0.25, 4, 6)), 24);
    // A capped tube: 3 × 8 side quads and two 8-sided caps.
    let tube = node_tube(1.0, 1.0, 2.0, 3, 8, true);
    assert_eq!(poly::count(&tube), 24 + 2);
    let caps: Vec<usize> = poly::runs(&tube).iter().filter(|r| r.len() == 8).map(|r| poly::boundary(&tube, r).unwrap().len()).collect();
    assert_eq!(caps, vec![8, 8], "each cap's outline is the ring");
    // A sphere: quads, with one triangle per pole cell.
    let sphere = node_sphere(1.0, 4, 6);
    let sizes: Vec<usize> = poly::runs(&sphere).iter().map(|r| r.len()).collect();
    assert_eq!(sizes.iter().filter(|n| **n == 2).count(), 6 * 2, "two rows of quads");
    assert_eq!(sizes.iter().filter(|n| **n == 1).count(), 6 * 2, "a triangle fan at each pole");
}

#[test]
fn vex_runs_once_per_polygon() {
    // @primnum counts polygons, and what one writes covers both its triangles.
    let out = node_wrangle(quads(), &[], "f@n = @primnum; f@np = @numprim; f@r = rand(@primnum + 0.5);", 0.0, 1, None, None);
    assert_eq!(out.point_attributes["prim:n"], vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
    assert!(out.point_attributes["prim:np"].iter().all(|v| *v == 4.0));
    let r = &out.point_attributes["prim:r"];
    assert!(r.chunks(2).all(|pair| pair[0] == pair[1]));
    // @P is the polygon's centre; removeprim removes the whole quad.
    let out = node_wrangle(quads(), &[], "if (@P.x > 0 && @P.z > 0) removeprim(0, @primnum, 1);", 0.0, 1, None, None);
    assert_eq!((poly::count(&out), out.indices.len(), out.vertices.len()), (3, 6, 8));
    // Point domain functions see polygons too.
    let out = node_wrangle(quads(), &[], "i@n = nprimitives(0); i@c = len(primpoints(0, 0)); i@p = len(pointprims(0, 4));", 0.0, 2, None, None);
    assert_eq!((out.point_attributes["n"][0], out.point_attributes["c"][0], out.point_attributes["p"][0]), (4.0, 4.0, 4.0));
    // addprim adds one polygon of any number of points.
    let out = node_wrangle(quads(), &[], "int a = addpoint(0, {0, 1, 0}); addprim(0, \"poly\", 0, 1, 2, 5, a);", 0.0, 0, None, None);
    assert_eq!((poly::count(&out), out.indices.len()), (5, 8 + 3));
    let out = node_wrangle(quads(), &[], "int pts[] = array(0, 1, 2); addprim(0, \"poly\", pts);", 0.0, 0, None, None);
    assert_eq!(out.indices.len(), 9);
    // xyzdist gives a polygon and a uv that primuv reads back.
    let out = node_wrangle(quads(), &[], "int prim; vector uv; xyzdist(0, {0.7, 0.3, 0.6}, prim, uv); i@hit = prim; v@back = primuv(0, \"P\", prim, uv);", 0.0, 0, None, None);
    assert_eq!(out.point_attributes["detail:hit"][0], 3.0);
    let back = &out.point_attributes["detail:back"];
    assert!((back[0] - 0.7).abs() < 1e-4 && back[1].abs() < 1e-4 && (back[2] - 0.6).abs() < 1e-4, "{back:?}");
}

#[test]
fn files_keep_quads() {
    let grid = quads();
    let obj = fileio::write_obj(&grid);
    assert_eq!(obj.lines().filter(|l| l.starts_with("f ")).count(), 4);
    assert!(obj.lines().filter(|l| l.starts_with("f ")).all(|l| l.split_whitespace().count() == 5));
    let back = fileio::read_obj(&obj).unwrap();
    assert_eq!((poly::count(&back), back.indices.len()), (4, 8));
    let mut stage = usd::Stage::default();
    stage.define("/geo", "Mesh").mesh = Some(Arc::new(grid));
    let text = usd::to_usda(&stage);
    assert!(text.contains("faceVertexCounts = [4, 4, 4, 4]"), "{text}");
    assert!(!text.contains("__poly"));
}

#[test]
fn copies_turn_by_orient_quaternions_and_normals_follow() {
    let piece = node_box(1.0, 1);
    let mut points = Mesh::default();
    points.vertices = vec![Vec3::new(5.0, 0.0, 0.0)];
    // 90° about Z: (sin 45°, cos 45°) on the z axis.
    let h = std::f32::consts::FRAC_1_SQRT_2;
    points.point_attributes.insert("orient".into(), vec![0.0, 0.0, h, h]);
    let mut tall = piece.clone();
    tall.vertices.iter_mut().for_each(|v| v.y *= 3.0);
    let copies = node_copy_to_points(tall, points.clone(), 1.0, [0.0; 3]);
    let (lo, hi) = copies.bounds().unwrap();
    assert!((hi.x - lo.x - 3.0).abs() < 1e-4 && (hi.y - lo.y - 1.0).abs() < 1e-4, "the tall box lies along x: {lo:?} {hi:?}");
    assert!((lo.add(hi).scale(0.5).x - 5.0).abs() < 1e-4);
    // A single-number orient still turns about Y in degrees.
    points.point_attributes.insert("orient".into(), vec![90.0]);
    let mut long = piece.clone();
    long.vertices.iter_mut().for_each(|v| v.x *= 3.0);
    let (lo, hi) = node_copy_to_points(long, points, 1.0, [0.0; 3]).bounds().unwrap();
    assert!((hi.z - lo.z - 3.0).abs() < 1e-4);
    // Transform turns normals.
    let mut grid = quads();
    grid.point_attributes.insert("N".into(), vec![0.0, 1.0, 0.0].repeat(9));
    let turned = node_transform(grid, [0.0; 3], [90.0, 0.0, 0.0], [1.0, 2.0, 1.0]);
    let n = &turned.point_attributes["N"];
    assert!(n[1].abs() < 1e-4 && (n[0].powi(2) + n[2].powi(2) - 1.0).abs() < 1e-4, "{:?}", &n[..3]);
}

#[test]
fn merging_fills_missing_colour_with_white() {
    let mut red = node_box(1.0, 1);
    let n = red.vertices.len();
    red.point_attributes.insert("Cd".into(), [1.0, 0.0, 0.0].repeat(n));
    red.point_attributes.insert("mass".into(), vec![3.0; n]);
    let merged = node_merge(quads(), red);
    let cd = &merged.point_attributes["Cd"];
    assert!(cd[..27].iter().all(|v| *v == 1.0), "the grid is white, not black");
    assert!(merged.point_attributes["mass"][..9].iter().all(|v| *v == 0.0), "others default to 0");
}

#[test]
fn merging_with_normals_keeps_the_other_sides_lit() {
    let mut rocks = node_sphere(1.0, 6, 8);
    let n = rocks.compute_normals();
    rocks.point_attributes.insert("N".into(), n.iter().flat_map(|v| [v.x, v.y, v.z]).collect());
    let merged = node_merge(quads(), rocks);
    let normals = &merged.point_attributes["N"];
    // The grid (first 9 points) faces up, not zero.
    assert!(normals[..27].chunks(3).all(|c| (c[1].abs() - 1.0).abs() < 1e-5), "{:?}", &normals[..27]);
}

#[test]
fn subdivide_mirror_and_blast_keep_faces() {
    let mut grid = quads();
    grid.point_attributes.insert("prim:tag".into(), (0..8).map(|t| (t / 2) as f32).collect());
    // Each quad becomes four quads, keeping its primitive attributes.
    let fine = node_subdivide(grid.clone(), 1);
    assert_eq!((poly::count(&fine), fine.indices.len()), (16, 32));
    assert_eq!(fine.point_attributes["prim:tag"][..8], [0.0; 8]);
    // A mirror's copy keeps its own polygons.
    let mirrored = node_mirror(grid.clone(), 1);
    assert_eq!(poly::count(&mirrored), 8);
    // Blast keeps the primitive attributes of what's left.
    let blasted = node_blast(grid, 1, &[0, 1], false);
    assert_eq!(blasted.point_attributes["prim:tag"], vec![1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
    assert_eq!(poly::count(&blasted), 3);
}

#[test]
fn spreadsheet_rows_are_polygons() {
    let mut grid = quads();
    grid.point_attributes.insert("prim:tag".into(), (0..8).map(|t| (t / 2) as f32 * 10.0).collect());
    assert_eq!(SheetDomain::Primitives.count(&grid), 4);
    let columns = sheet_columns(&grid, SheetDomain::Primitives);
    assert!(columns.iter().all(|c| !c.header.contains("__poly")));
    let polys = SheetPolys::new(&grid);
    let points = columns.iter().find(|c| c.header == "Points").unwrap();
    assert_eq!(sheet_cell(&grid, &[], &polys, &points.source, 3).split(' ').count(), 4);
    let tag = columns.iter().find(|c| c.header == "tag").unwrap();
    assert_eq!(sheet_cell(&grid, &[], &polys, &tag.source, 3), "30");
}
