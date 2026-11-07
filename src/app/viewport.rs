// The 3D viewport and its gizmos.

use crate::*;

mod curve_draw;
mod gizmo;
mod handles;
mod overlays;
mod picking;
mod view;

pub(crate) use curve_draw::PLANES as CURVE_PLANES;
pub(crate) use view::ViewCamera;

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
        self.curve_drag_start(ctx, &response);
        let paint_node = self
            .selected
            .filter(|id| self.paint_active && matches!(self.graph.nodes.get(id).map(|n| &n.kind), Some(NodeKind::AttributePaint { .. })));
        if response.dragged_by(egui::PointerButton::Primary) && !box_select_modifier && self.gizmo_drag.is_none() && paint_node.is_none() && self.curve_drag.is_none() {
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
        let rect_min = response.rect.min;
        let view = ViewCamera { center, sin_x, cos_x, sin_y, cos_y, focal, zoom, ortho_scale, target, near_plane, rect: response.rect, orthographic: self.orthographic };
        self.draw_environment(&painter, &view);
        let gizmo_clicked = self.view_axes(&painter, &response, recording);
        // Drawing a Curve: its clicks place points rather than pick geometry.
        let gizmo_clicked = if recording { gizmo_clicked } else { self.curve_draw_frame(ctx, &response, &painter, &view) || gizmo_clicked };

        let to_cam = |p: Vec3| view.to_cam(p);
        let project_cam = |c: Vec3| view.project_cam(c);

        // Attribute Paint brush: dabs while dragging, applied to the node after drawing.
        let pending_paint = self.paint_brush(ctx, &response, &painter, &view, paint_node);

        let clip_triangle = |vertices: [Vec3; 3]| view.clip_triangle(vertices);

        self.draw_floor_and_template(&painter, &view);

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
        let (pending_edge_loop, clear_selection_on_miss) = self.pick_components(ctx, &response, &projected, &diagonals, gizmo_clicked);
        // What's drawn: each shown object from its own mesh (so one object's attributes never
        // change another's look), then, inside a network, the other objects around it.
        let main_parts: Vec<&Mesh> = if self.shown_parts.is_empty() { vec![&self.current_mesh] } else { self.shown_parts.iter().collect() };
        let parts: Vec<&Mesh> = main_parts.into_iter().chain(self.context_parts.iter()).collect();
        // Picking, the selection and the CPU's points use the network's own geometry.
        let mesh = &self.current_mesh;
        // GPU: the lit surfaces stay uploaded until the mesh, lights or display mode change,
        // so moving the camera skips building them (gpu_viewport.rs).
        let gpu_key = self.gpu_viewport.is_some().then(|| self.viewport_scene_key());
        let gpu_fresh = gpu_key.is_some_and(|key| self.gpu_viewport.as_ref().is_some_and(|gpu| gpu.has_scene(key)));
        let empty = Mesh::default();
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
        let wire: Option<[u8; 4]> = match self.render_mode {
            RenderMode::Wireframe => Some([80, 220, 120, 255]),
            RenderMode::ShadedWireframe => Some([0, 0, 0, 110]),
            _ => None,
        };
        let mut queue: Vec<Poly> = Vec::new();
        let (mut gpu_faces, mut gpu_edges) = (Vec::new(), Vec::new());
        let mut toon_slots: HashMap<usize, usize> = HashMap::new();
        let mut toon_table: Vec<gpu_viewport::ToonMaterial> = Vec::new();
        let mut gpu_toon: Vec<gpu_viewport::ToonVertex> = Vec::new();
        let mut scene_texture: Option<Arc<raster::Texture>> = None;
        for part in &parts {
            let built: &Mesh = if gpu_fresh { &empty } else { part };
            let mesh: &Mesh = built;
            // Stored normals (Normal node) shade the surface; otherwise they're computed.
            let corner_normals = sops::corner_normals(built);
            // UV Quick Shade: the texture and each triangle corner's UV.
            let quickshade = sops::quickshade_file(built).and_then(|file| sops::corner_uvs(built).map(|uvs| (quickshade_texture(&file), uvs)));
            // One texture shows (the viewport has one): the first object's.
            if scene_texture.is_none() {
                scene_texture = quickshade.as_ref().map(|(texture, _)| texture.clone());
            }
            let quickshade = quickshade.filter(|(texture, _)| scene_texture.as_ref().is_some_and(|t| Arc::ptr_eq(t, texture)));
            let textured = quickshade.is_some() && !matches!(self.render_mode, RenderMode::Wireframe | RenderMode::Normals);
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
                // Cd is linear: shown through the display transform, as Houdini does.
                let [r, g, b] = [r, g, b].map(crate::color::to_display);
                Some(egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8))
            };
            let outlines = poly::outline_edges(built);
            // Toon Shader faces: cel-shaded per pixel on the GPU, with outlines (gpu_viewport.rs).
            let toon_ids = built.point_attributes.get(TOON_ATTR).filter(|v| v.len() == built.indices.len() && !matches!(self.render_mode, RenderMode::Wireframe | RenderMode::Normals));
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
        }
        let cd_color = |i: usize| cd_color_of(mesh, i);
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
                    gpu.set_scene(key, &gpu_faces, &gpu_edges, &gpu_toon, &toon_table, scene_texture.clone());
                }
                if !gpu.has_points(points_key) {
                    let (mut dots, mut splats, mut curves) = (Vec::new(), Vec::new(), Vec::new());
                    // One fog volume is ray-marched: the first object's.
                    let mut volume: Option<gpu_volume::VolumeGrid> = None;
                    for part in &parts {
                        let mesh: &Mesh = part;
                        let point_count = mesh.vertices.len();
                        let at = |i: usize| [mesh.vertices[i].x, mesh.vertices[i].y, mesh.vertices[i].z];
                        let cd_color = |i: usize| cd_color_of(mesh, i);
                        let curve_ranges = groom::curves(mesh);
                        let referenced = used_points(mesh, &curve_ranges);
                        let has_unreferenced_points = !kinefx::Skeleton::is_skeleton(mesh) && referenced.iter().any(|&r| !r);
                        let splat_alpha = mesh.point_attributes.get("Alpha").filter(|a| a.len() == point_count && point_count > 0 && has_unreferenced_points);
                        // Hair and other curves, piece by piece, as thick as their width attribute
                        // (a plain line where they have none), in their Cd or hair colour.
                        let widths = mesh.point_attributes.get("width").filter(|w| w.len() == point_count);
                        let width = |i: usize| widths.map_or(-1.0, |w| w[i].max(0.0));
                        curves.extend(curve_ranges.iter().flat_map(|range| {
                            let c = cd_color(range.start).unwrap_or(egui::Color32::from_rgb(200, 165, 115));
                            let color = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0];
                            (range.start..range.end.saturating_sub(1)).map(move |i| gpu_viewport::Strand { a: at(i), width_a: width(i), b: at(i + 1), width_b: width(i + 1), color })
                        }));
                        if let Some(alpha) = splat_alpha {
                            // Soft splats in world units, as opaque at the centre as the CPU's two discs.
                            let pscale = mesh.point_attributes.get("pscale").filter(|v| v.len() == point_count);
                            // Smoke lit by the scene's lights, when it has any.
                            let relit = (!volume_lights.is_empty()).then(|| sims::relight_smoke(mesh, &volume_lights)).flatten();
                            // Cd (or the relit smoke) is linear: shown through the display transform.
                            let shown: Option<Vec<f32>> = relit.as_ref().or_else(|| mesh.point_attributes.get("Cd").filter(|v| v.len() == point_count * 3)).map(|v| v.iter().map(|c| crate::color::to_display(*c)).collect());
                            let colors = shown.as_ref();
                            let emission = mesh.point_attributes.get("emission").filter(|v| v.len() == point_count * 3);
                            let triple = |values: Option<&Vec<f32>>, i: usize, default: f32| values.map_or([default; 3], |v| [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]]);
                            // A fog volume (Pyro, a fog VDB) whose points are still on its grid is
                            // ray-marched instead (gpu_volume.rs): smooth at any distance.
                            let grid = (volume.is_none() && volume::is_volume(mesh))
                                .then(|| {
                                    let detail = |name: &str| mesh.point_attributes.get(name).filter(|v| !v.is_empty());
                                    let fog = detail("detail:volume_class").is_some_and(|c| c[0] >= 0.5);
                                    let voxel = detail("detail:volume_voxelsize")?[0];
                                    let origin = detail("detail:volume_origin").filter(|o| o.len() >= 3)?;
                                    let voxels: Vec<_> = (0..point_count).filter(|&i| alpha[i] > 0.004 && !referenced[i]).map(|i| (at(i), triple(colors, i, 0.8), alpha[i], triple(emission, i, 0.0))).collect();
                                    fog.then(|| gpu_volume::VolumeGrid::from_points([origin[0], origin[1], origin[2]], voxel, &voxels)).flatten()
                                })
                                .flatten();
                            let marched = grid.is_some();
                            if marched {
                                volume = grid;
                            }
                            splats.extend((0..point_count).filter(|&i| !marched && alpha[i] > 0.004 && !referenced[i]).map(|i| {
                                let a = alpha[i].clamp(0.0, 1.0);
                                let [r, g, b] = triple(colors, i, 0.8).map(|c| c.clamp(0.0, 1.0));
                                gpu_viewport::Point::new(at(i), pscale.map_or(0.05, |p| p[i]), [r, g, b, 1.0 - (1.0 - a * 0.5) * (1.0 - a * 0.6)], triple(emission, i, 0.0))
                            }));
                        } else if self.show_points || particle_display || has_unreferenced_points {
                            let radius = if particle_display || has_unreferenced_points { 3.5 } else { 2.0 };
                            dots.extend((0..point_count).filter(|&i| !(has_unreferenced_points && referenced[i])).map(|i| {
                                let c = cd_color(i).unwrap_or(point_color);
                                gpu_viewport::Point::new(at(i), radius, [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0], [0.0; 3])
                            }));
                        }
                    }
                    gpu.set_volume(volume.as_ref());
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
                let texture = scene_texture.clone();
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
                    painter.add(egui::Shape::line(line, egui::Stroke::new(1.0_f32, colour)));
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
                        painter.line_segment([a.0, b.0], egui::Stroke::new(2.5_f32, bone));
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
            let shown: Option<Vec<f32>> = mesh.point_attributes.get("Cd").filter(|v| v.len() == point_count * 3).map(|v| v.iter().map(|c| crate::color::to_display(*c)).collect());
            let colors = shown.as_ref();
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
                    egui::Stroke::new(2.0_f32, egui::Color32::YELLOW),
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
                        egui::Stroke::new(3.0_f32, egui::Color32::YELLOW),
                    );
                }
            }
        }
        for &(a, b) in &self.selected_edges {
            if let (Some(pa), Some(pb)) = (projected.get(a).copied().flatten(), projected.get(b).copied().flatten()) {
                painter.line_segment(
                    [pa.0, pb.0],
                    egui::Stroke::new(4.0_f32, egui::Color32::from_rgb(255, 150, 40)),
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
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(100, 190, 255)),
                egui::StrokeKind::Inside,
            );
        }

        if self.show_gizmos {
            self.draw_scene_handles(&painter, &view);
        }

        if !recording {
            self.draw_info(&painter, response.rect, view_display);
        }
        self.draw_gizmo(&painter, &response, &view, recording);
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
        self.draw_cooking_notice(ui, ctx, &painter, response.rect);
    }
}

// A point's Cd as the viewport shows it (linear Cd through the display transform; one value:
// a heat colour), if the mesh has point colours.
fn cd_color_of(mesh: &Mesh, index: usize) -> Option<egui::Color32> {
    let values = mesh.point_attributes.get("Cd")?;
    let vertex_count = mesh.vertices.len();
    let [r, g, b] = if values.len() >= vertex_count.saturating_mul(3) {
        [*values.get(index * 3)?, *values.get(index * 3 + 1)?, *values.get(index * 3 + 2)?]
    } else {
        attribute_color(*values.get(index)?)
    };
    let [r, g, b] = [r, g, b].map(crate::color::to_display);
    Some(egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8))
}
