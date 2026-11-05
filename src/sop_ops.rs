// Sop operations: moved out of main.rs.

use crate::*;

pub(crate) fn node_grid(width: f32, depth: f32, res_x: usize, res_z: usize) -> Mesh {
    let (res_x, res_z) = (res_x.max(2), res_z.max(2));
    let mut mesh = Mesh::default();
    let dx = width / (res_x - 1) as f32;
    let dz = depth / (res_z - 1) as f32;
    let (hw, hd) = (width / 2.0, depth / 2.0);
    for z in 0..res_z {
        for x in 0..res_x {
            mesh.vertices
                .push(Vec3::new(x as f32 * dx - hw, 0.0, z as f32 * dz - hd));
        }

    }
    for z in 0..res_z - 1 {
        for x in 0..res_x - 1 {
            let i0 = z * res_x + x;
            let i1 = i0 + 1;
            let i2 = (z + 1) * res_x + x;
            let i3 = i2 + 1;
            mesh.indices.push([i0, i2, i1]);
            mesh.indices.push([i1, i2, i3]);
        }
    }
    poly::tag_quads(&mut mesh, 0, (res_x - 1) * (res_z - 1));
    mesh
}

// A closed cone: rings of quads narrowing to one apex point (the top row is
// triangles), and a polygon base when `caps` is on.
pub(crate) fn node_cone(radius: f32, height: f32, rows: usize, cols: usize, caps: bool) -> Mesh {
    let rows = rows.max(1);
    let cols = cols.max(3);
    let mut mesh = Mesh::default();
    for r in 0..rows {
        let t = r as f32 / rows as f32;
        let y = -height * 0.5 + height * t;
        let ring_radius = radius * (1.0 - t);
        for c in 0..cols {
            let a = std::f32::consts::TAU * c as f32 / cols as f32;
            mesh.vertices.push(Vec3::new(ring_radius * a.cos(), y, ring_radius * a.sin()));
        }
    }
    let apex = mesh.vertices.len();
    mesh.vertices.push(Vec3::new(0.0, height * 0.5, 0.0));
    for r in 0..rows - 1 {
        for c in 0..cols {
            let a = r * cols + c;
            let b = r * cols + (c + 1) % cols;
            let d = (r + 1) * cols + c;
            let e = (r + 1) * cols + (c + 1) % cols;
            mesh.indices.push([a, d, b]);
            mesh.indices.push([b, d, e]);
        }
    }
    poly::tag_quads(&mut mesh, 0, (rows - 1) * cols);
    let top = (rows - 1) * cols;
    for c in 0..cols {
        mesh.indices.push([top + c, apex, top + (c + 1) % cols]);
    }
    if caps {
        // The base is one polygon (a fan around its centre point), facing down.
        let bottom = mesh.vertices.len();
        mesh.vertices.push(Vec3::new(0.0, -height * 0.5, 0.0));
        let first = mesh.indices.len();
        for c in 0..cols {
            mesh.indices.push([bottom, c, (c + 1) % cols]);
        }
        poly::tag(&mut mesh, first, [cols]);
    }
    mesh
}

pub(crate) fn node_capsule(radius: f32, length: f32, rows: usize, cols: usize) -> Mesh {
    let mut mesh = node_sphere(radius, rows.max(4), cols.max(8));
    let half_length = (length * 0.5).max(0.0);
    for vertex in &mut mesh.vertices {
        vertex.y += if vertex.y >= 0.0 { half_length } else { -half_length };
    }
    mesh
}

/// Subdivided quad patch; normal = u × v.
pub(crate) fn add_patch(m: &mut Mesh, o: Vec3, u: Vec3, v: Vec3, d: usize) {
    let base = m.vertices.len();
    for j in 0..=d {
        for i in 0..=d {
            let (fi, fj) = (i as f32 / d as f32, j as f32 / d as f32);
            m.vertices.push(o.add(u.scale(fi)).add(v.scale(fj)));
        }
    }
    let row = d + 1;
    for j in 0..d {
        for i in 0..d {
            let a = base + j * row + i;
            let (b, c) = (a + 1, a + row);
            m.indices.push([a, b, c]);
            m.indices.push([b, c + 1, c]);
        }
    }
    let first = m.indices.len() - 2 * d * d;
    poly::tag_quads(m, first, d * d);
}

pub(crate) fn node_box(size: f32, div: usize) -> Mesh {
    let d = div.max(1);
    let (s, h) = (size, size / 2.0);
    let mut m = Mesh::default();
    let x = Vec3::new(s, 0.0, 0.0);
    let y = Vec3::new(0.0, s, 0.0);
    let z = Vec3::new(0.0, 0.0, s);
    let n = Vec3::new(-h, -h, -h);
    add_patch(&mut m, Vec3::new(-h, h, -h), z, x, d); // +Y
    add_patch(&mut m, n, x, z, d); // -Y
    add_patch(&mut m, Vec3::new(h, -h, -h), y, z, d); // +X
    add_patch(&mut m, n, z, y, d); // -X
    add_patch(&mut m, Vec3::new(-h, -h, h), x, y, d); // +Z
    add_patch(&mut m, n, y, x, d); // -Z
    node_fuse(m, 1e-4)
}

pub(crate) fn node_sphere(radius: f32, rows: usize, cols: usize) -> Mesh {
    let (rows, cols) = (rows.max(2), cols.max(3));
    let mut m = Mesh::default();
    for r in 0..=rows {
        let t = std::f32::consts::PI * r as f32 / rows as f32;
        for c in 0..=cols {
            let p = std::f32::consts::TAU * c as f32 / cols as f32;
            m.vertices.push(
                Vec3::new(t.sin() * p.cos(), t.cos(), t.sin() * p.sin()).scale(radius),
            );
        }
    }
    for r in 0..rows {
        for c in 0..cols {
            let a = r * (cols + 1) + c;
            let (b, cc) = (a + 1, a + cols + 1);
            m.indices.push([a, b, cc]);
            m.indices.push([b, cc + 1, cc]);
        }
    }
    // Quads, except at the poles, where fusing leaves one triangle of each.
    poly::tag_quads(&mut m, 0, rows * cols);
    node_fuse(m, 1e-4)
}

pub(crate) fn node_torus(major: f32, minor: f32, rows: usize, cols: usize) -> Mesh {
    let (rows, cols) = (rows.max(3), cols.max(3));
    let mut m = Mesh::default();
    for r in 0..rows {
        let t = std::f32::consts::TAU * r as f32 / rows as f32;
        for c in 0..cols {
            let p = std::f32::consts::TAU * c as f32 / cols as f32;
            let ring = major + minor * t.cos();
            m.vertices
                .push(Vec3::new(ring * p.cos(), minor * t.sin(), ring * p.sin()));
        }
    }
    for r in 0..rows {
        for c in 0..cols {
            let idx = |rr: usize, cc: usize| (rr % rows) * cols + (cc % cols);
            let (a, b, cc, d) = (idx(r, c), idx(r, c + 1), idx(r + 1, c), idx(r + 1, c + 1));
            m.indices.push([a, cc, b]);
            m.indices.push([b, cc, d]);
        }
    }
    poly::tag_quads(&mut m, 0, rows * cols);
    m
}

/// Merge vertices closer than `tol`, drop degenerate triangles.
pub(crate) fn node_fuse(mesh: Mesh, tol: f32) -> Mesh {
    let tol = tol.max(1e-7);
    let mut map: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut out = Mesh::default();
    let remap: Vec<usize> = mesh
        .vertices
        .iter()
        .map(|v| {
            let key = (
                (v.x / tol).round() as i64,
                (v.y / tol).round() as i64,
                (v.z / tol).round() as i64,
            );
            *map.entry(key).or_insert_with(|| {
                out.vertices.push(*v);
                out.vertices.len() - 1
            })
        })
        .collect();
    let ids = mesh.point_attributes.get(poly::KEY).filter(|ids| ids.len() == mesh.indices.len());
    let mut kept_ids = Vec::new();
    for (i, t) in mesh.indices.iter().enumerate() {
        let n = [remap[t[0]], remap[t[1]], remap[t[2]]];
        if n[0] != n[1] && n[1] != n[2] && n[0] != n[2] {
            out.indices.push(n);
            kept_ids.extend(ids.map(|ids| ids[i]));
        }
    }
    // Polygons stay polygons (the generators fuse their seams).
    if ids.is_some() {
        out.point_attributes.insert(poly::KEY.into(), kept_ids);
    }
    out
}

pub(crate) fn node_transform(mut mesh: Mesh, t: [f32; 3], r: [f32; 3], s: [f32; 3]) -> Mesh {
    // Packed points and skeleton joints turn and scale with the geometry.
    if mesh.point_attributes.contains_key("transform") {
        let scale = [Vec3::new(s[0], 0.0, 0.0), Vec3::new(0.0, s[1], 0.0), Vec3::new(0.0, 0.0, s[2])];
        packed::turn_frames(&mut mesh, &kinefx::mm(&scale, &kinefx::euler(r)));
    }
    let (t, s) = (Vec3::from_arr(t), Vec3::from_arr(s));
    for v in mesh.vertices.iter_mut() {
        *v = v.mul(s).rotate_deg(r).add(t);
    }
    // Normals turn with the geometry (scaled inversely, so they stay perpendicular to it).
    let inverse = Vec3::new(1.0 / s.x.abs().max(1e-12) * s.x.signum(), 1.0 / s.y.abs().max(1e-12) * s.y.signum(), 1.0 / s.z.abs().max(1e-12) * s.z.signum());
    let (points, tris) = (mesh.vertices.len(), mesh.indices.len());
    for (key, count) in [("N", points), ("vertex:N", tris * 3)] {
        if let Some(normals) = mesh.point_attributes.get_mut(key).filter(|n| count > 0 && n.len() == count * 3) {
            for c in normals.chunks_exact_mut(3) {
                let n = Vec3::new(c[0], c[1], c[2]).mul(inverse).rotate_deg(r).normalize();
                c.copy_from_slice(&[n.x, n.y, n.z]);
            }
        }
    }
    // Negative scale on an odd number of axes flips winding
    if s.x * s.y * s.z < 0.0 {
        for tri in mesh.indices.iter_mut() {
            tri.swap(1, 2);
        }
    }
    mesh
}

// The attribute an attribute node works on: a Common menu that fills everything in, the
// name, the class and the type (with its size for floats). Returns whether anything changed,
// and the Common entry chosen, (name, type, size, value), for the caller to fill in.
pub(crate) type CommonAttribute = (&'static str, usize, usize, [f32; 4]);
pub(crate) fn attribute_target_ui(ui: &mut egui::Ui, id: NodeId, name: &mut String, class: &mut usize, kind: &mut usize, size: &mut usize) -> (bool, Option<CommonAttribute>) {
    let mut c = false;
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.label("Attribute");
        c |= ui.add(egui::TextEdit::singleline(name).desired_width(110.0).hint_text("name")).changed();
        egui::ComboBox::from_id_salt(("attribute_common", id)).selected_text("Common").show_ui(ui, |ui| {
            for (n, k, s, v) in attrib::COMMON {
                if ui.selectable_label(name.trim() == n, format!("{n}  ({})", attrib::TYPES[k])).clicked() {
                    chosen = Some((n, k, s, v));
                }
            }
        });
    });
    if !attrib::valid_name(name.trim()) {
        ui.colored_label(egui::Color32::from_rgb(240, 110, 110), "Use letters, digits and _, not starting with a digit");
    }
    ui.horizontal(|ui| {
        ui.label("Class");
        for (i, n) in attrib::CLASSES.iter().enumerate() {
            c |= ui.selectable_value(class, i, *n).changed();
        }
    });
    ui.horizontal(|ui| {
        ui.label("Type");
        for (i, n) in attrib::TYPES.iter().enumerate() {
            c |= ui.selectable_value(kind, i, *n).changed();
        }
    });
    if *kind == 0 {
        c |= pfield(ui, pslider(size, 1..=4).text("Components")).changed();
    }
    (c || chosen.is_some(), chosen)
}

