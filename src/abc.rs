// Alembic (.abc) import and export, through native/abc_bridge.cpp.
//
// Import reads every mesh and point cloud at a time, in world space: faces
// become polygons; uv, N and Cd come along on points, corners or faces as the
// file stores them; ids and widths become id and pscale. Several objects get a
// `piece` attribute each. Export writes frames of one mesh (or, without faces,
// one point cloud) into a single file, sampled at the frame rate.
//
// Alembic is right-handed with clockwise faces: Z is mirrored, as for USD and
// OBJ, and the mirror alone turns its winding into ours.

use super::{fileio, poly, sops, Mesh, Vec3};

fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, -v.z)
}

// One object of the bridge's description.
#[derive(Default)]
struct Object {
    particles: bool,
    points: Vec<f32>,
    counts: Vec<usize>,
    indices: Vec<usize>,
    // (scope, values): scope is point, corner or face.
    uv: Option<(String, Vec<f32>)>,
    normals: Option<(String, Vec<f32>)>,
    color: Option<(String, Vec<f32>)>,
    ids: Vec<f32>,
    widths: Vec<f32>,
}

// The archive's time range and its objects.
pub(crate) fn parse(text: &str) -> Result<((f64, f64), Vec<Mesh>), String> {
    let mut range = (0.0, 0.0);
    let mut objects: Vec<Object> = Vec::new();
    for line in text.lines() {
        let mut tokens = line.split_whitespace();
        let Some(keyword) = tokens.next() else { continue };
        let rest: Vec<&str> = tokens.collect();
        let numbers = |from: usize| -> Vec<f32> { rest.iter().skip(from + 1).filter_map(|t| t.parse().ok()).collect() };
        let current = objects.last_mut();
        match (keyword, current) {
            ("range", _) => {
                let v: Vec<f64> = rest.iter().filter_map(|t| t.parse().ok()).collect();
                range = (v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0));
            }
            ("mesh", _) => objects.push(Object::default()),
            ("particles", _) => objects.push(Object { particles: true, ..Object::default() }),
            ("points", Some(o)) => o.points = numbers(0),
            ("counts", Some(o)) => o.counts = numbers(0).into_iter().map(|v| v as usize).collect(),
            ("indices", Some(o)) => o.indices = numbers(0).into_iter().map(|v| v as usize).collect(),
            ("uv", Some(o)) => o.uv = Some((rest.first().copied().unwrap_or("point").to_owned(), numbers(1))),
            ("normals", Some(o)) => o.normals = Some((rest.first().copied().unwrap_or("point").to_owned(), numbers(1))),
            ("color", Some(o)) => o.color = Some((rest.first().copied().unwrap_or("point").to_owned(), numbers(1))),
            ("ids", Some(o)) => o.ids = numbers(0),
            ("widths", Some(o)) => o.widths = numbers(0),
            _ => {}
        }
    }
    Ok((range, objects.into_iter().map(to_mesh).collect()))
}

