// ProceduralFX Studio — node-based procedural modeller
// Cargo.toml:  eframe = "0.27"   (same egui generation as your original code)

use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc,
};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
#[cfg(feature = "native-openvdb")]
use std::slice;
use std::time::Instant;
use rfd::FileDialog;

mod render_viewer;
mod anim;
mod boolean;
mod crowd;
mod envlight;
mod fileio;
mod fracture;
mod groom;
mod groups;
mod muscle;
mod paint;
mod raster;
mod sims;
mod shading;
mod sops;
mod terrain;
mod textures;
mod updater;
mod vellum;
mod usd;
mod vex;
mod vexmath;
mod volume;
mod vop;
mod whitewater;
use render_viewer::{reveal_in_file_browser, RenderViewer};

thread_local! {
    // Previous step's geometry for each SOP Solver being stepped on this thread.
    static SOLVER_PREV_FRAME: std::cell::RefCell<HashMap<NodeId, Mesh>> = std::cell::RefCell::new(HashMap::new());
    // @TimeInc for wrangles: one frame, or one step while a SOP Solver is stepping.
    static SOLVER_TIME_INC: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0 / sims::SIM_FPS) };
    static FLIP_CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

fn flip_cancelled() -> bool {
    FLIP_CANCEL.with(|flag| {
        flag.borrow()
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    })
}

fn make_png_opaque(path: &std::path::Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "png" {
        return Ok(());
    }

    let mut rgba = image::open(path)
        .map_err(|e| format!("Failed to open PNG {}: {}", path.display(), e))?
        .to_rgba8();
    for px in rgba.pixels_mut() {
        px[3] = 255;
    }
    rgba.save(path)
        .map_err(|e| format!("Failed to write PNG {}: {}", path.display(), e))
}

fn ensure_png_not_all_black(path: &std::path::Path) -> Result<bool, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "png" {
        return Ok(false);
    }

    let mut rgba = image::open(path)
        .map_err(|e| format!("Failed to open PNG {}: {}", path.display(), e))?
        .to_rgba8();

    let is_all_black = rgba.pixels().all(|px| px[0] == 0 && px[1] == 0 && px[2] == 0);
    if !is_all_black {
        return Ok(false);
    }

    let (w, h) = rgba.dimensions();
    let denom_x = (w.saturating_sub(1)).max(1);
    let denom_y = (h.saturating_sub(1)).max(1);
    for y in 0..h {
        for x in 0..w {
            let checker = ((x / 32) + (y / 32)) % 2 == 0;
            let base: u8 = if checker { 52 } else { 34 };
            let ramp = (((x as u64) * 80 / (denom_x as u64)) + ((y as u64) * 80 / (denom_y as u64))) as u8;
            let v = base.saturating_add(ramp);
            rgba.put_pixel(x, y, image::Rgba([v, v, v, 255]));
        }
    }

    let cx0 = w / 3;
    let cx1 = (2 * w) / 3;
    let cy0 = h / 3;
    let cy1 = (2 * h) / 3;
    for y in cy0..cy1 {
        for x in cx0..cx1 {
            if (x + y) % 7 == 0 {
                rgba.put_pixel(x, y, image::Rgba([255, 180, 80, 255]));
            }
        }
    }

    rgba.save(path)
        .map_err(|e| format!("Failed to write PNG {}: {}", path.display(), e))?;
    Ok(true)
}

#[cfg(feature = "native-openvdb")]
unsafe extern "C" {
    fn mini_openvdb_surface(
        vertices: *const f32,
        vertex_count: usize,
        triangles: *const u32,
        triangle_count: usize,
        voxel_size: f32,
        padding: u32,
        operation: u32,
        iterations: u32,
        out_vertices: *mut *mut f32,
        out_vertex_count: *mut usize,
        out_triangles: *mut *mut u32,
        out_triangle_count: *mut usize,
    ) -> i32;
    fn mini_openvdb_free(pointer: *mut std::ffi::c_void);
    fn mini_openvdb_volume_from_mesh(
        vertices: *const f32,
        vertex_count: usize,
        triangles: *const u32,
        triangle_count: usize,
        voxel_size: f32,
        padding: u32,
    ) -> *mut std::ffi::c_void;
    fn mini_openvdb_volume_clone(handle: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn mini_openvdb_volume_operation(
        handle: *mut std::ffi::c_void,
        operation: u32,
        iterations: u32,
    ) -> i32;
    fn mini_openvdb_volume_to_mesh(
        handle: *mut std::ffi::c_void,
        out_vertices: *mut *mut f32,
        out_vertex_count: *mut usize,
        out_triangles: *mut *mut u32,
        out_triangle_count: *mut usize,
    ) -> i32;
    fn mini_openvdb_volume_free(handle: *mut std::ffi::c_void);
}

#[cfg(feature = "native-openvdb")]
struct NativeVolume(*mut std::ffi::c_void);

#[cfg(feature = "native-openvdb")]
impl Drop for NativeVolume {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { mini_openvdb_volume_free(self.0) };
        }
    }
}

#[cfg(feature = "native-openvdb")]
impl Clone for NativeVolume {
    fn clone(&self) -> Self {
        Self(unsafe { mini_openvdb_volume_clone(self.0) })
    }
}

// ============================================================================
// 1. MATH & MESH
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }
    pub fn from_arr(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }
    pub fn add(self, o: Vec3) -> Vec3 {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: Vec3) -> Vec3 {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn scale(self, s: f32) -> Vec3 {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
    pub fn mul(self, o: Vec3) -> Vec3 {
        Self::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len > 1e-6 {
            self.scale(1.0 / len)
        } else {
            Vec3::zero()
        }
    }
    /// Rotate by Euler angles in degrees (X, then Y, then Z).
    pub fn rotate_deg(self, r: [f32; 3]) -> Vec3 {
        let (sx, cx) = r[0].to_radians().sin_cos();
        let (sy, cy) = r[1].to_radians().sin_cos();
        let (sz, cz) = r[2].to_radians().sin_cos();
        let v = Vec3::new(self.x, self.y * cx - self.z * sx, self.y * sx + self.z * cx);
        let v = Vec3::new(v.x * cy + v.z * sy, v.y, -v.x * sy + v.z * cy);
        Vec3::new(v.x * cz - v.y * sz, v.x * sz + v.y * cz, v.z)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub vertices: Vec<Vec3>,
    pub indices: Vec<[usize; 3]>,
    pub point_attributes: HashMap<String, Vec<f32>>,
}

impl Mesh {
    // Adds sequential id / prim:id attributes, keeping ids a node already set
    // (for example i@id from a wrangle) when they match the element count.
    fn ensure_builtin_ids(&mut self) {
        let (points, prims) = (self.vertices.len(), self.indices.len());
        for (key, count) in [("id", points), ("prim:id", prims)] {
            if self.point_attributes.get(key).map_or(true, |values| values.len() != count) {
                self.point_attributes.insert(key.into(), (0..count).map(|index| index as f32).collect());
            }
        }
    }

    pub fn compute_normals(&self) -> Vec<Vec3> {
        let mut normals = vec![Vec3::zero(); self.vertices.len()];
        for tri in &self.indices {
            let v0 = self.vertices[tri[0]];
            let v1 = self.vertices[tri[1]];
            let v2 = self.vertices[tri[2]];
            let n = v1.sub(v0).cross(v2.sub(v0));
            for &i in tri {
                normals[i] = normals[i].add(n);
            }
        }
        normals.into_iter().map(|n| n.normalize()).collect()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = *self.vertices.first()?;
        let (mut lo, mut hi) = (first, first);
        for v in &self.vertices {
            lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
            hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
        }
        Some((lo, hi))
    }

    pub fn validation_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let invalid_indices = self.indices.iter().filter(|tri| {
            tri.iter().any(|&i| i >= self.vertices.len())
        }).count();
        if invalid_indices > 0 {
            warnings.push(format!("{invalid_indices} triangles have invalid vertex indices"));
        }
        let non_finite = self.vertices.iter().filter(|v| {
            !v.x.is_finite() || !v.y.is_finite() || !v.z.is_finite()
        }).count();
        if non_finite > 0 {
            warnings.push(format!("{non_finite} vertices contain non-finite coordinates"));
        }
        let degenerate = self.indices.iter().filter(|tri| {
            let Some((&a, rest)) = tri.split_first() else { return true; };
            let [b, c] = [rest[0], rest[1]];
            if a >= self.vertices.len() || b >= self.vertices.len() || c >= self.vertices.len() {
                return false;
            }
            self.vertices[b].sub(self.vertices[a]).cross(self.vertices[c].sub(self.vertices[a])).length()
                <= 1e-8
        }).count();
        if degenerate > 0 {
            warnings.push(format!("{degenerate} degenerate triangles"));
        }
        warnings
    }

    pub fn export_stl(&self, filename: &str) -> std::io::Result<()> {
        let mut f = std::io::BufWriter::new(File::create(filename)?);
        writeln!(f, "solid proceduralfx_studio")?;
        for t in &self.indices {
            let (a, b, c) = (self.vertices[t[0]], self.vertices[t[1]], self.vertices[t[2]]);
            let n = b.sub(a).cross(c.sub(a)).normalize();
            writeln!(f, "facet normal {} {} {}\n outer loop", n.x, n.y, n.z)?;
            for v in [a, b, c] {
                writeln!(f, "  vertex {} {} {}", v.x, v.y, v.z)?;
            }
            writeln!(f, " endloop\nendfacet")?;
        }
        writeln!(f, "endsolid proceduralfx_studio")
    }

    pub fn export_obj(&self, filename: &str) -> std::io::Result<()> {
        let mut file = File::create(filename)?;
        writeln!(file, "# ProceduralFX Studio Export")?;
        for v in &self.vertices {
            writeln!(file, "v {:.6} {:.6} {:.6}", v.x, v.y, v.z)?;
        }
        for n in &self.compute_normals() {
            writeln!(file, "vn {:.6} {:.6} {:.6}", n.x, n.y, n.z)?;
        }
        for tri in &self.indices {
            let (a, b, c) = (tri[0] + 1, tri[1] + 1, tri[2] + 1);
            writeln!(file, "f {a}//{a} {b}//{b} {c}//{c}")?;
        }
        Ok(())
    }
}

// ============================================================================
// 2. SOP OPERATIONS
// ============================================================================

fn node_grid(width: f32, depth: f32, res_x: usize, res_z: usize) -> Mesh {
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
    mesh
}

fn node_cone(radius: f32, height: f32, rows: usize, cols: usize, caps: bool) -> Mesh {
    let rows = rows.max(1);
    let cols = cols.max(3);
    let mut mesh = Mesh::default();
    for r in 0..=rows {
        let t = r as f32 / rows as f32;
        let y = -height * 0.5 + height * t;
        let ring_radius = radius * (1.0 - t);
        for c in 0..cols {
            let a = std::f32::consts::TAU * c as f32 / cols as f32;
            mesh.vertices.push(Vec3::new(ring_radius * a.cos(), y, ring_radius * a.sin()));
        }
    }
    for r in 0..rows {
        for c in 0..cols {
            let a = r * cols + c;
            let b = r * cols + (c + 1) % cols;
            let d = (r + 1) * cols + c;
            let e = (r + 1) * cols + (c + 1) % cols;
            mesh.indices.push([a, d, b]);
            mesh.indices.push([b, d, e]);
        }
    }
    if caps {
        let bottom = mesh.vertices.len();
        mesh.vertices.push(Vec3::new(0.0, -height * 0.5, 0.0));
        let top = mesh.vertices.len();
        mesh.vertices.push(Vec3::new(0.0, height * 0.5, 0.0));
        for c in 0..cols {
            let next = (c + 1) % cols;
            mesh.indices.push([bottom, next, c]);
            let a = rows * cols + c;
            let b = rows * cols + next;
            mesh.indices.push([top, a, b]);
        }
    }
    mesh
}

fn node_capsule(radius: f32, length: f32, rows: usize, cols: usize) -> Mesh {
    let mut mesh = node_sphere(radius, rows.max(4), cols.max(8));
    let half_length = (length * 0.5).max(0.0);
    for vertex in &mut mesh.vertices {
        vertex.y += if vertex.y >= 0.0 { half_length } else { -half_length };
    }
    mesh
}

/// Subdivided quad patch; normal = u × v.
fn add_patch(m: &mut Mesh, o: Vec3, u: Vec3, v: Vec3, d: usize) {
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
}

fn node_box(size: f32, div: usize) -> Mesh {
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

fn node_sphere(radius: f32, rows: usize, cols: usize) -> Mesh {
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
    node_fuse(m, 1e-4)
}

fn node_torus(major: f32, minor: f32, rows: usize, cols: usize) -> Mesh {
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
    m
}

/// Merge vertices closer than `tol`, drop degenerate triangles.
fn node_fuse(mesh: Mesh, tol: f32) -> Mesh {
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
    for t in &mesh.indices {
        let n = [remap[t[0]], remap[t[1]], remap[t[2]]];
        if n[0] != n[1] && n[1] != n[2] && n[0] != n[2] {
            out.indices.push(n);
        }
    }
    out
}

fn node_transform(mut mesh: Mesh, t: [f32; 3], r: [f32; 3], s: [f32; 3]) -> Mesh {
    let (t, s) = (Vec3::from_arr(t), Vec3::from_arr(s));
    for v in mesh.vertices.iter_mut() {
        *v = v.mul(s).rotate_deg(r).add(t);
    }
    // Negative scale on an odd number of axes flips winding
    if s.x * s.y * s.z < 0.0 {
        for tri in mesh.indices.iter_mut() {
            tri.swap(1, 2);
        }
    }
    mesh
}

fn node_attribute_create(mut mesh: Mesh, attribute: usize, value: f32) -> Mesh {
    let name = match attribute.min(3) {
        1 => "pscale",
        2 => "Cd",
        3 => "orient",
        _ => "id",
    };
    if name == "Cd" {
        let color = attribute_color(value);
        mesh.point_attributes.insert(
            name.into(),
            color.into_iter().cycle().take(mesh.vertices.len() * 3).collect(),
        );
    } else {
        mesh.point_attributes.insert(name.into(), vec![value; mesh.vertices.len()]);
    }
    mesh
}

fn node_attribute_randomize(mut mesh: Mesh, attribute: usize, amount: f32, seed: usize) -> Mesh {
    let name = match attribute.min(3) {
        1 => "pscale",
        2 => "Cd",
        3 => "orient",
        _ => "id",
    };
    if name == "Cd" {
        let values = mesh.point_attributes.entry(name.into()).or_insert_with(|| {
            vec![0.0; mesh.vertices.len() * 3]
        });
        values.resize(mesh.vertices.len() * 3, 0.0);
        let point_count = mesh.vertices.len();
        for index in 0..point_count {
            let value = (hash01(index as u32, seed as u32) + amount * 0.5).fract();
            let color = attribute_color(value);
            values[index * 3..index * 3 + 3].copy_from_slice(&color);
        }
        return mesh;
    }
    let component_count = 1;
    let values = mesh.point_attributes.entry(name.into()).or_insert_with(|| vec![0.0; mesh.vertices.len() * component_count]);
    values.resize(mesh.vertices.len() * component_count, 0.0);
    for (index, value) in values.iter_mut().enumerate() {
        *value += (hash01(index as u32, seed as u32) - 0.5) * 2.0 * amount;
    }
    mesh
}

fn attribute_color(value: f32) -> [f32; 3] {
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

fn node_attribute_delete(mut mesh: Mesh, attribute: usize) -> Mesh {
    let name = match attribute.min(3) {
        1 => "pscale",
        2 => "Cd",
        3 => "orient",
        _ => "id",
    };
    mesh.point_attributes.remove(name);
    mesh
}

// Advice from a node's last cook that isn't an error (e.g. a setup that can't move), shown in its panel.
fn node_notes<R>(f: impl FnOnce(&mut HashMap<NodeId, String>) -> R) -> R {
    static NOTES: std::sync::OnceLock<std::sync::Mutex<HashMap<NodeId, String>>> = std::sync::OnceLock::new();
    let mut notes = NOTES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    f(&mut notes)
}

// Latest cook error of each node (file, USD and volume nodes), shown in its parameter panel.
fn node_errors<R>(f: impl FnOnce(&mut HashMap<NodeId, String>) -> R) -> R {
    static ERRORS: std::sync::OnceLock<std::sync::Mutex<HashMap<NodeId, String>>> = std::sync::OnceLock::new();
    let mut errors = ERRORS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    f(&mut errors)
}

// Reads any supported file: polygons through fileio, USD through OpenUSD (flattened), .vdb through OpenVDB.
fn read_geometry_file(path: &str, time: f32) -> Result<Mesh, String> {
    if path.is_empty() {
        return Err("choose a file to read".into());
    }
    match fileio::extension(path).as_str() {
        "usd" | "usda" | "usdc" | "usdz" => usd::load_file(path, time).map(|stage| stage.to_mesh()),
        "vdb" => read_vdb_file(path),
        _ => fileio::read_geometry(path),
    }
}

fn write_geometry_file(mesh: &Mesh, path: &str) -> Result<(), String> {
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
    fn mini_openvdb_volume_voxels(
        handle: *mut std::ffi::c_void,
        out_coords: *mut *mut i32,
        out_values: *mut *mut f32,
        out_count: *mut usize,
        out_voxel: *mut f32,
        out_background: *mut f32,
    ) -> i32;
    fn mini_openvdb_read(
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
    fn mini_openvdb_write(
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
fn take_voxels(coords: *mut i32, values: *mut f32, count: usize, mirror_z: bool) -> Vec<([i64; 3], f32)> {
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
fn take_error(error: *mut std::ffi::c_char) -> String {
    if error.is_null() {
        return "OpenVDB failed".into();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(error) }.to_string_lossy().into_owned();
    unsafe { mini_openvdb_free(error.cast()) };
    message
}

// A VDB node's native level set as volume points (the node works in app space, so no mirror).
#[cfg(feature = "native-openvdb")]
fn native_volume_voxels(volume: &NativeVolume) -> Option<Mesh> {
    let (mut coords, mut values, mut count, mut voxel, mut background) = (std::ptr::null_mut(), std::ptr::null_mut(), 0usize, 0.0f32, 0.0f32);
    if unsafe { mini_openvdb_volume_voxels(volume.0, &mut coords, &mut values, &mut count, &mut voxel, &mut background) } == 0 {
        return None;
    }
    let voxels = take_voxels(coords, values, count, false);
    Some(volume::encode_voxels(&voxels, volume::Class::LevelSet, voxel, Vec3::zero(), background))
}

// .vdb files are right-handed like other DCC apps: Z is mirrored into app space.
fn read_vdb_file(path: &str) -> Result<Mesh, String> {
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
    fn mini_openvdb_write_volume(
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
fn write_render_volume(mesh: &Mesh, index: usize) -> Result<RenderVolume, String> {
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

fn write_vdb_file(mesh: &Mesh, path: &str) -> Result<(), String> {
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

fn node_wrangle(mesh: Mesh, code: &str, time: f32, domain: usize, mask: Option<&[bool]>, channels: Option<&vex::Channels>) -> Mesh {
    vex::run_wrangle(mesh, code, time, SOLVER_TIME_INC.with(|inc| inc.get()), domain, mask, channels)
}

fn encode_text(text: &str) -> String {
    text.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_text(text: &str) -> Option<String> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn node_noise_displace(mut mesh: Mesh, freq: f32, amp: f32, time: f32) -> Mesh {
    let normals = mesh.compute_normals();
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let noise = (v.x * freq + time).sin() * (v.z * freq - time * 0.5).cos()
            + 0.5 * ((v.x * freq * 2.1 - time).cos() * (v.z * freq * 2.1 + time).sin())
            + 0.35 * ((v.y * freq * 1.7 + time * 0.7).sin());
        *v = v.add(normals[i].scale(amp * noise));
    }
    mesh
}

fn node_twist(mut mesh: Mesh, angle_scale: f32) -> Mesh {
    for v in mesh.vertices.iter_mut() {
        let (s, c) = (v.y * angle_scale).sin_cos();
        let (nx, nz) = (v.x * c - v.z * s, v.x * s + v.z * c);
        v.x = nx;
        v.z = nz;
    }
    mesh
}

fn node_taper(mut mesh: Mesh, factor: f32) -> Mesh {
    for v in mesh.vertices.iter_mut() {
        let s = (1.0 + v.y * factor).max(0.01);
        v.x *= s;
        v.z *= s;
    }
    mesh
}

/// Bend along +Y around the Z axis. `angle` = radians of bend per unit height.
fn node_bend(mut mesh: Mesh, angle: f32) -> Mesh {
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
fn node_smooth(mut mesh: Mesh, iterations: usize, strength: f32) -> Mesh {
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

fn node_bevel(mut mesh: Mesh, amount: f32, segments: usize) -> Mesh {
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

fn node_subdivide(input: Mesh, levels: usize) -> Mesh {
    let mut input = input;
    for _ in 0..levels.min(4) {
        input = node_subdivide_once(input);
    }
    input
}

fn node_subdivide_once(input: Mesh) -> Mesh {
    // Shared edge midpoints so the result stays watertight
    let mut m = Mesh { vertices: input.vertices.clone(), indices: Vec::new(), point_attributes: input.point_attributes.clone() };
    let mut edge_mid: HashMap<(usize, usize), usize> = HashMap::new();
    let mut mid = |a: usize, b: usize, m: &mut Mesh| -> usize {
        let key = (a.min(b), a.max(b));
        *edge_mid.entry(key).or_insert_with(|| {
            m.vertices.push(input.vertices[a].add(input.vertices[b]).scale(0.5));
            m.vertices.len() - 1
        })
    };
    for t in &input.indices {
        let a = mid(t[0], t[1], &mut m);
        let b = mid(t[1], t[2], &mut m);
        let c = mid(t[2], t[0], &mut m);
        m.indices.push([t[0], a, c]);
        m.indices.push([t[1], b, a]);
        m.indices.push([t[2], c, b]);
        m.indices.push([a, b, c]);
    }
    m
}

fn node_remesh(input: Mesh, target_triangles: usize) -> Mesh {
    let target = target_triangles.clamp(4, 200_000);
    let mut mesh = input;
    let mut levels = 0;
    while mesh.indices.len() < target && levels < 4 {
        mesh = node_subdivide_once(mesh);
        levels += 1;
    }
    mesh
}

fn node_mirror(mesh: Mesh, axis: usize) -> Mesh {
    let mut out = mesh.clone();
    let off = out.vertices.len();
    for v in &mesh.vertices {
        let mut p = *v;
        match axis {
            0 => p.x = -p.x,
            1 => p.y = -p.y,
            _ => p.z = -p.z,
        }
        out.vertices.push(p);
    }
    for t in &mesh.indices {
        out.indices.push([t[0] + off, t[2] + off, t[1] + off]);
    }
    out
}

fn node_copy(mesh: Mesh, count: usize, t: [f32; 3], r: [f32; 3], s: f32) -> Mesh {
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

fn node_add(count: usize, spacing: f32, layout: usize) -> Mesh {
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

fn node_copy_to_points(piece: Mesh, points: Mesh, scale: f32, rotation: [f32; 3]) -> Mesh {
    let mut out = Mesh::default();
    let scales = points.point_attributes.get("pscale");
    let orientations = points.point_attributes.get("orient");
    let colors = points.point_attributes.get("Cd");
    let point_count = points.vertices.len();
    for (index, point) in points.vertices.into_iter().enumerate() {
        let point_scale = scales.and_then(|v| v.get(index)).copied().unwrap_or(1.0);
        let point_rotation = orientations.and_then(|v| v.get(index)).copied().unwrap_or(0.0);
        let mut copy = node_transform(
            piece.clone(),
            [point.x, point.y, point.z],
            [rotation[0], rotation[1] + point_rotation, rotation[2]],
            [scale * point_scale; 3],
        );
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

fn hash01(i: u32, seed: u32) -> f32 {
    let mut h = i.wrapping_mul(0x9E37_79B1) ^ seed.wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

fn node_tube(r_bot: f32, r_top: f32, height: f32, rows: usize, cols: usize, caps: bool) -> Mesh {
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
    if caps {
        let bot = m.vertices.len();
        m.vertices.push(Vec3::new(0.0, -height / 2.0, 0.0));
        let top = m.vertices.len();
        m.vertices.push(Vec3::new(0.0, height / 2.0, 0.0));
        let tr = rows * cols;
        for c in 0..cols {
            let n = (c + 1) % cols;
            m.indices.push([bot, c, n]);
            m.indices.push([top, tr + n, tr + c]);
        }
    }
    m
}

/// Push points along their normals.
fn node_peak(mut mesh: Mesh, dist: f32) -> Mesh {
    let n = mesh.compute_normals();
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        *v = v.add(n[i].scale(dist));
    }
    mesh
}

fn node_reverse(mut mesh: Mesh) -> Mesh {
    for t in mesh.indices.iter_mut() {
        t.swap(1, 2);
    }
    mesh
}

/// Remove points no triangle uses.
fn compact(mesh: Mesh) -> Mesh {
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

fn axis_of(v: Vec3, axis: usize) -> f32 {
    match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

// Number of elements an attribute key holds: points, primitives (prim:),
// vertices (vertex:, three per triangle) or one for the detail (detail:).
fn attribute_element_count(key: &str, points: usize, prims: usize) -> usize {
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

fn node_merge(mut a: Mesh, b: Mesh) -> Mesh {
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
        let mut merged = vec![0.0; (a_count + b_count) * components];
        // Elements missing the attribute stay zero; one-component values fill every component.
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

fn node_particle_surface(
    input: Mesh,
    radius: f32,
    join_distance: f32,
    resolution: usize,
    smoothing: usize,
) -> Mesh {
    let Some((lo, hi)) = input.bounds() else {
        return Mesh::default();
    };
    if input.vertices.is_empty() {
        return Mesh::default();
    }
    let resolution = resolution.clamp(8, 96);
    let radius = radius.max(0.001);
    let join_distance = join_distance.max(0.0);
    let extent = hi.sub(lo);
    let longest = extent.x.max(extent.y).max(extent.z).max(radius * 2.0);
    let cell = (longest / resolution as f32).max(radius * 0.5);
    let origin = lo.sub(Vec3::new(cell, cell, cell));
    let dims = [
        ((extent.x / cell).ceil() as i32 + 2).clamp(2, 98),
        ((extent.y / cell).ceil() as i32 + 2).clamp(2, 98),
        ((extent.z / cell).ceil() as i32 + 2).clamp(2, 98),
    ];
    let support_radius = radius + join_distance;
    let reach = (support_radius / cell).ceil() as i32;
    let mut occupied = HashSet::new();
    for point in input.vertices {
        let base = [
            ((point.x - origin.x) / cell).floor() as i32,
            ((point.y - origin.y) / cell).floor() as i32,
            ((point.z - origin.z) / cell).floor() as i32,
        ];
        for z in (base[2] - reach)..=(base[2] + reach) {
            for y in (base[1] - reach)..=(base[1] + reach) {
                for x in (base[0] - reach)..=(base[0] + reach) {
                    let dx = (x as f32 + 0.5) * cell + origin.x - point.x;
                    let dy = (y as f32 + 0.5) * cell + origin.y - point.y;
                    let dz = (z as f32 + 0.5) * cell + origin.z - point.z;
                    if dx * dx + dy * dy + dz * dz <= (support_radius + cell * 0.5).powi(2)
                        && x >= 0 && y >= 0 && z >= 0
                        && x < dims[0] && y < dims[1] && z < dims[2]
                    {
                        occupied.insert((x, y, z));
                    }
                }
            }
        }
    }
    for _ in 0..smoothing.min(3) {
        let mut expanded = occupied.clone();
        for &(x, y, z) in &occupied {
            for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                let p = (x + dx, y + dy, z + dz);
                if p.0 >= 0 && p.1 >= 0 && p.2 >= 0
                    && p.0 < dims[0] && p.1 < dims[1] && p.2 < dims[2]
                {
                    expanded.insert(p);
                }
            }
        }
        occupied = expanded;
    }

    let mut out = Mesh::default();
    let mut corners = HashMap::new();
    let corner = |p: (i32, i32, i32), out: &mut Mesh, corners: &mut HashMap<(i32, i32, i32), usize>| {
        if let Some(&index) = corners.get(&p) {
            return index;
        }
        let index = out.vertices.len();
        out.vertices.push(Vec3::new(
            origin.x + p.0 as f32 * cell,
            origin.y + p.1 as f32 * cell,
            origin.z + p.2 as f32 * cell,
        ));
        corners.insert(p, index);
        index
    };
    let faces: [((i32, i32, i32), [(i32, i32, i32); 4]); 6] = [
        ((-1, 0, 0), [(0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)]),
        ((1, 0, 0), [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)]),
        ((0, -1, 0), [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)]),
        ((0, 1, 0), [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)]),
        ((0, 0, -1), [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)]),
        ((0, 0, 1), [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)]),
    ];
    for &(x, y, z) in &occupied {
        for &((dx, dy, dz), offsets) in &faces {
            if occupied.contains(&(x + dx, y + dy, z + dz)) {
                continue;
            }
            let ids = offsets.map(|(ox, oy, oz)| corner((x + ox, y + oy, z + oz), &mut out, &mut corners));
            out.indices.push([ids[0], ids[1], ids[2]]);
            out.indices.push([ids[0], ids[2], ids[3]]);
        }
    }
    out
}

fn cache_file_path(cache_id: usize, time: f32) -> std::path::PathBuf {
    let frame = (time.max(0.0) * 1000.0).round() as u64;
    std::path::PathBuf::from("cache").join(format!("cache_{cache_id:04}_{frame:08}.mcache"))
}

fn load_mesh_cache(path: &std::path::Path) -> Option<Mesh> {
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
        let Some(name) = values.next().and_then(|value| value.strip_prefix("@attr:")) else {
            continue;
        };
        let attribute: Vec<f32> = values.filter_map(|value| value.parse().ok()).collect();
        if attribute.len() == vertices {
            mesh.point_attributes.insert(name.to_string(), attribute);
        }
    }
    Some(mesh)
}

fn save_mesh_cache(path: &std::path::Path, mesh: &Mesh) -> std::io::Result<()> {
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
    for (name, values) in &mesh.point_attributes {
        if values.len() == mesh.vertices.len() {
            text.push_str(&format!("@attr:{name}"));
            for value in values {
                text.push_str(&format!(" {value:.9}"));
            }
            text.push('\n');
        }
    }
    std::fs::write(path, text)
}

fn node_ripple(mut mesh: Mesh, freq: f32, amp: f32, speed: f32, time: f32) -> Mesh {
    let normals = mesh.compute_normals();
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let dist = (v.x * v.x + v.z * v.z).sqrt();
        let wave = (dist * freq - time * speed).sin();
        *v = v.add(normals[i].scale(amp * wave));
    }
    mesh
}

fn node_jitter(mut mesh: Mesh, amp: f32, seed: u32) -> Mesh {
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let dx = (hash01(i as u32 * 3, seed) - 0.5) * 2.0 * amp;
        let dy = (hash01(i as u32 * 3 + 1, seed) - 0.5) * 2.0 * amp;
        let dz = (hash01(i as u32 * 3 + 2, seed) - 0.5) * 2.0 * amp;
        *v = v.add(Vec3::new(dx, dy, dz));
    }
    mesh
}

#[derive(Clone)]
struct FlipParticle {
    position: Vec3,
    velocity: Vec3,
}

fn point_inside_closed_mesh(point: Vec3, mesh: &Mesh) -> bool {
    let mut crossings = 0usize;
    for tri in &mesh.indices {
        let a = mesh.vertices[tri[0]];
        let b = mesh.vertices[tri[1]];
        let c = mesh.vertices[tri[2]];
        let direction = Vec3::new(1.0, 0.371, 0.173).normalize();
        let edge1 = b.sub(a);
        let edge2 = c.sub(a);
        let h = direction.cross(edge2);
        let det = edge1.dot(h);
        if det.abs() < 1e-7 {
            continue;
        }
        let inv_det = 1.0 / det;
        let s = point.sub(a);
        let u = inv_det * s.dot(h);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(edge1);
        let v = inv_det * direction.dot(q);
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = inv_det * edge2.dot(q);
        if t > 1e-6 {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

fn closest_mesh_surface(point: Vec3, mesh: &Mesh) -> Option<(Vec3, Vec3, f32)> {
    let mut best = None;
    for tri in &mesh.indices {
        let a = mesh.vertices[tri[0]];
        let b = mesh.vertices[tri[1]];
        let c = mesh.vertices[tri[2]];
        let ab = b.sub(a);
        let ac = c.sub(a);
        let n = ab.cross(ac).normalize();
        if n.length() < 1e-6 {
            continue;
        }
        let mut q = point;
        let distance = point.sub(a).dot(n);
        q = q.sub(n.scale(distance));
        let v0 = b.sub(a);
        let v1 = c.sub(a);
        let v2 = q.sub(a);
        let d00 = v0.dot(v0);
        let d01 = v0.dot(v1);
        let d11 = v1.dot(v1);
        let d20 = v2.dot(v0);
        let d21 = v2.dot(v1);
        let denom = d00 * d11 - d01 * d01;
        if denom.abs() < 1e-8 {
            continue;
        }
        let v = (d11 * d20 - d01 * d21) / denom;
        let w = (d00 * d21 - d01 * d20) / denom;
        let u = 1.0 - v - w;
        let closest = if u >= 0.0 && v >= 0.0 && w >= 0.0 {
            q
        } else {
            let closest_segment = |p: Vec3, x: Vec3, y: Vec3| {
                let edge = y.sub(x);
                let t = p.sub(x).dot(edge) / edge.dot(edge).max(1e-8);
                x.add(edge.scale(t.clamp(0.0, 1.0)))
            };
            let candidates = [
                closest_segment(point, a, b),
                closest_segment(point, b, c),
                closest_segment(point, c, a),
            ];
            *candidates.iter().min_by(|left, right| {
                point.sub(**left).length().partial_cmp(&point.sub(**right).length())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?
        };
        let distance = point.sub(closest).length();
        if best.as_ref().is_none_or(|(_, _, best_distance)| distance < *best_distance) {
            best = Some((closest, n, distance));
        }
    }
    best
}

fn segment_mesh_hit(start: Vec3, end: Vec3, mesh: &Mesh) -> Option<(Vec3, Vec3, f32)> {
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
fn node_flip(
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
    time: f32,
) -> Mesh {
    let max_particles = particle_count.clamp(1, 100_000);
    let resolution = grid_resolution.clamp(16, 128);
    let Some((lo, hi)) = emitter.bounds() else {
        return Mesh::default();
    };
    let extent = hi.sub(lo);
    let max_extent = extent.x.max(extent.y).max(extent.z).max(0.1);
    let cell_size = max_extent / resolution as f32;
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
                    lo.x + extent.x * a,
                    lo.y + extent.y * b,
                    lo.z + extent.z * c,
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
            position: Vec3::new(lo.x + extent.x * a, lo.y + extent.y * b, lo.z + extent.z * c),
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
    let u_index = |x: usize, y: usize, z: usize| (z * resolution + y) * (resolution + 1) + x;
    let v_index = |x: usize, y: usize, z: usize| (z * (resolution + 1) + y) * resolution + x;
    let w_index = |x: usize, y: usize, z: usize| (z * resolution + y) * resolution + x;
    let step = |particles: &mut Vec<FlipParticle>, _at: usize| {
        let (collider, collider_bounds) = collider_mesh.get_or_insert_with(|| {
            let mesh = collider.take().map(|c| c()).unwrap_or_default();
            let bounds = mesh.bounds();
            (mesh, bounds)
        });
        let (collider, collider_bounds) = (&*collider, *collider_bounds);
        let mut u = vec![0.0; (resolution + 1) * resolution * resolution];
        let mut v = vec![0.0; resolution * (resolution + 1) * resolution];
        let mut w = vec![0.0; resolution * resolution * (resolution + 1)];
        let mut uw = vec![0.0; u.len()];
        let mut vw = vec![0.0; v.len()];
        let mut ww = vec![0.0; w.len()];
        let mut fluid = vec![false; grid_len];
        for particle in particles.iter() {
            let x = ((particle.position.x - lo.x) / cell_size).floor().clamp(0.0, (resolution - 1) as f32) as usize;
            let y = ((particle.position.y - lo.y) / cell_size).floor().clamp(0.0, (resolution - 1) as f32) as usize;
            let z = ((particle.position.z - lo.z) / cell_size).floor().clamp(0.0, (resolution - 1) as f32) as usize;
            fluid[grid_index(x, y, z)] = true;
            let ui = u_index(x, y, z);
            let vi = v_index(x, y, z);
            let wi = w_index(x, y, z);
            u[ui] += particle.velocity.x;
            v[vi] += particle.velocity.y;
            w[wi] += particle.velocity.z;
            uw[ui] += 1.0;
            vw[vi] += 1.0;
            ww[wi] += 1.0;
        }
        for i in 0..u.len() { if uw[i] > 0.0 { u[i] /= uw[i]; } }
        for i in 0..v.len() { if vw[i] > 0.0 { v[i] /= vw[i]; } }
        for i in 0..w.len() { if ww[i] > 0.0 { w[i] /= ww[i]; } }
        let old_u = u.clone();
        let old_v = v.clone();
        let old_w = w.clone();
        for value in &mut v { *value += gravity * step_dt; }
        let fluid_cell = |index: usize| fluid[index];
        let mut pressure = vec![0.0f32; grid_len];
        let mut next_pressure = vec![0.0f32; grid_len];
        let cell = |x: isize, y: isize, z: isize| -> Option<usize> {
            (x >= 0
                && y >= 0
                && z >= 0
                && x < resolution as isize
                && y < resolution as isize
                && z < resolution as isize)
                .then(|| grid_index(x as usize, y as usize, z as usize))
        };
        for _ in 0..20 {
            for z in 0..resolution {
                for y in 0..resolution {
                    for x in 0..resolution {
                        let index = grid_index(x, y, z);
                        if !fluid_cell(index) {
                            continue;
                        }
                        let mut sum = 0.0;
                        let mut count = 0.0;
                        for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                            if let Some(neighbor) = cell(x as isize + dx, y as isize + dy, z as isize + dz) {
                                if fluid_cell(neighbor) {
                                    sum += pressure[neighbor];
                                    count += 1.0;
                                }
                            }
                        }
                        let divergence = (u[u_index(x + 1, y, z)] - u[u_index(x, y, z)]
                            + v[v_index(x, y + 1, z)] - v[v_index(x, y, z)]
                            + w[w_index(x, y, z + 1)] - w[w_index(x, y, z)]) / cell_size;
                        next_pressure[index] = if count > 0.0 {
                            let target = divergence * cell_size * cell_size / step_dt.max(1e-4);
                            (sum - target).clamp(-100.0, 100.0) / count
                        } else {
                            0.0
                        };
                    }
                }
            }
            std::mem::swap(&mut pressure, &mut next_pressure);
        }
        let pressure_gradient = step_dt / cell_size.max(1e-4);
        for z in 0..resolution {
            for y in 0..resolution {
                for x in 1..resolution {
                    let left = grid_index(x - 1, y, z);
                    let right = grid_index(x, y, z);
                    if fluid_cell(left) || fluid_cell(right) {
                        u[u_index(x, y, z)] -=
                            (pressure[right] - pressure[left]) * pressure_gradient;
                    }
                }
            }
        }
        for z in 0..resolution {
            for y in 1..resolution {
                for x in 0..resolution {
                    let below = grid_index(x, y - 1, z);
                    let above = grid_index(x, y, z);
                    if fluid_cell(below) || fluid_cell(above) {
                        v[v_index(x, y, z)] -=
                            (pressure[above] - pressure[below]) * pressure_gradient;
                    }
                }
            }
        }
        for z in 1..resolution {
            for y in 0..resolution {
                for x in 0..resolution {
                    let back = grid_index(x, y, z - 1);
                    let front = grid_index(x, y, z);
                    if fluid_cell(back) || fluid_cell(front) {
                        w[w_index(x, y, z)] -=
                            (pressure[front] - pressure[back]) * pressure_gradient;
                    }
                }
            }
        }
        for particle in particles.iter_mut() {
            let previous_position = particle.position;
            let x = ((particle.position.x - lo.x) / cell_size).floor().clamp(0.0, (resolution - 1) as f32) as usize;
            let y = ((particle.position.y - lo.y) / cell_size).floor().clamp(0.0, (resolution - 1) as f32) as usize;
            let z = ((particle.position.z - lo.z) / cell_size).floor().clamp(0.0, (resolution - 1) as f32) as usize;
            let grid_velocity_sample = Vec3::new(
                u[u_index(x, y, z)],
                v[v_index(x, y, z)],
                w[w_index(x, y, z)],
            );
            let previous_velocity = Vec3::new(
                old_u[u_index(x, y, z)],
                old_v[v_index(x, y, z)],
                old_w[w_index(x, y, z)],
            );
            let flip_velocity = particle.velocity.add(grid_velocity_sample.sub(previous_velocity));
            particle.velocity = grid_velocity_sample
                .scale(1.0 - blend)
                .add(flip_velocity.scale(blend));
            let max_speed = (1.5 * cell_size / step_dt.max(1e-4)).max(1.0);
            particle.velocity.x = particle.velocity.x.clamp(-max_speed, max_speed);
            particle.velocity.y = particle.velocity.y.clamp(-max_speed, max_speed);
            particle.velocity.z = particle.velocity.z.clamp(-max_speed, max_speed);
            particle.velocity = particle.velocity.scale((1.0 - viscosity * step_dt).clamp(0.0, 1.0));
            particle.position = particle.position.add(particle.velocity.scale(step_dt));
            if particle.position.y < lo.y {
                particle.position.y = lo.y;
                particle.velocity.y = particle.velocity.y.abs() * 0.15;
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

fn node_extrude(mesh: Mesh, dist: f32, inset: f32, mode: usize) -> Mesh {
    if mesh.indices.is_empty() {
        return mesh;
    }
    let inset_scale = (1.0 - inset).max(0.0);
    let mut out = Mesh::default();
    let mut edge_counts = HashMap::<(usize, usize), usize>::new();
    for tri in &mesh.indices {
        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let edge = if a < b { (a, b) } else { (b, a) };
            *edge_counts.entry(edge).or_default() += 1;
        }
    }
    if mode == 1 {
        let mut normals = vec![Vec3::zero(); mesh.vertices.len()];
        for tri in &mesh.indices {
            let n = mesh.vertices[tri[1]]
                .sub(mesh.vertices[tri[0]])
                .cross(mesh.vertices[tri[2]].sub(mesh.vertices[tri[0]]));
            for &vertex in tri {
                normals[vertex] = normals[vertex].add(n);
            }
        }
        let center = mesh.bounds().map(|(lo, hi)| lo.add(hi).scale(0.5)).unwrap_or(Vec3::zero());
        let offset: Vec<Vec3> = mesh.vertices.iter().enumerate().map(|(i, &vertex)| {
            center
                .add(vertex.sub(center).scale(inset_scale))
                .add(normals[i].normalize().scale(dist))
        }).collect();
        let base = mesh.vertices.len();
        out.vertices.extend_from_slice(&mesh.vertices);
        out.vertices.extend_from_slice(&offset);
        for tri in &mesh.indices {
            out.indices.push([tri[2], tri[1], tri[0]]);
            out.indices.push([tri[0] + base, tri[1] + base, tri[2] + base]);
        }
        for tri in &mesh.indices {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let edge = if a < b { (a, b) } else { (b, a) };
                if edge_counts.get(&edge) == Some(&1) {
                    out.indices.push([a, b, b + base]);
                    out.indices.push([a, b + base, a + base]);
                }
            }
        }
        return out;
    }
    for tri in &mesh.indices {
        let v0 = mesh.vertices[tri[0]];
        let v1 = mesh.vertices[tri[1]];
        let v2 = mesh.vertices[tri[2]];
        let n = v1.sub(v0).cross(v2.sub(v0)).normalize();
        let centroid = v0.add(v1).add(v2).scale(1.0 / 3.0);

        let inset_scale = (1.0 - inset).max(0.0);
        let e0 = centroid.add(v0.sub(centroid).scale(inset_scale)).add(n.scale(dist));
        let e1 = centroid.add(v1.sub(centroid).scale(inset_scale)).add(n.scale(dist));
        let e2 = centroid.add(v2.sub(centroid).scale(inset_scale)).add(n.scale(dist));

        let base = out.vertices.len();
        out.vertices.extend_from_slice(&[v0, v1, v2, e0, e1, e2]);

        // Preserve the original surface and add the offset cap.
        out.indices.push([base + 2, base + 1, base]);
        out.indices.push([base + 3, base + 4, base + 5]);

        // Side quads (2 triangles each)
        out.indices.push([base + 0, base + 1, base + 4]);
        out.indices.push([base + 0, base + 4, base + 3]);

        out.indices.push([base + 1, base + 2, base + 5]);
        out.indices.push([base + 1, base + 5, base + 4]);

        out.indices.push([base + 2, base + 0, base + 3]);
        out.indices.push([base + 2, base + 3, base + 5]);
    }
    node_fuse(out, 1e-5)
}

fn node_clip(mesh: Mesh, axis: usize, offset: f32, invert: bool) -> Mesh {
    let mut out = Mesh {
        vertices: mesh.vertices.clone(),
        indices: Vec::new(),
        point_attributes: mesh.point_attributes.clone(),
    };
    for tri in &mesh.indices {
        let v0 = mesh.vertices[tri[0]];
        let v1 = mesh.vertices[tri[1]];
        let v2 = mesh.vertices[tri[2]];

        let keep = |v: Vec3| {
            let val = axis_of(v, axis);
            if invert { val <= offset } else { val >= offset }
        };

        if keep(v0) && keep(v1) && keep(v2) {
            out.indices.push(*tri);
        }
    }
    compact(out)
}

fn node_blast(mesh: Mesh, component: usize, indices: &[usize], invert: bool) -> Mesh {
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
            let mut out = Mesh {
                vertices: mesh.vertices,
                indices: mesh.indices
                    .into_iter()
                    .filter(|tri| !tri.iter().any(|index| remove_vertex.contains(index)))
                    .collect(),
                point_attributes: mesh.point_attributes,
            };
            compact(out)
        }
        1 => compact(Mesh {
            vertices: mesh.vertices,
            indices: mesh.indices
                .into_iter()
                .enumerate()
                .filter_map(|(index, tri)| (!selected(index)).then_some(tri))
                .collect(),
            point_attributes: mesh.point_attributes,
        }),
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
            compact(Mesh {
                vertices: mesh.vertices,
                indices: mesh.indices
                    .into_iter()
                    .filter(|tri| {
                        ![(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                            .into_iter()
                            .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
                            .any(|edge| remove_edges.contains(&edge))
                    })
                    .collect(),
                point_attributes: mesh.point_attributes,
            })
        }
    }
}

/// Dependency-free sparse voxel surface extraction. This mirrors the useful
/// part of a VDB workflow while keeping the app portable: occupied cells are
/// stored sparsely and only exposed voxel faces are emitted.
fn node_vdb(mesh: Mesh, voxel_size: f32, padding: usize, operation: usize, iterations: usize) -> Mesh {
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
fn node_vdb_native(
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
fn native_volume_from_mesh(mesh: &Mesh, voxel_size: f32, padding: usize) -> Option<NativeVolume> {
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
fn native_volume_to_mesh(volume: &NativeVolume) -> Option<Mesh> {
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

// ============================================================================
// 3. NODE TYPES
// ============================================================================

type NodeId = usize;

fn prim_path_ui(ui: &mut egui::Ui, label: &str, path: &mut String) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        let response = ui.add(egui::TextEdit::singleline(path).desired_width(200.0));
        if response.lost_focus() {
            *path = usd::sanitize_path(path);
        }
        response.changed()
    })
    .inner
}

// Prim pattern field with a button that fills in the prim picked in the scene graph.
fn prim_pattern_ui(ui: &mut egui::Ui, pattern: &mut String, picked: &Option<String>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Primitives");
        changed |= ui.add(egui::TextEdit::singleline(pattern).hint_text("/geo/* /lights/**").desired_width(190.0)).changed();
        if ui.add_enabled(picked.is_some(), egui::Button::new("⇐ picked")).on_hover_text("Use the prim selected in the scene graph").clicked() {
            *pattern = picked.clone().unwrap_or_default();
            changed = true;
        }
    });
    ui.weak("Paths with * (one level) and ** (any depth); ^ excludes.");
    changed
}

fn render_camera_combo(ui: &mut egui::Ui, choice: &mut RenderCamera, cameras: &[NodeId]) {
    let label = match *choice {
        RenderCamera::Auto => "Auto (selected camera, else viewport)".to_string(),
        RenderCamera::Viewport => "Viewport".to_string(),
        RenderCamera::Node(id) => format!("camera{id}"),
    };
    egui::ComboBox::from_id_salt("render_camera")
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.selectable_value(choice, RenderCamera::Auto, "Auto (selected camera, else viewport)");
            ui.selectable_value(choice, RenderCamera::Viewport, "Viewport");
            for id in cameras {
                ui.selectable_value(choice, RenderCamera::Node(*id), format!("camera{id}"));
            }
        });
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RenderCamera {
    // Selected camera node if any, otherwise the viewport.
    Auto,
    Viewport,
    Node(NodeId),
}

struct CamRig {
    target: Vec3,
    rot_x: f32,
    rot_y: f32,
    zoom: f32,
    focal: f32,
    orthographic: bool,
    pan: [f32; 2],
}

#[derive(Debug, Clone, PartialEq)]
struct RenderOpts {
    camera: RenderCamera,
    max_depth: usize,
    light_samples: usize,
    bsdf_samples: usize,
    background: [f32; 3],
    // Timeline range rendered frame by frame when `sequence` is on.
    sequence: bool,
    range_start: f32,
    range_end: f32,
    range_step: f32,
    // Render the STAGE network's displayed USD stage instead of the OBJ objects.
    stage: bool,
    // In-scattering light samples per volume step: the main cost of smoke and fire renders.
    volume_samples: usize,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            camera: RenderCamera::Auto,
            max_depth: 5,
            light_samples: 2,
            bsdf_samples: 2,
            background: [0.0; 3],
            sequence: false,
            range_start: 0.0,
            range_end: 1.0,
            range_step: 0.1,
            stage: false,
            volume_samples: 2,
        }
    }
}

// Timeline times covered by a frame range, capped to keep runaway ranges bounded.
fn frame_times(start: f32, end: f32, step: f32) -> Vec<f32> {
    let step = step.max(1e-4);
    let count = (((end - start) / step) + 1e-4).floor().max(0.0) as usize + 1;
    (0..count.min(10_000)).map(|i| start + i as f32 * step).collect()
}

// Inserts a frame number into an output path: `name.####.exr` style, or `name.0001.exr` when no `#` is present.
fn frame_output(output: &str, frame: usize) -> String {
    if let Some(start) = output.find('#') {
        let width = output[start..].chars().take_while(|c| *c == '#').count();
        return format!("{}{:0width$}{}", &output[..start], frame, &output[start + width..], width = width);
    }
    let path = std::path::Path::new(output);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
    let name = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.{frame:04}.{ext}"),
        None => format!("{stem}.{frame:04}"),
    };
    path.with_file_name(name).to_string_lossy().into_owned()
}

// MoonRay inside the running app bundle (Contents/Resources or Contents/MacOS), if any.
fn bundled_moonray() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    // A Linux install (e.g. the Flatpak) puts it next to the app in <prefix>/bin.
    [contents.join("Resources").join("moonray"), contents.join("MacOS").join("moonray"), contents.join("bin").join("moonray")]
        .into_iter()
        .find(|p| p.exists())
}

// Detect a bundled or system MoonRay executable in common locations. For macOS
// packaging the binary lives in the app bundle; during development it can be
// placed in `native/moonray/macos/moonray` before packaging.
fn detect_moonray_executable() -> String {
    if let Some(bundled) = bundled_moonray() {
        return bundled.to_string_lossy().into_owned();
    }
    let candidates = [
        "./moonray",
        "./Resources/moonray",
        "./Contents/Resources/moonray",
        "./Contents/MacOS/moonray",
        "./native/moonray/macos/moonray",
        "/usr/local/bin/moonray",
        "/opt/homebrew/bin/moonray",
    ];
    candidates
        .into_iter()
        .find(|c| std::path::Path::new(c).exists())
        .map(str::to_owned)
        .or_else(|| resolve_executable("moonray").map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "./moonray".into())
}

// The file a render would actually run for `executable`, resolved the way
// spawn_moonray_render does: an existing path, the bundled copy, then PATH.
fn resolve_executable(executable: &str) -> Option<PathBuf> {
    let path = PathBuf::from(executable.trim());
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.exists() {
        return Some(path);
    }
    if let Some(bundled) = bundled_moonray() {
        return Some(bundled);
    }
    if path.components().count() == 1 {
        let paths = std::env::var_os("PATH")?;
        return std::env::split_paths(&paths).map(|dir| dir.join(&path)).find(|p| p.is_file());
    }
    None
}

// Sliders for the ch("name") parameters `code` uses (like Houdini's spare
// parameters, created automatically); values unused by the code are dropped.
fn channel_params_ui(ui: &mut egui::Ui, code: &str, channels: &mut vex::Channels) -> bool {
    let refs = vex::channel_refs(code);
    let mut changed = false;
    let before = channels.len();
    channels.retain(|name, _| refs.iter().any(|(n, _)| n == name));
    changed |= channels.len() != before;
    if refs.is_empty() {
        return changed;
    }
    ui.separator();
    ui.strong("Parameters");
    for (name, kind) in refs {
        let value = channels.entry(name.clone()).or_insert([0.0; 3]);
        ui.horizontal(|ui| {
            ui.label(&name);
            match kind {
                'i' => {
                    let mut v = value[0].round() as i32;
                    if ui.add(egui::DragValue::new(&mut v).speed(0.1)).changed() {
                        value[0] = v as f32;
                        changed = true;
                    }
                }
                'v' => {
                    for c in value.iter_mut() {
                        changed |= ui.add(egui::DragValue::new(c).speed(0.01)).changed();
                    }
                }
                _ => {
                    changed |= ui.add(egui::Slider::new(&mut value[0], 0.0..=1.0).clamping(egui::SliderClamping::Never)).changed();
                }
            }
        });
    }
    ui.small("From ch(\"name\") / chi / chv in the code. Drag past the slider's end for larger values.");
    changed
}

// The window and Dock icon, from packaging/logo.
fn app_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../packaging/logo/icon-256.png")).expect("bundled icon").to_rgba8();
    egui::IconData { width: image.width(), height: image.height(), rgba: image.into_raw() }
}

// The image a UV Quick Shade file shows ("" = the built-in checker), loaded once.
fn quickshade_texture(file: &str) -> Arc<raster::Texture> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<raster::Texture>>>> = std::sync::OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(file.to_owned())
        .or_insert_with(|| {
            let loaded = (!file.is_empty()).then(|| image::open(file).ok()).flatten().map(|img| {
                let img = img.thumbnail(1024, 1024).to_rgba8();
                raster::Texture { width: img.width() as usize, height: img.height() as usize, pixels: img.pixels().map(|p| p.0).collect() }
            });
            Arc::new(loaded.unwrap_or_else(|| {
                let size = 256;
                raster::Texture { width: size, height: size, pixels: sops::checker_image(size) }
            }))
        })
        .clone()
}

// The viewport's rasterized surfaces, uploaded into one reused texture.
fn viewport_texture(ctx: &egui::Context, image: egui::ColorImage) -> egui::TextureHandle {
    static TEXTURE: std::sync::OnceLock<std::sync::Mutex<Option<egui::TextureHandle>>> = std::sync::OnceLock::new();
    let mut slot = TEXTURE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_mut() {
        Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
        None => *slot = Some(ctx.load_texture("viewport-surfaces", image, egui::TextureOptions::LINEAR)),
    }
    slot.clone().expect("set above")
}

// Per-user settings folder (recent files, updater preferences).
pub(crate) fn app_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        Some(home?.join("Library/Application Support/ProceduralFX Studio"))
    } else {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config")))?;
        Some(base.join("proceduralfx-studio"))
    }
}

const MAX_RECENT_SCENES: usize = 10;

fn recent_scenes_file() -> Option<PathBuf> {
    Some(app_config_dir()?.join("recent_scenes.txt"))
}

fn load_recent_scenes() -> Vec<PathBuf> {
    recent_scenes_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| parse_recent_scenes(&text))
        .unwrap_or_default()
}

fn parse_recent_scenes(text: &str) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = PathBuf::from(line);
        if !list.contains(&path) && list.len() < MAX_RECENT_SCENES {
            list.push(path);
        }
    }
    list
}

// Moves `path` to the front of the recent list.
fn push_recent_scene(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(MAX_RECENT_SCENES);
}

fn save_recent_scenes(list: &[PathBuf]) {
    let Some(file) = recent_scenes_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = std::fs::write(file, text);
}

// Writes via a temporary file so a failed save never leaves a half-written scene.
fn write_file_atomically(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".saving");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

const SHORTCUT_OPEN: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::O);
const SHORTCUT_SAVE: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
const SHORTCUT_SAVE_AS: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT), egui::Key::S);

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

// Runs `cmd` with stdout and stderr written to `log_path`, killing it once `cancel` is set.
fn run_logged(cmd: &mut Command, log_path: &std::path::Path, cancel: &AtomicBool) -> Result<std::process::ExitStatus, String> {
    let log = File::create(log_path).map_err(|e| format!("Could not create render log: {e}"))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let mut child = cmd.stdout(log).stderr(log_err).spawn().map_err(|e| e.to_string())?;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status);
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Render cancelled".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// Bucket rectangles (x0, y0, x1, y1; exclusive, top-down) ordered from the centre outward.
fn bucket_layout(width: usize, height: usize) -> Vec<[usize; 4]> {
    let tile = (((width.max(height) as f32 / 6.0).ceil() as usize).div_ceil(16) * 16).clamp(32, 256);
    let mut buckets = Vec::new();
    for y in (0..height).step_by(tile) {
        for x in (0..width).step_by(tile) {
            buckets.push([x, y, (x + tile).min(width), (y + tile).min(height)]);
        }
    }
    let centre = (width as f32 * 0.5, height as f32 * 0.5);
    let distance = |b: &[usize; 4]| {
        let (cx, cy) = ((b[0] + b[2]) as f32 * 0.5, (b[1] + b[3]) as f32 * 0.5);
        (cx - centre.0).powi(2) + (cy - centre.1).powi(2)
    };
    buckets.sort_by(|a, b| distance(a).partial_cmp(&distance(b)).unwrap_or(std::cmp::Ordering::Equal));
    buckets
}

// Renders the frame bucket by bucket with MoonRay's -sub_viewport, two processes at a time,
// streaming finished buckets into `progress`, then writes the assembled image to `output`.
#[allow(clippy::too_many_arguments)]
fn render_buckets(
    make_cmd: &(dyn Fn(&[String]) -> Command + Sync),
    args: &[String],
    output: &str,
    width: usize,
    height: usize,
    log_path: &std::path::Path,
    cancel: &AtomicBool,
    progress: &render_viewer::SharedProgress,
) -> Result<(), String> {
    let buckets = bucket_layout(width, height);
    if let Ok(mut state) = progress.lock() {
        state.total = buckets.len();
    }
    let workers = buckets.len().clamp(1, 2);
    let threads = (std::thread::available_parallelism().map_or(4, |n| n.get()) / workers).max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failure: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let _ = std::fs::write(log_path, "");
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) || failure.lock().map_or(true, |f| f.is_some()) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(&bucket) = buckets.get(index) else { break };
                let [x0, y0, x1, y1] = bucket;
                if let Ok(mut state) = progress.lock() {
                    state.active.push(bucket);
                    state.version += 1;
                }
                let tmp = std::env::temp_dir();
                let bucket_out = tmp.join(format!("proceduralfx_bucket_{stamp}_{index}.exr"));
                let bucket_log = tmp.join(format!("proceduralfx_bucket_{stamp}_{index}.log"));
                let mut bucket_args: Vec<String> = args.to_vec();
                match bucket_args.iter().position(|a| a == "-out") {
                    Some(at) if at + 1 < bucket_args.len() => bucket_args[at + 1] = bucket_out.to_string_lossy().into_owned(),
                    _ => bucket_args.extend(["-out".to_string(), bucket_out.to_string_lossy().into_owned()]),
                }
                // MoonRay's region is left/bottom inclusive, right/top exclusive, rows counted from the bottom.
                bucket_args.extend(
                    ["-sub_viewport".to_string(), x0.to_string(), (height - y1).to_string(), x1.to_string(), (height - y0).to_string()],
                );
                bucket_args.extend(["-threads".to_string(), threads.to_string()]);
                let status = run_logged(&mut make_cmd(&bucket_args), &bucket_log, cancel);
                if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
                    let _ = writeln!(log, "--- bucket {} of {} ({x0},{y0})-({x1},{y1}) ---", index + 1, buckets.len());
                    let _ = log.write_all(&std::fs::read(&bucket_log).unwrap_or_default());
                }
                let result = match status {
                    Ok(s) if s.success() => image::open(&bucket_out).map(|img| img.to_rgba32f()).map_err(|e| format!("could not read bucket: {e}")),
                    Ok(s) => Err(format!("MoonRay exited with {s}: {}", read_log_tail(&bucket_log, 600).trim())),
                    Err(e) => Err(e),
                };
                let _ = std::fs::remove_file(&bucket_out);
                let _ = std::fs::remove_file(&bucket_log);
                if let Ok(mut state) = progress.lock() {
                    if let Ok(img) = &result {
                        for y in y0..y1.min(img.height() as usize) {
                            for x in x0..x1.min(img.width() as usize) {
                                state.pixels[y * width + x] = img.get_pixel(x as u32, y as u32).0;
                            }
                        }
                    }
                    state.active.retain(|b| *b != bucket);
                    state.done += 1;
                    state.version += 1;
                }
                if let Err(e) = result {
                    if let Ok(mut f) = failure.lock() {
                        f.get_or_insert(e);
                    }
                }
            });
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return Err("Render cancelled".into());
    }
    if let Some(error) = failure.lock().ok().and_then(|mut f| f.take()) {
        return Err(error);
    }
    let pixels = progress.lock().map_err(|_| "render state poisoned".to_string())?.pixels.clone();
    write_render_image(&pixels, width, height, output)
}

// Writes linear RGBA pixels: EXR as floats, other formats sRGB-encoded 8-bit.
fn write_render_image(pixels: &[[f32; 4]], width: usize, height: usize, output: &str) -> Result<(), String> {
    let (w, h) = (width as u32, height as u32);
    if fileio::extension(output) == "exr" {
        let raw: Vec<f32> = pixels.iter().flatten().copied().collect();
        let buffer = image::Rgba32FImage::from_raw(w, h, raw).ok_or("image size mismatch")?;
        return image::DynamicImage::ImageRgba32F(buffer).save(output).map_err(|e| e.to_string());
    }
    let encode = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0 + 0.5) as u8
    };
    let raw: Vec<u8> = pixels.iter().flat_map(|p| [encode(p[0]), encode(p[1]), encode(p[2]), (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8]).collect();
    let buffer = image::RgbaImage::from_raw(w, h, raw).ok_or("image size mismatch")?;
    image::DynamicImage::ImageRgba8(buffer).to_rgb8().save(output).map_err(|e| e.to_string())
}

// Last `max` bytes of a render log.
fn read_log_tail(path: &std::path::Path, max: usize) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

// What the render worker thread reports back: a status message and the finished image, plus its log.
struct RenderOutcome {
    result: Result<(String, PathBuf), String>,
    log: String,
}

// (name, samples per pixel, max depth, light samples, BSDF samples)
const QUALITY_PRESETS: [(&str, usize, usize, usize, usize); 4] = [
    ("Draft", 4, 2, 1, 1),
    ("Preview", 16, 4, 1, 1),
    ("Production", 64, 5, 2, 2),
    ("Final", 256, 8, 4, 4),
];

const RESOLUTION_PRESETS: [(&str, usize, usize); 7] = [
    ("HD 720p", 1280, 720),
    ("Full HD 1080p", 1920, 1080),
    ("2K DCI", 2048, 1080),
    ("UHD 4K", 3840, 2160),
    ("Square 1K", 1024, 1024),
    ("Square 2K", 2048, 2048),
    ("Portrait 4:5", 1080, 1350),
];

// How a render part is shaded: a MAT network node, a USD preview surface, or the default.
#[derive(Clone, Debug)]
enum PartMaterial {
    Default,
    Node(NodeId),
    Preview { diffuse: [f32; 3], roughness: f32, metallic: f32, opacity: f32, emissive: [f32; 3], ior: f32 },
}

// Everything a MoonRay render needs, from the OBJ objects or from the USD stage.
struct RenderScene {
    parts: Vec<(Mesh, PartMaterial)>,
    // (position, travel direction, color, intensity, kind) as scene_lights().
    lights: Vec<(Vec3, Vec3, [f32; 3], f32, usize)>,
    // Dome light colour, added to the environment.
    environment: Option<[f32; 3]>,
    // Stage camera in app space: eye, right, up, forward, focal length and horizontal aperture (mm).
    camera: Option<(Vec3, Vec3, Vec3, Vec3, f32, f32)>,
    // Fog volumes written to .vdb files for MoonRay.
    volumes: Vec<RenderVolume>,
}

#[derive(Clone, Debug)]
struct RenderVolume {
    file: String,
    // Extinction per world unit for density 1, albedo, and emission scale (None: no emission grid).
    extinction: f32,
    albedo: [f32; 3],
    emission: Option<f32>,
    // Pyro Shader settings: emission colour multiplier and scattering anisotropy.
    emission_tint: [f32; 3],
    anisotropy: f32,
}

struct SequenceJob {
    executable: String,
    arguments: String,
    output: String,
    width: usize,
    height: usize,
    samples: usize,
    opts: RenderOpts,
    times: Vec<f32>,
    next: usize,
}

const MATERIAL_NAMES: [&str; 4] = ["diffuse", "metal", "glass", "emissive"];
// USD gprim types made by the Primitive LOP, and light types made by the Light LOP.
const LOP_SHAPES: [&str; 5] = ["Sphere", "Cube", "Cylinder", "Cone", "Capsule"];
const LOP_LIGHTS: [&str; 4] = ["DistantLight", "SphereLight", "DomeLight", "RectLight"];
// Key of the STAGE network's display flag in Graph::displays (top-level LOPs have no container).
const STAGE_NETWORK: NodeId = usize::MAX;

#[derive(Clone, Debug)]
enum NodeKind {
    Add { count: usize, spacing: f32, layout: usize },
    Grid { size: f32, res: usize },
    Plane { size: f32, res: usize },
    Cone { radius: f32, height: f32, rows: usize, cols: usize, caps: bool },
    Capsule { radius: f32, length: f32, rows: usize, cols: usize },
    Box { size: f32, div: usize },
    Sphere { radius: f32, rows: usize, cols: usize },
    Torus { major: f32, minor: f32, rows: usize, cols: usize },
    Transform { t: [f32; 3], r: [f32; 3], s: [f32; 3] },
    Noise { freq: f32, amp: f32 },
    Taper { factor: f32 },
    Twist { angle: f32 },
    Bend { angle: f32 },
    Smooth { iterations: usize, strength: f32 },
    Bevel { amount: f32, segments: usize },
    Fuse { tol: f32 },
    Subdivide { levels: usize },
    Remesh { target: usize },
    Mirror { axis: usize },
    Copy { count: usize, t: [f32; 3], r: [f32; 3], s: f32 },
    CopyToPoints { scale: f32, rotation: [f32; 3] },
    AttributeCreate { attribute: usize, value: f32 },
    AttributeRandomize { attribute: usize, amount: f32, seed: usize },
    AttributeDelete { attribute: usize },
    Wrangle { code: String, domain: usize },
    Merge,
    Null,
    Tube { r_bot: f32, r_top: f32, height: f32, rows: usize, cols: usize, caps: bool },
    Peak { dist: f32 },
    Ripple { freq: f32, amp: f32, speed: f32 },
    Jitter { amp: f32, seed: usize },
    // Point particles. Input 1: optional source geometry (emits from its
    // surface or points; the origin when unconnected), input 2: optional static collider.
    Particles {
        count: usize,
        emission_rate: f32,
        lifetime: f32,
        speed: f32,
        spread: f32,
        gravity: f32,
        seed: usize,
        drag: f32,
        bounce: f32,
        friction: f32,
        radius: f32,
        substeps: usize,
        along_normals: bool,
    },
    Flip {
        particle_count: usize,
        grid_resolution: usize,
        flip_ratio: f32,
        viscosity: f32,
        gravity: f32,
        max_dt: f32,
        substeps: usize,
        seed: usize,
    },
    // Spray, foam and bubbles from a FLIP fluid. Input 1: fluid particles (with v), input 2: optional collider.
    WhitewaterSolver(whitewater::WhitewaterSettings),
    ParticleSurface { radius: f32, join_distance: f32, resolution: usize, smoothing: usize },
    Extrude { dist: f32, inset: f32, mode: usize },
    Clip { axis: usize, offset: f32, invert: bool },
    Blast { component: usize, indices: Vec<usize>, invert: bool },
    Cache { cache_id: usize, mode: usize },
    // VDB from Polygons. output: 0 polygons (the surface directly), 1 VDB (a level set volume).
    Vdb { voxel_size: f32, padding: usize, operation: usize, iterations: usize, output: usize },
    // Converts a VDB or volume (VDB, Pyro, .vdb files). mode: 0 polygons at `iso`, 1 fog volume.
    ConvertVdb { mode: usize, iso: f32 },
    // Reads geometry from disk: .obj, .fbx, .stl, .usd/.usda/.usdc/.usdz and .vdb; $F is the frame.
    FileImport { file: String },
    // Passes its input through and writes it to disk on demand (same formats; $F for sequences).
    FileExport { file: String },
    MoonRayRender {
        executable: String,
        arguments: String,
        output: String,
        width: usize,
        height: usize,
        samples: usize,
        opts: RenderOpts,
    },
    Camera {
        position: [f32; 3],
        rotation: [f32; 3],
        focal: f32,
        near: f32,
        orthographic: bool,
        pan: [f32; 2],
        zoom: f32,
    },
    Light {
        position: [f32; 3],
        rotation: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        kind: usize,
    },
    Reverse,
    Switch { index: usize },
    // Smoke and fire on a voxel grid. Input 1: source geometry, input 2: optional static collider.
    Pyro(sims::PyroSettings),
    // Groom. Hair Guides: input 1 is the skin mesh.
    HairGuides(groom::GuideSettings),
    // Shapes guide (or hair) curves from input 1.
    GuideProcess(groom::ProcessSettings),
    // Dense hair. Input 1: skin, input 2: guides.
    HairGenerate(groom::HairSettings),
    // Moves hair with an animated skin. Input 1: hair or guides, input 2: skin.
    HairDeform(groom::DeformSettings),
    // Rigid bodies (rapier, Bullet-style). Input 1: pieces, input 2: optional static collider.
    RbdSolver(sims::RbdSettings),
    // XPBD cloth, hair, softbody or grains. Input 1: geometry, input 2: optional static collider.
    VellumSolver(vellum::VellumSettings),
    // Writes N on points or vertices.
    Normal(sops::NormalSettings),
    // Points on (or inside) the input's surface.
    Scatter(sops::ScatterSettings),
    // Union / intersection / subtraction of two closed meshes.
    Boolean(boolean::BooleanSettings),
    // Breaks a closed mesh into concrete, glass or wood pieces (with `piece` for RBD).
    MaterialFracture(fracture::FractureSettings),
    // Agents walking with steering. Inputs: start points, optional ground, optional obstacles.
    CrowdSolver(crowd::CrowdSettings),
    // A muscle body from origin to insertion (or along input curve), with animatable activation.
    Muscle(muscle::MuscleSettings),
    // Jiggle (secondary motion) on animated muscles; tendon tips stay pinned.
    MuscleSolver(muscle::JiggleSettings),
    // Moves skin (input 1) with the muscles under it (input 2) relative to frame 0.
    MuscleSkin(muscle::SkinSettings),
    // Heightfield terrain: a grid plus nodes that shape its heights.
    Terrain(terrain::TerrainSettings),
    TerrainNoise(terrain::NoiseSettings),
    TerrainErode(terrain::ErodeSettings),
    TerrainTerrace(terrain::TerraceSettings),
    TerrainMask(terrain::MaskSettings),
    TerrainColorize(terrain::ColorizeSettings),
    // UV projection, transform and viewport texture display.
    UvTexture(sops::UvTextureSettings),
    UvTransform(sops::UvTransformSettings),
    UvQuickShade { file: String },
    // Object-level HDRI environment light (MoonRay EnvLight); also lights and backs the viewport.
    EnvironmentLight { file: String, color: [f32; 3], intensity: f32, rotation: f32, visible: bool },
    // Makes a named point or primitive group.
    GroupCreate(groups::GroupCreateSettings),
    // Node-based VEX: a container of VOP nodes compiled to VEX and run over `domain` (like Wrangle).
    AttributeVop { domain: usize },
    // One operation inside an Attribute VOP network.
    Vop(vop::VopNode),
    // A float point attribute painted in the viewport (one stored value per input point).
    AttributePaint { attribute: String, values: Vec<f32>, settings: paint::PaintSettings },
    // Container whose network is cooked once per step on the previous step's output (Prev_Frame).
    SopSolver { substeps: usize },
    // Inside a SOP Solver network: 0 = Prev_Frame, 1 = Input_1 (the solver's input at the step time).
    SolverInput { index: usize },
    // STAGE (LOP) nodes: each builds on the USD stage from its first input.
    // Imports an OBJ geometry object (None: all of them) as Mesh prims under `path`.
    LopSopImport { geometry: Option<NodeId>, path: String },
    // References a USD file under `path`, or sublayers it at the root.
    LopReference { file: String, path: String, sublayer: bool },
    // A USD gprim; shape indexes LOP_SHAPES.
    LopPrimitive { shape: usize, path: String, size: f32, t: [f32; 3] },
    // Applies a transform to prims matching `pattern`, in their parent's space.
    LopTransform { pattern: String, t: [f32; 3], r: [f32; 3], s: [f32; 3] },
    // A UsdLux light aimed from `position` at `target`; kind indexes LOP_LIGHTS.
    LopLight { kind: usize, path: String, position: [f32; 3], target: [f32; 3], color: [f32; 3], intensity: f32, size: f32 },
    LopCamera { path: String, position: [f32; 3], target: [f32; 3], focal: f32, aperture: f32 },
    // Binds a MAT network material (as a UsdPreviewSurface) to prims matching `pattern`.
    LopMaterial { pattern: String, material: Option<NodeId> },
    // Deactivates (or only hides) prims matching `pattern`.
    LopPrune { pattern: String, hide: bool },
    LopMerge,
    // Writes its input stage to a USD file (format from the extension).
    LopExport { file: String },
    // OBJ-level container; its SOP network lives in nodes whose parent is this node.
    Geometry { material: Option<NodeId> },
    // MAT-level MoonRay material; kind indexes MATERIAL_NAMES. `value` is specular (diffuse), IOR (glass) or intensity (emissive).
    Material { kind: usize, color: [f32; 3], roughness: f32, value: f32 },
    // MAT shaders: Principled, Toon, Hair or Pyro (volume), with presets.
    Shader(shading::Values),
    // MAT container of shader nodes wired into a Surface Output.
    MaterialBuilder,
    // One node inside a Material Builder.
    ShaderNode(shading::ShaderNode),
}

#[derive(Clone, Copy, PartialEq)]
enum GroupSupport {
    None,
    // Cooks on everything, then restores what changed outside the group.
    Restrict,
    // The node uses the group itself (wrangle, blast, scatter, groom roots).
    Native,
    // Vellum: the group's points are pinned.
    Pin,
}

impl NodeKind {
    fn group_support(&self) -> GroupSupport {
        match self {
            NodeKind::Transform { .. }
            | NodeKind::Noise { .. }
            | NodeKind::Taper { .. }
            | NodeKind::Twist { .. }
            | NodeKind::Bend { .. }
            | NodeKind::Smooth { .. }
            | NodeKind::Peak { .. }
            | NodeKind::Ripple { .. }
            | NodeKind::Jitter { .. }
            | NodeKind::AttributeCreate { .. }
            | NodeKind::AttributeRandomize { .. }
            | NodeKind::Normal(_)
            | NodeKind::UvTexture(_)
            | NodeKind::UvTransform(_)
            | NodeKind::AttributePaint { .. }
            | NodeKind::GuideProcess(_)
            | NodeKind::HairDeform(_) => GroupSupport::Restrict,
            NodeKind::Wrangle { .. }
            | NodeKind::AttributeVop { .. }
            | NodeKind::Blast { .. }
            | NodeKind::Scatter(_)
            | NodeKind::HairGuides(_)
            | NodeKind::HairGenerate(_) => GroupSupport::Native,
            NodeKind::VellumSolver(_) => GroupSupport::Pin,
            _ => GroupSupport::None,
        }
    }

    fn is_object(&self) -> bool {
        matches!(self, NodeKind::Geometry { .. } | NodeKind::Camera { .. } | NodeKind::Light { .. } | NodeKind::EnvironmentLight { .. })
    }

    fn is_material(&self) -> bool {
        matches!(self, NodeKind::Material { .. } | NodeKind::Shader(_) | NodeKind::MaterialBuilder)
    }

    fn is_render(&self) -> bool {
        matches!(self, NodeKind::MoonRayRender { .. })
    }

    fn is_lop(&self) -> bool {
        matches!(
            self,
            NodeKind::LopSopImport { .. }
                | NodeKind::LopReference { .. }
                | NodeKind::LopPrimitive { .. }
                | NodeKind::LopTransform { .. }
                | NodeKind::LopLight { .. }
                | NodeKind::LopCamera { .. }
                | NodeKind::LopMaterial { .. }
                | NodeKind::LopPrune { .. }
                | NodeKind::LopMerge
                | NodeKind::LopExport { .. }
        )
    }

    // Parameters that can be keyframed, with their channel names.
    fn animatable(&self) -> Vec<(ChannelTarget, String)> {
        let named = |list: &[(usize, &str)]| list.iter().map(|(i, n)| (ChannelTarget::Param(*i), n.to_string())).collect();
        match self {
            NodeKind::Geometry { .. } => XFORM_NAMES.iter().enumerate().map(|(i, n)| (ChannelTarget::Xform(i), n.to_string())).collect(),
            NodeKind::Light { .. } => named(&[
                (0, "tx"), (1, "ty"), (2, "tz"), (3, "rx"), (4, "ry"), (5, "rz"),
                (6, "color.r"), (7, "color.g"), (8, "color.b"), (9, "intensity"),
            ]),
            NodeKind::EnvironmentLight { .. } => named(&[(0, "color.r"), (1, "color.g"), (2, "color.b"), (3, "intensity"), (4, "ry")]),
            NodeKind::Camera { .. } => named(&[
                (0, "tx"), (1, "ty"), (2, "tz"), (3, "rx"), (4, "ry"), (5, "rz"),
                (6, "focal"), (7, "near"), (9, "pan.x"), (10, "pan.y"), (11, "zoom"),
            ]),
            NodeKind::Transform { .. } => XFORM_NAMES.iter().enumerate().map(|(i, n)| (ChannelTarget::Param(i), n.to_string())).collect(),
            NodeKind::Material { .. } => named(&[(1, "color.r"), (2, "color.g"), (3, "color.b"), (4, "roughness"), (5, "value")]),
            NodeKind::Muscle(_) => named(&[
                (0, "origin.x"),
                (1, "origin.y"),
                (2, "origin.z"),
                (3, "insertion.x"),
                (4, "insertion.y"),
                (5, "insertion.z"),
                (6, "radius"),
                (10, "activation"),
            ]),
            NodeKind::CrowdSolver(_) => named(&[(0, "speed"), (7, "goal.x"), (8, "goal.y"), (9, "goal.z"), (10, "goal_weight")]),
            NodeKind::Wrangle { .. } | NodeKind::MoonRayRender { .. } | NodeKind::AttributeVop { .. } | NodeKind::Vop(_) => Vec::new(),
            other => (0..other.params().len()).map(|i| (ChannelTarget::Param(i), format!("parm{i}"))).collect(),
        }
    }

    // Text parameters, saved hex-encoded next to the numeric ones.
    fn texts(&self) -> Vec<String> {
        match self {
            NodeKind::LopSopImport { path, .. }
            | NodeKind::LopPrimitive { path, .. }
            | NodeKind::LopLight { path, .. }
            | NodeKind::LopCamera { path, .. } => vec![path.clone()],
            NodeKind::LopReference { file, path, .. } => vec![file.clone(), path.clone()],
            NodeKind::LopTransform { pattern, .. } | NodeKind::LopMaterial { pattern, .. } | NodeKind::LopPrune { pattern, .. } => {
                vec![pattern.clone()]
            }
            NodeKind::LopExport { file } | NodeKind::FileImport { file } | NodeKind::FileExport { file } | NodeKind::UvQuickShade { file } => {
                vec![file.clone()]
            }
            NodeKind::AttributePaint { attribute, .. } => vec![attribute.clone()],
            NodeKind::EnvironmentLight { file, .. } => vec![file.clone()],
            NodeKind::GroupCreate(g) => vec![g.name.clone(), g.expression.clone()],
            NodeKind::Vop(v) => vec![v.name.clone()],
            NodeKind::Shader(v) => v.textures.clone(),
            NodeKind::ShaderNode(n) => n.texts.clone(),
            _ => Vec::new(),
        }
    }

    fn set_texts(&mut self, texts: &[String]) {
        let mut texts = texts.iter().cloned();
        let mut next = |field: &mut String| {
            if let Some(text) = texts.next() {
                *field = text;
            }
        };
        match self {
            NodeKind::LopSopImport { path, .. }
            | NodeKind::LopPrimitive { path, .. }
            | NodeKind::LopLight { path, .. }
            | NodeKind::LopCamera { path, .. } => next(path),
            NodeKind::LopReference { file, path, .. } => {
                next(file);
                next(path);
            }
            NodeKind::LopTransform { pattern, .. } | NodeKind::LopMaterial { pattern, .. } | NodeKind::LopPrune { pattern, .. } => next(pattern),
            NodeKind::LopExport { file } | NodeKind::FileImport { file } | NodeKind::FileExport { file } | NodeKind::UvQuickShade { file } => {
                next(file)
            }
            NodeKind::AttributePaint { attribute, .. } => next(attribute),
            NodeKind::EnvironmentLight { file, .. } => next(file),
            NodeKind::GroupCreate(g) => {
                next(&mut g.name);
                next(&mut g.expression);
            }
            NodeKind::Vop(v) => next(&mut v.name),
            NodeKind::Shader(v) => v.textures.iter_mut().for_each(|t| next(t)),
            NodeKind::ShaderNode(n) => n.texts.iter_mut().for_each(|t| next(t)),
            _ => {}
        }
    }

    // Nodes whose network lives in their children: Geometry objects and SOP Solvers.
    fn is_container(&self) -> bool {
        matches!(self, NodeKind::Geometry { .. } | NodeKind::SopSolver { .. } | NodeKind::AttributeVop { .. } | NodeKind::MaterialBuilder)
    }

    fn is_solver(&self) -> bool {
        matches!(
            self,
            NodeKind::Particles { .. }
                | NodeKind::Flip { .. }
                | NodeKind::WhitewaterSolver(_)
                | NodeKind::CrowdSolver(_)
                | NodeKind::MuscleSolver(_)
                | NodeKind::Pyro(_)
                | NodeKind::RbdSolver(_)
                | NodeKind::VellumSolver(_)
                | NodeKind::SopSolver { .. }
        )
    }

    // Name in the add-node menus and TAB search ("Add", "Noise"... for VOPs).
    fn menu_name(&self) -> String {
        match self {
            NodeKind::Vop(v) => v.def().label.to_owned(),
            NodeKind::Shader(v) => v.name().to_owned(),
            NodeKind::ShaderNode(n) => n.def().label.to_owned(),
            NodeKind::MaterialFracture(f) => format!("material_fracture ({})", fracture::MATERIALS[f.material.min(2)]),
            other => other.name().to_owned(),
        }
    }

    fn menu_help(&self) -> Option<&'static str> {
        match self {
            NodeKind::Vop(v) => Some(v.def().help),
            NodeKind::ShaderNode(n) => Some(n.def().help),
            _ => None,
        }
    }

    // Title shown on the node in the network editor.
    fn label(&self, id: NodeId) -> String {
        match self {
            NodeKind::SolverInput { index: 0 } => format!("Prev_Frame{id}"),
            NodeKind::SolverInput { .. } => format!("Input_1·{id}"),
            NodeKind::Vop(v) => v.label(id),
            NodeKind::Shader(v) => format!("{}{id}", v.name().to_lowercase().replace(' ', "_")),
            NodeKind::ShaderNode(n) => n.label(id),
            _ => format!("{}{id}", self.name()),
        }
    }

    fn templates() -> Vec<(&'static str, Vec<NodeKind>)> {
        let mut all = vec![
            (
                "USD Create",
                vec![
                    NodeKind::LopSopImport { geometry: None, path: "/geo".into() },
                    NodeKind::LopReference { file: String::new(), path: "/ref".into(), sublayer: false },
                    NodeKind::LopPrimitive { shape: 0, path: "/shapes/sphere".into(), size: 1.0, t: [0.0, 1.0, 0.0] },
                    NodeKind::LopLight {
                        kind: 0,
                        path: "/lights/key".into(),
                        position: [4.0, 6.0, 4.0],
                        target: [0.0; 3],
                        color: [1.0, 0.97, 0.9],
                        intensity: 1.0,
                        size: 1.0,
                    },
                    NodeKind::LopCamera { path: "/cameras/cam".into(), position: [5.0, 4.0, 6.0], target: [0.0; 3], focal: 35.0, aperture: 36.0 },
                ],
            ),
            (
                "USD Edit",
                vec![
                    NodeKind::LopTransform { pattern: "/geo/*".into(), t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] },
                    NodeKind::LopMaterial { pattern: "/geo/**".into(), material: None },
                    NodeKind::LopPrune { pattern: String::new(), hide: false },
                    NodeKind::LopMerge,
                    NodeKind::LopExport { file: String::new() },
                ],
            ),
            (
                "Objects",
                vec![
                    NodeKind::Geometry { material: None },
                    NodeKind::Camera {
                        position: [0.0, 0.0, 0.0],
                        rotation: [23.0, -35.0, 0.0],
                        focal: 1.2,
                        near: 0.5,
                        orthographic: false,
                        pan: [0.0, 0.0],
                        zoom: 6.0,
                    },
                    NodeKind::Light {
                        position: [3.0, 4.0, 3.0],
                        rotation: [-35.0, 35.0, 0.0],
                        color: [1.0, 0.95, 0.85],
                        intensity: 0.9,
                        kind: 0,
                    },
                    NodeKind::EnvironmentLight { file: String::new(), color: [1.0; 3], intensity: 1.0, rotation: 0.0, visible: true },
                ],
            ),
            (
                "Generators",
                vec![
                    NodeKind::Add { count: 1, spacing: 1.0, layout: 0 },
                    NodeKind::Grid { size: 4.0, res: 20 },
                    NodeKind::Plane { size: 4.0, res: 20 },
                    NodeKind::Cone { radius: 1.0, height: 2.0, rows: 1, cols: 32, caps: true },
                    NodeKind::Capsule { radius: 0.6, length: 1.5, rows: 16, cols: 32 },
                    NodeKind::Box { size: 1.5, div: 4 },
                    NodeKind::Sphere { radius: 1.0, rows: 16, cols: 32 },
                    NodeKind::Torus { major: 1.0, minor: 0.35, rows: 16, cols: 32 },
                    NodeKind::Tube { r_bot: 0.5, r_top: 0.5, height: 2.0, rows: 8, cols: 24, caps: true },
                    NodeKind::ParticleSurface {
                        radius: 0.12,
                        join_distance: 0.08,
                        resolution: 48,
                        smoothing: 0,
                    },
                ],
            ),
            (
                "Groom",
                vec![
                    NodeKind::HairGuides(groom::GuideSettings {
                        count: 200,
                        length: 0.6,
                        length_variance: 0.2,
                        segments: 8,
                        droop: 0.3,
                        seed: 1,
                    }),
                    NodeKind::GuideProcess(groom::ProcessSettings { op: 1, amount: 0.5, frequency: 2.0, seed: 1 }),
                    NodeKind::HairGenerate(groom::HairSettings {
                        count: 5000,
                        segments: 8,
                        clump: 0.3,
                        length_variance: 0.15,
                        frizz: 0.2,
                        width_root: 0.004,
                        width_tip: 0.001,
                        seed: 1,
                    }),
                    NodeKind::HairDeform(groom::DeformSettings { rest_frame: 1.0 }),
                    NodeKind::AttributePaint { attribute: "density".into(), values: Vec::new(), settings: paint::PaintSettings::default() },
                ],
            ),
            (
                "Solvers",
                vec![
                    NodeKind::Particles {
                        count: 2000,
                        emission_rate: 400.0,
                        lifetime: 4.0,
                        speed: 2.0,
                        spread: 0.35,
                        gravity: -1.5,
                        seed: 1,
                        drag: 0.0,
                        bounce: 0.4,
                        friction: 0.1,
                        radius: 0.02,
                        substeps: 2,
                        along_normals: false,
                    },
                    NodeKind::Flip {
                        particle_count: 20_000,
                        grid_resolution: 64,
                        flip_ratio: 0.95,
                        viscosity: 0.1,
                        gravity: -9.8,
                        max_dt: 0.02,
                        substeps: 2,
                        seed: 11,
                    },
                    NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings::defaults()),
                    NodeKind::Pyro(sims::PyroSettings::default()),
                    NodeKind::RbdSolver(sims::RbdSettings::default()),
                    NodeKind::VellumSolver(vellum::VellumSettings::defaults(0)),
                    NodeKind::SopSolver { substeps: 1 },
                    NodeKind::SolverInput { index: 0 },
                    NodeKind::SolverInput { index: 1 },
                ],
            ),
            (
                "Import/Export",
                vec![NodeKind::FileImport { file: String::new() }, NodeKind::FileExport { file: String::new() }],
            ),
            (
                "Volumes",
                vec![
                    NodeKind::Vdb { voxel_size: 0.1, padding: 0, operation: 0, iterations: 1, output: 1 },
                    NodeKind::ConvertVdb { mode: 0, iso: 0.0 },
                ],
            ),
            (
                "Deformers",
                vec![
                    NodeKind::Transform { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] },
                    NodeKind::Noise { freq: 2.2, amp: 0.3 },
                    NodeKind::Taper { factor: 0.3 },
                    NodeKind::Twist { angle: 1.0 },
                    NodeKind::Bend { angle: 0.5 },
                    NodeKind::Smooth { iterations: 5, strength: 0.5 },
                    NodeKind::Bevel { amount: 0.25, segments: 2 },
                    NodeKind::Peak { dist: 0.1 },
                    NodeKind::Ripple { freq: 6.0, amp: 0.3, speed: 4.0 },
                    NodeKind::Jitter { amp: 0.05, seed: 1 },
                ],
            ),
            (
                "Topology",
                vec![
                    NodeKind::Subdivide { levels: 1 },
                    NodeKind::Remesh { target: 2_000 },
                    NodeKind::Fuse { tol: 0.001 },
                    NodeKind::Mirror { axis: 0 },
                    NodeKind::Copy { count: 5, t: [1.5, 0.0, 0.0], r: [0.0; 3], s: 1.0 },
                    NodeKind::CopyToPoints { scale: 1.0, rotation: [0.0; 3] },
                    NodeKind::Extrude { dist: 0.2, inset: 0.2, mode: 0 },
                    NodeKind::Clip { axis: 1, offset: 0.0, invert: false },
                    NodeKind::Blast { component: 1, indices: Vec::new(), invert: false },
                    NodeKind::Scatter(sops::ScatterSettings { count: 1000, seed: 1, relax_iterations: 5, volume: false }),
                    NodeKind::Boolean(boolean::BooleanSettings { operation: 2, mode: 0, resolution: 128 }),
                    NodeKind::Cache { cache_id: 1, mode: 1 },
                    NodeKind::Reverse,
                    NodeKind::Switch { index: 0 },
                    NodeKind::Merge,
                    NodeKind::Null,
                ],
            ),
            (
                "Destruction",
                (0..fracture::MATERIALS.len()).map(|m| NodeKind::MaterialFracture(fracture::FractureSettings::defaults(m))).collect(),
            ),
            (
                "Crowds",
                vec![NodeKind::CrowdSolver(crowd::CrowdSettings::defaults())],
            ),
            (
                "Muscles",
                vec![
                    NodeKind::Muscle(muscle::MuscleSettings::defaults()),
                    NodeKind::MuscleSolver(muscle::JiggleSettings::defaults()),
                    NodeKind::MuscleSkin(muscle::SkinSettings::defaults()),
                ],
            ),
            (
                "Terrain",
                vec![
                    NodeKind::Terrain(terrain::TerrainSettings { size: [20.0, 20.0], resolution: 256 }),
                    NodeKind::TerrainNoise(terrain::NoiseSettings::defaults()),
                    NodeKind::TerrainErode(terrain::ErodeSettings::defaults()),
                    NodeKind::TerrainTerrace(terrain::TerraceSettings { step: 0.5, offset: 0.0, sharpness: 0.6, use_mask: false }),
                    NodeKind::TerrainMask(terrain::MaskSettings {
                        source: 1,
                        min: 30.0,
                        max: 90.0,
                        falloff: 5.0,
                        blur: 1,
                        invert: false,
                        combine: 0,
                        visualize: true,
                    }),
                    NodeKind::TerrainColorize(terrain::ColorizeSettings::defaults()),
                ],
            ),
            (
                "Attributes",
                vec![
                    NodeKind::AttributeCreate { attribute: 1, value: 1.0 },
                    NodeKind::AttributeRandomize { attribute: 1, amount: 0.25, seed: 1 },
                    NodeKind::AttributeDelete { attribute: 1 },
                    NodeKind::Wrangle { code: "@P.y += sin(@P.x * 3.0) * 0.2;".into(), domain: 0 },
                    NodeKind::AttributeVop { domain: 2 },
                    NodeKind::Normal(sops::NormalSettings { domain: 1, cusp_angle: 60.0, weighting: 0, reverse: false }),
                    NodeKind::AttributePaint { attribute: "mask".into(), values: Vec::new(), settings: paint::PaintSettings::default() },
                ],
            ),
            (
                "Groups",
                vec![NodeKind::GroupCreate(groups::GroupCreateSettings {
                    name: "group1".into(),
                    domain: groups::GroupDomain::Points,
                    method: 0,
                    bbox_min: [-1.0, -1.0, -1.0],
                    bbox_max: [1.0, 1.0, 1.0],
                    percent: 50.0,
                    seed: 1,
                    direction: [0.0, 1.0, 0.0],
                    spread: 30.0,
                    expression: "@P.y>0".into(),
                    combine: 0,
                    invert: false,
                })],
            ),
            (
                "UV",
                vec![
                    NodeKind::UvTexture(sops::UvTextureSettings { projection: 3, axis: 1, scale: [1.0, 1.0], offset: [0.0, 0.0] }),
                    NodeKind::UvTransform(sops::UvTransformSettings { translate: [0.0; 2], rotate: 0.0, scale: [1.0; 2], pivot: [0.5; 2] }),
                    NodeKind::UvQuickShade { file: String::new() },
                ],
            ),
            (
                "Materials",
                vec![
                    NodeKind::Material { kind: 0, color: [0.8, 0.8, 0.8], roughness: 0.5, value: 0.5 },
                    NodeKind::Material { kind: 1, color: [0.9, 0.7, 0.3], roughness: 0.3, value: 0.0 },
                    NodeKind::Material { kind: 2, color: [1.0, 1.0, 1.0], roughness: 0.02, value: 1.5 },
                    NodeKind::Material { kind: 3, color: [1.0, 0.9, 0.7], roughness: 0.0, value: 1.0 },
                ],
            ),
            (
                "Shaders",
                vec![
                    NodeKind::Shader(shading::Values::new(shading::Table::Principled)),
                    NodeKind::Shader(shading::Values::new(shading::Table::Toon)),
                    NodeKind::Shader(shading::Values::new(shading::Table::Hair)),
                    NodeKind::Shader(shading::Values::new(shading::Table::Pyro)),
                    NodeKind::MaterialBuilder,
                ],
            ),
            (
                "Render",
                vec![NodeKind::MoonRayRender {
                    executable: "moonray".into(),
                    arguments: "--scene {scene} --output {output}".into(),
                    output: "moonray_render.exr".into(),
                    width: 1280,
                    height: 720,
                    samples: 64,
                    opts: RenderOpts::default(),
                }],
            ),
        ];
        // Shader presets, one menu per kind of material.
        for (category, range) in shading::PRESET_CATEGORIES {
            all.push((category, range.clone().map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Principled, i))).collect()));
        }
        all.push(("Toon & Hair Presets", (1..shading::TOON_PRESETS.len()).map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Toon, i)))
            .chain((1..shading::HAIR_PRESETS.len()).map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Hair, i)))).collect()));
        all.push(("Volume Presets", (1..shading::PYRO_PRESETS.len()).map(|i| NodeKind::Shader(shading::Values::preset(shading::Table::Pyro, i))).collect()));
        // Material Builder nodes (only shown inside a Material Builder).
        all.push((shading::CATEGORY, (0..shading::SHADER_OPS.len()).map(|op| NodeKind::ShaderNode(shading::ShaderNode::new(op))).collect()));
        // VOP networks: one menu per category (only shown inside an Attribute VOP).
        all.extend(vop::CATEGORIES.iter().map(|category| {
            (*category, (0..vop::OPS.len()).filter(|&op| vop::OPS[op].category == *category).map(|op| NodeKind::Vop(vop::VopNode::new(op))).collect())
        }));
        all
    }

    fn name(&self) -> &'static str {
        match self {
            NodeKind::Add { .. } => "add",
            NodeKind::Grid { .. } => "grid",
            NodeKind::Plane { .. } => "plane",
            NodeKind::Cone { .. } => "cone",
            NodeKind::Capsule { .. } => "capsule",
            NodeKind::Box { .. } => "box",
            NodeKind::Sphere { .. } => "sphere",
            NodeKind::Torus { .. } => "torus",
            NodeKind::Transform { .. } => "xform",
            NodeKind::Noise { .. } => "noise",
            NodeKind::Taper { .. } => "taper",
            NodeKind::Twist { .. } => "twist",
            NodeKind::Bend { .. } => "bend",
            NodeKind::Smooth { .. } => "smooth",
            NodeKind::Bevel { .. } => "bevel",
            NodeKind::Fuse { .. } => "fuse",
            NodeKind::Subdivide { .. } => "subdivide",
            NodeKind::Remesh { .. } => "remesh",
            NodeKind::Mirror { .. } => "mirror",
            NodeKind::Copy { .. } => "copy",
            NodeKind::CopyToPoints { .. } => "copy_to_points",
            NodeKind::AttributeCreate { .. } => "attribute_create",
            NodeKind::AttributeRandomize { .. } => "attribute_randomize",
            NodeKind::AttributeDelete { .. } => "attribute_delete",
            NodeKind::Wrangle { .. } => "wrangle",
            NodeKind::AttributeVop { .. } => "attribute_vop",
            NodeKind::Vop(_) => "vop",
            NodeKind::Merge => "merge",
            NodeKind::Null => "null",
            NodeKind::Tube { .. } => "tube",
            NodeKind::Peak { .. } => "peak",
            NodeKind::Ripple { .. } => "ripple",
            NodeKind::Jitter { .. } => "jitter",
            NodeKind::Particles { .. } => "particles",
            NodeKind::Flip { .. } => "flip",
            NodeKind::WhitewaterSolver(_) => "whitewater_solver",
            NodeKind::ParticleSurface { .. } => "particle_surface",
            NodeKind::Extrude { .. } => "extrude",
            NodeKind::Clip { .. } => "clip",
            NodeKind::Blast { .. } => "blast",
            NodeKind::Cache { .. } => "cache",
            NodeKind::Vdb { .. } => "vdb",
            NodeKind::ConvertVdb { .. } => "convert_vdb",
            NodeKind::FileImport { .. } => "file",
            NodeKind::FileExport { .. } => "file_export",
            NodeKind::MoonRayRender { .. } => "moonray_render",
            NodeKind::Camera { .. } => "camera",
            NodeKind::Light { .. } => "light",
            NodeKind::EnvironmentLight { .. } => "environment_light",
            NodeKind::Reverse => "reverse",
            NodeKind::Pyro(_) => "pyro",
            NodeKind::HairGuides(_) => "hair_guides",
            NodeKind::GuideProcess(_) => "guide_process",
            NodeKind::HairGenerate(_) => "hair_generate",
            NodeKind::HairDeform(_) => "hair_deform",
            NodeKind::LopSopImport { .. } => "sop_import",
            NodeKind::LopReference { .. } => "usd_reference",
            NodeKind::LopPrimitive { .. } => "usd_primitive",
            NodeKind::LopTransform { .. } => "usd_transform",
            NodeKind::LopLight { .. } => "usd_light",
            NodeKind::LopCamera { .. } => "usd_camera",
            NodeKind::LopMaterial { .. } => "assign_material",
            NodeKind::LopPrune { .. } => "usd_prune",
            NodeKind::LopMerge => "usd_merge",
            NodeKind::LopExport { .. } => "usd_export",
            NodeKind::RbdSolver(_) => "rbd_solver",
            NodeKind::VellumSolver(_) => "vellum_solver",
            NodeKind::Normal(_) => "normal",
            NodeKind::Scatter(_) => "scatter",
            NodeKind::Boolean(_) => "boolean",
            NodeKind::MaterialFracture(_) => "material_fracture",
            NodeKind::CrowdSolver(_) => "crowd_solver",
            NodeKind::Muscle(_) => "muscle",
            NodeKind::MuscleSolver(_) => "muscle_solver",
            NodeKind::MuscleSkin(_) => "muscle_skin",
            NodeKind::Terrain(_) => "terrain",
            NodeKind::TerrainNoise(_) => "terrain_noise",
            NodeKind::TerrainErode(_) => "terrain_erode",
            NodeKind::TerrainTerrace(_) => "terrain_terrace",
            NodeKind::TerrainMask(_) => "terrain_mask",
            NodeKind::TerrainColorize(_) => "terrain_colorize",
            NodeKind::UvTexture(_) => "uv_texture",
            NodeKind::UvTransform(_) => "uv_transform",
            NodeKind::UvQuickShade { .. } => "uv_quickshade",
            NodeKind::AttributePaint { .. } => "attribute_paint",
            NodeKind::GroupCreate(_) => "group_create",
            NodeKind::SopSolver { .. } => "sop_solver",
            NodeKind::SolverInput { .. } => "solver_input",
            NodeKind::Switch { .. } => "switch",
            NodeKind::Geometry { .. } => "geometry",
            NodeKind::Material { kind, .. } => MATERIAL_NAMES[(*kind).min(MATERIAL_NAMES.len() - 1)],
            NodeKind::Shader(v) => match v.table {
                shading::Table::Principled => "principled_shader",
                shading::Table::Toon => "toon_shader",
                shading::Table::Hair => "hair_shader",
                shading::Table::Pyro => "pyro_shader",
            },
            NodeKind::MaterialBuilder => "material_builder",
            NodeKind::ShaderNode(_) => "shader_node",
        }
    }

    fn num_inputs(&self) -> usize {
        match self {
            NodeKind::Add { .. } => 0,
            NodeKind::Grid { .. }
            | NodeKind::Plane { .. }
            | NodeKind::Cone { .. }
            | NodeKind::Capsule { .. }
            | NodeKind::Box { .. }
            | NodeKind::Sphere { .. }
            | NodeKind::Torus { .. }
            | NodeKind::Tube { .. } => 0,
            NodeKind::Terrain(_) => 0,
            NodeKind::Boolean(_) => 2,
            NodeKind::CrowdSolver(_) => 3,
            NodeKind::MuscleSkin(_) => 2,
            NodeKind::Particles { .. } => 2,
            NodeKind::Flip { .. } => 2,
            NodeKind::WhitewaterSolver(_) => 2,
            NodeKind::Pyro(_) | NodeKind::RbdSolver(_) | NodeKind::VellumSolver(_) => 2,
            NodeKind::HairGenerate(_) | NodeKind::HairDeform(_) => 2,
            NodeKind::SopSolver { .. } => 1,
            NodeKind::SolverInput { .. } => 0,
            NodeKind::LopMerge => 2,
            NodeKind::FileImport { .. } => 0,
            NodeKind::ParticleSurface { .. } => 1,
            NodeKind::Merge | NodeKind::Switch { .. } | NodeKind::CopyToPoints { .. } => 2,
            NodeKind::AttributeCreate { .. }
            | NodeKind::AttributeRandomize { .. }
            | NodeKind::AttributeDelete { .. } => 1,
            NodeKind::Wrangle { .. } => 1,
            NodeKind::AttributeVop { .. } => 1,
            NodeKind::Vop(v) => v.def().inputs.len(),
            NodeKind::MoonRayRender { .. } => 0,
            NodeKind::Camera { .. } | NodeKind::Light { .. } | NodeKind::Geometry { .. } | NodeKind::Material { .. } => 0,
            NodeKind::EnvironmentLight { .. } => 0,
            NodeKind::Shader(_) | NodeKind::MaterialBuilder => 0,
            NodeKind::ShaderNode(n) => n.def().inputs.len(),
            _ => 1,
        }
    }

    fn color(&self) -> egui::Color32 {
        if matches!(self, NodeKind::Geometry { .. }) {
            return egui::Color32::from_rgb(110, 200, 150);
        }
        if matches!(self, NodeKind::Material { .. }) {
            return egui::Color32::from_rgb(200, 110, 160);
        }
        if self.is_solver() {
            return egui::Color32::from_rgb(225, 115, 60);
        }
        if self.is_lop() {
            return egui::Color32::from_rgb(70, 175, 190);
        }
        if matches!(self, NodeKind::SolverInput { .. }) {
            return egui::Color32::from_rgb(150, 150, 110);
        }
        match self.num_inputs() {
            0 => egui::Color32::from_rgb(90, 170, 90),
            2 => egui::Color32::from_rgb(170, 120, 200),
            _ => match self {
                NodeKind::Subdivide { .. }
                | NodeKind::Remesh { .. }
                | NodeKind::Fuse { .. }
                | NodeKind::Mirror { .. }
                | NodeKind::Copy { .. }
                |                 NodeKind::CopyToPoints { .. }
                    | NodeKind::AttributeCreate { .. }
                    | NodeKind::AttributeRandomize { .. }
                    | NodeKind::AttributeDelete { .. }
                    | NodeKind::Wrangle { .. } => egui::Color32::from_rgb(200, 140, 60),
                NodeKind::Null => egui::Color32::from_gray(150),
                NodeKind::MoonRayRender { .. } => egui::Color32::from_rgb(100, 150, 210),
                NodeKind::Camera { .. } => egui::Color32::from_rgb(80, 180, 220),
                NodeKind::Light { .. } => egui::Color32::from_rgb(240, 180, 70),
                NodeKind::EnvironmentLight { .. } => egui::Color32::from_rgb(235, 200, 110),
                _ => egui::Color32::from_rgb(80, 140, 210),
            },
        }
    }

    // ---- serialization (flat list of numbers) ----
    fn params(&self) -> Vec<f32> {
        match self {
            NodeKind::Add { count, spacing, layout } => {
                vec![*count as f32, *spacing, *layout as f32]
            }
            NodeKind::Grid { size, res } => vec![*size, *res as f32],
            NodeKind::Plane { size, res } => vec![*size, *res as f32],
            NodeKind::Cone { radius, height, rows, cols, caps } => {
                vec![*radius, *height, *rows as f32, *cols as f32, *caps as u8 as f32]
            }
            NodeKind::Capsule { radius, length, rows, cols } => {
                vec![*radius, *length, *rows as f32, *cols as f32]
            }
            NodeKind::Box { size, div } => vec![*size, *div as f32],
            NodeKind::Sphere { radius, rows, cols } => vec![*radius, *rows as f32, *cols as f32],
            NodeKind::Torus { major, minor, rows, cols } => {
                vec![*major, *minor, *rows as f32, *cols as f32]
            }
            NodeKind::Transform { t, r, s } => [t.as_slice(), r, s].concat(),
            NodeKind::Noise { freq, amp } => vec![*freq, *amp],
            NodeKind::Taper { factor } => vec![*factor],
            NodeKind::Twist { angle } | NodeKind::Bend { angle } => vec![*angle],
            NodeKind::Smooth { iterations, strength } => vec![*iterations as f32, *strength],
            NodeKind::Bevel { amount, segments } => vec![*amount, *segments as f32],
            NodeKind::Fuse { tol } => vec![*tol],
            NodeKind::Mirror { axis } => vec![*axis as f32],
            NodeKind::Copy { count, t, r, s } => {
                [vec![*count as f32], t.to_vec(), r.to_vec(), vec![*s]].concat()
            }
            NodeKind::CopyToPoints { scale, rotation } => {
                vec![*scale, rotation[0], rotation[1], rotation[2]]
            }
            NodeKind::AttributeCreate { attribute, value } => vec![*attribute as f32, *value],
            NodeKind::AttributeRandomize { attribute, amount, seed } => {
                vec![*attribute as f32, *amount, *seed as f32]
            }
            NodeKind::AttributeDelete { attribute } => vec![*attribute as f32],
            NodeKind::Wrangle { .. } => vec![],
            NodeKind::AttributeVop { domain } => vec![*domain as f32],
            NodeKind::Vop(v) => vec![v.op as f32, v.ty as f32, v.value[0], v.value[1], v.value[2]],
            NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                vec![*r_bot, *r_top, *height, *rows as f32, *cols as f32, *caps as u8 as f32]
            }
            NodeKind::Peak { dist } => vec![*dist],
            NodeKind::Ripple { freq, amp, speed } => vec![*freq, *amp, *speed],
            NodeKind::Jitter { amp, seed } => vec![*amp, *seed as f32],
            NodeKind::Particles {
                count,
                emission_rate,
                lifetime,
                speed,
                spread,
                gravity,
                seed,
                drag,
                bounce,
                friction,
                radius,
                substeps,
                along_normals,
            } => {
                vec![
                    *count as f32,
                    *emission_rate,
                    *lifetime,
                    *speed,
                    *spread,
                    *gravity,
                    *seed as f32,
                    *drag,
                    *bounce,
                    *friction,
                    *radius,
                    *substeps as f32,
                    *along_normals as u8 as f32,
                ]
            }
            NodeKind::WhitewaterSolver(w) => w.to_params(),
            NodeKind::Flip {
                particle_count,
                grid_resolution,
                flip_ratio,
                viscosity,
                gravity,
                max_dt,
                substeps,
                seed,
            } => vec![
                *particle_count as f32,
                *grid_resolution as f32,
                *flip_ratio,
                *viscosity,
                *gravity,
                *max_dt,
                *substeps as f32,
                *seed as f32,
            ],
            NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                vec![*radius, *join_distance, *resolution as f32, *smoothing as f32]
            }
            NodeKind::Extrude { dist, inset, mode } => vec![*dist, *inset, *mode as f32],
            NodeKind::Clip { axis, offset, invert } => vec![*axis as f32, *offset, *invert as u8 as f32],
            NodeKind::Blast { component, indices, invert } => {
                let mut params = vec![*component as f32, *invert as u8 as f32, indices.len() as f32];
                params.extend(indices.iter().map(|index| *index as f32));
                params
            }
            NodeKind::Cache { cache_id, mode } => vec![*cache_id as f32, *mode as f32],
            NodeKind::Vdb { voxel_size, padding, operation, iterations, output } => {
                vec![*voxel_size, *padding as f32, *operation as f32, *iterations as f32, *output as f32]
            }
            NodeKind::ConvertVdb { mode, iso } => vec![*mode as f32, *iso],
            NodeKind::FileImport { .. } | NodeKind::FileExport { .. } | NodeKind::UvQuickShade { .. } => vec![],
            NodeKind::MoonRayRender { .. } => vec![],
            NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } => {
                [position.to_vec(), rotation.to_vec(), vec![*focal, *near, *orthographic as u8 as f32, pan[0], pan[1], *zoom]].concat()
            }
            NodeKind::Light { position, rotation, color, intensity, kind } => {
                [position.to_vec(), rotation.to_vec(), color.to_vec(), vec![*intensity, *kind as f32]].concat()
            }
            NodeKind::Switch { index } => vec![*index as f32],
            NodeKind::Reverse => vec![],
            NodeKind::Pyro(settings) => settings.to_params(),
            NodeKind::HairGuides(s) => {
                vec![s.count as f32, s.length, s.length_variance, s.segments as f32, s.droop, s.seed as f32]
            }
            NodeKind::GuideProcess(s) => vec![s.op as f32, s.amount, s.frequency, s.seed as f32],
            NodeKind::HairDeform(s) => vec![s.rest_frame],
            NodeKind::HairGenerate(s) => vec![
                s.count as f32,
                s.segments as f32,
                s.clump,
                s.length_variance,
                s.frizz,
                s.width_root,
                s.width_tip,
                s.seed as f32,
            ],
            NodeKind::RbdSolver(settings) => settings.to_params(),
            NodeKind::Normal(s) => vec![s.domain as f32, s.cusp_angle, s.weighting as f32, s.reverse as u8 as f32],
            NodeKind::GroupCreate(g) => {
                let mut p = vec![(g.domain == groups::GroupDomain::Prims) as u8 as f32, g.method as f32];
                p.extend(g.bbox_min);
                p.extend(g.bbox_max);
                p.extend([g.percent, g.seed as f32]);
                p.extend(g.direction);
                p.extend([g.spread, g.combine as f32, g.invert as u8 as f32]);
                p
            }
            NodeKind::EnvironmentLight { color, intensity, rotation, visible, .. } => {
                vec![color[0], color[1], color[2], *intensity, *rotation, *visible as u8 as f32]
            }
            // Brush settings, then every painted value.
            NodeKind::AttributePaint { values, settings: s, .. } => {
                let mut p = vec![s.value, s.default, s.radius, s.strength, s.mode as f32, s.visualize as u8 as f32];
                p.extend_from_slice(values);
                p
            }
            NodeKind::Scatter(s) => vec![s.count as f32, s.seed as f32, s.relax_iterations as f32, s.volume as u8 as f32],
            NodeKind::Boolean(s) => vec![s.operation as f32, s.mode as f32, s.resolution as f32],
            NodeKind::MaterialFracture(s) => vec![
                s.material as f32,
                s.pieces as f32,
                s.seed as f32,
                s.impact[0],
                s.impact[1],
                s.impact[2],
                s.rings as f32,
                s.spokes as f32,
                s.grain_axis as f32,
                s.splinter,
                s.separate,
            ],
            NodeKind::CrowdSolver(s) => s.to_params(),
            NodeKind::Muscle(s) => s.to_params(),
            NodeKind::MuscleSolver(s) => vec![s.stiffness, s.damping, s.smoothing as f32, s.substeps as f32],
            NodeKind::MuscleSkin(s) => vec![s.falloff, s.influences as f32],
            NodeKind::Terrain(s) => vec![s.size[0], s.size[1], s.resolution as f32],
            NodeKind::TerrainNoise(s) => vec![
                s.kind as f32,
                s.blend as f32,
                s.amplitude,
                s.element_size,
                s.octaves as f32,
                s.roughness,
                s.offset[0],
                s.offset[1],
                s.seed as f32,
                s.positive as u8 as f32,
                s.use_mask as u8 as f32,
            ],
            NodeKind::TerrainErode(s) => vec![
                s.droplets as f32,
                s.erosion,
                s.deposition,
                s.capacity,
                s.evaporation,
                s.inertia,
                s.radius as f32,
                s.thermal_iterations as f32,
                s.talus_angle,
                s.seed as f32,
            ],
            NodeKind::TerrainTerrace(s) => vec![s.step, s.offset, s.sharpness, s.use_mask as u8 as f32],
            NodeKind::TerrainMask(s) => vec![
                s.source as f32,
                s.min,
                s.max,
                s.falloff,
                s.blur as f32,
                s.invert as u8 as f32,
                s.combine as f32,
                s.visualize as u8 as f32,
            ],
            NodeKind::TerrainColorize(s) => [
                s.sand.to_vec(),
                s.grass.to_vec(),
                s.rock.to_vec(),
                s.snow.to_vec(),
                vec![s.sand_level, s.snow_level, s.rock_slope, s.blend],
            ]
            .concat(),
            NodeKind::UvTexture(s) => vec![s.projection as f32, s.axis as f32, s.scale[0], s.scale[1], s.offset[0], s.offset[1]],
            NodeKind::UvTransform(s) => vec![s.translate[0], s.translate[1], s.rotate, s.scale[0], s.scale[1], s.pivot[0], s.pivot[1]],
            NodeKind::VellumSolver(s) => vec![
                s.kind as f32,
                s.stretch_stiffness,
                s.bend_stiffness,
                s.shape_stiffness,
                s.mass,
                s.thickness,
                s.gravity,
                s.damping,
                s.friction,
                s.substeps as f32,
                s.iterations as f32,
                s.ground as u8 as f32,
                s.self_collision as u8 as f32,
                s.pin_to_animation as u8 as f32,
            ],
            NodeKind::SopSolver { substeps } => vec![*substeps as f32],
            NodeKind::LopSopImport { geometry, .. } => vec![geometry.map_or(-1.0, |g| g as f32)],
            NodeKind::LopReference { sublayer, .. } => vec![*sublayer as u8 as f32],
            NodeKind::LopPrimitive { shape, size, t, .. } => vec![*shape as f32, *size, t[0], t[1], t[2]],
            NodeKind::LopTransform { t, r, s, .. } => [*t, *r, *s].concat(),
            NodeKind::LopLight { kind, position, target, color, intensity, size, .. } => {
                [vec![*kind as f32], position.to_vec(), target.to_vec(), color.to_vec(), vec![*intensity, *size]].concat()
            }
            NodeKind::LopCamera { position, target, focal, aperture, .. } => [position.to_vec(), target.to_vec(), vec![*focal, *aperture]].concat(),
            NodeKind::LopMaterial { material, .. } => vec![material.map_or(-1.0, |m| m as f32)],
            NodeKind::LopPrune { hide, .. } => vec![*hide as u8 as f32],
            NodeKind::LopMerge | NodeKind::LopExport { .. } => vec![],
            NodeKind::SolverInput { index } => vec![*index as f32],
            NodeKind::Subdivide { levels } => vec![*levels as f32],
            NodeKind::Remesh { target } => vec![*target as f32],
            NodeKind::Merge | NodeKind::Null => vec![],
            NodeKind::Geometry { material } => vec![material.map_or(-1.0, |m| m as f32)],
            NodeKind::Material { kind, color, roughness, value } => {
                vec![*kind as f32, color[0], color[1], color[2], *roughness, *value]
            }
            NodeKind::Shader(v) => v.to_params(),
            NodeKind::MaterialBuilder => vec![],
            NodeKind::ShaderNode(n) => n.to_params(),
        }
    }
    fn from_params(name: &str, p: &[f32]) -> Option<NodeKind> {
        let g = |i: usize| p.get(i).copied().unwrap_or(0.0);
        let u = |i: usize| g(i).max(0.0) as usize;
        let v3 = |i: usize| [g(i), g(i + 1), g(i + 2)];
        Some(match name {
            "add" => NodeKind::Add {
                count: u(0).clamp(1, 100_000),
                spacing: g(1).max(0.001),
                layout: u(2).min(2),
            },
            "grid" => NodeKind::Grid { size: g(0), res: u(1) },
            "plane" => NodeKind::Plane { size: g(0), res: u(1) },
            "cone" => NodeKind::Cone {
                radius: g(0), height: g(1), rows: u(2), cols: u(3), caps: g(4) != 0.0,
            },
            "capsule" => NodeKind::Capsule {
                radius: g(0), length: g(1), rows: u(2), cols: u(3),
            },
            "box" => NodeKind::Box { size: g(0), div: u(1) },
            "sphere" => NodeKind::Sphere { radius: g(0), rows: u(1), cols: u(2) },
            "torus" => NodeKind::Torus { major: g(0), minor: g(1), rows: u(2), cols: u(3) },
            "xform" => NodeKind::Transform { t: v3(0), r: v3(3), s: v3(6) },
            "noise" => NodeKind::Noise { freq: g(0), amp: g(1) },
            "taper" => NodeKind::Taper { factor: g(0) },
            "twist" => NodeKind::Twist { angle: g(0) },
            "bend" => NodeKind::Bend { angle: g(0) },
            "smooth" => NodeKind::Smooth { iterations: u(0), strength: g(1) },
            "bevel" => NodeKind::Bevel { amount: g(0), segments: u(1).clamp(1, 8) },
            "fuse" => NodeKind::Fuse { tol: g(0) },
            "subdivide" => NodeKind::Subdivide { levels: u(0).clamp(1, 4) },
            "remesh" => NodeKind::Remesh { target: u(0).clamp(4, 200_000) },
            "mirror" => NodeKind::Mirror { axis: u(0) },
            "copy" => NodeKind::Copy { count: u(0), t: v3(1), r: v3(4), s: g(7) },
            "copy_to_points" => NodeKind::CopyToPoints {
                scale: g(0).max(0.001),
                rotation: v3(1),
            },
            "attribute_create" => NodeKind::AttributeCreate { attribute: u(0).min(3), value: g(1) },
            "attribute_randomize" => NodeKind::AttributeRandomize {
                attribute: u(0).min(3), amount: g(1).max(0.0), seed: u(2),
            },
            "attribute_delete" => NodeKind::AttributeDelete { attribute: u(0).min(3) },
            "wrangle" => NodeKind::Wrangle { code: String::new(), domain: u(0).min(3) },
            "attribute_vop" => NodeKind::AttributeVop { domain: u(0).min(3) },
            "vop" => NodeKind::Vop(vop::VopNode {
                op: u(0).min(vop::OPS.len() - 1),
                name: String::new(),
                ty: u(1),
                value: [g(2), g(3), g(4)],
            }),
            "merge" => NodeKind::Merge,
            "null" => NodeKind::Null,
            "tube" => NodeKind::Tube {
                r_bot: g(0),
                r_top: g(1),
                height: g(2),
                rows: u(3),
                cols: u(4),
                caps: g(5) > 0.5,
            },
            "peak" => NodeKind::Peak { dist: g(0) },
            "ripple" => NodeKind::Ripple { freq: g(0), amp: g(1), speed: g(2) },
            "jitter" => NodeKind::Jitter { amp: g(0), seed: u(1) },
            "particles" => {
                let or = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
                NodeKind::Particles {
                    count: u(0).clamp(1, 50_000),
                    emission_rate: g(1).max(0.01),
                    lifetime: g(2).max(0.01),
                    speed: g(3),
                    spread: g(4).max(0.0),
                    gravity: g(5),
                    seed: u(6),
                    drag: or(7, 0.0).max(0.0),
                    bounce: or(8, 0.4).clamp(0.0, 1.0),
                    friction: or(9, 0.1).clamp(0.0, 1.0),
                    radius: or(10, 0.02).max(0.0),
                    substeps: (or(11, 2.0).max(1.0) as usize).min(8),
                    along_normals: or(12, 0.0) > 0.5,
                }
            }
            "whitewater_solver" => NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings::from_params(p)),
            "flip" => NodeKind::Flip {
                particle_count: u(0).clamp(1, 100_000),
                grid_resolution: u(1).clamp(16, 128),
                flip_ratio: g(2).clamp(0.0, 1.0),
                viscosity: g(3).max(0.0),
                gravity: g(4),
                max_dt: g(5).clamp(0.001, 0.1),
                substeps: u(6).clamp(1, 8),
                seed: u(7),
            },
            "particle_surface" => NodeKind::ParticleSurface {
                radius: g(0).max(0.001),
                join_distance: g(1).max(0.0),
                resolution: u(2).clamp(8, 96),
                smoothing: u(3).min(3),
            },
            "extrude" => NodeKind::Extrude { dist: g(0), inset: g(1), mode: u(2).min(1) },
            "clip" => NodeKind::Clip { axis: u(0), offset: g(1), invert: g(2) > 0.5 },
            "blast" => {
                let count = u(2).min(p.len().saturating_sub(3));
                NodeKind::Blast {
                    component: u(0).min(2),
                    indices: p.iter().skip(3).take(count).map(|value| (*value).max(0.0) as usize).collect(),
                    invert: g(1) > 0.5,
                }
            }
            "cache" => NodeKind::Cache { cache_id: u(0), mode: u(1).min(1) },
            "vdb" => NodeKind::Vdb {
                voxel_size: g(0).max(0.01),
                padding: u(1).min(4),
                operation: u(2).min(3),
                iterations: u(3).clamp(1, 4),
                output: u(4).min(1),
            },
            "convert_vdb" => NodeKind::ConvertVdb { mode: u(0).min(1), iso: g(1) },
            "file" => NodeKind::FileImport { file: String::new() },
            "file_export" => NodeKind::FileExport { file: String::new() },
            "camera" => NodeKind::Camera {
                position: v3(0),
                rotation: v3(3),
                focal: g(6).clamp(0.5, 2.5),
                near: g(7).clamp(0.05, 10.0),
                orthographic: g(8) > 0.5,
                pan: [g(9), g(10)],
                zoom: g(11).clamp(0.5, 200.0),
            },
            "light" => NodeKind::Light {
                position: v3(0),
                rotation: v3(3),
                color: v3(6),
                intensity: g(9).max(0.0),
                kind: u(10).min(2),
            },
            "reverse" => NodeKind::Reverse,
            "pyro" => NodeKind::Pyro(sims::PyroSettings::from_params(p)),
            "hair_guides" => NodeKind::HairGuides(groom::GuideSettings {
                count: u(0).clamp(1, 20_000),
                length: g(1).max(0.0),
                length_variance: g(2).clamp(0.0, 1.0),
                segments: u(3).clamp(1, 64),
                droop: g(4).max(0.0),
                seed: u(5) as u32,
            }),
            "guide_process" => NodeKind::GuideProcess(groom::ProcessSettings {
                op: u(0).min(groom::GUIDE_OPS.len() - 1),
                amount: g(1),
                frequency: g(2),
                seed: u(3) as u32,
            }),
            "hair_deform" => NodeKind::HairDeform(groom::DeformSettings { rest_frame: p.first().copied().unwrap_or(1.0) }),
            "hair_generate" => NodeKind::HairGenerate(groom::HairSettings {
                count: u(0).clamp(1, 200_000),
                segments: u(1).clamp(1, 64),
                clump: g(2).clamp(0.0, 1.0),
                length_variance: g(3).clamp(0.0, 1.0),
                frizz: g(4).max(0.0),
                width_root: g(5).max(0.0),
                width_tip: g(6).max(0.0),
                seed: u(7) as u32,
            }),
            "rbd_solver" => NodeKind::RbdSolver(sims::RbdSettings::from_params(p)),
            "group_create" => NodeKind::GroupCreate(groups::GroupCreateSettings {
                name: "group1".into(),
                domain: if g(0) > 0.5 { groups::GroupDomain::Prims } else { groups::GroupDomain::Points },
                method: u(1).min(groups::CREATE_METHODS.len() - 1),
                bbox_min: v3(2),
                bbox_max: v3(5),
                percent: g(8),
                seed: u(9) as u32,
                direction: v3(10),
                spread: g(13),
                expression: String::new(),
                combine: u(14).min(groups::COMBINE_MODES.len() - 1),
                invert: g(15) > 0.5,
            }),
            "environment_light" => NodeKind::EnvironmentLight {
                file: String::new(),
                color: v3(0),
                intensity: g(3).max(0.0),
                rotation: g(4),
                visible: p.get(5).is_none_or(|v| *v > 0.5),
            },
            "attribute_paint" => NodeKind::AttributePaint {
                attribute: "density".into(),
                values: p.get(6..).map(<[f32]>::to_vec).unwrap_or_default(),
                settings: paint::PaintSettings {
                    value: g(0),
                    default: g(1),
                    radius: g(2).clamp(1.0, 2000.0),
                    strength: g(3).clamp(0.0, 1.0),
                    mode: u(4).min(paint::PAINT_MODES.len() - 1),
                    visualize: g(5) > 0.5,
                },
            },
            "normal" => NodeKind::Normal(sops::NormalSettings {
                domain: u(0).min(1),
                cusp_angle: g(1).clamp(0.0, 180.0),
                weighting: u(2).min(sops::NORMAL_WEIGHTS.len() - 1),
                reverse: g(3) > 0.5,
            }),
            "scatter" => NodeKind::Scatter(sops::ScatterSettings {
                count: u(0).clamp(1, 1_000_000),
                seed: u(1) as u32,
                relax_iterations: u(2).min(50),
                volume: g(3) > 0.5,
            }),
            "boolean" => NodeKind::Boolean(boolean::BooleanSettings {
                operation: u(0).min(boolean::OPERATIONS.len() - 1),
                mode: u(1).min(boolean::MODES.len() - 1),
                resolution: u(2).clamp(16, 400),
            }),
            "material_fracture" => NodeKind::MaterialFracture(fracture::FractureSettings {
                material: u(0).min(fracture::MATERIALS.len() - 1),
                pieces: u(1).clamp(2, 500),
                seed: u(2) as u32,
                impact: v3(3),
                rings: u(6).clamp(1, 20),
                spokes: u(7).clamp(3, 64),
                grain_axis: u(8).min(2),
                splinter: g(9).max(1.0),
                separate: g(10).max(0.0),
            }),
            "crowd_solver" => NodeKind::CrowdSolver(crowd::CrowdSettings::from_params(p)),
            "muscle" => NodeKind::Muscle(muscle::MuscleSettings::from_params(p)),
            "muscle_solver" => NodeKind::MuscleSolver(muscle::JiggleSettings {
                stiffness: g(0).max(0.0),
                damping: g(1).max(0.0),
                smoothing: u(2).min(50),
                substeps: u(3).clamp(1, 16),
            }),
            "muscle_skin" => NodeKind::MuscleSkin(muscle::SkinSettings { falloff: g(0).max(1e-4), influences: u(1).clamp(1, 16) }),
            "terrain" => NodeKind::Terrain(terrain::TerrainSettings { size: [g(0).max(1e-3), g(1).max(1e-3)], resolution: u(2).clamp(2, 2048) }),
            "terrain_noise" => NodeKind::TerrainNoise(terrain::NoiseSettings {
                kind: u(0).min(terrain::NOISE_TYPES.len() - 1),
                blend: u(1).min(terrain::BLEND_MODES.len() - 1),
                amplitude: g(2),
                element_size: g(3).max(1e-3),
                octaves: u(4).clamp(1, 12),
                roughness: g(5).clamp(0.0, 1.0),
                offset: [g(6), g(7)],
                seed: u(8) as u32,
                positive: g(9) > 0.5,
                use_mask: g(10) > 0.5,
            }),
            "terrain_erode" => NodeKind::TerrainErode(terrain::ErodeSettings {
                droplets: u(0).min(5_000_000),
                erosion: g(1).clamp(0.0, 1.0),
                deposition: g(2).clamp(0.0, 1.0),
                capacity: g(3).max(0.0),
                evaporation: g(4).clamp(0.0, 1.0),
                inertia: g(5).clamp(0.0, 1.0),
                radius: u(6).clamp(1, 8),
                thermal_iterations: u(7).min(500),
                talus_angle: g(8).clamp(1.0, 89.0),
                seed: u(9) as u32,
            }),
            "terrain_terrace" => NodeKind::TerrainTerrace(terrain::TerraceSettings { step: g(0).max(1e-4), offset: g(1), sharpness: g(2).clamp(0.0, 1.0), use_mask: g(3) > 0.5 }),
            "terrain_mask" => NodeKind::TerrainMask(terrain::MaskSettings {
                source: u(0).min(terrain::MASK_SOURCES.len() - 1),
                min: g(1),
                max: g(2),
                falloff: g(3).max(0.0),
                blur: u(4).min(100),
                invert: g(5) > 0.5,
                combine: u(6).min(terrain::MASK_COMBINE.len() - 1),
                visualize: g(7) > 0.5,
            }),
            "terrain_colorize" => NodeKind::TerrainColorize(terrain::ColorizeSettings {
                sand: v3(0),
                grass: v3(3),
                rock: v3(6),
                snow: v3(9),
                sand_level: g(12),
                snow_level: g(13),
                rock_slope: g(14),
                blend: g(15).max(0.0),
            }),
            "uv_texture" => NodeKind::UvTexture(sops::UvTextureSettings {
                projection: u(0).min(sops::UV_PROJECTIONS.len() - 1),
                axis: u(1).min(2),
                scale: [g(2), g(3)],
                offset: [g(4), g(5)],
            }),
            "uv_transform" => NodeKind::UvTransform(sops::UvTransformSettings {
                translate: [g(0), g(1)],
                rotate: g(2),
                scale: [g(3), g(4)],
                pivot: [g(5), g(6)],
            }),
            "uv_quickshade" => NodeKind::UvQuickShade { file: String::new() },
            "vellum_solver" => {
                let kind = u(0).min(vellum::VELLUM_TYPES.len() - 1);
                let d = vellum::VellumSettings::defaults(kind);
                let or = |i: usize, default: f32| p.get(i).copied().unwrap_or(default);
                NodeKind::VellumSolver(vellum::VellumSettings {
                    kind,
                    stretch_stiffness: or(1, d.stretch_stiffness).max(1e-3),
                    bend_stiffness: or(2, d.bend_stiffness).max(1e-3),
                    shape_stiffness: or(3, d.shape_stiffness).clamp(0.0, 1.0),
                    mass: or(4, d.mass).max(1e-4),
                    thickness: or(5, d.thickness).max(1e-4),
                    gravity: or(6, d.gravity),
                    damping: or(7, d.damping).max(0.0),
                    friction: or(8, d.friction).clamp(0.0, 1.0),
                    substeps: (or(9, d.substeps as f32).max(1.0) as usize).min(100),
                    iterations: (or(10, d.iterations as f32).max(1.0) as usize).min(200),
                    ground: or(11, d.ground as u8 as f32) > 0.5,
                    self_collision: or(12, d.self_collision as u8 as f32) > 0.5,
                    pin_to_animation: or(13, d.pin_to_animation as u8 as f32) > 0.5,
                })
            }
            "sop_solver" => NodeKind::SopSolver { substeps: u(0).clamp(1, 16) },
            "sop_import" => NodeKind::LopSopImport { geometry: p.first().filter(|v| **v >= 0.0).map(|v| *v as NodeId), path: "/geo".into() },
            "usd_reference" => NodeKind::LopReference { file: String::new(), path: "/ref".into(), sublayer: g(0) != 0.0 },
            "usd_primitive" => NodeKind::LopPrimitive { shape: u(0).min(LOP_SHAPES.len() - 1), path: "/shapes/shape".into(), size: g(1).max(0.001), t: v3(2) },
            "usd_transform" => NodeKind::LopTransform { pattern: String::new(), t: v3(0), r: v3(3), s: v3(6) },
            "usd_light" => NodeKind::LopLight {
                kind: u(0).min(LOP_LIGHTS.len() - 1),
                path: "/lights/light".into(),
                position: v3(1),
                target: v3(4),
                color: v3(7),
                intensity: g(10).max(0.0),
                size: g(11).max(0.0),
            },
            "usd_camera" => NodeKind::LopCamera { path: "/cameras/cam".into(), position: v3(0), target: v3(3), focal: g(6).max(1.0), aperture: g(7).max(1.0) },
            "assign_material" => NodeKind::LopMaterial { pattern: String::new(), material: p.first().filter(|v| **v >= 0.0).map(|v| *v as NodeId) },
            "usd_prune" => NodeKind::LopPrune { pattern: String::new(), hide: g(0) != 0.0 },
            "usd_merge" => NodeKind::LopMerge,
            "usd_export" => NodeKind::LopExport { file: String::new() },
            "solver_input" => NodeKind::SolverInput { index: u(0).min(1) },
            "switch" => NodeKind::Switch { index: u(0) },
            "geometry" => NodeKind::Geometry {
                material: p.first().filter(|v| **v >= 0.0).map(|v| *v as usize),
            },
            "diffuse" | "metal" | "glass" | "emissive" => NodeKind::Material {
                kind: MATERIAL_NAMES.iter().position(|n| *n == name).unwrap_or(0),
                color: [g(1).clamp(0.0, 1000.0), g(2).clamp(0.0, 1000.0), g(3).clamp(0.0, 1000.0)],
                roughness: g(4).clamp(0.0, 1.0),
                value: g(5).max(0.0),
            },
            "principled_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Principled, p)),
            "toon_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Toon, p)),
            "hair_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Hair, p)),
            "pyro_shader" => NodeKind::Shader(shading::Values::from_params(shading::Table::Pyro, p)),
            "material_builder" => NodeKind::MaterialBuilder,
            "shader_node" => NodeKind::ShaderNode(shading::ShaderNode::from_params(p)),
            _ => return None,
        })
    }
}

// ============================================================================
// 4. GRAPH
// ============================================================================

#[derive(Clone)]
struct Node {
    kind: NodeKind,
    pos: egui::Pos2,
    bypass: bool,
    context: AppContext,
    // Geometry node that contains this node; None for top-level (OBJ/MAT/OUT) nodes.
    parent: Option<NodeId>,
    // Object-level transform of Geometry objects (scale, then rotate, then translate).
    xform: ObjectXform,
    // The group the node applies to (Houdini's Group parameter); empty = everything.
    group: groups::NodeGroup,
    // Values of the ch("...") parameters its code uses (wrangles, VOP networks).
    channels: vex::Channels,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ObjectXform {
    t: [f32; 3],
    r: [f32; 3],
    s: [f32; 3],
}

impl Default for ObjectXform {
    fn default() -> Self {
        Self { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] }
    }
}

impl ObjectXform {
    fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    fn apply(&self, mesh: Mesh) -> Mesh {
        if self.is_identity() {
            return mesh;
        }
        let mut out = node_transform(mesh, self.t, self.r, self.s);
        // Volumes keep their grid description: the origin moves with the object and the
        // voxel size follows the average scale (grids cannot represent rotation).
        if volume::is_volume(&out) {
            let scale = (self.s[0].abs() + self.s[1].abs() + self.s[2].abs()) / 3.0;
            if let Some(origin) = out.point_attributes.get_mut("detail:volume_origin").filter(|o| o.len() >= 3) {
                let moved = Vec3::new(origin[0], origin[1], origin[2]).mul(Vec3::from_arr(self.s)).rotate_deg(self.r).add(Vec3::from_arr(self.t));
                origin[..3].copy_from_slice(&[moved.x, moved.y, moved.z]);
            }
            for key in ["detail:volume_voxelsize", "pscale"] {
                if let Some(values) = out.point_attributes.get_mut(key) {
                    values.iter_mut().for_each(|v| *v *= scale);
                }
            }
        }
        out
    }
}

#[derive(Clone)]
struct Link {
    from: NodeId,
    to: NodeId,
    slot: usize,
}

// An animated parameter: a numeric parameter (index into NodeKind::params) or a
// component of an object's transform (tx ty tz rx ry rz sx sy sz).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum ChannelTarget {
    Param(usize),
    Xform(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ChannelId {
    node: NodeId,
    target: ChannelTarget,
}

const XFORM_NAMES: [&str; 9] = ["tx", "ty", "tz", "rx", "ry", "rz", "sx", "sy", "sz"];

fn xform_component(x: &ObjectXform, i: usize) -> f32 {
    [x.t, x.r, x.s].concat()[i]
}

fn set_xform_component(x: &mut ObjectXform, i: usize, v: f32) {
    match i {
        0..=2 => x.t[i] = v,
        3..=5 => x.r[i - 3] = v,
        _ => x.s[i - 6] = v,
    }
}

#[derive(Default, Clone)]
struct Graph {
    nodes: HashMap<NodeId, Node>,
    links: Vec<Link>,
    next_id: NodeId,
    // Display node of each Geometry container.
    displays: HashMap<NodeId, NodeId>,
    // Keyframed parameters.
    channels: std::collections::BTreeMap<ChannelId, anim::Channel>,
    // Values written by the last apply_animation, to tell user edits from playback.
    applied: HashMap<ChannelId, f32>,
}

impl Graph {
    fn add(&mut self, kind: NodeKind, pos: egui::Pos2) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(id, Node { kind, pos, bypass: false, context: AppContext::Obj, parent: None, xform: ObjectXform::default(), group: groups::NodeGroup::default(), channels: vex::Channels::new() });
        id
    }

    // What a background cook of `displays` is working on, for the status bar:
    // the solvers upstream of them ("Simulating Vellum + Pyro"), else an animated frame.
    fn cooking_label(&self, displays: &[NodeId]) -> String {
        let mut names: Vec<&'static str> = Vec::new();
        let mut seen = HashSet::new();
        let mut stack: Vec<NodeId> = displays.to_vec();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(node) = self.nodes.get(&id) else { continue };
            if !node.bypass {
                let name = match node.kind {
                    NodeKind::Flip { .. } => Some("FLIP"),
                    NodeKind::WhitewaterSolver(_) => Some("Whitewater"),
                    NodeKind::CrowdSolver(_) => Some("Crowd"),
                    NodeKind::MuscleSolver(_) => Some("Muscle"),
                    NodeKind::Pyro(_) => Some("Pyro"),
                    NodeKind::RbdSolver(_) => Some("RBD"),
                    NodeKind::VellumSolver(_) => Some("Vellum"),
                    NodeKind::SopSolver { .. } => Some("SOP Solver"),
                    NodeKind::Particles { .. } => Some("Particles"),
                    _ => None,
                };
                if let Some(name) = name.filter(|n| !names.contains(n)) {
                    names.push(name);
                }
            }
            // Containers (Geometry, SOP Solver) cook their inner display node.
            if node.kind.is_container() {
                stack.extend(self.display_of(id));
            }
            stack.extend(self.links.iter().filter(|l| l.to == id).map(|l| l.from));
        }
        if names.is_empty() {
            "Cooking the animated frame".to_owned()
        } else {
            names.reverse();
            format!("Simulating {}", names.join(" + "))
        }
    }

    fn display_of(&self, container: NodeId) -> Option<NodeId> {
        self.displays.get(&container).copied().filter(|d| self.nodes.contains_key(d))
    }

    // Network whose display flag a node can hold: its container, or the STAGE network for LOPs.
    fn display_network(&self, id: NodeId) -> Option<NodeId> {
        let node = self.nodes.get(&id)?;
        node.parent.or((node.context == AppContext::Stage).then_some(STAGE_NETWORK))
    }

    fn is_display(&self, id: NodeId) -> bool {
        self.display_network(id).is_some_and(|p| self.displays.get(&p) == Some(&id))
    }

    // Returns false for nodes outside a container or the STAGE network.
    fn set_display(&mut self, id: NodeId) -> bool {
        match self.display_network(id) {
            Some(network) => {
                self.displays.insert(network, id);
                true
            }
            None => false,
        }
    }

    fn children(&self, container: NodeId) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.parent == Some(container))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    // Top-level Geometry nodes in id order.
    fn geometry_ids(&self) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.parent.is_none() && matches!(n.kind, NodeKind::Geometry { .. }))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    fn node_channels(&self, id: NodeId) -> impl Iterator<Item = (&ChannelId, &anim::Channel)> {
        let lo = ChannelId { node: id, target: ChannelTarget::Param(0) };
        let hi = ChannelId { node: id, target: ChannelTarget::Xform(usize::MAX) };
        self.channels.range(lo..=hi)
    }

    fn is_animated(&self, id: NodeId) -> bool {
        self.node_channels(id).next().is_some()
    }

    // The node's parameters with its channels evaluated at `time`.
    fn kind_at(&self, id: NodeId, time: f32) -> std::borrow::Cow<'_, NodeKind> {
        let Some(node) = self.nodes.get(&id) else {
            return std::borrow::Cow::Owned(NodeKind::Null);
        };
        let mut params = node.kind.params();
        let mut any = false;
        for (channel_id, channel) in self.node_channels(id) {
            if let ChannelTarget::Param(i) = channel_id.target {
                if i < params.len() && !channel.keys.is_empty() {
                    params[i] = channel.eval(time);
                    any = true;
                }
            }
        }
        if !any {
            return std::borrow::Cow::Borrowed(&node.kind);
        }
        match NodeKind::from_params(node.kind.name(), &params) {
            Some(mut kind) => {
                kind.set_texts(&node.kind.texts());
                std::borrow::Cow::Owned(kind)
            }
            None => std::borrow::Cow::Borrowed(&node.kind),
        }
    }

    fn xform_at(&self, id: NodeId, time: f32) -> ObjectXform {
        let Some(node) = self.nodes.get(&id) else { return ObjectXform::default() };
        let mut x = node.xform;
        for (channel_id, channel) in self.node_channels(id) {
            if let (ChannelTarget::Xform(i), false) = (channel_id.target, channel.keys.is_empty()) {
                if i < 9 {
                    set_xform_component(&mut x, i, channel.eval(time));
                }
            }
        }
        x
    }

    fn channel_value(&self, id: ChannelId) -> Option<f32> {
        let node = self.nodes.get(&id.node)?;
        match id.target {
            ChannelTarget::Param(i) => node.kind.params().get(i).copied(),
            ChannelTarget::Xform(i) => (i < 9).then(|| xform_component(&node.xform, i)),
        }
    }

    // Writes every channel's value at `time` into its node, so panels, lights, cameras and
    // gizmos show the animated values.
    fn apply_animation(&mut self, time: f32) {
        self.applied.clear();
        let animated: std::collections::BTreeSet<NodeId> = self.channels.keys().map(|c| c.node).collect();
        for id in animated {
            if !self.nodes.contains_key(&id) {
                continue;
            }
            let kind = self.kind_at(id, time).into_owned();
            let xform = self.xform_at(id, time);
            let node = self.nodes.get_mut(&id).expect("checked");
            node.kind = kind;
            node.xform = xform;
            let ids: Vec<ChannelId> = self.node_channels(id).map(|(c, _)| *c).collect();
            for channel_id in ids {
                if let Some(v) = self.channel_value(channel_id) {
                    self.applied.insert(channel_id, v);
                }
            }
        }
    }

    // Keys every animated parameter that was changed since apply_animation (panel edits,
    // gizmo drags) at `time`. Returns whether any key changed.
    fn commit_animation_edits(&mut self, time: f32) -> bool {
        let mut changed = false;
        let applied: Vec<(ChannelId, f32)> = self.applied.iter().map(|(c, v)| (*c, *v)).collect();
        for (channel_id, before) in applied {
            let Some(now) = self.channel_value(channel_id) else { continue };
            if (now - before).abs() > 1e-4 * before.abs().max(1.0) {
                if let Some(channel) = self.channels.get_mut(&channel_id) {
                    channel.set_key(time, now);
                    self.applied.insert(channel_id, now);
                    changed = true;
                }
            }
        }
        changed
    }

    // Sets a key at `time` with the parameter's current value, creating the channel if needed.
    fn set_key(&mut self, id: ChannelId, time: f32) {
        if let Some(value) = self.channel_value(id) {
            self.channels.entry(id).or_default().set_key(time, value);
            self.applied.insert(id, value);
        }
    }

    // Fingerprint of everything that affects the scene except node positions (and `skip`,
    // the active camera, which follows the viewport while orbiting).
    fn content_signature(&self, skip: Option<NodeId>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut ids: Vec<&NodeId> = self.nodes.keys().filter(|id| Some(**id) != skip).collect();
        ids.sort_unstable();
        for id in ids {
            let node = &self.nodes[id];
            id.hash(&mut hasher);
            // Animated parameters are compared at time 0, so playback itself is not an edit.
            format!("{:?}{:?}{:?}", self.kind_at(*id, 0.0), node.parent, self.xform_at(*id, 0.0)).hash(&mut hasher);
            (node.bypass, node.context as u8).hash(&mut hasher);
        }
        for link in &self.links {
            (link.from, link.to, link.slot).hash(&mut hasher);
        }
        let mut displays: Vec<_> = self.displays.iter().collect();
        displays.sort_unstable();
        displays.hash(&mut hasher);
        for (channel_id, channel) in &self.channels {
            (channel_id, channel.to_text()).hash(&mut hasher);
        }
        hasher.finish()
    }

    // The Geometry object whose network (possibly through SOP Solvers) contains `id`.
    fn enclosing_geometry(&self, id: NodeId) -> Option<NodeId> {
        let mut current = self.nodes.get(&id)?.parent;
        while let Some(parent) = current {
            let node = self.nodes.get(&parent)?;
            if matches!(node.kind, NodeKind::Geometry { .. }) {
                return Some(parent);
            }
            current = node.parent;
        }
        None
    }

    // What the viewport shows for `ids`: like cook_many, but geometry objects without colour
    // attributes take their MAT material's colour, so the viewport matches renders.
    fn cook_display(&self, ids: &[NodeId], time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        let mut merged: Option<Mesh> = None;
        for id in ids {
            let mut mesh = self.cook(*id, time, cache);
            // Level-set VDBs are shown as their surface, like Houdini; fog volumes stay soft splats.
            if volume::is_volume(&mesh) {
                if let Some(grid) = volume::from_mesh(&mesh).ok().filter(|g| g.class == volume::Class::LevelSet) {
                    mesh = volume::to_polygons(&grid, 0.0);
                }
            }
            // Inside a geometry network the display node is shown where its object is.
            if !matches!(self.nodes.get(id).map(|n| &n.kind), Some(NodeKind::Geometry { .. })) {
                if let Some(object) = self.enclosing_geometry(*id) {
                    mesh = self.xform_at(object, time).apply(mesh);
                }
            }
            let material = match self.nodes.get(id).map(|n| &n.kind) {
                Some(NodeKind::Geometry { material: Some(m) }) => self.nodes.get(m).map(|n| &n.kind),
                _ => None,
            };
            let _ = material;
            let material = match self.nodes.get(id).map(|n| &n.kind) {
                Some(NodeKind::Geometry { material: Some(m) }) => self.material_preview(*m),
                _ => None,
            };
            if let Some((color, texture, uses_cd)) = material {
                let coloured = mesh.point_attributes.keys().any(|k| k == "Cd" || k.ends_with(":Cd"));
                if !(coloured && uses_cd) && !mesh.indices.is_empty() {
                    mesh.point_attributes.remove("Cd");
                    mesh.point_attributes.insert("prim:Cd".into(), color.repeat(mesh.indices.len()));
                }
                // Image textures show in the viewport like UV Quick Shade.
                if let Some(texture) = texture {
                    if let Ok(shaded) = sops::uv_quickshade(&mesh, &texture) {
                        mesh = shaded;
                    }
                }
            }
            merged = Some(match merged {
                Some(acc) => node_merge(acc, mesh),
                None => mesh,
            });
        }
        merged.unwrap_or_default()
    }

    fn cook_many(&self, ids: &[NodeId], time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        let mut merged: Option<Mesh> = None;
        for id in ids {
            let mesh = self.cook(*id, time, cache);
            merged = Some(match merged {
                Some(acc) => node_merge(acc, mesh),
                None => mesh,
            });
        }
        merged.unwrap_or_default()
    }

    fn remove(&mut self, id: NodeId) {
        for child in self.children(id) {
            self.remove(child);
        }
        self.nodes.remove(&id);
        self.links.retain(|l| l.from != id && l.to != id);
        self.displays.remove(&id);
        self.displays.retain(|_, d| *d != id);
        self.channels.retain(|c, _| c.node != id);
    }

    fn copy_children(&mut self, from: NodeId, to: NodeId) {
        let mut map: HashMap<NodeId, NodeId> = HashMap::new();
        for child in self.children(from) {
            let Some(src) = self.nodes.get(&child).cloned() else {
                continue;
            };
            let new = self.add(src.kind, src.pos);
            if let Some(node) = self.nodes.get_mut(&new) {
                node.bypass = src.bypass;
                node.context = src.context;
                node.parent = Some(to);
            }
            map.insert(child, new);
        }
        let links: Vec<Link> = self.links.clone();
        for l in links {
            if let (Some(a), Some(b)) = (map.get(&l.from), map.get(&l.to)) {
                self.links.push(Link { from: *a, to: *b, slot: l.slot });
            }
        }
        if let Some(display) = self.displays.get(&from).and_then(|d| map.get(d)) {
            self.displays.insert(to, *display);
        }
    }

    fn duplicate(&mut self, id: NodeId) -> Option<NodeId> {
        let n = self.nodes.get(&id)?.clone();
        let is_geometry = matches!(n.kind, NodeKind::Geometry { .. });
        let new = self.add(n.kind, n.pos + egui::vec2(30.0, 60.0));
        if let Some(node) = self.nodes.get_mut(&new) {
            node.bypass = n.bypass;
            node.context = n.context;
            node.parent = n.parent;
            node.xform = n.xform;
        }
        let copied: Vec<(ChannelTarget, anim::Channel)> = self.node_channels(id).map(|(c, ch)| (c.target, ch.clone())).collect();
        for (target, channel) in copied {
            self.channels.insert(ChannelId { node: new, target }, channel);
        }
        if self.nodes.get(&new).is_some_and(|n| n.kind.is_container()) {
            self.copy_children(id, new);
            if !is_geometry {
                let inputs: Vec<Link> = self.links.iter().filter(|l| l.to == id).cloned().collect();
                for l in inputs {
                    self.links.push(Link { from: l.from, to: new, slot: l.slot });
                }
            }
            return Some(new);
        }
        let inputs: Vec<Link> = self.links.iter().filter(|l| l.to == id).cloned().collect();
        for l in inputs {
            self.links.push(Link { from: l.from, to: new, slot: l.slot });
        }
        Some(new)
    }

    fn input_of(&self, to: NodeId, slot: usize) -> Option<NodeId> {
        self.links.iter().find(|l| l.to == to && l.slot == slot).map(|l| l.from)
    }

    fn depends_on(&self, node: NodeId, candidate: NodeId) -> bool {
        node == candidate
            || self
                .links
                .iter()
                .filter(|l| l.to == node)
                .any(|l| self.depends_on(l.from, candidate))
    }

    fn connect(&mut self, from: NodeId, to: NodeId, slot: usize) -> bool {
        if self.depends_on(from, to) {
            return false;
        }
        self.links.retain(|l| !(l.to == to && l.slot == slot));
        self.links.push(Link { from, to, slot });
        true
    }

    /// True if this node or anything upstream animates with time.
    // Records a node's cook error for its parameter panel; errors cook to empty geometry.
    fn report(&self, id: NodeId, result: Result<Mesh, String>) -> Mesh {
        node_errors(|errors| match result {
            Ok(mesh) => {
                errors.remove(&id);
                mesh
            }
            Err(error) => {
                errors.insert(id, error);
                Mesh::default()
            }
        })
    }

    // VDB from Polygons as a level set volume: OpenVDB when built with it, else the Rust rasterizer.
    #[allow(clippy::too_many_arguments)]
    fn vdb_volume(&self, id: NodeId, source: Mesh, time: f32, voxel: f32, padding: usize, operation: usize, iterations: usize) -> Result<Mesh, String> {
        #[cfg(feature = "native-openvdb")]
        if !volume::is_volume(&source) {
            if let Some(native) = self.cook_native_volume(id, time, &mut HashMap::new()) {
                if let Some(mesh) = native_volume_voxels(&native) {
                    return Ok(mesh);
                }
            }
        }
        let _ = (id, time);
        let mut grid = if volume::is_volume(&source) {
            volume::from_mesh(&source)?
        } else {
            volume::from_polygons(&source, voxel, 3.0 + padding as f32)?
        };
        let shift = iterations as f32 * grid.voxel;
        match operation {
            1 => volume::offset(&mut grid, shift),
            2 => volume::offset(&mut grid, -shift),
            3 => volume::smooth(&mut grid, iterations),
            _ => {}
        }
        Ok(volume::to_mesh(&grid))
    }

    // The USD stage a LOP node produces from its inputs.
    fn cook_stage(&self, id: NodeId, time: f32, cache: &mut HashMap<NodeId, Arc<usd::Stage>>) -> Arc<usd::Stage> {
        if let Some(stage) = cache.get(&id) {
            return stage.clone();
        }
        let Some(node) = self.nodes.get(&id) else {
            return Arc::default();
        };
        let input = |slot: usize, cache: &mut HashMap<NodeId, Arc<usd::Stage>>| {
            self.input_of(id, slot)
                .filter(|src| self.nodes.get(src).is_some_and(|n| n.kind.is_lop()))
                .map(|src| self.cook_stage(src, time, cache))
                .unwrap_or_default()
        };
        let mut error = None;
        let stage = if node.bypass {
            input(0, cache)
        } else {
            let mut stage = (*input(0, cache)).clone();
            let animated = self.kind_at(id, time);
            match animated.as_ref() {
                NodeKind::LopSopImport { geometry, path } => {
                    let objects = match geometry {
                        Some(g) => vec![*g],
                        None => self.geometry_ids().into_iter().filter(|g| !self.nodes[g].bypass).collect(),
                    };
                    for object in objects {
                        let mesh = self.cook(object, time, &mut HashMap::new());
                        if mesh.vertices.is_empty() {
                            continue;
                        }
                        let prim_path = if geometry.is_some() { path.clone() } else { format!("{path}/geometry{object}") };
                        let material = match self.nodes.get(&object).map(|n| &n.kind) {
                            Some(NodeKind::Geometry { material: Some(m) }) => self.material_prim(&mut stage, *m),
                            _ => None,
                        };
                        let prim = stage.define(&prim_path, "Mesh");
                        prim.mesh = Some(Arc::new(mesh));
                        prim.binding = material;
                    }
                }
                NodeKind::LopReference { file, path, sublayer } => match usd::load_file(file, time) {
                    Ok(loaded) if *sublayer => stage.merge(&loaded),
                    Ok(loaded) => stage.merge(&loaded.rerooted(path)),
                    Err(e) => error = Some(e),
                },
                NodeKind::LopPrimitive { shape, path, size, t } => {
                    let shape = (*shape).min(LOP_SHAPES.len() - 1);
                    let prim = stage.define(path, LOP_SHAPES[shape]);
                    prim.xform = usd::trs(*t, [0.0; 3], [1.0; 3]);
                    let size = size.max(0.001);
                    let attrs: &[(&str, f32)] = match shape {
                        0 => &[("radius", size)],
                        1 => &[("size", size * 2.0)],
                        4 => &[("radius", size * 0.5), ("height", size)],
                        _ => &[("radius", size), ("height", size * 2.0)],
                    };
                    for (name, value) in attrs {
                        prim.attrs.insert(name.to_string(), usd::AttrValue::Float(*value));
                    }
                    if shape >= 2 {
                        prim.attrs.insert("axis".into(), usd::AttrValue::Token("Y".into()));
                    }
                }
                NodeKind::LopTransform { pattern, t, r, s } => {
                    let m = usd::trs(*t, *r, *s);
                    for (path, prim) in stage.prims.iter_mut() {
                        if usd::matches_pattern(pattern, path) {
                            prim.xform = usd::mat_mul(&prim.xform, &m);
                        }
                    }
                }
                NodeKind::LopLight { kind, path, position, target, color, intensity, size } => {
                    let kind = (*kind).min(LOP_LIGHTS.len() - 1);
                    let prim = stage.define(path, LOP_LIGHTS[kind]);
                    prim.xform = if kind == 2 { usd::IDENTITY } else { usd::look_at(*position, *target) };
                    prim.attrs.insert("intensity".into(), usd::AttrValue::Float(*intensity));
                    prim.attrs.insert("color".into(), usd::AttrValue::Vec3(*color));
                    let sizes: &[&str] = match kind {
                        0 => &["angle"],
                        1 => &["radius"],
                        3 => &["width", "height"],
                        _ => &[],
                    };
                    for name in sizes {
                        prim.attrs.insert(name.to_string(), usd::AttrValue::Float(*size));
                    }
                }
                NodeKind::LopCamera { path, position, target, focal, aperture } => {
                    let prim = stage.define(path, "Camera");
                    prim.xform = usd::look_at(*position, *target);
                    prim.attrs.insert("focalLength".into(), usd::AttrValue::Float(*focal));
                    prim.attrs.insert("horizontalAperture".into(), usd::AttrValue::Float(*aperture));
                    prim.attrs.insert("verticalAperture".into(), usd::AttrValue::Float(aperture * 9.0 / 16.0));
                }
                NodeKind::LopMaterial { pattern, material } => match material.and_then(|m| self.material_prim(&mut stage, m)) {
                    Some(material_path) => {
                        for (path, prim) in stage.prims.iter_mut() {
                            if prim.type_name != "Material" && usd::matches_pattern(pattern, path) {
                                prim.binding = Some(material_path.clone());
                            }
                        }
                    }
                    None => error = Some("choose a MAT material to assign".into()),
                },
                NodeKind::LopPrune { pattern, hide } => {
                    for (path, prim) in stage.prims.iter_mut() {
                        if usd::matches_pattern(pattern, path) {
                            if *hide {
                                prim.visible = false;
                            } else {
                                prim.active = false;
                            }
                        }
                    }
                }
                NodeKind::LopMerge => {
                    let second = input(1, cache);
                    stage.merge(&second);
                }
                _ => {}
            }
            Arc::new(stage)
        };
        node_errors(|errors| match error {
            Some(e) => {
                errors.insert(id, e);
            }
            None => {
                errors.remove(&id);
            }
        });
        cache.insert(id, stage.clone());
        stage
    }

    // The Surface Output a Material Builder renders: its display node, else its first one.
    fn builder_output(&self, builder: NodeId) -> Option<NodeId> {
        let is_output = |id: &NodeId| matches!(self.nodes.get(id).map(|n| &n.kind), Some(NodeKind::ShaderNode(n)) if n.op == shading::SURFACE_OUTPUT);
        self.display_of(builder).filter(is_output).or_else(|| {
            let mut outputs: Vec<NodeId> = self.children(builder).into_iter().filter(is_output).collect();
            outputs.sort_unstable();
            outputs.first().copied()
        })
    }

    fn builder_nodes(&self, builder: NodeId) -> HashMap<NodeId, shading::ShaderNode> {
        self.children(builder)
            .into_iter()
            .filter_map(|child| match &self.nodes.get(&child)?.kind {
                NodeKind::ShaderNode(n) if !self.nodes[&child].bypass => Some((child, n.clone())),
                _ => None,
            })
            .collect()
    }

    // What the viewport shows for a MAT node: (colour, image texture, whether point Cd is used).
    fn material_preview(&self, material: NodeId) -> Option<([f32; 3], Option<String>, bool)> {
        match &self.nodes.get(&material)?.kind {
            NodeKind::Material { color, .. } => Some((*color, None, true)),
            NodeKind::Shader(v) => {
                let texture = (v.table == shading::Table::Principled && !v.texture(0).is_empty()).then(|| v.texture(0).to_owned());
                Some((v.viewport_color(), texture, v.table == shading::Table::Principled && v.flag("use_point_color")))
            }
            NodeKind::MaterialBuilder => {
                let nodes = self.builder_nodes(material);
                let output = self.builder_output(material)?;
                let (colour, texture) = shading::builder_preview(&nodes, |to, slot| self.input_of(to, slot), output);
                Some((colour, texture, false))
            }
            _ => None,
        }
    }

    // Adds a UsdPreviewSurface Material prim for a MAT network node; returns its path.
    fn material_prim(&self, stage: &mut usd::Stage, material: NodeId) -> Option<String> {
        let surface = match &self.nodes.get(&material)?.kind {
            NodeKind::Shader(v) if v.table == shading::Table::Principled => Some(v.clone()),
            NodeKind::MaterialBuilder => self.builder_output(material).and_then(|o| match &self.nodes.get(&o)?.kind {
                NodeKind::ShaderNode(n) => Some(n.surface.clone()),
                _ => None,
            }),
            _ => None,
        };
        if let Some(v) = surface {
            let path = format!("/materials/mat{material}");
            let prim = stage.define(&path, "Material");
            prim.material_node = Some(material);
            let attrs = &mut prim.attrs;
            let (colour, _, _) = self.material_preview(material)?;
            attrs.insert("diffuseColor".into(), usd::AttrValue::Vec3(colour));
            attrs.insert("roughness".into(), usd::AttrValue::Float(v.float("roughness")));
            attrs.insert("metallic".into(), usd::AttrValue::Float(v.float("metallic")));
            attrs.insert("ior".into(), usd::AttrValue::Float(v.float("ior")));
            attrs.insert("clearcoat".into(), usd::AttrValue::Float(v.float("clearcoat")));
            let opacity = v.float("opacity") * (1.0 - 0.8 * v.float("transmission"));
            if opacity < 1.0 {
                attrs.insert("opacity".into(), usd::AttrValue::Float(opacity));
            }
            if v.float("emission_intensity") > 0.0 {
                attrs.insert("emissiveColor".into(), usd::AttrValue::Vec3(v.color("emission_color").map(|c| c * v.float("emission_intensity"))));
            }
            return Some(path);
        }
        if let Some((colour, _, _)) = self.material_preview(material).filter(|_| !matches!(self.nodes[&material].kind, NodeKind::Material { .. })) {
            // Toon, hair and pyro shaders reach USD as their colour.
            let path = format!("/materials/mat{material}");
            let prim = stage.define(&path, "Material");
            prim.material_node = Some(material);
            prim.attrs.insert("diffuseColor".into(), usd::AttrValue::Vec3(colour));
            return Some(path);
        }
        let NodeKind::Material { kind, color, roughness, value } = self.nodes.get(&material)?.kind else {
            return None;
        };
        let path = format!("/materials/mat{material}");
        let prim = stage.define(&path, "Material");
        prim.material_node = Some(material);
        let attrs = &mut prim.attrs;
        attrs.insert("diffuseColor".into(), usd::AttrValue::Vec3(color));
        attrs.insert("roughness".into(), usd::AttrValue::Float(roughness));
        attrs.insert("metallic".into(), usd::AttrValue::Float(if kind == 1 { 1.0 } else { 0.0 }));
        if kind == 2 {
            attrs.insert("opacity".into(), usd::AttrValue::Float(0.2));
            attrs.insert("ior".into(), usd::AttrValue::Float(value.max(1.0)));
        }
        if kind == 3 {
            attrs.insert("emissiveColor".into(), usd::AttrValue::Vec3(color.map(|c| c * value)));
        }
        Some(path)
    }

    // The VEX an Attribute VOP network compiles to.
    fn vop_code(&self, id: NodeId) -> Result<String, String> {
        let nodes: HashMap<vop::NodeId, vop::VopNode> = self
            .children(id)
            .into_iter()
            .filter_map(|child| match &self.nodes.get(&child)?.kind {
                NodeKind::Vop(v) if !self.nodes[&child].bypass => Some((child, v.clone())),
                _ => None,
            })
            .collect();
        vop::compile(&nodes, |to, slot| self.input_of(to, slot).filter(|src| nodes.contains_key(src)))
    }

    // The input restricted to the node's group of primitives (all of it when there's no group).
    fn group_prims(&self, mesh: &Mesh, group: &groups::NodeGroup) -> Result<Mesh, String> {
        match groups::prim_mask(mesh, group)? {
            Some(prims) => Ok(groups::only_prims(mesh, &prims)),
            None => Ok(mesh.clone()),
        }
    }

    // Cooks `node` at `time` with its own cache (solvers cook inputs at other times than the frame).
    fn cook_fresh(&self, node: Option<NodeId>, time: f32) -> Mesh {
        node.map(|n| self.cook(n, time, &mut HashMap::new())).unwrap_or_default()
    }

    // One SOP Solver step: cooks the solver's display node with Prev_Frame bound to `prev`.
    fn sop_solver_step(&self, solver: NodeId, prev: &Mesh, time: f32, time_inc: f32) -> Mesh {
        let Some(display) = self.display_of(solver) else {
            return prev.clone();
        };
        SOLVER_PREV_FRAME.with(|frames| frames.borrow_mut().insert(solver, prev.clone()));
        let outer_inc = SOLVER_TIME_INC.with(|inc| inc.replace(time_inc));
        let out = self.cook(display, time, &mut HashMap::new());
        SOLVER_TIME_INC.with(|inc| inc.set(outer_inc));
        SOLVER_PREV_FRAME.with(|frames| frames.borrow_mut().remove(&solver));
        out
    }

    // Identifies a solver's simulation by everything that affects it: its settings,
    // its upstream nodes and, for containers, their networks. Node positions are excluded,
    // so moving nodes keeps cached simulations while any parameter edit restarts them.
    fn sim_key(&self, id: NodeId) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hasher);
        self.hash_upstream(id, &mut hasher, &mut HashSet::new());
        hasher.finish()
    }

    fn hash_upstream(&self, id: NodeId, hasher: &mut std::collections::hash_map::DefaultHasher, seen: &mut HashSet<NodeId>) {
        use std::hash::Hash;
        id.hash(hasher);
        if !seen.insert(id) {
            return;
        }
        let Some(node) = self.nodes.get(&id) else { return };
        format!("{:?}", node.kind).hash(hasher);
        format!("{:?}", node.group).hash(hasher);
        format!("{:?}", node.channels).hash(hasher);
        node.bypass.hash(hasher);
        let mut inputs: Vec<&Link> = self.links.iter().filter(|l| l.to == id).collect();
        inputs.sort_by_key(|l| l.slot);
        for link in inputs {
            link.slot.hash(hasher);
            self.hash_upstream(link.from, hasher, seen);
        }
        if node.kind.is_container() {
            self.display_of(id).hash(hasher);
            for child in self.children(id) {
                self.hash_upstream(child, hasher, seen);
            }
        }
    }

    fn time_dependent(&self, id: NodeId) -> bool {
        let Some(n) = self.nodes.get(&id) else {
            return false;
        };
        (!n.bypass && matches!(
            n.kind,
            NodeKind::Noise { .. }
                | NodeKind::Ripple { .. }
                | NodeKind::Particles { .. }
                | NodeKind::Flip { .. }
                | NodeKind::WhitewaterSolver(_)
                | NodeKind::CrowdSolver(_)
                | NodeKind::MuscleSolver(_)
                | NodeKind::Cache { .. }
                | NodeKind::Pyro(_)
                | NodeKind::RbdSolver(_)
                | NodeKind::VellumSolver(_)
                | NodeKind::SopSolver { .. }
                | NodeKind::SolverInput { .. }
        ))
            || matches!(&n.kind, NodeKind::Wrangle { code, .. } if code.contains("@Time") || code.contains("@Frame") || code.contains("@SimTime") || code.contains("@SimFrame"))
            || (matches!(n.kind, NodeKind::AttributeVop { .. })
                && self.vop_code(id).is_ok_and(|code| ["@Time", "@Frame", "@SimTime", "@SimFrame"].iter().any(|t| code.contains(t))))
            || self.is_animated(id)
            || match &n.kind {
                NodeKind::LopSopImport { geometry: Some(g), .. } => self.time_dependent(*g),
                NodeKind::LopSopImport { geometry: None, .. } => self.geometry_ids().into_iter().any(|g| self.time_dependent(g)),
                NodeKind::LopReference { file, .. } => usd::is_animated(file),
                NodeKind::FileImport { file } => {
                    fileio::has_frame_token(file) || (matches!(fileio::extension(file).as_str(), "usd" | "usda" | "usdc" | "usdz") && usd::is_animated(file))
                }
                _ => false,
            }
            || (!n.bypass
                && matches!(n.kind, NodeKind::Geometry { .. })
                && self.display_of(id).is_some_and(|d| self.time_dependent(d)))
            || self
                .links
                .iter()
                .filter(|l| l.to == id)
                .any(|l| self.time_dependent(l.from))
    }

    fn disconnect(&mut self, to: NodeId, slot: usize) {
        self.links.retain(|l| !(l.to == to && l.slot == slot));
    }

    #[cfg(feature = "native-openvdb")]
    fn cook_native_volume(
        &self,
        id: NodeId,
        time: f32,
        cache: &mut HashMap<NodeId, NativeVolume>,
    ) -> Option<NativeVolume> {
        if let Some(volume) = cache.get(&id) {
            return Some(volume.clone());
        }
        let node = self.nodes.get(&id)?;
        let NodeKind::Vdb { voxel_size, padding, operation, iterations, .. } = &node.kind else {
            return None;
        };
        let source = self.input_of(id, 0)?;
        let mut volume = if let Some(upstream) = self.cook_native_volume(source, time, cache) {
            upstream
        } else {
            let mut mesh_cache = HashMap::new();
            native_volume_from_mesh(
                &self.cook(source, time, &mut mesh_cache),
                *voxel_size,
                *padding,
            )?
        };
        if *operation != 0
            && unsafe {
                mini_openvdb_volume_operation(volume.0, *operation as u32, *iterations as u32)
            } == 0
        {
            return None;
        }
        cache.insert(id, volume.clone());
        Some(volume)
    }

    fn cook(&self, id: NodeId, time: f32, cache: &mut HashMap<NodeId, Mesh>) -> Mesh {
        if let Some(m) = cache.get(&id) {
            return m.clone();
        }
        let Some(node) = self.nodes.get(&id) else {
            return Mesh::default();
        };
        let input = |slot: usize, cache: &mut HashMap<NodeId, Mesh>| {
            self.input_of(id, slot)
                .map(|src| self.cook(src, time, cache))
                .unwrap_or_default()
        };
        let out = if node.bypass {
            if node.kind.num_inputs() > 0 {
                input(0, cache)
            } else {
                Mesh::default()
            }
        } else {
            let animated = self.kind_at(id, time);
            match animated.as_ref() {
                NodeKind::Add { count, spacing, layout } => node_add(*count, *spacing, *layout),
                NodeKind::Grid { size, res } => node_grid(*size, *size, *res, *res),
                NodeKind::Plane { size, res } => node_grid(*size, *size, *res, *res),
                NodeKind::Cone { radius, height, rows, cols, caps } => {
                    node_cone(*radius, *height, *rows, *cols, *caps)
                }
                NodeKind::Capsule { radius, length, rows, cols } => {
                    node_capsule(*radius, *length, *rows, *cols)
                }
                NodeKind::Box { size, div } => node_box(*size, *div),
                NodeKind::Sphere { radius, rows, cols } => node_sphere(*radius, *rows, *cols),
                NodeKind::Torus { major, minor, rows, cols } => {
                    node_torus(*major, *minor, *rows, *cols)
                }
                NodeKind::Transform { t, r, s } => node_transform(input(0, cache), *t, *r, *s),
                NodeKind::Noise { freq, amp } => {
                    node_noise_displace(input(0, cache), *freq, *amp, time)
                }
                NodeKind::Taper { factor } => node_taper(input(0, cache), *factor),
                NodeKind::Twist { angle } => node_twist(input(0, cache), *angle),
                NodeKind::Bend { angle } => node_bend(input(0, cache), *angle),
                NodeKind::Smooth { iterations, strength } => {
                    node_smooth(input(0, cache), *iterations, *strength)
                }
                NodeKind::Bevel { amount, segments } => {
                    node_bevel(input(0, cache), *amount, *segments)
                }
                NodeKind::Fuse { tol } => node_fuse(input(0, cache), *tol),
                NodeKind::Subdivide { levels } => node_subdivide(input(0, cache), *levels),
                NodeKind::Remesh { target } => node_remesh(input(0, cache), *target),
                NodeKind::Mirror { axis } => node_mirror(input(0, cache), *axis),
                NodeKind::Copy { count, t, r, s } => node_copy(input(0, cache), *count, *t, *r, *s),
                NodeKind::CopyToPoints { scale, rotation } => {
                    node_copy_to_points(input(0, cache), input(1, cache), *scale, *rotation)
                }
                NodeKind::AttributeCreate { attribute, value } => {
                    node_attribute_create(input(0, cache), *attribute, *value)
                }
                NodeKind::AttributeRandomize { attribute, amount, seed } => {
                    node_attribute_randomize(input(0, cache), *attribute, *amount, *seed)
                }
                NodeKind::AttributeDelete { attribute } => {
                    node_attribute_delete(input(0, cache), *attribute)
                }
                NodeKind::AttributeVop { domain } => match self.vop_code(id) {
                    Ok(code) => {
                        let source = input(0, cache);
                        let mask = match *domain {
                            2 => groups::point_mask(&source, &node.group),
                            1 => groups::prim_mask(&source, &node.group),
                            3 => groups::prim_mask(&source, &node.group).map(|m| m.map(|m| m.iter().flat_map(|&b| [b; 3]).collect())),
                            _ => Ok(None),
                        };
                        match mask {
                            Ok(mask) => {
                                node_errors(|e| e.remove(&id));
                                node_wrangle(source, &code, time, *domain, mask.as_deref(), Some(&node.channels))
                            }
                            Err(e) => self.report(id, Err(e)),
                        }
                    }
                    Err(e) => self.report(id, Err(e)),
                },
                // VOPs only exist to be compiled by their Attribute VOP.
                NodeKind::Vop(_) => Mesh::default(),
                NodeKind::Wrangle { code, domain } => {
                    let source = input(0, cache);
                    // Points (2) and primitives (1) run on the group's members; vertices on its primitives'.
                    let mask = match *domain {
                        2 => groups::point_mask(&source, &node.group),
                        1 => groups::prim_mask(&source, &node.group),
                        3 => groups::prim_mask(&source, &node.group).map(|m| m.map(|m| m.iter().flat_map(|&b| [b; 3]).collect())),
                        _ => Ok(None),
                    };
                    match mask {
                        Ok(mask) => node_wrangle(source, code, time, *domain, mask.as_deref(), Some(&node.channels)),
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::Merge => node_merge(input(0, cache), input(1, cache)),
                NodeKind::Null => input(0, cache),
                NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                    node_tube(*r_bot, *r_top, *height, *rows, *cols, *caps)
                }
                NodeKind::Peak { dist } => node_peak(input(0, cache), *dist),
                NodeKind::Ripple { freq, amp, speed } => {
                    node_ripple(input(0, cache), *freq, *amp, *speed, time)
                }
                NodeKind::Jitter { amp, seed } => node_jitter(input(0, cache), *amp, *seed as u32),
                NodeKind::Particles {
                    count,
                    emission_rate,
                    lifetime,
                    speed,
                    spread,
                    gravity,
                    seed,
                    drag,
                    bounce,
                    friction,
                    radius,
                    substeps,
                    along_normals,
                } => {
                    let settings = sims::ParticleSettings {
                        max_particles: *count,
                        rate: *emission_rate,
                        lifetime: *lifetime,
                        speed: *speed,
                        spread: *spread,
                        gravity: *gravity,
                        drag: *drag,
                        bounce: *bounce,
                        friction: *friction,
                        radius: *radius,
                        substeps: *substeps,
                        along_normals: *along_normals,
                        seed: *seed as u32,
                    };
                    let (source, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    let animated = source.is_some_and(|s| self.time_dependent(s));
                    sims::particles(
                        self.sim_key(id),
                        &settings,
                        time,
                        animated,
                        |t| self.cook_fresh(source, t),
                        || self.cook_fresh(collider, 0.0),
                    )
                }
                NodeKind::Flip {
                    particle_count,
                    grid_resolution,
                    flip_ratio,
                    viscosity,
                    gravity,
                    max_dt,
                    substeps,
                    seed,
                } => node_flip(
                    self.sim_key(id),
                    input(0, cache),
                    || self.cook_fresh(self.input_of(id, 1), 0.0),
                    *particle_count,
                    *grid_resolution,
                    *flip_ratio,
                    *viscosity,
                    *gravity,
                    *max_dt,
                    *substeps,
                    *seed,
                    time,
                ),
                NodeKind::WhitewaterSolver(settings) => {
                    let (fluid, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    if fluid.is_none() {
                        self.report(id, Err("connect a FLIP fluid to input 1".to_owned()))
                    } else {
                        whitewater::whitewater(self.sim_key(id), settings, time, |t| self.cook_fresh(fluid, t), || self.cook_fresh(collider, 0.0))
                    }
                }
                NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                    node_particle_surface(
                        input(0, cache),
                        *radius,
                        *join_distance,
                        *resolution,
                        *smoothing,
                    )
                }
                NodeKind::Extrude { dist, inset, mode } => {
                    node_extrude(input(0, cache), *dist, *inset, *mode)
                }
                NodeKind::Clip { axis, offset, invert } => node_clip(input(0, cache), *axis, *offset, *invert),
                NodeKind::Blast { component, indices, invert } if !node.group.is_empty() => {
                    // Blast deletes the node's group (points or primitives).
                    let source = input(0, cache);
                    let as_points = groups::guess_domain(&source, &node.group.pattern) == groups::GroupDomain::Points && node.group.kind != 2 || node.group.kind == 1;
                    let mask = if as_points { groups::point_mask(&source, &node.group) } else { groups::prim_mask(&source, &node.group) };
                    let _ = (component, indices);
                    match mask {
                        Ok(Some(mask)) => {
                            let chosen: Vec<usize> = mask.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect();
                            self.report(id, Ok(node_blast(source, if as_points { 0 } else { 1 }, &chosen, *invert)))
                        }
                        Ok(None) => source,
                        Err(e) => self.report(id, Err(e)),
                    }
                }
                NodeKind::Blast { component, indices, invert } => {
                    node_blast(input(0, cache), *component, indices, *invert)
                }
                NodeKind::Cache { cache_id, mode } => {
                    let path = cache_file_path(*cache_id, time);
                    if *mode == 0 {
                        load_mesh_cache(&path).unwrap_or_else(|| input(0, cache))
                    } else {
                        let mesh = input(0, cache);
                        if let Err(error) = save_mesh_cache(&path, &mesh) {
                            eprintln!("Could not write cache {}: {error}", path.display());
                        }
                        mesh
                    }
                }
                NodeKind::Vdb { voxel_size, padding, operation, iterations, output: 1 } => {
                    let result = self.vdb_volume(id, input(0, cache), time, *voxel_size, *padding, *operation, *iterations);
                    self.report(id, result)
                }
                NodeKind::ConvertVdb { mode, iso } => {
                    let source = input(0, cache);
                    let result = volume::from_mesh(&source).map(|v| match mode {
                        0 => volume::to_polygons(&v, *iso),
                        _ => volume::to_mesh(&volume::to_fog(&v)),
                    });
                    self.report(id, result)
                }
                NodeKind::FileImport { file } => {
                    let path = fileio::expand_frame(file.trim(), time, sims::SIM_FPS);
                    let result = read_geometry_file(&path, time);
                    self.report(id, result)
                }
                NodeKind::FileExport { .. } => input(0, cache),
                NodeKind::Vdb { voxel_size, padding, operation, iterations, .. } => {
                    #[cfg(feature = "native-openvdb")]
                    {
                        let mut volume_cache = HashMap::new();
                        if let Some(volume) = self.cook_native_volume(id, time, &mut volume_cache) {
                            if let Some(mesh) = native_volume_to_mesh(&volume) {
                                mesh
                            } else {
                                node_vdb(input(0, cache), *voxel_size, *padding, *operation, *iterations)
                            }
                        } else {
                            node_vdb(input(0, cache), *voxel_size, *padding, *operation, *iterations)
                        }
                    }
                    #[cfg(not(feature = "native-openvdb"))]
                    {
                        node_vdb(input(0, cache), *voxel_size, *padding, *operation, *iterations)
                    }
                }
                NodeKind::MoonRayRender { .. } => input(0, cache),
                NodeKind::Camera { .. }
                | NodeKind::Light { .. }
                | NodeKind::EnvironmentLight { .. }
                | NodeKind::Material { .. }
                | NodeKind::Shader(_)
                | NodeKind::MaterialBuilder
                | NodeKind::ShaderNode(_) => Mesh::default(),
                NodeKind::Geometry { .. } => match self.display_of(id) {
                    Some(display) => self.xform_at(id, time).apply(self.cook(display, time, cache)),
                    None => Mesh::default(),
                },
                NodeKind::Reverse => node_reverse(input(0, cache)),
                NodeKind::LopSopImport { .. }
                | NodeKind::LopReference { .. }
                | NodeKind::LopPrimitive { .. }
                | NodeKind::LopTransform { .. }
                | NodeKind::LopLight { .. }
                | NodeKind::LopCamera { .. }
                | NodeKind::LopMaterial { .. }
                | NodeKind::LopPrune { .. }
                | NodeKind::LopMerge
                | NodeKind::LopExport { .. } => self.cook_stage(id, time, &mut HashMap::new()).to_mesh(),
                NodeKind::Pyro(settings) => {
                    let (source, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    let animated = source.is_some_and(|s| self.time_dependent(s));
                    sims::pyro(
                        self.sim_key(id),
                        settings,
                        time,
                        animated,
                        |t| self.cook_fresh(source, t),
                        || self.cook_fresh(collider, 0.0),
                    )
                }
                NodeKind::HairGuides(settings) => match self.group_prims(&input(0, cache), &node.group) {
                    Ok(skin) => groom::hair_guides(&skin, settings),
                    Err(e) => self.report(id, Err(e)),
                },
                NodeKind::GuideProcess(settings) => groom::guide_process(&input(0, cache), settings),
                NodeKind::HairGenerate(settings) => match self.group_prims(&input(0, cache), &node.group) {
                    Ok(skin) => groom::hair_generate(&skin, &input(1, cache), settings),
                    Err(e) => self.report(id, Err(e)),
                },
                NodeKind::HairDeform(settings) => {
                    // The groom and skin at the rest frame, and the skin now.
                    let rest_time = (settings.rest_frame - 1.0) / sims::SIM_FPS;
                    let (hair, skin) = (self.input_of(id, 0), self.input_of(id, 1));
                    let result = if skin.is_none() {
                        Err("connect the animated skin to input 2".to_owned())
                    } else {
                        groom::hair_deform(&self.cook_fresh(hair, rest_time), &self.cook_fresh(skin, rest_time), &input(1, cache))
                    };
                    self.report(id, result)
                }
                NodeKind::Normal(settings) => sops::normal(&input(0, cache), settings),
                NodeKind::GroupCreate(settings) => self.report(id, groups::group_create(&input(0, cache), settings)),
                NodeKind::AttributePaint { attribute, values, settings } => {
                    self.report(id, paint::attribute_paint(&input(0, cache), attribute, values, settings))
                }
                NodeKind::Scatter(settings) => {
                    let result = self.group_prims(&input(0, cache), &node.group).and_then(|surface| sops::scatter(&surface, settings));
                    self.report(id, result)
                }
                NodeKind::Boolean(settings) => {
                    let result = if self.input_of(id, 1).is_none() {
                        Err("connect the cutting geometry (B) to the right input".to_owned())
                    } else {
                        boolean::boolean(&input(0, cache), &input(1, cache), settings)
                    };
                    self.report(id, result)
                }
                NodeKind::MaterialFracture(settings) => self.report(id, fracture::fracture(&input(0, cache), settings)),
                NodeKind::CrowdSolver(settings) => {
                    let (start, ground, obstacles) = (self.input_of(id, 0), self.input_of(id, 1), self.input_of(id, 2));
                    if start.is_none() {
                        self.report(id, Err("connect start points to input 1 (for example Scatter on a grid)".to_owned()))
                    } else {
                        crowd::crowd(
                            self.sim_key(id),
                            settings,
                            time,
                            || self.cook_fresh(start, 0.0),
                            || self.cook_fresh(ground, 0.0),
                            || self.cook_fresh(obstacles, 0.0),
                        )
                    }
                }
                NodeKind::Muscle(settings) => self.report(id, muscle::muscle(&input(0, cache), settings)),
                NodeKind::MuscleSolver(settings) => {
                    let source = self.input_of(id, 0);
                    muscle::jiggle(self.sim_key(id), settings, time, |t| self.cook_fresh(source, t))
                }
                NodeKind::MuscleSkin(settings) => {
                    let muscles = self.input_of(id, 1);
                    let rest = self.cook_fresh(muscles, 0.0);
                    self.report(id, muscle::skin(&input(0, cache), &rest, &input(1, cache), settings))
                }
                NodeKind::Terrain(settings) => terrain::terrain(settings),
                NodeKind::TerrainNoise(settings) => self.report(id, terrain::terrain_noise(&input(0, cache), settings)),
                NodeKind::TerrainErode(settings) => self.report(id, terrain::terrain_erode(&input(0, cache), settings)),
                NodeKind::TerrainTerrace(settings) => self.report(id, terrain::terrain_terrace(&input(0, cache), settings)),
                NodeKind::TerrainMask(settings) => self.report(id, terrain::terrain_mask(&input(0, cache), settings)),
                NodeKind::TerrainColorize(settings) => self.report(id, terrain::terrain_colorize(&input(0, cache), settings)),
                NodeKind::UvTexture(settings) => sops::uv_texture(&input(0, cache), settings),
                NodeKind::UvTransform(settings) => self.report(id, sops::uv_transform(&input(0, cache), settings)),
                NodeKind::UvQuickShade { file } => self.report(id, sops::uv_quickshade(&input(0, cache), file)),
                NodeKind::VellumSolver(settings) => {
                    let (geometry, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    let animated = geometry.is_some_and(|g| self.time_dependent(g));
                    // A setup that can't move is the usual "Vellum does nothing": say why.
                    let start = self.cook_fresh(geometry, 0.0);
                    let lowest = start.vertices.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
                    let note = if geometry.is_none() {
                        Some("connect the cloth, hair or softbody geometry to input 1".to_owned())
                    } else if settings.ground && lowest <= settings.thickness + 1e-4 {
                        Some(format!(
                            "the geometry starts on the ground plane (lowest point y = {lowest:.3}), so it can't fall. \
                             Move it above y = 0, or turn off Ground plane to use only the collider."
                        ))
                    } else {
                        None
                    };
                    node_notes(|notes| match note {
                        Some(n) => {
                            notes.insert(id, n);
                        }
                        None => {
                            notes.remove(&id);
                        }
                    });
                    // The node's group is pinned (on top of any f@pin).
                    let pin_group = node.group.clone();
                    let pinned = |mut mesh: Mesh| -> Mesh {
                        if let Ok(Some(mask)) = groups::point_mask(&mesh, &pin_group) {
                            let pin = mesh.point_attributes.entry("pin".into()).or_insert_with(|| vec![0.0; mask.len()]);
                            pin.resize(mask.len(), 0.0);
                            for (p, m) in pin.iter_mut().zip(mask) {
                                if m {
                                    *p = 1.0;
                                }
                            }
                        }
                        mesh
                    };
                    vellum::vellum(
                        self.sim_key(id),
                        settings,
                        time,
                        animated,
                        |t| pinned(self.cook_fresh(geometry, t)),
                        || self.cook_fresh(collider, 0.0),
                    )
                }
                NodeKind::RbdSolver(settings) => {
                    let (pieces, collider) = (self.input_of(id, 0), self.input_of(id, 1));
                    sims::rbd(self.sim_key(id), settings, time, || self.cook_fresh(pieces, 0.0), || self.cook_fresh(collider, 0.0))
                }
                NodeKind::SopSolver { substeps } => {
                    let source = self.input_of(id, 0);
                    sims::sop_solver(
                        self.sim_key(id),
                        time,
                        *substeps,
                        || self.cook_fresh(source, 0.0),
                        |prev, step_time| self.sop_solver_step(id, prev, step_time, 1.0 / (sims::SIM_FPS * (*substeps).clamp(1, 16) as f32)),
                    )
                }
                NodeKind::SolverInput { index } => {
                    let solver = node.parent;
                    let prev = solver.and_then(|s| SOLVER_PREV_FRAME.with(|frames| frames.borrow().get(&s).cloned()));
                    match (index, prev) {
                        (0, Some(prev)) => prev,
                        // Input_1, or Prev_Frame outside a solver step (viewing the network): the solver's input.
                        _ => solver.and_then(|s| self.input_of(s, 0)).map(|src| self.cook(src, time, cache)).unwrap_or_default(),
                    }
                }
                NodeKind::Switch { index } => input((*index).min(1), cache),
            }
        };
        let mut out = out;
        // Houdini-style Group: only the group's points/primitives keep this node's changes.
        if !node.bypass && !node.group.is_empty() && node.kind.group_support() == GroupSupport::Restrict {
            let source = input(0, cache);
            match (groups::point_mask(&source, &node.group), groups::prim_mask(&source, &node.group)) {
                (Ok(Some(points)), Ok(Some(prims))) => {
                    groups::restrict_to_group(&source, &mut out, &points, &prims);
                }
                (Err(e), _) | (_, Err(e)) => out = self.report(id, Err(e)),
                _ => {}
            }
        }
        out.ensure_builtin_ids();
        cache.insert(id, out.clone());
        out
    }

    // ---- scene file / undo snapshot format ----
    fn serialize(&self) -> String {
        let mut s = String::from("pfs 2\n");
        let mut ids: Vec<_> = self.nodes.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let n = &self.nodes[&id];
            // Animated nodes are written with their time-0 values; channels follow below.
            let kind = self.kind_at(id, 0.0);
            let params: Vec<String> = kind.params().iter().map(|p| p.to_string()).collect();
            let serialized_params = match kind.as_ref() {
                NodeKind::Wrangle { code, domain } => format!("{}:{}", domain, encode_text(code)),
                NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts } => {
                    format!(
                        "mr:{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
                        encode_text(executable),
                        encode_text(arguments),
                        encode_text(output),
                        width,
                        height,
                        samples,
                        match opts.camera {
                            RenderCamera::Auto => "auto".to_string(),
                            RenderCamera::Viewport => "viewport".to_string(),
                            RenderCamera::Node(id) => format!("n{id}"),
                        },
                        opts.max_depth,
                        opts.light_samples,
                        opts.bsdf_samples,
                        opts.background[0],
                        opts.background[1],
                        opts.background[2],
                        opts.sequence as u8,
                        opts.range_start,
                        opts.range_end,
                        opts.range_step,
                        opts.stage as u8,
                        opts.volume_samples,
                    )
                }
                kind => match kind.texts() {
                    texts if texts.is_empty() => params.join(" "),
                    texts => format!(
                        "t:{} {}",
                        texts.iter().map(|t| encode_text(t)).collect::<Vec<_>>().join("|"),
                        params.join(" ")
                    ),
                },
            };
            s += &format!(
                "node {} {} {} {} {} {} {}\n",
                id,
                n.pos.x,
                n.pos.y,
                n.bypass as u8,
                match n.context {
                    AppContext::Obj => "obj",
                    AppContext::Mat => "mat",
                    AppContext::Out => "out",
                    AppContext::Stage => "stage",
                },
                n.kind.name(),
                serialized_params
            );
        }
        for l in &self.links {
            s += &format!("link {} {} {}\n", l.from, l.to, l.slot);
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            if let Some(parent) = self.nodes[&id].parent {
                s += &format!("parent {id} {parent}\n");
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            let x = self.xform_at(id, 0.0);
            if !x.is_identity() {
                let values: Vec<String> = [x.t, x.r, x.s].concat().iter().map(|v| v.to_string()).collect();
                s += &format!("xform {id} {}\n", values.join(" "));
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            for (name, v) in &self.nodes[&id].channels {
                s += &format!("chan {id} {} {} {} {}\n", encode_text(name), v[0], v[1], v[2]);
            }
        }
        for id in self.nodes.keys().copied().collect::<std::collections::BTreeSet<_>>() {
            let group = &self.nodes[&id].group;
            if !group.is_empty() {
                s += &format!("group {id} {} {}\n", group.kind, encode_text(&group.pattern));
            }
        }
        let mut displays: Vec<_> = self.displays.iter().collect();
        displays.sort_unstable();
        for (container, display) in displays {
            s += &format!("display {container} {display}\n");
        }
        for (channel_id, channel) in &self.channels {
            if channel.keys.is_empty() || !self.nodes.contains_key(&channel_id.node) {
                continue;
            }
            let (kind, index) = match channel_id.target {
                ChannelTarget::Param(i) => ("p", i),
                ChannelTarget::Xform(i) => ("x", i),
            };
            s += &format!("channel {} {kind} {index} {}\n", channel_id.node, channel.to_text());
        }
        s += &format!("next {}\n", self.next_id);
        s
    }

    fn deserialize(text: &str) -> Graph {
        let mut g = Graph::default();
        let mut legacy_display: Option<NodeId> = None;
        let mut parents: Vec<(NodeId, NodeId)> = Vec::new();
        let mut xforms: Vec<(NodeId, ObjectXform)> = Vec::new();
        // "pfs 2"; older files say "mhip 2" (same format) or "mhip 1".
        let is_v2 = text.lines().next().is_some_and(|l| matches!(l.trim(), "pfs 2" | "mhip 2"));
        for line in text.lines() {
            let t: Vec<&str> = line.split_whitespace().collect();
            match t.as_slice() {
                ["node", id, x, y, bp, ..] => {
                    // Determine optional context token; support both old and new formats
                    let mut ctx = AppContext::Obj;
                    let mut cursor = 5usize; // index of token after bp
                    if t.len() > cursor {
                        let tok = *t.get(cursor).unwrap_or(&"");
                        match tok {
                            "obj" => { ctx = AppContext::Obj; cursor += 1; }
                            "mat" => { ctx = AppContext::Mat; cursor += 1; }
                            "out" => { ctx = AppContext::Out; cursor += 1; }
                            "stage" => { ctx = AppContext::Stage; cursor += 1; }
                            _ => {}
                        }
                    }
                    let name = *t.get(cursor).unwrap_or(&"");
                    let rest = if t.len() > cursor + 1 { &t[cursor + 1..] } else { &[] as &[&str] };
                    if name == "wrangle" {
                        let (domain, encoded) = rest.first()
                            .and_then(|value| value.split_once(':'))
                            .map_or((0, rest.first().copied().unwrap_or("")), |(domain, encoded)| {
                                (domain.parse::<usize>().unwrap_or(0).min(3), encoded)
                            });
                        let code = decode_text(encoded).unwrap_or_default();
                        if let (Ok(id), Ok(x), Ok(y)) = (id.parse::<usize>(), x.parse::<f32>(), y.parse::<f32>()) {
                            g.nodes.insert(
                                id,
                                Node {
                                    kind: NodeKind::Wrangle { code, domain },
                                    pos: egui::pos2(x, y),
                                    bypass: *bp == "1",
                                    context: ctx,
                                    parent: None,
                                    xform: ObjectXform::default(),
                                    group: groups::NodeGroup::default(),
                                    channels: vex::Channels::new(),
                                },
                            );
                            g.next_id = g.next_id.max(id + 1);
                        }
                        continue;
                    }
                    if name == "moonray_render" {
                        let fields = rest.first().and_then(|value| value.strip_prefix("mr:"))
                            .map(|value| value.split('|').collect::<Vec<_>>());
                        let decode = |index: usize| fields.as_ref()
                            .and_then(|values| values.get(index))
                            .and_then(|value| decode_text(value))
                            .unwrap_or_default();
                        let parse = |index: usize, default: usize| fields.as_ref()
                            .and_then(|values| values.get(index))
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(default);
                        let parse_f = |index: usize| fields.as_ref()
                            .and_then(|values| values.get(index))
                            .and_then(|value| value.parse::<f32>().ok())
                            .unwrap_or(0.0)
                            .max(0.0);
                        let camera = fields.as_ref()
                            .and_then(|values| values.get(6))
                            .map_or(RenderCamera::Auto, |value| match *value {
                                "viewport" => RenderCamera::Viewport,
                                other => other.strip_prefix('n')
                                    .and_then(|id| id.parse().ok())
                                    .map_or(RenderCamera::Auto, RenderCamera::Node),
                            });
                        let defaults = RenderOpts::default();
                        let opts = RenderOpts {
                            camera,
                            max_depth: parse(7, defaults.max_depth).clamp(1, 32),
                            light_samples: parse(8, defaults.light_samples).clamp(1, 16),
                            bsdf_samples: parse(9, defaults.bsdf_samples).clamp(1, 16),
                            background: [parse_f(10), parse_f(11), parse_f(12)],
                            sequence: parse(13, 0) != 0,
                            range_start: parse_f(14),
                            range_end: fields.as_ref().and_then(|v| v.get(15)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(defaults.range_end).max(0.0),
                            range_step: fields.as_ref().and_then(|v| v.get(16)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(defaults.range_step).max(1e-4),
                            stage: parse(17, 0) != 0,
                            volume_samples: parse(18, defaults.volume_samples).clamp(1, 16),
                        };
                        if let (Ok(id), Ok(x), Ok(y)) = (id.parse::<usize>(), x.parse::<f32>(), y.parse::<f32>()) {
                            g.nodes.insert(
                                id,
                                Node {
                                    kind: NodeKind::MoonRayRender {
                                        executable: decode(0),
                                        arguments: decode(1),
                                        output: decode(2),
                                        width: parse(3, 1280).max(1),
                                        height: parse(4, 720).max(1),
                                        samples: parse(5, 64).max(1),
                                        opts,
                                    },
                                    pos: egui::pos2(x, y),
                                    bypass: *bp == "1",
                                    context: ctx,
                                    parent: None,
                                    xform: ObjectXform::default(),
                                    group: groups::NodeGroup::default(),
                                    channels: vex::Channels::new(),
                                },
                            );
                            g.next_id = g.next_id.max(id + 1);
                        }
                        continue;
                    }
                    // Text parameters come first as t:<hex>|<hex>.
                    let (texts, rest) = match rest.split_first() {
                        Some((first, numbers)) if first.starts_with("t:") => (
                            first[2..].split('|').map(|t| decode_text(t).unwrap_or_default()).collect::<Vec<_>>(),
                            numbers,
                        ),
                        _ => (Vec::new(), rest),
                    };
                    let p: Vec<f32> = rest.iter().filter_map(|v| v.parse().ok()).collect();
                    let kind = NodeKind::from_params(name, &p).map(|mut kind| {
                        kind.set_texts(&texts);
                        kind
                    });
                    if let (Ok(id), Ok(x), Ok(y), Some(kind)) = (
                        id.parse::<usize>(),
                        x.parse::<f32>(),
                        y.parse::<f32>(),
                        kind,
                    ) {
                        g.nodes.insert(
                            id,
                            Node { kind, pos: egui::pos2(x, y), bypass: *bp == "1", context: ctx, parent: None, xform: ObjectXform::default(), group: groups::NodeGroup::default(), channels: vex::Channels::new() },
                        );
                        g.next_id = g.next_id.max(id + 1);
                    }
                }
                ["link", a, b, s] => {
                    if let (Ok(from), Ok(to), Ok(slot)) = (a.parse(), b.parse(), s.parse()) {
                        g.links.push(Link { from, to, slot });
                    }
                }
                ["chan", id, name, x, y, z] => {
                    if let (Ok(id), Some(name)) = (id.parse::<usize>(), decode_text(name)) {
                        if let Some(node) = g.nodes.get_mut(&id) {
                            let v = [x, y, z].map(|c| c.parse::<f32>().unwrap_or(0.0));
                            node.channels.insert(name, v);
                        }
                    }
                }
                ["group", id, kind, pattern] => {
                    if let (Ok(id), Some(pattern)) = (id.parse::<usize>(), decode_text(pattern)) {
                        if let Some(node) = g.nodes.get_mut(&id) {
                            node.group = groups::NodeGroup { pattern, kind: kind.parse::<u8>().unwrap_or(0).min(2) };
                        }
                    }
                }
                ["parent", id, parent] => {
                    if let (Ok(id), Ok(parent)) = (id.parse(), parent.parse()) {
                        parents.push((id, parent));
                    }
                }
                ["display", container, node] => {
                    if let (Ok(container), Ok(node)) = (container.parse(), node.parse()) {
                        g.displays.insert(container, node);
                    }
                }
                ["display", d] => legacy_display = d.parse().ok(),
                ["channel", node, kind, index, keys] => {
                    if let (Ok(node), Ok(index)) = (node.parse::<usize>(), index.parse::<usize>()) {
                        let target = if *kind == "x" { ChannelTarget::Xform(index) } else { ChannelTarget::Param(index) };
                        let channel = anim::Channel::from_text(keys);
                        if !channel.keys.is_empty() {
                            g.channels.insert(ChannelId { node, target }, channel);
                        }
                    }
                }
                ["xform", id, rest @ ..] if rest.len() == 9 => {
                    let v: Vec<f32> = rest.iter().filter_map(|t| t.parse().ok()).collect();
                    if let (Ok(id), true) = (id.parse::<usize>(), v.len() == 9) {
                        xforms.push((id, ObjectXform { t: [v[0], v[1], v[2]], r: [v[3], v[4], v[5]], s: [v[6], v[7], v[8]] }));
                    }
                }
                ["next", n] => {
                    if let Ok(n) = n.parse::<usize>() {
                        g.next_id = g.next_id.max(n);
                    }
                }
                _ => {}
            }
        }
        g.links
            .retain(|l| g.nodes.contains_key(&l.from) && g.nodes.contains_key(&l.to));
        // Render nodes live in OUT and take no inputs.
        let render_ids: Vec<NodeId> = g
            .nodes
            .iter()
            .filter(|(_, n)| n.kind.is_render())
            .map(|(id, _)| *id)
            .collect();
        for id in &render_ids {
            if let Some(node) = g.nodes.get_mut(id) {
                node.context = AppContext::Out;
            }
        }
        g.links.retain(|l| !render_ids.contains(&l.to));
        for (id, parent) in parents {
            if g.nodes.contains_key(&parent) {
                if let Some(node) = g.nodes.get_mut(&id) {
                    node.parent = Some(parent);
                }
            }
        }
        if !is_v2 {
            // Flat v1 scene: wrap its OBJ-context SOP nodes in a single Geometry container.
            let sop_ids: Vec<NodeId> = g
                .nodes
                .iter()
                .filter(|(_, n)| {
                    n.context == AppContext::Obj
                        && !n.kind.is_object()
                        && !matches!(n.kind, NodeKind::MoonRayRender { .. })
                })
                .map(|(id, _)| *id)
                .collect();
            if !sop_ids.is_empty() {
                let top_left = sop_ids
                    .iter()
                    .filter_map(|id| g.nodes.get(id).map(|n| n.pos))
                    .reduce(|a, b| a.min(b))
                    .unwrap_or(egui::pos2(40.0, 40.0));
                let geo = g.add(NodeKind::Geometry { material: None }, top_left);
                for id in &sop_ids {
                    if let Some(node) = g.nodes.get_mut(id) {
                        node.parent = Some(geo);
                    }
                }
                if let Some(display) = legacy_display.filter(|d| sop_ids.contains(d)) {
                    g.displays.insert(geo, display);
                }
            }
        }
        g.displays.retain(|c, d| (*c == STAGE_NETWORK || g.nodes.contains_key(c)) && g.nodes.contains_key(d));
        for (id, xform) in xforms {
            if let Some(node) = g.nodes.get_mut(&id) {
                node.xform = xform;
            }
        }
        g.channels.retain(|c, _| g.nodes.contains_key(&c.node));
        g
    }
}

// ============================================================================
// 5. NETWORK EDITOR HELPERS
// ============================================================================

const NODE_W: f32 = 140.0;
const NODE_H: f32 = 40.0;
// Scene files are .pfs (ProceduralFX Studio); .mhip is the old name, still opened.
const SCENE_EXTENSION: &str = "pfs";
const SCENE_FILE: &str = "scene.pfs";
const LEGACY_SCENE_FILE: &str = "scene.mhip";

// Returns the MoonRay class name and its RDLA definition for a MAT node's settings.
// One parameter of a shader table: a slider, colour swatch or checkbox.
fn shader_param_ui(ui: &mut egui::Ui, param: &shading::Param, slot: &mut [f32]) -> bool {
    match param.kind {
        shading::Kind::Float(min, max) => {
            let log = min >= 0.0 && max / min.max(0.01) >= 1000.0;
            ui.add(egui::Slider::new(&mut slot[0], min..=max).logarithmic(log).text(param.label)).changed()
        }
        shading::Kind::Color => {
            ui.horizontal(|ui| {
                let mut rgb = [slot[0], slot[1], slot[2]];
                let changed = ui.color_edit_button_rgb(&mut rgb).changed();
                slot.copy_from_slice(&rgb);
                ui.label(param.label);
                changed
            })
            .inner
        }
        shading::Kind::Toggle => {
            let mut on = slot[0] > 0.5;
            let changed = ui.checkbox(&mut on, param.label).changed();
            slot[0] = on as u8 as f32;
            changed
        }
    }
}

// Preset menu, textures and the grouped parameters of a shader.
fn shader_values_ui(ui: &mut egui::Ui, values: &mut shading::Values) -> bool {
    let mut c = false;
    let presets = values.table.presets();
    let mut preset = values.preset;
    egui::ComboBox::from_label("Preset").selected_text(presets[preset.min(presets.len() - 1)].0).height(400.0).show_ui(ui, |ui| {
        for (i, (name, _)) in presets.iter().enumerate() {
            ui.selectable_value(&mut preset, i, *name);
        }
    });
    if preset != values.preset {
        values.apply_preset(preset);
        c = true;
    }
    if values.table == shading::Table::Principled {
        for (label, text) in shading::PRINCIPLED_TEXTURES.iter().zip(values.textures.iter_mut()) {
            ui.horizontal(|ui| {
                ui.label(*label);
                c |= ui.add(egui::TextEdit::singleline(text).desired_width(160.0)).changed();
                if ui.button("…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "exr", "tif", "tiff", "hdr", "tx"]).pick_file() {
                        *text = path.to_string_lossy().into_owned();
                        c = true;
                    }
                }
            });
        }
    }
    let mut at = 0;
    let mut group = "";
    for param in values.table.params() {
        if param.group != group {
            group = param.group;
            ui.separator();
            ui.label(egui::RichText::new(group).strong());
        }
        let width = param.width();
        c |= shader_param_ui(ui, param, &mut values.values[at..at + width]);
        at += width;
    }
    c
}

// Points the renderer draws as spheres (Whitewater output), not left out like other point clouds.
fn renders_points(mesh: &Mesh) -> bool {
    mesh.indices.is_empty() && !mesh.vertices.is_empty() && mesh.point_attributes.get(whitewater::RENDER_POINTS).is_some_and(|v| v.first() == Some(&1.0))
}

fn rdla_material(name: &str, kind: usize, color: [f32; 3], roughness: f32, value: f32) -> (&'static str, String) {
    let rgb = |c: [f32; 3]| format!("Rgb({:.4}, {:.4}, {:.4})", c[0], c[1], c[2]);
    let (class, attrs) = match kind {
        1 => (
            "DwaMetalMaterial",
            format!("    [\"metallic_color\"] = {},\n    [\"roughness\"] = {:.4},\n", rgb(color), roughness),
        ),
        2 => (
            "DwaSolidDielectricMaterial",
            format!(
                "    [\"albedo\"] = {},\n    [\"roughness\"] = {:.4},\n    [\"refractive_index\"] = {:.4},\n",
                rgb(color),
                roughness,
                value.max(1.0),
            ),
        ),
        3 => (
            "DwaEmissiveMaterial",
            format!("    [\"emission\"] = {},\n", rgb([color[0] * value, color[1] * value, color[2] * value])),
        ),
        _ => (
            "DwaBaseMaterial",
            format!(
                "    [\"albedo\"] = {},\n    [\"roughness\"] = {:.4},\n    [\"specular\"] = {:.4},\n",
                rgb(color),
                roughness,
                value.clamp(0.0, 1.0),
            ),
        ),
    };
    (class, format!("{class}(\"{name}\") {{\n{attrs}}}\n"))
}
// Viewport light intensity -> MoonRay intensity, measured against MoonRay so a render
// shown through the sRGB display transform looks close to the viewport:
// a distant light of intensity 1 head-on renders at the albedo, and a sphere light of
// intensity 1 gives albedo · 251 / distance², which the viewport's falloff matches below.
const DISTANT_LIGHT_SCALE: f32 = 1.0;
const POINT_LIGHT_FALLOFF: f32 = 18.0;
const POINT_LIGHT_SCALE: f32 = POINT_LIGHT_FALLOFF / 251.0;

fn node_rect(pos: egui::Pos2, origin: egui::Vec2, zoom: f32) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(origin.x + pos.x * zoom, origin.y + pos.y * zoom),
        egui::vec2(NODE_W, NODE_H) * zoom,
    )
}
fn in_port(rect: egui::Rect, slot: usize, n: usize) -> egui::Pos2 {
    let x = rect.left() + rect.width() * (slot as f32 + 1.0) / (n as f32 + 1.0);
    egui::pos2(x, rect.top())
}
fn out_port(rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(rect.center().x, rect.bottom())
}
fn draw_wire(p: &egui::Painter, a: egui::Pos2, b: egui::Pos2, color: egui::Color32) {
    let d = ((b.y - a.y).abs() * 0.5).max(30.0);
    p.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [a, a + egui::vec2(0.0, d), b - egui::vec2(0.0, d), b],
        false,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(2.0, color),
    ));
}
fn vec3_ui(ui: &mut egui::Ui, label: &str, v: &mut [f32; 3], speed: f64) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for x in v.iter_mut() {
            changed |= ui.add(egui::DragValue::new(x).speed(speed)).changed();
        }
    });
    changed
}

enum Action {
    Add(NodeKind, egui::Pos2),
    AddBelow(NodeKind, NodeId),
    Display(NodeId),
    Enter(NodeId),
    Bypass(NodeId),
    Duplicate(NodeId),
    Delete(NodeId),
}

// ============================================================================
// 6. APP
// ============================================================================

#[derive(PartialEq, Clone, Copy)]
pub enum RenderMode {
    Wireframe,
    FlatShaded,
    ShadedWireframe,
    Normals,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum AppContext {
    Obj,
    Mat,
    Out,
    // USD stage (LOP) network.
    Stage,
}

#[derive(PartialEq, Clone, Copy)]
enum SelectionMode {
    Vertex,
    Triangle,
    Edge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GizmoHandle {
    Axis(usize),
    Ring(usize),
    // Screen-plane move, or uniform scale.
    Center,
}

// One handle as drawn last frame, for hit testing and for turning mouse motion into edits.
#[derive(Clone, Debug)]
struct GizmoPart {
    handle: GizmoHandle,
    // Screen polyline (a segment for axes, a loop for rings, one point for the centre).
    points: Vec<egui::Pos2>,
    // Axis handles: screen pixels per world unit along the axis.
    axis_screen: egui::Vec2,
    // Ring handles: whether screen angle increases with the rotation.
    ring_sign: f32,
}

#[derive(Clone, Debug)]
struct GizmoFrame {
    node: NodeId,
    pivot: egui::Pos2,
    pixels_per_unit: f32,
    cam_right: Vec3,
    cam_up: Vec3,
    handle_px: f32,
    parts: Vec<GizmoPart>,
}

impl GizmoFrame {
    fn hit(&self, p: egui::Pos2) -> Option<&GizmoPart> {
        let distance = |part: &GizmoPart| match part.points.as_slice() {
            [c] => c.distance(p),
            points => points
                .windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1]);
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
                    (a + ab * t).distance(p)
                })
                .fold(f32::INFINITY, f32::min),
        };
        self.parts
            .iter()
            .map(|part| (part, distance(part) - if part.handle == GizmoHandle::Center { 4.0 } else { 0.0 }))
            .filter(|(_, d)| *d < 8.0)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(part, _)| part)
    }
}

#[derive(Clone, Debug)]
struct GizmoDrag {
    part: GizmoPart,
    frame: GizmoFrame,
    start_pointer: egui::Pos2,
    start: ObjectXform,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SheetDomain {
    Points,
    Vertices,
    Primitives,
    Detail,
}

impl SheetDomain {
    fn prefix(self) -> &'static str {
        match self {
            SheetDomain::Points => "",
            SheetDomain::Vertices => "vertex:",
            SheetDomain::Primitives => "prim:",
            SheetDomain::Detail => "detail:",
        }
    }

    fn count(self, mesh: &Mesh) -> usize {
        match self {
            SheetDomain::Points => mesh.vertices.len(),
            SheetDomain::Vertices => mesh.indices.len() * 3,
            SheetDomain::Primitives => mesh.indices.len(),
            SheetDomain::Detail => 1,
        }
    }

    // Whether a stored attribute key belongs to this domain.
    fn owns(self, key: &str) -> bool {
        match self {
            SheetDomain::Points => !["prim:", "vertex:", "detail:"].iter().any(|p| key.starts_with(p)),
            _ => key.starts_with(self.prefix()),
        }
    }
}

// An attribute of one domain as the spreadsheet shows it.
struct SheetAttribute {
    name: String,
    key: String,
    components: usize,
    // Values whose length does not match the element count are still shown, but flagged.
    size_ok: bool,
    integral: bool,
}

fn sheet_attributes(mesh: &Mesh, domain: SheetDomain) -> Vec<SheetAttribute> {
    let count = domain.count(mesh);
    let mut attributes: Vec<SheetAttribute> = mesh
        .point_attributes
        .iter()
        .filter(|(key, _)| domain.owns(key))
        .map(|(key, values)| {
            let components = if count > 0 { (values.len() / count).max(1) } else { 1 };
            SheetAttribute {
                name: key[domain.prefix().len()..].to_string(),
                key: key.clone(),
                components,
                size_ok: count > 0 && values.len() == count * components,
                integral: !values.is_empty() && values.iter().take(1024).all(|v| v.fract() == 0.0),
            }
        })
        .collect();
    attributes.sort_by_key(|a| (a.name != "Cd", a.name.to_lowercase()));
    attributes
}

fn component_labels(name: &str, components: usize) -> Vec<String> {
    let suffixes: Vec<String> = match components {
        1 => return vec![name.to_string()],
        3 if matches!(name, "Cd" | "Cs" | "Ce" | "color") => ["r", "g", "b"].map(String::from).to_vec(),
        3 => ["x", "y", "z"].map(String::from).to_vec(),
        4 if name == "orient" => ["x", "y", "z", "w"].map(String::from).to_vec(),
        n => (0..n).map(|c| format!("[{c}]")).collect(),
    };
    suffixes.into_iter().map(|s| if s.starts_with('[') { format!("{name}{s}") } else { format!("{name}.{s}") }).collect()
}

enum SheetSource {
    Index,
    Position(usize),
    ComputedNormal(usize),
    PrimPoints,
    VertexPrim,
    VertexPoint,
    Attribute { key: String, components: usize, component: usize, integral: bool },
}

struct SheetColumn {
    header: String,
    tooltip: String,
    source: SheetSource,
}

fn sheet_columns(mesh: &Mesh, domain: SheetDomain) -> Vec<SheetColumn> {
    let column = |header: String, tooltip: &str, source| SheetColumn { header, tooltip: tooltip.to_string(), source };
    let mut columns = Vec::new();
    if domain != SheetDomain::Detail {
        columns.push(column("#".into(), "Element number", SheetSource::Index));
    }
    let attributes = sheet_attributes(mesh, domain);
    match domain {
        SheetDomain::Points => {
            for (c, label) in component_labels("P", 3).into_iter().enumerate() {
                columns.push(column(label, "P: point position (3 floats)", SheetSource::Position(c)));
            }
            if !attributes.iter().any(|a| a.name == "N") {
                for (c, label) in component_labels("N", 3).into_iter().enumerate() {
                    columns.push(column(label, "N: computed point normal (no N attribute is stored)", SheetSource::ComputedNormal(c)));
                }
            }
        }
        SheetDomain::Vertices => {
            columns.push(column("Prim:Vtx".into(), "Primitive number and the vertex's position in it", SheetSource::VertexPrim));
            columns.push(column("Point".into(), "Point the vertex refers to", SheetSource::VertexPoint));
        }
        SheetDomain::Primitives => {
            columns.push(column("Points".into(), "Point numbers of the triangle", SheetSource::PrimPoints));
        }
        SheetDomain::Detail => {}
    }
    for attribute in attributes {
        let kind = if attribute.integral { "ints" } else { "floats" };
        let mut tooltip = format!("{} ({} {kind})", attribute.name, attribute.components);
        if !attribute.size_ok {
            tooltip.push_str(" · warning: the value count does not match the element count");
        }
        for (component, label) in component_labels(&attribute.name, attribute.components).into_iter().enumerate() {
            columns.push(column(
                label,
                &tooltip,
                SheetSource::Attribute {
                    key: attribute.key.clone(),
                    components: attribute.components,
                    component,
                    integral: attribute.integral,
                },
            ));
        }
    }
    columns
}

fn sheet_cell(mesh: &Mesh, normals: &[Vec3], source: &SheetSource, row: usize) -> String {
    let float = |v: f32| format!("{v:.3}");
    match source {
        SheetSource::Index => row.to_string(),
        SheetSource::Position(c) => mesh.vertices.get(row).map_or_else(String::new, |p| float([p.x, p.y, p.z][*c])),
        SheetSource::ComputedNormal(c) => normals.get(row).map_or_else(String::new, |n| float([n.x, n.y, n.z][*c])),
        SheetSource::PrimPoints => mesh.indices.get(row).map_or_else(String::new, |t| format!("{} {} {}", t[0], t[1], t[2])),
        SheetSource::VertexPrim => format!("{}:{}", row / 3, row % 3),
        SheetSource::VertexPoint => mesh.indices.get(row / 3).map_or_else(String::new, |t| t[row % 3].to_string()),
        SheetSource::Attribute { key, components, component, integral } => {
            match mesh.point_attributes.get(key).and_then(|values| values.get(row * components + component)) {
                Some(v) if *integral => format!("{}", *v as i64),
                Some(v) => float(*v),
                None => String::new(),
            }
        }
    }
}

// The app's dockable panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
enum Pane {
    Viewport,
    Network,
    Parameters,
    Timeline,
    Spreadsheet,
    GraphEditor,
}

impl Pane {
    const ALL: [Pane; 6] = [Pane::Viewport, Pane::Network, Pane::Parameters, Pane::Timeline, Pane::Spreadsheet, Pane::GraphEditor];

    fn title(self) -> &'static str {
        match self {
            Pane::Viewport => "Viewport",
            Pane::Network => "Network",
            Pane::Parameters => "Parameters",
            Pane::Timeline => "Timeline",
            Pane::Spreadsheet => "Geometry Spreadsheet",
            Pane::GraphEditor => "Graph Editor",
        }
    }
}

enum PaneRequest {
    Show(Pane, bool),
    PopOut(Pane),
    Reset,
    SaveAs(String),
    Load(String),
    Delete(String),
}

// Network on the left, parameters on the right, timeline under the viewport.
fn default_layout() -> egui_dock::DockState<Pane> {
    use egui_dock::NodeIndex;
    let mut dock = egui_dock::DockState::new(vec![Pane::Viewport]);
    let tree = dock.main_surface_mut();
    let [centre, _network] = tree.split_left(NodeIndex::root(), 0.23, vec![Pane::Network]);
    let [centre, _parameters] = tree.split_right(centre, 0.74, vec![Pane::Parameters]);
    tree.split_below(centre, 0.82, vec![Pane::Timeline]);
    dock
}

// Layouts are RON (it keeps the dock's infinite "not laid out yet" sizes, which JSON can't).
fn layout_file() -> Option<PathBuf> {
    Some(app_config_dir()?.join("layout.ron"))
}

fn layouts_dir() -> Option<PathBuf> {
    Some(app_config_dir()?.join("layouts"))
}

fn layout_to_text(dock: &egui_dock::DockState<Pane>) -> Option<String> {
    ron::to_string(dock).ok()
}

fn layout_from_text(text: &str) -> Option<egui_dock::DockState<Pane>> {
    let dock: egui_dock::DockState<Pane> = ron::from_str(text).ok()?;
    // A layout that lost the viewport isn't worth restoring.
    dock.find_tab(&Pane::Viewport).map(|_| dock)
}

fn load_layout() -> Option<egui_dock::DockState<Pane>> {
    layout_from_text(&std::fs::read_to_string(layout_file()?).ok()?)
}

// Names of the saved layouts, alphabetically.
fn saved_layouts() -> Vec<String> {
    let mut names: Vec<String> = layouts_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| e.path().file_stem().filter(|_| e.path().extension().is_some_and(|x| x == "ron")).map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

fn safe_layout_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| c.is_alphanumeric() || " -_".contains(*c)).collect();
    (!name.trim().is_empty()).then(|| name.trim().to_owned())
}

// Draws each tab with the app's own pane functions.
struct PaneViewer<'a> {
    app: &'a mut ProceduralFxStudioApp,
    ctx: egui::Context,
    pop_out: Option<Pane>,
}

impl egui_dock::TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
        self.app.pane_ui(ui, &self.ctx, *tab);
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Pane, _surface: egui_dock::SurfaceIndex, _node: egui_dock::NodeIndex) {
        if ui.button("Open in New Window").clicked() {
            self.pop_out = Some(*tab);
            ui.close();
        }
    }

    // The viewport stays; everything else can be closed and reopened from Windows.
    fn is_closeable(&self, tab: &Pane) -> bool {
        *tab != Pane::Viewport
    }

    fn scroll_bars(&self, tab: &Pane) -> [bool; 2] {
        match tab {
            Pane::Parameters => [false, true],
            _ => [false, false],
        }
    }
}

struct GroupPick {
    node: NodeId,
    domain: groups::GroupDomain,
    // The node's input, whose element numbers the group refers to.
    input: Mesh,
    previous_mode: SelectionMode,
}

struct ProceduralFxStudioApp {
    graph: Graph,
    selected: Option<NodeId>,
    dragging_link: Option<NodeId>,
    graph_pan: egui::Vec2,
    graph_zoom: f32,
    menu_pos: egui::Pos2,

    render_mode: RenderMode,
    show_points: bool,
    show_floor: bool,
    show_gizmos: bool,
    gizmo_mode: GizmoMode,
    // Handles drawn last frame, and the handle being dragged.
    gizmo_frame: Option<GizmoFrame>,
    gizmo_drag: Option<GizmoDrag>,
    animate: bool,
    anim_speed: f32,
    time: f32,
    playback_start: f32,
    playback_end: f32,
    pause_on_edit: bool,
    graph_editor: anim::GraphEditor,
    // Key every parameter that changes (not only already-animated ones).
    auto_key: bool,
    // Selected node's parameters after this frame's animation was applied, for Auto Key.
    anim_snapshot: Option<(NodeId, Vec<f32>, ObjectXform)>,
    // Scene fingerprint last frame, to pause playback on any edit.
    last_content_signature: Option<u64>,

    cam_rot_x: f32,
    cam_rot_y: f32,
    cam_pan_x: f32,
    cam_pan_y: f32,
    cam_zoom: f32,
    cam_target: Vec3,
    camera_focal: f32,
    camera_near: f32,
    orthographic: bool,
    viewport_bg: u8,
    grid_spacing: f32,
    grid_extent: f32,
    ambient_light: f32,
    key_light_dir: [f32; 3],
    key_light_intensity: f32,
    fill_light_dir: [f32; 3],
    fill_light_intensity: f32,
    active_camera: Option<NodeId>,
    use_camera_requested: bool,
    render_camera: RenderCamera,
    // Options of the render being started; set by start_moonray_render.
    render_opts: RenderOpts,
    // Timeline time used when cooking a sequence frame; None renders the current time.
    render_at: Option<f32>,
    sequence: Option<SequenceJob>,
    viewport_focal_px: f32,
    // Geometry node whose network is being edited; None shows the OBJ-level network.
    current_network: Option<NodeId>,

    current_mesh: Mesh,
    cook_ms: f32,
    dirty: bool,
    status: String,

    undo: Vec<String>,
    redo: Vec<String>,
    last_state: String,

    template: Option<NodeId>,
    template_mesh: Mesh,
    static_cache: HashMap<NodeId, Mesh>,
    cache_key: String,
    frame_cache: HashMap<String, Mesh>,
    frame_cache_order: VecDeque<String>,
    frame_cache_hits: usize,
    frame_cache_misses: usize,
    simulation_rx: Option<Receiver<Mesh>>,
    simulation_key: String,
    cancelled_simulation_key: String,
    simulation_cancel: Option<Arc<AtomicBool>>,
    moonray_rx: Option<Receiver<RenderOutcome>>,
    updater: updater::Updater,
    // The file Save writes to; None until the scene is opened or saved as.
    scene_path: Option<PathBuf>,
    recent_scenes: Vec<PathBuf>,
    window_title: String,
    // Viewport drags paint the selected Attribute Paint node instead of orbiting.
    paint_active: bool,
    paint_fill: Option<NodeId>,
    // Group names on a node's input, for its Group menu: (node, point groups, primitive groups).
    group_choices: Option<(NodeId, Vec<String>, Vec<String>)>,
    group_choices_request: Option<NodeId>,
    // Picking a node's group in the viewport: the viewport shows the node's input meanwhile.
    group_pick: Option<GroupPick>,
    group_pick_request: Option<NodeId>,
    // Some(true) accepts the pick, Some(false) cancels it.
    group_pick_finish: Option<bool>,
    group_pick_domain: Option<groups::GroupDomain>,
    moonray_render_requested: bool,
    // Embedded MoonRay defaults and settings
    moonray_default_executable: String,
    moonray_default_arguments: String,
    moonray_default_output: String,
    moonray_default_width: usize,
    moonray_default_height: usize,
    moonray_default_samples: usize,
    show_moonray_settings: bool,
    // Render Settings dialog: used by toolbar renders; a selected render node overrides them.
    moonray_default_opts: RenderOpts,
    // Auto-named file in the temp folder instead of moonray_default_output.
    render_to_temp: bool,
    render_temp_ext: &'static str,
    // Scales every render's resolution, render nodes included (quick previews).
    resolution_scale: f32,
    open_viewer_after_render: bool,
    // Render in buckets so the Render Viewer fills in progressively.
    bucket_render: bool,
    render_extra_args: String,
    render_started: Option<Instant>,
    render_cancel: Option<Arc<AtomicBool>>,
    render_log_path: Option<PathBuf>,
    last_render_log: String,
    last_render_summary: String,
    viewer: RenderViewer,
    node_stats: HashMap<NodeId, (usize, usize)>,
    // The dockable panes' arrangement (taken out while it's drawn), panes popped out into
    // their own OS windows, which panes are open, and pending open/close/pop-out requests.
    dock: Option<egui_dock::DockState<Pane>>,
    popped: Vec<Pane>,
    open_panes: HashSet<Pane>,
    pane_requests: Vec<PaneRequest>,
    saved_layout: String,
    // Some while the "Save Current Layout" name box is open.
    layout_name_prompt: Option<String>,
    // Pyro node whose domain should be fitted around its source this frame.
    pyro_fit_request: Option<NodeId>,
    // USD Export node whose stage should be written this frame.
    usd_export_request: Option<NodeId>,
    // File Export node to write: (node, whole timeline range).
    file_export_request: Option<(NodeId, bool)>,
    // Prim chosen in the scene graph tree.
    stage_selected_prim: Option<String>,
    // Selected LOP's stage, keyed by node and cook generation.
    lop_stage_cache: Option<(NodeId, u64, Arc<usd::Stage>)>,
    sheet_domain: SheetDomain,
    // Show the selected node's own output instead of the displayed geometry.
    sheet_from_selected: bool,
    sheet_selected_only: bool,
    // Selected node's cooked output, keyed by node and cook generation.
    sheet_cache: Option<(NodeId, u64, Mesh)>,
    // Bumped on every graph rebuild so cached cooks know they are stale.
    cook_generation: u64,
    tab_open: bool,
    tab_query: String,
    tab_pos: egui::Pos2,
    context: AppContext,
    selected_vertices: Vec<usize>,
    selection_mode: SelectionMode,
    selected_triangles: Vec<usize>,
    selected_edges: Vec<(usize, usize)>,
    isolate_selection: bool,
    selection_box_start: Option<egui::Pos2>,
    selection_box_end: Option<egui::Pos2>,
    inspect_node: Option<NodeId>,
}

impl Default for ProceduralFxStudioApp {
    fn default() -> Self {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(40.0, 20.0));
        let grid = g.add(NodeKind::Grid { size: 4.0, res: 20 }, egui::pos2(40.0, 20.0));
        let noise = g.add(NodeKind::Noise { freq: 2.2, amp: 0.65 }, egui::pos2(40.0, 100.0));
        let taper = g.add(NodeKind::Taper { factor: 0.0 }, egui::pos2(40.0, 180.0));
        let twist = g.add(NodeKind::Twist { angle: 1.8 }, egui::pos2(40.0, 260.0));
        for id in [grid, noise, taper, twist] {
            if let Some(node) = g.nodes.get_mut(&id) {
                node.parent = Some(geo);
            }
        }
        g.connect(grid, noise, 0);
        g.connect(noise, taper, 0);
        g.connect(taper, twist, 0);
        g.displays.insert(geo, twist);
        g.add(
            NodeKind::Light {
                position: [3.0, 4.0, 3.0],
                rotation: [-35.0, 35.0, 0.0],
                color: [1.0, 0.95, 0.85],
                intensity: 0.9,
                kind: 0,
            },
            egui::pos2(240.0, 20.0),
        );
        let g = std::fs::read_to_string(SCENE_FILE)
            .or_else(|_| std::fs::read_to_string(LEGACY_SCENE_FILE))
            .ok()
            .map(|scene| Graph::deserialize(&scene))
            .filter(|scene| !scene.nodes.is_empty())
            .unwrap_or(g);
        let selected = g.geometry_ids().first().copied();
        let last_state = g.serialize();

        let detected_moonray = detect_moonray_executable();
        let default_mr_args = String::from("{scene} {output} {width} {height} {samples}");
        let default_mr_output = home_dir()
            .map(|home| home.join("Pictures").join("render.exr").to_string_lossy().into_owned())
            .unwrap_or_else(|| "render.exr".into());
        let default_mr_w = 1280usize;
        let default_mr_h = 720usize;
        let default_mr_s = 64usize;

        Self {
            graph: g,
            selected,
            dragging_link: None,
            graph_pan: egui::Vec2::ZERO,
            graph_zoom: 1.0,
            menu_pos: egui::Pos2::ZERO,
            render_mode: RenderMode::ShadedWireframe,
            show_points: false,
            show_floor: true,
            show_gizmos: true,
            gizmo_mode: GizmoMode::Translate,
            gizmo_frame: None,
            gizmo_drag: None,
            animate: true,
            anim_speed: 0.02,
            time: 0.0,
            playback_start: 0.0,
            playback_end: 20.0,
            pause_on_edit: true,
            graph_editor: anim::GraphEditor::default(),
            auto_key: false,
            anim_snapshot: None,
            last_content_signature: None,
            cam_rot_x: 0.4,
            cam_rot_y: -0.6,
            cam_pan_x: 0.0,
            cam_pan_y: 0.0,
            cam_zoom: 6.0,
            cam_target: Vec3::zero(),
            camera_focal: 1.2,
            camera_near: 0.5,
            orthographic: false,
            viewport_bg: 45,
            grid_spacing: 0.5,
            grid_extent: 5.0,
            ambient_light: 0.25,
            key_light_dir: [0.5, 0.8, -0.6],
            key_light_intensity: 0.75,
            fill_light_dir: [-0.6, 0.2, 0.5],
            fill_light_intensity: 0.25,
            active_camera: None,
            use_camera_requested: false,
            render_camera: RenderCamera::Auto,
            render_opts: RenderOpts::default(),
            render_at: None,
            sequence: None,
            viewport_focal_px: 720.0 * 1.2,
            current_network: None,
            current_mesh: Mesh::default(),
            cook_ms: 0.0,
            dirty: true,
            status: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_state,
            template: None,
            template_mesh: Mesh::default(),
            static_cache: HashMap::new(),
            cache_key: String::new(),
            frame_cache: HashMap::new(),
            frame_cache_order: VecDeque::new(),
            frame_cache_hits: 0,
            frame_cache_misses: 0,
            simulation_rx: None,
            simulation_key: String::new(),
            cancelled_simulation_key: String::new(),
            simulation_cancel: None,
            moonray_rx: None,
            updater: updater::Updater::new(),
            scene_path: None,
            recent_scenes: load_recent_scenes(),
            window_title: String::new(),
            paint_active: false,
            paint_fill: None,
            group_choices: None,
            group_choices_request: None,
            group_pick: None,
            group_pick_request: None,
            group_pick_finish: None,
            group_pick_domain: None,
            moonray_render_requested: false,
            moonray_default_executable: detected_moonray,
            moonray_default_arguments: default_mr_args,
            moonray_default_output: default_mr_output,
            moonray_default_width: default_mr_w,
            moonray_default_height: default_mr_h,
            moonray_default_samples: default_mr_s,
            show_moonray_settings: false,
            moonray_default_opts: RenderOpts::default(),
            render_to_temp: true,
            render_temp_ext: "exr",
            resolution_scale: 1.0,
            open_viewer_after_render: true,
            bucket_render: true,
            render_extra_args: String::new(),
            render_started: None,
            render_cancel: None,
            render_log_path: None,
            last_render_log: String::new(),
            last_render_summary: String::new(),
            viewer: RenderViewer::default(),
            node_stats: HashMap::new(),
            dock: Some(load_layout().unwrap_or_else(default_layout)),
            popped: Vec::new(),
            open_panes: HashSet::new(),
            pane_requests: Vec::new(),
            saved_layout: String::new(),
            layout_name_prompt: None,
            pyro_fit_request: None,
            usd_export_request: None,
            file_export_request: None,
            stage_selected_prim: None,
            lop_stage_cache: None,
            sheet_domain: SheetDomain::Points,
            sheet_from_selected: false,
            sheet_selected_only: false,
            sheet_cache: None,
            cook_generation: 0,
            tab_open: false,
            tab_query: String::new(),
            tab_pos: egui::Pos2::ZERO,
            context: AppContext::Obj,
            selected_vertices: Vec::new(),
            selection_mode: SelectionMode::Vertex,
            selected_triangles: Vec::new(),
            selected_edges: Vec::new(),
            isolate_selection: false,
            selection_box_start: None,
            selection_box_end: None,
            inspect_node: None,
        }
    }
}

impl ProceduralFxStudioApp {
    fn sync_active_camera_from_viewport(&mut self) {
        let Some(id) = self.active_camera else {
            return;
        };
        let Some(node) = self.graph.nodes.get_mut(&id) else {
            self.active_camera = None;
            return;
        };
        let NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } = &mut node.kind else {
            self.active_camera = None;
            return;
        };
        *position = [self.cam_target.x, self.cam_target.y, self.cam_target.z];
        *rotation = [
            self.cam_rot_x.to_degrees(),
            self.cam_rot_y.to_degrees(),
            rotation[2],
        ];
        *focal = self.camera_focal;
        *near = self.camera_near;
        *orthographic = self.orthographic;
        *pan = [self.cam_pan_x, self.cam_pan_y];
        *zoom = self.cam_zoom;
    }

    fn camera_ids(&self) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. }))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    fn resolve_render_camera(&self) -> (CamRig, String) {
        let node_rig = |id: NodeId| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Camera { position, rotation, focal, orthographic, pan, zoom, .. }) => Some((
                CamRig {
                    target: Vec3::from_arr(*position),
                    rot_x: rotation[0].to_radians(),
                    rot_y: rotation[1].to_radians(),
                    zoom: *zoom,
                    focal: *focal,
                    orthographic: *orthographic,
                    pan: *pan,
                },
                format!("camera{id}"),
            )),
            _ => None,
        };
        let chosen = match self.render_opts.camera {
            RenderCamera::Node(id) => node_rig(id),
            RenderCamera::Auto => self.selected.and_then(node_rig),
            RenderCamera::Viewport => None,
        };
        chosen.unwrap_or_else(|| {
            (
                CamRig {
                    target: self.cam_target,
                    rot_x: self.cam_rot_x,
                    rot_y: self.cam_rot_y,
                    zoom: self.cam_zoom,
                    focal: self.camera_focal,
                    orthographic: self.orthographic,
                    pan: [self.cam_pan_x, self.cam_pan_y],
                },
                "viewport".to_string(),
            )
        })
    }

    fn start_moonray_render(&mut self) {
        if self.moonray_rx.is_some() || self.sequence.is_some() {
            self.status = "A MoonRay render is already running".into();
            return;
        }
        self.status.clear();
        self.render_at = None;
        // A selected render node supplies its own settings; otherwise the toolbar defaults apply.
        if let Some(id) = self.selected {
            if let Some(NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts }) =
                self.graph.nodes.get(&id).map(|node| &node.kind)
            {
                self.render_opts = opts.clone();
                if opts.sequence {
                    let job = SequenceJob {
                        executable: executable.clone(),
                        arguments: arguments.clone(),
                        output: output.clone(),
                        width: *width,
                        height: *height,
                        samples: *samples,
                        opts: opts.clone(),
                        times: frame_times(opts.range_start, opts.range_end, opts.range_step),
                        next: 0,
                    };
                    self.begin_sequence(job);
                    return;
                }
                self.status = format!("Rendering with {} camera...", self.resolve_render_camera().1);
                self.spawn_moonray_render(
                    executable.clone(),
                    arguments.clone(),
                    output.clone(),
                    *width,
                    *height,
                    *samples,
                );
                return;
            }
        }
        // Otherwise the Render Settings dialog applies.
        let opts = RenderOpts { camera: self.render_camera, ..self.moonray_default_opts.clone() };
        self.render_opts = opts.clone();
        // Relative outputs are routed to the temp folder by spawn_moonray_render.
        let output = if self.render_to_temp {
            format!("render.{}", self.render_temp_ext)
        } else {
            self.moonray_default_output.clone()
        };
        if opts.sequence {
            let job = SequenceJob {
                executable: self.moonray_default_executable.clone(),
                arguments: self.moonray_default_arguments.clone(),
                output,
                width: self.moonray_default_width,
                height: self.moonray_default_height,
                samples: self.moonray_default_samples,
                times: frame_times(opts.range_start, opts.range_end, opts.range_step),
                opts,
                next: 0,
            };
            self.begin_sequence(job);
            return;
        }
        self.status = format!("Rendering with {} camera...", self.resolve_render_camera().1);
        self.spawn_moonray_render(
            self.moonray_default_executable.clone(),
            self.moonray_default_arguments.clone(),
            output,
            self.moonray_default_width,
            self.moonray_default_height,
            self.moonray_default_samples,
        );
    }

    fn pane_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pane: Pane) {
        match pane {
            Pane::Viewport => self.viewport(ui, ctx),
            Pane::Network => self.node_editor(ui),
            Pane::Parameters => self.parameters_pane(ui),
            Pane::Timeline => self.timeline_pane(ui),
            Pane::Spreadsheet => self.spreadsheet_pane(ui),
            Pane::GraphEditor => self.graph_editor_pane(ui),
        }
    }

    fn apply_pane_requests(&mut self, dock: &mut egui_dock::DockState<Pane>) {
        for request in std::mem::take(&mut self.pane_requests) {
            match request {
                PaneRequest::Reset => {
                    *dock = default_layout();
                    self.popped.clear();
                }
                PaneRequest::SaveAs(name) => {
                    let saved = safe_layout_name(&name).zip(layouts_dir()).zip(layout_to_text(dock)).map(|((name, dir), text)| {
                        let _ = std::fs::create_dir_all(&dir);
                        (std::fs::write(dir.join(format!("{name}.ron")), text), name)
                    });
                    self.status = match saved {
                        Some((Ok(()), name)) => format!("Layout \"{name}\" saved"),
                        Some((Err(e), _)) => format!("Couldn't save the layout: {e}"),
                        None => "Give the layout a name (letters, numbers, spaces, - or _)".into(),
                    };
                }
                PaneRequest::Load(name) => {
                    match layouts_dir().and_then(|dir| std::fs::read_to_string(dir.join(format!("{name}.ron"))).ok()).and_then(|t| layout_from_text(&t)) {
                        Some(loaded) => {
                            *dock = loaded;
                            self.popped.clear();
                            self.status = format!("Layout \"{name}\"");
                        }
                        None => self.status = format!("Couldn't read layout \"{name}\""),
                    }
                }
                PaneRequest::Delete(name) => {
                    if let Some(dir) = layouts_dir() {
                        let _ = std::fs::remove_file(dir.join(format!("{name}.ron")));
                    }
                    self.status = format!("Layout \"{name}\" deleted");
                }
                PaneRequest::Show(pane, true) => {
                    if dock.find_tab(&pane).is_none() && !self.popped.contains(&pane) {
                        // A new pane goes next to the viewport.
                        let at = dock.find_tab(&Pane::Viewport).map(|(surface, node, _)| (surface, node));
                        match at {
                            Some((surface, node)) if surface == egui_dock::SurfaceIndex::main() => {
                                dock.main_surface_mut().split_below(node, 0.62, vec![pane]);
                            }
                            _ => dock.push_to_first_leaf(pane),
                        }
                    }
                }
                PaneRequest::Show(pane, false) => {
                    if pane != Pane::Viewport {
                        if let Some(at) = dock.find_tab(&pane) {
                            dock.remove_tab(at);
                        }
                        self.popped.retain(|p| *p != pane);
                    }
                }
                PaneRequest::PopOut(pane) => {
                    if !self.popped.contains(&pane) {
                        if let Some(at) = dock.find_tab(&pane) {
                            dock.remove_tab(at);
                        }
                        self.popped.push(pane);
                    }
                }
            }
        }
    }

    fn show_panes(&mut self, ctx: &egui::Context) {
        if let Some(mut name) = self.layout_name_prompt.take() {
            let mut keep = true;
            egui::Window::new("Save Layout").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show(ctx, |ui| {
                ui.label("Layout name");
                let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text("e.g. Modelling, Rendering"));
                field.request_focus();
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || enter {
                        self.pane_requests.push(PaneRequest::SaveAs(name.clone()));
                        keep = false;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        keep = false;
                    }
                });
                if saved_layouts().iter().any(|n| Some(n) == safe_layout_name(&name).as_ref()) {
                    ui.weak("Saving replaces the layout with this name.");
                }
            });
            if keep {
                self.layout_name_prompt = Some(name);
            }
        }
        let mut dock = self.dock.take().unwrap_or_else(default_layout);
        self.apply_pane_requests(&mut dock);
        let pop_out = {
            let mut viewer = PaneViewer { app: self, ctx: ctx.clone(), pop_out: None };
            egui_dock::DockArea::new(&mut dock)
                .style(egui_dock::Style::from_egui(ctx.style().as_ref()))
                .show_leaf_collapse_buttons(false)
                .show(ctx, &mut viewer);
            viewer.pop_out
        };
        if let Some(pane) = pop_out {
            self.pane_requests.push(PaneRequest::PopOut(pane));
        }
        // Popped-out panes: real OS windows; closing one puts the pane back in the dock.
        let mut redock = Vec::new();
        for pane in self.popped.clone() {
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{} — ProceduralFX Studio", pane.title()))
                .with_inner_size(match pane {
                    Pane::Viewport => [900.0, 640.0],
                    Pane::Timeline => [900.0, 160.0],
                    _ => [560.0, 520.0],
                });
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("pane", pane.title())), builder, |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| self.pane_ui(ui, ctx, pane));
                if ctx.input(|i| i.viewport().close_requested()) {
                    redock.push(pane);
                }
            });
        }
        for pane in redock {
            self.popped.retain(|p| *p != pane);
            self.pane_requests.push(PaneRequest::Show(pane, true));
        }
        self.open_panes = dock.iter_all_tabs().map(|(_, tab)| *tab).chain(self.popped.iter().copied()).collect();
        // Keep the arrangement for next time.
        if let Some(text) = layout_to_text(&dock) {
            if text != self.saved_layout {
                if let Some(file) = layout_file() {
                    if let Some(dir) = file.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(file, &text);
                }
                self.saved_layout = text;
            }
        }
        self.dock = Some(dock);
    }

    fn parameters_pane(&mut self, ui: &mut egui::Ui) {
        if self.params_ui(ui) {
            if self.pause_on_edit {
                self.animate = false;
            }
            self.dirty = true;
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.label(
                egui::RichText::new(
                    "Network: RMB add node • drag output→input to wire\n\
                     click input to disconnect • dbl-click = display\n\
                     OBJ: Geometry / Light / Camera nodes need no wiring •\n\
                     dbl-click Geometry to enter • U = up • dbl-click Light/Camera = activate\n\
                     TAB search/add • H home network • T template\n\
                     B bypass • Ctrl+D duplicate • Del delete\n\
                     Ctrl+Z / Ctrl+Shift+Z undo/redo • Space play\n\
                     Selection: All • None • Invert • Grow • Shrink • Boundary • Non-manifold • Isolated • Connected\n\
                     Viewport: LMB orbit • RMB pan • wheel zoom • F frame",
                )
                .small()
                .weak(),
            );
        });
    }

    fn timeline_pane(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                if ui.button("⏮ Start").on_hover_text("Jump to the playback start").clicked() {
                    self.time = self.playback_start;
                    self.dirty = true;
                }
                if ui.add(egui::Button::new(if self.animate { "⏸ Pause" } else { "▶ Play" })).on_hover_text("Space toggles playback").clicked() {
                    self.animate = !self.animate;
                    self.dirty = true;
                }
                if ui.button("⏭ End").on_hover_text("Jump to the playback end").clicked() {
                    self.time = self.playback_end;
                    self.dirty = true;
                }
                ui.label(format!("Time: {:.2}", self.time));
                ui.add(egui::Slider::new(&mut self.anim_speed, 0.001..=0.1).text("Speed")).on_hover_text("Playback step per update");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.status.is_empty() {
                        ui.label(egui::RichText::new(&self.status).color(egui::Color32::LIGHT_GREEN));
                    }
                });
            });

            // Scrub bar: dragging pauses playback and the viewport follows the time.
            ui.horizontal(|ui| {
                let frame = (self.time * sims::SIM_FPS).round() as i64 + 1;
                ui.monospace(format!("Frame {frame:>4}"));
                ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(120.0);
                let scrub = ui
                    .add(egui::Slider::new(&mut self.time, self.playback_start..=self.playback_end).suffix(" s").show_value(true))
                    .on_hover_text("Drag to scrub the timeline (← / → step one frame)");
                if scrub.drag_started() {
                    self.animate = false;
                }
                // Ticks at the selected node's keys.
                if let Some(id) = self.selected {
                    let span = (self.playback_end - self.playback_start).max(1e-6);
                    let rail = scrub.rect.height() * 0.5;
                    let width = ui.spacing().slider_width - rail * 2.0;
                    let times: Vec<f32> = self.graph.node_channels(id).flat_map(|(_, ch)| ch.keys.iter().map(|k| k.t)).collect();
                    for t in times.into_iter().filter(|t| (self.playback_start..=self.playback_end).contains(t)) {
                        let x = scrub.rect.left() + rail + (t - self.playback_start) / span * width;
                        ui.painter().line_segment(
                            [egui::pos2(x, scrub.rect.top()), egui::pos2(x, scrub.rect.top() + 6.0)],
                            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 60)),
                        );
                    }
                }
                if scrub.changed() {
                    self.dirty = true;
                }
            });
            let typing = ui.ctx().wants_keyboard_input();
            let step = ui.input(|i| {
                (i.key_pressed(egui::Key::ArrowRight) as i32) - (i.key_pressed(egui::Key::ArrowLeft) as i32)
            });
            if step != 0 && !typing {
                self.animate = false;
                self.time = (self.time + step as f32 / sims::SIM_FPS).clamp(self.playback_start, self.playback_end);
                self.dirty = true;
            }

            ui.horizontal(|ui| {
                let range_changed = ui.add(egui::DragValue::new(&mut self.playback_start).speed(0.1).prefix("Start ")).changed()
                    | ui.add(egui::DragValue::new(&mut self.playback_end).speed(0.1).prefix("End   ")).changed();
                if self.playback_end < self.playback_start {
                    self.playback_end = self.playback_start;
                }
                if range_changed {
                    self.time = self.time.clamp(self.playback_start, self.playback_end);
                    self.dirty = true;
                }
                ui.checkbox(&mut self.pause_on_edit, "Pause on edit").on_hover_text("Pause playback whenever the scene is edited: parameters, wiring, flags or gizmo drags");
            });
        });
    }

    fn spreadsheet_pane(&mut self, ui: &mut egui::Ui) {
        let source_node = if self.sheet_from_selected { self.selected } else { None };
        if let Some(id) = source_node {
            let fresh = matches!(&self.sheet_cache, Some((cached, generation, _)) if *cached == id && *generation == self.cook_generation);
            if !fresh {
                let mut cache = HashMap::new();
                let mesh = self.graph.cook(id, self.time, &mut cache);
                self.sheet_cache = Some((id, self.cook_generation, mesh));
            }
        }
        let source_label = match source_node {
            Some(id) => self
                .graph
                .nodes
                .get(&id)
                .map(|node| node.kind.label(id))
                .unwrap_or_else(|| format!("node {id}")),
            None if self.sheet_from_selected => "no node selected".into(),
            None => "final geometry in the viewport".into(),
        };
        // Borrow the mesh out of self for the frame instead of cloning it.
        let mesh = match (source_node, self.sheet_cache.as_mut()) {
            (Some(_), Some((_, _, cached))) => std::mem::take(cached),
            _ if self.sheet_from_selected => Mesh::default(),
            _ => std::mem::take(&mut self.current_mesh),
        };
        self.spreadsheet_ui(ui, &mesh, source_node.is_none() && !self.sheet_from_selected, &source_label);
        match (source_node, self.sheet_cache.as_mut()) {
            (Some(_), Some((_, _, cached))) => *cached = mesh,
            _ if self.sheet_from_selected => {}
            _ => self.current_mesh = mesh,
        }
    }

    // `selectable`: rows map to the viewport geometry, so clicking them selects components.
    fn spreadsheet_ui(&mut self, ui: &mut egui::Ui, mesh: &Mesh, selectable: bool, source_label: &str) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Geometry");
            ui.selectable_value(&mut self.sheet_from_selected, false, "Displayed output");
            ui.selectable_value(&mut self.sheet_from_selected, true, "Selected node")
                .on_hover_text("Inspect the output of the node selected in the network, e.g. a wrangle mid-chain");
            ui.weak(source_label);
        });
        ui.horizontal_wrapped(|ui| {
            for domain in [SheetDomain::Points, SheetDomain::Vertices, SheetDomain::Primitives, SheetDomain::Detail] {
                let label = match domain {
                    SheetDomain::Points => format!("Points ({})", mesh.vertices.len()),
                    SheetDomain::Vertices => format!("Vertices ({})", mesh.indices.len() * 3),
                    SheetDomain::Primitives => format!("Primitives ({})", mesh.indices.len()),
                    SheetDomain::Detail => "Detail".to_string(),
                };
                ui.selectable_value(&mut self.sheet_domain, domain, label);
            }
        });

        let domain = self.sheet_domain;
        let columns = sheet_columns(mesh, domain);
        let normals = if columns.iter().any(|c| matches!(c.source, SheetSource::ComputedNormal(_))) {
            mesh.compute_normals()
        } else {
            Vec::new()
        };
        let count = domain.count(mesh);
        let selection: Option<&Vec<usize>> = match domain {
            SheetDomain::Points if selectable => Some(&self.selected_vertices),
            SheetDomain::Primitives if selectable => Some(&self.selected_triangles),
            _ => None,
        };
        let selected: HashSet<usize> = selection.map(|s| s.iter().copied().collect()).unwrap_or_default();
        let filter = self.sheet_selected_only && selection.is_some();
        let rows: Vec<usize> = if domain == SheetDomain::Detail {
            if columns.is_empty() { Vec::new() } else { vec![0] }
        } else if filter {
            let mut rows: Vec<usize> = selected.iter().copied().filter(|i| *i < count).collect();
            rows.sort_unstable();
            rows
        } else {
            (0..count).collect()
        };

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(selection.is_some(), |ui| {
                ui.checkbox(&mut self.sheet_selected_only, "Selected only")
                    .on_hover_text("Points and primitives of the displayed output; click rows to select them (Shift adds)");
            });
            if ui.add_enabled(!rows.is_empty(), egui::Button::new("Copy CSV")).on_hover_text("Copy the rows shown as comma-separated values").clicked() {
                let quote = |text: &str| if text.contains(',') || text.contains('"') { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() };
                let mut csv = columns.iter().map(|c| quote(&c.header)).collect::<Vec<_>>().join(",");
                for &row in &rows {
                    csv.push('\n');
                    csv.push_str(&columns.iter().map(|c| quote(&sheet_cell(mesh, &normals, &c.source, row))).collect::<Vec<_>>().join(","));
                }
                ui.ctx().copy_text(csv);
                self.status = format!("Copied {} rows", rows.len());
            }
            if let Some((lo, hi)) = mesh.bounds() {
                ui.weak(format!("Bounds ({:.2}, {:.2}, {:.2}) to ({:.2}, {:.2}, {:.2})", lo.x, lo.y, lo.z, hi.x, hi.y, hi.z));
            }
            let warnings = mesh.validation_warnings();
            if !warnings.is_empty() {
                ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", warnings.join(" · ")));
            }
        });

        egui::CollapsingHeader::new("Attribute summary").id_salt("sheet_summary").show(ui, |ui| {
            egui::Grid::new("sheet_summary_grid").striped(true).num_columns(4).show(ui, |ui| {
                for header in ["Attribute", "Class", "Size", "Range"] {
                    ui.strong(header);
                }
                ui.end_row();
                for summary_domain in [SheetDomain::Points, SheetDomain::Vertices, SheetDomain::Primitives, SheetDomain::Detail] {
                    let class = match summary_domain {
                        SheetDomain::Points => "point",
                        SheetDomain::Vertices => "vertex",
                        SheetDomain::Primitives => "primitive",
                        SheetDomain::Detail => "detail",
                    };
                    for attribute in sheet_attributes(mesh, summary_domain) {
                        let values = &mesh.point_attributes[&attribute.key];
                        let ranges = (0..attribute.components.min(4))
                            .map(|c| {
                                let lane = values.iter().skip(c).step_by(attribute.components);
                                let (lo, hi) = lane.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
                                if lo > hi { "-".to_string() } else { format!("{lo:.3} … {hi:.3}") }
                            })
                            .collect::<Vec<_>>()
                            .join("   ");
                        ui.monospace(&attribute.name);
                        ui.label(class);
                        ui.label(format!("{}{}", attribute.components, if attribute.size_ok { "" } else { " ⚠" }));
                        ui.monospace(ranges);
                        ui.end_row();
                    }
                }
            });
        });
        ui.separator();

        if rows.is_empty() {
            ui.weak(match domain {
                SheetDomain::Detail => "No detail attributes. Create one with a Detail wrangle, e.g. f@total = npoints(0);",
                _ if filter => "Nothing selected.",
                _ => "No elements.",
            });
            return;
        }
        let shift = ui.input(|i| i.modifiers.shift);
        let mut clicked = None;
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            let mut table = egui_extras::TableBuilder::new(ui)
                .id_salt(("geometry_sheet", domain as u8, columns.len()))
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .min_scrolled_height(0.0)
                .max_scroll_height(f32::INFINITY);
            for (i, column) in columns.iter().enumerate() {
                table = table.column(if i == 0 && matches!(column.source, SheetSource::Index) {
                    egui_extras::Column::auto().at_least(44.0)
                } else {
                    egui_extras::Column::initial(74.0).at_least(36.0).clip(true)
                });
            }
            table
                .header(20.0, |mut header| {
                    for column in &columns {
                        header.col(|ui| {
                            ui.strong(&column.header).on_hover_text(&column.tooltip);
                        });
                    }
                })
                .body(|body| {
                    body.rows(18.0, rows.len(), |mut row| {
                        let element = rows[row.index()];
                        row.set_selected(selected.contains(&element));
                        for column in &columns {
                            row.col(|ui| {
                                ui.label(egui::RichText::new(sheet_cell(mesh, &normals, &column.source, element)).monospace());
                            });
                        }
                        if row.response().clicked() {
                            clicked = Some(element);
                        }
                    });
                });
        });

        if let (Some(element), true) = (clicked, selection.is_some()) {
            let (list, mode) = match domain {
                SheetDomain::Points => (&mut self.selected_vertices, SelectionMode::Vertex),
                _ => (&mut self.selected_triangles, SelectionMode::Triangle),
            };
            if shift {
                match list.iter().position(|i| *i == element) {
                    Some(at) => {
                        list.remove(at);
                    }
                    None => list.push(element),
                }
            } else {
                *list = vec![element];
            }
            if self.selection_mode != mode {
                self.selection_mode = mode;
                self.selected_edges.clear();
                match mode {
                    SelectionMode::Vertex => self.selected_triangles.clear(),
                    _ => self.selected_vertices.clear(),
                }
            }
        }
    }

    // Writes a File Export node's input at the current time, or every timeline frame ($F in the name).
    fn write_file_export(&mut self, id: NodeId, range: bool) -> Result<String, String> {
        let Some(NodeKind::FileExport { file }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else {
            return Err("not a File Export node".into());
        };
        let source = self.graph.input_of(id, 0).ok_or("connect geometry to the input")?;
        let times = if range {
            frame_times(self.playback_start.max(0.0), self.playback_end, 1.0 / sims::SIM_FPS)
        } else {
            vec![self.time]
        };
        let mut written = Vec::new();
        for time in &times {
            let mesh = self.graph.cook_fresh(Some(source), *time);
            let path = fileio::expand_frame(file.trim(), *time, sims::SIM_FPS);
            write_geometry_file(&mesh, &path).map_err(|e| format!("{path}: {e}"))?;
            written.push(path);
        }
        Ok(match written.as_slice() {
            [one] => format!("Wrote {one}"),
            many => format!("Wrote {} files ({} … {})", many.len(), many[0], many[many.len() - 1]),
        })
    }

    // Applies the animation at the current time; an animated camera being looked through drives the view.
    fn begin_animation_frame(&mut self, ctx: &egui::Context) {
        self.graph.apply_animation(self.time);
        if let Some(camera) = self.active_camera.filter(|c| self.graph.is_animated(*c)) {
            if let Some(NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom }) = self.graph.nodes.get(&camera).map(|n| n.kind.clone()) {
                self.cam_target = Vec3::from_arr(position);
                self.cam_rot_x = rotation[0].to_radians();
                self.cam_rot_y = rotation[1].to_radians();
                self.camera_focal = focal;
                self.camera_near = near;
                self.orthographic = orthographic;
                (self.cam_pan_x, self.cam_pan_y) = (pan[0], pan[1]);
                self.cam_zoom = zoom;
            }
        }
        self.anim_snapshot = self
            .selected
            .and_then(|id| self.graph.nodes.get(&id).map(|n| (id, n.kind.params(), n.xform)));
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::K) && !i.modifiers.command) {
            self.key_selected();
        }
    }

    // Keys this frame's edits: animated parameters always, anything else with Auto Key.
    fn end_animation_frame(&mut self) {
        let mut changed = self.graph.commit_animation_edits(self.time);
        if self.auto_key {
            if let Some((id, params, xform)) = self.anim_snapshot.take() {
                let targets = self.graph.nodes.get(&id).map(|n| n.kind.animatable()).unwrap_or_default();
                for (target, _) in targets {
                    let channel_id = ChannelId { node: id, target };
                    if self.graph.channels.contains_key(&channel_id) {
                        continue;
                    }
                    let before = match target {
                        ChannelTarget::Param(i) => params.get(i).copied(),
                        ChannelTarget::Xform(i) => Some(xform_component(&xform, i)),
                    };
                    let now = self.graph.channel_value(channel_id);
                    if let (Some(before), Some(now)) = (before, now) {
                        if (now - before).abs() > 1e-4 * before.abs().max(1.0) {
                            self.graph.set_key(channel_id, self.time);
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    fn frame_number(&self) -> i64 {
        (self.time * sims::SIM_FPS).round() as i64 + 1
    }

    // Keys the selected node at the current time: an object's transform, or all its animatable parameters.
    fn key_selected(&mut self) {
        let Some(id) = self.selected else {
            self.status = "Select a node to key".into();
            return;
        };
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let mut targets = node.kind.animatable();
        if matches!(node.kind, NodeKind::Light { .. } | NodeKind::Camera { .. }) {
            targets.truncate(6);
        }
        if targets.is_empty() {
            self.status = format!("{} has no parameters that can be animated", node.kind.label(id));
            return;
        }
        let label = node.kind.label(id);
        for (target, _) in &targets {
            self.graph.set_key(ChannelId { node: id, target: *target }, self.time);
        }
        self.dirty = true;
        self.status = format!("Keyed {} channels of {label} at frame {}", targets.len(), self.frame_number());
    }

    fn graph_editor_pane(&mut self, ui: &mut egui::Ui) {
        // The selected node's curves when it is animated, otherwise every animated channel.
        let focus = self.selected.filter(|id| self.graph.is_animated(*id));
        let mut wanted: HashMap<ChannelId, (String, String)> = HashMap::new();
        for channel_id in self.graph.channels.keys() {
            let Some(node) = self.graph.nodes.get(&channel_id.node) else { continue };
            if focus.is_some_and(|f| f != channel_id.node) {
                continue;
            }
            let group = node.kind.label(channel_id.node);
            let name = node
                .kind
                .animatable()
                .into_iter()
                .find(|(t, _)| *t == channel_id.target)
                .map_or_else(|| format!("{:?}", channel_id.target), |(_, n)| n);
            wanted.insert(*channel_id, (format!("{group} · {name}"), group));
        }
        let mut rows: Vec<anim::ChannelRow> = self
            .graph
            .channels
            .iter_mut()
            .filter_map(|(id, channel)| wanted.get(id).map(|(label, group)| anim::ChannelRow { label: label.clone(), group: group.clone(), channel }))
            .collect();
        let result = self.graph_editor.ui_inside(ui, &mut rows, self.time, (self.playback_start, self.playback_end), sims::SIM_FPS);
        drop(rows);
        if result.changed {
            self.graph.channels.retain(|_, channel| !channel.keys.is_empty());
            self.dirty = true;
        }
        if let Some(t) = result.scrub {
            self.time = t.clamp(self.playback_start, self.playback_end.max(self.playback_start));
            self.animate = false;
            self.dirty = true;
        }
    }

    // Eye and look-at point of the viewport camera, for Camera LOPs.
    fn viewport_eye_target(&self) -> ([f32; 3], [f32; 3]) {
        let (sx, cx) = self.cam_rot_x.sin_cos();
        let (sy, cy) = self.cam_rot_y.sin_cos();
        let right = Vec3::new(cy, 0.0, -sy);
        let up = Vec3::new(-sx * sy, cx, -sx * cy);
        let forward = Vec3::new(cx * sy, sx, cx * cy);
        let pan_scale = self.cam_zoom / self.viewport_focal_px.max(1.0);
        let target = self.cam_target.sub(right.scale(self.cam_pan_x * pan_scale)).add(up.scale(self.cam_pan_y * pan_scale));
        let eye = target.sub(forward.scale(self.cam_zoom));
        ([eye.x, eye.y, eye.z], [target.x, target.y, target.z])
    }

    // Prim hierarchy of a LOP's stage, with the picked prim's properties.
    fn scene_graph_ui(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let fresh = matches!(&self.lop_stage_cache, Some((node, generation, _)) if *node == id && *generation == self.cook_generation);
        if !fresh {
            let stage = self.graph.cook_stage(id, self.time, &mut HashMap::new());
            self.lop_stage_cache = Some((id, self.cook_generation, stage));
        }
        let Some((_, _, stage)) = self.lop_stage_cache.clone() else { return };
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Scene Graph");
            ui.weak(format!("{} prims", stage.prims.len()));
        });
        egui::ScrollArea::vertical().id_salt("scene_graph").max_height(240.0).show(ui, |ui| {
            if stage.prims.is_empty() {
                ui.weak("Empty stage. Add SOP Import, Reference or Primitive nodes.");
            }
            let roots: Vec<String> = stage.prims.keys().filter(|p| usd::parent_path(p) == Some("/")).cloned().collect();
            for root in roots {
                self.prim_tree_row(ui, &stage, &root, 0);
            }
        });
        let Some(path) = self.stage_selected_prim.clone() else { return };
        let Some(prim) = stage.prims.get(&path) else { return };
        ui.separator();
        egui::Grid::new("prim_properties").num_columns(2).striped(true).show(ui, |ui| {
            ui.weak("Path");
            ui.horizontal(|ui| {
                ui.monospace(&path);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(path.clone());
                }
            });
            ui.end_row();
            ui.weak("Type");
            ui.label(&prim.type_name);
            ui.end_row();
            let world = stage.world_xform(&path);
            ui.weak("World position");
            ui.monospace(format!("{:.3}, {:.3}, {:.3}", world[3][0], world[3][1], world[3][2]));
            ui.end_row();
            ui.weak("State");
            ui.label(match (prim.active, prim.visible) {
                (false, _) => "inactive",
                (true, false) => "invisible",
                _ => "active, visible",
            });
            ui.end_row();
            if let Some(mesh) = &prim.mesh {
                if prim.type_name == "Mesh" {
                    ui.weak("Geometry");
                    ui.label(format!("{} points, {} triangles", mesh.vertices.len(), mesh.indices.len()));
                    ui.end_row();
                    let mut names: Vec<&String> = mesh.point_attributes.keys().collect();
                    names.sort();
                    ui.weak("Primvars");
                    ui.label(names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", "));
                    ui.end_row();
                }
            }
            if let Some(material) = stage.bound_material(&path) {
                ui.weak("Material");
                ui.monospace(material);
                ui.end_row();
            }
            for (name, value) in &prim.attrs {
                ui.weak(name);
                ui.monospace(match value {
                    usd::AttrValue::Float(f) => format!("{f:.4}"),
                    usd::AttrValue::Vec3(v) => format!("{:.3}, {:.3}, {:.3}", v[0], v[1], v[2]),
                    usd::AttrValue::Token(t) => t.clone(),
                });
                ui.end_row();
            }
        });
    }

    fn prim_tree_row(&mut self, ui: &mut egui::Ui, stage: &usd::Stage, path: &str, depth: usize) {
        let Some(prim) = stage.prims.get(path) else { return };
        let children: Vec<String> = stage.children(path).map(|(p, _)| p.clone()).collect();
        let selected = self.stage_selected_prim.as_deref() == Some(path);
        let mut text = egui::RichText::new(format!("{}  ", usd::prim_name(path)));
        if !prim.active {
            text = text.strikethrough().weak();
        } else if !prim.visible {
            text = text.weak();
        }
        let mut row = |ui: &mut egui::Ui, this: &mut Self| {
            ui.horizontal(|ui| {
                if ui.selectable_label(selected, text.clone()).clicked() {
                    this.stage_selected_prim = Some(path.to_string());
                }
                ui.weak(&prim.type_name);
            });
        };
        if children.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                row(ui, self);
            });
            return;
        }
        let state_id = ui.make_persistent_id(("scene_graph_prim", path));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), state_id, depth < 2)
            .show_header(ui, |ui| row(ui, self))
            .body(|ui| {
                for child in &children {
                    self.prim_tree_row(ui, stage, child, depth + 1);
                }
            });
    }

    // Menu bar for occasional commands, plus one row of the controls used all the time.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.strong("ProceduralFX Studio");
            ui.separator();
            ui.menu_button("File", |ui| {
                let menu_ctx = ui.ctx().clone();
                let shortcut = |s: &egui::KeyboardShortcut| menu_ctx.format_shortcut(s);
                if ui.add(egui::Button::new("Open…").shortcut_text(shortcut(&SHORTCUT_OPEN))).clicked() {
                    self.open_scene_dialog();
                    ui.close();
                }
                ui.add_enabled_ui(!self.recent_scenes.is_empty(), |ui| {
                    ui.menu_button("Open Recent", |ui| {
                        let mut chosen = None;
                        for path in &self.recent_scenes {
                            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
                            let button = ui
                                .add_enabled(path.exists(), egui::Button::new(name))
                                .on_hover_text(path.display().to_string())
                                .on_disabled_hover_text(format!("{} (not found)", path.display()));
                            if button.clicked() {
                                chosen = Some(path.clone());
                            }
                        }
                        ui.separator();
                        if ui.button("Clear Recent").clicked() {
                            self.recent_scenes.clear();
                            save_recent_scenes(&self.recent_scenes);
                            ui.close();
                        }
                        if let Some(path) = chosen {
                            self.open_scene(path);
                            ui.close();
                        }
                    });
                });
                ui.separator();
                let save_hover = match &self.scene_path {
                    Some(path) => format!("Save to {}", path.display()),
                    None => "Choose where to save this scene".to_owned(),
                };
                if ui.add(egui::Button::new("Save").shortcut_text(shortcut(&SHORTCUT_SAVE))).on_hover_text(save_hover).clicked() {
                    self.save_scene();
                    ui.close();
                }
                if ui.add(egui::Button::new("Save As…").shortcut_text(shortcut(&SHORTCUT_SAVE_AS))).clicked() {
                    self.save_scene_dialog();
                    ui.close();
                }
                ui.separator();
                if ui.button("Export OBJ").on_hover_text("Writes the displayed mesh to procedural_output.obj").clicked() {
                    self.status = match self.current_mesh.export_obj("procedural_output.obj") {
                        Ok(_) => "Exported procedural_output.obj".into(),
                        Err(e) => format!("Export failed: {e}"),
                    };
                    ui.close();
                }
                if ui.button("Export STL").on_hover_text("Writes the displayed mesh to procedural_output.stl").clicked() {
                    self.status = match self.current_mesh.export_stl("procedural_output.stl") {
                        Ok(_) => "Exported procedural_output.stl".into(),
                        Err(e) => format!("Export failed: {e}"),
                    };
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
                    self.do_undo();
                    ui.close();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z")).clicked() {
                    self.do_redo();
                    ui.close();
                }
            });
            ui.menu_button("Select", |ui| {
                let actions: [(&str, fn(&mut Self)); 9] = [
                    ("All", Self::select_all_components),
                    ("None", Self::clear_component_selection),
                    ("Invert", Self::invert_component_selection),
                    ("Grow", Self::grow_component_selection),
                    ("Shrink", Self::shrink_component_selection),
                    ("Boundary edges", Self::select_boundary_edges),
                    ("Non-manifold edges", Self::select_non_manifold_edges),
                    ("Isolated points", Self::select_isolated_vertices),
                    ("Connected", Self::select_connected_components),
                ];
                for (i, (label, action)) in actions.into_iter().enumerate() {
                    if i == 5 {
                        ui.separator();
                    }
                    if ui.button(label).clicked() {
                        action(self);
                        ui.close();
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.isolate_selection, "Isolate selection");
                ui.weak("Shift-drag: box select • double-click an edge: loop");
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_points, "Points");
                ui.checkbox(&mut self.show_floor, "Floor grid");
                ui.checkbox(&mut self.show_gizmos, "Gizmos");
                ui.checkbox(&mut self.orthographic, "Orthographic");
                ui.separator();
                for (label, rot_x, rot_y) in [
                    ("Top", -1.52, 0.0),
                    ("Front", 0.0, 0.0),
                    ("Right", 0.0, std::f32::consts::FRAC_PI_2),
                    ("Perspective", 0.4, -0.6),
                ] {
                    if ui.button(format!("{label} view")).clicked() {
                        self.set_camera_view(rot_x, rot_y);
                        ui.close();
                    }
                }
                if ui.add(egui::Button::new("Frame all").shortcut_text("F")).clicked() {
                    self.frame_all();
                    ui.close();
                }
                ui.separator();
                ui.add(egui::Slider::new(&mut self.viewport_bg, 15..=80).text("Background"));
                ui.add(egui::Slider::new(&mut self.grid_spacing, 0.1..=2.0).text("Grid spacing"));
                ui.add(egui::Slider::new(&mut self.grid_extent, 1.0..=20.0).text("Grid extent"));
                ui.menu_button("Camera & lights", |ui| {
                    ui.add(egui::Slider::new(&mut self.camera_focal, 0.5..=2.5).text("Focal scale"));
                    ui.add(egui::Slider::new(&mut self.camera_near, 0.05..=2.0).text("Near clip"));
                    ui.separator();
                    ui.label("Viewport lighting (used when the scene has no lights)");
                    ui.add(egui::Slider::new(&mut self.ambient_light, 0.0..=1.0).text("Ambient"));
                    vec3_ui(ui, "Key direction", &mut self.key_light_dir, 0.05);
                    ui.add(egui::Slider::new(&mut self.key_light_intensity, 0.0..=2.0).text("Key intensity"));
                    vec3_ui(ui, "Fill direction", &mut self.fill_light_dir, 0.05);
                    ui.add(egui::Slider::new(&mut self.fill_light_intensity, 0.0..=2.0).text("Fill intensity"));
                });
            });
            ui.menu_button("Render", |ui| {
                if ui.button("Render with MoonRay").clicked() {
                    self.start_moonray_render();
                    ui.close();
                }
                if ui.button("Render Settings…").clicked() {
                    self.show_moonray_settings = true;
                    ui.close();
                }
                if ui.button("Render Viewer").clicked() {
                    self.viewer.show();
                    ui.close();
                }
            });
            ui.menu_button("Windows", |ui| {
                for pane in Pane::ALL {
                    let mut open = self.open_panes.contains(&pane);
                    if ui.checkbox(&mut open, pane.title()).changed() {
                        self.pane_requests.push(PaneRequest::Show(pane, open));
                    }
                }
                ui.separator();
                ui.menu_button("Open in New Window", |ui| {
                    for pane in Pane::ALL {
                        if ui.button(pane.title()).clicked() {
                            self.pane_requests.push(PaneRequest::PopOut(pane));
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Layouts", |ui| {
                    if ui.button("Save Current Layout…").clicked() {
                        self.layout_name_prompt = Some(String::new());
                        ui.close();
                    }
                    let names = saved_layouts();
                    if !names.is_empty() {
                        ui.separator();
                    }
                    for name in names {
                        ui.horizontal(|ui| {
                            if ui.button(&name).on_hover_text("Switch to this layout").clicked() {
                                self.pane_requests.push(PaneRequest::Load(name.clone()));
                                ui.close();
                            }
                            if ui.small_button("🗑").on_hover_text("Delete this layout").clicked() {
                                self.pane_requests.push(PaneRequest::Delete(name.clone()));
                            }
                        });
                    }
                    ui.separator();
                    if ui.button("Reset Layout").on_hover_text("Back to the default arrangement of panes").clicked() {
                        self.pane_requests.push(PaneRequest::Reset);
                        ui.close();
                    }
                });
                ui.weak("Drag tabs to rearrange or split; drag one out to float it.");
                ui.separator();
                if ui.button("Render Viewer").clicked() {
                    self.viewer.show();
                    ui.close();
                }
                if ui.button("Render Settings").clicked() {
                    self.show_moonray_settings = true;
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| self.updater.menu(ui));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.updater.indicator(ui));
        });

        ui.horizontal(|ui| {
            for (context, label, hover) in [
                (AppContext::Obj, "OBJ", "Objects and their geometry (SOP) networks"),
                (AppContext::Mat, "MAT", "Materials"),
                (AppContext::Out, "OUT", "Render nodes"),
                (AppContext::Stage, "STAGE", "USD stage (LOP) network"),
            ] {
                if ui.selectable_label(self.context == context, egui::RichText::new(label).strong()).on_hover_text(hover).clicked()
                    && self.context != context
                {
                    self.context = context;
                    // An open network belongs to its own context (a Material Builder to MAT).
                    if self.network().is_some_and(|id| self.graph.nodes.get(&id).is_some_and(|n| n.context != context)) {
                        self.current_network = None;
                    }
                    self.selected = None;
                    self.dirty = true;
                }
            }
            ui.separator();
            ui.selectable_value(&mut self.render_mode, RenderMode::ShadedWireframe, "Shaded");
            ui.selectable_value(&mut self.render_mode, RenderMode::FlatShaded, "Flat");
            ui.selectable_value(&mut self.render_mode, RenderMode::Wireframe, "Wire");
            ui.selectable_value(&mut self.render_mode, RenderMode::Normals, "Normals");
            ui.separator();
            let mode_changed = ui.selectable_value(&mut self.selection_mode, SelectionMode::Vertex, "Points").changed()
                || ui.selectable_value(&mut self.selection_mode, SelectionMode::Triangle, "Faces").changed()
                || ui.selectable_value(&mut self.selection_mode, SelectionMode::Edge, "Edges").changed();
            if mode_changed {
                self.clear_component_selection();
            }
            ui.separator();
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("↶")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                self.do_undo();
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("↷")).on_hover_text("Redo (Ctrl+Shift+Z)").clicked() {
                self.do_redo();
            }
            ui.separator();
            for (mode, label, hover) in [
                (GizmoMode::Translate, "✥ Move", "Move the selected object (W)"),
                (GizmoMode::Rotate, "⟳ Rotate", "Rotate the selected object (E)"),
                (GizmoMode::Scale, "⤢ Scale", "Scale the selected geometry object (R)"),
            ] {
                ui.selectable_value(&mut self.gizmo_mode, mode, label).on_hover_text(hover);
            }
            ui.separator();
            if ui.button("◆ Key").on_hover_text("Key the selected node at this frame (K)").clicked() {
                self.key_selected();
            }
            ui.toggle_value(&mut self.auto_key, "Auto Key").on_hover_text("Key every parameter you change, not only animated ones");
            let mut graph_open = self.open_panes.contains(&Pane::GraphEditor);
            if ui.toggle_value(&mut graph_open, "📈 Graph").on_hover_text("Graph Editor").changed() {
                self.pane_requests.push(PaneRequest::Show(Pane::GraphEditor, graph_open));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙").on_hover_text("Render Settings").clicked() {
                    self.show_moonray_settings = true;
                }
                if self.moonray_rx.is_some() || self.sequence.is_some() {
                    if ui.button("■ Cancel").clicked() {
                        self.cancel_render();
                    }
                    if let Some(started) = self.render_started {
                        ui.label(format!("{:.0}s", started.elapsed().as_secs_f32()));
                    }
                    ui.spinner();
                } else if ui.button(egui::RichText::new("▶ Render").strong()).on_hover_text("Render with MoonRay").clicked() {
                    self.start_moonray_render();
                }
                let cameras = self.camera_ids();
                render_camera_combo(ui, &mut self.render_camera, &cameras);
                ui.weak("Camera");
            });
        });
    }

    fn open_scene_dialog(&mut self) {
        let mut dialog = FileDialog::new().add_filter("ProceduralFX Scene", &[SCENE_EXTENSION, "mhip", "json"]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.pick_file() {
            Some(path) => self.open_scene(path),
            None => self.status = "Load canceled".into(),
        }
    }

    fn open_scene(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                self.undo.push(self.last_state.clone());
                self.load_state(s);
                self.status = format!("Loaded {}", path.display());
                push_recent_scene(&mut self.recent_scenes, path.clone());
                save_recent_scenes(&self.recent_scenes);
                self.scene_path = Some(path);
            }
            Err(e) => {
                self.status = format!("Load failed: {e}");
                if !path.exists() {
                    self.recent_scenes.retain(|p| p != &path);
                    save_recent_scenes(&self.recent_scenes);
                }
            }
        }
    }

    // Saves to the current scene file, or asks where to save if there isn't one yet.
    fn save_scene(&mut self) {
        match self.scene_path.clone() {
            // An old .mhip scene is saved as .pfs beside it; the original is left alone.
            Some(path) if path.extension().is_some_and(|e| e == "mhip") => self.write_scene(path.with_extension(SCENE_EXTENSION)),
            Some(path) => self.write_scene(path),
            None => self.save_scene_dialog(),
        }
    }

    fn save_scene_dialog(&mut self) {
        let name = self.scene_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        // Saving an old .mhip scene writes a .pfs next to it.
        let name = name.map(|n| match n.strip_suffix(".mhip") {
            Some(stem) => format!("{stem}.{SCENE_EXTENSION}"),
            None => n,
        });
        let mut dialog = FileDialog::new().set_file_name(name.as_deref().unwrap_or(SCENE_FILE)).add_filter("ProceduralFX Scene", &[SCENE_EXTENSION]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.save_file() {
            Some(path) => self.write_scene(path),
            None => self.status = "Save canceled".into(),
        }
    }

    fn write_scene(&mut self, path: PathBuf) {
        match write_file_atomically(&path, &self.graph.serialize()) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                push_recent_scene(&mut self.recent_scenes, path.clone());
                save_recent_scenes(&self.recent_scenes);
                self.scene_path = Some(path);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.scene_path.as_ref().and_then(|p| p.file_name()) {
            Some(name) => format!("{} — ProceduralFX Studio", name.to_string_lossy()),
            None => "ProceduralFX Studio".to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn render_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_moonray_settings {
            return;
        }
        let cameras = self.camera_ids();
        let node_selected = self
            .selected
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::MoonRayRender { .. })));
        let rendering = self.moonray_rx.is_some() || self.sequence.is_some();
        // Live log while a render runs; the finished log afterwards.
        let log = match (&self.render_log_path, self.moonray_rx.is_some()) {
            (Some(path), true) => read_log_tail(path, 64 * 1024),
            _ => self.last_render_log.clone(),
        };
        let mut open = true;
        let mut start = false;
        let mut cancel = false;
        egui::Window::new("Render Settings")
            .open(&mut open)
            .default_width(440.0)
            .resizable(true)
            .show(ctx, |ui| {
                if node_selected {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 190, 90),
                        "A MoonRay render node is selected: Render uses that node's settings. Resolution scale still applies.",
                    );
                }
                egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
                    egui::CollapsingHeader::new("Renderer").default_open(true).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Executable");
                            ui.add(egui::TextEdit::singleline(&mut self.moonray_default_executable).desired_width(260.0));
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Browse…").clicked() {
                                if let Some(path) = FileDialog::new().pick_file() {
                                    self.moonray_default_executable = path.to_string_lossy().into_owned();
                                }
                            }
                            if ui.button("Auto-detect").clicked() {
                                self.moonray_default_executable = detect_moonray_executable();
                            }
                            match resolve_executable(&self.moonray_default_executable) {
                                Some(path) => ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✔ {}", path.display())),
                                None => ui.colored_label(egui::Color32::from_rgb(240, 110, 110), "✖ not found"),
                            };
                        });
                    });

                    egui::CollapsingHeader::new("Image").default_open(true).show(ui, |ui| {
                        let (w, h) = (&mut self.moonray_default_width, &mut self.moonray_default_height);
                        let current = RESOLUTION_PRESETS
                            .iter()
                            .find(|(_, pw, ph)| *pw == *w && *ph == *h)
                            .map(|(name, ..)| *name)
                            .unwrap_or("Custom");
                        ui.horizontal(|ui| {
                            ui.label("Resolution");
                            egui::ComboBox::from_id_salt("render_resolution_preset")
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    for (name, pw, ph) in RESOLUTION_PRESETS {
                                        if ui.selectable_label(current == name, format!("{name}  ({pw}×{ph})")).clicked() {
                                            (*w, *h) = (pw, ph);
                                        }
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(w).range(1..=16384).prefix("W "));
                            if ui.button("⇄").on_hover_text("Swap width and height").clicked() {
                                std::mem::swap(w, h);
                            }
                            ui.add(egui::DragValue::new(h).range(1..=16384).prefix("H "));
                            ui.weak(format!("aspect {:.3}", *w as f32 / (*h).max(1) as f32));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Resolution scale");
                            for (scale, label) in [(1.0, "100%"), (0.5, "50%"), (0.25, "25%")] {
                                ui.selectable_value(&mut self.resolution_scale, scale, label);
                            }
                        })
                        .response
                        .on_hover_text("Quick previews: scales every render, render nodes included");
                        if self.resolution_scale < 1.0 {
                            ui.weak(format!(
                                "Renders at {}×{}",
                                ((*w as f32 * self.resolution_scale).round() as usize).max(1),
                                ((*h as f32 * self.resolution_scale).round() as usize).max(1)
                            ));
                        }
                        ui.horizontal(|ui| {
                            ui.label("Camera");
                            ui.push_id("settings_camera", |ui| render_camera_combo(ui, &mut self.render_camera, &cameras));
                        });
                        ui.checkbox(&mut self.moonray_default_opts.stage, "Render the STAGE (USD) instead of the OBJ objects")
                            .on_hover_text("Uses the displayed LOP's stage: its meshes, materials, lights and (with the Auto camera) its first camera");
                    });

                    egui::CollapsingHeader::new("Sampling").default_open(true).show(ui, |ui| {
                        let opts = &mut self.moonray_default_opts;
                        let samples = &mut self.moonray_default_samples;
                        ui.horizontal(|ui| {
                            ui.label("Quality");
                            for (name, spp, depth, light, bsdf) in QUALITY_PRESETS {
                                let active = *samples == spp && opts.max_depth == depth && opts.light_samples == light && opts.bsdf_samples == bsdf;
                                if ui
                                    .selectable_label(active, name)
                                    .on_hover_text(format!("{spp} spp · depth {depth} · light {light} · BSDF {bsdf}"))
                                    .clicked()
                                {
                                    *samples = spp;
                                    (opts.max_depth, opts.light_samples, opts.bsdf_samples) = (depth, light, bsdf);
                                }
                            }
                        });
                        ui.add(egui::DragValue::new(samples).range(1..=4096).prefix("Samples per pixel "));
                        ui.add(egui::Slider::new(&mut opts.max_depth, 1..=32).text("Max ray depth"));
                        ui.add(egui::Slider::new(&mut opts.light_samples, 1..=16).text("Light samples"));
                        ui.add(egui::Slider::new(&mut opts.bsdf_samples, 1..=16).text("BSDF samples"));
                        ui.add(egui::Slider::new(&mut opts.volume_samples, 1..=8).text("Volume lighting samples"))
                            .on_hover_text("Light samples per step inside smoke and fire. Render time grows roughly in proportion: 1 for previews, 4+ for clean finals.");
                        ui.horizontal(|ui| {
                            ui.label("Environment light");
                            ui.color_edit_button_rgb(&mut opts.background);
                        });
                        ui.small("A non-black color adds a uniform environment light (not visible in camera).");
                    });

                    egui::CollapsingHeader::new("Output").default_open(true).show(ui, |ui| {
                        ui.radio_value(&mut self.render_to_temp, true, "Temporary folder (auto-named, open in the viewer)");
                        if self.render_to_temp {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label("Format");
                                ui.selectable_value(&mut self.render_temp_ext, "exr", "EXR (HDR)");
                                ui.selectable_value(&mut self.render_temp_ext, "png", "PNG");
                                if ui.button("Open folder").clicked() {
                                    if let Err(error) = reveal_in_file_browser(&std::env::temp_dir()) {
                                        self.status = error;
                                    }
                                }
                            });
                        }
                        ui.radio_value(&mut self.render_to_temp, false, "Save to file");
                        if !self.render_to_temp {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.add(egui::TextEdit::singleline(&mut self.moonray_default_output).desired_width(260.0));
                                if ui.button("Choose…").clicked() {
                                    if let Some(path) = FileDialog::new()
                                        .set_file_name("render.exr")
                                        .add_filter("Images", &["exr", "png", "jpg", "tif"])
                                        .save_file()
                                    {
                                        self.moonray_default_output = path.to_string_lossy().into_owned();
                                    }
                                }
                            });
                            if PathBuf::from(&self.moonray_default_output).is_relative() {
                                ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "Relative paths are written to the temp folder; choose a full path.");
                            }
                        }
                        ui.checkbox(&mut self.open_viewer_after_render, "Open in the Render Viewer when finished");
                        ui.checkbox(&mut self.bucket_render, "Progressive buckets")
                            .on_hover_text("Shows the image filling in tile by tile in the Render Viewer. Each tile restarts MoonRay, so turn this off for very heavy scenes.");
                    });

                    egui::CollapsingHeader::new("Frame range").default_open(false).show(ui, |ui| {
                        let opts = &mut self.moonray_default_opts;
                        ui.checkbox(&mut opts.sequence, "Render frame range");
                        if opts.sequence {
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut opts.range_start).speed(0.05).range(0.0..=100_000.0).prefix("Start "));
                                ui.add(egui::DragValue::new(&mut opts.range_end).speed(0.05).range(0.0..=100_000.0).prefix("End "));
                                ui.add(egui::DragValue::new(&mut opts.range_step).speed(0.01).range(0.0001..=1000.0).prefix("Step "));
                            });
                            if ui.button("Use timeline range").clicked() {
                                opts.range_start = self.playback_start.max(0.0);
                                opts.range_end = self.playback_end.max(opts.range_start);
                            }
                            let frames = frame_times(opts.range_start, opts.range_end, opts.range_step).len();
                            ui.small(format!("{frames} frames, numbered render.0001, render.0002, … (#### in a file name sets the position)."));
                        }
                    });

                    egui::CollapsingHeader::new("Advanced").default_open(false).show(ui, |ui| {
                        ui.label("Arguments ({scene} {output} {width} {height} {samples})");
                        ui.text_edit_singleline(&mut self.moonray_default_arguments);
                        ui.label("Extra MoonRay arguments");
                        ui.add(egui::TextEdit::singleline(&mut self.render_extra_args).hint_text("e.g. -threads 8"));
                        if ui.button("Reset all settings to defaults").clicked() {
                            let defaults = Self::default();
                            self.moonray_default_opts = RenderOpts::default();
                            self.moonray_default_arguments = defaults.moonray_default_arguments;
                            self.moonray_default_width = defaults.moonray_default_width;
                            self.moonray_default_height = defaults.moonray_default_height;
                            self.moonray_default_samples = defaults.moonray_default_samples;
                            self.render_to_temp = defaults.render_to_temp;
                            self.render_temp_ext = defaults.render_temp_ext;
                            self.resolution_scale = defaults.resolution_scale;
                            self.open_viewer_after_render = defaults.open_viewer_after_render;
                            self.render_extra_args.clear();
                        }
                    });

                    egui::CollapsingHeader::new("Render log").default_open(false).show(ui, |ui| {
                        if !self.last_render_summary.is_empty() {
                            ui.label(&self.last_render_summary);
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!log.is_empty(), egui::Button::new("Copy log")).clicked() {
                                ui.ctx().copy_text(log.clone());
                            }
                            if let Some(path) = &self.render_log_path {
                                if ui.button("Reveal log file").clicked() {
                                    if let Err(error) = reveal_in_file_browser(path) {
                                        self.status = error;
                                    }
                                }
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("render_log").max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                            let text = if log.is_empty() { "No render yet." } else { log.as_str() };
                            ui.add(egui::Label::new(egui::RichText::new(text).monospace().small()).wrap());
                        });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if rendering {
                        ui.spinner();
                        if let Some(started) = self.render_started {
                            ui.label(format!("Rendering… {:.0}s", started.elapsed().as_secs_f32()));
                        }
                        cancel = ui.button("■ Cancel").clicked();
                    } else {
                        let label = if self.moonray_default_opts.sequence && !node_selected { "Render sequence" } else { "Render" };
                        start = ui.button(label).clicked();
                    }
                    if ui.button("🖼 Render Viewer").clicked() {
                        self.viewer.show();
                    }
                });
            });
        self.show_moonray_settings = open;
        if start {
            self.start_moonray_render();
        }
        if cancel {
            self.cancel_render();
        }
    }

    // Stops the running render (killing MoonRay) and any sequence it belongs to.
    fn cancel_render(&mut self) {
        if let Some(cancel) = &self.render_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.sequence = None;
        self.render_at = None;
        self.status = "Cancelling render...".into();
    }

    fn spawn_moonray_render(&mut self, executable: String, arguments: String, output: String, width: usize, height: usize, samples: usize) {
        // Lights and cameras are read from the nodes: evaluate them at the frame being rendered.
        let render_time = self.render_at.unwrap_or(self.time);
        self.graph.apply_animation(render_time);
        // Those values are not user edits for Auto Key.
        self.anim_snapshot = None;
        if executable.trim().is_empty() || output.trim().is_empty() {
            self.status = "MoonRay executable and output image are required".into();
            return;
        }
        let scale = self.resolution_scale.clamp(0.05, 1.0);
        let width = ((width as f32 * scale).round() as usize).max(1);
        let height = ((height as f32 * scale).round() as usize).max(1);

        // Preserve the requested output path and export the scene to a writable
        // temporary file to avoid failures when the configured output path is
        // on a read-only filesystem (for example when running from inside a
        // .app bundle Resources directory).
        let output_path = PathBuf::from(&output);
        
        // Export the scene to temp
        // (use a unique filename to avoid clobbering concurrent renders)
        // Export the scene to a writable temporary file to avoid failures when
        // the configured output path is on a read-only filesystem (for example
        // when running from inside a .app bundle Resources directory).
        let tmp = std::env::temp_dir();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let scene_filename = format!("proceduralfx_scene_{}.obj", ts);
        let scene_path = tmp.join(scene_filename);
        let scene = match self.render_scene() {
            Ok(scene) => scene,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let scene_parts = &scene.parts;
        let scene_mesh = scene_parts
            .iter()
            .fold(Mesh::default(), |merged, (mesh, _)| node_merge(merged, mesh.clone()));
        if let Err(error) = scene_mesh.export_obj(scene_path.to_string_lossy().as_ref()) {
            self.status = format!("MoonRay scene export failed: {error}");
            return;
        }

        // Also attempt to generate a minimal RDLA wrapper that embeds the
        // exported mesh as vertex/index arrays. MoonRay accepts .rdla files
        // via `-in`, so prefer that when available. This is a best-effort
        // ASCII RDLA writer — the format may need adjustments depending on
        // the MoonRay build; we iterate as needed.
        let rdla_filename = format!("proceduralfx_scene_{}.rdla", ts);
        let rdla_path = tmp.join(rdla_filename);
        let rdla_result = self.export_scene_rdla(&rdla_path, &scene, width, height, samples);
        // If RDLA export succeeded, use it as the scene input; otherwise
        // fall back to the OBJ we wrote above.
        let scene_path = if rdla_result.is_ok() { rdla_path } else { scene_path };

        let replace = |text: &str| {
            text.replace("{scene}", &scene_path.to_string_lossy())
                .replace("{width}", &width.to_string())
                .replace("{height}", &height.to_string())
                .replace("{samples}", &samples.to_string())
        };
        // Resolve executable: if the configured executable doesn't exist or
        // is relative, try to find a bundled MoonRay inside the app bundle
        // (Contents/Resources/moonray or Contents/MacOS/moonray).
        let mut executable_to_run = executable.clone();
        let mut exe_path = PathBuf::from(&executable_to_run);
        if !exe_path.is_absolute() || !exe_path.exists() {
            if let Ok(cur) = std::env::current_exe() {
                if let Some(bundle_dir) = cur.parent().and_then(|p| p.parent()) {
                    let candidates = [
                        bundle_dir.join("Resources").join("moonray"),
                        bundle_dir.join("MacOS").join("moonray"),
                        bundle_dir.join("bin").join("moonray"),
                    ];
                    for c in &candidates {
                        if c.exists() {
                            exe_path = c.clone();
                            break;
                        }
                    }
                }
            }
        }

        // Ensure execute permission on Unix-like systems if the file exists.
        if exe_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&exe_path) {
                    let mut perms = meta.permissions();
                    let mode = perms.mode();
                    if mode & 0o111 == 0 {
                        perms.set_mode(mode | 0o111);
                        let _ = std::fs::set_permissions(&exe_path, perms);
                    }
                }
            }
            executable_to_run = exe_path.to_string_lossy().into_owned();
        }

        // Decide where MoonRay should write its output. If the requested
        // output path is not writable (for example inside the app bundle),
        // write to a temp file instead and copy the result back after render.
        let orig_output_pb = PathBuf::from(&output);
        let mut used_output_pb = orig_output_pb.clone();
        let mut copy_back_target = orig_output_pb.clone();
        let tmp = std::env::temp_dir();
        // Relative output paths are ambiguous in packaged apps (Finder launch
        // may set an unwritable working directory). Route them to temp.
        if orig_output_pb.is_relative() {
            let ext = orig_output_pb.extension().and_then(|s| s.to_str()).unwrap_or("png");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            used_output_pb = tmp.join(format!("proceduralfx_output_{}.{}", ts, ext));
            if self.render_at.is_some() {
                // Sequence frames keep their numbered names in one folder.
                let folder = tmp.join("proceduralfx_sequence");
                let _ = std::fs::create_dir_all(&folder);
                if let Some(name) = orig_output_pb.file_name() {
                    used_output_pb = folder.join(name);
                }
            }
            // No copy-back for relative paths; report the temp output path.
            copy_back_target = used_output_pb.clone();
        }
        // try to ensure parent exists and try creating the file to test writability
        let output_writable = (|| {
            if let Some(parent) = used_output_pb.parent() {
                // For relative paths with no explicit parent (e.g.
                // "procedural_output.png"), parent is empty; do not assume
                // writability — we still probe by attempting to create the
                // file below.
                if !parent.as_os_str().is_empty() {
                    if let Err(_) = std::fs::create_dir_all(parent) {
                        return false;
                    }
                }
            }
            match std::fs::OpenOptions::new().write(true).create(true).open(&used_output_pb) {
                Ok(mut f) => {
                    let _ = f.flush();
                    let _ = std::fs::remove_file(&used_output_pb);
                    true
                }
                Err(_) => false,
            }
        })();
        if !output_writable {
            let ext = orig_output_pb.extension().and_then(|s| s.to_str()).unwrap_or("png");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            used_output_pb = tmp.join(format!("proceduralfx_output_{}.{}", ts, ext));
            if copy_back_target.is_relative() {
                copy_back_target = used_output_pb.clone();
            }
        }

        // Now perform argument replacement with the (possibly) adjusted output path
        let arguments = replace(&arguments).replace("{output}", &used_output_pb.to_string_lossy());
        let output_path = used_output_pb.to_string_lossy().into_owned();
        let (tx, rx) = mpsc::channel();
        self.moonray_rx = Some(rx);
        let cancel = Arc::new(AtomicBool::new(false));
        self.render_cancel = Some(cancel.clone());
        let log_path = tmp.join(format!("proceduralfx_render_{ts}.log"));
        self.render_log_path = Some(log_path.clone());
        self.render_started = Some(Instant::now());
        let extra_args = self.render_extra_args.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        self.status = format!("Rendering with MoonRay: {executable_to_run}");
        // The Render Viewer opens on every render; bucket mode streams the image into it.
        let progress = self.bucket_render.then(|| Arc::new(std::sync::Mutex::new(render_viewer::RenderProgress::new(width, height))));
        match &progress {
            Some(progress) => self.viewer.watch(progress.clone()),
            None => self.viewer.show(),
        }
        let exec_run = executable_to_run.clone();
        let copy_back_target_clone = copy_back_target.clone();
        let requested_output = output.clone();
        std::thread::spawn(move || {
                let raw_tokens = arguments.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
                // Map common templates/flags to MoonRay CLI (-in, -out, -size).
                let mut args = Vec::new();
                let mut i = 0usize;
                while i < raw_tokens.len() {
                    let t = &raw_tokens[i];
                    if t.starts_with("--scene=") {
                        args.push("-in".to_string());
                        args.push(t[8..].to_string());
                    } else if t == "--scene" {
                        args.push("-in".to_string());
                        if i + 1 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); }
                    } else if t.starts_with("--output=") {
                        args.push("-out".to_string());
                        args.push(t[9..].to_string());
                    } else if t == "--output" {
                        args.push("-out".to_string());
                        if i + 1 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); }
                    } else if t.starts_with("--size=") {
                        // --size=WxH or --size W H; try to split by x or comma
                        let val = t[7..].to_string();
                        let mut parts: Vec<&str> = val.split(|c| c == 'x' || c == 'X' || c == ',').collect();
                        if parts.len() == 1 && i + 2 < raw_tokens.len() {
                            parts = vec![&raw_tokens[i+1], &raw_tokens[i+2]];
                            i += 2;
                        }
                        args.push("-size".to_string());
                        for p in parts { args.push(p.to_string()); }
                    } else if t == "--size" {
                        args.push("-size".to_string());
                        if i + 2 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); i += 1; args.push(raw_tokens[i].clone()); }
                    } else {
                        args.push(t.clone());
                    }
                    i += 1;
                }

                // If the user-provided template produced bare positional
                // tokens (e.g. "{scene} {output} {width} {height} {samples}"),
                // insert MoonRay flags where missing so MoonRay sees `-in` and
                // `-out`. Also ensure a `-size` is present.
                if !args.iter().any(|a| a == "-in") {
                    for idx in 0..args.len() {
                        let t = args[idx].to_lowercase();
                        if t.ends_with(".rdla") || t.ends_with(".rdlb") || t.ends_with(".obj") || t.ends_with(".ply") || t.ends_with(".usd") {
                            args.insert(idx, "-in".to_string());
                            break;
                        }
                    }
                }
                if !args.iter().any(|a| a == "-out") {
                    for idx in 0..args.len() {
                        let t = args[idx].to_lowercase();
                        if t.ends_with(".png") || t.ends_with(".exr") || t.ends_with(".jpg") || t.ends_with(".jpeg") || t.ends_with(".tif") || t.ends_with(".tiff") {
                            args.insert(idx, "-out".to_string());
                            break;
                        }
                    }
                }
                if !args.iter().any(|a| a == "-size") {
                    args.push("-size".to_string());
                    args.push(width.to_string());
                    args.push(height.to_string());
                }

                // Final output-path normalization: regardless of what the
                // user template provided, make MoonRay write to our vetted
                // writable path.
                let mut out_flag_found = false;
                let mut idx = 0usize;
                while idx < args.len() {
                    if args[idx] == "-out" {
                        out_flag_found = true;
                        if idx + 1 < args.len() {
                            args[idx + 1] = output_path.clone();
                        } else {
                            args.push(output_path.clone());
                        }
                        idx += 2;
                        continue;
                    }
                    if args[idx] == requested_output {
                        args[idx] = output_path.clone();
                    }
                    idx += 1;
                }
                if !out_flag_found {
                    args.push("-out".to_string());
                    args.push(output_path.clone());
                }

                args.extend(extra_args);
                let mut dso_paths: Option<String> = None;
                // If running from inside an app bundle, help MoonRay find its
                // RDL DSOs by setting RDL2_DSO_PATH to the bundle Resources
                // directory (where packaging should copy DSOs). Also include
                // the executable parent as a fallback.
                if let Ok(cur_exe) = std::env::current_exe() {
                    if let Some(bundle_dir) = cur_exe.parent().and_then(|p| p.parent()) {
                        let mut candidates = Vec::new();
                        candidates.push(bundle_dir.join("Resources").join("rdl2dso"));
                        candidates.push(bundle_dir.join("Resources"));
                        candidates.push(bundle_dir.join("MacOS"));
                        candidates.push(bundle_dir.join("rdl2dso"));
                        if let Ok(existing) = std::env::var("RDL2_DSO_PATH") {
                            for p in existing.split(':') {
                                if !p.is_empty() {
                                    candidates.push(std::path::PathBuf::from(p));
                                }
                            }
                        }
                        // join as colon-separated paths
                        let paths = candidates.iter()
                            .filter(|p| p.exists())
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(":");
                        if !paths.is_empty() {
                            dso_paths = Some(paths);
                        }
                    }
                }
                let make_cmd = |args: &[String]| {
                    let mut cmd = Command::new(&exec_run);
                    cmd.args(args);
                    if let Some(paths) = &dso_paths {
                        cmd.env("RDL2_DSO_PATH", paths);
                    }
                    cmd
                };
                let outcome: Result<(), String> = match &progress {
                    Some(progress) => render_buckets(&make_cmd, &args, &output_path, width, height, &log_path, &cancel, progress),
                    None => run_logged(&mut make_cmd(&args), &log_path, &cancel)
                        .map_err(|error| if cancel.load(Ordering::Relaxed) { error } else { format!("Could not start {exec_run}: {error}") })
                        .and_then(|status| {
                            if status.success() {
                                Ok(())
                            } else {
                                Err(format!("MoonRay exited with {status}: {}", read_log_tail(&log_path, 600).trim()))
                            }
                        }),
                };
                let result = outcome.and_then(|()| {
                    {
                        let rendered_pb = PathBuf::from(&output_path);
                        let _ = make_png_opaque(&rendered_pb);
                        let replaced_preview = ensure_png_not_all_black(&rendered_pb).unwrap_or(false);
                        // If we rendered to a temp file because the requested
                        // output path was not writable, copy it back to the
                        // original requested location.
                        if copy_back_target_clone != PathBuf::from(&output_path) {
                            let _ = std::fs::create_dir_all(copy_back_target_clone.parent().unwrap_or_else(|| std::path::Path::new(".")));
                            match std::fs::copy(&output_path, &copy_back_target_clone) {
                                Ok(_) => {
                                    let _ = make_png_opaque(&copy_back_target_clone);
                                    let _ = ensure_png_not_all_black(&copy_back_target_clone);
                                    if replaced_preview {
                                        return Ok((format!("MoonRay render complete: {} (copied to {}) [fallback preview inserted because output was fully black]", output_path, copy_back_target_clone.display()), copy_back_target_clone.clone()));
                                    }
                                    return Ok((format!("MoonRay render complete: {} (copied to {})", output_path, copy_back_target_clone.display()), copy_back_target_clone.clone()));
                                }
                                Err(e) => return Err(format!("Render complete but failed to copy output to {}: {}", copy_back_target_clone.display(), e)),
                            }
                        }
                        if replaced_preview {
                            return Ok((format!("MoonRay render complete: {output_path} [fallback preview inserted because output was fully black]"), rendered_pb));
                        }
                        Ok((format!("MoonRay render complete: {output_path}"), rendered_pb))
                    }
                });
            let log = read_log_tail(&log_path, 256 * 1024);
            let _ = tx.send(RenderOutcome { result, log });
        });
    }

    fn poll_moonray_render(&mut self) {
        let Some(rx) = self.moonray_rx.as_ref() else {
            return;
        };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Disconnected) => RenderOutcome {
                result: Err("MoonRay render worker disconnected".to_string()),
                log: String::new(),
            },
            Err(mpsc::TryRecvError::Empty) => return,
        };
        self.moonray_rx = None;
        self.render_cancel = None;
        let seconds = self.render_started.take().map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        self.last_render_log = outcome.log;
        let outcome = outcome.result.map(|(message, image)| {
            self.viewer.add(image, self.open_viewer_after_render);
            format!("{message} in {seconds:.1}s")
        });
        self.last_render_summary = match &outcome {
            Ok(message) => message.clone(),
            Err(error) => format!("Failed after {seconds:.1}s: {error}"),
        };
        if self.sequence.is_none() {
            self.status = outcome.unwrap_or_else(|error| error);
            return;
        }
        // Within a sequence only failures surface; advance_sequence reports progress.
        if let Err(error) = outcome {
            self.sequence = None;
            self.render_at = None;
            self.status = format!("Sequence stopped: {error}");
        }
    }

    fn begin_sequence(&mut self, job: SequenceJob) {
        if job.times.is_empty() {
            self.status = "Frame range is empty".into();
            return;
        }
        self.sequence = Some(job);
        self.advance_sequence();
    }

    // Starts the next frame of the running sequence once the previous one has finished.
    fn advance_sequence(&mut self) {
        if self.moonray_rx.is_some() {
            return;
        }
        let Some(job) = self.sequence.as_mut() else {
            return;
        };
        let total = job.times.len();
        if job.next >= total {
            let first = PathBuf::from(frame_output(&job.output, 1));
            let folder = if first.is_relative() {
                std::env::temp_dir().join("proceduralfx_sequence")
            } else {
                first.parent().map(PathBuf::from).unwrap_or_default()
            };
            self.status = format!("Sequence complete: {total} frames in {}", folder.display());
            self.sequence = None;
            self.render_at = None;
            return;
        }
        let index = job.next;
        job.next += 1;
        let time = job.times[index];
        let (executable, arguments, output) = (job.executable.clone(), job.arguments.clone(), frame_output(&job.output, index + 1));
        let (width, height, samples) = (job.width, job.height, job.samples);
        self.render_opts = job.opts.clone();
        self.render_at = Some(time);
        self.spawn_moonray_render(executable, arguments, output, width, height, samples);
        if self.moonray_rx.is_none() {
            // spawn_moonray_render rejected the settings and set the status.
            self.sequence = None;
            self.render_at = None;
            return;
        }
        self.status = format!("Rendering frame {}/{total} (t = {time:.3})...", index + 1);
    }

    fn export_scene_rdla(
        &self,
        path: &std::path::Path,
        scene: &RenderScene,
        width: usize,
        height: usize,
        samples: usize,
    ) -> Result<(), String> {
        use std::fs::File;
        use std::io::Write;

        let mut f = File::create(path).map_err(|e| format!("Failed to create RDLA file {}: {}", path.display(), e))?;
        // Best-effort RDLA writer: one RdlMeshGeometry per active geometry,
        // MoonRay materials from MAT nodes, lights, and a camera.
        writeln!(f, "-- RDLA generated by ProceduralFX Studio") .map_err(|e| e.to_string())?;
        writeln!(f, "") .map_err(|e| e.to_string())?;

        let opts = &self.render_opts;
        writeln!(f, "SceneVariables {{") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"image_width\"] = {},", width).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"image_height\"] = {},", height).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"output_file\"] = \"\",") .map_err(|e| e.to_string())?;
        // MoonRay's pixel_samples is per axis, so N samples per pixel is sqrt(N) per side.
        writeln!(f, "    [\"pixel_samples\"] = {},", (samples.max(1) as f32).sqrt().ceil() as usize) .map_err(|e| e.to_string())?;
        // Adaptive sampling stops early where pixels have converged (measured: same image,
        // fewer samples); the sample count is the maximum.
        writeln!(f, "    [\"sampling_mode\"] = 2,") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"min_adaptive_samples\"] = {},", (samples / 4).max(4).min(samples.max(1))) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"max_adaptive_samples\"] = {},", samples.max(1)) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"target_adaptive_error\"] = 1.0,") .map_err(|e| e.to_string())?;
        if !scene.volumes.is_empty() {
            // Finer ray marching and one bounce inside volumes keep smoke from rendering too dark.
            writeln!(f, "    [\"volume_quality\"] = 1.0,") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"volume_indirect_samples\"] = 1,") .map_err(|e| e.to_string())?;
            // In-scattering samples dominate volume render time (each marches shadow rays).
            writeln!(f, "    [\"volume_illumination_samples\"] = {},", opts.volume_samples.max(1)) .map_err(|e| e.to_string())?;
        }
        writeln!(f, "    [\"max_depth\"] = {},", opts.max_depth) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"light_samples\"] = {},", opts.light_samples) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"bsdf_samples\"] = {},", opts.bsdf_samples) .map_err(|e| e.to_string())?;
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        let parts = &scene.parts;
        // The viewport is left-handed and MoonRay right-handed, so z is mirrored
        // here and in the camera, and triangle winding is swapped to match.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            if mesh.indices.is_empty() {
                continue;
            }
            writeln!(f, "RdlMeshGeometry(\"/scene/objects/mesh{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"vertex_list_0\"] = {{") .map_err(|e| e.to_string())?;
            for vertex in &mesh.vertices {
                writeln!(f, "        Vec3({:.9}, {:.9}, {:.9}),", vertex.x, vertex.y, -vertex.z).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"vertices_by_index\"] = {{") .map_err(|e| e.to_string())?;
            for tri in &mesh.indices {
                writeln!(f, "        {},", tri[0]).map_err(|e| e.to_string())?;
                writeln!(f, "        {},", tri[2]).map_err(|e| e.to_string())?;
                writeln!(f, "        {},", tri[1]).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"face_vertex_count\"] = {{") .map_err(|e| e.to_string())?;
            for _ in &mesh.indices {
                writeln!(f, "        3,") .map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_list\"] = {{ \"tri_shape_0\" }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_face_count_list\"] = {{ {} }},", mesh.indices.len()) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_face_indices\"] = {{") .map_err(|e| e.to_string())?;
            for i in 0..mesh.indices.len() {
                writeln!(f, "        {},", i).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            // UVs per face-vertex (in the swapped winding) for image textures.
            if let Some(uvs) = sops::corner_uvs(mesh) {
                writeln!(f, "    [\"uv_list\"] = {{") .map_err(|e| e.to_string())?;
                for uv in &uvs {
                    for k in [0, 2, 1] {
                        writeln!(f, "        Vec2({:.6}, {:.6}),", uv[k][0], uv[k][1]).map_err(|e| e.to_string())?;
                    }
                }
                writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            }
            // Cd as user data, for shaders that read point colour (rate guessed from the count).
            let cd = mesh
                .point_attributes
                .get("Cd")
                .filter(|c| c.len() == mesh.vertices.len() * 3)
                .or_else(|| mesh.point_attributes.get("prim:Cd").filter(|c| c.len() == mesh.indices.len() * 3));
            if cd.is_some() {
                writeln!(f, "    [\"primitive_attributes\"] = {{ UserData(\"/scene/objects/mesh{index}_Cd\") }},") .map_err(|e| e.to_string())?;
            }
            writeln!(f, "    [\"is_subd\"] = false") .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            if let Some(cd) = cd {
                let mut body = format!("UserData(\"/scene/objects/mesh{index}_Cd\") {{\n    [\"color_key\"] = \"Cd\",\n    [\"color_values_0\"] = {{\n");
                for c in cd.chunks(3) {
                    body.push_str(&format!("        Rgb({:.4}, {:.4}, {:.4}),\n", c[0], c[1], c[2]));
                }
                body.push_str("    }\n}\n\n");
                f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            }
        }

        // Hair and other curves: linear RdlCurveGeometry, radius from the width attribute.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            let curves = groom::curves(mesh);
            if curves.is_empty() {
                continue;
            }
            let width = mesh.point_attributes.get("width").filter(|w| w.len() == mesh.vertices.len());
            let mut body = format!("RdlCurveGeometry(\"/scene/objects/curves{index}\") {{\n    [\"curve_type\"] = 0,\n    [\"vertex_list_0\"] = {{\n");
            for range in &curves {
                for v in &mesh.vertices[range.clone()] {
                    body.push_str(&format!("        Vec3({:.7}, {:.7}, {:.7}),\n", v.x, v.y, -v.z));
                }
            }
            body.push_str("    },\n    [\"curves_vertex_count\"] = {\n");
            for range in &curves {
                body.push_str(&format!("        {},\n", range.len()));
            }
            body.push_str("    },\n    [\"radius_list\"] = {\n");
            for range in &curves {
                for i in range.clone() {
                    body.push_str(&format!("        {:.6},\n", width.map_or(0.005, |w| w[i] * 0.5).max(1e-5)));
                }
            }
            body.push_str("    }\n}\n\n");
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }

        // Whitewater and other marked points: spheres of radius pscale.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            if !renders_points(mesh) {
                continue;
            }
            let pscale = mesh.point_attributes.get("pscale").filter(|p| p.len() == mesh.vertices.len());
            let mut body = format!("RdlPointGeometry(\"/scene/objects/points{index}\") {{\n    [\"vertex_list_0\"] = {{\n");
            for v in &mesh.vertices {
                body.push_str(&format!("        Vec3({:.6}, {:.6}, {:.6}),\n", v.x, v.y, -v.z));
            }
            body.push_str("    },\n    [\"radius_list\"] = {\n");
            for i in 0..mesh.vertices.len() {
                body.push_str(&format!("        {:.6},\n", pscale.map_or(0.01, |p| p[i]).max(1e-5)));
            }
            body.push_str("    }\n}\n\n");
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }

        // Fog volumes: VdbGeometry reading the written .vdb, shaded by VdbVolume.
        for (index, vol) in scene.volumes.iter().enumerate() {
            writeln!(f, "VdbGeometry(\"/scene/volumes/volume{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"model\"] = \"{}\",", vol.file.replace('\\', "/")) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"density_grid\"] = \"density\",") .map_err(|e| e.to_string())?;
            if vol.emission.is_some() {
                writeln!(f, "    [\"emission_grid\"] = \"emission\",") .map_err(|e| e.to_string())?;
                // Sample the fire densely so flames render smooth instead of speckled.
                writeln!(f, "    [\"emission_sample_rate\"] = 1.0,") .map_err(|e| e.to_string())?;
            }
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            let emission = vol.emission.unwrap_or(0.0);
            writeln!(f, "VdbVolume(\"/scene/volumes/shader{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"opacity_gain_mult\"] = Rgb({0:.5}, {0:.5}, {0:.5}),", vol.extinction) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color_mult\"] = Rgb({:.4}, {:.4}, {:.4}),", vol.albedo[0], vol.albedo[1], vol.albedo[2]) .map_err(|e| e.to_string())?;
            let tint = vol.emission_tint.map(|t| t * emission);
            writeln!(f, "    [\"incandescence_gain_mult\"] = Rgb({:.4}, {:.4}, {:.4}),", tint[0], tint[1], tint[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"anisotropy\"] = {:.4}", vol.anisotropy) .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
        }

        // Each part renders as a mesh, curves, or both.
        let part_geometry: Vec<Vec<String>> = parts
            .iter()
            .enumerate()
            .map(|(index, (mesh, _))| {
                let mut refs = Vec::new();
                if !mesh.indices.is_empty() {
                    refs.push(format!("RdlMeshGeometry(\"/scene/objects/mesh{index}\")"));
                }
                if groom::has_curves(mesh) && !groom::curves(mesh).is_empty() {
                    refs.push(format!("RdlCurveGeometry(\"/scene/objects/curves{index}\")"));
                }
                if renders_points(mesh) {
                    refs.push(format!("RdlPointGeometry(\"/scene/objects/points{index}\")"));
                }
                refs
            })
            .collect();
        writeln!(f, "GeometrySet(\"/scene/geometrysets/GeometrySet1\") {{") .map_err(|e| e.to_string())?;
        for geometry in part_geometry.iter().flatten() {
            writeln!(f, "    {geometry},") .map_err(|e| e.to_string())?;
        }
        for index in 0..scene.volumes.len() {
            writeln!(f, "    VdbGeometry(\"/scene/volumes/volume{index}\"),") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        // Lights come from active Light nodes (or the default key/fill rig).
        // z is mirrored like the mesh; MoonRay lights emit along local -Z.
        let mat4 = |x: Vec3, y: Vec3, z: Vec3, p: Vec3| {
            let m = format!(
                "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},1)",
                x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z, p.x, p.y, p.z
            );
            format!("blur({m}, {m})")
        };
        let mut light_names: Vec<String> = Vec::new();
        for (index, (position, direction, color, intensity, kind)) in scene.lights.iter().enumerate() {
            let name = format!("/scene/lights/light{index}");
            let rgb = format!("Rgb({:.4}, {:.4}, {:.4})", color[0], color[1], color[2]);
            if *kind == 1 {
                let xform = mat4(
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(0.0, 0.0, 1.0),
                    Vec3::new(position.x, position.y, -position.z),
                );
                writeln!(f, "SphereLight(\"{name}\") {{") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"node_xform\"] = {xform},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"color\"] = {rgb},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"intensity\"] = {:.4},", intensity * POINT_LIGHT_SCALE) .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"radius\"] = 0.1,") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"visible_in_camera\"] = \"force off\",") .map_err(|e| e.to_string())?;
                writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            } else {
                let z = Vec3::new(-direction.x, -direction.y, direction.z).normalize();
                let hint = if z.y.abs() > 0.99 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
                let x = hint.cross(z).normalize();
                let y = z.cross(x);
                let xform = mat4(x, y, z, Vec3::zero());
                writeln!(f, "DistantLight(\"{name}\") {{") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"node_xform\"] = {xform},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"color\"] = {rgb},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"intensity\"] = {:.4},", intensity * DISTANT_LIGHT_SCALE) .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"angular_extent\"] = {},", if *kind == 2 { 10.0 } else { 0.53 }) .map_err(|e| e.to_string())?;
                writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            }
            light_names.push(format!("{}(\"{name}\")", if *kind == 1 { "SphereLight" } else { "DistantLight" }));
        }

        // One MoonRay material per MAT node used by a geometry, plus a neutral default.
        let (default_class, default_body) = rdla_material("/scene/materials/default", 0, [0.8; 3], 0.5, 0.5);
        write!(f, "{default_body}\n") .map_err(|e| e.to_string())?;
        let mut written: Vec<NodeId> = Vec::new();
        let mut layer_materials: Vec<String> = Vec::new();
        for (index, (_, material)) in parts.iter().enumerate() {
            let node = match material {
                PartMaterial::Node(id) => self.graph.nodes.get(id).map(|n| (*id, &n.kind)),
                _ => None,
            };
            let reference = match (material, node) {
                (_, Some((id, NodeKind::Shader(v)))) if v.table != shading::Table::Pyro => {
                    let name = format!("/scene/materials/mat{id}");
                    let mut objects = String::new();
                    let (class, body) = match v.table {
                        shading::Table::Toon => shading::toon_rdla(&name, v),
                        shading::Table::Hair => shading::hair_rdla(&name, v),
                        _ => shading::principled_rdla(&name, v, &HashMap::new(), &mut objects),
                    };
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{objects}{body}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (_, Some((id, NodeKind::MaterialBuilder))) => {
                    let name = format!("/scene/materials/mat{id}");
                    let output = self.graph.builder_output(id).ok_or("Material Builder: add a Surface Output node inside it")?;
                    let nodes = self.graph.builder_nodes(id);
                    let (class, text) = shading::compile_builder(&name, &nodes, |to, slot| self.graph.input_of(to, slot), output)
                        .map_err(|e| format!("Material Builder {}: {e}", self.graph.nodes[&id].kind.label(id)))?;
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{text}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (_, Some((id, NodeKind::Material { kind, color, roughness, value }))) => {
                    let name = format!("/scene/materials/mat{id}");
                    let (class, body) = rdla_material(&name, *kind, *color, *roughness, *value);
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{body}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (PartMaterial::Preview { diffuse, roughness, metallic, opacity, emissive, ior }, _) => {
                    // A USD preview surface mapped onto the closest MAT material type.
                    let name = format!("/scene/materials/usd{index}");
                    let emission = emissive.iter().cloned().fold(0.0, f32::max);
                    let (kind, color, value) = if emission > 0.0 {
                        (3, emissive.map(|c| c / emission), emission)
                    } else if *metallic > 0.5 {
                        (1, *diffuse, 0.5)
                    } else if *opacity < 0.5 {
                        (2, *diffuse, *ior)
                    } else {
                        (0, *diffuse, 0.5)
                    };
                    let (class, body) = rdla_material(&name, kind, color, *roughness, value);
                    write!(f, "{body}\n") .map_err(|e| e.to_string())?;
                    format!("{class}(\"{name}\")")
                }
                _ => format!("{default_class}(\"/scene/materials/default\")"),
            };
            layer_materials.push(reference);
        }

        // An HDRI environment light replaces the uniform ambient environment below.
        let hdri = self.environment_light();
        if let Some((file, color, rotation, visible)) = &hdri {
            envlight::load(file).map_err(|e| format!("Environment light: {e}"))?;
            let r = envlight::rotation_rows(*rotation);
            let m = format!(
                "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, 0,0,0,1)",
                r[0].x, r[0].y, r[0].z, r[1].x, r[1].y, r[1].z, r[2].x, r[2].y, r[2].z
            );
            writeln!(f, "EnvLight(\"/scene/lights/hdri\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"texture\"] = \"{}\",", file.replace('\\', "/").replace('"', "\\\"")) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color\"] = Rgb({:.4}, {:.4}, {:.4}),", color[0], color[1], color[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"visible_in_camera\"] = {},", if *visible { 1 } else { 0 }) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"node_xform\"] = blur({m}, {m})") .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            light_names.push("EnvLight(\"/scene/lights/hdri\")".to_string());
        }
        // The viewport's ambient term becomes a dim environment light (decoded from sRGB).
        let ambient = self.ambient_light.max(0.0).powf(2.2);
        let environment: [f32; 3] = std::array::from_fn(|i| opts.background[i] + scene.environment.map_or(0.0, |e| e[i]) + ambient);
        if hdri.is_none() && environment.iter().any(|c| *c > 0.0) {
            writeln!(f, "EnvLight(\"/scene/lights/env\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color\"] = Rgb({:.4}, {:.4}, {:.4}),", environment[0], environment[1], environment[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            light_names.push("EnvLight(\"/scene/lights/env\")".to_string());
        }

        writeln!(f, "LightSet(\"/scene/lightsets/LightSet1\") {{") .map_err(|e| e.to_string())?;
        for light in &light_names {
            writeln!(f, "    {light},") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        writeln!(f, "Layer(\"/scene/layers/Layer1\") {{") .map_err(|e| e.to_string())?;
        for (geometry, material) in part_geometry.iter().zip(&layer_materials) {
            for geometry in geometry {
                writeln!(f, "    {{{geometry}, \"\", {material}, LightSet(\"/scene/lightsets/LightSet1\"), undef(), undef(), undef(), undef()}},") .map_err(|e| e.to_string())?;
            }
        }
        for index in 0..scene.volumes.len() {
            writeln!(f, "    {{VdbGeometry(\"/scene/volumes/volume{index}\"), \"\", LightSet(\"/scene/lightsets/LightSet1\"), VdbVolume(\"/scene/volumes/shader{index}\")}},") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        // Camera from the stage, the selected/chosen camera node, or the viewport.
        let (rig, _) = self.resolve_render_camera();
        let (sx, cx) = rig.rot_x.sin_cos();
        let (sy, cy) = rig.rot_y.sin_cos();
        let mut right = Vec3::new(cy, 0.0, -sy);
        let mut up = Vec3::new(-sx * sy, cx, -sx * cy);
        let mut fwd = Vec3::new(cx * sy, sx, cx * cy);
        let pan_scale = rig.zoom / self.viewport_focal_px.max(1.0);
        let target = rig
            .target
            .sub(right.scale(rig.pan[0] * pan_scale))
            .add(up.scale(rig.pan[1] * pan_scale));
        let mut eye = target.sub(fwd.scale(rig.zoom));
        let mut lens = None;
        if let Some((stage_eye, stage_right, stage_up, stage_fwd, focal, aperture)) = scene.camera {
            (eye, right, up, fwd) = (stage_eye, stage_right, stage_up, stage_fwd);
            lens = Some((focal, aperture));
        }
        // z is mirrored (see mesh export); MoonRay cameras look down local -Z.
        let xform = format!(
            "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},1)",
            right.x, right.y, -right.z,
            up.x, up.y, -up.z,
            -fwd.x, -fwd.y, fwd.z,
            eye.x, eye.y, -eye.z,
        );
        let min_dim = width.min(height).max(1) as f32;
        let aspect_w = width.max(1) as f32;
        let orthographic = rig.orthographic && lens.is_none();
        writeln!(f, "{}(\"/scene/cameras/Camera1\") {{", if orthographic { "OrthographicCamera" } else { "PerspectiveCamera" }).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"node_xform\"] = blur({xform}, {xform}),") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"near\"] = 0.001,") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"far\"] = 100000,") .map_err(|e| e.to_string())?;
        if let Some((focal, aperture)) = lens {
            writeln!(f, "    [\"film_width_aperture\"] = {aperture:.6},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"focal\"] = blur({focal:.6}, {focal:.6})") .map_err(|e| e.to_string())?;
        } else if rig.orthographic {
            writeln!(f, "    [\"film_width_aperture\"] = {:.6}", rig.zoom / rig.focal * aspect_w / min_dim) .map_err(|e| e.to_string())?;
        } else {
            // Match the viewport's field of view along the smaller image dimension.
            let focal_mm = 24.0 * min_dim * rig.focal / aspect_w;
            writeln!(f, "    [\"film_width_aperture\"] = 24,") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"focal\"] = blur({focal_mm:.6}, {focal_mm:.6})") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}") .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn set_camera_view(&mut self, rot_x: f32, rot_y: f32) {
        self.cam_rot_x = rot_x;
        self.cam_rot_y = rot_y;
        self.cam_pan_x = 0.0;
        self.cam_pan_y = 0.0;
        self.frame_all();
    }

    // Geometry node being edited, if it still exists.
    fn network(&self) -> Option<NodeId> {
        self.current_network.filter(|id| self.graph.nodes.get(id).is_some_and(|n| n.kind.is_container()))
    }

    // The network containing the open one (a SOP Solver's geometry), or None at the top.
    fn parent_network(&self) -> Option<NodeId> {
        self.network()
            .and_then(|id| self.graph.nodes.get(&id))
            .and_then(|n| n.parent)
            .filter(|p| self.graph.nodes.get(p).is_some_and(|n| n.kind.is_container()))
    }

    // Parent assigned to nodes created in the visible network.
    fn view_parent(&self) -> Option<NodeId> {
        self.network().filter(|id| self.graph.nodes.get(id).is_some_and(|n| n.context == self.context))
    }

    // Nodes cooked into the viewport: the open geometry's display, else every top-level geometry.
    fn view_displays(&self) -> Vec<NodeId> {
        if self.context == AppContext::Stage {
            return self.graph.display_of(STAGE_NETWORK).into_iter().collect();
        }
        match self.network() {
            Some(geo) => self.graph.display_of(geo).into_iter().collect(),
            None => self.graph.geometry_ids(),
        }
    }

    // Opens `network` (None: the top level). The editor takes the context its nodes
    // live in: OBJ for geometry networks, MAT for a Material Builder; going back to
    // the top stays in the context of the network being left.
    fn enter_network(&mut self, network: Option<NodeId>) {
        let context_of = |id: NodeId| self.graph.nodes.get(&id).map(|n| n.context);
        self.context = match network {
            Some(id) => context_of(id),
            None => self.network().and_then(context_of),
        }
        .unwrap_or(AppContext::Obj);
        self.current_network = network;
        self.selected = None;
        self.graph_pan = egui::Vec2::ZERO;
        self.clear_component_selection();
        self.dirty = true;
    }

    fn templates_for_view(&self) -> Vec<(&'static str, Vec<NodeKind>)> {
        let in_obj = self.context == AppContext::Obj;
        let top_obj = in_obj && self.network().is_none();
        let in_solver = self
            .network()
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::SopSolver { .. })));
        let in_vop = self
            .network()
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::AttributeVop { .. })));
        let in_builder = self
            .network()
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::MaterialBuilder)));
        let context = self.context;
        NodeKind::templates()
            .into_iter()
            .filter_map(|(category, kinds)| {
                let kinds: Vec<NodeKind> = kinds
                    .into_iter()
                    .filter(|k| match context {
                        // Inside a Material Builder only shader nodes; shader nodes nowhere else.
                        AppContext::Mat if in_builder => matches!(k, NodeKind::ShaderNode(_)),
                        AppContext::Mat => k.is_material(),
                        AppContext::Out => k.is_render(),
                        AppContext::Stage => k.is_lop(),
                        AppContext::Obj if k.is_lop() => false,
                        // Inside an Attribute VOP only VOPs; VOPs nowhere else.
                        AppContext::Obj if in_vop => matches!(k, NodeKind::Vop(_)),
                        AppContext::Obj if matches!(k, NodeKind::Vop(_)) => false,
                        AppContext::Obj if top_obj => k.is_object(),
                        AppContext::Obj if matches!(k, NodeKind::SolverInput { .. }) => in_solver,
                        AppContext::Obj => !k.is_object() && !k.is_material() && !k.is_render(),
                    })
                    .collect();
                (!kinds.is_empty()).then_some((category, kinds))
            })
            .collect()
    }

    // (position, travel direction, color, intensity, kind) for every active Light node,
    // or the viewport's default key/fill rig when the scene has none.
    // The active (first, not bypassed) environment light with a file: (file, color * intensity, rotation, visible).
    fn environment_light(&self) -> Option<(String, [f32; 3], f32, bool)> {
        let mut ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter().find_map(|id| {
            let node = &self.graph.nodes[&id];
            match &node.kind {
                NodeKind::EnvironmentLight { file, color, intensity, rotation, visible } if !node.bypass && node.parent.is_none() && !file.trim().is_empty() => {
                    Some((file.trim().to_owned(), color.map(|c| c * intensity), *rotation, *visible))
                }
                _ => None,
            }
        })
    }

    fn scene_lights(&self) -> Vec<(Vec3, Vec3, [f32; 3], f32, usize)> {
        let mut ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        ids.sort_unstable();
        let lights: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                let node = &self.graph.nodes[&id];
                if node.bypass || node.parent.is_some() {
                    return None;
                }
                let NodeKind::Light { position, rotation, color, intensity, kind } = &node.kind else {
                    return None;
                };
                Some((
                    Vec3::from_arr(*position),
                    Vec3::new(0.0, 0.0, -1.0).rotate_deg(*rotation).normalize(),
                    *color,
                    *intensity,
                    *kind,
                ))
            })
            .collect();
        // Any light, including an HDRI, replaces the default key/fill rig.
        if !lights.is_empty() || self.environment_light().is_some() {
            return lights;
        }
        vec![
            (
                Vec3::zero(),
                Vec3::from_arr(self.key_light_dir).normalize(),
                [1.0; 3],
                self.key_light_intensity.max(0.0),
                0,
            ),
            (
                Vec3::zero(),
                Vec3::from_arr(self.fill_light_dir).normalize(),
                [1.0; 3],
                self.fill_light_intensity.max(0.0),
                2,
            ),
        ]
    }

    // Active geometries with their MAT assignment, regardless of which network the viewport shows.
    fn render_scene_parts(&self) -> Vec<(Mesh, Option<NodeId>)> {
        let material_of = |id: NodeId| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Geometry { material }) => *material,
            _ => None,
        };
        let active: Vec<NodeId> = self
            .graph
            .geometry_ids()
            .into_iter()
            .filter(|id| !self.graph.nodes[id].bypass)
            .collect();
        let time = self.render_at.unwrap_or(self.time);
        if self.render_at.is_none() && active.len() == 1 && self.network().is_none() && self.context != AppContext::Stage {
            return vec![(self.current_mesh.clone(), material_of(active[0]))];
        }
        let mut cache = self.static_cache.clone();
        active
            .into_iter()
            .map(|id| (self.graph.cook(id, time, &mut cache), material_of(id)))
            .filter(|(mesh, _)| !mesh.indices.is_empty() || volume::is_volume(mesh) || groom::has_curves(mesh) || renders_points(mesh))
            .collect()
    }

    // The scene to render: the displayed USD stage when the render options ask for it, else the OBJ objects.
    fn render_scene(&self) -> Result<RenderScene, String> {
        if !self.render_opts.stage {
            let mut parts = Vec::new();
            let mut volumes = Vec::new();
            for (index, (mesh, material)) in self.render_scene_parts().into_iter().enumerate() {
                let material = material.map_or(PartMaterial::Default, PartMaterial::Node);
                if !volume::is_volume(&mesh) {
                    parts.push((mesh, material));
                    continue;
                }
                let grid = volume::from_mesh(&mesh)?;
                if grid.class == volume::Class::LevelSet {
                    // Level sets render as their surface.
                    parts.push((volume::to_polygons(&grid, 0.0), material));
                } else {
                    let mut volume = write_render_volume(&mesh, index)?;
                    if let PartMaterial::Node(m) = material {
                        if let Some(NodeKind::Shader(v)) = self.graph.nodes.get(&m).map(|n| &n.kind).filter(|k| matches!(k, NodeKind::Shader(v) if v.table == shading::Table::Pyro)) {
                            volume.extinction *= v.float("smoke_density");
                            volume.albedo = v.color("smoke_color");
                            volume.emission = volume.emission.map(|e| e * v.float("fire_intensity"));
                            volume.emission_tint = v.color("fire_color");
                            volume.anisotropy = v.float("anisotropy");
                        }
                    }
                    volumes.push(volume);
                }
            }
            return Ok(RenderScene { parts, lights: self.scene_lights(), environment: None, camera: None, volumes });
        }
        let display = self.graph.display_of(STAGE_NETWORK).ok_or("Render STAGE: no LOP node has the display flag in the STAGE network")?;
        let time = self.render_at.unwrap_or(self.time);
        let stage = self.graph.cook_stage(display, time, &mut HashMap::new());
        let parts = stage
            .flatten()
            .into_iter()
            .filter(|(_, mesh, _)| !mesh.indices.is_empty())
            .map(|(_, mesh, material)| {
                let material = material.as_deref().and_then(|path| stage.material(path)).map_or(PartMaterial::Default, |m| {
                    match m.material_node.filter(|id| self.graph.nodes.contains_key(id)) {
                        Some(node) => PartMaterial::Node(node),
                        None => PartMaterial::Preview {
                            diffuse: m.vec3("diffuseColor").unwrap_or([0.18; 3]),
                            roughness: m.float("roughness").unwrap_or(0.5),
                            metallic: m.float("metallic").unwrap_or(0.0),
                            opacity: m.float("opacity").unwrap_or(1.0),
                            emissive: m.vec3("emissiveColor").unwrap_or([0.0; 3]),
                            ior: m.float("ior").unwrap_or(1.5),
                        },
                    }
                });
                (mesh, material)
            })
            .collect();
        let mut lights = Vec::new();
        let mut environment: Option<[f32; 3]> = None;
        let mut camera = None;
        for (path, prim) in &stage.prims {
            if !stage.is_shown(path) {
                continue;
            }
            let world = stage.world_xform(path);
            let colour = prim.vec3("color").unwrap_or([1.0; 3]);
            let intensity = prim.float("intensity").unwrap_or(1.0) * 2f32.powf(prim.float("exposure").unwrap_or(0.0));
            match prim.type_name.as_str() {
                "DomeLight" => {
                    let add = colour.map(|c| c * intensity);
                    let current = environment.unwrap_or([0.0; 3]);
                    environment = Some(std::array::from_fn(|i| current[i] + add[i]));
                }
                "DistantLight" => lights.push((usd::row(&world, 3), usd::row(&world, 2).normalize(), colour, intensity, 0)),
                kind if kind.ends_with("Light") => lights.push((usd::row(&world, 3), usd::row(&world, 2).normalize(), colour, intensity, 1)),
                "Camera" if camera.is_none() && self.render_opts.camera == RenderCamera::Auto => {
                    camera = Some((
                        usd::row(&world, 3),
                        usd::row(&world, 0).normalize(),
                        usd::row(&world, 1).normalize(),
                        usd::row(&world, 2).normalize(),
                        prim.float("focalLength").unwrap_or(50.0),
                        prim.float("horizontalAperture").unwrap_or(36.0),
                    ));
                }
                _ => {}
            }
        }
        if lights.is_empty() && environment.is_none() {
            lights = self.scene_lights();
        }
        Ok(RenderScene { parts, lights, environment, camera, volumes: Vec::new() })
    }

    fn rebuild_graph(&mut self) {
        // While picking a group the viewport keeps showing the node's input.
        if self.group_pick.is_some() {
            return;
        }
        self.cook_generation += 1;
        let t0 = Instant::now();
        // Static nodes stay cached until the network changes; only
        // time-dependent branches recook during playback.
        let key = self.graph.serialize();
        if key != self.cache_key {
            if let Some(cancel) = &self.simulation_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.simulation_rx = None;
            self.simulation_cancel = None;
            self.simulation_key.clear();
            self.cancelled_simulation_key.clear();
            self.static_cache.clear();
            self.frame_cache.clear();
            self.frame_cache_order.clear();
            self.frame_cache_hits = 0;
            self.frame_cache_misses = 0;
            self.cache_key = key;
        }
        let mut cache = self.static_cache.clone();
        let displays = self.view_displays();
        let frame_key = format!(
            "{}|display={:?}|time={:08x}",
            self.cache_key,
            displays,
            self.time.to_bits()
        );
        if self.simulation_rx.is_some() && self.simulation_key != frame_key {
            if let Some(cancel) = &self.simulation_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.simulation_rx = None;
            self.simulation_cancel = None;
            self.simulation_key.clear();
        }
        if let Some(mesh) = self.frame_cache.get(&frame_key) {
            self.current_mesh = mesh.clone();
            self.frame_cache_hits += 1;
            self.status = format!("Frame from cache · {} cached frames", self.frame_cache.len());
        } else if displays.iter().any(|id| self.graph.time_dependent(*id)) {
            self.frame_cache_misses += 1;
            if self.simulation_key != frame_key
                && self.cancelled_simulation_key != frame_key
            {
                if let Some(cancel) = &self.simulation_cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                let graph = self.graph.clone();
                let worker_displays = displays.clone();
                let time = self.time;
                let (tx, rx) = mpsc::channel();
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = cancel.clone();
                std::thread::spawn(move || {
                    FLIP_CANCEL.with(|flag| *flag.borrow_mut() = Some(worker_cancel));
                    let mut worker_cache = HashMap::new();
                    let mesh = graph.cook_display(&worker_displays, time, &mut worker_cache);
                    let _ = tx.send(mesh);
                });
                self.simulation_rx = Some(rx);
                self.simulation_cancel = Some(cancel);
                self.simulation_key = frame_key;
                self.cancelled_simulation_key.clear();
                self.status = format!("{}… press Escape to cancel", self.graph.cooking_label(&displays));
            } else if let Some(rx) = &self.simulation_rx {
                match rx.try_recv() {
                    Ok(mesh) => {
                        self.current_mesh = mesh.clone();
                        const MAX_FRAME_CACHE: usize = 120;
                        if self.frame_cache.len() >= MAX_FRAME_CACHE {
                            if let Some(oldest) = self.frame_cache_order.pop_front() {
                                self.frame_cache.remove(&oldest);
                            }
                        }
                        self.frame_cache.insert(frame_key.clone(), mesh);
                        self.frame_cache_order.push_back(frame_key);
                        self.simulation_rx = None;
                        self.simulation_cancel = None;
                        self.simulation_key.clear();
                        self.status.clear();
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.simulation_rx = None;
                        self.simulation_cancel = None;
                        self.simulation_key.clear();
                        self.status = "The frame failed to cook".into();
                    }
                }
            }
        } else {
            self.current_mesh = self.graph.cook_display(&displays, self.time, &mut cache);
            const MAX_FRAME_CACHE: usize = 120;
            if self.frame_cache.len() >= MAX_FRAME_CACHE {
                if let Some(oldest) = self.frame_cache_order.pop_front() {
                    self.frame_cache.remove(&oldest);
                }
            }
            self.frame_cache.insert(frame_key.clone(), self.current_mesh.clone());
            self.frame_cache_order.push_back(frame_key);
        }
        self.template_mesh = match self.template {
            Some(id) if self.graph.nodes.contains_key(&id) => self.graph.cook(id, self.time, &mut cache),
            _ => Mesh::default(),
        };
        self.node_stats = cache
            .iter()
            .map(|(id, m)| (*id, (m.vertices.len(), m.indices.len())))
            .collect();
        for (id, m) in cache {
            if !self.graph.time_dependent(id) {
                self.static_cache.entry(id).or_insert(m);
            }
        }
        self.cook_ms = t0.elapsed().as_secs_f32() * 1000.0;
    }

    fn frame_all(&mut self) {
        if let Some((lo, hi)) = self.current_mesh.bounds() {
            self.cam_target = lo.add(hi).scale(0.5);
            self.cam_zoom = (hi.sub(lo).length() * 1.4).clamp(0.5, 200.0);
            self.cam_pan_x = 0.0;
            self.cam_pan_y = 0.0;
        }
    }

    fn load_state(&mut self, s: String) {
        self.graph = Graph::deserialize(&s);
        self.last_state = s;
        if self.selected.map_or(false, |id| !self.graph.nodes.contains_key(&id)) {
            self.selected = None;
        }
        self.dragging_link = None;
        self.dirty = true;
    }

    fn do_undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.last_state.clone());
            self.load_state(prev);
        }
    }

    fn do_redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.last_state.clone());
            self.load_state(next);
        }
    }

    fn create_blast_from_selection(&mut self) -> bool {
        let Some(source) = self.network().and_then(|geo| self.graph.display_of(geo)) else {
            return false;
        };
        let (component, mut indices) = match self.selection_mode {
            SelectionMode::Vertex => (0, self.selected_vertices.clone()),
            SelectionMode::Triangle => (1, self.selected_triangles.clone()),
            SelectionMode::Edge => {
                let mut edges = Vec::new();
                let mut edge_ids = HashMap::new();
                for tri in &self.current_mesh.indices {
                    for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                        let edge = if a < b { (a, b) } else { (b, a) };
                        if !edge_ids.contains_key(&edge) {
                            let id = edges.len();
                            edges.push(edge);
                            edge_ids.insert(edge, id);
                        }
                    }
                }
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                (2, edges.into_iter().enumerate()
                    .filter_map(|(id, edge)| selected.contains(&edge).then_some(id))
                    .collect())
            }
        };
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            return false;
        }
        let pos = self.graph.nodes.get(&source)
            .map_or(egui::pos2(40.0, 40.0), |node| node.pos + egui::vec2(0.0, 80.0));
        let blast = self.graph.add(
            NodeKind::Blast { component, indices, invert: false },
            pos,
        );
        let network = self.network();
        if let Some(n) = self.graph.nodes.get_mut(&blast) {
            n.context = self.context;
            n.parent = network;
        }
        self.graph.connect(source, blast, 0);
        self.graph.set_display(blast);
        self.selected = Some(blast);
        self.clear_component_selection();
        self.status = "Blast node created from viewport selection".into();
        self.dirty = true;
        true
    }

    // A new SOP Solver starts like Houdini's: Prev_Frame and Input_1 nodes, displaying Prev_Frame.
    fn populate_new_node(&mut self, id: NodeId) {
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_lop()) && self.graph.display_of(STAGE_NETWORK).is_none() {
            self.graph.set_display(id);
        }
        if matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::AttributeVop { .. })) {
            // Like Houdini's: P read in and written back out, ready to be modified in between.
            let bind = self.graph.add(NodeKind::Vop(vop::VopNode::new(vop::BIND)), egui::pos2(40.0, 30.0));
            let export = self.graph.add(NodeKind::Vop(vop::VopNode::new(vop::EXPORT)), egui::pos2(40.0, 260.0));
            for child in [bind, export] {
                if let Some(n) = self.graph.nodes.get_mut(&child) {
                    n.context = AppContext::Obj;
                    n.parent = Some(id);
                }
            }
            self.graph.connect(bind, export, 0);
            self.status = "Attribute VOP created: double-click it to build its network (bind P → export P)".into();
        }
        if matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::MaterialBuilder)) {
            let output = self.graph.add(NodeKind::ShaderNode(shading::ShaderNode::new(shading::SURFACE_OUTPUT)), egui::pos2(40.0, 240.0));
            if let Some(n) = self.graph.nodes.get_mut(&output) {
                n.context = AppContext::Mat;
                n.parent = Some(id);
            }
            self.graph.set_display(output);
            self.status = "Material Builder created: double-click it and wire texture nodes into the Surface Output".into();
            return;
        }
        if !matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::SopSolver { .. })) {
            return;
        }
        for (index, x) in [(0, 40.0), (1, 220.0)] {
            let input = self.graph.add(NodeKind::SolverInput { index }, egui::pos2(x, 20.0));
            if let Some(n) = self.graph.nodes.get_mut(&input) {
                n.context = AppContext::Obj;
                n.parent = Some(id);
            }
            if index == 0 {
                self.graph.set_display(input);
            }
        }
        self.status = "SOP Solver created: double-click it to edit its network (Prev_Frame is last step's result)".into();
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::Add(kind, pos) => {
                let is_particles = matches!(
                    kind,
                    NodeKind::Add { .. }
                        | NodeKind::Particles { .. }
                                | NodeKind::Flip { .. }
                        | NodeKind::WhitewaterSolver(_)
                        | NodeKind::Cache { .. }
                );
                let id = self.graph.add(kind, pos);
                let parent = self.view_parent();
                if let Some(n) = self.graph.nodes.get_mut(&id) {
                    n.context = self.context;
                    n.parent = parent;
                }
                self.populate_new_node(id);
                self.selected = Some(id);
                if is_particles {
                    self.graph.set_display(id);
                    self.show_points = true;
                    self.status = "Particle/fluid output displayed; Points mode enabled".into();
                }
            }
            Action::AddBelow(kind, from) => {
                let pos = self
                    .graph
                    .nodes
                    .get(&from)
                    .map_or(egui::pos2(40.0, 40.0), |n| n.pos + egui::vec2(0.0, 80.0));
                let takes_input = kind.num_inputs() > 0;
                let is_particles = matches!(
                    kind,
                    NodeKind::Add { .. }
                        | NodeKind::Particles { .. }
                                | NodeKind::Flip { .. }
                        | NodeKind::WhitewaterSolver(_)
                        | NodeKind::Cache { .. }
                );
                let id = self.graph.add(kind, pos);
                let parent = self.view_parent();
                if let Some(n) = self.graph.nodes.get_mut(&id) {
                    n.context = self.context;
                    n.parent = parent;
                }
                self.populate_new_node(id);
                if takes_input {
                    if self.graph.nodes.get(&from).map_or(false, |n| n.context == self.context && n.parent == parent) {
                        self.graph.connect(from, id, 0);
                        if self.graph.is_display(from) {
                            self.graph.set_display(id);
                        }
                    }
                }
                self.selected = Some(id);
                if is_particles {
                    self.graph.set_display(id);
                    self.show_points = true;
                    self.status = "Particle/fluid output displayed; Points mode enabled".into();
                }
            }
            Action::Display(id) => match self.graph.nodes.get(&id).map(|n| &n.kind) {
                Some(NodeKind::Camera { .. }) => {
                    self.selected = Some(id);
                    self.use_camera_requested = true;
                }
                Some(kind) if kind.is_object() => {
                    if let Some(n) = self.graph.nodes.get_mut(&id) {
                        n.bypass = !n.bypass;
                    }
                }
                Some(_) => {
                    self.graph.set_display(id);
                }
                None => {}
            },
            Action::Enter(id) => self.enter_network(Some(id)),
            Action::Bypass(id) => {
                if let Some(n) = self.graph.nodes.get_mut(&id) {
                    n.bypass = !n.bypass;
                }
            }
            Action::Duplicate(id) => {
                if let Some(new) = self.graph.duplicate(id) {
                    self.selected = Some(new);
                }
            }
            Action::Delete(id) => {
                self.graph.remove(id);
                if self.template == Some(id) {
                    self.template = None;
                }
                if self.selected.is_some_and(|s| !self.graph.nodes.contains_key(&s)) {
                    self.selected = None;
                }
                if self.active_camera == Some(id) {
                    self.active_camera = None;
                }
            }
        }
        self.dirty = true;
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // File shortcuts work even while a text field (e.g. VEX code) has focus.
        // Save As is consumed first so Cmd+Shift+S isn't taken as Cmd+S.
        let (open, save, save_as) = ctx.input_mut(|i| {
            let save_as = i.consume_shortcut(&SHORTCUT_SAVE_AS);
            (i.consume_shortcut(&SHORTCUT_OPEN), i.consume_shortcut(&SHORTCUT_SAVE), save_as)
        });
        if save_as {
            self.save_scene_dialog();
        } else if save {
            self.save_scene();
        }
        if open {
            self.open_scene_dialog();
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let (undo, redo, dup, byp, frame, del, play, escape) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && ((i.modifiers.shift && i.key_pressed(egui::Key::Z)) || i.key_pressed(egui::Key::Y)),
                cmd && i.key_pressed(egui::Key::D),
                !cmd && i.key_pressed(egui::Key::B),
                !cmd && i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            self.animate = false;
            if let Some(cancel) = &self.simulation_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            if self.simulation_rx.take().is_some() {
                self.simulation_cancel = None;
                self.cancelled_simulation_key = self.simulation_key.clone();
                self.status = "Cooking cancelled".into();
            }
            if self.sequence.take().is_some() {
                self.render_at = None;
                self.status = "Sequence cancelled (the current frame will finish)".into();
            }
        }
        if undo {
            self.do_undo();
        }
        if redo {
            self.do_redo();
        }
        if frame {
            self.frame_all();
        }
        if play {
            self.animate = !self.animate;
        }
        if let Some(sel) = self.selected {
            if ctx.input(|i| !i.modifiers.command && i.key_pressed(egui::Key::T)) {
                self.template = if self.template == Some(sel) { None } else { Some(sel) };
                self.dirty = true;
            }
        }
        if let Some(sel) = self.selected {
            if dup {
                self.apply(Action::Duplicate(sel));
            }
            if byp {
                self.apply(Action::Bypass(sel));
            }
            if del && self.selected_vertices.is_empty()
                && self.selected_triangles.is_empty()
                && self.selected_edges.is_empty()
            {
                self.apply(Action::Delete(sel));
            }
        }
        if del
            && (!self.selected_vertices.is_empty()
                || !self.selected_triangles.is_empty()
                || !self.selected_edges.is_empty())
        {
            self.create_blast_from_selection();
        }
    }

    // Shows the node's input and selects its current group, ready for picking.
    fn start_group_pick(&mut self, id: NodeId) {
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let (group, support) = (node.group.clone(), node.kind.group_support());
        let mut input = self.graph.input_of(id, 0).map(|i| self.graph.cook(i, self.time, &mut HashMap::new())).unwrap_or_default();
        let domain = match group.kind {
            1 => groups::GroupDomain::Points,
            2 => groups::GroupDomain::Prims,
            // Nodes that work on primitives (scatter, groom roots, blast) start with primitives.
            _ if group.is_empty() && support == GroupSupport::Native && !matches!(node.kind, NodeKind::Wrangle { .. }) => groups::GroupDomain::Prims,
            _ => groups::guess_domain(&input, &group.pattern),
        };
        let current: Vec<usize> = if group.is_empty() {
            Vec::new()
        } else {
            groups::evaluate(&input, &group.pattern, domain).map(|m| m.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect()).unwrap_or_default()
        };
        if let Some(object) = self.graph.enclosing_geometry(id) {
            input = self.graph.xform_at(object, self.time).apply(input);
        }
        self.clear_component_selection();
        let previous_mode = self.selection_mode;
        match domain {
            groups::GroupDomain::Points => {
                self.selection_mode = SelectionMode::Vertex;
                self.selected_vertices = current;
            }
            groups::GroupDomain::Prims => {
                self.selection_mode = SelectionMode::Triangle;
                self.selected_triangles = current;
            }
        }
        self.current_mesh = input.clone();
        self.group_pick = Some(GroupPick { node: id, domain, input, previous_mode });
        self.status = "Select in the viewport, then press Enter (or ✓) to set the group; Esc cancels".into();
    }

    // Switches a running pick between points and primitives.
    fn set_group_pick_domain(&mut self, domain: groups::GroupDomain) {
        if let Some(pick) = &mut self.group_pick {
            pick.domain = domain;
            self.selection_mode = if domain == groups::GroupDomain::Points { SelectionMode::Vertex } else { SelectionMode::Triangle };
            self.clear_component_selection();
        }
    }

    fn finish_group_pick(&mut self, accept: bool) {
        let Some(pick) = self.group_pick.take() else { return };
        if accept {
            let selected = match pick.domain {
                groups::GroupDomain::Points => self.selected_vertices.clone(),
                groups::GroupDomain::Prims => self.selected_triangles.clone(),
            };
            let pattern = groups::describe(&pick.input, &selected, pick.domain);
            if let Some(node) = self.graph.nodes.get_mut(&pick.node) {
                node.group.kind = if pick.domain == groups::GroupDomain::Points { 1 } else { 2 };
                self.status = if pattern.is_empty() { "Group cleared (nothing selected): the node applies to everything".into() } else { format!("Group set to {pattern}") };
                node.group.pattern = pattern;
            }
        } else {
            self.status = "Group selection cancelled".into();
        }
        self.selection_mode = pick.previous_mode;
        self.clear_component_selection();
        self.dirty = true;
    }

    fn clear_component_selection(&mut self) {
        self.selected_vertices.clear();
        self.selected_triangles.clear();
        self.selected_edges.clear();
    }

    fn select_edge_loop(&mut self, seed: (usize, usize)) {
        let seed = (seed.0.min(seed.1), seed.0.max(seed.1));
        let mut incident: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        for tri in &self.current_mesh.indices {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let edge = (a.min(b), a.max(b));
                incident.entry(a).or_default().push(edge);
                incident.entry(b).or_default().push(edge);
            }
        }
        for edges in incident.values_mut() {
            edges.sort_unstable();
            edges.dedup();
        }
        let mut edges = HashSet::from([seed]);
        let vertices = &self.current_mesh.vertices;
        let continue_from = |edge: (usize, usize), endpoint: usize, edges: &HashSet<(usize, usize)>| {
            let other = if edge.0 == endpoint { edge.1 } else { edge.0 };
            let direction = vertices[other].sub(vertices[endpoint]).normalize();
            incident.get(&endpoint)
                .into_iter()
                .flatten()
                .filter(|candidate| **candidate != edge && !edges.contains(candidate))
                .filter_map(|candidate| {
                    let next = if candidate.0 == endpoint { candidate.1 } else { candidate.0 };
                    let next_direction = vertices[next].sub(vertices[endpoint]).normalize();
                    let alignment = direction.dot(next_direction).abs();
                    (alignment > 0.35).then_some((alignment, *candidate))
                })
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(_, candidate)| candidate)
        };
        for &(endpoint, other) in &[(seed.0, seed.1), (seed.1, seed.0)] {
            let mut current = (endpoint, other);
            for _ in 0..2048 {
                let Some(next) = continue_from(
                    (current.0.min(current.1), current.0.max(current.1)),
                    current.1,
                    &edges,
                ) else {
                    break;
                };
                edges.insert(next);
                current = (current.1, if next.0 == current.1 { next.1 } else { next.0 });
            }
        }
        self.selected_edges = edges.into_iter().collect();
        self.selected_edges.sort_unstable();
    }

    fn select_all_components(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                self.selected_vertices = (0..self.current_mesh.vertices.len()).collect();
            }
            SelectionMode::Triangle => {
                self.selected_triangles = (0..self.current_mesh.indices.len()).collect();
            }
            SelectionMode::Edge => {
                let mut edges = HashSet::new();
                for tri in &self.current_mesh.indices {
                    let pairs = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])];
                    for (a, b) in pairs {
                        edges.insert(if a < b { (a, b) } else { (b, a) });
                    }
                }
                self.selected_edges = edges.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    fn invert_component_selection(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let selected: HashSet<_> = self.selected_vertices.iter().copied().collect();
                self.selected_vertices = (0..self.current_mesh.vertices.len())
                    .filter(|index| !selected.contains(index))
                    .collect();
            }
            SelectionMode::Triangle => {
                let selected: HashSet<_> = self.selected_triangles.iter().copied().collect();
                self.selected_triangles = (0..self.current_mesh.indices.len())
                    .filter(|index| !selected.contains(index))
                    .collect();
            }
            SelectionMode::Edge => {
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                self.select_all_components();
                self.selected_edges.retain(|edge| !selected.contains(edge));
            }
        }
    }

    fn grow_component_selection(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let selected: HashSet<_> = self.selected_vertices.iter().copied().collect();
                let mut grown = selected.clone();
                for tri in &self.current_mesh.indices {
                    if tri.iter().any(|index| selected.contains(index)) {
                        grown.extend(tri);
                    }
                }
                self.selected_vertices = grown.into_iter().collect();
                self.selected_vertices.sort_unstable();
            }
            SelectionMode::Triangle => {
                let selected: HashSet<_> = self.selected_triangles.iter().copied().collect();
                let mut grown = selected.clone();
                for (index, tri) in self.current_mesh.indices.iter().enumerate() {
                    if selected.iter().any(|selected_index| {
                        self.current_mesh.indices.get(*selected_index).is_some_and(|other| {
                            tri.iter().any(|vertex| other.contains(vertex))
                        })
                    }) {
                        grown.insert(index);
                    }
                }
                self.selected_triangles = grown.into_iter().collect();
                self.selected_triangles.sort_unstable();
            }
            SelectionMode::Edge => {
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                let mut grown = selected.clone();
                for tri in &self.current_mesh.indices {
                    let edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                        .map(|(a, b)| if a < b { (a, b) } else { (b, a) });
                    if edges.iter().any(|edge| selected.contains(edge)) {
                        grown.extend(edges);
                    }
                }
                self.selected_edges = grown.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    fn shrink_component_selection(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let selected: HashSet<_> = self.selected_vertices.iter().copied().collect();
                let mut retained = HashSet::new();
                for &vertex in &selected {
                    let incident: Vec<_> = self
                        .current_mesh
                        .indices
                        .iter()
                        .filter(|tri| tri.contains(&vertex))
                        .collect();
                    if !incident.is_empty()
                        && incident.iter().all(|tri| tri.iter().all(|index| selected.contains(index)))
                    {
                        retained.insert(vertex);
                    }
                }
                self.selected_vertices = retained.into_iter().collect();
                self.selected_vertices.sort_unstable();
            }
            SelectionMode::Triangle => {
                let selected: HashSet<_> = self.selected_triangles.iter().copied().collect();
                let mut retained = HashSet::new();
                for &triangle_index in &selected {
                    let Some(triangle) = self.current_mesh.indices.get(triangle_index) else {
                        continue;
                    };
                    let neighbors: Vec<_> = self
                        .current_mesh
                        .indices
                        .iter()
                        .enumerate()
                        .filter(|(index, other)| {
                            *index != triangle_index
                                && triangle.iter().filter(|vertex| other.contains(vertex)).count() >= 2
                        })
                        .map(|(index, _)| index)
                        .collect();
                    if neighbors.iter().all(|neighbor| selected.contains(neighbor)) {
                        retained.insert(triangle_index);
                    }
                }
                self.selected_triangles = retained.into_iter().collect();
                self.selected_triangles.sort_unstable();
            }
            SelectionMode::Edge => {
                let selected: HashSet<_> = self.selected_edges.iter().copied().collect();
                let mut retained = HashSet::new();
                for &edge in &selected {
                    let touching: Vec<_> = self
                        .current_mesh
                        .indices
                        .iter()
                        .filter(|tri| tri.contains(&edge.0) && tri.contains(&edge.1))
                        .collect();
                    if touching.iter().all(|tri| {
                        [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                            .iter()
                            .map(|&(a, b)| if a < b { (a, b) } else { (b, a) })
                            .all(|candidate| selected.contains(&candidate))
                    }) {
                        retained.insert(edge);
                    }
                }
                self.selected_edges = retained.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    fn select_boundary_edges(&mut self) {
        self.selection_mode = SelectionMode::Edge;
        self.clear_component_selection();
        self.selected_edges = self
            .current_mesh
            .indices
            .iter()
            .flat_map(|tri| [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])])
            .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
            .filter(|edge| {
                self.current_mesh
                    .indices
                    .iter()
                    .filter(|tri| tri.contains(&edge.0) && tri.contains(&edge.1))
                    .count()
                    == 1
            })
            .collect();
        self.selected_edges.sort_unstable();
    }

    fn select_non_manifold_edges(&mut self) {
        let mut counts = HashMap::new();
        for tri in &self.current_mesh.indices {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let edge = if a < b { (a, b) } else { (b, a) };
                *counts.entry(edge).or_insert(0usize) += 1;
            }
        }
        self.selection_mode = SelectionMode::Edge;
        self.clear_component_selection();
        self.selected_edges = counts
            .into_iter()
            .filter_map(|(edge, count)| (count > 2).then_some(edge))
            .collect();
        self.selected_edges.sort_unstable();
    }

    fn select_isolated_vertices(&mut self) {
        let mut referenced: HashSet<usize> = HashSet::new();
        for tri in &self.current_mesh.indices {
            referenced.extend(tri);
        }
        self.selection_mode = SelectionMode::Vertex;
        self.clear_component_selection();
        self.selected_vertices = (0..self.current_mesh.vertices.len())
            .filter(|index| !referenced.contains(index))
            .collect();
    }

    fn select_connected_components(&mut self) {
        match self.selection_mode {
            SelectionMode::Vertex => {
                let mut connected: HashSet<usize> = self.selected_vertices.iter().copied().collect();
                let mut changed = true;
                while changed {
                    changed = false;
                    for tri in &self.current_mesh.indices {
                        if tri.iter().any(|vertex| connected.contains(vertex)) {
                            changed |= tri.iter().any(|vertex| connected.insert(*vertex));
                        }
                    }
                }
                self.selected_vertices = connected.into_iter().collect();
                self.selected_vertices.sort_unstable();
            }
            SelectionMode::Triangle => {
                let mut connected: HashSet<usize> = self.selected_triangles.iter().copied().collect();
                let mut changed = true;
                while changed {
                    changed = false;
                    for index in 0..self.current_mesh.indices.len() {
                        if connected.contains(&index) {
                            continue;
                        }
                        let triangle = self.current_mesh.indices[index];
                        if connected.iter().any(|selected| {
                            self.current_mesh.indices.get(*selected).is_some_and(|other| {
                                triangle.iter().filter(|vertex| other.contains(vertex)).count() >= 2
                            })
                        }) {
                            changed |= connected.insert(index);
                        }
                    }
                }
                self.selected_triangles = connected.into_iter().collect();
                self.selected_triangles.sort_unstable();
            }
            SelectionMode::Edge => {
                let mut connected: HashSet<(usize, usize)> =
                    self.selected_edges.iter().copied().collect();
                let mut changed = true;
                while changed {
                    changed = false;
                    for tri in &self.current_mesh.indices {
                        let edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]
                            .map(|(a, b)| if a < b { (a, b) } else { (b, a) });
                        if edges.iter().any(|edge| connected.contains(edge)) {
                            changed |= edges.iter().any(|edge| connected.insert(*edge));
                        }
                    }
                }
                self.selected_edges = connected.into_iter().collect();
                self.selected_edges.sort_unstable();
            }
        }
    }

    // ---------------- Parameter panel ----------------
    fn params_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(id) = self.selected else {
            ui.label("Select a node to edit its parameters.");
            return false;
        };
        let is_display = self.graph.is_display(id);
        let geometry_objects = self.graph.geometry_ids();
        let picked_prim = self.stage_selected_prim.clone();
        let viewport_view = self.viewport_eye_target();
        let solver_display_is_input = self
            .graph
            .display_of(id)
            .and_then(|d| self.graph.nodes.get(&d))
            .is_some_and(|n| matches!(n.kind, NodeKind::SolverInput { .. }));
        let camera_ids = self.camera_ids();
        let mut material_ids: Vec<(NodeId, String)> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| n.kind.is_material())
            .filter(|(_, n)| n.parent.is_none())
            .map(|(mid, n)| (*mid, n.kind.label(*mid)))
            .collect();
        material_ids.sort_unstable();
        // An Attribute VOP's generated code and its Parameter VOPs' defaults.
        let vop_code = matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::AttributeVop { .. })).then(|| self.graph.vop_code(id));
        let vop_parameters: Vec<(String, [f32; 3])> = self
            .graph
            .children(id)
            .into_iter()
            .filter_map(|child| match &self.graph.nodes.get(&child)?.kind {
                NodeKind::Vop(v) if v.op == vop::PARAMETER => Some((v.name.clone(), v.value)),
                _ => None,
            })
            .collect();
        // Group names on the node's input, for the Group field's menu (filled in on request).
        let group_choices = self.group_choices.as_ref().filter(|(for_id, ..)| *for_id == id).map(|(_, p, q)| (p.clone(), q.clone()));
        let Some(node) = self.graph.nodes.get_mut(&id) else {
            return false;
        };
        let is_object = node.kind.is_object();
        ui.heading(match &node.kind {
            NodeKind::Vop(v) => format!("{} · {}", v.def().label, v.label(id)),
            NodeKind::ShaderNode(n) => format!("{} · {}", n.def().label, n.label(id)),
            kind => kind.label(id),
        });
        let mut c = false;
        ui.horizontal(|ui| {
            if is_object {
                let mut active = !node.bypass;
                if ui.checkbox(&mut active, "Active").changed() {
                    node.bypass = !active;
                    c = true;
                }
            } else {
                c |= ui.checkbox(&mut node.bypass, "Bypass").changed();
                ui.label(if is_display { "● display" } else { "" });
            }
        });
        let support = node.kind.group_support();
        if support != GroupSupport::None {
            ui.horizontal(|ui| {
                ui.label(if support == GroupSupport::Pin { "Pin group" } else { "Group" });
                c |= ui
                    .add(egui::TextEdit::singleline(&mut node.group.pattern).hint_text("everything").desired_width(150.0))
                    .on_hover_text("Names (a b), wildcards (side*), numbers (0-10 20-40:2), expressions (@P.y>0), ^name to exclude, !name for everything else")
                    .changed();
                let menu = ui.menu_button("▾", |ui| {
                    match &group_choices {
                        None => {
                            ui.weak("Reading groups…");
                        }
                        Some((points, prims)) if points.is_empty() && prims.is_empty() => {
                            ui.weak("The input has no groups (make one with Group Create)");
                        }
                        Some((points, prims)) => {
                            for (list, label) in [(points, "Point groups"), (prims, "Primitive groups")] {
                                if list.is_empty() {
                                    continue;
                                }
                                ui.strong(label);
                                for name in list {
                                    if ui.button(name).clicked() {
                                        node.group.pattern = name.clone();
                                        c = true;
                                        ui.close();
                                    }
                                }
                            }
                        }
                    }
                });
                if menu.inner.is_some() {
                    self.group_choices_request = Some(id);
                }
                let picking = self.group_pick.as_ref().is_some_and(|p| p.node == id);
                if !picking && ui.button("⬚ Select").on_hover_text("Pick the group's points or primitives in the viewport").clicked() {
                    self.group_pick_request = Some(id);
                }
                egui::ComboBox::from_id_salt("group_type").selected_text(groups::GROUP_TYPES[node.group.kind.min(2) as usize]).width(110.0).show_ui(ui, |ui| {
                    for (value, label) in groups::GROUP_TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut node.group.kind, value as u8, *label).changed();
                    }
                });
            });
            if let Some(pick) = self.group_pick.as_ref().filter(|p| p.node == id) {
                let picked = match pick.domain {
                    groups::GroupDomain::Points => self.selected_vertices.len(),
                    groups::GroupDomain::Prims => self.selected_triangles.len(),
                };
                let domain = pick.domain;
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 210, 90), format!("Selecting: {picked}"));
                    for (d, label) in [(groups::GroupDomain::Points, "Points"), (groups::GroupDomain::Prims, "Primitives")] {
                        if ui.selectable_label(domain == d, label).clicked() && domain != d {
                            self.group_pick_domain = Some(d);
                        }
                    }
                    if ui.button("✓ Accept").clicked() {
                        self.group_pick_finish = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        self.group_pick_finish = Some(false);
                    }
                });
            }
        }
        ui.separator();

        match &mut node.kind {
            NodeKind::Geometry { material } => {
                ui.label("Object container. Its SOP network defines the shape rendered in the scene.");
                ui.strong("Transform");
                c |= vec3_ui(ui, "Translate", &mut node.xform.t, 0.05);
                c |= vec3_ui(ui, "Rotate°", &mut node.xform.r, 0.5);
                c |= vec3_ui(ui, "Scale", &mut node.xform.s, 0.02);
                ui.small("Or drag the gizmo in the viewport (W move, E rotate, R scale).");
                let selected_name = material
                    .and_then(|m| material_ids.iter().find(|(mid, _)| *mid == m))
                    .map_or("default (grey)".to_string(), |(_, name)| name.clone());
                ui.horizontal(|ui| {
                    ui.label("Material");
                    egui::ComboBox::from_id_salt("geometry_material")
                        .selected_text(selected_name)
                        .show_ui(ui, |ui| {
                            c |= ui.selectable_value(material, None, "default (grey)").changed();
                            for (mid, name) in &material_ids {
                                c |= ui.selectable_value(material, Some(*mid), name).changed();
                            }
                        });
                });
                ui.small("Create materials in the MAT context.");
                if ui.button("Enter geometry network").clicked() {
                    self.current_network = Some(id);
                    self.context = AppContext::Obj;
                    self.selected = None;
                    self.graph_pan = egui::Vec2::ZERO;
                    c = true;
                }
                ui.small("Double-click the node to enter; press U in the network to go back up.");
            }
            NodeKind::Shader(values) => {
                ui.small(match values.table {
                    shading::Table::Principled => "Physically based surface (MoonRay DwaBaseMaterial). Assign it from a Geometry node.",
                    shading::Table::Toon => "Cel-shaded surface with banded shadows and a toon highlight (DwaToonMaterial).",
                    shading::Table::Hair => "For hair and fur curves from Hair Generate (HairMaterial_v3).",
                    shading::Table::Pyro => "For Pyro and other fog volumes: smoke colour and density, fire brightness and tint.",
                });
                c |= shader_values_ui(ui, values);
            }
            NodeKind::MaterialBuilder => {
                ui.small("Build a material from texture nodes: double-click to enter, then wire Image Texture, Noise, Mix… into the Surface Output's inputs.");
            }
            NodeKind::ShaderNode(node) => {
                ui.small(node.def().help);
                if node.op == shading::SURFACE_OUTPUT {
                    ui.small("Connected inputs replace the values below.");
                    c |= shader_values_ui(ui, &mut node.surface);
                } else {
                    let labels = node.def().texts;
                    for (label, text) in labels.iter().zip(node.texts.iter_mut()) {
                        ui.horizontal(|ui| {
                            ui.label(*label);
                            c |= ui.text_edit_singleline(text).changed();
                            if *label == "File" && ui.button("…").clicked() {
                                if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "exr", "tif", "tiff", "hdr", "tx"]).pick_file() {
                                    *text = path.to_string_lossy().into_owned();
                                    c = true;
                                }
                            }
                        });
                    }
                    for (param, slot) in node.param_mut() {
                        c |= shader_param_ui(ui, param, slot);
                    }
                }
            }
            NodeKind::Material { kind, color, roughness, value } => {
                ui.label("MoonRay material. Assign it from a Geometry node.");
                ui.horizontal(|ui| {
                    for (index, name) in MATERIAL_NAMES.iter().enumerate() {
                        c |= ui.selectable_value(kind, index, *name).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                match *kind {
                    0 => {
                        c |= ui.add(egui::Slider::new(roughness, 0.0..=1.0).text("Roughness")).changed();
                        c |= ui.add(egui::Slider::new(value, 0.0..=1.0).text("Specular")).changed();
                    }
                    1 => {
                        c |= ui.add(egui::Slider::new(roughness, 0.0..=1.0).text("Roughness")).changed();
                    }
                    2 => {
                        c |= ui.add(egui::Slider::new(roughness, 0.0..=1.0).text("Roughness")).changed();
                        c |= ui.add(egui::Slider::new(value, 1.0..=3.0).text("IOR")).changed();
                    }
                    _ => {
                        c |= ui.add(egui::Slider::new(value, 0.0..=50.0).logarithmic(true).text("Intensity")).changed();
                    }
                }
            }
            NodeKind::Add { count, spacing, layout } => {
                ui.label("Create points for attribute workflows and Copy to Points.");
                c |= ui.add(egui::Slider::new(count, 1..=100_000).logarithmic(true).text("Point count")).changed();
                c |= ui.add(egui::Slider::new(spacing, 0.01..=10.0).logarithmic(true).text("Spacing")).changed();
                ui.horizontal(|ui| {
                    ui.label("Layout");
                    c |= ui.selectable_value(layout, 0, "Line").changed();
                    c |= ui.selectable_value(layout, 1, "Vertical").changed();
                    c |= ui.selectable_value(layout, 2, "Grid").changed();
                });
            }
            NodeKind::Grid { size, res } => {
                c |= ui.add(egui::Slider::new(size, 0.5..=20.0).text("Size")).changed();
                c |= ui.add(egui::Slider::new(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Plane { size, res } => {
                c |= ui.add(egui::Slider::new(size, 0.5..=20.0).text("Size")).changed();
                c |= ui.add(egui::Slider::new(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Cone { radius, height, rows, cols, caps } => {
                c |= ui.add(egui::Slider::new(radius, 0.1..=10.0).text("Radius")).changed();
                c |= ui.add(egui::Slider::new(height, 0.1..=20.0).text("Height")).changed();
                c |= ui.add(egui::Slider::new(rows, 1..=32).text("Rows")).changed();
                c |= ui.add(egui::Slider::new(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Capsule { radius, length, rows, cols } => {
                c |= ui.add(egui::Slider::new(radius, 0.1..=10.0).text("Radius")).changed();
                c |= ui.add(egui::Slider::new(length, 0.0..=20.0).text("Length")).changed();
                c |= ui.add(egui::Slider::new(rows, 4..=96).text("Rows")).changed();
                c |= ui.add(egui::Slider::new(cols, 8..=128).text("Columns")).changed();
            }
            NodeKind::Box { size, div } => {
                c |= ui.add(egui::Slider::new(size, 0.1..=10.0).text("Size")).changed();
                c |= ui.add(egui::Slider::new(div, 1..=30).text("Divisions")).changed();
            }
            NodeKind::Sphere { radius, rows, cols } => {
                c |= ui.add(egui::Slider::new(radius, 0.1..=10.0).text("Radius")).changed();
                c |= ui.add(egui::Slider::new(rows, 2..=96).text("Rows")).changed();
                c |= ui.add(egui::Slider::new(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Torus { major, minor, rows, cols } => {
                c |= ui.add(egui::Slider::new(major, 0.1..=10.0).text("Major Radius")).changed();
                c |= ui.add(egui::Slider::new(minor, 0.01..=5.0).text("Minor Radius")).changed();
                c |= ui.add(egui::Slider::new(rows, 3..=96).text("Rows")).changed();
                c |= ui.add(egui::Slider::new(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Transform { t, r, s } => {
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= vec3_ui(ui, "Scale    ", s, 0.01);
            }
            NodeKind::Noise { freq, amp } => {
                c |= ui.add(egui::Slider::new(freq, 0.0..=10.0).text("Frequency")).changed();
                c |= ui.add(egui::Slider::new(amp, 0.0..=2.0).text("Amplitude")).changed();
            }
            NodeKind::Taper { factor } => {
                c |= ui.add(egui::Slider::new(factor, -2.0..=2.0).text("Taper")).changed();
            }
            NodeKind::Twist { angle } => {
                c |= ui.add(egui::Slider::new(angle, -5.0..=5.0).text("Twist / unit")).changed();
            }
            NodeKind::Bend { angle } => {
                c |= ui.add(egui::Slider::new(angle, -3.0..=3.0).text("Bend / unit")).changed();
            }
            NodeKind::Smooth { iterations, strength } => {
                c |= ui.add(egui::Slider::new(iterations, 0..=50).text("Iterations")).changed();
                c |= ui.add(egui::Slider::new(strength, 0.0..=1.0).text("Strength")).changed();
            }
            NodeKind::Bevel { amount, segments } => {
                c |= ui.add(egui::Slider::new(amount, 0.0..=1.0).text("Amount")).changed();
                c |= ui.add(egui::Slider::new(segments, 1..=8).text("Segments")).changed();
            }
            NodeKind::Fuse { tol } => {
                c |= ui
                    .add(egui::Slider::new(tol, 0.0001..=0.5).logarithmic(true).text("Distance"))
                    .changed();
            }
            NodeKind::Mirror { axis } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
            }
            NodeKind::Copy { count, t, r, s } => {
                c |= ui.add(egui::Slider::new(count, 1..=100).text("Copies")).changed();
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= ui.add(egui::Slider::new(s, 0.5..=1.5).text("Uniform Scale")).changed();
            }
            NodeKind::CopyToPoints { scale, rotation } => {
                ui.label("Copy the first input onto every point from the second input.");
                c |= ui.add(egui::Slider::new(scale, 0.01..=10.0).logarithmic(true).text("Copy scale")).changed();
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                ui.small("Use Add or a particle output as the point input.");
            }
            NodeKind::AttributeCreate { attribute, value } => {
                ui.label("Create or replace a point attribute.");
                c |= ui.add(egui::Slider::new(attribute, 0..=3).text("Attribute (0=id, 1=pscale, 2=Cd, 3=orient)")).changed();
                c |= ui.add(egui::DragValue::new(value).speed(0.05).prefix("Value ")).changed();
            }
            NodeKind::AttributeRandomize { attribute, amount, seed } => {
                ui.label("Add deterministic random variation to a point attribute.");
                c |= ui.add(egui::Slider::new(attribute, 0..=3).text("Attribute (0=id, 1=pscale, 2=Cd, 3=orient)")).changed();
                c |= ui.add(egui::Slider::new(amount, 0.0..=10.0).text("Amount")).changed();
                c |= ui.add(egui::DragValue::new(seed).prefix("Seed ")).changed();
            }
            NodeKind::AttributeDelete { attribute } => {
                ui.label("Remove a point attribute.");
                c |= ui.add(egui::Slider::new(attribute, 0..=3).text("Attribute (0=id, 1=pscale, 2=Cd, 3=orient)")).changed();
            }
            NodeKind::AttributeVop { domain } => {
                ui.heading("Attribute VOP");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.small("Double-click the node to edit its network. Bind reads attributes, Export writes them.");
                match &vop_code {
                    Some(Ok(code)) => {
                        // Parameter VOPs start at their default value.
                        for (name, default) in &vop_parameters {
                            node.channels.entry(name.clone()).or_insert(*default);
                        }
                        c |= channel_params_ui(ui, code, &mut node.channels);
                        egui::CollapsingHeader::new("Generated VEX").id_salt("vop_code").show(ui, |ui| {
                            ui.label(egui::RichText::new(code.as_str()).monospace().small());
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {e}"));
                    }
                    None => {}
                }
            }
            NodeKind::Vop(v) => {
                let def = v.def();
                ui.small(def.help);
                match v.op {
                    vop::BIND | vop::EXPORT | vop::PARAMETER | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        ui.horizontal(|ui| {
                            ui.label(if v.op == vop::PARAMETER { "Parameter" } else { "Attribute" });
                            c |= ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(120.0)).changed();
                            if matches!(v.op, vop::BIND | vop::GET_ATTRIB | vop::PC_AVERAGE) {
                                ui.menu_button("▾", |ui| {
                                    for (name, ty) in [("P", 1), ("N", 1), ("Cd", 1), ("v", 1), ("pscale", 0), ("ptnum", 2), ("primnum", 2), ("numpt", 2), ("Time", 0), ("Frame", 0), ("orient", 3)] {
                                        if ui.button(name).clicked() {
                                            v.name = name.into();
                                            v.ty = ty;
                                            c = true;
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        });
                    }
                    vop::SNIPPET => {
                        c |= ui.add(egui::TextEdit::singleline(&mut v.name).code_editor().desired_width(f32::INFINITY).hint_text("$0 * 2 + $1")).changed();
                        ui.small("$0 … $3 are the inputs.");
                    }
                    _ => {}
                }
                match v.op {
                    vop::BIND | vop::CONSTANT | vop::PARAMETER | vop::EXPORT | vop::SNIPPET | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        let choices: &[(&str, char)] = if v.op == vop::PARAMETER { &vop::TYPES[..3] } else { &vop::TYPES };
                        ui.horizontal(|ui| {
                            ui.label("Type");
                            for (i, (name, _)) in choices.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *name).changed();
                            }
                        });
                    }
                    vop::COMPONENT => {
                        ui.horizontal(|ui| {
                            ui.label("Component");
                            for (i, axis) in ["x", "y", "z"].iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *axis).changed();
                            }
                        });
                    }
                    vop::COMPARE => {
                        ui.horizontal(|ui| {
                            ui.label("a");
                            for (i, op) in vop::COMPARES.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *op).changed();
                            }
                            ui.label("b");
                        });
                    }
                    _ => {}
                }
                if matches!(v.op, vop::CONSTANT | vop::PARAMETER) {
                    let label = if v.op == vop::CONSTANT { "Value" } else { "Default" };
                    if v.ty == 1 || v.ty == 3 {
                        c |= vec3_ui(ui, label, &mut v.value, 0.01);
                    } else {
                        c |= ui.add(egui::DragValue::new(&mut v.value[0]).speed(0.01).prefix(format!("{label} "))).changed();
                    }
                }
                if !def.inputs.is_empty() {
                    let names: Vec<String> = def.inputs.iter().map(|(n, d)| format!("{n} ({d})")).collect();
                    ui.weak(format!("Inputs, left to right: {}", names.join(", ")));
                }
            }
            NodeKind::Wrangle { code, domain } => {
                ui.heading("VEX Wrangle");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.menu_button("Insert example ▾", |ui| {
                        for (label, example_domain, example) in vex::EXAMPLES {
                            if ui.button(*label).on_hover_text("Replaces the current code").clicked() {
                                *code = example.to_string();
                                *domain = *example_domain;
                                c = true;
                                ui.close_menu();
                            }
                        }
                    });
                    ui.weak("Edits cook live");
                });
                c |= ui.add(
                    egui::TextEdit::multiline(code)
                        .code_editor()
                        .desired_rows(14)
                        .desired_width(f32::INFINITY)
                        .hint_text("@P.y += sin(@P.x * 3.0) * 0.2;"),
                ).changed();
                c |= channel_params_ui(ui, code, &mut node.channels);

                let report = vex::last_report(code, *domain);
                let error = match &report {
                    Some(report) => report.error.clone(),
                    None => vex::check(code).err(),
                };
                match error {
                    Some(error) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {error}"));
                        // Show the offending source line under the message.
                        let line = error
                            .strip_prefix("line ")
                            .and_then(|rest| rest.split(|ch: char| !ch.is_ascii_digit()).next())
                            .and_then(|n| n.parse::<usize>().ok());
                        if let Some(text) = line.and_then(|n| code.lines().nth(n.saturating_sub(1))) {
                            ui.monospace(format!("{:>4} | {}", line.unwrap_or(0), text.trim_end()));
                        }
                        if report.is_some() {
                            ui.small("The node passes its input through unchanged until the error is fixed.");
                        }
                    }
                    None => match &report {
                        Some(report) => {
                            let noun = match *domain {
                                0 => "detail",
                                1 => "primitives",
                                2 => "points",
                                _ => "vertices",
                            };
                            ui.colored_label(
                                egui::Color32::LIGHT_GREEN,
                                format!("✔ Ran over {} {noun} in {:.2} ms", report.elements, report.millis),
                            );
                        }
                        None => {
                            ui.weak("✔ Compiles. It runs when the node is part of the displayed network.");
                        }
                    },
                }
                if let Some(report) = report.as_ref().filter(|r| !r.output.is_empty()) {
                    egui::CollapsingHeader::new(format!("Output ({} lines)", report.output.len()))
                        .id_salt("wrangle_output")
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                                for line in &report.output {
                                    ui.monospace(line);
                                }
                            });
                        });
                }
                egui::CollapsingHeader::new("Language reference").id_salt("wrangle_reference").show(ui, |ui| {
                    ui.label(egui::RichText::new(vex::SYNTAX_HELP).monospace().small());
                    let mut group = "";
                    for (_, _, _, function_group, signature) in vex::BUILTINS {
                        if *function_group != group {
                            group = function_group;
                            ui.add_space(4.0);
                            ui.strong(group);
                        }
                        ui.label(egui::RichText::new(*signature).monospace().small());
                    }
                });
            }
            NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                c |= ui.add(egui::Slider::new(r_bot, 0.0..=5.0).text("Bottom Radius")).changed();
                c |= ui.add(egui::Slider::new(r_top, 0.0..=5.0).text("Top Radius")).changed();
                c |= ui.add(egui::Slider::new(height, 0.1..=10.0).text("Height")).changed();
                c |= ui.add(egui::Slider::new(rows, 1..=64).text("Rows")).changed();
                c |= ui.add(egui::Slider::new(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Peak { dist } => {
                c |= ui.add(egui::Slider::new(dist, -2.0..=2.0).text("Distance")).changed();
            }
            NodeKind::Ripple { freq, amp, speed } => {
                c |= ui.add(egui::Slider::new(freq, 0.0..=20.0).text("Frequency")).changed();
                c |= ui.add(egui::Slider::new(amp, 0.0..=2.0).text("Amplitude")).changed();
                c |= ui.add(egui::Slider::new(speed, 0.0..=20.0).text("Speed")).changed();
            }
            NodeKind::Jitter { amp, seed } => {
                c |= ui.add(egui::Slider::new(amp, 0.0..=1.0).text("Amount")).changed();
                c |= ui.add(egui::Slider::new(seed, 0..=999).text("Seed")).changed();
            }
            NodeKind::Particles {
                count,
                emission_rate,
                lifetime,
                speed,
                spread,
                gravity,
                seed,
                drag,
                bounce,
                friction,
                radius,
                substeps,
                along_normals,
            } => {
                ui.small("Input 1: optional source (emits from its surface, or its points; the origin when unconnected) • Input 2: optional static collider.");
                ui.strong("Emission");
                c |= ui.add(egui::Slider::new(count, 1..=50_000).logarithmic(true).text("Max particles")).changed();
                c |= ui.add(egui::Slider::new(emission_rate, 1.0..=10_000.0).logarithmic(true).text("Birth rate (per second)")).changed();
                c |= ui.add(egui::Slider::new(lifetime, 0.1..=20.0).text("Lifetime")).changed();
                c |= ui.add(egui::Slider::new(speed, 0.0..=20.0).text("Speed")).changed();
                c |= ui.add(egui::Slider::new(spread, 0.0..=2.0).text("Spread")).changed();
                c |= ui
                    .checkbox(along_normals, "Emit along source normals")
                    .on_hover_text("Off: particles head up (+Y). Points inherit their v attribute either way.")
                    .changed();
                ui.strong("Forces");
                c |= ui.add(egui::Slider::new(gravity, -20.0..=10.0).text("Gravity")).changed();
                c |= ui.add(egui::Slider::new(drag, 0.0..=5.0).text("Drag")).changed();
                ui.strong("Collisions");
                c |= ui.add(egui::Slider::new(bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= ui.add(egui::Slider::new(friction, 0.0..=1.0).text("Friction")).changed();
                c |= ui.add(egui::Slider::new(radius, 0.0..=0.5).text("Particle radius")).changed();
                ui.strong("Solver");
                c |= ui.add(egui::Slider::new(substeps, 1..=8).text("Substeps")).changed();
                c |= ui.add(egui::Slider::new(seed, 0..=9999).text("Seed")).changed();
                ui.small("Outputs points with v, age, life and id attributes.");
            }
            NodeKind::Flip {
                particle_count,
                grid_resolution,
                flip_ratio,
                viscosity,
                gravity,
                max_dt,
                substeps,
                seed,
            } => {
                ui.label("3D CPU FLIP solver");
                c |= ui.add(egui::Slider::new(particle_count, 1..=100_000).logarithmic(true).text("Particles")).changed();
                c |= ui.add(egui::Slider::new(grid_resolution, 16..=128).text("Grid resolution")).changed();
                c |= ui.add(egui::Slider::new(flip_ratio, 0.0..=1.0).text("FLIP blend")).changed();
                c |= ui.add(egui::Slider::new(viscosity, 0.0..=4.0).text("Viscosity")).changed();
                c |= ui.add(egui::Slider::new(gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= ui.add(egui::Slider::new(max_dt, 0.001..=0.1).logarithmic(true).text("Max timestep")).changed();
                c |= ui.add(egui::Slider::new(substeps, 1..=8).text("Substeps")).changed();
                c |= ui.add(egui::Slider::new(seed, 0..=9999).text("Seed")).changed();
                ui.small("Input 1: emitter surface • Input 2: optional static collider.");
            }
            NodeKind::WhitewaterSolver(w) => {
                ui.small("Spray, foam and bubbles where the fluid input splashes. Input 1: FLIP fluid • Input 2: optional collider. Writes `type` and spray/foam/bubble groups; renders as small spheres.");
                c |= ui.add(egui::Slider::new(&mut w.max_particles, 0..=2_000_000).logarithmic(true).text("Max particles")).changed();
                c |= ui.add(egui::Slider::new(&mut w.emission_rate, 0.0..=10_000.0).logarithmic(true).text("Emission rate")).changed();
                c |= ui.add(egui::Slider::new(&mut w.trapped_air, 0.0..=4.0).text("Trapped air")).on_hover_text("Emission where fluid collides or tears apart").changed();
                c |= ui.add(egui::Slider::new(&mut w.wave_crest, 0.0..=4.0).text("Wave crests")).on_hover_text("Emission from breaking crests").changed();
                c |= ui.add(egui::Slider::new(&mut w.min_speed, 0.0..=10.0).text("Min speed")).changed();
                c |= ui.add(egui::Slider::new(&mut w.max_speed, 0.1..=30.0).text("Full emission speed")).changed();
                c |= ui.add(egui::Slider::new(&mut w.lifespan, 0.05..=10.0).text("Foam lifespan (s)")).changed();
                c |= ui.add(egui::Slider::new(&mut w.spray_drag, 0.0..=5.0).text("Spray drag")).changed();
                c |= ui.add(egui::Slider::new(&mut w.bubble_buoyancy, 0.0..=10.0).text("Bubble buoyancy")).changed();
                c |= ui.add(egui::Slider::new(&mut w.bubble_drag, 0.0..=1.0).text("Bubble drag")).changed();
                c |= ui.add(egui::Slider::new(&mut w.spray_neighbours, 0..=50).text("Spray below (neighbours)")).changed();
                c |= ui.add(egui::Slider::new(&mut w.bubble_neighbours, 1..=100).text("Bubbles above (neighbours)")).changed();
                c |= ui.add(egui::Slider::new(&mut w.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= ui.add(egui::Slider::new(&mut w.particle_size, 0.0005..=0.2).logarithmic(true).text("Particle size")).changed();
                c |= ui.add(egui::Slider::new(&mut w.substeps, 1..=8).text("Substeps")).changed();
                c |= ui.add(egui::Slider::new(&mut w.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                ui.label("Voxel particle-fluid surface");
                c |= ui.add(egui::Slider::new(radius, 0.01..=1.0).logarithmic(true).text("Particle radius")).changed();
                c |= ui.add(egui::Slider::new(join_distance, 0.0..=1.0).text("Join distance")).changed();
                c |= ui.add(egui::Slider::new(resolution, 8..=96).text("Resolution")).changed();
                c |= ui.add(egui::Slider::new(smoothing, 0..=3).text("Smoothing")).changed();
                ui.small("Nearby particles are unioned into one fluid-like surface before polygon extraction.");
            }
            NodeKind::Extrude { dist, inset, mode } => {
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    c |= ui.selectable_value(mode, 0, "Individual polygons").changed();
                    c |= ui.selectable_value(mode, 1, "Whole surface").changed();
                });
                c |= ui.add(egui::Slider::new(dist, -2.0..=2.0).text("Distance")).changed();
                c |= ui.add(egui::Slider::new(inset, 0.0..=1.0).text("Inset")).changed();
                ui.small("Whole surface shares vertices and creates walls only along open boundaries.");
            }
            NodeKind::Clip { axis, offset, invert } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
                c |= ui.add(egui::Slider::new(offset, -5.0..=5.0).text("Offset")).changed();
                c |= ui.checkbox(invert, "Keep Below").changed();
            }
            NodeKind::Blast { component, indices, invert } => {
                ui.label(format!("Delete {} selected component(s)", indices.len()));
                ui.horizontal(|ui| {
                    ui.label("Component:");
                    c |= ui.selectable_value(component, 0, "Vertices").changed();
                    c |= ui.selectable_value(component, 1, "Faces").changed();
                    c |= ui.selectable_value(component, 2, "Edges").changed();
                });
                c |= ui.checkbox(invert, "Keep selected (delete outside)").changed();
                if ui.button("Clear stored selection").clicked() {
                    indices.clear();
                    c = true;
                }
                ui.small("Select components in the viewport and press Delete to create this node.");
            }
            NodeKind::Cache { cache_id, mode } => {
                ui.label("Persistent frame cache: cache/cache_ID_frame.mcache");
                c |= ui.add(egui::Slider::new(cache_id, 0..=9999).text("Cache ID")).changed();
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    c |= ui.selectable_value(mode, 0, "Read").changed();
                    c |= ui.selectable_value(mode, 1, "Write").changed();
                });
                ui.small("Write saves each evaluated frame. Read reuses files and falls back to the input on a miss.");
            }
            NodeKind::Vdb { voxel_size, padding, operation, iterations, output } => {
                ui.heading("VDB from Polygons");
                ui.horizontal(|ui| {
                    ui.label("Output");
                    c |= ui.selectable_value(output, 1, "VDB (level set)").changed();
                    c |= ui.selectable_value(output, 0, "Polygons").changed();
                });
                ui.small(if *output == 1 {
                    "Outputs a signed-distance VDB; use Convert VDB to turn it into polygons or a fog volume."
                } else {
                    "Converts the mesh to a signed-distance volume and outputs its surface directly."
                });
                c |= ui
                    .add(egui::Slider::new(voxel_size, 0.03..=1.0).logarithmic(true).text("Voxel size"))
                    .changed();
                ui.small("Smaller voxels = more detail and slower cooking.");
                c |= ui.add(egui::Slider::new(padding, 0..=4).text("Volume padding")).changed();
                ui.horizontal(|ui| {
                    ui.label("Surface operation");
                    egui::ComboBox::from_id_salt("vdb-operation")
                        .selected_text(match *operation {
                            1 => "Dilate",
                            2 => "Erode",
                            3 => "Smooth",
                            _ => "Rasterize",
                        })
                        .show_ui(ui, |ui| {
                            c |= ui.selectable_value(operation, 0, "Rasterize").changed();
                            c |= ui.selectable_value(operation, 1, "Dilate").changed();
                            c |= ui.selectable_value(operation, 2, "Erode").changed();
                            c |= ui.selectable_value(operation, 3, "Smooth").changed();
                        });
                });
                if *operation == 1 || *operation == 2 {
                    c |= ui.add(egui::Slider::new(iterations, 1..=4).text("Surface offset (voxels)")).changed();
                    ui.small(if *operation == 1 {
                        "Moves the surface outward by the selected number of voxels."
                    } else {
                        "Moves the surface inward by the selected number of voxels."
                    });
                } else if *operation == 3 {
                    c |= ui.add(egui::Slider::new(iterations, 1..=4).text("Smoothing passes")).changed();
                    ui.small("Removes small voxel-scale bumps.");
                } else {
                    ui.small("Creates the base VDB surface without changing its shape.");
                }
                ui.small("The VDB node is evaluated from its first input.");
            }
            NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts } => {
                ui.heading("MoonRay Renderer");
                ui.small("Renders every active geometry with its MAT material, the active lights and the chosen camera, or the displayed USD stage.");
                ui.label("Executable");
                c |= ui.text_edit_singleline(executable).changed();
                ui.label("Arguments ({scene} {output} {width} {height} {samples})");
                c |= ui.text_edit_singleline(arguments).changed();
                ui.label("Output image");
                c |= ui.text_edit_singleline(output).changed();
                if ui.button("Choose output file...").clicked() {
                    if let Some(path) = FileDialog::new().set_file_name("render.exr").save_file() {
                        *output = path.to_string_lossy().into_owned();
                        c = true;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Camera");
                    render_camera_combo(ui, &mut opts.camera, &camera_ids);
                });
                c |= ui.checkbox(&mut opts.stage, "Render the STAGE (USD) instead of the OBJ objects").changed();
                c |= ui.add(egui::DragValue::new(width).range(1..=16384).prefix("Width ")).changed();
                c |= ui.add(egui::DragValue::new(height).range(1..=16384).prefix("Height ")).changed();
                c |= ui.add(egui::DragValue::new(samples).range(1..=4096).prefix("Samples per pixel ")).changed();
                c |= ui.add(egui::Slider::new(&mut opts.max_depth, 1..=32).text("Max ray depth")).changed();
                c |= ui.add(egui::Slider::new(&mut opts.light_samples, 1..=16).text("Light samples")).changed();
                c |= ui.add(egui::Slider::new(&mut opts.bsdf_samples, 1..=16).text("BSDF samples")).changed();
                c |= ui
                    .add(egui::Slider::new(&mut opts.volume_samples, 1..=8).text("Volume lighting samples"))
                    .on_hover_text("Light samples per step inside smoke and fire; render time grows roughly in proportion")
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("Environment light");
                    c |= ui.color_edit_button_rgb(&mut opts.background).changed();
                });
                ui.small("A non-black color adds a uniform environment light (not visible in camera).");
                ui.separator();
                c |= ui.checkbox(&mut opts.sequence, "Render frame range").changed();
                if opts.sequence {
                    ui.horizontal(|ui| {
                        c |= ui.add(egui::DragValue::new(&mut opts.range_start).speed(0.05).range(0.0..=100_000.0).prefix("Start ")).changed();
                        c |= ui.add(egui::DragValue::new(&mut opts.range_end).speed(0.05).range(0.0..=100_000.0).prefix("End ")).changed();
                        c |= ui.add(egui::DragValue::new(&mut opts.range_step).speed(0.01).range(0.0001..=1000.0).prefix("Step ")).changed();
                    });
                    if ui.button("Use timeline range").clicked() {
                        opts.range_start = self.playback_start.max(0.0);
                        opts.range_end = self.playback_end.max(opts.range_start);
                        c = true;
                    }
                    let frames = frame_times(opts.range_start, opts.range_end, opts.range_step).len();
                    ui.small(format!(
                        "{frames} frames, timeline units. Files: {} (use #### for the frame number position).",
                        frame_output(output, 1)
                    ));
                }
                if let Some(job) = &self.sequence {
                    ui.label(format!("Sequence: frame {}/{}", job.next, job.times.len()));
                    if ui.button("Cancel sequence").clicked() {
                        self.cancel_render();
                    }
                } else if self.moonray_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("MoonRay render in progress...");
                        if ui.button("Cancel").clicked() {
                            self.cancel_render();
                        }
                    });
                } else if ui.button(if opts.sequence { "Render sequence" } else { "Render" }).clicked() {
                    self.start_moonray_render();
                }
                ui.small("Set arguments to match your MoonRay installation. The node passes the exported scene path and output path as placeholders.");
            }
            NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } => {
                ui.label("Viewport camera rig");
                c |= vec3_ui(ui, "Target", position, 0.05);
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                c |= ui.add(egui::Slider::new(focal, 0.5..=2.5).text("Focal scale")).changed();
                c |= ui.add(egui::Slider::new(near, 0.05..=10.0).text("Near clip")).changed();
                c |= ui.checkbox(orthographic, "Orthographic").changed();
                c |= ui.add(egui::DragValue::new(&mut pan[0]).speed(1.0).prefix("Pan X ")).changed();
                c |= ui.add(egui::DragValue::new(&mut pan[1]).speed(1.0).prefix("Pan Y ")).changed();
                c |= ui.add(egui::Slider::new(zoom, 0.5..=200.0).logarithmic(true).text("Zoom")).changed();
                if self.active_camera == Some(id) {
                    ui.label("Active viewport camera");
                } else if ui.button("Use as viewport camera").clicked() {
                    self.use_camera_requested = true;
                }
                ui.small("Target is the orbit center used by the procedural viewport.");
            }
            NodeKind::Light { position, rotation, color, intensity, kind } => {
                ui.label("Viewport light");
                c |= vec3_ui(ui, "Position", position, 0.05);
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                c |= vec3_ui(ui, "Color", color, 0.02);
                c |= ui.add(egui::Slider::new(intensity, 0.0..=4.0).text("Intensity")).changed();
                ui.horizontal(|ui| {
                    ui.label("Type");
                    c |= ui.selectable_value(kind, 0, "Directional").changed();
                    c |= ui.selectable_value(kind, 1, "Point").changed();
                    c |= ui.selectable_value(kind, 2, "Fill").changed();
                });
                ui.small("Directional lights use rotation. Point lights use position and are evaluated per surface.\nActive lights drive both the viewport and MoonRay; with none active, a default key/fill rig is used.");
            }
            NodeKind::Switch { index } => {
                ui.horizontal(|ui| {
                    c |= ui.selectable_value(index, 0, "Input 1").changed();
                    c |= ui.selectable_value(index, 1, "Input 2").changed();
                });
            }
            NodeKind::Reverse => {
                ui.label("Flips triangle winding (normals).");
            }
            NodeKind::Subdivide { levels } => {
                c |= ui.add(egui::Slider::new(levels, 1..=4).text("Levels")).changed();
            }
            NodeKind::Remesh { target } => {
                c |= ui
                    .add(egui::Slider::new(target, 4..=50_000).logarithmic(true).text("Target triangles"))
                    .changed();
                ui.label("Adds shared-edge subdivisions until the target is reached.");
            }
            NodeKind::HairGuides(set) => {
                ui.small("Input 1: skin mesh. Grows guide curves from its surface along the normals.");
                c |= ui.add(egui::Slider::new(&mut set.count, 1..=20_000).logarithmic(true).text("Guides")).changed();
                c |= ui.add(egui::Slider::new(&mut set.length, 0.0..=5.0).text("Length")).changed();
                c |= ui.add(egui::Slider::new(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= ui.add(egui::Slider::new(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= ui.add(egui::Slider::new(&mut set.droop, 0.0..=3.0).text("Droop")).changed();
                c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::GuideProcess(set) => {
                ui.small("Shapes the guide curves from input 1. Chain several to combine operations.");
                egui::ComboBox::from_label("Operation").selected_text(groom::GUIDE_OPS[set.op.min(groom::GUIDE_OPS.len() - 1)]).show_ui(ui, |ui| {
                    for (i, label) in groom::GUIDE_OPS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.op, i, *label).changed();
                    }
                });
                let (amount_label, range) = match set.op {
                    0 => ("Length", 0.0..=5.0),
                    1 => ("Bend", 0.0..=5.0),
                    2 => ("Frizz", 0.0..=2.0),
                    3 => ("Curl radius", 0.0..=2.0),
                    _ => ("Smoothing", 0.0..=2.0),
                };
                c |= ui.add(egui::Slider::new(&mut set.amount, range).text(amount_label)).changed();
                if matches!(set.op, 2 | 3) {
                    let label = if set.op == 2 { "Frequency" } else { "Turns per unit length" };
                    c |= ui.add(egui::Slider::new(&mut set.frequency, 0.0..=20.0).text(label)).changed();
                    c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
                }
            }
            NodeKind::HairDeform(set) => {
                ui.small("Input 1: hair or guides • Input 2: animated skin. Each strand moves with the skin under its root.");
                c |= ui
                    .add(egui::DragValue::new(&mut set.rest_frame).speed(1.0).range(-10_000.0..=100_000.0).prefix("Rest frame "))
                    .on_hover_text("The frame the hair was groomed on. The skin must keep the same topology at every frame.")
                    .changed();
            }
            NodeKind::HairGenerate(set) => {
                ui.small("Input 1: skin • Input 2: guides. Interpolates dense hair from the three nearest guides.");
                c |= ui.add(egui::Slider::new(&mut set.count, 1..=200_000).logarithmic(true).text("Hairs")).changed();
                c |= ui.add(egui::Slider::new(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= ui.add(egui::Slider::new(&mut set.clump, 0.0..=1.0).text("Clumping")).changed();
                c |= ui.add(egui::Slider::new(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= ui.add(egui::Slider::new(&mut set.frizz, 0.0..=2.0).text("Frizz")).changed();
                c |= ui.add(egui::Slider::new(&mut set.width_root, 0.0..=0.05).text("Root width")).changed();
                c |= ui.add(egui::Slider::new(&mut set.width_tip, 0.0..=0.05).text("Tip width")).changed();
                c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
                ui.small("Merge the result with the skin to render both.");
            }
            NodeKind::Pyro(set) => {
                ui.heading("Pyro Solver");
                ui.small("Smoke and fire on a voxel grid. Input 1: source (closed mesh or points, pscale sets point radius) • Input 2: optional collider (closed mesh).");
                ui.horizontal(|ui| {
                    ui.label("Preset");
                    for (label, preset) in set.presets() {
                        if ui.button(label).clicked() {
                            *set = preset;
                            c = true;
                        }
                    }
                });
                ui.separator();
                ui.strong("Domain");
                c |= vec3_ui(ui, "Center", &mut set.center, 0.05);
                c |= vec3_ui(ui, "Size", &mut set.size, 0.05);
                if ui.button("Fit around source").on_hover_text("Centre the box on the source and leave room for the smoke to rise").clicked() {
                    self.pyro_fit_request = Some(id);
                }
                c |= ui.add(egui::Slider::new(&mut set.resolution, 8..=160).text("Resolution (voxels on longest side)")).changed();
                let voxels = {
                    let h = set.size.iter().cloned().fold(0.0, f32::max) / set.resolution.max(4) as f32;
                    set.size.iter().map(|s| ((s / h).ceil() as usize).max(4)).product::<usize>()
                };
                ui.weak(format!("{voxels} voxels"));
                c |= ui.add(egui::Slider::new(&mut set.substeps, 1..=8).text("Substeps")).changed();
                ui.separator();
                ui.strong("Source");
                c |= ui.add(egui::Slider::new(&mut set.density, 0.0..=4.0).text("Density")).changed();
                c |= ui.add(egui::Slider::new(&mut set.temperature, 0.0..=4.0).text("Temperature")).changed();
                c |= ui.add(egui::Slider::new(&mut set.fuel, 0.0..=4.0).text("Fuel (fire)")).changed();
                ui.strong("Shape");
                c |= ui.add(egui::Slider::new(&mut set.buoyancy, -2.0..=5.0).text("Buoyancy")).changed();
                c |= ui.add(egui::Slider::new(&mut set.dissipation, 0.0..=3.0).text("Dissipation")).changed();
                c |= ui.add(egui::Slider::new(&mut set.cooling, 0.0..=5.0).text("Cooling")).changed();
                c |= ui.add(egui::Slider::new(&mut set.burn_rate, 0.0..=5.0).text("Burn rate")).changed();
                c |= ui.add(egui::Slider::new(&mut set.vorticity, 0.0..=3.0).text("Vorticity")).changed();
                c |= ui.add(egui::Slider::new(&mut set.turbulence, 0.0..=3.0).text("Turbulence")).changed();
                c |= vec3_ui(ui, "Wind", &mut set.wind, 0.05);
                c |= ui.add(egui::Slider::new(&mut set.opacity, 0.0..=4.0).text("Display opacity")).changed();
                ui.small("Output: points with density, temperature, flame, Cd, Alpha and pscale, drawn as soft volume splats. Edits restart the simulation.");
            }
            NodeKind::GroupCreate(set) => {
                ui.horizontal(|ui| {
                    ui.label("Group name");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.name).desired_width(140.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Type");
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Points, "Points").changed();
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Prims, "Primitives").changed();
                });
                egui::ComboBox::from_label("Method").selected_text(groups::CREATE_METHODS[set.method.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in groups::CREATE_METHODS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, value, *label).changed();
                    }
                });
                match set.method {
                    0 => {
                        c |= vec3_ui(ui, "Min", &mut set.bbox_min, 0.05);
                        c |= vec3_ui(ui, "Max", &mut set.bbox_max, 0.05);
                    }
                    1 => {
                        c |= ui.add(egui::Slider::new(&mut set.percent, 0.0..=100.0).text("Percent")).changed();
                        c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
                    }
                    2 => {
                        c |= vec3_ui(ui, "Direction", &mut set.direction, 0.05);
                        c |= ui.add(egui::Slider::new(&mut set.spread, 0.0..=180.0).text("Spread angle")).changed();
                    }
                    _ => {
                        c |= ui.add(egui::TextEdit::singleline(&mut set.expression).hint_text("@P.y>0  0-10  other ^skip").desired_width(220.0)).changed();
                    }
                }
                egui::ComboBox::from_label("Combine").selected_text(groups::COMBINE_MODES[set.combine.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in groups::COMBINE_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                ui.small("Stored as a 0/1 attribute (group:name / prim:group:name); use the name in any node's Group field.");
            }
            NodeKind::EnvironmentLight { file, color, intensity, rotation, visible } => {
                ui.heading("Environment Light");
                ui.small("Lights the scene from a lat-long HDRI (.exr or .hdr), in MoonRay and the viewport.");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/sky.exr").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("HDRI", &["exr", "hdr", "png", "jpg", "jpeg", "tif", "tiff"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if !file.is_empty() {
                    if let Err(e) = envlight::load(file) {
                        ui.colored_label(egui::Color32::from_rgb(230, 110, 90), e);
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                c |= ui.add(egui::Slider::new(intensity, 0.0..=10.0).text("Intensity")).changed();
                c |= ui.add(egui::Slider::new(rotation, -180.0..=180.0).text("Rotate (Y°)")).changed();
                c |= ui.checkbox(visible, "Visible in camera (background)").changed();
            }
            NodeKind::AttributePaint { attribute, values, settings: set } => {
                let painting = self.paint_active;
                if ui
                    .selectable_label(painting, if painting { "🖌 Painting — drag in the viewport (Ctrl erases)" } else { "🖌 Paint in viewport" })
                    .on_hover_text("While on, dragging in the viewport paints instead of orbiting. Right/middle drag still pans.")
                    .clicked()
                {
                    self.paint_active = !painting;
                }
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(attribute).desired_width(110.0)).changed();
                    egui::ComboBox::from_id_salt("paint_presets").selected_text("Groom ▾").show_ui(ui, |ui| {
                        for name in paint::GROOM_ATTRIBUTES {
                            if ui.selectable_label(attribute == name, name).clicked() {
                                *attribute = name.to_owned();
                                c = true;
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    for (value, label) in paint::PAINT_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                c |= ui.add(egui::Slider::new(&mut set.value, -2.0..=4.0).text("Paint value")).changed();
                c |= ui.add(egui::Slider::new(&mut set.default, -2.0..=4.0).text("Default (unpainted)")).changed();
                c |= ui.add(egui::Slider::new(&mut set.radius, 2.0..=400.0).logarithmic(true).text("Brush radius (px)")).changed();
                c |= ui.add(egui::Slider::new(&mut set.strength, 0.01..=1.0).text("Strength")).changed();
                c |= ui.checkbox(&mut set.visualize, "Show as colour (Cd)").changed();
                ui.horizontal(|ui| {
                    if ui.button("Fill with paint value").clicked() {
                        // Needs the input's point count; done at the start of the next frame.
                        self.paint_fill = Some(id);
                    }
                    if ui.button("Clear").clicked() {
                        values.clear();
                        c = true;
                    }
                });
                ui.small(format!(
                    "{} painted point(s). The groom reads density, length, clump, frizz and mask from the skin.",
                    values.len()
                ));
            }
            NodeKind::Normal(set) => {
                ui.horizontal(|ui| {
                    ui.label("Add normals to");
                    for (value, label) in sops::NORMAL_DOMAINS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.domain, value, *label).changed();
                    }
                });
                if set.domain == 1 {
                    c |= ui
                        .add(egui::Slider::new(&mut set.cusp_angle, 0.0..=180.0).text("Cusp angle"))
                        .on_hover_text("Faces meeting at a sharper angle than this keep a hard edge")
                        .changed();
                }
                egui::ComboBox::from_label("Weighting").selected_text(sops::NORMAL_WEIGHTS[set.weighting.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in sops::NORMAL_WEIGHTS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.weighting, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.reverse, "Reverse normals").changed();
                ui.small("The viewport shades with these normals (vertex N, then point N).");
            }
            NodeKind::Scatter(set) => {
                ui.small("Scatters points on the input's surface, or inside it when it's a closed mesh. Points get N, pscale and sourceprim.");
                c |= ui.add(egui::Slider::new(&mut set.count, 1..=1_000_000).logarithmic(true).text("Points")).changed();
                c |= ui.checkbox(&mut set.volume, "Fill the inside (closed meshes)").changed();
                c |= ui.add(egui::Slider::new(&mut set.relax_iterations, 0..=50).text("Relax iterations")).on_hover_text("Spreads the points out evenly").changed();
                c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::Boolean(set) => {
                ui.small("Combines input A (left) with input B (right). Both should be closed meshes. Result faces are in the a_faces and b_faces primitive groups.");
                egui::ComboBox::from_label("Operation").selected_text(boolean::OPERATIONS[set.operation.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::OPERATIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.operation, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Method").selected_text(boolean::MODES[set.mode.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                if set.mode == 1 {
                    c |= ui.add(egui::Slider::new(&mut set.resolution, 16..=400).text("Voxel resolution")).on_hover_text("Voxels along the longest side").changed();
                } else {
                    ui.small("Exact keeps sharp edges. Use Volume for very dense or leaky meshes.");
                }
            }
            NodeKind::MaterialFracture(set) => {
                ui.small("Breaks a closed mesh into pieces for the RBD Solver. Pieces get `piece`; cut faces are in the `inside` group, the original surface in `outside`.");
                egui::ComboBox::from_label("Material").selected_text(fracture::MATERIALS[set.material.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in fracture::MATERIALS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.material, value, *label).changed();
                    }
                });
                match set.material {
                    1 => {
                        ui.horizontal(|ui| {
                            ui.label("Impact point");
                            for k in 0..3 {
                                c |= ui.add(egui::DragValue::new(&mut set.impact[k]).speed(0.01)).changed();
                            }
                        });
                        c |= ui.add(egui::Slider::new(&mut set.rings, 1..=20).text("Crack rings")).changed();
                        c |= ui.add(egui::Slider::new(&mut set.spokes, 3..=64).text("Radial cracks")).changed();
                    }
                    m => {
                        c |= ui.add(egui::Slider::new(&mut set.pieces, 2..=500).logarithmic(true).text("Pieces")).changed();
                        if m == 2 {
                            ui.horizontal(|ui| {
                                ui.label("Grain along");
                                for (value, label) in sops::AXES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut set.grain_axis, value, *label).changed();
                                }
                            });
                            c |= ui.add(egui::Slider::new(&mut set.splinter, 1.0..=20.0).text("Splinter length")).changed();
                        }
                    }
                }
                c |= ui.add(egui::Slider::new(&mut set.separate, 0.0..=1.0).text("Explode view")).on_hover_text("Moves pieces apart to see the cracks").changed();
                c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::CrowdSolver(set) => {
                ui.small("Input 1: start points (e.g. Scatter on a grid) • Input 2: optional ground to walk on • Input 3: optional obstacles.");
                ui.horizontal(|ui| {
                    ui.label("Display");
                    for (value, label) in crowd::DISPLAYS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.display, value, *label).changed();
                    }
                });
                c |= ui.add(egui::Slider::new(&mut set.speed, 0.0..=10.0).text("Walk speed")).changed();
                c |= ui.add(egui::Slider::new(&mut set.speed_variance, 0.0..=1.0).text("Speed variance")).changed();
                c |= ui.add(egui::Slider::new(&mut set.turn_rate, 1.0..=720.0).text("Turn rate°/s")).changed();
                c |= ui.add(egui::Slider::new(&mut set.personal_space, 0.05..=5.0).text("Personal space")).changed();
                c |= ui.add(egui::Slider::new(&mut set.separation, 0.0..=10.0).text("Separation")).changed();
                c |= ui.add(egui::Slider::new(&mut set.alignment, 0.0..=5.0).text("Alignment")).changed();
                c |= ui.add(egui::Slider::new(&mut set.cohesion, 0.0..=5.0).text("Cohesion")).changed();
                c |= ui.add(egui::Slider::new(&mut set.wander, 0.0..=5.0).text("Wander")).changed();
                ui.horizontal(|ui| {
                    ui.label("Goal");
                    for k in 0..3 {
                        c |= ui.add(egui::DragValue::new(&mut set.goal[k]).speed(0.1)).changed();
                    }
                });
                c |= ui.add(egui::Slider::new(&mut set.goal_weight, 0.0..=10.0).text("Goal strength")).changed();
                c |= ui.add(egui::Slider::new(&mut set.avoid_distance, 0.0..=10.0).text("Obstacle look-ahead")).changed();
                c |= ui.add(egui::Slider::new(&mut set.agent_height, 0.1..=5.0).text("Agent height")).changed();
                c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::Muscle(set) => {
                ui.small("A muscle from origin to insertion, or along a curve connected to input 1. Animate Activation to flex it.");
                for (label, point) in [("Origin", &mut set.origin), ("Insertion", &mut set.insertion)] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        for k in 0..3 {
                            c |= ui.add(egui::DragValue::new(&mut point[k]).speed(0.01)).changed();
                        }
                    });
                }
                egui::ComboBox::from_label("Shape").selected_text(muscle::PROFILES[set.profile.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in muscle::PROFILES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.profile, value, *label).changed();
                    }
                });
                c |= ui.add(egui::Slider::new(&mut set.radius, 0.005..=2.0).logarithmic(true).text("Belly radius")).changed();
                if set.profile == 1 {
                    c |= ui.add(egui::Slider::new(&mut set.flatness, 1.0..=10.0).text("Flatness")).changed();
                    c |= ui.add(egui::Slider::new(&mut set.twist, -180.0..=180.0).text("Twist°")).changed();
                }
                c |= ui.add(egui::Slider::new(&mut set.tendon, 0.0..=0.45).text("Tendon length")).changed();
                c |= ui.add(egui::Slider::new(&mut set.activation, 0.0..=1.0).text("Activation")).changed();
                c |= ui.add(egui::Slider::new(&mut set.contraction, 0.0..=0.8).text("Max contraction")).changed();
                c |= ui.add(egui::Slider::new(&mut set.segments, 2..=128).text("Segments")).changed();
                c |= ui.add(egui::Slider::new(&mut set.sides, 3..=64).text("Sides")).changed();
                c |= ui.add(egui::Slider::new(&mut set.id, 0..=999).text("Muscle id")).changed();
            }
            NodeKind::MuscleSolver(set) => {
                ui.small("Jiggle on animated muscles (flexing or moved by their bones). The origin and insertion tips stay pinned.");
                c |= ui.add(egui::Slider::new(&mut set.stiffness, 1.0..=2000.0).logarithmic(true).text("Firmness")).changed();
                c |= ui.add(egui::Slider::new(&mut set.damping, 0.0..=50.0).text("Damping")).changed();
                c |= ui.add(egui::Slider::new(&mut set.smoothing, 0..=50).text("Coherence")).on_hover_text("Higher wobbles more as one body").changed();
                c |= ui.add(egui::Slider::new(&mut set.substeps, 1..=16).text("Substeps")).changed();
            }
            NodeKind::MuscleSkin(set) => {
                ui.small("Input 1: skin • Input 2: muscles (animated). The skin follows how the muscles move from frame 0.");
                c |= ui.add(egui::Slider::new(&mut set.falloff, 0.001..=5.0).logarithmic(true).text("Reach")).changed();
                c |= ui.add(egui::Slider::new(&mut set.influences, 1..=16).text("Influences")).changed();
            }
            NodeKind::Terrain(set) => {
                ui.small("A flat heightfield grid. Shape it with Terrain Noise, Erode and Terrace.");
                ui.horizontal(|ui| {
                    ui.label("Size");
                    c |= ui.add(egui::DragValue::new(&mut set.size[0]).speed(0.1).range(0.01..=10_000.0)).changed();
                    c |= ui.add(egui::DragValue::new(&mut set.size[1]).speed(0.1).range(0.01..=10_000.0)).changed();
                });
                c |= ui.add(egui::Slider::new(&mut set.resolution, 2..=2048).logarithmic(true).text("Resolution")).on_hover_text("Grid cells along the longer side").changed();
            }
            NodeKind::TerrainNoise(set) => {
                egui::ComboBox::from_label("Noise").selected_text(terrain::NOISE_TYPES[set.kind.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::NOISE_TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Blend").selected_text(terrain::BLEND_MODES[set.blend.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::BLEND_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.blend, value, *label).changed();
                    }
                });
                c |= ui.add(egui::Slider::new(&mut set.amplitude, -10.0..=10.0).text("Height")).changed();
                c |= ui.add(egui::Slider::new(&mut set.element_size, 0.05..=100.0).logarithmic(true).text("Element size")).on_hover_text("Size of the largest features").changed();
                c |= ui.add(egui::Slider::new(&mut set.octaves, 1..=12).text("Octaves")).changed();
                c |= ui.add(egui::Slider::new(&mut set.roughness, 0.0..=1.0).text("Roughness")).changed();
                ui.horizontal(|ui| {
                    ui.label("Offset");
                    c |= ui.add(egui::DragValue::new(&mut set.offset[0]).speed(0.05)).changed();
                    c |= ui.add(egui::DragValue::new(&mut set.offset[1]).speed(0.05)).changed();
                });
                c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
                c |= ui.checkbox(&mut set.positive, "Positive only (0 to Height)").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").on_hover_text("Uses the `mask` attribute from Terrain Mask or Attribute Paint").changed();
            }
            NodeKind::TerrainErode(set) => {
                ui.small("Hydraulic erosion carves rivers and gullies; thermal erosion crumbles steep slopes. Writes `flow` and `debris` for Terrain Mask.");
                ui.label("Hydraulic");
                c |= ui.add(egui::Slider::new(&mut set.droplets, 0..=2_000_000).logarithmic(true).text("Raindrops")).changed();
                c |= ui.add(egui::Slider::new(&mut set.erosion, 0.0..=1.0).text("Erosion rate")).changed();
                c |= ui.add(egui::Slider::new(&mut set.deposition, 0.0..=1.0).text("Deposition rate")).changed();
                c |= ui.add(egui::Slider::new(&mut set.capacity, 0.0..=20.0).text("Sediment capacity")).changed();
                c |= ui.add(egui::Slider::new(&mut set.evaporation, 0.0..=0.5).text("Evaporation")).changed();
                c |= ui.add(egui::Slider::new(&mut set.inertia, 0.0..=0.95).text("Inertia")).on_hover_text("Higher: straighter, wider rivers").changed();
                c |= ui.add(egui::Slider::new(&mut set.radius, 1..=8).text("Erosion radius (cells)")).changed();
                ui.label("Thermal");
                c |= ui.add(egui::Slider::new(&mut set.thermal_iterations, 0..=500).text("Iterations")).changed();
                c |= ui.add(egui::Slider::new(&mut set.talus_angle, 1.0..=89.0).text("Talus angle°")).changed();
                c |= ui.add(egui::Slider::new(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::TerrainTerrace(set) => {
                c |= ui.add(egui::Slider::new(&mut set.step, 0.01..=10.0).logarithmic(true).text("Step height")).changed();
                c |= ui.add(egui::Slider::new(&mut set.offset, -5.0..=5.0).text("Offset")).changed();
                c |= ui.add(egui::Slider::new(&mut set.sharpness, 0.0..=1.0).text("Sharpness")).on_hover_text("1: flat steps with sheer walls").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").changed();
            }
            NodeKind::TerrainMask(set) => {
                ui.small("Writes a 0–1 `mask` point attribute for Terrain Noise, Terrace or any other node.");
                egui::ComboBox::from_label("From").selected_text(terrain::MASK_SOURCES[set.source.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_SOURCES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.source, value, *label).changed();
                    }
                });
                let unit = match set.source {
                    0 => "height",
                    1 => "slope°",
                    _ => "value",
                };
                c |= ui.add(egui::DragValue::new(&mut set.min).speed(0.05).prefix(format!("min {unit} "))).changed();
                c |= ui.add(egui::DragValue::new(&mut set.max).speed(0.05).prefix(format!("max {unit} "))).changed();
                c |= ui.add(egui::Slider::new(&mut set.falloff, 0.0..=50.0).text("Falloff")).changed();
                c |= ui.add(egui::Slider::new(&mut set.blur, 0..=100).text("Blur passes")).changed();
                egui::ComboBox::from_label("Combine with existing mask").selected_text(terrain::MASK_COMBINE[set.combine.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_COMBINE.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                c |= ui.checkbox(&mut set.visualize, "Show in viewport (Cd)").changed();
            }
            NodeKind::TerrainColorize(set) => {
                ui.small("Colours the terrain (Cd) by height and slope, for the viewport and MoonRay.");
                for (label, colour) in [("Sand", &mut set.sand), ("Grass", &mut set.grass), ("Rock", &mut set.rock), ("Snow", &mut set.snow)] {
                    ui.horizontal(|ui| {
                        c |= ui.color_edit_button_rgb(colour).changed();
                        ui.label(label);
                    });
                }
                c |= ui.add(egui::Slider::new(&mut set.sand_level, 0.0..=1.0).text("Sand up to")).changed();
                c |= ui.add(egui::Slider::new(&mut set.snow_level, 0.0..=1.0).text("Snow from")).changed();
                c |= ui.add(egui::Slider::new(&mut set.rock_slope, 0.0..=90.0).text("Rock slope°")).changed();
                c |= ui.add(egui::Slider::new(&mut set.blend, 0.0..=0.5).text("Blend")).changed();
            }
            NodeKind::UvTexture(set) => {
                egui::ComboBox::from_label("Projection").selected_text(sops::UV_PROJECTIONS[set.projection.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in sops::UV_PROJECTIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.projection, value, *label).changed();
                    }
                });
                if matches!(set.projection, 0 | 1) {
                    ui.horizontal(|ui| {
                        ui.label(if set.projection == 0 { "Project along" } else { "Cylinder axis" });
                        for (value, label) in sops::AXES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Scale");
                    c |= ui.add(egui::DragValue::new(&mut set.scale[0]).speed(0.01)).changed();
                    c |= ui.add(egui::DragValue::new(&mut set.scale[1]).speed(0.01)).changed();
                    ui.label("Offset");
                    c |= ui.add(egui::DragValue::new(&mut set.offset[0]).speed(0.01)).changed();
                    c |= ui.add(egui::DragValue::new(&mut set.offset[1]).speed(0.01)).changed();
                });
                ui.small("Writes vertex:uv. Follow with UV Quick Shade to see them.");
            }
            NodeKind::UvTransform(set) => {
                for (label, value) in [("Translate", &mut set.translate), ("Scale", &mut set.scale), ("Pivot", &mut set.pivot)] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        c |= ui.add(egui::DragValue::new(&mut value[0]).speed(0.01)).changed();
                        c |= ui.add(egui::DragValue::new(&mut value[1]).speed(0.01)).changed();
                    });
                }
                c |= ui.add(egui::Slider::new(&mut set.rotate, -180.0..=180.0).text("Rotate")).changed();
            }
            NodeKind::UvQuickShade { file } => {
                ui.small("Shows a texture on the UVs in the viewport. Leave the file empty for a checker grid.");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("checker grid").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg", "tga", "bmp", "exr", "tif", "tiff"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                    if !file.is_empty() && ui.button("Clear").clicked() {
                        file.clear();
                        c = true;
                    }
                });
            }
            NodeKind::VellumSolver(set) => {
                ui.heading("Vellum Solver");
                ui.small("Input 1: geometry (a mesh for cloth/softbody, curves for hair, points for grains) • Input 2: optional static collider. Pin points with f@pin = 1; hair roots are pinned automatically.");
                ui.horizontal(|ui| {
                    ui.label("Type");
                    for (value, label) in vellum::VELLUM_TYPES.iter().enumerate() {
                        if ui.selectable_label(set.kind == value, *label).clicked() && set.kind != value {
                            // Switching type loads that type's typical settings.
                            *set = vellum::VellumSettings::defaults(value);
                            c = true;
                        }
                    }
                });
                ui.strong("Constraints");
                if set.kind != 3 {
                    c |= ui.add(egui::Slider::new(&mut set.stretch_stiffness, 1.0..=1e8).logarithmic(true).text("Stretch stiffness")).changed();
                }
                if matches!(set.kind, 0 | 1) {
                    c |= ui.add(egui::Slider::new(&mut set.bend_stiffness, 1e-3..=1e5).logarithmic(true).text("Bend stiffness")).changed();
                }
                if set.kind == 2 {
                    c |= ui.add(egui::Slider::new(&mut set.shape_stiffness, 0.0..=1.0).text("Shape stiffness")).changed();
                }
                c |= ui.add(egui::Slider::new(&mut set.mass, 1e-3..=100.0).logarithmic(true).text("Mass per point")).changed();
                let thickness_label = if set.kind == 3 { "Grain radius (pscale overrides)" } else { "Thickness" };
                c |= ui.add(egui::Slider::new(&mut set.thickness, 1e-3..=0.5).logarithmic(true).text(thickness_label)).changed();
                ui.strong("Forces and collisions");
                c |= ui.add(egui::Slider::new(&mut set.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= ui.add(egui::Slider::new(&mut set.damping, 0.0..=5.0).text("Damping")).changed();
                c |= ui.add(egui::Slider::new(&mut set.friction, 0.0..=1.0).text("Friction")).changed();
                c |= ui.checkbox(&mut set.ground, "Ground plane (y = 0)").changed();
                c |= ui.checkbox(&mut set.self_collision, "Self collision").on_hover_text("Point-based: keeps points apart by their thickness").changed();
                c |= ui
                    .checkbox(&mut set.pin_to_animation, "Pins follow animation")
                    .on_hover_text("Pinned points track the animated input (e.g. hair roots on a moving head)")
                    .changed();
                ui.strong("Solver");
                c |= ui
                    .add(egui::Slider::new(&mut set.substeps, 1..=100).text("Substeps"))
                    .on_hover_text("Raise this first if cloth stretches: small steps converge better than extra iterations")
                    .changed();
                c |= ui.add(egui::Slider::new(&mut set.iterations, 1..=200).text("Constraint iterations")).changed();
            }
            NodeKind::RbdSolver(set) => {
                ui.heading("RBD Bullet Solver");
                ui.small("Input 1: pieces — each connected part (or each value of a piece point attribute) is one rigid body; points become small spheres • Input 2: optional static collider.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Collision shape");
                    for (value, label) in sims::RbdSettings::SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.shape, value, *label).changed();
                    }
                });
                if set.shape == 1 {
                    ui.weak("Convex decomposition fits concave pieces but is slow to build.");
                }
                c |= ui.add(egui::Slider::new(&mut set.density, 0.01..=10.0).logarithmic(true).text("Density")).changed();
                c |= ui.add(egui::Slider::new(&mut set.friction, 0.0..=2.0).text("Friction")).changed();
                c |= ui.add(egui::Slider::new(&mut set.bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= ui.add(egui::Slider::new(&mut set.gravity, -20.0..=30.0).text("Gravity")).changed();
                c |= ui.add(egui::Slider::new(&mut set.substeps, 1..=16).text("Substeps")).changed();
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.ground, "Ground plane").changed();
                    if set.ground {
                        c |= ui.add(egui::DragValue::new(&mut set.ground_height).speed(0.05).prefix("Height ")).changed();
                    }
                });
                c |= vec3_ui(ui, "Initial velocity", &mut set.velocity, 0.05);
                c |= vec3_ui(ui, "Initial spin (rad/s)", &mut set.spin, 0.05);
                ui.small("A point attribute v adds per-piece initial velocity. Output adds point attributes v and piece.");
            }
            NodeKind::SopSolver { substeps } => {
                ui.heading("SOP Solver");
                if solver_display_is_input {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 190, 90),
                        "⚠ The display flag inside this solver is on Prev_Frame/Input_1, so each step changes nothing. Enter the network and set the display flag (double-click) on your last node.",
                    );
                }
                ui.label("Each step cooks this node's network with Prev_Frame set to the previous step's result, so changes accumulate over time. Input_1 is the solver's input at the current step.");
                c |= ui.add(egui::Slider::new(substeps, 1..=16).text("Substeps per frame")).changed();
                if ui.button("Enter solver network").clicked() {
                    self.current_network = Some(id);
                    self.context = AppContext::Obj;
                    self.selected = None;
                    self.graph_pan = egui::Vec2::ZERO;
                    c = true;
                }
                ui.small("At time 0 the output is the input. Example: Prev_Frame → Wrangle (@P.y += 0.02;) with the display flag on the wrangle.");
            }
            NodeKind::SolverInput { index } => {
                ui.heading(if *index == 0 { "Prev_Frame" } else { "Input_1" });
                ui.horizontal(|ui| {
                    c |= ui.selectable_value(index, 0, "Prev_Frame").changed();
                    c |= ui.selectable_value(index, 1, "Input_1").changed();
                });
                ui.small("Prev_Frame is the solver's result from the previous step; Input_1 is the geometry wired into the SOP Solver, at the current step time.");
            }
            NodeKind::LopSopImport { geometry, path } => {
                ui.heading("SOP Import");
                ui.small("Brings OBJ geometry objects into the stage as Mesh prims: attributes become primvars (Cd → displayColor) and the object's MAT material is bound.");
                egui::ComboBox::from_id_salt("sop_import_geometry")
                    .selected_text(geometry.map_or("All geometry objects".to_string(), |g| format!("geometry{g}")))
                    .show_ui(ui, |ui| {
                        c |= ui.selectable_value(geometry, None, "All geometry objects").changed();
                        for g in &geometry_objects {
                            c |= ui.selectable_value(geometry, Some(*g), format!("geometry{g}")).changed();
                        }
                    });
                c |= prim_path_ui(ui, if geometry.is_some() { "Prim path" } else { "Parent path" }, path);
            }
            NodeKind::LopReference { file, path, sublayer } => {
                ui.heading("Reference / Sublayer");
                if !usd::NATIVE {
                    ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "This build has no OpenUSD (native-usd feature), so USD files cannot be read.");
                }
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/asset.usd").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("USD", &["usd", "usda", "usdc", "usdz"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    c |= ui.selectable_value(sublayer, false, "Reference under a prim").changed();
                    c |= ui.selectable_value(sublayer, true, "Sublayer at the root").changed();
                });
                if !*sublayer {
                    c |= prim_path_ui(ui, "Prim path", path);
                }
                if ui.button("Reload file").on_hover_text("Read the file again after it changed on disk").clicked() {
                    usd::forget_loaded_files();
                    c = true;
                }
                ui.small("Opens .usd, .usda, .usdc and .usdz with full composition; animated files follow the timeline.");
            }
            NodeKind::LopPrimitive { shape, path, size, t } => {
                ui.heading("Primitive");
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in LOP_SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(shape, value, *label).changed();
                    }
                });
                c |= prim_path_ui(ui, "Prim path", path);
                c |= ui.add(egui::Slider::new(size, 0.01..=10.0).logarithmic(true).text("Size (radius)")).changed();
                c |= vec3_ui(ui, "Translate", t, 0.05);
            }
            NodeKind::LopTransform { pattern, t, r, s } => {
                ui.heading("Transform");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                c |= vec3_ui(ui, "Translate", t, 0.05);
                c |= vec3_ui(ui, "Rotate°", r, 0.5);
                c |= vec3_ui(ui, "Scale", s, 0.02);
                ui.small("Applied in each matching prim's parent space, after its own transform.");
            }
            NodeKind::LopLight { kind, path, position, target, color, intensity, size } => {
                ui.heading("Light");
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in ["Distant", "Sphere", "Dome", "Rect"].iter().enumerate() {
                        c |= ui.selectable_value(kind, value, *label).changed();
                    }
                });
                c |= prim_path_ui(ui, "Prim path", path);
                if *kind != 2 {
                    c |= vec3_ui(ui, "Position", position, 0.05);
                    c |= vec3_ui(ui, "Aim at", target, 0.05);
                }
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                c |= ui.add(egui::Slider::new(intensity, 0.0..=20.0).text("Intensity")).changed();
                let size_label = match *kind {
                    0 => Some("Angle (degrees)"),
                    1 => Some("Radius"),
                    3 => Some("Width / height"),
                    _ => None,
                };
                if let Some(label) = size_label {
                    c |= ui.add(egui::Slider::new(size, 0.0..=20.0).text(label)).changed();
                }
            }
            NodeKind::LopCamera { path, position, target, focal, aperture } => {
                ui.heading("Camera");
                c |= prim_path_ui(ui, "Prim path", path);
                c |= vec3_ui(ui, "Position", position, 0.05);
                c |= vec3_ui(ui, "Look at", target, 0.05);
                c |= ui.add(egui::Slider::new(focal, 8.0..=300.0).logarithmic(true).text("Focal length (mm)")).changed();
                c |= ui.add(egui::Slider::new(aperture, 5.0..=70.0).text("Horizontal aperture (mm)")).changed();
                if ui.button("Copy from viewport").clicked() {
                    (*position, *target) = viewport_view;
                    c = true;
                }
                ui.small("Rendering the STAGE with the Auto camera uses the first stage camera.");
            }
            NodeKind::LopMaterial { pattern, material } => {
                ui.heading("Assign Material");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                let selected = material
                    .and_then(|m| material_ids.iter().find(|(id, _)| *id == m))
                    .map_or("Choose a material".to_string(), |(_, name)| name.clone());
                egui::ComboBox::from_id_salt("lop_material").selected_text(selected).show_ui(ui, |ui| {
                    for (mid, name) in &material_ids {
                        c |= ui.selectable_value(material, Some(*mid), name).changed();
                    }
                });
                ui.small("MAT materials become UsdPreviewSurface materials under /materials; MoonRay renders them with the full MAT settings.");
            }
            NodeKind::LopPrune { pattern, hide } => {
                ui.heading("Prune");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                c |= ui.checkbox(hide, "Only hide (keep the prims active)").changed();
            }
            NodeKind::LopMerge => {
                ui.heading("Merge");
                ui.label("Layers the second input's prims over the first input's stage.");
            }
            NodeKind::LopExport { file } => {
                ui.heading("USD Export");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/scene.usda").desired_width(220.0)).changed();
                    if ui.button("Choose…").clicked() {
                        if let Some(chosen) = FileDialog::new().set_file_name("scene.usda").add_filter("USD", &["usda", "usdc", "usd", "usdz"]).save_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if ui.add_enabled(!file.trim().is_empty(), egui::Button::new("💾 Save USD")).clicked() {
                    self.usd_export_request = Some(id);
                }
                ui.small(if usd::NATIVE {
                    "The extension picks the format: .usda (text), .usdc or .usd (binary), .usdz (package)."
                } else {
                    "Saves .usda; .usdc and .usdz need the native-usd build."
                });
            }
            NodeKind::ConvertVdb { mode, iso } => {
                ui.heading("Convert VDB");
                ui.horizontal(|ui| {
                    ui.label("Convert to");
                    c |= ui.selectable_value(mode, 0, "Polygons").changed();
                    c |= ui.selectable_value(mode, 1, "Volume (fog)").changed();
                });
                if *mode == 0 {
                    c |= ui.add(egui::Slider::new(iso, -1.0..=1.0).text("Isovalue")).changed();
                    ui.small("Level sets: distance from the surface (0 = the surface). Fog volumes and Pyro: density threshold, e.g. 0.5.");
                } else {
                    ui.small("Level sets become fog densities (1 inside); fog volumes pass through. Drawn as a soft volume.");
                }
                ui.small("Input: a VDB from VDB from Polygons, a .vdb file read with the File node, or Pyro smoke.");
            }
            NodeKind::FileImport { file } => {
                ui.heading("File");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/model.$F4.obj").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new()
                            .add_filter("Geometry", &["obj", "fbx", "stl", "usd", "usda", "usdc", "usdz", "vdb"])
                            .pick_file()
                        {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if fileio::has_frame_token(file) {
                    ui.weak(format!("Frame now: {}", fileio::expand_frame(file, self.time, sims::SIM_FPS)));
                }
                if ui.button("Reload").on_hover_text("Read the file again after it changed on disk").clicked() {
                    usd::forget_loaded_files();
                    c = true;
                }
                ui.small("Reads .obj, .fbx, .stl, USD (.usd .usda .usdc .usdz) and .vdb. $F / $F4 insert the frame number for sequences. Separate objects get a piece attribute.");
            }
            NodeKind::FileExport { file } => {
                ui.heading("File Export");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/out.$F4.fbx").desired_width(220.0)).changed();
                    if ui.button("Choose…").clicked() {
                        if let Some(chosen) = FileDialog::new()
                            .set_file_name("geometry.obj")
                            .add_filter("Geometry", &["obj", "fbx", "stl", "usd", "usda", "usdc", "vdb"])
                            .save_file()
                        {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(!file.trim().is_empty(), egui::Button::new("💾 Write current frame")).clicked() {
                        self.file_export_request = Some((id, false));
                    }
                    let can_range = fileio::has_frame_token(file);
                    if ui
                        .add_enabled(can_range, egui::Button::new("Write timeline range"))
                        .on_disabled_hover_text("Add $F or $F4 to the file name to write one file per frame")
                        .clicked()
                    {
                        self.file_export_request = Some((id, true));
                    }
                });
                ui.small("The extension picks the format: .obj, .fbx, .stl, .usd/.usda/.usdc or .vdb (VDB input). The geometry passes through unchanged.");
            }
            NodeKind::Merge | NodeKind::Null => {
                ui.label("No parameters.");
            }
        }

        let targets = self.graph.nodes.get(&id).map(|n| n.kind.animatable()).unwrap_or_default();
        if !targets.is_empty() {
            let animated = self.graph.is_animated(id);
            egui::CollapsingHeader::new(if animated { "Animation ◆" } else { "Animation" })
                .id_salt(("animation", id))
                .default_open(animated)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("◆ Key all").on_hover_text("Key every parameter below at this frame").clicked() {
                            for (target, _) in &targets {
                                self.graph.set_key(ChannelId { node: id, target: *target }, self.time);
                            }
                            c = true;
                        }
                        if animated && ui.button("Remove animation").clicked() {
                            self.graph.channels.retain(|ch, _| ch.node != id);
                            c = true;
                        }
                        if ui.button("📈 Graph Editor").clicked() {
                            self.pane_requests.push(PaneRequest::Show(Pane::GraphEditor, true));
                            self.graph_editor.frame_all();
                        }
                    });
                    egui::Grid::new(("animation_grid", id)).num_columns(3).striped(true).show(ui, |ui| {
                        for (target, name) in &targets {
                            let channel_id = ChannelId { node: id, target: *target };
                            let value = self.graph.channel_value(channel_id).unwrap_or(0.0);
                            let state = self.graph.channels.get(&channel_id).map(|ch| ch.has_key_at(self.time));
                            let (symbol, hover) = match state {
                                Some(true) => ("◆", "Key at this frame: click to delete it"),
                                Some(false) => ("◇", "Animated: click to key this frame"),
                                None => ("○", "Click to animate: keys this frame"),
                            };
                            let colour = match state {
                                Some(true) => egui::Color32::from_rgb(255, 200, 60),
                                Some(false) => egui::Color32::from_rgb(120, 200, 120),
                                None => egui::Color32::GRAY,
                            };
                            if ui.add(egui::Button::new(egui::RichText::new(symbol).color(colour)).small()).on_hover_text(hover).clicked() {
                                if state == Some(true) {
                                    if let Some(channel) = self.graph.channels.get_mut(&channel_id) {
                                        channel.remove_key_at(self.time);
                                        if channel.keys.is_empty() {
                                            self.graph.channels.remove(&channel_id);
                                        }
                                    }
                                } else {
                                    self.graph.set_key(channel_id, self.time);
                                }
                                c = true;
                            }
                            ui.label(name);
                            ui.monospace(format!("{value:.3}"));
                            ui.end_row();
                        }
                    });
                    ui.small("Edits to animated parameters (panel or gizmo) key the current frame automatically.");
                });
        }
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_lop()) {
            // Cook the stage first so its errors below are current.
            self.scene_graph_ui(ui, id);
        }
        if let Some(error) = node_errors(|errors| errors.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {error}"));
        }
        if let Some(note) = node_notes(|notes| notes.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 190, 90), format!("⚠ {note}"));
        }
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_solver()) {
            ui.add_space(4.0);
            if ui.button("Reset simulations").on_hover_text("Forget cached solver steps and frames, freeing their memory, and simulate again from the start").clicked() {
                sims::clear_all();
                self.frame_cache.clear();
                self.frame_cache_order.clear();
                c = true;
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if !is_object && ui.button("Set Display").clicked() {
                self.graph.set_display(id);
                c = true;
            }
            if ui.button("Duplicate").clicked() {
                self.selected = self.graph.duplicate(id).or(Some(id));
                c = true;
            }
        });
        c
    }

    // ---------------- Network editor ----------------
    fn node_editor(&mut self, ui: &mut egui::Ui) {
        let view_parent = self.view_parent();
        if self.context == AppContext::Obj || self.network().is_some() {
            let root = if self.context == AppContext::Mat { "MAT" } else { "OBJ" };
            ui.horizontal(|ui| {
                if ui.selectable_label(self.network().is_none(), root).clicked() && self.network().is_some() {
                    self.enter_network(None);
                }
                if let Some(open) = self.network() {
                    if let Some(outer) = self.parent_network() {
                        ui.label("›");
                        let outer_label = self.graph.nodes.get(&outer).map_or_else(String::new, |n| n.kind.label(outer));
                        if ui.selectable_label(false, outer_label).clicked() {
                            self.enter_network(Some(outer));
                        }
                    }
                    ui.label("›");
                    ui.strong(self.graph.nodes.get(&open).map_or_else(String::new, |n| n.kind.label(open)));
                    if ui.small_button("⬆ Up").on_hover_text("Back to the containing network (U)").clicked() {
                        let outer = self.parent_network();
                        self.enter_network(outer);
                    }
                }
            });
        }
        let (canvas, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        painter.rect_filled(canvas.rect, 0.0, egui::Color32::from_gray(30));
        let origin = canvas.rect.min.to_vec2() + self.graph_pan;
        if canvas.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.0 {
                if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
                    let graph_point = (pointer.to_vec2() - origin) / self.graph_zoom;
                    self.graph_zoom = (self.graph_zoom * (1.0 + scroll * 0.001)).clamp(0.35, 2.5);
                    self.graph_pan = pointer.to_vec2()
                        - canvas.rect.min.to_vec2()
                        - graph_point * self.graph_zoom;
                }
            }
        }
        let mut actions: Vec<Action> = Vec::new();

        // Background dot grid
        let step = 40.0 * self.graph_zoom;
        let off = egui::vec2(self.graph_pan.x.rem_euclid(step), self.graph_pan.y.rem_euclid(step));
        let mut y = canvas.rect.top() + off.y;
        while y < canvas.rect.bottom() {
            let mut x = canvas.rect.left() + off.x;
            while x < canvas.rect.right() {
                painter.circle_filled(egui::pos2(x, y), 1.0, egui::Color32::from_gray(55));
                x += step;
            }
            y += step;
        }

        if canvas.dragged_by(egui::PointerButton::Middle)
            || (canvas.dragged_by(egui::PointerButton::Primary) && self.dragging_link.is_none())
        {
            self.graph_pan += canvas.drag_delta();
        }
        if canvas.clicked() {
            self.selected = None;
        }
        if canvas.secondary_clicked() {
            if let Some(p) = canvas.interact_pointer_pos() {
                self.menu_pos = p;
            }
        }
        let menu_pos = self.menu_pos;
        let templates = self.templates_for_view();
        canvas.clone().context_menu(|ui| {
            for (cat, kinds) in templates {
                ui.menu_button(cat, |ui| {
                    for kind in kinds {
                        let button = ui.button(kind.menu_name());
                        let button = match kind.menu_help() {
                            Some(help) => button.on_hover_text(help),
                            None => button,
                        };
                        if button.clicked() {
                            actions.push(Action::Add(
                                kind,
                                egui::pos2(
                                    (menu_pos.x - origin.x) / self.graph_zoom,
                                    (menu_pos.y - origin.y) / self.graph_zoom,
                                ),
                            ));
                            ui.close_menu();
                        }
                    }
                });
            }
        });

        // Wires (only within current context)
        for l in &self.graph.links {
            if let (Some(a), Some(b)) = (self.graph.nodes.get(&l.from), self.graph.nodes.get(&l.to)) {
                if a.context != self.context || b.context != self.context { continue; }
                if a.parent != view_parent || b.parent != view_parent { continue; }
                let p0 = out_port(node_rect(a.pos, origin, self.graph_zoom));
                let p1 = in_port(node_rect(b.pos, origin, self.graph_zoom), l.slot, b.kind.num_inputs());
                draw_wire(&painter, p0, p1, egui::Color32::from_gray(170));
            }
        }

        let pointer = ui.input(|i| i.pointer.hover_pos());
        let released = ui.input(|i| i.pointer.any_released());
        let mut new_link: Option<(NodeId, NodeId, usize)> = None;
        let mut disconnects: Vec<(NodeId, usize)> = Vec::new();

        let mut ids: Vec<NodeId> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| n.context == self.context && n.parent == view_parent)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();

        for id in ids {
            let rect = node_rect(self.graph.nodes[&id].pos, origin, self.graph_zoom);
            let resp = ui.interact(rect, ui.id().with(("node", id)), egui::Sense::click_and_drag());
            let resp = match self.node_stats.get(&id) {
                Some((p, t)) => resp.on_hover_text(format!("{p} points, {t} triangles")),
                None => resp.on_hover_text("Not cooked (not upstream of display/template)"),
            };
            if resp.dragged_by(egui::PointerButton::Primary) {
                if let Some(n) = self.graph.nodes.get_mut(&id) {
                    n.pos += resp.drag_delta() / self.graph_zoom;
                }
            }
            if resp.clicked() || resp.drag_started() {
                self.selected = Some(id);
            }
            if resp.middle_clicked() {
                self.selected = Some(id);
                self.inspect_node = Some(id);
            }
            if resp.double_clicked() {
                if self.graph.nodes[&id].kind.is_container() {
                    actions.push(Action::Enter(id));
                } else {
                    actions.push(Action::Display(id));
                }
            }
            let is_object = self.graph.nodes[&id].kind.is_object();
            let is_geometry = self.graph.nodes[&id].kind.is_container();
            resp.clone().context_menu(|ui| {
                if is_geometry && ui.button("Enter (double-click)").clicked() {
                    actions.push(Action::Enter(id));
                    ui.close_menu();
                }
                let display_label = if is_object { "Toggle Active / Activate Camera" } else { "Set Display Flag" };
                if ui.button(display_label).clicked() {
                    actions.push(Action::Display(id));
                    ui.close_menu();
                }
                if !is_object && ui.button("Toggle Bypass (B)").clicked() {
                    actions.push(Action::Bypass(id));
                    ui.close_menu();
                }
                if ui.button("Duplicate (Ctrl+D)").clicked() {
                    actions.push(Action::Duplicate(id));
                    ui.close_menu();
                }
                if ui.button("Delete").clicked() {
                    actions.push(Action::Delete(id));
                    ui.close_menu();
                }
            });

            let node = &self.graph.nodes[&id];
            let rect = node_rect(node.pos, origin, self.graph_zoom);
            let n_in = node.kind.num_inputs();

            let op = out_port(rect);
            let op_resp = ui.interact(
                egui::Rect::from_center_size(op, egui::vec2(16.0, 16.0)),
                ui.id().with(("out", id)),
                egui::Sense::drag(),
            );
            if op_resp.drag_started() {
                self.dragging_link = Some(id);
            }

            for slot in 0..n_in {
                let ip = in_port(rect, slot, n_in);
                let ip_resp = ui.interact(
                    egui::Rect::from_center_size(ip, egui::vec2(16.0, 16.0)),
                    ui.id().with(("in", id, slot)),
                    egui::Sense::click(),
                );
                let ip_resp = match &self.graph.nodes[&id].kind {
                    NodeKind::Vop(v) => match v.def().inputs.get(slot) {
                        Some((input_name, default)) => ip_resp.on_hover_text(format!("{input_name} (unconnected: {default})")),
                        None => ip_resp,
                    },
                    NodeKind::ShaderNode(n) => match n.def().inputs.get(slot) {
                        Some((input_name, ..)) => ip_resp.on_hover_text(*input_name),
                        None => ip_resp,
                    },
                    _ => ip_resp,
                };
                if ip_resp.clicked() {
                    disconnects.push((id, slot));
                }
                if let (Some(src), Some(p)) = (self.dragging_link, pointer) {
                    if released && p.distance(ip) < 16.0 {
                        new_link = Some((src, id, slot));
                    }
                }
            }

            // Draw
            let sel = self.selected == Some(id);
            let disp = if node.kind.is_object() {
                match node.kind {
                    NodeKind::Camera { .. } => self.active_camera == Some(id),
                    _ => !node.bypass,
                }
            } else {
                self.graph.is_display(id)
            };
            let body = if node.bypass {
                egui::Color32::from_rgb(90, 80, 30)
            } else {
                egui::Color32::from_gray(58)
            };
            painter.rect(
                rect,
                egui::CornerRadius::same(6),
                body,
                egui::Stroke::new(
                    if sel { 2.0 } else { 1.0 },
                    if sel { egui::Color32::from_rgb(255, 200, 60) } else { egui::Color32::from_gray(95) },
                ),
                egui::StrokeKind::Outside,
            );
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(8.0, rect.height())),
                egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 },
                node.kind.color(),
            );
            if disp {
                painter.rect_filled(
                    egui::Rect::from_min_size(rect.right_top() - egui::vec2(10.0, 0.0), egui::vec2(10.0, rect.height())),
                    egui::CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 },
                    egui::Color32::from_rgb(60, 140, 255),
                );
            }
            if self.template == Some(id) {
                painter.rect_stroke(
                    rect.shrink(3.0),
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 80, 200)),
                    egui::StrokeKind::Outside,
                );
            }
            if self.graph.time_dependent(id) {
                painter.circle_filled(
                    rect.left_top() + egui::vec2(16.0, 8.0),
                    3.0,
                    egui::Color32::from_rgb(120, 230, 120),
                );
            }
            if let Some((color, ..)) = node.kind.is_material().then(|| self.graph.material_preview(id)).flatten() {
                let swatch = egui::Color32::from_rgb(
                    (color[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[2].clamp(0.0, 1.0) * 255.0) as u8,
                );
                painter.circle_filled(rect.right_center() - egui::vec2(14.0 * self.graph_zoom, 0.0), 6.0 * self.graph_zoom, swatch);
            }
            let label = format!("{}{}", node.kind.label(id), if node.bypass { " (byp)" } else { "" });
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
            for slot in 0..n_in {
                let ip = in_port(rect, slot, n_in);
                let hot = self.dragging_link.is_some() && pointer.map_or(false, |p| p.distance(ip) < 16.0);
                painter.circle_filled(
                    ip,
                    if hot { 7.0 } else { 5.0 },
                    if hot { egui::Color32::YELLOW } else { egui::Color32::from_gray(200) },
                );
            }
            painter.circle_filled(op, 5.0, egui::Color32::from_gray(200));
        }

        for (id, slot) in disconnects {
            self.graph.disconnect(id, slot);
            self.dirty = true;
        }

        if let (Some(src), Some(p)) = (self.dragging_link, pointer) {
            if let Some(n) = self.graph.nodes.get(&src) {
                draw_wire(
                    &painter,
                    out_port(node_rect(n.pos, origin, self.graph_zoom)),
                    p,
                    egui::Color32::YELLOW,
                );
            }
        }
        if released {
            if let Some((a, b, s)) = new_link {
                if self.graph.connect(a, b, s) {
                    self.dirty = true;
                } else {
                    self.status = "Connection refused: would create a cycle".into();
                }
            }
            self.dragging_link = None;
        }

        let pointer_in = ui.rect_contains_pointer(canvas.rect);
        let typing = ui.ctx().wants_keyboard_input();

        // U: leave the open geometry
        if pointer_in && !typing && self.network().is_some() && ui.input(|i| i.key_pressed(egui::Key::U)) {
            let outer = self.parent_network();
            self.enter_network(outer);
            return;
        }

        // H: home the network view
        if pointer_in && !typing && ui.input(|i| i.key_pressed(egui::Key::H)) {
            if let Some(min) = self.graph.nodes.values().map(|n| n.pos).reduce(|a, b| a.min(b)) {
                self.graph_pan = egui::vec2(20.0, 20.0) - min.to_vec2() * self.graph_zoom;
            }
        }

        // TAB: searchable add-node menu (auto-wires below the selected node)
        if pointer_in && !typing && ui.input(|i| i.key_pressed(egui::Key::Tab)) {
            self.tab_open = true;
            self.tab_query.clear();
            self.tab_pos = pointer.unwrap_or(canvas.rect.center());
        }
        if self.tab_open {
            let q = self.tab_query.to_lowercase();
            let matches: Vec<NodeKind> = self
                .templates_for_view()
                .into_iter()
                .flat_map(|(_, k)| k)
                .filter(|k| k.name().contains(&q) || k.menu_name().to_lowercase().contains(&q))
                .collect();
            let selected = self.selected.filter(|id| self.graph.nodes.contains_key(id));
            let tab_pos = self.tab_pos;
            let mut close = false;
            egui::Area::new(egui::Id::new("tab_menu"))
                .fixed_pos(tab_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(170.0);
                        ui.text_edit_singleline(&mut self.tab_query).request_focus();
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for (i, k) in matches.iter().enumerate() {
                                if ui.button(k.menu_name()).clicked() || (enter && i == 0) {
                                    actions.push(match selected {
                                        Some(from) => Action::AddBelow(k.clone(), from),
                                        None => Action::Add(k.clone(), tab_pos - origin),
                                    });
                                    close = true;
                                }
                            }
                        });
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                    });
                });
            if close {
                self.tab_open = false;
            }
        }

        for a in actions {
            self.apply(a);
        }
    }

    // ---------------- 3D viewport ----------------
    fn viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        painter.rect_filled(response.rect, 0.0, egui::Color32::from_gray(self.viewport_bg));

        // Restrict painter rendering strictly to the viewport rectangle
        let painter = painter.with_clip_rect(response.rect);

        let box_select_modifier = ctx.input(|i| i.modifiers.shift);
        // Gizmo handles are hit-tested against where they were drawn last frame.
        if response.drag_started_by(egui::PointerButton::Primary) && !box_select_modifier {
            if let (Some(frame), Some(pointer)) = (self.gizmo_frame.clone(), response.interact_pointer_pos()) {
                if let (Some(part), Some((node, xform))) = (frame.hit(pointer).cloned(), self.gizmo_target()) {
                    if node == frame.node {
                        self.gizmo_drag = Some(GizmoDrag { part, frame, start_pointer: pointer, start: xform });
                    }
                }
            }
        }
        if let Some(drag) = self.gizmo_drag.clone() {
            if let Some(pointer) = ctx.input(|i| i.pointer.latest_pos()) {
                self.apply_gizmo_drag(&drag, pointer);
            }
            if !ctx.input(|i| i.pointer.primary_down()) {
                self.gizmo_drag = None;
            }
        }
        if response.hovered() && !ctx.wants_keyboard_input() {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::W) {
                    self.gizmo_mode = GizmoMode::Translate;
                } else if i.key_pressed(egui::Key::E) {
                    self.gizmo_mode = GizmoMode::Rotate;
                } else if i.key_pressed(egui::Key::R) {
                    self.gizmo_mode = GizmoMode::Scale;
                }
            });
        }
        let paint_node = self
            .selected
            .filter(|id| self.paint_active && matches!(self.graph.nodes.get(id).map(|n| &n.kind), Some(NodeKind::AttributePaint { .. })));
        if response.dragged_by(egui::PointerButton::Primary) && !box_select_modifier && self.gizmo_drag.is_none() && paint_node.is_none() {
            self.cam_rot_y += response.drag_delta().x * 0.01;
            self.cam_rot_x = (self.cam_rot_x + response.drag_delta().y * 0.01).clamp(-1.55, 1.55);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.cam_pan_x += response.drag_delta().x;
            self.cam_pan_y += response.drag_delta().y;
        }
        if response.hovered() {
            let scroll = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                self.cam_zoom = (self.cam_zoom * (1.0 - scroll * 0.002)).clamp(0.5, 200.0);
            }
        }
        self.sync_active_camera_from_viewport();

        let center = response.rect.center() + egui::vec2(self.cam_pan_x, self.cam_pan_y);
        let (sin_x, cos_x) = self.cam_rot_x.sin_cos();
        let (sin_y, cos_y) = self.cam_rot_y.sin_cos();
        let focal = response.rect.height().min(response.rect.width()) * self.camera_focal;
        self.viewport_focal_px = focal;
        let zoom = self.cam_zoom;
        let ortho_scale = focal / zoom.max(0.5);
        let target = self.cam_target;

        let near_plane = self.camera_near;
        let half_w = response.rect.width() * 0.5;
        let half_h = response.rect.height() * 0.5;
        let hw_focal = half_w / focal;
        let hh_focal = half_h / focal;
        let ortho_half_w = half_w / ortho_scale;
        let ortho_half_h = half_h / ortho_scale;
        let rect_min = response.rect.min;
        let rect_max = response.rect.max;
        // HDRI background: the environment along each view ray, on a coarse grid.
        if let Some((file, scale, rotation, true)) = self.environment_light() {
            if let Ok(map) = envlight::load(&file) {
                let view_dir = |pos: egui::Pos2| -> Vec3 {
                    let (rx, ry, rrz) = if self.orthographic {
                        (0.0, 0.0, 1.0)
                    } else {
                        ((pos.x - center.x) / focal, -(pos.y - center.y) / focal, 1.0)
                    };
                    // Undo the viewport rotation (inverse of to_cam).
                    let y = ry * cos_x + rrz * sin_x;
                    let rz = -ry * sin_x + rrz * cos_x;
                    Vec3::new(rx * cos_y + rz * sin_y, y, -rx * sin_y + rz * cos_y).normalize()
                };
                let tone = |v: f32| ((v / (1.0 + v)).powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8;
                let cell = 16.0;
                let (cols, rows) = ((response.rect.width() / cell).ceil() as usize + 1, (response.rect.height() / cell).ceil() as usize + 1);
                let mut sky = egui::epaint::Mesh::default();
                for j in 0..rows {
                    for i in 0..cols {
                        let pos = egui::pos2(rect_min.x + i as f32 * cell, rect_min.y + j as f32 * cell);
                        let l = map.radiance(view_dir(pos), rotation);
                        sky.colored_vertex(pos, egui::Color32::from_rgb(tone(l[0] * scale[0]), tone(l[1] * scale[1]), tone(l[2] * scale[2])));
                    }
                }
                for j in 0..rows - 1 {
                    for i in 0..cols - 1 {
                        let a = (j * cols + i) as u32;
                        sky.add_triangle(a, a + 1, a + cols as u32);
                        sky.add_triangle(a + 1, a + cols as u32 + 1, a + cols as u32);
                    }
                }
                painter.add(egui::Shape::mesh(sky));
            }
        }
        let gizmo_center = response.rect.right_top() + egui::vec2(-54.0, 54.0);
        let gizmo_clicked = response.clicked()
            && response.interact_pointer_pos().is_some_and(|p| {
                p.distance(gizmo_center) <= 34.0
            });
        if gizmo_clicked {
            if let Some(pointer) = response.interact_pointer_pos() {
                let delta = pointer - gizmo_center;
                if delta.x.abs() > delta.y.abs() {
                    self.set_camera_view(0.0, if delta.x < 0.0 {
                        -std::f32::consts::FRAC_PI_2
                    } else {
                        std::f32::consts::FRAC_PI_2
                    });
                } else {
                    self.set_camera_view(
                        if delta.y < 0.0 { -1.52 } else { 1.52 },
                        0.0,
                    );
                }
            }
        }
        painter.circle_filled(gizmo_center, 30.0, egui::Color32::from_black_alpha(150));
        painter.circle_stroke(
            gizmo_center,
            30.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(180)),
        );
        painter.text(
            gizmo_center + egui::vec2(0.0, -13.0),
            egui::Align2::CENTER_CENTER,
            "Y",
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(90, 220, 110),
        );
        painter.text(
            gizmo_center + egui::vec2(17.0, 5.0),
            egui::Align2::CENTER_CENTER,
            "X",
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(230, 90, 90),
        );
        painter.text(
            gizmo_center + egui::vec2(-17.0, 5.0),
            egui::Align2::CENTER_CENTER,
            "Z",
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(90, 140, 240),
        );

        // Transform world coordinate into 3D camera space (+Z in front of camera)
        let to_cam = |p: Vec3| -> Vec3 {
            let v = p.sub(target);
            let rx = v.x * cos_y - v.z * sin_y;
            let rz = v.x * sin_y + v.z * cos_y;
            let ry = v.y * cos_x - rz * sin_x;
            let rrz = v.y * sin_x + rz * cos_x;
            Vec3::new(rx, ry, rrz + zoom)
        };

        // Project 3D camera-space coordinate to 2D screen coordinate
        let project_cam = |c: Vec3| -> egui::Pos2 {
            if self.orthographic {
                egui::pos2(center.x + c.x * ortho_scale, center.y - c.y * ortho_scale)
            } else {
                egui::pos2(center.x + c.x / c.z * focal, center.y - c.y / c.z * focal)
            }
        };

        // Attribute Paint brush: dabs while dragging, applied to the node after drawing.
        let mut pending_paint: Option<(NodeId, Vec<f32>)> = None;
        if let (Some(id), Some(hover)) = (paint_node, response.hover_pos()) {
            if let Some(NodeKind::AttributePaint { values, settings, .. }) = self.graph.nodes.get(&id).map(|n| &n.kind) {
                painter.circle_stroke(hover, settings.radius, egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 210, 90)));
                if response.dragged_by(egui::PointerButton::Primary) || response.clicked() {
                    // Paint on the node's own points, placed where the viewport shows them.
                    let mut points = self.graph.cook(id, self.time, &mut HashMap::new());
                    if let Some(object) = self.graph.enclosing_geometry(id) {
                        points = self.graph.xform_at(object, self.time).apply(points);
                    }
                    let facing = paint::facing(&points, to_cam);
                    let screen: Vec<Option<(f32, f32)>> = points
                        .vertices
                        .iter()
                        .zip(&facing)
                        .map(|(p, front)| {
                            let c = to_cam(*p);
                            (*front && c.z > near_plane).then(|| project_cam(c)).map(|s| (s.x, s.y))
                        })
                        .collect();
                    let mut brush = settings.clone();
                    if ctx.input(|i| i.modifiers.command) {
                        brush.mode = 1;
                    }
                    let mut updated = paint::values_for(values, points.vertices.len(), settings.default);
                    if paint::dab(&mut updated, &points, &screen, (hover.x, hover.y), &brush) {
                        pending_paint = Some((id, updated));
                    }
                }
            }
        }

        // 3D camera-space frustum line clipper
        let clip_line_frustum_3d = |mut p1: Vec3, mut p2: Vec3| -> Option<(Vec3, Vec3)> {
            let plane_dist = |p: Vec3| p.z - near_plane;

            // Side clipping is performed after projection in screen space.
            // Applying camera-centered side planes here incorrectly clips
            // geometry after the viewport has been panned.
            for _ in 0..1 {
                let d1 = plane_dist(p1);
                let d2 = plane_dist(p2);

                if d1 < 0.0 && d2 < 0.0 {
                    return None; // Completely outside this frustum plane
                }
                if d1 < 0.0 {
                    let denom = d1 - d2;
                    if denom.abs() < 1e-7 { return None; }
                    let t = d1 / denom;
                    p1 = Vec3::new(
                        p1.x + t * (p2.x - p1.x),
                        p1.y + t * (p2.y - p1.y),
                        p1.z + t * (p2.z - p1.z),
                    );
                } else if d2 < 0.0 {
                    let denom = d2 - d1;
                    if denom.abs() < 1e-7 { return None; }
                    let t = d2 / denom;
                    p2 = Vec3::new(
                        p2.x + t * (p1.x - p2.x),
                        p2.y + t * (p1.y - p2.y),
                        p2.z + t * (p1.z - p2.z),
                    );
                }
            }

            Some((p1, p2))
        };

        // 2D Liang-Barsky line clipper against visible viewport bounds
        let clip_line_2d = |p1: egui::Pos2, p2: egui::Pos2| -> Option<(egui::Pos2, egui::Pos2)> {
            let dx = p2.x - p1.x;
            let dy = p2.y - p1.y;
            let p = [-dx, dx, -dy, dy];
            let q = [p1.x - rect_min.x, rect_max.x - p1.x, p1.y - rect_min.y, rect_max.y - p1.y];
            let mut u1 = 0.0f32;
            let mut u2 = 1.0f32;

            for i in 0..4 {
                if p[i].abs() < 1e-9 {
                    if q[i] < 0.0 {
                        return None;
                    }
                } else {
                    let t = q[i] / p[i];
                    if p[i] < 0.0 {
                        if t > u1 { u1 = t; }
                    } else if t < u2 {
                        u2 = t;
                    }
                }
            }

            if u1 > u2 {
                return None;
            }
            Some((
                egui::pos2(p1.x + u1 * dx, p1.y + u1 * dy),
                egui::pos2(p1.x + u2 * dx, p1.y + u2 * dy),
            ))
        };

        // Safe 3D line renderer with both 3D frustum & 2D viewport clipping
        let line3d = |a: Vec3, b: Vec3, stroke: egui::Stroke| {
            let c1 = to_cam(a);
            let c2 = to_cam(b);

            if let Some((cp1, cp2)) = clip_line_frustum_3d(c1, c2) {
                let p1 = project_cam(cp1);
                let p2 = project_cam(cp2);
                if let Some((cp1_2d, cp2_2d)) = clip_line_2d(p1, p2) {
                    painter.line_segment([cp1_2d, cp2_2d], stroke);
                }
            }
        };

        // Clip filled triangles in camera space before perspective projection.
        // Projecting an unclipped triangle can otherwise create long slivers
        // when it crosses the near plane or viewport edge.
        let clip_triangle = |vertices: [Vec3; 3]| -> Vec<Vec3> {
            let plane_dist = |i: usize, p: Vec3| -> f32 {
                p.z - near_plane
            };

            let mut polygon = vertices.to_vec();
            // Keep only near-plane clipping in camera space. The viewport
            // rectangle clips the projected polygon and remains correct
            // when the camera is panned.
            for plane in 0..1 {
                if polygon.is_empty() {
                    break;
                }
                let mut clipped = Vec::with_capacity(polygon.len() + 1);
                for i in 0..polygon.len() {
                    let a = polygon[i];
                    let b = polygon[(i + 1) % polygon.len()];
                    let da = plane_dist(plane, a);
                    let db = plane_dist(plane, b);
                    let a_inside = da >= 0.0;
                    let b_inside = db >= 0.0;

                    if a_inside != b_inside {
                        let denom = da - db;
                        if denom.abs() > 1e-7 {
                            let t = da / denom;
                            clipped.push(Vec3::new(
                                a.x + t * (b.x - a.x),
                                a.y + t * (b.y - a.y),
                                a.z + t * (b.z - a.z),
                            ));
                        }
                    }
                    if b_inside {
                        clipped.push(b);
                    }
                }
                polygon = clipped;
            }
            polygon
        };

        // Floor grid + axes
        if self.show_floor {
            let gs = egui::Stroke::new(1.0, egui::Color32::from_gray(62));
            let steps = (self.grid_extent / self.grid_spacing).round() as i32;
            for i in -steps..=steps {
                let s = i as f32 * self.grid_spacing;
                line3d(
                    Vec3::new(s, 0.0, -self.grid_extent),
                    Vec3::new(s, 0.0, self.grid_extent),
                    gs,
                );
                line3d(
                    Vec3::new(-self.grid_extent, 0.0, s),
                    Vec3::new(self.grid_extent, 0.0, s),
                    gs,
                );
            }
            let o = Vec3::zero();
            line3d(o, Vec3::new(1.0, 0.0, 0.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 70, 70)));
            line3d(o, Vec3::new(0.0, 1.0, 0.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(70, 200, 70)));
            line3d(o, Vec3::new(0.0, 0.0, 1.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(70, 120, 230)));
        }

        // Template geometry ghosted wireframe
        if self.template.is_some() && self.template_mesh.indices.len() <= 20_000 {
            let ts = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(230, 80, 200, 140));
            let tm = &self.template_mesh;
            for t in &tm.indices {
                for k in 0..3 {
                    line3d(tm.vertices[t[k]], tm.vertices[t[(k + 1) % 3]], ts);
                }
            }
        }

        if let Some(pick) = &self.group_pick {
            let (count, what) = match pick.domain {
                groups::GroupDomain::Points => (self.selected_vertices.len(), "points"),
                groups::GroupDomain::Prims => (self.selected_triangles.len(), "primitives"),
            };
            let name = self.graph.nodes.get(&pick.node).map_or(String::new(), |n| n.kind.label(pick.node));
            painter.text(
                response.rect.center_top() + egui::vec2(0.0, 14.0),
                egui::Align2::CENTER_TOP,
                format!("Group for {name}: {count} {what} selected — Shift-drag to box select · Enter to accept · Esc to cancel"),
                egui::FontId::proportional(14.0),
                egui::Color32::from_rgb(255, 210, 90),
            );
        }
        let mesh = &self.current_mesh;
        let projected: Vec<Option<(egui::Pos2, f32)>> = mesh
            .vertices
            .iter()
            .map(|v| {
                let c = to_cam(*v);
                (c.z >= near_plane).then(|| (project_cam(c), c.z))
            })
            .collect();
        let mut pending_edge_loop = None;
        let mut clear_selection_on_miss = false;
        let box_selecting = ctx.input(|i| i.modifiers.shift);
        if box_selecting && response.drag_started_by(egui::PointerButton::Primary) && !gizmo_clicked {
            self.selection_box_start = response.interact_pointer_pos();
            self.selection_box_end = self.selection_box_start;
        }
        if self.selection_box_start.is_some() && response.dragged_by(egui::PointerButton::Primary) {
            self.selection_box_end = response.interact_pointer_pos();
        }
        if self.selection_box_start.is_some() && response.drag_stopped_by(egui::PointerButton::Primary) {
            if let (Some(start), Some(end)) = (self.selection_box_start, self.selection_box_end) {
                let bounds = egui::Rect::from_two_pos(start, end);
                let additive = ctx.input(|i| i.modifiers.shift);
                match self.selection_mode {
                    SelectionMode::Vertex => {
                        let selected: Vec<_> = projected.iter().enumerate()
                            .filter_map(|(i, p)| p.filter(|(pos, _)| bounds.contains(*pos)).map(|_| i))
                            .collect();
                        if !additive { self.selected_vertices.clear(); }
                        self.selected_vertices.extend(selected);
                        self.selected_vertices.sort_unstable();
                        self.selected_vertices.dedup();
                    }
                    SelectionMode::Triangle => {
                        let selected: Vec<_> = mesh.indices.iter().enumerate()
                            .filter_map(|(i, tri)| {
                                let points = [projected[tri[0]], projected[tri[1]], projected[tri[2]]];
                                let center = points.iter().flatten()
                                    .fold((egui::Pos2::ZERO, 0usize), |(sum, count), (p, _)| {
                                        (sum + p.to_vec2(), count + 1)
                                    });
                                (center.1 == 3).then(|| {
                                    let center = center.0 / 3.0;
                                    bounds.contains(center).then_some(i)
                                }).flatten()
                            })
                            .collect();
                        if !additive { self.selected_triangles.clear(); }
                        self.selected_triangles.extend(selected);
                        self.selected_triangles.sort_unstable();
                        self.selected_triangles.dedup();
                    }
                    SelectionMode::Edge => {
                        let mut selected = Vec::new();
                        let mut seen = HashSet::new();
                        for tri in &mesh.indices {
                            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                                let edge = (a.min(b), a.max(b));
                                if !seen.insert(edge) { continue; }
                                if let (Some((pa, _)), Some((pb, _))) = (projected[a], projected[b]) {
                                    if bounds.contains(pa) && bounds.contains(pb) {
                                        selected.push(edge);
                                    }
                                }
                            }
                        }
                        if !additive { self.selected_edges.clear(); }
                        self.selected_edges.extend(selected);
                        self.selected_edges.sort_unstable();
                        self.selected_edges.dedup();
                    }
                }
            }
            self.selection_box_start = None;
            self.selection_box_end = None;
        }
        if response.double_clicked() && self.selection_mode == SelectionMode::Edge && !gizmo_clicked {
            if let Some(pointer) = response.interact_pointer_pos() {
                let mut best = None;
                for tri in &mesh.indices {
                    for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                        let (Some((pa, _)), Some((pb, _))) = (projected[a], projected[b]) else { continue };
                        let ab = pb - pa;
                        let t = if ab.length_sq() > 1e-6 {
                            ((pointer - pa).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
                        } else { 0.0 };
                        let distance = pointer.distance(pa + ab * t);
                        if distance <= 16.0 && best.is_none_or(|(d, _)| distance < d) {
                            best = Some((distance, (a.min(b), a.max(b))));
                        }
                    }
                }
                if let Some((_, edge)) = best {
                    pending_edge_loop = Some(edge);
                }
            }
        } else if response.clicked() && !gizmo_clicked && !box_selecting {
            if let Some(pointer) = response.interact_pointer_pos() {
                let front_only = self.render_mode != RenderMode::Wireframe;
                let triangle_hit = |tri: [usize; 3]| -> Option<(f32, f32)> {
                    let (a, za) = projected[tri[0]]?;
                    let (b, zb) = projected[tri[1]]?;
                    let (c, zc) = projected[tri[2]]?;
                    let v0 = b - a;
                    let v1 = c - a;
                    let v2 = pointer - a;
                    let denom = v0.x * v1.y - v1.x * v0.y;
                    if denom.abs() < 1e-6 {
                        return None;
                    }
                    let u = (v2.x * v1.y - v1.x * v2.y) / denom;
                    let v = (v0.x * v2.y - v2.x * v0.y) / denom;
                    if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                        Some((za + u * (zb - za) + v * (zc - za), 0.0))
                    } else {
                        None
                    }
                };
                let front_depth = || {
                    mesh.indices
                        .iter()
                        .filter_map(|&tri| triangle_hit(tri).map(|(depth, _)| depth))
                        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                };
                match self.selection_mode {
                    SelectionMode::Vertex => {
                        let hit = projected
                            .iter()
                            .enumerate()
                            .filter_map(|(i, p)| {
                                let (pos, depth) = *p.as_ref()?;
                                let distance = pos.distance(pointer);
                                if front_only {
                                    let Some(front) = front_depth() else {
                                        return None;
                                    };
                                    if depth > front + 0.5 {
                                        return None;
                                    }
                                }
                                (distance <= 14.0).then_some((distance, depth, i))
                            })
                            .min_by(|a, b| {
                                if front_only {
                                    a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                                } else {
                                    a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                                }
                            })
                            .map(|(_, _, i)| i);
                        if let Some(i) = hit {
                            if let Some(position) = self.selected_vertices.iter().position(|&v| v == i) {
                                self.selected_vertices.remove(position);
                            } else {
                                self.selected_vertices.push(i);
                            }
                        } else {
                            clear_selection_on_miss = true;
                        }
                    }
                    SelectionMode::Triangle => {
                        let mut candidates = mesh.indices.iter().enumerate().filter_map(|(i, &tri)| {
                            let (depth, _) = triangle_hit(tri)?;
                            Some((depth, i))
                        }).collect::<Vec<_>>();
                        candidates.sort_by(|a, b| {
                            if front_only {
                                a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                            } else {
                                a.1.cmp(&b.1)
                            }
                        });
                        let hit = candidates.first().map(|(_, i)| *i);
                        if let Some(i) = hit {
                            if let Some(position) = self.selected_triangles.iter().position(|&v| v == i) {
                                self.selected_triangles.remove(position);
                            } else {
                                self.selected_triangles.push(i);
                            }
                        } else {
                            clear_selection_on_miss = true;
                        }
                    }
                    SelectionMode::Edge => {
                        let mut best: Option<(f32, f32, (usize, usize))> = None;
                        for tri in &mesh.indices {
                            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                                let Some((pa, za)) = projected[a] else { continue };
                                let Some((pb, zb)) = projected[b] else { continue };
                                let ab = pb - pa;
                                let denom = ab.length_sq();
                                let t = if denom > 1e-6 {
                                    ((pointer - pa).dot(ab) / denom).clamp(0.0, 1.0)
                                } else { 0.0 };
                                let distance = pointer.distance(pa + ab * t);
                                let depth = (za + zb) * 0.5;
                                if front_only {
                                    let Some(front) = front_depth() else {
                                        continue;
                                    };
                                    if depth > front + 0.5 {
                                        continue;
                                    }
                                }
                                let better = best.map_or(true, |current| {
                                    if front_only { depth < current.1 } else { distance < current.0 }
                                });
                                if distance <= 12.0 && better {
                                    best = Some((distance, depth, (a.min(b), b.max(a))));
                                }
                            }
                        }
                        if let Some((_, _, edge)) = best {
                            if let Some(position) = self.selected_edges.iter().position(|&v| v == edge) {
                                self.selected_edges.remove(position);
                            } else {
                                self.selected_edges.push(edge);
                            }
                        } else {
                            clear_selection_on_miss = true;
                        }
                    }
                }
            }
        }
        // Stored normals (Normal node) shade the surface; otherwise they're computed.
        let corner_normals = sops::corner_normals(mesh);
        // UV Quick Shade: the texture and each triangle corner's UV.
        let quickshade = sops::quickshade_file(mesh).and_then(|file| sops::corner_uvs(mesh).map(|uvs| (quickshade_texture(&file), uvs)));
        let textured = quickshade.is_some() && !matches!(self.render_mode, RenderMode::Wireframe | RenderMode::Normals);
        let ambient = self.ambient_light.clamp(0.0, 1.0);
        let lights = self.scene_lights();
        let environment = self.environment_light().and_then(|(file, scale, rotation, _)| envlight::load(&file).ok().map(|map| (map, scale, rotation)));
        let illumination = |normal: Vec3, point: Vec3| -> [f32; 3] {
            let mut rgb = [ambient; 3];
            // Image-based diffuse light from the HDRI.
            if let Some((map, scale, rotation)) = &environment {
                let e = map.diffuse(normal, *rotation);
                for c in 0..3 {
                    rgb[c] += e[c] * scale[c];
                }
            }
            for (position, direction, color, intensity, kind) in &lights {
                let to_light = if *kind == 1 {
                    (*position).sub(point).normalize()
                } else {
                    direction.scale(-1.0)
                };
                // Inverse-square, like the MoonRay sphere lights it renders as.
                let attenuation = if *kind == 1 {
                    (POINT_LIGHT_FALLOFF / (*position).sub(point).length().powi(2).max(1e-4)).min(1.5)
                } else {
                    1.0
                };
                let amount = *intensity * attenuation * normal.dot(to_light).max(0.0);
                for channel in 0..3 {
                    rgb[channel] += amount * color[channel];
                }
            }
            rgb.map(|value| value.clamp(0.0, 1.5))
        };

        struct Poly {
            pts: [egui::Pos2; 3],
            // Camera-space depth of each corner, for the depth buffer.
            depth: [f32; 3],
            colors: [egui::Color32; 3],
            // Texture coordinates when the mesh is quick-shaded.
            uv: Option<[egui::Pos2; 3]>,
        }
        let cd = mesh.point_attributes.get("Cd");
        let cd_color = |index: usize| -> Option<egui::Color32> {
            let values = cd?;
            let vertex_count = mesh.vertices.len();
            let (r, g, b) = if values.len() >= vertex_count.saturating_mul(3) {
                (
                    values.get(index * 3).copied()?,
                    values.get(index * 3 + 1).copied()?,
                    values.get(index * 3 + 2).copied()?,
                )
            } else {
                let value = values.get(index).copied()?;
                let [r, g, b] = attribute_color(value);
                (r, g, b)
            };
            Some(egui::Color32::from_rgb(
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
            ))
        };
        // Triangle corner colours follow Houdini's precedence: the first Cd found
        // on vertices, points, primitives, then the detail.
        let (point_count, prim_count) = (mesh.vertices.len(), mesh.indices.len());
        let surface_cd = [
            ("vertex:Cd", SheetDomain::Vertices, prim_count * 3),
            ("Cd", SheetDomain::Points, point_count),
            ("prim:Cd", SheetDomain::Primitives, prim_count),
            ("detail:Cd", SheetDomain::Detail, 1),
        ]
        .into_iter()
        .find_map(|(key, domain, count)| {
            let values = mesh.point_attributes.get(key)?;
            (count > 0 && values.len() >= count).then(|| (values.as_slice(), values.len() / count, domain))
        });
        let corner_color = |tri_index: usize, corner: usize, point: usize| -> Option<egui::Color32> {
            let (values, components, domain) = surface_cd?;
            let index = match domain {
                SheetDomain::Vertices => tri_index * 3 + corner,
                SheetDomain::Points => point,
                SheetDomain::Primitives => tri_index,
                SheetDomain::Detail => 0,
            };
            let [r, g, b] = if components >= 3 {
                let at = index * components;
                [*values.get(at)?, *values.get(at + 1)?, *values.get(at + 2)?]
            } else {
                attribute_color(*values.get(index * components)?)
            };
            Some(egui::Color32::from_rgb(
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
            ))
        };
        let mut queue: Vec<Poly> = Vec::with_capacity(mesh.indices.len());
        for (tri_index, tri) in mesh.indices.iter().enumerate() {
            if self.isolate_selection {
                let keep = match self.selection_mode {
                    SelectionMode::Vertex => self.selected_vertices.iter().any(|&i| tri.contains(&i)),
                    SelectionMode::Triangle => self.selected_triangles.contains(&tri_index),
                    SelectionMode::Edge => self.selected_edges.iter().any(|&(a, b)| {
                        tri.contains(&a) && tri.contains(&b)
                    }),
                };
                if !keep {
                    continue;
                }
            }
            let (v0, v1, v2) = (mesh.vertices[tri[0]], mesh.vertices[tri[1]], mesh.vertices[tri[2]]);
            let face_normal = v1.sub(v0).cross(v2.sub(v0)).normalize();
            let shade = |n: Vec3, point: Vec3| -> egui::Color32 {
                match self.render_mode {
                    RenderMode::Wireframe => egui::Color32::TRANSPARENT,
                    RenderMode::Normals => egui::Color32::from_rgb(
                        ((n.x * 0.5 + 0.5) * 255.0) as u8,
                        ((n.y * 0.5 + 0.5) * 255.0) as u8,
                        ((n.z * 0.5 + 0.5) * 255.0) as u8,
                    ),
                    RenderMode::FlatShaded => {
                        let rgb = illumination(face_normal, point);
                        egui::Color32::from_rgb(
                            (200.0 * rgb[0]) as u8,
                            (205.0 * rgb[1]) as u8,
                            (215.0 * rgb[2]) as u8,
                        )
                    }
                    _ => {
                        let rgb = illumination(n, point);
                        egui::Color32::from_rgb(
                            (200.0 * rgb[0]) as u8,
                            (205.0 * rgb[1]) as u8,
                            (215.0 * rgb[2]) as u8,
                        )
                    }
                }
            };
            let lit_color = |corner: usize, point: Vec3, normal: Vec3| {
                let Some(base) = corner_color(tri_index, corner, tri[corner]) else {
                    return shade(normal, point);
                };
                let lighting_normal = if self.render_mode == RenderMode::FlatShaded {
                    face_normal
                } else {
                    normal
                };
                let light = illumination(lighting_normal, point);
                egui::Color32::from_rgb(
                    (f32::from(base.r()) * light[0]).clamp(0.0, 255.0) as u8,
                    (f32::from(base.g()) * light[1]).clamp(0.0, 255.0) as u8,
                    (f32::from(base.b()) * light[2]).clamp(0.0, 255.0) as u8,
                )
            };
            let corner_n = corner_normals[tri_index];
            let colors = if textured {
                // White, lit; the texture supplies the colour.
                let lit = |point: Vec3, normal: Vec3| {
                    let n = if self.render_mode == RenderMode::FlatShaded { face_normal } else { normal };
                    let l = illumination(n, point);
                    egui::Color32::from_rgb((255.0 * l[0]).min(255.0) as u8, (255.0 * l[1]).min(255.0) as u8, (255.0 * l[2]).min(255.0) as u8)
                };
                [lit(v0, corner_n[0]), lit(v1, corner_n[1]), lit(v2, corner_n[2])]
            } else {
                [lit_color(0, v0, corner_n[0]), lit_color(1, v1, corner_n[1]), lit_color(2, v2, corner_n[2])]
            };
            let uv = quickshade.as_ref().filter(|_| textured).map(|(_, uvs)| uvs[tri_index].map(|c| egui::pos2(c[0], 1.0 - c[1])));
            let camera_triangle = [to_cam(v0), to_cam(v1), to_cam(v2)];
            let clipped = clip_triangle(camera_triangle);
            if clipped.len() < 3 {
                continue;
            }
            let first = clipped[0];
            for i in 1..clipped.len() - 1 {
                let a = project_cam(first);
                let b = project_cam(clipped[i]);
                let c = project_cam(clipped[i + 1]);
                let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if !area.is_finite() || area.abs() < 1e-4 {
                    continue;
                }
                queue.push(Poly {
                    // Near-plane clipped pieces fall back to untextured shading.
                    uv: if clipped.len() == 3 { uv } else { None },
                    pts: [a, b, c],
                    depth: [first.z, clipped[i].z, clipped[i + 1].z],
                    colors: if clipped.len() == 3 {
                        colors
                    } else {
                        let average = egui::Color32::from_rgb(
                            ((colors[0].r() as u16 + colors[1].r() as u16 + colors[2].r() as u16) / 3) as u8,
                            ((colors[0].g() as u16 + colors[1].g() as u16 + colors[2].g() as u16) / 3) as u8,
                            ((colors[0].b() as u16 + colors[1].b() as u16 + colors[2].b() as u16) / 3) as u8,
                        );
                        [average; 3]
                    },
                });
            }
        }
        // Surfaces and wireframe go through a depth buffer (see raster.rs), at the
        // screen's pixel density, then onto the viewport as one image.
        let wire: Option<[u8; 4]> = match self.render_mode {
            RenderMode::Wireframe => Some([80, 220, 120, 255]),
            RenderMode::ShadedWireframe => Some([0, 0, 0, 110]),
            _ => None,
        };
        if !queue.is_empty() {
            const MAX_PIXELS: f32 = 2.5e6;
            let size = response.rect.size();
            let mut scale = ctx.pixels_per_point();
            if size.x * size.y * scale * scale > MAX_PIXELS {
                scale = (MAX_PIXELS / (size.x * size.y)).sqrt();
            }
            let (w, h) = ((size.x * scale).round().max(1.0) as usize, (size.y * scale).round().max(1.0) as usize);
            let mut raster = raster::Raster::new(w, h, self.orthographic);
            let to_pixel = |p: egui::Pos2, z: f32| raster::ScreenVertex { x: (p.x - rect_min.x) * scale, y: (p.y - rect_min.y) * scale, z };
            let rgb = |c: egui::Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
            let texture = quickshade.as_ref().map(|(texture, _)| texture.clone());
            // Wireframe mode shows every edge, so its (transparent) faces don't occlude.
            let triangles: Vec<raster::Triangle> = if self.render_mode == RenderMode::Wireframe {
                Vec::new()
            } else {
                queue
                    .iter()
                    .map(|p| raster::Triangle {
                        v: [0, 1, 2].map(|k| to_pixel(p.pts[k], p.depth[k])),
                        rgb: p.colors.map(rgb),
                        uv: p.uv.zip(texture.as_deref()).map(|(uv, t)| (uv.map(|c| [c.x, c.y]), t)),
                    })
                    .collect()
            };
            let lines: Vec<raster::Line> = match wire {
                Some(colour) => queue
                    .iter()
                    .flat_map(|p| [(0, 1), (1, 2), (2, 0)].map(|(a, b)| raster::Line { a: to_pixel(p.pts[a], p.depth[a]), b: to_pixel(p.pts[b], p.depth[b]), rgba: colour }))
                    .collect(),
                None => Vec::new(),
            };
            raster.draw(&triangles, &lines);
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], raster.color.as_flattened());
            let texture = viewport_texture(ctx, image);
            painter.image(texture.id(), response.rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }

        let view_display = self.view_displays().first().copied();
        // Geometry containers display their inner display node's kind.
        let view_display_node = view_display.and_then(|id| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Geometry { .. }) => self.graph.display_of(id),
            _ => Some(id),
        });
        let particle_display = view_display_node
            .and_then(|id| self.graph.nodes.get(&id))
            .is_some_and(|node| matches!(
                node.kind,
                NodeKind::Add { .. }
                    | NodeKind::Particles { .. }
                        | NodeKind::Flip { .. }
                    | NodeKind::WhitewaterSolver(_)
                    | NodeKind::Cache { .. }
            ));
        let point_color = view_display_node
            .and_then(|id| self.graph.nodes.get(&id))
            .map(|node| match node.kind {
                NodeKind::Flip { .. } => egui::Color32::from_rgb(70, 180, 255),
                _ => egui::Color32::from_rgb(255, 170, 45),
            })
            .unwrap_or(egui::Color32::from_rgb(255, 220, 80));
        let point_attribute_color = |index: usize| -> egui::Color32 {
            cd_color(index).unwrap_or(point_color)
        };
        let mut referenced = vec![false; projected.len()];
        for tri in &mesh.indices {
            for &vertex in tri {
                if let Some(is_referenced) = referenced.get_mut(vertex) {
                    *is_referenced = true;
                }
            }
        }
        // Hair and other curves (see groom.rs) are drawn as lines, not loose points.
        let curve_ranges = groom::curves(mesh);
        for range in &curve_ranges {
            for i in range.clone() {
                if let Some(is_referenced) = referenced.get_mut(i) {
                    *is_referenced = true;
                }
            }
        }
        let has_unreferenced_points = referenced.iter().any(|&is_referenced| !is_referenced);
        if !curve_ranges.is_empty() {
            // Dense grooms are thinned for display; every curve still renders.
            const MAX_DRAWN_CURVES: usize = 8_000;
            let stride = curve_ranges.len().div_ceil(MAX_DRAWN_CURVES).max(1);
            let hair = egui::Color32::from_rgb(200, 165, 115);
            for range in curve_ranges.iter().step_by(stride) {
                let line: Vec<egui::Pos2> = range.clone().filter_map(|i| projected[i].map(|p| p.0)).collect();
                if line.len() >= 2 {
                    let colour = cd_color(range.start).unwrap_or(hair);
                    painter.add(egui::Shape::line(line, egui::Stroke::new(1.0, colour)));
                }
            }
        }
        // Volume points (Pyro output) carry Alpha and pscale: draw them as soft,
        // back-to-front splats sized in world units instead of opaque dots.
        let point_count = mesh.vertices.len();
        let splat_alpha = mesh.point_attributes.get("Alpha").filter(|a| a.len() == point_count && point_count > 0);
        if let Some(alpha) = splat_alpha {
            let pscale = mesh.point_attributes.get("pscale").filter(|v| v.len() == point_count);
            let colors = mesh.point_attributes.get("Cd").filter(|v| v.len() == point_count * 3);
            // Thin out very dense volumes, growing the remaining splats to compensate.
            const MAX_SPLATS: usize = 40_000;
            let stride = point_count.div_ceil(MAX_SPLATS).max(1);
            let grow = (stride as f32).cbrt();
            let mut order: Vec<(usize, f32)> = projected
                .iter()
                .enumerate()
                .step_by(stride)
                .filter_map(|(i, p)| p.map(|(_, depth)| (i, depth)))
                .filter(|(i, _)| alpha[*i] > 0.004)
                .collect();
            order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (i, depth) in order {
                let Some((position, _)) = projected[i] else { continue };
                let world_radius = pscale.map_or(0.05, |p| p[i]) * grow;
                let radius = if self.orthographic { world_radius * ortho_scale } else { world_radius / depth.max(1e-3) * focal };
                let [r, g, b] = colors.map_or([0.8; 3], |c| [c[i * 3], c[i * 3 + 1], c[i * 3 + 2]]).map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8);
                let a = 1.0 - (1.0 - alpha[i].clamp(0.0, 1.0)).powf(grow);
                let shade = |opacity: f32| egui::Color32::from_rgba_unmultiplied(r, g, b, (opacity.clamp(0.0, 1.0) * 255.0) as u8);
                // Two nested discs approximate a soft falloff.
                painter.circle_filled(position, radius.clamp(0.5, 120.0), shade(a * 0.5));
                painter.circle_filled(position, (radius * 0.55).clamp(0.5, 120.0), shade(a * 0.6));
            }
        }
        if (self.show_points || particle_display || has_unreferenced_points)
            && projected.len() <= 50_000
            && splat_alpha.is_none()
        {
            let point_radius = if particle_display || has_unreferenced_points { 3.5 } else { 2.0 };
            for (index, point) in projected.iter().enumerate() {
                if has_unreferenced_points && referenced[index] {
                    continue;
                }
                let Some(p) = point else { continue };
                painter.circle_filled(p.0, point_radius, point_attribute_color(index));
            }
        }
        for &i in self.selected_vertices.iter().filter(|&&i| i < projected.len()) {
            if let Some((position, _)) = projected[i] {
                painter.circle_stroke(
                    position,
                    7.0,
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
                painter.circle_filled(position, 3.0, egui::Color32::YELLOW);
            }
        }
        for &i in self.selected_triangles.iter().filter(|&&i| i < mesh.indices.len()) {
            let tri = mesh.indices[i];
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                if let (Some(pa), Some(pb)) = (projected[a], projected[b]) {
                    painter.line_segment(
                        [pa.0, pb.0],
                        egui::Stroke::new(3.0, egui::Color32::YELLOW),
                    );
                }
            }
        }
        for &(a, b) in &self.selected_edges {
            if let (Some(pa), Some(pb)) = (projected.get(a).copied().flatten(), projected.get(b).copied().flatten()) {
                painter.line_segment(
                    [pa.0, pb.0],
                    egui::Stroke::new(4.0, egui::Color32::from_rgb(255, 150, 40)),
                );
            }
        }
        if let (Some(start), Some(end)) = (self.selection_box_start, self.selection_box_end) {
            let selection_rect = egui::Rect::from_two_pos(start, end);
            painter.rect_filled(
                selection_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(70, 150, 240, 35),
            );
            painter.rect_stroke(
                selection_rect,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 190, 255)),
                egui::StrokeKind::Inside,
            );
        }

        if self.show_gizmos {
            let mut ids: Vec<NodeId> = self
                .graph
                .nodes
                .iter()
                .filter(|(_, n)| {
                    n.parent.is_none() && matches!(n.kind, NodeKind::Camera { .. } | NodeKind::Light { .. })
                })
                .map(|(id, _)| *id)
                .collect();
            ids.sort_unstable();
            // Gizmos keep a roughly constant on-screen size as the view zooms.
            let unit = zoom * 0.07;
            for id in ids {
                let node = &self.graph.nodes[&id];
                let selected = self.selected == Some(id);
                match &node.kind {
                    NodeKind::Light { position, rotation, color, kind, .. } => {
                        let pos = Vec3::from_arr(*position);
                        let channel = |v: f32| (90.0 + v.clamp(0.0, 1.0) * 165.0) as u8;
                        let tint = if node.bypass {
                            egui::Color32::from_gray(110)
                        } else {
                            egui::Color32::from_rgb(channel(color[0]), channel(color[1]), channel(color[2]))
                        };
                        let outline = if selected { egui::Color32::WHITE } else { tint };
                        let stroke = egui::Stroke::new(if selected { 2.0 } else { 1.5 }, outline);

                        if *kind != 1 {
                            let dir = Vec3::new(0.0, 0.0, -1.0).rotate_deg(*rotation).normalize();
                            let tip = pos.add(dir.scale(unit * 3.5));
                            line3d(pos, tip, egui::Stroke::new(1.0, tint));
                            let hint = if dir.y.abs() > 0.99 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
                            let side = dir.cross(hint).normalize().scale(unit * 0.25);
                            let lift = dir.cross(side).normalize().scale(unit * 0.25);
                            let base = tip.sub(dir.scale(unit * 0.6));
                            for offset in [side, side.scale(-1.0), lift, lift.scale(-1.0)] {
                                line3d(tip, base.add(offset), egui::Stroke::new(1.0, tint));
                            }
                        }

                        let cam_pos = to_cam(pos);
                        if cam_pos.z > near_plane {
                            let p = project_cam(cam_pos);
                            if response.rect.contains(p) {
                                let at = |x: f32, y: f32| p + egui::vec2(x, y);
                                let glass = at(0.0, -6.0);
                                painter.circle_filled(
                                    glass,
                                    8.0,
                                    egui::Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), if node.bypass { 20 } else { 70 }),
                                );
                                painter.circle_stroke(glass, 8.0, stroke);
                                painter.line_segment([at(-4.5, 0.5), at(-3.5, 5.0)], stroke);
                                painter.line_segment([at(4.5, 0.5), at(3.5, 5.0)], stroke);
                                for (y0, y1, half) in [(5.0, 8.0, 3.5), (9.0, 12.0, 2.5)] {
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(at(-half, y0), at(half, y1)),
                                        1.0,
                                        egui::Color32::from_gray(150),
                                    );
                                }
                                let filament = [at(-2.0, 0.0), at(-2.0, -5.0), at(0.0, -3.0), at(2.0, -5.0), at(2.0, 0.0)];
                                painter.add(egui::Shape::line(filament.to_vec(), egui::Stroke::new(1.0, tint)));
                                if !node.bypass {
                                    for step in 0..8 {
                                        let angle = std::f32::consts::TAU * step as f32 / 8.0;
                                        let ray = egui::vec2(angle.cos(), angle.sin());
                                        if ray.y > 0.6 {
                                            continue;
                                        }
                                        painter.line_segment([glass + ray * 12.0, glass + ray * 16.0], egui::Stroke::new(1.0, tint));
                                    }
                                }
                                painter.text(
                                    at(0.0, 20.0),
                                    egui::Align2::CENTER_TOP,
                                    format!("light{id}"),
                                    egui::FontId::proportional(11.0),
                                    outline,
                                );
                            }
                        }
                    }
                    NodeKind::Camera { position, rotation, pan, zoom: cam_zoom, .. } => {
                        let (sx, cx) = rotation[0].to_radians().sin_cos();
                        let (sy, cy) = rotation[1].to_radians().sin_cos();
                        let right = Vec3::new(cy, 0.0, -sy);
                        let up = Vec3::new(-sx * sy, cx, -sx * cy);
                        let fwd = Vec3::new(cx * sy, sx, cx * cy);
                        let pan_scale = *cam_zoom / focal.max(1.0);
                        let aim = Vec3::from_arr(*position)
                            .sub(right.scale(pan[0] * pan_scale))
                            .add(up.scale(pan[1] * pan_scale));
                        let eye = aim.sub(fwd.scale(*cam_zoom));
                        let tint = if node.bypass {
                            egui::Color32::from_gray(110)
                        } else if self.active_camera == Some(id) {
                            egui::Color32::from_rgb(255, 170, 60)
                        } else {
                            egui::Color32::from_rgb(80, 180, 220)
                        };
                        let stroke = egui::Stroke::new(if selected { 2.5 } else { 1.5 }, if selected { egui::Color32::WHITE } else { tint });
                        let s = unit;
                        let put = |x: f32, y: f32, z: f32| {
                            eye.add(right.scale(x * s)).add(up.scale(y * s)).add(fwd.scale(z * s))
                        };

                        // Body box behind the lens point
                        let (bx, by, back) = (0.5, 0.4, -1.3);
                        let corners = |z: f32| [put(-bx, -by, z), put(bx, -by, z), put(bx, by, z), put(-bx, by, z)];
                        let (front, rear) = (corners(0.0), corners(back));
                        for i in 0..4 {
                            line3d(front[i], front[(i + 1) % 4], stroke);
                            line3d(rear[i], rear[(i + 1) % 4], stroke);
                            line3d(front[i], rear[i], stroke);
                        }

                        // Lens flaring out toward the view direction
                        let lens_len = 0.7;
                        let lens_near = [put(-0.3, -0.3, 0.0), put(0.3, -0.3, 0.0), put(0.3, 0.3, 0.0), put(-0.3, 0.3, 0.0)];
                        let lens_far = [
                            put(-0.45, -0.45, lens_len),
                            put(0.45, -0.45, lens_len),
                            put(0.45, 0.45, lens_len),
                            put(-0.45, 0.45, lens_len),
                        ];
                        for i in 0..4 {
                            line3d(lens_far[i], lens_far[(i + 1) % 4], stroke);
                            line3d(lens_near[i], lens_far[i], stroke);
                        }

                        // Two film reels on top
                        let reel_radius = 0.32;
                        for reel_z in [-0.4, -1.0] {
                            for reel_x in [-0.12, 0.12] {
                                let segments = 14;
                                let ring = |k: usize| {
                                    let a = std::f32::consts::TAU * k as f32 / segments as f32;
                                    put(reel_x, by + reel_radius + reel_radius * a.sin(), reel_z + reel_radius * a.cos())
                                };
                                for k in 0..segments {
                                    line3d(ring(k), ring(k + 1), stroke);
                                }
                            }
                        }

                        let cam_pos = to_cam(put(0.0, 0.0, back));
                        if cam_pos.z > near_plane {
                            let p = project_cam(cam_pos);
                            if response.rect.contains(p) {
                                painter.text(
                                    p + egui::vec2(0.0, 26.0),
                                    egui::Align2::CENTER_TOP,
                                    format!("camera{id}"),
                                    egui::FontId::proportional(11.0),
                                    if selected { egui::Color32::WHITE } else { tint },
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // HUD
        let bounds = mesh.bounds().map(|(lo, hi)| {
            format!(
                "bounds [{:.2}, {:.2}, {:.2}]–[{:.2}, {:.2}, {:.2}]",
                lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
            )
        }).unwrap_or_else(|| "bounds empty".to_string());
        let warnings = mesh.validation_warnings();
        let disp = match view_display {
            None => "No display node — enter a geometry node and double-click a node".to_string(),
            Some(id) => format!(
                "{}  |  {} pts  {} tris  |  cook {:.1} ms  |  t = {:.2}  |  cache {}/{} \n{}{}",
                self.graph.nodes.get(&id).map_or_else(|| format!("?{id}"), |n| n.kind.label(id)),
                mesh.vertices.len(),
                mesh.indices.len(),
                self.cook_ms,
                self.time,
                self.frame_cache_hits,
                self.frame_cache.len(),
                bounds,
                if warnings.is_empty() {
                    String::new()
                } else {
                    format!("  ⚠ {}", warnings.join(", "))
                }
            ),
        };
        painter.text(
            response.rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            disp,
            egui::FontId::monospace(12.0),
            egui::Color32::from_gray(210),
        );
        // Transform gizmo for the selected object, drawn at its pivot with a constant screen size.
        let mut gizmo_frame = None;
        if let Some((node, xform)) = self.gizmo_target().filter(|_| self.show_gizmos) {
            let pivot = Vec3::from_arr(xform.t);
            let cam_right = Vec3::new(cos_y, 0.0, -sin_y);
            let cam_up = Vec3::new(-sin_x * sin_y, cos_x, -sin_x * cos_y);
            let pivot_cam = to_cam(pivot);
            if pivot_cam.z > near_plane {
                let pivot_screen = project_cam(pivot_cam);
                let pixels_per_unit = (project_cam(to_cam(pivot.add(cam_right))) - pivot_screen).length().max(1e-4);
                let handle_px = 85.0;
                let length = handle_px / pixels_per_unit;
                let screen = |p: Vec3| {
                    let c = to_cam(p);
                    (c.z > near_plane).then(|| project_cam(c))
                };
                let axes = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)];
                let colours = [egui::Color32::from_rgb(235, 85, 85), egui::Color32::from_rgb(105, 215, 105), egui::Color32::from_rgb(95, 145, 245)];
                let mut parts = Vec::new();
                match self.gizmo_mode {
                    GizmoMode::Translate | GizmoMode::Scale => {
                        for i in 0..3 {
                            // Scale works along the object's own (rotated) axes.
                            let axis = if self.gizmo_mode == GizmoMode::Scale { axes[i].rotate_deg(xform.r).normalize() } else { axes[i] };
                            if let Some(tip) = screen(pivot.add(axis.scale(length))) {
                                parts.push(GizmoPart { handle: GizmoHandle::Axis(i), points: vec![pivot_screen, tip], axis_screen: (tip - pivot_screen) / length, ring_sign: 1.0 });
                            }
                        }
                    }
                    GizmoMode::Rotate => {
                        for i in 0..3 {
                            let (u, v) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
                            let points: Vec<egui::Pos2> = (0..=48)
                                .filter_map(|k| {
                                    let a = k as f32 / 48.0 * std::f32::consts::TAU;
                                    screen(pivot.add(u.scale(a.cos() * length)).add(v.scale(a.sin() * length)))
                                })
                                .collect();
                            let (us, vs) = (screen(pivot.add(u.scale(length))), screen(pivot.add(v.scale(length))));
                            if let (true, Some(us), Some(vs)) = (points.len() > 8, us, vs) {
                                let (a, b) = (us - pivot_screen, vs - pivot_screen);
                                let sign = if a.x * b.y - a.y * b.x >= 0.0 { 1.0 } else { -1.0 };
                                parts.push(GizmoPart { handle: GizmoHandle::Ring(i), points, axis_screen: egui::Vec2::ZERO, ring_sign: sign });
                            }
                        }
                    }
                }
                if self.gizmo_mode != GizmoMode::Rotate {
                    parts.push(GizmoPart { handle: GizmoHandle::Center, points: vec![pivot_screen], axis_screen: egui::Vec2::ZERO, ring_sign: 1.0 });
                }
                let frame = GizmoFrame { node, pivot: pivot_screen, pixels_per_unit, cam_right, cam_up, handle_px, parts };
                let active = self.gizmo_drag.as_ref().map(|d| d.part.handle);
                let hovered = response.hover_pos().and_then(|p| frame.hit(p)).map(|p| p.handle);
                for part in &frame.parts {
                    let highlight = active == Some(part.handle) || (active.is_none() && hovered == Some(part.handle));
                    let colour = match part.handle {
                        _ if highlight => egui::Color32::from_rgb(255, 225, 70),
                        GizmoHandle::Axis(i) | GizmoHandle::Ring(i) => colours[i],
                        GizmoHandle::Center => egui::Color32::from_gray(235),
                    };
                    match (part.handle, part.points.as_slice()) {
                        (GizmoHandle::Center, [c]) => {
                            let rect = egui::Rect::from_center_size(*c, egui::vec2(11.0, 11.0));
                            painter.rect_stroke(rect, 2.0, egui::Stroke::new(2.0, colour), egui::StrokeKind::Middle);
                        }
                        (GizmoHandle::Axis(_), [a, b]) => {
                            painter.line_segment([*a, *b], egui::Stroke::new(if highlight { 3.5 } else { 2.5 }, colour));
                            if self.gizmo_mode == GizmoMode::Scale {
                                painter.rect_filled(egui::Rect::from_center_size(*b, egui::vec2(9.0, 9.0)), 1.0, colour);
                            } else {
                                // Arrow head.
                                let dir = (*b - *a).normalized();
                                let side = egui::vec2(-dir.y, dir.x) * 5.0;
                                painter.add(egui::Shape::convex_polygon(vec![*b + dir * 10.0, *b + side, *b - side], colour, egui::Stroke::NONE));
                            }
                        }
                        (_, points) => {
                            painter.add(egui::Shape::line(points.to_vec(), egui::Stroke::new(if highlight { 3.0 } else { 2.0 }, colour)));
                        }
                    }
                }
                gizmo_frame = Some(frame);
            }
        }
        self.gizmo_frame = gizmo_frame;
        if let Some(edge) = pending_edge_loop {
            self.select_edge_loop(edge);
            self.status = "Edge loop selected".into();
        }
        if clear_selection_on_miss {
            self.clear_component_selection();
        }
        if let Some((id, painted)) = pending_paint {
            if let Some(NodeKind::AttributePaint { values, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                *values = painted;
                self.dirty = true;
                ctx.request_repaint();
            }
        }
    }

    // The selected object the gizmo edits, as a transform (lights and cameras have no scale).
    fn gizmo_target(&self) -> Option<(NodeId, ObjectXform)> {
        if self.context != AppContext::Obj {
            return None;
        }
        let id = self.selected?;
        let node = self.graph.nodes.get(&id)?;
        match &node.kind {
            NodeKind::Geometry { .. } => Some((id, node.xform)),
            NodeKind::Light { position, rotation, .. } | NodeKind::Camera { position, rotation, .. } => {
                Some((id, ObjectXform { t: *position, r: *rotation, s: [1.0; 3] }))
            }
            _ => None,
        }
    }

    // Turns the mouse motion since the drag started into a new transform for the target.
    fn apply_gizmo_drag(&mut self, drag: &GizmoDrag, pointer: egui::Pos2) {
        let total = pointer - drag.start_pointer;
        let frame = &drag.frame;
        let mut x = drag.start;
        match (self.gizmo_mode, drag.part.handle) {
            (GizmoMode::Translate, GizmoHandle::Axis(i)) => {
                let a = drag.part.axis_screen;
                x.t[i] += total.dot(a) / a.length_sq().max(1e-6);
            }
            (GizmoMode::Translate, GizmoHandle::Center) => {
                let ppu = frame.pixels_per_unit;
                let delta = frame.cam_right.scale(total.x / ppu).sub(frame.cam_up.scale(total.y / ppu));
                x.t = [x.t[0] + delta.x, x.t[1] + delta.y, x.t[2] + delta.z];
            }
            (GizmoMode::Rotate, GizmoHandle::Ring(i)) => {
                let angle = |p: egui::Pos2| (p.y - frame.pivot.y).atan2(p.x - frame.pivot.x);
                let mut delta = angle(pointer) - angle(drag.start_pointer);
                if delta > std::f32::consts::PI {
                    delta -= std::f32::consts::TAU;
                } else if delta < -std::f32::consts::PI {
                    delta += std::f32::consts::TAU;
                }
                x.r[i] += delta.to_degrees() * drag.part.ring_sign;
            }
            (GizmoMode::Scale, GizmoHandle::Axis(i)) => {
                let dir = drag.part.axis_screen.normalized();
                x.s[i] = (drag.start.s[i] * (1.0 + total.dot(dir) / frame.handle_px)).max(0.001);
            }
            (GizmoMode::Scale, GizmoHandle::Center) => {
                let factor = (1.0 + (total.x - total.y) / frame.handle_px).max(0.001);
                x.s = drag.start.s.map(|s| (s * factor).max(0.001));
            }
            _ => return,
        }
        let Some(node) = self.graph.nodes.get_mut(&frame.node) else { return };
        match &mut node.kind {
            NodeKind::Geometry { .. } => node.xform = x,
            NodeKind::Light { position, rotation, .. } | NodeKind::Camera { position, rotation, .. } => {
                *position = x.t;
                *rotation = x.r;
            }
            _ => return,
        }
        self.dirty = true;
    }
}

// ============================================================================
// 7. UPDATE LOOP
// ============================================================================

impl eframe::App for ProceduralFxStudioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.shortcuts(ctx);

        // Keep the requested simulation time stable until the asynchronous
        // FLIP cook completes. Otherwise playback changes the cache key every
        // frame and cancels the job before it can produce a result.
        if self.animate && self.simulation_rx.is_none() {
            self.time += self.anim_speed;
            if self.time > self.playback_end {
                self.time = self.playback_start;
            }
            self.dirty = true;
            ctx.request_repaint();
        }

        self.begin_animation_frame(ctx);
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| self.toolbar(ui));
        self.updater.poll(ctx);
        if let Some(id) = self.group_pick_request.take() {
            self.start_group_pick(id);
        }
        if let Some(domain) = self.group_pick_domain.take() {
            self.set_group_pick_domain(domain);
        }
        if let Some(accept) = self.group_pick_finish.take() {
            self.finish_group_pick(accept);
        }
        if self.group_pick.is_some() {
            let (enter, escape) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if enter || escape {
                self.finish_group_pick(enter);
            }
            // Picking is tied to the node it was started from.
            if self.group_pick.as_ref().is_some_and(|p| Some(p.node) != self.selected) {
                self.finish_group_pick(false);
            }
        }
        if let Some(id) = self.group_choices_request.take() {
            let input = self.graph.input_of(id, 0).map(|i| self.graph.cook(i, self.time, &mut HashMap::new())).unwrap_or_default();
            let (points, prims) = groups::names(&input);
            self.group_choices = Some((id, points, prims));
        }
        if let Some(id) = self.paint_fill.take() {
            let count = self.graph.input_of(id, 0).map_or(0, |i| self.graph.cook(i, self.time, &mut HashMap::new()).vertices.len());
            if let Some(NodeKind::AttributePaint { values, settings, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                *values = vec![settings.value; count];
                self.dirty = true;
            }
        }
        if self.updater.show(ctx) == updater::UpdaterAction::SaveScene {
            self.save_scene();
        }
        self.update_window_title(ctx);

        self.poll_moonray_render();
        self.advance_sequence();

        if self.dirty || self.simulation_rx.is_some() {
            self.rebuild_graph();
            self.dirty = false;
        }
        if self.simulation_rx.is_some() || self.moonray_rx.is_some() || self.sequence.is_some() {
            ctx.request_repaint();
        }

        // The panes: a dock the user can rearrange, split, float and pop out (see Pane).
        self.show_panes(ctx);
        if self.dirty {
            ctx.request_repaint();
        }

        if self.use_camera_requested {
            self.use_camera_requested = false;
            if let Some(id) = self.selected.filter(|id| {
                matches!(
                    self.graph.nodes.get(id).map(|node| &node.kind),
                    Some(NodeKind::Camera { .. })
                )
            }) {
                if let Some(NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom }) =
                    self.graph.nodes.get(&id).map(|node| &node.kind)
                {
                    self.active_camera = Some(id);
                    self.cam_target = Vec3::from_arr(*position);
                    self.cam_rot_x = rotation[0].to_radians();
                    self.cam_rot_y = rotation[1].to_radians();
                    self.camera_focal = *focal;
                    self.camera_near = *near;
                    self.orthographic = *orthographic;
                    self.cam_pan_x = pan[0];
                    self.cam_pan_y = pan[1];
                    self.cam_zoom = *zoom;
                    self.status = format!("Viewport camera {id} activated");
                }
            }
        }
        if let Some((id, range)) = self.file_export_request.take() {
            self.status = self.write_file_export(id, range).unwrap_or_else(|e| format!("File Export failed: {e}"));
        }
        if let Some(id) = self.usd_export_request.take() {
            if let Some(NodeKind::LopExport { file }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) {
                let stage = self.graph.cook_stage(id, self.time, &mut HashMap::new());
                self.status = match usd::save(&stage, file.trim()) {
                    Ok(()) => format!("Saved USD ({} prims) to {}", stage.prims.len(), file.trim()),
                    Err(e) => format!("USD export failed: {e}"),
                };
            }
        }
        if let Some(id) = self.pyro_fit_request.take() {
            let source = self.graph.cook_fresh(self.graph.input_of(id, 0), 0.0);
            match (source.bounds(), self.graph.nodes.get_mut(&id).map(|n| &mut n.kind)) {
                (Some((lo, hi)), Some(NodeKind::Pyro(set))) => {
                    let extent = hi.sub(lo);
                    let width = (extent.x.max(extent.z) * 3.0).max(2.0);
                    let height = (extent.y * 2.0 + width * 1.5).max(3.0);
                    set.size = [width.max(extent.x * 3.0), height, width.max(extent.z * 3.0)];
                    set.center = [(lo.x + hi.x) * 0.5, lo.y - 0.1 * height + height * 0.5, (lo.z + hi.z) * 0.5];
                    self.dirty = true;
                }
                _ => self.status = "Pyro: connect a source to input 1 first".into(),
            }
        }
        if self.moonray_render_requested {
            self.moonray_render_requested = false;
            self.start_moonray_render();
        }

        self.render_settings_window(ctx);
        self.viewer.ui(ctx);


        if let Some(node_id) = self.inspect_node {
            let mut open = true;
            let node_name = self
                .graph
                .nodes
                .get(&node_id)
                .map(|node| node.kind.label(node_id))
                .unwrap_or_else(|| format!("Node {node_id}"));
            let mut cache = HashMap::new();
            let mesh = self.graph.cook(node_id, self.time, &mut cache);
            egui::Window::new(format!("Geometry Info · {node_name}"))
                .open(&mut open)
                .default_size([320.0, 260.0])
                .show(ctx, |ui| {
                    ui.heading(node_name);
                    ui.label(format!("Points: {}", mesh.vertices.len()));
                    ui.label(format!("Primitives: {}", mesh.indices.len()));
                    let edges: HashSet<(usize, usize)> = mesh
                        .indices
                        .iter()
                        .flat_map(|tri| [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])])
                        .map(|(a, b)| (a.min(b), a.max(b)))
                        .collect();
                    ui.label(format!("Edges: {}", edges.len()));
                    if let Some((lo, hi)) = mesh.bounds() {
                        ui.label(format!(
                            "Bounds: ({:.3}, {:.3}, {:.3}) to ({:.3}, {:.3}, {:.3})",
                            lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
                        ));
                    }
                    ui.separator();
                    ui.label("Point attributes:");
                    if mesh.point_attributes.is_empty() {
                        ui.small("None");
                    } else {
                        let mut names: Vec<_> = mesh.point_attributes.keys().collect();
                        names.sort();
                        for name in names {
                            let values = &mesh.point_attributes[name];
                            let components = if mesh.vertices.is_empty() {
                                0
                            } else {
                                values.len() / mesh.vertices.len()
                            };
                            ui.label(format!(
                                "• {name}: {} component{}",
                                components,
                                if components == 1 { "" } else { "s" }
                            ));
                        }
                    }
                    let warnings = mesh.validation_warnings();
                    if warnings.is_empty() {
                        ui.colored_label(egui::Color32::LIGHT_GREEN, "Validation: OK");
                    } else {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!("Validation: {}", warnings.join(" · ")),
                        );
                    }
                });
            if !open {
                self.inspect_node = None;
            }
        }

        self.end_animation_frame();

        // Pause on edit: any change to the scene (parameters, wiring, flags, gizmo drags, undo)
        // stops playback, not only edits in the parameter panel.
        let signature = self.graph.content_signature(self.active_camera);
        if self.last_content_signature.is_some_and(|last| last != signature) && self.pause_on_edit && self.animate {
            self.animate = false;
            self.status = "Playback paused: the scene was edited (turn off Pause on edit to keep playing)".into();
        }
        self.last_content_signature = Some(signature);

        // Undo snapshots: commit once the mouse is released (one step per drag)
        if !ctx.input(|i| i.pointer.any_down()) {
            let s = self.graph.serialize();
            if s != self.last_state {
                self.undo.push(std::mem::replace(&mut self.last_state, s));
                self.redo.clear();
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    // `--usd-info <file>` lists a USD file's prims and exits (checks USD works in a packaged app).
    let args: Vec<String> = std::env::args().collect();
    if let Some(at) = args.iter().position(|a| a == "--usd-info") {
        let Some(file) = args.get(at + 1) else {
            eprintln!("usage: --usd-info <file.usd>");
            std::process::exit(2);
        };
        match usd::load_file(file, 0.0) {
            Ok(stage) => {
                for (path, prim) in &stage.prims {
                    println!("{path} {}", prim.type_name);
                }
                println!("{} prims", stage.prims.len());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("ProceduralFX Studio")
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "ProceduralFX Studio",
        options,
        Box::new(|_cc| Ok(Box::new(ProceduralFxStudioApp::default()))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materials_and_render_options_round_trip() {
        let mut g = Graph::default();
        let m = g.add(NodeKind::Material { kind: 2, color: [0.5, 0.6, 0.7], roughness: 0.1, value: 1.4 }, egui::pos2(0.0, 0.0));
        g.add(NodeKind::Geometry { material: Some(m) }, egui::pos2(0.0, 0.0));
        let opts = RenderOpts {
            camera: RenderCamera::Node(7),
            max_depth: 9,
            light_samples: 3,
            bsdf_samples: 4,
            background: [0.1, 0.2, 0.3],
            sequence: true,
            range_start: 0.5,
            range_end: 2.0,
            range_step: 0.25,
            stage: true,
            volume_samples: 3,
        };
        let r = g.add(
            NodeKind::MoonRayRender {
                executable: "moonray".into(),
                arguments: "{scene}".into(),
                output: "out.exr".into(),
                width: 320,
                height: 200,
                samples: 16,
                opts: opts.clone(),
            },
            egui::pos2(0.0, 0.0),
        );
        g.nodes.get_mut(&r).unwrap().context = AppContext::Out;
        let loaded = Graph::deserialize(&g.serialize());
        assert_eq!(loaded.serialize(), g.serialize());
        assert!(matches!(&loaded.nodes[&r].kind, NodeKind::MoonRayRender { opts: o, .. } if *o == opts));
        assert!(matches!(loaded.nodes[&m].kind, NodeKind::Material { kind: 2, .. }));
    }

    #[test]
    fn frame_ranges_and_output_names() {
        assert_eq!(frame_times(0.0, 1.0, 0.25), vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(frame_times(2.0, 1.0, 0.5).len(), 1);
        assert_eq!(frame_output("/out/render.exr", 7), "/out/render.0007.exr");
        assert_eq!(frame_output("/out/shot_####.png", 12), "/out/shot_0012.png");
        assert_eq!(frame_output("beauty", 3), "beauty.0003");
    }

    #[test]
    fn flat_scene_is_wrapped_in_a_geometry_container() {
        let legacy = "mhip 1\n\
            node 0 0 0 0 obj grid 4 20\n\
            node 1 0 80 0 obj twist 1.8\n\
            node 2 0 0 0 obj light 3 4 3 -35 35 0 1 0.95 0.85 0.9 0\n\
            link 0 1 0\n\
            display 1\n\
            next 3\n";
        let g = Graph::deserialize(legacy);
        let geo = g.geometry_ids();
        assert_eq!(geo.len(), 1);
        assert_eq!(g.nodes[&0].parent, Some(geo[0]));
        assert_eq!(g.nodes[&1].parent, Some(geo[0]));
        assert_eq!(g.nodes[&2].parent, None);
        assert_eq!(g.display_of(geo[0]), Some(1));
        assert!(!g.cook_many(&geo, 0.0, &mut HashMap::new()).vertices.is_empty());

        let round_trip = Graph::deserialize(&g.serialize());
        assert_eq!(round_trip.serialize(), g.serialize());
    }

    #[test]
    fn duplicating_and_removing_geometry_carries_its_network() {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let grid = g.add(NodeKind::Grid { size: 1.0, res: 2 }, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
        g.set_display(grid);
        let copy = g.duplicate(geo).unwrap();
        assert_ne!(g.display_of(copy), Some(grid));
        assert!(g.display_of(copy).is_some());
        g.remove(geo);
        assert!(!g.nodes.contains_key(&grid));
        assert_eq!(g.nodes.len(), 2);
    }

    fn quad() -> Mesh {
        Mesh {
            vertices: vec![Vec3::zero(), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0)],
            indices: vec![[0, 1, 2], [0, 2, 3]],
            point_attributes: HashMap::new(),
        }
    }

    #[test]
    fn merge_keeps_primitive_vertex_and_detail_attributes_sized() {
        let mut a = quad();
        a.point_attributes.insert("prim:Cd".into(), vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        a.point_attributes.insert("detail:total".into(), vec![7.0]);
        let mut b = quad();
        b.point_attributes.insert("pscale".into(), vec![2.0; 4]);
        b.point_attributes.insert("vertex:uvw".into(), vec![0.5; 6]);
        let merged = node_merge(a, b);
        let attrs = &merged.point_attributes;
        assert_eq!(attrs["prim:Cd"], vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(attrs["pscale"], vec![0.0, 0.0, 0.0, 0.0, 2.0, 2.0, 2.0, 2.0]);
        assert_eq!(attrs["vertex:uvw"].len(), 12);
        assert_eq!(attrs["detail:total"], vec![7.0]);
    }

    #[test]
    fn spreadsheet_lists_attributes_created_by_a_wrangle() {
        let mesh = node_wrangle(quad(), "@Cd = {1, 0, 0}; f@mass = 2.5; i@group = @ptnum;", 0.0, 2, None, None);
        let mesh = node_wrangle(mesh, "f@area = 0.5;", 0.0, 1, None, None);
        let mesh = node_wrangle(mesh, "i@count = npoints(0);", 0.0, 0, None, None);
        let headers = |domain| sheet_columns(&mesh, domain).into_iter().map(|c| c.header).collect::<Vec<_>>();
        let points = headers(SheetDomain::Points);
        for expected in ["#", "P.x", "N.z", "Cd.r", "Cd.b", "group", "mass"] {
            assert!(points.contains(&expected.to_string()), "{expected} missing from {points:?}");
        }
        assert_eq!(headers(SheetDomain::Primitives), vec!["#", "Points", "area"]);
        assert_eq!(headers(SheetDomain::Detail), vec!["count"]);
        let columns = sheet_columns(&mesh, SheetDomain::Points);
        let group = columns.iter().find(|c| c.header == "group").unwrap();
        assert_eq!(sheet_cell(&mesh, &[], &group.source, 3), "3");
        let mass = columns.iter().find(|c| c.header == "mass").unwrap();
        assert_eq!(sheet_cell(&mesh, &[], &mass.source, 1), "2.500");
    }


    fn solver_scene() -> (Graph, NodeId) {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let grid = g.add(NodeKind::Grid { size: 1.0, res: 2 }, egui::pos2(0.0, 0.0));
        let solver = g.add(NodeKind::SopSolver { substeps: 1 }, egui::pos2(0.0, 80.0));
        let prev = g.add(NodeKind::SolverInput { index: 0 }, egui::pos2(0.0, 0.0));
        let wrangle = g.add(NodeKind::Wrangle { code: "@P.y += 1;".into(), domain: 2 }, egui::pos2(0.0, 80.0));
        for (node, parent) in [(grid, geo), (solver, geo), (prev, solver), (wrangle, solver)] {
            g.nodes.get_mut(&node).unwrap().parent = Some(parent);
        }
        g.connect(grid, solver, 0);
        g.connect(prev, wrangle, 0);
        g.set_display(wrangle);
        g.set_display(solver);
        (g, solver)
    }

    #[test]
    fn sop_solver_feeds_its_output_back_every_step() {
        let (g, solver) = solver_scene();
        let height = |t: f32| g.cook(solver, t, &mut HashMap::new()).vertices[0].y;
        assert_eq!(height(0.0), 0.0, "time 0 is the input");
        assert_eq!(height(1.0), sims::SIM_FPS);
        assert_eq!(height(0.5), sims::SIM_FPS / 2.0, "scrubbing back resumes from a checkpoint");
        assert!(g.time_dependent(solver));
    }

    #[test]
    fn solver_nodes_survive_save_and_load() {
        let (mut g, solver) = solver_scene();
        let geo = g.nodes[&solver].parent;
        let pyro = g.add(NodeKind::Pyro(sims::PyroSettings { fuel: 0.5, ..Default::default() }), egui::pos2(0.0, 0.0));
        let rbd = g.add(NodeKind::RbdSolver(sims::RbdSettings { shape: 2, ..Default::default() }), egui::pos2(0.0, 0.0));
        for node in [pyro, rbd] {
            g.nodes.get_mut(&node).unwrap().parent = geo;
        }
        let loaded = Graph::deserialize(&g.serialize());
        assert!(matches!(&loaded.nodes[&pyro].kind, NodeKind::Pyro(s) if s.fuel == 0.5));
        assert!(matches!(&loaded.nodes[&rbd].kind, NodeKind::RbdSolver(s) if s.shape == 2));
        assert!(matches!(loaded.nodes[&solver].kind, NodeKind::SopSolver { substeps: 1 }));
        assert_eq!(loaded.children(solver).len(), 2);
        assert_eq!(loaded.sim_key(solver), g.sim_key(solver), "same scene, same simulation");
    }


    #[test]
    fn time_inc_inside_a_sop_solver_is_the_step_length() {
        let (mut g, solver) = solver_scene();
        let wrangle = g.display_of(solver).unwrap();
        if let Some(NodeKind::Wrangle { code, .. }) = g.nodes.get_mut(&wrangle).map(|n| &mut n.kind) {
            *code = "float speed = 2.0; // Units per second\n@P.y += speed * @TimeInc;".into();
        }
        if let Some(NodeKind::SopSolver { substeps }) = g.nodes.get_mut(&solver).map(|n| &mut n.kind) {
            *substeps = 4;
        }
        let height = g.cook(solver, 1.5, &mut HashMap::new()).vertices[0].y;
        assert!((height - 3.0).abs() < 1e-3, "2 units/s for 1.5 s, got {height}");
    }


    #[test]
    fn scenes_with_the_removed_fluid_node_still_load() {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let grid = g.add(NodeKind::Grid { size: 1.0, res: 2 }, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
        g.set_display(grid);
        // An old file: a fluid node wired into the grid's network and holding the display flag.
        let old = g.serialize().replace(
            "next ",
            &format!("node 90 0 0 0 obj fluid 0 20 1 1 0.5 1 7\nparent 90 {geo}\nlink 90 {grid} 0\ndisplay {geo} 90\nnext "),
        );
        let loaded = Graph::deserialize(&old);
        assert!(!loaded.nodes.contains_key(&90));
        assert!(loaded.links.iter().all(|l| l.from != 90 && l.to != 90));
        assert!(loaded.nodes.contains_key(&grid));
    }


    // geometry object (grid) + Material, and a STAGE network importing it with a sphere, a light,
    // a transform, a material assignment and a prune.
    fn usd_scene() -> (Graph, NodeId) {
        let mut g = Graph::default();
        let material = g.add(NodeKind::Material { kind: 1, color: [0.9, 0.2, 0.1], roughness: 0.3, value: 0.5 }, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&material).unwrap().context = AppContext::Mat;
        let geo = g.add(NodeKind::Geometry { material: Some(material) }, egui::pos2(0.0, 0.0));
        let grid = g.add(NodeKind::Grid { size: 2.0, res: 2 }, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
        g.set_display(grid);
        let lops = [
            NodeKind::LopSopImport { geometry: None, path: "/geo".into() },
            NodeKind::LopPrimitive { shape: 0, path: "/shapes/ball".into(), size: 0.5, t: [0.0, 2.0, 0.0] },
            NodeKind::LopLight { kind: 0, path: "/lights/key".into(), position: [0.0, 5.0, 0.0], target: [0.0; 3], color: [1.0; 3], intensity: 2.0, size: 1.0 },
            NodeKind::LopTransform { pattern: "/shapes/*".into(), t: [1.0, 0.0, 0.0], r: [0.0; 3], s: [1.0; 3] },
            NodeKind::LopMaterial { pattern: "/shapes/**".into(), material: Some(material) },
            NodeKind::LopPrune { pattern: "/lights/key".into(), hide: true },
        ];
        let mut previous = None;
        let mut last = 0;
        for kind in lops {
            let id = g.add(kind, egui::pos2(0.0, 0.0));
            g.nodes.get_mut(&id).unwrap().context = AppContext::Stage;
            if let Some(p) = previous {
                g.connect(p, id, 0);
            }
            previous = Some(id);
            last = id;
        }
        assert!(g.set_display(last));
        (g, last)
    }

    #[test]
    fn lop_network_builds_a_stage() {
        let (g, last) = usd_scene();
        let stage = g.cook_stage(last, 0.0, &mut HashMap::new());
        let geo_prim = stage.prims.keys().find(|p| p.starts_with("/geo/geometry")).cloned().expect("imported mesh");
        assert_eq!(stage.prims[&geo_prim].type_name, "Mesh");
        assert!(stage.prims[&geo_prim].binding.as_deref().is_some_and(|b| b.starts_with("/materials/mat")));
        assert_eq!(stage.world_xform("/shapes/ball")[3], [1.0, 2.0, 0.0, 1.0]);
        assert_eq!(stage.prims["/shapes/ball"].binding, stage.prims[&geo_prim].binding);
        assert!(!stage.prims["/lights/key"].visible);
        // The node's mesh output is the flattened stage, coloured from the materials.
        let mesh = g.cook(last, 0.0, &mut HashMap::new());
        assert!(mesh.indices.len() > 8);
        assert!(mesh.point_attributes.contains_key("vertex:Cd"));
        assert_eq!(g.display_of(STAGE_NETWORK), Some(last));
    }

    #[test]
    fn lop_nodes_save_and_load_with_their_text() {
        let (g, last) = usd_scene();
        let loaded = Graph::deserialize(&g.serialize());
        assert!(matches!(&loaded.nodes[&last].kind, NodeKind::LopPrune { pattern, hide: true } if pattern == "/lights/key"));
        assert_eq!(loaded.nodes[&last].context, AppContext::Stage);
        assert_eq!(loaded.display_of(STAGE_NETWORK), Some(last));
        let stage = loaded.cook_stage(last, 0.0, &mut HashMap::new());
        assert!(stage.prims.contains_key("/shapes/ball"));
    }

    #[test]
    fn usd_files_round_trip_through_openusd() {
        let (g, last) = usd_scene();
        let stage = g.cook_stage(last, 0.0, &mut HashMap::new());
        let dir = std::env::temp_dir();
        for extension in ["usda", "usdc", "usdz"] {
            let path = dir.join(format!("pfx_round_trip.{extension}"));
            let path = path.to_string_lossy().into_owned();
            let saved = usd::save(&stage, &path);
            if !usd::NATIVE {
                assert_eq!(saved.is_ok(), extension == "usda");
                continue;
            }
            saved.unwrap_or_else(|e| panic!("save {extension}: {e}"));
            let loaded = usd::load_file(&path, 0.0).unwrap_or_else(|e| panic!("load {extension}: {e}"));
            let geo_prim = stage.prims.keys().find(|p| p.starts_with("/geo/geometry")).unwrap();
            let (a, b) = (stage.prims[geo_prim].mesh.as_ref().unwrap(), loaded.prims[geo_prim].mesh.as_ref().unwrap());
            assert_eq!(a.vertices, b.vertices, "{extension}: points come back in app space");
            assert_eq!(a.indices, b.indices, "{extension}: winding survives the mirror");
            assert_eq!(loaded.world_xform("/shapes/ball")[3], [1.0, 2.0, 0.0, 1.0], "{extension}");
            assert_eq!(loaded.prims["/shapes/ball"].float("radius"), Some(0.5), "{extension}");
            assert!(!loaded.prims["/lights/key"].visible, "{extension}");
            let material = loaded.prims[geo_prim].binding.clone().expect("binding survives");
            assert_eq!(loaded.prims[&material].vec3("diffuseColor"), Some([0.9, 0.2, 0.1]), "{extension}");
            assert_eq!(loaded.prims[&material].float("metallic"), Some(1.0), "{extension}");
            let _ = std::fs::remove_file(&path);
        }
    }


    #[test]
    fn vdb_from_polygons_then_convert_vdb() {
        let mut g = Graph::default();
        let sphere = g.add(NodeKind::Sphere { radius: 1.0, rows: 24, cols: 48 }, egui::pos2(0.0, 0.0));
        let vdb = g.add(NodeKind::Vdb { voxel_size: 0.08, padding: 0, operation: 0, iterations: 1, output: 1 }, egui::pos2(0.0, 0.0));
        let to_polygons = g.add(NodeKind::ConvertVdb { mode: 0, iso: 0.0 }, egui::pos2(0.0, 0.0));
        let to_fog = g.add(NodeKind::ConvertVdb { mode: 1, iso: 0.0 }, egui::pos2(0.0, 0.0));
        g.connect(sphere, vdb, 0);
        g.connect(vdb, to_polygons, 0);
        g.connect(vdb, to_fog, 0);
        let level_set = g.cook(vdb, 0.0, &mut HashMap::new());
        assert!(volume::is_volume(&level_set) && level_set.point_attributes.contains_key("surface"));
        let surface = g.cook(to_polygons, 0.0, &mut HashMap::new());
        assert!(!surface.indices.is_empty());
        assert!(surface.vertices.iter().all(|v| (v.length() - 1.0).abs() < 0.12), "surface should match the sphere");
        let fog = g.cook(to_fog, 0.0, &mut HashMap::new());
        assert!(fog.point_attributes["density"].iter().any(|d| *d > 0.99));
        // Convert VDB on plain polygons reports an error instead of guessing.
        let bad = g.add(NodeKind::ConvertVdb { mode: 0, iso: 0.0 }, egui::pos2(0.0, 0.0));
        g.connect(sphere, bad, 0);
        assert!(g.cook(bad, 0.0, &mut HashMap::new()).vertices.is_empty());
        assert!(node_errors(|e| e.get(&bad).cloned()).is_some_and(|e| e.contains("not a VDB")));
    }

    #[test]
    fn file_nodes_write_and_read_every_format() {
        let mut g = Graph::default();
        let source = g.add(NodeKind::Torus { major: 1.0, minor: 0.3, rows: 8, cols: 16 }, egui::pos2(0.0, 0.0));
        let original = g.cook(source, 0.0, &mut HashMap::new());
        let dir = std::env::temp_dir();
        let mut formats = vec!["obj", "fbx", "stl", "usda"];
        if usd::NATIVE {
            formats.push("usdc");
        }
        for extension in formats {
            let path = dir.join(format!("pfx_file_node.{extension}")).to_string_lossy().into_owned();
            write_geometry_file(&original, &path).unwrap_or_else(|e| panic!("write {extension}: {e}"));
            let reader = g.add(NodeKind::FileImport { file: path.clone() }, egui::pos2(0.0, 0.0));
            let back = g.cook(reader, 0.0, &mut HashMap::new());
            if extension.starts_with("usd") && !usd::NATIVE {
                continue;
            }
            assert_eq!(back.indices.len(), original.indices.len(), "{extension}");
            let (lo, hi) = (back.bounds().unwrap(), original.bounds().unwrap());
            assert!(lo.0.sub(hi.0).length() < 1e-4 && lo.1.sub(hi.1).length() < 1e-4, "{extension} bounds");
            let _ = std::fs::remove_file(&path);
        }
        // $F sequences.
        let pattern = dir.join("pfx_seq.$F4.obj").to_string_lossy().into_owned();
        assert!(fileio::expand_frame(&pattern, 1.0, sims::SIM_FPS).ends_with("pfx_seq.0025.obj"));
        let reader = g.add(NodeKind::FileImport { file: pattern }, egui::pos2(0.0, 0.0));
        assert!(g.time_dependent(reader));
    }

    #[test]
    fn vdb_files_round_trip_through_openvdb() {
        let sphere = node_sphere(1.0, 16, 32);
        let level_set = volume::to_mesh(&volume::from_polygons(&sphere, 0.1, 3.0).unwrap());
        let path = std::env::temp_dir().join("pfx_round_trip.vdb").to_string_lossy().into_owned();
        let written = write_geometry_file(&level_set, &path);
        if !cfg!(feature = "native-openvdb") {
            assert!(written.is_err());
            return;
        }
        written.unwrap();
        let back = read_geometry_file(&path, 0.0).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.vertices.len(), level_set.vertices.len());
        let surface = volume::to_polygons(&volume::from_mesh(&back).unwrap(), 0.0);
        assert!(surface.vertices.iter().all(|v| (v.length() - 1.0).abs() < 0.12));
    }


    #[test]
    fn object_transforms_apply_and_persist() {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let grid = g.add(NodeKind::Grid { size: 2.0, res: 2 }, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
        g.set_display(grid);
        g.nodes.get_mut(&geo).unwrap().xform = ObjectXform { t: [0.0, 3.0, 0.0], r: [0.0; 3], s: [2.0; 3] };
        let (lo, hi) = g.cook(geo, 0.0, &mut HashMap::new()).bounds().unwrap();
        assert!((lo.y - 3.0).abs() < 1e-5 && (hi.x - 2.0).abs() < 1e-5, "{lo:?} {hi:?}");
        // The display inside the network is shown where the object is.
        assert_eq!(g.cook_display(&[grid], 0.0, &mut HashMap::new()).bounds(), Some((lo, hi)));
        let loaded = Graph::deserialize(&g.serialize());
        assert_eq!(loaded.nodes[&geo].xform, g.nodes[&geo].xform);
        assert!(loaded.nodes[&grid].xform.is_identity());
    }

    #[test]
    fn gizmo_drags_edit_the_selected_object() {
        let mut app = ProceduralFxStudioApp::default();
        let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        app.selected = Some(geo);
        app.context = AppContext::Obj;
        let frame = GizmoFrame {
            node: geo,
            pivot: egui::pos2(100.0, 100.0),
            pixels_per_unit: 100.0,
            cam_right: Vec3::new(1.0, 0.0, 0.0),
            cam_up: Vec3::new(0.0, 1.0, 0.0),
            handle_px: 85.0,
            parts: Vec::new(),
        };
        let axis = GizmoPart { handle: GizmoHandle::Axis(0), points: vec![], axis_screen: egui::vec2(100.0, 0.0), ring_sign: 1.0 };
        let (_, start) = app.gizmo_target().unwrap();
        let drag = GizmoDrag { part: axis.clone(), frame: frame.clone(), start_pointer: egui::pos2(100.0, 100.0), start };
        app.gizmo_mode = GizmoMode::Translate;
        app.apply_gizmo_drag(&drag, egui::pos2(150.0, 130.0));
        assert!((app.graph.nodes[&geo].xform.t[0] - 0.5).abs() < 1e-5);
        app.gizmo_mode = GizmoMode::Scale;
        app.apply_gizmo_drag(&drag, egui::pos2(185.0, 100.0));
        assert!((app.graph.nodes[&geo].xform.s[0] - 2.0).abs() < 1e-4);
        let ring = GizmoPart { handle: GizmoHandle::Ring(1), points: vec![], axis_screen: egui::Vec2::ZERO, ring_sign: 1.0 };
        let drag = GizmoDrag { part: ring, frame, start_pointer: egui::pos2(200.0, 100.0), start };
        app.gizmo_mode = GizmoMode::Rotate;
        app.apply_gizmo_drag(&drag, egui::pos2(100.0, 200.0));
        assert!((app.graph.nodes[&geo].xform.r[1] - 90.0).abs() < 1e-3);
    }


    #[test]
    fn level_set_vdbs_display_as_surfaces_and_fog_volumes_render() {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let sphere = g.add(NodeKind::Sphere { radius: 1.0, rows: 16, cols: 32 }, egui::pos2(0.0, 0.0));
        let vdb = g.add(NodeKind::Vdb { voxel_size: 0.1, padding: 0, operation: 0, iterations: 1, output: 1 }, egui::pos2(0.0, 0.0));
        for n in [sphere, vdb] {
            g.nodes.get_mut(&n).unwrap().parent = Some(geo);
        }
        g.connect(sphere, vdb, 0);
        g.set_display(vdb);
        let shown = g.cook_display(&[vdb], 0.0, &mut HashMap::new());
        assert!(!shown.indices.is_empty(), "a level set is displayed as its surface");
        assert!(!shown.point_attributes.contains_key("Alpha"));
        // Fog volumes go to MoonRay as .vdb files.
        let fog = g.add(NodeKind::ConvertVdb { mode: 1, iso: 0.0 }, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&fog).unwrap().parent = Some(geo);
        g.connect(vdb, fog, 0);
        g.set_display(fog);
        let mut app = ProceduralFxStudioApp::default();
        app.graph = g;
        app.current_network = None;
        app.render_opts = RenderOpts::default();
        // Cook for an explicit time rather than reusing the viewport mesh.
        app.render_at = Some(0.0);
        let scene = app.render_scene();
        if cfg!(feature = "native-openvdb") {
            let scene = scene.unwrap();
            assert_eq!((scene.volumes.len(), scene.parts.len()), (1, 0));
            assert!(std::path::Path::new(&scene.volumes[0].file).exists());
            let _ = std::fs::remove_file(&scene.volumes[0].file);
        } else {
            assert!(scene.is_err());
        }
    }


    #[test]
    fn content_signature_ignores_layout_but_sees_edits() {
        let (mut g, solver) = solver_scene();
        let before = g.content_signature(None);
        g.nodes.get_mut(&solver).unwrap().pos += egui::vec2(50.0, 10.0);
        assert_eq!(g.content_signature(None), before, "moving a node in the editor is not an edit");
        g.nodes.get_mut(&solver).unwrap().bypass = true;
        assert_ne!(g.content_signature(None), before);
        g.nodes.get_mut(&solver).unwrap().bypass = false;
        g.nodes.get_mut(&solver).unwrap().xform.t[1] = 2.0;
        assert_ne!(g.content_signature(None), before);
        assert_eq!(g.content_signature(Some(solver)), solver_scene().0.content_signature(Some(solver)), "the skipped node is ignored");
    }


    fn animated_box() -> (Graph, NodeId, NodeId) {
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let shape = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&shape).unwrap().parent = Some(geo);
        g.set_display(shape);
        let ty = ChannelId { node: geo, target: ChannelTarget::Xform(1) };
        let mut channel = anim::Channel::constant(0.0, 0.0);
        channel.set_key(1.0, 4.0);
        channel.keys.iter_mut().for_each(|k| k.interp = anim::Interp::Linear);
        g.channels.insert(ty, channel);
        let light = g.add(NodeKind::Light { position: [0.0; 3], rotation: [0.0; 3], color: [1.0; 3], intensity: 1.0, kind: 0 }, egui::pos2(0.0, 0.0));
        let mut fade = anim::Channel::constant(0.0, 1.0);
        fade.set_key(2.0, 0.0);
        g.channels.insert(ChannelId { node: light, target: ChannelTarget::Param(9) }, fade);
        (g, geo, light)
    }

    #[test]
    fn keyframed_objects_animate_in_cooks() {
        let (g, geo, light) = animated_box();
        let centre_y = |t: f32| {
            let (lo, hi) = g.cook(geo, t, &mut HashMap::new()).bounds().unwrap();
            (lo.y + hi.y) * 0.5
        };
        assert!((centre_y(0.0) - 0.0).abs() < 1e-5);
        assert!((centre_y(0.5) - 2.0).abs() < 1e-5);
        assert!((centre_y(3.0) - 4.0).abs() < 1e-5, "held after the last key");
        assert!(g.time_dependent(geo));
        assert!(matches!(g.kind_at(light, 2.0).as_ref(), NodeKind::Light { intensity, .. } if intensity.abs() < 1e-5));
    }

    #[test]
    fn playback_is_not_an_edit_but_changes_are_keyed() {
        let (mut g, geo, light) = animated_box();
        let signature = g.content_signature(None);
        let saved = g.serialize();
        g.apply_animation(0.75);
        assert_eq!(g.content_signature(None), signature, "evaluating animation is not an edit");
        assert_eq!(g.serialize(), saved, "the saved scene does not depend on the current time");
        assert!((g.nodes[&geo].xform.t[1] - 3.0).abs() < 1e-5, "panels and gizmos see the animated value");
        // A gizmo or panel edit of an animated parameter keys the current frame.
        g.nodes.get_mut(&geo).unwrap().xform.t[1] = 10.0;
        assert!(g.commit_animation_edits(0.75));
        let channel = &g.channels[&ChannelId { node: geo, target: ChannelTarget::Xform(1) }];
        assert!(channel.has_key_at(0.75) && (channel.eval(0.75) - 10.0).abs() < 1e-5);
        assert!(!g.commit_animation_edits(0.75), "nothing more to key");
        // Channels survive saving, loading and duplication.
        let loaded = Graph::deserialize(&g.serialize());
        assert_eq!(loaded.channels, g.channels);
        let copy = g.duplicate(light).unwrap();
        assert!(g.is_animated(copy));
        g.remove(light);
        assert!(!g.channels.keys().any(|c| c.node == light));
    }


    #[test]
    fn buckets_cover_the_frame_once_from_the_centre() {
        for (w, h) in [(1280, 720), (333, 97), (64, 64)] {
            let buckets = bucket_layout(w, h);
            let mut covered = vec![0u8; w * h];
            for [x0, y0, x1, y1] in &buckets {
                for y in *y0..*y1 {
                    for x in *x0..*x1 {
                        covered[y * w + x] += 1;
                    }
                }
            }
            assert!(covered.iter().all(|c| *c == 1), "{w}x{h}");
            let first = buckets[0];
            assert!(first[0] <= w / 2 && w / 2 <= first[2] + 1 && first[1] <= h / 2 && h / 2 <= first[3] + 1, "starts in the centre");
        }
        assert_eq!(bucket_layout(1280, 720).len(), 24);
    }

}

#[cfg(test)]
mod scene_file_tests {
    use super::*;

    #[test]
    fn recent_scenes_dedupe_and_cap() {
        let text = "/a.pfs\n\n/b.pfs\n/a.pfs\n".to_owned() + &(0..20).map(|i| format!("/s{i}.pfs\n")).collect::<String>();
        let list = parse_recent_scenes(&text);
        assert_eq!(list.len(), MAX_RECENT_SCENES);
        assert_eq!(list[0], PathBuf::from("/a.pfs"));
        assert_eq!(list[1], PathBuf::from("/b.pfs"));

        let mut list = vec![PathBuf::from("/a.pfs"), PathBuf::from("/b.pfs")];
        push_recent_scene(&mut list, PathBuf::from("/b.pfs"));
        assert_eq!(list, vec![PathBuf::from("/b.pfs"), PathBuf::from("/a.pfs")]);
    }

    #[test]
    fn old_particle_nodes_load_with_defaults() {
        let old = NodeKind::from_params("particles", &[2000.0, 400.0, 4.0, 2.0, 0.35, -1.5, 1.0]).unwrap();
        let NodeKind::Particles { bounce, friction, radius, substeps, along_normals, drag, .. } = old else { panic!() };
        assert_eq!((bounce, friction, radius, substeps, along_normals, drag), (0.4, 0.1, 0.02, 2, false, 0.0));
        let round_trip = NodeKind::from_params("particles", &old.params()).unwrap();
        assert_eq!(format!("{round_trip:?}"), format!("{old:?}"));
    }

    #[test]
    fn new_nodes_survive_save_and_load() {
        let mut g = Graph::default();
        let painted = g.add(
            NodeKind::AttributePaint {
                attribute: "clump".into(),
                values: vec![0.0, 0.25, 1.0],
                settings: paint::PaintSettings { value: 0.8, default: 0.1, radius: 55.0, strength: 0.3, mode: 2, visualize: false },
            },
            egui::pos2(0.0, 0.0),
        );
        let kinds = [
            NodeKind::VellumSolver(vellum::VellumSettings { iterations: 33, ..vellum::VellumSettings::defaults(1) }),
            NodeKind::Normal(sops::NormalSettings { domain: 1, cusp_angle: 45.0, weighting: 2, reverse: true }),
            NodeKind::Scatter(sops::ScatterSettings { count: 77, seed: 4, relax_iterations: 2, volume: true }),
            NodeKind::UvTexture(sops::UvTextureSettings { projection: 1, axis: 2, scale: [2.0, 3.0], offset: [0.1, 0.2] }),
            NodeKind::UvTransform(sops::UvTransformSettings { translate: [1.0, 2.0], rotate: 30.0, scale: [0.5, 0.5], pivot: [0.5, 0.5] }),
            NodeKind::UvQuickShade { file: "/textures/grid.png".into() },
            NodeKind::Boolean(boolean::BooleanSettings { operation: 1, mode: 1, resolution: 90 }),
            NodeKind::MaterialFracture(fracture::FractureSettings { pieces: 33, splinter: 7.0, ..fracture::FractureSettings::defaults(2) }),
            NodeKind::CrowdSolver(crowd::CrowdSettings { goal: [1.0, 2.0, 3.0], display: 0, ..crowd::CrowdSettings::defaults() }),
            NodeKind::Muscle(muscle::MuscleSettings { activation: 0.4, profile: 1, ..muscle::MuscleSettings::defaults() }),
            NodeKind::MuscleSolver(muscle::JiggleSettings { stiffness: 77.0, ..muscle::JiggleSettings::defaults() }),
            NodeKind::MuscleSkin(muscle::SkinSettings { falloff: 0.7, influences: 3 }),
            NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings { lifespan: 3.5, ..whitewater::WhitewaterSettings::defaults() }),
            NodeKind::Shader(shading::Values::preset(shading::Table::Principled, 5)),
            NodeKind::Shader(shading::Values::preset(shading::Table::Pyro, 2)),
            NodeKind::Terrain(terrain::TerrainSettings { size: [12.0, 7.0], resolution: 300 }),
            NodeKind::TerrainNoise(terrain::NoiseSettings { kind: 3, blend: 2, offset: [1.0, -2.0], use_mask: true, ..terrain::NoiseSettings::defaults() }),
            NodeKind::TerrainErode(terrain::ErodeSettings { droplets: 1234, talus_angle: 33.0, ..terrain::ErodeSettings::defaults() }),
            NodeKind::TerrainTerrace(terrain::TerraceSettings { step: 0.3, offset: 0.1, sharpness: 0.9, use_mask: true }),
            NodeKind::TerrainMask(terrain::MaskSettings { source: 2, min: 0.2, max: 0.8, falloff: 0.1, blur: 3, invert: true, combine: 1, visualize: false }),
            NodeKind::TerrainColorize(terrain::ColorizeSettings { snow: [0.1, 0.2, 0.3], rock_slope: 50.0, ..terrain::ColorizeSettings::defaults() }),
        ];
        let ids: Vec<NodeId> = kinds.iter().cloned().map(|k| g.add(k, egui::pos2(0.0, 0.0))).collect();
        let loaded = Graph::deserialize(&g.serialize());
        assert_eq!(format!("{:?}", loaded.nodes[&painted].kind), format!("{:?}", g.nodes[&painted].kind));
        for id in ids {
            assert_eq!(format!("{:?}", loaded.nodes[&id].kind), format!("{:?}", g.nodes[&id].kind));
        }
    }

    #[test]
    fn boolean_and_terrain_nodes_cook_in_a_network() {
        let mut g = Graph::default();
        g.next_id += 9100;
        let a = g.add(NodeKind::Box { size: 2.0, div: 1 }, egui::pos2(0.0, 0.0));
        let b = g.add(NodeKind::Sphere { radius: 0.8, rows: 16, cols: 24 }, egui::pos2(0.0, 0.0));
        let cut = g.add(NodeKind::Boolean(boolean::BooleanSettings { operation: 2, mode: 0, resolution: 64 }), egui::pos2(0.0, 0.0));
        g.connect(a, cut, 0);
        let missing = g.cook(cut, 0.0, &mut HashMap::new());
        assert!(missing.indices.is_empty());
        assert!(node_errors(|e| e.get(&cut).cloned()).unwrap().contains("right input"));
        g.connect(b, cut, 1);
        let out = g.cook(cut, 0.0, &mut HashMap::new());
        assert!(!out.indices.is_empty() && node_errors(|e| e.get(&cut).cloned()).is_none());
        assert!(out.point_attributes.contains_key("prim:group:b_faces"));
        // Box minus sphere: 8 - 4/3·π·0.8³ (the sphere's facets make it a little smaller).
        let volume: f32 = out.indices.iter().map(|t| {
            let [p, q, r] = t.map(|i| out.vertices[i]);
            p.dot(q.cross(r)) / 6.0
        }).sum();
        assert!((volume - 5.86).abs() < 0.15, "volume {volume}");

        let land = g.add(NodeKind::Terrain(terrain::TerrainSettings { size: [10.0, 10.0], resolution: 64 }), egui::pos2(0.0, 0.0));
        let mut chain = vec![land];
        for kind in NodeKind::templates().into_iter().find(|(m, _)| *m == "Terrain").unwrap().1.into_iter().skip(1) {
            let node = g.add(kind, egui::pos2(0.0, 0.0));
            g.connect(*chain.last().unwrap(), node, 0);
            chain.push(node);
        }
        let out = g.cook(*chain.last().unwrap(), 0.0, &mut HashMap::new());
        for id in &chain {
            assert!(node_errors(|e| e.get(id).cloned()).is_none(), "{:?}", g.nodes[id].kind);
        }
        assert_eq!(out.vertices.len(), 65 * 65);
        assert!(out.vertices.iter().any(|v| v.y > 0.5));
        assert_eq!(out.point_attributes["Cd"].len(), out.vertices.len() * 3);
        assert!(out.point_attributes.contains_key("mask") && out.point_attributes.contains_key("flow"));
        // A non-terrain input is reported, not mangled.
        let grid = g.add(NodeKind::Grid { size: 1.0, res: 4 }, egui::pos2(0.0, 0.0));
        let noise = g.add(NodeKind::TerrainNoise(terrain::NoiseSettings::defaults()), egui::pos2(0.0, 0.0));
        g.connect(grid, noise, 0);
        g.cook(noise, 0.0, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&noise).cloned()).unwrap().contains("isn't a terrain"));
    }

    #[test]
    fn vellum_node_cooks_cloth_falling_onto_the_ground() {
        let mut g = Graph::default();
        let grid = g.add(NodeKind::Grid { size: 2.0, res: 8 }, egui::pos2(0.0, 0.0));
        let lift = g.add(NodeKind::Wrangle { code: "@P.y += 1.0;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
        let cloth = g.add(NodeKind::VellumSolver(vellum::VellumSettings::defaults(0)), egui::pos2(0.0, 0.0));
        g.connect(grid, lift, 0);
        g.connect(lift, cloth, 0);
        sims::clear_all();
        let start = g.cook(cloth, 0.0, &mut HashMap::new());
        let later = g.cook(cloth, 1.5, &mut HashMap::new());
        assert!(start.vertices.iter().all(|p| (p.y - 1.0).abs() < 1e-5));
        assert!(later.vertices.iter().all(|p| p.y < 0.2 && p.y > -1e-3), "lies on the ground");
        assert_eq!(later.indices, start.indices);
    }

    // The reported setup: a 4 x 4 grid (20 x 20 points) at y = 0 over a 1.5 box whose top is at y = -0.92.
    fn cloth_over_box(ground: bool) -> (Graph, NodeId, NodeId) {
        let mut g = Graph::default();
        // Distinct node ids per variant: node notes are shared by tests running in parallel.
        g.next_id = if ground { 500 } else { 600 };
        let grid = g.add(NodeKind::Grid { size: 4.0, res: 20 }, egui::pos2(0.0, 0.0));
        let cloth = g.add(NodeKind::VellumSolver(vellum::VellumSettings { ground, ..vellum::VellumSettings::defaults(0) }), egui::pos2(0.0, 0.0));
        let block = g.add(NodeKind::Box { size: 1.5, div: 4 }, egui::pos2(0.0, 0.0));
        let lower = g.add(NodeKind::Transform { t: [0.0, -1.67, 0.0], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
        g.connect(grid, cloth, 0);
        g.connect(block, lower, 0);
        g.connect(lower, cloth, 1);
        (g, grid, cloth)
    }

    #[test]
    fn vellum_cloth_drapes_over_a_lowered_box() {
        sims::clear_all();
        let (g, grid, cloth) = cloth_over_box(false);
        let rest = g.cook(grid, 0.0, &mut HashMap::new());
        let out = g.cook(cloth, 3.0, &mut HashMap::new());
        let centre = (0..rest.vertices.len()).min_by(|&a, &b| (rest.vertices[a].x.abs() + rest.vertices[a].z.abs()).total_cmp(&(rest.vertices[b].x.abs() + rest.vertices[b].z.abs()))).unwrap();
        assert!((out.vertices[centre].y + 0.90).abs() < 0.03, "rests on the box top: {:?}", out.vertices[centre]);
        assert!(out.vertices.iter().all(|p| !(p.x.abs() < 0.74 && p.z.abs() < 0.74 && p.y < -0.93 && p.y > -2.4)), "nothing inside the box");
        let stretch = rest
            .indices
            .iter()
            .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
            .map(|(a, b)| (out.vertices[a].sub(out.vertices[b]).length() / rest.vertices[a].sub(rest.vertices[b]).length() - 1.0).abs())
            .fold(0.0, f32::max);
        assert!(stretch < 0.08, "max stretch {:.1}%", stretch * 100.0);
        assert!(out.vertices.iter().map(|p| p.y).fold(f32::INFINITY, f32::min) < -2.0, "the sides hang down");
        assert!(node_notes(|n| n.get(&cloth).cloned()).is_none());
    }

    #[test]
    fn vellum_explains_geometry_resting_on_the_ground() {
        sims::clear_all();
        let (g, _, cloth) = cloth_over_box(true);
        let out = g.cook(cloth, 1.0, &mut HashMap::new());
        // It stays on the ground, without being launched off it...
        assert!(out.vertices.iter().all(|p| (p.y - 0.02).abs() < 1e-3), "{:?}", out.vertices[0]);
        // ...and the node says why it doesn't fall.
        assert!(node_notes(|n| n.get(&cloth).cloned()).is_some_and(|n| n.contains("ground plane")));
    }

    #[test]
    fn background_cook_names_its_solvers() {
        let (g, _, cloth) = cloth_over_box(false);
        assert_eq!(g.cooking_label(&[cloth]), "Simulating Vellum");
        let mut g = Graph::default();
        let grid = g.add(NodeKind::Grid { size: 1.0, res: 4 }, egui::pos2(0.0, 0.0));
        let wave = g.add(NodeKind::Ripple { freq: 1.0, amp: 0.1, speed: 1.0 }, egui::pos2(0.0, 0.0));
        g.connect(grid, wave, 0);
        assert_eq!(g.cooking_label(&[wave]), "Cooking the animated frame");
        let pyro = g.add(NodeKind::Pyro(sims::PyroSettings::default()), egui::pos2(0.0, 0.0));
        g.connect(wave, pyro, 0);
        let vellum = g.add(NodeKind::VellumSolver(vellum::VellumSettings::defaults(0)), egui::pos2(0.0, 0.0));
        g.connect(pyro, vellum, 0);
        assert_eq!(g.cooking_label(&[vellum]), "Simulating Pyro + Vellum");
    }

    #[test]
    fn atomic_save_replaces_file() {
        let path = std::env::temp_dir().join(format!("pfx_atomic_{}.pfs", std::process::id()));
        write_file_atomically(&path, "first").unwrap();
        write_file_atomically(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert!(!PathBuf::from(format!("{}.saving", path.display())).exists());
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod groom_render_tests {
    use super::*;

    fn skin() -> Mesh {
        let mut mesh = Mesh::default();
        mesh.vertices = vec![Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, 1.0)];
        mesh.indices = vec![[0, 2, 1], [0, 3, 2]];
        mesh
    }

    fn groomed(with_hair: bool) -> Mesh {
        let skin = skin();
        if !with_hair {
            return skin;
        }
        let guides = groom::hair_guides(&skin, &groom::GuideSettings { count: 30, length: 0.8, length_variance: 0.2, segments: 6, droop: 0.4, seed: 2 });
        let hair = groom::hair_generate(
            &skin,
            &guides,
            &groom::HairSettings { count: 3000, segments: 6, clump: 0.3, length_variance: 0.1, frizz: 0.2, width_root: 0.02, width_tip: 0.01, seed: 4 },
        );
        node_merge(skin, hair)
    }

    fn export(path: &std::path::Path, mesh: Mesh) -> String {
        let app = ProceduralFxStudioApp::default();
        let scene = RenderScene { parts: vec![(mesh, PartMaterial::Default)], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
        app.export_scene_rdla(path, &scene, 160, 120, 4).unwrap();
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn hair_exports_as_curve_geometry() {
        let path = std::env::temp_dir().join(format!("pfx_hair_{}.rdla", std::process::id()));
        let text = export(&path, groomed(true));
        assert!(text.contains("RdlMeshGeometry(\"/scene/objects/mesh0\") {"));
        assert!(text.contains("RdlCurveGeometry(\"/scene/objects/curves0\") {"));
        assert_eq!(text.matches("{RdlCurveGeometry(\"/scene/objects/curves0\")").count(), 1, "curves are in the layer");
        // Hair alone has no triangles: no mesh object is written for it.
        let text = export(&path, groom::hair_guides(&skin(), &groom::GuideSettings { count: 3, length: 1.0, length_variance: 0.0, segments: 2, droop: 0.0, seed: 1 }));
        assert!(!text.contains("RdlMeshGeometry"));
        assert!(text.contains("[\"curves_vertex_count\"] = {\n        3,\n        3,\n        3,\n    }"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hair_deform_node_follows_an_animated_skin() {
        let mut g = Graph::default();
        let grid = g.add(NodeKind::Grid { size: 2.0, res: 6 }, egui::pos2(0.0, 0.0));
        // The skin rises one unit per second.
        let skin = g.add(NodeKind::Wrangle { code: "@P.y += @Time;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
        let guides = g.add(
            NodeKind::HairGuides(groom::GuideSettings { count: 20, length: 0.5, length_variance: 0.0, segments: 4, droop: 0.0, seed: 1 }),
            egui::pos2(0.0, 0.0),
        );
        let deform = g.add(NodeKind::HairDeform(groom::DeformSettings { rest_frame: 1.0 }), egui::pos2(0.0, 0.0));
        g.connect(grid, skin, 0);
        g.connect(skin, guides, 0);
        g.connect(guides, deform, 0);
        g.connect(skin, deform, 1);
        assert!(g.time_dependent(deform));
        let rest = g.cook(deform, 0.0, &mut HashMap::new());
        let later = g.cook(deform, 1.5, &mut HashMap::new());
        assert!(!rest.vertices.is_empty());
        assert_eq!(rest.vertices.len(), later.vertices.len());
        for (a, b) in rest.vertices.iter().zip(&later.vertices) {
            assert!((b.y - a.y - 1.5).abs() < 1e-3 && (b.x - a.x).abs() < 1e-3, "{a:?} -> {b:?}");
        }
        // Without a skin the node reports an error instead of cooking.
        let lonely = g.add(NodeKind::HairDeform(groom::DeformSettings { rest_frame: 1.0 }), egui::pos2(0.0, 0.0));
        g.connect(guides, lonely, 0);
        assert!(g.cook(lonely, 0.0, &mut HashMap::new()).vertices.is_empty());
        assert!(node_errors(|e| e.get(&lonely).cloned()).is_some_and(|e| e.contains("input 2")));
    }

    // Renders with the bundled MoonRay: `cargo test -- --ignored hair_renders_in_moonray`.
    #[test]
    #[ignore]
    fn hair_renders_in_moonray() {
        let moonray = std::path::Path::new("native/moonray/macos/moonray");
        let dir = std::env::temp_dir();
        let render = |hair: bool| {
            let rdla = dir.join(format!("pfx_hair_render_{hair}.rdla"));
            let exr = dir.join(format!("pfx_hair_render_{hair}.exr"));
            export(&rdla, groomed(hair));
            let status = Command::new(moonray)
                .env("RDL2_DSO_PATH", std::env::var("RDL2_DSO_PATH").unwrap_or("/Users/loksangho/moonray/build/rdl2dso".into()))
                .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
                .output()
                .unwrap();
            assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
            image::open(&exr).unwrap().to_rgb32f()
        };
        let (bare, hairy) = (render(false), render(true));
        let changed = bare.pixels().zip(hairy.pixels()).filter(|(a, b)| (0..3).any(|c| (a[c] - b[c]).abs() > 0.02)).count();
        println!("{changed} of {} pixels changed by the hair", bare.pixels().len());
        assert!(changed > bare.pixels().len() / 50);
    }
}

#[cfg(test)]
mod envlight_render_tests {
    use super::*;

    // Checks the viewport's HDRI mapping against MoonRay: `cargo test -- --ignored hdri_matches_moonray`.
    #[test]
    #[ignore]
    fn hdri_matches_moonray() {
        let dir = std::env::temp_dir();
        // Each pixel's colour encodes its own image coordinates: R = u, G = v.
        let (w, h) = (256u32, 128u32);
        let mut img = image::Rgb32FImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, 0.25]);
        }
        let hdri = dir.join("pfx_uv_sky.exr");
        image::DynamicImage::ImageRgb32F(img).save(&hdri).unwrap();
        for (rot_x, rot_y, env_rotation) in [(0.3f32, 0.7f32, 0.0f32), (-0.2, 2.2, 0.0), (0.4, 0.7, 50.0)] {
            let mut app = ProceduralFxStudioApp::default();
            app.graph = Graph::default();
            app.graph.add(
                NodeKind::EnvironmentLight { file: hdri.to_string_lossy().into_owned(), color: [1.0; 3], intensity: 1.0, rotation: env_rotation, visible: true },
                egui::pos2(0.0, 0.0),
            );
            app.render_camera = RenderCamera::Viewport;
            (app.cam_rot_x, app.cam_rot_y) = (rot_x, rot_y);
            app.ambient_light = 0.0;
            // MoonRay only uses lights through a layer's geometry, so add a speck far out of view.
            let speck = Mesh {
                vertices: vec![Vec3::new(0.0, -900.0, 0.0), Vec3::new(0.001, -900.0, 0.0), Vec3::new(0.0, -900.0, 0.001)],
                indices: vec![[0, 1, 2]],
                ..Mesh::default()
            };
            let scene = RenderScene { parts: vec![(speck, PartMaterial::Default)], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
            let rdla = dir.join("pfx_uv_sky.rdla");
            let exr = dir.join("pfx_uv_sky_render.exr");
            app.export_scene_rdla(&rdla, &scene, 64, 48, 4).unwrap();
            let run = Command::new("native/moonray/macos/moonray")
                .env("RDL2_DSO_PATH", "/Users/loksangho/moonray/build/rdl2dso")
                .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
                .output()
                .unwrap();
            assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
            let render = image::open(&exr).unwrap().to_rgb32f();
            let centre = render.get_pixel(32, 24);
            // The viewport's forward ray (inverse of to_cam for the screen centre).
            let forward = Vec3::new(rot_x.cos() * rot_y.sin(), rot_x.sin(), rot_x.cos() * rot_y.cos());
            let (u, v) = envlight::uv_of(forward, env_rotation);
            println!("rot ({rot_x}, {rot_y}) env {env_rotation}: moonray ({:.3}, {:.3}) viewport ({u:.3}, {v:.3})", centre[0], centre[1]);
            assert!((centre[0] - u).abs() < 0.03 && (centre[1] - v).abs() < 0.03);
        }
    }
}

#[cfg(test)]
mod group_node_tests {
    use super::*;

    fn group_create(name: &str, domain: groups::GroupDomain, expression: &str) -> NodeKind {
        NodeKind::GroupCreate(groups::GroupCreateSettings {
            name: name.into(),
            domain,
            method: 3,
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
            percent: 0.0,
            seed: 0,
            direction: [0.0, 1.0, 0.0],
            spread: 0.0,
            expression: expression.into(),
            combine: 0,
            invert: false,
        })
    }

    // Grid -> Group Create (points with x > 0 as "right") -> `node`, with `pattern` as its group.
    fn chain(node: NodeKind, pattern: &str) -> (Graph, NodeId, NodeId) {
        let mut g = Graph::default();
        let grid = g.add(NodeKind::Grid { size: 2.0, res: 4 }, egui::pos2(0.0, 0.0));
        let make = g.add(group_create("right", groups::GroupDomain::Points, "@P.x>0.01"), egui::pos2(0.0, 0.0));
        let target = g.add(node, egui::pos2(0.0, 0.0));
        g.connect(grid, make, 0);
        g.connect(make, target, 0);
        g.nodes.get_mut(&target).unwrap().group = groups::NodeGroup { pattern: pattern.into(), kind: 0 };
        (g, make, target)
    }

    #[test]
    fn deformers_and_wrangles_only_touch_their_group() {
        let (g, make, transform) = chain(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [1.0; 3] }, "right");
        let before = g.cook(make, 0.0, &mut HashMap::new());
        let after = g.cook(transform, 0.0, &mut HashMap::new());
        for (a, b) in before.vertices.iter().zip(&after.vertices) {
            assert_eq!(b.y, if a.x > 0.01 { 1.0 } else { 0.0 }, "{a:?}");
        }
        // Wrangle: runs on the group's points only.
        let (g, make, wrangle) = chain(NodeKind::Wrangle { code: "f@hit = 1; @P.y = 2;".into(), domain: 2 }, "^right");
        let before = g.cook(make, 0.0, &mut HashMap::new());
        let after = g.cook(wrangle, 0.0, &mut HashMap::new());
        for (i, (a, b)) in before.vertices.iter().zip(&after.vertices).enumerate() {
            let inside = a.x <= 0.01;
            assert_eq!(b.y, if inside { 2.0 } else { 0.0 });
            assert_eq!(after.point_attributes["hit"][i], if inside { 1.0 } else { 0.0 });
        }
        // VEX group functions.
        let (g, _, tag) = chain(NodeKind::Wrangle { code: "i@r = inpointgroup(0, \"right\", @ptnum); setpointgroup(0, \"tagged\", @ptnum, @P.z > 0);".into(), domain: 2 }, "");
        let out = g.cook(tag, 0.0, &mut HashMap::new());
        for (i, p) in out.vertices.iter().enumerate() {
            assert_eq!(out.point_attributes["r"][i], (p.x > 0.01) as u8 as f32);
            assert_eq!(out.point_attributes["group:tagged"][i], (p.z > 0.0) as u8 as f32);
        }
        // A bad group is an error on the node.
        let (g, _, bad) = chain(NodeKind::Transform { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3] }, "nosuchgroup");
        assert!(g.cook(bad, 0.0, &mut HashMap::new()).vertices.is_empty());
        assert!(node_errors(|e| e.get(&bad).cloned()).is_some_and(|e| e.contains("nosuchgroup")));
    }

    #[test]
    fn blast_scatter_and_vellum_use_groups() {
        // Blast a primitive group.
        let mut g = Graph::default();
        let grid = g.add(NodeKind::Grid { size: 2.0, res: 4 }, egui::pos2(0.0, 0.0));
        let make = g.add(group_create("top", groups::GroupDomain::Prims, "@P.z>0"), egui::pos2(0.0, 0.0));
        let blast = g.add(NodeKind::Blast { component: 0, indices: Vec::new(), invert: false }, egui::pos2(0.0, 0.0));
        g.connect(grid, make, 0);
        g.connect(make, blast, 0);
        g.nodes.get_mut(&blast).unwrap().group = groups::NodeGroup { pattern: "top".into(), kind: 0 };
        let full = g.cook(make, 0.0, &mut HashMap::new());
        let blasted = g.cook(blast, 0.0, &mut HashMap::new());
        assert_eq!(blasted.indices.len(), full.indices.len() / 2);
        // Scatter only on that group.
        let scatter = g.add(NodeKind::Scatter(sops::ScatterSettings { count: 200, seed: 3, relax_iterations: 0, volume: false }), egui::pos2(0.0, 0.0));
        g.connect(make, scatter, 0);
        g.nodes.get_mut(&scatter).unwrap().group = groups::NodeGroup { pattern: "top".into(), kind: 0 };
        let points = g.cook(scatter, 0.0, &mut HashMap::new());
        assert_eq!(points.vertices.len(), 200);
        // Every point lies within the group's primitives (lowest z among their corners).
        let top = &full.point_attributes["prim:group:top"];
        let lowest = full.indices.iter().zip(top).filter(|(_, t)| **t > 0.5).flat_map(|(tri, _)| tri.map(|i| full.vertices[i].z)).fold(f32::INFINITY, f32::min);
        assert!(lowest > -0.5);
        assert!(points.vertices.iter().all(|p| p.z >= lowest - 1e-5));
        // Vellum pins its group: the right half stays, the rest falls.
        let (mut g, _, cloth) = chain(NodeKind::VellumSolver(vellum::VellumSettings { ground: false, ..vellum::VellumSettings::defaults(0) }), "right");
        g.nodes.get_mut(&cloth).unwrap().group.kind = 1;
        sims::clear_all();
        let out = g.cook(cloth, 1.0, &mut HashMap::new());
        let rest = g.cook(cloth, 0.0, &mut HashMap::new());
        for (a, b) in rest.vertices.iter().zip(&out.vertices) {
            if a.x > 0.01 {
                assert!(a.sub(*b).length() < 1e-5, "pinned point moved");
            }
        }
        assert!(out.vertices.iter().any(|p| p.y < -0.2), "the free half falls");
    }

    #[test]
    fn picking_a_group_in_the_viewport() {
        let mut app = ProceduralFxStudioApp::default();
        let (g, make, transform) = chain(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [1.0; 3] }, "right");
        app.graph = g;
        app.selected = Some(transform);
        app.start_group_pick(transform);
        let input = app.graph.cook(make, 0.0, &mut HashMap::new());
        // The viewport shows the node's input, with its current group selected.
        assert_eq!(app.current_mesh.vertices, input.vertices);
        let right: Vec<usize> = (0..input.vertices.len()).filter(|&i| input.vertices[i].x > 0.01).collect();
        assert_eq!(app.selected_vertices, right);
        assert!(app.selection_mode == SelectionMode::Vertex);
        app.rebuild_graph();
        assert_eq!(app.current_mesh.vertices, input.vertices, "the display is paused while picking");
        // Pick points 0-3 and accept.
        app.selected_vertices = vec![3, 1, 0, 2];
        app.finish_group_pick(true);
        assert!(app.group_pick.is_none());
        let group = &app.graph.nodes[&transform].group;
        assert_eq!(group.pattern, "0-3");
        assert_eq!(group.kind, 1);
        let out = app.graph.cook(transform, 0.0, &mut HashMap::new());
        for (i, p) in out.vertices.iter().enumerate() {
            assert_eq!(p.y, if i <= 3 { 1.0 } else { 0.0 });
        }
        // Primitives, then cancel: the group is left as it was.
        app.start_group_pick(transform);
        app.set_group_pick_domain(groups::GroupDomain::Prims);
        assert!(app.selection_mode == SelectionMode::Triangle);
        app.selected_triangles = vec![0, 1];
        app.finish_group_pick(false);
        assert_eq!(app.graph.nodes[&transform].group.pattern, "0-3");
        // Accepting primitives writes a primitive group.
        app.start_group_pick(transform);
        app.set_group_pick_domain(groups::GroupDomain::Prims);
        app.selected_triangles = vec![4, 5];
        app.finish_group_pick(true);
        assert_eq!(app.graph.nodes[&transform].group, groups::NodeGroup { pattern: "4-5".into(), kind: 2 });
    }

    #[test]
    fn groups_are_saved_with_the_scene() {
        let (g, make, target) = chain(NodeKind::Noise { freq: 1.0, amp: 0.2 }, "right ^0-3");
        let mut g = g;
        g.nodes.get_mut(&target).unwrap().group.kind = 1;
        let loaded = Graph::deserialize(&g.serialize());
        assert_eq!(loaded.nodes[&target].group, groups::NodeGroup { pattern: "right ^0-3".into(), kind: 1 });
        assert_eq!(format!("{:?}", loaded.nodes[&make].kind), format!("{:?}", g.nodes[&make].kind));
    }
}

#[cfg(test)]
mod raster_view_tests {
    use super::*;

    // Renders a cone like the viewport does, for checking by eye:
    // `PFX_ROTX=-0.45 PFX_WIRE=1 cargo test -- --ignored cone_renders_with_depth --nocapture`.
    #[test]
    #[ignore]
    fn cone_renders_with_depth() {
        let mesh = node_cone(1.0, 2.0, 1, 32, true);
        let rot_x: f32 = std::env::var("PFX_ROTX").ok().and_then(|v| v.parse().ok()).unwrap_or(0.35);
        let (rot_y, zoom, focal) = (0.6f32, 6.0f32, 900.0f32);
        let (sin_x, cos_x) = rot_x.sin_cos();
        let (sin_y, cos_y) = rot_y.sin_cos();
        let target = Vec3::new(0.0, 0.5, 0.0);
        let to_cam = |p: Vec3| {
            let v = p.sub(target);
            let rx = v.x * cos_y - v.z * sin_y;
            let rz = v.x * sin_y + v.z * cos_y;
            Vec3::new(rx, v.y * cos_x - rz * sin_x, v.y * sin_x + rz * cos_x + zoom)
        };
        let (w, h) = (640usize, 560usize);
        let mut r = raster::Raster::new(w, h, false);
        let normals = mesh.compute_normals();
        let light = Vec3::new(0.4, 0.8, -0.5).normalize();
        let project = |p: Vec3| {
            let c = to_cam(p);
            raster::ScreenVertex { x: w as f32 / 2.0 + c.x / c.z * focal, y: h as f32 / 2.0 - c.y / c.z * focal, z: c.z }
        };
        let wire = std::env::var("PFX_WIRE").is_ok();
        for tri in &mesh.indices {
            // The base cap (all corners on the lowest plane) in red, to see where it shows.
            let cap = tri.iter().all(|&i| (mesh.vertices[i].y - mesh.bounds().unwrap().0.y).abs() < 1e-4);
            let shade = tri.map(|i| {
                let l = 0.15 + 0.8 * normals[i].dot(light).max(0.0);
                if cap { [l.max(0.4), 0.1, 0.1] } else { [l; 3] }
            });
            r.triangle(tri.map(|i| project(mesh.vertices[i])), shade, None);
        }
        if wire {
            for tri in &mesh.indices {
                for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                    r.line(project(mesh.vertices[tri[a]]), project(mesh.vertices[tri[b]]), [0, 0, 0, 110]);
                }
            }
        }
        // A 4x crop of the base, to inspect.
        let full = image::RgbaImage::from_raw(w as u32, h as u32, r.color.as_flattened().to_vec()).unwrap();
        let crop = image::imageops::crop_imm(&full, 200, 470, 240, 70).to_image();
        image::imageops::resize(&crop, 960, 280, image::imageops::FilterType::Nearest).save(std::env::temp_dir().join("pfx_cone_crop.png")).unwrap();
        let out = std::env::temp_dir().join("pfx_cone_depth.png");
        image::RgbaImage::from_raw(w as u32, h as u32, r.color.as_flattened().to_vec()).unwrap().save(&out).unwrap();
        println!("wrote {}", out.display());
    }
}



#[cfg(test)]
mod vop_node_tests {
    use super::*;

    fn vop(op: usize, f: impl FnOnce(&mut vop::VopNode)) -> NodeKind {
        let mut v = vop::VopNode::new(op);
        f(&mut v);
        NodeKind::Vop(v)
    }

    fn op(label: &str) -> usize {
        vop::OPS.iter().position(|o| o.label == label).unwrap()
    }

    // grid -> Attribute VOP { P + (0, 1, 0) * ch("amp") -> P }
    fn lift_network() -> (Graph, NodeId, NodeId) {
        let mut g = Graph::default();
        let grid = g.add(NodeKind::Grid { size: 2.0, res: 3 }, egui::pos2(0.0, 0.0));
        let net = g.add(NodeKind::AttributeVop { domain: 2 }, egui::pos2(0.0, 0.0));
        g.connect(grid, net, 0);
        let mut child = |g: &mut Graph, kind: NodeKind| {
            let id = g.add(kind, egui::pos2(0.0, 0.0));
            g.nodes.get_mut(&id).unwrap().parent = Some(net);
            id
        };
        let p = child(&mut g, vop(vop::BIND, |_| {}));
        let up = child(&mut g, vop(vop::CONSTANT, |v| {
            v.ty = 1;
            v.value = [0.0, 1.0, 0.0];
        }));
        let amp = child(&mut g, vop(vop::PARAMETER, |v| v.name = "amp".into()));
        let mul = child(&mut g, vop(op("Multiply"), |_| {}));
        let add = child(&mut g, vop(op("Add"), |_| {}));
        let out = child(&mut g, vop(vop::EXPORT, |_| {}));
        g.connect(up, mul, 0);
        g.connect(amp, mul, 1);
        g.connect(p, add, 0);
        g.connect(mul, add, 1);
        g.connect(add, out, 0);
        g.nodes.get_mut(&net).unwrap().channels.insert("amp".into(), [2.0, 0.0, 0.0]);
        (g, grid, net)
    }

    #[test]
    fn attribute_vop_runs_its_network() {
        let (mut g, grid, net) = lift_network();
        let before = g.cook(grid, 0.0, &mut HashMap::new());
        let after = g.cook(net, 0.0, &mut HashMap::new());
        for (a, b) in before.vertices.iter().zip(&after.vertices) {
            assert!((b.y - a.y - 2.0).abs() < 1e-6 && b.x == a.x);
        }
        let code = g.vop_code(net).unwrap();
        assert!(code.contains("chf(\"amp\")") && code.contains("v@P = vector("), "{code}");
        assert!(!g.time_dependent(net));
        // The group limits it like a wrangle's.
        g.nodes.get_mut(&net).unwrap().group = groups::NodeGroup { pattern: "0-3".into(), kind: 1 };
        let grouped = g.cook(net, 0.0, &mut HashMap::new());
        assert!(grouped.vertices.iter().enumerate().all(|(i, v)| (v.y - if i <= 3 { 2.0 } else { 0.0 }).abs() < 1e-6));
        // Saved and loaded intact, parameter value included.
        let loaded = Graph::deserialize(&g.serialize());
        assert_eq!(loaded.vop_code(net).unwrap(), code);
        assert_eq!(loaded.nodes[&net].channels["amp"], [2.0, 0.0, 0.0]);
        assert!(loaded.cook(net, 0.0, &mut HashMap::new()).vertices[0].y > 1.9);
    }

    #[test]
    fn vop_menus_name_each_operation() {
        let mut app = ProceduralFxStudioApp::default();
        let (g, _, net) = lift_network();
        app.graph = g;
        let geometry = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        // Inside the Attribute VOP: only VOP categories, every entry named after its operation.
        app.current_network = Some(net);
        let inside = app.templates_for_view();
        let categories: Vec<&str> = inside.iter().map(|(c, _)| *c).collect();
        assert_eq!(categories, vop::CATEGORIES.to_vec());
        let names: Vec<String> = inside.iter().flat_map(|(_, kinds)| kinds.iter().map(NodeKind::menu_name)).collect();
        assert_eq!(names.len(), vop::OPS.len());
        let unique: HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "no two menu entries look the same");
        assert!(names.contains(&"Get Attribute".to_owned()) && names.contains(&"Noise".to_owned()));
        assert!(inside.iter().flat_map(|(_, k)| k).all(|k| matches!(k, NodeKind::Vop(_))));
        // In an ordinary geometry network: no VOPs.
        app.current_network = Some(geometry);
        assert!(app.templates_for_view().iter().flat_map(|(_, k)| k).all(|k| !matches!(k, NodeKind::Vop(_))));
    }

    // Every operation, wired into an Export with its inputs at their defaults, compiles and runs.
    #[test]
    fn every_vop_operation_runs() {
        let grid = node_grid(2.0, 2.0, 3, 3);
        for op in 0..vop::OPS.len() {
            if matches!(op, vop::EXPORT) {
                continue;
            }
            let mut nodes = HashMap::new();
            let mut node = vop::VopNode::new(op);
            if op == vop::BIND || op == vop::GET_ATTRIB || op == vop::PC_AVERAGE {
                node.name = "P".into();
            }
            nodes.insert(1, node);
            nodes.insert(2, vop::VopNode { name: "result".into(), ty: 0, ..vop::VopNode::new(vop::EXPORT) });
            let code = vop::compile(&nodes, |id, slot| (id == 2 && slot == 0).then_some(1)).unwrap();
            let _ = vex::run_wrangle(grid.clone(), &code, 0.0, 1.0 / 24.0, 2, None, None);
            let report = vex::last_report(&code, 2).unwrap();
            assert_eq!(report.error, None, "{}:\n{code}", vop::OPS[op].label);
        }
    }

    #[test]
    fn attribute_vop_errors_and_time() {
        let (mut g, _, net) = lift_network();
        // Binding Time makes it animate.
        let t = g.add(vop(vop::BIND, |v| {
            v.name = "Time".into();
            v.ty = 0;
        }), egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&t).unwrap().parent = Some(net);
        let export = g.children(net).into_iter().find(|c| matches!(&g.nodes[c].kind, NodeKind::Vop(v) if v.op == vop::EXPORT)).unwrap();
        let mul = g.children(net).into_iter().find(|c| matches!(&g.nodes[c].kind, NodeKind::Vop(v) if v.def().label == "Multiply")).unwrap();
        g.connect(t, mul, 1);
        assert!(g.time_dependent(net));
        let moved = g.cook(net, 3.0, &mut HashMap::new());
        assert!((moved.vertices[0].y - 3.0).abs() < 1e-5);
        // No export: the node reports why.
        g.nodes.remove(&export);
        assert!(g.cook(net, 0.0, &mut HashMap::new()).vertices.is_empty());
        assert!(node_errors(|e| e.get(&net).cloned()).is_some_and(|e| e.contains("no Export")));
    }
}

#[cfg(test)]
mod pane_tests {
    use super::*;

    fn tabs(dock: &egui_dock::DockState<Pane>) -> Vec<Pane> {
        let mut t: Vec<Pane> = dock.iter_all_tabs().map(|(_, p)| *p).collect();
        t.sort_by_key(|p| p.title());
        t
    }

    // A layout that has been drawn (so its areas have real sizes) still saves and loads.
    #[test]
    fn drawn_layout_saves() {
        struct Blank;
        impl egui_dock::TabViewer for Blank {
            type Tab = Pane;
            fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
                tab.title().into()
            }
            fn ui(&mut self, ui: &mut egui::Ui, _tab: &mut Pane) {
                ui.label("x");
            }
        }
        let ctx = egui::Context::default();
        let mut dock = default_layout();
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), ..Default::default() }, |ctx| {
                egui_dock::DockArea::new(&mut dock).show(ctx, &mut Blank);
            });
        }
        let text = ron::to_string(&dock);
        assert!(text.is_ok(), "{text:?}");
        assert!(layout_from_text(&text.unwrap()).is_some());
    }

    #[test]
    fn layout_defaults_saving_and_requests() {
        let dock = default_layout();
        assert_eq!(tabs(&dock), {
            let mut t = vec![Pane::Network, Pane::Parameters, Pane::Timeline, Pane::Viewport];
            t.sort_by_key(|p| p.title());
            t
        });
        let saved = layout_to_text(&dock).unwrap();
        let loaded = layout_from_text(&saved).expect("a saved layout loads back");
        assert_eq!(tabs(&loaded), tabs(&dock));
        assert!(layout_from_text("not a layout").is_none());
        assert_eq!(safe_layout_name("  My Desk/../top "), Some("My Desktop".into()), "path characters are dropped");
        assert_eq!(safe_layout_name("  /// "), None);

        let mut app = ProceduralFxStudioApp::default();
        let mut dock = default_layout();
        app.pane_requests = vec![PaneRequest::Show(Pane::Spreadsheet, true), PaneRequest::Show(Pane::Timeline, false)];
        app.apply_pane_requests(&mut dock);
        let open = tabs(&dock);
        assert!(open.contains(&Pane::Spreadsheet) && !open.contains(&Pane::Timeline));
        // The viewport can't be closed.
        app.pane_requests = vec![PaneRequest::Show(Pane::Viewport, false)];
        app.apply_pane_requests(&mut dock);
        assert!(tabs(&dock).contains(&Pane::Viewport));
        // Popping out moves a pane from the dock into its own window.
        app.pane_requests = vec![PaneRequest::PopOut(Pane::Spreadsheet)];
        app.apply_pane_requests(&mut dock);
        assert!(!tabs(&dock).contains(&Pane::Spreadsheet));
        assert_eq!(app.popped, vec![Pane::Spreadsheet]);
        // Reset brings back the default arrangement.
        app.pane_requests = vec![PaneRequest::Reset];
        app.apply_pane_requests(&mut dock);
        assert_eq!(tabs(&dock), tabs(&default_layout()));
        assert!(app.popped.is_empty());
    }
}

#[cfg(test)]
mod shader_render_tests {
    use super::*;

    // A lit quad with UVs and point colour, facing the default camera.
    fn panel() -> Mesh {
        let grid = node_grid(2.0, 2.0, 6, 6);
        let mut mesh = sops::uv_texture(&grid, &sops::UvTextureSettings { projection: 0, axis: 1, scale: [1.0, 1.0], offset: [0.0, 0.0] });
        let cd: Vec<f32> = mesh.vertices.iter().flat_map(|v| [0.5 + v.x * 0.25, 0.3, 0.5 - v.z * 0.25]).collect();
        mesh.point_attributes.insert("Cd".into(), cd);
        mesh
    }

    // An app whose graph holds a Principled shader, a Toon shader and a builder network.
    fn shaded_app() -> (ProceduralFxStudioApp, NodeId, NodeId, NodeId) {
        let mut app = ProceduralFxStudioApp::default();
        app.graph = Graph::default();
        let mut gold = shading::Values::preset(shading::Table::Principled, shading::PRINCIPLED_PRESETS.iter().position(|p| p.0 == "Gold").unwrap());
        gold.set("use_point_color", [1.0, 0.0, 0.0]);
        let principled = app.graph.add(NodeKind::Shader(gold), egui::pos2(0.0, 0.0));
        let toon = app.graph.add(NodeKind::Shader(shading::Values::preset(shading::Table::Toon, 1)), egui::pos2(0.0, 0.0));
        let builder = app.graph.add(NodeKind::MaterialBuilder, egui::pos2(0.0, 0.0));
        let child = |app: &mut ProceduralFxStudioApp, op: usize| {
            let id = app.graph.add(NodeKind::ShaderNode(shading::ShaderNode::new(op)), egui::pos2(0.0, 0.0));
            let n = app.graph.nodes.get_mut(&id).unwrap();
            n.context = AppContext::Mat;
            n.parent = Some(builder);
            id
        };
        let output = child(&mut app, shading::SURFACE_OUTPUT);
        let noise = child(&mut app, 4);
        let cells = child(&mut app, 5);
        let mix = child(&mut app, 8);
        let hsv = child(&mut app, 9);
        app.graph.connect(noise, mix, 0);
        app.graph.connect(cells, mix, 2);
        app.graph.connect(mix, hsv, 0);
        app.graph.connect(hsv, output, 0);
        app.graph.connect(cells, output, 1);
        (app, principled, toon, builder)
    }

    #[test]
    fn shaders_and_builders_export_to_rdla() {
        let (app, principled, toon, builder) = shaded_app();
        let scene = RenderScene {
            parts: vec![(panel(), PartMaterial::Node(principled)), (panel(), PartMaterial::Node(toon)), (panel(), PartMaterial::Node(builder))],
            lights: app.scene_lights(),
            environment: None,
            camera: None,
            volumes: Vec::new(),
        };
        let path = std::env::temp_dir().join(format!("pfx_shaders_{}.rdla", std::process::id()));
        app.export_scene_rdla(&path, &scene, 64, 48, 4).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!("DwaBaseMaterial(\"/scene/materials/mat{principled}\") {{")));
        assert!(text.contains(&format!("[\"albedo\"] = bind(AttributeMap(\"/scene/materials/mat{principled}_point_color\"))")));
        assert!(text.contains(&format!("DwaToonMaterial(\"/scene/materials/mat{toon}\") {{")));
        assert!(text.contains("NoiseMap_v2(") && text.contains("BlendMap(") && text.contains("ColorCorrectHsvMap("));
        assert!(text.contains("[\"uv_list\"] = {") && text.contains("UserData(\"/scene/objects/mesh0_Cd\") {"));
        // Each part's layer entry uses its own material.
        assert!(text.contains(&format!("DwaBaseMaterial(\"/scene/materials/mat{builder}\")}}").replace("}", "")) || text.contains(&format!("DwaBaseMaterial(\"/scene/materials/mat{builder}\"), LightSet")));
        // The viewport shows each material's colour.
        let gold = app.graph.material_preview(principled).unwrap();
        assert!(gold.0[0] > 0.9 && gold.2, "gold uses Cd: {gold:?}");
        assert!(app.graph.material_preview(builder).is_some());
        // Saving keeps shader nodes and the builder's network.
        let loaded = Graph::deserialize(&app.graph.serialize());
        for (id, node) in &app.graph.nodes {
            assert_eq!(format!("{:?}", loaded.nodes[id].kind), format!("{:?}", node.kind));
            assert_eq!(loaded.nodes[id].parent, node.parent);
        }
    }

    // Renders every shader in MoonRay: `cargo test --release -- --ignored every_shader_renders`.
    #[test]
    #[ignore]
    fn every_shader_renders_in_moonray() {
        let dir = std::env::temp_dir();
        let texture = dir.join("pfx_shader_texture.png");
        image::RgbImage::from_fn(32, 32, |x, y| if (x / 8 + y / 8) % 2 == 0 { image::Rgb([230, 60, 40]) } else { image::Rgb([40, 60, 230]) }).save(&texture).unwrap();
        let (mut app, ..) = shaded_app();
        let mut materials: Vec<NodeId> = app.graph.nodes.iter().filter(|(_, n)| n.kind.is_material()).map(|(id, _)| *id).collect();
        for table in [shading::Table::Principled, shading::Table::Toon, shading::Table::Hair] {
            for i in 0..table.presets().len() {
                materials.push(app.graph.add(NodeKind::Shader(shading::Values::preset(table, i)), egui::pos2(0.0, 0.0)));
            }
        }
        let mut textured = shading::Values::new(shading::Table::Principled);
        textured.textures = vec![texture.to_string_lossy().into_owned(), texture.to_string_lossy().into_owned(), String::new()];
        materials.push(app.graph.add(NodeKind::Shader(textured), egui::pos2(0.0, 0.0)));
        // A builder using every other node type, including an image and a normal map.
        let builder = app.graph.add(NodeKind::MaterialBuilder, egui::pos2(0.0, 0.0));
        let file = texture.to_string_lossy().into_owned();
        let mut child = |op: usize| {
            let mut node = shading::ShaderNode::new(op);
            node.texts.iter_mut().for_each(|t| *t = file.clone());
            if op == 3 {
                node.texts[0] = "Cd".into();
            }
            let id = app.graph.add(NodeKind::ShaderNode(node), egui::pos2(0.0, 0.0));
            let n = app.graph.nodes.get_mut(&id).unwrap();
            n.context = AppContext::Mat;
            n.parent = Some(builder);
            id
        };
        let (output, image, normal, attribute, checker, ramp, mix) = (child(0), child(1), child(2), child(3), child(6), child(7), child(8));
        app.graph.connect(image, mix, 0);
        app.graph.connect(attribute, mix, 1);
        app.graph.connect(checker, ramp, 0);
        app.graph.connect(ramp, mix, 2);
        app.graph.connect(mix, output, 0);
        app.graph.connect(normal, output, 10);
        materials.push(builder);
        app.render_camera = RenderCamera::Viewport;
        for material in materials {
            let scene = RenderScene { parts: vec![(panel(), PartMaterial::Node(material))], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
            let rdla = dir.join("pfx_shader.rdla");
            let exr = dir.join("pfx_shader.exr");
            let _ = std::fs::remove_file(&exr);
            app.export_scene_rdla(&rdla, &scene, 48, 36, 4).unwrap();
            let run = Command::new("native/moonray/macos/moonray")
                .env("RDL2_DSO_PATH", "/Users/loksangho/moonray/build/rdl2dso")
                .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
                .output()
                .unwrap();
            let label = app.graph.nodes[&material].kind.label(material);
            let log = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
            assert!(run.status.success(), "{label}: {log}");
            for bad in ["Error", "ERROR", "does not exist", "no attribute", "Unknown"] {
                assert!(!log.contains(bad), "{label}: {log}");
            }
            let render = image::open(&exr).unwrap().to_rgb32f();
            let lit = render.pixels().map(|p| p[0] + p[1] + p[2]).fold(0.0f32, f32::max);
            println!("{label}: brightest {lit:.3}");
            assert!(lit > 0.01, "{label} rendered black");
        }
    }
}

#[cfg(test)]
mod whitewater_node_tests {
    use super::*;

    fn flip() -> NodeKind {
        NodeKind::Flip { particle_count: 3000, grid_resolution: 24, flip_ratio: 0.95, viscosity: 0.1, gravity: -9.8, max_dt: 0.02, substeps: 1, seed: 3 }
    }

    #[test]
    fn flip_steps_from_its_cache_and_feeds_whitewater() {
        let mut g = Graph::default();
        g.next_id += 9300;
        let emitter = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
        let lift = g.add(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [1.0, 1.0, 1.0] }, egui::pos2(0.0, 0.0));
        let fluid = g.add(flip(), egui::pos2(0.0, 0.0));
        let white = g.add(NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings { emission_rate: 3000.0, min_speed: 0.2, ..whitewater::WhitewaterSettings::defaults() }), egui::pos2(0.0, 0.0));
        g.connect(emitter, lift, 0);
        g.connect(lift, fluid, 0);
        g.connect(fluid, white, 0);
        sims::clear_all();
        let a = g.cook(fluid, 0.5, &mut HashMap::new());
        assert_eq!(a.vertices.len(), 3000);
        assert_eq!(a.point_attributes["v"].len(), 3000 * 3);
        // The fluid (which sits on its domain floor, the emitter's bottom) is moving.
        let speed: f32 = a.point_attributes["v"].chunks(3).map(|v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()).sum::<f32>() / 3000.0;
        assert!(speed.is_finite() && speed > 0.1, "mean speed {speed}");
        let b = g.cook(fluid, 1.0, &mut HashMap::new());
        sims::clear_all();
        let fresh = g.cook(fluid, 1.0, &mut HashMap::new());
        assert_eq!(b.vertices, fresh.vertices, "cached stepping matches a fresh run");
        assert!(g.time_dependent(white));
        let out = g.cook(white, 1.0, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&white).cloned()).is_none());
        assert!(!out.vertices.is_empty(), "the splash should make whitewater");
        assert!(renders_points(&out));
        let loaded = Graph::deserialize(&g.serialize());
        assert_eq!(format!("{:?}", loaded.nodes[&white].kind), format!("{:?}", g.nodes[&white].kind));
        // Without a fluid input it says what's missing.
        let lonely = g.add(NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings::defaults()), egui::pos2(0.0, 0.0));
        g.cook(lonely, 1.0, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&lonely).cloned()).unwrap().contains("FLIP"));
    }

    #[test]
    #[ignore]
    fn whitewater_points_render_in_moonray() {
        let mut mesh = Mesh::default();
        for i in 0..200 {
            mesh.vertices.push(Vec3::new(hash01(i, 1) - 0.5, hash01(i, 2) - 0.5, hash01(i, 3) - 0.5));
        }
        mesh.point_attributes.insert("pscale".into(), vec![0.03; 200]);
        mesh.point_attributes.insert(whitewater::RENDER_POINTS.into(), vec![1.0]);
        let mut app = ProceduralFxStudioApp::default();
        app.render_camera = RenderCamera::Viewport;
        let scene = RenderScene { parts: vec![(mesh, PartMaterial::Default)], lights: app.scene_lights(), environment: None, camera: None, volumes: Vec::new() };
        let dir = std::env::temp_dir();
        let (rdla, exr) = (dir.join("pfx_points.rdla"), dir.join("pfx_points.exr"));
        app.export_scene_rdla(&rdla, &scene, 64, 48, 4).unwrap();
        let run = Command::new("native/moonray/macos/moonray")
            .env("RDL2_DSO_PATH", "/Users/loksangho/moonray/build/rdl2dso")
            .args(["-in", rdla.to_str().unwrap(), "-out", exr.to_str().unwrap()])
            .output()
            .unwrap();
        let log = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
        assert!(run.status.success() && !log.contains("Error"), "{log}");
        let render = image::open(&exr).unwrap().to_rgb32f();
        let lit = render.pixels().filter(|p| p[0] + p[1] + p[2] > 0.05).count();
        assert!(lit > 50, "{lit} lit pixels");
    }
}

#[cfg(test)]
mod fx_node_tests {
    use super::*;

    #[test]
    fn fractured_pieces_feed_rbd_and_crowds_and_muscles_cook() {
        let mut g = Graph::default();
        g.next_id += 9500;
        let cube = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
        let frac = g.add(NodeKind::MaterialFracture(fracture::FractureSettings { pieces: 8, ..fracture::FractureSettings::defaults(0) }), egui::pos2(0.0, 0.0));
        g.connect(cube, frac, 0);
        let pieces = g.cook(frac, 0.0, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&frac).cloned()).is_none());
        let count = pieces.point_attributes["piece"].iter().fold(0.0f32, |m, p| m.max(*p)) as usize + 1;
        assert!(count >= 4, "{count} pieces");

        // Crowd: agents from a few points, walking.
        let starts = g.add(NodeKind::Add { count: 6, spacing: 1.0, layout: 2 }, egui::pos2(0.0, 0.0));
        let crowd_node = g.add(NodeKind::CrowdSolver(crowd::CrowdSettings::defaults()), egui::pos2(0.0, 0.0));
        g.connect(starts, crowd_node, 0);
        assert!(g.time_dependent(crowd_node));
        sims::clear_all();
        let a = g.cook(crowd_node, 1.0, &mut HashMap::new());
        let b = g.cook(crowd_node, 2.0, &mut HashMap::new());
        assert!(!a.indices.is_empty() && a.vertices != b.vertices);
        assert_eq!(a.point_attributes["prim:agent"].len(), a.indices.len());

        // Muscles: flexing (activation keyframed) drives the jiggle solver and the skin.
        let m = g.add(NodeKind::Muscle(muscle::MuscleSettings::defaults()), egui::pos2(0.0, 0.0));
        let target = ChannelId { node: m, target: ChannelTarget::Param(10) };
        let mut channel = anim::Channel::default();
        channel.set_key(0.0, 0.0);
        channel.set_key(1.0, 1.0);
        g.channels.insert(target, channel);
        let solver = g.add(NodeKind::MuscleSolver(muscle::JiggleSettings::defaults()), egui::pos2(0.0, 0.0));
        g.connect(m, solver, 0);
        assert!(g.time_dependent(solver));
        // A skin tube around the muscle (y 0..1), however the Tube node centres itself.
        let tube = g.add(NodeKind::Tube { r_bot: 0.16, r_top: 0.16, height: 1.0, rows: 8, cols: 16, caps: false }, egui::pos2(0.0, 0.0));
        let tube_mesh = g.cook(tube, 0.0, &mut HashMap::new());
        let (lo, _) = tube_mesh.bounds().unwrap();
        let skin_src = g.add(NodeKind::Transform { t: [0.0, -lo.y, 0.0], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
        g.connect(tube, skin_src, 0);
        let skin = g.add(NodeKind::MuscleSkin(muscle::SkinSettings::defaults()), egui::pos2(0.0, 0.0));
        g.connect(skin_src, skin, 0);
        g.connect(solver, skin, 1);
        let rest = g.cook(skin, 0.0, &mut HashMap::new());
        let flexed = g.cook(skin, 1.5, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&skin).cloned()).is_none(), "{:?}", node_errors(|e| e.get(&skin).cloned()));
        assert_eq!(rest.vertices.len(), flexed.vertices.len());
        assert!(rest.vertices.iter().zip(&flexed.vertices).any(|(a, b)| a.sub(*b).length() > 1e-3), "skin should move as the muscle flexes");
    }
}

#[cfg(test)]
mod material_builder_ui_tests {
    use super::*;

    #[test]
    fn inside_a_material_builder_only_shader_nodes_are_offered() {
        let mut app = ProceduralFxStudioApp::default();
        app.graph = Graph::default();
        app.context = AppContext::Mat;
        app.apply(Action::Add(NodeKind::MaterialBuilder, egui::pos2(0.0, 0.0)));
        let builder = app.selected.unwrap();
        app.apply(Action::Enter(builder));
        assert_eq!(app.context, AppContext::Mat, "the builder's network is a MAT network");
        assert_eq!(app.view_parent(), Some(builder));
        let menus = app.templates_for_view();
        assert_eq!(menus.iter().map(|(c, _)| *c).collect::<Vec<_>>(), vec![shading::CATEGORY]);
        assert!(menus.iter().flat_map(|(_, k)| k).all(|k| matches!(k, NodeKind::ShaderNode(_))));
        assert_eq!(menus[0].1.len(), shading::SHADER_OPS.len());
        // The Surface Output made with the builder is in the network shown, and new nodes join it.
        let shown: Vec<NodeId> = app.graph.nodes.iter().filter(|(_, n)| n.context == app.context && n.parent == app.view_parent()).map(|(id, _)| *id).collect();
        assert_eq!(shown.len(), 1);
        assert!(matches!(&app.graph.nodes[&shown[0]].kind, NodeKind::ShaderNode(n) if n.op == shading::SURFACE_OUTPUT));
        app.apply(Action::Add(NodeKind::ShaderNode(shading::ShaderNode::new(4)), egui::pos2(0.0, 0.0)));
        let noise = app.selected.unwrap();
        assert_eq!((app.graph.nodes[&noise].context, app.graph.nodes[&noise].parent), (AppContext::Mat, Some(builder)));
        // Up goes back to the top of MAT, with materials (not shader nodes) on offer.
        app.enter_network(None);
        assert_eq!((app.context, app.network()), (AppContext::Mat, None));
        let top = app.templates_for_view();
        assert!(top.iter().flat_map(|(_, k)| k).all(|k| k.is_material()));
        // Switching to OBJ from inside the builder leaves it.
        app.apply(Action::Enter(builder));
        app.context = AppContext::Obj;
        assert_eq!(app.view_parent(), None);
    }
}
