// Attribute Blur: evens out point attributes (Cd, a mask, N, or P itself to smooth the
// shape) by moving each point's value toward the average of its neighbours, a step at a
// time. Neighbours are the points sharing a polygon edge (Connectivity), or the nearest
// points within a radius (Proximity), which also works on loose points.

use super::{attrib, deform, poly, Mesh, Vec3};

pub const MODES: [&str; 2] = ["Connectivity", "Proximity"];
pub const WEIGHTINGS: [&str; 2] = ["Uniform", "Edge length"];

#[derive(Clone, Debug, PartialEq)]
pub struct BlurSettings {
    // Point attributes to blur ("Cd", "Cd mask", "*"); P only when named.
    pub pattern: String,
    pub mode: usize,
    // Uniform: every neighbour counts the same. Edge length: nearer ones count more.
    pub weighting: usize,
    pub iterations: usize,
    // How far each pass moves toward the neighbours' average (0-1).
    pub step: f32,
    // Proximity: how far to look, and at most how many points.
    pub radius: f32,
    pub max_points: usize,
    // Connectivity: points on open edges keep their values.
    pub pin_border: bool,
}

impl BlurSettings {
    pub fn defaults() -> BlurSettings {
        BlurSettings { pattern: "Cd".into(), mode: 0, weighting: 0, iterations: 10, step: 1.0, radius: 0.2, max_points: 10, pin_border: false }
    }

    pub fn params(&self) -> Vec<f32> {
        vec![self.mode as f32, self.weighting as f32, self.iterations as f32, self.step, self.radius, self.max_points as f32, self.pin_border as u8 as f32]
    }

    pub fn from_params(p: &[f32], pattern: String) -> BlurSettings {
        let d = BlurSettings::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        BlurSettings {
            pattern,
            mode: (g(0, 0.0).max(0.0) as usize).min(MODES.len() - 1),
            weighting: (g(1, 0.0).max(0.0) as usize).min(WEIGHTINGS.len() - 1),
            iterations: g(2, d.iterations as f32).clamp(0.0, 10_000.0) as usize,
            step: g(3, d.step).clamp(0.0, 1.0),
            radius: g(4, d.radius).max(0.0),
            max_points: g(5, d.max_points as f32).clamp(1.0, 1000.0) as usize,
            pin_border: g(6, 0.0) > 0.5,
        }
    }
}

