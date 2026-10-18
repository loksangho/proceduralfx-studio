// Hair grooming: guide curves grown from a skin mesh, shaped by guide
// processing, then interpolated into dense hair.
//
// Curves are stored on ordinary points: each curve is a run of consecutive
// points, starting at a point whose `curveu` is 0 and running root to tip
// (`curveu` 0..1). `width` sets the strand thickness. Points-only nodes
// (transform, noise, wrangle, merge) therefore keep curves intact.

use super::{hash01, Mesh, Vec3};
use crate::vex::perlin3;
use std::ops::Range;

pub const CURVEU: &str = "curveu";

// Index ranges of the curves in `mesh` (at least two points each).
pub fn curves(mesh: &Mesh) -> Vec<Range<usize>> {
    let n = mesh.vertices.len();
    let Some(u) = mesh.point_attributes.get(CURVEU).filter(|u| u.len() == n) else { return Vec::new() };
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=n {
        if i == n || u[i] <= 0.0 {
            if i - start >= 2 {
                out.push(start..i);
            }
            start = i;
        }
    }
    out
}

pub fn has_curves(mesh: &Mesh) -> bool {
    mesh.point_attributes.get(CURVEU).is_some_and(|u| u.len() == mesh.vertices.len() && u.len() >= 2)
}

// Painted skin attributes the groom reads (Attribute Paint writes them).
pub const PAINTED: [&str; 5] = crate::paint::GROOM_ATTRIBUTES;

