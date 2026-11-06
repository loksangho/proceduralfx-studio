// Attribute Remap: reshapes a point or primitive attribute's values through a ramp. Each
// value is placed in its input range (or the attribute's own range, from its lowest to its
// highest value) as 0-1, looked up on the ramp, and set to that much of the way through
// the output range. Vectors and colours have each component remapped.

use super::Mesh;
use crate::ramp::Ramp;

pub const CLASSES: [&str; 2] = ["Points", "Primitives"];
pub const DEFAULT_RAMP: &str = "0 0 linear; 1 1 linear";

#[derive(Clone, Debug, PartialEq)]
pub struct RemapSettings {
    pub attribute: String,
    pub class: usize,
    // On: the input range is the attribute's own lowest to highest value.
    pub auto_range: bool,
    pub in_range: [f32; 2],
    pub out_range: [f32; 2],
    pub ramp: String,
}

impl RemapSettings {
    pub fn defaults() -> RemapSettings {
        RemapSettings { attribute: "pscale".into(), class: 0, auto_range: true, in_range: [0.0, 1.0], out_range: [0.0, 1.0], ramp: DEFAULT_RAMP.into() }
    }

    pub fn params(&self) -> Vec<f32> {
        vec![self.class as f32, self.auto_range as u8 as f32, self.in_range[0], self.in_range[1], self.out_range[0], self.out_range[1]]
    }

    pub fn from_params(p: &[f32]) -> RemapSettings {
        let d = RemapSettings::defaults();
        let g = |i: usize, v: f32| p.get(i).copied().unwrap_or(v);
        RemapSettings {
            class: (g(0, 0.0).max(0.0) as usize).min(1),
            auto_range: g(1, 1.0) > 0.5,
            in_range: [g(2, 0.0), g(3, 1.0)],
            out_range: [g(4, 0.0), g(5, 1.0)],
            ..d
        }
    }
}

pub fn remap(input: &Mesh, set: &RemapSettings) -> Result<Mesh, String> {
    let name = set.attribute.trim();
    if name.is_empty() {
        return Err("Attribute Remap needs an attribute's name".into());
    }
    let (key, count) = if set.class == 1 { (format!("prim:{name}"), input.indices.len()) } else { (name.to_owned(), input.vertices.len()) };
    let values = input.point_attributes.get(&key).filter(|v| count > 0 && !v.is_empty() && v.len() % count == 0);
    let Some(values) = values else {
        let what = if set.class == 1 { "primitive" } else { "point" };
        return Err(format!("there's no {what} attribute called {name} to remap"));
    };
    if crate::strings::is_string_key(&key) {
        return Err(format!("{name} is text, which can't be remapped"));
    }
    let ramp = Ramp::from_text(&set.ramp, 1, || Ramp::parse(DEFAULT_RAMP, 1).unwrap());
    let [lo, hi] = if set.auto_range {
        let (lo, hi) = values.iter().filter(|v| v.is_finite()).fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        if lo > hi { [0.0, 1.0] } else { [lo, hi] }
    } else {
        set.in_range
    };
    let span = hi - lo;
    let [out_lo, out_hi] = set.out_range;
    let mapped: Vec<f32> = values
        .iter()
        .map(|v| {
            let t = if span.abs() > 1e-12 { ((v - lo) / span).clamp(0.0, 1.0) } else { 0.0 };
            out_lo + ramp.eval1(t) * (out_hi - out_lo)
        })
        .collect();
    let mut out = input.clone();
    out.point_attributes.insert(key, mapped);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec3;

    #[test]
    fn remaps_through_the_ramp_and_ranges() {
        let mut m = Mesh { vertices: vec![Vec3::new(0.0, 0.0, 0.0); 3], ..Mesh::default() };
        m.point_attributes.insert("pscale".into(), vec![2.0, 3.0, 4.0]);
        // Its own range, 2-4, onto 0-10 through a ramp that goes up and back down.
        let set = RemapSettings { out_range: [0.0, 10.0], ramp: "0 0 linear; 0.5 1 linear; 1 0 linear".into(), ..RemapSettings::defaults() };
        assert_eq!(remap(&m, &set).unwrap().point_attributes["pscale"], vec![0.0, 10.0, 0.0]);
        // A given range clamps values outside it.
        let set = RemapSettings { auto_range: false, in_range: [0.0, 2.0], ..RemapSettings::defaults() };
        assert_eq!(remap(&m, &set).unwrap().point_attributes["pscale"], vec![1.0, 1.0, 1.0]);
        // Each component of a vector.
        m.point_attributes.insert("Cd".into(), vec![0.0, 0.5, 1.0, 1.0, 0.5, 0.0, 0.25, 0.25, 0.25]);
        let flip = RemapSettings { attribute: "Cd".into(), ramp: "0 1 linear; 1 0 linear".into(), auto_range: false, ..RemapSettings::defaults() };
        assert_eq!(remap(&m, &flip).unwrap().point_attributes["Cd"][..3], [1.0, 0.5, 0.0]);
        assert!(remap(&m, &RemapSettings { attribute: "nope".into(), ..RemapSettings::defaults() }).is_err());
    }
}
