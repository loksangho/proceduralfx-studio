// L-system: a premise rewritten by rules for a number of generations, then drawn by
// a 3D turtle as curves (runs of points with `curveu` and `width`, like groom curves),
// so Resample, Sweep-style tools, Copy to Points and the renderer all work on it.
//
// Turtle symbols (anything else is only rewritten):
//   F G    move forward and draw        f g   move forward without drawing
//   + -    turn left / right            & ^   pitch down / up
//   \ /    roll left / right            |     turn around
//   [ ]    start / end a branch         !     thicker → thinner (× thickness scale)
//   "      shorter steps (× step scale)
// A number in brackets after a symbol overrides its amount: F(0.5) steps 0.5,
// +(45) turns 45°, !(0.8) scales the width by 0.8.
//
// Rules are one per line, `A=F[+A][-A]` (or `A->…`); `#` starts a comment. Several
// rules for the same symbol pick one at random each time (seeded); `:0.3` at the
// end of a rule weights it.

use super::{hash01, Mesh, Vec3};
use std::collections::HashMap;

const MAX_SYMBOLS: usize = 2_000_000;

pub const SYMBOL_HELP: &str = "F G   forward, drawing        f g   forward, not drawing
+ -   turn left / right         & ^   pitch down / up
\\ /   roll left / right         |     turn around
[ ]   branch start / end        !     thinner (× Thickness Scale)
\"     shorter (× Step Scale)
F(0.5)  +(45)  !(0.8): an amount overrides the default
Other letters (A, X…) are only rewritten.
Rules: one per line, A=F[+A][-A]; several for one symbol pick at
random (Seed); :0.3 at the end of a rule weights it.";

// Each fork: a shorter, thinner segment, then three branches rolled around it.
const TREE_RULES: &str = "A=!\"F[&A]/////[&A]///////[&A]";

// (label, premise, rules, generations, angle, step) for the Presets menu.
pub const PRESETS: &[(&str, &str, &str, usize, f32, f32)] = &[
    ("3D tree", "FA", TREE_RULES, 6, 28.0, 0.6),
    ("Fractal plant", "X", "X=F+[[X]-X]-F[-FX]+X\nF=FF", 5, 25.0, 0.05),
    ("Grass (random)", "F", "F=F[+F]F[-F]F\nF=F[+F]F\nF=F[-F]F", 5, 25.7, 0.03),
    ("Koch curve", "F", "F=F+F-F-F+F", 4, 90.0, 0.05),
    ("Hilbert curve (3D)", "A", "A=B-F+CFC+F-D&F^D-F+&&CFC+F+B//\nB=A&F^CFB^F^D^^-F-D^|F^B|FC^F^A//\nC=|D^|F^B-F+C^F^A&&FA&F^C+F+B^F^D//\nD=|CFB-F+B|FA&F^A&&FB-F+B|FC//", 2, 90.0, 0.25),
];

#[derive(Clone, Debug, PartialEq)]
pub struct LSystemSettings {
    pub premise: String,
    pub rules: String,
    pub generations: usize,
    // Degrees per turn.
    pub angle: f32,
    pub step: f32,
    pub step_scale: f32,
    // Starting curve width.
    pub thickness: f32,
    pub thickness_scale: f32,
    // Random variation of each turn, in degrees.
    pub angle_random: f32,
    pub seed: u32,
}

impl LSystemSettings {
    pub fn defaults() -> Self {
        // A 3D tree: three branches per fork, rolled around the stem, thinning out.
        LSystemSettings {
            premise: "FA".into(),
            rules: TREE_RULES.into(),
            generations: 6,
            angle: 28.0,
            step: 0.6,
            step_scale: 0.78,
            thickness: 0.06,
            thickness_scale: 0.7,
            angle_random: 0.0,
            seed: 1,
        }
    }

    pub fn to_params(&self) -> Vec<f32> {
        vec![self.generations as f32, self.angle, self.step, self.step_scale, self.thickness, self.thickness_scale, self.angle_random, self.seed as f32]
    }

