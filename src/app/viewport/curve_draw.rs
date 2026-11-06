// Placing a Curve node's points in the viewport, while its "Draw in the viewport" is on:
// a click adds a point at the end of the curve, on the construction plane; dragging a
// point moves it (on the plane through it, parallel to the construction plane); a
// right-click on a point removes it. Left-dragging elsewhere still orbits.

use super::ViewCamera;
use crate::*;

pub(crate) const PLANES: [&str; 4] = ["Ground (XZ)", "Front (XY)", "Side (YZ)", "Facing the view"];

// How near (in pixels) the pointer must be to a point to grab it.
const GRAB: f32 = 9.0;

impl ViewCamera {
    // The ray through a point on screen: where it starts and its direction, in world space.
    pub(crate) fn ray(&self, pos: egui::Pos2) -> (Vec3, Vec3) {
        let forward = self.view_dir(self.center);
        if self.orthographic {
            let side = self.right().scale((pos.x - self.center.x) / self.ortho_scale);
            let up = self.up().scale(-(pos.y - self.center.y) / self.ortho_scale);
            (self.target.add(side).add(up).sub(forward.scale(self.zoom.max(1.0) * 100.0)), forward)
        } else {
            (self.target.sub(forward.scale(self.zoom)), self.view_dir(pos))
        }
    }
}

// Where a ray meets the plane through `point` with normal `normal`; None if it runs along it
// or the plane is behind.
pub(crate) fn ray_plane(origin: Vec3, dir: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let along = dir.dot(normal);
    if along.abs() < 1e-6 {
        return None;
    }
    let t = point.sub(origin).dot(normal) / along;
    (t > 0.0).then(|| origin.add(dir.scale(t)))
}

// An object's transform and its inverse, for points kept in the object's own space.
struct Space {
    xform: ObjectXform,
    axes: [Vec3; 3],
}

impl Space {
    fn new(xform: ObjectXform) -> Space {
        let axes = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)].map(|a| a.rotate_deg(xform.r));
        Space { xform, axes }
    }

    fn to_world(&self, p: [f32; 3]) -> Vec3 {
        Vec3::from_arr(p).mul(Vec3::from_arr(self.xform.s)).rotate_deg(self.xform.r).add(Vec3::from_arr(self.xform.t))
    }

    fn to_local(&self, w: Vec3) -> [f32; 3] {
        let v = w.sub(Vec3::from_arr(self.xform.t));
        let s = self.xform.s.map(|c| if c.abs() < 1e-9 { 1.0 } else { c });
        [v.dot(self.axes[0]) / s[0], v.dot(self.axes[1]) / s[1], v.dot(self.axes[2]) / s[2]]
    }
}

// The point of curve `id` drawn nearest `at`, if one is within reach.
fn nearest_handle(handles: &[(NodeId, usize, egui::Pos2)], id: NodeId, at: egui::Pos2) -> Option<usize> {
    handles
        .iter()
        .filter(|(node, _, pos)| *node == id && pos.distance(at) <= GRAB)
        .min_by(|a, b| a.2.distance(at).total_cmp(&b.2.distance(at)))
        .map(|(_, i, _)| *i)
}

impl ProceduralFxStudioApp {
    // The Curve node being drawn, if Draw in the viewport is on for the selected one.
    pub(crate) fn curve_draw_node(&self) -> Option<NodeId> {
        self.selected.filter(|id| self.curve_draw && matches!(self.graph.nodes.get(id).map(|n| &n.kind), Some(NodeKind::Curve(_))))
    }

    // Starts dragging a point when a drag begins on one (as drawn last frame).
    pub(super) fn curve_drag_start(&mut self, ctx: &egui::Context, response: &egui::Response) {
        let Some(id) = self.curve_draw_node() else {
            self.curve_drag = None;
            return;
        };
        if response.drag_started_by(egui::PointerButton::Primary) {
            // Where the button went down: by the time it counts as a drag, the pointer has moved.
            if let Some(at) = ctx.input(|i| i.pointer.press_origin()).or_else(|| response.interact_pointer_pos()) {
                self.curve_drag = nearest_handle(&self.curve_handles, id, at).map(|i| (id, i));
            }
        }
    }


    // The construction plane's normal (world space).
    fn curve_plane_normal(&self, view: &ViewCamera, space: &Space) -> Vec3 {
        match self.curve_plane {
            1 => space.axes[2],
            2 => space.axes[0],
            3 => view.view_dir(view.center),
            _ => space.axes[1],
        }
    }

