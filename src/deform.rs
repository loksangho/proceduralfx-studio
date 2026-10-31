// Point Deform and Transform Pieces: moving geometry with points.
//
// Point Deform (input 1: geometry, 2: rest points, 3: the same points moved)
// captures each point of the geometry to its nearest rest points. Every rest
// point gets a local rotation, fitted to how its own neighbours moved (shape
// matching), so the geometry bends and turns with the points instead of only
// shifting: p' = sum of w * ((p - rest) * R + moved).
//
// Transform Pieces (input 1: geometry with `piece` or `name`, 2: one template
// point per piece, 3: optional rest template points) moves each piece rigidly
// with its template point: P, and `transform` (3x3) or `orient` (quaternion),
// with `pscale`. Template points match pieces by their own `piece`/`name`
// value, else by order. Without rest points, pieces turn about their centre.

use super::{Mesh, Vec3};
use crate::kinefx::{transpose, vm, IDENTITY, M3};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct PointDeformSettings {
    // Rest points each geometry point follows.
    pub influences: usize,
    // Neighbours used to fit each rest point's rotation.
    pub fit_neighbours: usize,
    // Follow rotation (off: only the displacement).
    pub rotate: bool,
}

impl PointDeformSettings {
    pub fn defaults() -> Self {
        PointDeformSettings { influences: 4, fit_neighbours: 6, rotate: true }
    }
}

// Points on a grid, for nearest-neighbour queries.
pub(crate) struct Grid<'a> {
    points: &'a [Vec3],
    cell: f32,
    cells: HashMap<[i32; 3], Vec<usize>>,
}

impl<'a> Grid<'a> {
    pub(crate) fn new(points: &'a [Vec3]) -> Grid<'a> {
        let (mut lo, mut hi) = (Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN));
        for p in points {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        let e = hi.sub(lo);
        let volume = (e.x.max(1e-3) * e.y.max(1e-3) * e.z.max(1e-3)).max(1e-9);
        let cell = (volume / points.len().max(1) as f32).cbrt().max(1e-4) * 1.5;
        let mut cells: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
        for (i, p) in points.iter().enumerate() {
            cells.entry(Self::key(*p, cell)).or_default().push(i);
        }
        Grid { points, cell, cells }
    }

    fn key(p: Vec3, cell: f32) -> [i32; 3] {
        [(p.x / cell).floor() as i32, (p.y / cell).floor() as i32, (p.z / cell).floor() as i32]
    }

    // The `k` nearest points to `p` (fewer if there aren't that many), nearest first.
    pub(crate) fn nearest(&self, p: Vec3, k: usize) -> Vec<(f32, usize)> {
        let k = k.min(self.points.len());
        let c = Self::key(p, self.cell);
        let mut found: Vec<(f32, usize)> = Vec::new();
        for ring in 0..64i32 {
            // Visit the shell of cells at this ring.
            for dz in -ring..=ring {
                for dy in -ring..=ring {
                    for dx in -ring..=ring {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != ring {
                            continue;
                        }
                        for &i in self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                            found.push((self.points[i].sub(p).length(), i));
                        }
                    }
                }
            }
            // Points beyond this ring are at least `ring` cells away.
            if found.len() >= k {
                found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                if found[k - 1].0 <= ring as f32 * self.cell {
                    found.truncate(k);
                    return found;
                }
            }
            if found.len() == self.points.len() {
                break;
            }
        }
        // Sparse points far apart (64 rings of small cells didn't reach enough): check
        // them all.
        if found.len() < k {
            found = self.points.iter().enumerate().map(|(i, q)| (q.sub(p).length(), i)).collect();
        }
        found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        found.truncate(k);
        found
    }
}

