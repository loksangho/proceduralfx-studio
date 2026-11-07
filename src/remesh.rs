// Remesh: rebuilds a surface with even, nearly equilateral triangles of one size, as
// Houdini's Remesh does. It splits edges that are too long, collapses edges that are too
// short (so it adds triangles or takes them away), flips edges toward six around each
// point, slides points to even them out, and keeps every point on the original surface.
//
// The size is a target edge length, or worked out from a target triangle count. Borders,
// edges shared by more than two triangles and sharp edges (above Sharp angle) are kept:
// they're split and collapsed only along themselves, and never flipped across.
//
// Attributes come from the original surface where each new point and corner lands:
// point and corner (vertex) attributes interpolated, primitive attributes from the
// triangle underneath.

use super::{Mesh, Vec3};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct RemeshSettings {
    // The target edge length; 0 works it out from `target` (triangles).
    pub size: f32,
    pub target: usize,
    pub iterations: usize,
    // How far points slide toward their neighbours' centre each iteration, 0-1.
    pub smoothing: f32,
    // Edges whose faces meet at more than this many degrees are kept (0: none).
    pub sharp_angle: f32,
}

impl RemeshSettings {
    pub fn defaults() -> RemeshSettings {
        RemeshSettings { size: 0.0, target: 2_000, iterations: 4, smoothing: 0.5, sharp_angle: 40.0 }
    }

    pub fn params(&self) -> Vec<f32> {
        vec![self.target as f32, self.size, self.iterations as f32, self.smoothing, self.sharp_angle]
    }

    pub fn from_params(p: &[f32]) -> RemeshSettings {
        let d = RemeshSettings::defaults();
        let g = |i: usize, v: f32| p.get(i).copied().unwrap_or(v);
        RemeshSettings {
            target: (g(0, d.target as f32).max(4.0) as usize).min(500_000),
            size: g(1, 0.0).max(0.0),
            iterations: (g(2, d.iterations as f32).max(1.0) as usize).min(20),
            smoothing: g(3, d.smoothing).clamp(0.0, 1.0),
            sharp_angle: g(4, d.sharp_angle).clamp(0.0, 180.0),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Finding the nearest point on the original surface.

fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, [f32; 3]) {
    // Ericson, Real-Time Collision Detection 5.1.5.
    let (ab, ac, ap) = (b.sub(a), c.sub(a), p.sub(a));
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = p.sub(b);
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a.add(ab.scale(v)), [1.0 - v, v, 0.0]);
    }
    let cp = p.sub(c);
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a.add(ac.scale(w)), [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b.add(c.sub(b).scale(w)), [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (a.add(ab.scale(v)).add(ac.scale(w)), [1.0 - v - w, v, w])
}

struct Surface<'a> {
    points: &'a [Vec3],
    tris: &'a [[usize; 3]],
    cell: f32,
    cells: HashMap<[i32; 3], Vec<u32>>,
}

impl<'a> Surface<'a> {
    fn new(points: &'a [Vec3], tris: &'a [[usize; 3]]) -> Surface<'a> {
        let mut edge = 0.0;
        for t in tris.iter().take(2000) {
            edge += points[t[0]].sub(points[t[1]]).length();
        }
        let edge = edge / tris.len().clamp(1, 2000) as f32;
        let (lo, hi) = bounds(points);
        let diag = hi.sub(lo).length().max(1e-6);
        // About two triangles across, and no more than ~200 cells along the diagonal.
        let cell = (edge * 2.0).max(diag / 200.0).max(1e-6);
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for (i, t) in tris.iter().enumerate() {
            let [a, b, c] = t.map(|k| points[k]);
            let lo = key(Vec3::new(a.x.min(b.x).min(c.x), a.y.min(b.y).min(c.y), a.z.min(b.z).min(c.z)), cell);
            let hi = key(Vec3::new(a.x.max(b.x).max(c.x), a.y.max(b.y).max(c.y), a.z.max(b.z).max(c.z)), cell);
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        cells.entry([x, y, z]).or_default().push(i as u32);
                    }
                }
            }
        }
        Surface { points, tris, cell, cells }
    }

    // The nearest point on the surface: the triangle, the point and its barycentric weights.
    fn nearest(&self, p: Vec3) -> (usize, Vec3, [f32; 3]) {
        let c = key(p, self.cell);
        let mut best: Option<(f32, usize, Vec3, [f32; 3])> = None;
        let test = |t: usize, best: &mut Option<(f32, usize, Vec3, [f32; 3])>| {
            let [a, b, cc] = self.tris[t].map(|k| self.points[k]);
            let (q, w) = closest_on_triangle(p, a, b, cc);
            let d = q.sub(p).length();
            if best.is_none_or(|(bd, ..)| d < bd) {
                *best = Some((d, t, q, w));
            }
        };
        for r in 0..48i32 {
            for x in -r..=r {
                for y in -r..=r {
                    for z in -r..=r {
                        if x.abs().max(y.abs()).max(z.abs()) != r {
                            continue;
                        }
                        for &t in self.cells.get(&[c[0] + x, c[1] + y, c[2] + z]).into_iter().flatten() {
                            test(t as usize, &mut best);
                        }
                    }
                }
            }
            // Everything nearer than r cells has been seen.
            if best.is_some_and(|(d, ..)| d <= r as f32 * self.cell) {
                break;
            }
        }
        if best.is_none() {
            for t in 0..self.tris.len() {
                test(t, &mut best);
            }
        }
        let (_, t, q, w) = best.unwrap_or((0.0, 0, p, [1.0, 0.0, 0.0]));
        (t, q, w)
    }
}

