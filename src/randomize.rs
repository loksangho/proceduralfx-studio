// Attribute Randomize: deterministic random values for any attribute.
//
// It takes a name, class and type, as Attribute Create does (attrib.rs), Min and Max for
// each component, and how values are picked:
// - Uniform: each component on its own, between its Min and Max. For a quaternion these
//   are rotations in degrees, so Min 0 0 0 and Max 0 360 0 spins each point around Y.
// - Between Min and Max: one random point on the way from Min to Max, all components
//   together (a colour between two colours, a vector along a line).
// - Direction: a random unit vector, its length between Min and Max's first components.
// - Hue: a random colour of full saturation; Amount shifts the hues.
// - Rotation: a random orientation, blended in from none by Amount (0 to 1).
// Values are set, added to what's there, or multiply it; for quaternions adding or
// multiplying turns what's there by the random rotation.
//
// Scenes from before this had presets (id, pscale, Cd, orient) instead; they keep
// working exactly as they did until the panel switches them to these settings.

use super::{attrib, hash01, Mesh};

pub const PRESETS: [&str; 5] = ["id", "pscale", "Cd", "orient", "Custom"];
pub const MODES: [&str; 3] = ["Set", "Add", "Multiply"];
pub const DISTRIBUTIONS: [&str; 5] = ["Uniform", "Between Min and Max", "Direction", "Hue", "Rotation"];
pub const CUSTOM: usize = 4;
pub const DIRECTION: usize = 2;
pub const HUE: usize = 3;
pub const ROTATION: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct RandomizeSettings {
    // Below CUSTOM: an old preset, kept as it was.
    pub preset: usize,
    pub amount: f32,
    pub seed: usize,
    pub name: String,
    pub class: usize,
    pub kind: usize,
    pub size: usize,
    pub distribution: usize,
    pub min: [f32; 4],
    pub max: [f32; 4],
    pub mode: usize,
}

impl RandomizeSettings {
    pub fn defaults() -> Self {
        RandomizeSettings {
            preset: CUSTOM,
            amount: 1.0,
            seed: 1,
            name: "Cd".into(),
            class: 0,
            kind: attrib::COLOUR,
            size: 3,
            distribution: 0,
            min: [0.0; 4],
            max: [1.0; 4],
            mode: 0,
        }
    }

    // The old eight numbers first (preset, amount, seed, class, size, min, max, mode), so
    // older versions still read the rest of the scene, then type, distribution and the
    // other components of Min and Max.
    pub fn to_params(&self) -> Vec<f32> {
        let (mn, mx) = (self.min, self.max);
        vec![
            self.preset as f32, self.amount, self.seed as f32, self.class as f32, self.size as f32, mn[0], mx[0], self.mode as f32,
            self.kind as f32, self.distribution as f32, mn[1], mn[2], mn[3], mx[1], mx[2], mx[3],
        ]
    }

    // Old saves have three values (preset, amount, seed) or eight (one Min and Max for every
    // component, of a float attribute).
    pub fn from_params(p: &[f32]) -> Self {
        let g = |i: usize, default: f32| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(default);
        let (min0, max0) = (g(5, 0.0), g(6, 1.0));
        let extended = p.len() > 8;
        RandomizeSettings {
            preset: (g(0, CUSTOM as f32).max(0.0) as usize).min(CUSTOM),
            amount: g(1, 1.0).max(0.0),
            seed: g(2, 1.0).max(0.0) as usize,
            name: "rand".into(),
            class: (g(3, 0.0).max(0.0) as usize).min(attrib::CLASSES.len() - 1),
            size: (g(4, 1.0).max(1.0) as usize).min(4),
            mode: (g(7, 0.0).max(0.0) as usize).min(MODES.len() - 1),
            kind: if extended { (g(8, 0.0).max(0.0) as usize).min(attrib::TYPES.len() - 1) } else { 0 },
            distribution: if extended { (g(9, 0.0).max(0.0) as usize).min(DISTRIBUTIONS.len() - 1) } else { 0 },
            min: if extended { [min0, g(10, 0.0), g(11, 0.0), g(12, 0.0)] } else { [min0; 4] },
            max: if extended { [max0, g(13, 1.0), g(14, 1.0), g(15, 1.0)] } else { [max0; 4] },
        }
    }

