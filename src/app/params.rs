// The parameters editor.

use crate::*;

mod objects;
mod contexts;
mod materials;
mod code;
mod solvers;
mod timing;
mod characters;
mod landscape;
mod stage;
mod files;
mod routing;
mod geometry;

// What the panel works out about the selected node before drawing its controls.
pub(super) struct ParamsInfo {
    pub(super) geometry_objects: Vec<NodeId>,
    pub(super) picked_prim: Option<String>,
    pub(super) viewport_view: ([f32; 3], [f32; 3]),
    pub(super) pop_display_missing: bool,
    pub(super) solver_display_is_input: bool,
    pub(super) camera_ids: Vec<(NodeId, String)>,
    pub(super) material_ids: Vec<(NodeId, String)>,
    pub(super) vop_code: Option<Result<String, String>>,
    pub(super) vop_parameters: Vec<(String, [f32; 3])>,
    pub(super) switch_inputs: usize,
    pub(super) joint_names: Vec<String>,
    pub(super) fetch_targets: Vec<(NodeId, String)>,
    pub(super) wedge_targets: Vec<(NodeId, String, Vec<(usize, String)>)>,
    pub(super) top_preview: Option<Result<Vec<tops::WorkItem>, String>>,
    pub(super) top_states: Vec<(tops::State, String)>,
    pub(super) merge_sources: Vec<(NodeId, String, Vec<(NodeId, String)>)>,
    pub(super) merge_own_object: Option<NodeId>,
    pub(super) has_reference: bool,
    pub(super) loop_begin_name: Option<String>,
    pub(super) compiled_loop: bool,
    pub(super) top_busy: bool,
}

impl ProceduralFxStudioApp {
    // ---------------- Parameter panel ----------------
    // Text typed into a parameter field: an expression makes the parameter follow it
    // (an expression channel), a plain number replaces an expression. The parameter
    // is the one whose value the typing changed.
    pub(crate) fn apply_typed(&mut self, (id, params, xform): (NodeId, Vec<f32>, ObjectXform), typed: Typed) {
        // Delete Channel: the node's animated parameters showing the field's value.
        if let Typed::Delete(value) = typed {
            let matching: Vec<ChannelId> = self
                .graph
                .node_channels(id)
                .map(|(c, _)| *c)
                .filter(|c| self.graph.channel_value(*c).is_some_and(|v| (v - value).abs() <= 1e-4 * value.abs().max(1.0)))
                .collect();
            if matching.is_empty() {
                self.status = "That parameter has no channel (expression or keys) to delete".into();
            } else {
                for channel in &matching {
                    self.graph.channels.remove(channel);
                }
                self.status = format!("Deleted {} channel{}", matching.len(), if matching.len() == 1 { "" } else { "s" });
                self.dirty = true;
            }
            return;
        }
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let now = node.kind.params();
        let mut targets: Vec<ChannelTarget> = (0..params.len().min(now.len())).filter(|&i| params[i] != now[i]).map(ChannelTarget::Param).collect();
        targets.extend((0..9).filter(|&i| xform_component(&xform, i) != xform_component(&node.xform, i)).map(ChannelTarget::Xform));
        if targets.is_empty() {
            if let Typed::Error(error) = typed {
                self.status = format!("Expression error: {error}");
            }
            return;
        }
        for target in targets {
            let channel_id = ChannelId { node: id, target };
            match &typed {
                Typed::Expression(text) => {
                    self.graph.channels.entry(channel_id).or_default().expression = Some(text.clone());
                    self.status = format!("The parameter follows {text}");
                }
                Typed::Number => {
                    if let Some(channel) = self.graph.channels.get_mut(&channel_id) {
                        channel.expression = None;
                        if channel.is_empty() {
                            self.graph.channels.remove(&channel_id);
                        }
                    }
                }
                Typed::Error(_) | Typed::Delete(_) => {}
            }
        }
        self.dirty = true;
    }