// An attribute value as its type wants it: a colour picker, a vector, rotation angles,
// a whole number, or 1 to 4 floats.
pub(crate) fn attribute_value_ui(ui: &mut egui::Ui, label: &str, kind: usize, size: usize, value: &mut [f32; 4]) -> bool {
    let shown = match kind {
        attrib::COLOUR | attrib::VECTOR | attrib::QUATERNION => 3,
        attrib::INTEGER => 1,
        _ => size.clamp(1, 4),
    };
    let speed = match kind {
        attrib::QUATERNION => 1.0,
        attrib::INTEGER => 0.2,
        _ => 0.01,
    };
    let mut c = group_label(ui, label, speed, &mut value[..shown]);
    match kind {
        attrib::COLOUR => {
            let mut rgb = [value[0], value[1], value[2]];
            if ui.color_edit_button_rgb(&mut rgb).changed() {
                value[..3].copy_from_slice(&rgb);
                c = true;
            }
            for (k, label) in ["R ", "G ", "B "].iter().enumerate() {
                c |= pfield(ui, pdrag(&mut value[k]).speed(0.01).prefix(*label)).changed();
            }
        }
        attrib::INTEGER => {
            c |= pfield(ui, pdrag(&mut value[0]).speed(0.2).fixed_decimals(0)).changed();
        }
        _ => {
            let n = match kind {
                attrib::VECTOR | attrib::QUATERNION => 3,
                _ => size.clamp(1, 4),
            };
            for (k, label) in ["X ", "Y ", "Z ", "W "].iter().take(n).enumerate() {
                c |= pfield(ui, pdrag(&mut value[k]).speed(if kind == attrib::QUATERNION { 1.0 } else { 0.01 }).prefix(*label)).changed();
            }
        }
    }
    c
}

pub(crate) fn attribute_color(value: f32) -> [f32; 3] {
    let hue = value.clamp(0.0, 1.0) * 6.0;
    let x = 1.0 - (hue % 2.0 - 1.0).abs();
    match hue as usize {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

// Advice from a node's last cook that isn't an error (e.g. a setup that can't move), shown in its panel.
pub(crate) fn node_notes<R>(f: impl FnOnce(&mut HashMap<NodeId, String>) -> R) -> R {
    static NOTES: std::sync::OnceLock<std::sync::Mutex<HashMap<NodeId, String>>> = std::sync::OnceLock::new();
    let mut notes = NOTES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    f(&mut notes)
}

// Latest cook error of each node (file, USD and volume nodes), shown in its parameter panel.
pub(crate) fn node_errors<R>(f: impl FnOnce(&mut HashMap<NodeId, String>) -> R) -> R {
    static ERRORS: std::sync::OnceLock<std::sync::Mutex<HashMap<NodeId, String>>> = std::sync::OnceLock::new();
    let mut errors = ERRORS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    f(&mut errors)
}

// Reads any supported file: polygons through fileio, USD through OpenUSD (flattened), .vdb through OpenVDB.
pub(crate) fn read_geometry_file(path: &str, time: f32) -> Result<Mesh, String> {
    if path.is_empty() {
        return Err("choose a file to read".into());
    }
    match fileio::extension(path).as_str() {
        "usd" | "usda" | "usdc" | "usdz" => usd::load_file(path, time).map(|stage| stage.to_mesh()),
        "abc" => abc::read(path, time),
        "fbx" => fileio::read_fbx_at(path, time),
        "vdb" => read_vdb_file(path),
        _ => fileio::read_geometry(path),
    }
}

pub(crate) fn write_geometry_file(mesh: &Mesh, path: &str) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match fileio::extension(path).as_str() {
        "usd" | "usda" | "usdc" | "usdz" => {
            let mut stage = usd::Stage::default();
            stage.define("/geo/mesh", "Mesh").mesh = Some(Arc::new(mesh.clone()));
            usd::save(&stage, path)
        }
        "vdb" => write_vdb_file(mesh, path),
        _ => fileio::write_geometry(mesh, path),
    }
}

#[cfg(feature = "native-openvdb")]
unsafe extern "C" {
    pub(crate) fn mini_openvdb_volume_voxels(
        handle: *mut std::ffi::c_void,
        out_coords: *mut *mut i32,
        out_values: *mut *mut f32,
        out_count: *mut usize,
        out_voxel: *mut f32,
        out_background: *mut f32,
    ) -> i32;
    pub(crate) fn mini_openvdb_read(
        path: *const std::ffi::c_char,
        out_coords: *mut *mut i32,
        out_values: *mut *mut f32,
        out_count: *mut usize,
        out_voxel: *mut f32,
        out_origin: *mut f32,
        out_background: *mut f32,
        out_level_set: *mut i32,
        out_error: *mut *mut std::ffi::c_char,
    ) -> i32;
    pub(crate) fn mini_openvdb_write(
        path: *const std::ffi::c_char,
        coords: *const i32,
        values: *const f32,
        count: usize,
        voxel: f32,
        origin: *const f32,
        background: f32,
        level_set: i32,
        out_error: *mut *mut std::ffi::c_char,
    ) -> i32;
}

// Takes ownership of bridge-allocated voxel arrays.
#[cfg(feature = "native-openvdb")]
pub(crate) fn take_voxels(coords: *mut i32, values: *mut f32, count: usize, mirror_z: bool) -> Vec<([i64; 3], f32)> {
    let voxels = (0..count)
        .map(|i| unsafe {
            let c = [*coords.add(i * 3), *coords.add(i * 3 + 1), *coords.add(i * 3 + 2)].map(|v| v as i64);
            ([c[0], c[1], if mirror_z { -c[2] } else { c[2] }], *values.add(i))
        })
        .collect();
    unsafe {
        mini_openvdb_free(coords.cast());
        mini_openvdb_free(values.cast());
    }
    voxels
}

#[cfg(feature = "native-openvdb")]
pub(crate) fn take_error(error: *mut std::ffi::c_char) -> String {
    if error.is_null() {
        return "OpenVDB failed".into();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(error) }.to_string_lossy().into_owned();
    unsafe { mini_openvdb_free(error.cast()) };
    message
}

// A VDB node's native level set as volume points (the node works in app space, so no mirror).
#[cfg(feature = "native-openvdb")]
pub(crate) fn native_volume_voxels(volume: &NativeVolume) -> Option<Mesh> {
    let (mut coords, mut values, mut count, mut voxel, mut background) = (std::ptr::null_mut(), std::ptr::null_mut(), 0usize, 0.0f32, 0.0f32);
    if unsafe { mini_openvdb_volume_voxels(volume.0, &mut coords, &mut values, &mut count, &mut voxel, &mut background) } == 0 {
        return None;
    }
    let voxels = take_voxels(coords, values, count, false);
    Some(volume::encode_voxels(&voxels, volume::Class::LevelSet, voxel, Vec3::zero(), background))
}

// .vdb files are right-handed like other DCC apps: Z is mirrored into app space.
pub(crate) fn read_vdb_file(path: &str) -> Result<Mesh, String> {
    #[cfg(feature = "native-openvdb")]
    {
        if !std::path::Path::new(path).exists() {
            return Err(format!("file not found: {path}"));
        }
        let c_path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        let (mut coords, mut values, mut count) = (std::ptr::null_mut(), std::ptr::null_mut(), 0usize);
        let (mut voxel, mut origin, mut background, mut level_set, mut error) = (0.0f32, [0.0f32; 3], 0.0f32, 0i32, std::ptr::null_mut());
        let ok = unsafe {
            mini_openvdb_read(c_path.as_ptr(), &mut coords, &mut values, &mut count, &mut voxel, origin.as_mut_ptr(), &mut background, &mut level_set, &mut error)
        };
        if ok == 0 {
            return Err(take_error(error));
        }
        let voxels = take_voxels(coords, values, count, true);
        let class = if level_set != 0 { volume::Class::LevelSet } else { volume::Class::Fog };
        Ok(volume::encode_voxels(&voxels, class, voxel, Vec3::new(origin[0], origin[1], -origin[2]), background))
    }
    #[cfg(not(feature = "native-openvdb"))]
    {
        let _ = path;
        Err("reading .vdb files needs the native-openvdb build".into())
    }
}

#[cfg(feature = "native-openvdb")]
unsafe extern "C" {
    pub(crate) fn mini_openvdb_write_volume(
        path: *const std::ffi::c_char,
        coords: *const i32,
        density: *const f32,
        emission: *const f32,
        count: usize,
        voxel: f32,
        origin: *const f32,
        out_error: *mut *mut std::ffi::c_char,
    ) -> i32;
}

// Writes a fog volume (density, plus emission when it has an emission attribute) to a temporary
// .vdb in MoonRay's right-handed space, with the shading settings carried by the volume.
pub(crate) fn write_render_volume(mesh: &Mesh, index: usize) -> Result<RenderVolume, String> {
    let detail = |name: &str| mesh.point_attributes.get(&format!("detail:{name}")).and_then(|v| v.first().copied());
    let file = std::env::temp_dir()
        .join(format!("proceduralfx_volume_{}_{index}.vdb", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let (voxels, _, voxel, origin, _) = volume::decode_voxels(mesh)?;
    // Fire is sparse voxel by voxel; a 3×3×3 average gives smooth flames instead of sparkles.
    let emission: Option<Vec<f32>> = mesh
        .point_attributes
        .get("emission")
        .filter(|e| e.len() == mesh.vertices.len() * 3 && e.iter().any(|v| *v > 0.0))
        .map(|raw| {
            let index: HashMap<[i64; 3], usize> = voxels.iter().enumerate().map(|(i, (c, _))| (*c, i)).collect();
            voxels
                .iter()
                .flat_map(|(c, _)| {
                    let mut sum = [0.0f32; 3];
                    for dz in -1..=1 {
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                if let Some(&j) = index.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                                    (0..3).for_each(|k| sum[k] += raw[j * 3 + k]);
                                }
                            }
                        }
                    }
                    sum.map(|v| v / 27.0)
                })
                .collect()
        });
    let emission = emission.as_ref();
    let volume = RenderVolume {
        file: file.clone(),
        extinction: detail("volume_extinction").unwrap_or(1.0),
        albedo: [0.85; 3],
        emission: emission.map(|_| detail("volume_emission").unwrap_or(1.0)),
        emission_tint: [1.0; 3],
        anisotropy: 0.3,
    };
    #[cfg(feature = "native-openvdb")]
    {
        let coords: Vec<i32> = voxels.iter().flat_map(|(c, _)| [c[0] as i32, c[1] as i32, -c[2] as i32]).collect();
        let density: Vec<f32> = voxels.iter().map(|(_, v)| *v).collect();
        let usd_origin = [origin.x, origin.y, -origin.z];
        let c_path = std::ffi::CString::new(file.as_str()).map_err(|e| e.to_string())?;
        let mut error = std::ptr::null_mut();
        let ok = unsafe {
            mini_openvdb_write_volume(
                c_path.as_ptr(),
                coords.as_ptr(),
                density.as_ptr(),
                emission.map_or(std::ptr::null(), |e| e.as_ptr()),
                density.len(),
                voxel,
                usd_origin.as_ptr(),
                &mut error,
            )
        };
        if ok == 0 {
            return Err(format!("could not write the render volume: {}", take_error(error)));
        }
        Ok(volume)
    }
    #[cfg(not(feature = "native-openvdb"))]
    {
        let _ = (voxels, voxel, origin, volume);
        Err("rendering volumes needs the native-openvdb build".into())
    }
}