struct Root {
    p: Vec3,
    n: Vec3,
    // PAINTED attributes of the skin interpolated at the root (only those present).
    attrs: Vec<(&'static str, f32)>,
}

impl Root {
    fn get(&self, name: &str) -> Option<f32> {
        self.attrs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
}

// Area-weighted root positions on the skin's triangles, also weighted by a
// painted `density` attribute when the skin has one (0 = no hair).
fn scatter_roots(skin: &Mesh, count: usize, seed: u32) -> Vec<Root> {
    let painted: Vec<(&'static str, &Vec<f32>)> = PAINTED
        .iter()
        .filter_map(|name| skin.point_attributes.get(*name).filter(|a| a.len() == skin.vertices.len()).map(|a| (*name, a)))
        .collect();
    let density = painted.iter().find(|(n, _)| *n == "density").map(|(_, a)| *a);
    let mut cumulative = Vec::with_capacity(skin.indices.len());
    let mut total = 0.0;
    for tri in &skin.indices {
        let [a, b, c] = tri.map(|i| skin.vertices[i]);
        let weight = density.map_or(1.0, |d| (tri.iter().map(|&i| d[i]).sum::<f32>() / 3.0).max(0.0));
        total += b.sub(a).cross(c.sub(a)).length() * 0.5 * weight;
        cumulative.push(total);
    }
    if total <= 0.0 {
        return Vec::new();
    }
    (0..count as u32)
        .map(|i| {
            let r = |k: u32| hash01(i.wrapping_mul(7).wrapping_add(k), seed);
            let t = cumulative.partition_point(|&c| c < r(1) * total).min(skin.indices.len() - 1);
            let [a, b, c] = skin.indices[t].map(|i| skin.vertices[i]);
            let (mut u, mut v) = (r(2), r(3));
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            let p = a.add(b.sub(a).scale(u)).add(c.sub(a).scale(v));
            let n = b.sub(a).cross(c.sub(a));
            let [ia, ib, ic] = skin.indices[t];
            let attrs = painted.iter().map(|(name, values)| (*name, values[ia] * (1.0 - u - v) + values[ib] * u + values[ic] * v)).collect();
            Root { p, n: if n.length() > 1e-12 { n.normalize() } else { Vec3::new(0.0, 1.0, 0.0) }, attrs }
        })
        .collect()
}

fn push_curve(out: &mut Mesh, points: &[Vec3], width_root: f32, width_tip: f32, extra: &[(&str, f32)]) {
    let last = (points.len() - 1).max(1) as f32;
    for (j, p) in points.iter().enumerate() {
        let u = j as f32 / last;
        out.vertices.push(*p);
        out.point_attributes.entry(CURVEU.into()).or_default().push(u);
        out.point_attributes.entry("width".into()).or_default().push(width_root + (width_tip - width_root) * u);
        for (name, value) in extra {
            out.point_attributes.entry((*name).into()).or_default().push(*value);
        }
    }
}

// --------------------------------------------------------------------------
// Hair Guides
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct GuideSettings {
    pub count: usize,
    pub length: f32,
    // 0..1: random shortening of individual guides.
    pub length_variance: f32,
    pub segments: usize,
    // Bends guides down under their own weight (0 = straight along the normal).
    pub droop: f32,
    pub seed: u32,
}

pub fn hair_guides(skin: &Mesh, set: &GuideSettings) -> Mesh {
    let mut out = Mesh::default();
    let segments = set.segments.clamp(1, 64);
    let down = Vec3::new(0.0, -1.0, 0.0);
    for (g, r) in scatter_roots(skin, set.count.min(20_000), set.seed).into_iter().enumerate() {
        let (root, normal) = (r.p, r.n);
        let shorten = 1.0 - set.length_variance.clamp(0.0, 1.0) * hash01(g as u32 * 3 + 11, set.seed);
        let step = set.length.max(0.0) * shorten * r.get("length").unwrap_or(1.0).max(0.0) / segments as f32;
        let mut points = vec![root];
        let mut p = root;
        for s in 0..segments {
            // The growth direction leans further toward gravity along the strand.
            let t = (s as f32 + 0.5) / segments as f32;
            let dir = normal.add(down.scale(set.droop.max(0.0) * t * t * 2.0));
            let dir = if dir.length() > 1e-6 { dir.normalize() } else { normal };
            p = p.add(dir.scale(step));
            points.push(p);
        }
        let mut extra = vec![("guideid", g as f32)];
        extra.extend(r.attrs.iter().copied());
        push_curve(&mut out, &points, 0.01, 0.01, &extra);
        let base = out.vertices.len() - points.len();
        let rest = out.point_attributes.entry("rootN".into()).or_default();
        rest.resize(base * 3, 0.0);
        for _ in 0..points.len() {
            rest.extend([normal.x, normal.y, normal.z]);
        }
    }
    out
}

// --------------------------------------------------------------------------
// Guide Process
// --------------------------------------------------------------------------

pub const GUIDE_OPS: [&str; 5] = ["Set length", "Bend (gravity)", "Frizz", "Curl", "Smooth"];

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessSettings {
    pub op: usize,
    pub amount: f32,
    // Frizz/curl frequency.
    pub frequency: f32,
    pub seed: u32,
}

fn curve_length(points: &[Vec3]) -> f32 {
    points.windows(2).map(|w| w[1].sub(w[0]).length()).sum()
}

// Rebuilds a polyline from its segment directions, keeping each segment's length.
fn rebuild(points: &mut [Vec3], dirs: &[Vec3], lengths: &[f32]) {
    for j in 1..points.len() {
        points[j] = points[j - 1].add(dirs[j - 1].scale(lengths[j - 1]));
    }
}

pub fn guide_process(input: &Mesh, set: &ProcessSettings) -> Mesh {
    let mut out = input.clone();
    for (c, range) in curves(input).into_iter().enumerate() {
        let points = &mut out.vertices[range.clone()];
        let n = points.len();
        let lengths: Vec<f32> = points.windows(2).map(|w| w[1].sub(w[0]).length()).collect();
        let dirs: Vec<Vec3> = points.windows(2).map(|w| w[1].sub(w[0])).map(|d| if d.length() > 1e-9 { d.normalize() } else { Vec3::new(0.0, 1.0, 0.0) }).collect();
        let u = |j: usize| j as f32 / (n - 1) as f32;
        // A painted mask (carried from the skin) scales the effect per guide.
        let mask = input.point_attributes.get("mask").filter(|m| m.len() == input.vertices.len()).map_or(1.0, |m| m[range.start].max(0.0));
        let set = &ProcessSettings { amount: if set.op == 0 { set.amount } else { set.amount * mask }, ..set.clone() };
        match set.op {
            // Set length: scale every guide to `amount`.
            0 => {
                let total = curve_length(points);
                if total > 1e-9 {
                    let k = set.amount.max(0.0) / total;
                    let scaled: Vec<f32> = lengths.iter().map(|l| l * k).collect();
                    rebuild(points, &dirs, &scaled);
                }
            }
            // Bend: tilt each segment toward -Y, more toward the tip.
            1 => {
                let down = Vec3::new(0.0, -1.0, 0.0);
                let bent: Vec<Vec3> = dirs
                    .iter()
                    .enumerate()
                    .map(|(j, d)| {
                        let t = (u(j) + u(j + 1)) * 0.5;
                        let d = d.add(down.scale(set.amount.max(0.0) * t * t));
                        if d.length() > 1e-9 { d.normalize() } else { down }
                    })
                    .collect();
                rebuild(points, &bent, &lengths);
            }
            // Frizz: noisy offsets growing toward the tip.
            2 => {
                let total = curve_length(points);
                for j in 1..n {
                    let p = points[j];
                    let q = [p.x * set.frequency, p.y * set.frequency, p.z * set.frequency + c as f32 * 0.37];
                    let offset = Vec3::new(perlin3(q, set.seed), perlin3(q, set.seed + 1), perlin3(q, set.seed + 2));
                    points[j] = p.add(offset.scale(set.amount * total * 0.25 * u(j)));
                }
            }
            // Curl: wind the strand around its root direction as a helix.
            3 => {
                let axis = dirs.first().copied().unwrap_or(Vec3::new(0.0, 1.0, 0.0));
                let helper = if axis.y.abs() < 0.9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(1.0, 0.0, 0.0) };
                let (t1, t2) = (axis.cross(helper).normalize(), axis.cross(axis.cross(helper).normalize()));
                let total = curve_length(points);
                let phase = hash01(c as u32 * 5 + 1, set.seed) * std::f32::consts::TAU;
                let radius = set.amount.max(0.0) * total * 0.1;
                let mut along = 0.0;
                for j in 1..n {
                    along += lengths[j - 1];
                    let angle = phase + along * set.frequency.max(0.0) * std::f32::consts::TAU;
                    let ramp = u(j).min(0.2) / 0.2;
                    let swirl = t1.scale(angle.cos() - phase.cos()).add(t2.scale(angle.sin() - phase.sin())).scale(radius * ramp);
                    points[j] = points[j].add(swirl);
                }
            }
            // Smooth: Laplacian passes, keeping the root fixed.
            _ => {
                let passes = (set.amount.max(0.0) * 10.0).round() as usize;
                for _ in 0..passes {
                    let copy = points.to_vec();
                    for j in 1..n - 1 {
                        points[j] = copy[j].scale(0.5).add(copy[j - 1].add(copy[j + 1]).scale(0.25));
                    }
                    points[n - 1] = copy[n - 1].scale(0.5).add(copy[n - 2].scale(0.5));
                }
                // Restore the original length lost to shrinking.
                let (before, after) = (lengths.iter().sum::<f32>(), curve_length(points));
                if after > 1e-9 {
                    let k = before / after;
                    let root = points[0];
                    for p in points.iter_mut().skip(1) {
                        *p = root.add(p.sub(root).scale(k));
                    }
                }
            }
        }
    }
    out
}

// --------------------------------------------------------------------------
// Hair Generate
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct HairSettings {
    pub count: usize,
    pub segments: usize,
    // 0..1: pulls each hair onto its nearest guide toward the tip.
    pub clump: f32,
    pub length_variance: f32,
    pub frizz: f32,
    pub width_root: f32,
    pub width_tip: f32,
    pub seed: u32,
}

// The curve resampled to `n` points evenly spaced along its length.
fn resample(points: &[Vec3], n: usize) -> Vec<Vec3> {
    let total = curve_length(points);
    if total <= 1e-9 || points.len() < 2 {
        return vec![points[0]; n];
    }
    let mut out = Vec::with_capacity(n);
    let (mut seg, mut walked) = (0usize, 0.0f32);
    for k in 0..n {
        let target = total * k as f32 / (n - 1) as f32;
        while seg + 1 < points.len() - 1 && walked + points[seg + 1].sub(points[seg]).length() < target {
            walked += points[seg + 1].sub(points[seg]).length();
            seg += 1;
        }
        let len = points[seg + 1].sub(points[seg]).length().max(1e-9);
        let t = ((target - walked) / len).clamp(0.0, 1.0);
        out.push(points[seg].add(points[seg + 1].sub(points[seg]).scale(t)));
    }
    out
}

pub fn hair_generate(skin: &Mesh, guides: &Mesh, set: &HairSettings) -> Mesh {
    let n = set.segments.clamp(1, 64) + 1;
    let guide_curves: Vec<Vec<Vec3>> = curves(guides).into_iter().map(|r| resample(&guides.vertices[r], n)).collect();
    let mut out = Mesh::default();
    if guide_curves.is_empty() {
        return out;
    }
    // Hair is interpolated from the three nearest guide roots.
    const NEAREST: usize = 3;
    for (h, r) in scatter_roots(skin, set.count.min(200_000), set.seed ^ 0x68a1).into_iter().enumerate() {
        let root = r.p;
        let mut near: Vec<(usize, f32)> = guide_curves.iter().enumerate().map(|(g, c)| (g, c[0].sub(root).length())).collect();
        let k = NEAREST.min(near.len());
        near.select_nth_unstable_by(k - 1, |a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        near.truncate(k);
        near.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let weights: Vec<f32> = near.iter().map(|(_, d)| 1.0 / (d * d + 1e-6)).collect();
        let total: f32 = weights.iter().sum();
        let shorten = (1.0 - set.length_variance.clamp(0.0, 1.0) * hash01(h as u32 * 3 + 5, set.seed)) * r.get("length").unwrap_or(1.0).max(0.0);
        let (clump_amount, frizz) = (set.clump * r.get("clump").unwrap_or(1.0), set.frizz * r.get("frizz").unwrap_or(1.0));
        let clump_guide = &guide_curves[near[0].0];
        let mut points = Vec::with_capacity(n);
        for j in 0..n {
            let u = j as f32 / (n - 1) as f32;
            // Blend of the guides' shapes, carried to this hair's root.
            let mut offset = Vec3::new(0.0, 0.0, 0.0);
            for ((g, _), w) in near.iter().zip(&weights) {
                offset = offset.add(guide_curves[*g][j].sub(guide_curves[*g][0]).scale(w / total));
            }
            let mut p = root.add(offset.scale(shorten));
            // Clumping draws the strand toward its nearest guide, strongest at the tip.
            let clump = clump_amount.clamp(0.0, 1.0) * u.powf(1.5);
            let target = clump_guide[0].add(clump_guide[j].sub(clump_guide[0]).scale(shorten));
            p = p.add(target.sub(p).scale(clump));
            if frizz > 0.0 && j > 0 {
                let q = [p.x * 8.0, p.y * 8.0, p.z * 8.0 + h as f32 * 0.113];
                let jitter = Vec3::new(perlin3(q, set.seed), perlin3(q, set.seed + 1), perlin3(q, set.seed + 2));
                p = p.add(jitter.scale(frizz * 0.05 * u));
            }
            points.push(p);
        }
        push_curve(&mut out, &points, set.width_root, set.width_tip, &r.attrs);
    }
    out
}

// --------------------------------------------------------------------------
// Hair Deform
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct DeformSettings {
    // The frame the groom was built on; curves and skin are read there as the rest pose.
    pub rest_frame: f32,
}

// Triangles bucketed by their bounding boxes, to find the triangle under a point quickly.
pub(crate) struct TriangleGrid {
    cell: f32,
    cells: std::collections::HashMap<[i32; 3], Vec<usize>>,
}

impl TriangleGrid {
    pub(crate) fn new(mesh: &Mesh) -> Self {
        let mut extent = 0.0;
        for tri in &mesh.indices {
            let [a, b, c] = tri.map(|i| mesh.vertices[i]);
            extent += b.sub(a).length().max(c.sub(a).length()).max(c.sub(b).length());
        }
        let cell = (extent / mesh.indices.len().max(1) as f32).max(1e-5);
        let mut grid = TriangleGrid { cell, cells: std::collections::HashMap::new() };
        for (t, tri) in mesh.indices.iter().enumerate() {
            let [a, b, c] = tri.map(|i| mesh.vertices[i]);
            // Zero-area triangles (e.g. at a sphere's pole) have no usable barycentrics.
            if b.sub(a).cross(c.sub(a)).length() < 1e-12 {
                continue;
            }
            let lo = grid.key(Vec3::new(a.x.min(b.x).min(c.x), a.y.min(b.y).min(c.y), a.z.min(b.z).min(c.z)));
            let hi = grid.key(Vec3::new(a.x.max(b.x).max(c.x), a.y.max(b.y).max(c.y), a.z.max(b.z).max(c.z)));
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        grid.cells.entry([x, y, z]).or_default().push(t);
                    }
                }
            }
        }
        grid
    }

