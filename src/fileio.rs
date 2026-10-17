// Geometry file import and export for the File and File Export nodes:
// OBJ (with vertex colours and UVs), STL (ASCII and binary), FBX (read with
// ufbx; written as binary FBX 7.4), plus `$F` frame-number expansion.
//
// Like the USD and MoonRay exports, files use the right-handed Y-up convention
// of other DCC apps: the app's viewport space is that space mirrored in Z, so
// Z is negated and triangle winding swapped on the way in and out.

use super::{Mesh, Vec3};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;

pub fn extension(path: &str) -> String {
    std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

// Replaces $F (frame number) and $F2..$F9 (zero padded) with the frame at `time`;
// frames count from 1 at time 0, at `fps` frames per second.
pub fn expand_frame(path: &str, time: f32, fps: f32) -> String {
    let frame = (time * fps).round() as i64 + 1;
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(at) = rest.find("$F") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        match after.chars().next().and_then(|c| c.to_digit(10)).filter(|d| (2..=9).contains(d)) {
            Some(width) => {
                let _ = write!(out, "{frame:0width$}", width = width as usize);
                rest = &after[1..];
            }
            None => {
                let _ = write!(out, "{frame}");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn has_frame_token(path: &str) -> bool {
    path.contains("$F")
}

fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, -v.z)
}

// Point attribute values for `name` when present with `comps` components per point.
fn point_attr<'a>(mesh: &'a Mesh, name: &str, comps: usize) -> Option<&'a Vec<f32>> {
    mesh.point_attributes.get(name).filter(|v| v.len() == mesh.vertices.len() * comps && !v.is_empty())
}

// Per-corner values (3 components) from a vertex: attribute, else the point attribute.
fn corner_values(mesh: &Mesh, name: &str) -> Option<Vec<[f32; 3]>> {
    let corners = mesh.indices.len() * 3;
    if let Some(v) = mesh.point_attributes.get(&format!("vertex:{name}")).filter(|v| v.len() == corners * 3 && corners > 0) {
        return Some(v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect());
    }
    let points = point_attr(mesh, name, 3)?;
    Some(mesh.indices.iter().flatten().map(|p| [points[p * 3], points[p * 3 + 1], points[p * 3 + 2]]).collect())
}

pub fn read_geometry(path: &str) -> Result<Mesh, String> {
    if !std::path::Path::new(path).exists() {
        return Err(format!("file not found: {path}"));
    }
    match extension(path).as_str() {
        "obj" => read_obj(&std::fs::read_to_string(path).map_err(|e| e.to_string())?),
        "stl" => read_stl(&std::fs::read(path).map_err(|e| e.to_string())?),
        "fbx" => read_fbx(path),
        other => Err(format!("unsupported geometry format .{other}")),
    }
}

pub fn write_geometry(mesh: &Mesh, path: &str) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = match extension(path).as_str() {
        "obj" => write_obj(mesh).into_bytes(),
        "stl" => write_stl(mesh),
        "fbx" => write_fbx(mesh, std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("mesh")),
        other => return Err(format!("unsupported geometry format .{other}")),
    };
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

// ============================================================================
// OBJ
// ============================================================================

