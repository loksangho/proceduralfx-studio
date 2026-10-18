// Surface nodes: Normal, Scatter, and the UV tools (UV Texture, UV Transform,
// UV Quick Shade).
//
// UVs live on vertices as `vertex:uv` (three floats per triangle corner, w = 0),
// so projections can split them at seams without duplicating points.

use super::{hash01, Mesh, Vec3};
use crate::groom::TriangleGrid;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const VERTEX_UV: &str = "vertex:uv";

fn face_normal(mesh: &Mesh, t: usize) -> Vec3 {
    let [a, b, c] = mesh.indices[t].map(|i| mesh.vertices[i]);
    b.sub(a).cross(c.sub(a))
}

fn unit(v: Vec3) -> Vec3 {
    if v.length() > 1e-20 { v.normalize() } else { Vec3::new(0.0, 1.0, 0.0) }
}

// Interior angle of triangle `t` at corner `k`.
fn corner_angle(mesh: &Mesh, t: usize, k: usize) -> f32 {
    let tri = mesh.indices[t];
    let p = mesh.vertices[tri[k]];
    let (e1, e2) = (mesh.vertices[tri[(k + 1) % 3]].sub(p), mesh.vertices[tri[(k + 2) % 3]].sub(p));
    let denom = e1.length() * e2.length();
    if denom < 1e-20 { 0.0 } else { (e1.dot(e2) / denom).clamp(-1.0, 1.0).acos() }
}

// --------------------------------------------------------------------------
// Normal
// --------------------------------------------------------------------------

pub const NORMAL_DOMAINS: [&str; 2] = ["Points", "Vertices"];
pub const NORMAL_WEIGHTS: [&str; 3] = ["By area", "By angle", "Each face equally"];

#[derive(Clone, Debug, PartialEq)]
pub struct NormalSettings {
    // 0 = points (N), 1 = vertices (vertex:N, split at sharp edges).
    pub domain: usize,
    // Vertices: faces meeting at more than this angle (degrees) keep separate normals.
    pub cusp_angle: f32,
    pub weighting: usize,
    pub reverse: bool,
}

pub fn normal(input: &Mesh, set: &NormalSettings) -> Mesh {
    let mut out = input.clone();
    if input.indices.is_empty() {
        return out;
    }
    let faces: Vec<Vec3> = (0..input.indices.len()).map(|t| face_normal(input, t)).collect();
    let weight = |t: usize, k: usize| -> f32 {
        match set.weighting {
            0 => 1.0,
            1 => corner_angle(input, t, k) / faces[t].length().max(1e-20),
            _ => 1.0 / faces[t].length().max(1e-20),
        }
    };
    // Face normals are area-weighted as they come (|cross| = 2 * area).
    let sign = if set.reverse { -1.0 } else { 1.0 };
    let mut corners_of: Vec<Vec<(usize, usize)>> = vec![Vec::new(); input.vertices.len()];
    for (t, tri) in input.indices.iter().enumerate() {
        for k in 0..3 {
            corners_of[tri[k]].push((t, k));
        }
    }
    if set.domain == 0 {
        let mut values = Vec::with_capacity(input.vertices.len() * 3);
        for corners in &corners_of {
            let n = corners.iter().fold(Vec3::zero(), |acc, &(t, k)| acc.add(faces[t].scale(weight(t, k))));
            let n = unit(n).scale(sign);
            values.extend([n.x, n.y, n.z]);
        }
        out.point_attributes.insert("N".into(), values);
        out.point_attributes.remove("vertex:N");
    } else {
        let cos_cusp = set.cusp_angle.clamp(0.0, 180.0).to_radians().cos();
        let mut values = vec![0.0f32; input.indices.len() * 9];
        for (t, tri) in input.indices.iter().enumerate() {
            let own = unit(faces[t]);
            for k in 0..3 {
                // Average only the faces around this point that are smooth with this one.
                let n = corners_of[tri[k]]
                    .iter()
                    .filter(|&&(o, _)| unit(faces[o]).dot(own) >= cos_cusp - 1e-6)
                    .fold(Vec3::zero(), |acc, &(o, ok)| acc.add(faces[o].scale(weight(o, ok))));
                let n = unit(n).scale(sign);
                values[(t * 3 + k) * 3..(t * 3 + k) * 3 + 3].copy_from_slice(&[n.x, n.y, n.z]);
            }
        }
        out.point_attributes.insert("vertex:N".into(), values);
    }
    out
}

