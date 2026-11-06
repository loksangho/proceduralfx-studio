use crate::*;

// Every text the toolbar draws at a window width: (text, where).
fn toolbar_texts(width: f32) -> Vec<(String, egui::Rect)> {
    fn flatten(shape: egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| flatten(s, out)),
            egui::Shape::Text(t) => {
                let text = t.galley.text().to_owned();
                if !text.trim().is_empty() {
                    out.push((text, t.visual_bounding_rect()));
                }
            }
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    let mut app = ProceduralFxStudioApp::default();
    let mut texts = Vec::new();
    // A few passes: the layout uses widths measured on the pass before.
    for _ in 0..4 {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 700.0))), ..Default::default() };
        let output = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| app.toolbar(ui));
        });
        texts.clear();
        for clipped in output.shapes {
            flatten(clipped.shape, &mut texts);
        }
    }
    texts
}

#[test]
fn every_control_fits_at_any_window_width() {
    let controls = ["OBJ", "MAT", "OUT", "STAGE", "IMG", "TASKS", "Shaded", "Normals", "Points", "Edges", "↶", "↷", "✥ Move", "⇲ Scale", "◆ Key", "Auto Key", "📈 Graph", "▶ Render", "Camera", "⚙"];
    for width in [1800.0, 1280.0, 1000.0, 800.0, 640.0, 480.0, 380.0] {
        let texts = toolbar_texts(width);
        for control in controls {
            assert!(texts.iter().any(|(t, _)| t == control), "{width}: '{control}' is missing ({:?})", texts.iter().map(|t| &t.0).collect::<Vec<_>>());
        }
        // The menus are there, or folded into ☰ when there's no room.
        assert!(texts.iter().any(|(t, _)| t == "File") || texts.iter().any(|(t, _)| t == "☰"), "{width}: no menus");
        for (text, rect) in &texts {
            assert!(rect.min.x >= -0.5 && rect.max.x <= width + 0.5, "{width}: '{text}' is cut off at {rect:?}");
        }
        for (i, (a, ra)) in texts.iter().enumerate() {
            for (b, rb) in &texts[i + 1..] {
                let overlap = ra.intersect(*rb);
                assert!(overlap.width() <= 1.0 || overlap.height() <= 1.0, "{width}: '{a}' and '{b}' overlap");
            }
        }
    }
    // Wide: one toolbar row (the menus row and the toolbar row). Narrow: more rows.
    let rows = |w: f32| {
        let mut ys: Vec<i32> = toolbar_texts(w).iter().map(|(_, r)| r.center().y.round() as i32 / 8).collect();
        ys.sort_unstable();
        ys.dedup();
        ys.len()
    };
    assert!(rows(1800.0) <= 2, "{}", rows(1800.0));
    assert!(rows(480.0) > rows(1800.0));
}