pub(crate) fn write_vdb_file(mesh: &Mesh, path: &str) -> Result<(), String> {
    let (voxels, class, voxel, origin, background) = volume::decode_voxels(mesh)?;
    #[cfg(feature = "native-openvdb")]
    {
        let coords: Vec<i32> = voxels.iter().flat_map(|(c, _)| [c[0] as i32, c[1] as i32, -c[2] as i32]).collect();
        let values: Vec<f32> = voxels.iter().map(|(_, v)| *v).collect();
        let usd_origin = [origin.x, origin.y, -origin.z];
        let c_path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
        let mut error = std::ptr::null_mut();
        let ok = unsafe {
            mini_openvdb_write(
                c_path.as_ptr(),
                coords.as_ptr(),
                values.as_ptr(),
                values.len(),
                voxel,
                usd_origin.as_ptr(),
                background,
                (class == volume::Class::LevelSet) as i32,
                &mut error,
            )
        };
        if ok == 0 {
            return Err(take_error(error));
        }
        Ok(())
    }
    #[cfg(not(feature = "native-openvdb"))]
    {
        let _ = (voxels, class, voxel, origin, background, path);
        Err("writing .vdb files needs the native-openvdb build".into())
    }
}

pub(crate) fn node_wrangle(mesh: Mesh, inputs: &[Mesh], code: &str, time: f32, domain: usize, mask: Option<&[bool]>, channels: Option<&vex::Channels>) -> Mesh {
    vex::run_wrangle(mesh, inputs, code, time, SOLVER_TIME_INC.with(|inc| inc.get()), domain, mask, channels)
}

// Renumbers the nodes a node refers to (its material, Object Merge sources and so on), for
// Graph::merge. These are the only kinds that hold node ids.
pub(crate) fn remap_node_ids(kind: &mut NodeKind, map: impl Fn(NodeId) -> NodeId) {
    match kind {
        NodeKind::Geometry { material: Some(id) }
        | NodeKind::CompileEnd { begin: Some(id) }
        | NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(id), .. })
        | NodeKind::LopSopImport { geometry: Some(id), .. }
        | NodeKind::LopMaterial { material: Some(id), .. } => *id = map(*id),
        NodeKind::ObjectMerge { sources, .. } => sources.iter_mut().flatten().for_each(|id| *id = map(*id)),
        NodeKind::MaterialAssign { slots } => slots.iter_mut().filter_map(|(_, id)| id.as_mut()).for_each(|id| *id = map(*id)),
        NodeKind::MoonRayRender { opts, .. } => {
            if let RenderCamera::Node(id) = &mut opts.camera {
                *id = map(*id);
            }
        }
        NodeKind::Subnet { spec, .. } => spec.remap(map),
        _ => {}
    }
}

// A window of its own beside the app's, free to be as big as you like or on another screen,
// showing `add`; closing it clears `open`. Where the platform can't open more windows, a
// panel inside the app's window instead. `id` must be unique and stay the same between frames.
// A `modal` window (a dialog) locks the app's window until it's closed; others (tools such as
// the flipbook) leave it free.
#[allow(clippy::too_many_arguments)]
pub(crate) fn own_window(ctx: &egui::Context, id: &str, title: &str, size: [f32; 2], min: [f32; 2], open: &mut bool, modal: bool, add: impl FnOnce(&mut egui::Ui)) {
    if !*open {
        return;
    }
    let viewport = egui::ViewportId::from_hash_of(id);
    let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size(size).with_min_inner_size(min);
    // Drawn once a frame; the window's code may call this more than once, so take it once.
    let mut add = Some(add);
    ctx.show_viewport_immediate(viewport, builder, |ctx, class| {
        let Some(add) = add.take() else { return };
        if class == egui::ViewportClass::Embedded {
            if modal {
                // Inside the app's window: egui's own modal, which blocks the rest of it.
                egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading(title);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✖").on_hover_text("Close").clicked() {
                                *open = false;
                            }
                        });
                    });
                    add(ui);
                });
            } else {
                egui::Window::new(title).id(egui::Id::new(id)).open(open).default_size(size).min_size(min).resizable(true).show(ctx, add);
            }
            return;
        }
        if modal {
            ctx.data_mut(|d| d.insert_temp(modal_key(), (viewport, title.to_owned())));
        }
        egui::CentralPanel::default().show(ctx, add);
        if ctx.input(|i| i.viewport().close_requested()) {
            *open = false;
        }
    });
}

// Where an open modal window (see own_window) notes itself each frame: its viewport and title.
pub(crate) fn modal_key() -> egui::Id {
    egui::Id::new("pfx_modal_window")
}

// While a modal window is open in its own window, the app's window ignores the keyboard (so
// shortcuts can't act behind the dialog); a click on it brings the dialog to the front.
// Called at the start of the app's frame, with what the last frame's windows noted.
pub(crate) fn lock_for_modal(ctx: &egui::Context) -> Option<(egui::ViewportId, String)> {
    let modal = ctx.data_mut(|d| d.remove_temp::<(egui::ViewportId, String)>(modal_key()))?;
    ctx.input_mut(|i| {
        i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut));
    });
    if ctx.input(|i| i.pointer.any_pressed()) {
        ctx.send_viewport_cmd_to(modal.0, egui::ViewportCommand::Focus);
    }
    Some(modal)
}

// Covers the app's window while a modal window is open, taking every click, and says why.
pub(crate) fn modal_cover(ctx: &egui::Context, title: &str) {
    let screen = ctx.viewport_rect();
    egui::Area::new(egui::Id::new("pfx_modal_cover")).order(egui::Order::Foreground).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let response = ui.allocate_rect(screen, egui::Sense::click_and_drag());
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(110));
        ui.painter().text(screen.center(), egui::Align2::CENTER_CENTER, format!("Finish or close {title} first"), egui::FontId::proportional(18.0), egui::Color32::from_gray(230));
        response.on_hover_cursor(egui::CursorIcon::NotAllowed);
    });
}

pub(crate) fn encode_text(text: &str) -> String {
    text.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn decode_text(text: &str) -> Option<String> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

pub(crate) fn node_noise_displace(mut mesh: Mesh, freq: f32, amp: f32, time: f32) -> Mesh {
    let normals = mesh.compute_normals();
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let noise = (v.x * freq + time).sin() * (v.z * freq - time * 0.5).cos()
            + 0.5 * ((v.x * freq * 2.1 - time).cos() * (v.z * freq * 2.1 + time).sin())
            + 0.35 * ((v.y * freq * 1.7 + time * 0.7).sin());
        *v = v.add(normals[i].scale(amp * noise));
    }
    mesh
}

pub(crate) fn node_twist(mut mesh: Mesh, angle_scale: f32) -> Mesh {
    for v in mesh.vertices.iter_mut() {
        let (s, c) = (v.y * angle_scale).sin_cos();
        let (nx, nz) = (v.x * c - v.z * s, v.x * s + v.z * c);
        v.x = nx;
        v.z = nz;
    }
    mesh
}

pub(crate) fn node_taper(mut mesh: Mesh, factor: f32) -> Mesh {
    for v in mesh.vertices.iter_mut() {
        let s = (1.0 + v.y * factor).max(0.01);
        v.x *= s;
        v.z *= s;
    }
    mesh
}

/// Bend along +Y around the Z axis. `angle` = radians of bend per unit height.
pub(crate) fn node_bend(mut mesh: Mesh, angle: f32) -> Mesh {
    if angle.abs() < 1e-5 {
        return mesh;
    }
    let r = 1.0 / angle;
    for v in mesh.vertices.iter_mut() {
        let theta = v.y * angle;
        let (s, c) = theta.sin_cos();
        let d = r - v.x;
        v.x = r - d * c;
        v.y = d * s;
    }
    mesh
}

/// Laplacian smoothing.
pub(crate) fn node_smooth(mut mesh: Mesh, iterations: usize, strength: f32) -> Mesh {
    let n = mesh.vertices.len();
    let mut nbrs: Vec<Vec<usize>> = vec![Vec::new(); n];
    for t in &mesh.indices {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            nbrs[a].push(b);
            nbrs[b].push(a);
        }
    }
    for l in nbrs.iter_mut() {
        l.sort_unstable();
        l.dedup();
    }
    for _ in 0..iterations {
        let old = mesh.vertices.clone();
        for (i, v) in mesh.vertices.iter_mut().enumerate() {
            if nbrs[i].is_empty() {
                continue;
            }
            let avg = nbrs[i]
                .iter()
                .fold(Vec3::zero(), |acc, &j| acc.add(old[j]))
                .scale(1.0 / nbrs[i].len() as f32);
            *v = old[i].add(avg.sub(old[i]).scale(strength));
        }
    }
    mesh
}

pub(crate) fn node_bevel(mut mesh: Mesh, amount: f32, segments: usize) -> Mesh {
    let amount = amount.clamp(0.0, 1.0);
    let segments = segments.min(8);
    if amount <= 0.0 || segments == 0 {
        return mesh;
    }

    let mut neighbors = vec![Vec::<usize>::new(); mesh.vertices.len()];
    for tri in &mesh.indices {
        for (a, b) in [
            (tri[0], tri[1]),
            (tri[1], tri[2]),
            (tri[2], tri[0]),
        ] {
            if !neighbors[a].contains(&b) {
                neighbors[a].push(b);
            }
            if !neighbors[b].contains(&a) {
                neighbors[b].push(a);
            }
        }
    }

    let step = amount / segments as f32;
    for _ in 0..segments {
        let old = mesh.vertices.clone();
        for (i, vertex) in mesh.vertices.iter_mut().enumerate() {
            if neighbors[i].is_empty() {
                continue;
            }
            let average = neighbors[i]
                .iter()
                .map(|&j| old[j])
                .fold(Vec3::zero(), Vec3::add)
                .scale(1.0 / neighbors[i].len() as f32);
            *vertex = old[i].scale(1.0 - step).add(average.scale(step));
        }
    }
    mesh
}

