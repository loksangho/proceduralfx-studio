// The viewport's handles for cameras and lights: a frustum or lamp at each one.

use super::ViewCamera;
use crate::*;

impl ProceduralFxStudioApp {
    pub(super) fn draw_scene_handles(&self, painter: &egui::Painter, view: &ViewCamera) {
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
        let unit = view.zoom * 0.07;
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
                    let stroke = egui::Stroke::new(if selected { 2.0 } else { 1.5_f32 }, outline);

                    if *kind != 1 {
                        let dir = Vec3::new(0.0, 0.0, -1.0).rotate_deg(*rotation).normalize();
                        let tip = pos.add(dir.scale(unit * 3.5));
                        view.line3d(painter, pos, tip, egui::Stroke::new(1.0_f32, tint));
                        let hint = if dir.y.abs() > 0.99 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
                        let side = dir.cross(hint).normalize().scale(unit * 0.25);
                        let lift = dir.cross(side).normalize().scale(unit * 0.25);
                        let base = tip.sub(dir.scale(unit * 0.6));
                        for offset in [side, side.scale(-1.0), lift, lift.scale(-1.0)] {
                            view.line3d(painter, tip, base.add(offset), egui::Stroke::new(1.0_f32, tint));
                        }
                    }

                    let cam_pos = view.to_cam(pos);
                    if cam_pos.z > view.near_plane {
                        let p = view.project_cam(cam_pos);
                        if view.rect.contains(p) {
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
                            painter.add(egui::Shape::line(filament.to_vec(), egui::Stroke::new(1.0_f32, tint)));
                            if !node.bypass {
                                for step in 0..8 {
                                    let angle = std::f32::consts::TAU * step as f32 / 8.0;
                                    let ray = egui::vec2(angle.cos(), angle.sin());
                                    if ray.y > 0.6 {
                                        continue;
                                    }
                                    painter.line_segment([glass + ray * 12.0, glass + ray * 16.0], egui::Stroke::new(1.0_f32, tint));
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
                    let pan_scale = *cam_zoom / view.focal.max(1.0);
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
                    let stroke = egui::Stroke::new(if selected { 2.5 } else { 1.5_f32 }, if selected { egui::Color32::WHITE } else { tint });
                    let s = unit;
                    let put = |x: f32, y: f32, z: f32| {
                        eye.add(right.scale(x * s)).add(up.scale(y * s)).add(fwd.scale(z * s))
                    };

                    // Body box behind the lens point
                    let (bx, by, back) = (0.5, 0.4, -1.3);
                    let corners = |z: f32| [put(-bx, -by, z), put(bx, -by, z), put(bx, by, z), put(-bx, by, z)];
                    let (front, rear) = (corners(0.0), corners(back));
                    for i in 0..4 {
                        view.line3d(painter, front[i], front[(i + 1) % 4], stroke);
                        view.line3d(painter, rear[i], rear[(i + 1) % 4], stroke);
                        view.line3d(painter, front[i], rear[i], stroke);
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
                        view.line3d(painter, lens_far[i], lens_far[(i + 1) % 4], stroke);
                        view.line3d(painter, lens_near[i], lens_far[i], stroke);
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
                                view.line3d(painter, ring(k), ring(k + 1), stroke);
                            }
                        }
                    }

                    let cam_pos = view.to_cam(put(0.0, 0.0, back));
                    if cam_pos.z > view.near_plane {
                        let p = view.project_cam(cam_pos);
                        if view.rect.contains(p) {
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
}
