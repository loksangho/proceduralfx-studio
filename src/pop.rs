// POPs: the nodes inside a POP Network, Houdini-style particle dynamics.
//
// The particles travel through the network as a Mesh of loose points: P, and point
// attributes v, force, age, life and id, plus whatever the nodes add (Cd, mass, targetv,
// airresist, groups as group:name, dead, hit...). Each simulation step, the POP Network
// cooks its display node:
//
// - POP Object gives the particles as the last step left them.
// - POP Solver takes the objects (input 1), runs its pre-solve input (input 2, often forces)
//   on them, integrates (forces into v, v into P, ages them and removes the dead), then runs
//   its post-solve input (input 3; a new network's POP Source is there, as in Houdini).
// - Other POPs change the particles that reach them. Wired into a solver's second or third
//   input with nothing above them, they act on the solver's particles; wired after the
//   solver, they act on its result. Forces add to `force` (or set `targetv` and
//   `airresist`), which the next integration uses and then clears, so forces after the
//   solver act on the next step, as in Houdini.
//
// VEX nodes (POP Wrangle, POP VOP, and the Expression rules of POP Group and POP Kill) run
// in main.rs, which has the wrangle runner; everything else is here.

use super::{hash01, Mesh, Vec3};
use crate::shading::{c, f, t, vector, Param};

pub struct PopOp {
    pub label: &'static str,
    // The start of the default node name ("popsource" makes popsource1).
    pub name: &'static str,
    pub help: &'static str,
    pub inputs: usize,
    pub params: &'static [Param],
    // Text fields: label and default.
    pub texts: &'static [(&'static str, &'static str)],
}

pub const OBJECT: usize = 0;
pub const SOLVER: usize = 1;
pub const SOURCE: usize = 2;
pub const GRAVITY: usize = 3;
pub const FORCE: usize = 4;
pub const WIND: usize = 5;
pub const DRAG: usize = 6;
pub const COLOR: usize = 7;
pub const WRANGLE: usize = 8;
pub const VOP: usize = 9;
pub const GROUP: usize = 10;
pub const KILL: usize = 11;
pub const SPEED_LIMIT: usize = 12;
pub const ATTRACT: usize = 13;
pub const COLLIDE: usize = 14;
pub const REPLICATE: usize = 15;
pub const STATIC_OBJECT: usize = 16;
pub const GROUND_PLANE: usize = 17;
pub const ADVECT: usize = 18;

pub const EMISSION: [&str; 3] = ["Scatter onto surfaces", "Random points", "All points"];
pub const COLOR_MODES: [&str; 3] = ["Constant", "Random", "Ramp over life"];
pub const GROUP_RULES: [&str; 4] = ["All", "Random", "Bounding box", "Expression"];
pub const KILL_RULES: [&str; 3] = ["Expression", "Bounding box", "Random"];
pub const ATTRACT_MODES: [&str; 2] = ["Constant pull", "Spring"];
pub const ADVECT_MODES: [&str; 3] = ["Update velocity", "Update force", "Update position"];

const SURFACE: [Param; 3] = [
    f("bounce", "Bounce", "Collision", 0.0, 1.0, 0.4),
    f("friction", "Friction", "Collision", 0.0, 1.0, 0.1),
    f("radius", "Particle radius", "Collision", 0.0, 10.0, 0.02),
];

// A parameter shown as a menu: its choices.
pub fn choices(op: usize, key: &str) -> Option<&'static [&'static str]> {
    match (op, key) {
        (SOURCE, "emission") => Some(&EMISSION),
        (COLOR, "mode") => Some(&COLOR_MODES),
        (GROUP, "rule") => Some(&GROUP_RULES),
        (KILL, "rule") => Some(&KILL_RULES),
        (ATTRACT, "mode") => Some(&ATTRACT_MODES),
        (ADVECT, "mode") => Some(&ADVECT_MODES),
        _ => None,
    }
}

const NOISE: [Param; 3] = [
    f("noise", "Noise amplitude", "Noise", 0.0, 100.0, 0.0),
    f("frequency", "Noise frequency", "Noise", 0.01, 100.0, 1.0),
    f("speed", "Noise speed", "Noise", 0.0, 100.0, 0.5),
];
const BOX: [Param; 2] = [vector("center", "Box center", "Box", -1000.0, 1000.0, [0.0, 0.0, 0.0]), vector("size", "Box size", "Box", 0.0, 1000.0, [1.0, 1.0, 1.0])];