fn key(p: Vec3, cell: f32) -> [i32; 3] {
    [(p.x / cell).floor() as i32, (p.y / cell).floor() as i32, (p.z / cell).floor() as i32]
}

fn bounds(points: &[Vec3]) -> (Vec3, Vec3) {
    points.iter().fold((Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN)), |(lo, hi), p| {
        (Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)), Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z)))
    })
}

fn normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    b.sub(a).cross(c.sub(a))
}

// ---------------------------------------------------------------------------------------
// The mesh being rebuilt: triangles, which triangles each point is in, and which edges
// are kept (borders, non-manifold and sharp edges).

struct Work {
    p: Vec<Vec3>,
    tris: Vec<[usize; 3]>,
    alive: Vec<bool>,
    around: Vec<Vec<usize>>,
    dead_point: Vec<bool>,
    kept: std::collections::HashSet<(usize, usize)>,
}

fn edge(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

impl Work {
    fn new(p: Vec<Vec3>, tris: Vec<[usize; 3]>) -> Work {
        let mut around = vec![Vec::new(); p.len()];
        for (t, tri) in tris.iter().enumerate() {
            for &v in tri {
                around[v].push(t);
            }
        }
        let n = p.len();
        Work { alive: vec![true; tris.len()], p, tris, around, dead_point: vec![false; n], kept: Default::default() }
    }

    fn with_edge(&self, a: usize, b: usize) -> Vec<usize> {
        self.around[a].iter().copied().filter(|&t| self.alive[t] && self.tris[t].contains(&b)).collect()
    }

    fn neighbours(&self, v: usize) -> Vec<usize> {
        let mut n: Vec<usize> = self.around[v].iter().filter(|&&t| self.alive[t]).flat_map(|&t| self.tris[t]).filter(|&u| u != v).collect();
        n.sort_unstable();
        n.dedup();
        n
    }

    fn edges(&self) -> Vec<(usize, usize)> {
        let mut e: Vec<(usize, usize)> = self.tris.iter().zip(&self.alive).filter(|(_, a)| **a).flat_map(|(t, _)| [edge(t[0], t[1]), edge(t[1], t[2]), edge(t[2], t[0])]).collect();
        e.sort_unstable();
        e.dedup();
        e
    }

    fn face_normal(&self, t: usize) -> Vec3 {
        let [a, b, c] = self.tris[t].map(|k| self.p[k]);
        normal(a, b, c)
    }

    // Kept: a border, an edge of more than two triangles, or a sharp edge.
    fn mark_kept(&mut self, sharp_cos: Option<f32>) {
        self.kept.clear();
        for (a, b) in self.edges() {
            let t = self.with_edge(a, b);
            let keep = t.len() != 2
                || sharp_cos.is_some_and(|limit| {
                    let (n1, n2) = (self.face_normal(t[0]), self.face_normal(t[1]));
                    let (l1, l2) = (n1.length(), n2.length());
                    l1 > 1e-12 && l2 > 1e-12 && n1.dot(n2) / (l1 * l2) < limit
                });
            if keep {
                self.kept.insert((a, b));
            }
        }
    }

    // How many kept edges meet at a point (0: it moves freely; 2: along a crease or border;
    // other: a corner, which stays).
    fn kept_count(&self, v: usize) -> usize {
        self.neighbours(v).into_iter().filter(|&u| self.kept.contains(&edge(v, u))).count()
    }

    fn split(&mut self, a: usize, b: usize) {
        let m = self.p.len();
        self.p.push(self.p[a].add(self.p[b]).scale(0.5));
        self.around.push(Vec::new());
        self.dead_point.push(false);
        for t in self.with_edge(a, b) {
            let tri = self.tris[t];
            let i = (0..3).find(|&i| tri[i] != a && tri[i] != b).unwrap();
            // The triangle as (x, y, c) with x -> y its edge from a to b in its own order.
            let (x, y, c) = (tri[(i + 1) % 3], tri[(i + 2) % 3], tri[i]);
            self.tris[t] = [x, m, c];
            let new = self.tris.len();
            self.tris.push([m, y, c]);
            self.alive.push(true);
            self.around[y].retain(|&k| k != t);
            self.around[y].push(new);
            self.around[c].push(new);
            self.around[m].extend([t, new]);
        }
        if self.kept.remove(&edge(a, b)) {
            self.kept.insert(edge(a, m));
            self.kept.insert(edge(m, b));
        }
    }

    // Moves `gone` onto `stay` at `at`, if that keeps the surface sound.
    fn collapse(&mut self, stay: usize, gone: usize, at: Vec3, high: f32) -> bool {
        let shared = self.with_edge(stay, gone);
        if shared.is_empty() || shared.len() > 2 {
            return false;
        }
        // Link condition: the only points next to both are the shared triangles' third points.
        let (ns, ng) = (self.neighbours(stay), self.neighbours(gone));
        let common = ns.iter().filter(|v| ng.contains(v)).count();
        if common != shared.len() {
            return false;
        }
        // No triangle may flip or grow an edge past `high`.
        for &v in [stay, gone].iter() {
            for &t in &self.around[v] {
                if !self.alive[t] || shared.contains(&t) {
                    continue;
                }
                let before = self.face_normal(t);
                let moved = self.tris[t].map(|k| if k == stay || k == gone { at } else { self.p[k] });
                let after = normal(moved[0], moved[1], moved[2]);
                if after.length() < 1e-12 || before.dot(after) <= 0.2 * before.length() * after.length() {
                    return false;
                }
                if self.tris[t].iter().any(|&k| k != stay && k != gone && self.p[k].sub(at).length() > high) {
                    return false;
                }
            }
        }
        for &t in &shared {
            self.alive[t] = false;
        }
        let moving: Vec<usize> = self.around[gone].iter().copied().filter(|&t| self.alive[t]).collect();
        for t in moving {
            for k in self.tris[t].iter_mut() {
                if *k == gone {
                    *k = stay;
                }
            }
            self.around[stay].push(t);
        }
        // Kept edges of the gone point now start at the kept one.
        for u in ng {
            if self.kept.remove(&edge(gone, u)) && u != stay {
                self.kept.insert(edge(stay, u));
            }
        }
        self.around[gone].clear();
        self.around[stay].retain(|&t| self.alive[t]);
        self.dead_point[gone] = true;
        self.p[stay] = at;
        true
    }

    fn flip(&mut self, a: usize, b: usize, valence: &mut [i32]) -> bool {
        let shared = self.with_edge(a, b);
        if shared.len() != 2 || self.kept.contains(&edge(a, b)) {
            return false;
        }
        // t1 holds a -> b, t2 holds b -> a.
        let orient = |tri: [usize; 3]| (0..3).find(|&i| tri[i] == a && tri[(i + 1) % 3] == b);
        let (t1, t2) = if orient(self.tris[shared[0]]).is_some() { (shared[0], shared[1]) } else { (shared[1], shared[0]) };
        let third = |tri: [usize; 3]| *tri.iter().find(|&&k| k != a && k != b).unwrap();
        let (c, d) = (third(self.tris[t1]), third(self.tris[t2]));
        if c == d || !self.with_edge(c, d).is_empty() {
            return false;
        }
        let target = |kept: bool| if kept { 4 } else { 6 };
        let flags: Vec<bool> = [a, b, c, d].iter().map(|&v| self.kept_count(v) > 0).collect();
        let deviation = |vals: [i32; 4]| -> i32 { (0..4).map(|i| (vals[i] - target(flags[i])).abs()).sum() };
        let now = [valence[a], valence[b], valence[c], valence[d]];
        let then = [now[0] - 1, now[1] - 1, now[2] + 1, now[3] + 1];
        if deviation(then) >= deviation(now) {
            return false;
        }
        // Only where the two triangles lie nearly flat, and the new ones face the same way.
        let (n1, n2) = (self.face_normal(t1), self.face_normal(t2));
        let (m1, m2) = (normal(self.p[a], self.p[d], self.p[c]), normal(self.p[d], self.p[b], self.p[c]));
        let cos = |x: Vec3, y: Vec3| x.dot(y) / (x.length() * y.length()).max(1e-20);
        if cos(n1, n2) < 0.9 || cos(m1, m2) < 0.9 || cos(m1, n1.add(n2)) < 0.5 {
            return false;
        }
        self.tris[t1] = [a, d, c];
        self.tris[t2] = [d, b, c];
        self.around[b].retain(|&t| t != t1);
        self.around[a].retain(|&t| t != t2);
        self.around[d].push(t1);
        self.around[c].push(t2);
        valence[a] -= 1;
        valence[b] -= 1;
        valence[c] += 1;
        valence[d] += 1;
        true
    }
}

// Joins points at (nearly) the same place, so a surface split along seams remeshes as one.
fn weld(m: &Mesh) -> (Vec<Vec3>, Vec<[usize; 3]>) {
    let (lo, hi) = bounds(&m.vertices);
    let tolerance = (hi.sub(lo).length() * 1e-6).max(1e-9);
    let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut map = vec![0; m.vertices.len()];
    let mut points = Vec::new();
    for (i, p) in m.vertices.iter().enumerate() {
        let c = [(p.x / tolerance).round() as i64, (p.y / tolerance).round() as i64, (p.z / tolerance).round() as i64];
        let mut found = None;
        'look: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &j in cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                        if points[j] == *p || Vec3::sub(points[j], *p).length() <= tolerance {
                            found = Some(j);
                            break 'look;
                        }
                    }
                }
            }
        }
        map[i] = found.unwrap_or_else(|| {
            points.push(*p);
            cells.entry(c).or_default().push(points.len() - 1);
            points.len() - 1
        });
    }
    let mut tris: Vec<[usize; 3]> = m.indices.iter().map(|t| t.map(|k| map[k])).filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
    // The same triangle twice (two sides of a thin shell welded together) would tangle it.
    let mut seen = std::collections::HashSet::new();
    tris.retain(|t| {
        let mut k = *t;
        k.sort_unstable();
        seen.insert(k)
    });
    (points, tris)
}

