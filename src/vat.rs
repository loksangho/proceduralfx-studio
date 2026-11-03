// Vertex animation textures (VAT): animation baked for game engines. A mesh (OBJ) plus two
// textures (EXR) with one texel per element per frame; a small shader in the engine reads
// the current frame's texels and moves the mesh's vertices on the GPU, for the cost of a
// texture read or two per vertex. Three kinds, as in Houdini's VAT:
//
// - Soft: the points stay the same (cloth, deformers, wobbling or twisting shapes). An
//   element is a point: its offset from the first frame, and its normal. Half floats.
// - Rigid: pieces that move without bending (destruction, RBD). An element is a piece:
//   its centre and its rotation (a quaternion). Each vertex turns with its piece about the
//   piece's first-frame centre, then moves to where the centre is. Full floats.
// - Fluid: the mesh changes from frame to frame (liquids, growing shapes). An element is a
//   triangle corner: its position and normal. The OBJ has as many triangles as the busiest
//   frame; corners a frame doesn't use sit at the origin, so their triangles have no area
//   and don't show. Full floats.
//
// Layout: elements run left to right, `width` to a row, so a frame takes `rows_per_frame`
// rows; frame f's rows start at f * rows_per_frame. Row 0 is the top of the image. Vertex
// UVs pick the element's texel within a frame: ((column + 0.5) / width, (row + 0.5) /
// rows_per_frame). Two coordinates, not one index: engines store UVs in 16 bits, which
// can't tell more than 65536 elements apart along one.
//
// Space: the OBJ's (right-handed, like the other exports: z mirrored), so the textures'
// positions, normals and rotations are mirrored the same way.

use crate::{Mesh, Vec3};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// GPUs commonly allow textures up to this size on a side; most engines import up to it.
pub const MAX_SIDE: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Soft,
    Rigid,
    Fluid,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Soft => "soft",
            Mode::Rigid => "rigid",
            Mode::Fluid => "fluid",
        }
    }

    pub fn parse(text: &str) -> Option<Mode> {
        [Mode::Soft, Mode::Rigid, Mode::Fluid].into_iter().find(|m| m.name() == text)
    }

    // The two textures' roles (file suffixes and shader names).
    pub fn textures(self) -> [&'static str; 2] {
        match self {
            Mode::Soft => ["offsets", "normals"],
            Mode::Rigid => ["positions", "rotations"],
            Mode::Fluid => ["positions", "normals"],
        }
    }

    // Whether texture `which` is stored in half floats: plenty for soft offsets and for
    // normals; positions and rotations travel far, so they get full floats.
    fn half(self, which: usize) -> bool {
        match self {
            Mode::Soft => true,
            Mode::Rigid => false,
            Mode::Fluid => which == 1,
        }
    }

    // What an element is, for messages.
    fn element(self) -> &'static str {
        match self {
            Mode::Soft => "points",
            Mode::Rigid => "pieces",
            Mode::Fluid => "triangle corners",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub points: usize,
    pub frames: usize,
    pub width: usize,
    pub rows_per_frame: usize,
}

impl Layout {
    pub fn new(points: usize, frames: usize) -> Result<Layout, String> {
        if points == 0 || frames == 0 {
            return Err("there's nothing to bake: the geometry has no points".into());
        }
        // As wide as needed, up to the limit; a frame wraps onto more rows past it.
        let width = points.min(MAX_SIDE);
        let rows_per_frame = points.div_ceil(width);
        let layout = Layout { points, frames, width, rows_per_frame };
        if layout.height() > MAX_SIDE {
            return Err(format!(
                "{points} elements over {frames} frames needs a {}×{} texture, over the {MAX_SIDE}-pixel limit: bake fewer frames (or every other frame), or use less geometry",
                layout.width,
                layout.height()
            ));
        }
        Ok(layout)
    }

    pub fn height(&self) -> usize {
        self.frames * self.rows_per_frame
    }

    // The texel holding element `p` at frame `f`, from the top left.
    pub fn texel(&self, p: usize, f: usize) -> (usize, usize) {
        (p % self.width, f * self.rows_per_frame + p / self.width)
    }
}

// A bake: the mesh the OBJ holds (in the app's space), each of its vertices' element, and
// the two textures' values for every frame and element ([frame][element], in OBJ space).
#[derive(Debug)]
pub struct Bake {
    pub mode: Mode,
    pub mesh: Mesh,
    pub normals: Vec<Vec3>,
    pub element_of: Vec<usize>,
    pub elements: usize,
    pub frames: usize,
    pub data: [Vec<Vec<[f32; 4]>>; 2],
    // Everything the animation covers (for the engines' culling), in OBJ space.
    pub bounds: (Vec3, Vec3),
}

fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, -v.z)
}

fn bounds_of<'a>(points: impl Iterator<Item = &'a Vec3>) -> (Vec3, Vec3) {
    let mut lo = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
    for p in points {
        let m = mirror(*p);
        lo = Vec3::new(lo.x.min(m.x), lo.y.min(m.y), lo.z.min(m.z));
        hi = Vec3::new(hi.x.max(m.x), hi.y.max(m.y), hi.z.max(m.z));
    }
    if lo.x > hi.x { (Vec3::zero(), Vec3::zero()) } else { (lo, hi) }
}

fn same_shape(frames: &[Mesh]) -> bool {
    let first = &frames[0];
    frames.iter().all(|m| m.vertices.len() == first.vertices.len() && m.indices == first.indices)
}

// Bakes the frames as `mode`, or (None) as the kind they suit: fluid if the mesh changes,
// rigid if it's several pieces that each keep their shape, else soft.
pub fn bake(frames: &[Mesh], mode: Option<Mode>) -> Result<Bake, String> {
    let first = frames.first().ok_or("there are no frames to bake")?;
    if frames.iter().all(|m| m.indices.is_empty()) {
        return Err("the geometry has no polygons: vertex animation textures move a mesh".into());
    }
    let mode = match mode {
        Some(mode) => mode,
        None if !same_shape(frames) => Mode::Fluid,
        None => match pieces(first) {
            (_, count) if count > 1 && rigid_fit(frames).is_ok() => Mode::Rigid,
            _ => Mode::Soft,
        },
    };
    match mode {
        Mode::Soft => bake_soft(frames),
        Mode::Rigid => bake_rigid(frames),
        Mode::Fluid => bake_fluid(frames),
    }
}

fn changes_message(frames: &[Mesh], kind: &str) -> String {
    let at = frames.iter().position(|m| m.vertices.len() != frames[0].vertices.len() || m.indices != frames[0].indices).unwrap_or(0);
    format!(
        "the geometry changes at frame {} ({} points, then {}): {kind} needs the same points and polygons all the way through. Bake it as Fluid, which allows that",
        at + 1,
        frames[0].vertices.len(),
        frames[at].vertices.len()
    )
}

fn bake_soft(frames: &[Mesh]) -> Result<Bake, String> {
    let rest = &frames[0];
    if rest.indices.is_empty() {
        return Err("the first frame has no polygons to bake".into());
    }
    if !same_shape(frames) {
        return Err(changes_message(frames, "a Soft bake"));
    }
    let normals: Vec<Vec<Vec3>> = frames.iter().map(|m| m.compute_normals()).collect();
    let n = rest.vertices.len();
    let offsets = frames
        .iter()
        .map(|m| (0..n).map(|p| {
            let d = mirror(m.vertices[p]).sub(mirror(rest.vertices[p]));
            [d.x, d.y, d.z, 1.0]
        }).collect())
        .collect();
    let normal_data = normals.iter().map(|ns| ns.iter().map(|v| { let m = mirror(*v); [m.x, m.y, m.z, 1.0] }).collect()).collect();
    Ok(Bake {
        mode: Mode::Soft,
        mesh: rest.clone(),
        normals: normals[0].clone(),
        element_of: (0..n).collect(),
        elements: n,
        frames: frames.len(),
        data: [offsets, normal_data],
        bounds: bounds_of(frames.iter().flat_map(|m| m.vertices.iter())),
    })
}

