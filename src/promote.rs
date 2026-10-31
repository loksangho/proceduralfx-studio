// Attribute Promote: moves attributes between points, vertices, primitives and the
// detail, as Houdini's does. Where several values meet (a point shared by polygons,
// a polygon's points, the whole geometry for the detail), the method combines them:
// average, minimum, maximum, sum, median, first or last.
//
// Primitives are polygons (poly::runs), stored per triangle; vertices are polygon
// corners, stored per triangle corner, so a corner a polygon's triangles share is
// one vertex.

use super::{attrib, poly, Mesh};
use std::collections::HashMap;

pub const CLASSES: [&str; 4] = ["Points", "Vertices", "Primitives", "Detail"];
pub const METHODS: [&str; 7] = ["Average", "Minimum", "Maximum", "Sum", "Median", "First", "Last"];

const POINTS: usize = 0;
const VERTICES: usize = 1;
const PRIMS: usize = 2;
const DETAIL: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct PromoteSettings {
    // Attributes of the original class: names separated by spaces, * and ^name.
    pub pattern: String,
    pub from: usize,
    pub to: usize,
    pub method: usize,
    // A new name (one attribute only); empty keeps the name.
    pub rename: String,
    pub delete_original: bool,
}

impl PromoteSettings {
    pub fn defaults() -> PromoteSettings {
        PromoteSettings { pattern: "Cd".into(), from: POINTS, to: PRIMS, method: 0, rename: String::new(), delete_original: true }
    }

    pub fn params(&self) -> Vec<f32> {
        vec![self.from as f32, self.to as f32, self.method as f32, self.delete_original as u8 as f32]
    }

    pub fn from_params(p: &[f32], pattern: String, rename: String) -> PromoteSettings {
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        let class = |v: f32| (v.max(0.0) as usize).min(3);
        PromoteSettings {
            pattern,
            from: class(g(0, 0.0)),
            to: class(g(1, 2.0)),
            method: (g(2, 0.0).max(0.0) as usize).min(METHODS.len() - 1),
            rename,
            delete_original: g(3, 1.0) > 0.5,
        }
    }
}

fn prefix(class: usize) -> &'static str {
    match class {
        VERTICES => "vertex:",
        PRIMS => "prim:",
        DETAIL => "detail:",
        _ => "",
    }
}

// The geometry's elements of each class, and how they relate.
struct Elements {
    points: usize,
    polys: Vec<std::ops::Range<usize>>,
    // Polygon corners: (polygon, point), and each triangle corner's one.
    corners: Vec<(usize, usize)>,
    corner_of: Vec<usize>,
    // Each point's polygons and corners.
    point_polys: Vec<Vec<usize>>,
    point_corners: Vec<Vec<usize>>,
    // Each polygon's corners, in order.
    poly_corners: Vec<Vec<usize>>,
}

impl Elements {
    fn new(mesh: &Mesh) -> Elements {
        let points = mesh.vertices.len();
        let polys = poly::runs(mesh);
        let mut corners = Vec::new();
        let mut corner_of = vec![0; mesh.indices.len() * 3];
        let mut point_polys = vec![Vec::new(); points];
        let mut point_corners = vec![Vec::new(); points];
        let mut poly_corners = vec![Vec::new(); polys.len()];
        for (p, run) in polys.iter().enumerate() {
            let mut seen: HashMap<usize, usize> = HashMap::new();
            for t in run.clone() {
                for k in 0..3 {
                    let q = mesh.indices[t][k];
                    let corner = *seen.entry(q).or_insert_with(|| {
                        corners.push((p, q));
                        poly_corners[p].push(corners.len() - 1);
                        point_corners[q].push(corners.len() - 1);
                        point_polys[q].push(p);
                        corners.len() - 1
                    });
                    corner_of[t * 3 + k] = corner;
                }
            }
        }
        Elements { points, polys, corners, corner_of, point_polys, point_corners, poly_corners }
    }

    fn count(&self, class: usize) -> usize {
        match class {
            POINTS => self.points,
            VERTICES => self.corners.len(),
            PRIMS => self.polys.len(),
            _ => 1,
        }
    }

    // The elements of class `from` that meet element `index` of class `to`.
    fn sources(&self, to: usize, index: usize, from: usize) -> Vec<usize> {
        match (to, from) {
            (_, DETAIL) => vec![0],
            (DETAIL, _) => (0..self.count(from)).collect(),
            (a, b) if a == b => vec![index],
            (POINTS, PRIMS) => self.point_polys[index].clone(),
            (POINTS, VERTICES) => self.point_corners[index].clone(),
            (PRIMS, POINTS) => self.poly_corners[index].iter().map(|&c| self.corners[c].1).collect(),
            (PRIMS, VERTICES) => self.poly_corners[index].clone(),
            (VERTICES, POINTS) => vec![self.corners[index].1],
            (VERTICES, PRIMS) => vec![self.corners[index].0],
            _ => Vec::new(),
        }
    }
}