pub fn read_obj(text: &str) -> Result<Mesh, String> {
    let mut mesh = Mesh::default();
    let mut colors: Vec<f32> = Vec::new();
    let mut uvs: Vec<[f32; 3]> = Vec::new();
    let mut corner_uvs: Vec<f32> = Vec::new();
    let mut any_uv = false;
    let mut piece: Vec<f32> = Vec::new();
    let mut prim_piece: Vec<f32> = Vec::new();
    let mut current_piece = 0.0f32;
    let mut pieces_seen = 0usize;
    let mut faces_in_piece = 0usize;
    for (line_number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        let mut tokens = line.split_whitespace();
        let Some(keyword) = tokens.next() else { continue };
        let numbers = |tokens: std::str::SplitWhitespace| tokens.filter_map(|t| t.parse::<f32>().ok()).collect::<Vec<f32>>();
        match keyword {
            "v" => {
                let v = numbers(tokens);
                if v.len() < 3 {
                    return Err(format!("line {}: a vertex needs x y z", line_number + 1));
                }
                mesh.vertices.push(mirror(Vec3::new(v[0], v[1], v[2])));
                // The common "v x y z r g b" vertex-colour extension.
                colors.extend(if v.len() >= 6 { [v[3], v[4], v[5]] } else { [1.0; 3] });
                piece.push(current_piece);
            }
            "vt" => {
                let v = numbers(tokens);
                uvs.push([v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), 0.0]);
            }
            "o" | "g" => {
                // Each named object/group becomes a piece (used by RBD and others).
                if faces_in_piece > 0 || pieces_seen == 0 {
                    current_piece = pieces_seen as f32;
                    pieces_seen += 1;
                    faces_in_piece = 0;
                }
            }
            "f" => {
                let count = mesh.vertices.len() as i64;
                let resolve = |token: &str, len: i64| -> Option<usize> {
                    let i: i64 = token.parse().ok()?;
                    let index = if i < 0 { len + i } else { i - 1 };
                    (0..len).contains(&index).then_some(index as usize)
                };
                let mut corners: Vec<(usize, Option<usize>)> = Vec::new();
                for token in tokens {
                    let mut parts = token.split('/');
                    let v = parts.next().and_then(|t| resolve(t, count)).ok_or_else(|| format!("line {}: bad face index '{token}'", line_number + 1))?;
                    let vt = parts.next().filter(|t| !t.is_empty()).and_then(|t| resolve(t, uvs.len() as i64));
                    corners.push((v, vt));
                }
                for k in 1..corners.len().saturating_sub(1) {
                    // Mirroring Z flips the winding.
                    let tri = [corners[0], corners[k + 1], corners[k]];
                    mesh.indices.push(tri.map(|c| c.0));
                    for (_, vt) in tri {
                        any_uv |= vt.is_some();
                        corner_uvs.extend(vt.map_or([0.0; 3], |i| uvs[i]));
                    }
                    prim_piece.push(current_piece);
                    for (p, _) in tri {
                        piece[p] = current_piece;
                    }
                }
                faces_in_piece += 1;
            }
            _ => {}
        }
    }
    if colors.chunks_exact(3).any(|c| c != [1.0, 1.0, 1.0]) {
        mesh.point_attributes.insert("Cd".into(), colors);
    }
    if any_uv {
        mesh.point_attributes.insert("vertex:uv".into(), corner_uvs);
    }
    if pieces_seen > 1 {
        mesh.point_attributes.insert("piece".into(), piece);
        mesh.point_attributes.insert("prim:piece".into(), prim_piece);
    }
    Ok(mesh)
}

pub fn write_obj(mesh: &Mesh) -> String {
    let mut out = String::from("# ProceduralFX Studio\n");
    let colors = point_attr(mesh, "Cd", 3);
    for (i, v) in mesh.vertices.iter().enumerate() {
        let p = mirror(*v);
        let _ = match colors {
            Some(c) => writeln!(out, "v {} {} {} {} {} {}", p.x, p.y, p.z, c[i * 3], c[i * 3 + 1], c[i * 3 + 2]),
            None => writeln!(out, "v {} {} {}", p.x, p.y, p.z),
        };
    }
    let uvs = corner_values(mesh, "uv");
    if let Some(uvs) = &uvs {
        for uv in uvs {
            let _ = writeln!(out, "vt {} {}", uv[0], uv[1]);
        }
    }
    for (t, tri) in mesh.indices.iter().enumerate() {
        // Corner order 0, 2, 1 restores right-handed winding.
        let corner = |k: usize| match &uvs {
            Some(_) => format!("{}/{}", tri[k] + 1, t * 3 + k + 1),
            None => format!("{}", tri[k] + 1),
        };
        let _ = writeln!(out, "f {} {} {}", corner(0), corner(2), corner(1));
    }
    out
}

