// Ramps: a value (a float, or a colour) that changes along 0-1, set by keys. Each key has
// a position, a value and how it leads into the next key: held (Constant), a straight line
// (Linear), a smooth curve through the keys (Catmull-Rom), a smooth curve that never
// overshoots them (Monotone Cubic), or a smoother curve that passes near them (B-Spline).
//
// Nodes keep a ramp as text, one key after another: position, value (one number, or three
// for a colour) and interpolation, separated by semicolons:
//   "0 1 linear; 0.5 0.2 catmull-rom; 1 0 linear"
//   "0 1 0.5 0.1 linear; 1 0.2 0.2 0.2 linear"
// Empty text means the node's own fallback (often a ramp from its two colours).

pub const INTERPS: [&str; 5] = ["Constant", "Linear", "Catmull-Rom", "Monotone Cubic", "B-Spline"];
const NAMES: [&str; 5] = ["constant", "linear", "catmull-rom", "monotone", "bspline"];

pub const CONSTANT: usize = 0;
pub const LINEAR: usize = 1;
pub const CATMULL_ROM: usize = 2;
pub const MONOTONE: usize = 3;
pub const BSPLINE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub pos: f32,
    // A float ramp uses value[0] only.
    pub value: [f32; 3],
    pub interp: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ramp {
    // In order of position; never empty.
    pub keys: Vec<Key>,
}

impl Ramp {
    // From `a` at 0 to `b` at 1, linearly.
    pub fn linear(a: [f32; 3], b: [f32; 3]) -> Ramp {
        Ramp { keys: vec![Key { pos: 0.0, value: a, interp: LINEAR }, Key { pos: 1.0, value: b, interp: LINEAR }] }
    }

    pub fn flat(v: f32) -> Ramp {
        Ramp::linear([v; 3], [v; 3])
    }