// The normal to shade each triangle corner with: vertex:N, then N, then computed.
pub fn corner_normals(mesh: &Mesh) -> Vec<[Vec3; 3]> {
    let tris = mesh.indices.len();
    let read = |values: &[f32], i: usize| Vec3::new(values[i * 3], values[i * 3 + 1], values[i * 3 + 2]);
    if let Some(v) = mesh.point_attributes.get("vertex:N").filter(|v| v.len() == tris * 9) {
        return (0..tris).map(|t| [0, 1, 2].map(|k| read(v, t * 3 + k))).collect();
    }
    let point: Vec<Vec3> = match mesh.point_attributes.get("N").filter(|v| v.len() == mesh.vertices.len() * 3) {
        Some(v) => (0..mesh.vertices.len()).map(|i| read(v, i)).collect(),
        None => mesh.compute_normals(),
    };
    mesh.indices.iter().map(|tri| tri.map(|i| point[i])).collect()
}

// --------------------------------------------------------------------------
// Scatter
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ScatterSettings {
    pub count: usize,
    pub seed: u32,
    // Lloyd-style passes pushing points apart for even spacing.
    pub relax_iterations: usize,
    // Fill the inside of a closed mesh instead of its surface.
    pub volume: bool,
}

pub fn scatter(input: &Mesh, set: &ScatterSettings) -> Result<Mesh, String> {
    if input.indices.is_empty() {
        return Err("the input has no polygons to scatter on".into());
    }
    let count = set.count.min(1_000_000);
    if set.volume { scatter_volume(input, count, set) } else { Ok(scatter_surface(input, count, set)) }
}

fn scatter_surface(input: &Mesh, count: usize, set: &ScatterSettings) -> Mesh {
    let mut cumulative = Vec::with_capacity(input.indices.len());
    let mut area = 0.0;
    for t in 0..input.indices.len() {
        area += face_normal(input, t).length() * 0.5;
        cumulative.push(area);
    }
    let mut points = Vec::with_capacity(count);
    let mut prims = Vec::with_capacity(count);
    for i in 0..count as u32 {
        let r = |k: u32| hash01(i.wrapping_mul(5).wrapping_add(k), set.seed);
        let t = cumulative.partition_point(|&c| c < r(1) * area).min(input.indices.len() - 1);
        let [a, b, c] = input.indices[t].map(|v| input.vertices[v]);
        let (mut u, mut v) = (r(2), r(3));
        if u + v > 1.0 {
            u = 1.0 - u;
            v = 1.0 - v;
        }
        points.push(a.add(b.sub(a).scale(u)).add(c.sub(a).scale(v)));
        prims.push(t);
    }
    // Typical spacing between evenly spread points on this much area.
    let spacing = (area / count.max(1) as f32).sqrt();
    if set.relax_iterations > 0 && count > 1 {
        let grid = TriangleGrid::new(input);
        for _ in 0..set.relax_iterations.min(50) {
            relax(&mut points, spacing);
            // Back onto the surface.
            for (p, prim) in points.iter_mut().zip(prims.iter_mut()) {
                if let Some((t, uv, _)) = grid.closest(input, *p) {
                    let [a, b, c] = input.indices[t].map(|v| input.vertices[v]);
                    *p = a.add(b.sub(a).scale(uv[0])).add(c.sub(a).scale(uv[1]));
                    *prim = t;
                }
            }
        }
    }
    let mut out = Mesh { vertices: points, ..Mesh::default() };
    out.point_attributes.insert("N".into(), prims.iter().flat_map(|&t| { let n = unit(face_normal(input, t)); [n.x, n.y, n.z] }).collect());
    out.point_attributes.insert("sourceprim".into(), prims.iter().map(|&t| t as f32).collect());
    out.point_attributes.insert("pscale".into(), vec![spacing * 0.5; out.vertices.len()]);
    out
}