// ============================================================================
// STL
// ============================================================================

pub fn read_stl(bytes: &[u8]) -> Result<Mesh, String> {
    let mut triangles: Vec<[Vec3; 3]> = Vec::new();
    let binary_size = (bytes.len() >= 84).then(|| 84 + u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize * 50);
    if binary_size == Some(bytes.len()) {
        for chunk in bytes[84..].chunks_exact(50) {
            let f = |i: usize| f32::from_le_bytes([chunk[i], chunk[i + 1], chunk[i + 2], chunk[i + 3]]);
            triangles.push([0, 1, 2].map(|k| Vec3::new(f(12 + k * 12), f(16 + k * 12), f(20 + k * 12))));
        }
    } else {
        let text = String::from_utf8_lossy(bytes);
        let mut corners = Vec::new();
        for line in text.lines() {
            let mut tokens = line.split_whitespace();
            if tokens.next() == Some("vertex") {
                let v: Vec<f32> = tokens.filter_map(|t| t.parse().ok()).collect();
                if v.len() == 3 {
                    corners.push(Vec3::new(v[0], v[1], v[2]));
                }
            }
        }
        triangles = corners.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        if triangles.is_empty() {
            return Err("no triangles found (not a valid STL file)".into());
        }
    }
    // STL repeats shared corners; weld exact duplicates back into shared points.
    let mut mesh = Mesh::default();
    let mut index: HashMap<[u32; 3], usize> = HashMap::new();
    for tri in triangles {
        let ids = tri.map(|v| {
            let v = mirror(v);
            *index.entry([v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]).or_insert_with(|| {
                mesh.vertices.push(v);
                mesh.vertices.len() - 1
            })
        });
        mesh.indices.push([ids[0], ids[2], ids[1]]);
    }
    Ok(mesh)
}

pub fn write_stl(mesh: &Mesh) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out[..20].copy_from_slice(b"ProceduralFX Studio\0");
    out.extend((mesh.indices.len() as u32).to_le_bytes());
    for tri in &mesh.indices {
        let [a, b, c] = [tri[0], tri[2], tri[1]].map(|i| mirror(mesh.vertices[i]));
        let n = b.sub(a).cross(c.sub(a)).normalize();
        for v in [n, a, b, c] {
            for f in [v.x, v.y, v.z] {
                out.extend(f.to_le_bytes());
            }
        }
        out.extend([0u8, 0]);
    }
    out
}

// ============================================================================
// FBX
// ============================================================================

