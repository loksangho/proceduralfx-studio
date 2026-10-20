// Boolean: union, intersection and subtraction of closed meshes.
//
// Exact mode is polygon CSG on BSP trees (the csg.js algorithm by Evan Wallace):
// each mesh's polygons are clipped against the other's tree, so faces and
// sharp edges are kept exactly. Volume mode converts both meshes to signed
// distance fields, combines them per voxel and remeshes: softer edges, but it
// copes with dense or imperfect input.
//
// Triangles are wound counter-clockwise seen from outside: (b - a) x (c - a) points out.

use super::{Mesh, Vec3};
use std::collections::HashMap;

pub const OPERATIONS: [&str; 3] = ["Union", "Intersect", "Subtract (A − B)"];
pub const MODES: [&str; 2] = ["Exact (sharp)", "Volume (smooth, robust)"];

#[derive(Clone, Debug, PartialEq)]
pub struct BooleanSettings {
    pub operation: usize,
    pub mode: usize,
    // Volume mode: voxels along the combined bounding box's longest side.
    pub resolution: usize,
}

const EPS: f32 = 1e-5;

#[derive(Clone, Copy, Debug)]
struct Plane {
    n: Vec3,
    w: f32,
}

impl Plane {
    fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Option<Plane> {
        let n = b.sub(a).cross(c.sub(a));
        let len = n.length();
        (len > 1e-12).then(|| {
            let n = n.scale(1.0 / len);
            Plane { n, w: n.dot(a) }
        })
    }

    fn flip(&mut self) {
        self.n = self.n.scale(-1.0);
        self.w = -self.w;
    }
}

#[derive(Clone, Debug)]
struct Polygon {
    verts: Vec<Vec3>,
    plane: Plane,
    // 0 = from input A, 1 = from input B.
    source: u8,
    // The source triangle it was cut from, for carrying attributes over.
    tri: usize,
}

impl Polygon {
    fn flip(&mut self) {
        self.verts.reverse();
        self.plane.flip();
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

// Splits `poly` by `plane` into the four lists, as csg.js does.
fn split_polygon(
    plane: &Plane,
    poly: &Polygon,
    coplanar_front: &mut Vec<Polygon>,
    coplanar_back: &mut Vec<Polygon>,
    front: &mut Vec<Polygon>,
    back: &mut Vec<Polygon>,
) {
    let mut kind = 0u8;
    let types: Vec<u8> = poly
        .verts
        .iter()
        .map(|v| {
            let t = plane.n.dot(*v) - plane.w;
            let k = if t < -EPS { BACK } else if t > EPS { FRONT } else { COPLANAR };
            kind |= k;
            k
        })
        .collect();
    match kind {
        COPLANAR => {
            if plane.n.dot(poly.plane.n) > 0.0 {
                coplanar_front.push(poly.clone());
            } else {
                coplanar_back.push(poly.clone());
            }
        }
        FRONT => front.push(poly.clone()),
        BACK => back.push(poly.clone()),
        _ => {
            let (mut f, mut b) = (Vec::new(), Vec::new());
            let n = poly.verts.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (ti, tj) = (types[i], types[j]);
                let (vi, vj) = (poly.verts[i], poly.verts[j]);
                if ti != BACK {
                    f.push(vi);
                }
                if ti != FRONT {
                    b.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let t = (plane.w - plane.n.dot(vi)) / plane.n.dot(vj.sub(vi));
                    let v = vi.add(vj.sub(vi).scale(t));
                    f.push(v);
                    b.push(v);
                }
            }
            if f.len() >= 3 {
                front.push(Polygon { verts: f, plane: poly.plane, source: poly.source, tri: poly.tri });
            }
            if b.len() >= 3 {
                back.push(Polygon { verts: b, plane: poly.plane, source: poly.source, tri: poly.tri });
            }
        }
    }
}

// BSP nodes live in an arena so deep trees don't need deep recursion.
#[derive(Default)]
struct Bsp {
    planes: Vec<Option<Plane>>,
    fronts: Vec<Option<usize>>,
    backs: Vec<Option<usize>>,
    polys: Vec<Vec<Polygon>>,
}

impl Bsp {
    fn new(polygons: Vec<Polygon>) -> Bsp {
        let mut bsp = Bsp::default();
        bsp.add_node();
        bsp.build(0, polygons);
        bsp
    }

    fn add_node(&mut self) -> usize {
        self.planes.push(None);
        self.fronts.push(None);
        self.backs.push(None);
        self.polys.push(Vec::new());
        self.planes.len() - 1
    }

