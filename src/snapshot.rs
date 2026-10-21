// Snapshots for the AI assistant: the scene rendered like the viewport (same
// camera convention, software raster), so Claude can see what it built.
//
// Lit from the camera (nothing is black just because it faces away from a
// fixed light), on a ground grid at y = 0 with the X/Y/Z axes, colours from Cd,
// particles as dots and curves as lines. Returns a PNG.

use super::*;

pub const VIEWS: [&str; 5] = ["perspective", "front", "side", "top", "below"];

pub struct View {
    // Degrees: turn around Y, and tilt down (positive looks from above).
    pub azimuth: f32,
    pub elevation: f32,
    pub outlines: bool,
}

impl View {
    pub fn named(name: &str) -> Option<View> {
        let (azimuth, elevation) = match name {
            "perspective" => (35.0, 25.0),
            "front" => (0.0, 0.0),
            "side" => (90.0, 0.0),
            "top" => (0.0, 89.0),
            "below" => (0.0, -60.0),
            _ => return None,
        };
        Some(View { azimuth, elevation, outlines: false })
    }
}

// Corner colours: vertex:Cd, Cd, prim:Cd, detail:Cd, else a neutral grey.
fn corner_colors(mesh: &Mesh) -> Vec<[[f32; 3]; 3]> {
    let (n, t) = (mesh.vertices.len(), mesh.indices.len());
    let get = |key: &str, count: usize| mesh.point_attributes.get(key).filter(|v| count > 0 && v.len() == count * 3);
    let rgb = |v: &[f32], i: usize| [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]];
    (0..t)
        .map(|tri| {
            std::array::from_fn(|k| {
                if let Some(v) = get("vertex:Cd", t * 3) {
                    rgb(v, tri * 3 + k)
                } else if let Some(v) = get("Cd", n) {
                    rgb(v, mesh.indices[tri][k])
                } else if let Some(v) = get("prim:Cd", t) {
                    rgb(v, tri)
                } else if let Some(v) = mesh.point_attributes.get("detail:Cd").filter(|v| v.len() >= 3) {
                    [v[0], v[1], v[2]]
                } else {
                    [0.72, 0.72, 0.74]
                }
            })
        })
        .collect()
}