pub fn read_fbx(path: &str) -> Result<Mesh, String> {
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        ..Default::default()
    };
    let scene = ufbx::load_file(path, opts).map_err(|e| format!("could not read FBX: {}", e.description))?;
    let mut out = Mesh::default();
    let (mut colors, mut uvs, mut piece, mut prim_piece) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut any_color, mut any_uv) = (false, false);
    let mut triangle_corners: Vec<u32> = Vec::new();
    for (piece_index, node) in scene.nodes.iter().filter(|n| n.mesh.is_some()).enumerate() {
        let mesh = node.mesh.as_ref().expect("filtered");
        let to_world = node.geometry_to_world;
        let base = out.vertices.len();
        for v in mesh.vertices.iter() {
            let p = ufbx::transform_position(&to_world, *v);
            out.vertices.push(mirror(Vec3::new(p.x as f32, p.y as f32, p.z as f32)));
            piece.push(piece_index as f32);
        }
        for face in mesh.faces.iter() {
            let count = ufbx::triangulate_face_vec(&mut triangle_corners, mesh, *face) as usize;
            for tri in triangle_corners[..count * 3].chunks_exact(3) {
                // Corners 0, 2, 1: the mirror flips the winding.
                let corners = [tri[0], tri[2], tri[1]].map(|c| c as usize);
                out.indices.push(corners.map(|c| base + mesh.vertex_indices[c] as usize));
                prim_piece.push(piece_index as f32);
                for c in corners {
                    if mesh.vertex_color.exists {
                        any_color = true;
                        let v = mesh.vertex_color.values[mesh.vertex_color.indices[c] as usize];
                        colors.extend([v.x as f32, v.y as f32, v.z as f32]);
                    } else {
                        colors.extend([1.0f32; 3]);
                    }
                    if mesh.vertex_uv.exists {
                        any_uv = true;
                        let v = mesh.vertex_uv.values[mesh.vertex_uv.indices[c] as usize];
                        uvs.extend([v.x as f32, v.y as f32, 0.0]);
                    } else {
                        uvs.extend([0.0f32; 3]);
                    }
                }
            }
        }
    }
    if out.vertices.is_empty() {
        return Err("the FBX file contains no meshes".into());
    }
    if any_color {
        out.point_attributes.insert("vertex:Cd".into(), colors);
    }
    if any_uv {
        out.point_attributes.insert("vertex:uv".into(), uvs);
    }
    if prim_piece.iter().any(|p| *p > 0.0) {
        out.point_attributes.insert("piece".into(), piece);
        out.point_attributes.insert("prim:piece".into(), prim_piece);
    }
    Ok(out)
}

// A binary FBX node record.
struct FbxNode {
    name: &'static str,
    props: Vec<FbxProp>,
    children: Vec<FbxNode>,
}

enum FbxProp {
    Bool(bool),
    I32(i32),
    I64(i64),
    F64(f64),
    Str(String),
    ArrayF64(Vec<f64>),
    ArrayI32(Vec<i32>),
}

fn node(name: &'static str, props: Vec<FbxProp>, children: Vec<FbxNode>) -> FbxNode {
    FbxNode { name, props, children }
}

fn leaf(name: &'static str, props: Vec<FbxProp>) -> FbxNode {
    node(name, props, Vec::new())
}

// A Properties70 "P" entry.
fn property(name: &str, kind: &str, label: &str, flags: &str, values: Vec<FbxProp>) -> FbxNode {
    let mut props = vec![FbxProp::Str(name.into()), FbxProp::Str(kind.into()), FbxProp::Str(label.into()), FbxProp::Str(flags.into())];
    props.extend(values);
    leaf("P", props)
}

fn encode_prop(out: &mut Vec<u8>, prop: &FbxProp) {
    match prop {
        FbxProp::Bool(b) => {
            out.push(b'C');
            out.push(*b as u8);
        }
        FbxProp::I32(v) => {
            out.push(b'I');
            out.extend(v.to_le_bytes());
        }
        FbxProp::I64(v) => {
            out.push(b'L');
            out.extend(v.to_le_bytes());
        }
        FbxProp::F64(v) => {
            out.push(b'D');
            out.extend(v.to_le_bytes());
        }
        FbxProp::Str(s) => {
            out.push(b'S');
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s.as_bytes());
        }
        FbxProp::ArrayF64(values) => {
            out.push(b'd');
            out.extend((values.len() as u32).to_le_bytes());
            out.extend(0u32.to_le_bytes());
            out.extend(((values.len() * 8) as u32).to_le_bytes());
            values.iter().for_each(|v| out.extend(v.to_le_bytes()));
        }
        FbxProp::ArrayI32(values) => {
            out.push(b'i');
            out.extend((values.len() as u32).to_le_bytes());
            out.extend(0u32.to_le_bytes());
            out.extend(((values.len() * 4) as u32).to_le_bytes());
            values.iter().for_each(|v| out.extend(v.to_le_bytes()));
        }
    }
}