    fn build(&mut self, root: usize, polygons: Vec<Polygon>) {
        let mut stack = vec![(root, polygons)];
        while let Some((node, polygons)) = stack.pop() {
            if polygons.is_empty() {
                continue;
            }
            let plane = *self.planes[node].get_or_insert(polygons[0].plane);
            let (mut front, mut back) = (Vec::new(), Vec::new());
            let mut coplanar_front = Vec::new();
            let mut coplanar_back = Vec::new();
            for p in &polygons {
                split_polygon(&plane, p, &mut coplanar_front, &mut coplanar_back, &mut front, &mut back);
            }
            self.polys[node].extend(coplanar_front);
            self.polys[node].extend(coplanar_back);
            if !front.is_empty() {
                let child = match self.fronts[node] {
                    Some(c) => c,
                    None => {
                        let c = self.add_node();
                        self.fronts[node] = Some(c);
                        c
                    }
                };
                stack.push((child, front));
            }
            if !back.is_empty() {
                let child = match self.backs[node] {
                    Some(c) => c,
                    None => {
                        let c = self.add_node();
                        self.backs[node] = Some(c);
                        c
                    }
                };
                stack.push((child, back));
            }
        }
    }

    // Turns solid into empty space and back.
    fn invert(&mut self) {
        for node in 0..self.planes.len() {
            for p in &mut self.polys[node] {
                p.flip();
            }
            if let Some(plane) = &mut self.planes[node] {
                plane.flip();
            }
            std::mem::swap(&mut self.fronts[node], &mut self.backs[node]);
        }
    }

    // The parts of `polygons` outside this tree's solid.
    fn clip_polygons(&self, polygons: Vec<Polygon>) -> Vec<Polygon> {
        let mut out = Vec::new();
        let mut stack = vec![(0usize, polygons)];
        while let Some((node, polygons)) = stack.pop() {
            let Some(plane) = self.planes[node] else {
                out.extend(polygons);
                continue;
            };
            let (mut front, mut back) = (Vec::new(), Vec::new());
            let mut coplanar_front = Vec::new();
            let mut coplanar_back = Vec::new();
            for p in &polygons {
                split_polygon(&plane, p, &mut coplanar_front, &mut coplanar_back, &mut front, &mut back);
            }
            front.extend(coplanar_front);
            back.extend(coplanar_back);
            match self.fronts[node] {
                Some(c) => stack.push((c, front)),
                None => out.extend(front),
            }
            // Behind a leaf is inside the solid: dropped.
            if let Some(c) = self.backs[node] {
                stack.push((c, back));
            }
        }
        out
    }

    fn clip_to(&mut self, other: &Bsp) {
        for node in 0..self.polys.len() {
            let polys = std::mem::take(&mut self.polys[node]);
            self.polys[node] = other.clip_polygons(polys);
        }
    }

