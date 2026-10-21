// AI scene building: what the model is told and what it can do.
//
// - catalog(): every geometry (SOP) node it can create, with its inputs,
//   parameters (by name, with defaults) and a description from the docs.
// - describe(): the current scene, compactly: objects, their nodes, changed
//   parameters, wiring and display flags.
// - apply(): a batch of edits (create objects and nodes, set parameters by
//   name, wire, delete, set the display), validated against the graph; each
//   problem comes back as a message the model can act on.
// - inspect(): cooks a node and reports what came out (errors, counts, bounds,
//   attributes, groups), so the model can check its work.
//
// Nodes are named by the ids the model gives them in a batch, by their name
// in the network ("scatter3"), or by number ("#12").

use super::*;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

pub struct NodeSpec {
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    // None: any number (Merge).
    pub inputs: Option<usize>,
    pub input_names: Vec<&'static str>,
    pub outputs: usize,
    pub output_names: Vec<&'static str>,
    // (name, default)
    pub params: Vec<(String, f32)>,
    pub texts: Vec<String>,
}

// Geometry nodes, the ones a geometry object's network can hold.
fn is_sop(kind: &NodeKind) -> bool {
    !kind.is_object()
        && !kind.is_material()
        && !kind.is_render()
        && !kind.is_lop()
        && !matches!(kind, NodeKind::ShaderNode(_) | NodeKind::CopNetwork | NodeKind::Cop(_) | NodeKind::TopNetwork | NodeKind::Top(_) | NodeKind::Vop(_) | NodeKind::SolverInput { .. })
}

// The first menu template of each geometry node type: its defaults.
fn templates() -> Vec<(&'static str, NodeKind)> {
    let mut seen = std::collections::HashSet::new();
    NodeKind::templates()
        .into_iter()
        .flat_map(|(category, kinds)| kinds.into_iter().map(move |k| (category, k)))
        .filter(|(_, k)| is_sop(k) && seen.insert(k.name()))
        .collect()
}

fn template(name: &str) -> Option<NodeKind> {
    templates().into_iter().map(|(_, k)| k).find(|k| k.name() == name || k.menu_name() == name)
}

// Names of a node's text parameters, in texts() order.
fn text_names(kind: &NodeKind) -> Vec<String> {
    let names: &[&str] = match kind {
        NodeKind::Wrangle { .. } => return vec!["code".into()],
        NodeKind::FileImport { .. } | NodeKind::FileExport { .. } | NodeKind::UvQuickShade { .. } => &["file"],
        NodeKind::AttributePaint { .. } => &["attribute"],
        NodeKind::ForEachEnd(_) => &["attribute"],
        NodeKind::Measure(_) => &["name", "source"],
        NodeKind::Extrude(_) => &["front_group", "side_group", "back_group"],
        NodeKind::GroupCreate(_) => &["name", "expression"],
        NodeKind::AttributeRandomize(_) => &["name"],
        NodeKind::Connectivity(_) => &["attribute"],
        _ => &[],
    };
    let count = kind.texts().len();
    (0..count).map(|i| names.get(i).map_or_else(|| format!("text{}", i + 1), |n| n.to_string())).collect()
}

const ATTRIBUTES: [&str; 4] = ["id", "pscale", "Cd", "orient"];
const CLASS: [&str; 2] = ["Points", "Primitives"];

// Parameters that pick one of several options: their choices, in order (0, 1, 2…).
// The model sets them by name ("attribute": "Custom") or number.
fn options(node: &str, param: &str) -> Option<&'static [&'static str]> {
    Some(match (node, param) {
        ("attribute_randomize", "attribute") => &randomize::PRESETS,
        ("attribute_randomize", "class") => &randomize::CLASSES,
        ("attribute_randomize", "mode") => &randomize::MODES,
        ("attribute_create" | "attribute_delete", "attribute") => &ATTRIBUTES,
        ("boolean", "operation") => &boolean::OPERATIONS,
        ("boolean", "method") => &boolean::MODES,
        ("material_fracture", "material") => &fracture::MATERIALS,
        ("measure", "measure") => &measure::KINDS,
        ("measure", "curvature") => &measure::CURVATURES,
        ("measure", "summary") => &measure::SUMMARIES,
        ("extract_centroid", "partition") => &sopkit::PARTITIONS,
        ("extract_centroid", "method") => &sopkit::CENTRES,
        ("match_size", "justify.x" | "justify.y" | "justify.z") => &sopkit::JUSTIFY,
        ("group_create", "primitives") => &CLASS,
        ("group_create", "method") => &groups::CREATE_METHODS,
        ("group_create", "combine") => &groups::COMBINE_MODES,
        ("pack", "pack") => &packed::MODES,
        ("pack", "pivot") => &packed::PIVOTS,
        ("terrain_noise", "noise") => &terrain::NOISE_TYPES,
        ("terrain_noise", "blend") => &terrain::BLEND_MODES,
        ("terrain_mask", "source") => &terrain::MASK_SOURCES,
        ("terrain_mask", "combine") => &terrain::MASK_COMBINE,
        ("normal", "domain") => &sops::NORMAL_DOMAINS,
        ("normal", "weighting") => &sops::NORMAL_WEIGHTS,
        ("uv_texture", "projection") => &sops::UV_PROJECTIONS,
        ("uv_texture" | "mirror" | "clip", "axis") => &sops::AXES,
        ("blast", "component") => &["Points", "Primitives", "Edges"],
        ("connectivity", "class") => &CLASS,
        _ => return None,
    })
}

// "Custom" for attribute_randomize's attribute, from a name the model gives: the
// option itself, its first word, or the start of it (any case).
fn option_index(choices: &[&str], wanted: &str) -> Option<usize> {
    let w = wanted.trim().to_lowercase();
    let lower: Vec<String> = choices.iter().map(|c| c.to_lowercase()).collect();
    lower
        .iter()
        .position(|c| *c == w)
        .or_else(|| lower.iter().position(|c| c.split([' ', '(']).next() == Some(w.as_str())))
        .or_else(|| (w.len() >= 3).then(|| lower.iter().position(|c| c.starts_with(&w))).flatten())
}