// FBX 7.4 node record: end offset, property count, property bytes, name, properties, children.
fn encode_node(out: &mut Vec<u8>, node: &FbxNode) {
    let start = out.len();
    out.extend([0u8; 12]);
    out.push(node.name.len() as u8);
    out.extend(node.name.as_bytes());
    let props_start = out.len();
    for prop in &node.props {
        encode_prop(out, prop);
    }
    let props_len = (out.len() - props_start) as u32;
    for child in &node.children {
        encode_node(out, child);
    }
    if !node.children.is_empty() || node.props.is_empty() {
        out.extend([0u8; 13]);
    }
    let end = out.len() as u32;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&(node.props.len() as u32).to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&props_len.to_le_bytes());
}

// One mesh as a binary FBX 7.4 file in metres, right-handed Y-up, with vertex colours and UVs.
pub fn write_fbx(mesh: &Mesh, name: &str) -> Vec<u8> {
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    let name = if name.is_empty() { "mesh".to_string() } else { name };
    let (geometry_id, model_id) = (2_000_001i64, 2_000_002i64);
    let vertices: Vec<f64> = mesh.vertices.iter().flat_map(|v| {
        let p = mirror(*v);
        [p.x as f64, p.y as f64, p.z as f64]
    }).collect();
    // Polygon vertex lists end with a bitwise-negated index; corner order 0, 2, 1 for the mirror.
    let polygon_corners = |tri: &[usize; 3]| [tri[0], tri[2], tri[1]];
    let polygons: Vec<i32> = mesh
        .indices
        .iter()
        .flat_map(|tri| {
            let [a, b, c] = polygon_corners(tri).map(|i| i as i32);
            [a, b, !c]
        })
        .collect();
    let reorder = |values: Vec<[f32; 3]>| -> Vec<[f32; 3]> {
        values.chunks_exact(3).flat_map(|c| [c[0], c[2], c[1]]).collect()
    };
    let mut layer_elements = Vec::new();
    let mut geometry_children = vec![
        leaf("Vertices", vec![FbxProp::ArrayF64(vertices)]),
        leaf("PolygonVertexIndex", vec![FbxProp::ArrayI32(polygons)]),
        leaf("GeometryVersion", vec![FbxProp::I32(124)]),
    ];
    if let Some(colors) = corner_values(mesh, "Cd").map(reorder) {
        geometry_children.push(node(
            "LayerElementColor",
            vec![FbxProp::I32(0)],
            vec![
                leaf("Version", vec![FbxProp::I32(101)]),
                leaf("Name", vec![FbxProp::Str("Col".into())]),
                leaf("MappingInformationType", vec![FbxProp::Str("ByPolygonVertex".into())]),
                leaf("ReferenceInformationType", vec![FbxProp::Str("Direct".into())]),
                leaf("Colors", vec![FbxProp::ArrayF64(colors.iter().flat_map(|c| [c[0] as f64, c[1] as f64, c[2] as f64, 1.0]).collect())]),
            ],
        ));
        layer_elements.push("LayerElementColor");
    }
    if let Some(uvs) = corner_values(mesh, "uv").map(reorder) {
        geometry_children.push(node(
            "LayerElementUV",
            vec![FbxProp::I32(0)],
            vec![
                leaf("Version", vec![FbxProp::I32(101)]),
                leaf("Name", vec![FbxProp::Str("UVMap".into())]),
                leaf("MappingInformationType", vec![FbxProp::Str("ByPolygonVertex".into())]),
                leaf("ReferenceInformationType", vec![FbxProp::Str("IndexToDirect".into())]),
                leaf("UV", vec![FbxProp::ArrayF64(uvs.iter().flat_map(|uv| [uv[0] as f64, uv[1] as f64]).collect())]),
                leaf("UVIndex", vec![FbxProp::ArrayI32((0..uvs.len() as i32).collect())]),
            ],
        ));
        layer_elements.push("LayerElementUV");
    }
    if !layer_elements.is_empty() {
        let mut layer = vec![leaf("Version", vec![FbxProp::I32(100)])];
        for element in layer_elements {
            layer.push(node(
                "LayerElement",
                vec![],
                vec![leaf("Type", vec![FbxProp::Str(element.into())]), leaf("TypedIndex", vec![FbxProp::I32(0)])],
            ));
        }
        geometry_children.push(node("Layer", vec![FbxProp::I32(0)], layer));
    }
    let int = |v: i32| vec![FbxProp::I32(v)];
    let document = vec![
        node(
            "FBXHeaderExtension",
            vec![],
            vec![
                leaf("FBXHeaderVersion", int(1003)),
                leaf("FBXVersion", int(7400)),
                leaf("Creator", vec![FbxProp::Str("ProceduralFX Studio".into())]),
            ],
        ),
        node(
            "GlobalSettings",
            vec![],
            vec![
                leaf("Version", int(1000)),
                node(
                    "Properties70",
                    vec![],
                    vec![
                        property("UpAxis", "int", "Integer", "", int(1)),
                        property("UpAxisSign", "int", "Integer", "", int(1)),
                        property("FrontAxis", "int", "Integer", "", int(2)),
                        property("FrontAxisSign", "int", "Integer", "", int(1)),
                        property("CoordAxis", "int", "Integer", "", int(0)),
                        property("CoordAxisSign", "int", "Integer", "", int(1)),
                        // Centimetres per unit: 100 means the coordinates are metres.
                        property("UnitScaleFactor", "double", "Number", "", vec![FbxProp::F64(100.0)]),
                        property("OriginalUnitScaleFactor", "double", "Number", "", vec![FbxProp::F64(100.0)]),
                    ],
                ),
            ],
        ),
        node(
            "Definitions",
            vec![],
            vec![
                leaf("Version", int(100)),
                leaf("Count", int(3)),
                node("ObjectType", vec![FbxProp::Str("GlobalSettings".into())], vec![leaf("Count", int(1))]),
                node("ObjectType", vec![FbxProp::Str("Model".into())], vec![leaf("Count", int(1))]),
                node("ObjectType", vec![FbxProp::Str("Geometry".into())], vec![leaf("Count", int(1))]),
            ],
        ),
        node(
            "Objects",
            vec![],
            vec![
                node(
                    "Geometry",
                    vec![FbxProp::I64(geometry_id), FbxProp::Str(format!("{name}\u{0}\u{1}Geometry")), FbxProp::Str("Mesh".into())],
                    geometry_children,
                ),
                node(
                    "Model",
                    vec![FbxProp::I64(model_id), FbxProp::Str(format!("{name}\u{0}\u{1}Model")), FbxProp::Str("Mesh".into())],
                    vec![
                        leaf("Version", int(232)),
                        node("Properties70", vec![], vec![property("DefaultAttributeIndex", "int", "Integer", "", int(0))]),
                        leaf("Shading", vec![FbxProp::Bool(true)]),
                        leaf("Culling", vec![FbxProp::Str("CullingOff".into())]),
                    ],
                ),
            ],
        ),
        node(
            "Connections",
            vec![],
            vec![
                leaf("C", vec![FbxProp::Str("OO".into()), FbxProp::I64(geometry_id), FbxProp::I64(model_id)]),
                leaf("C", vec![FbxProp::Str("OO".into()), FbxProp::I64(model_id), FbxProp::I64(0)]),
            ],
        ),
    ];
    let mut out = Vec::new();
    out.extend(b"Kaydara FBX Binary  \x00\x1a\x00");
    out.extend(7400u32.to_le_bytes());
    for top in &document {
        encode_node(&mut out, top);
    }
    out.extend([0u8; 13]);
    // Standard footer: id, padding to 16 bytes, version, reserved block, magic.
    out.extend([0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e]);
    out.extend([0u8; 4]);
    let pad = (16 - out.len() % 16) % 16;
    out.extend(std::iter::repeat(0u8).take(pad));
    out.extend(7400u32.to_le_bytes());
    out.extend([0u8; 120]);
    out.extend([0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b]);
    out
}