fn scatter_volume(input: &Mesh, count: usize, set: &ScatterSettings) -> Result<Mesh, String> {
    let (lo, hi) = input.bounds().ok_or("the input is empty")?;
    let extent = hi.sub(lo);
    let voxel = (extent.x.max(extent.y).max(extent.z) / 128.0).max(1e-4);
    let sdf = crate::volume::from_polygons(input, voxel, 2.0)?;
    let mut points = Vec::with_capacity(count);
    let mut tries = 0u32;
    // Rejection sampling inside the bounds; give up if the mesh encloses (almost) nothing.
    while points.len() < count && tries < (count as u32).saturating_mul(200).max(10_000) {
        let r = |k: u32| hash01(tries.wrapping_mul(7).wrapping_add(k), set.seed);
        let p = Vec3::new(lo.x + r(1) * extent.x, lo.y + r(2) * extent.y, lo.z + r(3) * extent.z);
        if sdf.sample(p) < 0.0 {
            points.push(p);
        }
        tries += 1;
    }
    if points.is_empty() {
        return Err("nothing to fill: the input isn't a closed mesh".into());
    }
    let volume = points.len() as f32 / tries as f32 * extent.x * extent.y * extent.z;
    let spacing = (volume / points.len() as f32).cbrt();
    for _ in 0..set.relax_iterations.min(50) {
        let before = points.clone();
        relax(&mut points, spacing);
        // Points pushed outside stay where they were.
        for (p, old) in points.iter_mut().zip(before) {
            if sdf.sample(*p) >= 0.0 {
                *p = old;
            }
        }
    }
    let mut out = Mesh { vertices: points, ..Mesh::default() };
    out.point_attributes.insert("pscale".into(), vec![spacing * 0.5; out.vertices.len()]);
    Ok(out)
}

// One relaxation pass: each point moves away from neighbours closer than `spacing`.
fn relax(points: &mut [Vec3], spacing: f32) {
    if spacing <= 0.0 {
        return;
    }
    let key = |p: Vec3| [p.x, p.y, p.z].map(|c| (c / spacing).floor() as i32);
    let mut grid: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        grid.entry(key(*p)).or_default().push(i);
    }
    let snapshot = points.to_vec();
    for (i, p) in points.iter_mut().enumerate() {
        let k = key(snapshot[i]);
        let mut push = Vec3::zero();
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    for &j in grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]).map_or(&[][..], Vec::as_slice) {
                        if j == i {
                            continue;
                        }
                        let mut d = snapshot[i].sub(snapshot[j]);
                        if d.length() <= 1e-9 {
                            // Coincident points: separate them in a direction picked from the pair.
                            let (a, b) = (i.min(j) as u32, i.max(j) as u32);
                            let r = |k: u32| hash01(a.wrapping_mul(31).wrapping_add(b).wrapping_mul(3).wrapping_add(k), 7) - 0.5;
                            d = Vec3::new(r(1), r(2), r(3)).scale(if i < j { 1e-6 } else { -1e-6 });
                        }
                        let len = d.length();
                        if len < spacing {
                            push = push.add(d.scale((spacing - len) / len * 0.25));
                        }
                    }
                }
            }
        }
        *p = p.add(push);
    }
}

// --------------------------------------------------------------------------
// UV Texture (projection)
// --------------------------------------------------------------------------

pub const UV_PROJECTIONS: [&str; 4] = ["Planar", "Cylindrical", "Spherical", "Box"];
pub const AXES: [&str; 3] = ["X", "Y", "Z"];

