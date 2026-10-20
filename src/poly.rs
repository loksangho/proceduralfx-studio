// Polygons over triangles. Geometry is stored as triangles; `prim:__poly`
// records which polygon each triangle belongs to, so a quad is two triangles
// that the viewport, primitive numbers, groups, the spreadsheet, VEX and file
// export treat as one face.
//
// A polygon is a run of consecutive triangles with the same non-zero id; 0
// (also what new or merged-in triangles get when an attribute is filled in)
// is a triangle on its own. Geometry without the attribute is all triangles.
// Nodes that cut or re-triangulate faces drop it (`clear`), so their output is
// plain triangles instead of polygons that no longer make sense.

use super::{Mesh, Vec3};
use std::collections::HashSet;
use std::ops::Range;

pub const KEY: &str = "prim:__poly";

// Attributes the user never sees (spreadsheet, attribute menus).
pub fn is_internal(key: &str) -> bool {
    key == KEY
}

fn ids(mesh: &Mesh) -> Option<&[f32]> {
    mesh.point_attributes.get(KEY).map(Vec::as_slice).filter(|v| v.len() == mesh.indices.len() && !v.is_empty())
}

pub fn has_polygons(mesh: &Mesh) -> bool {
    ids(mesh).is_some_and(|ids| ids.windows(2).any(|w| w[0] != 0.0 && w[0] == w[1]))
}

// The triangles of each polygon, in order.
pub fn runs(mesh: &Mesh) -> Vec<Range<usize>> {
    let n = mesh.indices.len();
    let Some(ids) = ids(mesh) else { return (0..n).map(|t| t..t + 1).collect() };
    let mut out = Vec::with_capacity(n);
    let mut start = 0;
    for t in 1..=n {
        if t == n || ids[t] == 0.0 || ids[t] != ids[start] {
            out.push(start..t);
            start = t;
        }
    }
    out
}

pub fn count(mesh: &Mesh) -> usize {
    match ids(mesh) {
        None => mesh.indices.len(),
        Some(ids) => (0..ids.len()).filter(|&t| t == 0 || ids[t] == 0.0 || ids[t] != ids[t - 1]).count(),
    }
}

// The polygon each triangle belongs to.
pub fn of_triangles(mesh: &Mesh) -> Vec<usize> {
    let mut out = vec![0; mesh.indices.len()];
    for (p, run) in runs(mesh).into_iter().enumerate() {
        out[run].fill(p);
    }
    out
}

// Triangle mask from a polygon mask, and back (a polygon is in if any of its triangles is).
pub fn expand(runs: &[Range<usize>], polygons: &[bool], triangles: usize) -> Vec<bool> {
    let mut out = vec![false; triangles];
    for (run, inside) in runs.iter().zip(polygons) {
        if *inside {
            out[run.clone()].fill(true);
        }
    }
    out
}

pub fn collapse(runs: &[Range<usize>], triangles: &[bool]) -> Vec<bool> {
    runs.iter().map(|run| run.clone().any(|t| triangles.get(t).copied().unwrap_or(false))).collect()
}

// Every triangle of the polygons the given triangles belong to.
pub fn whole_polygons(mesh: &Mesh, triangles: &[usize]) -> Vec<usize> {
    let runs = runs(mesh);
    let of = of_triangles(mesh);
    let polygons: HashSet<usize> = triangles.iter().filter_map(|t| of.get(*t).copied()).collect();
    let mut out: Vec<usize> = polygons.into_iter().flat_map(|p| runs[p].clone()).collect();
    out.sort_unstable();
    out
}

// The polygon's points in winding order, when its triangles' outer edges make one
// loop (None for triangles that don't form a simple polygon).
pub fn boundary(mesh: &Mesh, run: &Range<usize>) -> Option<Vec<usize>> {
    let tris = &mesh.indices[run.clone()];
    if tris.len() == 1 {
        return Some(tris[0].to_vec());
    }
    let edges: HashSet<(usize, usize)> = tris.iter().flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]).collect();
    let outer: Vec<(usize, usize)> = tris
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .filter(|(a, b)| !edges.contains(&(*b, *a)))
        .collect();
    let mut next = std::collections::HashMap::new();
    for &(a, b) in &outer {
        if next.insert(a, b).is_some() {
            return None;
        }
    }
    let start = outer.first()?.0;
    let mut loop_points = vec![start];
    let mut at = *next.get(&start)?;
    while at != start {
        if loop_points.len() > outer.len() {
            return None;
        }
        loop_points.push(at);
        at = *next.get(&at)?;
    }
    (loop_points.len() == outer.len()).then_some(loop_points)
}

