// VOPs: node-based VEX. An Attribute VOP network is compiled into VEX code
// (one local variable per node, in dependency order) and run like a wrangle,
// as Houdini does, so every VEX function is available and the generated code
// can be inspected.

use std::collections::{HashMap, HashSet};

pub type NodeId = usize;

// Value types a VOP node can carry: (name, attribute prefix).
pub const TYPES: [(&str, char); 4] = [("float", 'f'), ("vector", 'v'), ("int", 'i'), ("vector4", 'p')];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Out {
    // The type chosen on the node (Bind, Constant, Parameter, Snippet).
    Chosen,
    // Vector if any of these inputs is a vector, else float (int if all int).
    Widest(&'static [usize]),
    Float,
    Vector,
    Int,
    // Exports produce no value.
    Nothing,
}

pub struct Op {
    pub label: &'static str,
    pub category: &'static str,
    // Input names, with the VEX used when an input isn't connected.
    pub inputs: &'static [(&'static str, &'static str)],
    out: Out,
    // VEX for the result, with {0}, {1}... for the inputs.
    template: &'static str,
    pub help: &'static str,
}

// Ops with special handling are identified by these indices.
pub const BIND: usize = 0;
pub const CONSTANT: usize = 1;
pub const PARAMETER: usize = 2;
pub const EXPORT: usize = 3;
pub const COMPONENT: usize = 4;
pub const COMPARE: usize = 5;
pub const SNIPPET: usize = 6;
// Ops that read an attribute named on the node (appended later in OPS).
pub const GET_ATTRIB: usize = 38;
pub const PC_AVERAGE: usize = 39;

// Menu order of the categories.
pub const CATEGORIES: [&str; 7] = ["Inputs", "Math", "Vector", "Geometry", "Noise", "Logic", "Outputs"];

pub const COMPARES: [&str; 6] = ["<", "<=", ">", ">=", "==", "!="];

pub const OPS: &[Op] = &[
    Op { label: "Bind", category: "Inputs", inputs: &[], out: Out::Chosen, template: "", help: "Reads an attribute (P, N, Cd, ptnum, Time, or any other)" },
    Op { label: "Constant", category: "Inputs", inputs: &[], out: Out::Chosen, template: "", help: "A fixed number or vector" },
    Op { label: "Parameter", category: "Inputs", inputs: &[], out: Out::Chosen, template: "", help: "A slider on the Attribute VOP node (ch)" },
    Op { label: "Export", category: "Outputs", inputs: &[("value", "0")], out: Out::Nothing, template: "", help: "Writes its input to an attribute" },
    Op { label: "Component", category: "Vector", inputs: &[("vector", "{0, 0, 0}")], out: Out::Float, template: "", help: "One component (x, y or z) of a vector" },
    Op { label: "Compare", category: "Logic", inputs: &[("a", "0"), ("b", "0")], out: Out::Int, template: "", help: "1 if the comparison holds, else 0" },
    Op { label: "Snippet", category: "Logic", inputs: &[("in0", "0"), ("in1", "0"), ("in2", "0"), ("in3", "0")], out: Out::Chosen, template: "", help: "A VEX expression using $0..$3 for its inputs" },
    Op { label: "Add", category: "Math", inputs: &[("a", "0"), ("b", "0")], out: Out::Widest(&[0, 1]), template: "{0} + {1}", help: "a + b" },
    Op { label: "Subtract", category: "Math", inputs: &[("a", "0"), ("b", "0")], out: Out::Widest(&[0, 1]), template: "{0} - {1}", help: "a - b" },
    Op { label: "Multiply", category: "Math", inputs: &[("a", "1"), ("b", "1")], out: Out::Widest(&[0, 1]), template: "{0} * {1}", help: "a * b" },
    Op { label: "Divide", category: "Math", inputs: &[("a", "1"), ("b", "1")], out: Out::Widest(&[0, 1]), template: "{0} / {1}", help: "a / b" },
    Op { label: "Negate", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "-{0}", help: "-a" },
    Op { label: "Absolute", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "abs({0})", help: "abs(a)" },
    Op { label: "Floor", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "floor({0})", help: "floor(a)" },
    Op { label: "Fraction", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "frac({0})", help: "frac(a)" },
    Op { label: "Power", category: "Math", inputs: &[("a", "0"), ("exponent", "2")], out: Out::Widest(&[0]), template: "pow({0}, {1})", help: "a to the power of b" },
    Op { label: "Square Root", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "sqrt({0})", help: "sqrt(a)" },
    Op { label: "Sine", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "sin({0})", help: "sin(a)" },
    Op { label: "Cosine", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "cos({0})", help: "cos(a)" },
    Op { label: "Minimum", category: "Math", inputs: &[("a", "0"), ("b", "0")], out: Out::Widest(&[0, 1]), template: "min({0}, {1})", help: "min(a, b)" },
    Op { label: "Maximum", category: "Math", inputs: &[("a", "0"), ("b", "0")], out: Out::Widest(&[0, 1]), template: "max({0}, {1})", help: "max(a, b)" },
    Op { label: "Mix", category: "Math", inputs: &[("a", "0"), ("b", "1"), ("bias", "0.5")], out: Out::Widest(&[0, 1]), template: "lerp({0}, {1}, {2})", help: "Blend from a to b by bias" },
    Op { label: "Clamp", category: "Math", inputs: &[("value", "0"), ("min", "0"), ("max", "1")], out: Out::Widest(&[0]), template: "clamp({0}, {1}, {2})", help: "Limit to min..max" },
    Op { label: "Fit Range", category: "Math", inputs: &[("value", "0"), ("srcmin", "0"), ("srcmax", "1"), ("destmin", "0"), ("destmax", "1")], out: Out::Widest(&[0]), template: "fit({0}, {1}, {2}, {3}, {4})", help: "Remap from one range to another" },
    Op { label: "Ramp Smooth", category: "Math", inputs: &[("min", "0"), ("max", "1"), ("value", "0")], out: Out::Float, template: "smooth({0}, {1}, {2})", help: "Smooth 0..1 step between min and max" },
    Op { label: "Dot Product", category: "Vector", inputs: &[("a", "{0, 0, 0}"), ("b", "{0, 0, 0}")], out: Out::Float, template: "dot({0}, {1})", help: "dot(a, b)" },
    Op { label: "Cross Product", category: "Vector", inputs: &[("a", "{1, 0, 0}"), ("b", "{0, 1, 0}")], out: Out::Vector, template: "cross({0}, {1})", help: "cross(a, b)" },
    Op { label: "Length", category: "Vector", inputs: &[("vector", "{0, 0, 0}")], out: Out::Float, template: "length({0})", help: "length(v)" },
    Op { label: "Normalize", category: "Vector", inputs: &[("vector", "{0, 1, 0}")], out: Out::Vector, template: "normalize({0})", help: "normalize(v)" },
    Op { label: "Distance", category: "Vector", inputs: &[("a", "{0, 0, 0}"), ("b", "{0, 0, 0}")], out: Out::Float, template: "distance({0}, {1})", help: "distance(a, b)" },
    Op { label: "Make Vector", category: "Vector", inputs: &[("x", "0"), ("y", "0"), ("z", "0")], out: Out::Vector, template: "set({0}, {1}, {2})", help: "Combine three floats" },
    Op { label: "Rotate", category: "Vector", inputs: &[("vector", "v@P"), ("angle", "0"), ("axis", "{0, 1, 0}")], out: Out::Vector, template: "qrotate(quaternion({1}, {2}), {0})", help: "Rotate around an axis (radians)" },
    Op { label: "Noise", category: "Noise", inputs: &[("position", "v@P"), ("frequency", "1"), ("offset", "0")], out: Out::Float, template: "noise({0} * {1} + {2})", help: "Perlin noise, 0..1" },
    Op { label: "Signed Noise", category: "Noise", inputs: &[("position", "v@P"), ("frequency", "1"), ("offset", "0")], out: Out::Float, template: "snoise({0} * {1} + {2})", help: "Perlin noise, -1..1" },
    Op { label: "Curl Noise", category: "Noise", inputs: &[("position", "v@P"), ("frequency", "1"), ("offset", "0")], out: Out::Vector, template: "curlnoise({0} * {1} + {2})", help: "Swirling, divergence-free vector noise" },
    Op { label: "Worley Noise", category: "Noise", inputs: &[("position", "v@P"), ("frequency", "1"), ("seed", "0")], out: Out::Float, template: "wnoise({0} * {1}, {2})", help: "Cellular noise: distance to the nearest feature" },
    Op { label: "Random", category: "Noise", inputs: &[("seed", "@ptnum"), ("seed2", "0")], out: Out::Float, template: "rand({0}, {1})", help: "Random 0..1 from seeds" },
    Op { label: "Switch", category: "Logic", inputs: &[("condition", "0"), ("if true", "1"), ("if false", "0")], out: Out::Widest(&[1, 2]), template: "select({0}, {1}, {2})", help: "Pick one of two values" },
    // New ops go at the end: scenes store the op's position in this list.
    Op { label: "Get Attribute", category: "Geometry", inputs: &[("point", "@ptnum")], out: Out::Chosen, template: "", help: "Reads an attribute from any point of the input (by number)" },
    Op { label: "Point Cloud Average", category: "Geometry", inputs: &[("position", "v@P"), ("radius", "0.5"), ("max points", "10")], out: Out::Chosen, template: "", help: "Distance-weighted average of an attribute over nearby points" },
    Op { label: "Nearest Point", category: "Geometry", inputs: &[("position", "v@P")], out: Out::Int, template: "nearpoint(0, {0})", help: "Number of the closest point" },
    Op { label: "Distance to Surface", category: "Geometry", inputs: &[("position", "v@P")], out: Out::Float, template: "xyzdist(0, {0})", help: "Distance to the input's surface" },
    Op { label: "Closest Point on Surface", category: "Geometry", inputs: &[("position", "v@P")], out: Out::Vector, template: "minpos(0, {0})", help: "The nearest position on the input's surface" },
    Op { label: "Neighbour Count", category: "Geometry", inputs: &[("point", "@ptnum")], out: Out::Int, template: "neighbourcount(0, {0})", help: "How many points are connected to this one" },
    Op { label: "Point Count", category: "Geometry", inputs: &[], out: Out::Int, template: "npoints(0)", help: "Number of points in the input" },
    Op { label: "Bounding Box Centre", category: "Geometry", inputs: &[], out: Out::Vector, template: "getbbox_center(0)", help: "Centre of the input's bounding box" },
    Op { label: "Bounding Box Size", category: "Geometry", inputs: &[], out: Out::Vector, template: "getbbox_size(0)", help: "Size of the input's bounding box" },
    Op { label: "Relative to Bounding Box", category: "Geometry", inputs: &[("position", "v@P")], out: Out::Vector, template: "relpointbbox(0, {0})", help: "Where a position sits inside the bounding box, 0..1 per axis" },
    Op { label: "Modulo", category: "Math", inputs: &[("a", "0"), ("b", "1")], out: Out::Widest(&[0, 1]), template: "{0} % {1}", help: "Remainder of a / b" },
    Op { label: "Round", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "round({0})", help: "round(a)" },
    Op { label: "Ceiling", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "ceil({0})", help: "ceil(a)" },
    Op { label: "Exponential", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "exp({0})", help: "e to the power of a" },
    Op { label: "Logarithm", category: "Math", inputs: &[("a", "1")], out: Out::Widest(&[0]), template: "log({0})", help: "Natural logarithm" },
    Op { label: "Tangent", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "tan({0})", help: "tan(a)" },
    Op { label: "Arctangent2", category: "Math", inputs: &[("y", "0"), ("x", "1")], out: Out::Float, template: "atan2({0}, {1})", help: "Angle of (x, y) in radians" },
    Op { label: "Average", category: "Math", inputs: &[("a", "0"), ("b", "0")], out: Out::Widest(&[0, 1]), template: "({0} + {1}) * 0.5", help: "(a + b) / 2" },
    Op { label: "Sign", category: "Math", inputs: &[("a", "0")], out: Out::Widest(&[0]), template: "sign({0})", help: "-1, 0 or 1" },
    Op { label: "Reflect", category: "Vector", inputs: &[("direction", "{0, -1, 0}"), ("normal", "{0, 1, 0}")], out: Out::Vector, template: "reflect({0}, {1})", help: "Mirror a direction about a normal" },
    Op { label: "Alligator Noise", category: "Noise", inputs: &[("position", "v@P"), ("frequency", "1"), ("offset", "0")], out: Out::Float, template: "anoise({0} * {1} + {2})", help: "Ridged, billowy noise, 0..1" },
    Op { label: "And", category: "Logic", inputs: &[("a", "1"), ("b", "1")], out: Out::Int, template: "{0} && {1}", help: "1 if both are true" },
    Op { label: "Or", category: "Logic", inputs: &[("a", "0"), ("b", "0")], out: Out::Int, template: "{0} || {1}", help: "1 if either is true" },
    Op { label: "Not", category: "Logic", inputs: &[("a", "0")], out: Out::Int, template: "!{0}", help: "1 if false, 0 if true" },
];

#[derive(Clone, Debug, PartialEq)]
pub struct VopNode {
    pub op: usize,
    // Attribute or parameter name (Bind, Export, Parameter), or the Snippet expression.
    pub name: String,
    // Index into TYPES (Bind, Constant, Parameter, Export, Snippet); the axis for Component;
    // the comparison for Compare.
    pub ty: usize,
    // Constant's value, or a Parameter's default.
    pub value: [f32; 3],
}

impl VopNode {
    pub fn new(op: usize) -> Self {
        let (name, ty) = match op {
            BIND | EXPORT => ("P".to_owned(), 1),
            GET_ATTRIB | PC_AVERAGE => ("P".to_owned(), 1),
            PARAMETER => ("amount".to_owned(), 0),
            SNIPPET => ("$0 * 2".to_owned(), 0),
            _ => (String::new(), 0),
        };
        VopNode { op: op.min(OPS.len() - 1), name, ty, value: if op == CONSTANT { [1.0, 0.0, 0.0] } else { [0.0; 3] } }
    }

    pub fn def(&self) -> &'static Op {
        &OPS[self.op.min(OPS.len() - 1)]
    }

    pub fn label(&self, id: NodeId) -> String {
        match self.op {
            BIND => format!("bind {}", self.name),
            EXPORT => format!("export {}", self.name),
            PARAMETER => format!("ch {}", self.name),
            GET_ATTRIB => format!("get {}", self.name),
            PC_AVERAGE => format!("pc average {}", self.name),
            CONSTANT if self.ty == 1 => format!("({}, {}, {})", self.value[0], self.value[1], self.value[2]),
            CONSTANT => format!("{}", self.value[0]),
            COMPONENT => format!(".{}", ["x", "y", "z"][self.ty.min(2)]),
            COMPARE => format!("compare {}", COMPARES[self.ty.min(5)]),
            _ => format!("{}{id}", self.def().label.to_lowercase().replace(' ', "_")),
        }
    }

    fn chosen(&self) -> usize {
        self.ty.min(TYPES.len() - 1)
    }
}