// The rotation closest to `a` (polar decomposition by Newton iteration); identity when degenerate.
fn polar(a: &M3) -> M3 {
    let flat = |m: &M3| [m[0].x, m[0].y, m[0].z, m[1].x, m[1].y, m[1].z, m[2].x, m[2].y, m[2].z];
    let rows = |v: &[f32]| [Vec3::new(v[0], v[1], v[2]), Vec3::new(v[3], v[4], v[5]), Vec3::new(v[6], v[7], v[8])];
    let mut r = *a;
    for _ in 0..30 {
        let Some(inv) = crate::vexmath::invert(&flat(&r), 3) else { return IDENTITY };
        let inv_t = transpose(&rows(&inv));
        let next: M3 = std::array::from_fn(|i| r[i].add(inv_t[i]).scale(0.5));
        let change: f32 = (0..3).map(|i| next[i].sub(r[i]).length()).sum();
        r = next;
        if change < 1e-6 {
            break;
        }
    }
    // A reflection means the fit was degenerate (flat or collinear neighbours).
    if r[0].cross(r[1]).dot(r[2]) < 0.0 || !r[0].x.is_finite() {
        return IDENTITY;
    }
    r
}

pub fn point_deform(mesh: &Mesh, rest: &Mesh, moved: &Mesh, set: &PointDeformSettings) -> Result<Mesh, String> {
    if rest.vertices.is_empty() {
        return Err("connect the rest points to input 2".into());
    }
    if rest.vertices.len() != moved.vertices.len() {
        return Err(format!("the rest ({}) and deformed ({}) points don't match: they must be the same points", rest.vertices.len(), moved.vertices.len()));
    }
    let grid = Grid::new(&rest.vertices);
    // Each rest point's rotation from its neighbourhood (rest -> moved, centred).
    let rotations: Vec<M3> = if set.rotate {
        (0..rest.vertices.len())
            .map(|j| {
                let near = grid.nearest(rest.vertices[j], set.fit_neighbours.max(3) + 1);
                let (rc, mc) = (rest.vertices[j], moved.vertices[j]);
                let mut a: M3 = [Vec3::zero(); 3];
                for (_, i) in near {
                    let (p, q) = (rest.vertices[i].sub(rc), moved.vertices[i].sub(mc));
                    // Rows of p^T q (row vectors: p * R ≈ q).
                    a[0] = a[0].add(q.scale(p.x));
                    a[1] = a[1].add(q.scale(p.y));
                    a[2] = a[2].add(q.scale(p.z));
                }
                polar(&a)
            })
            .collect()
    } else {
        vec![IDENTITY; rest.vertices.len()]
    };
    let k = set.influences.clamp(1, 32);
    let mut out = mesh.clone();
    for v in out.vertices.iter_mut() {
        let p = *v;
        let near = grid.nearest(p, k);
        let (mut sum, mut total) = (Vec3::zero(), 0.0f32);
        let d0 = near.first().map_or(1.0, |n| n.0).max(1e-6);
        for (d, j) in near {
            // Inverse-square weights, relative to the nearest so far-off points fade out.
            let w = 1.0 / (d / d0 + 0.05).powi(2);
            sum = sum.add(vm(p.sub(rest.vertices[j]), &rotations[j]).add(moved.vertices[j]).scale(w));
            total += w;
        }
        if total > 0.0 {
            *v = sum.scale(1.0 / total);
        }
    }
    out.point_attributes.remove("N");
    Ok(out)
}

// --- Transform Pieces ---------------------------------------------------------------

fn template_frames(points: &Mesh) -> Vec<(Vec3, M3)> {
    let n = points.vertices.len();
    let transform = points.point_attributes.get("transform").filter(|t| t.len() == n * 9);
    let orient = points.point_attributes.get("orient").filter(|o| o.len() == n * 4);
    let pscale = points.point_attributes.get("pscale").filter(|s| s.len() == n);
    (0..n)
        .map(|i| {
            let mut m = match (transform, orient) {
                (Some(t), _) => {
                    let c = &t[i * 9..i * 9 + 9];
                    [Vec3::new(c[0], c[1], c[2]), Vec3::new(c[3], c[4], c[5]), Vec3::new(c[6], c[7], c[8])]
                }
                (None, Some(o)) => {
                    let q = crate::vexmath::q_to_m3(&[o[i * 4], o[i * 4 + 1], o[i * 4 + 2], o[i * 4 + 3]]);
                    [Vec3::new(q[0], q[1], q[2]), Vec3::new(q[3], q[4], q[5]), Vec3::new(q[6], q[7], q[8])]
                }
                _ => IDENTITY,
            };
            if let Some(s) = pscale {
                m = m.map(|r| r.scale(s[i]));
            }
            (points.vertices[i], m)
        })
        .collect()
}