pub const OPS: &[PopOp] = &[
    PopOp {
        label: "POP Object",
        name: "popobject",
        help: "The particles, as the last step left them. Wire it into a POP Solver's first input.",
        inputs: 0,
        params: &[],
        texts: &[],
    },
    PopOp {
        label: "POP Solver",
        name: "popsolver",
        help: "Moves the particles each step: forces change their velocity, velocity moves them, they age, and the ones past their life (or with dead set) are removed. Input 1: the POP Object. Input 2 (pre-solve): what runs before moving them, such as forces. Input 3 (post-solve): what runs after, such as POP Sources, so new particles are born at the source and first move on the next step.",
        inputs: 3,
        params: &[],
        texts: &[],
    },
    PopOp {
        label: "POP Source",
        name: "popsource",
        help: "Births particles from one of the POP Network's inputs (the origin when it isn't connected). They copy the source's attributes, such as Cd and pscale, and its velocity v if you inherit it.",
        inputs: 1,
        params: &[
            f("input", "Source: POP Network input", "Source", 0.0, 3.0, 0.0),
            f("emission", "Emission type", "Source", 0.0, 2.0, 0.0),
            f("birth_rate", "Birth rate (per second)", "Birth", 0.0, 100000.0, 200.0),
            f("impulse", "Impulse count (per frame)", "Birth", 0.0, 100000.0, 0.0),
            f("life", "Life expectancy", "Birth", 0.01, 1000.0, 2.0),
            f("life_variance", "Life variance", "Birth", 0.0, 1000.0, 0.5),
            t("inherit", "Inherit the source's velocity (v)", "Velocity", true),
            vector("velocity", "Initial velocity", "Velocity", -1000.0, 1000.0, [0.0, 1.0, 0.0]),
            vector("variance", "Velocity variance", "Velocity", 0.0, 1000.0, [0.3, 0.3, 0.3]),
            f("along_normal", "Speed along the source's normals", "Velocity", -1000.0, 1000.0, 0.0),
            f("seed", "Seed", "Velocity", 0.0, 10000.0, 1.0),
        ],
        texts: &[],
    },
    PopOp {
        label: "Gravity Force",
        name: "gravity",
        help: "Pulls the particles with a constant acceleration, -9.8 down by default.",
        inputs: 1,
        params: &[vector("force", "Gravity", "", -1000.0, 1000.0, [0.0, -9.8, 0.0])],
        texts: &[],
    },
    PopOp {
        label: "POP Force",
        name: "popforce",
        help: "Pushes the particles with a force, plus noise that varies through space and time for turbulence. Heavier particles (mass) move less.",
        inputs: 1,
        params: &[vector("force", "Force", "", -1000.0, 1000.0, [0.0, 0.0, 0.0]), NOISE[0], NOISE[1], NOISE[2]],
        texts: &[],
    },
    PopOp {
        label: "POP Wind",
        name: "popwind",
        help: "Blows the particles toward the wind velocity: the higher the air resistance, the faster they match it. Noise makes gusts.",
        inputs: 1,
        params: &[
            vector("wind", "Wind velocity", "", -1000.0, 1000.0, [1.0, 0.0, 0.0]),
            f("air_resistance", "Air resistance", "", 0.0, 100.0, 1.0),
            NOISE[0],
            NOISE[1],
            NOISE[2],
        ],
        texts: &[],
    },
    PopOp {
        label: "POP Drag",
        name: "popdrag",
        help: "Slows the particles down, toward the air's velocity (still air by default).",
        inputs: 1,
        params: &[f("air_resistance", "Air resistance", "", 0.0, 100.0, 1.0), vector("wind", "Air velocity", "", -1000.0, 1000.0, [0.0, 0.0, 0.0])],
        texts: &[],
    },
    PopOp {
        label: "POP Color",
        name: "popcolor",
        help: "Colours the particles (Cd): one colour, a random colour each, or a ramp from the first colour at birth to the second at the end of their life.",
        inputs: 1,
        params: &[
            f("mode", "Color", "", 0.0, 2.0, 0.0),
            c("color", "Color (at birth)", "", [1.0, 0.55, 0.1]),
            c("color2", "Color at the end of life", "", [0.25, 0.25, 0.25]),
            f("seed", "Seed", "", 0.0, 10000.0, 1.0),
        ],
        texts: &[],
    },
    PopOp {
        label: "POP Wrangle",
        name: "popwrangle",
        help: "Runs VEX on every particle each step: @P, @v, @force, @age, @life, @id, @Cd... @TimeInc is the step's length. Set @dead = 1 to remove a particle. point(1, ...) and friends read the POP Network's inputs.",
        inputs: 1,
        params: &[],
        texts: &[("VEX code", "// Runs on every particle each step, e.g.:\n// v@force += set(0, 2, 0) * (1 - @age / @life);\n")],
    },
    PopOp {
        label: "POP VOP",
        name: "popvop",
        help: "A VOP network run on every particle each step, like a POP Wrangle built from nodes. Double-click it to edit. Bind and the geometry VOPs read the particles from input 0, and the POP Network's inputs 0-2 from their inputs 1-3.",
        inputs: 1,
        params: &[],
        texts: &[],
    },
    PopOp {
        label: "POP Group",
        name: "popgroup",
        help: "Puts particles in a point group (group:name), which other POPs can then use in their Group field: all of them, a random fraction, those in a box, or those an expression picks (VEX, such as @P.y > 1).",
        inputs: 1,
        params: &[
            f("rule", "Rule", "", 0.0, 3.0, 1.0),
            f("ratio", "Random: fraction in the group", "", 0.0, 1.0, 0.5),
            BOX[0],
            BOX[1],
            t("births_only", "Only decide for new particles (members stay members)", "", true),
            f("seed", "Seed", "", 0.0, 10000.0, 1.0),
        ],
        texts: &[("Group name", "group1"), ("Expression", "@P.y > 1")],
    },
    PopOp {
        label: "POP Kill",
        name: "popkill",
        help: "Removes particles: those an expression picks (VEX, such as @P.y < -1 or @hit), those in or outside a box, or at random.",
        inputs: 1,
        params: &[
            f("rule", "Rule", "", 0.0, 2.0, 0.0),
            BOX[0],
            BOX[1],
            t("inside", "Kill inside the box (otherwise outside it)", "", false),
            f("chance", "Random: chance per second", "", 0.0, 100.0, 0.5),
            f("seed", "Seed", "", 0.0, 10000.0, 1.0),
        ],
        texts: &[("Expression", "@P.y < -1")],
    },
    PopOp {
        label: "POP Speed Limit",
        name: "popspeedlimit",
        help: "Keeps every particle's speed between a minimum and a maximum.",
        inputs: 1,
        params: &[f("min", "Minimum speed", "", 0.0, 10000.0, 0.0), f("max", "Maximum speed", "", 0.0, 10000.0, 10.0)],
        texts: &[],
    },
    PopOp {
        label: "POP Attract",
        name: "popattract",
        help: "Pulls the particles toward a point: with the same force wherever they are, or like a spring, harder the further away.",
        inputs: 1,
        params: &[
            vector("goal", "Goal", "", -1000.0, 1000.0, [0.0, 1.0, 0.0]),
            f("strength", "Strength", "", 0.0, 1000.0, 5.0),
            f("mode", "Pull", "", 0.0, 1.0, 0.0),
        ],
        texts: &[],
    },
    PopOp {
        label: "POP Collide",
        name: "popcollide",
        help: "Bounces the particles off a ground plane and, optionally, off geometry from one of the POP Network's inputs. Put it after the solver (or in its third input). Particles that hit get @hit = 1 for that step.",
        inputs: 1,
        params: &[
            t("ground", "Ground plane", "", true),
            f("height", "Ground height", "", -1000.0, 1000.0, 0.0),
            t("use_input", "Collide with a POP Network input", "", false),
            f("input", "Collision geometry: POP Network input", "", 0.0, 3.0, 1.0),
            f("bounce", "Bounce", "", 0.0, 1.0, 0.4),
            f("friction", "Friction", "", 0.0, 1.0, 0.1),
            f("radius", "Particle radius", "", 0.0, 10.0, 0.02),
        ],
        texts: &[],
    },
    PopOp {
        label: "POP Replicate",
        name: "popreplicate",
        help: "Births new particles from the existing ones, for trails and sparks: they start where their parent is, with some of its velocity, and copy its attributes (Cd, groups...).",
        inputs: 1,
        params: &[
            f("rate", "Births per particle per second", "", 0.0, 10000.0, 10.0),
            f("life", "Life expectancy", "", 0.01, 1000.0, 0.5),
            f("life_variance", "Life variance", "", 0.0, 1000.0, 0.1),
            f("inherit", "Inherit velocity", "", -10.0, 10.0, 0.5),
            vector("variance", "Velocity variance", "", 0.0, 1000.0, [0.5, 0.5, 0.5]),
            f("seed", "Seed", "", 0.0, 10000.0, 1.0),
        ],
        texts: &[],
    },
    PopOp {
        label: "Static Object",
        name: "staticobject",
        help: "Geometry the particles collide with, from one of the POP Network's inputs. Wire it into the network (merge it with the POP Object into the solver's first input): the POP Solver bounces particles off every Static Object and Ground Plane wired in, after each move. Particles that hit get @hit = 1 for that step.",
        inputs: 0,
        params: &[f("input", "Geometry: POP Network input", "", 0.0, 3.0, 1.0), SURFACE[0], SURFACE[1], SURFACE[2]],
        texts: &[],
    },
    PopOp {
        label: "Ground Plane",
        name: "groundplane",
        help: "An endless floor the particles collide with. Wire it into the network like a Static Object (merge it with the POP Object).",
        inputs: 0,
        params: &[f("height", "Height", "", -1000.0, 1000.0, 0.0), SURFACE[0], SURFACE[1], SURFACE[2]],
        texts: &[],
    },
    PopOp {
        label: "POP Advect",
        name: "popadvect",
        help: "Carries the particles along a velocity field from one of the POP Network's inputs: a Pyro simulation (its smoke's velocity), FLIP particles, or any points with a v attribute. \
               Update velocity pulls each particle's velocity toward the field's (like wind), Update force pushes it by the field, Update position moves it along the field directly. \
               Particles away from the field are left alone.",
        inputs: 1,
        params: &[
            f("input", "Velocity field: POP Network input", "", 0.0, 3.0, 1.0),
            f("mode", "Calculation", "", 0.0, 2.0, 0.0),
            f("scale", "Velocity scale", "", -100.0, 100.0, 1.0),
            f("air_resistance", "Air resistance", "", 0.0, 1000.0, 5.0),
            f("radius", "Point field reach", "", 0.001, 100.0, 0.25),
        ],
        texts: &[],
    },
];