    fn all_polygons(&self) -> Vec<Polygon> {
        self.polys.iter().flatten().cloned().collect()
    }
}

fn polygons(mesh: &Mesh, source: u8) -> Vec<Polygon> {
    mesh.indices
        .iter()
        .enumerate()
        .filter_map(|(tri, t)| {
            let [a, b, c] = t.map(|i| mesh.vertices[i]);
            Plane::from_points(a, b, c).map(|plane| Polygon { verts: vec![a, b, c], plane, source, tri })
        })
        .collect()
}

fn csg(a: &Mesh, b: &Mesh, operation: usize) -> Vec<Polygon> {
    let mut a = Bsp::new(polygons(a, 0));
    let mut b = Bsp::new(polygons(b, 1));
    match operation {
        // Union
        0 => {
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            let rest = b.all_polygons();
            a.build(0, rest);
        }
        // Intersect
        1 => {
            a.invert();
            b.clip_to(&a);
            b.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            let rest = b.all_polygons();
            a.build(0, rest);
            a.invert();
        }
        // Subtract
        _ => {
            a.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            let rest = b.all_polygons();
            a.build(0, rest);
            a.invert();
        }
    }
    a.all_polygons()
}

// Triangulates (fans) the polygons and welds coincident points.
// Triangulates the result, then carries the inputs' attributes over (see transfer).
fn to_mesh(polys: &[Polygon], a: &Mesh, b: &Mesh, b_takes_a: bool) -> Mesh {
    let (mut mesh, origins) = triangulate(polys);
    transfer(&mut mesh, &origins, a, b, b_takes_a);
    mesh
}

// Weights of p in triangle (a, b, c), from its projection onto the triangle's plane.
fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 3] {
    let (v0, v1, v2) = (b.sub(a), c.sub(a), p.sub(a));
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1e-20 {
        return [1.0, 0.0, 0.0];
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    [1.0 - v - w, v, w]
}

// Copies attributes from the source triangles onto the result: primitive attributes as
// they were, point and vertex attributes (Cd, uv…) interpolated where each point lies on
// its source triangle. With `b_takes_a` (Material Fracture, where B is a cutting cell),
// faces cut by B take A's primitive attributes from a face of A in the same result (so
// they keep A's material) and their points the nearest A point's attributes.
fn transfer(out: &mut Mesh, origins: &[(u8, usize)], a: &Mesh, b: &Mesh, b_takes_a: bool) {
    let class = |k: &str| if k.starts_with("prim:") { 1 } else if k.starts_with("vertex:") { 2 } else if k.starts_with("detail:") { 3 } else { 0 };
    let (np, nt) = (out.vertices.len(), out.indices.len());
    let src = |o: u8| if o == 0 { a } else { b };
    // A face of A to stand in for B faces (fracture).
    let stand_in = origins.iter().find(|(o, _)| *o == 0).map(|(_, t)| *t);
    // Nearest A point, for B points in fracture (a plain search: cut faces are few).
    let nearest_a = |p: Vec3| -> Option<usize> {
        a.vertices.iter().enumerate().map(|(i, q)| (q.sub(p).length(), i)).min_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal)).map(|(_, i)| i)
    };
    let mut keys: Vec<String> = a.point_attributes.keys().chain(b.point_attributes.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    // The first result triangle (and corner) using each point.
    let mut first_use = vec![None; np];
    for (t, tri) in out.indices.iter().enumerate() {
        for (k, &v) in tri.iter().enumerate() {
            first_use[v].get_or_insert((t, k));
        }
    }
    let mut nearest_cache: Vec<Option<Option<usize>>> = vec![None; np];
    for key in keys {
        if key == "prim:group:a_faces" || key == "prim:group:b_faces" {
            continue;
        }
        let width_in = |m: &Mesh| -> Option<usize> {
            let v = m.point_attributes.get(&key)?;
            let count = match class(&key) {
                0 => m.vertices.len(),
                1 => m.indices.len(),
                2 => m.indices.len() * 3,
                _ => return Some(v.len()),
            };
            (count > 0 && v.len() % count == 0 && !v.is_empty()).then(|| v.len() / count)
        };
        let (wa, wb) = (width_in(a), width_in(b));
        let Some(width) = wa.or(wb) else { continue };
        // Where a result triangle reads from: (mesh, triangle), or None for zeros.
        let reads = |t: usize| -> Option<(&Mesh, usize)> {
            let (o, tri) = origins[t];
            let own = if o == 0 { wa } else { wb };
            if own == Some(width) {
                Some((src(o), tri))
            } else if o == 1 && b_takes_a && wa == Some(width) && !key.starts_with("prim:group:") {
                stand_in.map(|s| (a, s))
            } else {
                None
            }
        };
        let values: Vec<f32> = match class(&key) {
            3 => a.point_attributes.get(&key).or_else(|| b.point_attributes.get(&key)).cloned().unwrap_or_default(),
            1 => (0..nt)
                .flat_map(|t| match reads(t) {
                    Some((m, tri)) => m.point_attributes[&key][tri * width..tri * width + width].to_vec(),
                    None => vec![0.0; width],
                })
                .collect(),
            2 => (0..nt)
                .flat_map(|t| {
                    let corners: Vec<f32> = match reads(t).filter(|(m, _)| std::ptr::eq(*m, src(origins[t].0))) {
                        Some((m, tri)) => {
                            let [pa, pb, pc] = m.indices[tri].map(|i| m.vertices[i]);
                            let vals = &m.point_attributes[&key];
                            out.indices[t]
                                .iter()
                                .flat_map(|&v| {
                                    let w = barycentric(out.vertices[v], pa, pb, pc);
                                    (0..width).map(move |c| (0..3).map(|k| vals[(tri * 3 + k) * width + c] * w[k]).sum::<f32>()).collect::<Vec<_>>()
                                })
                                .collect()
                        }
                        None => vec![0.0; width * 3],
                    };
                    corners
                })
                .collect(),
            _ => (0..np)
                .flat_map(|v| {
                    let Some((t, _)) = first_use[v] else { return vec![0.0; width] };
                    let (o, tri) = origins[t];
                    let own = if o == 0 { wa } else { wb };
                    if own == Some(width) {
                        let m = src(o);
                        let [ia, ib, ic] = m.indices[tri];
                        let w = barycentric(out.vertices[v], m.vertices[ia], m.vertices[ib], m.vertices[ic]);
                        let vals = &m.point_attributes[&key];
                        (0..width).map(|c| vals[ia * width + c] * w[0] + vals[ib * width + c] * w[1] + vals[ic * width + c] * w[2]).collect()
                    } else if b_takes_a && wa == Some(width) && !key.starts_with("group:") {
                        let near = *nearest_cache[v].get_or_insert_with(|| nearest_a(out.vertices[v]));
                        match near {
                            Some(i) => a.point_attributes[&key][i * width..i * width + width].to_vec(),
                            None => vec![0.0; width],
                        }
                    } else {
                        vec![0.0; width]
                    }
                })
                .collect(),
        };
        out.point_attributes.insert(key, values);
    }
}

