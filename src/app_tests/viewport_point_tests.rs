use crate::*;

// A sphere of radius 0.001, framed (F), fills the view in either projection.
#[test]
fn tiny_things_can_be_framed() {
    for orthographic in [false, true] {
        let Some(gpu) = gpu_viewport::headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let mut app = ProceduralFxStudioApp::default();
        app.gpu_viewport = Some(gpu);
        app.graph = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.001 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n");
        app.selected = None;
        app.render_camera = RenderCamera::Viewport;
        app.orthographic = orthographic;
        app.rebuild_graph();
        app.frame_all();
        assert!(app.cam_zoom < 0.01, "zoom {}", app.cam_zoom);
        let ctx = egui::Context::default();
        let screen = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0))), ..Default::default() };
        let _ = ctx.run(screen, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.viewport(ui, ctx));
        });
        let camera = app.resolve_render_camera().0.realtime_camera([100, 100], app.camera_near.min(app.cam_zoom * 0.1), 1.0);
        let pixels = app.gpu_viewport.as_ref().unwrap().render([100, 100], &camera);
        let covered = pixels.iter().filter(|p| p[3] > 0).count();
        assert!(covered > 1000, "orthographic {orthographic}: the sphere covers {covered} of 10000 pixels");
    }
}

// Scattered points (no faces at all) show in perspective and orthographic views.
#[test]
fn points_alone_show_in_any_view() {
    for orthographic in [false, true] {
        let Some(gpu) = gpu_viewport::headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let mut app = ProceduralFxStudioApp::default();
        app.gpu_viewport = Some(gpu);
        let mut g = Graph::default();
        let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let grid = g.add(NodeKind::Grid { size: 4.0, res: 20 }, egui::pos2(0.0, 0.0));
        let scatter = g.add(NodeKind::Scatter(sops::ScatterSettings { count: 1000, seed: 1, relax_iterations: 5, volume: false }), egui::pos2(0.0, 80.0));
        for id in [grid, scatter] {
            g.nodes.get_mut(&id).unwrap().parent = Some(geo);
        }
        g.connect(grid, scatter, 0);
        g.set_display(scatter);
        app.graph = g;
        app.current_network = Some(geo);
        app.render_camera = RenderCamera::Viewport;
        (app.cam_target, app.cam_rot_x, app.cam_rot_y, app.cam_zoom, app.cam_pan_x, app.cam_pan_y) = (Vec3::zero(), 0.9, 0.0, 8.0, 0.0, 0.0);
        app.orthographic = orthographic;
        app.dirty = true;
        let ctx = egui::Context::default();
        let screen = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 400.0))), ..Default::default() };
        for _ in 0..2 {
            let _ = ctx.run(screen.clone(), |ctx| {
                if app.dirty {
                    app.rebuild_graph();
                    app.dirty = false;
                }
                egui::CentralPanel::default().show(ctx, |ui| app.viewport(ui, ctx));
            });
        }
        assert!(app.current_mesh.indices.is_empty() && app.current_mesh.vertices.len() == 1000);
        let camera = app.resolve_render_camera().0.realtime_camera([200, 200], 0.05, 1.0);
        let pixels = app.gpu_viewport.as_ref().unwrap().render([200, 200], &camera);
        let points = pixels.iter().filter(|p| p[3] > 0 && p[0] > 150 && p[2] < 100).count();
        assert!(points > 2000, "orthographic {orthographic}: {points} point pixels");
    }
}