// Each point's piece key: `piece` or `name` on points, else on its primitives.
fn piece_of_points(mesh: &Mesh) -> Result<Vec<i64>, String> {
    let (np, nt) = (mesh.vertices.len(), mesh.indices.len());
    for name in ["piece", "name"] {
        if let Some(v) = mesh.point_attributes.get(name).filter(|v| v.len() == np) {
            return Ok(v.iter().map(|x| (x * 1000.0).round() as i64).collect());
        }
        if let Some(v) = mesh.point_attributes.get(&format!("prim:{name}")).filter(|v| v.len() == nt) {
            let mut keys = vec![i64::MIN; np];
            for (t, tri) in mesh.indices.iter().enumerate() {
                for &p in tri {
                    keys[p] = (v[t] * 1000.0).round() as i64;
                }
            }
            return Ok(keys);
        }
    }
    Err("the geometry needs a `piece` (or `name`) attribute, e.g. from Material Fracture".into())
}

fn template_keys(points: &Mesh) -> Vec<i64> {
    let n = points.vertices.len();
    for name in ["piece", "name"] {
        if let Some(v) = points.point_attributes.get(name).filter(|v| v.len() == n) {
            return v.iter().map(|x| (x * 1000.0).round() as i64).collect();
        }
    }
    (0..n as i64).map(|i| i * 1000).collect()
}

pub fn transform_pieces(mesh: &Mesh, template: &Mesh, rest: Option<&Mesh>) -> Result<Mesh, String> {
    if template.vertices.is_empty() {
        return Err("connect the template points (one per piece) to input 2".into());
    }
    let keys = piece_of_points(mesh)?;
    let moved: HashMap<i64, (Vec3, M3)> = template_keys(template).into_iter().zip(template_frames(template)).collect();
    let rest: Option<HashMap<i64, (Vec3, M3)>> = match rest.filter(|r| !r.vertices.is_empty()) {
        Some(r) if r.vertices.len() != template.vertices.len() => return Err("the rest points must be the same points as the template".into()),
        Some(r) => Some(template_keys(r).into_iter().zip(template_frames(r)).collect()),
        None => None,
    };
    // Without rest points, each piece turns about its own centre.
    let mut centres: HashMap<i64, (Vec3, usize)> = HashMap::new();
    for (p, k) in mesh.vertices.iter().zip(&keys) {
        let e = centres.entry(*k).or_insert((Vec3::zero(), 0));
        e.0 = e.0.add(*p);
        e.1 += 1;
    }
    let mut out = mesh.clone();
    for (v, k) in out.vertices.iter_mut().zip(&keys) {
        let Some((target, m)) = moved.get(k) else { continue };
        let (pivot, from) = match rest.as_ref().and_then(|r| r.get(k)) {
            Some((p, rm)) => (*p, *rm),
            None => {
                let (sum, n) = centres[k];
                (sum.scale(1.0 / n as f32), IDENTITY)
            }
        };
        // Undo the rest orientation (and scale), then apply the template's.
        let local = vm(v.sub(pivot), &inverse(&from));
        *v = vm(local, m).add(*target);
    }
    out.point_attributes.remove("N");
    Ok(out)
}