fn triangulate(polys: &[Polygon]) -> (Mesh, Vec<(u8, usize)>) {
    let mut origins = Vec::new();
    let mut mesh = Mesh::default();
    let mut index: HashMap<[i64; 3], usize> = HashMap::new();
    let mut from_a = Vec::new();
    let quantum = 1e-5f32;
    let mut point = |mesh: &mut Mesh, v: Vec3| -> usize {
        let key = [v.x, v.y, v.z].map(|c| (c / quantum).round() as i64);
        *index.entry(key).or_insert_with(|| {
            mesh.vertices.push(v);
            mesh.vertices.len() - 1
        })
    };
    for poly in polys {
        let ids: Vec<usize> = poly.verts.iter().map(|v| point(&mut mesh, *v)).collect();
        for k in 1..ids.len() - 1 {
            let tri = [ids[0], ids[k], ids[k + 1]];
            if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                mesh.indices.push(tri);
                from_a.push(poly.source == 0);
                origins.push((poly.source, poly.tri));
            }
        }
    }
    mesh.point_attributes.insert("prim:group:a_faces".into(), from_a.iter().map(|a| *a as u8 as f32).collect());
    mesh.point_attributes.insert("prim:group:b_faces".into(), from_a.iter().map(|a| !*a as u8 as f32).collect());
    (mesh, origins)
}

fn volume_boolean(a: &Mesh, b: &Mesh, set: &BooleanSettings) -> Result<Mesh, String> {
    use crate::volume::{fill_distances, from_polygons, to_polygons, Class, Volume};
    let bounds = |m: &Mesh| m.bounds().ok_or("an input is empty");
    let ((alo, ahi), (blo, bhi)) = (bounds(a)?, bounds(b)?);
    let lo = Vec3::new(alo.x.min(blo.x), alo.y.min(blo.y), alo.z.min(blo.z));
    let hi = Vec3::new(ahi.x.max(bhi.x), ahi.y.max(bhi.y), ahi.z.max(bhi.z));
    let extent = hi.sub(lo);
    let voxel = (extent.x.max(extent.y).max(extent.z) / set.resolution.clamp(16, 400) as f32).max(1e-4);
    let field = |m: &Mesh| -> Result<Volume, String> {
        let mut v = from_polygons(m, voxel, 3.0)?;
        fill_distances(&mut v);
        Ok(v)
    };
    let (fa, fb) = (field(a)?, field(b)?);
    let pad = 2.0 * voxel;
    let origin = lo.sub(Vec3::new(pad, pad, pad));
    let dims = [extent.x, extent.y, extent.z].map(|e| ((e + 2.0 * pad) / voxel).ceil() as usize + 1);
    let mut values = Vec::with_capacity(dims[0] * dims[1] * dims[2]);
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let p = origin.add(Vec3::new(x as f32, y as f32, z as f32).scale(voxel));
                let (da, db) = (fa.sample(p), fb.sample(p));
                values.push(match set.operation {
                    0 => da.min(db),
                    1 => da.max(db),
                    _ => da.max(-db),
                });
            }
        }
    }
    let combined = Volume { class: Class::LevelSet, voxel, origin, dims, values, background: 3.0 * voxel };
    let mesh = to_polygons(&combined, 0.0);
    if mesh.indices.is_empty() {
        return Err("the result is empty".into());
    }
    Ok(mesh)
}

pub fn boolean(a: &Mesh, b: &Mesh, set: &BooleanSettings) -> Result<Mesh, String> {
    boolean_with(a, b, set, false)
}

