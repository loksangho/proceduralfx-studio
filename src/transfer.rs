// Attribute Transfer: copies attributes from the second input onto the nearest points
// (or primitives) of the first, blending a few nearby sources by distance. Elements
// farther than the distance threshold keep what they had, with an optional blend band
// beyond it so the change fades out instead of stopping at a hard edge.

use super::{attribute_default, attrib, deform, poly, Mesh, Vec3};

pub const CLASSES: [&str; 2] = ["Points", "Primitives"];

#[derive(Clone, Debug, PartialEq)]
pub struct TransferSettings {
    pub class: usize,
    // Which attributes: names separated by spaces, * matches anything, ^ leaves one out.
    pub pattern: String,
    // How many nearby sources are blended for each element.
    pub samples: usize,
    pub distance: f32,
    pub blend_width: f32,
}

impl TransferSettings {
    pub fn defaults() -> TransferSettings {
        TransferSettings { class: 0, pattern: "*".into(), samples: 4, distance: 1.0, blend_width: 0.0 }
    }

    pub fn params(&self) -> Vec<f32> {
        vec![self.class as f32, self.samples as f32, self.distance, self.blend_width]
    }

    pub fn from_params(p: &[f32], pattern: String) -> TransferSettings {
        let d = TransferSettings::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        TransferSettings {
            class: (g(0, 0.0) as usize).min(1),
            pattern,
            samples: (g(1, d.samples as f32) as usize).clamp(1, 64),
            distance: g(2, d.distance).max(0.0),
            blend_width: g(3, d.blend_width).max(0.0),
        }
    }
}

// Where each element is (points, or polygon centres) and, for primitives, the triangles
// of each polygon; `key` is an attribute's stored name for this class.
struct Elements {
    positions: Vec<Vec3>,
    // For primitives: each polygon's triangles. Points: one entry each, of themselves.
    runs: Vec<std::ops::Range<usize>>,
    // How many values an attribute of this class holds per component.
    stored: usize,
}

fn elements(mesh: &Mesh, class: usize) -> Elements {
    if class == 0 {
        Elements { positions: mesh.vertices.clone(), runs: (0..mesh.vertices.len()).map(|i| i..i + 1).collect(), stored: mesh.vertices.len() }
    } else {
        let runs = poly::runs(mesh);
        Elements { positions: runs.iter().map(|r| poly::centroid(mesh, r)).collect(), runs, stored: mesh.indices.len() }
    }
}