fn png(raster: &raster::Raster, background: [u8; 3]) -> Result<Vec<u8>, String> {
    let mut image = image::RgbImage::new(raster.width as u32, raster.height as u32);
    for (i, c) in raster.color.iter().enumerate() {
        let a = c[3] as f32 / 255.0;
        let mix = |k: usize| (c[k] as f32 * a + background[k] as f32 * (1.0 - a)).round() as u8;
        image.put_pixel((i % raster.width) as u32, (i / raster.width) as u32, image::Rgb([mix(0), mix(1), mix(2)]));
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

// Renders `mesh` (world space) as seen from `view`; `width` × `height` pixels.
pub fn render(mesh: &Mesh, view: &View, width: usize, height: usize) -> Result<Vec<u8>, String> {
    let (lo, hi) = mesh.bounds().ok_or("there's nothing to show")?;
    let target = lo.add(hi).scale(0.5);
    let radius = hi.sub(lo).length().max(1e-3) * 0.5;
    let focal = height as f32 * 1.3;
    // Far enough that the bounding sphere fits the frame (with a little room).
    let distance = radius * focal / (height as f32 * 0.46) + radius * 0.25;
    let (sin_x, cos_x) = view.elevation.to_radians().sin_cos();
    let (sin_y, cos_y) = view.azimuth.to_radians().sin_cos();
    // The viewport's camera: turn about Y, then tilt about X, then step back.
    let to_cam = |p: Vec3| {
        let v = p.sub(target);
        let rx = v.x * cos_y - v.z * sin_y;
        let rz = v.x * sin_y + v.z * cos_y;
        Vec3::new(rx, v.y * cos_x - rz * sin_x, v.y * sin_x + rz * cos_x + distance)
    };
    // Camera space back to a world direction (for the light).
    let to_world_dir = |c: Vec3| {
        let vy = c.y * cos_x + c.z * sin_x;
        let rz = -c.y * sin_x + c.z * cos_x;
        Vec3::new(c.x * cos_y + rz * sin_y, vy, -c.x * sin_y + rz * cos_y)
    };
    let light = to_world_dir(Vec3::new(0.35, 0.6, -1.0).normalize()).normalize();
    let project = |p: Vec3| {
        let c = to_cam(p);
        raster::ScreenVertex { x: width as f32 / 2.0 + c.x / c.z * focal, y: height as f32 / 2.0 - c.y / c.z * focal, z: c.z }
    };
    let mut r = raster::Raster::new(width, height, false);
    let near = distance * 0.02;
    // The ground grid at y = 0 (under the geometry's footprint) and the axes at the origin,
    // drawn first so the geometry covers them where it's in front.
    let extent = (hi.x - lo.x).max(hi.z - lo.z).max(1.0) * 0.6;
    let step = 10f32.powf((extent / 5.0).log10().floor());
    let (gx, gz) = (target.x, target.z);
    let lines = (extent / step).ceil() as i32;
    for i in -lines..=lines {
        let o = i as f32 * step;
        for (a, b) in [
            (Vec3::new(gx - extent, 0.0, (gz / step).round() * step + o), Vec3::new(gx + extent, 0.0, (gz / step).round() * step + o)),
            (Vec3::new((gx / step).round() * step + o, 0.0, gz - extent), Vec3::new((gx / step).round() * step + o, 0.0, gz + extent)),
        ] {
            if to_cam(a).z > near && to_cam(b).z > near {
                r.line(project(a), project(b), [120, 120, 120, 90]);
            }
        }
    }
    let axis = radius * 0.3;
    for (dir, rgba) in [(Vec3::new(axis, 0.0, 0.0), [230, 80, 80, 255]), (Vec3::new(0.0, axis, 0.0), [90, 210, 100, 255]), (Vec3::new(0.0, 0.0, axis), [90, 140, 240, 255])] {
        if to_cam(Vec3::zero()).z > near && to_cam(dir).z > near {
            r.line(project(Vec3::zero()), project(dir), rgba);
        }
    }
    // Surfaces.
    let normals = sops::corner_normals(mesh);
    let colors = corner_colors(mesh);
    for (t, tri) in mesh.indices.iter().enumerate() {
        let pts = tri.map(|i| mesh.vertices[i]);
        if pts.iter().any(|p| to_cam(*p).z < near) {
            continue;
        }
        let shade = std::array::from_fn(|k| {
            let l = 0.28 + 0.72 * normals[t][k].dot(light).max(0.0);
            colors[t][k].map(|c| (c.clamp(0.0, 1.0) * l).min(1.0))
        });
        r.triangle(pts.map(project), shade, None);
    }
    // Polygon outlines, when asked for.
    if view.outlines {
        for (tri, shown) in mesh.indices.iter().zip(poly::outline_edges(mesh)) {
            for k in (0..3).filter(|k| shown[*k]) {
                r.line(project(mesh.vertices[tri[k]]), project(mesh.vertices[tri[(k + 1) % 3]]), [20, 20, 20, 150]);
            }
        }
    }
    // Curves as lines, particles (points no polygon uses) as dots.
    let curves = groom::curves(mesh);
    for c in &curves {
        for i in c.start..c.end - 1 {
            r.line(project(mesh.vertices[i]), project(mesh.vertices[i + 1]), [235, 205, 120, 255]);
        }
    }
    if mesh.indices.is_empty() && curves.is_empty() {
        let cd = mesh.point_attributes.get("Cd").filter(|c| c.len() == mesh.vertices.len() * 3);
        for (i, p) in mesh.vertices.iter().enumerate() {
            let s = project(*p);
            if s.z < near {
                continue;
            }
            let rgb = cd.map_or([0.95, 0.75, 0.3], |c| [c[i * 3], c[i * 3 + 1], c[i * 3 + 2]]);
            let d = 2.0;
            let corner = |dx: f32, dy: f32| raster::ScreenVertex { x: s.x + dx, y: s.y + dy, z: s.z };
            r.triangle([corner(-d, -d), corner(d, -d), corner(d, d)], [rgb; 3], None);
            r.triangle([corner(-d, -d), corner(d, d), corner(-d, d)], [rgb; 3], None);
        }
    }
    png(&r, [46, 46, 50])
}

pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ABC[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ABC[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_png_and_encodes_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        let png = render(&crate::node_box(1.0, 1), &View::named("perspective").unwrap(), 160, 120).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let image = image::load_from_memory(&png).unwrap().to_rgb8();
        // The box covers the middle; the corner is background.
        let mid = image.get_pixel(80, 60).0;
        assert!(mid.iter().any(|c| *c > 100), "{mid:?}");
        assert_eq!(image.get_pixel(1, 1).0, [46, 46, 50]);
        assert!(View::named("upside").is_none());
        // Particles show as dots.
        let mut cloud = Mesh::default();
        cloud.vertices = (0..50).map(|i| Vec3::new(i as f32 * 0.1, 0.5, 0.0)).collect();
        assert!(render(&cloud, &View::named("front").unwrap(), 160, 120).is_ok());
        assert!(render(&Mesh::default(), &View::named("front").unwrap(), 160, 120).is_err());
    }
}