#[derive(Clone, Debug, PartialEq)]
pub struct PopNode {
    pub op: usize,
    pub values: Vec<f32>,
    pub texts: Vec<String>,
}

impl PopNode {
    pub fn new(op: usize) -> PopNode {
        let op = op.min(OPS.len() - 1);
        let def = &OPS[op];
        PopNode {
            op,
            values: def.params.iter().flat_map(|p| p.default[..p.width()].to_vec()).collect(),
            texts: def.texts.iter().map(|(_, default)| default.to_string()).collect(),
        }
    }

    pub fn def(&self) -> &'static PopOp {
        &OPS[self.op.min(OPS.len() - 1)]
    }

    pub fn to_params(&self) -> Vec<f32> {
        let mut p = vec![self.op as f32];
        p.extend_from_slice(&self.values);
        p
    }

    pub fn from_params(p: &[f32]) -> PopNode {
        let mut node = PopNode::new(p.first().map_or(0, |o| o.max(0.0) as usize));
        for (slot, v) in node.values.iter_mut().zip(p.iter().skip(1)) {
            *slot = *v;
        }
        node
    }

    pub fn get(&self, key: &str) -> [f32; 3] {
        let mut at = 0;
        for p in self.def().params {
            if p.key == key {
                return if p.width() == 3 { [self.values[at], self.values[at + 1], self.values[at + 2]] } else { [self.values[at]; 3] };
            }
            at += p.width();
        }
        [0.0; 3]
    }

    pub fn v(&self, key: &str) -> f32 {
        self.get(key)[0]
    }

    fn vec(&self, key: &str) -> Vec3 {
        Vec3::from_arr(self.get(key))
    }

    fn on(&self, key: &str) -> bool {
        self.v(key) > 0.5
    }

    fn index(&self, key: &str) -> usize {
        self.v(key).round().max(0.0) as usize
    }

    pub fn text(&self, i: usize) -> &str {
        self.texts.get(i).map_or("", |s| s.as_str())
    }

    pub fn param_mut(&mut self) -> impl Iterator<Item = (&'static Param, &mut [f32])> {
        let def = self.def();
        let mut rest: &mut [f32] = &mut self.values;
        def.params.iter().map(move |p| {
            let (head, tail) = std::mem::take(&mut rest).split_at_mut(p.width());
            rest = tail;
            (p, head)
        })
    }

    pub fn label(&self, id: usize) -> String {
        format!("{}{id}", self.def().name)
    }

    // The POP Network input a source or collider reads.
    pub fn input(&self) -> usize {
        self.index("input").min(3)
    }

    // The VEX a POP Group or POP Kill runs for its Expression rule, if it uses one.
    pub fn expression_code(&self) -> Option<String> {
        let expression = match self.op {
            GROUP if self.index("rule") == 3 => self.text(1),
            KILL if self.index("rule") == 0 => self.text(0),
            _ => return None,
        };
        let expression = expression.trim().trim_end_matches(';');
        Some(format!("f@{EXPRESSION} = ({});", if expression.is_empty() { "0" } else { expression }))
    }
}

// Where an Expression rule's result is left for apply().
pub const EXPRESSION: &str = "__popexpr";

// ============================================================================
// The particles
// ============================================================================

// Per-point attributes (not prim:, vertex: or detail: ones).
fn per_point(name: &str) -> bool {
    !name.starts_with("prim:") && !name.starts_with("vertex:") && !name.starts_with("detail:")
}

fn known_width(name: &str) -> usize {
    match name {
        "v" | "force" | "targetv" | "Cd" | "N" | "up" | "accel" => 3,
        "orient" => 4,
        _ => 1,
    }
}

fn width(m: &Mesh, name: &str) -> usize {
    let n = m.vertices.len();
    match m.point_attributes.get(name) {
        Some(values) if n > 0 && values.len() % n == 0 && !values.is_empty() => values.len() / n,
        _ => known_width(name),
    }
}

// The attribute, made (filled with `fill`) if it's missing or the wrong size.
fn attr<'a>(m: &'a mut Mesh, name: &str, w: usize, fill: f32) -> &'a mut Vec<f32> {
    let n = m.vertices.len();
    let values = m.point_attributes.entry(name.to_owned()).or_default();
    if values.len() != n * w {
        *values = vec![fill; n * w];
    }
    values
}

fn read3(values: &[f32], i: usize) -> Vec3 {
    Vec3::new(values[i * 3], values[i * 3 + 1], values[i * 3 + 2])
}

fn write3(values: &mut [f32], i: usize, v: Vec3) {
    values[i * 3..i * 3 + 3].copy_from_slice(&[v.x, v.y, v.z]);
}

// Makes the particles a set of loose points with the attributes every step relies on.
pub fn tidy(m: &mut Mesh) {
    m.indices.clear();
    m.point_attributes.retain(|k, _| !k.starts_with("prim:") && !k.starts_with("vertex:"));
    for (name, w) in [("v", 3), ("force", 3), ("age", 1), ("life", 1), ("id", 1)] {
        attr(m, name, w, 0.0);
    }
}

// Keeps the particles where `keep` is true.
pub fn retain(m: &mut Mesh, keep: &[bool]) {
    let n = m.vertices.len();
    if keep.iter().all(|k| *k) {
        return;
    }
    let mut i = 0;
    m.vertices.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    for (name, values) in m.point_attributes.iter_mut() {
        if !per_point(name) || n == 0 || values.len() % n != 0 {
            continue;
        }
        let w = values.len() / n;
        let mut out = Vec::with_capacity(values.len());
        for (p, chunk) in values.chunks_exact(w).enumerate() {
            if keep[p] {
                out.extend_from_slice(chunk);
            }
        }
        *values = out;
    }
}

// Adds `births` (loose points with their attributes) to the particles. Attributes only one
// side has are filled with zeros on the other.
pub fn append(m: &mut Mesh, births: Mesh) {
    let (n, b) = (m.vertices.len(), births.vertices.len());
    if b == 0 {
        return;
    }
    let mut names: Vec<String> = m.point_attributes.keys().chain(births.point_attributes.keys()).filter(|k| per_point(k)).cloned().collect();
    names.sort();
    names.dedup();
    for name in names {
        let w = if m.point_attributes.contains_key(&name) && n > 0 { width(m, &name) } else { width(&births, &name) };
        let theirs = births.point_attributes.get(&name).filter(|v| v.len() == b * w).cloned().unwrap_or_else(|| vec![0.0; b * w]);
        let ours = attr(m, &name, w, 0.0);
        ours.extend(theirs);
    }
    m.vertices.extend(births.vertices);
}