pub fn remesh(input: &Mesh, set: &RemeshSettings) -> Result<Mesh, String> {
    if input.indices.is_empty() {
        return Err("Remesh needs polygons (its input has none)".into());
    }
    let (points, tris) = weld(input);
    let area: f32 = tris.iter().map(|t| normal(points[t[0]], points[t[1]], points[t[2]]).length() * 0.5).sum();
    if area <= 1e-20 {
        return Err("the input's polygons have no area".into());
    }
    // An equilateral triangle of side L has area sqrt(3)/4 L².
    let length = if set.size > 0.0 { set.size } else { (4.0 * area / (3f32.sqrt() * set.target.max(4) as f32)).sqrt() };
    let estimate = 4.0 * area / (3f32.sqrt() * length * length);
    if estimate > 2_000_000.0 {
        return Err(format!("a target size of {length} would make about {} triangles; use a larger size", estimate as u64));
    }
    let (low, high) = (0.8 * length, 4.0 / 3.0 * length);
    let original = Surface::new(&input.vertices, &input.indices);
    let mut w = Work::new(points, tris);
    let sharp_cos = (set.sharp_angle > 0.0).then(|| set.sharp_angle.to_radians().cos());
    w.mark_kept(sharp_cos);

    for _ in 0..set.iterations.max(1) {
        // Split long edges until none are left.
        for _ in 0..12 {
            let long: Vec<(usize, usize)> = w.edges().into_iter().filter(|&(a, b)| w.p[a].sub(w.p[b]).length() > high).collect();
            if long.is_empty() {
                break;
            }
            for (a, b) in long {
                if !w.with_edge(a, b).is_empty() && w.p[a].sub(w.p[b]).length() > high {
                    w.split(a, b);
                }
            }
        }
        // Collapse short edges, shortest first, until none can go (taking a dense mesh down
        // to a coarse one needs many passes).
        for _ in 0..80 {
            let mut short: Vec<(f32, usize, usize)> = w.edges().into_iter().map(|(a, b)| (w.p[a].sub(w.p[b]).length(), a, b)).filter(|(l, ..)| *l < low).collect();
            short.sort_by(|x, y| x.0.total_cmp(&y.0));
            let mut done = 0;
            for (_, a, b) in short {
                if w.dead_point[a] || w.dead_point[b] || w.p[a].sub(w.p[b]).length() >= low {
                    continue;
                }
                let kept_edge = w.kept.contains(&edge(a, b));
                let (ka, kb) = (w.kept_count(a), w.kept_count(b));
                // Points on kept edges stay on them: a crease or border point only slides along
                // its own kept edge; corners (one or three or more kept edges) don't move.
                let corner = |k: usize| k != 0 && k != 2;
                let plan = match (ka, kb) {
                    (0, 0) => Some((a, b, w.p[a].add(w.p[b]).scale(0.5))),
                    (_, 0) => Some((a, b, w.p[a])),
                    (0, _) => Some((b, a, w.p[b])),
                    _ if !kept_edge => None,
                    _ if corner(ka) && corner(kb) => None,
                    _ if corner(ka) => Some((a, b, w.p[a])),
                    _ if corner(kb) => Some((b, a, w.p[b])),
                    _ => Some((a, b, w.p[a].add(w.p[b]).scale(0.5))),
                };
                if let Some((stay, gone, at)) = plan {
                    if w.collapse(stay, gone, at, high) {
                        done += 1;
                    }
                }
            }
            if done == 0 {
                break;
            }
        }
        // Flip edges toward six neighbours a point (four on kept edges).
        let mut valence: Vec<i32> = (0..w.p.len()).map(|v| if w.dead_point[v] { 0 } else { w.neighbours(v).len() as i32 }).collect();
        for _ in 0..3 {
            let mut flipped = 0;
            for (a, b) in w.edges() {
                if w.flip(a, b, &mut valence) {
                    flipped += 1;
                }
            }
            if flipped == 0 {
                break;
            }
        }
        // Slide free points toward their neighbours' centre, across the surface, then put
        // every point back on the original surface.
        if set.smoothing > 0.0 {
            let mut normals = vec![Vec3::zero(); w.p.len()];
            for (t, tri) in w.tris.iter().enumerate() {
                if w.alive[t] {
                    let n = w.face_normal(t);
                    for &v in tri {
                        normals[v] = normals[v].add(n);
                    }
                }
            }
            let moved: Vec<Vec3> = (0..w.p.len())
                .map(|v| {
                    if w.dead_point[v] || w.kept_count(v) > 0 {
                        return w.p[v];
                    }
                    let ns = w.neighbours(v);
                    if ns.is_empty() {
                        return w.p[v];
                    }
                    let centre = ns.iter().fold(Vec3::zero(), |s, &u| s.add(w.p[u])).scale(1.0 / ns.len() as f32);
                    let n = if normals[v].length() > 1e-20 { normals[v].normalize() } else { Vec3::zero() };
                    let d = centre.sub(w.p[v]);
                    w.p[v].add(d.sub(n.scale(n.dot(d))).scale(set.smoothing))
                })
                .collect();
            w.p = moved;
        }
        for v in 0..w.p.len() {
            if !w.dead_point[v] {
                w.p[v] = original.nearest(w.p[v]).1;
            }
        }
    }

    // The result, numbered afresh, with attributes from the original surface.
    let mut index = vec![usize::MAX; w.p.len()];
    let mut out = Mesh::default();
    for (v, p) in w.p.iter().enumerate() {
        if !w.dead_point[v] && !w.around[v].iter().all(|&t| !w.alive[t]) {
            index[v] = out.vertices.len();
            out.vertices.push(*p);
        }
    }
    let tris: Vec<[usize; 3]> = w.tris.iter().zip(&w.alive).filter(|(_, a)| **a).map(|(t, _)| t.map(|k| index[k])).filter(|t| t.iter().all(|k| *k != usize::MAX)).collect();
    out.indices = tris;
    transfer(input, &original, &mut out);
    Ok(out)
}