// A parameter's value as the model reads it: an option's name, else the number.
fn shown_value(node: &str, param: &str, v: f32) -> String {
    match options(node, param).and_then(|o| o.get(v.max(0.0) as usize)) {
        Some(label) => format!("{label:?}"),
        None => fmt_value(v),
    }
}

// Parameters the node ignores in its current setting, with why: a change to
// one of them is reported back, so the model notices it did nothing.
fn ignored(kind: &NodeKind) -> Vec<(&'static str, &'static str)> {
    match kind {
        NodeKind::AttributeRandomize(r) if r.preset != randomize::CUSTOM => ["class", "size", "min", "max", "mode", "name"].map(|p| (p, "it only applies when attribute is \"Custom\" (the presets use amount)")).to_vec(),
        NodeKind::AttributeRandomize(_) => vec![("amount", "it only applies to the presets; a Custom attribute uses min and max")],
        NodeKind::Resample(r) if r.by_length => vec![("segments", "by_length is on, so length decides")],
        NodeKind::Resample(_) => vec![("length", "by_length is off, so segments decides")],
        _ => Vec::new(),
    }
}

// What the model needs to know beyond the parameter list, from what went wrong
// in testing. One line each; shown in the catalog.
fn note(node: &str) -> Option<&'static str> {
    Some(match node {
        "copy_to_points" => "copies keep their own up direction (the points' N isn't used): set orient on the points (a quaternion, or degrees about Y) to tilt or turn copies; pscale scales each copy",
        "scatter" => "its points get N (the surface normal) and pscale (half their typical spacing), which Copy to Points then uses as the copy size: set pscale yourself to control it",
        "attribute_randomize" => "min, max, class, size and name only apply when attribute is \"Custom\"; the presets use amount (pscale adds a random value between -amount and +amount)",
        "wrangle" => "run_over chooses what the code runs once per: \"points\" (the default), \"primitives\" (polygons), \"detail\" (once in all) or \"vertices\"",
        "material_fracture" => "glue_strength is how hard the glue between pieces is to break: the default 200 holds through a few-units drop; single digits shatter on impact. Feed all three outputs to rbd_bullet_solver's first three inputs",
        "rbd_bullet_solver" => "inputs: material_fracture's outputs 1, 2, 3 into inputs 1, 2, 3; optional static collision geometry into input 4. glue_strength_scale multiplies every glue strength. ground adds a floor at ground_height",
        "merge" => "connect inputs 1, 2, 3, … as many as needed",
        "xform" => "translate tx ty tz, rotate rx ry rz (degrees), scale sx sy sz; accepts t, r, s as arrays",
        "terrain" => "a flat terrain grid to shape with terrain_noise and the other terrain nodes",
        _ => return None,
    })
}

fn spec(category: &'static str, kind: &NodeKind) -> NodeSpec {
    let (input_names, output_names) = kind.port_names();
    let params = kind.param_names().into_iter().zip(kind.params()).filter(|(n, _)| !n.is_empty()).collect();
    let mut params: Vec<(String, f32)> = params;
    if let NodeKind::Wrangle { domain, .. } = kind {
        params.push(("run_over".into(), *domain as f32));
    }
    NodeSpec {
        name: kind.name(),
        category,
        description: docs::node_doc(kind.name()),
        inputs: if kind.is_multi_input() { None } else { Some(kind.num_inputs()) },
        input_names: input_names.to_vec(),
        outputs: kind.num_outputs(),
        output_names: output_names.to_vec(),
        params,
        texts: text_names(kind),
    }
}

pub fn catalog() -> Vec<NodeSpec> {
    templates().into_iter().map(|(category, kind)| spec(category, &kind)).collect()
}

fn fmt_value(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e7 { format!("{}", v as i64) } else { format!("{v:.4}").trim_end_matches('0').to_owned() }
}

