// glTF 2.0: .glb (binary) and .gltf (JSON) import and export.
//
// Import reads every mesh in the file's scene, placed by its nodes' transforms (at rest:
// skins, morph targets and animation aren't applied). Points get N, uv (on corners) and
// Cd (and Alpha) from COLOR_0; a material's base colour (its factor and texture, sampled at
// each corner) becomes corner colours, so textured models show their colours. Each mesh
// placed in the scene is a piece. glTF lines aren't read.
//
// Export writes the geometry as one mesh: triangles (or points, or curves as lines), with
// normals when it has N, uv as TEXCOORD_0 and Cd (with Alpha) as COLOR_0. A .gltf is one
// self-contained file (its buffer is embedded). glTF is right-handed like OBJ, so Z is
// mirrored as for OBJ and FBX, and its uv start at the top left, so v is flipped.

use super::{Mesh, Vec3};
use serde_json::json;

fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, -v.z)
}

fn transform(m: &[[f32; 4]; 4], p: [f32; 3], w: f32) -> [f32; 3] {
    // Column-major, as glTF stores it.
    [0, 1, 2].map(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r] * w)
}

fn multiply(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (c, column) in out.iter_mut().enumerate() {
        for (r, value) in column.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

// Normals go through the inverse transpose of the upper 3x3.
fn normal_matrix(m: &[[f32; 4]; 4]) -> [[f32; 3]; 3] {
    let a = |r: usize, c: usize| m[c][r];
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0)) + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    let inv = if det.abs() > 1e-20 { 1.0 / det } else { 1.0 };
    // The cofactor matrix over the determinant is the inverse transpose.
    let c = |r0: usize, r1: usize, c0: usize, c1: usize| a(r0, c0) * a(r1, c1) - a(r0, c1) * a(r1, c0);
    [
        [c(1, 2, 1, 2) * inv, -c(1, 2, 0, 2) * inv, c(1, 2, 0, 1) * inv],
        [-c(0, 2, 1, 2) * inv, c(0, 2, 0, 2) * inv, -c(0, 2, 0, 1) * inv],
        [c(0, 1, 1, 2) * inv, -c(0, 1, 0, 2) * inv, c(0, 1, 0, 1) * inv],
    ]
}

