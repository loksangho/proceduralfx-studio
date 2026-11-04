// The viewport's overlays: the HDRI background, corner axes, floor grid, template,
// paint brush, info text and the cooking notice.

use super::ViewCamera;
use crate::*;

impl ProceduralFxStudioApp {
    // The HDRI background, when an Environment Light shows it: the environment along each
    // view ray, on a coarse grid.
    pub(super) fn draw_environment(&self, painter: &egui::Painter, view: &ViewCamera) {
        if let Some((file, scale, rotation, true)) = self.environment_light() {
            if let Ok(map) = envlight::load(&file) {
                let tone = |v: f32| ((v / (1.0 + v)).powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8;
                let cell = 16.0;
                let (cols, rows) = ((view.rect.width() / cell).ceil() as usize + 1, (view.rect.height() / cell).ceil() as usize + 1);
                let mut sky = egui::epaint::Mesh::default();
                for j in 0..rows {
                    for i in 0..cols {
                        let pos = egui::pos2(view.rect.min.x + i as f32 * cell, view.rect.min.y + j as f32 * cell);
                        let l = map.radiance(view.view_dir(pos), rotation);
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
    }

    // The axes in the viewport's corner: a click on one side looks along that axis. True
    // if it took the click.
    pub(super) fn view_axes(&mut self, painter: &egui::Painter, response: &egui::Response, recording: bool) -> bool {
        let gizmo_center = response.rect.right_top() + egui::vec2(-54.0, 54.0);
        let clicked = response.clicked()
            && response.interact_pointer_pos().is_some_and(|p| {
                p.distance(gizmo_center) <= 34.0
            });
        if clicked {
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
        if !recording {
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
        }
        clicked
    }

    // The Attribute Paint brush under the pointer, and the node's new values when it dabs.
    pub(super) fn paint_brush(&self, ctx: &egui::Context, response: &egui::Response, painter: &egui::Painter, view: &ViewCamera, paint_node: Option<NodeId>) -> Option<(NodeId, Vec<f32>)> {
        if let (Some(id), Some(hover)) = (paint_node, response.hover_pos()) {
            if let Some(NodeKind::AttributePaint { values, settings, .. }) = self.graph.nodes.get(&id).map(|n| &n.kind) {
                painter.circle_stroke(hover, settings.radius, egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 210, 90)));
                if response.dragged_by(egui::PointerButton::Primary) || response.clicked() {
                    // Paint on the node's own points, placed where the viewport shows them.
                    let mut points = self.graph.cook(id, self.time, &mut HashMap::new());
                    if let Some(object) = self.graph.enclosing_geometry(id) {
                        points = self.graph.xform_at(object, self.time).apply(points);
                    }
                    let facing = paint::facing(&points, |p| view.to_cam(p));
                    let screen: Vec<Option<(f32, f32)>> = points
                        .vertices
                        .iter()
                        .zip(&facing)
                        .map(|(p, front)| {
                            let c = view.to_cam(*p);
                            (*front && c.z > view.near_plane).then(|| view.project_cam(c)).map(|s| (s.x, s.y))
                        })
                        .collect();
                    let mut brush = settings.clone();
                    if ctx.input(|i| i.modifiers.command) {
                        brush.mode = 1;
                    }
                    let mut updated = paint::values_for(values, points.vertices.len(), settings.default);
                    if paint::dab(&mut updated, &points, &screen, (hover.x, hover.y), &brush) {
                        return Some((id, updated));
                    }
                }
            }
        }
        None
    }

    // The floor grid with the world axes, and the template's ghosted wireframe.
    pub(super) fn draw_floor_and_template(&self, painter: &egui::Painter, view: &ViewCamera) {
        // Floor grid + axes
        if self.show_floor {
            let gs = egui::Stroke::new(1.0, egui::Color32::from_gray(62));
            let steps = (self.grid_extent / self.grid_spacing).round() as i32;
            for i in -steps..=steps {
                let s = i as f32 * self.grid_spacing;
                view.line3d(painter, 
                    Vec3::new(s, 0.0, -self.grid_extent),
                    Vec3::new(s, 0.0, self.grid_extent),
                    gs,
                );
                view.line3d(painter, 
                    Vec3::new(-self.grid_extent, 0.0, s),
                    Vec3::new(self.grid_extent, 0.0, s),
                    gs,
                );
            }
            let o = Vec3::zero();
            view.line3d(painter, o, Vec3::new(1.0, 0.0, 0.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 70, 70)));
            view.line3d(painter, o, Vec3::new(0.0, 1.0, 0.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(70, 200, 70)));
            view.line3d(painter, o, Vec3::new(0.0, 0.0, 1.0), egui::Stroke::new(2.0, egui::Color32::from_rgb(70, 120, 230)));
        }

        // Template geometry ghosted wireframe
        if self.template.is_some() && self.template_mesh.indices.len() <= 20_000 {
            let ts = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(230, 80, 200, 140));
            let tm = &self.template_mesh;
            for (t, shown) in tm.indices.iter().zip(poly::outline_edges(tm)) {
                for k in (0..3).filter(|k| shown[*k]) {
                    view.line3d(painter, tm.vertices[t[k]], tm.vertices[t[(k + 1) % 3]], ts);
                }
            }
        }
    }

    // The info in the viewport's corner: the display node, counts, cook time, frame, cache
    // and bounds.
    pub(super) fn draw_info(&self, painter: &egui::Painter, rect: egui::Rect, view_display: Option<NodeId>) {
        let mesh = &self.current_mesh;
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
                "{}  |  {} pts  {} prims  |  cook {:.1} ms  |  t = {:.2}  |  cache {}/{} \n{}{}",
                self.graph.nodes.get(&id).map_or_else(|| format!("?{id}"), |n| n.title(id)),
                mesh.vertices.len(),
                poly::count(&mesh),
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
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            disp,
            egui::FontId::monospace(12.0),
            egui::Color32::from_gray(210),
        );
    }

    pub(super) fn draw_cooking_notice(&self, ui: &mut egui::Ui, ctx: &egui::Context, painter: &egui::Painter, rect: egui::Rect) {
        // While a frame cooks in the background, the viewport still shows the last one: say
        // so, so a click that's working doesn't look like one that did nothing.
        if self.simulation_rx.is_some() {
            let label = self.graph.cooking_label(&self.view_displays());
            let label = if label.is_empty() { "Cooking".to_owned() } else { label };
            let at = rect.left_top() + egui::vec2(12.0, 12.0);
            let text = format!("{label}…   Esc to cancel");
            let font = egui::FontId::proportional(14.0);
            let galley = painter.layout_no_wrap(text, font, egui::Color32::from_gray(235));
            let spinner = 16.0;
            let backing = egui::Rect::from_min_size(at, egui::vec2(galley.size().x + spinner + 26.0, galley.size().y.max(spinner) + 12.0));
            painter.rect_filled(backing, 6.0, egui::Color32::from_black_alpha(170));
            let spin = egui::Rect::from_min_size(at + egui::vec2(8.0, (backing.height() - spinner) * 0.5), egui::vec2(spinner, spinner));
            ui.put(spin, egui::Spinner::new().size(spinner).color(egui::Color32::from_gray(235)));
            painter.galley(at + egui::vec2(spinner + 16.0, 6.0), galley, egui::Color32::from_gray(235));
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
