// Attribute Paint: a float point attribute painted with a brush in the viewport.
// The painted values live on the node (one per input point) and are saved with
// the scene; the node writes them onto its input as an attribute.

use super::{Mesh, Vec3};

pub const PAINT_MODES: [&str; 3] = ["Paint", "Erase", "Smooth"];

// Attributes the groom nodes read from the skin (see groom.rs).
pub const GROOM_ATTRIBUTES: [&str; 5] = ["density", "length", "clump", "frizz", "mask"];

#[derive(Clone, Debug, PartialEq)]
pub struct PaintSettings {
    // The value Paint strokes move points toward.
    pub value: f32,
    // Unpainted points, and what Erase restores.
    pub default: f32,
    // Brush radius in screen pixels.
    pub radius: f32,
    // 0..1: how far each dab moves points toward the target.
    pub strength: f32,
    pub mode: usize,
    // Colour the output by the painted value (writes Cd).
    pub visualize: bool,
}

impl Default for PaintSettings {
    fn default() -> Self {
        PaintSettings { value: 1.0, default: 0.0, radius: 40.0, strength: 0.5, mode: 0, visualize: true }
    }
}

// Painted values for a mesh with `count` points: the stored ones when they
// still match the topology, otherwise the default everywhere.
pub fn values_for(values: &[f32], count: usize, default: f32) -> Vec<f32> {
    if values.len() == count { values.to_vec() } else { vec![default; count] }
}

// Low values dark blue, high values warm yellow, over the painted range.
fn heat(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [0.1 + 0.9 * t, 0.15 + 0.7 * t * t, 0.45 * (1.0 - t) + 0.1]
}

pub fn attribute_paint(input: &Mesh, attribute: &str, values: &[f32], set: &PaintSettings) -> Result<Mesh, String> {
    let name = attribute.trim();
    if name.is_empty() || name.contains(':') {
        return Err("give the attribute a plain name, e.g. density".into());
    }
    let n = input.vertices.len();
    let values = values_for(values, n, set.default);
    let mut out = input.clone();
    if set.visualize && name != "Cd" {
        let (lo, hi) = (set.default.min(set.value), set.default.max(set.value));
        let span = (hi - lo).max(1e-6);
        out.point_attributes.insert("Cd".into(), values.iter().flat_map(|v| heat((v - lo) / span)).collect());
    }
    out.point_attributes.insert(name.to_owned(), values);
    Ok(out)
}

// One brush dab. `screen` is each point's screen position when it faces the
// camera (None for hidden or back-facing points).
pub fn dab(values: &mut Vec<f32>, mesh: &Mesh, screen: &[Option<(f32, f32)>], brush: (f32, f32), set: &PaintSettings) -> bool {
    let n = mesh.vertices.len();
    if values.len() != n {
        *values = vec![set.default; n];
    }
    let radius = set.radius.max(1.0);
    let hits: Vec<(usize, f32)> = screen
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let (x, y) = (*p)?;
            let d = ((x - brush.0).powi(2) + (y - brush.1).powi(2)).sqrt() / radius;
            // Smooth falloff from the centre to the edge of the brush.
            (d < 1.0).then(|| (i, (1.0 - d * d).powi(2) * set.strength.clamp(0.0, 1.0)))
        })
        .collect();
    if hits.is_empty() {
        return false;
    }
    match set.mode {
        0 | 1 => {
            let target = if set.mode == 0 { set.value } else { set.default };
            for (i, w) in hits {
                values[i] += (target - values[i]) * w;
            }
        }
        _ => {
            let neighbours = neighbours(mesh);
            let before = values.clone();
            for (i, w) in hits {
                if let Some(avg) = (!neighbours[i].is_empty()).then(|| neighbours[i].iter().map(|&j| before[j]).sum::<f32>() / neighbours[i].len() as f32) {
                    values[i] += (avg - values[i]) * w;
                }
            }
        }
    }
    true
}