    // These settings for an old preset, so the panel can switch it over unchanged.
    pub fn converted(&self) -> Self {
        let a = self.amount;
        let base = RandomizeSettings { preset: CUSTOM, class: 0, ..self.clone() };
        match self.preset {
            0 | 1 => RandomizeSettings { name: PRESETS[self.preset].into(), kind: 0, size: 1, distribution: 0, min: [-a; 4], max: [a; 4], mode: 1, ..base },
            2 => RandomizeSettings { name: "Cd".into(), kind: attrib::COLOUR, size: 3, distribution: HUE, mode: 0, ..base },
            3 => RandomizeSettings { name: "orient".into(), kind: attrib::QUATERNION, size: 4, distribution: ROTATION, mode: 0, ..base },
            _ => self.clone(),
        }
    }

    pub fn key(&self) -> String {
        if self.preset < CUSTOM {
            return PRESETS[self.preset].to_owned();
        }
        attrib::key(&self.name, self.class)
    }

    // Distributions that make sense for this type.
    pub fn distributions(kind: usize) -> Vec<usize> {
        match kind {
            attrib::VECTOR => vec![0, 1, DIRECTION],
            attrib::COLOUR => vec![0, 1, HUE],
            attrib::QUATERNION => vec![0, 1, ROTATION],
            _ => vec![0, 1],
        }
    }
}

// A uniformly random rotation (Shoemake), blended in from none by `t`.
fn random_rotation(r: impl Fn(u32) -> f32, t: f32) -> [f32; 4] {
    let (u1, u2, u3) = (r(0), r(1) * std::f32::consts::TAU, r(2) * std::f32::consts::TAU);
    let (a, b) = ((1.0 - u1).sqrt(), u1.sqrt());
    let q = [a * u2.sin(), a * u2.cos(), b * u3.sin(), b * u3.cos()];
    let mixed = [q[0] * t, q[1] * t, q[2] * t, q[3] * t + (1.0 - t)];
    let len = mixed.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
    mixed.map(|v| v / len)
}

// One element's random value, as stored (a quaternion as x y z w).
fn pick(set: &RandomizeSettings, element: usize, components: usize) -> Vec<f32> {
    let seed = set.seed as u32 ^ 0x6375_7374;
    let r = |c: u32| hash01(element as u32 * 8 + c, seed);
    let lerp = |c: usize, t: f32| set.min[c] + (set.max[c] - set.min[c]) * t;
    let uniform: Vec<f32> = (0..components.min(4)).map(|c| lerp(c, r(c as u32))).collect();
    match (set.kind, set.distribution) {
        (attrib::QUATERNION, ROTATION) => random_rotation(r, set.amount.clamp(0.0, 1.0)).to_vec(),
        (attrib::QUATERNION, 1) => {
            let t = r(4);
            attrib::quaternion([lerp(0, t), lerp(1, t), lerp(2, t)]).to_vec()
        }
        (attrib::QUATERNION, _) => attrib::quaternion([uniform[0], uniform[1], uniform[2]]).to_vec(),
        (attrib::COLOUR, HUE) => crate::attribute_color((r(0) + set.amount * 0.5).fract()).to_vec(),
        (_, DIRECTION) => {
            // Normalised point in a ball, so every direction is as likely.
            let d = loop_free_direction(&r);
            let length = set.min[0] + (set.max[0] - set.min[0]) * r(5);
            d.iter().map(|v| v * length).collect()
        }
        (kind, 1) => {
            let t = r(4);
            let v: Vec<f32> = (0..components.min(4)).map(|c| lerp(c, t)).collect();
            if kind == attrib::INTEGER { v.iter().map(|x| x.round()).collect() } else { v }
        }
        // Whole numbers from Min to Max, each as likely.
        (attrib::INTEGER, _) => vec![(set.min[0] + (set.max[0] - set.min[0] + 1.0) * r(0)).floor().min(set.max[0])],
        _ => uniform,
    }
}

// A uniformly random direction from three random numbers.
fn loop_free_direction(r: &impl Fn(u32) -> f32) -> [f32; 3] {
    let z = r(6) * 2.0 - 1.0;
    let a = r(7) * std::f32::consts::TAU;
    let s = (1.0 - z * z).max(0.0).sqrt();
    [s * a.cos(), s * a.sin(), z]
}