// The next free particle id, carried in detail:pop_next_id.
fn take_ids(m: &mut Mesh, count: usize) -> u32 {
    const KEY: &str = "detail:pop_next_id";
    let next = match m.point_attributes.get(KEY).and_then(|v| v.first()) {
        Some(v) => *v as u32,
        None => m.point_attributes.get("id").map_or(0, |ids| ids.iter().fold(-1.0f32, |a, b| a.max(*b)) as i64 + 1) as u32,
    };
    m.point_attributes.insert(KEY.into(), vec![(next as usize + count) as f32]);
    next
}

// Where new particles come from: area-weighted triangles and points of a POP Network input.
pub struct Sampler<'a> {
    mesh: &'a Mesh,
    cumulative: Vec<f32>,
}

impl<'a> Sampler<'a> {
    pub fn new(mesh: &'a Mesh) -> Self {
        let mut total = 0.0;
        let cumulative = mesh
            .indices
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| mesh.vertices[i]);
                total += b.sub(a).cross(c.sub(a)).length() * 0.5;
                total
            })
            .collect();
        Sampler { mesh, cumulative }
    }

    // Weights of the source points making up a birth spot: one point, or a triangle's three.
    fn surface(&self, r: [f32; 3]) -> Option<([usize; 3], [f32; 3])> {
        let total = *self.cumulative.last()?;
        if total <= 0.0 {
            return None;
        }
        let t = self.cumulative.partition_point(|&c| c < r[0] * total).min(self.mesh.indices.len() - 1);
        let (mut u, mut v) = (r[1], r[2]);
        if u + v > 1.0 {
            (u, v) = (1.0 - u, 1.0 - v);
        }
        Some((self.mesh.indices[t], [1.0 - u - v, u, v]))
    }

    fn point(&self, i: usize) -> ([usize; 3], [f32; 3]) {
        ([i; 3], [1.0, 0.0, 0.0])
    }

    // A new particle at the weighted points: P, the face normal or N, and every point attribute.
    fn birth(&self, (points, weights): ([usize; 3], [f32; 3]), births: &mut Mesh) -> (Vec3, Vec3, Vec3) {
        let m = self.mesh;
        let n = m.vertices.len();
        let mix = |values: &[f32], w: usize| -> Vec<f32> { (0..w).map(|c| (0..3).map(|k| values[points[k] * w + c] * weights[k]).sum()).collect() };
        let p = (0..3).fold(Vec3::new(0.0, 0.0, 0.0), |acc, k| acc.add(m.vertices[points[k]].scale(weights[k])));
        let up = Vec3::new(0.0, 1.0, 0.0);
        let mut normal = if points[0] != points[1] {
            let [a, b, c] = points.map(|i| m.vertices[i]);
            let face = b.sub(a).cross(c.sub(a));
            if face.length() > 1e-12 { face.normalize() } else { up }
        } else {
            up
        };
        let mut velocity = Vec3::new(0.0, 0.0, 0.0);
        let index = births.vertices.len();
        for (name, values) in &m.point_attributes {
            if !per_point(name) || n == 0 || values.len() % n != 0 || matches!(name.as_str(), "id" | "age" | "life" | "force") {
                continue;
            }
            let w = values.len() / n;
            let value = mix(values, w);
            match (name.as_str(), w) {
                ("v", 3) => velocity = Vec3::new(value[0], value[1], value[2]),
                ("N", 3) if points[0] == points[1] => {
                    let read = Vec3::new(value[0], value[1], value[2]);
                    if read.length() > 1e-12 {
                        normal = read.normalize();
                    }
                }
                _ => {}
            }
            if name == "v" {
                continue;
            }
            let out = births.point_attributes.entry(name.clone()).or_default();
            out.resize(index * w, 0.0);
            out.extend(value);
        }
        (p, normal, velocity)
    }
}

// Everything a POP needs besides its own particles and settings.
#[derive(Clone, Copy)]
pub struct Step<'a> {
    pub time: f32,
    pub dt: f32,
    // Steps since the start (the first is 1).
    pub step: usize,
    // The first step of a frame (impulses birth then).
    pub frame_start: bool,
    // Tells sources apart that share a seed.
    pub node_seed: u32,
    // The POP Network's inputs at this step.
    pub inputs: &'a [Mesh],
    // The Static Objects and Ground Planes wired into the network.
    pub colliders: &'a [Collider],
}

// Something particles bounce off: a floor or a geometry's distance field.
pub struct Collider {
    pub ground: Option<f32>,
    pub field: Option<std::sync::Arc<crate::volume::Volume>>,
    pub bounce: f32,
    pub friction: f32,
    pub radius: f32,
}

impl Collider {
    // A Static Object's or Ground Plane's collider (None for other POPs, or empty geometry).
    pub fn of(node: &PopNode, inputs: &[Mesh]) -> Option<Collider> {
        let (ground, field) = match node.op {
            GROUND_PLANE => (Some(node.v("height")), None),
            STATIC_OBJECT => (None, Some(inputs.get(node.input()).and_then(crate::sims::collider_field)?)),
            COLLIDE => (node.on("ground").then(|| node.v("height")), if node.on("use_input") { inputs.get(node.input()).and_then(crate::sims::collider_field) } else { None }),
            _ => return None,
        };
        Some(Collider { ground, field, bounce: node.v("bounce").clamp(0.0, 1.0), friction: node.v("friction").clamp(0.0, 1.0), radius: node.v("radius").max(0.0) })
    }
}

fn noise3(p: Vec3, frequency: f32, time: f32, speed: f32) -> Vec3 {
    let q = p.scale(frequency.max(1e-4));
    let shift = time * speed;
    let at = |seed: u32| crate::vex::perlin3([q.x + shift, q.y + shift * 0.7, q.z - shift * 0.3], seed);
    Vec3::new(at(11), at(23), at(37))
}

fn masked(mask: Option<&[bool]>, i: usize) -> bool {
    mask.is_none_or(|m| m.get(i).copied().unwrap_or(false))
}

// Moves the particles one step (the POP Solver): forces and the pull toward targetv into
// velocity, velocity into position, then ages them and removes the dead.
pub fn integrate(m: &mut Mesh, dt: f32) {
    tidy(m);
    let n = m.vertices.len();
    let mass = m.point_attributes.get("mass").filter(|v| v.len() == n).cloned();
    let air = m.point_attributes.get("airresist").filter(|v| v.len() == n).cloned();
    let target = m.point_attributes.get("targetv").filter(|v| v.len() == n * 3).cloned();
    let force = m.point_attributes["force"].clone();
    {
        let v = attr(m, "v", 3, 0.0);
        for i in 0..n {
            let mut vel = read3(v, i);
            let weight = mass.as_ref().map_or(1.0, |ms| ms[i]).max(1e-6);
            let accel = read3(&force, i).scale(1.0 / weight);
            let resistance = air.as_ref().map_or(0.0, |a| a[i]);
            vel = match &target {
                // dv/dt = accel + airresist (targetv - v), solved exactly over the step: v
                // settles toward targetv + accel / airresist, so strong resistance stays stable.
                Some(target) if resistance > 0.0 => {
                    let settle = read3(target, i).add(accel.scale(1.0 / resistance));
                    settle.add(vel.sub(settle).scale((-resistance * dt).exp()))
                }
                _ => vel.add(accel.scale(dt)),
            };
            write3(v, i, vel);
        }
    }
    let v = m.point_attributes["v"].clone();
    for (i, p) in m.vertices.iter_mut().enumerate() {
        *p = p.add(read3(&v, i).scale(dt));
    }
    for age in attr(m, "age", 1, 0.0).iter_mut() {
        *age += dt;
    }
    // Forces are used up; the next step's POPs set them again.
    attr(m, "force", 3, 0.0).fill(0.0);
    for (name, w) in [("airresist", 1), ("targetv", 3)] {
        if m.point_attributes.contains_key(name) {
            attr(m, name, w, 0.0).fill(0.0);
        }
    }
    let (age, life) = (m.point_attributes["age"].clone(), m.point_attributes["life"].clone());
    let dead = m.point_attributes.get("dead").filter(|d| d.len() == n).cloned();
    let keep: Vec<bool> = (0..n).map(|i| !(life[i] > 0.0 && age[i] >= life[i]) && dead.as_ref().is_none_or(|d| d[i] <= 0.5)).collect();
    retain(m, &keep);
}

