// The viewport's camera for one frame: world space to camera space to the screen, and
// clipping lines and triangles to what the camera can see.

use crate::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ViewCamera {
    // Where the camera's target lands on screen (the viewport's centre, plus the pan).
    pub(crate) center: egui::Pos2,
    pub(crate) sin_x: f32,
    pub(crate) cos_x: f32,
    pub(crate) sin_y: f32,
    pub(crate) cos_y: f32,
    // Pixels per unit at distance 1 (perspective).
    pub(crate) focal: f32,
    // The camera's distance from its target.
    pub(crate) zoom: f32,
    // Pixels per unit (orthographic).
    pub(crate) ortho_scale: f32,
    pub(crate) target: Vec3,
    // Nothing nearer the camera than this is drawn.
    pub(crate) near_plane: f32,
    pub(crate) rect: egui::Rect,
    pub(crate) orthographic: bool,
}

impl ViewCamera {
    // A world position in camera space: +Z in front of the camera.
    pub(crate) fn to_cam(&self, p: Vec3) -> Vec3 {
        let v = p.sub(self.target);
        let rx = v.x * self.cos_y - v.z * self.sin_y;
        let rz = v.x * self.sin_y + v.z * self.cos_y;
        let ry = v.y * self.cos_x - rz * self.sin_x;
        let rrz = v.y * self.sin_x + rz * self.cos_x;
        Vec3::new(rx, ry, rrz + self.zoom)
    }

    // A camera-space position on screen.
    pub(crate) fn project_cam(&self, c: Vec3) -> egui::Pos2 {
        if self.orthographic {
            egui::pos2(self.center.x + c.x * self.ortho_scale, self.center.y - c.y * self.ortho_scale)
        } else {
            egui::pos2(self.center.x + c.x / c.z * self.focal, self.center.y - c.y / c.z * self.focal)
        }
    }

    // The world direction the camera looks along through a point on screen.
    pub(crate) fn view_dir(&self, pos: egui::Pos2) -> Vec3 {
        let (rx, ry, rrz) = if self.orthographic {
            (0.0, 0.0, 1.0)
        } else {
            ((pos.x - self.center.x) / self.focal, -(pos.y - self.center.y) / self.focal, 1.0)
        };
        // Undo the camera's rotation (the inverse of to_cam).
        let y = ry * self.cos_x + rrz * self.sin_x;
        let rz = -ry * self.sin_x + rrz * self.cos_x;
        Vec3::new(rx * self.cos_y + rz * self.sin_y, y, -rx * self.sin_y + rz * self.cos_y).normalize()
    }

    // The camera's right and up directions in world space.
    pub(crate) fn right(&self) -> Vec3 {
        Vec3::new(self.cos_y, 0.0, -self.sin_y)
    }

    pub(crate) fn up(&self) -> Vec3 {
        Vec3::new(-self.sin_x * self.sin_y, self.cos_x, -self.sin_x * self.cos_y)
    }

    // A camera-space line cut at the near plane; None if it's all behind it. (The sides
    // are clipped after projection, on screen: side planes through the camera would cut
    // wrongly once the view is panned.)
    pub(crate) fn clip_line_near(&self, mut p1: Vec3, mut p2: Vec3) -> Option<(Vec3, Vec3)> {
        let d1 = p1.z - self.near_plane;
        let d2 = p2.z - self.near_plane;
        if d1 < 0.0 && d2 < 0.0 {
            return None;
        }
        if d1 < 0.0 {
            let denom = d1 - d2;
            if denom.abs() < 1e-7 {
                return None;
            }
            let t = d1 / denom;
            p1 = Vec3::new(p1.x + t * (p2.x - p1.x), p1.y + t * (p2.y - p1.y), p1.z + t * (p2.z - p1.z));
        } else if d2 < 0.0 {
            let denom = d2 - d1;
            if denom.abs() < 1e-7 {
                return None;
            }
            let t = d2 / denom;
            p2 = Vec3::new(p2.x + t * (p1.x - p2.x), p2.y + t * (p1.y - p2.y), p2.z + t * (p1.z - p2.z));
        }
        Some((p1, p2))
    }