    pub fn from_params(p: &[f32]) -> Self {
        let d = Self::defaults();
        let g = |i: usize, default: f32| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(default);
        LSystemSettings {
            premise: d.premise,
            rules: d.rules,
            generations: g(0, d.generations as f32).clamp(0.0, 16.0) as usize,
            angle: g(1, d.angle),
            step: g(2, d.step).max(0.0),
            step_scale: g(3, d.step_scale).max(0.0),
            thickness: g(4, d.thickness).max(0.0),
            thickness_scale: g(5, d.thickness_scale).max(0.0),
            angle_random: g(6, d.angle_random).max(0.0),
            seed: g(7, d.seed as f32).max(0.0) as u32,
        }
    }
}

// A symbol and its optional amount, e.g. F or F(0.5).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Symbol {
    ch: char,
    amount: Option<f32>,
}

fn parse(text: &str) -> Result<Vec<Symbol>, String> {
    let mut out = Vec::new();
    let mut chars = text.chars().filter(|c| !c.is_whitespace()).peekable();
    while let Some(ch) = chars.next() {
        let mut amount = None;
        if chars.peek() == Some(&'(') {
            chars.next();
            let number: String = chars.by_ref().take_while(|c| *c != ')').collect();
            amount = Some(number.trim().parse::<f32>().map_err(|_| format!("{ch}({number}): the amount must be a number"))?);
        }
        out.push(Symbol { ch, amount });
    }
    Ok(out)
}

struct Rule {
    successor: Vec<Symbol>,
    weight: f32,
}

fn parse_rules(text: &str) -> Result<HashMap<char, Vec<Rule>>, String> {
    let mut rules: HashMap<char, Vec<Rule>> = HashMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (left, right) = line
            .split_once("->")
            .or_else(|| line.split_once('='))
            .ok_or_else(|| format!("rule {}: write it as A=… (the symbol, =, what it becomes)", number + 1))?;
        let mut predecessor = left.trim().chars();
        let (Some(symbol), None) = (predecessor.next(), predecessor.next()) else {
            return Err(format!("rule {}: the left side must be one symbol, got \"{}\"", number + 1, left.trim()));
        };
        // An optional weight at the end: A=F[+A]:0.3
        let (body, weight) = match right.rsplit_once(':') {
            Some((body, w)) if w.trim().parse::<f32>().is_ok() => (body, w.trim().parse::<f32>().unwrap_or(1.0).max(0.0)),
            _ => (right, 1.0),
        };
        rules.entry(symbol).or_default().push(Rule { successor: parse(body)?, weight });
    }
    Ok(rules)
}

// The symbol string after `generations` rewrites.
fn rewrite(set: &LSystemSettings) -> Result<Vec<Symbol>, String> {
    let rules = parse_rules(&set.rules)?;
    let mut current = parse(&set.premise)?;
    if current.is_empty() {
        return Err("the premise is empty: it's the symbols the L-system starts from, e.g. F or A".into());
    }
    let mut counter = 0u32;
    for generation in 0..set.generations {
        let mut next = Vec::with_capacity(current.len() * 2);
        for s in &current {
            // Symbols with an amount are drawn as written, never rewritten.
            match rules.get(&s.ch).filter(|_| s.amount.is_none()) {
                Some(choices) => {
                    let rule = if choices.len() == 1 {
                        &choices[0]
                    } else {
                        counter = counter.wrapping_add(1);
                        let total: f32 = choices.iter().map(|r| r.weight).sum();
                        let mut pick = hash01(counter ^ ((generation as u32) << 24), set.seed.wrapping_mul(7919)) * total;
                        choices.iter().find(|r| {
                            pick -= r.weight;
                            pick <= 0.0
                        }).unwrap_or(&choices[choices.len() - 1])
                    };
                    next.extend_from_slice(&rule.successor);
                }
                None => next.push(*s),
            }
            if next.len() > MAX_SYMBOLS {
                return Err(format!("generation {} has over {MAX_SYMBOLS} symbols: lower Generations", generation + 1));
            }
        }
        current = next;
    }
    Ok(current)
}

