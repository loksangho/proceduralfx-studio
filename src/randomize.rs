// Attribute Randomize: deterministic random values for an attribute.
//
// Presets keep the original behaviour (id and pscale get ±amount added, Cd a
// random colour, orient a random rotation blended in by amount). A custom
// attribute takes any name, class (points, primitives or the detail), size
// (1 to 4 components) and range; values are set, or added to what's there.
// Primitives are polygons: each gets one value, on all its triangles.

use super::{hash01, poly, Mesh};

pub const PRESETS: [&str; 5] = ["id", "pscale", "Cd", "orient", "Custom"];
pub const CLASSES: [&str; 3] = ["Points", "Primitives", "Detail"];
pub const MODES: [&str; 2] = ["Set", "Add"];
pub const CUSTOM: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct RandomizeSettings {
    pub preset: usize,
    pub amount: f32,
    pub seed: usize,
    // Custom attribute.
    pub name: String,
    pub class: usize,
    pub size: usize,
    pub min: f32,
    pub max: f32,
    pub mode: usize,
}

impl RandomizeSettings {
    pub fn defaults() -> Self {
        RandomizeSettings { preset: 1, amount: 0.25, seed: 1, name: "rand".into(), class: 0, size: 1, min: 0.0, max: 1.0, mode: 0 }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![self.preset as f32, self.amount, self.seed as f32, self.class as f32, self.size as f32, self.min, self.max, self.mode as f32]
    }

