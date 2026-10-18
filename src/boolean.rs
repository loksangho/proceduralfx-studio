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
                front.push(Polygon { verts: f, plane: poly.plane, source: poly.source });
            }
            if b.len() >= 3 {
                back.push(Polygon { verts: b, plane: poly.plane, source: poly.source });
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
        .filter_map(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices[i]);
            Plane::from_points(a, b, c).map(|plane| Polygon { verts: vec![a, b, c], plane, source })
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
fn to_mesh(polys: &[Polygon]) -> Mesh {
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
            }
        }
    }
    mesh.point_attributes.insert("prim:group:a_faces".into(), from_a.iter().map(|a| *a as u8 as f32).collect());
    mesh.point_attributes.insert("prim:group:b_faces".into(), from_a.iter().map(|a| !*a as u8 as f32).collect());
    mesh
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
    let (a, b, op) = (a.clone(), b.clone(), set.operation);
    let polys = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || csg(&a, &b, op))
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "the boolean failed".to_owned())?;
    Ok(to_mesh(&polys))
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