// Adds to the pull toward a velocity: airresist sums, targetv is their weighted average.
fn pull_toward(m: &mut Mesh, i: usize, velocity: Vec3, resistance: f32) {
    if resistance <= 0.0 {
        return;
    }
    let before = attr(m, "airresist", 1, 0.0)[i];
    let total = before + resistance;
    attr(m, "airresist", 1, 0.0)[i] = total;
    let target = attr(m, "targetv", 3, 0.0);
    let mixed = read3(target, i).scale(before / total).add(velocity.scale(resistance / total));
    write3(target, i, mixed);
}

// Runs a POP (other than the object, the solver and the VEX ones) on the particles.
// `mask`: the particles in the node's group (all when None).
pub fn apply(node: &PopNode, m: &mut Mesh, s: &Step, mask: Option<&[bool]>) {
    tidy(m);
    let n = m.vertices.len();
    let seed = node.v("seed") as u32;
    match node.op {
        SOURCE => source(node, m, s),
        GRAVITY | FORCE | ATTRACT => {
            let mass = m.point_attributes.get("mass").filter(|v| v.len() == n).cloned();
            let positions = m.vertices.clone();
            let force = attr(m, "force", 3, 0.0);
            for i in (0..n).filter(|&i| masked(mask, i)) {
                let push = match node.op {
                    // An acceleration: the same for light and heavy particles.
                    GRAVITY => node.vec("force").scale(mass.as_ref().map_or(1.0, |ms| ms[i])),
                    FORCE => node.vec("force").add(noise3(positions[i], node.v("frequency"), s.time, node.v("speed")).scale(node.v("noise"))),
                    _ => {
                        let to = node.vec("goal").sub(positions[i]);
                        let strength = node.v("strength");
                        if node.index("mode") == 1 {
                            to.scale(strength)
                        } else if to.length() > 1e-6 {
                            to.normalize().scale(strength)
                        } else {
                            Vec3::new(0.0, 0.0, 0.0)
                        }
                    }
                };
                let total = read3(force, i).add(push);
                write3(force, i, total);
            }
        }
        WIND | DRAG => {
            let positions = m.vertices.clone();
            for i in (0..n).filter(|&i| masked(mask, i)) {
                let mut wind = node.vec("wind");
                if node.op == WIND && node.v("noise") > 0.0 {
                    wind = wind.add(noise3(positions[i], node.v("frequency"), s.time, node.v("speed")).scale(node.v("noise")));
                }
                pull_toward(m, i, wind, node.v("air_resistance"));
            }
        }
        COLOR => {
            let (age, life, id) = (m.point_attributes["age"].clone(), m.point_attributes["life"].clone(), m.point_attributes["id"].clone());
            let (a, b) = (node.vec("color"), node.vec("color2"));
            let cd = attr(m, "Cd", 3, 1.0);
            for i in (0..n).filter(|&i| masked(mask, i)) {
                let color = match node.index("mode") {
                    1 => {
                        let r = |k: u32| hash01((id[i] as u32).wrapping_mul(7).wrapping_add(k), seed ^ 0x2c1b_3c6d);
                        Vec3::new(r(1), r(2), r(3))
                    }
                    2 => a.add(b.sub(a).scale(if life[i] > 0.0 { (age[i] / life[i]).clamp(0.0, 1.0) } else { 0.0 })),
                    _ => a,
                };
                write3(cd, i, color);
            }
        }
        GROUP => {
            let name = format!("group:{}", node.text(0).trim().replace(char::is_whitespace, "_"));
            if name == "group:" {
                return;
            }
            let expression = m.point_attributes.remove(EXPRESSION);
            let (age, id) = (m.point_attributes["age"].clone(), m.point_attributes["id"].clone());
            let positions = m.vertices.clone();
            let births_only = node.on("births_only");
            let group = attr(m, &name, 1, 0.0);
            for i in 0..n {
                // New this step: born at age 0 before the solver, or within a step after it.
                if births_only && age[i] > s.dt * 1.001 {
                    continue;
                }
                let inside = masked(mask, i)
                    && match node.index("rule") {
                        0 => true,
                        1 => hash01((id[i] as u32).wrapping_mul(31).wrapping_add(5), seed ^ 0x6a09_e667) < node.v("ratio"),
                        2 => in_box(positions[i], node),
                        _ => expression.as_ref().is_some_and(|e| e.get(i).is_some_and(|v| *v > 0.5)),
                    };
                group[i] = if inside { 1.0 } else if births_only { group[i] } else { 0.0 };
            }
        }
        KILL => {
            let expression = m.point_attributes.remove(EXPRESSION);
            let id = m.point_attributes["id"].clone();
            let keep: Vec<bool> = (0..n)
                .map(|i| {
                    !(masked(mask, i)
                        && match node.index("rule") {
                            1 => in_box(m.vertices[i], node) == node.on("inside"),
                            2 => {
                                let chance = 1.0 - (-node.v("chance").max(0.0) * s.dt).exp();
                                hash01((id[i] as u32).wrapping_mul(17).wrapping_add(s.step as u32 * 7919), seed ^ 0x3c6e_f372) < chance
                            }
                            _ => expression.as_ref().is_some_and(|e| e.get(i).is_some_and(|v| *v > 0.5)),
                        })
                })
                .collect();
            retain(m, &keep);
        }
        SPEED_LIMIT => {
            let (low, high) = (node.v("min").max(0.0), node.v("max").max(0.0));
            let v = attr(m, "v", 3, 0.0);
            for i in (0..n).filter(|&i| masked(mask, i)) {
                let vel = read3(v, i);
                let speed = vel.length();
                if speed > high {
                    write3(v, i, vel.scale(high / speed));
                } else if speed < low && speed > 1e-9 {
                    write3(v, i, vel.scale(low / speed));
                }
            }
        }
        COLLIDE => collide(node, m, s, mask),
        REPLICATE => replicate(node, m, s, mask),
        ADVECT => {
            let Some(field) = s.inputs.get(node.input()).and_then(|g| VelocityField::new(g, node.v("radius"))) else { return };
            let scale = node.v("scale");
            let positions = m.vertices.clone();
            for i in (0..n).filter(|&i| masked(mask, i)) {
                let Some(flow) = field.at(positions[i]) else { continue };
                let flow = flow.scale(scale);
                match node.index("mode") {
                    1 => {
                        let force = attr(m, "force", 3, 0.0);
                        let total = read3(force, i).add(flow);
                        write3(force, i, total);
                    }
                    2 => m.vertices[i] = positions[i].add(flow.scale(s.dt)),
                    _ => pull_toward(m, i, flow, node.v("air_resistance")),
                }
            }
        }
        _ => {}
    }
}

