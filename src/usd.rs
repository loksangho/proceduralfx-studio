// USD stage model for the STAGE (LOP) context.
//
// A Stage is a tree of prims keyed by path. Prims hold a local transform,
// optional triangulated geometry, typed attributes (gprim sizes, camera lens,
// light and preview-surface inputs) and a material binding.
//
// Coordinates: the app's viewport space is USD's right-handed space mirrored
// in Z (see the MoonRay export), so everything crossing the USD boundary is
// mirrored: points and matrices on import and export, and triangle winding is
// swapped. Inside the app, cameras and lights look along their local +Z.
//
// Reading .usd/.usda/.usdc/.usdz (with composition) and writing .usdc/.usdz
// go through OpenUSD (native/usd_bridge.cpp, feature "native-usd"); writing
// .usda is plain Rust and always available.

use super::{Mesh, Vec3};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};

// Row-major 4×4 in USD's row-vector convention: p' = p · M, translation in the last row.
pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
}

pub fn transform_point(m: &Mat4, p: Vec3) -> Vec3 {
    Vec3::new(
        p.x * m[0][0] + p.y * m[1][0] + p.z * m[2][0] + m[3][0],
        p.x * m[0][1] + p.y * m[1][1] + p.z * m[2][1] + m[3][1],
        p.x * m[0][2] + p.y * m[1][2] + p.z * m[2][2] + m[3][2],
    )
}

pub fn row(m: &Mat4, r: usize) -> Vec3 {
    Vec3::new(m[r][0], m[r][1], m[r][2])
}

// Scale, then rotate (degrees, X then Y then Z as Vec3::rotate_deg), then translate.
pub fn trs(t: [f32; 3], r: [f32; 3], s: [f32; 3]) -> Mat4 {
    let axis = |i: usize| {
        let mut e = [0.0; 3];
        e[i] = s[i];
        Vec3::from_arr(e).rotate_deg(r)
    };
    let (x, y, z) = (axis(0), axis(1), axis(2));
    [[x.x, x.y, x.z, 0.0], [y.x, y.y, y.z, 0.0], [z.x, z.y, z.z, 0.0], [t[0], t[1], t[2], 1.0]]
}

// A frame at `eye` whose local +Z points at `target` (cameras and lights).
pub fn look_at(eye: [f32; 3], target: [f32; 3]) -> Mat4 {
    let eye_v = Vec3::from_arr(eye);
    let mut z = Vec3::from_arr(target).sub(eye_v).normalize();
    if z.length() < 0.5 {
        z = Vec3::new(0.0, 0.0, 1.0);
    }
    let up = if z.y.abs() > 0.999 { Vec3::new(0.0, 0.0, 1.0) } else { Vec3::new(0.0, 1.0, 0.0) };
    let x = up.cross(z).normalize();
    let y = z.cross(x);
    [[x.x, x.y, x.z, 0.0], [y.x, y.y, y.z, 0.0], [z.x, z.y, z.z, 0.0], [eye[0], eye[1], eye[2], 1.0]]
}

// Converts between app space and USD space (the same operation both ways).
fn mirror_matrix(m: &Mat4) -> Mat4 {
    let s = [1.0, 1.0, -1.0, 1.0];
    std::array::from_fn(|r| std::array::from_fn(|c| m[r][c] * s[r] * s[c]))
}

#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    Float(f32),
    Vec3([f32; 3]),
    Token(String),
    Bool(bool),
}

#[derive(Clone, Debug)]
pub struct Prim {
    pub type_name: String,
    pub xform: Mat4,
    // Local-space triangles; colour as Cd (point), prim:Cd, vertex:Cd or detail:Cd.
    pub mesh: Option<Arc<Mesh>>,
    pub attrs: BTreeMap<String, AttrValue>,
    pub binding: Option<String>,
    pub active: bool,
    pub visible: bool,
    // MAT network node a Material prim was made from, so MoonRay can use the full material.
    pub material_node: Option<usize>,
}

impl Prim {
    pub fn new(type_name: &str) -> Self {
        Prim {
            type_name: type_name.to_string(),
            xform: IDENTITY,
            mesh: None,
            attrs: BTreeMap::new(),
            binding: None,
            active: true,
            visible: true,
            material_node: None,
        }
    }

    pub fn float(&self, name: &str) -> Option<f32> {
        match self.attrs.get(name) {
            Some(AttrValue::Float(f)) => Some(*f),
            _ => None,
        }
    }