    // Draws the points and their control polygon, and moves, adds or removes points.
    // Returns whether the viewport click was used (so it doesn't also pick geometry).
    pub(super) fn curve_draw_frame(&mut self, ctx: &egui::Context, response: &egui::Response, painter: &egui::Painter, view: &ViewCamera) -> bool {
        // Where the points were drawn last frame, which is what the pointer is over.
        let handles = std::mem::take(&mut self.curve_handles);
        let Some(id) = self.curve_draw_node() else { return false };
        let space = Space::new(self.graph.enclosing_geometry(id).map_or_else(ObjectXform::default, |o| self.graph.xform_at(o, self.time)));
        let normal = self.curve_plane_normal(view, &space);
        let Some(NodeKind::Curve(set)) = self.graph.nodes.get(&id).map(|n| &n.kind) else { return false };
        let mut points = set.points.clone();
        let mut changed = false;

        // Dragging a point: along the plane through it, parallel to the construction plane.
        if let (Some((node, i)), Some(pointer)) = (self.curve_drag, ctx.input(|inp| inp.pointer.latest_pos())) {
            if node == id && i < points.len() {
                let (origin, dir) = view.ray(pointer);
                if let Some(hit) = ray_plane(origin, dir, space.to_world(points[i]), normal) {
                    let local = space.to_local(hit);
                    if local != points[i] {
                        points[i] = local;
                        changed = true;
                    }
                }
            }
            if !ctx.input(|inp| inp.pointer.primary_down()) {
                self.curve_drag = None;
            }
        }
        let mut used_click = false;
        let pointer = response.interact_pointer_pos();
        // Right-click a point: remove it (a curve keeps two).
        if response.secondary_clicked() {
            if let Some(i) = pointer.and_then(|at| nearest_handle(&handles, id, at)) {
                if points.len() > 2 {
                    points.remove(i);
                    changed = true;
                    self.status = "Curve point removed".into();
                }
                used_click = true;
            }
        }
        // Click: a new point at the end, on the construction plane (through the object's
        // origin, or for Facing the view, through the last point).
        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(at) = pointer {
                if nearest_handle(&handles, id, at).is_none() {
                    let (origin, dir) = view.ray(at);
                    let through = if self.curve_plane == 3 { points.last().map_or(view.target, |p| space.to_world(*p)) } else { space.to_world([0.0; 3]) };
                    if let Some(hit) = ray_plane(origin, dir, through, normal) {
                        points.push(space.to_local(hit));
                        changed = true;
                        self.status = format!("Curve point {} added", points.len() - 1);
                    }
                }
                used_click = true;
            }
        }

        // The control polygon and the points, numbered.
        let line = egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(255, 210, 90, 140));
        let world: Vec<Vec3> = points.iter().map(|p| space.to_world(*p)).collect();
        for w in world.windows(2) {
            view.line3d(painter, w[0], w[1], line);
        }
        let hover = response.hover_pos();
        for (i, w) in world.iter().enumerate() {
            let c = view.to_cam(*w);
            if c.z <= view.near_plane {
                continue;
            }
            let at = view.project_cam(c);
            self.curve_handles.push((id, i, at));
            let active = self.curve_drag == Some((id, i)) || hover.is_some_and(|h| h.distance(at) <= GRAB);
            let fill = if i == 0 { egui::Color32::from_rgb(120, 220, 120) } else { egui::Color32::from_rgb(255, 200, 70) };
            painter.circle(at, if active { 6.0 } else { 4.5 }, fill, egui::Stroke::new(1.0_f32, egui::Color32::BLACK));
            painter.text(at + egui::vec2(7.0, -7.0), egui::Align2::LEFT_BOTTOM, i.to_string(), egui::FontId::proportional(11.0), egui::Color32::from_gray(230));
        }
        if hover.is_some_and(|h| response.rect.contains(h)) {
            let hint = if self.curve_drag.is_some() { "Drag to move the point" } else { "Click: add a point · drag a point: move it · right-click a point: remove it" };
            painter.text(egui::pos2(response.rect.left() + 10.0, response.rect.bottom() - 10.0), egui::Align2::LEFT_BOTTOM, hint, egui::FontId::proportional(12.0), egui::Color32::from_rgb(255, 215, 120));
        }
        if changed {
            if let Some(NodeKind::Curve(set)) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                set.points = points;
            }
            self.dirty = true;
            ctx.request_repaint();
        }
        used_click || self.curve_drag.is_some()
    }
}