// ---------------------------------------------------------------------------
// Rigid pieces
// ---------------------------------------------------------------------------

// Each point's piece, and how many: the `piece` attribute (as fracturing and the RBD
// solvers set it) if there is one, else what's connected to what.
pub fn pieces(mesh: &Mesh) -> (Vec<usize>, usize) {
    let n = mesh.vertices.len();
    if let Some(labels) = mesh.point_attributes.get("piece").filter(|v| v.len() == n) {
        let mut ids: Vec<i64> = labels.iter().map(|v| v.round() as i64).collect();
        ids.sort_unstable();
        ids.dedup();
        let of: Vec<usize> = labels.iter().map(|v| ids.binary_search(&(v.round() as i64)).unwrap_or(0)).collect();
        return (of, ids.len().max(1));
    }
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for tri in &mesh.indices {
        let (a, b, c) = (root(&mut parent, tri[0]), root(&mut parent, tri[1]), root(&mut parent, tri[2]));
        parent[b] = a;
        let c = root(&mut parent, c);
        parent[c] = a;
    }
    let mut number = std::collections::HashMap::new();
    let of: Vec<usize> = (0..n)
        .map(|i| {
            let r = root(&mut parent, i);
            let next = number.len();
            *number.entry(r).or_insert(next)
        })
        .collect();
    (of, number.len().max(1))
}