fn transfer(input: &Mesh, original: &Surface, out: &mut Mesh) {
    let (n_in, t_in) = (input.vertices.len(), input.indices.len());
    let blend = |values: &[f32], w: usize, at: [usize; 3], weights: [f32; 3], nearest_only: bool| -> Vec<f32> {
        if nearest_only {
            let k = (0..3).max_by(|&x, &y| weights[x].total_cmp(&weights[y])).unwrap();
            return values[at[k] * w..at[k] * w + w].to_vec();
        }
        (0..w).map(|c| (0..3).map(|k| values[at[k] * w + c] * weights[k]).sum()).collect()
    };
    let text_like = |key: &str| crate::strings::is_string_key(key) || key.contains("group:");
    // Points.
    let hits: Vec<(usize, [f32; 3])> = out.vertices.iter().map(|p| { let (t, _, w) = original.nearest(*p); (t, w) }).collect();
    // Each new triangle's source triangle (under its centre), and each corner's weights there.
    let faces: Vec<usize> = out.indices.iter().map(|t| original.nearest(t.iter().fold(Vec3::zero(), |s, &k| s.add(out.vertices[k])).scale(1.0 / 3.0)).0).collect();
    for (key, values) in &input.point_attributes {
        if key == "id" || key == "prim:id" || key == crate::poly::KEY {
            continue;
        }
        if let Some(name) = key.strip_prefix("vertex:") {
            if t_in == 0 || values.len() % (t_in * 3) != 0 {
                continue;
            }
            let w = values.len() / (t_in * 3);
            let mut v = Vec::with_capacity(out.indices.len() * 3 * w);
            for (t, tri) in out.indices.iter().enumerate() {
                let src = faces[t];
                let [a, b, c] = input.indices[src].map(|k| input.vertices[k]);
                for &k in tri {
                    let (_, weights) = closest_on_triangle(out.vertices[k], a, b, c);
                    v.extend(blend(values, w, [src * 3, src * 3 + 1, src * 3 + 2], weights, text_like(name)));
                }
            }
            out.point_attributes.insert(key.clone(), v);
        } else if key.starts_with("prim:") {
            if t_in == 0 || values.len() % t_in != 0 {
                continue;
            }
            let w = values.len() / t_in;
            out.point_attributes.insert(key.clone(), faces.iter().flat_map(|&t| values[t * w..t * w + w].to_vec()).collect());
        } else if key.starts_with("detail:") {
            out.point_attributes.insert(key.clone(), values.clone());
        } else if n_in > 0 && values.len() % n_in == 0 && !values.is_empty() {
            let w = values.len() / n_in;
            let mut v: Vec<f32> = hits.iter().flat_map(|&(t, weights)| blend(values, w, input.indices[t], weights, text_like(key))).collect();
            if key == "N" && w == 3 {
                for n in v.chunks_mut(3) {
                    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    if l > 1e-12 {
                        n.iter_mut().for_each(|c| *c /= l);
                    }
                }
            }
            out.point_attributes.insert(key.clone(), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_lengths(m: &Mesh) -> Vec<f32> {
        m.indices.iter().flat_map(|t| (0..3).map(move |k| (t[k], t[(k + 1) % 3]))).map(|(a, b)| m.vertices[a].sub(m.vertices[b]).length()).collect()
    }

    #[test]
    fn remesh_reduces_and_refines_to_even_triangles() {
        // A dense sphere (radius 1) brought down to about 300 triangles, all on the sphere.
        let dense = crate::node_sphere(1.0, 96, 192);
        assert!(dense.indices.len() > 30_000);
        let coarse = remesh(&dense, &RemeshSettings { target: 300, ..RemeshSettings::defaults() }).unwrap();
        let n = coarse.indices.len();
        assert!((150..600).contains(&n), "{n} triangles");
        assert!(coarse.vertices.iter().all(|p| (p.length() - 1.0).abs() < 0.02), "on the surface");
        // Fairly even: no edge more than four times another's typical length.
        let lengths = edge_lengths(&coarse);
        let mean = lengths.iter().sum::<f32>() / lengths.len() as f32;
        assert!(lengths.iter().all(|l| *l < mean * 2.5 && *l > mean * 0.2), "mean {mean}");
        // Closed: every edge has two triangles.
        let mut count: HashMap<(usize, usize), usize> = HashMap::new();
        for t in &coarse.indices {
            for k in 0..3 {
                *count.entry(edge(t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        assert!(count.values().all(|c| *c == 2), "watertight");

        // A two-triangle quad refined to a given edge length; its border stays put.
        let mut quad = crate::node_grid(2.0, 2.0, 2, 2);
        quad.point_attributes.insert("Cd".into(), quad.vertices.iter().flat_map(|p| [p.x * 0.5 + 0.5, 0.0, 0.0]).collect());
        let fine = remesh(&quad, &RemeshSettings { size: 0.2, ..RemeshSettings::defaults() }).unwrap();
        assert!(fine.indices.len() > 150, "{}", fine.indices.len());
        let (lo, hi) = fine.bounds().unwrap();
        assert!((lo.x + 1.0).abs() < 1e-4 && (hi.x - 1.0).abs() < 1e-4 && (lo.z + 1.0).abs() < 1e-4 && (hi.z - 1.0).abs() < 1e-4, "{lo:?} {hi:?}");
        // Cd follows the original: red rises with x.
        let cd = &fine.point_attributes["Cd"];
        assert!(fine.vertices.iter().enumerate().all(|(i, p)| (cd[i * 3] - (p.x * 0.5 + 0.5)).abs() < 1e-3));
    }

    #[test]
    fn sharp_edges_and_seams_are_kept() {
        // A box whose faces are separate (split along every edge, as UV seams leave models).
        let cube = crate::node_box(2.0, 6);
        let mut split = Mesh::default();
        for t in &cube.indices {
            let base = split.vertices.len();
            split.vertices.extend(t.map(|k| cube.vertices[k]));
            split.indices.push([base, base + 1, base + 2]);
        }
        let out = remesh(&split, &RemeshSettings { target: 400, ..RemeshSettings::defaults() }).unwrap();
        // Welded into one closed box, its corners and edges still square.
        let (lo, hi) = out.bounds().unwrap();
        assert!((lo.x + 1.0).abs() < 1e-4 && (hi.y - 1.0).abs() < 1e-4);
        for corner in [Vec3::new(1.0, 1.0, 1.0), Vec3::new(-1.0, -1.0, 1.0)] {
            assert!(out.vertices.iter().any(|p| p.sub(corner).length() < 1e-4), "corner {corner:?} kept");
        }
        assert!(out.vertices.iter().all(|p| [p.x, p.y, p.z].iter().any(|c| (c.abs() - 1.0).abs() < 1e-3)), "on the box's faces");
        assert!(remesh(&Mesh::default(), &RemeshSettings::defaults()).is_err());
    }
}
