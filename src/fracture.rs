// Material Fracture: breaks a closed mesh into pieces for the RBD solver.
//
// Every piece is a Voronoi cell (a convex polyhedron: a box clipped by the
// bisector planes to the other seeds) intersected with the input by the exact
// Boolean, so pieces are closed and their cut faces are flat. Materials differ
// in where the seeds go:
// - Concrete: seeds scattered through the volume, chunky pieces.
// - Glass: seeds in rings and spokes around an impact point on the pane's
//   middle plane, so cracks radiate and shards get smaller near the impact.
// - Wood: cells computed in space squashed along the grain, so pieces are long
//   splinters along it.
//
// Output: `piece` (point and prim attributes, what RBD reads), and prim groups
// `inside` (new cut faces) and `outside` (the original surface).

use super::{hash01, Mesh, Vec3};
use crate::boolean::{boolean_with, BooleanSettings};

pub const MATERIALS: [&str; 3] = ["Concrete", "Glass", "Wood"];

#[derive(Clone, Debug, PartialEq)]
pub struct FractureSettings {
    pub material: usize,
    pub pieces: usize,
    pub seed: u32,
    // Glass: impact point and crack pattern.
    pub impact: [f32; 3],
    pub rings: usize,
    pub spokes: usize,
    // Wood: grain axis (0 x, 1 y, 2 z) and how much longer splinters are than wide.
    pub grain_axis: usize,
    pub splinter: f32,
    // Moves pieces apart (fraction of their size) to show the cracks.
    pub separate: f32,
    // Force the glue constraints between neighbouring pieces hold before breaking.
    pub glue_strength: f32,
}

impl FractureSettings {
    pub fn defaults(material: usize) -> Self {
        FractureSettings { material, pieces: 20, seed: 1, impact: [0.0; 3], rings: 4, spokes: 9, grain_axis: 1, splinter: 5.0, separate: 0.0, glue_strength: 200.0 }
    }
}

// A convex polyhedron as convex polygons, wound counter-clockwise from outside.
struct Convex {
    faces: Vec<Vec<Vec3>>,
    // The seed whose bisector made each face (None: the outer box).
    tags: Vec<Option<usize>>,
}