// The eigenvector of a symmetric 4×4 matrix with the largest eigenvalue (Jacobi rotations).
fn largest_eigenvector(mut a: [[f64; 4]; 4]) -> [f64; 4] {
    let mut v = [[0.0f64; 4]; 4];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..50 {
        let off: f64 = (0..4).flat_map(|i| (0..4).filter(move |&j| j != i).map(move |j| (i, j))).map(|(i, j)| a[i][j] * a[i][j]).sum();
        if off < 1e-22 {
            break;
        }
        for p in 0..4 {
            for q in p + 1..4 {
                if a[p][q].abs() < 1e-30 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let (c, s) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
                for k in 0..4 {
                    let (akp, akq) = (a[k][p], a[k][q]);
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..4 {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let best = (0..4).max_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap_or(0);
    [v[0][best], v[1][best], v[2][best], v[3][best]]
}

// The rotation (w, x, y, z) best turning the `from` offsets onto the `to` offsets
// (Horn's method: the top eigenvector of a 4×4 matrix built from their correlations).
fn best_rotation(from: &[Vec3], to: &[Vec3]) -> [f64; 4] {
    let mut s = [[0.0f64; 3]; 3];
    for (a, b) in from.iter().zip(to) {
        let (a, b) = ([a.x as f64, a.y as f64, a.z as f64], [b.x as f64, b.y as f64, b.z as f64]);
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += a[i] * b[j];
            }
        }
    }
    let [[xx, xy, xz], [yx, yy, yz], [zx, zy, zz]] = s;
    let n = [
        [xx + yy + zz, yz - zy, zx - xz, xy - yx],
        [yz - zy, xx - yy - zz, xy + yx, zx + xz],
        [zx - xz, xy + yx, -xx + yy - zz, yz + zy],
        [xy - yx, zx + xz, yz + zy, -xx - yy + zz],
    ];
    let q = largest_eigenvector(n);
    let len = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    if len < 1e-12 { [1.0, 0.0, 0.0, 0.0] } else { q.map(|c| c / len) }
}

// Turns `v` by the quaternion (w, x, y, z).
pub fn rotate(q: [f32; 4], v: Vec3) -> Vec3 {
    let [w, x, y, z] = q;
    let u = Vec3::new(x, y, z);
    let t = u.cross(v).scale(2.0);
    v.add(t.scale(w)).add(u.cross(t))
}

// Every piece's centre and rotation, every frame (in the app's space), or why the motion
// isn't rigid: a piece bends or stretches.
#[allow(clippy::type_complexity)]
fn rigid_fit(frames: &[Mesh]) -> Result<(Vec<usize>, usize, Vec<Vec<(Vec3, [f32; 4])>>), String> {
    if !same_shape(frames) {
        return Err(changes_message(frames, "a Rigid bake"));
    }
    let rest = &frames[0];
    let (of, count) = pieces(rest);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (p, &k) in of.iter().enumerate() {
        members[k].push(p);
    }
    let centre = |mesh: &Mesh, points: &[usize]| points.iter().fold(Vec3::zero(), |s, &p| s.add(mesh.vertices[p])).scale(1.0 / points.len().max(1) as f32);
    let rest_centres: Vec<Vec3> = members.iter().map(|m| centre(rest, m)).collect();
    let (lo, hi) = bounds_of(rest.vertices.iter());
    let tolerance = hi.sub(lo).length().max(1e-3) * 1e-3;
    let mut out = Vec::with_capacity(frames.len());
    let mut last: Vec<[f32; 4]> = vec![[1.0, 0.0, 0.0, 0.0]; count];
    for (f, mesh) in frames.iter().enumerate() {
        let mut frame = Vec::with_capacity(count);
        for (k, points) in members.iter().enumerate() {
            let c = centre(mesh, points);
            let from: Vec<Vec3> = points.iter().map(|&p| rest.vertices[p].sub(rest_centres[k])).collect();
            let to: Vec<Vec3> = points.iter().map(|&p| mesh.vertices[p].sub(c)).collect();
            let q = best_rotation(&from, &to);
            let mut q = [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32];
            // q and -q are the same turn: keep each piece's sign steady, so frames blend.
            if q.iter().zip(&last[k]).map(|(a, b)| a * b).sum::<f32>() < 0.0 {
                q = q.map(|c| -c);
            }
            last[k] = q;
            for (a, b) in from.iter().zip(&to) {
                if rotate(q, *a).sub(*b).length() > tolerance {
                    return Err(format!("piece {} bends or stretches at frame {}, so it can't be baked as rigid: bake it as Soft", k + 1, f + 1));
                }
            }
            frame.push((c, q));
        }
        out.push(frame);
    }
    Ok((of, count, out))
}

fn bake_rigid(frames: &[Mesh]) -> Result<Bake, String> {
    let (of, count, motion) = rigid_fit(frames)?;
    let rest = &frames[0];
    // Into the OBJ's space: centres mirrored, rotations conjugated by the mirror.
    let positions = motion.iter().map(|frame| frame.iter().map(|(c, _)| { let m = mirror(*c); [m.x, m.y, m.z, 1.0] }).collect()).collect();
    let rotations = motion.iter().map(|frame| frame.iter().map(|(_, [w, x, y, z])| [-x, -y, *z, *w]).collect()).collect();
    Ok(Bake {
        mode: Mode::Rigid,
        mesh: rest.clone(),
        normals: rest.compute_normals(),
        element_of: of,
        elements: count,
        frames: frames.len(),
        data: [positions, rotations],
        bounds: bounds_of(frames.iter().flat_map(|m| m.vertices.iter())),
    })
}

// ---------------------------------------------------------------------------
// Fluids (a changing mesh)
// ---------------------------------------------------------------------------

fn bake_fluid(frames: &[Mesh]) -> Result<Bake, String> {
    let triangles = frames.iter().map(|m| m.indices.len()).max().unwrap_or(0);
    let corners = triangles * 3;
    let bounds = bounds_of(frames.iter().flat_map(|m| m.vertices.iter()));
    // The OBJ: that many small triangles spread over everything the animation covers (so
    // engines' culling sees its whole size), each corner its own vertex.
    let (lo, hi) = bounds;
    let span = hi.sub(lo);
    let side = (triangles as f32).cbrt().ceil().max(1.0) as usize;
    let cell = |i: usize| Vec3::new((i % side) as f32 + 0.5, ((i / side) % side) as f32 + 0.5, (i / (side * side)) as f32 + 0.5).scale(1.0 / side as f32);
    let size = span.length().max(1e-3) * 0.01;
    let mut mesh = Mesh::default();
    for t in 0..triangles {
        let c = cell(t);
        let at = Vec3::new(lo.x + span.x * c.x, lo.y + span.y * c.y, lo.z + span.z * c.z);
        // In the app's space: the OBJ mirrors it back.
        let at = mirror(at);
        mesh.vertices.extend([at, at.add(Vec3::new(size, 0.0, 0.0)), at.add(Vec3::new(0.0, 0.0, size))]);
        mesh.indices.push([t * 3, t * 3 + 1, t * 3 + 2]);
    }
    let mut positions = Vec::with_capacity(frames.len());
    let mut normals = Vec::with_capacity(frames.len());
    for m in frames {
        let point_normals = m.compute_normals();
        let mut p = vec![[0.0f32; 4]; corners];
        let mut n = vec![[0.0f32, 1.0, 0.0, 0.0]; corners];
        for (t, tri) in m.indices.iter().enumerate() {
            // Corner k of triangle t is vertex t * 3 + k; the OBJ's faces swap two
            // corners for the mirror, as for every export.
            for (k, &point) in tri.iter().enumerate() {
                let (v, nn) = (mirror(m.vertices[point]), mirror(point_normals[point]));
                p[t * 3 + k] = [v.x, v.y, v.z, 1.0];
                n[t * 3 + k] = [nn.x, nn.y, nn.z, 1.0];
            }
        }
        positions.push(p);
        normals.push(n);
    }
    Ok(Bake {
        mode: Mode::Fluid,
        normals: vec![Vec3::new(0.0, 1.0, 0.0); corners],
        element_of: (0..corners).collect(),
        elements: corners,
        frames: frames.len(),
        data: [positions, normals],
        bounds,
        mesh,
    })
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

// The mesh as OBJ: positions, each vertex's element UV and normal, and triangles wound
// for the mirrored (right-handed) space.
pub fn obj(bake: &Bake) -> String {
    let layout = Layout { points: bake.elements, frames: 1, width: bake.elements.clamp(1, MAX_SIDE), rows_per_frame: bake.elements.max(1).div_ceil(bake.elements.clamp(1, MAX_SIDE)) };
    let mut out = format!("# ProceduralFX Studio vertex animation ({}): UVs pick each vertex's texels.\n", bake.mode.name());
    for v in &bake.mesh.vertices {
        let p = mirror(*v);
        let _ = writeln!(out, "v {} {} {}", p.x, p.y, p.z);
    }
    for &e in &bake.element_of {
        let (column, row) = layout.texel(e, 0);
        let _ = writeln!(out, "vt {} {}", (column as f64 + 0.5) / layout.width as f64, (row as f64 + 0.5) / layout.rows_per_frame as f64);
    }
    for v in &bake.normals {
        let m = mirror(*v);
        let _ = writeln!(out, "vn {} {} {}", m.x, m.y, m.z);
    }
    for [a, b, c] in &bake.mesh.indices {
        // Mirroring flips the winding: swap two corners back.
        let _ = writeln!(out, "f {0}/{0}/{0} {2}/{2}/{2} {1}/{1}/{1}", a + 1, b + 1, c + 1);
    }
    out
}

// A texture's texels (index x + y * width).
pub fn texels(bake: &Bake, layout: &Layout, which: usize) -> Vec<[f32; 4]> {
    let mut out = vec![[0.0f32; 4]; layout.width * layout.height()];
    for (f, frame) in bake.data[which].iter().enumerate() {
        for (e, value) in frame.iter().enumerate() {
            let (x, y) = layout.texel(e, f);
            out[x + y * layout.width] = *value;
        }
    }
    out
}

fn write_exr(path: &Path, layout: &Layout, data: &[[f32; 4]], half: bool) -> Result<(), String> {
    use exr::prelude::*;
    let width = layout.width;
    let result = if half {
        write_rgba_file(path, width, layout.height(), |x, y| {
            let [r, g, b, a] = data[x + y * width];
            (f16::from_f32(r), f16::from_f32(g), f16::from_f32(b), f16::from_f32(a))
        })
    } else {
        write_rgba_file(path, width, layout.height(), |x, y| {
            let [r, g, b, a] = data[x + y * width];
            (r, g, b, a)
        })
    };
    result.map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

// The files a bake writes beside `stem` (name.obj, name_offsets.exr...).
pub struct Files {
    pub mode: Mode,
    pub obj: PathBuf,
    pub textures: [PathBuf; 2],
    pub json: PathBuf,
}

impl Files {
    pub fn new(stem: &Path, mode: Mode) -> Files {
        let name = stem.file_name().map_or_else(|| "vat".into(), |n| n.to_string_lossy().into_owned());
        let with = |suffix: &str| stem.with_file_name(format!("{name}{suffix}"));
        let [a, b] = mode.textures();
        Files { mode, obj: with(".obj"), textures: [with(&format!("_{a}.exr")), with(&format!("_{b}.exr"))], json: with(".vat.json") }
    }

    pub fn name(&self) -> String {
        self.obj.file_stem().map_or_else(|| "vat".into(), |s| s.to_string_lossy().into_owned())
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

// Writes the mesh, the two textures and the JSON describing them. Returns the layout.
pub fn write(bake: &Bake, stem: &Path, fps: f32) -> Result<(Files, Layout), String> {
    let layout = Layout::new(bake.elements, bake.frames)?;
    let files = Files::new(stem, bake.mode);
    if let Some(dir) = files.obj.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&files.obj, obj(bake)).map_err(|e| e.to_string())?;
    for which in 0..2 {
        write_exr(&files.textures[which], &layout, &texels(bake, &layout, which), bake.mode.half(which))?;
    }
    let [a, b] = bake.mode.textures();
    let (lo, hi) = bake.bounds;
    let json = serde_json::json!({
        "format": "procedural-fx-vat",
        "version": 2,
        "mode": bake.mode.name(),
        "mesh": file_name(&files.obj),
        a: file_name(&files.textures[0]),
        b: file_name(&files.textures[1]),
        "element": bake.mode.element(),
        "element_count": layout.points,
        "frame_count": layout.frames,
        "fps": fps,
        "texture_size": [layout.width, layout.height()],
        "width": layout.width,
        "rows_per_frame": layout.rows_per_frame,
        "bounds": [[lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z]],
        "uv": "a vertex's UV picks its element e: ((e + 0.5) / element_count, 0.5); e's texel at frame f is (e % width, f * rows_per_frame + e / width), from the top left",
        "space": "right-handed, y up (the OBJ's)",
    });
    std::fs::write(&files.json, serde_json::to_string_pretty(&json).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    Ok((files, layout))
}

// Cooks `object` (a geometry object, with its transform) at each time and bakes it as
// `mode` (None: the kind it suits). Simulations step as they cook. `progress` hears frames
// done and the total.
pub fn bake_object(graph: &crate::Graph, object: crate::NodeId, times: &[f32], mode: Option<Mode>, progress: &mut dyn FnMut(usize, usize)) -> Result<Bake, String> {
    let mut frames = Vec::with_capacity(times.len());
    for (i, &t) in times.iter().enumerate() {
        progress(i, times.len());
        frames.push(graph.cook(object, t, &mut std::collections::HashMap::new()));
    }
    progress(times.len(), times.len());
    bake(&frames, mode)
}

// ---------------------------------------------------------------------------
// Godot 4
// ---------------------------------------------------------------------------

// What each mode's vertex() does (GLSL), with `fetch(tex, e, f)` reading a texel.
fn godot_vertex(mode: Mode) -> &'static str {
    match mode {
        Mode::Soft => "	VERTEX += mix(fetch(offsets, e, f0).rgb, fetch(offsets, e, f1).rgb, blend);
	NORMAL = normalize(mix(fetch(normals, e, f0).rgb, fetch(normals, e, f1).rgb, blend));",
        Mode::Rigid => "	vec3 rest_centre = fetch(positions, e, 0).rgb;
	vec3 centre = mix(fetch(positions, e, f0).rgb, fetch(positions, e, f1).rgb, blend);
	vec4 q0 = fetch(rotations, e, f0);
	vec4 q1 = fetch(rotations, e, f1);
	q1 = dot(q0, q1) < 0.0 ? -q1 : q1;
	vec4 q = normalize(mix(q0, q1, blend));
	VERTEX = turn(q, VERTEX - rest_centre) + centre;
	NORMAL = turn(q, NORMAL);",
        // A changing mesh can't blend between frames: each frame's corners are its own.
        Mode::Fluid => "	VERTEX = fetch(positions, e, f0).rgb;
	NORMAL = fetch(normals, e, f0).rgb;",
    }
}

// A spatial shader playing the bake: put it (or name_vat_material.tres) on a MeshInstance3D
// showing name.obj. TIME drives it; set `frame` from a script instead with `use_time` off.
pub fn godot_shader(mode: Mode, layout: &Layout, fps: f32) -> String {
    let [a, b] = mode.textures();
    format!(
        "// ProceduralFX Studio vertex animation ({mode_name}).
shader_type spatial;

uniform sampler2D {a} : filter_nearest, repeat_disable;
uniform sampler2D {b} : filter_nearest, repeat_disable;
uniform int element_count = {points};
uniform int frame_count = {frames};
uniform int width = {width};
uniform int rows_per_frame = {rows};
uniform float fps = {fps:.3};
uniform bool use_time = true;
uniform bool looping = true;
// The frame to show when use_time is off (from 0; fractions blend to the next frame).
uniform float frame = 0.0;
uniform vec4 albedo : source_color = vec4(1.0);
uniform float roughness : hint_range(0.0, 1.0) = 0.6;

vec4 fetch(sampler2D tex, int e, int f) {{
	return texelFetch(tex, ivec2(e % width, f * rows_per_frame + e / width), 0);
}}

// The element a vertex's UV picks: its column and row within a frame (Godot's OBJ import
// flips v).
int element(vec2 uv) {{
	int column = clamp(int(uv.x * float(width)), 0, width - 1);
	int row = clamp(int((1.0 - uv.y) * float(rows_per_frame)), 0, rows_per_frame - 1);
	return min(row * width + column, element_count - 1);
}}

// Turns v by the quaternion q (x, y, z, w).
vec3 turn(vec4 q, vec3 v) {{
	vec3 t = 2.0 * cross(q.xyz, v);
	return v + q.w * t + cross(q.xyz, t);
}}

void vertex() {{
	int e = element(UV);
	float at = use_time ? TIME * fps : frame;
	at = looping ? mod(at, float(frame_count)) : clamp(at, 0.0, float(frame_count - 1));
	int f0 = int(floor(at));
	int f1 = looping ? (f0 + 1) % frame_count : min(f0 + 1, frame_count - 1);
	float blend = fract(at);
{vertex}
}}

void fragment() {{
	ALBEDO = albedo.rgb;
	ROUGHNESS = roughness;
}}
",
        mode_name = mode.name(),
        points = layout.points,
        frames = layout.frames,
        width = layout.width,
        rows = layout.rows_per_frame,
        vertex = godot_vertex(mode),
    )
}

// A ShaderMaterial using the shader and both textures, found beside it.
pub fn godot_material(files: &Files) -> String {
    let name = files.name();
    let [a, b] = files.mode.textures();
    let [ta, tb] = files.textures.each_ref().map(|p| file_name(p));
    format!(
        "[gd_resource type=\"ShaderMaterial\" load_steps=4 format=3]

[ext_resource type=\"Shader\" path=\"{name}_vat.gdshader\" id=\"1_shader\"]
[ext_resource type=\"Texture2D\" path=\"{ta}\" id=\"2_a\"]
[ext_resource type=\"Texture2D\" path=\"{tb}\" id=\"3_b\"]

[resource]
shader = ExtResource(\"1_shader\")
shader_parameter/{a} = ExtResource(\"2_a\")
shader_parameter/{b} = ExtResource(\"3_b\")
"
    )
}

// Import settings for a data texture: lossless (Godot would otherwise compress it once
// it's used in 3D, ruining the numbers), no mipmaps.
pub fn godot_texture_import() -> String {
    "[remap]

importer=\"texture\"
type=\"CompressedTexture2D\"

[params]

compress/mode=0
compress/high_quality=false
compress/lossy_quality=0.7
compress/hdr_compression=0
compress/normal_map=2
compress/channel_pack=0
mipmaps/generate=false
mipmaps/limit=-1
roughness/mode=0
process/fix_alpha_border=false
process/premult_alpha=false
process/normal_map_invert_y=false
process/hdr_as_srgb=false
process/hdr_clamp_exposure=false
process/size_limit=0
detect_3d/compress_to=0
"
    .to_owned()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

// Import settings for the OBJ: no mesh compression (it would round positions and UVs),
// no reordering of vertices.
pub fn godot_mesh_import() -> String {
    "[remap]

importer=\"wavefront_obj\"
importer_version=1
type=\"Mesh\"

[params]

generate_tangents=true
generate_lods=false
generate_shadow_mesh=false
generate_lightmap_uv2=false
generate_lightmap_uv2_texel_size=0.2
scale_mesh=Vector3(1, 1, 1)
offset_mesh=Vector3(0, 0, 0)
optimize_mesh=false
force_disable_mesh_compression=true
"
    .to_owned()
}

pub fn godot_paths(files: &Files) -> [PathBuf; 5] {
    let name = files.name();
    let beside = |file: String| files.obj.with_file_name(file);
    [
        beside(format!("{name}_vat.gdshader")),
        beside(format!("{name}_vat_material.tres")),
        with_suffix(&files.textures[0], ".import"),
        with_suffix(&files.textures[1], ".import"),
        with_suffix(&files.obj, ".import"),
    ]
}

pub fn write_godot(files: &Files, layout: &Layout, fps: f32) -> Result<(), String> {
    let [shader, material, import_a, import_b, import_mesh] = godot_paths(files);
    for (path, text) in [
        (shader, godot_shader(files.mode, layout, fps)),
        (material, godot_material(files)),
        (import_a, godot_texture_import()),
        (import_b, godot_texture_import()),
        (import_mesh, godot_mesh_import()),
    ] {
        std::fs::write(&path, text).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unity (URP)
// ---------------------------------------------------------------------------

// A 32-hex-digit Unity guid: the one already in `meta`'s file if there is one (so
// re-exports keep materials linked), else a new one.
fn unity_guid(meta: &Path, seed: &str) -> String {
    use sha2::{Digest, Sha256};
    if let Some(guid) = std::fs::read_to_string(meta).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("guid: ").map(|g| g.trim().to_owned()))) {
        if guid.len() == 32 && guid.chars().all(|c| c.is_ascii_hexdigit()) {
            return guid;
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    Sha256::digest(format!("{seed}:{}:{stamp}", meta.display()).as_bytes())[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn meta_path(path: &Path) -> PathBuf {
    with_suffix(path, ".meta")
}

pub fn unity_paths(files: &Files) -> [PathBuf; 2] {
    let name = files.name();
    [files.obj.with_file_name(format!("{name}_vat.shader")), files.obj.with_file_name(format!("{name}_vat.mat"))]
}

// Each mode's vertex code (HLSL). Unity's OBJ import mirrors x into its left-handed
// space, so positions, normals and rotations are mirrored the same way here.
fn unity_vertex(mode: Mode) -> &'static str {
    match mode {
        Mode::Soft => "                float3 offset = lerp(Fetch(_Offsets, e, f0).rgb, Fetch(_Offsets, e, f1).rgb, blend);
                float3 normal = normalize(lerp(Fetch(_Normals, e, f0).rgb, Fetch(_Normals, e, f1).rgb, blend));
                float3 position = v.positionOS.xyz + offset * float3(-1, 1, 1);
                normal *= float3(-1, 1, 1);",
        Mode::Rigid => "                float3 restCentre = Fetch(_Positions, e, 0).rgb * float3(-1, 1, 1);
                float3 centre = lerp(Fetch(_Positions, e, f0).rgb, Fetch(_Positions, e, f1).rgb, blend) * float3(-1, 1, 1);
                float4 q0 = Fetch(_Rotations, e, f0);
                float4 q1 = Fetch(_Rotations, e, f1);
                q1 = dot(q0, q1) < 0 ? -q1 : q1;
                // Mirroring x turns a rotation (x, y, z, w) into (x, -y, -z, w).
                float4 q = normalize(lerp(q0, q1, blend)) * float4(1, -1, -1, 1);
                float3 position = Turn(q, v.positionOS.xyz - restCentre) + centre;
                float3 normal = Turn(q, v.normalOS);",
        Mode::Fluid => "                float3 position = Fetch(_Positions, e, f0).rgb * float3(-1, 1, 1);
                float3 normal = Fetch(_Normals, e, f0).rgb * float3(-1, 1, 1);",
    }
}

// A URP shader playing the bake. Unity's textures' rows run from the bottom, which Fetch
// undoes. Needs the Linear colour space (URP's default): in Gamma, Unity changes the
// textures' values, so the shader shows magenta instead of wrong motion.
pub fn unity_shader(name: &str, mode: Mode, layout: &Layout, fps: f32) -> String {
    let [a, b] = mode.textures().map(|t| {
        let mut c = t.chars();
        c.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
    });
    format!(
        "// ProceduralFX Studio vertex animation ({mode_name}, URP).
Shader \"ProceduralFX/Vertex Animation/{name}\"
{{
    Properties
    {{
        [NoScaleOffset] _{a}(\"{a}\", 2D) = \"black\" {{}}
        [NoScaleOffset] _{b}(\"{b}\", 2D) = \"black\" {{}}
        _BaseColor(\"Color\", Color) = (1, 1, 1, 1)
        _Fps(\"Frames per second\", Float) = {fps:.3}
        [Toggle] _UseTime(\"Play by itself\", Float) = 1
        [Toggle] _Loop(\"Loop\", Float) = 1
        _Frame(\"Frame (when not playing by itself)\", Float) = 0
        [HideInInspector] _ElementCount(\"Element count\", Float) = {points}
        [HideInInspector] _FrameCount(\"Frame count\", Float) = {frames}
        [HideInInspector] _Width(\"Texture width\", Float) = {width}
        [HideInInspector] _RowsPerFrame(\"Rows per frame\", Float) = {rows}
        [HideInInspector] _TextureHeight(\"Texture height\", Float) = {height}
    }}
    SubShader
    {{
        Tags {{ \"RenderType\" = \"Opaque\" \"RenderPipeline\" = \"UniversalPipeline\" \"Queue\" = \"Geometry\" }}
        Pass
        {{
            Name \"ForwardLit\"
            Tags {{ \"LightMode\" = \"UniversalForward\" }}
            HLSLPROGRAM
            #pragma vertex vert
            #pragma fragment frag
            #pragma target 3.5
            #include \"Packages/com.unity.render-pipelines.universal/ShaderLibrary/Core.hlsl\"
            #include \"Packages/com.unity.render-pipelines.universal/ShaderLibrary/Lighting.hlsl\"

            TEXTURE2D(_{a});
            TEXTURE2D(_{b});
            CBUFFER_START(UnityPerMaterial)
                float4 _BaseColor;
                float _Fps, _UseTime, _Loop, _Frame, _ElementCount, _FrameCount, _Width, _RowsPerFrame, _TextureHeight;
            CBUFFER_END

            struct Attributes {{ float4 positionOS : POSITION; float3 normalOS : NORMAL; float2 uv : TEXCOORD0; }};
            struct Varyings {{ float4 positionCS : SV_POSITION; float3 normalWS : TEXCOORD0; }};

            // Element e's texel at frame f, counting rows from the bottom as Unity does.
            // (Unsigned: integer division is quicker that way.)
            #define Fetch(tex, e, f) LOAD_TEXTURE2D(tex, int2((uint)(e) % (uint)_Width, (int)_TextureHeight - 1 - (int)((uint)(f) * (uint)_RowsPerFrame + (uint)(e) / (uint)_Width)))

            // Turns v by the quaternion q (x, y, z, w).
            float3 Turn(float4 q, float3 v)
            {{
                float3 t = 2.0 * cross(q.xyz, v);
                return v + q.w * t + cross(q.xyz, t);
            }}

            Varyings vert(Attributes v)
            {{
                int count = (int)_FrameCount;
                // The element the UV picks: its column and row within a frame.
                int column = clamp((int)(v.uv.x * _Width), 0, (int)_Width - 1);
                int row = clamp((int)(v.uv.y * _RowsPerFrame), 0, (int)_RowsPerFrame - 1);
                int e = min(row * (int)_Width + column, (int)_ElementCount - 1);
                float at = _UseTime > 0.5 ? _Time.y * _Fps : _Frame;
                at = _Loop > 0.5 ? fmod(at, _FrameCount) : clamp(at, 0.0, _FrameCount - 1.0);
                int f0 = (int)floor(at);
                int f1 = _Loop > 0.5 ? (int)((uint)(f0 + 1) % (uint)count) : min(f0 + 1, count - 1);
                float blend = frac(at);
{vertex}
                Varyings o;
                o.positionCS = TransformObjectToHClip(position);
                o.normalWS = TransformObjectToWorldNormal(normalize(normal));
                return o;
            }}

            half4 frag(Varyings i) : SV_Target
            {{
            #ifdef UNITY_COLORSPACE_GAMMA
                // In Gamma colour space Unity bends the textures' numbers, so the motion is
                // wrong: show that plainly. Project Settings > Player > Color Space: Linear.
                return half4(1.0, 0.0, 1.0, 1.0);
            #endif
                float3 n = normalize(i.normalWS);
                Light light = GetMainLight();
                float3 lit = light.color * saturate(dot(n, light.direction)) + SampleSH(n);
                return half4(_BaseColor.rgb * lit, 1.0);
            }}
            ENDHLSL
        }}
    }}
}}
",
        mode_name = mode.name(),
        points = layout.points,
        frames = layout.frames,
        width = layout.width,
        rows = layout.rows_per_frame,
        height = layout.height(),
        vertex = unity_vertex(mode),
    )
}

// Import settings for a data texture: linear, no mipmaps, nearest filtering, clamped,
// uncompressed, never resized, kept at its precision (Unity would make full floats half);
// readable, for scripts.
fn unity_texture_meta(guid: &str, largest: usize, half: bool) -> String {
    let size = largest.next_power_of_two().clamp(32, 16384);
    // Unity's RGBAHalf and RGBAFloat formats.
    let format = if half { 17 } else { 20 };
    format!(
        "fileFormatVersion: 2
guid: {guid}
TextureImporter:
  internalIDToNameTable: []
  externalObjects: {{}}
  serializedVersion: 13
  mipmaps:
    mipMapMode: 0
    enableMipMap: 0
    sRGBTexture: 0
    linearTexture: 0
    fadeOut: 0
    borderMipMap: 0
    mipMapsPreserveCoverage: 0
    alphaTestReferenceValue: 0.5
    mipMapFadeDistanceStart: 1
    mipMapFadeDistanceEnd: 3
  bumpmap:
    convertToNormalMap: 0
    externalNormalMap: 0
    heightScale: 0.25
    normalMapFilter: 0
    flipGreenChannel: 0
  isReadable: 1
  streamingMipmaps: 0
  streamingMipmapsPriority: 0
  vTOnly: 0
  ignoreMipmapLimit: 1
  grayScaleToAlpha: 0
  generateCubemap: 6
  cubemapConvolution: 0
  seamlessCubemap: 0
  textureFormat: 1
  maxTextureSize: {size}
  textureSettings:
    serializedVersion: 2
    filterMode: 0
    aniso: 1
    mipBias: 0
    wrapU: 1
    wrapV: 1
    wrapW: 1
  nPOTScale: 0
  lightmap: 0
  compressionQuality: 50
  spriteMode: 0
  spriteExtrude: 1
  spriteMeshType: 1
  alignment: 0
  spritePivot: {{x: 0.5, y: 0.5}}
  spritePixelsToUnits: 100
  spriteBorder: {{x: 0, y: 0, z: 0, w: 0}}
  spriteGenerateFallbackPhysicsShape: 1
  alphaUsage: 0
  alphaIsTransparency: 0
  spriteTessellationDetail: -1
  textureType: 0
  textureShape: 1
  singleChannelComponent: 0
  flipbookRows: 1
  flipbookColumns: 1
  maxTextureSizeSet: 0
  compressionQualitySet: 0
  textureFormatSet: 0
  ignorePngGamma: 0
  applyGammaDecoding: 0
  swizzle: 50462976
  cookieLightType: 0
  platformSettings:
  - serializedVersion: 4
    buildTarget: DefaultTexturePlatform
    maxTextureSize: {size}
    resizeAlgorithm: 0
    textureFormat: {format}
    textureCompression: 0
    compressionQuality: 50
    crunchedCompression: 0
    allowsAlphaSplitting: 0
    overridden: 0
    ignorePlatformSupport: 0
    androidETC2FallbackOverride: 0
    forceMaximumCompressionQuality_BC6H_BC7: 0
  spriteSheet:
    serializedVersion: 2
    sprites: []
    outline: []
    customData:
    physicsShape: []
    bones: []
    spriteID:
    internalID: 0
    vertices: []
    indices:
    edges: []
    weights: []
    secondaryTextures: []
    spriteCustomMetadata:
      entries: []
    nameFileIdTable: {{}}
  mipmapLimitGroupName:
  pSDRemoveMatte: 0
  userData:
  assetBundleName:
  assetBundleVariant:
"
    )
}

#[allow(clippy::too_many_arguments)]
fn unity_material(name: &str, mode: Mode, shader: &str, textures: [&str; 2], layout: &Layout, fps: f32) -> String {
    let [a, b] = mode.textures().map(|t| {
        let mut c = t.chars();
        c.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
    });
    // Unity keeps texture properties sorted by name.
    let mut entries = [(a, textures[0]), (b, textures[1])];
    entries.sort();
    let tex_envs: String = entries
        .iter()
        .map(|(prop, guid)| format!("    - _{prop}:\n        m_Texture: {{fileID: 2800000, guid: {guid}, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n"))
        .collect();
    format!(
        "%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!21 &2100000
Material:
  serializedVersion: 8
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_Name: {name}_vat
  m_Shader: {{fileID: 4800000, guid: {shader}, type: 3}}
  m_Parent: {{fileID: 0}}
  m_ModifiedSerializedProperties: 0
  m_ValidKeywords: []
  m_InvalidKeywords: []
  m_LightmapFlags: 4
  m_EnableInstancingVariants: 0
  m_DoubleSidedGI: 0
  m_CustomRenderQueue: -1
  stringTagMap: {{}}
  disabledShaderPasses: []
  m_LockedProperties:
  m_SavedProperties:
    serializedVersion: 3
    m_TexEnvs:
{tex_envs}    m_Ints: []
    m_Floats:
    - _ElementCount: {points}
    - _Fps: {fps}
    - _Frame: 0
    - _FrameCount: {frames}
    - _Loop: 1
    - _RowsPerFrame: {rows}
    - _TextureHeight: {height}
    - _UseTime: 1
    - _Width: {width}
    m_Colors:
    - _BaseColor: {{r: 1, g: 1, b: 1, a: 1}}
  m_BuildTextureStacks: []
  m_AllowLocking: 1
",
        points = layout.points,
        frames = layout.frames,
        width = layout.width,
        rows = layout.rows_per_frame,
        height = layout.height(),
    )
}

// The shader, a material using it with both textures, and import settings (.meta files)
// for the textures, shader and material, beside the bake.
pub fn write_unity(files: &Files, layout: &Layout, fps: f32) -> Result<(), String> {
    let name = files.name();
    let [shader, material] = unity_paths(files);
    let guid = |path: &Path| unity_guid(&meta_path(path), &name);
    let (shader_guid, material_guid) = (guid(&shader), guid(&material));
    let texture_guids = files.textures.each_ref().map(|t| guid(t));
    let largest = layout.width.max(layout.height());
    let shader_meta = format!("fileFormatVersion: 2\nguid: {shader_guid}\nShaderImporter:\n  externalObjects: {{}}\n  defaultTextures: []\n  nonModifiableTextures: []\n  preprocessorOverride: 0\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n");
    let material_meta = format!("fileFormatVersion: 2\nguid: {material_guid}\nNativeFormatImporter:\n  externalObjects: {{}}\n  mainObjectFileID: 2100000\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n");
    let mut writes = vec![
        (shader.clone(), unity_shader(&name, files.mode, layout, fps)),
        (meta_path(&shader), shader_meta),
        (material.clone(), unity_material(&name, files.mode, &shader_guid, [&texture_guids[0], &texture_guids[1]], layout, fps)),
        (meta_path(&material), material_meta),
    ];
    for (which, (texture, g)) in files.textures.iter().zip(&texture_guids).enumerate() {
        writes.push((meta_path(texture), unity_texture_meta(g, largest, files.mode.half(which))));
    }
    for (path, text) in writes {
        std::fs::write(&path, text).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A triangle whose first point rises by 0.1 a frame.
    fn frames(count: usize) -> Vec<Mesh> {
        (0..count)
            .map(|f| Mesh {
                vertices: vec![Vec3::new(0.0, f as f32 * 0.1, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0)],
                indices: vec![[0, 1, 2]],
                ..Mesh::default()
            })
            .collect()
    }

    // Two separate boxes, each turning about its own centre and moving: rigid pieces.
    fn spinning_pieces(count: usize) -> Vec<Mesh> {
        let cube = |c: Vec3| -> (Vec<Vec3>, Vec<[usize; 3]>) {
            let mut v = Vec::new();
            for i in 0..8 {
                v.push(c.add(Vec3::new(if i & 1 == 0 { -0.5 } else { 0.5 }, if i & 2 == 0 { -0.3 } else { 0.3 }, if i & 4 == 0 { -0.2 } else { 0.2 })));
            }
            let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
            (v, quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect())
        };
        (0..count)
            .map(|f| {
                let t = f as f32 * 0.2;
                let mut mesh = Mesh::default();
                for (k, base) in [Vec3::new(-2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)].into_iter().enumerate() {
                    let (v, tris) = cube(Vec3::zero());
                    let start = mesh.vertices.len();
                    // A turn about a tilted axis, by a different amount for each piece.
                    let axis = Vec3::new(0.3, 1.0, 0.5).normalize();
                    let angle = t * (1.0 + k as f32);
                    let q = [(angle / 2.0).cos(), axis.x * (angle / 2.0).sin(), axis.y * (angle / 2.0).sin(), axis.z * (angle / 2.0).sin()];
                    let centre = base.add(Vec3::new(0.0, t * 3.0 - t * t, t * (k as f32 * 2.0 - 1.0)));
                    mesh.vertices.extend(v.iter().map(|p| rotate(q, *p).add(centre)));
                    mesh.indices.extend(tris.iter().map(|t| t.map(|i| i + start)));
                }
                mesh
            })
            .collect()
    }

    // What the shaders compute, on the CPU: every vertex's position at frame f, in OBJ space.
    fn rebuild(bake: &Bake, f: usize) -> Vec<Vec3> {
        let get = |which: usize, e: usize, frame: usize| bake.data[which][frame][e];
        let v3 = |c: [f32; 4]| Vec3::new(c[0], c[1], c[2]);
        bake.mesh
            .vertices
            .iter()
            .zip(&bake.element_of)
            .map(|(v, &e)| {
                let v = mirror(*v);
                match bake.mode {
                    Mode::Soft => v.add(v3(get(0, e, f))),
                    Mode::Rigid => {
                        let [x, y, z, w] = get(1, e, f);
                        rotate([w, x, y, z], v.sub(v3(get(0, e, 0)))).add(v3(get(0, e, f)))
                    }
                    Mode::Fluid => v3(get(0, e, f)),
                }
            })
            .collect()
    }

    #[test]
    fn layouts_wrap_wide_frames_onto_more_rows() {
        assert_eq!(Layout::new(3, 10).unwrap(), Layout { points: 3, frames: 10, width: 3, rows_per_frame: 1 });
        let wide = Layout::new(20_000, 48).unwrap();
        assert_eq!((wide.width, wide.rows_per_frame, wide.height()), (8192, 3, 144));
        assert_eq!(wide.texel(8192, 0), (0, 1));
        assert_eq!(wide.texel(8193, 2), (1, 7));
        assert!(Layout::new(8192, 9000).unwrap_err().contains("over the 8192-pixel limit"));
        assert!(Layout::new(0, 10).is_err());
    }

    #[test]
    fn the_mode_suits_the_motion() {
        assert_eq!(bake(&frames(4), None).unwrap().mode, Mode::Soft, "one piece, bending");
        assert_eq!(bake(&spinning_pieces(4), None).unwrap().mode, Mode::Rigid, "two pieces keeping their shape");
        let mut growing = frames(4);
        growing[2].vertices.push(Vec3::new(1.0, 1.0, 1.0));
        growing[2].indices.push([0, 1, 3]);
        assert_eq!(bake(&growing, None).unwrap().mode, Mode::Fluid, "the mesh changes");
        assert!(bake(&growing, Some(Mode::Soft)).unwrap_err().contains("Bake it as Fluid"));
        // Bending pieces aren't rigid.
        let mut bent = spinning_pieces(3);
        bent[2].vertices[0] = bent[2].vertices[0].add(Vec3::new(0.0, 0.5, 0.0));
        assert!(bake(&bent, Some(Mode::Rigid)).unwrap_err().contains("bends or stretches"));
        assert_eq!(bake(&bent, None).unwrap().mode, Mode::Soft);
        assert!(bake(&[Mesh { vertices: vec![Vec3::zero()], ..Mesh::default() }], None).unwrap_err().contains("no polygons"));
    }

    #[test]
    fn pieces_come_from_the_piece_attribute_or_connectivity() {
        let mesh = &spinning_pieces(1)[0];
        let (of, count) = pieces(mesh);
        assert_eq!(count, 2);
        assert!(of[..8].iter().all(|&k| k == of[0]) && of[8..].iter().all(|&k| k == of[8]) && of[0] != of[8]);
        let mut labelled = mesh.clone();
        labelled.point_attributes.insert("piece".into(), (0..16).map(|i| if i < 4 { 7.0 } else { 3.0 }).collect());
        let (of, count) = pieces(&labelled);
        assert_eq!((count, of[0], of[4]), (2, 1, 0));
    }

    // Every mode, rebuilt as the shaders do, gives every frame's shape.
    #[test]
    fn every_mode_rebuilds_every_frame() {
        // Soft and rigid: each vertex is a point of the frame.
        for (source, mode) in [(frames(5), Mode::Soft), (spinning_pieces(8), Mode::Rigid)] {
            let b = bake(&source, Some(mode)).unwrap();
            for (f, truth) in source.iter().enumerate() {
                for (p, (got, want)) in rebuild(&b, f).iter().zip(&truth.vertices).enumerate() {
                    assert!(got.sub(mirror(*want)).length() < 1e-4, "{mode:?} point {p} at frame {f}: {got:?} vs {want:?}");
                }
            }
        }
        // Rigid stores one element a piece.
        assert_eq!(bake(&spinning_pieces(3), Some(Mode::Rigid)).unwrap().elements, 2);
        // Fluid: each frame's triangles are the corners it uses, the rest collapse.
        let mut changing = frames(3);
        changing[1].vertices.push(Vec3::new(2.0, 2.0, 2.0));
        changing[1].indices.push([0, 1, 3]);
        let b = bake(&changing, Some(Mode::Fluid)).unwrap();
        assert_eq!(b.elements, 6, "room for the busiest frame's two triangles");
        for (f, truth) in changing.iter().enumerate() {
            let got = rebuild(&b, f);
            for (t, tri) in truth.indices.iter().enumerate() {
                for (k, &point) in tri.iter().enumerate() {
                    assert!(got[t * 3 + k].sub(mirror(truth.vertices[point])).length() < 1e-6, "frame {f} triangle {t}");
                }
            }
            for unused in got.iter().skip(truth.indices.len() * 3) {
                assert_eq!(*unused, Vec3::zero(), "unused corners collapse");
            }
        }
        // The OBJ's spare triangles span everything the animation covers.
        let (lo, hi) = b.bounds;
        assert!(hi.x >= 2.0 && hi.y >= 2.0 && lo.z <= -2.0);
    }

    #[test]
    fn offsets_are_in_the_objs_space() {
        let mut moving = frames(3);
        moving[2].vertices[2] = Vec3::new(0.0, 0.0, 1.5);
        let b = bake(&moving, Some(Mode::Soft)).unwrap();
        let layout = Layout::new(3, 3).unwrap();
        let t = texels(&b, &layout, 0);
        assert_eq!(t[0], [0.0, 0.0, 0.0, 1.0], "frame 1 is the rest pose");
        assert_eq!(t[2 + 2 * 3], [0.0, 0.0, -0.5, 1.0], "z is mirrored");
        let text = obj(&b);
        assert!(text.contains("v 0 0 -1\n"), "positions are mirrored like the other exports");
        assert!(text.contains("vt 0.5 0.5\n"), "the middle point's UV is the middle of its row");
        // Wide bakes: a UV gives the column and the row within a frame.
        let many = Bake { elements: 10_000, element_of: vec![0, 8191, 8192, 9999], ..bake(&frames(2), Some(Mode::Soft)).unwrap() };
        let text = obj(&many);
        let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("vt ")).collect();
        assert_eq!(lines, ["vt 0.00006103515625 0.25", "vt 0.99993896484375 0.25", "vt 0.00006103515625 0.75", "vt 0.22064208984375 0.75"]);
        assert!(text.contains("f 1/1/1 3/3/3 2/2/2\n"));
    }

    // An animated scene (moving noise on a grid) baked from the graph.
    #[test]
    fn scenes_bake_from_their_objects() {
        let graph = crate::Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nnode 2 40 100 0 obj noise 2.2 0.65\nparent 1 0\nparent 2 0\nlink 1 2 0\ndisplay 0 2\nnext 3\n");
        let times: Vec<f32> = (0..6).map(|f| f as f32 / 24.0).collect();
        let mut seen = Vec::new();
        let b = bake_object(&graph, 0, &times, None, &mut |done, total| seen.push((done, total))).unwrap();
        assert_eq!(seen.last(), Some(&(6, 6)));
        assert_eq!(b.mode, Mode::Soft);
        let mut moved = 0.0f32;
        for (f, &t) in times.iter().enumerate() {
            let truth = graph.cook(0, t, &mut std::collections::HashMap::new());
            for (got, want) in rebuild(&b, f).iter().zip(&truth.vertices) {
                assert!(got.sub(mirror(*want)).length() < 1e-5);
                moved = moved.max(got.sub(mirror(b.mesh.vertices[0])).length());
            }
        }
        assert!(moved > 0.01, "the noise moves the grid, so this tests something");
    }

    #[test]
    fn writing_makes_the_mesh_textures_and_description() {
        let dir = std::env::temp_dir().join(format!("pfx_vat_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (source, mode, a, b) in [(frames(5), Mode::Soft, "offsets", "normals"), (spinning_pieces(5), Mode::Rigid, "positions", "rotations")] {
            let (files, layout) = write(&bake(&source, Some(mode)).unwrap(), &dir.join(mode.name()), 24.0).unwrap();
            assert_eq!(files.textures[0].file_name().unwrap().to_string_lossy(), format!("{}_{a}.exr", mode.name()));
            assert_eq!(files.textures[1].file_name().unwrap().to_string_lossy(), format!("{}_{b}.exr", mode.name()));
            for path in [&files.obj, &files.textures[0], &files.textures[1], &files.json] {
                assert!(path.is_file(), "{}", path.display());
            }
            let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&files.json).unwrap()).unwrap();
            assert_eq!(json["mode"], mode.name());
            assert_eq!(json["frame_count"], 5);
            assert_eq!(json["element_count"], layout.points);
        }
        // Half floats for soft offsets: point 0 at frame 5 (row 4) has risen 0.4.
        let image = exr::prelude::read_first_rgba_layer_from_file(
            dir.join("soft_offsets.exr"),
            |size, _| vec![[0.0f32; 3]; size.width() * size.height()],
            |pixels, at, (r, g, b, _): (f32, f32, f32, f32)| pixels[at.x() + at.y() * 3] = [r, g, b],
        )
        .unwrap();
        assert!((image.layer_data.channel_data.pixels[4 * 3][1] - 0.4).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_files_carry_the_layout_and_keep_unity_ids() {
        let dir = std::env::temp_dir().join(format!("pfx_vat_engines_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (source, mode) in [(frames(5), Mode::Soft), (spinning_pieces(5), Mode::Rigid)] {
            let (files, layout) = write(&bake(&source, Some(mode)).unwrap(), &dir.join("wobble"), 12.0).unwrap();
            write_godot(&files, &layout, 12.0).unwrap();
            write_unity(&files, &layout, 12.0).unwrap();
            let read = |p: &Path| std::fs::read_to_string(p).unwrap();
            let [shader, material, import_a, _, import_mesh] = godot_paths(&files);
            assert!(read(&import_mesh).contains("force_disable_mesh_compression=true"));
            let gd = read(&shader);
            assert!(gd.contains(&format!("uniform int element_count = {};", layout.points)) && gd.contains("uniform int frame_count = 5;") && gd.contains("uniform float fps = 12.000;"));
            let [a, b] = mode.textures();
            assert!(gd.contains(&format!("uniform sampler2D {a}")) && gd.contains(&format!("uniform sampler2D {b}")));
            assert!(read(&material).contains(&format!("shader_parameter/{a} = ")));
            assert!(read(&import_a).contains("compress/mode=0") && read(&import_a).contains("detect_3d/compress_to=0"));
            let [unity_shader_path, unity_material_path] = unity_paths(&files);
            let hlsl = read(&unity_shader_path);
            assert!(hlsl.contains("Shader \"ProceduralFX/Vertex Animation/wobble\"") && hlsl.contains("_TextureHeight(\"Texture height\", Float) = 5"));
            assert!(hlsl.contains("UNITY_COLORSPACE_GAMMA"));
            let guid_of = |p: &Path| read(&meta_path(p)).lines().find_map(|l| l.strip_prefix("guid: ").map(str::to_owned)).unwrap();
            let mat = read(&unity_material_path);
            assert!(mat.contains(&format!("guid: {}, type: 3}}", guid_of(&unity_shader_path))), "the material uses the shader");
            assert!(mat.contains(&guid_of(&files.textures[0])) && mat.contains(&guid_of(&files.textures[1])), "and both textures");
            let texture_meta = read(&meta_path(&files.textures[0]));
            assert!(texture_meta.contains("sRGBTexture: 0") && texture_meta.contains("filterMode: 0") && texture_meta.contains("nPOTScale: 0") && texture_meta.contains("textureCompression: 0"));
            // Exporting again keeps every id, so scenes using the material stay linked.
            let before = guid_of(&unity_material_path);
            write_unity(&files, &layout, 12.0).unwrap();
            assert_eq!(guid_of(&unity_material_path), before);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