// A texture's colour at (u, v), linear, repeating; None for formats it doesn't read.
fn sample(image: &gltf::image::Data, uv: [f32; 2]) -> Option<[f32; 4]> {
    use gltf::image::Format;
    let channels = match image.format {
        Format::R8 => 1,
        Format::R8G8 => 2,
        Format::R8G8B8 => 3,
        Format::R8G8B8A8 => 4,
        _ => return None,
    };
    let (w, h) = (image.width as usize, image.height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let x = ((uv[0].rem_euclid(1.0) * w as f32) as usize).min(w - 1);
    let y = ((uv[1].rem_euclid(1.0) * h as f32) as usize).min(h - 1);
    let at = (y * w + x) * channels;
    let px = image.pixels.get(at..at + channels)?;
    let c = |k: usize| crate::color::to_linear(px[k] as f32 / 255.0);
    Some(match channels {
        1 => [c(0), c(0), c(0), 1.0],
        2 => [c(0), c(0), c(0), px[1] as f32 / 255.0],
        3 => [c(0), c(1), c(2), 1.0],
        _ => [c(0), c(1), c(2), px[3] as f32 / 255.0],
    })
}

pub fn read(path: &str) -> Result<Mesh, String> {
    let (doc, buffers, images) = gltf::import(path).map_err(|e| format!("couldn't read the glTF file: {e}"))?;
    // The scene's nodes with their world transforms.
    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let mut placed: Vec<(gltf::Mesh, [[f32; 4]; 4])> = Vec::new();
    let mut stack: Vec<(gltf::Node, [[f32; 4]; 4])> = match doc.default_scene().or_else(|| doc.scenes().next()) {
        Some(scene) => scene.nodes().map(|n| (n, identity)).collect(),
        None => Vec::new(),
    };
    while let Some((node, parent)) = stack.pop() {
        let world = multiply(&parent, &node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            placed.push((mesh, world));
        }
        stack.extend(node.children().map(|c| (c, world)));
    }
    // A file of meshes without a scene: each mesh once, where it is.
    if placed.is_empty() {
        placed = doc.meshes().map(|m| (m, identity)).collect();
    }
    placed.reverse();

    struct Part {
        p: Vec<Vec3>,
        n: Option<Vec<Vec3>>,
        uv: Option<Vec<[f32; 2]>>,
        color: Option<Vec<[f32; 4]>>,
        tris: Vec<[usize; 3]>,
        base: [f32; 4],
        texture: Option<(usize, u32)>,
        piece: usize,
    }
    let mut parts: Vec<Part> = Vec::new();
    for (piece, (mesh, world)) in placed.iter().enumerate() {
        let normals_by = normal_matrix(world);
        for prim in mesh.primitives() {
            use gltf::mesh::Mode;
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = reader.read_positions() else { continue };
            let p: Vec<Vec3> = positions.map(|q| mirror(Vec3::from_arr(transform(world, q, 1.0)))).collect();
            let count = p.len();
            let order: Vec<u32> = reader.read_indices().map(|i| i.into_u32().collect()).unwrap_or_else(|| (0..count as u32).collect());
            let tris: Vec<[usize; 3]> = match prim.mode() {
                Mode::Triangles => order.chunks_exact(3).map(|t| [t[0] as usize, t[1] as usize, t[2] as usize]).collect(),
                Mode::TriangleStrip => (2..order.len()).map(|k| if k % 2 == 0 { [order[k - 2], order[k - 1], order[k]] } else { [order[k - 1], order[k - 2], order[k]] }).map(|t| t.map(|i| i as usize)).collect(),
                Mode::TriangleFan => (2..order.len()).map(|k| [order[0] as usize, order[k - 1] as usize, order[k] as usize]).collect(),
                Mode::Points => Vec::new(),
                _ => continue,
            };
            // Mirroring Z flips the winding.
            let tris: Vec<[usize; 3]> = tris.into_iter().filter(|t| t.iter().all(|&i| i < count)).map(|[a, b, c]| [a, c, b]).collect();
            let n = reader.read_normals().map(|ns| {
                ns.map(|q| {
                    let m = &normals_by;
                    let t = [0, 1, 2].map(|r| m[r][0] * q[0] + m[r][1] * q[1] + m[r][2] * q[2]);
                    let v = mirror(Vec3::from_arr(t));
                    if v.length() > 1e-12 { v.normalize() } else { v }
                })
                .collect::<Vec<_>>()
            });
            let material = prim.material();
            let pbr = material.pbr_metallic_roughness();
            let texture = pbr.base_color_texture().map(|info| (info.texture().source().index(), info.tex_coord()));
            let uv_set = texture.map_or(0, |(_, set)| set);
            let uv = reader.read_tex_coords(uv_set).or_else(|| reader.read_tex_coords(0)).map(|t| t.into_f32().collect::<Vec<_>>());
            let color = reader.read_colors(0).map(|c| c.into_rgba_f32().collect::<Vec<_>>());
            parts.push(Part { p, n: n.filter(|n| n.len() == count), uv: uv.filter(|u| u.len() == count), color: color.filter(|c| c.len() == count), tris, base: pbr.base_color_factor(), texture, piece });
        }
    }
    if parts.is_empty() {
        return Err("the glTF file has no meshes".into());
    }

    let mut out = Mesh::default();
    let all_normals = parts.iter().all(|p| p.n.is_some() || p.p.is_empty());
    let any_uv = parts.iter().any(|p| p.uv.is_some() && !p.tris.is_empty());
    let any_color = parts.iter().any(|p| p.color.is_some());
    // Corner colours when a material colours it (a texture, or a base colour other than white).
    let by_material = parts.iter().any(|p| p.texture.is_some_and(|(i, _)| i < images.len()) || p.base[..3] != [1.0, 1.0, 1.0]);
    let any_alpha = parts.iter().any(|p| p.color.as_ref().is_some_and(|c| c.iter().any(|c| c[3] < 1.0)));
    let (mut normals, mut cd, mut alpha, mut corner_uv, mut corner_cd, mut piece, mut prim_piece) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for part in &parts {
        let base = out.vertices.len();
        out.vertices.extend(&part.p);
        if all_normals {
            normals.extend(part.n.iter().flatten().flat_map(|v| [v.x, v.y, v.z]));
        }
        for i in 0..part.p.len() {
            let c = part.color.as_ref().map_or([1.0; 4], |c| c[i]);
            cd.extend(&c[..3]);
            alpha.push(c[3]);
            piece.push(part.piece as f32);
        }
        for t in &part.tris {
            out.indices.push(t.map(|i| base + i));
            prim_piece.push(part.piece as f32);
            for &i in t {
                let uv = part.uv.as_ref().map_or([0.0; 2], |u| u[i]);
                corner_uv.extend([uv[0], 1.0 - uv[1], 0.0]);
                if by_material {
                    let tex = part.texture.and_then(|(k, _)| images.get(k)).and_then(|img| sample(img, uv)).unwrap_or([1.0; 4]);
                    let vc = part.color.as_ref().map_or([1.0; 4], |c| c[i]);
                    corner_cd.extend((0..3).map(|k| part.base[k] * tex[k] * vc[k]));
                }
            }
        }
    }
    if all_normals && normals.len() == out.vertices.len() * 3 {
        out.point_attributes.insert("N".into(), normals);
    }
    if any_color {
        out.point_attributes.insert("Cd".into(), cd);
        if any_alpha {
            out.point_attributes.insert("Alpha".into(), alpha);
        }
    }
    if any_uv && !out.indices.is_empty() {
        out.point_attributes.insert("vertex:uv".into(), corner_uv);
    }
    if by_material && !out.indices.is_empty() {
        out.point_attributes.insert("vertex:Cd".into(), corner_cd);
    }
    if placed.len() > 1 {
        out.point_attributes.insert("piece".into(), piece);
        out.point_attributes.insert("prim:piece".into(), prim_piece);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// The geometry as glTF: a .glb's bytes when `binary`, else a .gltf's JSON.
pub fn write(mesh: &Mesh, binary: bool) -> Result<Vec<u8>, String> {
    if mesh.vertices.is_empty() {
        return Err("there's nothing to export".into());
    }
    let n = mesh.vertices.len();
    let tris = mesh.indices.len();
    let attr = |name: &str, w: usize| mesh.point_attributes.get(name).filter(|v| v.len() == n * w && n > 0);
    let corner = |name: &str, w: usize| mesh.point_attributes.get(&format!("vertex:{name}")).filter(|v| tris > 0 && v.len() == tris * 3 * w);
    let prim = |name: &str, w: usize| mesh.point_attributes.get(&format!("prim:{name}")).filter(|v| tris > 0 && v.len() == tris * w);
    let has_alpha = attr("Alpha", 1).is_some() || corner("Alpha", 1).is_some();
    let has_color = attr("Cd", 3).is_some() || corner("Cd", 3).is_some() || prim("Cd", 3).is_some();
    let has_normals = attr("N", 3).is_some() || corner("N", 3).is_some();
    let has_uv = attr("uv", 3).is_some() || corner("uv", 3).is_some();

    // Each glTF vertex has one value of everything: when values differ per corner, every
    // triangle corner is its own vertex.
    let split = tris > 0 && (corner("uv", 3).is_some() || corner("N", 3).is_some() || corner("Cd", 3).is_some() || corner("Alpha", 1).is_some() || prim("Cd", 3).is_some());
    let (mut pos, mut nrm, mut uvs, mut col, mut idx) = (Vec::<f32>::new(), Vec::<f32>::new(), Vec::<f32>::new(), Vec::<f32>::new(), Vec::<u32>::new());
    let mut push = |p: usize, c: Option<(usize, usize)>| {
        let q = mirror(mesh.vertices[p]);
        pos.extend([q.x, q.y, q.z]);
        let three = |name: &str| -> Option<[f32; 3]> {
            if let (Some((t, k)), Some(v)) = (c, corner(name, 3)) {
                let at = (t * 3 + k) * 3;
                return Some([v[at], v[at + 1], v[at + 2]]);
            }
            if let (Some((t, _)), Some(v)) = (c, prim(name, 3)) {
                return Some([v[t * 3], v[t * 3 + 1], v[t * 3 + 2]]);
            }
            attr(name, 3).map(|v| [v[p * 3], v[p * 3 + 1], v[p * 3 + 2]])
        };
        if has_normals {
            let v = mirror(Vec3::from_arr(three("N").unwrap_or([0.0, 1.0, 0.0])));
            let v = if v.length() > 1e-12 { v.normalize() } else { Vec3::new(0.0, 1.0, 0.0) };
            nrm.extend([v.x, v.y, v.z]);
        }
        if has_uv {
            let uv = three("uv").unwrap_or([0.0; 3]);
            uvs.extend([uv[0], 1.0 - uv[1]]);
        }
        if has_color || has_alpha {
            col.extend(three("Cd").unwrap_or([1.0; 3]));
            if has_alpha {
                let a = match (c, corner("Alpha", 1), attr("Alpha", 1)) {
                    (Some((t, k)), Some(v), _) => v[t * 3 + k],
                    (_, _, Some(v)) => v[p],
                    _ => 1.0,
                };
                col.push(a);
            }
        }
    };
    let mode;
    if tris > 0 {
        mode = 4;
        if split {
            for (t, tri) in mesh.indices.iter().enumerate() {
                // Mirroring Z flips the winding back.
                for k in [0, 2, 1] {
                    push(tri[k], Some((t, k)));
                    idx.push(idx.len() as u32);
                }
            }
        } else {
            for p in 0..n {
                push(p, None);
            }
            idx.extend(mesh.indices.iter().flat_map(|t| [t[0] as u32, t[2] as u32, t[1] as u32]));
        }
    } else {
        for p in 0..n {
            push(p, None);
        }
        let curves = crate::groom::curves(mesh);
        if curves.is_empty() {
            mode = 0;
        } else {
            mode = 1;
            idx.extend(curves.iter().flat_map(|r| r.clone().collect::<Vec<_>>().windows(2).flat_map(|w| [w[0] as u32, w[1] as u32]).collect::<Vec<_>>()));
        }
    }
    drop(push);
    // Corners with the same values everywhere share a vertex again.
    if split {
        let widths = [3, if has_normals { 3 } else { 0 }, if has_uv { 2 } else { 0 }, if has_color || has_alpha { if has_alpha { 4 } else { 3 } } else { 0 }];
        let arrays = [&pos, &nrm, &uvs, &col];
        let mut seen: std::collections::HashMap<Vec<u32>, u32> = std::collections::HashMap::new();
        let mut kept: [Vec<f32>; 4] = Default::default();
        let mut remap = Vec::with_capacity(idx.len());
        for v in 0..pos.len() / 3 {
            let key: Vec<u32> = (0..4).flat_map(|a| arrays[a][v * widths[a]..(v + 1) * widths[a]].iter().map(|f| f.to_bits())).collect();
            let next = seen.len() as u32;
            let at = *seen.entry(key).or_insert_with(|| {
                for a in 0..4 {
                    kept[a].extend_from_slice(&arrays[a][v * widths[a]..(v + 1) * widths[a]]);
                }
                next
            });
            remap.push(at);
        }
        for i in idx.iter_mut() {
            *i = remap[*i as usize];
        }
        [pos, nrm, uvs, col] = kept;
    }

    // One buffer, one view per array.
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |bytes: &[u8], count: usize, kind: &str, component: u32, target: u32, extra: serde_json::Value| -> usize {
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len(), "target": target}));
        let mut a = json!({"bufferView": views.len() - 1, "componentType": component, "count": count, "type": kind});
        if let (Some(a), Some(extra)) = (a.as_object_mut(), extra.as_object()) {
            a.extend(extra.clone());
        }
        accessors.push(a);
        accessors.len() - 1
    };
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let count = pos.len() / 3;
    let (lo, hi) = pos.chunks_exact(3).fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| ([0, 1, 2].map(|k| lo[k].min(p[k])), [0, 1, 2].map(|k| hi[k].max(p[k]))));
    let mut attributes = serde_json::Map::new();
    attributes.insert("POSITION".into(), json!(add(&floats(&pos), count, "VEC3", 5126, 34962, json!({"min": lo, "max": hi}))));
    if has_normals {
        attributes.insert("NORMAL".into(), json!(add(&floats(&nrm), count, "VEC3", 5126, 34962, json!({}))));
    }
    if has_uv {
        attributes.insert("TEXCOORD_0".into(), json!(add(&floats(&uvs), count, "VEC2", 5126, 34962, json!({}))));
    }
    if has_color || has_alpha {
        attributes.insert("COLOR_0".into(), json!(add(&floats(&col), count, if has_alpha { "VEC4" } else { "VEC3" }, 5126, 34962, json!({}))));
    }
    let mut primitive = json!({"attributes": attributes, "mode": mode, "material": 0});
    if !idx.is_empty() {
        let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
        primitive["indices"] = json!(add(&bytes, idx.len(), "SCALAR", 5125, 34963, json!({})));
    }
    let buffer = if binary { json!({"byteLength": bin.len()}) } else { json!({"byteLength": bin.len(), "uri": format!("data:application/octet-stream;base64,{}", base64(&bin))}) };
    let mut material = json!({"name": "material", "pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.8}});
    if has_alpha {
        material["alphaMode"] = json!("BLEND");
    }
    let doc = json!({
        "asset": {"version": "2.0", "generator": "ProceduralFX Studio"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "geometry", "mesh": 0}],
        "meshes": [{"name": "geometry", "primitives": [primitive]}],
        "materials": [material],
        "buffers": [buffer],
        "bufferViews": views,
        "accessors": accessors,
    });
    let text = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    if !binary {
        return Ok(text);
    }
    let mut json_chunk = text;
    while json_chunk.len() % 4 != 0 {
        json_chunk.push(b' ');
    }
    let total = 12 + 8 + json_chunk.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend(0x4654_6C67u32.to_le_bytes());
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json_chunk.len() as u32).to_le_bytes());
    out.extend(0x4E4F_534Au32.to_le_bytes());
    out.extend(json_chunk);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(0x004E_4942u32.to_le_bytes());
    out.extend(bin);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pfx_gltf_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn meshes_go_out_and_come_back_as_glb_and_gltf() {
        let mut cube = crate::node_box(2.0, 1);
        let corners = cube.indices.len() * 3;
        cube.point_attributes.insert("vertex:uv".into(), (0..corners).flat_map(|i| [(i % 3) as f32 * 0.5, 0.25, 0.0]).collect());
        cube.point_attributes.insert("Cd".into(), cube.vertices.iter().flat_map(|p| [p.x * 0.5 + 0.5, 0.2, 0.1]).collect());
        for (name, binary) in [("cube.glb", true), ("cube.gltf", false)] {
            let path = temp(name);
            crate::fileio::write_geometry(&cube, path.to_str().unwrap()).unwrap();
            let back = read(path.to_str().unwrap()).unwrap();
            assert_eq!(back.indices.len(), cube.indices.len(), "{name}");
            let (lo, hi) = back.bounds().unwrap();
            assert!((lo.x + 1.0).abs() < 1e-5 && (hi.z - 1.0).abs() < 1e-5, "{name}: {lo:?} {hi:?}");
            // Same winding as before: the normals point out.
            let outward = back.indices.iter().all(|t| {
                let [a, b, c] = t.map(|k| back.vertices[k]);
                let centre = a.add(b).add(c).scale(1.0 / 3.0);
                b.sub(a).cross(c.sub(a)).dot(centre) > 0.0
            });
            assert_eq!(outward, cube.indices.iter().all(|t| {
                let [a, b, c] = t.map(|k| cube.vertices[k]);
                b.sub(a).cross(c.sub(a)).dot(a.add(b).add(c)) > 0.0
            }), "{name}: winding kept");
            // uv and colour come back on each corner and point.
            assert_eq!(back.point_attributes["vertex:uv"], cube.point_attributes["vertex:uv"], "{name}");
            let cd = &back.point_attributes["Cd"];
            assert!(back.vertices.iter().enumerate().all(|(i, p)| (cd[i * 3] - (p.x * 0.5 + 0.5)).abs() < 1e-5), "{name}");
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(&bytes[..4] == b"glTF", binary, "{name}");
        }
        // Points stay points, curves become lines.
        let mut points = Mesh { vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 2.0, 3.0)], ..Mesh::default() };
        points.point_attributes.insert("Alpha".into(), vec![0.5, 1.0]);
        let path = temp("points.glb");
        std::fs::write(&path, write(&points, true).unwrap()).unwrap();
        let back = read(path.to_str().unwrap()).unwrap();
        assert_eq!((back.vertices.len(), back.indices.len()), (2, 0));
        assert_eq!(back.vertices[1], Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(back.point_attributes["Alpha"], vec![0.5, 1.0]);
        let curve = crate::curve::curve(&crate::curve::CurveSettings::defaults()).unwrap();
        assert!(write(&curve, true).is_ok());
        assert!(write(&Mesh::default(), true).is_err());
    }

    #[test]
    fn a_textured_model_shows_its_texture_as_corner_colours() {
        // A quad with a 2x1 texture, red on the left and blue on the right, under a node
        // that moves it up by 2.
        let mut png = Vec::new();
        image::RgbImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) })
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let mut bin = floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        bin.extend(floats(&[0.1, 0.5, 0.9, 0.5, 0.9, 0.5, 0.1, 0.5]));
        bin.extend([0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()));
        let doc = json!({
            "asset": {"version": "2.0"},
            "scene": 0, "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0, "translation": [0.0, 2.0, 0.0]}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "indices": 2, "material": 0}]}],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
            "textures": [{"source": 0}],
            "images": [{"uri": format!("data:image/png;base64,{}", base64(&png))}],
            "buffers": [{"byteLength": bin.len(), "uri": format!("data:application/octet-stream;base64,{}", base64(&bin))}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 48}, {"buffer": 0, "byteOffset": 48, "byteLength": 32}, {"buffer": 0, "byteOffset": 80, "byteLength": 12}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
                {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"},
                {"bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR"}
            ]
        });
        let path = temp("textured.gltf");
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        let m = read(path.to_str().unwrap()).unwrap();
        assert!(m.vertices.iter().all(|p| (2.0..=3.0).contains(&p.y)), "moved up by its node");
        let cd = &m.point_attributes["vertex:Cd"];
        for (t, tri) in m.indices.iter().enumerate() {
            for (k, &p) in tri.iter().enumerate() {
                let c = &cd[(t * 3 + k) * 3..(t * 3 + k) * 3 + 3];
                let expected = if m.vertices[p].x < 0.5 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
                assert_eq!(c, expected, "corner at {:?}", m.vertices[p]);
            }
        }
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }
}