pub(crate) fn node_subdivide(input: Mesh, levels: usize) -> Mesh {
    let mut input = input;
    for _ in 0..levels.min(4) {
        input = node_subdivide_once(input);
    }
    input
}

// Linear subdivision: triangles split into four; polygons into one quad per
// corner around a new centre point (as Houdini does), so quads stay quads.
pub(crate) fn node_subdivide_once(input: Mesh) -> Mesh {
    // Shared edge midpoints so the result stays watertight
    let mut m = Mesh { vertices: input.vertices.clone(), indices: Vec::new(), point_attributes: input.point_attributes.clone() };
    m.point_attributes.remove(poly::KEY);
    let mut edge_mid: HashMap<(usize, usize), usize> = HashMap::new();
    let mut mid = |a: usize, b: usize, m: &mut Mesh| -> usize {
        let key = (a.min(b), a.max(b));
        *edge_mid.entry(key).or_insert_with(|| {
            m.vertices.push(input.vertices[a].add(input.vertices[b]).scale(0.5));
            m.vertices.len() - 1
        })
    };
    let mut sizes = Vec::new();
    // The input triangle each new triangle comes from, for primitive attributes.
    let mut source = Vec::new();
    for run in poly::runs(&input) {
        let outline = (run.len() > 1).then(|| poly::boundary(&input, &run)).flatten();
        match outline {
            Some(loop_points) => {
                let centre = m.vertices.len();
                m.vertices.push(poly::centroid(&input, &run));
                let n = loop_points.len();
                for i in 0..n {
                    let (prev, here, next) = (loop_points[(i + n - 1) % n], loop_points[i], loop_points[(i + 1) % n]);
                    let (a, b) = (mid(here, next, &mut m), mid(prev, here, &mut m));
                    m.indices.push([here, a, centre]);
                    m.indices.push([here, centre, b]);
                    sizes.push(2);
                    source.extend([run.start; 2]);
                }
            }
            None => {
                for (ti, t) in input.indices[run.clone()].iter().enumerate() {
                    source.extend([run.start + ti; 4]);
                    let a = mid(t[0], t[1], &mut m);
                    let b = mid(t[1], t[2], &mut m);
                    let c = mid(t[2], t[0], &mut m);
                    m.indices.push([t[0], a, c]);
                    m.indices.push([t[1], b, a]);
                    m.indices.push([t[2], c, b]);
                    m.indices.push([a, b, c]);
                    sizes.extend([1; 4]);
                }
            }
        }
    }
    let tris = input.indices.len();
    for (_, values) in m.point_attributes.iter_mut().filter(|(k, _)| k.starts_with("prim:")) {
        if tris > 0 && values.len() % tris == 0 {
            let w = values.len() / tris;
            *values = source.iter().flat_map(|&t| values[t * w..t * w + w].to_vec()).collect();
        }
    }
    // Corner attributes would need interpolating; they're dropped rather than left misaligned.
    m.point_attributes.retain(|k, _| !k.starts_with("vertex:"));
    if sizes.iter().any(|n| *n > 1) {
        poly::tag(&mut m, 0, sizes);
    }
    m
}

pub(crate) fn node_remesh(input: Mesh, target_triangles: usize) -> Mesh {
    let target = target_triangles.clamp(4, 200_000);
    let mut mesh = input;
    let mut levels = 0;
    while mesh.indices.len() < target && levels < 4 {
        mesh = node_subdivide_once(mesh);
        levels += 1;
    }
    mesh
}

// The input plus its mirror image (merged, so attributes and polygons come along).
pub(crate) fn node_mirror(mesh: Mesh, axis: usize) -> Mesh {
    let mut copy = mesh.clone();
    for p in &mut copy.vertices {
        match axis {
            0 => p.x = -p.x,
            1 => p.y = -p.y,
            _ => p.z = -p.z,
        }
    }
    for t in &mut copy.indices {
        t.swap(1, 2);
    }
    // Corner attributes follow the swapped corners.
    let tris = copy.indices.len();
    for (_, values) in copy.point_attributes.iter_mut().filter(|(k, _)| k.starts_with("vertex:")) {
        if tris == 0 || values.len() % (tris * 3) != 0 {
            continue;
        }
        let w = values.len() / (tris * 3);
        for t in 0..tris {
            for c in 0..w {
                values.swap((t * 3 + 1) * w + c, (t * 3 + 2) * w + c);
            }
        }
    }
    node_merge(mesh, copy)
}

pub(crate) fn node_copy(mesh: Mesh, count: usize, t: [f32; 3], r: [f32; 3], s: f32) -> Mesh {
    let mut out = Mesh::default();
    for i in 0..count.max(1) {
        let f = i as f32;
        let k = s.powi(i as i32);
        let copy = node_transform(
            mesh.clone(),
            [t[0] * f, t[1] * f, t[2] * f],
            [r[0] * f, r[1] * f, r[2] * f],
            [k, k, k],
        );
        out = node_merge(out, copy);
    }
    out
}

pub(crate) fn node_add(count: usize, spacing: f32, layout: usize) -> Mesh {
    let mut mesh = Mesh::default();
    let count = count.clamp(1, 100_000);
    let spacing = spacing.max(0.001);
    let columns = (count as f32).sqrt().ceil() as usize;
    for i in 0..count {
        let (x, y, z) = match layout.min(2) {
            1 => (0.0, (i as f32 - (count - 1) as f32 * 0.5) * spacing, 0.0),
            2 => {
                let x = (i % columns) as f32 - (columns.saturating_sub(1) as f32 * 0.5);
                let z = (i / columns) as f32 - (columns.saturating_sub(1) as f32 * 0.5);
                (x * spacing, 0.0, z * spacing)
            }
            _ => (i as f32 * spacing, 0.0, 0.0),
        };
        mesh.vertices.push(Vec3::new(x, y, z));
    }
    mesh
}

pub(crate) fn node_copy_to_points(piece: Mesh, points: Mesh, scale: f32, rotation: [f32; 3]) -> Mesh {
    copy_to_points_with(points, scale, rotation, |_| Some(&piece))
}

// Copy to Points with a piece attribute: each point gets the piece of the first input
// whose value of the attribute matches its own (pieces by a primitive attribute, or
// point attribute; points by a point attribute). Points matching no piece get nothing.
pub(crate) fn node_copy_pieces_to_points(source: Mesh, points: Mesh, scale: f32, rotation: [f32; 3], piece_attribute: &str) -> Result<Mesh, String> {
    let name = piece_attribute.trim();
    if name.is_empty() {
        return Ok(node_copy_to_points(source, points, scale, rotation));
    }
    let key = |v: f32| (v * 1000.0).round() as i64;
    let pieces: HashMap<i64, Mesh> = flow::named_pieces(&source, name)
        .map_err(|e| format!("first input: {e}"))?
        .into_iter()
        .map(|(value, piece)| (key(value), piece))
        .collect();
    let n = points.vertices.len();
    // A string attribute by that name (s@name) works too.
    let point_name = crate::strings::resolve(&points, name);
    let chosen: Vec<f32> = match points.point_attributes.get(&point_name).filter(|v| n > 0 && v.len() % n == 0) {
        Some(values) => values.chunks(values.len() / n).map(|c| c[0]).collect(),
        None if n == 0 => Vec::new(),
        None => return Err(format!("the points have no `{name}` attribute to choose their piece; give them one (Attribute Randomize, integer, 0 to the number of pieces − 1)")),
    };
    Ok(copy_to_points_with(points, scale, rotation, |i| pieces.get(&key(chosen[i]))))
}

pub(crate) fn copy_to_points_with<'a>(points: Mesh, scale: f32, rotation: [f32; 3], piece_for: impl Fn(usize) -> Option<&'a Mesh>) -> Mesh {
    let mut out = Mesh::default();
    let scales = points.point_attributes.get("pscale");
    let orientations = points.point_attributes.get("orient");
    let colors = points.point_attributes.get("Cd");
    let point_count = points.vertices.len();
    // orient: a quaternion per point (as in Houdini), or one number, a turn about Y in degrees.
    let quaternions = orientations.filter(|o| point_count > 0 && o.len() == point_count * 4);
    for (index, point) in points.vertices.into_iter().enumerate() {
        let Some(piece) = piece_for(index) else {
            continue;
        };
        let point_scale = scales.and_then(|v| v.get(index)).copied().unwrap_or(1.0);
        let point_rotation = if quaternions.is_some() { 0.0 } else { orientations.and_then(|v| v.get(index)).copied().unwrap_or(0.0) };
        let mut copy = node_transform(
            piece.clone(),
            [0.0; 3],
            [rotation[0], rotation[1] + point_rotation, rotation[2]],
            [scale * point_scale; 3],
        );
        if let Some(q) = quaternions {
            let m = vexmath::q_to_m3(&[q[index * 4], q[index * 4 + 1], q[index * 4 + 2], q[index * 4 + 3]]);
            let rows = [Vec3::new(m[0], m[1], m[2]), Vec3::new(m[3], m[4], m[5]), Vec3::new(m[6], m[7], m[8])];
            copy.vertices.iter_mut().for_each(|v| *v = kinefx::vm(*v, &rows));
            let (pts, tris) = (copy.vertices.len(), copy.indices.len());
            for (key, count) in [("N", pts), ("vertex:N", tris * 3)] {
                if let Some(normals) = copy.point_attributes.get_mut(key).filter(|n| count > 0 && n.len() == count * 3) {
                    for c in normals.chunks_exact_mut(3) {
                        let n = kinefx::vm(Vec3::new(c[0], c[1], c[2]), &rows);
                        c.copy_from_slice(&[n.x, n.y, n.z]);
                    }
                }
            }
        }
        copy.vertices.iter_mut().for_each(|v| *v = v.add(point));
        if let Some(values) = colors {
            let color = if values.len() >= point_count.saturating_mul(3) {
                [
                    values.get(index * 3).copied().unwrap_or(0.0),
                    values.get(index * 3 + 1).copied().unwrap_or(0.0),
                    values.get(index * 3 + 2).copied().unwrap_or(0.0),
                ]
            } else {
                let value = values.get(index).copied().unwrap_or(0.0);
                attribute_color(value)
            };
            copy.point_attributes.insert(
                "Cd".into(),
                color.into_iter().cycle().take(copy.vertices.len() * 3).collect(),
            );
        }
        out = node_merge(
            out,
            copy,
        );
    }
    out
}