#[derive(Clone, Debug, PartialEq)]
pub struct UvTextureSettings {
    pub projection: usize,
    // Planar: the axis projected along. Cylindrical: the cylinder axis.
    pub axis: usize,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

// The two axes perpendicular to `axis`, ordered so planar projections aren't mirrored.
fn plane_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (2, 1),
        1 => (0, 2),
        _ => (0, 1),
    }
}

fn comp(v: Vec3, a: usize) -> f32 {
    [v.x, v.y, v.z][a]
}

pub fn uv_texture(input: &Mesh, set: &UvTextureSettings) -> Mesh {
    let mut out = input.clone();
    let Some((lo, hi)) = input.bounds() else { return out };
    let centre = lo.add(hi).scale(0.5);
    let size = hi.sub(lo);
    let extent = |a: usize| comp(size, a).max(1e-9);
    let mut uv = Vec::with_capacity(input.indices.len() * 9);
    for (t, tri) in input.indices.iter().enumerate() {
        let corners = tri.map(|i| input.vertices[i]);
        let mut coords: [[f32; 2]; 3] = match set.projection {
            // Planar: fit the bounds to 0..1 across the plane.
            0 => {
                let (a, b) = plane_axes(set.axis.min(2));
                corners.map(|p| [(comp(p, a) - comp(lo, a)) / extent(a), (comp(p, b) - comp(lo, b)) / extent(b)])
            }
            // Cylindrical: angle around the axis, height along it.
            1 => {
                let axis = set.axis.min(2);
                let (a, b) = plane_axes(axis);
                corners.map(|p| {
                    let d = p.sub(centre);
                    let angle = comp(d, b).atan2(comp(d, a));
                    [angle / std::f32::consts::TAU + 0.5, (comp(p, axis) - comp(lo, axis)) / extent(axis)]
                })
            }
            // Spherical: longitude and latitude around the centre.
            2 => corners.map(|p| {
                let d = p.sub(centre);
                let r = d.length().max(1e-9);
                [d.z.atan2(d.x) / std::f32::consts::TAU + 0.5, (d.y / r).clamp(-1.0, 1.0).asin() / std::f32::consts::PI + 0.5]
            }),
            // Box: each face is projected along its dominant normal axis.
            _ => {
                let n = face_normal(input, t);
                let axis = [n.x.abs(), n.y.abs(), n.z.abs()].iter().enumerate().fold(0, |best, (i, v)| if *v > [n.x.abs(), n.y.abs(), n.z.abs()][best] { i } else { best });
                let (a, b) = plane_axes(axis);
                let flip = if comp(n, axis) < 0.0 { -1.0 } else { 1.0 };
                let span = size.x.max(size.y).max(size.z).max(1e-9);
                corners.map(|p| [flip * (comp(p, a) - comp(centre, a)) / span + 0.5, (comp(p, b) - comp(centre, b)) / span + 0.5])
            }
        };
        // Cylindrical and spherical wrap at u = 0/1: keep each face on one side of the seam.
        if matches!(set.projection, 1 | 2) {
            let max = coords.iter().map(|c| c[0]).fold(f32::MIN, f32::max);
            for c in &mut coords {
                if max - c[0] > 0.5 {
                    c[0] += 1.0;
                }
            }
        }
        for c in coords {
            uv.extend([c[0] * set.scale[0] + set.offset[0], c[1] * set.scale[1] + set.offset[1], 0.0]);
        }
    }
    out.point_attributes.insert(VERTEX_UV.into(), uv);
    out
}

// --------------------------------------------------------------------------
// UV Transform
// --------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct UvTransformSettings {
    pub translate: [f32; 2],
    // Degrees, around `pivot`.
    pub rotate: f32,
    pub scale: [f32; 2],
    pub pivot: [f32; 2],
}