    // Reads a ramp's text (see the top of this file); None when it's empty or has no keys.
    // `width` is 1 for a float ramp, 3 for a colour.
    pub fn parse(text: &str, width: usize) -> Option<Ramp> {
        let mut keys = Vec::new();
        for part in text.split(';') {
            let words: Vec<&str> = part.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '(' || ch == ')').filter(|w| !w.is_empty()).collect();
            if words.is_empty() {
                continue;
            }
            let numbers: Vec<f32> = words.iter().map_while(|w| w.parse::<f32>().ok()).collect();
            if numbers.len() < 2 {
                continue;
            }
            let value = if width >= 3 && numbers.len() >= 4 { [numbers[1], numbers[2], numbers[3]] } else { [numbers[1]; 3] };
            let interp = words.get(numbers.len()).map_or(LINEAR, |w| interp_named(w));
            keys.push(Key { pos: numbers[0].clamp(0.0, 1.0), value, interp });
        }
        if keys.is_empty() {
            return None;
        }
        let mut ramp = Ramp { keys };
        ramp.sort();
        Some(ramp)
    }

    // The ramp in `text`, or `fallback` when the text is empty.
    pub fn from_text(text: &str, width: usize, fallback: impl FnOnce() -> Ramp) -> Ramp {
        Ramp::parse(text, width).unwrap_or_else(fallback)
    }

    pub fn format(&self, width: usize) -> String {
        let number = |v: f32| {
            let s = format!("{:.4}", v);
            let s = s.trim_end_matches('0').trim_end_matches('.').to_owned();
            if s == "-0" { "0".to_owned() } else { s }
        };
        self.keys
            .iter()
            .map(|k| {
                let value = if width >= 3 { k.value.iter().map(|v| number(*v)).collect::<Vec<_>>().join(" ") } else { number(k.value[0]) };
                format!("{} {} {}", number(k.pos), value, NAMES[k.interp.min(NAMES.len() - 1)])
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn sort(&mut self) {
        self.keys.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    }

    pub fn eval1(&self, t: f32) -> f32 {
        self.eval_component(t, 0)
    }

    pub fn eval(&self, t: f32) -> [f32; 3] {
        [0, 1, 2].map(|c| self.eval_component(t, c))
    }

    fn eval_component(&self, t: f32, c: usize) -> f32 {
        let keys = &self.keys;
        let Some(first) = keys.first() else { return 0.0 };
        let last = keys[keys.len() - 1];
        if !t.is_finite() || t <= first.pos || keys.len() == 1 {
            return first.value[c];
        }
        if t >= last.pos {
            return last.value[c];
        }
        // The segment t is in: keys i and i + 1.
        let i = keys.partition_point(|k| k.pos <= t).saturating_sub(1).min(keys.len() - 2);
        let (k0, k1) = (keys[i], keys[i + 1]);
        let h = k1.pos - k0.pos;
        if h <= 1e-9 {
            return k1.value[c];
        }
        let u = ((t - k0.pos) / h).clamp(0.0, 1.0);
        let (v0, v1) = (k0.value[c], k1.value[c]);
        // Neighbours past the ends repeat the end key.
        let before = if i > 0 { keys[i - 1] } else { k0 };
        let after = if i + 2 < keys.len() { keys[i + 2] } else { k1 };
        match k0.interp {
            CONSTANT => v0,
            CATMULL_ROM => {
                // Tangents from the neighbours, for keys spaced unevenly.
                let slope = |a: Key, b: Key| if b.pos - a.pos > 1e-9 { (b.value[c] - a.value[c]) / (b.pos - a.pos) } else { 0.0 };
                let m0 = if i > 0 { slope(before, k1) } else { (v1 - v0) / h };
                let m1 = if i + 2 < keys.len() { slope(k0, after) } else { (v1 - v0) / h };
                hermite(v0, v1, m0 * h, m1 * h, u)
            }
            MONOTONE => {
                // Fritsch-Carlson: tangents that keep each segment between its keys' values.
                let secant = |a: Key, b: Key| if b.pos - a.pos > 1e-9 { (b.value[c] - a.value[c]) / (b.pos - a.pos) } else { 0.0 };
                let d = secant(k0, k1);
                let tangent = |left: Option<f32>, right: Option<f32>| -> f32 {
                    match (left, right) {
                        (Some(l), Some(r)) if l * r > 0.0 => {
                            let m = 0.5 * (l + r);
                            m.signum() * m.abs().min(3.0 * l.abs()).min(3.0 * r.abs())
                        }
                        (Some(_), Some(_)) => 0.0,
                        (Some(s), None) | (None, Some(s)) => s,
                        (None, None) => 0.0,
                    }
                };
                let m0 = tangent((i > 0).then(|| secant(before, k0)), Some(d));
                let m1 = tangent(Some(d), (i + 2 < keys.len()).then(|| secant(k1, after)));
                hermite(v0, v1, m0 * h, m1 * h, u)
            }
            BSPLINE => {
                // A uniform cubic B-spline with the keys' values as its control points.
                let (p0, p3) = (before.value[c], after.value[c]);
                let (u2, u3) = (u * u, u * u * u);
                let b0 = (1.0 - u).powi(3) / 6.0;
                let b1 = (3.0 * u3 - 6.0 * u2 + 4.0) / 6.0;
                let b2 = (-3.0 * u3 + 3.0 * u2 + 3.0 * u + 1.0) / 6.0;
                let b3 = u3 / 6.0;
                b0 * p0 + b1 * v0 + b2 * v1 + b3 * p3
            }
            _ => v0 + (v1 - v0) * u,
        }
    }
}

fn hermite(v0: f32, v1: f32, m0: f32, m1: f32, u: f32) -> f32 {
    let (u2, u3) = (u * u, u * u * u);
    (2.0 * u3 - 3.0 * u2 + 1.0) * v0 + (u3 - 2.0 * u2 + u) * m0 + (-2.0 * u3 + 3.0 * u2) * v1 + (u3 - u2) * m1
}

fn interp_named(word: &str) -> usize {
    match word.to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
        "constant" | "step" | "hold" => CONSTANT,
        "catmull-rom" | "catmullrom" | "spline" | "smooth" => CATMULL_ROM,
        "monotone" | "monotone-cubic" | "monotonecubic" | "monotonic" => MONOTONE,
        "bspline" | "b-spline" => BSPLINE,
        _ => LINEAR,
    }
}

// A ramp parameter of a node: its key in the node's texts, label, whether it's a colour,
// and its default text.
#[derive(Clone, Copy, Debug)]
pub struct RampParam {
    pub key: &'static str,
    pub label: &'static str,
    pub color: bool,
    pub default: &'static str,
}

impl RampParam {
    pub fn width(&self) -> usize {
        if self.color { 3 } else { 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_read_write_and_interpolate() {
        let ramp = Ramp::parse("1 0 linear; 0 1 linear", 1).unwrap();
        assert_eq!(ramp.keys[0].pos, 0.0, "keys are put in order");
        assert!((ramp.eval1(0.25) - 0.75).abs() < 1e-6);
        assert_eq!(ramp.eval1(-1.0), 1.0);
        assert_eq!(ramp.eval1(2.0), 0.0);
        assert_eq!(Ramp::parse(&ramp.format(1), 1).unwrap(), ramp, "text round-trips");
        assert!(Ramp::parse("", 1).is_none() && Ramp::parse("nonsense", 1).is_none());

        // Constant holds a key's value up to the next key.
        let steps = Ramp::parse("0 0 constant; 0.5 1 constant; 1 2 constant", 1).unwrap();
        assert_eq!((steps.eval1(0.49), steps.eval1(0.5), steps.eval1(0.99)), (0.0, 1.0, 1.0));

        // Catmull-Rom passes through the keys, smoothly; Monotone never overshoots them.
        let keys = "0 0 {}; 0.3 1 {}; 0.6 1 {}; 1 0 {}";
        let with = |name: &str| Ramp::parse(&keys.replace("{}", name), 1).unwrap();
        let smooth = with("catmull-rom");
        assert!((smooth.eval1(0.3) - 1.0).abs() < 1e-5 && (smooth.eval1(0.6) - 1.0).abs() < 1e-5);
        assert!(smooth.eval1(0.45) > 1.0, "Catmull-Rom bulges between equal keys");
        let monotone = with("monotone");
        assert!((0..=100).all(|k| monotone.eval1(k as f32 / 100.0) <= 1.0 + 1e-5));
        assert!(monotone.eval1(0.15) > 0.3 && monotone.eval1(0.15) < 0.7);
        // B-Spline stays within the keys and is smooth, but doesn't pass through the middle ones.
        let b = with("bspline");
        assert!((0..=100).all(|k| (-1e-5..=1.0 + 1e-5).contains(&b.eval1(k as f32 / 100.0))));
        assert!(b.eval1(0.3) < 0.99);

        // Colours: three numbers a key.
        let colour = Ramp::parse("0 1 0 0 linear; 1 0 0 1 linear", 3).unwrap();
        assert_eq!(colour.eval(0.5), [0.5, 0.0, 0.5]);
        assert_eq!(Ramp::parse(&colour.format(3), 3).unwrap(), colour);
        assert_eq!(Ramp::parse("0 0.5; 1 1", 3).unwrap().keys[0].value, [0.5; 3], "one number is a grey");
    }
}