// `b_takes_a`: B is only a cutter (Material Fracture's cells), so its faces take A's
// attributes instead of B's.
pub fn boolean_with(a: &Mesh, b: &Mesh, set: &BooleanSettings, b_takes_a: bool) -> Result<Mesh, String> {
    // Cut faces come out as triangles: polygons don't survive being split.
    let (a, b) = (crate::poly::without(a), crate::poly::without(b));
    let (a, b) = (a.as_ref(), b.as_ref());
    if a.indices.is_empty() {
        return Err("input A (left) has no polygons".into());
    }
    if b.indices.is_empty() {
        return Err("input B (right) has no polygons".into());
    }
    if set.mode == 1 {
        return volume_boolean(a, b, set);
    }
    // Clipping is iterative, but run on a big stack anyway: very fragmented
    // inputs make long polygon lists.
    let (ca, cb, op) = (a.clone(), b.clone(), set.operation);
    let polys = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || csg(&ca, &cb, op))
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "the boolean failed".to_owned())?;
    Ok(to_mesh(&polys, a, b, b_takes_a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(centre: Vec3, size: f32) -> Mesh {
        let mut mesh = Mesh::default();
        for k in 0..8 {
            let o = Vec3::new((k & 1) as f32 - 0.5, ((k >> 1) & 1) as f32 - 0.5, ((k >> 2) & 1) as f32 - 0.5);
            mesh.vertices.push(centre.add(o.scale(size)));
        }
        // Outward-facing: (b - a) x (c - a) points out of the cube.
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            mesh.indices.push([f[0], f[1], f[2]]);
            mesh.indices.push([f[0], f[2], f[3]]);
        }
        mesh
    }

    // Signed volume by the divergence theorem (positive for outward winding).
    fn volume(mesh: &Mesh) -> f32 {
        mesh.indices.iter().map(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices[i]);
            a.dot(b.cross(c)) / 6.0
        }).sum()
    }

    // Every edge used by exactly two triangles, in opposite directions.
    fn is_closed(mesh: &Mesh) -> bool {
        let mut edges: HashMap<(usize, usize), i32> = HashMap::new();
        for t in &mesh.indices {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
            }
        }
        edges.values().all(|v| *v == 0)
    }

    #[test]
    fn exact_booleans_of_two_cubes() {
        let a = cube(Vec3::new(0.0, 0.0, 0.0), 2.0);
        let b = cube(Vec3::new(1.0, 1.0, 1.0), 2.0);
        assert!((volume(&a) - 8.0).abs() < 1e-4);
        // Overlap is a 1 x 1 x 1 cube.
        let expected = [8.0 + 8.0 - 1.0, 1.0, 8.0 - 1.0];
        for op in 0..3 {
            let out = boolean(&a, &b, &BooleanSettings { operation: op, mode: 0, resolution: 64 }).unwrap();
            let v = volume(&out);
            assert!((v - expected[op]).abs() < 1e-3, "{}: volume {v}", OPERATIONS[op]);
            assert!(is_closed(&out), "{} leaves holes", OPERATIONS[op]);
            let a_faces = &out.point_attributes["prim:group:a_faces"];
            assert_eq!(a_faces.len(), out.indices.len());
            assert!(a_faces.iter().any(|f| *f > 0.5) && a_faces.iter().any(|f| *f < 0.5));
        }
        // Subtracting something that doesn't touch leaves A as it was.
        let far = cube(Vec3::new(10.0, 0.0, 0.0), 1.0);
        let out = boolean(&a, &far, &BooleanSettings { operation: 2, mode: 0, resolution: 64 }).unwrap();
        assert!((volume(&out) - 8.0).abs() < 1e-4);
        assert!(boolean(&Mesh::default(), &a, &BooleanSettings { operation: 0, mode: 0, resolution: 64 }).is_err());
    }

    #[test]
    fn volume_booleans_approximate_the_same_shapes() {
        let a = cube(Vec3::new(0.0, 0.0, 0.0), 2.0);
        let b = cube(Vec3::new(1.0, 1.0, 1.0), 2.0);
        let expected = [15.0, 1.0, 7.0];
        for op in 0..3 {
            let out = boolean(&a, &b, &BooleanSettings { operation: op, mode: 1, resolution: 96 }).unwrap();
            let v = volume(&out).abs();
            assert!((v - expected[op]).abs() / expected[op] < 0.12, "{}: volume {v}", OPERATIONS[op]);
        }
    }
}