impl Convex {
    fn cuboid(lo: Vec3, hi: Vec3) -> Convex {
        let c = |x: usize, y: usize, z: usize| {
            Vec3::new(if x == 0 { lo.x } else { hi.x }, if y == 0 { lo.y } else { hi.y }, if z == 0 { lo.z } else { hi.z })
        };
        Convex {
            faces: vec![
                vec![c(0, 0, 0), c(0, 1, 0), c(1, 1, 0), c(1, 0, 0)],
                vec![c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)],
                vec![c(0, 0, 0), c(1, 0, 0), c(1, 0, 1), c(0, 0, 1)],
                vec![c(0, 1, 0), c(0, 1, 1), c(1, 1, 1), c(1, 1, 0)],
                vec![c(0, 0, 0), c(0, 0, 1), c(0, 1, 1), c(0, 1, 0)],
                vec![c(1, 0, 0), c(1, 1, 0), c(1, 1, 1), c(1, 0, 1)],
            ],
            tags: vec![None; 6],
        }
    }

    // Keeps the part where n·x <= w and closes the cut with a cap face.
    fn clip(&mut self, n: Vec3, w: f32, tag: usize) {
        let eps = 1e-6;
        let mut cut: Vec<Vec3> = Vec::new();
        let mut faces = Vec::with_capacity(self.faces.len() + 1);
        let mut tags = Vec::with_capacity(self.faces.len() + 1);
        for (face, face_tag) in self.faces.iter().zip(&self.tags) {
            let mut out = Vec::new();
            for i in 0..face.len() {
                let (a, b) = (face[i], face[(i + 1) % face.len()]);
                let (da, db) = (n.dot(a) - w, n.dot(b) - w);
                if da <= eps {
                    out.push(a);
                }
                if (da < -eps && db > eps) || (da > eps && db < -eps) {
                    let p = a.add(b.sub(a).scale(da / (da - db)));
                    out.push(p);
                    cut.push(p);
                } else if da.abs() <= eps {
                    cut.push(a);
                }
            }
            if out.len() >= 3 {
                faces.push(out);
                tags.push(*face_tag);
            }
        }
        if cut.len() >= 3 {
            // Order the cut points around their centre; the cap faces along +n.
            let centre = cut.iter().fold(Vec3::zero(), |s, p| s.add(*p)).scale(1.0 / cut.len() as f32);
            let helper = if n.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
            let u = n.cross(helper).normalize();
            let v = n.cross(u);
            cut.sort_by(|a, b| {
                let (pa, pb) = (a.sub(centre), b.sub(centre));
                pa.dot(v).atan2(pa.dot(u)).partial_cmp(&pb.dot(v).atan2(pb.dot(u))).unwrap_or(std::cmp::Ordering::Equal)
            });
            cut.dedup_by(|a, b| a.sub(*b).length() < 1e-6);
            if cut.len() >= 3 {
                faces.push(cut);
                tags.push(Some(tag));
            }
        }
        self.faces = faces;
        self.tags = tags;
    }

    // Seeds whose cells touch this one.
    fn neighbours(&self) -> Vec<usize> {
        let mut n: Vec<usize> = self.tags.iter().flatten().copied().collect();
        n.sort_unstable();
        n.dedup();
        n
    }

    fn to_mesh(&self) -> Mesh {
        let mut mesh = Mesh::default();
        for face in &self.faces {
            let base = mesh.vertices.len();
            mesh.vertices.extend_from_slice(face);
            for k in 1..face.len() - 1 {
                mesh.indices.push([base, base + k, base + k + 1]);
            }
        }
        mesh
    }
}

fn axis(v: Vec3, a: usize) -> f32 {
    [v.x, v.y, v.z][a.min(2)]
}

fn with_axis(mut v: Vec3, a: usize, value: f32) -> Vec3 {
    match a.min(2) {
        0 => v.x = value,
        1 => v.y = value,
        _ => v.z = value,
    }
    v
}

fn scale_axis(mesh: &Mesh, a: usize, s: f32) -> Mesh {
    let mut out = mesh.clone();
    out.vertices.iter_mut().for_each(|v| *v = with_axis(*v, a, axis(*v, a) * s));
    out
}

// Seed points for the chosen material, in the space the cells are built in.
fn seeds(mesh: &Mesh, set: &FractureSettings, lo: Vec3, hi: Vec3) -> Result<Vec<Vec3>, String> {
    let count = set.pieces.clamp(2, 500);
    if set.material == 1 {
        // The pane's thin axis is the smallest extent; seeds lie on its middle plane.
        let e = hi.sub(lo);
        let thin = if e.x <= e.y && e.x <= e.z { 0 } else if e.y <= e.z { 1 } else { 2 };
        let (a1, a2) = ((thin + 1) % 3, (thin + 2) % 3);
        let mid = (axis(lo, thin) + axis(hi, thin)) * 0.5;
        let impact = Vec3::from_arr(set.impact);
        let reach = axis(e, a1).max(axis(e, a2));
        let mut out = vec![with_axis(impact, thin, mid)];
        let (rings, spokes) = (set.rings.clamp(1, 20), set.spokes.clamp(3, 64));
        for ring in 1..=rings {
            // Rings spread out away from the impact: small shards near it.
            let r = reach * (ring as f32 / rings as f32).powf(1.6);
            for s in 0..spokes {
                let h = |k: u32| hash01((ring * 131 + s) as u32 * 7 + k, set.seed);
                let angle = (s as f32 + 0.5 * h(1)) / spokes as f32 * std::f32::consts::TAU + ring as f32 * 0.37;
                let r = r * (0.85 + 0.3 * h(2));
                let mut p = with_axis(impact, thin, mid);
                p = with_axis(p, a1, axis(impact, a1) + r * angle.cos());
                p = with_axis(p, a2, axis(impact, a2) + r * angle.sin());
                out.push(p);
            }
        }
        return Ok(out);
    }
    let points = crate::sops::scatter(mesh, &crate::sops::ScatterSettings { count, seed: set.seed, relax_iterations: 3, volume: true })?;
    Ok(points.vertices)
}

