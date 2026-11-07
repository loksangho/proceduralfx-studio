// The viewport's gizmo: what it edits, drawing it, and dragging it.

use super::ViewCamera;
use crate::*;

impl ProceduralFxStudioApp {
    // The gizmo for the selected node, drawn at its pivot with a constant screen size:
    // move/rotate/scale handles, or a domain box with a handle on each face. Where its
    // handles are is kept for hit-testing next frame.
    pub(super) fn draw_gizmo(&mut self, painter: &egui::Painter, response: &egui::Response, view: &ViewCamera, recording: bool) {
        let mut gizmo_frame = None;
        if let Some(target) = self.gizmo_target().filter(|_| self.show_gizmos && !recording) {
            let xform = target.value;
            let pivot = target.point(Vec3::from_arr(xform.t));
            let cam_right = view.right();
            let cam_up = view.up();
            let pivot_cam = view.to_cam(pivot);
            if pivot_cam.z > view.near_plane {
                let pivot_screen = view.project_cam(pivot_cam);
                let pixels_per_unit = (view.project_cam(view.to_cam(pivot.add(cam_right))) - pivot_screen).length().max(1e-4);
                let handle_px = 85.0;
                let length = handle_px / pixels_per_unit;
                let screen = |p: Vec3| {
                    let c = view.to_cam(p);
                    (c.z > view.near_plane).then(|| view.project_cam(c))
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
                                painter.line_segment([pa, pb], egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(120, 200, 255)));
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
                            painter.rect_stroke(rect, 2.0, egui::Stroke::new(2.0_f32, colour), egui::StrokeKind::Middle);
                        }
                        (GizmoHandle::Face(..), [c]) => {
                            // A dot with a stub pointing out of the face.
                            let out = part.axis_screen.normalized() * 14.0;
                            painter.line_segment([*c, *c + out], egui::Stroke::new(2.0_f32, colour));
                            painter.circle_filled(*c, if highlight { 6.5 } else { 5.0 }, colour);
                        }
                        (GizmoHandle::Axis(_), [a, b]) => {
                            painter.line_segment([*a, *b], egui::Stroke::new(if highlight { 3.5 } else { 2.5_f32 }, colour));
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
                            painter.add(egui::Shape::line(points.to_vec(), egui::Stroke::new(if highlight { 3.0 } else { 2.0_f32 }, colour)));
                        }
                    }
                }
                gizmo_frame = Some(frame);
            }
        }
        self.gizmo_frame = gizmo_frame;
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
            // At the pivot, where it ends up.
            NodeKind::Transform { t, r, s, p } => target(ObjectXform { t: [t[0] + p[0], t[1] + p[1], t[2] + p[2]], r: *r, s: *s }, space, GizmoStyle::Xform),
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
            NodeKind::Transform { t, r, s, p } => {
                *t = [x.t[0] - p[0], x.t[1] - p[1], x.t[2] - p[2]];
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