fn to_mesh(o: Object) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.vertices = o.points.chunks_exact(3).map(|p| mirror(Vec3::new(p[0], p[1], p[2]))).collect();
    let n = mesh.vertices.len();
    if o.particles {
        if o.ids.len() == n {
            mesh.point_attributes.insert("id".into(), o.ids);
        }
        if o.widths.len() == n {
            mesh.point_attributes.insert("pscale".into(), o.widths);
        }
        if let Some((_, cd)) = o.color.filter(|(_, c)| c.len() == n * 3) {
            mesh.point_attributes.insert("Cd".into(), cd);
        }
        return mesh;
    }
    // Fan-triangulate each face; remember each triangle corner's face vertex and face.
    let (mut corner_of, mut face_of, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
    let mut offset = 0;
    for (face, &count) in o.counts.iter().enumerate() {
        let before = mesh.indices.len();
        for k in 1..count.saturating_sub(1) {
            let corners = [offset, offset + k, offset + k + 1];
            if corners.iter().all(|&c| o.indices.get(c).is_some_and(|&p| p < n)) {
                mesh.indices.push(corners.map(|c| o.indices[c]));
                corner_of.push(corners);
                face_of.push(face);
            }
        }
        sizes.push(mesh.indices.len() - before);
        offset += count;
    }
    // Values per triangle corner, from point, corner or face scoped data of `width` components.
    let per_corner = |(scope, values): &(String, Vec<f32>), width: usize| -> Option<Vec<f32>> {
        let pick = |i: usize| values.get(i * width..i * width + width);
        let mut out = Vec::with_capacity(corner_of.len() * 3 * width);
        for (t, corners) in corner_of.iter().enumerate() {
            for (k, &c) in corners.iter().enumerate() {
                let index = match scope.as_str() {
                    "corner" => c,
                    "face" => face_of[t],
                    _ => mesh.indices[t][k],
                };
                out.extend_from_slice(pick(index)?);
            }
        }
        Some(out)
    };
    if let Some(uv) = o.uv.as_ref().and_then(|uv| per_corner(uv, 2)) {
        mesh.point_attributes.insert("vertex:uv".into(), uv.chunks_exact(2).flat_map(|c| [c[0], c[1], 0.0]).collect());
    }
    if let Some(normals) = o.normals.as_ref().and_then(|nm| per_corner(nm, 3)) {
        mesh.point_attributes.insert("vertex:N".into(), normals.chunks_exact(3).flat_map(|c| [c[0], c[1], -c[2]]).collect());
    }
    if let Some((scope, cd)) = &o.color {
        match scope.as_str() {
            "point" if cd.len() == n * 3 => {
                mesh.point_attributes.insert("Cd".into(), cd.clone());
            }
            "face" => {
                let v: Vec<f32> = face_of.iter().flat_map(|f| cd.get(f * 3..f * 3 + 3).map_or([0.7; 3], |c| [c[0], c[1], c[2]])).collect();
                mesh.point_attributes.insert("prim:Cd".into(), v);
            }
            _ => {
                if let Some(v) = per_corner(&(scope.clone(), cd.clone()), 3) {
                    mesh.point_attributes.insert("vertex:Cd".into(), v);
                }
            }
        }
    }
    if sizes.iter().any(|s| *s > 1) {
        poly::tag(&mut mesh, 0, sizes);
    }
    mesh
}

// Everything in the file at `seconds`, as one geometry (`piece` tells objects apart).
pub(crate) fn assemble(meshes: Vec<Mesh>) -> Mesh {
    let several = meshes.len() > 1;
    meshes.into_iter().enumerate().fold(Mesh::default(), |merged, (i, mut mesh)| {
        if several {
            mesh.point_attributes.insert("piece".into(), vec![i as f32; mesh.vertices.len()]);
            mesh.point_attributes.insert("prim:piece".into(), vec![i as f32; mesh.indices.len()]);
        }
        crate::node_merge(merged, mesh)
    })
}

fn list<T: std::fmt::Display>(out: &mut String, name: &str, values: impl IntoIterator<Item = T>) {
    let values: Vec<String> = values.into_iter().map(|v| v.to_string()).collect();
    out.push_str(&format!("{name} {} {}\n", values.len(), values.join(" ")));
}

