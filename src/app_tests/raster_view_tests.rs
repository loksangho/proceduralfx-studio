use crate::*;

// Renders a cone like the viewport does, for checking by eye:
// `PFX_ROTX=-0.45 PFX_WIRE=1 cargo test -- --ignored cone_renders_with_depth --nocapture`.
#[test]
#[ignore]
fn cone_renders_with_depth() {
    // PFX_MESH=tube draws a capped tube instead, to check polygon outlines.
    let mesh = match std::env::var("PFX_MESH").as_deref() {
        Ok("tube") => node_tube(1.0, 1.0, 2.0, 3, 12, true),
        // The cone of a scene's Material Fracture (20 concrete pieces).
        Ok("fracture") => {
            let g = Graph::deserialize("pfs 2\nnode 8 0 0 0 obj cone 1 2 1 32 1\nnode 10 0 0 0 obj material_fracture 0 20 1 0 0 0 4 9 1 5 0 200\nlink 8 10 0\nnext 11\n");
            g.cook(10, 0.0, &mut HashMap::new())
        }
        // PFX_MESH=scene:<file>: a saved scene's first object, as displayed.
        Ok(m) if m.starts_with("scene:") => {
            let g = Graph::deserialize(&std::fs::read_to_string(&m[6..]).unwrap());
            let object = g.geometry_ids()[0];
            // PFX_TIME: the frame's time in seconds.
            let time: f32 = std::env::var("PFX_TIME").ok().and_then(|t| t.parse().ok()).unwrap_or(0.0);
            g.cook(object, time, &mut HashMap::new())
        }
        _ => node_cone(1.0, 2.0, 1, 32, true),
    };
    let rot_x: f32 = std::env::var("PFX_ROTX").ok().and_then(|v| v.parse().ok()).unwrap_or(0.35);
    // Framed on the geometry.
    let (lo, hi) = mesh.bounds().unwrap();
    let (rot_y, zoom, focal) = (0.6f32, hi.sub(lo).length() * 1.1, 900.0f32);
    let (sin_x, cos_x) = rot_x.sin_cos();
    let (sin_y, cos_y) = rot_y.sin_cos();
    let target = lo.add(hi).scale(0.5);
    let to_cam = |p: Vec3| {
        let v = p.sub(target);
        let rx = v.x * cos_y - v.z * sin_y;
        let rz = v.x * sin_y + v.z * cos_y;
        Vec3::new(rx, v.y * cos_x - rz * sin_x, v.y * sin_x + rz * cos_x + zoom)
    };
    let (w, h) = (640usize, 560usize);
    let mut r = raster::Raster::new(w, h, false);
    // The viewport's shading normals.
    let corners = sops::corner_normals(&mesh);
    let light = Vec3::new(0.4, 0.8, -0.5).normalize();
    let project = |p: Vec3| {
        let c = to_cam(p);
        raster::ScreenVertex { x: w as f32 / 2.0 + c.x / c.z * focal, y: h as f32 / 2.0 - c.y / c.z * focal, z: c.z }
    };
    let wire = std::env::var("PFX_WIRE").is_ok();
    for (t, tri) in mesh.indices.iter().enumerate() {
        // The base cap (all corners on the lowest plane) in red, to see where it shows.
        let cap = tri.iter().all(|&i| (mesh.vertices[i].y - mesh.bounds().unwrap().0.y).abs() < 1e-4);
        let shade = [0, 1, 2].map(|k| {
            let l = 0.15 + 0.8 * corners[t][k].dot(light).max(0.0);
            if cap { [l.max(0.4), 0.1, 0.1] } else { [l; 3] }
        });
        r.triangle(tri.map(|i| project(mesh.vertices[i])), shade, None);
    }
    if wire {
        // Polygon outlines only, as the viewport draws them.
        for (tri, shown) in mesh.indices.iter().zip(poly::outline_edges(&mesh)) {
            for (a, b) in [(0, 1), (1, 2), (2, 0)].into_iter().filter(|(a, _)| shown[*a]) {
                r.line(project(mesh.vertices[tri[a]]), project(mesh.vertices[tri[b]]), [0, 0, 0, 160]);
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