pub fn fracture(input: &Mesh, set: &FractureSettings) -> Result<Mesh, String> {
    fracture_all(input, &Mesh::default(), &Mesh::default(), set).map(|(geo, ..)| geo)
}

// Point attribute naming the piece each constraint end holds.
pub const CONSTRAINT_PIECE: &str = "constraint_piece";

// All three outputs: the pieces; glue constraints between neighbouring pieces
// (two-point lines from centre to centre, plus `constraints_in`); and proxy
// collision shapes (`proxy_in` cut the same way, else each piece's convex hull).
pub fn fracture_all(input: &Mesh, constraints_in: &Mesh, proxy_in: &Mesh, set: &FractureSettings) -> Result<(Mesh, Mesh, Mesh), String> {
    if input.indices.is_empty() {
        return Err("the input has no polygons to fracture".into());
    }
    // Wood: fracture a copy squashed along the grain, then stretch the pieces back.
    let stretch = if set.material == 2 { set.splinter.max(1.0) } else { 1.0 };
    let work = if stretch != 1.0 { scale_axis(input, set.grain_axis, 1.0 / stretch) } else { input.clone() };
    let (lo, hi) = work.bounds().ok_or("the input is empty")?;
    let pad = hi.sub(lo).length() * 0.05 + 1e-3;
    let (blo, bhi) = (lo.sub(Vec3::new(pad, pad, pad)), hi.add(Vec3::new(pad, pad, pad)));
    let seeds = seeds(&work, set, lo, hi)?;
    let mut out = Mesh::default();
    let (mut piece_points, mut piece_prims, mut inside) = (Vec::new(), Vec::new(), Vec::new());
    let mut piece = 0usize;
    let proxy_work = if proxy_in.indices.is_empty() { None } else if stretch != 1.0 { Some(scale_axis(proxy_in, set.grain_axis, 1.0 / stretch)) } else { Some(proxy_in.clone()) };
    // Per piece: its seed, its cell's neighbours, its centre, and the proxy shape.
    let mut seed_piece: Vec<Option<usize>> = vec![None; seeds.len()];
    let mut cell_neighbours: Vec<Vec<usize>> = Vec::new();
    let mut centres: Vec<Vec3> = Vec::new();
    let mut proxy = Mesh::default();
    for (i, s) in seeds.iter().enumerate() {
        let mut cell = Convex::cuboid(blo, bhi);
        // Nearest seeds first: they cut the most, so later clips have less to do.
        let mut others: Vec<(usize, &Vec3)> = seeds.iter().enumerate().filter(|(j, _)| *j != i).collect();
        others.sort_by(|a, b| a.1.sub(*s).length().partial_cmp(&b.1.sub(*s).length()).unwrap_or(std::cmp::Ordering::Equal));
        for (j, o) in others {
            let d = o.sub(*s);
            let len = d.length();
            if len < 1e-6 {
                continue;
            }
            let n = d.scale(1.0 / len);
            // Planes further than the cell's farthest corner can't cut it.
            let reach = cell.faces.iter().flatten().map(|p| p.sub(*s).dot(n)).fold(f32::MIN, f32::max);
            if reach < len * 0.5 {
                continue;
            }
            cell.clip(n, n.dot(s.add(d.scale(0.5))), j);
            if cell.faces.len() < 4 {
                break;
            }
        }
        if cell.faces.len() < 4 {
            continue;
        }
        // The cell only cuts: its faces take the input's attributes (material, Cd, groups stay with A's faces).
        let Ok(mut part) = boolean_with(&work, &cell.to_mesh(), &BooleanSettings { operation: 1, mode: 0, resolution: 64 }, true) else {
            continue;
        };
        if part.indices.is_empty() {
            continue;
        }
        if stretch != 1.0 {
            part = scale_axis(&part, set.grain_axis, stretch);
        }
        let mut offset = Vec3::zero();
        if set.separate > 0.0 {
            let (plo, phi) = part.bounds().unwrap();
            let (clo, chi) = input.bounds().unwrap();
            offset = plo.add(phi).scale(0.5).sub(clo.add(chi).scale(0.5)).scale(set.separate);
            part.vertices.iter_mut().for_each(|v| *v = v.add(offset));
        }
        // The proxy: the proxy input cut by the same cell, else the piece's convex hull.
        let mut shape = match &proxy_work {
            Some(pw) => match boolean_with(pw, &cell.to_mesh(), &BooleanSettings { operation: 1, mode: 0, resolution: 64 }, true) {
                Ok(mut p) => {
                    if stretch != 1.0 {
                        p = scale_axis(&p, set.grain_axis, stretch);
                    }
                    p.vertices.iter_mut().for_each(|v| *v = v.add(offset));
                    p.point_attributes.retain(|k, _| !k.starts_with("prim:group:"));
                    p
                }
                Err(_) => Mesh::default(),
            },
            None => convex_hull(&part.vertices),
        };
        shape.point_attributes.insert("piece".into(), vec![piece as f32; shape.vertices.len()]);
        shape.point_attributes.insert("prim:piece".into(), vec![piece as f32; shape.indices.len()]);
        proxy = super::node_merge(proxy, shape);
        seed_piece[i] = Some(piece);
        cell_neighbours.push(cell.neighbours());
        centres.push(part.vertices.iter().fold(Vec3::zero(), |a, v| a.add(*v)).scale(1.0 / part.vertices.len().max(1) as f32));
        let cut = part.point_attributes.remove("prim:group:b_faces").unwrap_or_default();
        part.point_attributes.remove("prim:group:a_faces");
        piece_points.extend(std::iter::repeat_n(piece as f32, part.vertices.len()));
        piece_prims.extend(std::iter::repeat_n(piece as f32, part.indices.len()));
        inside.extend((0..part.indices.len()).map(|t| cut.get(t).copied().unwrap_or(0.0)));
        // Merged with every attribute the piece carries (materials, Cd, uv, groups…).
        out = super::node_merge(out, part);
        piece += 1;
    }
    if piece == 0 {
        return Err("no pieces: is the input a closed mesh?".into());
    }
    out.point_attributes.insert("piece".into(), piece_points);
    out.point_attributes.insert("prim:piece".into(), piece_prims);
    out.point_attributes.insert("prim:group:outside".into(), inside.iter().map(|v| 1.0 - v).collect());
    out.point_attributes.insert("prim:group:inside".into(), inside);
    // Glue between pieces whose cells touch: one two-point line per pair.
    let mut constraints = Mesh::default();
    let (mut ends, mut u) = (Vec::new(), Vec::new());
    for (a, neighbours) in cell_neighbours.iter().enumerate() {
        for &seed in neighbours {
            if let Some(b) = seed_piece[seed].filter(|b| *b > a) {
                constraints.vertices.extend([centres[a], centres[b]]);
                ends.extend([a as f32, b as f32]);
                u.extend([0.0, 1.0]);
            }
        }
    }
    let n = constraints.vertices.len();
    constraints.point_attributes.insert(CONSTRAINT_PIECE.into(), ends);
    constraints.point_attributes.insert(crate::groom::CURVEU.into(), u);
    constraints.point_attributes.insert("strength".into(), vec![set.glue_strength; n]);
    constraints.point_attributes.insert("group:glue".into(), vec![1.0; n]);
    if !constraints_in.vertices.is_empty() {
        constraints = super::node_merge(constraints_in.clone(), constraints);
    }
    Ok((out, constraints, proxy))
}