// The catalog as compact text, one node per line (the model's reference).
pub fn catalog_text() -> String {
    let mut out = String::new();
    let mut category = "";
    for s in catalog() {
        if s.category != category {
            category = s.category;
            let _ = writeln!(out, "\n## {category}");
        }
        let ports = |n: usize, names: &[&str]| if names.is_empty() { n.to_string() } else { format!("{n} ({})", names.join(", ")) };
        let inputs = s.inputs.map_or("any number".to_owned(), |n| ports(n, &s.input_names));
        let params: Vec<String> = s
            .params
            .iter()
            .map(|(n, v)| match options(s.name, n) {
                Some(choices) => format!("{n}={}{{{}}}", shown_value(s.name, n, *v), choices.join("|")),
                None => format!("{n}={}", fmt_value(*v)),
            })
            .collect();
        let _ = write!(out, "- {}: {} | inputs: {inputs}", s.name, if s.description.is_empty() { "-" } else { s.description });
        if s.outputs > 1 {
            let _ = write!(out, " | outputs: {}", ports(s.outputs, &s.output_names));
        }
        if !params.is_empty() {
            let _ = write!(out, " | params: {}", params.join(" "));
        }
        if !s.texts.is_empty() {
            let _ = write!(out, " | text: {}", s.texts.join(" "));
        }
        if let Some(n) = note(s.name) {
            let _ = write!(out, " | note: {n}");
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Scene description
// ---------------------------------------------------------------------------

fn sorted_ids(graph: &Graph, pick: impl Fn(&Node) -> bool) -> Vec<NodeId> {
    let mut ids: Vec<NodeId> = graph.nodes.iter().filter(|(_, n)| pick(n)).map(|(id, _)| *id).collect();
    ids.sort_unstable();
    ids
}

// Parameters that differ from the node type's defaults, as name=value.
fn changed_params(kind: &NodeKind) -> Vec<String> {
    let defaults = template(kind.name()).map(|t| t.params()).unwrap_or_default();
    let mut out: Vec<String> = kind
        .param_names()
        .into_iter()
        .zip(kind.params())
        .enumerate()
        .filter(|(i, (n, v))| !n.is_empty() && defaults.get(*i).is_none_or(|d| (d - v).abs() > 1e-6))
        .map(|(_, (n, v))| format!("{n}={}", shown_value(kind.name(), &n, v)))
        .collect();
    if let NodeKind::Wrangle { code, domain } = kind {
        out.push(format!("run_over={domain}"));
        out.push(format!("code={code:?}"));
    }
    for (name, text) in text_names(kind).into_iter().zip(kind.texts()).filter(|(_, t)| !t.is_empty()) {
        if name != "code" {
            out.push(format!("{name}={text:?}"));
        }
    }
    out
}

// The scene's geometry objects and their networks.
pub fn describe(graph: &Graph) -> String {
    let objects = sorted_ids(graph, |n| matches!(n.kind, NodeKind::Geometry { .. }) && n.parent.is_none());
    if objects.is_empty() {
        return "The scene has no geometry objects.".into();
    }
    let mut out = String::new();
    for object in objects {
        let o = &graph.nodes[&object];
        let display = graph.display_of(object).map_or("none".to_owned(), |d| graph.nodes[&d].title(d));
        let _ = writeln!(out, "object {} (#{object}), display: {display}", o.title(object));
        let mut members: Vec<NodeId> = graph.nodes.keys().copied().filter(|id| graph.enclosing_geometry(*id) == Some(object)).collect();
        members.sort_unstable();
        for id in members {
            let n = &graph.nodes[&id];
            let mut line = format!("  {} (#{id}) {}", n.title(id), n.kind.name());
            let params = changed_params(&n.kind);
            if !params.is_empty() {
                let _ = write!(line, " [{}]", params.join(" "));
            }
            if !n.group.is_empty() {
                let _ = write!(line, " group={:?}", n.group.pattern);
            }
            let mut inputs: Vec<&Link> = graph.links.iter().filter(|l| l.to == id).collect();
            inputs.sort_by_key(|l| l.slot);
            if !inputs.is_empty() {
                let wires: Vec<String> = inputs
                    .iter()
                    .map(|l| {
                        let from = graph.nodes.get(&l.from).map_or(format!("#{}", l.from), |f| f.title(l.from));
                        if l.out == 0 { format!("{}<-{from}", l.slot + 1) } else { format!("{}<-{from}:{}", l.slot + 1, l.out + 1) }
                    })
                    .collect();
                let _ = write!(line, " inputs: {}", wires.join(" "));
            }
            if n.bypass {
                line.push_str(" (bypassed)");
            }
            let _ = writeln!(out, "{line}");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

fn one() -> usize {
    1
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    // A new geometry object; `id` names it for the rest of the batch.
    CreateObject {
        id: String,
        #[serde(default)]
        name: Option<String>,
    },
    // A node inside `object` (an id from this batch, or an existing object).
    AddNode {
        id: String,
        #[serde(rename = "type")]
        kind: String,
        object: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        params: BTreeMap<String, serde_json::Value>,
        #[serde(default)]
        group: Option<String>,
    },
    SetParams {
        node: String,
        #[serde(default)]
        params: BTreeMap<String, serde_json::Value>,
        #[serde(default)]
        group: Option<String>,
    },
    // Inputs and outputs count from 1.
    Connect {
        from: String,
        to: String,
        #[serde(default = "one")]
        input: usize,
        #[serde(default = "one")]
        output: usize,
    },
    Disconnect {
        node: String,
        input: usize,
    },
    Delete {
        node: String,
    },
    SetDisplay {
        node: String,
    },
}

impl Edit {
    fn label(&self) -> String {
        match self {
            Edit::CreateObject { id, .. } => format!("create_object {id}"),
            Edit::AddNode { id, kind, .. } => format!("add_node {id} ({kind})"),
            Edit::SetParams { node, .. } => format!("set_params {node}"),
            Edit::Connect { from, to, .. } => format!("connect {from} -> {to}"),
            Edit::Disconnect { node, .. } => format!("disconnect {node}"),
            Edit::Delete { node } => format!("delete {node}"),
            Edit::SetDisplay { node } => format!("set_display {node}"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    // Batch ids and the nodes they became.
    pub ids: BTreeMap<String, NodeId>,
    pub errors: Vec<String>,
    // Changes that were made but have no effect.
    pub notes: Vec<String>,
    pub applied: usize,
}

impl Report {
    // For the model: what was created and what went wrong.
    pub fn to_text(&self, graph: &Graph) -> String {
        let mut out = format!("{} edit(s) applied.", self.applied);
        if !self.ids.is_empty() {
            let made: Vec<String> = self.ids.iter().map(|(k, id)| format!("{k} = {} (#{id})", graph.nodes.get(id).map_or(String::new(), |n| n.title(*id)))).collect();
            let _ = write!(out, " Created: {}.", made.join(", "));
        }
        for e in &self.errors {
            let _ = write!(out, "\nError: {e}");
        }
        for n in &self.notes {
            let _ = write!(out, "\nNote: {n}");
        }
        out
    }
}

struct Resolver<'a> {
    graph: &'a Graph,
    ids: &'a BTreeMap<String, NodeId>,
}

impl Resolver<'_> {
    fn node(&self, name: &str) -> Result<NodeId, String> {
        let name = name.trim();
        if let Some(id) = self.ids.get(name) {
            return Ok(*id);
        }
        if let Some(id) = name.strip_prefix('#').and_then(|n| n.parse::<NodeId>().ok()).filter(|id| self.graph.nodes.contains_key(id)) {
            return Ok(id);
        }
        let matches: Vec<NodeId> = sorted_ids(self.graph, |_| true).into_iter().filter(|id| self.graph.nodes[id].title(*id) == name).collect();
        match matches.as_slice() {
            [one] => Ok(*one),
            [] => Err(format!("no node named '{name}'")),
            _ => Err(format!("several nodes are named '{name}': use #id")),
        }
    }

    fn object(&self, name: &str) -> Result<NodeId, String> {
        let id = self.node(name)?;
        match self.graph.nodes[&id].kind {
            NodeKind::Geometry { .. } => Ok(id),
            _ => Err(format!("'{name}' isn't a geometry object")),
        }
    }
}

// Sets parameters by name on `kind`. Arrays fill vector parameters (t: [1, 2, 3]
// sets tx ty tz, or t.x t.y t.z); strings set text parameters; booleans are 1/0.
// Returns notes about values that have no effect as the node is set up.
fn set_params(kind: &mut NodeKind, values: &BTreeMap<String, serde_json::Value>) -> Result<Vec<String>, String> {
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let names = kind.param_names();
    let mut params = kind.params();
    let tnames = text_names(kind);
    let mut texts = kind.texts();
    let mut wrangle_code: Option<String> = None;
    let mut wrangle_domain: Option<usize> = None;
    let index = |n: &str| names.iter().position(|p| p == n);
    let number = |name: &str, v: &serde_json::Value| -> Result<f32, String> {
        match v {
            serde_json::Value::Number(n) => n.as_f64().map(|f| f as f32).ok_or_else(|| format!("'{name}': not a number")),
            serde_json::Value::Bool(b) => Ok(*b as u8 as f32),
            other => Err(format!("'{name}' takes a number, got {other}")),
        }
    };
    for (name, value) in values {
        if matches!(kind, NodeKind::Wrangle { .. }) {
            match name.as_str() {
                "code" => {
                    wrangle_code = Some(value.as_str().ok_or("'code' takes VEX source text")?.to_owned());
                    continue;
                }
                "run_over" => {
                    let domain = match value {
                        serde_json::Value::String(s) => ["detail", "primitives", "points", "vertices"].iter().position(|d| s.starts_with(&d[..4])).ok_or(format!("run_over: '{s}' (use points, primitives, detail or vertices)"))?,
                        v => number(name, v)? as usize,
                    };
                    wrangle_domain = Some(domain.min(3));
                    continue;
                }
                _ => {}
            }
        }
        if let Some(t) = tnames.iter().position(|t| t == name) {
            texts[t] = value.as_str().ok_or_else(|| format!("'{name}' takes text"))?.to_owned();
            continue;
        }
        if let Some(list) = value.as_array() {
            let suffixes: [&[&str]; 3] = [&[".x", ".y", ".z"], &[".r", ".g", ".b"], &["x", "y", "z"]];
            let targets = suffixes
                .iter()
                .map(|s| s.iter().take(list.len()).map(|x| index(&format!("{name}{x}"))).collect::<Option<Vec<usize>>>())
                .find_map(|t| t)
                .ok_or_else(|| format!("'{name}' isn't a vector parameter"))?;
            for (slot, v) in targets.into_iter().zip(list) {
                params[slot] = number(name, v)?;
            }
            continue;
        }
        if let (Some(choices), Some(text), Some(i)) = (options(kind.name(), name), value.as_str(), index(name)) {
            params[i] = option_index(choices, text).ok_or_else(|| format!("'{name}' is one of: {}", choices.join(", ")))? as f32;
            continue;
        }
        let Some(i) = index(name) else {
            let mut known: Vec<String> = names.iter().filter(|n| !n.is_empty()).cloned().collect();
            known.extend(tnames.iter().cloned());
            if matches!(kind, NodeKind::Wrangle { .. }) {
                known.extend(["code".into(), "run_over".into()]);
            }
            return Err(format!("{} has no parameter '{name}' (it has: {})", kind.name(), if known.is_empty() { "none".into() } else { known.join(", ") }));
        };
        params[i] = number(name, value)?;
    }
    if let NodeKind::Wrangle { code, domain } = kind {
        if let Some(c) = wrangle_code {
            *code = c;
        }
        if let Some(d) = wrangle_domain {
            *domain = d;
        }
        return Ok(Vec::new());
    }
    let mut rebuilt = NodeKind::from_params(kind.name(), &params).ok_or_else(|| format!("{} doesn't take these values", kind.name()))?;
    rebuilt.set_texts(&texts);
    *kind = rebuilt;
    let notes = ignored(kind)
        .into_iter()
        .filter(|(p, _)| values.contains_key(*p))
        .map(|(p, why)| format!("{} ignores {p}: {why}", kind.name()))
        .collect();
    Ok(notes)
}

fn group_of(pattern: &Option<String>) -> Option<groups::NodeGroup> {
    pattern.as_ref().map(|p| groups::NodeGroup { pattern: p.trim().to_owned(), kind: 0 })
}

fn apply_one(graph: &mut Graph, edit: &Edit, ids: &mut BTreeMap<String, NodeId>, placed: &mut Vec<NodeId>, notes: &mut Vec<String>) -> Result<(), String> {
    let fresh = |ids: &BTreeMap<String, NodeId>, id: &str| -> Result<(), String> {
        if id.trim().is_empty() {
            Err("give it an id".into())
        } else if ids.contains_key(id) {
            Err(format!("id '{id}' is already used: pick a new one"))
        } else {
            Ok(())
        }
    };
    match edit {
        Edit::CreateObject { id, name } => {
            fresh(ids, id)?;
            let count = graph.nodes.values().filter(|n| matches!(n.kind, NodeKind::Geometry { .. })).count();
            let object = graph.add(NodeKind::Geometry { material: None }, egui::pos2(40.0 + 200.0 * count as f32, 40.0));
            if let Some(name) = initial_name(graph, object, name, id) {
                graph.nodes.get_mut(&object).expect("added").name = name;
            }
            ids.insert(id.clone(), object);
        }
        Edit::AddNode { id, kind, object, name, params, group } => {
            fresh(ids, id)?;
            let object = Resolver { graph, ids }.object(object)?;
            let mut node_kind = template(kind).ok_or_else(|| format!("unknown node type '{kind}' (see the catalog)"))?;
            notes.extend(set_params(&mut node_kind, params)?.into_iter().map(|n| format!("{id}: {n}")));
            let node = graph.add(node_kind, egui::pos2(0.0, 0.0));
            let display_name = initial_name(graph, node, name, id);
            let n = graph.nodes.get_mut(&node).expect("added");
            n.parent = Some(object);
            if let Some(name) = display_name {
                n.name = name;
            }
            if let Some(g) = group_of(group) {
                n.group = g;
            }
            ids.insert(id.clone(), node);
            placed.push(node);
        }
        Edit::SetParams { node, params, group } => {
            let id = Resolver { graph, ids }.node(node)?;
            let n = graph.nodes.get_mut(&id).expect("resolved");
            let mut kind = n.kind.clone();
            notes.extend(set_params(&mut kind, params)?.into_iter().map(|note| format!("{node}: {note}")));
            n.kind = kind;
            if let Some(g) = group_of(group) {
                n.group = g;
            }
        }
        Edit::Connect { from, to, input, output } => {
            let r = Resolver { graph, ids };
            let (from_id, to_id) = (r.node(from)?, r.node(to)?);
            let (from_kind, to_kind) = (&graph.nodes[&from_id].kind, &graph.nodes[&to_id].kind);
            if *input == 0 || *output == 0 {
                return Err("inputs and outputs count from 1".into());
            }
            if !to_kind.is_multi_input() && *input > to_kind.num_inputs() {
                return Err(format!("{} has {} input(s)", to_kind.name(), to_kind.num_inputs()));
            }
            if *output > from_kind.num_outputs() {
                return Err(format!("{} has {} output(s)", from_kind.name(), from_kind.num_outputs()));
            }
            if graph.enclosing_geometry(from_id) != graph.enclosing_geometry(to_id) {
                return Err("both nodes must be in the same object (use object_merge to bring geometry across)".into());
            }
            if !graph.connect_out(from_id, output - 1, to_id, input - 1) {
                return Err("that would make a loop".into());
            }
        }
        Edit::Disconnect { node, input } => {
            let id = Resolver { graph, ids }.node(node)?;
            if *input == 0 {
                return Err("inputs count from 1".into());
            }
            graph.disconnect(id, input - 1);
        }
        Edit::Delete { node } => {
            let id = Resolver { graph, ids }.node(node)?;
            graph.remove(id);
            ids.retain(|_, v| *v != id);
            placed.retain(|p| *p != id);
        }
        Edit::SetDisplay { node } => {
            let id = Resolver { graph, ids }.node(node)?;
            if !graph.set_display(id) {
                return Err(format!("'{node}' can't take the display flag"));
            }
        }
    }
    Ok(())
}

// Lays new nodes out below what feeds them, so the network reads top to bottom.
fn place(graph: &mut Graph, placed: &[NodeId]) {
    for &id in placed {
        let Some(object) = graph.nodes.get(&id).and_then(|n| n.parent) else { continue };
        let inputs: Vec<egui::Pos2> = graph.links.iter().filter(|l| l.to == id).filter_map(|l| graph.nodes.get(&l.from).map(|n| n.pos)).collect();
        let pos = if inputs.is_empty() {
            // A new chain: right of everything already in the object.
            let right = graph.nodes.iter().filter(|(other, n)| **other != id && n.parent == Some(object) && (n.pos.x != 0.0 || n.pos.y != 0.0)).map(|(_, n)| n.pos.x).fold(f32::NEG_INFINITY, f32::max);
            egui::pos2(if right.is_finite() { right + 220.0 } else { 40.0 }, 40.0)
        } else {
            let x = inputs.iter().map(|p| p.x).sum::<f32>() / inputs.len() as f32;
            let y = inputs.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max) + 100.0;
            egui::pos2(x, y)
        };
        graph.nodes.get_mut(&id).expect("placed").pos = pos;
    }
}

// Applies the edits in order; a failed edit is skipped (and reported), the rest still run.
pub fn apply(graph: &mut Graph, edits: &[Edit]) -> Report {
    apply_session(graph, edits, &mut BTreeMap::new())
}

// As apply, with ids that last across calls (one assistant request makes several).
pub fn apply_session(graph: &mut Graph, edits: &[Edit], ids: &mut BTreeMap<String, NodeId>) -> Report {
    let mut report = Report::default();
    let mut placed = Vec::new();
    let before: std::collections::BTreeSet<String> = ids.keys().cloned().collect();
    for (i, edit) in edits.iter().enumerate() {
        match apply_one(graph, edit, ids, &mut placed, &mut report.notes) {
            Ok(()) => report.applied += 1,
            Err(e) => report.errors.push(format!("edit {} ({}): {e}", i + 1, edit.label())),
        }
    }
    place(graph, &placed);
    report.ids = ids.iter().filter(|(k, _)| !before.contains(*k)).map(|(k, v)| (k.clone(), *v)).collect();
    report
}

// A new node's name: the one given, else its id when that reads as a name and no
// other node has it (so the network shows "rocks_scatter", not "scatter14").
fn initial_name(graph: &Graph, node: NodeId, name: &Option<String>, id: &str) -> Option<String> {
    let wanted = name.as_ref().map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).or_else(|| {
        let id = id.trim();
        (id.len() > 1 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !id.starts_with(|c: char| c.is_ascii_digit())).then(|| id.to_owned())
    })?;
    let taken = graph.nodes.iter().any(|(other, n)| *other != node && n.title(*other) == wanted);
    (!taken).then_some(wanted)
}

// Edits from the model's JSON (an array of edit objects).
pub fn parse_edits(json: &serde_json::Value) -> Result<Vec<Edit>, String> {
    let list = json.as_array().ok_or("edits must be an array")?;
    list.iter()
        .enumerate()
        .map(|(i, e)| serde_json::from_value::<Edit>(e.clone()).map_err(|err| format!("edit {}: {err}", i + 1)))
        .collect()
}

// ---------------------------------------------------------------------------
// Inspect
// ---------------------------------------------------------------------------

// What a node produces at `time`: its error, point and primitive counts, bounds,
// attributes and groups.
pub fn inspect(graph: &Graph, node: &str, time: f32) -> Result<String, String> {
    let ids = BTreeMap::new();
    let id = Resolver { graph, ids: &ids }.node(node)?;
    let n = &graph.nodes[&id];
    let mesh = match n.kind {
        NodeKind::Geometry { .. } => graph.display_of(id).map(|d| graph.cook(d, time, &mut HashMap::new())).unwrap_or_default(),
        _ => graph.cook(id, time, &mut HashMap::new()),
    };
    let mut out = format!("{} (#{id}, {}):", n.title(id), n.kind.name());
    if let Some(error) = node_errors(|e| e.get(&id).cloned()) {
        let _ = write!(out, " ERROR: {error}.");
    }
    if let Some(note) = node_notes(|e| e.get(&id).cloned()) {
        let _ = write!(out, " Warning: {note}.");
    }
    let curves = groom::curves(&mesh).len();
    let _ = write!(out, " {} points, {} primitives", mesh.vertices.len(), poly::count(&mesh));
    if curves > 0 {
        let _ = write!(out, ", {curves} curves");
    }
    if volume::is_volume(&mesh) {
        out.push_str(", a volume");
    }
    if let Some((lo, hi)) = mesh.bounds() {
        let _ = write!(out, ", bounds ({:.3}, {:.3}, {:.3}) to ({:.3}, {:.3}, {:.3})", lo.x, lo.y, lo.z, hi.x, hi.y, hi.z);
    }
    out.push('.');
    let (points, prims) = groups::names(&mesh);
    let mut attributes: Vec<String> = mesh
        .point_attributes
        .keys()
        .filter(|k| !poly::is_internal(k) && !k.contains("group:"))
        .map(|k| match k.split_once(':') {
            Some((class, name)) => format!("{name} ({class})"),
            None => format!("{k} (point)"),
        })
        .collect();
    attributes.sort();
    if !attributes.is_empty() {
        let _ = write!(out, " Attributes: {}.", attributes.join(", "));
    }
    if !points.is_empty() || !prims.is_empty() {
        let _ = write!(out, " Groups: {}.", points.iter().map(|g| format!("{g} (points)")).chain(prims.iter().map(|g| format!("{g} (primitives)"))).collect::<Vec<_>>().join(", "));
    }
    // The Bullet solver: how much glue still holds.
    if matches!(n.kind, NodeKind::RbdBullet(_)) {
        let holding = graph.cook_output(id, 1, time, &mut HashMap::new()).vertices.len() / 2;
        let initial = graph.cook_input_fresh(id, 1, 0.0).vertices.len() / 2;
        let _ = write!(out, " Glue bonds: {holding} of {initial} holding ({} broken).", initial.saturating_sub(holding));
    }
    if let Some(stats) = piece_stats(graph, id, &n.kind, &mesh, time) {
        let _ = write!(out, " {stats}");
    }
    Ok(out)
}

// Each piece's points (by the piece attribute, on points or primitives).
fn pieces(mesh: &Mesh) -> Option<Vec<Vec<usize>>> {
    let n = mesh.vertices.len();
    let mut by: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    if let Some(p) = mesh.point_attributes.get("piece").filter(|p| p.len() == n && n > 0) {
        for (i, v) in p.iter().enumerate() {
            by.entry(v.round() as i64).or_default().push(i);
        }
    } else if let Some(p) = mesh.point_attributes.get("prim:piece").filter(|p| p.len() == mesh.indices.len() && !p.is_empty()) {
        let mut seen = std::collections::HashSet::new();
        for (tri, v) in mesh.indices.iter().zip(p) {
            for &pt in tri {
                if seen.insert(pt) {
                    by.entry(v.round() as i64).or_default().push(pt);
                }
            }
        }
    } else {
        return None;
    }
    (by.len() >= 2 && by.len() <= 5000).then(|| by.into_values().collect())
}

fn bounds_of(mesh: &Mesh, pts: &[usize]) -> (Vec3, Vec3) {
    pts.iter().fold((Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN)), |(lo, hi), &i| {
        let v = mesh.vertices[i];
        (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
    })
}

// For geometry in pieces (fractures, simulations): how many, whether they've come
// apart (touching pieces count as one cluster), the tallest and highest, and,
// after the first frame, how many are still moving.
fn piece_stats(graph: &Graph, id: NodeId, kind: &NodeKind, mesh: &Mesh, time: f32) -> Option<String> {
    let parts = pieces(mesh)?;
    let boxes: Vec<(Vec3, Vec3)> = parts.iter().map(|p| bounds_of(mesh, p)).collect();
    let (lo, hi) = mesh.bounds()?;
    let tol = hi.sub(lo).length() * 0.01;
    // Clusters of touching pieces (bounding boxes within 1% of the whole's size).
    let mut parent: Vec<usize> = (0..boxes.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for a in 0..boxes.len() {
        for b in a + 1..boxes.len() {
            let ((alo, ahi), (blo, bhi)) = (boxes[a], boxes[b]);
            let near = alo.x <= bhi.x + tol && blo.x <= ahi.x + tol && alo.y <= bhi.y + tol && blo.y <= ahi.y + tol && alo.z <= bhi.z + tol && blo.z <= ahi.z + tol;
            if near {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut clusters: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..boxes.len() {
        let r = root(&mut parent, i);
        clusters.entry(r).or_default().push(i);
    }
    let largest = clusters.values().max_by_key(|c| c.len()).cloned().unwrap_or_default();
    let (clo, chi) = largest.iter().fold((Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN)), |(l, h), &i| {
        let (bl, bh) = boxes[i];
        (Vec3::new(l.x.min(bl.x), l.y.min(bl.y), l.z.min(bl.z)), Vec3::new(h.x.max(bh.x), h.y.max(bh.y), h.z.max(bh.z)))
    });
    let tallest = boxes.iter().map(|(l, h)| h.y - l.y).fold(0.0f32, f32::max);
    let mut out = format!(
        "Pieces: {}, in {} cluster(s) of touching pieces; the largest cluster has {} piece(s), {:.2} × {:.2} × {:.2}, top at y={:.2}; the tallest single piece is {tallest:.2} high.",
        parts.len(),
        clusters.len(),
        largest.len(),
        chi.x - clo.x,
        chi.y - clo.y,
        chi.z - clo.z,
        chi.y
    );
    // Moving: compare with the frame before (same pieces, same order).
    let step = 1.0 / sims::SIM_FPS;
    if time >= step {
        let before = match kind {
            NodeKind::Geometry { .. } => graph.display_of(id).map(|d| graph.cook(d, time - step, &mut HashMap::new())).unwrap_or_default(),
            _ => graph.cook(id, time - step, &mut HashMap::new()),
        };
        if let Some(prev) = pieces(&before).filter(|p| p.len() == parts.len()) {
            let centre = |m: &Mesh, p: &[usize]| p.iter().fold(Vec3::zero(), |a, &i| a.add(m.vertices[i])).scale(1.0 / p.len().max(1) as f32);
            // Faster than 0.25 units a second (slower is contact jitter at rest).
            let moving = parts.iter().zip(&prev).filter(|(now, was)| centre(mesh, now).sub(centre(&before, was)).length() / step > 0.25).count();
            let _ = write!(out, " Moving: {moving} of {} pieces (the rest have come to rest).", parts.len());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edits(json: serde_json::Value) -> Vec<Edit> {
        parse_edits(&json).unwrap()
    }

    #[test]
    fn the_catalog_lists_geometry_nodes_with_named_parameters() {
        let text = catalog_text();
        assert!(text.contains("- scatter: "), "{text}");
        assert!(text.contains("count=1000"));
        assert!(text.contains("- merge: ") && text.contains("inputs: any number"));
        assert!(text.contains("- wrangle: ") && text.contains("text: code") && text.contains("run_over="));
        assert!(!text.contains("- geometry:"), "objects aren't SOPs");
        assert!(!text.contains("principled"), "no shaders");
        // Every listed node can be made from its name.
        for s in catalog() {
            assert!(template(s.name).is_some(), "{}", s.name);
        }
    }

    #[test]
    fn option_parameters_are_named() {
        // Every option table names a real parameter of a real node.
        for s in catalog() {
            for (param, _) in &s.params {
                if let Some(choices) = options(s.name, param) {
                    assert!(!choices.is_empty());
                }
            }
        }
        let table = [
            ("attribute_randomize", "attribute"), ("attribute_randomize", "class"), ("attribute_randomize", "mode"), ("attribute_create", "attribute"),
            ("attribute_delete", "attribute"), ("boolean", "operation"), ("boolean", "method"), ("material_fracture", "material"), ("measure", "measure"),
            ("measure", "curvature"), ("measure", "summary"), ("extract_centroid", "partition"), ("extract_centroid", "method"), ("match_size", "justify.x"),
            ("group_create", "primitives"), ("group_create", "method"), ("group_create", "combine"), ("pack", "pack"), ("pack", "pivot"),
            ("terrain_noise", "noise"), ("terrain_noise", "blend"), ("terrain_mask", "source"), ("terrain_mask", "combine"), ("normal", "domain"),
            ("normal", "weighting"), ("uv_texture", "projection"), ("uv_texture", "axis"), ("mirror", "axis"), ("clip", "axis"), ("blast", "component"),
            ("connectivity", "class"),
        ];
        for (node, param) in table {
            let kind = template(node).unwrap_or_else(|| panic!("no node {node}"));
            assert!(kind.param_names().iter().any(|n| n == param), "{node} has no {param}: {:?}", kind.param_names());
        }
        let text = catalog_text();
        assert!(text.contains("attribute=\"pscale\"{id|pscale|Cd|orient|Custom}"), "{}", text.lines().find(|l| l.contains("attribute_randomize")).unwrap_or(""));
        // Set by name.
        let mut g = Graph::default();
        let report = apply(&mut g, &edits(serde_json::json!([
            {"op": "create_object", "id": "obj"},
            {"op": "add_node", "id": "r", "type": "attribute_randomize", "object": "obj", "params": {"attribute": "custom", "class": "primitives", "name": "rock_size", "min": 0.15, "max": 0.55}},
            {"op": "add_node", "id": "b", "type": "boolean", "object": "obj", "params": {"operation": "subtract"}},
            {"op": "add_node", "id": "x", "type": "boolean", "object": "obj", "params": {"operation": "melt"}}
        ])));
        assert!(matches!(&g.nodes[&report.ids["r"]].kind, NodeKind::AttributeRandomize(r) if r.preset == randomize::CUSTOM && r.class == 1 && r.name == "rock_size"));
        assert!(matches!(&g.nodes[&report.ids["b"]].kind, NodeKind::Boolean(b) if b.operation == 2));
        assert!(report.errors.iter().any(|e| e.contains("one of: Union, Intersect")), "{:?}", report.errors);
        assert!(describe(&g).contains("attribute=\"Custom\""), "{}", describe(&g));
        // A value that does nothing as the node is set up comes back as a note.
        let pscale = apply(&mut g, &edits(serde_json::json!([
            {"op": "add_node", "id": "p", "type": "attribute_randomize", "object": "#0", "params": {"min": 0.1, "max": 0.5}}
        ])));
        assert!(pscale.errors.is_empty() && pscale.notes.iter().any(|n| n.contains("ignores min") && n.contains("Custom")), "{:?}", pscale.notes);
        assert!(pscale.to_text(&g).contains("Note: p: attribute_randomize ignores min"));
    }

    #[test]
    fn inspect_reports_pieces_and_node_notes_reach_the_catalog() {
        // Two boxes far apart: two pieces, two clusters.
        let mut g = Graph::default();
        let report = apply(&mut g, &edits(serde_json::json!([
            {"op": "create_object", "id": "obj"},
            {"op": "add_node", "id": "a", "type": "box", "object": "obj"},
            {"op": "add_node", "id": "b", "type": "box", "object": "obj", "params": {"size": 2}},
            {"op": "add_node", "id": "far", "type": "xform", "object": "obj", "params": {"t": [5, 0, 0]}},
            {"op": "add_node", "id": "both", "type": "merge", "object": "obj"},
            {"op": "add_node", "id": "numbered", "type": "connectivity", "object": "obj", "params": {"class": "points", "attribute": "piece"}},
            {"op": "connect", "from": "b", "to": "far"},
            {"op": "connect", "from": "a", "to": "both", "input": 1},
            {"op": "connect", "from": "far", "to": "both", "input": 2},
            {"op": "connect", "from": "both", "to": "numbered"}
        ])));
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let text = inspect(&g, "numbered", 0.0).unwrap();
        assert!(text.contains("Pieces: 2, in 2 cluster(s)") && text.contains("the tallest single piece is 2.00 high"), "{text}");
        let catalog = catalog_text();
        assert!(catalog.lines().any(|l| l.starts_with("- copy_to_points:") && l.contains("note: copies keep their own up direction")));
    }

    #[test]
    fn edits_build_a_working_network() {
        let mut g = Graph::default();
        let report = apply(&mut g, &edits(serde_json::json!([
            {"op": "create_object", "id": "obj", "name": "rocks"},
            {"op": "add_node", "id": "ground", "type": "grid", "object": "obj", "params": {"size": 10, "resolution": 20}},
            {"op": "add_node", "id": "pts", "type": "scatter", "object": "obj", "params": {"count": 50, "seed": 4, "relax_iterations": 0}},
            {"op": "add_node", "id": "rock", "type": "sphere", "object": "obj", "params": {"radius": 0.2}},
            {"op": "add_node", "id": "copies", "type": "copy_to_points", "object": "obj"},
            {"op": "add_node", "id": "lift", "type": "xform", "object": "obj", "params": {"t": [0, 0.1, 0]}},
            {"op": "connect", "from": "ground", "to": "pts"},
            {"op": "connect", "from": "rock", "to": "copies", "input": 1},
            {"op": "connect", "from": "pts", "to": "copies", "input": 2},
            {"op": "connect", "from": "copies", "to": "lift"},
            {"op": "set_display", "node": "lift"}
        ])));
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.applied, 11);
        let lift = report.ids["lift"];
        assert!(matches!(g.nodes[&lift].kind, NodeKind::Transform { t, .. } if t == [0.0, 0.1, 0.0]));
        let object = report.ids["obj"];
        assert_eq!(g.nodes[&object].title(object), "rocks");
        assert_eq!(g.display_of(object), Some(lift));
        let info = inspect(&g, "rocks", 0.0).unwrap();
        assert!(info.contains(&format!("{} points", 50 * g.cook(report.ids["rock"], 0.0, &mut HashMap::new()).vertices.len())), "{info}");
        // New nodes are laid out top to bottom along the wiring.
        assert!(g.nodes[&lift].pos.y > g.nodes[&report.ids["copies"]].pos.y);
        // The scene, as the model sees it next time.
        let scene = describe(&g);
        assert!(scene.contains("object rocks") && scene.contains("count=50") && scene.contains("inputs: 1<-") && scene.contains(&format!("display: {}", g.nodes[&lift].title(lift))), "{scene}");
    }

    #[test]
    fn mistakes_come_back_as_messages() {
        let mut g = Graph::default();
        let report = apply(&mut g, &edits(serde_json::json!([
            {"op": "create_object", "id": "obj"},
            {"op": "add_node", "id": "a", "type": "boxx", "object": "obj"},
            {"op": "add_node", "id": "b", "type": "box", "object": "obj", "params": {"sizee": 2}},
            {"op": "add_node", "id": "c", "type": "box", "object": "obj"},
            {"op": "add_node", "id": "d", "type": "xform", "object": "obj"},
            {"op": "connect", "from": "c", "to": "d", "input": 3},
            {"op": "connect", "from": "d", "to": "c"},
            {"op": "set_params", "node": "nosuch", "params": {"size": 1}}
        ])));
        assert_eq!(report.applied, 3);
        let text = report.errors.join("\n");
        assert!(text.contains("unknown node type 'boxx'"), "{text}");
        assert!(text.contains("no parameter 'sizee'") && text.contains("size, divisions"), "{text}");
        assert!(text.contains("has 1 input"), "{text}");
        assert!(text.contains("no node named 'nosuch'"), "{text}");
        assert!(parse_edits(&serde_json::json!([{"op": "explode"}])).is_err());
    }

    #[test]
    fn wrangles_text_parameters_and_existing_nodes() {
        let mut g = Graph::default();
        let report = apply(&mut g, &edits(serde_json::json!([
            {"op": "create_object", "id": "obj"},
            {"op": "add_node", "id": "grid", "type": "grid", "object": "obj"},
            {"op": "add_node", "id": "w", "type": "wrangle", "object": "obj", "params": {"code": "@P.y += 1;", "run_over": "points"}},
            {"op": "add_node", "id": "g", "type": "group_create", "object": "obj", "params": {"name": "top"}},
            {"op": "connect", "from": "grid", "to": "w"},
            {"op": "set_display", "node": "w"}
        ])));
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let w = report.ids["w"];
        assert!(matches!(&g.nodes[&w].kind, NodeKind::Wrangle { code, domain: 2 } if code == "@P.y += 1;"));
        let lifted = g.cook(w, 0.0, &mut HashMap::new());
        assert!(lifted.vertices.iter().all(|v| (v.y - 1.0).abs() < 1e-5));
        // Later batches refer to nodes by name or #id.
        let grid_name = g.nodes[&report.ids["grid"]].title(report.ids["grid"]);
        let again = apply(&mut g, &edits(serde_json::json!([
            {"op": "set_params", "node": grid_name, "params": {"size": 4}},
            {"op": "delete", "node": format!("#{}", report.ids["g"])}
        ])));
        assert!(again.errors.is_empty(), "{:?}", again.errors);
        assert!(!g.nodes.contains_key(&report.ids["g"]));
        assert!(matches!(g.nodes[&report.ids["grid"]].kind, NodeKind::Grid { size, .. } if size == 4.0));
    }
}