    // Old saves have three values (preset, amount, seed).
    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(default);
        RandomizeSettings {
            preset: (g(0, d.preset as f32).max(0.0) as usize).min(CUSTOM),
            amount: g(1, d.amount).max(0.0),
            seed: g(2, d.seed as f32).max(0.0) as usize,
            name: d.name,
            class: (g(3, 0.0).max(0.0) as usize).min(CLASSES.len() - 1),
            size: (g(4, 1.0).max(1.0) as usize).min(4),
            min: g(5, d.min),
            max: g(6, d.max),
            mode: (g(7, 0.0).max(0.0) as usize).min(MODES.len() - 1),
        }
    }

    // The attribute's storage key on the geometry.
    pub fn key(&self) -> String {
        match self.preset {
            CUSTOM => {
                let name = self.name.trim();
                match self.class {
                    1 => format!("prim:{name}"),
                    2 => format!("detail:{name}"),
                    _ => name.to_owned(),
                }
            }
            p => PRESETS[p].to_owned(),
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
}

pub fn randomize(mut mesh: Mesh, set: &RandomizeSettings) -> Result<Mesh, String> {
    let points = mesh.vertices.len();
    let seed = set.seed as u32;
    match set.preset.min(CUSTOM) {
        2 => {
            let values = mesh.point_attributes.entry("Cd".into()).or_default();
            values.resize(points * 3, 0.0);
            for index in 0..points {
                let value = (hash01(index as u32, seed) + set.amount * 0.5).fract();
                values[index * 3..index * 3 + 3].copy_from_slice(&crate::attribute_color(value));
            }
        }
        3 => {
            // A random rotation per point, blended in from no rotation by `amount` (0..1).
            let t = set.amount.clamp(0.0, 1.0);
            let values = mesh.point_attributes.entry("orient".into()).or_default();
            values.resize(points * 4, 0.0);
            for index in 0..points {
                let r = |c: u32| hash01(index as u32 * 4 + c, seed ^ 0x6f72_6965);
                // Uniform random unit quaternion (Shoemake).
                let (u1, u2, u3) = (r(0), r(1) * std::f32::consts::TAU, r(2) * std::f32::consts::TAU);
                let (a, b) = ((1.0 - u1).sqrt(), u1.sqrt());
                let q = [a * u2.sin(), a * u2.cos(), b * u3.sin(), b * u3.cos()];
                let mixed = [q[0] * t, q[1] * t, q[2] * t, q[3] * t + (1.0 - t)];
                let len = mixed.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
                values[index * 4..index * 4 + 4].copy_from_slice(&mixed.map(|v| v / len));
            }
        }
        CUSTOM => {
            let name = set.name.trim();
            if !valid_name(name) {
                return Err(format!("'{name}' isn't an attribute name: use letters, digits and _"));
            }
            let size = set.size.clamp(1, 4);
            let tris = mesh.indices.len();
            // (elements that get their own value, how many stored elements each covers)
            let (values_for, stored): (Vec<std::ops::Range<usize>>, usize) = match set.class {
                1 => (poly::runs(&mesh), tris),
                2 => (vec![0..1], 1),
                _ => ((0..points).map(|p| p..p + 1).collect(), points),
            };
            let key = set.key();
            let values = mesh.point_attributes.entry(key).or_default();
            if values.len() != stored * size {
                // A different size or element count: start from zeros.
                *values = vec![0.0; stored * size];
            }
            for (element, covers) in values_for.into_iter().enumerate() {
                for c in 0..size {
                    let v = set.min + (set.max - set.min) * hash01((element * 4 + c) as u32, seed ^ 0x6375_7374);
                    for e in covers.clone() {
                        let slot = &mut values[e * size + c];
                        *slot = if set.mode == 1 { *slot + v } else { v };
                    }
                }
            }
        }
        preset => {
            let name = PRESETS[preset];
            let values = mesh.point_attributes.entry(name.into()).or_default();
            values.resize(points, 0.0);
            for (index, value) in values.iter_mut().enumerate() {
                *value += (hash01(index as u32, seed) - 0.5) * 2.0 * set.amount;
            }
        }
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Mesh {
        crate::node_grid(2.0, 2.0, 3, 3)
    }

    #[test]
    fn custom_attributes_of_any_class_and_size() {
        let set = RandomizeSettings { preset: CUSTOM, name: "wind".into(), size: 3, min: -2.0, max: 2.0, ..RandomizeSettings::defaults() };
        let out = randomize(grid(), &set).unwrap();
        let wind = &out.point_attributes["wind"];
        assert_eq!(wind.len(), 9 * 3);
        assert!(wind.iter().all(|v| (-2.0..=2.0).contains(v)));
        assert!(wind.windows(2).any(|w| w[0] != w[1]), "components and points differ");
        // Primitives: one value per polygon, on both its triangles.
        let prims = randomize(grid(), &RandomizeSettings { class: 1, name: "tag".into(), ..set.clone() }).unwrap();
        let tag = &prims.point_attributes["prim:tag"];
        assert_eq!(tag.len(), 8 * 3);
        assert!(tag.chunks(6).all(|quad| quad[..3] == quad[3..]));
        let detail = randomize(grid(), &RandomizeSettings { class: 2, size: 1, name: "level".into(), ..set.clone() }).unwrap();
        assert_eq!(detail.point_attributes["detail:level"].len(), 1);
        // Add mode adds to what's there; seeds change the values.
        let once = randomize(grid(), &RandomizeSettings { size: 1, ..set.clone() }).unwrap();
        let twice = randomize(once.clone(), &RandomizeSettings { size: 1, mode: 1, ..set.clone() }).unwrap();
        assert!(once.point_attributes["wind"].iter().zip(&twice.point_attributes["wind"]).all(|(a, b)| (b - 2.0 * a).abs() < 1e-5));
        let other = randomize(grid(), &RandomizeSettings { seed: 9, ..set.clone() }).unwrap();
        assert_ne!(other.point_attributes["wind"], out.point_attributes["wind"]);
        assert!(randomize(grid(), &RandomizeSettings { name: "bad name".into(), ..set }).is_err());
    }

    #[test]
    fn presets_and_old_saves() {
        // Old three-value saves load as presets.
        let old = RandomizeSettings::from_params(&[2.0, 0.5, 7.0]);
        assert_eq!((old.preset, old.seed, old.class, old.size), (2, 7, 0, 1));
        let s = RandomizeSettings { preset: CUSTOM, class: 1, size: 3, min: -1.0, max: 4.0, mode: 1, ..RandomizeSettings::defaults() };
        assert_eq!(RandomizeSettings { name: s.name.clone(), ..RandomizeSettings::from_params(&s.to_params()) }, s);
        // orient is a unit quaternion per point; amount 0 is no rotation.
        let out = randomize(grid(), &RandomizeSettings { preset: 3, amount: 1.0, ..RandomizeSettings::defaults() }).unwrap();
        assert!(out.point_attributes["orient"].chunks(4).all(|q| (q.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4));
        let still = randomize(grid(), &RandomizeSettings { preset: 3, amount: 0.0, ..RandomizeSettings::defaults() }).unwrap();
        assert!(still.point_attributes["orient"].chunks(4).all(|q| q == [0.0, 0.0, 0.0, 1.0]));
        let ps = randomize(grid(), &RandomizeSettings::defaults()).unwrap();
        assert!(ps.point_attributes["pscale"].iter().all(|v| v.abs() <= 0.25));
    }
}