pub(crate) fn hash01(i: u32, seed: u32) -> f32 {
    let mut h = i.wrapping_mul(0x9E37_79B1) ^ seed.wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

pub(crate) fn node_tube(r_bot: f32, r_top: f32, height: f32, rows: usize, cols: usize, caps: bool) -> Mesh {
    let (rows, cols) = (rows.max(1), cols.max(3));
    let mut m = Mesh::default();
    for j in 0..=rows {
        let f = j as f32 / rows as f32;
        let y = -height / 2.0 + height * f;
        let r = r_bot + (r_top - r_bot) * f;
        for c in 0..cols {
            let p = std::f32::consts::TAU * c as f32 / cols as f32;
            m.vertices.push(Vec3::new(r * p.cos(), y, r * p.sin()));
        }
    }
    for j in 0..rows {
        for c in 0..cols {
            let a = j * cols + c;
            let b = j * cols + (c + 1) % cols;
            let (cc, d) = (a + cols, b + cols);
            m.indices.push([a, cc, b]);
            m.indices.push([b, cc, d]);
        }
    }
    poly::tag_quads(&mut m, 0, rows * cols);
    if caps {
        // Each cap is one polygon (a fan around its centre point).
        let bot = m.vertices.len();
        m.vertices.push(Vec3::new(0.0, -height / 2.0, 0.0));
        let top = m.vertices.len();
        m.vertices.push(Vec3::new(0.0, height / 2.0, 0.0));
        let tr = rows * cols;
        let first = m.indices.len();
        for c in 0..cols {
            m.indices.push([bot, c, (c + 1) % cols]);
        }
        for c in 0..cols {
            let n = (c + 1) % cols;
            m.indices.push([top, tr + n, tr + c]);
        }
        poly::tag(&mut m, first, [cols, cols]);
    }
    m
}

/// Push points along their normals.
pub(crate) fn node_peak(mut mesh: Mesh, dist: f32) -> Mesh {
    let n = mesh.compute_normals();
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        *v = v.add(n[i].scale(dist));
    }
    mesh
}

pub(crate) fn node_reverse(mut mesh: Mesh) -> Mesh {
    for t in mesh.indices.iter_mut() {
        t.swap(1, 2);
    }
    mesh
}

/// Remove points no triangle uses.
// The triangles `keep` accepts, with their primitive and vertex attributes, and
// only the points they use.
pub(crate) fn keep_triangles(mesh: Mesh, keep: impl Fn(usize, &[usize; 3]) -> bool) -> Mesh {
    let mask: Vec<bool> = mesh.indices.iter().enumerate().map(|(i, t)| keep(i, t)).collect();
    compact(groups::only_prims(&mesh, &mask))
}

// Drops unused points (renumbering point attributes); triangles keep their order.
pub(crate) fn compact(mesh: Mesh) -> Mesh {
    let mut remap = vec![usize::MAX; mesh.vertices.len()];
    let mut out = Mesh::default();
    for t in &mesh.indices {
        let mut nt = [0usize; 3];
        for k in 0..3 {
            let i = t[k];
            if remap[i] == usize::MAX {
                remap[i] = out.vertices.len();
                out.vertices.push(mesh.vertices[i]);
            }
            nt[k] = remap[i];
        }
        out.indices.push(nt);
    }
    for (name, values) in mesh.point_attributes {
        // Primitive, vertex and detail attributes follow the triangles, which stay as they are.
        if name.starts_with("prim:") || name.starts_with("vertex:") || name.starts_with("detail:") {
            out.point_attributes.insert(name, values);
            continue;
        }
        let components = if mesh.vertices.is_empty() {
            1
        } else {
            (values.len() / mesh.vertices.len()).max(1)
        };
        let mut compacted = vec![0.0; out.vertices.len() * components];
        for (old, &new) in remap.iter().enumerate() {
            if new != usize::MAX {
                let old_start = old * components;
                let new_start = new * components;
                for component in 0..components {
                    compacted[new_start + component] =
                        values.get(old_start + component).copied().unwrap_or(0.0);
                }
            }
        }
        out.point_attributes.insert(name, compacted);
    }
    out
}

pub(crate) fn axis_of(v: Vec3, axis: usize) -> f32 {
    match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

// Number of elements an attribute key holds: points, primitives (prim:),
// vertices (vertex:, three per triangle) or one for the detail (detail:).
pub(crate) fn attribute_element_count(key: &str, points: usize, prims: usize) -> usize {
    if key.starts_with("prim:") {
        prims
    } else if key.starts_with("vertex:") {
        prims * 3
    } else if key.starts_with("detail:") {
        1
    } else {
        points
    }
}

// What an element without the attribute reads (Houdini's defaults for the common ones).
pub(crate) fn attribute_default(key: &str) -> f32 {
    let name = key.rsplit(':').next().unwrap_or(key);
    if matches!(name, "Cd" | "pscale" | "Alpha" | "width") { 1.0 } else { 0.0 }
}

// Normals one side has and the other lacks: the other gets real ones, not zeros
// (zero normals shade black).
pub(crate) fn fill_missing_normals(a: &Mesh, b: &mut Mesh) {
    let tris = b.indices.len();
    if a.point_attributes.contains_key("N") && !b.point_attributes.contains_key("N") && !b.vertices.is_empty() {
        let n = b.compute_normals();
        b.point_attributes.insert("N".into(), n.iter().flat_map(|v| [v.x, v.y, v.z]).collect());
    }
    if a.point_attributes.contains_key("vertex:N") && !b.point_attributes.contains_key("vertex:N") && tris > 0 {
        let corners = sops::corner_normals(b);
        b.point_attributes.insert("vertex:N".into(), corners.iter().flatten().flat_map(|v| [v.x, v.y, v.z]).collect());
    }
}

pub(crate) fn node_merge(mut a: Mesh, mut b: Mesh) -> Mesh {
    poly::offset_for_merge(&a, &mut b);
    fill_missing_normals(&a, &mut b);
    fill_missing_normals(&b, &mut a);
    let (a_points, a_prims) = (a.vertices.len(), a.indices.len());
    let (b_points, b_prims) = (b.vertices.len(), b.indices.len());
    a.vertices.extend(b.vertices);
    a.indices
        .extend(b.indices.into_iter().map(|t| [t[0] + a_points, t[1] + a_points, t[2] + a_points]));
    let a_attributes = std::mem::take(&mut a.point_attributes);
    let b_attributes = b.point_attributes;
    let names: HashSet<String> = a_attributes
        .keys()
        .chain(b_attributes.keys())
        .cloned()
        .collect();
    let mut attributes = HashMap::new();
    for name in names {
        let a_values = a_attributes.get(&name);
        let b_values = b_attributes.get(&name);
        if name.starts_with("detail:") {
            // One detail per mesh: the first input's value wins.
            if let Some(values) = a_values.or(b_values) {
                attributes.insert(name, values.clone());
            }
            continue;
        }
        let a_count = attribute_element_count(&name, a_points, a_prims);
        let b_count = attribute_element_count(&name, b_points, b_prims);
        let components_of = |values: Option<&Vec<f32>>, count: usize| {
            values
                .filter(|values| count > 0 && values.len() % count == 0)
                .map_or(1, |values| values.len() / count)
                .max(1)
        };
        let a_components = components_of(a_values, a_count);
        let b_components = components_of(b_values, b_count);
        let components = a_components.max(b_components);
        // Elements missing the attribute get its natural default (as in Houdini: white for
        // Cd, 1 for pscale and Alpha, else 0); one-component values fill every component.
        let default = attribute_default(&name);
        let mut merged = vec![default; (a_count + b_count) * components];
        let mut copy = |values: Option<&Vec<f32>>, count: usize, from: usize, offset: usize| {
            let Some(values) = values else { return };
            for index in 0..count {
                for component in 0..components {
                    merged[(offset + index) * components + component] =
                        values.get(index * from + component.min(from - 1)).copied().unwrap_or(0.0);
                }
            }
        };
        copy(a_values, a_count, a_components, 0);
        copy(b_values, b_count, b_components, a_count);
        attributes.insert(name, merged);
    }
    a.point_attributes = attributes;
    a
}

pub(crate) fn node_particle_surface(
    input: Mesh,
    radius: f32,
    join_distance: f32,
    resolution: usize,
    smoothing: usize,
) -> Mesh {
    fluidsurface::surface(&input, &fluidsurface::SurfaceSettings { radius, join_distance, resolution, smoothing })
}

pub(crate) fn cache_file_path(cache_id: usize, time: f32) -> std::path::PathBuf {
    tops::cache_path(cache_id, time)
}

pub(crate) fn load_mesh_cache(path: &std::path::Path) -> Option<Mesh> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let vertices = lines.next()?.parse::<usize>().ok()?;
    let indices = lines.next()?.parse::<usize>().ok()?;
    let mut mesh = Mesh {
        vertices: Vec::with_capacity(vertices),
        indices: Vec::with_capacity(indices),
        point_attributes: HashMap::new(),
    };
    for _ in 0..vertices {
        let values: Vec<f32> = lines.next()?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        if values.len() != 3 {
            return None;
        }
        mesh.vertices.push(Vec3::new(values[0], values[1], values[2]));
    }
    for _ in 0..indices {
        let values: Vec<usize> = lines.next()?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        if values.len() != 3 {
            return None;
        }
        mesh.indices.push([values[0], values[1], values[2]]);
    }
    for line in lines {
        let mut values = line.split_whitespace();
        let Some(first) = values.next() else { continue };
        // String attributes are saved as their text, each value x<hex> (strings.rs).
        if let Some(name) = first.strip_prefix("@sattr:") {
            let ids: Vec<f32> = values
                .map(|v| {
                    let hex = v.strip_prefix('x').unwrap_or("");
                    let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
                    crate::strings::id(&String::from_utf8_lossy(&bytes))
                })
                .collect();
            mesh.point_attributes.insert(name.to_string(), ids);
            continue;
        }
        let Some(name) = first.strip_prefix("@attr:") else {
            continue;
        };
        let attribute: Vec<f32> = values.filter_map(|value| value.parse().ok()).collect();
        mesh.point_attributes.insert(name.to_string(), attribute);
    }
    Some(mesh)
}

pub(crate) fn save_mesh_cache(path: &std::path::Path, mesh: &Mesh) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = format!("{}\n{}\n", mesh.vertices.len(), mesh.indices.len());
    for vertex in &mesh.vertices {
        text.push_str(&format!("{:.9} {:.9} {:.9}\n", vertex.x, vertex.y, vertex.z));
    }
    for tri in &mesh.indices {
        text.push_str(&format!("{} {} {}\n", tri[0], tri[1], tri[2]));
    }
    // Every attribute: point ones, and the primitive, vertex and detail ones (polygons,
    // volume and terrain settings) that have other lengths.
    for (name, values) in &mesh.point_attributes {
        if name.contains(char::is_whitespace) {
            continue;
        }
        if crate::strings::is_string_key(name) {
            text.push_str(&format!("@sattr:{name}"));
            for value in values {
                let hex: String = crate::strings::text(*value).bytes().map(|b| format!("{b:02x}")).collect();
                text.push_str(&format!(" x{hex}"));
            }
            text.push('\n');
            continue;
        }
        text.push_str(&format!("@attr:{name}"));
        for value in values {
            text.push_str(&format!(" {value:.9}"));
        }
        text.push('\n');
    }
    std::fs::write(path, text)
}

pub(crate) fn node_ripple(mut mesh: Mesh, freq: f32, amp: f32, speed: f32, time: f32) -> Mesh {
    let normals = mesh.compute_normals();
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let dist = (v.x * v.x + v.z * v.z).sqrt();
        let wave = (dist * freq - time * speed).sin();
        *v = v.add(normals[i].scale(amp * wave));
    }
    mesh
}