pub fn uv_transform(input: &Mesh, set: &UvTransformSettings) -> Result<Mesh, String> {
    let mut out = input.clone();
    let key = [VERTEX_UV, "uv"].into_iter().find(|k| out.point_attributes.get(*k).is_some_and(|v| v.len() % 3 == 0 && !v.is_empty()));
    let Some(key) = key else { return Err("the input has no UVs (add a UV Texture node first)".into()) };
    let (s, c) = set.rotate.to_radians().sin_cos();
    for uv in out.point_attributes.get_mut(key).unwrap().chunks_exact_mut(3) {
        let (u, v) = ((uv[0] - set.pivot[0]) * set.scale[0], (uv[1] - set.pivot[1]) * set.scale[1]);
        uv[0] = c * u - s * v + set.pivot[0] + set.translate[0];
        uv[1] = s * u + c * v + set.pivot[1] + set.translate[1];
    }
    Ok(out)
}

// The UV of each triangle corner: vertex:uv, else a point uv attribute.
pub fn corner_uvs(mesh: &Mesh) -> Option<Vec<[[f32; 2]; 3]>> {
    let tris = mesh.indices.len();
    if let Some(v) = mesh.point_attributes.get(VERTEX_UV).filter(|v| v.len() == tris * 9) {
        return Some((0..tris).map(|t| [0, 1, 2].map(|k| [v[(t * 3 + k) * 3], v[(t * 3 + k) * 3 + 1]])).collect());
    }
    let v = mesh.point_attributes.get("uv").filter(|v| v.len() == mesh.vertices.len() * 3)?;
    Some(mesh.indices.iter().map(|tri| tri.map(|i| [v[i * 3], v[i * 3 + 1]])).collect())
}

// --------------------------------------------------------------------------
// UV Quick Shade
// --------------------------------------------------------------------------

// The texture a UV Quick Shade node shows is recorded on the geometry as
// detail:uvquickshade, an index into this list (0 = the built-in checker).
fn quickshade_files() -> &'static Mutex<Vec<String>> {
    static FILES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    FILES.get_or_init(|| Mutex::new(vec![String::new()]))
}

pub const QUICKSHADE: &str = "detail:uvquickshade";

pub fn uv_quickshade(input: &Mesh, file: &str) -> Result<Mesh, String> {
    if corner_uvs(input).is_none() {
        return Err("the input has no UVs (add a UV Texture node first)".into());
    }
    let index = {
        let mut files = quickshade_files().lock().unwrap_or_else(|e| e.into_inner());
        let file = file.trim().to_owned();
        match files.iter().position(|f| *f == file) {
            Some(i) => i,
            None => {
                files.push(file);
                files.len() - 1
            }
        }
    };
    let mut out = input.clone();
    out.point_attributes.insert(QUICKSHADE.into(), vec![index as f32]);
    Ok(out)
}

// The image file a quick-shaded mesh shows ("" for the checker), if it has one.
pub fn quickshade_file(mesh: &Mesh) -> Option<String> {
    let index = *mesh.point_attributes.get(QUICKSHADE)?.first()? as usize;
    quickshade_files().lock().unwrap_or_else(|e| e.into_inner()).get(index).cloned()
}

