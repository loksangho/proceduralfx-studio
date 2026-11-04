// The 3D viewport and its gizmos.

use crate::*;

impl ProceduralFxStudioApp {
    // What the viewport's lit surfaces depend on besides the camera: the GPU viewport
    // keeps them uploaded while this stays the same.
    pub(crate) fn viewport_scene_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        (self.mesh_revision, self.render_mode as u8, self.ambient_light.to_bits()).hash(&mut h);
        for (position, direction, color, intensity, kind) in self.scene_lights() {
            for v in [position.x, position.y, position.z, direction.x, direction.y, direction.z, color[0], color[1], color[2], intensity] {
                v.to_bits().hash(&mut h);
            }
            kind.hash(&mut h);
        }
        if let Some((file, scale, rotation, shown)) = self.environment_light() {
            (file, shown).hash(&mut h);
            for v in scale.into_iter().chain([rotation]) {
                v.to_bits().hash(&mut h);
            }
        }
        if self.isolate_selection {
            (self.selection_mode as u8, &self.selected_vertices, &self.selected_triangles, &self.selected_edges).hash(&mut h);
        }
        // Toon Shader settings shade the faces without changing the mesh.
        for (id, node) in &self.graph.nodes {
            if let NodeKind::Shader(v) = &node.kind {
                if v.table == shading::Table::Toon {
                    id.hash(&mut h);
                    v.values.iter().for_each(|x| x.to_bits().hash(&mut h));
                }
            }
        }
        h.finish()
    }

    // ---------------- 3D viewport ----------------
    pub(crate) fn viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        // The flipbook captures this rect of the main window (not a popped-out viewport).
        if ui.ctx().viewport_id() == egui::ViewportId::ROOT {
            self.viewport_rect = Some(response.rect);
        }
        // A flipbook shows the scene only: no info text, axes or handles.
        let recording = self.flipbook_recording.is_some();
        painter.rect_filled(response.rect, 0.0, egui::Color32::from_gray(self.viewport_bg));
        // IMG: the viewport shows the composite.
        if self.context == AppContext::Img {
            let _ = ctx;
            self.image_view(ui, &response, &painter.with_clip_rect(response.rect));
            return;
        }

        // Restrict painter rendering strictly to the viewport rectangle
        let painter = painter.with_clip_rect(response.rect);

        let box_select_modifier = ctx.input(|i| i.modifiers.shift);
        // Gizmo handles are hit-tested against where they were drawn last frame.
        if response.drag_started_by(egui::PointerButton::Primary) && !box_select_modifier {
            if let (Some(frame), Some(pointer)) = (self.gizmo_frame.clone(), response.interact_pointer_pos()) {
                if let (Some(part), Some(target)) = (frame.hit(pointer).cloned(), self.gizmo_target()) {
                    if target.node == frame.node {
                        self.gizmo_drag = Some(GizmoDrag { part, frame, start_pointer: pointer, target });
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
                self.cam_zoom = (self.cam_zoom * (1.0 - scroll * 0.002)).clamp(MIN_ZOOM, 200.0);
            }
        }
        self.sync_active_camera_from_viewport();

        let center = response.rect.center() + egui::vec2(self.cam_pan_x, self.cam_pan_y);
        let (sin_x, cos_x) = self.cam_rot_x.sin_cos();
        let (sin_y, cos_y) = self.cam_rot_y.sin_cos();
        let focal = response.rect.height().min(response.rect.width()) * self.camera_focal;
        self.viewport_focal_px = focal;
        let zoom = self.cam_zoom;
        let ortho_scale = focal / zoom.max(MIN_ZOOM);
        let target = self.cam_target;

        // Close in, the near clip comes closer too, or it would cut away what's in view.
        let near_plane = self.camera_near.min(self.cam_zoom * 0.1);
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

            // In front of the near plane: unchanged, corners in their own order (the
            // loop below starts from the second corner, which would misplace corner
            // colours, uvs and polygon edges).
            if vertices.iter().all(|v| plane_dist(0, *v) >= 0.0) {
                return vertices.to_vec();
            }
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
            for (t, shown) in tm.indices.iter().zip(poly::outline_edges(tm)) {
                for k in (0..3).filter(|k| shown[*k]) {
                    line3d(tm.vertices[t[k]], tm.vertices[t[(k + 1) % 3]], ts);
                }
            }
        }

        if let Some(pick) = &self.group_pick {
            let (count, what) = match pick.domain {
                groups::GroupDomain::Points => (self.selected_vertices.len(), "points"),
                groups::GroupDomain::Prims => (self.selected_triangles.len(), "primitives"),
            };
            let name = self.graph.nodes.get(&pick.node).map_or(String::new(), |n| n.title(pick.node));
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
        // Polygon diagonals: hidden in the wireframe, so not pickable as edges either.
        let diagonals: HashSet<(usize, usize)> = mesh.indices.iter().zip(poly::outline_edges(mesh)).flat_map(|(t, shown)| (0..3).filter(move |k| !shown[*k]).map(move |k| (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3])))).collect();
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
                        self.selected_triangles.extend(poly::whole_polygons(&mesh, &selected));
                        self.selected_triangles.sort_unstable();
                        self.selected_triangles.dedup();
                    }
                    SelectionMode::Edge => {
                        let mut selected = Vec::new();
                        let mut seen = HashSet::new();
                        for tri in &mesh.indices {
                            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                                if diagonals.contains(&(a.min(b), a.max(b))) { continue; }
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
                        if diagonals.contains(&(a.min(b), a.max(b))) { continue; }
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
                            // Clicking picks (or unpicks) the whole polygon.
                            let face = poly::whole_polygons(&mesh, &[i]);
                            if self.selected_triangles.contains(&i) {
                                self.selected_triangles.retain(|t| !face.contains(t));
                            } else {
                                self.selected_triangles.extend(face);
                            }
                        } else {
                            clear_selection_on_miss = true;
                        }
                    }
                    SelectionMode::Edge => {
                        let mut best: Option<(f32, f32, (usize, usize))> = None;
                        for tri in &mesh.indices {
                            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                                if diagonals.contains(&(a.min(b), a.max(b))) { continue; }
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
        // GPU: the lit surfaces stay uploaded until the mesh, lights or display mode change,
        // so moving the camera skips building them (gpu_viewport.rs).
        let gpu_key = self.gpu_viewport.is_some().then(|| self.viewport_scene_key());
        let gpu_fresh = gpu_key.is_some_and(|key| self.gpu_viewport.as_ref().is_some_and(|gpu| gpu.has_scene(key)));
        let built = if gpu_fresh { &Mesh::default() } else { mesh };
        // Stored normals (Normal node) shade the surface; otherwise they're computed.
        let corner_normals = sops::corner_normals(built);
        // UV Quick Shade: the texture and each triangle corner's UV.
        let quickshade = sops::quickshade_file(built).and_then(|file| sops::corner_uvs(built).map(|uvs| (quickshade_texture(&file), uvs)));
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
            // Which edges (0-1, 1-2, 2-0) the wireframe draws: polygon outlines, not their diagonals.
            edges: [bool; 3],
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
        let wire: Option<[u8; 4]> = match self.render_mode {
            RenderMode::Wireframe => Some([80, 220, 120, 255]),
            RenderMode::ShadedWireframe => Some([0, 0, 0, 110]),
            _ => None,
        };
        let mut queue: Vec<Poly> = Vec::new();
        let (mut gpu_faces, mut gpu_edges) = (Vec::new(), Vec::new());
        let outlines = poly::outline_edges(built);
        // Toon Shader faces: cel-shaded per pixel on the GPU, with outlines (gpu_viewport.rs).
        let toon_ids = built.point_attributes.get(TOON_ATTR).filter(|v| v.len() == built.indices.len() && !matches!(self.render_mode, RenderMode::Wireframe | RenderMode::Normals));
        let mut toon_slots: HashMap<usize, usize> = HashMap::new();
        let mut toon_table: Vec<gpu_viewport::ToonMaterial> = Vec::new();
        let mut gpu_toon: Vec<gpu_viewport::ToonVertex> = Vec::new();
        // Smooth point normals push the outline out without gaps at hard edges.
        let smooth_normals = if toon_ids.is_some() { built.compute_normals() } else { Vec::new() };
        // See-through faces: the material's opacity, times Alpha when it uses point alpha.
        let opacity = built.point_attributes.get(OPACITY_ATTR).filter(|v| v.len() == built.indices.len() * 2);
        let point_alpha = built.point_attributes.get("Alpha").filter(|a| a.len() == built.vertices.len());
        let prim_alpha = built.point_attributes.get("prim:Alpha").filter(|a| a.len() == built.indices.len());
        let corner_alpha = |tri_index: usize, point: usize| -> f32 {
            let Some(o) = opacity else { return 1.0 };
            let (amount, uses_alpha) = (o[tri_index * 2], o[tri_index * 2 + 1] > 0.5);
            let alpha = if uses_alpha { point_alpha.map(|a| a[point]).or(prim_alpha.map(|a| a[tri_index])).unwrap_or(1.0) } else { 1.0 };
            (amount * alpha).clamp(0.0, 1.0)
        };
        for (tri_index, tri) in built.indices.iter().enumerate() {
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
            // A toon face: its material's slot in the table, its corners' colours and light.
            let toon = toon_ids.map(|ids| ids[tri_index]).filter(|id| *id >= 0.5).and_then(|id| {
                let id = (id - 1.0).round() as usize;
                let look = match &self.graph.nodes.get(&id)?.kind {
                    NodeKind::Shader(v) if v.table == shading::Table::Toon => v.toon_look(),
                    _ => return None,
                };
                let slot = *toon_slots.entry(id).or_insert_with(|| {
                    toon_table.push(look);
                    toon_table.len() - 1
                });
                let corners = [v0, v1, v2];
                let base = |k: usize| corner_color(tri_index, k, tri[k]).map_or([0.8; 3], |c| [c.r(), c.g(), c.b()].map(|v| v as f32 / 255.0));
                let light = |k: usize| {
                    let n = if self.render_mode == RenderMode::FlatShaded { face_normal } else { corner_n[k] };
                    let rgb = illumination(n, corners[k]);
                    ((rgb[0] + rgb[1] + rgb[2]) / 3.0 - ambient).clamp(0.0, 1.0)
                };
                Some((slot.min(gpu_viewport::MAX_TOONS - 1), look, [base(0), base(1), base(2)], [light(0), light(1), light(2)]))
            });
            let colors = match &toon {
                // The CPU viewport bands each corner.
                Some((_, look, base, light)) if gpu_key.is_none() => std::array::from_fn(|k| {
                    let c = look.shade(base[k], light[k]);
                    egui::Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
                }),
                _ => colors,
            };
            if gpu_key.is_some() {
                // World-space corners; the GPU does the camera.
                let corner = |k: usize| [[v0, v1, v2][k].x, [v0, v1, v2][k].y, [v0, v1, v2][k].z];
                let rgba = |c: egui::Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0];
                if let Some((slot, _, base, light)) = &toon {
                    gpu_toon.extend((0..3).map(|k| {
                        let n = smooth_normals.get(tri[k]).copied().unwrap_or(face_normal);
                        gpu_viewport::ToonVertex { position: corner(k), light: light[k], color: [base[k][0], base[k][1], base[k][2], 1.0], normal: [n.x, n.y, n.z], material: *slot as f32 }
                    }));
                } else if self.render_mode != RenderMode::Wireframe {
                    // Wireframe mode shows every edge, so its (transparent) faces don't occlude.
                    gpu_faces.extend((0..3).map(|k| {
                        let mut colour = rgba(colors[k]);
                        colour[3] = corner_alpha(tri_index, tri[k]);
                        gpu_viewport::Vertex::new(corner(k), colour, uv.map(|uv| [uv[k].x, uv[k].y]))
                    }));
                }
                // Toon faces have their outline instead of the wireframe.
                if let Some(c) = wire.filter(|_| toon.is_none()) {
                    let colour = c.map(|v| v as f32 / 255.0);
                    gpu_edges.extend((0..3).filter(|&k| outlines[tri_index][k]).flat_map(|k| [k, (k + 1) % 3]).map(|k| gpu_viewport::Vertex::new(corner(k), colour, None)));
                }
                continue;
            }
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
                    // Near-plane clipped pieces fall back to untextured shading (and show all their edges).
                    uv: if clipped.len() == 3 { uv } else { None },
                    edges: if clipped.len() == 3 { outlines[tri_index] } else { [true; 3] },
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
        // Surfaces and wireframe go through a depth buffer (gpu_viewport.rs, else
        // raster.rs), at the screen's pixel density, then onto the viewport as one image.
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
        // Hair and other curves (see groom.rs) are drawn as lines, not loose points.
        let curve_ranges = groom::curves(mesh);
        let referenced = used_points(mesh, &curve_ranges);
        let has_unreferenced_points = !kinefx::Skeleton::is_skeleton(mesh) && referenced.iter().any(|&is_referenced| !is_referenced);
        let point_count = mesh.vertices.len();
        // Loose points with Alpha (Pyro, smoke, particles) are soft splats. A surface's
        // Alpha is its opacity (Principled "Use point alpha"), so its points aren't.
        let splat_alpha = mesh.point_attributes.get("Alpha").filter(|a| a.len() == point_count && point_count > 0 && has_unreferenced_points);
        if gpu_key.is_some() || !queue.is_empty() {
            // The GPU draws at full pixel density; the CPU is capped to stay interactive.
            let max_pixels: f32 = if self.gpu_viewport.is_some() { 1.6e7 } else { 2.5e6 };
            let size = response.rect.size();
            let mut scale = ctx.pixels_per_point();
            if size.x * size.y * scale * scale > max_pixels {
                scale = (max_pixels / (size.x * size.y)).sqrt();
            }
            let (w, h) = ((size.x * scale).round().max(1.0) as usize, (size.y * scale).round().max(1.0) as usize);
            let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            // The points stay uploaded too, until the mesh or what decides their look changes.
            let points_key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                (self.mesh_revision, self.show_points, particle_display, point_color.to_array()).hash(&mut h);
                // Smoke is lit by the scene's lights, so moving one redraws it.
                format!("{:?}", self.scene_lights()).hash(&mut h);
                h.finish()
            };
            let volume_lights = self.volume_lights();
            // A flipbook or real-time render frame waiting to be rendered, once this frame's
            // scene is uploaded: through the render camera at its size, or the viewport's view.
            let capture = self.flipbook_recording.as_ref().filter(|r| r.waiting).and_then(|r| r.capture.as_ref()).map(|c| c.render);
            let render_camera = capture.flatten().map(|size| (self.resolve_render_camera().0.realtime_camera(size, near_plane, focal), size));
            if let (Some(gpu), Some(key)) = (self.gpu_viewport.as_mut(), gpu_key) {
                if !gpu_fresh {
                    gpu.set_scene(key, &gpu_faces, &gpu_edges, &gpu_toon, &toon_table, quickshade.as_ref().map(|(texture, _)| texture.clone()));
                }
                if !gpu.has_points(points_key) {
                    let at = |i: usize| [mesh.vertices[i].x, mesh.vertices[i].y, mesh.vertices[i].z];
                    let (mut dots, mut splats) = (Vec::new(), Vec::new());
                    // Hair and other curves, piece by piece, as thick as their width attribute
                    // (a plain line where they have none), in their Cd or hair colour.
                    let widths = mesh.point_attributes.get("width").filter(|w| w.len() == point_count);
                    let width = |i: usize| widths.map_or(-1.0, |w| w[i].max(0.0));
                    let curves: Vec<gpu_viewport::Strand> = curve_ranges
                        .iter()
                        .flat_map(|range| {
                            let c = cd_color(range.start).unwrap_or(egui::Color32::from_rgb(200, 165, 115));
                            let color = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0];
                            (range.start..range.end.saturating_sub(1)).map(move |i| gpu_viewport::Strand { a: at(i), width_a: width(i), b: at(i + 1), width_b: width(i + 1), color })
                        })
                        .collect();
                    if let Some(alpha) = splat_alpha {
                        // Soft splats in world units, as opaque at the centre as the CPU's two discs.
                        let pscale = mesh.point_attributes.get("pscale").filter(|v| v.len() == point_count);
                        // Smoke lit by the scene's lights, when it has any.
                        let relit = (!volume_lights.is_empty()).then(|| sims::relight_smoke(mesh, &volume_lights)).flatten();
                        let colors = relit.as_ref().or_else(|| mesh.point_attributes.get("Cd").filter(|v| v.len() == point_count * 3));
                        let emission = mesh.point_attributes.get("emission").filter(|v| v.len() == point_count * 3);
                        let triple = |values: Option<&Vec<f32>>, i: usize, default: f32| values.map_or([default; 3], |v| [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]]);
                        splats.extend((0..point_count).filter(|&i| alpha[i] > 0.004 && !referenced[i]).map(|i| {
                            let a = alpha[i].clamp(0.0, 1.0);
                            let [r, g, b] = triple(colors, i, 0.8).map(|c| c.clamp(0.0, 1.0));
                            gpu_viewport::Point::new(at(i), pscale.map_or(0.05, |p| p[i]), [r, g, b, 1.0 - (1.0 - a * 0.5) * (1.0 - a * 0.6)], triple(emission, i, 0.0))
                        }));
                    } else if self.show_points || particle_display || has_unreferenced_points {
                        let radius = if particle_display || has_unreferenced_points { 3.5 } else { 2.0 };
                        dots.extend((0..point_count).filter(|&i| !(has_unreferenced_points && referenced[i])).map(|i| {
                            let c = point_attribute_color(i);
                            gpu_viewport::Point::new(at(i), radius, [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0], [0.0; 3])
                        }));
                    }
                    gpu.set_points(points_key, &dots, &splats, &curves);
                }
                let camera = gpu_viewport::Camera {
                    target: [target.x, target.y, target.z],
                    rotation: [sin_x, cos_x, sin_y, cos_y],
                    zoom,
                    center: center - rect_min,
                    size,
                    focal,
                    ortho_scale,
                    orthographic: self.orthographic,
                    near: near_plane,
                };
                let id = gpu.draw([w as u32, h as u32], &camera);
                painter.image(id, response.rect, full, egui::Color32::WHITE);
                if capture.is_some() {
                    let frame = match render_camera {
                        // Transparent where empty (premultiplied, as egui images are).
                        Some((render, size)) => {
                            let wants_glow = self.flipbook_recording.as_ref().and_then(|r| r.capture.as_ref()).and_then(|c| c.sheet.as_ref()).is_some_and(|o| o.glow);
                            let (pixels, glow) = if wants_glow { gpu.render_final_glow(size, &render) } else { (gpu.render_final(size, &render), None) };
                            if wants_glow {
                                let glow = spritesheet::encode_glow([size[0] as usize, size[1] as usize], glow.as_deref());
                                if let Some(recording) = self.flipbook_recording.as_mut() {
                                    recording.glows.push(glow);
                                }
                            }
                            egui::ColorImage::new([size[0] as usize, size[1] as usize], pixels.into_iter().map(|[r, g, b, a]| egui::Color32::from_rgba_premultiplied(r, g, b, a)).collect())
                        }
                        // The viewport's view, at most flipbook::MAX_WIDTH wide, over its background.
                        None => {
                            let width = (w as u32).min(flipbook::MAX_WIDTH);
                            let size = [width, ((h as f32 * width as f32 / w as f32).round() as u32).max(1)];
                            let bg = self.viewport_bg as u32;
                            let over = |c: u8, a: u8| (c as u32 + bg * (255 - a as u32) / 255).min(255) as u8;
                            let pixels = gpu.render(size, &camera);
                            egui::ColorImage::new([size[0] as usize, size[1] as usize], pixels.into_iter().map(|[r, g, b, a]| egui::Color32::from_rgb(over(r, a), over(g, a), over(b, a))).collect())
                        }
                    };
                    if let Some(recording) = self.flipbook_recording.as_mut() {
                        recording.frames.push(frame);
                        recording.next += 1;
                        recording.waiting = false;
                    }
                }
            } else {
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
                        .flat_map(|p| [(0, 1), (1, 2), (2, 0)].into_iter().filter(|(a, _)| p.edges[*a]).map(|(a, b)| raster::Line { a: to_pixel(p.pts[a], p.depth[a]), b: to_pixel(p.pts[b], p.depth[b]), rgba: colour }))
                        .collect(),
                    None => Vec::new(),
                };
                raster.draw(&triangles, &lines);
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], raster.color.as_flattened());
                let texture = viewport_texture(ctx, image);
                painter.image(texture.id(), response.rect, full, egui::Color32::WHITE);
            }
        }

        if !curve_ranges.is_empty() && self.gpu_viewport.is_none() {
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
        // KineFX skeletons: a bone from each joint to its parent, and a dot per joint.
        if kinefx::Skeleton::is_skeleton(mesh) {
            let parent = &mesh.point_attributes["parent"];
            let bone = egui::Color32::from_rgb(90, 220, 255);
            for (j, p) in parent.iter().enumerate() {
                if *p >= 0.0 {
                    if let (Some(a), Some(b)) = (projected.get(j).copied().flatten(), projected.get(*p as usize).copied().flatten()) {
                        painter.line_segment([a.0, b.0], egui::Stroke::new(2.5, bone));
                    }
                }
                if let Some(a) = projected.get(j).copied().flatten() {
                    painter.circle_filled(a.0, 3.5, egui::Color32::from_rgb(255, 230, 120));
                }
            }
        }
        // Volume points (Pyro output) carry Alpha and pscale: draw them as soft,
        // back-to-front splats sized in world units instead of opaque dots. (The GPU
        // draws these and the points below itself, depth-tested and uncapped.)
        if let Some(alpha) = splat_alpha.filter(|_| self.gpu_viewport.is_none()) {
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
                .filter(|(i, _)| alpha[*i] > 0.004 && !referenced[*i])
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
            && self.gpu_viewport.is_none()
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
                                    node.title(id),
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
        if !recording {
            painter.text(
                response.rect.left_top() + egui::vec2(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                disp,
                egui::FontId::monospace(12.0),
                egui::Color32::from_gray(210),
            );
        }
        // The gizmo for the selected node, drawn at its pivot with a constant screen size:
        // move/rotate/scale handles, or a domain box with a handle on each face.
        let mut gizmo_frame = None;
        if let Some(target) = self.gizmo_target().filter(|_| self.show_gizmos && !recording) {
            let xform = target.value;
            let pivot = target.point(Vec3::from_arr(xform.t));
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
                // Local point -> screen, and the local length that's `world` long along `dir`.
                let local_screen = |p: Vec3| screen(target.point(p));
                let local_len = |dir: Vec3, world: f32| world / target.dir(dir).length().max(1e-6);
                let centre = Vec3::from_arr(xform.t);
                let axes = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)];
                let colours = [egui::Color32::from_rgb(235, 85, 85), egui::Color32::from_rgb(105, 215, 105), egui::Color32::from_rgb(95, 145, 245)];
                let mut parts = Vec::new();
                match (target.style, self.gizmo_mode) {
                    (GizmoStyle::Box, _) => {
                        let half = Vec3::from_arr(xform.s).scale(0.5);
                        // The box's edges.
                        let corner = |k: usize| centre.add(Vec3::new(
                            if k & 1 == 0 { -half.x } else { half.x },
                            if k & 2 == 0 { -half.y } else { half.y },
                            if k & 4 == 0 { -half.z } else { half.z },
                        ));
                        for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
                            if let (Some(pa), Some(pb)) = (local_screen(corner(a)), local_screen(corner(b))) {
                                painter.line_segment([pa, pb], egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 200, 255)));
                            }
                        }
                        for i in 0..3 {
                            for plus in [true, false] {
                                let sign = if plus { 1.0 } else { -1.0 };
                                let face = centre.add(axes[i].scale(sign * [half.x, half.y, half.z][i]));
                                let out = local_len(axes[i], length * 0.3);
                                if let (Some(c), Some(tip)) = (local_screen(face), local_screen(face.add(axes[i].scale(sign * out)))) {
                                    parts.push(GizmoPart { handle: GizmoHandle::Face(i, plus), points: vec![c], axis_screen: (tip - c) / out, ring_sign: 1.0 });
                                }
                            }
                        }
                    }
                    (GizmoStyle::Xform, GizmoMode::Translate | GizmoMode::Scale) => {
                        for i in 0..3 {
                            // Scale works along the node's own (rotated) axes.
                            let axis = if self.gizmo_mode == GizmoMode::Scale { axes[i].rotate_deg(xform.r).normalize() } else { axes[i] };
                            let l = local_len(axis, length);
                            if let Some(tip) = local_screen(centre.add(axis.scale(l))) {
                                parts.push(GizmoPart { handle: GizmoHandle::Axis(i), points: vec![pivot_screen, tip], axis_screen: (tip - pivot_screen) / l, ring_sign: 1.0 });
                            }
                        }
                    }
                    (GizmoStyle::Xform, GizmoMode::Rotate) => {
                        for i in 0..3 {
                            let (u, v) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
                            let (lu, lv) = (local_len(u, length), local_len(v, length));
                            let points: Vec<egui::Pos2> = (0..=48)
                                .filter_map(|k| {
                                    let a = k as f32 / 48.0 * std::f32::consts::TAU;
                                    local_screen(centre.add(u.scale(a.cos() * lu)).add(v.scale(a.sin() * lv)))
                                })
                                .collect();
                            let (us, vs) = (local_screen(centre.add(u.scale(lu))), local_screen(centre.add(v.scale(lv))));
                            if let (true, Some(us), Some(vs)) = (points.len() > 8, us, vs) {
                                let (a, b) = (us - pivot_screen, vs - pivot_screen);
                                let sign = if a.x * b.y - a.y * b.x >= 0.0 { 1.0 } else { -1.0 };
                                parts.push(GizmoPart { handle: GizmoHandle::Ring(i), points, axis_screen: egui::Vec2::ZERO, ring_sign: sign });
                            }
                        }
                    }
                }
                if target.style == GizmoStyle::Box || self.gizmo_mode != GizmoMode::Rotate {
                    parts.push(GizmoPart { handle: GizmoHandle::Center, points: vec![pivot_screen], axis_screen: egui::Vec2::ZERO, ring_sign: 1.0 });
                }
                let frame = GizmoFrame { node: target.node, pivot: pivot_screen, pixels_per_unit, cam_right, cam_up, handle_px, parts };
                let active = self.gizmo_drag.as_ref().map(|d| d.part.handle);
                let hovered = response.hover_pos().and_then(|p| frame.hit(p)).map(|p| p.handle);
                for part in &frame.parts {
                    let highlight = active == Some(part.handle) || (active.is_none() && hovered == Some(part.handle));
                    let colour = match part.handle {
                        _ if highlight => egui::Color32::from_rgb(255, 225, 70),
                        GizmoHandle::Axis(i) | GizmoHandle::Ring(i) | GizmoHandle::Face(i, _) => colours[i],
                        GizmoHandle::Center => egui::Color32::from_gray(235),
                    };
                    match (part.handle, part.points.as_slice()) {
                        (GizmoHandle::Center, [c]) => {
                            let rect = egui::Rect::from_center_size(*c, egui::vec2(11.0, 11.0));
                            painter.rect_stroke(rect, 2.0, egui::Stroke::new(2.0, colour), egui::StrokeKind::Middle);
                        }
                        (GizmoHandle::Face(..), [c]) => {
                            // A dot with a stub pointing out of the face.
                            let out = part.axis_screen.normalized() * 14.0;
                            painter.line_segment([*c, *c + out], egui::Stroke::new(2.0, colour));
                            painter.circle_filled(*c, if highlight { 6.5 } else { 5.0 }, colour);
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
        // While a frame cooks in the background, the viewport still shows the last one: say
        // so, so a click that's working doesn't look like one that did nothing.
        if self.simulation_rx.is_some() {
            let label = self.graph.cooking_label(&self.view_displays());
            let label = if label.is_empty() { "Cooking".to_owned() } else { label };
            let at = response.rect.left_top() + egui::vec2(12.0, 12.0);
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

    // What the gizmo edits for the selected node: an object's transform, a Transform
    // SOP, or the domain box of a Pyro or FLIP solver.
    pub(crate) fn gizmo_target(&self) -> Option<GizmoTarget> {
        if self.context != AppContext::Obj {
            return None;
        }
        let id = self.selected?;
        let node = self.graph.nodes.get(&id)?;
        let identity = ObjectXform::default();
        // SOPs live in their geometry object's space.
        let space = self.graph.enclosing_geometry(id).map_or(identity, |object| self.graph.xform_at(object, self.time));
        let target = |value: ObjectXform, space: ObjectXform, style: GizmoStyle| Some(GizmoTarget { node: id, value, space, style });
        let boxed = |centre: Vec3, size: Vec3| ObjectXform { t: [centre.x, centre.y, centre.z], r: [0.0; 3], s: [size.x, size.y, size.z] };
        match &node.kind {
            NodeKind::Geometry { .. } => target(node.xform, identity, GizmoStyle::Xform),
            NodeKind::Light { position, rotation, .. } | NodeKind::Camera { position, rotation, .. } => {
                target(ObjectXform { t: *position, r: *rotation, s: [1.0; 3] }, identity, GizmoStyle::Xform)
            }
            NodeKind::Transform { t, r, s } => target(ObjectXform { t: *t, r: *r, s: *s }, space, GizmoStyle::Xform),
            NodeKind::Pyro(p) => target(boxed(Vec3::from_arr(p.center), Vec3::from_arr(p.size)), space, GizmoStyle::Box),
            NodeKind::Flip { domain, .. } if domain[3] > 0.0 => {
                target(boxed(Vec3::new(domain[0], domain[1], domain[2]), Vec3::new(domain[3], domain[4], domain[5])), space, GizmoStyle::Box)
            }
            NodeKind::Flip { .. } => {
                // Automatic domain: the emitter's bounds (cooked once per upstream change).
                let key = self.graph.sim_key(id);
                let cached = self.gizmo_bounds.borrow().filter(|(k, _)| *k == key).map(|(_, b)| b);
                let bounds = cached.unwrap_or_else(|| {
                    let b = self.graph.cook_fresh(self.graph.input_of(id, 0), 0.0).bounds();
                    *self.gizmo_bounds.borrow_mut() = Some((key, b));
                    b
                });
                let (lo, hi) = bounds?;
                target(boxed(lo.add(hi).scale(0.5), hi.sub(lo)), space, GizmoStyle::Box)
            }
            _ => None,
        }
    }

    // Turns the mouse motion since the drag started into new values for the target.
    pub(crate) fn apply_gizmo_drag(&mut self, drag: &GizmoDrag, pointer: egui::Pos2) {
        let total = pointer - drag.start_pointer;
        let frame = &drag.frame;
        let target = &drag.target;
        let start = target.value;
        let mut x = start;
        let screen_move = || {
            let ppu = frame.pixels_per_unit;
            target.local_dir(frame.cam_right.scale(total.x / ppu).sub(frame.cam_up.scale(total.y / ppu)))
        };
        match (target.style, self.gizmo_mode, drag.part.handle) {
            (GizmoStyle::Box, _, GizmoHandle::Face(i, plus)) => {
                let a = drag.part.axis_screen;
                let grow = total.dot(a) / a.length_sq().max(1e-6);
                let size = (start.s[i] + grow).max(0.01);
                x.s[i] = size;
                x.t[i] = start.t[i] + if plus { 0.5 } else { -0.5 } * (size - start.s[i]);
            }
            (GizmoStyle::Box, _, GizmoHandle::Center) | (GizmoStyle::Xform, GizmoMode::Translate, GizmoHandle::Center) => {
                let d = screen_move();
                x.t = [start.t[0] + d.x, start.t[1] + d.y, start.t[2] + d.z];
            }
            (GizmoStyle::Xform, GizmoMode::Translate, GizmoHandle::Axis(i)) => {
                let a = drag.part.axis_screen;
                x.t[i] += total.dot(a) / a.length_sq().max(1e-6);
            }
            (GizmoStyle::Xform, GizmoMode::Rotate, GizmoHandle::Ring(i)) => {
                let angle = |p: egui::Pos2| (p.y - frame.pivot.y).atan2(p.x - frame.pivot.x);
                let mut delta = angle(pointer) - angle(drag.start_pointer);
                if delta > std::f32::consts::PI {
                    delta -= std::f32::consts::TAU;
                } else if delta < -std::f32::consts::PI {
                    delta += std::f32::consts::TAU;
                }
                x.r[i] += delta.to_degrees() * drag.part.ring_sign;
            }
            (GizmoStyle::Xform, GizmoMode::Scale, GizmoHandle::Axis(i)) => {
                let dir = drag.part.axis_screen.normalized();
                x.s[i] = (start.s[i] * (1.0 + total.dot(dir) / frame.handle_px)).max(0.001);
            }
            (GizmoStyle::Xform, GizmoMode::Scale, GizmoHandle::Center) => {
                let factor = (1.0 + (total.x - total.y) / frame.handle_px).max(0.001);
                x.s = start.s.map(|s| (s * factor).max(0.001));
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
            NodeKind::Transform { t, r, s } => {
                *t = x.t;
                *r = x.r;
                *s = x.s;
            }
            NodeKind::Pyro(p) => {
                p.center = x.t;
                p.size = x.s;
            }
            NodeKind::Flip { domain, .. } => *domain = [x.t[0], x.t[1], x.t[2], x.s[0], x.s[1], x.s[2]],
            _ => return,
        }
        self.dirty = true;
    }
}