// The values of one attribute, one row of `width` per element of its class.
fn read(mesh: &Mesh, e: &Elements, class: usize, key: &str) -> Option<(Vec<f32>, usize)> {
    if class == POINTS && key == "P" {
        return Some((mesh.vertices.iter().flat_map(|p| [p.x, p.y, p.z]).collect(), 3));
    }
    let values = mesh.point_attributes.get(key)?;
    let stored = match class {
        POINTS => e.points,
        VERTICES => mesh.indices.len() * 3,
        PRIMS => mesh.indices.len(),
        _ => 1,
    };
    if stored == 0 || values.is_empty() || values.len() % stored != 0 {
        return None;
    }
    let width = values.len() / stored;
    // Primitives: a polygon's first triangle; vertices: a corner's first triangle corner.
    let rows: Vec<f32> = match class {
        PRIMS => e.polys.iter().flat_map(|run| values[run.start * width..(run.start + 1) * width].iter().copied()).collect(),
        VERTICES => {
            let mut first = vec![usize::MAX; e.corners.len()];
            for (slot, &corner) in e.corner_of.iter().enumerate() {
                if first[corner] == usize::MAX {
                    first[corner] = slot;
                }
            }
            first.iter().flat_map(|&slot| values[slot * width..(slot + 1) * width].iter().copied()).collect()
        }
        _ => values.clone(),
    };
    Some((rows, width))
}