pub fn randomize(mut mesh: Mesh, set: &RandomizeSettings) -> Result<Mesh, String> {
    if set.preset < CUSTOM {
        return Ok(legacy(mesh, set));
    }
    let name = set.name.trim();
    if !attrib::valid_name(name) {
        return Err(format!("'{name}' isn't an attribute name: use letters, digits and _"));
    }
    let size = attrib::components(set.kind, set.size);
    let (values_for, stored) = attrib::elements(&mesh, set.class);
    let values = mesh.point_attributes.entry(attrib::key(name, set.class)).or_default();
    let fresh = values.len() != stored * size;
    if fresh {
        // A different size or element count: start from nothing (no rotation for quaternions).
        *values = if set.kind == attrib::QUATERNION { [0.0, 0.0, 0.0, 1.0].repeat(stored) } else { vec![0.0; stored * size] };
    }
    for (element, covers) in values_for.into_iter().enumerate() {
        let v = pick(set, element, size);
        for e in covers {
            let slot = &mut values[e * size..(e + 1) * size];
            if set.kind == attrib::QUATERNION {
                let old = [slot[0], slot[1], slot[2], slot[3]];
                let q = [v[0], v[1], v[2], v[3]];
                slot.copy_from_slice(&if set.mode == 0 { q } else { attrib::multiply(q, old) });
                continue;
            }
            for (s, x) in slot.iter_mut().zip(&v) {
                *s = match set.mode {
                    1 => *s + x,
                    // Multiplying nothing yet: start from 1.
                    2 => if fresh { *x } else { *s * x },
                    _ => *x,
                };
            }
        }
    }
    Ok(mesh)
}