fn inverse(m: &M3) -> M3 {
    let flat = [m[0].x, m[0].y, m[0].z, m[1].x, m[1].y, m[1].z, m[2].x, m[2].y, m[2].z];
    match crate::vexmath::invert(&flat, 3) {
        Some(i) => [Vec3::new(i[0], i[1], i[2]), Vec3::new(i[3], i[4], i[5]), Vec3::new(i[6], i[7], i[8])],
        None => IDENTITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize) -> Mesh {
        Mesh { vertices: (0..n).map(|i| Vec3::new(0.0, i as f32 * 0.25, 0.0)).collect(), ..Mesh::default() }
    }

    #[test]
    fn point_deform_follows_moves_and_rotations() {
        // A 3D cloud of rest points; moved = rotated 90° about y and shifted.
        let mut rest = Mesh::default();
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    rest.vertices.push(Vec3::new(x as f32, y as f32, z as f32));
                }
            }
        }
        let shift = Vec3::new(10.0, 0.0, 0.0);
        let mut moved = rest.clone();
        moved.vertices.iter_mut().for_each(|p| *p = p.rotate_deg([0.0, 90.0, 0.0]).add(shift));
        let mesh = Mesh { vertices: vec![Vec3::new(1.5, 1.5, 1.5), Vec3::new(0.2, 2.7, 1.1)], ..Mesh::default() };
        let out = point_deform(&mesh, &rest, &moved, &PointDeformSettings::defaults()).unwrap();
        for (a, b) in out.vertices.iter().zip(&mesh.vertices) {
            let want = b.rotate_deg([0.0, 90.0, 0.0]).add(shift);
            assert!(a.sub(want).length() < 1e-3, "{a:?} vs {want:?}");
        }
        // Without rotation, a pure shift is still followed exactly.
        let mut slid = rest.clone();
        slid.vertices.iter_mut().for_each(|p| *p = p.add(shift));
        let shifted = point_deform(&mesh, &rest, &slid, &PointDeformSettings { rotate: false, ..PointDeformSettings::defaults() }).unwrap();
        assert!(shifted.vertices[0].sub(mesh.vertices[0].add(shift)).length() < 1e-4);
        assert!(point_deform(&mesh, &rest, &line(3), &PointDeformSettings::defaults()).is_err());
    }

    #[test]
    fn transform_pieces_moves_each_piece_rigidly() {
        // Two pieces (two points each), template points per piece.
        let mut geo = Mesh { vertices: vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 0.0), Vec3::new(5.0, 2.0, 0.0)], ..Mesh::default() };
        geo.point_attributes.insert("piece".into(), vec![0.0, 0.0, 1.0, 1.0]);
        let mut template = Mesh { vertices: vec![Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.0, -10.0, 0.0)], ..Mesh::default() };
        // Piece 0 turns 90° about z; piece 1 is scaled 2.
        let q = crate::vexmath::q_axis_angle(std::f32::consts::FRAC_PI_2, [0.0, 0.0, 1.0]);
        template.point_attributes.insert("orient".into(), [q.to_vec(), vec![0.0, 0.0, 0.0, 1.0]].concat());
        template.point_attributes.insert("pscale".into(), vec![1.0, 2.0]);
        let out = transform_pieces(&geo, &template, None).unwrap();
        // Piece 0: centre (0,0,0) -> (0,10,0); (1,0,0) turns to (0,1,0).
        assert!(out.vertices[0].sub(Vec3::new(0.0, 11.0, 0.0)).length() < 1e-4, "{:?}", out.vertices[0]);
        // Piece 1: centre (5,1,0) -> (0,-10,0), doubled: (5,2,0) -> (0,-8,0).
        assert!(out.vertices[3].sub(Vec3::new(0.0, -8.0, 0.0)).length() < 1e-4, "{:?}", out.vertices[3]);
        // With rest points equal to the template, nothing moves.
        let same = transform_pieces(&geo, &template, Some(&template)).unwrap();
        for (a, b) in same.vertices.iter().zip(&geo.vertices) {
            assert!(a.sub(*b).length() < 1e-4);
        }
        assert!(transform_pieces(&Mesh { vertices: geo.vertices.clone(), ..Mesh::default() }, &template, None).is_err());
    }
}