// A face to write to a file: its triangles, and its corners in winding order as
// (point, triangle corner t*3+k that holds the corner's vertex attributes).
pub struct Face {
    pub tris: Range<usize>,
    pub corners: Vec<(usize, usize)>,
}

impl Face {
    // Corners in the opposite winding, starting from the same corner (for
    // file formats in right-handed space, where Z is mirrored).
    pub fn mirrored(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.corners.first().copied().into_iter().chain(self.corners[1..].iter().rev().copied())
    }
}

// The faces for export: each polygon's outline, or its triangles when they
// don't make one loop.
pub fn faces(mesh: &Mesh) -> Vec<Face> {
    let mut out = Vec::with_capacity(mesh.indices.len());
    for run in runs(mesh) {
        let corner_of = |p: usize| {
            run.clone().find_map(|t| mesh.indices[t].iter().position(|&v| v == p).map(|k| t * 3 + k)).unwrap_or(run.start * 3)
        };
        match boundary(mesh, &run).filter(|_| run.len() > 1) {
            Some(loop_points) => out.push(Face { tris: run.clone(), corners: loop_points.iter().map(|&p| (p, corner_of(p))).collect() }),
            None => {
                for t in run {
                    out.push(Face { tris: t..t + 1, corners: (0..3).map(|k| (mesh.indices[t][k], t * 3 + k)).collect() });
                }
            }
        }
    }
    out
}

// Which of each triangle's edges (0-1, 1-2, 2-0) are a polygon's outline, not a diagonal inside it.
pub fn outline_edges(mesh: &Mesh) -> Vec<[bool; 3]> {
    let mut out = vec![[true; 3]; mesh.indices.len()];
    if ids(mesh).is_none() {
        return out;
    }
    for run in runs(mesh).into_iter().filter(|r| r.len() > 1) {
        let edges: HashSet<(usize, usize)> = mesh.indices[run.clone()].iter().flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]).collect();
        for t in run {
            let tri = mesh.indices[t];
            for k in 0..3 {
                out[t][k] = !edges.contains(&(tri[(k + 1) % 3], tri[k]));
            }
        }
    }
    out
}

// The distinct points of a polygon.
pub fn points(mesh: &Mesh, run: &Range<usize>) -> Vec<usize> {
    boundary(mesh, run).unwrap_or_else(|| {
        let mut seen = Vec::new();
        for t in &mesh.indices[run.clone()] {
            for &v in t {
                if !seen.contains(&v) {
                    seen.push(v);
                }
            }
        }
        seen
    })
}

pub fn centroid(mesh: &Mesh, run: &Range<usize>) -> Vec3 {
    let pts = points(mesh, run);
    pts.iter().fold(Vec3::zero(), |sum, &p| sum.add(mesh.vertices[p])).scale(1.0 / pts.len().max(1) as f32)
}

// Area-weighted normal of the polygon's triangles.
pub fn normal(mesh: &Mesh, run: &Range<usize>) -> Vec3 {
    let n = mesh.indices[run.clone()].iter().fold(Vec3::zero(), |sum, t| {
        let [a, b, c] = t.map(|v| mesh.vertices[v]);
        sum.add(b.sub(a).cross(c.sub(a)))
    });
    if n.length() > 1e-20 { n.normalize() } else { n }
}

// Marks triangles `first..` as polygons of `sizes` triangles each, in order.
pub fn tag(mesh: &mut Mesh, first: usize, sizes: impl IntoIterator<Item = usize>) {
    let n = mesh.indices.len();
    let ids = mesh.point_attributes.entry(KEY.into()).or_default();
    ids.resize(n, 0.0);
    let mut next = ids.iter().copied().fold(0.0f32, f32::max) + 1.0;
    let mut t = first;
    for size in sizes {
        let end = (t + size).min(n);
        if end - t > 1 {
            ids[t..end].fill(next);
            next += 1.0;
        } else if t < end {
            ids[t] = 0.0;
        }
        t = end;
    }
}

// Every pair of triangles from `first` on is one quad (grids, tubes, boxes…).
pub fn tag_quads(mesh: &mut Mesh, first: usize, quads: usize) {
    tag(mesh, first, std::iter::repeat_n(2, quads));
}

// Adds a polygon (points in winding order) as a fan of triangles.
pub fn add(mesh: &mut Mesh, points: &[usize]) {
    if points.len() < 3 {
        return;
    }
    let first = mesh.indices.len();
    for i in 1..points.len() - 1 {
        mesh.indices.push([points[0], points[i], points[i + 1]]);
    }
    tag(mesh, first, [points.len() - 2]);
}