    pub fn vec3(&self, name: &str) -> Option<[f32; 3]> {
        match self.attrs.get(name) {
            Some(AttrValue::Vec3(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn token(&self, name: &str) -> Option<&str> {
        match self.attrs.get(name) {
            Some(AttrValue::Token(t)) => Some(t),
            _ => None,
        }
    }

    pub fn is_light(&self) -> bool {
        self.type_name.ends_with("Light")
    }

    pub fn is_geometry(&self) -> bool {
        matches!(self.type_name.as_str(), "Mesh" | "Sphere" | "Cube" | "Cylinder" | "Cone" | "Capsule")
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stage {
    // Sorted so every prim comes after its parent.
    pub prims: BTreeMap<String, Prim>,
    // Any referenced file has time samples.
    pub animated: bool,
}

pub fn parent_path(path: &str) -> Option<&str> {
    if path == "/" || path.is_empty() {
        return None;
    }
    match path.rfind('/') {
        Some(0) => Some("/"),
        Some(i) => Some(&path[..i]),
        None => None,
    }
}

pub fn prim_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// A valid USD prim name: letters, digits and underscores, not starting with a digit.
pub fn sanitize_name(name: &str) -> String {
    let mut out: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

// A clean absolute prim path ("geo/box 1" → "/geo/box_1").
pub fn sanitize_path(path: &str) -> String {
    let parts: Vec<String> = path.split('/').filter(|p| !p.is_empty()).map(sanitize_name).collect();
    format!("/{}", parts.join("/"))
}

// Houdini-style prim patterns, separated by spaces: an exact path, `*` within one
// path segment, `**` across segments. A leading `^` excludes matches.
pub fn matches_pattern(pattern: &str, path: &str) -> bool {
    let mut included = false;
    for term in pattern.split_whitespace() {
        let (exclude, glob) = match term.strip_prefix('^') {
            Some(rest) => (true, rest),
            None => (false, term),
        };
        if glob_match(glob.as_bytes(), path.as_bytes()) {
            included = !exclude;
        }
    }
    included
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) if rest.first() == Some(&b'*') => {
            let rest = &rest[1..];
            (0..=text.len()).any(|i| glob_match(rest, &text[i..]))
        }
        Some((b'*', rest)) => {
            let segment_end = text.iter().position(|c| *c == b'/').unwrap_or(text.len());
            (0..=segment_end).any(|i| glob_match(rest, &text[i..]))
        }
        Some((c, rest)) => text.first() == Some(c) && glob_match(rest, &text[1..]),
    }
}

impl Stage {
    // Adds (or retypes) a prim, creating missing ancestors as Xforms.
    pub fn define(&mut self, path: &str, type_name: &str) -> &mut Prim {
        let path = sanitize_path(path);
        let mut ancestor = parent_path(&path).map(str::to_string);
        while let Some(a) = ancestor.filter(|a| a != "/") {
            self.prims.entry(a.clone()).or_insert_with(|| Prim::new("Xform"));
            ancestor = parent_path(&a).map(str::to_string);
        }
        let prim = self.prims.entry(path).or_insert_with(|| Prim::new(type_name));
        prim.type_name = type_name.to_string();
        prim
    }

    // Layers `other` over this stage: its prims replace prims at the same paths.
    pub fn merge(&mut self, other: &Stage) {
        for (path, prim) in &other.prims {
            self.prims.insert(path.clone(), prim.clone());
        }
        self.animated |= other.animated;
    }

    // The same prims under `root` (references), keeping material bindings pointing inside.
    pub fn rerooted(&self, root: &str) -> Stage {
        let root = sanitize_path(root);
        if root == "/" {
            return self.clone();
        }
        let move_path = |p: &str| format!("{root}{p}");
        let mut out = Stage { animated: self.animated, ..Default::default() };
        out.define(&root, "Xform");
        for (path, prim) in &self.prims {
            let mut prim = prim.clone();
            if let Some(binding) = &prim.binding {
                prim.binding = Some(move_path(binding));
            }
            out.prims.insert(move_path(path), prim);
        }
        out
    }

    pub fn children<'a>(&'a self, path: &'a str) -> impl Iterator<Item = (&'a String, &'a Prim)> + 'a {
        self.prims.iter().filter(move |(p, _)| parent_path(p) == Some(path))
    }

    pub fn world_xform(&self, path: &str) -> Mat4 {
        let local = self.prims.get(path).map_or(IDENTITY, |p| p.xform);
        match parent_path(path).filter(|p| *p != "/") {
            Some(parent) => mat_mul(&local, &self.world_xform(parent)),
            None => local,
        }
    }

    // Inactive or invisible prims hide their descendants.
    pub fn is_shown(&self, path: &str) -> bool {
        let mut current = Some(path);
        while let Some(p) = current.filter(|p| *p != "/") {
            if self.prims.get(p).is_some_and(|prim| !prim.active || !prim.visible) {
                return false;
            }
            current = parent_path(p);
        }
        true
    }

    // The material bound to a prim or, failing that, its nearest bound ancestor.
    pub fn bound_material(&self, path: &str) -> Option<&str> {
        let mut current = Some(path);
        while let Some(p) = current.filter(|p| *p != "/") {
            if let Some(binding) = self.prims.get(p).and_then(|prim| prim.binding.as_deref()) {
                return Some(binding);
            }
            current = parent_path(p);
        }
        None
    }

    // Visible geometry in world space, one entry per prim, with its bound material path.
    pub fn flatten(&self) -> Vec<(String, Mesh, Option<String>)> {
        self.prims
            .iter()
            .filter(|(path, prim)| prim.is_geometry() && self.is_shown(path))
            .filter_map(|(path, prim)| {
                let local = prim_geometry(prim)?;
                let world = self.world_xform(path);
                let mut mesh = local;
                for v in &mut mesh.vertices {
                    *v = transform_point(&world, *v);
                }
                // A mirroring transform flips the winding.
                if determinant3(&world) < 0.0 {
                    for tri in &mut mesh.indices {
                        tri.swap(1, 2);
                    }
                }
                Some((path.clone(), mesh, self.bound_material(path).map(str::to_string)))
            })
            .collect()
    }

    // The viewport mesh: all visible geometry, coloured by displayColor, else the bound
    // material's diffuse colour.
    pub fn to_mesh(&self) -> Mesh {
        let parts = self.flatten();
        let any_colour = parts.iter().any(|(_, mesh, material)| corner_colors(mesh).is_some() || material.is_some());
        let mut merged = Mesh::default();
        for (_, mut mesh, material) in parts {
            if any_colour {
                let colors = corner_colors(&mesh).unwrap_or_else(|| {
                    let diffuse = material
                        .as_deref()
                        .and_then(|m| self.prims.get(m))
                        .and_then(|m| m.vec3("diffuseColor"))
                        .unwrap_or([0.72, 0.72, 0.72]);
                    diffuse.repeat(mesh.indices.len() * 3)
                });
                mesh.point_attributes.retain(|key, _| !key.ends_with("Cd"));
                mesh.point_attributes.insert("vertex:Cd".into(), colors);
            }
            merged = super::node_merge(merged, mesh);
        }
        merged
    }

    pub fn material(&self, path: &str) -> Option<&Prim> {
        self.prims.get(path).filter(|p| p.type_name == "Material")
    }
}

fn determinant3(m: &Mat4) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

// Colour of every triangle corner from vertex:Cd, Cd, prim:Cd or detail:Cd, in that order.
pub fn corner_colors(mesh: &Mesh) -> Option<Vec<f32>> {
    let (points, tris) = (mesh.vertices.len(), mesh.indices.len());
    let rgb = |values: &[f32], i: usize| [values[i * 3], values[i * 3 + 1], values[i * 3 + 2]];
    let attrs = &mesh.point_attributes;
    if let Some(v) = attrs.get("vertex:Cd").filter(|v| v.len() == tris * 9) {
        return Some(v.clone());
    }
    let pick: Box<dyn Fn(usize, usize) -> [f32; 3]> = if let Some(v) = attrs.get("Cd").filter(|v| v.len() == points * 3) {
        Box::new(move |_, point| rgb(v, point))
    } else if let Some(v) = attrs.get("prim:Cd").filter(|v| v.len() == tris * 3) {
        Box::new(move |tri, _| rgb(v, tri))
    } else if let Some(v) = attrs.get("detail:Cd").filter(|v| v.len() >= 3) {
        Box::new(move |_, _| rgb(v, 0))
    } else {
        return None;
    };
    let pick = &pick;
    Some(mesh.indices.iter().enumerate().flat_map(|(t, tri)| tri.iter().flat_map(move |p| pick(t, *p))).collect::<Vec<f32>>())
}

// Local-space triangles for Mesh prims and tessellated gprims.
fn prim_geometry(prim: &Prim) -> Option<Mesh> {
    let size = |name: &str, default: f32| prim.float(name).unwrap_or(default).abs().max(1e-4);
    let along_axis = |mesh: Mesh| match prim.token("axis").unwrap_or("Z") {
        "X" => super::node_transform(mesh, [0.0; 3], [0.0, 0.0, 90.0], [1.0; 3]),
        "Z" => super::node_transform(mesh, [0.0; 3], [90.0, 0.0, 0.0], [1.0; 3]),
        _ => mesh,
    };
    let mut mesh = match prim.type_name.as_str() {
        "Mesh" => return prim.mesh.as_deref().cloned(),
        "Sphere" => super::node_sphere(size("radius", 1.0), 16, 32),
        "Cube" => super::node_box(size("size", 2.0), 1),
        "Cylinder" => along_axis(super::node_tube(size("radius", 1.0), size("radius", 1.0), size("height", 2.0), 1, 32, true)),
        "Cone" => along_axis(super::node_cone(size("radius", 1.0), size("height", 2.0), 1, 32, true)),
        "Capsule" => along_axis(super::node_capsule(size("radius", 0.5), size("height", 1.0), 12, 24)),
        _ => return None,
    };
    // Gprims carry their displayColor on the prim.
    if let Some(colour) = prim.mesh.as_ref().and_then(|m| m.point_attributes.get("detail:Cd")) {
        mesh.point_attributes.insert("detail:Cd".into(), colour.clone());
    }
    Some(mesh)
}

// ============================================================================
// Writing USDA
// ============================================================================

fn fmt_f(v: f32) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

fn fmt_v3(v: [f32; 3]) -> String {
    format!("({}, {}, {})", fmt_f(v[0]), fmt_f(v[1]), fmt_f(v[2]))
}

fn write_array<T>(out: &mut String, items: impl Iterator<Item = T>, fmt: impl Fn(T) -> String) {
    out.push('[');
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&fmt(item));
    }
    out.push(']');
}

// USDA text for the whole stage, converted to USD's right-handed Y-up space.
pub fn to_usda(stage: &Stage) -> String {
    let mut out = String::from("#usda 1.0\n(\n");
    let roots: Vec<&String> = stage.prims.keys().filter(|p| parent_path(p) == Some("/")).collect();
    if let Some(first) = roots.first() {
        let _ = writeln!(out, "    defaultPrim = \"{}\"", prim_name(first));
    }
    out.push_str("    metersPerUnit = 1\n    upAxis = \"Y\"\n    doc = \"Written by ProceduralFX Studio\"\n)\n\n");
    for root in roots {
        write_prim(&mut out, stage, root, 0);
    }
    out
}

fn write_prim(out: &mut String, stage: &Stage, path: &str, depth: usize) {
    let Some(prim) = stage.prims.get(path) else { return };
    let pad = "    ".repeat(depth);
    let inner = "    ".repeat(depth + 1);
    let mut metadata = Vec::new();
    if prim.binding.is_some() {
        metadata.push("prepend apiSchemas = [\"MaterialBindingAPI\"]".to_string());
    }
    if !prim.active {
        metadata.push("active = false".to_string());
    }
    let _ = write!(out, "{pad}def {} \"{}\"", prim.type_name, prim_name(path));
    if metadata.is_empty() {
        out.push('\n');
    } else {
        let _ = writeln!(out, " (\n{}\n{pad})", metadata.iter().map(|m| format!("{inner}{m}")).collect::<Vec<_>>().join("\n"));
    }
    let _ = writeln!(out, "{pad}{{");
    if prim.xform != IDENTITY && prim.type_name != "Material" {
        let m = mirror_matrix(&prim.xform);
        let rows: Vec<String> = m.iter().map(|r| format!("({}, {}, {}, {})", fmt_f(r[0]), fmt_f(r[1]), fmt_f(r[2]), fmt_f(r[3]))).collect();
        let _ = writeln!(out, "{inner}matrix4d xformOp:transform = ( {} )", rows.join(", "));
        let _ = writeln!(out, "{inner}uniform token[] xformOpOrder = [\"xformOp:transform\"]");
    }
    if !prim.visible {
        let _ = writeln!(out, "{inner}token visibility = \"invisible\"");
    }
    if let Some(binding) = &prim.binding {
        let _ = writeln!(out, "{inner}rel material:binding = <{binding}>");
    }
    match prim.type_name.as_str() {
        "Mesh" => {
            if let Some(mesh) = &prim.mesh {
                write_mesh(out, mesh, &inner);
            }
        }
        "Material" => write_material(out, prim, path, &inner),
        _ => {}
    }
    if prim.is_geometry() && prim.type_name != "Mesh" {
        if let Some(colour) = prim.mesh.as_ref().and_then(|m| m.point_attributes.get("detail:Cd")).filter(|c| c.len() >= 3) {
            let _ = writeln!(out, "{inner}color3f[] primvars:displayColor = [{}]", fmt_v3([colour[0], colour[1], colour[2]]));
        }
    }
    for (name, value) in &prim.attrs {
        if prim.type_name == "Material" {
            continue;
        }
        let light_input = prim.is_light() && matches!(name.as_str(), "intensity" | "exposure" | "color" | "radius" | "angle" | "width" | "height" | "normalize");
        let full_name = if light_input { format!("inputs:{name}") } else { name.clone() };
        let _ = match value {
            AttrValue::Float(f) => {
                let ty = if matches!(name.as_str(), "radius" | "size" | "height") && !light_input { "double" } else { "float" };
                writeln!(out, "{inner}{ty} {full_name} = {}", fmt_f(*f))
            }
            AttrValue::Vec3(v) => writeln!(out, "{inner}color3f {full_name} = {}", fmt_v3(*v)),
            AttrValue::Token(t) => writeln!(out, "{inner}uniform token {full_name} = \"{t}\""),
            AttrValue::Bool(b) => writeln!(out, "{inner}bool {full_name} = {}", *b as u8),
        };
    }
    let children: Vec<String> = stage.children(path).map(|(p, _)| p.clone()).collect();
    for child in children {
        out.push('\n');
        write_prim(out, stage, &child, depth + 1);
    }
    let _ = writeln!(out, "{pad}}}");
}

fn write_mesh(out: &mut String, mesh: &Mesh, inner: &str) {
    let points = mesh.vertices.iter().map(|v| [v.x, v.y, -v.z]);
    let _ = write!(out, "{inner}point3f[] points = ");
    write_array(out, points, fmt_v3);
    out.push('\n');
    let _ = write!(out, "{inner}int[] faceVertexCounts = ");
    write_array(out, mesh.indices.iter(), |_| "3".to_string());
    out.push('\n');
    // Mirroring Z flips the winding back to USD's right-handed convention.
    let _ = write!(out, "{inner}int[] faceVertexIndices = ");
    write_array(out, mesh.indices.iter().flat_map(|t| [t[0], t[2], t[1]]), |i| i.to_string());
    out.push('\n');
    if let Some((lo, hi)) = mesh.bounds() {
        let _ = writeln!(out, "{inner}float3[] extent = [{}, {}]", fmt_v3([lo.x, lo.y, -hi.z]), fmt_v3([hi.x, hi.y, -lo.z]));
    }
    let _ = writeln!(out, "{inner}uniform token subdivisionScheme = \"none\"");
    // Attributes become primvars with the matching interpolation.
    let (points, tris) = (mesh.vertices.len(), mesh.indices.len());
    let mut keys: Vec<&String> = mesh.point_attributes.keys().collect();
    keys.sort();
    for key in keys {
        let values = &mesh.point_attributes[key];
        let (interpolation, name, count) = if let Some(n) = key.strip_prefix("prim:") {
            ("uniform", n, tris)
        } else if let Some(n) = key.strip_prefix("vertex:") {
            ("faceVarying", n, tris * 3)
        } else if let Some(n) = key.strip_prefix("detail:") {
            ("constant", n, 1)
        } else {
            ("vertex", key.as_str(), points)
        };
        if count == 0 || values.len() % count != 0 || name == "id" {
            continue;
        }
        let comps = values.len() / count;
        let name = if name == "Cd" { "displayColor".to_string() } else { sanitize_name(name) };
        // faceVarying data follows the swapped winding.
        let reorder = |i: usize| if interpolation == "faceVarying" { i / 3 * 3 + [0, 2, 1][i % 3] } else { i };
        let _ = write!(out, "{inner}");
        match comps {
            1 => {
                let _ = write!(out, "float[] primvars:{name} = ");
                write_array(out, 0..count, |i| fmt_f(values[reorder(i)]));
            }
            3 => {
                let ty = if name == "displayColor" { "color3f" } else { "float3" };
                let _ = write!(out, "{ty}[] primvars:{name} = ");
                write_array(out, 0..count, |i| {
                    let j = reorder(i) * 3;
                    fmt_v3([values[j], values[j + 1], values[j + 2]])
                });
            }
            _ => continue,
        }
        let _ = writeln!(out, " (\n{inner}    interpolation = \"{interpolation}\"\n{inner})");
    }
}

fn write_material(out: &mut String, prim: &Prim, path: &str, inner: &str) {
    let _ = writeln!(out, "{inner}token outputs:surface.connect = <{path}/PreviewSurface.outputs:surface>\n");
    let _ = writeln!(out, "{inner}def Shader \"PreviewSurface\"\n{inner}{{");
    let _ = writeln!(out, "{inner}    uniform token info:id = \"UsdPreviewSurface\"");
    for name in ["diffuseColor", "emissiveColor"] {
        if let Some(v) = prim.vec3(name) {
            let _ = writeln!(out, "{inner}    color3f inputs:{name} = {}", fmt_v3(v));
        }
    }
    for name in ["roughness", "metallic", "opacity", "ior"] {
        if let Some(f) = prim.float(name) {
            let _ = writeln!(out, "{inner}    float inputs:{name} = {}", fmt_f(f));
        }
    }
    let _ = writeln!(out, "{inner}    token outputs:surface\n{inner}}}");
}

// ============================================================================
// Reading through OpenUSD
// ============================================================================

// Parses the bridge's prim description (see native/usd_bridge.cpp) into app space.
pub fn parse_bridge_dump(text: &str) -> Result<Stage, String> {
    let mut stage = Stage::default();
    let mut current: Option<String> = None;
    let mut points: Vec<Vec3> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut colour: Option<(String, Vec<f32>)> = None;
    let mut left_handed = false;

    fn numbers<T: std::str::FromStr>(tokens: &[&str]) -> Vec<T> {
        tokens.iter().filter_map(|t| t.parse().ok()).collect()
    }
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some((&keyword, rest)) = tokens.split_first() else { continue };
        let prim = current.as_ref().and_then(|p| stage.prims.get_mut(p));
        match (keyword, prim) {
            ("animated", _) => stage.animated = rest.first() == Some(&"1"),
            ("prim", _) if rest.len() >= 4 => {
                let path = rest[0].to_string();
                let mut prim = Prim::new(rest[1]);
                prim.active = rest[2] == "1";
                prim.visible = rest[3] == "1";
                stage.prims.insert(path.clone(), prim);
                current = Some(path);
                (points, counts, colour, left_handed) = (Vec::new(), Vec::new(), None, false);
            }
            ("xform", Some(prim)) => {
                let v: Vec<f32> = numbers(rest);
                if v.len() == 16 {
                    let m: Mat4 = std::array::from_fn(|r| std::array::from_fn(|c| v[r * 4 + c]));
                    prim.xform = mirror_matrix(&m);
                }
            }
            ("binding", Some(prim)) => prim.binding = rest.first().map(|s| s.to_string()),
            ("f", Some(prim)) if rest.len() == 2 => {
                if let Ok(f) = rest[1].parse() {
                    prim.attrs.insert(rest[0].to_string(), AttrValue::Float(f));
                }
            }
            ("v", Some(prim)) if rest.len() == 4 => {
                let v: Vec<f32> = numbers(&rest[1..]);
                if v.len() == 3 {
                    prim.attrs.insert(rest[0].to_string(), AttrValue::Vec3([v[0], v[1], v[2]]));
                }
            }
            ("s", Some(prim)) if rest.len() == 2 => {
                prim.attrs.insert(rest[0].to_string(), AttrValue::Token(rest[1].to_string()));
            }
            ("lefthanded", Some(_)) => left_handed = true,
            ("color", Some(prim)) if rest.len() >= 2 => {
                let values: Vec<f32> = numbers(&rest[2..]);
                colour = Some((rest[0].to_string(), values.clone()));
                if !matches!(prim.type_name.as_str(), "Mesh") && values.len() >= 3 {
                    // Gprims keep a single colour for tessellation.
                    let mut holder = Mesh::default();
                    holder.point_attributes.insert("detail:Cd".into(), values[..3].to_vec());
                    prim.mesh = Some(Arc::new(holder));
                }
            }
            ("points", Some(_)) => {
                let v: Vec<f32> = numbers(&rest[1..]);
                points = v.chunks_exact(3).map(|c| Vec3::new(c[0], c[1], -c[2])).collect();
            }
            ("counts", Some(_)) => counts = numbers(&rest[1..]),
            ("indices", Some(prim)) => {
                let indices: Vec<usize> = numbers(&rest[1..]);
                prim.mesh = Some(Arc::new(triangulate(&points, &counts, &indices, colour.as_ref(), left_handed)));
            }
            _ => {}
        }
    }
    Ok(stage)
}

// Fan-triangulates polygons; mirroring Z (and leftHanded orientation) set the winding.
fn triangulate(points: &[Vec3], counts: &[usize], indices: &[usize], colour: Option<&(String, Vec<f32>)>, left_handed: bool) -> Mesh {
    let mut mesh = Mesh { vertices: points.to_vec(), ..Default::default() };
    let mut face_of_tri = Vec::new();
    let mut corner_of = Vec::new();
    let mut offset = 0;
    for (face, &count) in counts.iter().enumerate() {
        let face_indices = &indices[offset.min(indices.len())..(offset + count).min(indices.len())];
        for k in 1..count.saturating_sub(1) {
            let corners = [0, k, k + 1];
            if corners.iter().all(|c| *c < face_indices.len() && face_indices[*c] < points.len()) {
                let order = if left_handed { [0, 1, 2] } else { [0, 2, 1] };
                let tri = order.map(|o| face_indices[corners[o]]);
                mesh.indices.push(tri);
                face_of_tri.push(face);
                corner_of.push(order.map(|o| offset + corners[o]));
            }
        }
        offset += count;
    }
    if let Some((interpolation, values)) = colour {
        let rgb = |i: usize| values.get(i * 3..i * 3 + 3).map(|c| [c[0], c[1], c[2]]);
        match interpolation.as_str() {
            "constant" if values.len() >= 3 => {
                mesh.point_attributes.insert("detail:Cd".into(), values[..3].to_vec());
            }
            "uniform" => {
                let v: Vec<f32> = face_of_tri.iter().flat_map(|f| rgb(*f).unwrap_or([0.7; 3])).collect();
                mesh.point_attributes.insert("prim:Cd".into(), v);
            }
            "vertex" | "varying" if values.len() == points.len() * 3 => {
                mesh.point_attributes.insert("Cd".into(), values.clone());
            }
            "faceVarying" => {
                let v: Vec<f32> = corner_of.iter().flat_map(|c| c.iter().flat_map(|i| rgb(*i).unwrap_or([0.7; 3]))).collect();
                mesh.point_attributes.insert("vertex:Cd".into(), v);
            }
            _ => {}
        }
    }
    mesh
}

#[cfg(feature = "native-usd")]
mod native {
    use std::ffi::{c_char, c_int, CStr, CString};

    extern "C" {
        fn pfx_usd_load(path: *const c_char, seconds: f64, reload: c_int, error: *mut *mut c_char) -> *mut c_char;
        fn pfx_usd_export(usda: *const c_char, path: *const c_char, error: *mut *mut c_char) -> c_int;
        fn pfx_usd_free(text: *mut c_char);
    }

    fn take(text: *mut c_char) -> String {
        if text.is_null() {
            return String::new();
        }
        let owned = unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned();
        unsafe { pfx_usd_free(text) };
        owned
    }

    pub fn load(path: &str, seconds: f32, reload: bool) -> Result<String, String> {
        let c_path = CString::new(path).map_err(|_| "invalid path".to_string())?;
        let mut error = std::ptr::null_mut();
        let text = unsafe { pfx_usd_load(c_path.as_ptr(), seconds as f64, reload as c_int, &mut error) };
        if text.is_null() {
            return Err(take(error));
        }
        Ok(take(text))
    }

    pub fn export(usda: &str, path: &str) -> Result<(), String> {
        let (c_usda, c_path) = (CString::new(usda).map_err(|e| e.to_string())?, CString::new(path).map_err(|e| e.to_string())?);
        let mut error = std::ptr::null_mut();
        if unsafe { pfx_usd_export(c_usda.as_ptr(), c_path.as_ptr(), &mut error) } == 1 {
            Ok(())
        } else {
            Err(take(error))
        }
    }
}

pub const NATIVE: bool = cfg!(feature = "native-usd");

// Parsed files by (path, time in ms), so cooks during playback only read each frame once.
fn load_cache() -> &'static Mutex<HashMap<(String, i64), Arc<Stage>>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, i64), Arc<Stage>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

// Opens a USD file with OpenUSD and returns its composed prims at `seconds`.
pub fn load_file(path: &str, seconds: f32) -> Result<Arc<Stage>, String> {
    if path.trim().is_empty() {
        return Err("no file chosen".into());
    }
    let key = (path.to_string(), (seconds * 1000.0).round() as i64);
    if let Some(stage) = load_cache().lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return Ok(stage);
    }
    #[cfg(feature = "native-usd")]
    let stage = {
        let dump = native::load(path, seconds, false)?;
        Arc::new(parse_bridge_dump(&dump)?)
    };
    #[cfg(not(feature = "native-usd"))]
    let stage: Arc<Stage> = return Err("reading USD files needs the native-usd build (OpenUSD)".into());
    if let Ok(mut cache) = load_cache().lock() {
        if cache.len() > 256 {
            cache.clear();
        }
        // A static file looks the same at every time: cache it once under time 0 too.
        if !stage.animated {
            cache.insert((path.to_string(), 0), stage.clone());
        }
        cache.insert(key, stage.clone());
    }
    Ok(stage)
}

pub fn is_animated(path: &str) -> bool {
    load_file(path, 0.0).is_ok_and(|s| s.animated)
}

// Re-reads files that changed on disk.
pub fn forget_loaded_files() {
    if let Ok(mut cache) = load_cache().lock() {
        cache.clear();
    }
    #[cfg(feature = "native-usd")]
    {
        // Reopening with reload refreshes the bridge's open stages lazily on the next load.
    }
}

// Writes the stage. .usda is written directly; .usd, .usdc and .usdz go through OpenUSD.
pub fn save(stage: &Stage, path: &str) -> Result<(), String> {
    let text = to_usda(stage);
    let extension = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if extension == "usda" {
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    } else {
        #[cfg(feature = "native-usd")]
        native::export(&text, path)?;
        #[cfg(not(feature = "native-usd"))]
        return Err(format!("writing .{extension} needs the native-usd build; save as .usda instead"));
    }
    forget_loaded_files();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_stage() -> Stage {
        let mut stage = Stage::default();
        let prim = stage.define("/geo/tri", "Mesh");
        prim.mesh = Some(Arc::new(Mesh {
            vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 1.0)],
            indices: vec![[0, 1, 2]],
            point_attributes: HashMap::from([("Cd".to_string(), vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])]),
        }));
        stage.prims.get_mut("/geo").unwrap().xform = trs([2.0, 0.0, 0.0], [0.0; 3], [1.0; 3]);
        stage
    }

    #[test]
    fn define_creates_parents_and_sanitizes() {
        let mut stage = Stage::default();
        stage.define("geo/my box", "Cube");
        assert_eq!(stage.prims.keys().cloned().collect::<Vec<_>>(), vec!["/geo", "/geo/my_box"]);
        assert_eq!(stage.prims["/geo"].type_name, "Xform");
    }

    #[test]
    fn patterns_match_like_houdini() {
        assert!(matches_pattern("/geo/*", "/geo/box"));
        assert!(!matches_pattern("/geo/*", "/geo/box/shape"));
        assert!(matches_pattern("/geo/**", "/geo/box/shape"));
        assert!(matches_pattern("/geo/* ^/geo/box", "/geo/ball"));
        assert!(!matches_pattern("/geo/* ^/geo/box", "/geo/box"));
        assert!(matches_pattern("/lights/key", "/lights/key"));
    }

    #[test]
    fn flatten_applies_parent_transforms_and_visibility() {
        let mut stage = triangle_stage();
        let parts = stage.flatten();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].1.vertices[1], Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(stage.to_mesh().point_attributes["vertex:Cd"].len(), 9);
        stage.prims.get_mut("/geo").unwrap().visible = false;
        assert!(stage.flatten().is_empty());
    }

    #[test]
    fn usda_round_trips_through_the_bridge_format() {
        // The writer mirrors to USD space; reading the same data back mirrors it again.
        let stage = triangle_stage();
        let text = to_usda(&stage);
        assert!(text.starts_with("#usda 1.0"));
        assert!(text.contains("def Xform \"geo\""));
        assert!(text.contains("point3f[] points = [(0, 0, -0), (1, 0, -0), (0, 1, -1)]"));
        assert!(text.contains("faceVertexIndices = [0, 2, 1]"));
        assert!(text.contains("color3f[] primvars:displayColor"));
        let dump = "animated 0\nprim /geo/tri Mesh 1 1\nxform 1 0 0 0 0 1 0 0 0 0 1 0 2 0 -0 1\npoints 3 0 0 0 1 0 0 0 1 -1\ncounts 1 3\nindices 3 0 2 1\n";
        let back = parse_bridge_dump(dump).unwrap();
        let mesh = back.prims["/geo/tri"].mesh.as_ref().unwrap();
        assert_eq!(mesh.vertices[2], Vec3::new(0.0, 1.0, 1.0));
        assert_eq!(mesh.indices[0], [0, 1, 2]);
        assert_eq!(back.prims["/geo/tri"].xform[3][0], 2.0);
    }

    #[test]
    fn quads_triangulate_and_keep_face_colours() {
        let dump = "prim /q Mesh 1 1\ncolor uniform 1 0.2 0.4 0.6\npoints 4 0 0 0 1 0 0 1 1 0 0 1 0\ncounts 1 4\nindices 4 0 1 2 3\n";
        let stage = parse_bridge_dump(dump).unwrap();
        let mesh = stage.prims["/q"].mesh.as_ref().unwrap();
        assert_eq!(mesh.indices.len(), 2);
        assert_eq!(mesh.point_attributes["prim:Cd"], vec![0.2, 0.4, 0.6, 0.2, 0.4, 0.6]);
    }

    #[test]
    fn gprims_tessellate_with_size() {
        let mut stage = Stage::default();
        stage.define("/ball", "Sphere").attrs.insert("radius".into(), AttrValue::Float(2.0));
        let (lo, hi) = stage.to_mesh().bounds().unwrap();
        assert!((hi.y - 2.0).abs() < 1e-3 && (lo.y + 2.0).abs() < 1e-3);
    }

    #[test]
    fn rerooting_moves_bindings() {
        let mut stage = Stage::default();
        stage.define("/mat", "Material");
        stage.define("/box", "Cube").binding = Some("/mat".into());
        let moved = stage.rerooted("/ref");
        assert_eq!(moved.prims["/ref/box"].binding.as_deref(), Some("/ref/mat"));
    }
}