fn literal_type(vex: &str) -> usize {
    if vex.starts_with('{') || vex.starts_with("v@") {
        1
    } else if vex.starts_with('@') && !vex.contains('.') {
        2
    } else {
        0
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
}

fn number(v: f32) -> String {
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") }
}

// Compiles the network: `nodes` are the VOPs inside one Attribute VOP and
// `input(node, slot)` the node wired into an input. Only nodes that feed an
// Export are used.
pub fn compile(nodes: &HashMap<NodeId, VopNode>, input: impl Fn(NodeId, usize) -> Option<NodeId>) -> Result<String, String> {
    let mut exports: Vec<NodeId> = nodes.iter().filter(|(_, n)| n.op == EXPORT).map(|(id, _)| *id).collect();
    exports.sort_unstable();
    if exports.is_empty() {
        return Err("the network has no Export node, so it doesn't change anything".into());
    }
    // Depth-first ordering, inputs before the nodes that use them.
    let mut order = Vec::new();
    let (mut done, mut active) = (HashSet::new(), HashSet::new());
    fn visit(
        id: NodeId,
        nodes: &HashMap<NodeId, VopNode>,
        input: &dyn Fn(NodeId, usize) -> Option<NodeId>,
        done: &mut HashSet<NodeId>,
        active: &mut HashSet<NodeId>,
        order: &mut Vec<NodeId>,
    ) -> Result<(), String> {
        if done.contains(&id) {
            return Ok(());
        }
        if !active.insert(id) {
            return Err("the network has a loop: a node depends on its own result".into());
        }
        let node = nodes.get(&id).ok_or("a wire comes from outside the VOP network")?;
        for slot in 0..node.def().inputs.len() {
            if let Some(src) = input(id, slot) {
                visit(src, nodes, input, done, active, order)?;
            }
        }
        active.remove(&id);
        done.insert(id);
        order.push(id);
        Ok(())
    }
    for &id in &exports {
        visit(id, nodes, &input, &mut done, &mut active, &mut order)?;
    }
    let mut types: HashMap<NodeId, usize> = HashMap::new();
    let mut lines = vec!["// Generated by Attribute VOP".to_owned()];
    for id in order {
        let node = &nodes[&id];
        let def = node.def();
        // Each input: the wired node's variable, or the op's default.
        let args: Vec<(String, usize)> = def
            .inputs
            .iter()
            .enumerate()
            .map(|(slot, (_, default))| match input(id, slot) {
                Some(src) => (format!("v{src}"), types[&src]),
                None => (default.to_string(), literal_type(default)),
            })
            .collect();
        let ty = match def.out {
            Out::Chosen => node.chosen(),
            Out::Float => 0,
            Out::Vector => 1,
            Out::Int => 2,
            Out::Nothing => 0,
            Out::Widest(which) => {
                let tys: Vec<usize> = which.iter().filter_map(|&i| args.get(i).map(|a| a.1)).collect();
                if tys.contains(&1) {
                    1
                } else if !tys.is_empty() && tys.iter().all(|t| *t == 2) {
                    2
                } else {
                    0
                }
            }
        };
        let (type_name, prefix) = TYPES[ty];
        let expr = match node.op {
            BIND | PARAMETER | EXPORT | GET_ATTRIB | PC_AVERAGE if !valid_name(&node.name) => {
                return Err(format!("'{}' isn't a valid {} name", node.name, if node.op == PARAMETER { "parameter" } else { "attribute" }));
            }
            BIND => format!("{prefix}@{}", node.name),
            CONSTANT => match ty {
                1 => format!("{{{}, {}, {}}}", number(node.value[0]), number(node.value[1]), number(node.value[2])),
                2 => format!("{}", node.value[0].round() as i32),
                3 => format!("{{{}, {}, {}, 1.0}}", number(node.value[0]), number(node.value[1]), number(node.value[2])),
                _ => number(node.value[0]),
            },
            PARAMETER => match ty {
                1 | 3 => format!("chv(\"{}\")", node.name),
                2 => format!("chi(\"{}\")", node.name),
                _ => format!("chf(\"{}\")", node.name),
            },
            EXPORT => {
                let (out_type, out_prefix) = TYPES[node.chosen()];
                lines.push(format!("{out_prefix}@{} = {}({});", node.name, out_type, args[0].0));
                types.insert(id, node.chosen());
                continue;
            }
            COMPONENT => format!("getcomp({}, {})", args[0].0, node.ty.min(2)),
            GET_ATTRIB => format!("point(0, \"{}\", {})", node.name, args[0].0),
            PC_AVERAGE => format!("pcfilter(pcopen(0, \"P\", {}, {}, {}), \"{}\")", args[0].0, args[1].0, args[2].0, node.name),
            COMPARE => format!("{} {} {}", args[0].0, COMPARES[node.ty.min(5)], args[1].0),
            SNIPPET => {
                if node.name.trim().is_empty() {
                    return Err("a Snippet node is empty".into());
                }
                let mut code = node.name.clone();
                for (i, (arg, _)) in args.iter().enumerate().rev() {
                    code = code.replace(&format!("${i}"), &format!("({arg})"));
                }
                code
            }
            _ => {
                let mut code = def.template.to_owned();
                for (i, (arg, _)) in args.iter().enumerate() {
                    code = code.replace(&format!("{{{i}}}"), arg);
                }
                code
            }
        };
        lines.push(format!("{type_name} v{id} = {expr};"));
        types.insert(id, ty);
    }
    Ok(lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(name: &str) -> usize {
        OPS.iter().position(|o| o.label == name).unwrap()
    }

    #[test]
    fn compiles_in_dependency_order_with_types() {
        let mut nodes = HashMap::new();
        nodes.insert(1, VopNode::new(BIND));
        nodes.insert(2, VopNode { name: "amp".into(), ..VopNode::new(PARAMETER) });
        nodes.insert(3, VopNode::new(op("Noise")));
        nodes.insert(4, VopNode::new(op("Multiply")));
        nodes.insert(5, VopNode::new(op("Add")));
        nodes.insert(6, VopNode::new(EXPORT));
        // P + noise(P) * amp -> P
        let links: HashMap<(NodeId, usize), NodeId> = [((3, 0), 1), ((4, 0), 3), ((4, 1), 2), ((5, 0), 1), ((5, 1), 4), ((6, 0), 5)].into_iter().collect();
        let code = compile(&nodes, |id, slot| links.get(&(id, slot)).copied()).unwrap();
        let expected = [
            "// Generated by Attribute VOP",
            "vector v1 = v@P;",
            "float v3 = noise(v1 * 1 + 0);",
            "float v2 = chf(\"amp\");",
            "float v4 = v3 * v2;",
            "vector v5 = v1 + v4;",
            "v@P = vector(v5);",
        ];
        assert_eq!(code, expected.join("\n") + "\n");
        // Unconnected inputs use their defaults; nodes not feeding an export are left out.
        nodes.insert(7, VopNode::new(op("Cosine")));
        let code = compile(&nodes, |id, slot| links.get(&(id, slot)).copied()).unwrap();
        assert!(!code.contains("v7"));
    }

    #[test]
    fn special_nodes_and_errors() {
        let mut nodes = HashMap::new();
        nodes.insert(1, VopNode { ty: 1, value: [1.0, 2.0, 3.0], ..VopNode::new(CONSTANT) });
        nodes.insert(2, VopNode { ty: 2, ..VopNode::new(COMPONENT) });
        nodes.insert(3, VopNode { name: "$0 * 10 + $1".into(), ..VopNode::new(SNIPPET) });
        nodes.insert(4, VopNode { ty: 2, ..VopNode::new(COMPARE) });
        nodes.insert(5, VopNode { name: "result".into(), ty: 0, ..VopNode::new(EXPORT) });
        let links: HashMap<(NodeId, usize), NodeId> = [((2, 0), 1), ((3, 0), 2), ((4, 0), 3), ((5, 0), 4)].into_iter().collect();
        let code = compile(&nodes, |id, slot| links.get(&(id, slot)).copied()).unwrap();
        assert!(code.contains("vector v1 = {1.0, 2.0, 3.0};"), "{code}");
        assert!(code.contains("float v2 = getcomp(v1, 2);"));
        assert!(code.contains("float v3 = (v2) * 10 + (0);"));
        assert!(code.contains("int v4 = v3 > 0;"));
        assert!(code.contains("f@result = float(v4);"));
        // Ops that read a named attribute, and every op has a valid category.
        let mut reads = HashMap::new();
        reads.insert(1, VopNode { name: "Cd".into(), ..VopNode::new(GET_ATTRIB) });
        reads.insert(2, VopNode { name: "Cd".into(), ..VopNode::new(PC_AVERAGE) });
        reads.insert(3, VopNode::new(op("Add")));
        reads.insert(4, VopNode { name: "Cd".into(), ..VopNode::new(EXPORT) });
        let wires: HashMap<(NodeId, usize), NodeId> = [((3, 0), 1), ((3, 1), 2), ((4, 0), 3)].into_iter().collect();
        let code = compile(&reads, |id, slot| wires.get(&(id, slot)).copied()).unwrap();
        assert!(code.contains("vector v1 = point(0, \"Cd\", @ptnum);"), "{code}");
        assert!(code.contains("vector v2 = pcfilter(pcopen(0, \"P\", v@P, 0.5, 10), \"Cd\");"), "{code}");
        assert_eq!(OPS[GET_ATTRIB].label, "Get Attribute");
        assert_eq!(OPS[PC_AVERAGE].label, "Point Cloud Average");
        assert!(OPS.iter().all(|o| CATEGORIES.contains(&o.category)));
        // Errors.
        assert!(compile(&HashMap::new(), |_, _| None).unwrap_err().contains("no Export"));
        let mut looped = HashMap::new();
        looped.insert(1, VopNode::new(op("Add")));
        looped.insert(2, VopNode::new(EXPORT));
        let cycle: HashMap<(NodeId, usize), NodeId> = [((1, 0), 1), ((2, 0), 1)].into_iter().collect();
        assert!(compile(&looped, |id, slot| cycle.get(&(id, slot)).copied()).unwrap_err().contains("loop"));
        let mut bad = HashMap::new();
        bad.insert(1, VopNode { name: "my attr".into(), ..VopNode::new(EXPORT) });
        assert!(compile(&bad, |_, _| None).unwrap_err().contains("valid attribute name"));
    }
}
