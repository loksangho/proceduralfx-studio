// Color node: sets Cd on points or primitives, as one colour, random colours (per
// element, or per value of an attribute such as `piece`), or a ramp between two
// colours along an axis.

use super::{hash01, poly, Mesh, Vec3};

pub const CLASSES: [&str; 2] = ["Points", "Primitives"];
pub const MODES: [&str; 3] = ["Constant", "Random", "Ramp"];
pub const AXES: [&str; 3] = ["X", "Y", "Z"];

#[derive(Clone, Debug, PartialEq)]
pub struct ColorSettings {
    pub class: usize,
    pub mode: usize,
    pub color: [f32; 3],
    // The ramp's far end.
    pub color2: [f32; 3],
    pub seed: u32,
    pub axis: usize,
    // Random: elements sharing this attribute's value get the same colour (empty: each its own).
    pub random_by: String,
}

impl ColorSettings {
    pub fn defaults() -> ColorSettings {
        ColorSettings { class: 0, mode: 0, color: [1.0, 0.45, 0.1], color2: [0.1, 0.3, 1.0], seed: 1, axis: 1, random_by: String::new() }
    }

    pub fn params(&self) -> Vec<f32> {
        let [r, g, b] = self.color;
        let [r2, g2, b2] = self.color2;
        vec![self.class as f32, self.mode as f32, r, g, b, r2, g2, b2, self.seed as f32, self.axis as f32]
    }

    pub fn from_params(p: &[f32], random_by: String) -> ColorSettings {
        let d = ColorSettings::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
        ColorSettings {
            class: (g(0, 0.0) as usize).min(1),
            mode: (g(1, 0.0) as usize).min(MODES.len() - 1),
            color: [g(2, d.color[0]), g(3, d.color[1]), g(4, d.color[2])],
            color2: [g(5, d.color2[0]), g(6, d.color2[1]), g(7, d.color2[2])],
            seed: g(8, 1.0).max(0.0) as u32,
            axis: (g(9, 1.0) as usize).min(2),
            random_by,
        }
    }
}

// A bright, varied colour for `key`: any hue, fairly saturated, never dark.
fn random_color(key: u32, seed: u32) -> [f32; 3] {
    let h = hash01(key, seed.wrapping_mul(3).wrapping_add(11)) * 6.0;
    let s = 0.45 + 0.45 * hash01(key, seed.wrapping_mul(3).wrapping_add(12));
    let v = 0.65 + 0.35 * hash01(key, seed.wrapping_mul(3).wrapping_add(13));
    let f = h.fract();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match h as u32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub fn color(input: &Mesh, set: &ColorSettings) -> Result<Mesh, String> {
    let points = set.class == 0;
    if !points && input.indices.is_empty() {
        return Err("there are no primitives to colour; set Class to Points".into());
    }
    let polys = poly::runs(input);
    let count = if points { input.vertices.len() } else { polys.len() };
    // The attribute Random groups by: its first component per element.
    let by = set.random_by.trim();
    let keys: Option<Vec<f32>> = if by.is_empty() {
        None
    } else {
        let read = |name: &str, n: usize| input.point_attributes.get(name).filter(|v| !v.is_empty() && v.len() % n.max(1) == 0).map(|v| (v.clone(), v.len() / n.max(1)));
        let found = if points {
            read(by, input.vertices.len()).map(|(v, w)| (0..count).map(|i| v[i * w]).collect())
        } else if let Some((v, w)) = read(&format!("prim:{by}"), input.indices.len()) {
            Some(polys.iter().map(|run| v[run.start * w]).collect())
        } else {
            // A point attribute still works for primitives: their first point's value.
            read(by, input.vertices.len()).map(|(v, w)| polys.iter().map(|run| v[input.indices[run.start][0] * w]).collect())
        };
        Some(found.ok_or_else(|| format!("there's no attribute called {by} to pick colours by"))?)
    };
    let position = |i: usize| if points { input.vertices[i] } else { poly::centroid(input, &polys[i]) };
    let axis_of = |p: Vec3| [p.x, p.y, p.z][set.axis.min(2)];
    let (lo, hi) = (0..count).map(|i| axis_of(position(i))).fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let colors: Vec<[f32; 3]> = (0..count)
        .map(|i| match set.mode {
            1 => {
                let key = keys.as_ref().map_or(i as u32, |k| k[i].to_bits());
                random_color(key, set.seed)
            }
            2 => {
                let t = if hi > lo { (axis_of(position(i)) - lo) / (hi - lo) } else { 0.0 };
                [0, 1, 2].map(|c| set.color[c] + (set.color2[c] - set.color[c]) * t)
            }
            _ => set.color,
        })
        .collect();
    let mut out = input.clone();
    if points {
        out.point_attributes.insert("Cd".into(), colors.iter().flatten().copied().collect());
    } else {
        let mut prim = vec![0.0; input.indices.len() * 3];
        for (run, c) in polys.iter().zip(&colors) {
            for t in run.clone() {
                prim[t * 3..t * 3 + 3].copy_from_slice(c);
            }
        }
        // One Cd at a time: primitive colour replaces point colour, as the viewport and
        // renderers read whichever is there.
        out.point_attributes.remove("Cd");
        out.point_attributes.insert("prim:Cd".into(), prim);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_pair() -> Mesh {
        Mesh {
            vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 2.0, 0.0)],
            indices: vec![[0, 1, 2], [0, 2, 3], [3, 2, 5], [3, 5, 4]],
            ..Mesh::default()
        }
    }

    #[test]
    fn constant_ramp_and_random() {
        let mesh = quad_pair();
        let constant = color(&mesh, &ColorSettings::defaults()).unwrap();
        assert_eq!(&constant.point_attributes["Cd"][..3], &[1.0, 0.45, 0.1]);
        let ramp = color(&mesh, &ColorSettings { mode: 2, color: [0.0; 3], color2: [1.0; 3], ..ColorSettings::defaults() }).unwrap();
        let cd = &ramp.point_attributes["Cd"];
        assert_eq!((cd[0], cd[3 * 3], cd[4 * 3]), (0.0, 0.5, 1.0), "bottom, middle and top of the ramp");
        let prims = color(&mesh, &ColorSettings { class: 1, mode: 1, ..ColorSettings::defaults() }).unwrap();
        let cd = &prims.point_attributes["prim:Cd"];
        assert_eq!(cd.len(), 4 * 3);
        assert!(!prims.point_attributes.contains_key("Cd"));
        assert_ne!(&cd[..3], &cd[3..6], "each triangle its own colour");
    }

    #[test]
    fn random_by_attribute_shares_colours() {
        let mut mesh = quad_pair();
        mesh.point_attributes.insert("prim:piece".into(), vec![0.0, 0.0, 1.0, 1.0]);
        let out = color(&mesh, &ColorSettings { class: 1, mode: 1, random_by: "piece".into(), ..ColorSettings::defaults() }).unwrap();
        let cd = &out.point_attributes["prim:Cd"];
        assert_eq!(&cd[..3], &cd[3..6]);
        assert_eq!(&cd[6..9], &cd[9..12]);
        assert_ne!(&cd[..3], &cd[6..9]);
        assert!(color(&mesh, &ColorSettings { mode: 1, random_by: "nope".into(), ..ColorSettings::defaults() }).unwrap_err().contains("nope"));
    }
}