// Houdini-style UV grid: an 8 x 8 checker whose light squares are tinted by
// position, so orientation and stretching are easy to read.
pub fn checker_image(size: usize) -> Vec<[u8; 4]> {
    let cells = 8;
    let mut pixels = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let (cx, cy) = (x * cells / size, (size - 1 - y) * cells / size);
            let edge = x % (size / cells) == 0 || y % (size / cells) == 0;
            let light = (cx + cy) % 2 == 0;
            let [r, g, b] = if edge {
                [30, 30, 30]
            } else if light {
                [(150 + cx * 13) as u8, (150 + cy * 13) as u8, 210]
            } else {
                [70, 70, 80]
            };
            pixels.push([r, g, b, 255]);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        let mut mesh = Mesh::default();
        for k in 0..8 {
            mesh.vertices.push(Vec3::new((k & 1) as f32 - 0.5, ((k >> 1) & 1) as f32 - 0.5, ((k >> 2) & 1) as f32 - 0.5));
        }
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            mesh.indices.push([f[0], f[1], f[2]]);
            mesh.indices.push([f[0], f[2], f[3]]);
        }
        mesh
    }

    fn sphere(rows: usize, cols: usize) -> Mesh {
        let mut mesh = Mesh::default();
        for r in 0..=rows {
            let theta = std::f32::consts::PI * r as f32 / rows as f32;
            for c in 0..cols {
                let phi = std::f32::consts::TAU * c as f32 / cols as f32;
                mesh.vertices.push(Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()));
            }
        }
        for r in 0..rows {
            for c in 0..cols {
                let (a, b) = (r * cols + c, r * cols + (c + 1) % cols);
                let (d, e) = (a + cols, b + cols);
                mesh.indices.push([a, b, e]);
                mesh.indices.push([a, e, d]);
            }
        }
        mesh
    }

    fn vec_at(values: &[f32], i: usize) -> Vec3 {
        Vec3::new(values[i * 3], values[i * 3 + 1], values[i * 3 + 2])
    }

    #[test]
    fn cube_normals_points_and_cusped_vertices() {
        let cube = cube();
        let points = normal(&cube, &NormalSettings { domain: 0, cusp_angle: 60.0, weighting: 0, reverse: false });
        // Point normals at a cube corner point diagonally outward.
        let n = vec_at(&points.point_attributes["N"], 7);
        assert!((n.sub(Vec3::new(1.0, 1.0, 1.0).normalize())).length() < 1e-4, "{n:?}");
        // Vertex normals split at the 90 degree edges: each corner gets its face's normal.
        let vertices = normal(&cube, &NormalSettings { domain: 1, cusp_angle: 60.0, weighting: 1, reverse: false });
        let vn = &vertices.point_attributes["vertex:N"];
        for t in 0..cube.indices.len() {
            let face = unit(face_normal(&cube, t));
            for k in 0..3 {
                assert!(vec_at(vn, t * 3 + k).sub(face).length() < 1e-4);
            }
        }
        // A cusp above 90 degrees smooths them again.
        let smooth = normal(&cube, &NormalSettings { domain: 1, cusp_angle: 120.0, weighting: 0, reverse: true });
        let corner = vec_at(&smooth.point_attributes["vertex:N"], 0);
        assert!((corner.length() - 1.0).abs() < 1e-4 && corner.x.abs() > 0.3 && corner.y.abs() > 0.3);
        let corners = corner_normals(&smooth);
        assert_eq!(corners.len(), cube.indices.len());
        assert!(corners[0][0].sub(corner).length() < 1e-6);
    }

    #[test]
    fn scatter_on_surface_is_even_after_relaxing() {
        let s = sphere(16, 32);
        let nearest = |m: &Mesh| -> f32 {
            (0..m.vertices.len())
                .map(|i| (0..m.vertices.len()).filter(|&j| j != i).map(|j| m.vertices[i].sub(m.vertices[j]).length()).fold(f32::INFINITY, f32::min))
                .fold(f32::INFINITY, f32::min)
        };
        let rough = scatter(&s, &ScatterSettings { count: 300, seed: 1, relax_iterations: 0, volume: false }).unwrap();
        let relaxed = scatter(&s, &ScatterSettings { count: 300, seed: 1, relax_iterations: 10, volume: false }).unwrap();
        assert_eq!(relaxed.vertices.len(), 300);
        // Every point stays on the (approximately unit) sphere.
        for p in &relaxed.vertices {
            assert!((p.length() - 1.0).abs() < 0.03, "{p:?}");
        }
        assert!(nearest(&relaxed) > nearest(&rough) * 1.5, "{} vs {}", nearest(&relaxed), nearest(&rough));
        assert_eq!(relaxed.point_attributes["N"].len(), 900);
        assert!(scatter(&Mesh::default(), &ScatterSettings { count: 5, seed: 1, relax_iterations: 0, volume: false }).is_err());
    }

    #[test]
    fn scatter_in_volume_stays_inside() {
        let out = scatter(&cube(), &ScatterSettings { count: 400, seed: 2, relax_iterations: 3, volume: true }).unwrap();
        assert_eq!(out.vertices.len(), 400);
        assert!(out.vertices.iter().all(|p| p.x.abs() < 0.5 && p.y.abs() < 0.5 && p.z.abs() < 0.5));
        let mut open = cube();
        open.indices.truncate(10);
        let _ = scatter(&open, &ScatterSettings { count: 10, seed: 2, relax_iterations: 0, volume: true });
    }

    #[test]
    fn uv_projections_and_transform() {
        let cube = cube();
        for projection in 0..UV_PROJECTIONS.len() {
            let wraps = matches!(projection, 1 | 2);
            let out = uv_texture(&cube, &UvTextureSettings { projection, axis: 1, scale: [1.0, 1.0], offset: [0.0, 0.0] });
            let uvs = corner_uvs(&out).unwrap();
            assert_eq!(uvs.len(), cube.indices.len());
            for tri in &uvs {
                // No face straddles a seam: its corners are less than half the texture apart.
                let us: Vec<f32> = tri.iter().map(|c| c[0]).collect();
                if wraps {
                    assert!(us.iter().cloned().fold(f32::MIN, f32::max) - us.iter().cloned().fold(f32::MAX, f32::min) <= 0.5 + 1e-4, "{}", UV_PROJECTIONS[projection]);
                }
                assert!(tri.iter().all(|c| c[0].is_finite() && c[1].is_finite()));
            }
        }
        // Planar along Y maps the bounds to the unit square.
        let planar = uv_texture(&cube, &UvTextureSettings { projection: 0, axis: 1, scale: [2.0, 1.0], offset: [0.5, 0.0] });
        let all: Vec<[f32; 2]> = corner_uvs(&planar).unwrap().into_iter().flatten().collect();
        assert_eq!(all.iter().map(|c| c[0]).fold(f32::MIN, f32::max), 2.5);
        assert_eq!(all.iter().map(|c| c[1]).fold(f32::MAX, f32::min), 0.0);
        // Rotating 90 degrees about (0.5, 0.5) maps (1, 0.5) to (0.5, 1).
        let mut one = cube.clone();
        one.point_attributes.insert("uv".into(), [[1.0, 0.5, 0.0]; 8].concat());
        let turned = uv_transform(&one, &UvTransformSettings { translate: [0.0, 0.0], rotate: 90.0, scale: [1.0, 1.0], pivot: [0.5, 0.5] }).unwrap();
        let uv = &turned.point_attributes["uv"];
        assert!((uv[0] - 0.5).abs() < 1e-5 && (uv[1] - 1.0).abs() < 1e-5);
        assert!(uv_transform(&cube, &UvTransformSettings { translate: [0.0; 2], rotate: 0.0, scale: [1.0; 2], pivot: [0.0; 2] }).is_err());
    }

    #[test]
    fn quickshade_records_its_texture() {
        let cube = cube();
        assert!(uv_quickshade(&cube, "").is_err(), "needs UVs");
        let textured = uv_texture(&cube, &UvTextureSettings { projection: 3, axis: 0, scale: [1.0; 2], offset: [0.0; 2] });
        assert_eq!(quickshade_file(&uv_quickshade(&textured, "").unwrap()).as_deref(), Some(""));
        let file = uv_quickshade(&textured, "/tmp/grid.png").unwrap();
        assert_eq!(quickshade_file(&file).as_deref(), Some("/tmp/grid.png"));
        assert_eq!(checker_image(64).len(), 64 * 64);
    }
}