// A velocity field to sample anywhere: a volume's voxels (Pyro smoke, with v per voxel),
// blended between the eight around a point, or loose points with v (FLIP particles),
// weighted by distance within `reach`. Nothing (None) where there's no field nearby.
pub struct VelocityField {
    cells: std::collections::HashMap<[i64; 3], Vec<(Vec3, Vec3)>>,
    // Volumes: voxel size and the centre of voxel 0; points: the reach.
    size: f32,
    origin: Option<Vec3>,
}

impl VelocityField {
    pub fn new(geometry: &Mesh, reach: f32) -> Option<VelocityField> {
        let v = geometry.point_attributes.get("v").filter(|v| v.len() == geometry.vertices.len() * 3 && !v.is_empty())?;
        let detail = |name: &str| geometry.point_attributes.get(name).filter(|d| !d.is_empty());
        let voxel = detail("detail:volume_voxelsize").map(|d| d[0]).filter(|h| *h > 0.0);
        let origin = detail("detail:volume_origin").filter(|o| o.len() >= 3).map(|o| Vec3::new(o[0], o[1], o[2]));
        let (size, origin) = match (voxel, origin) {
            (Some(h), Some(o)) => (h, Some(o)),
            _ => (reach.max(1e-3), None),
        };
        let mut cells: std::collections::HashMap<[i64; 3], Vec<(Vec3, Vec3)>> = std::collections::HashMap::new();
        for (i, p) in geometry.vertices.iter().enumerate() {
            let at = match origin {
                // Voxel points sit (jittered) in their voxel: the nearest voxel index.
                Some(o) => [((p.x - o.x) / size).round() as i64, ((p.y - o.y) / size).round() as i64, ((p.z - o.z) / size).round() as i64],
                None => [(p.x / size).floor() as i64, (p.y / size).floor() as i64, (p.z / size).floor() as i64],
            };
            cells.entry(at).or_default().push((*p, Vec3::new(v[i * 3], v[i * 3 + 1], v[i * 3 + 2])));
        }
        Some(VelocityField { cells, size, origin })
    }

    pub fn at(&self, p: Vec3) -> Option<Vec3> {
        let mean = |values: &[(Vec3, f32)]| -> Option<Vec3> {
            let weight: f32 = values.iter().map(|(_, w)| w).sum();
            (weight > 1e-6).then(|| values.iter().fold(Vec3::zero(), |sum, (v, w)| sum.add(v.scale(*w))).scale(1.0 / weight))
        };
        match self.origin {
            Some(o) => {
                // Between the eight voxels round p, each weighted as in trilinear filtering;
                // missing voxels (no smoke there) don't count.
                let f = [(p.x - o.x) / self.size, (p.y - o.y) / self.size, (p.z - o.z) / self.size];
                let base = f.map(|c| c.floor() as i64);
                let t = [0, 1, 2].map(|a| f[a] - base[a] as f32);
                let mut values = Vec::with_capacity(8);
                for k in 0..8 {
                    let corner = [base[0] + (k & 1) as i64, base[1] + ((k >> 1) & 1) as i64, base[2] + ((k >> 2) & 1) as i64];
                    let w: f32 = (0..3).map(|a| if (k >> a) & 1 == 1 { t[a] } else { 1.0 - t[a] }).product();
                    if let Some(cell) = self.cells.get(&corner) {
                        values.push((cell[0].1, w));
                    }
                }
                mean(&values)
            }
            None => {
                // Points within reach, nearer ones counting more.
                let c = [(p.x / self.size).floor() as i64, (p.y / self.size).floor() as i64, (p.z / self.size).floor() as i64];
                let mut values = Vec::new();
                for dz in -1..=1 {
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            for (q, v) in self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                                let d = q.sub(p).length();
                                if d < self.size {
                                    let w = 1.0 - d / self.size;
                                    values.push((*v, w * w));
                                }
                            }
                        }
                    }
                }
                mean(&values)
            }
        }
    }
}

fn in_box(p: Vec3, node: &PopNode) -> bool {
    let (center, size) = (node.vec("center"), node.vec("size").scale(0.5));
    (p.x - center.x).abs() <= size.x && (p.y - center.y).abs() <= size.y && (p.z - center.z).abs() <= size.z
}

// Births this step at a steady rate: whole particles, so none are lost between steps.
fn steady(rate: f32, step: usize, dt: f32) -> usize {
    let rate = rate.max(0.0) as f64;
    let at = |k: usize| (rate * k as f64 * dt as f64 + 1e-6).floor() as usize;
    at(step).saturating_sub(at(step.saturating_sub(1)))
}

fn source(node: &PopNode, m: &mut Mesh, s: &Step) {
    let empty = Mesh::default();
    let mesh = s.inputs.get(node.input()).unwrap_or(&empty);
    let sampler = Sampler::new(mesh);
    let points = mesh.vertices.len();
    let emission = node.index("emission");
    let mut count = steady(node.v("birth_rate"), s.step, s.dt) + if s.frame_start { node.v("impulse").max(0.0) as usize } else { 0 };
    if emission == 2 && s.frame_start {
        count += points;
    }
    if count == 0 {
        return;
    }
    let count = count.min(1_000_000);
    let first = take_ids(m, count);
    let seed = (node.v("seed") as u32) ^ s.node_seed;
    let mut births = Mesh::default();
    let (mut v, mut age, mut life, mut ids) = (Vec::with_capacity(count * 3), Vec::with_capacity(count), Vec::with_capacity(count), Vec::with_capacity(count));
    let all_points = if emission == 2 && s.frame_start { points } else { 0 };
    for k in 0..count {
        let id = first.wrapping_add(k as u32);
        let r = |j: u32| hash01(id.wrapping_mul(13).wrapping_add(j), seed);
        // "All points" births one at every point first, then the steady rate at random points.
        let spot = if k < all_points {
            Some(sampler.point(k))
        } else if emission == 0 {
            sampler.surface([r(1), r(2), r(3)]).or_else(|| (points > 0).then(|| sampler.point(((r(1) * points as f32) as usize).min(points - 1))))
        } else {
            (points > 0).then(|| sampler.point(((r(1) * points as f32) as usize).min(points - 1)))
        };
        let (p, normal, inherited) = match spot {
            Some(spot) => sampler.birth(spot, &mut births),
            None => (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 0.0)),
        };
        births.vertices.push(p);
        let variance = node.vec("variance");
        let jitter = Vec3::new(variance.x * (2.0 * r(4) - 1.0), variance.y * (2.0 * r(5) - 1.0), variance.z * (2.0 * r(6) - 1.0));
        let start = if node.on("inherit") { inherited } else { Vec3::new(0.0, 0.0, 0.0) };
        let vel = start.add(node.vec("velocity")).add(jitter).add(normal.scale(node.v("along_normal")));
        v.extend([vel.x, vel.y, vel.z]);
        age.push(0.0);
        life.push((node.v("life") + node.v("life_variance") * (2.0 * r(7) - 1.0)).max(0.01));
        ids.push(id as f32);
    }
    // Source attributes copied for only some births (a sampler point without them) are padded.
    for values in births.point_attributes.values_mut() {
        let w = values.len().div_ceil(births.vertices.len().max(1)).max(1);
        values.resize(births.vertices.len() * w, 0.0);
    }
    births.point_attributes.insert("v".into(), v);
    births.point_attributes.insert("age".into(), age);
    births.point_attributes.insert("life".into(), life);
    births.point_attributes.insert("id".into(), ids);
    append(m, births);
}