    fn key(&self, p: Vec3) -> [i32; 3] {
        [p.x, p.y, p.z].map(|c| (c / self.cell).floor().clamp(-1e9, 1e9) as i32)
    }

    // The closest triangle to `p` and the closest point's barycentric (u, v).
    pub(crate) fn closest(&self, mesh: &Mesh, p: Vec3) -> Option<(usize, [f32; 2], f32)> {
        let c = self.key(p);
        let mut best: Option<(usize, [f32; 2], f32)> = None;
        let mut seen = std::collections::HashSet::new();
        // Rings of cells outward; anything beyond ring k is at least (k - 1) cells away.
        for k in 0..64i32 {
            for z in c[2] - k..=c[2] + k {
                for y in c[1] - k..=c[1] + k {
                    for x in c[0] - k..=c[0] + k {
                        if (x - c[0]).abs() != k && (y - c[1]).abs() != k && (z - c[2]).abs() != k {
                            continue;
                        }
                        for &t in self.cells.get(&[x, y, z]).map_or(&[][..], Vec::as_slice) {
                            if !seen.insert(t) {
                                continue;
                            }
                            let [a, b, cc] = mesh.indices[t].map(|i| mesh.vertices[i]);
                            let q = crate::volume::closest_on_triangle(p, a, b, cc);
                            let d = q.sub(p).length();
                            if best.is_none_or(|(_, _, bd)| d < bd) {
                                best = Some((t, barycentric(q, a, b, cc), d));
                            }
                        }
                    }
                }
            }
            if best.is_some_and(|(_, _, d)| d <= (k - 1).max(0) as f32 * self.cell) {
                break;
            }
        }
        best
    }
}

fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 2] {
    let (e1, e2, d) = (b.sub(a), c.sub(a), p.sub(a));
    let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
    let denom = d11 * d22 - d12 * d12;
    if denom.abs() < 1e-20 {
        return [0.0, 0.0];
    }
    let (d1, d2) = (d.dot(e1), d.dot(e2));
    [(d22 * d1 - d12 * d2) / denom, (d11 * d2 - d12 * d1) / denom]
}

// Position and orthonormal frame (tangent, bitangent, normal) at (u, v) on triangle `t`,
// using interpolated vertex normals so neighbouring strands turn together.
fn surface_frame(mesh: &Mesh, normals: &[Vec3], t: usize, uv: [f32; 2]) -> (Vec3, [Vec3; 3]) {
    let [ia, ib, ic] = mesh.indices[t];
    let [a, b, c] = [ia, ib, ic].map(|i| mesh.vertices[i]);
    let w = [1.0 - uv[0] - uv[1], uv[0], uv[1]];
    let p = a.scale(w[0]).add(b.scale(w[1])).add(c.scale(w[2]));
    let face = b.sub(a).cross(c.sub(a));
    let mut n = normals[ia].scale(w[0]).add(normals[ib].scale(w[1])).add(normals[ic].scale(w[2]));
    if n.length() < 1e-9 {
        n = face;
    }
    let n = if n.length() > 1e-12 { n.normalize() } else { Vec3::new(0.0, 1.0, 0.0) };
    // The first edge, made perpendicular to the normal.
    let edge = b.sub(a);
    let mut tangent = edge.sub(n.scale(edge.dot(n)));
    if tangent.length() < 1e-9 {
        tangent = if n.y.abs() < 0.9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(1.0, 0.0, 0.0) }.cross(n);
    }
    let tangent = tangent.normalize();
    (p, [tangent, n.cross(tangent), n])
}