// The bridge's export text for frames (seconds, geometry) at `fps`.
pub(crate) fn export_text(frames: &[(f32, Mesh)], fps: f32) -> Result<String, String> {
    let first = &frames.first().ok_or("no frames to write")?.1;
    let points_only = first.indices.is_empty();
    let mut out = format!("fps {fps} kind {} name geometry\n", if points_only { "points" } else { "mesh" });
    for (time, mesh) in frames {
        out.push_str(&format!("frame {time}\n"));
        list(&mut out, "points", mesh.vertices.iter().flat_map(|v| { let m = mirror(*v); [m.x, m.y, m.z] }));
        let n = mesh.vertices.len();
        if points_only {
            if let Some(ids) = mesh.point_attributes.get("id").filter(|v| v.len() == n) {
                list(&mut out, "ids", ids.iter().map(|v| v.max(0.0) as u64));
            }
            if let Some(widths) = mesh.point_attributes.get("pscale").filter(|v| v.len() == n) {
                list(&mut out, "widths", widths.iter().copied());
            }
            if let Some(cd) = mesh.point_attributes.get("Cd").filter(|v| v.len() == n * 3) {
                out.push_str("color point ");
                list(&mut out, "", cd.iter().copied());
            }
            continue;
        }
        // Faces in our winding (mirrored, that's Alembic's clockwise).
        let faces = poly::faces(mesh);
        let corners: Vec<(usize, usize)> = faces.iter().flat_map(|f| f.corners.iter().copied()).collect();
        list(&mut out, "counts", faces.iter().map(|f| f.corners.len()));
        list(&mut out, "indices", corners.iter().map(|c| c.0));
        if let Some(uvs) = fileio::corner_values(mesh, "uv") {
            list(&mut out, "uv", corners.iter().flat_map(|c| [uvs[c.1][0], uvs[c.1][1]]));
        }
        let normals = sops::corner_normals(mesh);
        list(&mut out, "normals", corners.iter().flat_map(|c| { let v = normals[c.1 / 3][c.1 % 3]; [v.x, v.y, -v.z] }));
        let tris = mesh.indices.len();
        if let Some(cd) = mesh.point_attributes.get("vertex:Cd").filter(|v| v.len() == tris * 9) {
            out.push_str("color corner ");
            list(&mut out, "", corners.iter().flat_map(|c| [cd[c.1 * 3], cd[c.1 * 3 + 1], cd[c.1 * 3 + 2]]));
        } else if let Some(cd) = mesh.point_attributes.get("Cd").filter(|v| v.len() == n * 3) {
            out.push_str("color point ");
            list(&mut out, "", cd.iter().copied());
        } else if let Some(cd) = mesh.point_attributes.get("prim:Cd").filter(|v| v.len() == tris * 3) {
            out.push_str("color face ");
            list(&mut out, "", faces.iter().flat_map(|f| [cd[f.tris.start * 3], cd[f.tris.start * 3 + 1], cd[f.tris.start * 3 + 2]]));
        }
    }
    Ok(out)
}

#[cfg(feature = "native-alembic")]
mod native {
    use std::ffi::{c_char, c_int, CStr, CString};

    extern "C" {
        fn pfx_abc_read(path: *const c_char, seconds: f64, error: *mut *mut c_char) -> *mut c_char;
        fn pfx_abc_write(path: *const c_char, data: *const c_char, error: *mut *mut c_char) -> c_int;
        fn pfx_abc_free(text: *mut c_char);
    }

    fn take(text: *mut c_char) -> String {
        let s = unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned();
        unsafe { pfx_abc_free(text) };
        s
    }

    pub fn read(path: &str, seconds: f64) -> Result<String, String> {
        let path = CString::new(path).map_err(|e| e.to_string())?;
        let mut error = std::ptr::null_mut();
        let text = unsafe { pfx_abc_read(path.as_ptr(), seconds, &mut error) };
        if text.is_null() {
            return Err(if error.is_null() { "could not read the Alembic file".into() } else { take(error) });
        }
        Ok(take(text))
    }

    pub fn write(path: &str, data: &str) -> Result<(), String> {
        let (path, data) = (CString::new(path).map_err(|e| e.to_string())?, CString::new(data).map_err(|e| e.to_string())?);
        let mut error = std::ptr::null_mut();
        if unsafe { pfx_abc_write(path.as_ptr(), data.as_ptr(), &mut error) } == 0 {
            return Err(if error.is_null() { "could not write the Alembic file".into() } else { take(error) });
        }
        Ok(())
    }
}

#[cfg(not(feature = "native-alembic"))]
mod native {
    const MISSING: &str = "this build has no Alembic support (build with the native-alembic feature)";
    pub fn read(_: &str, _: f64) -> Result<String, String> {
        Err(MISSING.into())
    }
    pub fn write(_: &str, _: &str) -> Result<(), String> {
        Err(MISSING.into())
    }
}

pub fn read(path: &str, seconds: f32) -> Result<Mesh, String> {
    if !std::path::Path::new(path).exists() {
        return Err(format!("file not found: {path}"));
    }
    let (_, meshes) = parse(&native::read(path, seconds as f64)?)?;
    Ok(assemble(meshes))
}

// Whether the file's contents change over time (cached per file and modification time).
pub fn is_animated(path: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, (Option<std::time::SystemTime>, bool)>>> = OnceLock::new();
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((when, animated)) = cache.lock().ok().and_then(|c| c.get(path).copied()) {
        if when == modified {
            return animated;
        }
    }
    let animated = native::read(path, 0.0).ok().and_then(|text| parse(&text).ok()).is_some_and(|((start, end), _)| end > start);
    if let Ok(mut c) = cache.lock() {
        c.insert(path.to_owned(), (modified, animated));
    }
    animated
}