fn rotate(v: Vec3, axis: Vec3, degrees: f32) -> Vec3 {
    let (s, c) = degrees.to_radians().sin_cos();
    v.scale(c).add(axis.cross(v).scale(s)).add(axis.scale(axis.dot(v) * (1.0 - c)))
}

#[derive(Clone, Copy)]
struct Turtle {
    pos: Vec3,
    heading: Vec3,
    left: Vec3,
    up: Vec3,
    step: f32,
    width: f32,
    // The curve being drawn; its last point is `pos`.
    stroke: Option<usize>,
}

// Runs the L-system and draws it.
pub fn generate(set: &LSystemSettings) -> Result<Mesh, String> {
    let symbols = rewrite(set)?;
    // Grows up +Y.
    let mut t = Turtle {
        pos: Vec3::zero(),
        heading: Vec3::new(0.0, 1.0, 0.0),
        left: Vec3::new(-1.0, 0.0, 0.0),
        up: Vec3::new(0.0, 0.0, 1.0),
        step: set.step,
        width: set.thickness,
        stroke: None,
    };
    let mut stack: Vec<Turtle> = Vec::new();
    let mut strokes: Vec<(Vec<Vec3>, Vec<f32>)> = Vec::new();
    let mut turns = 0u32;
    let mut jitter = || {
        turns = turns.wrapping_add(1);
        if set.angle_random > 0.0 { (hash01(turns, set.seed.wrapping_mul(104729)) * 2.0 - 1.0) * set.angle_random } else { 0.0 }
    };
    for s in &symbols {
        let angle = s.amount.unwrap_or(set.angle);
        match s.ch {
            'F' | 'G' => {
                let index = match t.stroke {
                    Some(i) => i,
                    None => {
                        strokes.push((vec![t.pos], vec![t.width]));
                        strokes.len() - 1
                    }
                };
                t.pos = t.pos.add(t.heading.scale(s.amount.unwrap_or(t.step)));
                strokes[index].0.push(t.pos);
                strokes[index].1.push(t.width);
                t.stroke = Some(index);
            }
            'f' | 'g' => {
                t.pos = t.pos.add(t.heading.scale(s.amount.unwrap_or(t.step)));
                t.stroke = None;
            }
            '+' | '-' => {
                let a = if s.ch == '+' { angle } else { -angle } + jitter();
                t.heading = rotate(t.heading, t.up, a);
                t.left = rotate(t.left, t.up, a);
            }
            '&' | '^' => {
                let a = if s.ch == '&' { angle } else { -angle } + jitter();
                t.heading = rotate(t.heading, t.left, a);
                t.up = rotate(t.up, t.left, a);
            }
            '\\' | '/' => {
                let a = if s.ch == '\\' { angle } else { -angle } + jitter();
                t.left = rotate(t.left, t.heading, a);
                t.up = rotate(t.up, t.heading, a);
            }
            '|' => {
                t.heading = t.heading.scale(-1.0);
                t.left = t.left.scale(-1.0);
            }
            '[' => {
                stack.push(t);
                // The branch is a curve of its own, starting where the turtle is.
                t.stroke = None;
            }
            ']' => t = stack.pop().ok_or("a ] has no matching [")?,
            '!' => t.width *= s.amount.unwrap_or(set.thickness_scale),
            '"' => t.step *= s.amount.unwrap_or(set.step_scale),
            _ => {}
        }
    }
    let mut mesh = Mesh::default();
    let (mut curveu, mut width) = (Vec::new(), Vec::new());
    for (points, widths) in strokes.into_iter().filter(|(p, _)| p.len() >= 2) {
        let mut lengths = vec![0.0f32];
        for w in points.windows(2) {
            lengths.push(lengths.last().unwrap() + w[1].sub(w[0]).length());
        }
        let total = lengths.last().copied().unwrap_or(0.0).max(1e-9);
        // curveu starts each curve at 0; the first point of a curve must be exactly 0.
        curveu.extend(lengths.iter().enumerate().map(|(i, l)| if i == 0 { 0.0 } else { (l / total).max(1e-6) }));
        width.extend(widths);
        mesh.vertices.extend(points);
    }
    if mesh.vertices.is_empty() {
        return Err("nothing was drawn: the result has no F or G (the symbols that draw)".into());
    }
    mesh.point_attributes.insert(crate::groom::CURVEU.into(), curveu);
    mesh.point_attributes.insert("width".into(), width);
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(premise: &str, rules: &str, generations: usize) -> LSystemSettings {
        LSystemSettings { premise: premise.into(), rules: rules.into(), generations, angle: 90.0, step: 1.0, ..LSystemSettings::defaults() }
    }

    #[test]
    fn rewrites_and_draws_one_curve() {
        let mesh = generate(&with("F", "F=F+F-F-F+F", 1)).unwrap();
        assert_eq!(mesh.vertices.len(), 6);
        assert_eq!(crate::groom::curves(&mesh).len(), 1);
        // Up, left, up, right, up: the Koch step ends three steps above the start.
        let last = mesh.vertices[5];
        assert!(last.sub(Vec3::new(0.0, 3.0, 0.0)).length() < 1e-4, "{last:?}");
        assert!((mesh.vertices[1].y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn branches_are_separate_curves_and_the_stem_continues() {
        let mesh = generate(&with("F[+F]F", "", 0)).unwrap();
        let curves = crate::groom::curves(&mesh);
        // The stem F F (3 points) and one branch (2 points).
        let mut sizes: Vec<usize> = curves.iter().map(|c| c.len()).collect();
        sizes.sort();
        assert_eq!(sizes, [2, 3]);
        let top = mesh.vertices.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((top - 2.0).abs() < 1e-5);
        assert!(mesh.point_attributes["width"].iter().all(|w| *w > 0.0));
    }

    #[test]
    fn widths_amounts_and_stochastic_rules() {
        let mesh = generate(&with("F!F(2)", "", 0)).unwrap();
        let w = &mesh.point_attributes["width"];
        assert!(w[2] < w[0]);
        assert!((mesh.vertices[2].y - 3.0).abs() < 1e-5);
        // Two rules for F, picked by seed: the same seed gives the same plant.
        let set = LSystemSettings { seed: 3, ..with("F", "F=F[+F]F\nF=F[-F]F:2", 4) };
        let a = generate(&set).unwrap();
        assert_eq!(a.vertices, generate(&set).unwrap().vertices);
        let b = generate(&LSystemSettings { seed: 9, ..set.clone() }).unwrap();
        assert_ne!(a.vertices, b.vertices);
        // The default tree grows and fits a few units.
        let tree = generate(&LSystemSettings::defaults()).unwrap();
        assert!(crate::groom::curves(&tree).len() > 50, "{}", crate::groom::curves(&tree).len());
        let top = tree.vertices.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!(top > 1.0 && top < 20.0, "{top}");
    }

    #[test]
    fn errors_are_explained() {
        assert!(generate(&with("", "", 1)).unwrap_err().contains("premise"));
        assert!(generate(&with("F]", "", 0)).unwrap_err().contains("matching"));
        assert!(generate(&with("A", "A=B", 2)).unwrap_err().contains("nothing was drawn"));
        assert!(generate(&with("F", "FF=F", 1)).unwrap_err().contains("one symbol"));
        assert!(generate(&with("F", "F=FFFFFFFF", 9)).unwrap_err().contains("Generations"));
        assert!(generate(&with("F(x)", "", 0)).unwrap_err().contains("number"));
    }
}