// Writes `bytes` atomically so a viewer reading the file never sees a half-written one.
pub fn write_atomic(path: &str, bytes: &[u8]) -> Result<(), String> {
    let temp = format!("{path}.tmp");
    let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coloured_quad() -> Mesh {
        let mut mesh = Mesh {
            vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.5), Vec3::new(0.0, 1.0, 0.5)],
            indices: vec![[0, 1, 2], [0, 2, 3]],
            point_attributes: HashMap::new(),
        };
        mesh.point_attributes.insert("Cd".into(), vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0]);
        mesh
    }

    #[test]
    fn frame_tokens_expand() {
        assert_eq!(expand_frame("/out/geo.$F4.obj", 0.0, 24.0), "/out/geo.0001.obj");
        assert_eq!(expand_frame("/out/geo.$F.obj", 1.0, 24.0), "/out/geo.25.obj");
        assert_eq!(expand_frame("plain.obj", 3.0, 24.0), "plain.obj");
    }

    #[test]
    fn obj_round_trips_with_colours() {
        let mesh = coloured_quad();
        let back = read_obj(&write_obj(&mesh)).unwrap();
        assert_eq!(back.vertices, mesh.vertices);
        assert_eq!(back.indices, mesh.indices);
        assert_eq!(back.point_attributes["Cd"], mesh.point_attributes["Cd"]);
    }

    #[test]
    fn obj_reads_quads_negative_indices_uvs_and_groups() {
        let text = "o a\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/1 2/2 3/3 4/4\no b\nv 5 0 0\nv 6 0 0\nv 6 1 0\nf -3 -2 -1\n";
        let mesh = read_obj(text).unwrap();
        assert_eq!(mesh.indices.len(), 3);
        assert_eq!(mesh.point_attributes["vertex:uv"].len(), 3 * 3 * 3);
        assert_eq!(mesh.point_attributes["prim:piece"], vec![0.0, 0.0, 1.0]);
        // Mirrored into app space: winding flips so the face still faces the same way.
        assert_eq!(mesh.indices[0], [0, 2, 1]);
    }

    #[test]
    fn stl_round_trips_and_welds() {
        let mesh = coloured_quad();
        let back = read_stl(&write_stl(&mesh)).unwrap();
        assert_eq!(back.vertices.len(), 4);
        // Welding renumbers points; each triangle keeps its corners and orientation.
        for (a, b) in back.indices.iter().zip(&mesh.indices) {
            assert_eq!(a.map(|i| back.vertices[i]), b.map(|i| mesh.vertices[i]));
        }
    }

    #[test]
    fn fbx_round_trips_through_ufbx() {
        let mut mesh = coloured_quad();
        let uvs: Vec<f32> = (0..6).flat_map(|i| [i as f32 * 0.1, 1.0 - i as f32 * 0.1, 0.0]).collect();
        mesh.point_attributes.insert("vertex:uv".into(), uvs);
        let path = std::env::temp_dir().join("pfx_round_trip.fbx");
        let path = path.to_string_lossy().into_owned();
        write_geometry(&mesh, &path).unwrap();
        let back = read_geometry(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.indices.len(), 2);
        for (a, b) in back.vertices.iter().zip(&mesh.vertices) {
            assert!(a.sub(*b).length() < 1e-5, "{a:?} vs {b:?}");
        }
        assert_eq!(back.indices, mesh.indices);
        let colours = &back.point_attributes["vertex:Cd"];
        assert_eq!(&colours[..3], &[1.0, 0.0, 0.0]);
        let uvs = &back.point_attributes["vertex:uv"];
        for (a, b) in uvs.iter().zip(mesh.point_attributes["vertex:uv"].iter()) {
            assert!((a - b).abs() < 1e-5, "uv {a} vs {b}");
        }
    }
}