// Renumbers polygon ids 1, 2, 3… in order (same faces, so equal geometry compares equal).
pub fn normalize(mesh: &mut Mesh) {
    if ids(mesh).is_none() {
        return;
    }
    let mut ids = vec![0.0; mesh.indices.len()];
    let mut next = 1.0;
    for run in runs(mesh).into_iter().filter(|r| r.len() > 1) {
        ids[run].fill(next);
        next += 1.0;
    }
    mesh.point_attributes.insert(KEY.into(), ids);
}

// Back to plain triangles, for nodes whose output no longer has the input's faces.
pub fn clear(mesh: &mut Mesh) {
    mesh.point_attributes.remove(KEY);
}

// The mesh without polygons (borrowed when it has none), for operations that cut faces.
pub fn without(mesh: &Mesh) -> std::borrow::Cow<'_, Mesh> {
    if mesh.point_attributes.contains_key(KEY) {
        let mut plain = mesh.clone();
        clear(&mut plain);
        std::borrow::Cow::Owned(plain)
    } else {
        std::borrow::Cow::Borrowed(mesh)
    }
}

// Before appending `b` to `a`: shift b's polygon ids past a's, so polygons stay apart.
pub fn offset_for_merge(a: &Mesh, b: &mut Mesh) {
    let top = ids(a).map_or(0.0, |ids| ids.iter().copied().fold(0.0f32, f32::max));
    if top > 0.0 {
        if let Some(ids) = b.point_attributes.get_mut(KEY) {
            ids.iter_mut().filter(|v| **v != 0.0).for_each(|v| *v += top);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_grid() -> Mesh {
        crate::node_grid(2.0, 2.0, 3, 3)
    }

    #[test]
    fn grids_are_quads() {
        let g = quad_grid();
        assert_eq!(g.indices.len(), 8);
        assert_eq!(count(&g), 4);
        assert!(has_polygons(&g));
        let runs = runs(&g);
        assert!(runs.iter().all(|r| r.len() == 2));
        for run in &runs {
            let loop_points = boundary(&g, run).unwrap();
            assert_eq!(loop_points.len(), 4, "a quad's outline has four points");
        }
        // Each quad's diagonal is hidden: 8 triangles × 3 edges, minus 2 per quad.
        let shown: usize = outline_edges(&g).iter().map(|e| e.iter().filter(|x| **x).count()).sum();
        assert_eq!(shown, 24 - 8);
        assert!((normal(&g, &runs[0]).y.abs() - 1.0).abs() < 1e-5);
        assert!(centroid(&g, &runs[0]).sub(Vec3::new(-0.5, 0.0, -0.5)).length() < 1e-5);
    }

    #[test]
    fn untagged_and_zero_triangles_stand_alone() {
        let mut g = quad_grid();
        clear(&mut g);
        assert_eq!(count(&g), 8);
        assert!(!has_polygons(&g));
        // Zero-filled triangles (an attribute grown for new triangles) are singles.
        let mut g = quad_grid();
        g.point_attributes.get_mut(KEY).unwrap()[0..2].fill(0.0);
        assert_eq!(count(&g), 5);
    }

    #[test]
    fn merged_polygons_stay_apart() {
        let a = quad_grid();
        let mut b = quad_grid();
        offset_for_merge(&a, &mut b);
        let merged = crate::node_merge(a, b);
        assert_eq!(count(&merged), 8);
        // Merging with plain triangles keeps them as triangles.
        let mut tris = quad_grid();
        clear(&mut tris);
        let merged = crate::node_merge(quad_grid(), tris);
        assert_eq!(count(&merged), 4 + 8);
    }

    #[test]
    fn polygons_can_be_added_and_expanded() {
        let mut m = Mesh::default();
        m.vertices = (0..6).map(|i| { let a = i as f32 * std::f32::consts::TAU / 6.0; Vec3::new(a.cos(), 0.0, a.sin()) }).collect();
        add(&mut m, &[0, 1, 2, 3, 4, 5]);
        add(&mut m, &[0, 2, 4]);
        assert_eq!(m.indices.len(), 5);
        assert_eq!(count(&m), 2);
        assert_eq!(boundary(&m, &runs(&m)[0]).unwrap(), vec![0, 1, 2, 3, 4, 5]);
        let r = runs(&m);
        assert_eq!(expand(&r, &[true, false], 5), vec![true, true, true, true, false]);
        assert_eq!(collapse(&r, &[false, true, false, false, true]), vec![true, true]);
        assert_eq!(whole_polygons(&m, &[1]), vec![0, 1, 2, 3]);
    }
}