pub fn transfer(target: &Mesh, source: &Mesh, set: &TransferSettings) -> Result<Mesh, String> {
    let class = set.class.min(1);
    let prefix = if class == 0 { "" } else { "prim:" };
    let from = elements(source, class);
    let to = elements(target, class);
    if from.positions.is_empty() {
        return Err(format!("the second input has no {} to take attributes from", if class == 0 { "points" } else { "primitives" }));
    }
    // The source's attributes of this class that match, with their component counts.
    let mut names: Vec<(String, usize)> = source
        .point_attributes
        .iter()
        .filter_map(|(key, values)| {
            let name = if class == 0 {
                (!key.contains(':')).then_some(key.as_str())?
            } else {
                key.strip_prefix(prefix)?
            };
            let usable = !name.starts_with("__") && name != "P" && from.stored > 0 && !values.is_empty() && values.len() % from.stored == 0;
            (usable && attrib::matches(&set.pattern, name)).then(|| (key.clone(), values.len() / from.stored))
        })
        .collect();
    names.sort();
    let mut out = target.clone();
    if names.is_empty() || to.positions.is_empty() {
        return Ok(out);
    }
    // For each target element: its nearby sources and weights, and how much of the
    // blend replaces its own value (1 inside the distance, fading to 0 over the band).
    let grid = deform::Grid::new(&from.positions);
    let reach = set.distance + set.blend_width;
    let picks: Vec<(Vec<(usize, f32)>, f32)> = to
        .positions
        .iter()
        .map(|&p| {
            let near: Vec<(f32, usize)> = grid.nearest(p, set.samples.max(1)).into_iter().filter(|(d, _)| *d <= reach).collect();
            let Some(&(closest, _)) = near.first() else { return (Vec::new(), 0.0) };
            let amount = if closest <= set.distance {
                1.0
            } else {
                let t = 1.0 - (closest - set.distance) / set.blend_width.max(1e-9);
                t * t * (3.0 - 2.0 * t)
            };
            // Inverse-distance weights; a source right on the element wins outright.
            let weights: Vec<(usize, f32)> = if closest < 1e-6 {
                vec![(near[0].1, 1.0)]
            } else {
                let raw: Vec<(usize, f32)> = near.iter().map(|&(d, i)| (i, 1.0 / (d * d))).collect();
                let total: f32 = raw.iter().map(|(_, w)| w).sum();
                raw.into_iter().map(|(i, w)| (i, w / total)).collect()
            };
            (weights, amount)
        })
        .collect();
    for (key, width) in names {
        let values = &source.point_attributes[&key];
        // What the target had, if it's the same shape; otherwise the attribute's default.
        let mut result = target.point_attributes.get(&key).filter(|v| v.len() == to.stored * width).cloned().unwrap_or_else(|| vec![attribute_default(&key); to.stored * width]);
        for (element, (weights, amount)) in picks.iter().enumerate() {
            if weights.is_empty() {
                continue;
            }
            let mut blended = vec![0.0f32; width];
            for &(source_element, weight) in weights {
                // A polygon's value is stored on each of its triangles: read the first.
                let at = from.runs[source_element].start * width;
                for (c, b) in blended.iter_mut().enumerate() {
                    *b += values[at + c] * weight;
                }
            }
            for slot in to.runs[element].clone() {
                for (c, b) in blended.iter().enumerate() {
                    let v = &mut result[slot * width + c];
                    *v += (b - *v) * amount;
                }
            }
        }
        out.point_attributes.insert(key, result);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(at: &[[f32; 3]]) -> Mesh {
        Mesh { vertices: at.iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect(), ..Mesh::default() }
    }

    #[test]
    fn nearest_colours_carry_over_within_the_distance() {
        let mut source = points(&[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);
        source.point_attributes.insert("Cd".into(), vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        source.point_attributes.insert("temperature".into(), vec![5.0, 9.0]);
        let mut target = points(&[[0.1, 0.0, 0.0], [9.9, 0.0, 0.0], [5.0, 0.0, 0.0]]);
        target.point_attributes.insert("Cd".into(), vec![0.5; 9]);
        let set = TransferSettings { samples: 1, ..TransferSettings::defaults() };
        let out = transfer(&target, &source, &set).unwrap();
        let cd = &out.point_attributes["Cd"];
        assert_eq!(&cd[..3], &[1.0, 0.0, 0.0]);
        assert_eq!(&cd[3..6], &[0.0, 0.0, 1.0]);
        assert_eq!(&cd[6..9], &[0.5, 0.5, 0.5], "too far from both: keeps its own colour");
        // A new attribute: far elements get its default.
        assert_eq!(out.point_attributes["temperature"], vec![5.0, 9.0, 0.0]);
        // Only what the pattern names.
        let only = transfer(&target, &source, &TransferSettings { pattern: "* ^Cd".into(), ..set.clone() }).unwrap();
        assert_eq!(only.point_attributes["Cd"], vec![0.5; 9]);
        assert!(only.point_attributes.contains_key("temperature"));
    }

    #[test]
    fn blends_nearby_sources_and_fades_past_the_distance() {
        let mut source = points(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
        source.point_attributes.insert("heat".into(), vec![0.0, 1.0]);
        let target = points(&[[1.0, 0.0, 0.0], [3.5, 0.0, 0.0]]);
        let out = transfer(&target, &source, &TransferSettings { pattern: "heat".into(), samples: 2, distance: 1.0, blend_width: 1.0, ..TransferSettings::defaults() }).unwrap();
        let heat = &out.point_attributes["heat"];
        assert!((heat[0] - 0.5).abs() < 1e-5, "halfway between the two: {}", heat[0]);
        // 1.5 from the nearest source: halfway through the blend band.
        assert!(heat[1] > 0.2 && heat[1] < 0.8, "{}", heat[1]);
    }

    #[test]
    fn primitives_take_the_nearest_polygons_value() {
        let mut source = crate::node_grid(2.0, 2.0, 3, 3);
        let n = source.indices.len();
        // The grid's four quads: left two 0, right two 1 (by centre x).
        let polys = poly::runs(&source);
        let mut piece = vec![0.0; n];
        for run in &polys {
            let x = poly::centroid(&source, run).x;
            for t in run.clone() {
                piece[t] = if x > 0.0 { 1.0 } else { 0.0 };
            }
        }
        source.point_attributes.insert("prim:piece".into(), piece);
        let target = crate::node_grid(2.0, 2.0, 5, 5);
        let out = transfer(&target, &source, &TransferSettings { class: 1, samples: 1, ..TransferSettings::defaults() }).unwrap();
        let values = &out.point_attributes["prim:piece"];
        assert_eq!(values.len(), target.indices.len());
        for run in poly::runs(&target) {
            let x = poly::centroid(&target, &run).x;
            assert_eq!(values[run.start], if x > 0.0 { 1.0 } else { 0.0 });
        }
    }
}