pub(crate) fn node_jitter(mut mesh: Mesh, amp: f32, seed: u32, scale: [f32; 3]) -> Mesh {
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let dx = (hash01(i as u32 * 3, seed) - 0.5) * 2.0 * amp * scale[0];
        let dy = (hash01(i as u32 * 3 + 1, seed) - 0.5) * 2.0 * amp * scale[1];
        let dz = (hash01(i as u32 * 3 + 2, seed) - 0.5) * 2.0 * amp * scale[2];
        *v = v.add(Vec3::new(dx, dy, dz));
    }
    mesh
}

#[derive(Clone)]
pub(crate) struct FlipParticle {
    pub(crate) position: Vec3,
    pub(crate) velocity: Vec3,
}

pub(crate) fn segment_mesh_hit(start: Vec3, end: Vec3, mesh: &Mesh) -> Option<(Vec3, Vec3, f32)> {
    let direction = end.sub(start);
    let mut best: Option<(Vec3, Vec3, f32)> = None;
    for tri in &mesh.indices {
        let a = mesh.vertices[tri[0]];
        let b = mesh.vertices[tri[1]];
        let c = mesh.vertices[tri[2]];
        let edge1 = b.sub(a);
        let edge2 = c.sub(a);
        let normal = edge1.cross(edge2);
        let pvec = direction.cross(edge2);
        let determinant = edge1.dot(pvec);
        if determinant.abs() < 1e-8 {
            continue;
        }
        let inverse = 1.0 / determinant;
        let tvec = start.sub(a);
        let u = tvec.dot(pvec) * inverse;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let qvec = tvec.cross(edge1);
        let v = direction.dot(qvec) * inverse;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = edge2.dot(qvec) * inverse;
        if !(0.0..=1.0).contains(&t) {
            continue;
        }
        if best.as_ref().is_none_or(|(_, _, best_t)| t < *best_t) {
            best = Some((start.add(direction.scale(t)), normal.normalize(), t));
        }
    }
    best
}