fn collide(node: &PopNode, m: &mut Mesh, s: &Step, mask: Option<&[bool]>) {
    if let Some(collider) = Collider::of(node, s.inputs) {
        bounce_off(m, &[collider], mask);
    }
}

// Pushes particles out of the colliders and bounces their velocity; sets @hit on the ones
// that touched (and clears it on the others).
pub fn bounce_off(m: &mut Mesh, colliders: &[Collider], mask: Option<&[bool]>) {
    tidy(m);
    let n = m.vertices.len();
    let mut hit = vec![0.0; n];
    let mut v = m.point_attributes["v"].clone();
    for i in (0..n).filter(|&i| masked(mask, i)) {
        let mut p = m.vertices[i];
        let mut vel = read3(&v, i);
        for c in colliders {
            // The surface's outward normal and how far the particle is inside it.
            let mut contacts: Vec<(Vec3, f32)> = Vec::new();
            if let Some(h) = c.ground {
                if p.y < h + c.radius {
                    contacts.push((Vec3::new(0.0, 1.0, 0.0), h + c.radius - p.y));
                }
            }
            if let Some(sdf) = &c.field {
                let phi = sdf.sample(p);
                if phi < c.radius {
                    let g = sdf.gradient(p);
                    if g.length() > 1e-6 {
                        contacts.push((g.normalize(), c.radius - phi));
                    }
                }
            }
            for (normal, depth) in contacts {
                p = p.add(normal.scale(depth));
                let into = vel.dot(normal);
                if into < 0.0 {
                    let along = normal.scale(into);
                    vel = vel.sub(along).scale(1.0 - c.friction).sub(along.scale(c.bounce));
                }
                hit[i] = 1.0;
            }
        }
        m.vertices[i] = p;
        write3(&mut v, i, vel);
    }
    m.point_attributes.insert("v".into(), v);
    m.point_attributes.insert("hit".into(), hit);
}

fn replicate(node: &PopNode, m: &mut Mesh, s: &Step, mask: Option<&[bool]>) {
    let n = m.vertices.len();
    let rate = node.v("rate").max(0.0);
    if n == 0 || rate <= 0.0 {
        return;
    }
    let (age, id, v) = (m.point_attributes["age"].clone(), m.point_attributes["id"].clone(), m.point_attributes["v"].clone());
    // Each parent's births this step: steady along its own age, so a parent replicates evenly.
    let counts: Vec<usize> = (0..n)
        .map(|i| if masked(mask, i) { ((rate * age[i]).floor() - (rate * (age[i] - s.dt).max(0.0)).floor()).max(0.0) as usize } else { 0 })
        .collect();
    let total: usize = counts.iter().sum();
    if total == 0 {
        return;
    }
    let total = total.min(1_000_000);
    let first = take_ids(m, total);
    let seed = (node.v("seed") as u32) ^ s.node_seed;
    let mut births = Mesh::default();
    let names: Vec<(String, usize)> = m
        .point_attributes
        .iter()
        .filter(|(k, values)| per_point(k) && values.len() == n * (values.len() / n).max(1) && !matches!(k.as_str(), "v" | "force" | "age" | "life" | "id" | "hit" | "dead"))
        .map(|(k, values)| (k.clone(), values.len() / n))
        .collect();
    let (mut bv, mut bage, mut blife, mut bid) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut k = 0usize;
    'parents: for (i, &count) in counts.iter().enumerate() {
        for _ in 0..count {
            if k >= total {
                break 'parents;
            }
            let child = first.wrapping_add(k as u32);
            let r = |j: u32| hash01(child.wrapping_mul(19).wrapping_add(j), seed ^ (id[i] as u32).wrapping_mul(2_654_435_761));
            births.vertices.push(m.vertices[i]);
            for (name, w) in &names {
                let values = &m.point_attributes[name];
                births.point_attributes.entry(name.clone()).or_default().extend_from_slice(&values[i * w..(i + 1) * w]);
            }
            let variance = node.vec("variance");
            let vel = read3(&v, i).scale(node.v("inherit")).add(Vec3::new(variance.x * (2.0 * r(1) - 1.0), variance.y * (2.0 * r(2) - 1.0), variance.z * (2.0 * r(3) - 1.0)));
            bv.extend([vel.x, vel.y, vel.z]);
            bage.push(0.0);
            blife.push((node.v("life") + node.v("life_variance") * (2.0 * r(4) - 1.0)).max(0.01));
            bid.push(child as f32);
            k += 1;
        }
    }
    births.point_attributes.insert("v".into(), bv);
    births.point_attributes.insert("age".into(), bage);
    births.point_attributes.insert("life".into(), blife);
    births.point_attributes.insert("id".into(), bid);
    append(m, births);
}

