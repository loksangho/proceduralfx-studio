// Attribute Fade: scales a point attribute in, holds it, then scales it out over time, as
// Houdini's Attribute Fade does: to start effects one after another, grow a fuel map, or
// fade particles in and out.
//
// Each point starts at its Start frame attribute (from input 2 when it's connected, else
// the geometry's own; 0 when it has none) plus the Frame offset. From there the attribute
// rises over Fade in, through the fade-in ramp, stays whole for Hold (times the point's
// Hold scale attribute, from input 3 when connected), then falls over Fade out through the
// fade-out ramp, and stays at 0. The attribute's own value is what's scaled: 1 if the
// geometry doesn't have it yet. Times are frames ($F, from 1), or seconds.

use super::Mesh;
use crate::ramp::Ramp;

pub const FADE_IN_RAMP: &str = "0 0 linear; 1 1 linear";
pub const FADE_OUT_RAMP: &str = "0 1 linear; 1 0 linear";

#[derive(Clone, Debug, PartialEq)]
pub struct FadeSettings {
    pub attribute: String,
    pub start_attribute: String,
    pub hold_attribute: String,
    pub offset: f32,
    pub fade_in: f32,
    pub hold: f32,
    pub fade_out: f32,
    // Times in seconds instead of frames.
    pub seconds: bool,
    // Cd shows the fade (white faded in, black faded out).
    pub visualize: bool,
    pub in_ramp: String,
    pub out_ramp: String,
}

impl FadeSettings {
    pub fn defaults() -> FadeSettings {
        FadeSettings {
            attribute: "fade".into(),
            start_attribute: "fadestart".into(),
            hold_attribute: "holdscale".into(),
            offset: 0.0,
            fade_in: 12.0,
            hold: 24.0,
            fade_out: 12.0,
            seconds: false,
            visualize: false,
            in_ramp: FADE_IN_RAMP.into(),
            out_ramp: FADE_OUT_RAMP.into(),
        }
    }

    pub fn params(&self) -> Vec<f32> {
        vec![self.offset, self.fade_in, self.hold, self.fade_out, self.seconds as u8 as f32, self.visualize as u8 as f32]
    }

    pub fn from_params(p: &[f32]) -> FadeSettings {
        let d = FadeSettings::defaults();
        let g = |i: usize, v: f32| p.get(i).copied().unwrap_or(v);
        FadeSettings {
            offset: g(0, d.offset),
            fade_in: g(1, d.fade_in).max(0.0),
            hold: g(2, d.hold).max(0.0),
            fade_out: g(3, d.fade_out).max(0.0),
            seconds: g(4, 0.0) > 0.5,
            visualize: g(5, 0.0) > 0.5,
            ..d
        }
    }

    pub fn texts(&self) -> Vec<String> {
        vec![self.attribute.clone(), self.start_attribute.clone(), self.hold_attribute.clone(), self.in_ramp.clone(), self.out_ramp.clone()]
    }

    // How much of the attribute shows `local` frames (or seconds) after a point's start.
    pub fn amount(&self, local: f32, hold_scale: f32, in_ramp: &Ramp, out_ramp: &Ramp) -> f32 {
        let hold = self.hold * hold_scale.max(0.0);
        if local < 0.0 {
            0.0
        } else if local < self.fade_in {
            in_ramp.eval1(local / self.fade_in)
        } else if local < self.fade_in + hold {
            1.0
        } else if local < self.fade_in + hold + self.fade_out {
            out_ramp.eval1((local - self.fade_in - hold) / self.fade_out)
        } else {
            0.0
        }
    }
}

// A point attribute's first component on every point, from `from` when it has it.
fn per_point(from: Option<&Mesh>, own: &Mesh, name: &str) -> Option<Vec<f32>> {
    let n = own.vertices.len();
    let read = |m: &Mesh| {
        let v = m.point_attributes.get(name.trim()).filter(|v| n > 0 && v.len() >= n && v.len() % m.vertices.len().max(1) == 0)?;
        let w = v.len() / m.vertices.len().max(1);
        (m.vertices.len() == n).then(|| (0..n).map(|i| v[i * w]).collect())
    };
    from.and_then(read).or_else(|| read(own))
}