// FLIP fluid: a stepped simulation like the other solvers, so playing forward
// only computes new frames (and the Whitewater Solver can follow it frame by
// frame). Each frame takes enough substeps to keep the step under `max_dt`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn node_flip(
    key: u64,
    emitter: Mesh,
    collider: impl FnOnce() -> Mesh,
    particle_count: usize,
    grid_resolution: usize,
    flip_ratio: f32,
    viscosity: f32,
    gravity: f32,
    max_dt: f32,
    substeps: usize,
    seed: usize,
    // Domain box: centre xyz, size xyz. A zero size uses the emitter's bounds.
    domain: [f32; 6],
    time: f32,
) -> Mesh {
    let max_particles = particle_count.clamp(1, 100_000);
    let resolution = grid_resolution.clamp(16, 128);
    let Some((seed_lo, seed_hi)) = emitter.bounds() else {
        return Mesh::default();
    };
    let seed_extent = seed_hi.sub(seed_lo);
    let boxed = domain[3] > 0.0 && domain[4] > 0.0 && domain[5] > 0.0;
    let (lo, extent) = if boxed {
        let size = Vec3::new(domain[3], domain[4], domain[5]);
        (Vec3::new(domain[0], domain[1], domain[2]).sub(size.scale(0.5)), size)
    } else {
        (seed_lo, seed_extent)
    };
    let max_extent = extent.x.max(extent.y).max(extent.z).max(0.1);
    let cell_size = max_extent / resolution as f32;
    // Particles per cell at rest: the emitter's volume (a closed mesh's, or
    // its bounds') shared out among the particles. Cells packed above this
    // get pushed apart, which keeps FLIP from slowly squashing itself flat.
    let emitter_volume = {
        let enclosed = emitter.indices.iter().map(|t| {
            let [a, b, c] = t.map(|i| emitter.vertices[i]);
            a.dot(b.cross(c)) / 6.0
        }).sum::<f32>().abs();
        if enclosed > 1e-6 { enclosed } else { (seed_extent.x * seed_extent.y * seed_extent.z).max(1e-6) }
    };
    let rest_density = (max_particles as f32 * cell_size.powi(3) / emitter_volume).max(0.5);
    let init = || {
    let mut particles = Vec::with_capacity(max_particles);
    let point_inside = |point: Vec3| {
        let mut crossings = 0usize;
        for tri in &emitter.indices {
            let a = emitter.vertices[tri[0]];
            let b = emitter.vertices[tri[1]];
            let c = emitter.vertices[tri[2]];
            let edge1 = b.sub(a);
            let edge2 = c.sub(a);
            let h = Vec3::new(0.0, -edge2.z, edge2.y);
            let det = edge1.x * h.x + edge1.y * h.y + edge1.z * h.z;
            if det.abs() < 1e-7 {
                continue;
            }
            let inv_det = 1.0 / det;
            let s = point.sub(a);
            let u = inv_det * (s.x * h.x + s.y * h.y + s.z * h.z);
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = s.cross(edge1);
            let v = inv_det * q.x;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let t = inv_det * (edge2.x * q.x + edge2.y * q.y + edge2.z * q.z);
            if t > 1e-6 {
                crossings += 1;
            }
        }
        crossings % 2 == 1
    };
    for i in 0..max_particles {
        let mut a = hash01(i as u32 * 31 + 1, seed as u32);
        let mut b = hash01(i as u32 * 31 + 2, seed as u32);
        let mut c = hash01(i as u32 * 31 + 3, seed as u32);
        if !emitter.indices.is_empty() {
            for attempt in 0..32 {
                let candidate = Vec3::new(
                    seed_lo.x + seed_extent.x * a,
                    seed_lo.y + seed_extent.y * b,
                    seed_lo.z + seed_extent.z * c,
                );
                if point_inside(candidate) {
                    break;
                }
                let key = i as u32 * 37 + attempt + 1;
                a = hash01(key * 31 + 1, seed as u32);
                b = hash01(key * 31 + 2, seed as u32);
                c = hash01(key * 31 + 3, seed as u32);
            }
        }
        particles.push(FlipParticle {
            position: Vec3::new(seed_lo.x + seed_extent.x * a, seed_lo.y + seed_extent.y * b, seed_lo.z + seed_extent.z * c),
            velocity: Vec3::zero(),
        });
    }

    particles
    };
    let frame = 1.0 / sims::SIM_FPS;
    let per_frame = ((frame / max_dt.max(0.001)).ceil() as usize).saturating_mul(substeps.clamp(1, 8)).clamp(1, 64);
    let step_dt = frame / per_frame as f32;
    let mut collider = Some(collider);
    let mut collider_mesh: Option<(Mesh, Option<(Vec3, Vec3)>)> = None;
    let blend = flip_ratio.clamp(0.0, 1.0);
    let grid_len = resolution * resolution * resolution;
    let grid_index = |x: usize, y: usize, z: usize| (z * resolution + y) * resolution + x;
    let step = |particles: &mut Vec<FlipParticle>, _at: usize| {
        let (collider, collider_bounds) = collider_mesh.get_or_insert_with(|| {
            let mesh = collider.take().map(|c| c()).unwrap_or_default();
            let bounds = mesh.bounds();
            (mesh, bounds)
        });
        let (collider, collider_bounds) = (&*collider, *collider_bounds);
        // Velocities live on the cell faces (a MAC grid): component `c` on the
        // walls across axis `c`, so its grid is one longer along that axis
        // (and "component" 3 is the cell centres).
        let dims = |c: usize| [resolution + (c == 0) as usize, resolution + (c == 1) as usize, resolution + (c == 2) as usize];
        // The 8 faces around a point for component `c` and their trilinear
        // weights, so velocity varies smoothly inside a cell instead of every
        // particle in it moving as one block.
        let stencil = |position: Vec3, c: usize| -> [(usize, f32); 8] {
            let d = dims(c);
            let g = [(position.x - lo.x) / cell_size, (position.y - lo.y) / cell_size, (position.z - lo.z) / cell_size];
            let mut base = [0usize; 3];
            let mut t = [0.0f32; 3];
            for k in 0..3 {
                let f = (g[k] - if k == c { 0.0 } else { 0.5 }).clamp(0.0, (d[k] - 1) as f32);
                base[k] = (f.floor() as usize).min(d[k] - 2);
                t[k] = f - base[k] as f32;
            }
            let mut out = [(0, 0.0); 8];
            for (n, slot) in out.iter_mut().enumerate() {
                let o = [n & 1, (n >> 1) & 1, (n >> 2) & 1];
                let weight = (0..3).map(|k| if o[k] == 1 { t[k] } else { 1.0 - t[k] }).product();
                *slot = (((base[2] + o[2]) * d[1] + base[1] + o[1]) * d[0] + base[0] + o[0], weight);
            }
            out
        };
        let face_count = |c: usize| dims(c).iter().product::<usize>();
        let mut vel: [Vec<f32>; 3] = [0, 1, 2].map(|c| vec![0.0; face_count(c)]);
        let mut weights: [Vec<f32>; 3] = [0, 1, 2].map(|c| vec![0.0; face_count(c)]);
        let mut fluid = vec![false; grid_len];
        let cell_of = |p: Vec3| {
            let at = |v: f32, l: f32| ((v - l) / cell_size).floor().clamp(0.0, (resolution - 1) as f32) as usize;
            grid_index(at(p.x, lo.x), at(p.y, lo.y), at(p.z, lo.z))
        };
        let mut density = vec![0.0f32; grid_len];
        for particle in particles.iter() {
            fluid[cell_of(particle.position)] = true;
            // Component 3 isn't a velocity: it's the cell centres.
            for (i, weight) in stencil(particle.position, 3) {
                density[i] += weight;
            }
            let components = [particle.velocity.x, particle.velocity.y, particle.velocity.z];
            for c in 0..3 {
                for (i, weight) in stencil(particle.position, c) {
                    vel[c][i] += weight * components[c];
                    weights[c][i] += weight;
                }
            }
        }
        // Average, then carry velocities a couple of faces out past the
        // particles, so surface particles don't read empty (zero) faces.
        for c in 0..3 {
            let d = dims(c);
            let mut valid: Vec<bool> = weights[c].iter().map(|&w| w > 1e-6).collect();
            for (value, &weight) in vel[c].iter_mut().zip(&weights[c]) {
                if weight > 1e-6 {
                    *value /= weight;
                }
            }
            for _ in 0..2 {
                let (field, known) = (vel[c].clone(), valid.clone());
                for z in 0..d[2] {
                    for y in 0..d[1] {
                        for x in 0..d[0] {
                            let i = (z * d[1] + y) * d[0] + x;
                            if known[i] {
                                continue;
                            }
                            let (mut sum, mut count) = (0.0, 0.0);
                            let mut take = |j: usize| if known[j] { sum += field[j]; count += 1.0; };
                            if x > 0 { take(i - 1); }
                            if x + 1 < d[0] { take(i + 1); }
                            if y > 0 { take(i - d[0]); }
                            if y + 1 < d[1] { take(i + d[0]); }
                            if z > 0 { take(i - d[0] * d[1]); }
                            if z + 1 < d[2] { take(i + d[0] * d[1]); }
                            if count > 0.0 {
                                vel[c][i] = sum / count;
                                valid[i] = true;
                            }
                        }
                    }
                }
            }
        }
        let old = vel.clone();
        for value in &mut vel[1] { *value += gravity * step_dt; }
        // Outside the grid is solid below the floor and, in a domain box, on
        // every side; otherwise it's open air.
        let solid_outside = |axis: usize, below: bool| boxed || (axis == 1 && below);
        // No flow through solid walls.
        for c in 0..3 {
            let d = dims(c);
            for z in 0..d[2] {
                for y in 0..d[1] {
                    for x in 0..d[0] {
                        let along = [x, y, z][c];
                        if (along == 0 && solid_outside(c, true)) || (along == resolution && solid_outside(c, false)) {
                            vel[c][(z * d[1] + y) * d[0] + x] = 0.0;
                        }
                    }
                }
            }
        }
        // Pressure: solve for the pressure that makes the flow divergence-free
        // (preconditioned conjugate gradient over the fluid cells; air is zero
        // pressure, solid walls block flow).
        let fluid_cells: Vec<usize> = (0..grid_len).filter(|&i| fluid[i]).collect();
        let mut slot = vec![u32::MAX; grid_len];
        for (k, &i) in fluid_cells.iter().enumerate() {
            slot[i] = k as u32;
        }
        let n = fluid_cells.len();
        let mut diagonal = vec![0.0f32; n];
        let mut neighbours = vec![[u32::MAX; 6]; n];
        let mut rhs = vec![0.0f32; n];
        let face = |c: usize, x: usize, y: usize, z: usize| {
            let d = dims(c);
            (z * d[1] + y) * d[0] + x
        };
        for (k, &i) in fluid_cells.iter().enumerate() {
            let (x, y, z) = (i % resolution, (i / resolution) % resolution, i / (resolution * resolution));
            let at = [x, y, z];
            for axis in 0..3 {
                for (side, below) in [(0, true), (1, false)] {
                    let inside = if below { at[axis] > 0 } else { at[axis] + 1 < resolution };
                    if !inside {
                        if !solid_outside(axis, below) {
                            diagonal[k] += 1.0;
                        }
                        continue;
                    }
                    diagonal[k] += 1.0;
                    let mut other = at;
                    if below { other[axis] -= 1 } else { other[axis] += 1 }
                    let j = slot[grid_index(other[0], other[1], other[2])];
                    if j != u32::MAX {
                        neighbours[k][axis * 2 + side] = j;
                    }
                }
            }
            let divergence = vel[0][face(0, x + 1, y, z)] - vel[0][face(0, x, y, z)]
                + vel[1][face(1, x, y + 1, z)] - vel[1][face(1, x, y, z)]
                + vel[2][face(2, x, y, z + 1)] - vel[2][face(2, x, y, z)];
            // Overfull cells ask for a little outflow, spread over a few steps.
            let excess = (density[i] / rest_density - 1.0).clamp(0.0, 1.0);
            let spread = 0.3 * excess * cell_size / step_dt.max(1e-4);
            rhs[k] = -(divergence - spread) * cell_size / step_dt.max(1e-4);
        }
        let apply = |p: &[f32], out: &mut [f32]| {
            for k in 0..n {
                let mut value = diagonal[k] * p[k];
                for &j in &neighbours[k] {
                    if j != u32::MAX {
                        value -= p[j as usize];
                    }
                }
                out[k] = value;
            }
        };
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum::<f64>();
        let mut solution = vec![0.0f32; n];
        let mut residual = rhs.clone();
        let precondition = |r: &[f32]| r.iter().zip(&diagonal).map(|(r, d)| r / d.max(1.0)).collect::<Vec<f32>>();
        let mut search = precondition(&residual);
        let mut rz = dot(&residual, &search);
        let tolerance = rhs.iter().fold(0.0f32, |m, v| m.max(v.abs())) * 1e-4;
        let mut product = vec![0.0f32; n];
        for _ in 0..(resolution * 4).max(100) {
            if residual.iter().fold(0.0f32, |m, v| m.max(v.abs())) <= tolerance {
                break;
            }
            apply(&search, &mut product);
            let curvature = dot(&search, &product);
            if curvature.abs() < 1e-20 {
                break;
            }
            let alpha = (rz / curvature) as f32;
            for k in 0..n {
                solution[k] += alpha * search[k];
                residual[k] -= alpha * product[k];
            }
            let z = precondition(&residual);
            let rz_next = dot(&residual, &z);
            let beta = (rz_next / rz.max(1e-30)) as f32;
            rz = rz_next;
            for k in 0..n {
                search[k] = z[k] + beta * search[k];
            }
        }
        let mut pressure = vec![0.0f32; grid_len];
        for (k, &i) in fluid_cells.iter().enumerate() {
            pressure[i] = solution[k];
        }
        let pressure_gradient = step_dt / cell_size.max(1e-4);
        for c in 0..3 {
            let d = dims(c);
            for z in 0..d[2] {
                for y in 0..d[1] {
                    for x in 0..d[0] {
                        let at = [x, y, z];
                        let along = at[c];
                        // The cells either side of this face; outside the grid
                        // is either solid (the face stays put) or air.
                        let lower = (along > 0).then(|| {
                            let mut p = at;
                            p[c] -= 1;
                            grid_index(p[0], p[1], p[2])
                        });
                        let upper = (along < resolution).then(|| grid_index(x, y, z));
                        if (lower.is_none() && solid_outside(c, true)) || (upper.is_none() && solid_outside(c, false)) {
                            continue;
                        }
                        let is_fluid = |cell: Option<usize>| cell.is_some_and(|i| fluid[i]);
                        if !is_fluid(lower) && !is_fluid(upper) {
                            continue;
                        }
                        let p = |cell: Option<usize>| cell.map_or(0.0, |i| pressure[i]);
                        vel[c][(z * d[1] + y) * d[0] + x] -= (p(upper) - p(lower)) * pressure_gradient;
                    }
                }
            }
        }
        let sample = |field: &[Vec<f32>; 3], position: Vec3| {
            let mut out = [0.0f32; 3];
            for (c, value) in out.iter_mut().enumerate() {
                *value = stencil(position, c).iter().map(|&(i, weight)| field[c][i] * weight).sum();
            }
            Vec3::new(out[0], out[1], out[2])
        };
        for particle in particles.iter_mut() {
            let previous_position = particle.position;
            let grid_velocity_sample = sample(&vel, particle.position);
            let previous_velocity = sample(&old, particle.position);
            let flip_velocity = particle.velocity.add(grid_velocity_sample.sub(previous_velocity));
            particle.velocity = grid_velocity_sample
                .scale(1.0 - blend)
                .add(flip_velocity.scale(blend));
            // A safety limit only: a few cells a step, but never so low that
            // fine grids hold falling water back.
            let max_speed = (3.0 * cell_size / step_dt.max(1e-4)).max(15.0);
            let speed = particle.velocity.length();
            if speed > max_speed {
                particle.velocity = particle.velocity.scale(max_speed / speed);
            }
            particle.velocity = particle.velocity.scale((1.0 - viscosity * step_dt).clamp(0.0, 1.0));
            particle.position = particle.position.add(particle.velocity.scale(step_dt));
            if particle.position.y < lo.y {
                particle.position.y = lo.y;
                particle.velocity.y = particle.velocity.y.abs() * 0.15;
            }
            // A domain box is closed on every side.
            if boxed {
                let hi = lo.add(extent);
                for (p, v, a, b) in [
                    (&mut particle.position.x, &mut particle.velocity.x, lo.x, hi.x),
                    (&mut particle.position.y, &mut particle.velocity.y, lo.y, hi.y),
                    (&mut particle.position.z, &mut particle.velocity.z, lo.z, hi.z),
                ] {
                    if *p < a {
                        *p = a;
                        *v = v.abs() * 0.15;
                    } else if *p > b {
                        *p = b;
                        *v = -v.abs() * 0.15;
                    }
                }
            }
            if !collider.indices.is_empty() {
                // Collide with the actual swept mesh faces. Do not use the
                // collider AABB or a closed-volume test here: an open-top
                // container must let particles leave through its missing top.
                if let Some((surface, face_normal, _)) =
                    segment_mesh_hit(previous_position, particle.position, collider)
                {
                    let epsilon = cell_size * 0.08;
                    let outward = if particle.velocity.dot(face_normal) >= 0.0 {
                        face_normal
                    } else {
                        face_normal.scale(-1.0)
                    };
                    particle.position = surface.sub(outward.scale(epsilon));
                    let outward_speed = particle.velocity.dot(outward);
                    if outward_speed > 0.0 {
                        particle.velocity = particle.velocity.sub(outward.scale(outward_speed * 1.05));
                    }
                }
            } else if let Some((collision_lo, collision_hi)) = collider_bounds {
                let thickness = cell_size.max(1e-4);
                let inside_x = particle.position.x >= collision_lo.x - thickness
                    && particle.position.x <= collision_hi.x + thickness;
                let inside_z = particle.position.z >= collision_lo.z - thickness
                    && particle.position.z <= collision_hi.z + thickness;

                // Treat the static mesh as an open container rather than a
                // solid AABB. The old test considered every point inside the
                // bounds to be colliding, which lifted an emitter inside the
                // box onto the top face.
                if inside_x && inside_z && particle.position.y < collision_lo.y + thickness {
                    particle.position.y = collision_lo.y + thickness;
                    particle.velocity.y = particle.velocity.y.abs() * 0.15;
                }
                if particle.position.y >= collision_lo.y
                    && particle.position.y <= collision_hi.y
                    && particle.position.z >= collision_lo.z
                    && particle.position.z <= collision_hi.z
                {
                    if particle.position.x < collision_lo.x + thickness {
                        particle.position.x = collision_lo.x + thickness;
                        particle.velocity.x = particle.velocity.x.abs() * 0.15;
                    } else if particle.position.x > collision_hi.x - thickness {
                        particle.position.x = collision_hi.x - thickness;
                        particle.velocity.x = -particle.velocity.x.abs() * 0.15;
                    }
                }
                if particle.position.y >= collision_lo.y
                    && particle.position.y <= collision_hi.y
                    && particle.position.x >= collision_lo.x
                    && particle.position.x <= collision_hi.x
                {
                    if particle.position.z < collision_lo.z + thickness {
                        particle.position.z = collision_lo.z + thickness;
                        particle.velocity.z = particle.velocity.z.abs() * 0.15;
                    } else if particle.position.z > collision_hi.z - thickness {
                        particle.position.z = collision_hi.z - thickness;
                        particle.velocity.z = -particle.velocity.z.abs() * 0.15;
                    }
                }
            }
        }
    };
    // Positions with velocity `v` (the Whitewater Solver reads it).
    let output = |particles: &Vec<FlipParticle>| Mesh {
        vertices: particles.iter().map(|particle| particle.position).collect(),
        indices: Vec::new(),
        point_attributes: HashMap::from([("v".to_owned(), particles.iter().flat_map(|p| [p.velocity.x, p.velocity.y, p.velocity.z]).collect())]),
    };
    sims::run(key, sims::step_count(time, per_frame), per_frame * 12, init, Clone::clone, Clone::clone, step, output)
}