// Moves each curve with the skin triangle under its root, from the rest skin to the deformed skin.
pub fn hair_deform(curves_rest: &Mesh, skin_rest: &Mesh, skin_now: &Mesh) -> Result<Mesh, String> {
    let list = curves(curves_rest);
    if list.is_empty() {
        return Err("input 1 has no hair or guide curves".into());
    }
    if skin_rest.indices.is_empty() {
        return Err("input 2 (the skin) has no polygons".into());
    }
    if skin_now.vertices.len() != skin_rest.vertices.len() || skin_now.indices != skin_rest.indices {
        return Err(format!(
            "the skin's topology changes between the rest frame and now ({} vs {} points); \
             Hair Deform needs the same mesh, only moved or deformed",
            skin_rest.vertices.len(),
            skin_now.vertices.len()
        ));
    }
    let grid = TriangleGrid::new(skin_rest);
    let (rest_normals, now_normals) = (skin_rest.compute_normals(), skin_now.compute_normals());
    let mut out = curves_rest.clone();
    for range in list {
        let root = curves_rest.vertices[range.start];
        let Some((t, uv, _)) = grid.closest(skin_rest, root) else { continue };
        let (rest_origin, rest_frame) = surface_frame(skin_rest, &rest_normals, t, uv);
        let (now_origin, now_frame) = surface_frame(skin_now, &now_normals, t, uv);
        // The root keeps its offset from the surface (normally zero).
        for i in range {
            let d = curves_rest.vertices[i].sub(rest_origin);
            let local = rest_frame.map(|axis| d.dot(axis));
            out.vertices[i] = now_origin
                .add(now_frame[0].scale(local[0]))
                .add(now_frame[1].scale(local[1]))
                .add(now_frame[2].scale(local[2]));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skin() -> Mesh {
        let mut mesh = Mesh::default();
        mesh.vertices = vec![Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, 1.0)];
        // Wound so the normal points up (+Y).
        mesh.indices = vec![[0, 2, 1], [0, 3, 2]];
        mesh
    }

    fn guides() -> Mesh {
        hair_guides(&skin(), &GuideSettings { count: 40, length: 1.0, length_variance: 0.0, segments: 6, droop: 0.0, seed: 3 })
    }

    #[test]
    fn guides_grow_from_the_skin_along_the_normal() {
        let g = guides();
        let list = curves(&g);
        assert_eq!(list.len(), 40);
        for r in &list {
            assert_eq!(r.len(), 7);
            let pts = &g.vertices[r.clone()];
            assert!(pts[0].y.abs() < 1e-5 && pts[0].x.abs() <= 1.0 && pts[0].z.abs() <= 1.0);
            assert!((curve_length(pts) - 1.0).abs() < 1e-4);
            assert!((pts[6].y - 1.0).abs() < 1e-4, "straight up without droop");
        }
        assert_eq!(g.point_attributes["width"].len(), g.vertices.len());
        assert_eq!(g.point_attributes["rootN"].len(), g.vertices.len() * 3);
    }

    #[test]
    fn process_ops_keep_roots_and_respect_lengths() {
        let g = guides();
        let roots: Vec<Vec3> = curves(&g).iter().map(|r| g.vertices[r.start]).collect();
        for op in 0..GUIDE_OPS.len() {
            let out = guide_process(&g, &ProcessSettings { op, amount: 0.5, frequency: 2.0, seed: 1 });
            let list = curves(&out);
            assert_eq!(list.len(), roots.len(), "{}", GUIDE_OPS[op]);
            for (r, root) in list.iter().zip(&roots) {
                assert_eq!(out.vertices[r.start], *root, "{} moved a root", GUIDE_OPS[op]);
                let len = curve_length(&out.vertices[r.clone()]);
                match op {
                    0 => assert!((len - 0.5).abs() < 1e-4),
                    1 | 4 => assert!((len - 1.0).abs() < 1e-3, "{} changed length to {len}", GUIDE_OPS[op]),
                    _ => assert!(len > 0.9),
                }
            }
        }
        // Bending pulls tips below where they started.
        let bent = guide_process(&g, &ProcessSettings { op: 1, amount: 2.0, frequency: 1.0, seed: 1 });
        let r = curves(&bent)[0].clone();
        assert!(bent.vertices[r.end - 1].y < 1.0 - 0.1);
    }

    #[test]
    fn generated_hair_follows_guides_and_clumps() {
        let g = guides();
        let settings = HairSettings { count: 500, segments: 5, clump: 0.0, length_variance: 0.0, frizz: 0.0, width_root: 0.004, width_tip: 0.001, seed: 9 };
        let hair = hair_generate(&skin(), &g, &settings);
        let list = curves(&hair);
        assert_eq!(list.len(), 500);
        for r in &list {
            let pts = &hair.vertices[r.clone()];
            assert_eq!(pts.len(), 6);
            // Straight upright guides give straight upright hair of the same length.
            assert!((pts[5].y - 1.0).abs() < 1e-3 && (pts[5].x - pts[0].x).abs() < 1e-3);
        }
        let w = &hair.point_attributes["width"];
        assert!((w[0] - 0.004).abs() < 1e-6 && (w[5] - 0.001).abs() < 1e-6);
        // Full clumping puts every tip on a guide tip.
        let clumped = hair_generate(&skin(), &g, &HairSettings { clump: 1.0, ..settings });
        let tips: Vec<Vec3> = curves(&g).iter().map(|r| g.vertices[r.end - 1]).collect();
        for r in curves(&clumped) {
            let tip = clumped.vertices[r.end - 1];
            assert!(tips.iter().any(|t| t.sub(tip).length() < 1e-3));
        }
        assert!(hair_generate(&skin(), &Mesh::default(), &settings).vertices.is_empty());
    }

    // A bumpy grid, so vertex normals vary across it.
    fn bumpy_skin() -> Mesh {
        let mut mesh = Mesh::default();
        let n = 12;
        for z in 0..=n {
            for x in 0..=n {
                let (fx, fz) = (x as f32 / n as f32 * 2.0 - 1.0, z as f32 / n as f32 * 2.0 - 1.0);
                mesh.vertices.push(Vec3::new(fx, 0.15 * (fx * 3.0).sin() * (fz * 2.0).cos(), fz));
            }
        }
        for z in 0..n {
            for x in 0..n {
                let i = z * (n + 1) + x;
                mesh.indices.push([i, i + n + 1, i + 1]);
                mesh.indices.push([i + 1, i + n + 1, i + n + 2]);
            }
        }
        mesh
    }

    fn rotate_translate(mesh: &Mesh, angle: f32, offset: Vec3) -> Mesh {
        let (s, c) = angle.sin_cos();
        let mut out = mesh.clone();
        for v in &mut out.vertices {
            *v = Vec3::new(c * v.x - s * v.y, s * v.x + c * v.y, v.z).add(offset);
        }
        out
    }

    #[test]
    fn deform_follows_a_rigid_skin_motion_exactly() {
        let skin = bumpy_skin();
        let guides = hair_guides(&skin, &GuideSettings { count: 60, length: 0.5, length_variance: 0.3, segments: 5, droop: 0.5, seed: 4 });
        let (angle, offset) = (0.7, Vec3::new(0.3, 2.0, -1.0));
        let moved_skin = rotate_translate(&skin, angle, offset);
        let deformed = hair_deform(&guides, &skin, &moved_skin).unwrap();
        // A rigid skin motion moves the hair rigidly with it.
        let expected = rotate_translate(&guides, angle, offset);
        for (a, b) in deformed.vertices.iter().zip(&expected.vertices) {
            assert!(a.sub(*b).length() < 1e-3, "{a:?} vs {b:?}");
        }
        assert_eq!(deformed.point_attributes[CURVEU], guides.point_attributes[CURVEU]);
        // No motion: no change.
        let still = hair_deform(&guides, &skin, &skin).unwrap();
        for (a, b) in still.vertices.iter().zip(&guides.vertices) {
            assert!(a.sub(*b).length() < 1e-4);
        }
    }

    #[test]
    fn deform_follows_local_skin_motion_and_keeps_strand_length() {
        let skin = bumpy_skin();
        let guides = hair_guides(&skin, &GuideSettings { count: 80, length: 0.4, length_variance: 0.0, segments: 4, droop: 0.0, seed: 8 });
        // Bend the skin: points with x > 0 rise.
        let mut bent = skin.clone();
        for v in &mut bent.vertices {
            v.y += (v.x.max(0.0)).powi(2) * 0.8;
        }
        let out = hair_deform(&guides, &skin, &bent).unwrap();
        for range in curves(&out) {
            let (before, after) = (&guides.vertices[range.clone()], &out.vertices[range.clone()]);
            assert!((curve_length(before) - curve_length(after)).abs() < 1e-3, "strands stay rigid");
            let root = before[0];
            let lift = root.x.max(0.0).powi(2) * 0.8;
            assert!((after[0].y - (root.y + lift)).abs() < 0.02, "root follows the skin");
        }
    }

    #[test]
    fn deform_rejects_bad_inputs() {
        let skin = bumpy_skin();
        let guides = guides();
        assert!(hair_deform(&Mesh::default(), &skin, &skin).unwrap_err().contains("no hair"));
        assert!(hair_deform(&guides, &Mesh::default(), &Mesh::default()).unwrap_err().contains("no polygons"));
        let mut other = skin.clone();
        other.vertices.pop();
        assert!(hair_deform(&guides, &skin, &other).unwrap_err().contains("topology"));
    }

    #[test]
    fn painted_density_length_and_mask_steer_the_groom() {
        let mut skin = bumpy_skin();
        // Paint: no hair where x < 0, double length where z > 0, mask off where z <= 0.
        let n = skin.vertices.len();
        let density: Vec<f32> = skin.vertices.iter().map(|p| if p.x < -0.05 { 0.0 } else { 1.0 }).collect();
        let length: Vec<f32> = skin.vertices.iter().map(|p| if p.z > 0.0 { 2.0 } else { 1.0 }).collect();
        let mask: Vec<f32> = skin.vertices.iter().map(|p| if p.z > 0.0 { 1.0 } else { 0.0 }).collect();
        for (name, values) in [("density", density), ("length", length), ("mask", mask)] {
            assert_eq!(values.len(), n);
            skin.point_attributes.insert(name.into(), values);
        }
        let guides = hair_guides(&skin, &GuideSettings { count: 200, length: 0.3, length_variance: 0.0, segments: 4, droop: 0.0, seed: 6 });
        let list = curves(&guides);
        assert_eq!(list.len(), 200);
        for r in &list {
            let root = guides.vertices[r.start];
            assert!(root.x > -0.25, "no roots in the unpainted half: {root:?}");
            let len = curve_length(&guides.vertices[r.clone()]);
            if root.z > 0.2 {
                assert!(len > 0.45, "long side: {len}");
            } else if root.z < -0.2 {
                assert!(len < 0.35, "short side: {len}");
            }
        }
        assert_eq!(guides.point_attributes["mask"].len(), guides.vertices.len(), "painted values travel with the curves");
        // The mask limits Bend to the masked guides.
        let bent = guide_process(&guides, &ProcessSettings { op: 1, amount: 3.0, frequency: 1.0, seed: 1 });
        for r in curves(&bent) {
            let moved = bent.vertices[r.end - 1].sub(guides.vertices[r.end - 1]).length();
            if guides.point_attributes["mask"][r.start] < 0.01 {
                assert!(moved < 5e-3, "masked-off guide moved {moved}");
            }
        }
        let hair = hair_generate(&skin, &guides, &HairSettings { count: 400, segments: 4, clump: 0.0, length_variance: 0.0, frizz: 0.0, width_root: 0.01, width_tip: 0.01, seed: 2 });
        assert!(curves(&hair).iter().all(|r| hair.vertices[r.start].x > -0.25));
    }

    #[test]
    fn curves_split_on_curveu_zero() {
        let mut mesh = Mesh::default();
        mesh.vertices = vec![Vec3::new(0.0, 0.0, 0.0); 7];
        mesh.point_attributes.insert(CURVEU.into(), vec![0.0, 0.5, 1.0, 0.0, 1.0, 0.0, 0.0]);
        // The last two "curves" are single points and are skipped.
        assert_eq!(curves(&mesh), vec![0..3, 3..5]);
    }
}