// A convex hull of points as a closed mesh (empty for fewer than four).
fn convex_hull(points: &[Vec3]) -> Mesh {
    if points.len() < 4 {
        return Mesh::default();
    }
    let pts: Vec<rapier3d::math::Vector> = points.iter().map(|p| rapier3d::math::Vector::new(p.x, p.y, p.z)).collect();
    let hull = std::panic::catch_unwind(|| rapier3d::parry::transformation::convex_hull(&pts));
    match hull {
        Ok((vertices, indices)) => Mesh {
            vertices: vertices.iter().map(|v| Vec3::new(v.x, v.y, v.z)).collect(),
            indices: indices.iter().map(|t| t.map(|i| i as usize)).collect(),
            ..Mesh::default()
        },
        Err(_) => Mesh::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cuboid(size: Vec3) -> Mesh {
        let half = size.scale(0.5);
        Convex::cuboid(Vec3::zero().sub(half), half).to_mesh()
    }

    fn volume(mesh: &Mesh) -> f32 {
        mesh.indices.iter().map(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices[i]);
            a.dot(b.cross(c)) / 6.0
        }).sum()
    }

    fn piece_count(mesh: &Mesh) -> usize {
        mesh.point_attributes["piece"].iter().fold(0.0f32, |m, p| m.max(*p)) as usize + 1
    }

    #[test]
    fn cells_partition_the_input() {
        let cube = cuboid(Vec3::new(2.0, 2.0, 2.0));
        assert!((volume(&cube) - 8.0).abs() < 1e-4);
        for material in 0..3 {
            let out = fracture(&cube, &FractureSettings { pieces: 12, ..FractureSettings::defaults(material) }).unwrap();
            let pieces = piece_count(&out);
            assert!(pieces >= 6, "{}: {pieces} pieces", MATERIALS[material]);
            // The pieces fill the cube exactly: no gaps, no overlaps.
            assert!((volume(&out) - 8.0).abs() < 0.02, "{}: volume {}", MATERIALS[material], volume(&out));
            assert_eq!(out.point_attributes["prim:piece"].len(), out.indices.len());
            let inside = &out.point_attributes["prim:group:inside"];
            assert!(inside.iter().any(|v| *v > 0.5) && inside.iter().any(|v| *v < 0.5));
        }
        assert!(fracture(&Mesh::default(), &FractureSettings::defaults(0)).is_err());
    }

    #[test]
    fn constraints_link_neighbours_and_proxies_are_hulls() {
        let cube = cuboid(Vec3::new(2.0, 2.0, 2.0));
        let (geo, glue, proxy) = fracture_all(&cube, &Mesh::default(), &Mesh::default(), &FractureSettings { pieces: 8, ..FractureSettings::defaults(0) }).unwrap();
        let pieces = piece_count(&geo);
        let ends = &glue.point_attributes[CONSTRAINT_PIECE];
        assert!(glue.vertices.len() >= 2 * (pieces - 1), "a connected set of {pieces} pieces needs {} constraints, got {}", pieces - 1, glue.vertices.len() / 2);
        assert!(ends.chunks(2).all(|p| p[0] != p[1] && (p[0] as usize) < pieces && (p[1] as usize) < pieces));
        assert_eq!(crate::groom::curves(&glue).len(), glue.vertices.len() / 2, "each constraint is a two-point line");
        // One closed hull per piece, filling the cube (pieces are convex here).
        assert_eq!(piece_count(&proxy), pieces);
        assert!((volume(&proxy).abs() - 8.0).abs() < 0.05, "proxy volume {}", volume(&proxy));
        // A proxy input is cut the same way.
        let (_, _, cut) = fracture_all(&cube, &Mesh::default(), &cuboid(Vec3::new(1.0, 1.0, 1.0)), &FractureSettings { pieces: 8, ..FractureSettings::defaults(0) }).unwrap();
        assert!((volume(&cut).abs() - 1.0).abs() < 0.02);
    }

    #[test]
    fn pieces_keep_materials_and_colours() {
        let mut cube = cuboid(Vec3::new(2.0, 2.0, 2.0));
        let n = cube.indices.len();
        // Material on half the faces, Cd on every point, a primitive group.
        cube.point_attributes.insert("prim:material".into(), (0..n).map(|t| if t < n / 2 { 5.0 } else { 9.0 }).collect());
        cube.point_attributes.insert("Cd".into(), cube.vertices.iter().flat_map(|_| [0.2, 0.4, 0.6]).collect());
        cube.point_attributes.insert("prim:group:shell".into(), vec![1.0; n]);
        let out = fracture(&cube, &FractureSettings { pieces: 6, ..FractureSettings::defaults(0) }).unwrap();
        let mats = &out.point_attributes["prim:material"];
        assert_eq!(mats.len(), out.indices.len());
        assert!(mats.iter().all(|m| *m == 5.0 || *m == 9.0), "every face, inside ones too, has one of the materials");
        assert!(mats.contains(&5.0) && mats.contains(&9.0));
        let cd = &out.point_attributes["Cd"];
        assert!(cd.chunks(3).all(|c| (c[0] - 0.2).abs() < 1e-4 && (c[2] - 0.6).abs() < 1e-4), "Cd carried to every point");
        // Groups stay with the original faces only.
        let shell = &out.point_attributes["prim:group:shell"];
        let inside = &out.point_attributes["prim:group:inside"];
        for (s, i) in shell.iter().zip(inside) {
            assert_eq!(*s, 1.0 - *i);
        }
    }

    #[test]
    fn glass_shards_shrink_towards_the_impact_and_wood_splinters() {
        let pane = cuboid(Vec3::new(2.0, 2.0, 0.05));
        let set = FractureSettings { impact: [0.3, 0.2, 0.0], ..FractureSettings::defaults(1) };
        let out = fracture(&pane, &set).unwrap();
        let pieces = piece_count(&out);
        assert!(pieces > 20, "{pieces}");
        let impact = Vec3::from_arr(set.impact);
        let size_and_distance = |p: usize| {
            let pts: Vec<Vec3> = out.vertices.iter().zip(&out.point_attributes["piece"]).filter(|(_, k)| **k as usize == p).map(|(v, _)| *v).collect();
            let c = pts.iter().fold(Vec3::zero(), |s, v| s.add(*v)).scale(1.0 / pts.len() as f32);
            let r = pts.iter().map(|v| v.sub(c).length()).fold(0.0f32, f32::max);
            (r, c.sub(impact).length())
        };
        let (near, far): (Vec<(f32, f32)>, Vec<(f32, f32)>) = (0..pieces).map(size_and_distance).partition(|(_, d)| *d < 0.4);
        let mean = |v: &[(f32, f32)]| v.iter().map(|x| x.0).sum::<f32>() / v.len().max(1) as f32;
        assert!(!near.is_empty() && mean(&near) < mean(&far), "near {} far {}", mean(&near), mean(&far));
        // Wood pieces are longer along the grain (y) than across.
        let log = cuboid(Vec3::new(1.0, 3.0, 1.0));
        let out = fracture(&log, &FractureSettings { pieces: 16, ..FractureSettings::defaults(2) }).unwrap();
        let (mut along, mut across) = (0.0, 0.0);
        for p in 0..piece_count(&out) {
            let pts: Vec<Vec3> = out.vertices.iter().zip(&out.point_attributes["piece"]).filter(|(_, k)| **k as usize == p).map(|(v, _)| *v).collect();
            let (lo, hi) = pts.iter().fold((pts[0], pts[0]), |(lo, hi), v| {
                (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
            });
            along += hi.y - lo.y;
            across += (hi.x - lo.x).max(hi.z - lo.z);
        }
        assert!(along > across * 1.5, "along {along} across {across}");
    }
}