// The viewport pick for `target`: a Select button, or while it runs, how many are
// picked, the points/primitives switch (unless `prims_only`), Accept and Cancel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn group_pick_ui(
    ui: &mut egui::Ui,
    pick: Option<&GroupPick>,
    picked: (usize, usize),
    id: NodeId,
    target: PickTarget,
    prims_only: bool,
    request: &mut Option<(NodeId, PickTarget)>,
    switch_domain: &mut Option<groups::GroupDomain>,
    finish: &mut Option<bool>,
) {
    let Some(pick) = pick.filter(|p| p.node == id && p.target == target) else {
        if ui.button("⬚ Select").on_hover_text("Pick the group's points or primitives in the viewport").clicked() {
            *request = Some((id, target));
        }
        return;
    };
    ui.horizontal(|ui| {
        let count = if pick.domain == groups::GroupDomain::Points { picked.0 } else { picked.1 };
        ui.colored_label(egui::Color32::from_rgb(255, 210, 90), format!("Selecting: {count}"));
        if !prims_only {
            for (d, label) in [(groups::GroupDomain::Points, "Points"), (groups::GroupDomain::Prims, "Primitives")] {
                if ui.selectable_label(pick.domain == d, label).clicked() && pick.domain != d {
                    *switch_domain = Some(d);
                }
            }
        }
        if ui.button("✓ Accept").clicked() {
            *finish = Some(true);
        }
        if ui.button("Cancel").clicked() {
            *finish = Some(false);
        }
    });
}

pub(crate) fn node_clip(mesh: Mesh, axis: usize, offset: f32, invert: bool) -> Mesh {
    let keep = |v: Vec3| {
        let val = axis_of(v, axis);
        if invert { val <= offset } else { val >= offset }
    };
    let vertices = mesh.vertices.clone();
    keep_triangles(mesh, |_, tri| tri.iter().all(|&v| keep(vertices[v])))
}

pub(crate) fn node_blast(mesh: Mesh, component: usize, indices: &[usize], invert: bool) -> Mesh {
    if mesh.indices.is_empty() {
        return mesh;
    }
    let selected_indices: HashSet<usize> = indices.iter().copied().collect();
    let selected = |index: usize| {
        let in_range = selected_indices.contains(&index);
        if invert { !in_range } else { in_range }
    };

    match component.min(2) {
        0 => {
            let remove_vertex: HashSet<usize> = (0..mesh.vertices.len())
                .filter(|&index| selected(index))
                .collect();
            keep_triangles(mesh, |_, tri| !tri.iter().any(|index| remove_vertex.contains(index)))
        }
        1 => keep_triangles(mesh, |index, _| !selected(index)),
        _ => {
            let mut edges = Vec::<(usize, usize)>::new();
            let mut edge_ids = HashMap::<(usize, usize), usize>::new();
            for tri in &mesh.indices {
                for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    let edge = if a < b { (a, b) } else { (b, a) };
                    if !edge_ids.contains_key(&edge) {
                        let id = edges.len();
                        edges.push(edge);
                        edge_ids.insert(edge, id);
                    }
                }
            }
            let remove_edges: HashSet<(usize, usize)> = edges
                .into_iter()
                .enumerate()
                .filter_map(|(index, edge)| selected(index).then_some(edge))
                .collect();
            keep_triangles(mesh, |_, tri| {
                ![(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                    .into_iter()
                    .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
                    .any(|edge| remove_edges.contains(&edge))
            })
        }
    }
}

/// Dependency-free sparse voxel surface extraction. This mirrors the useful
/// part of a VDB workflow while keeping the app portable: occupied cells are
/// stored sparsely and only exposed voxel faces are emitted.
pub(crate) fn node_vdb(mesh: Mesh, voxel_size: f32, padding: usize, operation: usize, iterations: usize) -> Mesh {
    let voxel_size = voxel_size.max(0.01);
    let padding = padding.min(4) as i32;
    const MAX_VOXELS: usize = 1_000_000;
    let mut occupied = HashSet::new();
    let to_cell = |v: Vec3| {
        (
            (v.x / voxel_size).floor() as i32,
            (v.y / voxel_size).floor() as i32,
            (v.z / voxel_size).floor() as i32,
        )
    };
    for tri in &mesh.indices {
        let a = mesh.vertices[tri[0]];
        let b = mesh.vertices[tri[1]];
        let c = mesh.vertices[tri[2]];
        let steps = ((b.sub(a).length().max(c.sub(a).length()).max(c.sub(b).length())
            / voxel_size)
            .ceil() as usize)
            .clamp(1, 128);
        for i in 0..=steps {
            for j in 0..=steps - i {
                let u = i as f32 / steps as f32;
                let v = j as f32 / steps as f32;
                let p = a.add(b.sub(a).scale(u)).add(c.sub(a).scale(v));
                let cell = to_cell(p);
                for dx in -padding..=padding {
                    for dy in -padding..=padding {
                        for dz in -padding..=padding {
                            if occupied.len() < MAX_VOXELS {
                                occupied.insert((cell.0 + dx, cell.1 + dy, cell.2 + dz));
                            }

                        }
                    }
                    let directions = [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ];
                    for _ in 0..iterations.min(4) {
                        let next = match operation.min(3) {
                            1 => {
                                let mut expanded = HashSet::with_capacity(
                                    occupied.len().saturating_mul(3).min(MAX_VOXELS),
                                );
                                for &(x, y, z) in &occupied {
                                    if expanded.len() >= MAX_VOXELS {
                                        break;
                                    }
                                    expanded.insert((x, y, z));
                                    for (dx, dy, dz) in directions {
                                        if expanded.len() >= MAX_VOXELS {
                                            break;
                                        }
                                        expanded.insert((x + dx, y + dy, z + dz));
                                    }
                                }
                                expanded
                            }
                            2 => occupied
                                .iter()
                                .copied()
                                .filter(|&(x, y, z)| {
                                    directions
                                        .iter()
                                        .all(|&(dx, dy, dz)| occupied.contains(&(x + dx, y + dy, z + dz)))
                                })
                                .collect(),
                            3 => {
                                let mut smoothed = HashSet::with_capacity(occupied.len());
                                for &(x, y, z) in &occupied {
                                    let neighbors = directions
                                        .iter()
                                        .filter(|&&(dx, dy, dz)| occupied.contains(&(x + dx, y + dy, z + dz)))
                                        .count();
                                    if neighbors >= 3 {
                                        smoothed.insert((x, y, z));
                                    }
                                }
                                smoothed
                            }
                            _ => occupied.clone(),
                        };
                        occupied = next;
                    }
                }
            }
        }
    }
    let faces = [
        ((1, 0, 0), [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)]),
        ((-1, 0, 0), [(0, 0, 1), (0, 1, 1), (0, 1, 0), (0, 0, 0)]),
        ((0, 1, 0), [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)]),
        ((0, -1, 0), [(0, 0, 1), (0, 0, 0), (1, 0, 0), (1, 0, 1)]),
        ((0, 0, 1), [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)]),
        ((0, 0, -1), [(1, 0, 0), (0, 1, 0), (0, 0, 0), (1, 1, 0)]),
    ];
    let mut out = Mesh::default();
    for &(cell, corners) in &faces {
        for &(x, y, z) in &occupied {
            if occupied.contains(&(x + cell.0, y + cell.1, z + cell.2)) {
                continue;
            }
            let base = out.vertices.len();
            for (cx, cy, cz) in corners {
                out.vertices.push(Vec3::new(
                    (x + cx) as f32 * voxel_size,
                    (y + cy) as f32 * voxel_size,
                    (z + cz) as f32 * voxel_size,
                ));
            }
            out.indices.push([base, base + 1, base + 2]);
            out.indices.push([base, base + 2, base + 3]);
        }
    }
    out
}

#[cfg(feature = "native-openvdb")]
pub(crate) fn node_vdb_native(
    mesh: &Mesh,
    voxel_size: f32,
    padding: usize,
    operation: usize,
    iterations: usize,
) -> Option<Mesh> {
    let vertices: Vec<f32> = mesh
        .vertices
        .iter()
        .flat_map(|v| [v.x, v.y, v.z])
        .collect();
    let triangles: Vec<u32> = mesh
        .indices
        .iter()
        .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
        .collect();
    let mut out_vertices = std::ptr::null_mut();
    let mut out_vertex_count = 0;
    let mut out_triangles = std::ptr::null_mut();
    let mut out_triangle_count = 0;
    let ok = unsafe {
        mini_openvdb_surface(
            vertices.as_ptr(),
            mesh.vertices.len(),
            triangles.as_ptr(),
            mesh.indices.len(),
            voxel_size,
            padding as u32,
            operation as u32,
            iterations as u32,
            &mut out_vertices,
            &mut out_vertex_count,
            &mut out_triangles,
            &mut out_triangle_count,
        )
    };
    if ok == 0 {
        return None;
    }
    let result = unsafe {
        let vertices_slice = slice::from_raw_parts(out_vertices, out_vertex_count * 3);
        let triangles_slice = slice::from_raw_parts(out_triangles, out_triangle_count * 3);
        Mesh {
            vertices: vertices_slice
                .chunks_exact(3)
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .collect(),
            indices: triangles_slice
                .chunks_exact(3)
                .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
                .collect(),
            point_attributes: HashMap::new(),
        }
    };
    unsafe {
        mini_openvdb_free(out_vertices.cast());
        mini_openvdb_free(out_triangles.cast());
    }
    Some(result)
}

#[cfg(feature = "native-openvdb")]
pub(crate) fn native_volume_from_mesh(mesh: &Mesh, voxel_size: f32, padding: usize) -> Option<NativeVolume> {
    let vertices: Vec<f32> = mesh.vertices.iter().flat_map(|v| [v.x, v.y, v.z]).collect();
    let triangles: Vec<u32> = mesh
        .indices
        .iter()
        .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
        .collect();
    let handle = unsafe {
        mini_openvdb_volume_from_mesh(
            vertices.as_ptr(),
            mesh.vertices.len(),
            triangles.as_ptr(),
            mesh.indices.len(),
            voxel_size,
            padding as u32,
        )
    };
    (!handle.is_null()).then_some(NativeVolume(handle))
}

#[cfg(feature = "native-openvdb")]
pub(crate) fn native_volume_to_mesh(volume: &NativeVolume) -> Option<Mesh> {
    let mut vertices = std::ptr::null_mut();
    let mut vertex_count = 0;
    let mut triangles = std::ptr::null_mut();
    let mut triangle_count = 0;
    let ok = unsafe {
        mini_openvdb_volume_to_mesh(
            volume.0,
            &mut vertices,
            &mut vertex_count,
            &mut triangles,
            &mut triangle_count,
        )
    };
    if ok == 0 {
        return None;
    }
    let mesh = unsafe {
        let vs = slice::from_raw_parts(vertices, vertex_count * 3);
        let ts = slice::from_raw_parts(triangles, triangle_count * 3);
        Mesh {
            vertices: vs.chunks_exact(3).map(|v| Vec3::new(v[0], v[1], v[2])).collect(),
            indices: ts
                .chunks_exact(3)
                .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
                .collect(),
            point_attributes: HashMap::new(),
        }
    };
    unsafe {
        mini_openvdb_free(vertices.cast());
        mini_openvdb_free(triangles.cast());
    }
    Some(mesh)
}