// Each point's neighbours, with their weights.
fn neighbours(mesh: &Mesh, set: &BlurSettings) -> Vec<Vec<(usize, f32)>> {
    let n = mesh.vertices.len();
    let weight = |a: usize, b: usize| if set.weighting == 1 { 1.0 / mesh.vertices[a].sub(mesh.vertices[b]).length().max(1e-6) } else { 1.0 };
    let mut lists: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    if set.mode == 1 {
        let grid = deform::Grid::new(&mesh.vertices);
        for (i, p) in mesh.vertices.iter().enumerate() {
            // One more than wanted: the point finds itself.
            for (distance, j) in grid.nearest(*p, set.max_points + 1) {
                if j != i && distance <= set.radius {
                    lists[i].push((j, weight(i, j)));
                }
            }
        }
        return lists;
    }
    // Polygon edges (not the diagonals splitting a polygon into triangles), each once.
    let outlines = poly::outline_edges(mesh);
    let mut edges: std::collections::HashMap<(usize, usize), u32> = std::collections::HashMap::new();
    for (t, tri) in mesh.indices.iter().enumerate() {
        for k in 0..3 {
            if outlines[t][k] {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
    }
    let mut border = vec![false; n];
    let mut sorted: Vec<_> = edges.into_iter().collect();
    sorted.sort_unstable();
    for ((a, b), uses) in sorted {
        lists[a].push((b, weight(a, b)));
        lists[b].push((a, weight(a, b)));
        // An edge on only one polygon is on the border.
        if uses == 1 {
            border[a] = true;
            border[b] = true;
        }
    }
    if set.pin_border {
        for (list, on_border) in lists.iter_mut().zip(border) {
            if on_border {
                list.clear();
            }
        }
    }
    lists
}

// `mask`: the points that change (the node's group); the others are read but kept.
pub fn blur(input: &Mesh, set: &BlurSettings, mask: Option<&[bool]>) -> Result<Mesh, String> {
    let n = input.vertices.len();
    let mut out = input.clone();
    if n == 0 || set.iterations == 0 || set.step <= 0.0 {
        return Ok(out);
    }
    let mut names: Vec<(String, usize)> = input
        .point_attributes
        .iter()
        .filter(|(name, values)| !name.contains(':') && !name.starts_with("__") && !values.is_empty() && values.len() % n == 0)
        .filter(|(name, _)| attrib::matches(&set.pattern, name))
        .map(|(name, values)| (name.clone(), values.len() / n))
        .collect();
    names.sort();
    // P only when named, as blurring it moves the points.
    let positions = set.pattern.split_whitespace().any(|word| word == "P");
    if names.is_empty() && !positions {
        return Err(format!("no point attribute matches \"{}\"", set.pattern.trim()));
    }
    let lists = neighbours(input, set);
    let changes = |i: usize| mask.is_none_or(|m| m.get(i).copied().unwrap_or(false));
    let step = set.step.clamp(0.0, 1.0);
    let smooth = |values: &mut Vec<f32>, width: usize| {
        let mut next = values.clone();
        for _ in 0..set.iterations {
            for (i, list) in lists.iter().enumerate() {
                let total: f32 = list.iter().map(|(_, w)| w).sum();
                if !changes(i) || total <= 0.0 {
                    continue;
                }
                for c in 0..width {
                    let average = list.iter().map(|(j, w)| values[j * width + c] * w).sum::<f32>() / total;
                    let here = values[i * width + c];
                    next[i * width + c] = here + (average - here) * step;
                }
            }
            std::mem::swap(values, &mut next);
            next.copy_from_slice(values);
        }
    };
    for (name, width) in names {
        let mut values = out.point_attributes[&name].clone();
        smooth(&mut values, width);
        // Normals stay unit length.
        if name == "N" && width == 3 {
            for c in values.chunks_exact_mut(3) {
                let v = Vec3::new(c[0], c[1], c[2]).normalize();
                c.copy_from_slice(&[v.x, v.y, v.z]);
            }
        }
        out.point_attributes.insert(name, values);
    }
    if positions {
        let mut values: Vec<f32> = input.vertices.iter().flat_map(|p| [p.x, p.y, p.z]).collect();
        smooth(&mut values, 3);
        out.vertices = values.chunks_exact(3).map(|c| Vec3::new(c[0], c[1], c[2])).collect();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A row of 5 points joined by quads to a second row: 10 points, 4 quads.
    fn strip() -> Mesh {
        let mut mesh = Mesh::default();
        for row in 0..2 {
            for x in 0..5 {
                mesh.vertices.push(Vec3::new(x as f32, row as f32, 0.0));
            }
        }
        for x in 0..4 {
            mesh.indices.push([x, x + 1, x + 6]);
            mesh.indices.push([x, x + 6, x + 5]);
        }
        poly::tag_quads(&mut mesh, 0, 4);
        mesh
    }

    fn values(mesh: &Mesh, name: &str) -> Vec<f32> {
        mesh.point_attributes[name].clone()
    }

    #[test]
    fn spreads_a_spike_and_keeps_the_total() {
        let mut mesh = strip();
        let mut mask = vec![0.0; 10];
        mask[2] = 1.0;
        mask[7] = 1.0;
        mesh.point_attributes.insert("mask".into(), mask);
        let set = BlurSettings { pattern: "mask".into(), iterations: 1, step: 0.5, ..BlurSettings::defaults() };
        let out = blur(&mesh, &set, None).unwrap();
        let m = values(&out, "mask");
        // Point 2's polygon-edge neighbours are 1, 3 and 7 (not the diagonals).
        assert_eq!(m[2], 1.0 - 0.5 * (1.0 - 1.0 / 3.0));
        assert!(m[1] > 0.0 && m[3] > 0.0 && m[0] == 0.0, "{m:?}");
        // Many passes even it out.
        let even = blur(&mesh, &BlurSettings { pattern: "mask".into(), iterations: 500, ..BlurSettings::defaults() }, None).unwrap();
        let e = values(&even, "mask");
        assert!(e.iter().all(|v| (v - 0.2).abs() < 0.05), "{e:?}");
    }

    #[test]
    fn groups_borders_and_proximity() {
        let mut mesh = strip();
        mesh.point_attributes.insert("Cd".into(), (0..10).flat_map(|i| [i as f32, 0.0, 0.0]).collect());
        // Only points 0-4 change.
        let mask: Vec<bool> = (0..10).map(|i| i < 5).collect();
        let out = blur(&mesh, &BlurSettings::defaults(), Some(&mask)).unwrap();
        let cd = values(&out, "Cd");
        assert!((5..10).all(|i| cd[i * 3] == i as f32), "outside the group, unchanged");
        assert!(cd[0] != 0.0);
        // Pinned border: every point of the strip is on its border.
        let pinned = blur(&mesh, &BlurSettings { pin_border: true, ..BlurSettings::defaults() }, None).unwrap();
        assert_eq!(values(&pinned, "Cd"), values(&mesh, "Cd"));
        // Loose points blur by proximity.
        let mut dots = Mesh { vertices: (0..4).map(|i| Vec3::new(i as f32 * 0.1, 0.0, 0.0)).collect(), ..Mesh::default() };
        dots.point_attributes.insert("heat".into(), vec![1.0, 0.0, 0.0, 0.0]);
        assert_eq!(values(&blur(&dots, &BlurSettings { pattern: "heat".into(), ..BlurSettings::defaults() }, None).unwrap(), "heat"), vec![1.0, 0.0, 0.0, 0.0], "no edges: nothing moves");
        let near = blur(&dots, &BlurSettings { pattern: "heat".into(), mode: 1, iterations: 1, radius: 0.15, ..BlurSettings::defaults() }, None).unwrap();
        assert_eq!(values(&near, "heat"), vec![0.0, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn blurring_p_smooths_and_n_stays_unit() {
        let mut mesh = strip();
        mesh.vertices[2].z = 1.0;
        mesh.point_attributes.insert("N".into(), (0..10).flat_map(|i| if i == 2 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] }).collect());
        let out = blur(&mesh, &BlurSettings { pattern: "P N".into(), iterations: 3, step: 0.5, ..BlurSettings::defaults() }, None).unwrap();
        assert!(out.vertices[2].z < 1.0 && out.vertices[1].z > 0.0, "the bump spreads");
        assert!(values(&out, "N").chunks(3).all(|n| (Vec3::new(n[0], n[1], n[2]).length() - 1.0).abs() < 1e-5));
        // "*" leaves P alone.
        assert_eq!(blur(&mesh, &BlurSettings { pattern: "*".into(), ..BlurSettings::defaults() }, None).unwrap().vertices, mesh.vertices);
        assert!(blur(&mesh, &BlurSettings { pattern: "nothing".into(), ..BlurSettings::defaults() }, None).is_err());
    }
}
