// Cameras: the viewport's view, looking through Camera nodes, and the render camera.

use crate::*;

impl ProceduralFxStudioApp {
    // The viewport's view settings, to compare with what a camera last set.
    pub(crate) fn view_state(&self) -> [f32; 11] {
        [self.cam_target.x, self.cam_target.y, self.cam_target.z, self.cam_rot_x, self.cam_rot_y, self.camera_focal, self.camera_near, self.orthographic as u8 as f32, self.cam_pan_x, self.cam_pan_y, self.cam_zoom]
    }

    // Puts the view where camera `id` is. False if it isn't a camera.
    pub(crate) fn view_from_camera(&mut self, id: NodeId) -> bool {
        let Some(NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else {
            return false;
        };
        self.cam_target = Vec3::from_arr(position);
        self.cam_rot_x = rotation[0].to_radians();
        self.cam_rot_y = rotation[1].to_radians();
        self.camera_focal = focal;
        self.camera_near = near;
        self.orthographic = orthographic;
        (self.cam_pan_x, self.cam_pan_y) = (pan[0], pan[1]);
        self.cam_zoom = zoom;
        true
    }

    // "Use as viewport camera": the selected camera takes the view, and moving the view
    // moves it.
    pub(crate) fn use_requested_camera(&mut self) {
        if !std::mem::take(&mut self.use_camera_requested) {
            return;
        }
        if let Some(id) = self.selected.filter(|id| matches!(self.graph.nodes.get(id).map(|node| &node.kind), Some(NodeKind::Camera { .. }))) {
            if self.view_from_camera(id) {
                self.active_camera = Some(id);
                self.looking_through = false;
                self.camera_view = None;
                self.status = format!("Viewport camera {id} activated");
            }
        }
    }

    // Looks through camera `id` without changing it (None: back to the free view, which
    // stays where it is).
    pub(crate) fn look_through(&mut self, id: Option<NodeId>) {
        match id {
            Some(id) if self.view_from_camera(id) => {
                self.active_camera = Some(id);
                self.looking_through = true;
                self.camera_view = Some(self.view_state());
                let name = self.graph.nodes.get(&id).map_or(String::new(), |n| n.title(id));
                self.status = format!("Looking through {name}; moving the view leaves it unchanged");
            }
            _ => {
                if self.looking_through {
                    self.active_camera = None;
                }
                self.looking_through = false;
                self.camera_view = None;
            }
        }
        self.dirty = true;
    }

    pub(crate) fn sync_active_camera_from_viewport(&mut self) {
        if self.looking_through {
            return;
        }
        let Some(id) = self.active_camera else {
            return;
        };
        let Some(node) = self.graph.nodes.get_mut(&id) else {
            self.active_camera = None;
            return;
        };
        let NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } = &mut node.kind else {
            self.active_camera = None;
            return;
        };
        *position = [self.cam_target.x, self.cam_target.y, self.cam_target.z];
        *rotation = [
            self.cam_rot_x.to_degrees(),
            self.cam_rot_y.to_degrees(),
            rotation[2],
        ];
        *focal = self.camera_focal;
        *near = self.camera_near;
        *orthographic = self.orthographic;
        *pan = [self.cam_pan_x, self.cam_pan_y];
        *zoom = self.cam_zoom;
    }

    // The scene's cameras and their names, as the camera menus list them.
    pub(crate) fn camera_ids(&self) -> Vec<(NodeId, String)> {
        let mut cameras: Vec<(NodeId, String)> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. }))
            .map(|(id, n)| (*id, n.title(*id)))
            .collect();
        cameras.sort_unstable();
        cameras
    }

    pub(crate) fn resolve_render_camera(&self) -> (CamRig, String) {
        let node_rig = |id: NodeId| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Camera { position, rotation, focal, orthographic, pan, zoom, .. }) => Some((
                CamRig {
                    target: Vec3::from_arr(*position),
                    rot_x: rotation[0].to_radians(),
                    rot_y: rotation[1].to_radians(),
                    zoom: *zoom,
                    focal: *focal,
                    orthographic: *orthographic,
                    pan: *pan,
                },
                self.graph.nodes[&id].title(id),
            )),
            _ => None,
        };
        let chosen = match self.render_opts.camera {
            RenderCamera::Node(id) => node_rig(id),
            RenderCamera::Auto => self.selected.and_then(node_rig),
            RenderCamera::Viewport => None,
        };
        chosen.unwrap_or_else(|| {
            (
                CamRig {
                    target: self.cam_target,
                    rot_x: self.cam_rot_x,
                    rot_y: self.cam_rot_y,
                    zoom: self.cam_zoom,
                    focal: self.camera_focal,
                    orthographic: self.orthographic,
                    pan: [self.cam_pan_x, self.cam_pan_y],
                },
                "viewport".to_string(),
            )
        })
    }

    // Eye and look-at point of the viewport camera, for Camera LOPs.
    pub(crate) fn viewport_eye_target(&self) -> ([f32; 3], [f32; 3]) {
        let (sx, cx) = self.cam_rot_x.sin_cos();
        let (sy, cy) = self.cam_rot_y.sin_cos();
        let right = Vec3::new(cy, 0.0, -sy);
        let up = Vec3::new(-sx * sy, cx, -sx * cy);
        let forward = Vec3::new(cx * sy, sx, cx * cy);
        let pan_scale = self.cam_zoom / self.viewport_focal_px.max(1.0);
        let target = self.cam_target.sub(right.scale(self.cam_pan_x * pan_scale)).add(up.scale(self.cam_pan_y * pan_scale));
        let eye = target.sub(forward.scale(self.cam_zoom));
        ([eye.x, eye.y, eye.z], [target.x, target.y, target.z])
    }

    pub(crate) fn set_camera_view(&mut self, rot_x: f32, rot_y: f32) {
        self.cam_rot_x = rot_x;
        self.cam_rot_y = rot_y;
        self.cam_pan_x = 0.0;
        self.cam_pan_y = 0.0;
        self.frame_all();
    }

    pub(crate) fn frame_all(&mut self) {
        if let Some((lo, hi)) = self.current_mesh.bounds() {
            self.cam_target = lo.add(hi).scale(0.5);
            self.cam_zoom = (hi.sub(lo).length() * 1.4).clamp(MIN_ZOOM, 200.0);
            self.cam_pan_x = 0.0;
            self.cam_pan_y = 0.0;
        }
    }
}