    // A screen line cut to the viewport (Liang–Barsky); None if it's all outside.
    pub(crate) fn clip_line_screen(&self, p1: egui::Pos2, p2: egui::Pos2) -> Option<(egui::Pos2, egui::Pos2)> {
        let (min, max) = (self.rect.min, self.rect.max);
        let dx = p2.x - p1.x;
        let dy = p2.y - p1.y;
        let p = [-dx, dx, -dy, dy];
        let q = [p1.x - min.x, max.x - p1.x, p1.y - min.y, max.y - p1.y];
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
                    if t > u1 {
                        u1 = t;
                    }
                } else if t < u2 {
                    u2 = t;
                }
            }
        }
        if u1 > u2 {
            return None;
        }
        Some((egui::pos2(p1.x + u1 * dx, p1.y + u1 * dy), egui::pos2(p1.x + u2 * dx, p1.y + u2 * dy)))
    }

    // Draws a world-space line, clipped to the near plane and the viewport.
    pub(crate) fn line3d(&self, painter: &egui::Painter, a: Vec3, b: Vec3, stroke: egui::Stroke) {
        if let Some((c1, c2)) = self.clip_line_near(self.to_cam(a), self.to_cam(b)) {
            if let Some((p1, p2)) = self.clip_line_screen(self.project_cam(c1), self.project_cam(c2)) {
                painter.line_segment([p1, p2], stroke);
            }
        }
    }

    // A camera-space triangle cut at the near plane, as a polygon (empty if it's all
    // behind it). Clipping before projection keeps a triangle that crosses the near plane
    // from turning into long slivers; the viewport's edges clip the projected polygon.
    pub(crate) fn clip_triangle(&self, vertices: [Vec3; 3]) -> Vec<Vec3> {
        let inside = |p: Vec3| p.z - self.near_plane >= 0.0;
        // All in front: unchanged, corners in their own order (the loop below starts from
        // the second corner, which would misplace corner colours, uvs and polygon edges).
        if vertices.iter().all(|v| inside(*v)) {
            return vertices.to_vec();
        }
        let mut clipped = Vec::with_capacity(4);
        for i in 0..3 {
            let a = vertices[i];
            let b = vertices[(i + 1) % 3];
            let da = a.z - self.near_plane;
            let db = b.z - self.near_plane;
            if inside(a) != inside(b) {
                let denom = da - db;
                if denom.abs() > 1e-7 {
                    let t = da / denom;
                    clipped.push(Vec3::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y), a.z + t * (b.z - a.z)));
                }
            }
            if inside(b) {
                clipped.push(b);
            }
        }
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(orthographic: bool) -> ViewCamera {
        let (sin_x, cos_x) = 0.3f32.sin_cos();
        let (sin_y, cos_y) = 0.7f32.sin_cos();
        ViewCamera {
            center: egui::pos2(400.0, 300.0),
            sin_x,
            cos_x,
            sin_y,
            cos_y,
            focal: 500.0,
            zoom: 6.0,
            ortho_scale: 80.0,
            target: Vec3::new(0.5, 1.0, -0.5),
            near_plane: 0.1,
            rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0)),
            orthographic,
        }
    }

    #[test]
    fn the_target_is_straight_ahead_at_the_centre() {
        for ortho in [false, true] {
            let view = camera(ortho);
            let c = view.to_cam(view.target);
            assert!(c.x.abs() < 1e-5 && c.y.abs() < 1e-5 && (c.z - view.zoom).abs() < 1e-5);
            assert!((view.project_cam(c) - view.center).length() < 1e-3);
        }
    }

    #[test]
    fn a_ray_through_a_pixel_passes_through_what_projects_there() {
        let view = camera(false);
        let point = Vec3::new(1.2, -0.4, 0.9);
        let screen = view.project_cam(view.to_cam(point));
        // The camera sits zoom units behind the target, along the view direction at the centre.
        let eye = view.target.sub(view.view_dir(view.center).scale(view.zoom));
        let towards = point.sub(eye).normalize();
        let dir = view.view_dir(screen);
        assert!((dir.x - towards.x).abs() < 1e-4 && (dir.y - towards.y).abs() < 1e-4 && (dir.z - towards.z).abs() < 1e-4);
        assert!((view.right().length() - 1.0).abs() < 1e-5 && (view.up().length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn lines_are_cut_at_the_near_plane_and_the_viewport() {
        let view = camera(false);
        assert!(view.clip_line_near(Vec3::new(0.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -2.0)).is_none(), "all behind");
        let (a, b) = view.clip_line_near(Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, 1.0)).unwrap();
        assert!((a.z - view.near_plane).abs() < 1e-6 && b.z == 1.0);
        let (p, q) = view.clip_line_screen(egui::pos2(-100.0, 300.0), egui::pos2(900.0, 300.0)).unwrap();
        assert_eq!((p.x, q.x), (0.0, 800.0));
        assert!(view.clip_line_screen(egui::pos2(-100.0, -5.0), egui::pos2(900.0, -5.0)).is_none(), "above the viewport");
    }

    #[test]
    fn triangles_in_front_keep_their_corners_and_crossing_ones_are_cut() {
        let view = camera(false);
        let front = [Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 2.0), Vec3::new(0.0, 1.0, 3.0)];
        let kept = view.clip_triangle(front);
        assert!(kept.iter().zip(front).all(|(a, b)| a.x == b.x && a.y == b.y && a.z == b.z), "same corners, same order");
        assert!(view.clip_triangle([Vec3::new(0.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -2.0), Vec3::new(0.0, 1.0, -3.0)]).is_empty());
        // One corner behind: a quad, all on or in front of the near plane.
        let cut = view.clip_triangle([Vec3::new(0.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 2.0), Vec3::new(0.0, 1.0, 2.0)]);
        assert_eq!(cut.len(), 4);
        assert!(cut.iter().all(|p| p.z >= view.near_plane - 1e-6));
    }
}