// The presets as they behaved before: id and pscale get ±amount added, Cd a random colour,
// orient a random rotation blended in by amount.
fn legacy(mut mesh: Mesh, set: &RandomizeSettings) -> Mesh {
    let points = mesh.vertices.len();
    let seed = set.seed as u32;
    match set.preset {
        2 => {
            let values = mesh.point_attributes.entry("Cd".into()).or_default();
            values.resize(points * 3, 0.0);
            for index in 0..points {
                let value = (hash01(index as u32, seed) + set.amount * 0.5).fract();
                values[index * 3..index * 3 + 3].copy_from_slice(&crate::attribute_color(value));
            }
        }
        3 => {
            let t = set.amount.clamp(0.0, 1.0);
            let values = mesh.point_attributes.entry("orient".into()).or_default();
            values.resize(points * 4, 0.0);
            for index in 0..points {
                let r = |c: u32| hash01(index as u32 * 4 + c, seed ^ 0x6f72_6965);
                values[index * 4..index * 4 + 4].copy_from_slice(&random_rotation(r, t));
            }
        }
        preset => {
            let values = mesh.point_attributes.entry(PRESETS[preset].into()).or_default();
            values.resize(points, 0.0);
            for (index, value) in values.iter_mut().enumerate() {
                *value += (hash01(index as u32, seed) - 0.5) * 2.0 * set.amount;
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Mesh {
        crate::node_grid(2.0, 2.0, 3, 3)
    }

    fn custom(name: &str, kind: usize, size: usize) -> RandomizeSettings {
        RandomizeSettings { name: name.into(), kind, size, ..RandomizeSettings::defaults() }
    }

    #[test]
    fn each_component_has_its_own_range() {
        let set = RandomizeSettings { min: [0.0, 5.0, -2.0, 0.0], max: [1.0, 6.0, -1.0, 0.0], ..custom("wind", attrib::VECTOR, 3) };
        let wind = &randomize(grid(), &set).unwrap().point_attributes["wind"];
        assert_eq!(wind.len(), 9 * 3);
        for v in wind.chunks(3) {
            assert!((0.0..=1.0).contains(&v[0]) && (5.0..=6.0).contains(&v[1]) && (-2.0..=-1.0).contains(&v[2]), "{v:?}");
        }
        // Primitives: one value per polygon, on both its triangles; vertices one per corner.
        let prims = randomize(grid(), &RandomizeSettings { class: 1, ..custom("tag", 0, 1) }).unwrap();
        assert!(prims.point_attributes["prim:tag"].chunks(2).all(|quad| quad[0] == quad[1]));
        let corners = randomize(grid(), &RandomizeSettings { class: 3, ..custom("w", 0, 1) }).unwrap();
        assert_eq!(corners.point_attributes["vertex:w"].len(), 8 * 3);
        assert!(randomize(grid(), &custom("bad name", 0, 1)).is_err());
    }

    #[test]
    fn colours_between_two_colours_and_whole_numbers() {
        // Between: every colour lies on the line from Min to Max.
        let set = RandomizeSettings { distribution: 1, min: [1.0, 0.0, 0.0, 0.0], max: [1.0, 1.0, 0.0, 0.0], ..custom("Cd", attrib::COLOUR, 3) };
        let cd = &randomize(grid(), &set).unwrap().point_attributes["Cd"];
        assert!(cd.chunks(3).all(|c| c[0] == 1.0 && c[2] == 0.0 && (0.0..=1.0).contains(&c[1])));
        assert!(cd.chunks(3).any(|c| c[1] > 0.5) && cd.chunks(3).any(|c| c[1] < 0.5));
        // Integers: whole numbers from Min to Max, both ends included.
        let ids = randomize(grid(), &RandomizeSettings { min: [1.0; 4], max: [3.0; 4], ..custom("variant", attrib::INTEGER, 1) }).unwrap();
        let v = &ids.point_attributes["variant"];
        assert!(v.iter().all(|x| x.fract() == 0.0 && (1.0..=3.0).contains(x)));
        // Directions are unit length (times the length range).
        let dirs = randomize(grid(), &RandomizeSettings { distribution: DIRECTION, min: [2.0; 4], max: [2.0; 4], ..custom("v", attrib::VECTOR, 3) }).unwrap();
        assert!(dirs.point_attributes["v"].chunks(3).all(|d| (d.iter().map(|x| x * x).sum::<f32>().sqrt() - 2.0).abs() < 1e-4));
    }

    #[test]
    fn quaternions_are_rotations() {
        // Uniform on angles: a random turn around Y only.
        let spin = RandomizeSettings { min: [0.0; 4], max: [0.0, 360.0, 0.0, 0.0], ..custom("orient", attrib::QUATERNION, 4) };
        let q = &randomize(grid(), &spin).unwrap().point_attributes["orient"];
        assert!(q.chunks(4).all(|q| q[0].abs() < 1e-6 && q[2].abs() < 1e-6 && (q[1] * q[1] + q[3] * q[3] - 1.0).abs() < 1e-5));
        // Rotation: unit quaternions, none at Amount 0.
        let still = randomize(grid(), &RandomizeSettings { distribution: ROTATION, amount: 0.0, ..custom("orient", attrib::QUATERNION, 4) }).unwrap();
        assert!(still.point_attributes["orient"].chunks(4).all(|q| q == [0.0, 0.0, 0.0, 1.0]));
        // Adding turns what's there: no rotation, then a turn, is that turn.
        let added = randomize(still, &RandomizeSettings { mode: 1, ..spin.clone() }).unwrap();
        assert_eq!(&added.point_attributes["orient"], q);
    }

    #[test]
    fn set_add_and_multiply() {
        let set = RandomizeSettings { min: [2.0; 4], max: [2.0; 4], ..custom("pscale", 0, 1) };
        let once = randomize(grid(), &set).unwrap();
        let plus = randomize(once.clone(), &RandomizeSettings { mode: 1, ..set.clone() }).unwrap();
        let times = randomize(once.clone(), &RandomizeSettings { mode: 2, ..set.clone() }).unwrap();
        assert!(plus.point_attributes["pscale"].iter().all(|v| *v == 4.0));
        assert!(times.point_attributes["pscale"].iter().all(|v| *v == 4.0));
        assert_ne!(randomize(grid(), &RandomizeSettings { seed: 9, max: [3.0; 4], ..set.clone() }).unwrap().point_attributes["pscale"], randomize(grid(), &RandomizeSettings { max: [3.0; 4], ..set }).unwrap().point_attributes["pscale"]);
    }

    #[test]
    fn old_presets_and_saves_keep_working() {
        // Old three-value saves load as the preset, which behaves as before.
        let old = RandomizeSettings::from_params(&[1.0, 0.25, 7.0]);
        assert_eq!((old.preset, old.seed), (1, 7));
        let ps = randomize(grid(), &old).unwrap();
        assert!(ps.point_attributes["pscale"].iter().all(|v| v.abs() <= 0.25));
        // Converting to the new settings gives the same values.
        let converted = randomize(grid(), &old.converted()).unwrap();
        assert_eq!(old.converted().preset, CUSTOM);
        assert_eq!(converted.point_attributes["pscale"].len(), 9);
        let orient = RandomizeSettings::from_params(&[3.0, 1.0, 1.0]);
        let q = randomize(grid(), &orient).unwrap();
        assert!(q.point_attributes["orient"].chunks(4).all(|q| (q.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4));
        // Old custom saves: one range for every component.
        let eight = RandomizeSettings::from_params(&[4.0, 0.0, 1.0, 1.0, 3.0, -1.0, 4.0, 1.0]);
        assert_eq!((eight.kind, eight.min, eight.max, eight.mode), (0, [-1.0; 4], [4.0; 4], 1));
        // New settings round-trip (the name travels as text).
        let s = RandomizeSettings { name: "rand".into(), kind: attrib::VECTOR, distribution: DIRECTION, min: [1.0, 2.0, 3.0, 4.0], max: [5.0, 6.0, 7.0, 8.0], class: 3, mode: 2, ..RandomizeSettings::defaults() };
        assert_eq!(RandomizeSettings::from_params(&s.to_params()), s);
    }
}