pub fn fade(input: &Mesh, starts: Option<&Mesh>, holds: Option<&Mesh>, time: f32, set: &FadeSettings) -> Result<Mesh, String> {
    let name = set.attribute.trim();
    if name.is_empty() {
        return Err("Attribute Fade needs the name of the attribute to fade".into());
    }
    if name.contains(':') || name == "P" {
        return Err(format!("{name} can't be faded: choose a point attribute such as fade, Alpha or pscale"));
    }
    let n = input.vertices.len();
    let now = if set.seconds { time } else { time * crate::sims::SIM_FPS + 1.0 };
    let start = per_point(starts, input, &set.start_attribute);
    let hold = per_point(holds, input, &set.hold_attribute);
    let in_ramp = Ramp::from_text(&set.in_ramp, 1, || Ramp::parse(FADE_IN_RAMP, 1).unwrap());
    let out_ramp = Ramp::from_text(&set.out_ramp, 1, || Ramp::parse(FADE_OUT_RAMP, 1).unwrap());
    let amounts: Vec<f32> = (0..n)
        .map(|i| {
            let begins = start.as_ref().map_or(0.0, |s| s[i]) + set.offset;
            set.amount(now - begins, hold.as_ref().map_or(1.0, |h| h[i]), &in_ramp, &out_ramp)
        })
        .collect();
    let mut out = input.clone();
    let values = match input.point_attributes.get(name).filter(|v| n > 0 && v.len() % n == 0 && !v.is_empty()) {
        Some(v) => {
            let w = v.len() / n;
            v.iter().enumerate().map(|(k, x)| x * amounts[k / w]).collect()
        }
        None => amounts.clone(),
    };
    out.point_attributes.insert(name.to_owned(), values);
    if set.visualize {
        out.point_attributes.insert("Cd".into(), amounts.iter().flat_map(|a| [*a; 3]).collect());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec3;

    #[test]
    fn points_fade_in_hold_and_fade_out_from_their_start_frames() {
        // Two points starting at frames 1 and 11; 10 frames in, 10 held, 10 out.
        let mut m = Mesh { vertices: vec![Vec3::zero(); 2], ..Mesh::default() };
        m.point_attributes.insert("fadestart".into(), vec![1.0, 11.0]);
        let set = FadeSettings { fade_in: 10.0, hold: 10.0, fade_out: 10.0, ..FadeSettings::defaults() };
        let at = |frame: f32| fade(&m, None, None, (frame - 1.0) / crate::sims::SIM_FPS, &set).unwrap().point_attributes["fade"].clone();
        let close = |a: Vec<f32>, b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(close(at(1.0), [0.0, 0.0]));
        assert!(close(at(6.0), [0.5, 0.0]), "{:?}", at(6.0));
        assert!(close(at(16.0), [1.0, 0.5]));
        assert!(close(at(26.0), [0.5, 1.0]), "fading out");
        assert!(close(at(40.0), [0.0, 0.1]));
        assert!(close(at(60.0), [0.0, 0.0]));
        // The attribute's own value is what fades; hold scale stretches the hold.
        m.point_attributes.insert("Alpha".into(), vec![0.8, 0.8]);
        m.point_attributes.insert("holdscale".into(), vec![1.0, 3.0]);
        let alpha = FadeSettings { attribute: "Alpha".into(), visualize: true, ..set.clone() };
        let out = fade(&m, None, None, (41.0 - 1.0) / crate::sims::SIM_FPS, &alpha).unwrap();
        assert_eq!(out.point_attributes["Alpha"], vec![0.0, 0.8], "the second holds three times longer");
        assert_eq!(out.point_attributes["Cd"], vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        // In seconds, with start frames from another input.
        let mut starts = m.clone();
        starts.point_attributes.insert("fadestart".into(), vec![0.0, 1.0]);
        let secs = FadeSettings { seconds: true, fade_in: 1.0, hold: 1.0, fade_out: 1.0, ..FadeSettings::defaults() };
        assert_eq!(fade(&m, Some(&starts), None, 0.5, &secs).unwrap().point_attributes["fade"], vec![0.5, 0.0]);
        assert!(fade(&m, None, None, 0.0, &FadeSettings { attribute: "P".into(), ..set }).is_err());
    }
}