fn combine(method: usize, samples: &mut [f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    match method {
        1 => samples.iter().copied().fold(f32::INFINITY, f32::min),
        2 => samples.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        3 => samples.iter().sum(),
        4 => {
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = samples.len() / 2;
            if samples.len() % 2 == 1 { samples[mid] } else { (samples[mid - 1] + samples[mid]) * 0.5 }
        }
        5 => samples[0],
        6 => samples[samples.len() - 1],
        _ => samples.iter().sum::<f32>() / samples.len() as f32,
    }
}

pub fn promote(input: &Mesh, set: &PromoteSettings) -> Result<Mesh, String> {
    let (from, to) = (set.from.min(3), set.to.min(3));
    let e = Elements::new(input);
    let source_prefix = prefix(from);
    let mut names: Vec<String> = input
        .point_attributes
        .keys()
        .filter_map(|key| {
            let name = if from == POINTS { (!key.contains(':')).then_some(key.as_str()) } else { key.strip_prefix(source_prefix) }?;
            (!name.starts_with("__") && attrib::matches(&set.pattern, name)).then(|| name.to_owned())
        })
        .collect();
    if from == POINTS && set.pattern.split_whitespace().any(|w| w == "P") {
        names.push("P".into());
    }
    names.sort();
    names.dedup();
    let rename = set.rename.trim();
    if names.is_empty() {
        return Err(format!("no {} attribute matches \"{}\"", CLASSES[from].to_lowercase().trim_end_matches('s'), set.pattern.trim()));
    }
    if !rename.is_empty() && names.len() > 1 {
        return Err(format!("a new name needs one attribute, but \"{}\" matches {}", set.pattern.trim(), names.join(", ")));
    }
    if (from == PRIMS || to == PRIMS || from == VERTICES || to == VERTICES) && input.indices.is_empty() {
        return Err("there are no primitives".into());
    }
    let mut out = input.clone();
    for name in names {
        let new_name = if rename.is_empty() { name.clone() } else { rename.to_owned() };
        if to == POINTS && new_name == "P" {
            return Err("P can't be written by promoting: it's the points' positions".into());
        }
        let Some((values, width)) = read(input, &e, from, &format!("{source_prefix}{name}")) else {
            continue;
        };
        // One row per element of the new class.
        let rows: Vec<f32> = (0..e.count(to))
            .flat_map(|index| {
                let sources = e.sources(to, index, from);
                (0..width).map(|c| combine(set.method, &mut sources.iter().map(|&s| values[s * width + c]).collect::<Vec<_>>())).collect::<Vec<_>>()
            })
            .collect();
        // Stored per triangle (primitives) and per triangle corner (vertices).
        let stored: Vec<f32> = match to {
            PRIMS => {
                let mut per_triangle = vec![0.0; input.indices.len() * width];
                for (p, run) in e.polys.iter().enumerate() {
                    for t in run.clone() {
                        per_triangle[t * width..(t + 1) * width].copy_from_slice(&rows[p * width..(p + 1) * width]);
                    }
                }
                per_triangle
            }
            VERTICES => e.corner_of.iter().flat_map(|&c| rows[c * width..(c + 1) * width].iter().copied()).collect(),
            _ => rows,
        };
        if set.delete_original && !(from == POINTS && name == "P") {
            out.point_attributes.remove(&format!("{source_prefix}{name}"));
        }
        out.point_attributes.insert(format!("{}{new_name}", prefix(to)), stored);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec3;

    // Two quads side by side sharing an edge: points 0-5, quads (0,1,4,3) and (1,2,5,4).
    fn quads() -> Mesh {
        let mut mesh = Mesh::default();
        for y in 0..2 {
            for x in 0..3 {
                mesh.vertices.push(Vec3::new(x as f32, y as f32, 0.0));
            }
        }
        mesh.indices = vec![[0, 1, 4], [0, 4, 3], [1, 2, 5], [1, 5, 4]];
        poly::tag_quads(&mut mesh, 0, 2);
        mesh
    }

    fn set(from: usize, to: usize, method: usize) -> PromoteSettings {
        PromoteSettings { pattern: "v".into(), from, to, method, ..PromoteSettings::defaults() }
    }

    #[test]
    fn points_to_primitives_and_back() {
        let mut mesh = quads();
        mesh.point_attributes.insert("v".into(), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        let prims = promote(&mesh, &set(POINTS, PRIMS, 0)).unwrap();
        // Quad 0 averages points 0, 1, 4, 3; quad 1 points 1, 2, 5, 4; both triangles hold it.
        assert_eq!(prims.point_attributes["prim:v"], vec![2.0, 2.0, 3.0, 3.0]);
        assert!(!prims.point_attributes.contains_key("v"), "the original goes");
        let max = promote(&mesh, &PromoteSettings { delete_original: false, ..set(POINTS, PRIMS, 2) }).unwrap();
        assert_eq!(max.point_attributes["prim:v"], vec![4.0, 4.0, 5.0, 5.0]);
        assert!(max.point_attributes.contains_key("v"));
        // Back to points: the shared points (1 and 4) average both quads.
        let back = promote(&prims, &set(PRIMS, POINTS, 0)).unwrap();
        assert_eq!(back.point_attributes["v"], vec![2.0, 2.5, 3.0, 2.0, 2.5, 3.0]);
        let sum = promote(&prims, &set(PRIMS, POINTS, 3)).unwrap();
        assert_eq!(sum.point_attributes["v"], vec![2.0, 5.0, 3.0, 2.0, 5.0, 3.0]);
    }

    #[test]
    fn vertices_and_detail() {
        let mut mesh = quads();
        mesh.point_attributes.insert("v".into(), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        // Points to vertices: each triangle corner its point's value.
        let corners = promote(&mesh, &set(POINTS, VERTICES, 0)).unwrap();
        let expected: Vec<f32> = mesh.indices.iter().flatten().map(|&p| p as f32).collect();
        assert_eq!(corners.point_attributes["vertex:v"], expected);
        // Vertices to primitives: the polygon's 4 corners (not 6 triangle corners).
        let prims = promote(&corners, &set(VERTICES, PRIMS, 3)).unwrap();
        assert_eq!(prims.point_attributes["prim:v"], vec![8.0, 8.0, 12.0, 12.0]);
        // To the detail: median of all points; and back out to every point.
        let detail = promote(&mesh, &set(POINTS, DETAIL, 4)).unwrap();
        assert_eq!(detail.point_attributes["detail:v"], vec![2.5]);
        let everywhere = promote(&detail, &set(DETAIL, POINTS, 0)).unwrap();
        assert_eq!(everywhere.point_attributes["v"], vec![2.5; 6]);
    }

    #[test]
    fn vectors_names_and_errors() {
        let mut mesh = quads();
        mesh.point_attributes.insert("Cd".into(), (0..6).flat_map(|i| [i as f32, 1.0, 0.0]).collect());
        // P to primitives under a new name: the polygons' centres.
        let centres = promote(&mesh, &PromoteSettings { pattern: "P".into(), rename: "centre".into(), ..set(POINTS, PRIMS, 0) }).unwrap();
        assert_eq!(&centres.point_attributes["prim:centre"][..3], &[0.5, 0.5, 0.0]);
        assert_eq!(centres.vertices, mesh.vertices, "P stays");
        let first = promote(&mesh, &PromoteSettings { pattern: "Cd".into(), ..set(POINTS, PRIMS, 5) }).unwrap();
        assert_eq!(&first.point_attributes["prim:Cd"][6..9], &[1.0, 1.0, 0.0]);
        assert!(promote(&mesh, &PromoteSettings { pattern: "nothing".into(), ..set(POINTS, PRIMS, 0) }).is_err());
        mesh.point_attributes.insert("heat".into(), vec![0.0; 6]);
        assert!(promote(&mesh, &PromoteSettings { pattern: "*".into(), rename: "x".into(), ..set(POINTS, PRIMS, 0) }).unwrap_err().contains("one attribute"));
        assert!(promote(&centres, &PromoteSettings { pattern: "centre".into(), rename: "P".into(), ..set(PRIMS, POINTS, 0) }).is_err());
    }
}