    pub(crate) fn params_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(id) = self.selected else {
            ui.label("Select a node to edit its parameters.");
            return false;
        };
        let is_display = self.graph.is_display(id);
        let geometry_objects = self.graph.geometry_ids();
        let picked_prim = self.stage_selected_prim.clone();
        let viewport_view = self.viewport_eye_target();
        let pop_display_missing = matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::PopNet { .. })) && self.graph.display_of(id).is_none();
        let solver_display_is_input = self
            .graph
            .display_of(id)
            .and_then(|d| self.graph.nodes.get(&d))
            .is_some_and(|n| matches!(n.kind, NodeKind::SolverInput { .. }));
        let camera_ids = self.camera_ids();
        let mut material_ids: Vec<(NodeId, String)> = self
            .graph
            .nodes
            .iter()
            .filter(|(_, n)| n.kind.is_material())
            .filter(|(_, n)| n.parent.is_none())
            .map(|(mid, n)| (*mid, n.title(*mid)))
            .collect();
        material_ids.sort_unstable();
        // An Attribute VOP's generated code and its Parameter VOPs' defaults.
        let vop_code = matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::AttributeVop { .. }) | Some(NodeKind::Pop(pop::PopNode { op: pop::VOP, .. })))
            .then(|| self.graph.vop_code(id));
        let vop_parameters: Vec<(String, [f32; 3])> = self
            .graph
            .children(id)
            .into_iter()
            .filter_map(|child| match &self.graph.nodes.get(&child)?.kind {
                NodeKind::Vop(v) if v.op == vop::PARAMETER => Some((v.name.clone(), v.value)),
                _ => None,
            })
            .collect();
        let switch_inputs = self.graph.links.iter().filter(|l| l.to == id).map(|l| l.slot + 1).max().unwrap_or(0);
        // Joint names of the input skeleton, for KineFX nodes' joint menus.
        let joint_names: Vec<String> = match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::RigPose(_) | NodeKind::TwoBoneIk(_)) => {
                let input = self.graph.cook_fresh(self.graph.input_of(id, 0), self.time);
                kinefx::Skeleton::from_mesh(&input).map(|s| s.names).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        // Group names on the node's input, for the Group field's menu (filled in on request).
        let group_choices = self.group_choices.as_ref().filter(|(for_id, ..)| *for_id == id).map(|(_, p, q)| (p.clone(), q.clone()));
        let focus_rename = std::mem::take(&mut self.focus_rename);
        // TOP nodes: what they can fetch or wedge, their items and the last cook's results.
        let (fetch_targets, wedge_targets, top_preview) = match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Top(_)) => {
                let mut fetch: Vec<(NodeId, String)> = self
                    .graph
                    .nodes
                    .iter()
                    .filter(|(_, n)| matches!(n.kind, NodeKind::MoonRayRender { .. } | NodeKind::FileExport { .. } | NodeKind::Cache { .. }))
                    .map(|(nid, n)| (*nid, n.title(*nid)))
                    .collect();
                fetch.sort_unstable();
                let mut wedge: Vec<(NodeId, String, Vec<(usize, String)>)> = self
                    .graph
                    .nodes
                    .iter()
                    .filter(|(_, n)| !matches!(n.kind, NodeKind::Top(_) | NodeKind::TopNetwork))
                    .filter_map(|(nid, n)| {
                        let params: Vec<(usize, String)> = n.kind.animatable().into_iter().filter_map(|(t, name)| match t {
                            ChannelTarget::Param(i) => Some((i, name)),
                            _ => None,
                        }).collect();
                        (!params.is_empty()).then(|| (*nid, n.title(*nid), params))
                    })
                    .collect();
                wedge.sort_by_key(|w| w.0);
                (fetch, wedge, Some(self.graph.top_items(id)))
            }
            _ => (Vec::new(), Vec::new(), None),
        };
        let top_states = self.top_results.get(&id).cloned().unwrap_or_default();
        // Object Merge: what it can merge, by object: each object's display output and
        // the nodes inside it (its own network too, except what's downstream of the merge).
        let merge_sources: Vec<(NodeId, String, Vec<(NodeId, String)>)> = if matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::ObjectMerge { .. })) {
            let downstream = self.graph.downstream(id);
            let mut objects: Vec<(NodeId, String, Vec<(NodeId, String)>)> = self
                .graph
                .nodes
                .iter()
                .filter(|(_, n)| n.context == AppContext::Obj && matches!(n.kind, NodeKind::Geometry { .. }))
                .map(|(oid, n)| (*oid, n.title(*oid), Vec::new()))
                .collect();
            for (nid, n) in &self.graph.nodes {
                if n.context != AppContext::Obj || matches!(n.kind, NodeKind::Geometry { .. }) || downstream.contains(nid) {
                    continue;
                }
                let Some(object) = self.graph.enclosing_geometry(*nid) else { continue };
                if let Some(entry) = objects.iter_mut().find(|(o, ..)| *o == object) {
                    entry.2.push((*nid, self.graph.path_in_object(*nid)));
                }
            }
            objects.sort_by(|a, b| a.1.cmp(&b.1));
            for (.., nodes) in &mut objects {
                nodes.sort_by(|a, b| a.1.cmp(&b.1));
            }
            objects
        } else {
            Vec::new()
        };
        let merge_own_object = self.graph.enclosing_geometry(id);
        // Match Size: whether input 2 gives the target box.
        let has_reference = self.graph.input_of(id, 1).is_some();
        let loop_begin_name = match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(b), .. }) | NodeKind::CompileEnd { begin: Some(b) }) => self.graph.nodes.get(b).map(|n| n.title(*b)),
            Some(NodeKind::ForEachBegin) | Some(NodeKind::CompileBegin) => self
                .graph
                .nodes
                .iter()
                .find(|(_, n)| matches!(n.kind, NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(b), .. }) | NodeKind::CompileEnd { begin: Some(b) } if b == id))
                .map(|(e, n)| n.title(*e)),
            _ => None,
        };
        let compiled_loop = matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::ForEachEnd(flow::ForEachSettings { begin: Some(b), .. })) if self.graph.in_compile_block(*b));
        let top_busy = self.top_run.is_some();
        let Some(node) = self.graph.nodes.get_mut(&id) else {
            return false;
        };
        let is_object = node.kind.is_object();
        let title = node.title(id);
        ui.heading(match &node.kind {
            NodeKind::Vop(v) => format!("{} · {title}", v.def().label),
            NodeKind::ShaderNode(n) => format!("{} · {title}", n.def().label),
            _ if !node.name.trim().is_empty() => format!("{title} · {}", node.type_label()),
            _ => title,
        });
        let mut c = false;
        // Renaming: empty goes back to the generated name.
        ui.horizontal(|ui| {
            ui.label("Name");
            let field = ui.add(egui::TextEdit::singleline(&mut node.name).hint_text(node.kind.label(id)).desired_width(160.0));
            if focus_rename {
                field.request_focus();
            }
            c |= field.changed();
            if ui.button("ℹ").on_hover_text("Node Info: counts, bounds, attributes and groups (or hold the middle button on the node)").clicked() {
                self.inspect_node = Some(id);
            }
        });
        ui.horizontal(|ui| {
            if is_object {
                let mut active = !node.bypass;
                if ui.checkbox(&mut active, "Active").changed() {
                    node.bypass = !active;
                    c = true;
                }
            } else {
                c |= ui.checkbox(&mut node.bypass, "Bypass").changed();
                ui.label(if is_display { "● display" } else { "" });
            }
        });
        let support = node.kind.group_support();
        if support != GroupSupport::None {
            ui.horizontal(|ui| {
                ui.label(if support == GroupSupport::Pin { "Pin group" } else { "Group" });
                c |= ui
                    .add(egui::TextEdit::singleline(&mut node.group.pattern).hint_text("everything").desired_width(150.0))
                    .on_hover_text("Names (a b), wildcards (side*), numbers (0-10 20-40:2), expressions (@P.y>0), ^name to exclude, !name for everything else")
                    .changed();
                let menu = ui.menu_button("▾", |ui| {
                    match &group_choices {
                        None => {
                            ui.weak("Reading groups…");
                        }
                        Some((points, prims)) if points.is_empty() && prims.is_empty() => {
                            ui.weak("The input has no groups (make one with Group Create)");
                        }
                        Some((points, prims)) => {
                            for (list, label) in [(points, "Point groups"), (prims, "Primitive groups")] {
                                if list.is_empty() {
                                    continue;
                                }
                                ui.strong(label);
                                for name in list {
                                    if ui.button(name).clicked() {
                                        node.group.pattern = name.clone();
                                        c = true;
                                        ui.close();
                                    }
                                }
                            }
                        }
                    }
                });
                if menu.inner.is_some() {
                    self.group_choices_request = Some(id);
                }
                let picking = self.group_pick.as_ref().is_some_and(|p| p.node == id && p.target == PickTarget::NodeGroup);
                if !picking && ui.button("⬚ Select").on_hover_text("Pick the group's points or primitives in the viewport").clicked() {
                    self.group_pick_request = Some((id, PickTarget::NodeGroup));
                }
                egui::ComboBox::from_id_salt("group_type").selected_text(groups::GROUP_TYPES[node.group.kind.min(2) as usize]).width(110.0).show_ui(ui, |ui| {
                    for (value, label) in groups::GROUP_TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut node.group.kind, value as u8, *label).changed();
                    }
                });
            });
            if let Some(pick) = self.group_pick.as_ref().filter(|p| p.node == id && p.target == PickTarget::NodeGroup) {
                let picked = match pick.domain {
                    groups::GroupDomain::Points => self.selected_vertices.len(),
                    groups::GroupDomain::Prims => self.selected_triangles.len(),
                };
                let domain = pick.domain;
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 210, 90), format!("Selecting: {picked}"));
                    for (d, label) in [(groups::GroupDomain::Points, "Points"), (groups::GroupDomain::Prims, "Primitives")] {
                        if ui.selectable_label(domain == d, label).clicked() && domain != d {
                            self.group_pick_domain = Some(d);
                        }
                    }
                    if ui.button("✓ Accept").clicked() {
                        self.group_pick_finish = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        self.group_pick_finish = Some(false);
                    }
                });
            }
        }
        ui.separator();

        // The node is out of the graph while its own controls draw, so they can use the rest
        // of the app freely; it goes back straight after.
        let info = ParamsInfo { geometry_objects, picked_prim, viewport_view, pop_display_missing, solver_display_is_input, camera_ids, material_ids, vop_code, vop_parameters, switch_inputs, joint_names, fetch_targets, wedge_targets, top_preview, top_states, merge_sources, merge_own_object, has_reference, loop_begin_name, compiled_loop, top_busy };
        let Some(mut node) = self.graph.nodes.remove(&id) else {
            return c;
        };
        for family in [Self::objects_params, Self::contexts_params, Self::materials_params, Self::code_params, Self::solvers_params, Self::timing_params, Self::characters_params, Self::landscape_params, Self::stage_params, Self::files_params, Self::routing_params, Self::geometry_params] {
            if let Some(changed) = family(self, ui, id, &mut node, &info) {
                c |= changed;
                break;
            }
        }
        self.graph.nodes.insert(id, node);
        if std::mem::take(&mut self.start_render_request) {
            self.start_moonray_render();
        }

        let targets = self.graph.nodes.get(&id).map(|n| n.kind.animatable()).unwrap_or_default();
        if !targets.is_empty() {
            let animated = self.graph.is_animated(id);
            egui::CollapsingHeader::new(if animated { "Animation ◆" } else { "Animation" })
                .id_salt(("animation", id))
                .default_open(animated)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("◆ Key all").on_hover_text("Key every parameter below at this frame").clicked() {
                            for (target, _) in &targets {
                                self.graph.set_key(ChannelId { node: id, target: *target }, self.time);
                            }
                            c = true;
                        }
                        if animated && ui.button("Remove animation").clicked() {
                            self.graph.channels.retain(|ch, _| ch.node != id);
                            c = true;
                        }
                        if ui.button("📈 Graph Editor").clicked() {
                            self.pane_requests.push(PaneRequest::Show(Pane::GraphEditor, true));
                            self.graph_editor.frame_all();
                        }
                    });
                    egui::Grid::new(("animation_grid", id)).num_columns(4).striped(true).show(ui, |ui| {
                        for (target, name) in &targets {
                            let channel_id = ChannelId { node: id, target: *target };
                            let value = self.graph.channel_value(channel_id).unwrap_or(0.0);
                            let expression = self.graph.channels.get(&channel_id).and_then(|ch| ch.expression.clone());
                            let state = self.graph.channels.get(&channel_id).map(|ch| ch.has_key_at(self.time));
                            let (symbol, hover) = match (&expression, state) {
                                (Some(_), _) => ("=", "Driven by its expression"),
                                (None, Some(true)) => ("◆", "Key at this frame: click to delete it"),
                                (None, Some(false)) => ("◇", "Animated: click to key this frame"),
                                (None, None) => ("○", "Click to animate: keys this frame"),
                            };
                            let colour = match (&expression, state) {
                                (Some(_), _) => egui::Color32::from_rgb(110, 170, 255),
                                (None, Some(true)) => egui::Color32::from_rgb(255, 200, 60),
                                (None, Some(false)) => egui::Color32::from_rgb(120, 200, 120),
                                (None, None) => egui::Color32::GRAY,
                            };
                            if ui.add_enabled(expression.is_none(), egui::Button::new(egui::RichText::new(symbol).color(colour)).small()).on_hover_text(hover).on_disabled_hover_text(hover).clicked() {
                                if state == Some(true) {
                                    if let Some(channel) = self.graph.channels.get_mut(&channel_id) {
                                        channel.remove_key_at(self.time);
                                        if channel.is_empty() {
                                            self.graph.channels.remove(&channel_id);
                                        }
                                    }
                                } else {
                                    self.graph.set_key(channel_id, self.time);
                                }
                                c = true;
                            }
                            ui.label(name);
                            ui.monospace(format!("{value:.3}"));
                            // An expression drives the parameter at every frame, e.g. $F for Scatter's seed.
                            let mut text = expression.clone().unwrap_or_default();
                            let error = expression.as_deref().and_then(|e| vex::eval_expression(e, self.time).err());
                            let mut edit = egui::TextEdit::singleline(&mut text).hint_text("expression").desired_width(110.0).font(egui::TextStyle::Monospace);
                            if error.is_some() {
                                edit = edit.text_color(egui::Color32::from_rgb(240, 110, 110));
                            }
                            let response = ui.add(edit).on_hover_text(error.clone().unwrap_or_else(|| {
                                "Expression (VEX): $F frame, $FF fractional frame, $T seconds, $FPS; e.g. $F, $FF * 0.1, sin($T * 2) * 0.5, rand($F) * 10".into()
                            }));
                            if response.changed() {
                                let trimmed = text.trim();
                                if trimmed.is_empty() {
                                    if let Some(channel) = self.graph.channels.get_mut(&channel_id) {
                                        channel.expression = None;
                                        if channel.is_empty() {
                                            self.graph.channels.remove(&channel_id);
                                        }
                                    }
                                } else {
                                    self.graph.channels.entry(channel_id).or_default().expression = Some(text.clone());
                                }
                                c = true;
                            }
                            ui.end_row();
                        }
                    });
                    ui.small("Edits to animated parameters (panel or gizmo) key the current frame automatically. An expression replaces the keys.");
                });
        }
        if self.graph.nodes.get(&id).is_some_and(|n| matches!(n.kind, NodeKind::Subnet { .. })) {
            c |= self.subnet_ui(ui, id);
        }
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_lop()) {
            // Cook the stage first so its errors below are current.
            self.scene_graph_ui(ui, id);
        }
        if let Some(error) = node_errors(|errors| errors.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {error}"));
        }
        if let Some(note) = node_notes(|notes| notes.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 190, 90), format!("⚠ {note}"));
        }
        if self.graph.nodes.get(&id).is_some_and(|n| n.kind.is_solver()) {
            ui.add_space(4.0);
            if ui.button("Reset simulations").on_hover_text("Forget cached solver steps and frames, freeing their memory, and simulate again from the start").clicked() {
                sims::clear_all();
                self.frame_cache.clear();
                self.frame_cache_order.clear();
                c = true;
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if !is_object && ui.button("Set Display").clicked() {
                self.graph.set_display(id);
                c = true;
            }
            if ui.button("Duplicate").clicked() {
                self.selected = self.graph.duplicate(id).or(Some(id));
                c = true;
            }
        });
        c
    }
}