// What the POP Network outputs: the particles, without the step's bookkeeping.
pub fn output(state: &Mesh) -> Mesh {
    let mut out = state.clone();
    out.point_attributes.remove("detail:pop_next_id");
    out.point_attributes.remove(EXPRESSION);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(time_step: usize, inputs: &[Mesh]) -> Step<'_> {
        let dt = 1.0 / 24.0;
        Step { time: time_step as f32 * dt, dt, step: time_step, frame_start: true, node_seed: 0, inputs, colliders: &[] }
    }

    fn with(op: usize, set: &[(&str, &[f32])]) -> PopNode {
        let mut node = PopNode::new(op);
        for (key, values) in set {
            for (p, slot) in node.param_mut() {
                if p.key == *key {
                    slot.copy_from_slice(values);
                }
            }
        }
        node
    }

    #[test]
    fn sources_birth_at_their_rate_with_ids_and_attributes() {
        let mut quad = Mesh {
            vertices: vec![Vec3::new(-1.0, 2.0, -1.0), Vec3::new(1.0, 2.0, -1.0), Vec3::new(1.0, 2.0, 1.0), Vec3::new(-1.0, 2.0, 1.0)],
            indices: vec![[0, 2, 1], [0, 3, 2]],
            ..Mesh::default()
        };
        quad.point_attributes.insert("Cd".into(), vec![1.0, 0.0, 0.0].repeat(4));
        let inputs = [quad];
        let source = with(SOURCE, &[("birth_rate", &[240.0])]);
        let mut m = Mesh::default();
        for k in 1..=24 {
            apply(&source, &mut m, &step(k, &inputs), None);
        }
        // 240 a second for a second, every one on the quad, red, and numbered once.
        assert_eq!(m.vertices.len(), 240);
        assert!(m.vertices.iter().all(|p| (p.y - 2.0).abs() < 1e-5 && p.x.abs() <= 1.0 && p.z.abs() <= 1.0));
        assert!(m.point_attributes["Cd"].chunks(3).all(|c| (c[0] - 1.0).abs() < 1e-5 && c[1].abs() < 1e-5 && c[2].abs() < 1e-5));
        let mut ids = m.point_attributes["id"].clone();
        ids.sort_by(f32::total_cmp);
        ids.dedup();
        assert_eq!(ids.len(), 240);
        // All points, once a frame: one at each corner.
        let corners = with(SOURCE, &[("birth_rate", &[0.0]), ("emission", &[2.0])]);
        let mut m = Mesh::default();
        apply(&corners, &mut m, &step(1, &inputs), None);
        assert_eq!(m.vertices.len(), 4);
    }

    #[test]
    fn the_solver_moves_ages_and_removes_the_dead() {
        let mut m = Mesh { vertices: vec![Vec3::new(0.0, 0.0, 0.0); 2], ..Mesh::default() };
        tidy(&mut m);
        m.point_attributes.insert("life".into(), vec![1.0, 0.05]);
        let gravity = PopNode::new(GRAVITY);
        let dt = 0.1;
        apply(&gravity, &mut m, &step(1, &[]), None);
        integrate(&mut m, dt);
        // The short-lived one is gone; the other fell, and its force is used up.
        assert_eq!(m.vertices.len(), 1);
        assert!((m.point_attributes["v"][1] + 0.98).abs() < 1e-5);
        assert!((m.vertices[0].y + 0.098).abs() < 1e-5);
        assert_eq!(m.point_attributes["force"], vec![0.0; 3]);
        assert!((m.point_attributes["age"][0] - 0.1).abs() < 1e-6);
        // Drag pulls toward still air.
        m.point_attributes.insert("v".into(), vec![10.0, 0.0, 0.0]);
        apply(&with(DRAG, &[("air_resistance", &[5.0])]), &mut m, &step(2, &[]), None);
        integrate(&mut m, dt);
        assert!((m.point_attributes["v"][0] - 10.0 * (-0.5f32).exp()).abs() < 1e-4);
    }

    #[test]
    fn groups_kills_collisions_and_replication() {
        let mut m = Mesh { vertices: (0..10).map(|i| Vec3::new(i as f32, -0.5, 0.0)).collect(), ..Mesh::default() };
        tidy(&mut m);
        m.point_attributes.insert("id".into(), (0..10).map(|i| i as f32).collect());
        m.point_attributes.insert("life".into(), vec![5.0; 10]);
        // A box around x 0-4 groups half of them.
        let mut group = with(GROUP, &[("rule", &[2.0]), ("center", &[2.0, 0.0, 0.0]), ("size", &[5.0, 2.0, 2.0])]);
        group.texts[0] = "left".into();
        apply(&group, &mut m, &step(1, &[]), None);
        assert_eq!(m.point_attributes["group:left"].iter().filter(|g| **g > 0.5).count(), 5);
        // The ground pushes them up to y = radius and marks the hits.
        m.point_attributes.insert("v".into(), [0.0, -2.0, 0.0].repeat(10));
        apply(&PopNode::new(COLLIDE), &mut m, &step(1, &[]), None);
        assert!(m.vertices.iter().all(|p| (p.y - 0.02).abs() < 1e-5));
        assert!(m.point_attributes["v"].chunks(3).all(|v| (v[1] - 0.8).abs() < 1e-5));
        assert!(m.point_attributes["hit"].iter().all(|h| *h == 1.0));
        // Killing inside the box leaves the other five.
        let kill = with(KILL, &[("rule", &[1.0]), ("center", &[2.0, 0.0, 0.0]), ("size", &[5.0, 2.0, 2.0]), ("inside", &[1.0])]);
        apply(&kill, &mut m, &step(1, &[]), None);
        assert_eq!(m.vertices.len(), 5);
        assert!(m.vertices.iter().all(|p| p.x >= 5.0));
        // Ten births a second each, over one second of age: about ten children each.
        m.point_attributes.insert("age".into(), vec![1.0; 5]);
        let before = m.vertices.len();
        let replicate = with(REPLICATE, &[("rate", &[10.0])]);
        apply(&replicate, &mut m, &Step { time: 1.0, dt: 1.0, step: 24, frame_start: true, node_seed: 0, inputs: &[], colliders: &[] }, None);
        assert_eq!(m.vertices.len(), before + 50);
        assert_eq!(m.point_attributes["group:left"].len(), m.vertices.len());
    }

    #[test]
    fn advect_carries_particles_along_a_velocity_field() {
        // A volume (as Pyro outputs it): voxels 0.5 across along x, all flowing up at 2,
        // and points (as FLIP outputs them) flowing +x at 3.
        let mut smoke = Mesh { vertices: (0..4).map(|i| Vec3::new(i as f32 * 0.5, 0.0, 0.0)).collect(), ..Mesh::default() };
        smoke.point_attributes.insert("v".into(), [0.0, 2.0, 0.0].repeat(4));
        smoke.point_attributes.insert("detail:volume_voxelsize".into(), vec![0.5]);
        smoke.point_attributes.insert("detail:volume_origin".into(), vec![0.0; 3]);
        let mut water = Mesh { vertices: vec![Vec3::new(5.0, 0.0, 0.0), Vec3::new(5.1, 0.0, 0.0)], ..Mesh::default() };
        water.point_attributes.insert("v".into(), [3.0, 0.0, 0.0].repeat(2));
        let field = VelocityField::new(&smoke, 0.25).unwrap();
        let up = field.at(Vec3::new(0.7, 0.1, 0.0)).unwrap();
        assert!((up.y - 2.0).abs() < 1e-5 && up.x.abs() < 1e-5, "{up:?}");
        assert!(field.at(Vec3::new(10.0, 0.0, 0.0)).is_none(), "nothing away from the smoke");
        assert!((VelocityField::new(&water, 0.25).unwrap().at(Vec3::new(5.05, 0.05, 0.0)).unwrap().x - 3.0).abs() < 1e-5);
        assert!(VelocityField::new(&Mesh::default(), 0.25).is_none(), "no v, no field");

        let particles = || {
            let mut m = Mesh { vertices: vec![Vec3::new(0.7, 0.1, 0.0), Vec3::new(10.0, 0.0, 0.0)], ..Mesh::default() };
            tidy(&mut m);
            m.point_attributes.insert("life".into(), vec![10.0; 2]);
            m
        };
        let inputs = [smoke];
        let step = Step { time: 0.0, dt: 0.1, step: 1, frame_start: true, node_seed: 0, inputs: &inputs, colliders: &[] };
        let advect = |mode: f32| with(ADVECT, &[("input", &[0.0]), ("mode", &[mode])]);
        // Update velocity: pulled toward the flow, as wind pulls; the one far away isn't.
        let mut m = particles();
        apply(&advect(0.0), &mut m, &step, None);
        integrate(&mut m, 0.1);
        let v = &m.point_attributes["v"];
        assert!(v[1] > 0.5 && v[1] < 2.0 && v[4] == 0.0, "{v:?}");
        // Update force: pushed by it.
        let mut m = particles();
        apply(&advect(1.0), &mut m, &step, None);
        assert!((m.point_attributes["force"][1] - 2.0).abs() < 1e-5 && m.point_attributes["force"][4] == 0.0);
        // Update position: moved along it, velocity untouched.
        let mut m = particles();
        apply(&advect(2.0), &mut m, &step, None);
        assert!((m.vertices[0].y - 0.3).abs() < 1e-5 && m.point_attributes["v"][1] == 0.0);
    }
}