fn neighbours(mesh: &Mesh) -> Vec<Vec<usize>> {
    let mut lists = vec![Vec::new(); mesh.vertices.len()];
    for t in &mesh.indices {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            lists[a].push(b);
            lists[b].push(a);
        }
    }
    for list in &mut lists {
        list.sort_unstable();
        list.dedup();
    }
    lists
}

// Which points face the camera, for painting only the visible side. `to_cam`
// maps world positions into camera space (camera at the origin).
pub fn facing(mesh: &Mesh, to_cam: impl Fn(Vec3) -> Vec3) -> Vec<bool> {
    if mesh.indices.is_empty() {
        return vec![true; mesh.vertices.len()];
    }
    let normals = mesh.compute_normals();
    mesh.vertices
        .iter()
        .zip(&normals)
        .map(|(p, n)| {
            let at = to_cam(*p);
            to_cam(p.add(*n)).sub(at).dot(at) <= 0.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Mesh {
        let mut mesh = Mesh::default();
        for i in 0..5 {
            mesh.vertices.push(Vec3::new(i as f32, 0.0, 0.0));
            mesh.vertices.push(Vec3::new(i as f32, 0.0, 1.0));
        }
        for i in 0..4 {
            let a = i * 2;
            mesh.indices.push([a, a + 1, a + 2]);
            mesh.indices.push([a + 1, a + 3, a + 2]);
        }
        mesh
    }

    // Each point straight onto the screen, 100 px per unit along x.
    fn screen(mesh: &Mesh) -> Vec<Option<(f32, f32)>> {
        mesh.vertices.iter().map(|p| Some((p.x * 100.0, p.z * 100.0))).collect()
    }

    #[test]
    fn paint_erase_and_smooth() {
        let mesh = strip();
        let set = PaintSettings { value: 1.0, default: 0.0, radius: 120.0, strength: 1.0, mode: 0, visualize: true };
        let mut values = Vec::new();
        assert!(dab(&mut values, &mesh, &screen(&mesh), (0.0, 0.0), &set));
        assert_eq!(values.len(), 10);
        assert_eq!(values[0], 1.0, "the brush centre gets the full value");
        assert!(values[2] > 0.0 && values[2] < 1.0, "the edge of the brush is partial");
        assert_eq!(values[8], 0.0, "outside the brush stays unpainted");
        // Erase returns the centre to the default.
        dab(&mut values, &mesh, &screen(&mesh), (0.0, 0.0), &PaintSettings { mode: 1, ..set.clone() });
        assert_eq!(values[0], 0.0);
        // Smooth spreads a spike to its neighbours' average.
        let mut spike = vec![0.0; 10];
        spike[4] = 1.0;
        dab(&mut spike, &mesh, &screen(&mesh), (200.0, 0.0), &PaintSettings { mode: 2, radius: 30.0, ..set.clone() });
        assert!(spike[4] < 0.5);
        // Nothing under the brush: no change.
        assert!(!dab(&mut values, &mesh, &screen(&mesh), (5000.0, 0.0), &set));
    }

    #[test]
    fn node_output_and_topology_changes() {
        let mesh = strip();
        let set = PaintSettings::default();
        let painted: Vec<f32> = (0..10).map(|i| i as f32 / 9.0).collect();
        let out = attribute_paint(&mesh, "density", &painted, &set).unwrap();
        assert_eq!(out.point_attributes["density"], painted);
        assert_eq!(out.point_attributes["Cd"].len(), 30);
        // Stale values (the input's point count changed) fall back to the default.
        let stale = attribute_paint(&mesh, "density", &[1.0, 1.0], &set).unwrap();
        assert!(stale.point_attributes["density"].iter().all(|v| *v == set.default));
        assert!(attribute_paint(&mesh, "prim:x", &[], &set).is_err());
        let hidden = attribute_paint(&mesh, "mask", &[], &PaintSettings { visualize: false, ..set }).unwrap();
        assert!(!hidden.point_attributes.contains_key("Cd"));
    }
}
