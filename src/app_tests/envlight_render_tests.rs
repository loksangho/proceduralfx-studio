use crate::*;

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