pub fn write(path: &str, frames: &[(f32, Mesh)], fps: f32) -> Result<(), String> {
    native::write(path, &export_text(frames, fps)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_text_and_import_parse_agree() {
        // A cube with point colours, written as two frames and read back from the same text shape.
        let mut cube = crate::node_box(2.0, 1);
        let n = cube.vertices.len();
        cube.point_attributes.insert("Cd".into(), (0..n * 3).map(|i| (i % 3) as f32 * 0.5).collect());
        let text = export_text(&[(0.0, cube.clone()), (1.0 / 24.0, cube.clone())], 24.0).unwrap();
        assert!(text.starts_with("fps 24 kind mesh"));
        assert_eq!(text.matches("frame ").count(), 2);
        // The bridge's read format for the first frame: the same points, faces and colours.
        let frame = text.split("frame ").nth(1).unwrap();
        let line = |name: &str| frame.lines().find(|l| l.starts_with(name)).unwrap().to_owned();
        let dump = format!("range 0 0.0416\nmesh /geometry\n{}\n{}\n{}\n{}\n", line("points"), line("counts"), line("indices"), line("color point").replacen("color point  ", "color point ", 1));
        let ((start, end), meshes) = parse(&dump).unwrap();
        assert!(end > start);
        let back = assemble(meshes);
        assert_eq!(back.vertices.len(), n);
        assert!(back.vertices.iter().zip(&cube.vertices).all(|(a, b)| a.sub(*b).length() < 1e-5));
        assert_eq!(poly::count(&back), 6, "six quad faces");
        // Same winding: the cube still faces outwards.
        let volume = |m: &Mesh| m.indices.iter().map(|t| { let [a, b, c] = t.map(|i| m.vertices[i]); a.dot(b.cross(c)) / 6.0 }).sum::<f32>();
        assert!((volume(&back) - 8.0).abs() < 1e-3);
        assert_eq!(back.point_attributes["Cd"], cube.point_attributes["Cd"]);
        // Points without faces export as a point cloud.
        let mut cloud = Mesh::default();
        cloud.vertices = vec![Vec3::new(1.0, 2.0, 3.0)];
        cloud.point_attributes.insert("pscale".into(), vec![0.5]);
        let text = export_text(&[(0.0, cloud)], 24.0).unwrap();
        assert!(text.contains("kind points") && text.contains("widths 1 0.5"));
    }
}

#[cfg(all(test, feature = "native-alembic"))]
mod native_tests {
    use super::*;

    #[test]
    fn alembic_files_round_trip_with_animation() {
        let path = std::env::temp_dir().join("pfx_abc_round_trip.abc");
        let path = path.to_str().unwrap();
        let mut cube = crate::node_box(2.0, 1);
        let n = cube.vertices.len();
        cube.point_attributes.insert("Cd".into(), vec![0.25; n * 3]);
        let moved = crate::node_transform(cube.clone(), [3.0, 0.0, 0.0], [0.0; 3], [1.0; 3]);
        write(path, &[(0.0, cube.clone()), (1.0, moved)], 24.0).unwrap();
        assert!(is_animated(path));
        let at = |t: f32| read(path, t).unwrap();
        let (start, end) = (at(0.0), at(1.0));
        assert_eq!((start.vertices.len(), poly::count(&start)), (n, 6));
        assert!(start.vertices.iter().zip(&cube.vertices).all(|(a, b)| a.sub(*b).length() < 1e-5));
        let shift = end.vertices[0].sub(start.vertices[0]);
        assert!((shift.x - 3.0).abs() < 1e-4, "{shift:?}");
        assert_eq!(start.point_attributes["Cd"], vec![0.25; n * 3]);
        assert!(start.point_attributes.contains_key("vertex:N"));
        // A point cloud, with ids and pscale.
        let mut cloud = Mesh::default();
        cloud.vertices = vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(-1.0, 0.0, 0.5)];
        cloud.point_attributes.insert("pscale".into(), vec![0.5, 0.25]);
        write(path, &[(0.0, cloud.clone())], 24.0).unwrap();
        let back = read(path, 0.0).unwrap();
        assert_eq!(back.vertices, cloud.vertices);
        assert_eq!(back.point_attributes["pscale"], vec![0.5, 0.25]);
        assert_eq!(back.point_attributes["id"], vec![0.0, 1.0]);
        let _ = std::fs::remove_file(path);
    }
}
