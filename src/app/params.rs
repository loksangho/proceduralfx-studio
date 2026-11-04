// The parameters editor.

use crate::*;

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

        match &mut node.kind {
            NodeKind::Geometry { material } => {
                ui.label("Object container. Its SOP network defines the shape rendered in the scene.");
                ui.strong("Transform");
                c |= vec3_ui(ui, "Translate", &mut node.xform.t, 0.05);
                c |= vec3_ui(ui, "Rotate°", &mut node.xform.r, 0.5);
                c |= vec3_ui(ui, "Scale", &mut node.xform.s, 0.02);
                ui.small("Or drag the gizmo in the viewport (W move, E rotate, R scale).");
                let selected_name = material
                    .and_then(|m| material_ids.iter().find(|(mid, _)| *mid == m))
                    .map_or("default (grey)".to_string(), |(_, name)| name.clone());
                ui.horizontal(|ui| {
                    ui.label("Material");
                    egui::ComboBox::from_id_salt("geometry_material")
                        .selected_text(selected_name)
                        .show_ui(ui, |ui| {
                            c |= ui.selectable_value(material, None, "default (grey)").changed();
                            for (mid, name) in &material_ids {
                                c |= ui.selectable_value(material, Some(*mid), name).changed();
                            }
                        });
                });
                ui.small("Create materials in the MAT context.");
                if ui.button("Enter geometry network").clicked() {
                    self.current_network = Some(id);
                    self.context = AppContext::Obj;
                    self.selected = None;
                    self.graph_pan = egui::Vec2::ZERO;
                    c = true;
                }
                ui.small("Double-click the node to enter; press U in the network to go back up.");
            }
            NodeKind::CopNetwork => {
                ui.small("A compositing network: double-click to build it from COP nodes. The viewport shows the display node's image while you're in IMG.");
            }
            NodeKind::TopNetwork => {
                ui.small("A task graph: double-click to build it from TOP nodes, then cook the last node to run its work items.");
            }
            NodeKind::PopNet { substeps } => {
                ui.heading("POP Network");
                if pop_display_missing {
                    ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "⚠ Nothing inside has the display flag, so there are no particles. Double-click to enter it and set the display flag on the last node.");
                }
                c |= pfield(ui, pslider(substeps, 1..=16).text("Substeps")).changed();
                ui.small("A particle simulation built from POP nodes: double-click it to edit. A new one has a POP Source in the POP Solver's third input (post-solve, as in Houdini: particles are born at the source at the end of each step), with Gravity Force after it. Inputs 0-3 are geometry for POP Sources to birth from (input 0 by default), for POP Collide to collide with, and for POP Wrangles to read (point(1, ...) reads input 0).");
                ui.small("The output is points with v, age, life and id, plus what the POPs add (Cd, groups...): ready for Copy to Points, Particle Fluid Surface, Loop Particles or a sprite sheet.");
            }
            NodeKind::Pop(pop_node) => {
                ui.heading(pop_node.def().label);
                ui.small(pop_node.def().help);
                let op = pop_node.op;
                if op == pop::WRANGLE {
                    if let Some(code) = pop_node.texts.get_mut(0) {
                        c |= vex_editor_ui(ui, code, 2, "particles", &mut node.channels, "v@force += set(0, 2, 0);");
                    }
                } else {
                    let labels = pop_node.def().texts;
                    let rule = pop_node.v("rule").round() as usize;
                    for (i, ((label, _), text)) in labels.iter().zip(pop_node.texts.iter_mut()).enumerate() {
                        // The expression only matters for the Expression rule.
                        let is_expression = (op == pop::GROUP && i == 1) || op == pop::KILL;
                        let expression_rule = (op == pop::GROUP && rule == 3) || (op == pop::KILL && rule == 0);
                        if is_expression && !expression_rule {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.label(*label);
                            let edit = egui::TextEdit::singleline(text).desired_width(220.0);
                            c |= ui.add(if is_expression { edit.font(egui::TextStyle::Monospace) } else { edit }).changed();
                        });
                    }
                }
                for (param, slot) in pop_node.param_mut() {
                    if let Some(choices) = pop::choices(op, param.key) {
                        let mut value = (slot[0].round().max(0.0) as usize).min(choices.len() - 1);
                        egui::ComboBox::from_label(param.label).selected_text(choices[value]).show_ui(ui, |ui| {
                            for (i, label) in choices.iter().enumerate() {
                                c |= ui.selectable_value(&mut value, i, *label).changed();
                            }
                        });
                        slot[0] = value as f32;
                        continue;
                    }
                    match param.key {
                        "input" | "seed" => {
                            let mut whole = slot[0].round().max(0.0) as usize;
                            let top = if param.key == "input" { 3 } else { 10_000 };
                            c |= pfield(ui, pslider(&mut whole, 0..=top).text(param.label)).changed();
                            slot[0] = whole as f32;
                        }
                        _ => c |= shader_param_ui(ui, param, slot),
                    }
                }
                if op == pop::VOP {
                    ui.small("Double-click to edit its VOP network. Bind reads the particles' attributes (P, v, force, age, Cd...) and Export writes them back.");
                    match &vop_code {
                        Some(Ok(code)) => {
                            for (name, default) in &vop_parameters {
                                node.channels.entry(name.clone()).or_insert(*default);
                            }
                            c |= channel_params_ui(ui, code, &mut node.channels);
                            egui::CollapsingHeader::new("Generated VEX").id_salt("vop_code").show(ui, |ui| {
                                ui.label(egui::RichText::new(code.as_str()).monospace().small());
                            });
                        }
                        Some(Err(e)) => {
                            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {e}"));
                        }
                        None => {}
                    }
                }
            }
            NodeKind::Cop(cop) => {
                ui.small(cop.def().help);
                let labels = cop.def().texts;
                for (label, text) in labels.iter().zip(cop.texts.iter_mut()) {
                    ui.horizontal(|ui| {
                        ui.label(*label);
                        c |= ui.add(egui::TextEdit::singleline(text).desired_width(200.0)).changed();
                        if ui.button("…").clicked() {
                            let dialog = FileDialog::new().add_filter("Images", &["exr", "hdr", "png", "jpg", "jpeg", "tif", "tiff", "tga", "bmp"]);
                            let chosen = if cop.op == cops::OUTPUT { dialog.set_file_name("comp.$F4.exr").save_file() } else { dialog.pick_file() };
                            if let Some(path) = chosen {
                                *text = path.to_string_lossy().into_owned();
                                c = true;
                            }
                        }
                    });
                }
                let op = cop.op;
                for (param, slot) in cop.param_mut() {
                    match param.key {
                        "mode" => {
                            let mut mode = slot[0].round() as usize;
                            egui::ComboBox::from_label("Mode").selected_text(cops::BLEND_MODES[mode.min(9)]).show_ui(ui, |ui| {
                                for (value, label) in cops::BLEND_MODES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut mode, value, *label).changed();
                                }
                            });
                            slot[0] = mode as f32;
                        }
                        "type" if op == 3 => {
                            let mut kind = slot[0].round() as usize;
                            ui.horizontal(|ui| {
                                for (value, label) in cops::RAMP_TYPES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut kind, value, *label).changed();
                                }
                            });
                            slot[0] = kind as f32;
                        }
                        _ => c |= shader_param_ui(ui, param, slot),
                    }
                }
                if op == cops::OUTPUT {
                    ui.horizontal(|ui| {
                        if ui.button("💾 Write current frame").clicked() {
                            self.cop_write_request = Some((id, false));
                        }
                        if ui.button("Write timeline range").on_hover_text("Put $F or $F4 in the file name for one file per frame").clicked() {
                            self.cop_write_request = Some((id, true));
                        }
                    });
                }
            }
            NodeKind::Top(top) => {
                ui.small(top.def().help);
                let op = top.op;
                // Node pickers: what to fetch, or which node's parameter to wedge.
                if op == tops::FETCH {
                    let current = top.node();
                    let shown = current.and_then(|n| fetch_targets.iter().find(|(t, _)| *t == n)).map_or("(choose)".to_owned(), |(_, name)| name.clone());
                    egui::ComboBox::from_label("Fetch").selected_text(shown).show_ui(ui, |ui| {
                        for (target, name) in &fetch_targets {
                            if ui.selectable_label(current == Some(*target), name).clicked() {
                                top.set("node", *target as f32);
                                c = true;
                            }
                        }
                    });
                    if fetch_targets.is_empty() {
                        ui.small("Add a MoonRay render node (OUT), or a File Export or Cache node (in a geometry network), to fetch.");
                    }
                }
                if op == tops::WEDGE {
                    let current = top.node();
                    let shown = current.and_then(|n| wedge_targets.iter().find(|(t, ..)| *t == n)).map_or("(none: attribute only)".to_owned(), |(_, name, _)| name.clone());
                    egui::ComboBox::from_label("Node").selected_text(shown).height(400.0).show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "(none: attribute only)").clicked() {
                            top.set("node", -1.0);
                            c = true;
                        }
                        for (target, name, _) in &wedge_targets {
                            if ui.selectable_label(current == Some(*target), name).clicked() {
                                top.set("node", *target as f32);
                                c = true;
                            }
                        }
                    });
                    if let Some((_, _, params)) = current.and_then(|n| wedge_targets.iter().find(|(t, ..)| *t == n)) {
                        let mut param = top.v("param") as usize;
                        let shown = params.iter().find(|(i, _)| *i == param).map_or("(choose)".to_owned(), |(_, n)| n.clone());
                        egui::ComboBox::from_label("Parameter").selected_text(shown).height(400.0).show_ui(ui, |ui| {
                            for (i, name) in params {
                                c |= ui.selectable_value(&mut param, *i, name).changed();
                            }
                        });
                        top.set("param", param as f32);
                    }
                }
                let labels = top.def().texts;
                for (label, text) in labels.iter().zip(top.texts.iter_mut()) {
                    ui.label(*label);
                    c |= ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY)).changed();
                }
                if matches!(op, tops::FETCH | tops::COMMAND) {
                    ui.small("Tokens: $F / $F4 frame, @index, @wedgeindex, @<wedge attribute>, {input} (the item's input files).");
                }
                let def = top.def();
                for (i, param) in def.params.iter().enumerate() {
                    if param.key == "node" || param.key == "param" {
                        continue;
                    }
                    let mut v = top.values[i];
                    if let shading::Kind::Float(min, max) = param.kind {
                        if matches!(param.key, "count" | "start" | "end" | "step") && op != tops::WEDGE || param.key == "count" || param.key == "step" {
                            let mut whole = v.round() as i64;
                            c |= pfield(ui, pdrag(&mut whole).range(min as i64..=max as i64).prefix(format!("{}: ", param.label))).changed();
                            v = whole as f32;
                        } else {
                            c |= pfield(ui, pdrag(&mut v).speed(0.01).range(min..=max).prefix(format!("{}: ", param.label))).changed();
                        }
                    }
                    top.values[i] = v;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(!top_busy, egui::Button::new("▶ Cook")).on_hover_text("Runs this node's work items (and its inputs' first)").clicked() {
                        self.top_cook_request = Some(id);
                    }
                    if ui.add_enabled(top_busy, egui::Button::new("■ Cancel")).clicked() {
                        self.top_cancel_request = true;
                    }
                });
                match &top_preview {
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 120, 100), e);
                    }
                    Some(Ok(items)) => {
                        let done = top_states.iter().filter(|s| s.0 == tops::State::Done).count();
                        let failed = top_states.iter().filter(|s| s.0 == tops::State::Failed).count();
                        ui.label(format!("{} work items{}", items.len(), if top_states.is_empty() { String::new() } else { format!(" · {done} done · {failed} failed") }));
                        egui::ScrollArea::vertical().max_height(260.0).id_salt(("top_items", id)).show(ui, |ui| {
                            for (i, item) in items.iter().take(500).enumerate() {
                                let (state, log) = top_states.get(i).cloned().unwrap_or((tops::State::Waiting, String::new()));
                                let colour = top_state_colour(state);
                                let mut line = format!("#{} · frame {}", item.index, item.frame);
                                for (name, value) in &item.attrs {
                                    line += &format!(" · {name} {value}");
                                }
                                ui.horizontal(|ui| {
                                    ui.colored_label(colour, "●");
                                    ui.label(line);
                                })
                                .response
                                .on_hover_text(match &item.job {
                                    tops::Job::Shell(cmd) => format!("{cmd}\n{log}"),
                                    _ if !log.is_empty() => log.clone(),
                                    _ => item.output.clone(),
                                });
                                if !item.output.is_empty() {
                                    ui.small(&item.output);
                                }
                            }
                        });
                    }
                    None => {}
                }
            }
            NodeKind::Shader(values) => {
                ui.small(match values.table {
                    shading::Table::Principled => "Physically based surface (MoonRay DwaBaseMaterial). Assign it from a Geometry node. Opacity, and Use point alpha (the geometry's Alpha), also show in the viewport and sprite sheets.",
                    shading::Table::Toon => "Cel-shaded surface with banded shadows, a toon highlight and outlines, in the viewport and sprite sheets as in MoonRay (DwaToonMaterial; outlines are real-time only).",
                    shading::Table::Hair => "For hair and fur curves from Hair Generate (HairMaterial_v3).",
                    shading::Table::Pyro => "For Pyro and other fog volumes: smoke colour and density, fire brightness and tint.",
                });
                c |= shader_values_ui(ui, values);
            }
            NodeKind::MaterialBuilder => {
                ui.small("Build a material from texture nodes: double-click to enter, then wire Image Texture, Noise, Mix… into the Surface Output's inputs.");
            }
            NodeKind::ShaderNode(node) => {
                ui.small(node.def().help);
                if node.op == shading::SURFACE_OUTPUT {
                    ui.small("Connected inputs replace the values below.");
                    c |= shader_values_ui(ui, &mut node.surface);
                } else {
                    let labels = node.def().texts;
                    for (label, text) in labels.iter().zip(node.texts.iter_mut()) {
                        ui.horizontal(|ui| {
                            ui.label(*label);
                            c |= ui.text_edit_singleline(text).changed();
                            if *label == "File" && ui.button("…").clicked() {
                                if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "exr", "tif", "tiff", "hdr", "tx"]).pick_file() {
                                    *text = path.to_string_lossy().into_owned();
                                    c = true;
                                }
                            }
                        });
                    }
                    for (param, slot) in node.param_mut() {
                        c |= shader_param_ui(ui, param, slot);
                    }
                }
            }
            NodeKind::Material { kind, color, roughness, value } => {
                ui.label("MoonRay material. Assign it from a Geometry node.");
                ui.horizontal(|ui| {
                    for (index, name) in MATERIAL_NAMES.iter().enumerate() {
                        c |= ui.selectable_value(kind, index, *name).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                match *kind {
                    0 => {
                        c |= pfield(ui, pslider(roughness, 0.0..=1.0).text("Roughness")).changed();
                        c |= pfield(ui, pslider(value, 0.0..=1.0).text("Specular")).changed();
                    }
                    1 => {
                        c |= pfield(ui, pslider(roughness, 0.0..=1.0).text("Roughness")).changed();
                    }
                    2 => {
                        c |= pfield(ui, pslider(roughness, 0.0..=1.0).text("Roughness")).changed();
                        c |= pfield(ui, pslider(value, 1.0..=3.0).text("IOR")).changed();
                    }
                    _ => {
                        c |= pfield(ui, pslider(value, 0.0..=50.0).logarithmic(true).text("Intensity")).changed();
                    }
                }
            }
            NodeKind::Add { count, spacing, layout, points, delete_geometry } => {
                ui.small("Adds points (to the input, if one is connected): at the positions below, and/or a line or grid of them.");
                c |= ui.checkbox(delete_geometry, "Delete geometry but keep the points").on_hover_text("Removes the input's polygons, leaving its points").changed();
                ui.separator();
                ui.strong("Points");
                let mut remove = None;
                for (i, p) in points.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, format!("{i}"), 0.01, &mut p[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut p[k]).speed(0.01)).changed();
                        }
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    points.remove(i);
                    c = true;
                }
                if ui.button("➕ Add point").clicked() {
                    let next = points.last().map_or([0.0; 3], |p| [p[0] + 1.0, p[1], p[2]]);
                    points.push(next);
                    c = true;
                }
                ui.separator();
                ui.strong("Pattern");
                c |= pfield(ui, pslider(count, 0..=100_000).logarithmic(true).text("Point count")).on_hover_text("0: no pattern").changed();
                if *count > 0 {
                    c |= pfield(ui, pslider(spacing, 0.01..=10.0).logarithmic(true).text("Spacing")).changed();
                    ui.horizontal(|ui| {
                        ui.label("Layout");
                        c |= ui.selectable_value(layout, 0, "Line").changed();
                        c |= ui.selectable_value(layout, 1, "Vertical").changed();
                        c |= ui.selectable_value(layout, 2, "Grid").changed();
                    });
                }
            }
            NodeKind::Grid { size, res } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Plane { size, res } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Cone { radius, height, rows, cols, caps } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(height, 0.001..=100.0).logarithmic(true).text("Height")).changed();
                c |= pfield(ui, pslider(rows, 1..=32).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Capsule { radius, length, rows, cols } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(length, 0.0..=20.0).text("Length")).changed();
                c |= pfield(ui, pslider(rows, 4..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 8..=128).text("Columns")).changed();
            }
            NodeKind::Box { size, div } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(div, 1..=30).text("Divisions")).changed();
            }
            NodeKind::Sphere { radius, rows, cols } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(rows, 2..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Torus { major, minor, rows, cols } => {
                c |= pfield(ui, pslider(major, 0.001..=100.0).logarithmic(true).text("Major Radius")).changed();
                c |= pfield(ui, pslider(minor, 0.0005..=50.0).logarithmic(true).text("Minor Radius")).changed();
                c |= pfield(ui, pslider(rows, 3..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Transform { t, r, s } => {
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= vec3_ui(ui, "Scale    ", s, 0.01);
            }
            NodeKind::Noise { freq, amp } => {
                c |= pfield(ui, pslider(freq, 0.0..=10.0).text("Frequency")).changed();
                c |= pfield(ui, pslider(amp, 0.0..=2.0).text("Amplitude")).changed();
            }
            NodeKind::Taper { factor } => {
                c |= pfield(ui, pslider(factor, -2.0..=2.0).text("Taper")).changed();
            }
            NodeKind::Twist { angle } => {
                c |= pfield(ui, pslider(angle, -5.0..=5.0).text("Twist / unit")).changed();
            }
            NodeKind::Bend { angle } => {
                c |= pfield(ui, pslider(angle, -3.0..=3.0).text("Bend / unit")).changed();
            }
            NodeKind::Smooth { iterations, strength } => {
                c |= pfield(ui, pslider(iterations, 0..=50).text("Iterations")).changed();
                c |= pfield(ui, pslider(strength, 0.0..=1.0).text("Strength")).changed();
            }
            NodeKind::Bevel { amount, segments } => {
                c |= pfield(ui, pslider(amount, 0.0..=1.0).text("Amount")).changed();
                c |= pfield(ui, pslider(segments, 1..=8).text("Segments")).changed();
            }
            NodeKind::Fuse { tol } => {
                c |= pfield(ui, pslider(tol, 0.0001..=0.5).logarithmic(true).text("Distance"))
                    .changed();
            }
            NodeKind::Mirror { axis } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
            }
            NodeKind::Copy { count, t, r, s } => {
                c |= pfield(ui, pslider(count, 1..=100).text("Copies")).changed();
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= pfield(ui, pslider(s, 0.5..=1.5).text("Uniform Scale")).changed();
            }
            NodeKind::CopyToPoints { scale, rotation, piece_attribute } => {
                ui.label("Copy the first input onto every point from the second input.");
                c |= pfield(ui, pslider(scale, 0.01..=10.0).logarithmic(true).text("Copy scale")).changed();
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                let mut use_pieces = !piece_attribute.is_empty();
                if ui
                    .checkbox(&mut use_pieces, "Piece attribute")
                    .on_hover_text("Each point copies only the piece of the first input with its value of this attribute")
                    .changed()
                {
                    *piece_attribute = if use_pieces { "variant".into() } else { String::new() };
                    c = true;
                }
                if use_pieces {
                    c |= ui.add(egui::TextEdit::singleline(piece_attribute).desired_width(140.0)).changed();
                    ui.small("Pieces: number them with Enumerate (Primitives, piece attribute name) or Connectivity. Points: Attribute Randomize an integer from 0 to the number of pieces − 1 to choose randomly.");
                }
                ui.small("Use Add or a particle output as the point input.");
            }
            NodeKind::AttributeCreate(a) => {
                ui.label("Creates or replaces an attribute, of any name and type, with one value on every element.");
                let (changed, chosen) = attribute_target_ui(ui, id, &mut a.name, &mut a.class, &mut a.kind, &mut a.size);
                c |= changed;
                if let Some((name, kind, size, value)) = chosen {
                    (a.name, a.kind, a.size, a.value) = (name.into(), kind, size, value);
                }
                ui.horizontal(|ui| {
                    c |= attribute_value_ui(ui, if a.kind == attrib::QUATERNION { "Rotation°" } else { "Value" }, a.kind, a.size, &mut a.value);
                });
                if a.kind == attrib::QUATERNION {
                    ui.small("Stored as a quaternion (x y z w): turns around X, then Y, then Z, like Transform. Copy to Points uses orient to turn copies.");
                }
            }
            NodeKind::AttributeRandomize(r) if r.preset < randomize::CUSTOM => {
                ui.colored_label(egui::Color32::from_rgb(230, 190, 90), format!("This node uses an older preset: {}.", randomize::PRESETS[r.preset]));
                if ui.button("Switch to the full settings").on_hover_text("The same values, with any name, type and range").clicked() {
                    *r = r.converted();
                    c = true;
                }
                c |= pfield(ui, pslider(&mut r.amount, 0.0..=10.0).text("Amount")).changed();
                ui.small(match r.preset {
                    2 => "A random colour per point; Amount shifts the hues.",
                    3 => "A random rotation per point (a quaternion), blended in from none by Amount (0 to 1).",
                    _ => "Adds a random value between -Amount and +Amount to each point.",
                });
                c |= pfield(ui, pdrag(&mut r.seed).prefix("Seed ")).changed();
            }
            NodeKind::AttributeRandomize(r) => {
                ui.label("Random values for an attribute, the same every time for the same seed.");
                let (changed, chosen) = attribute_target_ui(ui, id, &mut r.name, &mut r.class, &mut r.kind, &mut r.size);
                c |= changed;
                if let Some((name, kind, size, value)) = chosen {
                    (r.name, r.kind, r.size) = (name.into(), kind, size);
                    // A useful range to start from.
                    (r.distribution, r.min, r.max) = match (name, kind) {
                        (_, attrib::QUATERNION) => (randomize::ROTATION, [0.0; 4], [0.0, 360.0, 0.0, 0.0]),
                        ("N" | "v" | "up", _) => (randomize::DIRECTION, [1.0; 4], [1.0; 4]),
                        ("pscale", _) => (0, [value[0] * 0.5; 4], [value[0] * 1.5; 4]),
                        ("id", _) => (0, [0.0; 4], [9.0; 4]),
                        _ => (0, [0.0; 4], [1.0; 4]),
                    };
                }
                let choices = randomize::RandomizeSettings::distributions(r.kind);
                if !choices.contains(&r.distribution) {
                    r.distribution = 0;
                    c = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Values");
                    egui::ComboBox::from_id_salt(("randomize_distribution", id)).selected_text(randomize::DISTRIBUTIONS[r.distribution]).show_ui(ui, |ui| {
                        for i in choices {
                            c |= ui.selectable_value(&mut r.distribution, i, randomize::DISTRIBUTIONS[i]).changed();
                        }
                    });
                });
                match r.distribution {
                    randomize::HUE => {
                        c |= pfield(ui, pslider(&mut r.amount, 0.0..=2.0).text("Hue shift")).changed();
                    }
                    randomize::ROTATION => {
                        c |= pfield(ui, pslider(&mut r.amount, 0.0..=1.0).text("Amount (0 none, 1 any rotation)")).changed();
                    }
                    randomize::DIRECTION => {
                        ui.horizontal(|ui| {
                            c |= pfield(ui, pdrag(&mut r.min[0]).speed(0.01).prefix("Length from ")).changed();
                            c |= pfield(ui, pdrag(&mut r.max[0]).speed(0.01).prefix("to ")).changed();
                        });
                    }
                    _ => {
                        let unit = if r.kind == attrib::QUATERNION { "°" } else { "" };
                        ui.horizontal(|ui| {
                            c |= attribute_value_ui(ui, &format!("Min{unit}"), r.kind, r.size, &mut r.min);
                        });
                        ui.horizontal(|ui| {
                            c |= attribute_value_ui(ui, &format!("Max{unit}"), r.kind, r.size, &mut r.max);
                        });
                        ui.small(match (r.kind, r.distribution) {
                            (attrib::QUATERNION, 1) => "A rotation on the way from Min to Max, in degrees around X, then Y, then Z.",
                            (attrib::QUATERNION, _) => "Each angle between its Min and Max, in degrees around X, then Y, then Z: Max 0 360 0 spins each point around Y.",
                            (_, 1) => "One random point on the way from Min to Max, all components together: a colour between two colours.",
                            (attrib::INTEGER, _) => "Whole numbers from Min to Max, both included.",
                            _ => "Each component on its own, between its Min and Max.",
                        });
                    }
                }
                ui.horizontal(|ui| {
                    for (i, name) in randomize::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut r.mode, i, *name).on_hover_text(match i {
                            0 => "Replace what's there",
                            1 => "Add to what's there (for rotations: turn it further)",
                            _ => "Multiply what's there (for rotations: turn it further)",
                        }).changed();
                    }
                });
                c |= pfield(ui, pdrag(&mut r.seed).prefix("Seed ")).changed();
            }
            NodeKind::AttributeDelete(d) => {
                ui.label("Removes attributes by name.");
                ui.horizontal(|ui| {
                    ui.label("Names");
                    c |= ui.add(egui::TextEdit::singleline(&mut d.pattern).desired_width(180.0).hint_text("Cd N fx_*")).changed();
                });
                ui.horizontal(|ui| {
                    for (i, name) in attrib::CLASSES.iter().chain(&["All"]).enumerate() {
                        c |= ui.selectable_value(&mut d.class, i, *name).changed();
                    }
                });
                ui.small("Names separated by spaces; * matches anything, and ^ in front keeps a name: \"* ^P ^N\" removes all but those.");
            }
            NodeKind::AttributeVop { domain } => {
                ui.heading("Attribute VOP");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.small("Double-click the node to edit its network. Bind reads attributes, Export writes them.");
                match &vop_code {
                    Some(Ok(code)) => {
                        // Parameter VOPs start at their default value.
                        for (name, default) in &vop_parameters {
                            node.channels.entry(name.clone()).or_insert(*default);
                        }
                        c |= channel_params_ui(ui, code, &mut node.channels);
                        egui::CollapsingHeader::new("Generated VEX").id_salt("vop_code").show(ui, |ui| {
                            ui.label(egui::RichText::new(code.as_str()).monospace().small());
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {e}"));
                    }
                    None => {}
                }
            }
            NodeKind::Vop(v) => {
                let def = v.def();
                ui.small(def.help);
                match v.op {
                    vop::BIND | vop::EXPORT | vop::PARAMETER | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        ui.horizontal(|ui| {
                            ui.label(if v.op == vop::PARAMETER { "Parameter" } else { "Attribute" });
                            c |= ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(120.0)).changed();
                            if matches!(v.op, vop::BIND | vop::GET_ATTRIB | vop::PC_AVERAGE) {
                                ui.menu_button("▾", |ui| {
                                    for (name, ty) in [("P", 1), ("N", 1), ("Cd", 1), ("v", 1), ("pscale", 0), ("ptnum", 2), ("primnum", 2), ("numpt", 2), ("Time", 0), ("Frame", 0), ("orient", 3)] {
                                        if ui.button(name).clicked() {
                                            v.name = name.into();
                                            v.ty = ty;
                                            c = true;
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        });
                    }
                    vop::SNIPPET => {
                        c |= ui.add(egui::TextEdit::singleline(&mut v.name).code_editor().desired_width(f32::INFINITY).hint_text("$0 * 2 + $1")).changed();
                        ui.small("$0 … $3 are the inputs.");
                    }
                    _ => {}
                }
                match v.op {
                    vop::BIND | vop::CONSTANT | vop::PARAMETER | vop::EXPORT | vop::SNIPPET | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        let choices: &[(&str, char)] = if v.op == vop::PARAMETER { &vop::TYPES[..3] } else { &vop::TYPES };
                        ui.horizontal(|ui| {
                            ui.label("Type");
                            for (i, (name, _)) in choices.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *name).changed();
                            }
                        });
                    }
                    vop::COMPONENT => {
                        ui.horizontal(|ui| {
                            ui.label("Component");
                            for (i, axis) in ["x", "y", "z"].iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *axis).changed();
                            }
                        });
                    }
                    vop::COMPARE => {
                        ui.horizontal(|ui| {
                            ui.label("a");
                            for (i, op) in vop::COMPARES.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *op).changed();
                            }
                            ui.label("b");
                        });
                    }
                    _ => {}
                }
                if matches!(v.op, vop::CONSTANT | vop::PARAMETER) {
                    let label = if v.op == vop::CONSTANT { "Value" } else { "Default" };
                    if v.ty == 1 || v.ty == 3 {
                        c |= vec3_ui(ui, label, &mut v.value, 0.01);
                    } else {
                        c |= pfield(ui, pdrag(&mut v.value[0]).speed(0.01).prefix(format!("{label} "))).changed();
                    }
                }
                if v.reads_geometry() {
                    ui.horizontal(|ui| {
                        ui.label("Geometry");
                        for (i, label) in ["0: being changed", "1", "2", "3"].iter().enumerate() {
                            c |= ui.selectable_value(&mut v.input, i, *label).on_hover_text("Which of the Attribute VOP's inputs it reads").changed();
                        }
                    });
                }
                if !def.inputs.is_empty() {
                    let names: Vec<String> = def.inputs.iter().map(|(n, d)| format!("{n} ({d})")).collect();
                    ui.weak(format!("Inputs, left to right: {}", names.join(", ")));
                }
            }
            NodeKind::Wrangle { code, domain } => {
                ui.heading("VEX Wrangle");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.menu_button("Insert example ▾", |ui| {
                        for (label, example_domain, example) in vex::EXAMPLES {
                            if ui.button(*label).on_hover_text("Replaces the current code").clicked() {
                                *code = example.to_string();
                                *domain = *example_domain;
                                c = true;
                                ui.close_menu();
                            }
                        }
                    });
                    ui.weak("Edits cook live");
                });
                let noun = match *domain {
                    0 => "detail",
                    1 => "primitives",
                    2 => "points",
                    _ => "vertices",
                };
                c |= vex_editor_ui(ui, code, *domain, noun, &mut node.channels, "@P.y += sin(@P.x * 3.0) * 0.2;");
            }
            NodeKind::VolumeWrangle { code } => {
                ui.heading("Volume Wrangle");
                ui.small("Runs over every voxel: @density (fog) or @surface (level set), with @P at the voxel's centre (read-only). volumesample(0, \"density\", pos) reads the volume anywhere; removepoint() switches a voxel off.");
                c |= examples_menu(ui, code, &VOLUME_WRANGLE_EXAMPLES);
                c |= vex_editor_ui(ui, code, 2, "voxels", &mut node.channels, "@density *= 0.5;");
            }
            NodeKind::HeightfieldWrangle { code } => {
                ui.heading("Heightfield Wrangle");
                ui.small("Runs over every point of a terrain: @height and @mask (and the terrain's other attributes), with @P's x and z fixed. volumesample(0, \"height\", pos) and volumeindex() read neighbouring heights.");
                c |= examples_menu(ui, code, &HEIGHTFIELD_WRANGLE_EXAMPLES);
                c |= vex_editor_ui(ui, code, 2, "points", &mut node.channels, "@height += 1;");
            }
            NodeKind::TestGeometry(set) => {
                egui::ComboBox::from_label("Shape").selected_text(testgeo::SHAPES[set.shape.min(testgeo::SHAPES.len() - 1)]).show_ui(ui, |ui| {
                    for (value, label) in testgeo::SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.shape, value, *label).changed();
                    }
                });
                if set.shape <= 2 {
                    ui.small("Keenan Crane's public-domain (CC0) models, quads with UVs.");
                    c |= ui.checkbox(&mut set.colours, "Painted colours (Cd from the model's texture)").changed();
                }
                if set.shape == testgeo::CHARACTER {
                    ui.small("Quaternius's public-domain (CC0) animated human: a skinned mesh with seven takes.");
                    let clips = testgeo::character_clips();
                    let shown = clips.get(set.clip).cloned().unwrap_or_default();
                    egui::ComboBox::from_label("Take").selected_text(shown).show_ui(ui, |ui| {
                        for (value, label) in clips.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.clip, value, label).changed();
                        }
                    });
                    c |= ui.checkbox(&mut set.looping, "Loop (else hold the last pose)").changed();
                    c |= ui.checkbox(&mut set.colours, "Colours").changed();
                    if set.colours {
                        egui::ComboBox::from_label("Look").selected_text(testgeo::LOOKS[set.look.min(testgeo::LOOKS.len() - 1)]).show_ui(ui, |ui| {
                            for (value, label) in testgeo::LOOKS.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.look, value, *label).changed();
                            }
                        });
                    }
                    ui.small("The take plays from frame 1, at the file's 24 frames per second.");
                }
                c |= pfield(ui, pslider(&mut set.size, 0.01..=100.0).logarithmic(true).text("Size")).on_hover_text("The largest side, in scene units").changed();
                c |= ui.checkbox(&mut set.on_ground, "Sit on the ground (else centred on the origin)").changed();
                if set.shape == 7 {
                    c |= pfield(ui, pslider(&mut set.p, 1..=12).text("Windings (p)")).changed();
                    c |= pfield(ui, pslider(&mut set.q, 1..=12).text("Twists (q)")).changed();
                }
                if set.shape >= 7 {
                    c |= pfield(ui, pslider(&mut set.detail, 8..=256).text("Detail")).changed();
                }
            }
            NodeKind::LSystem(set) => {
                ui.small("Rewrites the premise by the rules, generation after generation, then draws it as curves.");
                ui.menu_button("Presets ▾", |ui| {
                    for (label, premise, rules, generations, angle, step) in lsystem::PRESETS {
                        if ui.button(*label).clicked() {
                            set.premise = premise.to_string();
                            set.rules = rules.to_string();
                            set.generations = *generations;
                            set.angle = *angle;
                            set.step = *step;
                            c = true;
                            ui.close_menu();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Premise");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.premise).code_editor().desired_width(220.0)).changed();
                });
                ui.label("Rules (one per line, e.g. A=F[+A][-A])");
                c |= ui.add(egui::TextEdit::multiline(&mut set.rules).code_editor().desired_rows(4).desired_width(f32::INFINITY)).changed();
                c |= pfield(ui, pslider(&mut set.generations, 0..=12).text("Generations")).changed();
                c |= pfield(ui, pslider(&mut set.angle, 0.0..=180.0).text("Angle")).changed();
                c |= pfield(ui, pslider(&mut set.angle_random, 0.0..=45.0).text("Angle Randomness")).changed();
                c |= pfield(ui, pslider(&mut set.step, 0.001..=5.0).logarithmic(true).text("Step Size")).changed();
                c |= pfield(ui, pslider(&mut set.step_scale, 0.0..=1.5).text("Step Scale")).changed();
                c |= pfield(ui, pslider(&mut set.thickness, 0.0..=1.0).text("Thickness")).changed();
                c |= pfield(ui, pslider(&mut set.thickness_scale, 0.0..=1.5).text("Thickness Scale")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                egui::CollapsingHeader::new("Symbols").id_salt("lsystem_symbols").show(ui, |ui| {
                    ui.label(egui::RichText::new(lsystem::SYMBOL_HELP).monospace().small());
                });
            }
            NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                c |= pfield(ui, pslider(r_bot, 0.0..=5.0).text("Bottom Radius")).changed();
                c |= pfield(ui, pslider(r_top, 0.0..=5.0).text("Top Radius")).changed();
                c |= pfield(ui, pslider(height, 0.1..=10.0).text("Height")).changed();
                c |= pfield(ui, pslider(rows, 1..=64).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Peak { dist } => {
                c |= pfield(ui, pslider(dist, -2.0..=2.0).text("Distance")).changed();
            }
            NodeKind::Ripple { freq, amp, speed } => {
                c |= pfield(ui, pslider(freq, 0.0..=20.0).text("Frequency")).changed();
                c |= pfield(ui, pslider(amp, 0.0..=2.0).text("Amplitude")).changed();
                c |= pfield(ui, pslider(speed, 0.0..=20.0).text("Speed")).changed();
            }
            NodeKind::Jitter { amp, seed, scale } => {
                ui.small("Moves each point by a random offset of up to Amount, scaled per axis (0 keeps that axis still).");
                c |= pfield(ui, pslider(amp, 0.0..=1.0).text("Amount")).changed();
                ui.horizontal(|ui| {
                    ui.label("Axis scale");
                    for (axis, value) in ["x", "y", "z"].iter().zip(scale.iter_mut()) {
                        c |= pfield(ui, pdrag(value).speed(0.01).range(0.0..=10.0).prefix(format!("{axis} "))).changed();
                    }
                });
                c |= pfield(ui, pslider(seed, 0..=999).text("Seed")).changed();
            }
            NodeKind::Particles {
                count,
                emission_rate,
                lifetime,
                speed,
                spread,
                gravity,
                seed,
                drag,
                bounce,
                friction,
                radius,
                substeps,
                along_normals,
            } => {
                ui.small("Input 1: optional source (emits from its surface, or its points; the origin when unconnected) • Input 2: optional static collider.");
                ui.strong("Emission");
                c |= pfield(ui, pslider(count, 1..=50_000).logarithmic(true).text("Max particles")).changed();
                c |= pfield(ui, pslider(emission_rate, 1.0..=10_000.0).logarithmic(true).text("Birth rate (per second)")).changed();
                c |= pfield(ui, pslider(lifetime, 0.1..=20.0).text("Lifetime")).changed();
                c |= pfield(ui, pslider(speed, 0.0..=20.0).text("Speed")).changed();
                c |= pfield(ui, pslider(spread, 0.0..=2.0).text("Spread")).changed();
                c |= ui
                    .checkbox(along_normals, "Emit along source normals")
                    .on_hover_text("Off: particles head up (+Y). Points inherit their v attribute either way.")
                    .changed();
                ui.strong("Forces");
                c |= pfield(ui, pslider(gravity, -20.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(drag, 0.0..=5.0).text("Drag")).changed();
                ui.strong("Collisions");
                c |= pfield(ui, pslider(bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(friction, 0.0..=1.0).text("Friction")).changed();
                c |= pfield(ui, pslider(radius, 0.0..=0.5).text("Particle radius")).changed();
                ui.strong("Solver");
                c |= pfield(ui, pslider(substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(seed, 0..=9999).text("Seed")).changed();
                ui.small("Outputs points with v, age, life and id attributes.");
            }
            NodeKind::Flip {
                particle_count,
                grid_resolution,
                flip_ratio,
                viscosity,
                gravity,
                max_dt,
                substeps,
                seed,
                domain,
            } => {
                ui.label("3D CPU FLIP solver");
                c |= pfield(ui, pslider(particle_count, 1..=100_000).logarithmic(true).text("Particles")).changed();
                c |= pfield(ui, pslider(grid_resolution, 16..=128).text("Grid resolution")).changed();
                c |= pfield(ui, pslider(flip_ratio, 0.0..=1.0).text("FLIP blend")).changed();
                c |= pfield(ui, pslider(viscosity, 0.0..=4.0).text("Viscosity")).changed();
                c |= pfield(ui, pslider(gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(max_dt, 0.001..=0.1).logarithmic(true).text("Max timestep")).changed();
                c |= pfield(ui, pslider(substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(seed, 0..=9999).text("Seed")).changed();
                let mut auto = domain[3] <= 0.0;
                if ui.checkbox(&mut auto, "Domain from the emitter's bounds").on_hover_text("Or drag the domain box's faces in the viewport").changed() {
                    *domain = if auto { [0.0; 6] } else { [0.0, 1.5, 0.0, 4.0, 3.0, 4.0] };
                    c = true;
                }
                if !auto {
                    for (label, range) in [("Domain centre", 0..3), ("Domain size", 3..6)] {
                        ui.horizontal(|ui| {
                            if group_label(ui, label, 0.02, &mut domain[range.clone()]) {
                                c = true;
                                for k in range.clone() {
                                    domain[k] = if k >= 3 { domain[k].clamp(0.01, 1000.0) } else { domain[k].clamp(-1000.0, 1000.0) };
                                }
                            }
                            for k in range {
                                c |= pfield(ui, pdrag(&mut domain[k]).speed(0.02).range(if k >= 3 { 0.01..=1000.0 } else { -1000.0..=1000.0 })).changed();
                            }
                        });
                    }
                }
                ui.small("Input 1: emitter surface • Input 2: optional static collider. Select the node to drag its domain box in the viewport.");
            }
            NodeKind::WhitewaterSolver(w) => {
                ui.small("Spray, foam and bubbles where the fluid input splashes. Input 1: FLIP fluid • Input 2: optional collider. Writes `type` and spray/foam/bubble groups; renders as small spheres.");
                c |= pfield(ui, pslider(&mut w.max_particles, 0..=2_000_000).logarithmic(true).text("Max particles")).changed();
                c |= pfield(ui, pslider(&mut w.emission_rate, 0.0..=10_000.0).logarithmic(true).text("Emission rate")).changed();
                c |= pfield(ui, pslider(&mut w.trapped_air, 0.0..=4.0).text("Trapped air")).on_hover_text("Emission where fluid collides or tears apart").changed();
                c |= pfield(ui, pslider(&mut w.wave_crest, 0.0..=4.0).text("Wave crests")).on_hover_text("Emission from breaking crests").changed();
                c |= pfield(ui, pslider(&mut w.min_speed, 0.0..=10.0).text("Min speed")).changed();
                c |= pfield(ui, pslider(&mut w.max_speed, 0.1..=30.0).text("Full emission speed")).changed();
                c |= pfield(ui, pslider(&mut w.lifespan, 0.05..=10.0).text("Foam lifespan (s)")).changed();
                c |= pfield(ui, pslider(&mut w.spray_drag, 0.0..=5.0).text("Spray drag")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_buoyancy, 0.0..=10.0).text("Bubble buoyancy")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_drag, 0.0..=1.0).text("Bubble drag")).changed();
                c |= pfield(ui, pslider(&mut w.spray_neighbours, 0..=50).text("Spray below (neighbours)")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_neighbours, 1..=100).text("Bubbles above (neighbours)")).changed();
                c |= pfield(ui, pslider(&mut w.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut w.particle_size, 0.0005..=0.2).logarithmic(true).text("Particle size")).changed();
                c |= pfield(ui, pslider(&mut w.substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(&mut w.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                c |= pfield(ui, pslider(radius, 0.001..=1.0).logarithmic(true).text("Particle radius"))
                    .on_hover_text("A lone droplet's radius. 1–1.5× the particles' spacing, which for FLIP is ∛(emitter volume ÷ particles): 100,000 particles in a 1 m cube → about 0.03")
                    .changed();
                c |= pfield(ui, pslider(join_distance, 0.0..=1.0).text("Join distance")).on_hover_text("Extra reach: particles this much further apart still join into one surface. More fills gaps but rounds off splashes").changed();
                c |= pfield(ui, pslider(resolution, 16..=512).text("Resolution")).on_hover_text("Voxels across the particles' longest side. Never finer than a quarter of the radius; very large grids are made coarser").changed();
                c |= pfield(ui, pslider(smoothing, 0..=8).text("Smoothing")).on_hover_text("Passes that even out the surface. 1–2 for water; more melts away thin sheets").changed();
                ui.small("Particles (e.g. FLIP) to one smooth, closed surface. The particles' v, Cd and other point attributes carry onto it. For water: a glass material with IOR 1.33.");
            }
            NodeKind::Extrude(set) => {
                ui.small("Extrudes the node's group (or everything) along the normals. Faces outside the group stay as they are.");
                c |= ui.checkbox(&mut set.connected, "Keep shared points together").on_hover_text("On: connected faces extrude as one, with walls only around the outside. Off: every face on its own").changed();
                c |= pfield(ui, pslider(&mut set.distance, -10.0..=10.0).text("Distance")).changed();
                c |= pfield(ui, pslider(&mut set.inset, 0.0..=1.0).text("Inset")).changed();
                c |= pfield(ui, pslider(&mut set.divisions, 1..=50).text("Divisions")).changed();
                ui.separator();
                ui.strong("Output");
                c |= ui.checkbox(&mut set.output_front, "Output front").changed();
                c |= ui.checkbox(&mut set.output_back, "Output back").on_hover_text("Keep the original faces (flipped), closing the shape").changed();
                c |= ui.checkbox(&mut set.output_side, "Output side").changed();
                c |= ui.checkbox(&mut set.make_groups, "Output groups").changed();
                if set.make_groups {
                    for (label, name) in [("Front group", &mut set.front_group), ("Side group", &mut set.side_group), ("Back group", &mut set.back_group)] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            c |= ui.add(egui::TextEdit::singleline(name).desired_width(140.0)).changed();
                        });
                    }
                }
            }
            NodeKind::Clip { axis, offset, invert } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
                c |= pfield(ui, pslider(offset, -5.0..=5.0).text("Offset")).changed();
                c |= ui.checkbox(invert, "Keep Below").changed();
            }
            NodeKind::Blast { component, indices, invert } => {
                ui.label(format!("Delete {} selected component(s)", indices.len()));
                ui.horizontal(|ui| {
                    ui.label("Component:");
                    c |= ui.selectable_value(component, 0, "Vertices").changed();
                    c |= ui.selectable_value(component, 1, "Faces").changed();
                    c |= ui.selectable_value(component, 2, "Edges").changed();
                });
                c |= ui.checkbox(invert, "Keep selected (delete outside)").changed();
                if ui.button("Clear stored selection").clicked() {
                    indices.clear();
                    c = true;
                }
                ui.small("Select components in the viewport and press Delete to create this node.");
            }
            NodeKind::Cache { cache_id, mode, range } => {
                ui.label("Persistent frame cache: cache/cache_ID_frame.mcache");
                c |= pfield(ui, pslider(cache_id, 0..=9999).text("Cache ID")).changed();
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    c |= ui.selectable_value(mode, 0, "Read").changed();
                    c |= ui.selectable_value(mode, 1, "Write").changed();
                });
                ui.small("Write saves each evaluated frame. Read reuses files and falls back to the input on a miss.");
                ui.label("Frame range (what a Fetch TOP writes when nothing is connected to it)");
                ui.horizontal(|ui| {
                    c |= pfield(ui, pdrag(&mut range[0]).speed(1.0).prefix("Start ")).changed();
                    c |= pfield(ui, pdrag(&mut range[1]).speed(1.0).prefix("End ")).changed();
                    c |= pfield(ui, pdrag(&mut range[2]).speed(0.1).range(0.001..=1000.0).prefix("Step ")).changed();
                });
                ui.small("To write the cache in the background, or across wedges: in TASKS, point a Fetch at this node and cook it. Then set the mode to Read.");
            }
            NodeKind::Vdb { voxel_size, padding, operation, iterations, output } => {
                ui.heading("VDB from Polygons");
                ui.horizontal(|ui| {
                    ui.label("Output");
                    c |= ui.selectable_value(output, 1, "VDB (level set)").changed();
                    c |= ui.selectable_value(output, 0, "Polygons").changed();
                });
                ui.small(if *output == 1 {
                    "Outputs a signed-distance VDB; use Convert VDB to turn it into polygons or a fog volume."
                } else {
                    "Converts the mesh to a signed-distance volume and outputs its surface directly."
                });
                c |= pfield(ui, pslider(voxel_size, 0.03..=1.0).logarithmic(true).text("Voxel size"))
                    .changed();
                ui.small("Smaller voxels = more detail and slower cooking.");
                c |= pfield(ui, pslider(padding, 0..=4).text("Volume padding")).changed();
                ui.horizontal(|ui| {
                    ui.label("Surface operation");
                    egui::ComboBox::from_id_salt("vdb-operation")
                        .selected_text(match *operation {
                            1 => "Dilate",
                            2 => "Erode",
                            3 => "Smooth",
                            _ => "Rasterize",
                        })
                        .show_ui(ui, |ui| {
                            c |= ui.selectable_value(operation, 0, "Rasterize").changed();
                            c |= ui.selectable_value(operation, 1, "Dilate").changed();
                            c |= ui.selectable_value(operation, 2, "Erode").changed();
                            c |= ui.selectable_value(operation, 3, "Smooth").changed();
                        });
                });
                if *operation == 1 || *operation == 2 {
                    c |= pfield(ui, pslider(iterations, 1..=4).text("Surface offset (voxels)")).changed();
                    ui.small(if *operation == 1 {
                        "Moves the surface outward by the selected number of voxels."
                    } else {
                        "Moves the surface inward by the selected number of voxels."
                    });
                } else if *operation == 3 {
                    c |= pfield(ui, pslider(iterations, 1..=4).text("Smoothing passes")).changed();
                    ui.small("Removes small voxel-scale bumps.");
                } else {
                    ui.small("Creates the base VDB surface without changing its shape.");
                }
                ui.small("The VDB node is evaluated from its first input.");
            }
            NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts } => {
                ui.heading("MoonRay Renderer");
                ui.small("Renders every active geometry with its MAT material, the active lights and the chosen camera, or the displayed USD stage.");
                ui.label("Executable");
                c |= ui.text_edit_singleline(executable).changed();
                ui.label("Arguments ({scene} {output} {width} {height} {samples})");
                c |= ui.text_edit_singleline(arguments).changed();
                ui.label("Output image");
                c |= ui.text_edit_singleline(output).changed();
                if ui.button("Choose output file...").clicked() {
                    if let Some(path) = FileDialog::new().set_file_name("render.exr").save_file() {
                        *output = path.to_string_lossy().into_owned();
                        c = true;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Camera");
                    render_camera_combo(ui, &mut opts.camera, &camera_ids);
                });
                c |= ui.checkbox(&mut opts.stage, "Render the STAGE (USD) instead of the OBJ objects").changed();
                c |= hydra_checkbox(ui, opts);
                c |= pfield(ui, pdrag(width).range(1..=16384).prefix("Width ")).changed();
                c |= pfield(ui, pdrag(height).range(1..=16384).prefix("Height ")).changed();
                c |= pfield(ui, pdrag(samples).range(1..=4096).prefix("Samples per pixel ")).changed();
                c |= pfield(ui, pslider(&mut opts.max_depth, 1..=32).text("Max ray depth")).changed();
                c |= pfield(ui, pslider(&mut opts.light_samples, 1..=16).text("Light samples")).changed();
                c |= pfield(ui, pslider(&mut opts.bsdf_samples, 1..=16).text("BSDF samples")).changed();
                c |= pfield(ui, pslider(&mut opts.volume_samples, 1..=8).text("Volume lighting samples"))
                    .on_hover_text("Light samples per step inside smoke and fire; render time grows roughly in proportion")
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("Environment light");
                    c |= ui.color_edit_button_rgb(&mut opts.background).changed();
                });
                ui.small("A non-black color adds a uniform environment light (not visible in camera).");
                ui.separator();
                c |= ui.checkbox(&mut opts.sequence, "Render frame range").changed();
                if opts.sequence {
                    ui.horizontal(|ui| {
                        c |= pfield(ui, pdrag(&mut opts.range_start).speed(0.05).range(0.0..=100_000.0).prefix("Start ")).changed();
                        c |= pfield(ui, pdrag(&mut opts.range_end).speed(0.05).range(0.0..=100_000.0).prefix("End ")).changed();
                        c |= pfield(ui, pdrag(&mut opts.range_step).speed(0.01).range(0.0001..=1000.0).prefix("Step ")).changed();
                    });
                    if ui.button("Use timeline range").clicked() {
                        opts.range_start = self.playback_start.max(0.0);
                        opts.range_end = self.playback_end.max(opts.range_start);
                        c = true;
                    }
                    let frames = frame_times(opts.range_start, opts.range_end, opts.range_step).len();
                    ui.small(format!(
                        "{frames} frames, timeline units. Files: {} (use #### for the frame number position).",
                        frame_output(output, 1)
                    ));
                }
                if let Some(job) = &self.sequence {
                    ui.label(format!("Sequence: frame {}/{}", job.next, job.times.len()));
                    if ui.button("Cancel sequence").clicked() {
                        self.cancel_render();
                    }
                } else if self.moonray_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("MoonRay render in progress...");
                        if ui.button("Cancel").clicked() {
                            self.cancel_render();
                        }
                    });
                } else if ui.button(if opts.sequence { "Render sequence" } else { "Render" }).clicked() {
                    self.start_moonray_render();
                }
                ui.small("Set arguments to match your MoonRay installation. The node passes the exported scene path and output path as placeholders.");
            }
            NodeKind::Camera { position, rotation, focal, near, orthographic, pan, zoom } => {
                ui.label("Viewport camera rig");
                c |= vec3_ui(ui, "Target", position, 0.05);
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                c |= pfield(ui, pslider(focal, 0.5..=2.5).text("Focal scale")).changed();
                c |= pfield(ui, pslider(near, 0.05..=10.0).text("Near clip")).changed();
                c |= ui.checkbox(orthographic, "Orthographic").changed();
                c |= pfield(ui, pdrag(&mut pan[0]).speed(1.0).prefix("Pan X ")).changed();
                c |= pfield(ui, pdrag(&mut pan[1]).speed(1.0).prefix("Pan Y ")).changed();
                c |= pfield(ui, pslider(zoom, MIN_ZOOM..=200.0).logarithmic(true).text("Zoom")).changed();
                if self.active_camera == Some(id) && self.looking_through {
                    ui.horizontal(|ui| {
                        ui.label("Looking through this camera: moving the view leaves it.");
                        if ui.button("Stop looking through").clicked() {
                            self.look_through_request = Some(None);
                        }
                    });
                } else if self.active_camera == Some(id) {
                    ui.horizontal(|ui| {
                        ui.label("Active viewport camera: moving the viewport moves it.");
                        if ui.button("Stop using as viewport camera").on_hover_text("The viewport keeps its view; this camera stays where it is").clicked() {
                            self.active_camera = None;
                            self.status = "The viewport no longer moves this camera".into();
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("👁 Look through").on_hover_text("See through this camera without changing it; moving the view goes back to the free view").clicked() {
                            self.look_through_request = Some(Some(id));
                        }
                        if ui.button("Use as viewport camera").on_hover_text("Moving the view moves this camera").clicked() {
                            self.use_camera_requested = true;
                        }
                    });
                }
                ui.small("Target is the orbit center used by the procedural viewport.");
            }
            NodeKind::Light { position, rotation, color, intensity, kind } => {
                ui.label("Viewport light");
                c |= vec3_ui(ui, "Position", position, 0.05);
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                c |= vec3_ui(ui, "Color", color, 0.02);
                c |= pfield(ui, pslider(intensity, 0.0..=4.0).text("Intensity")).changed();
                ui.horizontal(|ui| {
                    ui.label("Type");
                    c |= ui.selectable_value(kind, 0, "Directional").changed();
                    c |= ui.selectable_value(kind, 1, "Point").changed();
                    c |= ui.selectable_value(kind, 2, "Fill").changed();
                });
                ui.small("Directional lights use rotation. Point lights use position and are evaluated per surface.\nActive lights drive both the viewport and MoonRay; with none active, a default key/fill rig is used.");
            }
            NodeKind::Switch { index } => {
                c |= pfield(ui, pslider(index, 0..=switch_inputs.max(2) - 1).text("Input")).changed();
                ui.small(format!(
                    "{switch_inputs} input(s) connected; 0 passes the first, 1 the second, and so on (past the last, the last). Type an expression to choose by frame, e.g. $F > 24 or $F % {}.",
                    switch_inputs.max(2)
                ));
            }
            NodeKind::Reverse => {
                ui.label("Flips triangle winding (normals).");
            }
            NodeKind::Subdivide { levels } => {
                c |= pfield(ui, pslider(levels, 1..=4).text("Levels")).changed();
            }
            NodeKind::Remesh { target } => {
                c |= pfield(ui, pslider(target, 4..=50_000).logarithmic(true).text("Target triangles"))
                    .changed();
                ui.label("Adds shared-edge subdivisions until the target is reached.");
            }
            NodeKind::HairGuides(set) => {
                ui.small("Input 1: skin mesh. Grows guide curves from its surface along the normals.");
                c |= pfield(ui, pslider(&mut set.count, 1..=20_000).logarithmic(true).text("Guides")).changed();
                c |= pfield(ui, pslider(&mut set.length, 0.0..=5.0).text("Length")).changed();
                c |= pfield(ui, pslider(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.droop, 0.0..=3.0).text("Droop")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::GuideProcess(set) => {
                ui.small("Shapes the guide curves from input 1. Chain several to combine operations.");
                egui::ComboBox::from_label("Operation").selected_text(groom::GUIDE_OPS[set.op.min(groom::GUIDE_OPS.len() - 1)]).show_ui(ui, |ui| {
                    for (i, label) in groom::GUIDE_OPS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.op, i, *label).changed();
                    }
                });
                let (amount_label, range) = match set.op {
                    0 => ("Length", 0.0..=5.0),
                    1 => ("Bend", 0.0..=5.0),
                    2 => ("Frizz", 0.0..=2.0),
                    3 => ("Curl radius", 0.0..=2.0),
                    _ => ("Smoothing", 0.0..=2.0),
                };
                c |= pfield(ui, pslider(&mut set.amount, range).text(amount_label)).changed();
                if matches!(set.op, 2 | 3) {
                    let label = if set.op == 2 { "Frequency" } else { "Turns per unit length" };
                    c |= pfield(ui, pslider(&mut set.frequency, 0.0..=20.0).text(label)).changed();
                    c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                }
            }
            NodeKind::HairDeform(set) => {
                ui.small("Input 1: hair or guides • Input 2: animated skin. Each strand moves with the skin under its root.");
                c |= pfield(ui, pdrag(&mut set.rest_frame).speed(1.0).range(-10_000.0..=100_000.0).prefix("Rest frame "))
                    .on_hover_text("The frame the hair was groomed on. The skin must keep the same topology at every frame.")
                    .changed();
            }
            NodeKind::HairGenerate(set) => {
                ui.small("Input 1: skin • Input 2: guides. Interpolates dense hair from the three nearest guides.");
                c |= pfield(ui, pslider(&mut set.count, 1..=200_000).logarithmic(true).text("Hairs")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.clump, 0.0..=1.0).text("Clumping")).changed();
                c |= pfield(ui, pslider(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= pfield(ui, pslider(&mut set.frizz, 0.0..=2.0).text("Frizz")).changed();
                c |= pfield(ui, pslider(&mut set.width_root, 0.0..=0.05).text("Root width")).changed();
                c |= pfield(ui, pslider(&mut set.width_tip, 0.0..=0.05).text("Tip width")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                ui.small("Merge the result with the skin to render both.");
            }
            NodeKind::TimeShift { offset, hold, at } => {
                ui.heading("Time Shift");
                ui.small("Input 1 as it is at another time: later or earlier by Offset, or frozen at one moment. Merge shifted copies to build your own loops, or start an effect later.");
                c |= ui.checkbox(hold, "Hold at one time").changed();
                if *hold {
                    c |= pfield(ui, pslider(at, 0.0..=20.0).text("At (s)")).changed();
                    ui.weak(format!("frame {}", (*at * sims::SIM_FPS).round() as i32 + 1));
                } else {
                    c |= pfield(ui, pslider(offset, -10.0..=10.0).text("Offset (s)")).changed();
                    ui.weak(format!("{:+} frames", (*offset * sims::SIM_FPS).round() as i32));
                }
            }
            NodeKind::TimeWarp { speed, anchor } => {
                ui.heading("Time Warp");
                ui.small("Input 1 retimed: time runs Speed times as fast, counting from Anchor, where it's unchanged. 0.5 is half speed. Put a Time Blend after it to smooth a slowed-down simulation.");
                c |= pfield(ui, pslider(speed, -4.0..=4.0).text("Speed")).changed();
                c |= pfield(ui, pslider(anchor, 0.0..=20.0).text("Anchor (s)")).changed();
            }
            NodeKind::TimeBlend => {
                ui.heading("Time Blend");
                ui.small("Simulations step in whole frames, so slowed or shifted by part of a frame they stutter. Between whole frames this mixes the frame before with the one after: particles matched by id, volumes voxel by voxel, other geometry point by point. Put it after a Time Warp or Time Shift.");
            }
            NodeKind::Trail { samples, step, mode, fade } => {
                ui.heading("Trail");
                ui.small("Input 1 over its last few moments, for streaks behind sparks and slashes. Copies merges them, each point with a trail attribute (0 newest, 1 oldest); Curves draws a line through each point's past positions, matched by id; Velocity adds v from how far each point moved.");
                ui.horizontal(|ui| {
                    for (value, label) in [(0, "Copies"), (1, "Curves"), (2, "Velocity")] {
                        c |= ui.selectable_value(mode, value, label).changed();
                    }
                });
                if *mode != 2 {
                    c |= pfield(ui, pslider(samples, 2..=60).text("Samples")).changed();
                }
                c |= pfield(ui, pslider(step, 0.1..=10.0).text("Step (frames)")).changed();
                if *mode == 0 {
                    c |= ui.checkbox(fade, "Fade Alpha and pscale towards the oldest").changed();
                }
            }
            NodeKind::LoopParticles { start, length } => {
                ui.heading("Loop Particles");
                ui.small("Makes a particle simulation (input 1) loop exactly, for looping game effects: no blending, so nothing ghosts. Set the playback range to Start until Start + Length and export it as a sprite sheet. The particles mustn't affect each other, and by Start the emitter should have been running for at least a particle's lifetime.");
                c |= pfield(ui, pslider(start, 0.0..=20.0).text("Start (s)")).changed();
                c |= pfield(ui, pslider(length, 1.0 / sims::SIM_FPS..=10.0).text("Length (s)")).changed();
                ui.weak(format!("{} frames", (*length * sims::SIM_FPS).round()));
            }
            NodeKind::MakeLoop { start, length, blend } => {
                ui.heading("Make Loop");
                ui.small("Makes a simulation (input 1) loop, such as Pyro: it carries on past the end and blends that into the start, voxel by voxel or point by point, over Blend seconds. Set the playback range to Start until Start + Length. For particles, Loop Particles loops exactly.");
                c |= pfield(ui, pslider(start, 0.0..=20.0).text("Start (s)")).changed();
                c |= pfield(ui, pslider(length, 1.0 / sims::SIM_FPS..=10.0).text("Length (s)")).changed();
                c |= pfield(ui, pslider(blend, 0.0..=*length).text("Blend (s)")).changed();
                ui.weak(format!("{} frames, blending over the first {}", (*length * sims::SIM_FPS).round(), (*blend * sims::SIM_FPS).round()));
                if ui.button("Find best length").on_hover_text("Tries lengths from half to twice this one and picks the one where the effect comes back closest to how it started, so the blend has least to hide. Keeps this length if none comes back. Simulates up to twice the length past Start.").clicked() {
                    self.loop_search_request = Some(id);
                }
            }
            NodeKind::Pyro(set) => {
                ui.heading("Pyro Solver");
                ui.small("Smoke and fire on a voxel grid. Input 1: source (closed mesh or points, pscale sets point radius) • Input 2: optional collider (closed mesh).");
                ui.horizontal(|ui| {
                    ui.label("Preset");
                    for (label, preset) in set.presets() {
                        if ui.button(label).clicked() {
                            *set = preset;
                            c = true;
                        }
                    }
                });
                ui.separator();
                ui.strong("Domain");
                c |= vec3_ui(ui, "Center", &mut set.center, 0.05);
                c |= vec3_ui(ui, "Size", &mut set.size, 0.05);
                if ui.button("Fit around source").on_hover_text("Centre the box on the source and leave room for the smoke to rise").clicked() {
                    self.pyro_fit_request = Some(id);
                }
                c |= pfield(ui, pslider(&mut set.resolution, 8..=160).text("Resolution (voxels on longest side)")).changed();
                let voxels = {
                    let h = set.size.iter().cloned().fold(0.0, f32::max) / set.resolution.max(4) as f32;
                    set.size.iter().map(|s| ((s / h).ceil() as usize).max(4)).product::<usize>()
                };
                ui.weak(format!("{voxels} voxels"));
                c |= pfield(ui, pslider(&mut set.substeps, 1..=8).text("Substeps")).changed();
                ui.separator();
                ui.strong("Source");
                c |= pfield(ui, pslider(&mut set.density, 0.0..=4.0).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.temperature, 0.0..=4.0).text("Temperature")).changed();
                c |= pfield(ui, pslider(&mut set.fuel, 0.0..=4.0).text("Fuel (fire)")).changed();
                ui.strong("Shape");
                c |= pfield(ui, pslider(&mut set.buoyancy, -2.0..=5.0).text("Buoyancy")).changed();
                c |= pfield(ui, pslider(&mut set.dissipation, 0.0..=3.0).text("Dissipation")).changed();
                c |= pfield(ui, pslider(&mut set.cooling, 0.0..=5.0).text("Cooling")).changed();
                c |= pfield(ui, pslider(&mut set.burn_rate, 0.0..=5.0).text("Burn rate")).changed();
                c |= pfield(ui, pslider(&mut set.vorticity, 0.0..=3.0).text("Vorticity")).changed();
                c |= pfield(ui, pslider(&mut set.turbulence, 0.0..=3.0).text("Turbulence")).changed();
                c |= vec3_ui(ui, "Wind", &mut set.wind, 0.05);
                c |= pfield(ui, pslider(&mut set.opacity, 0.0..=4.0).text("Display opacity")).changed();
                ui.small("Output: points with density, temperature, flame, Cd, Alpha and pscale, drawn as soft volume splats. Edits restart the simulation.");
            }
            NodeKind::GroupCreate(set) => {
                ui.horizontal(|ui| {
                    ui.label("Group name");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.name).desired_width(140.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Type");
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Points, "Points").changed();
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Prims, "Primitives").changed();
                });
                egui::ComboBox::from_label("Method").selected_text(groups::CREATE_METHODS[set.method.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in groups::CREATE_METHODS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, value, *label).changed();
                    }
                });
                match set.method {
                    0 => {
                        c |= vec3_ui(ui, "Min", &mut set.bbox_min, 0.05);
                        c |= vec3_ui(ui, "Max", &mut set.bbox_max, 0.05);
                    }
                    1 => {
                        c |= pfield(ui, pslider(&mut set.percent, 0.0..=100.0).text("Percent")).changed();
                        c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                    }
                    2 => {
                        c |= vec3_ui(ui, "Direction", &mut set.direction, 0.05);
                        c |= pfield(ui, pslider(&mut set.spread, 0.0..=180.0).text("Spread angle")).changed();
                    }
                    _ => {
                        c |= ui.add(egui::TextEdit::singleline(&mut set.expression).hint_text("@P.y>0  0-10  other ^skip").desired_width(220.0)).changed();
                    }
                }
                ui.horizontal(|ui| {
                    group_pick_ui(ui, self.group_pick.as_ref(), (self.selected_vertices.len(), self.selected_triangles.len()), id, PickTarget::GroupCreate, false, &mut self.group_pick_request, &mut self.group_pick_domain, &mut self.group_pick_finish);
                    ui.small("Picking sets the method to Expression, with the picked numbers.");
                });
                egui::ComboBox::from_label("Combine").selected_text(groups::COMBINE_MODES[set.combine.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in groups::COMBINE_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                ui.small("Stored as a 0/1 attribute (group:name / prim:group:name); use the name in any node's Group field.");
            }
            NodeKind::AttributePromote(set) => {
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.pattern).desired_width(160.0).hint_text("Cd"))
                        .on_hover_text("Attributes of the original class, separated by spaces: * for all, ^name to skip one. P (named) gives each polygon its centre")
                        .changed();
                });
                for (label, class) in [("From", &mut set.from), ("To", &mut set.to)] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        for (value, name) in promote::CLASSES.iter().enumerate() {
                            c |= ui.selectable_value(class, value, *name).changed();
                        }
                    });
                }
                egui::ComboBox::from_label("Promotion method").selected_text(promote::METHODS[set.method.min(promote::METHODS.len() - 1)]).show_ui(ui, |ui| {
                    for (value, name) in promote::METHODS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, value, *name).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("New name");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.rename).desired_width(140.0).hint_text("same name")).changed();
                });
                c |= ui.checkbox(&mut set.delete_original, "Delete original").changed();
                ui.small("Where several values meet (a point shared by polygons, a polygon's points, everything for the detail), the method combines them. A quad is one primitive with four vertices.");
            }
            NodeKind::AttributeBlur(set) => {
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.pattern).desired_width(160.0).hint_text("Cd mask"))
                        .on_hover_text("Point attributes, separated by spaces: * for all, ^name to skip one. P (named) smooths the shape itself")
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Neighbours");
                    for (value, label) in blur::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Weighting");
                    for (value, label) in blur::WEIGHTINGS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.weighting, value, *label).on_hover_text("Edge length: nearer neighbours count more").changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.iterations, 0..=200).logarithmic(true).text("Blurring iterations")).changed();
                c |= pfield(ui, pslider(&mut set.step, 0.0..=1.0).text("Step size")).on_hover_text("How far each pass moves toward the neighbours' average").changed();
                if set.mode == 1 {
                    c |= pfield(ui, pslider(&mut set.radius, 0.001..=10.0).logarithmic(true).text("Radius")).changed();
                    c |= pfield(ui, pslider(&mut set.max_points, 1..=100).text("Max points")).changed();
                } else {
                    c |= ui.checkbox(&mut set.pin_border, "Pin border").on_hover_text("Points on open edges keep their values").changed();
                }
                ui.small("Connectivity: points sharing a polygon edge. Proximity: the nearest points within the radius (works on loose points and particles). The group's points change; the others feed in but stay as they are.");
            }
            NodeKind::Enumerate(set) => {
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in enumerate::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).desired_width(140.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Piece attribute");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.piece_attribute).desired_width(140.0).hint_text("optional"))
                        .on_hover_text("Elements sharing a value of this attribute share a number: the pieces are numbered, in the order they first appear")
                        .changed();
                });
                c |= pfield(ui, pdrag(&mut set.start).speed(0.1).prefix("Start ")).changed();
                c |= pfield(ui, pdrag(&mut set.increment).speed(0.1).prefix("Increment ")).changed();
                ui.small("Writes 0, 1, 2… (from Start, by Increment) on each point or primitive (a polygon counts once). For Copy to Points' piece attribute: Class Primitives on the shapes to copy.");
            }
            NodeKind::Color(set) => {
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in color::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Color type").selected_text(color::MODES[set.mode.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in color::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                match set.mode {
                    1 => {
                        c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                        ui.horizontal(|ui| {
                            ui.label("One colour per");
                            c |= ui.add(egui::TextEdit::singleline(&mut set.random_by).hint_text("element, or e.g. piece").desired_width(130.0)).changed();
                        });
                    }
                    2 => {
                        ui.horizontal(|ui| {
                            ui.label("From");
                            c |= ui.color_edit_button_rgb(&mut set.color).changed();
                            ui.label("to");
                            c |= ui.color_edit_button_rgb(&mut set.color2).changed();
                        });
                        ui.horizontal(|ui| {
                            ui.label("Along");
                            for (value, label) in color::AXES.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                            }
                        });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.label("Color");
                            c |= ui.color_edit_button_rgb(&mut set.color).changed();
                        });
                    }
                }
                ui.small("Sets Cd (prim:Cd for primitives). Use the Group field above to colour only part of the geometry.");
            }
            NodeKind::AttributeTransfer(set) => {
                ui.small("Copies attributes from the second input onto the nearest points or primitives of the first.");
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in transfer::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.pattern).hint_text("* or Cd temperature ^id").desired_width(160.0)).changed();
                });
                c |= pfield(ui, pslider(&mut set.distance, 0.0..=100.0).logarithmic(true).smallest_positive(0.001).text("Distance threshold"))
                    .on_hover_text("Elements farther than this from every source keep what they had").changed();
                c |= pfield(ui, pslider(&mut set.blend_width, 0.0..=100.0).logarithmic(true).smallest_positive(0.001).text("Blend width"))
                    .on_hover_text("Past the threshold, the transfer fades out over this distance instead of stopping at a hard edge").changed();
                c |= pfield(ui, pslider(&mut set.samples, 1..=32).text("Max samples"))
                    .on_hover_text("How many nearby sources are blended, weighted by distance (1: the nearest only)").changed();
            }
            NodeKind::EnvironmentLight { file, color, intensity, rotation, visible } => {
                ui.heading("Environment Light");
                ui.small("Lights the scene from a lat-long HDRI (.exr or .hdr), in MoonRay and the viewport.");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/sky.exr").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("HDRI", &["exr", "hdr", "png", "jpg", "jpeg", "tif", "tiff"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if !file.is_empty() {
                    if let Err(e) = envlight::load(file) {
                        ui.colored_label(egui::Color32::from_rgb(230, 110, 90), e);
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                c |= pfield(ui, pslider(intensity, 0.0..=10.0).text("Intensity")).changed();
                c |= pfield(ui, pslider(rotation, -180.0..=180.0).text("Rotate (Y°)")).changed();
                c |= ui.checkbox(visible, "Visible in camera (background)").changed();
            }
            NodeKind::AttributePaint { attribute, values, settings: set } => {
                let painting = self.paint_active;
                if ui
                    .selectable_label(painting, if painting { "🖌 Painting — drag in the viewport (Ctrl erases)" } else { "🖌 Paint in viewport" })
                    .on_hover_text("While on, dragging in the viewport paints instead of orbiting. Right/middle drag still pans.")
                    .clicked()
                {
                    self.paint_active = !painting;
                }
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(attribute).desired_width(110.0)).changed();
                    egui::ComboBox::from_id_salt("paint_presets").selected_text("Groom ▾").show_ui(ui, |ui| {
                        for name in paint::GROOM_ATTRIBUTES {
                            if ui.selectable_label(attribute == name, name).clicked() {
                                *attribute = name.to_owned();
                                c = true;
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    for (value, label) in paint::PAINT_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.value, -2.0..=4.0).text("Paint value")).changed();
                c |= pfield(ui, pslider(&mut set.default, -2.0..=4.0).text("Default (unpainted)")).changed();
                c |= pfield(ui, pslider(&mut set.radius, 2.0..=400.0).logarithmic(true).text("Brush radius (px)")).changed();
                c |= pfield(ui, pslider(&mut set.strength, 0.01..=1.0).text("Strength")).changed();
                c |= ui.checkbox(&mut set.visualize, "Show as colour (Cd)").changed();
                ui.horizontal(|ui| {
                    if ui.button("Fill with paint value").clicked() {
                        // Needs the input's point count; done at the start of the next frame.
                        self.paint_fill = Some(id);
                    }
                    if ui.button("Clear").clicked() {
                        values.clear();
                        c = true;
                    }
                });
                ui.small(format!(
                    "{} painted point(s). The groom reads density, length, clump, frizz and mask from the skin.",
                    values.len()
                ));
            }
            NodeKind::Normal(set) => {
                ui.horizontal(|ui| {
                    ui.label("Add normals to");
                    for (value, label) in sops::NORMAL_DOMAINS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.domain, value, *label).changed();
                    }
                });
                if set.domain == 1 {
                    c |= pfield(ui, pslider(&mut set.cusp_angle, 0.0..=180.0).text("Cusp angle"))
                        .on_hover_text("Faces meeting at a sharper angle than this keep a hard edge")
                        .changed();
                }
                egui::ComboBox::from_label("Weighting").selected_text(sops::NORMAL_WEIGHTS[set.weighting.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in sops::NORMAL_WEIGHTS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.weighting, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.reverse, "Reverse normals").changed();
                ui.small("The viewport shades with these normals (vertex N, then point N).");
            }
            NodeKind::Scatter(set) => {
                ui.small("Scatters points on the input's surface, or inside it when it's a closed mesh. Points get N, pscale and sourceprim.");
                c |= pfield(ui, pslider(&mut set.count, 1..=1_000_000).logarithmic(true).text("Points")).changed();
                c |= ui.checkbox(&mut set.volume, "Fill the inside (closed meshes)").changed();
                c |= pfield(ui, pslider(&mut set.relax_iterations, 0..=50).text("Relax iterations")).on_hover_text("Spreads the points out evenly").changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::Boolean(set) => {
                ui.small("Combines input A (left) with input B (right). Both should be closed meshes. Result faces are in the a_faces and b_faces primitive groups.");
                egui::ComboBox::from_label("Operation").selected_text(boolean::OPERATIONS[set.operation.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::OPERATIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.operation, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Method").selected_text(boolean::MODES[set.mode.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                if set.mode == 1 {
                    c |= pfield(ui, pslider(&mut set.resolution, 16..=400).text("Voxel resolution")).on_hover_text("Voxels along the longest side").changed();
                } else {
                    ui.small("Exact keeps sharp edges. Use Volume for very dense or leaky meshes.");
                }
            }
            NodeKind::PointDeform(set) => {
                ui.small("Input 1: geometry • Input 2: rest points • Input 3: the same points, moved (a simulation, a lattice, an animated cloud). The geometry follows them, bending and turning.");
                c |= pfield(ui, pslider(&mut set.influences, 1..=32).text("Points followed")).changed();
                c |= pfield(ui, pslider(&mut set.fit_neighbours, 3..=64).text("Rotation from neighbours")).changed();
                c |= ui.checkbox(&mut set.rotate, "Follow rotation").changed();
            }
            NodeKind::TransformPieces => {
                ui.small("Input 1: geometry with `piece` (or `name`) • Input 2: one point per piece with P, orient or transform, pscale • Input 3 (optional): the template points at rest. Each piece moves rigidly with its point.");
            }
            NodeKind::ExtractCentroid(set) => {
                ui.small("One point per piece, at its centre (with a piece attribute): feed it to Copy to Points or Transform Pieces.");
                egui::ComboBox::from_label("Pieces").selected_text(sopkit::PARTITIONS[set.partition.min(2)]).show_ui(ui, |ui| {
                    for (i, name) in sopkit::PARTITIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.partition, i, *name).changed();
                    }
                });
                egui::ComboBox::from_label("Centre").selected_text(sopkit::CENTRES[set.method.min(1)]).show_ui(ui, |ui| {
                    for (i, name) in sopkit::CENTRES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, i, *name).changed();
                    }
                });
            }
            NodeKind::MatchSize(set) => {
                ui.small(if has_reference { "Lines the input up with input 2's bounding box." } else { "Lines the input up with the target box below (or connect a reference to input 2)." });
                for (a, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Justify {axis}"));
                        for (j, name) in sopkit::JUSTIFY.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.justify[a], j, *name).changed();
                        }
                    });
                }
                c |= ui.checkbox(&mut set.scale_to_fit, "Scale to fit").changed();
                if set.scale_to_fit {
                    c |= ui.checkbox(&mut set.uniform, "Uniform scale (keep proportions)").changed();
                }
                if !has_reference {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, "Target centre", 0.01, &mut set.target_center[..]);
                        for v in set.target_center.iter_mut() {
                            c |= pfield(ui, pdrag(v).speed(0.01)).changed();
                        }
                    });
                    ui.horizontal(|ui| {
                        if group_label(ui, "Target size", 0.01, &mut set.target_size[..]) {
                            c = true;
                            set.target_size.iter_mut().for_each(|v| *v = v.max(0.0));
                        }
                        for v in set.target_size.iter_mut() {
                            c |= pfield(ui, pdrag(v).speed(0.01).range(0.0..=1e6)).changed();
                        }
                    });
                }
            }
            NodeKind::ConvertLine => {
                ui.small("Every polygon edge (not the diagonals inside quads) as a two-point line, with a restlength attribute. Lines are curves: Resample, Copy to Points and grooming work on them.");
            }
            NodeKind::Metaball(set) => {
                ui.small("Soft blobs around the input's points (pscale scales each), joined where they meet; with no input, one ball at the origin.");
                c |= pfield(ui, pslider(&mut set.radius, 0.01..=10.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(&mut set.threshold, 0.01..=0.99).text("Threshold")).on_hover_text("Lower is fatter: blobs reach out and blend sooner").changed();
                c |= pfield(ui, pslider(&mut set.resolution, 8..=256).text("Resolution")).changed();
            }
            NodeKind::Connectivity(set) => {
                ui.small("Numbers the connected pieces 0, 1, 2… in an attribute.");
                ui.horizontal(|ui| {
                    ui.label("Class");
                    c |= ui.selectable_value(&mut set.class, 0, "Points").changed();
                    c |= ui.selectable_value(&mut set.class, 1, "Primitives").changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).desired_width(120.0)).changed();
                });
            }
            NodeKind::Resample(set) => {
                ui.small("Evenly spaced points along curves (Convert Line, grooms…); attributes are interpolated.");
                c |= ui.checkbox(&mut set.by_length, "Maximum segment length").changed();
                if set.by_length {
                    c |= pfield(ui, pslider(&mut set.length, 0.001..=10.0).logarithmic(true).text("Length")).changed();
                } else {
                    c |= pfield(ui, pslider(&mut set.segments, 1..=500).text("Segments")).changed();
                }
            }
            NodeKind::Measure(set) => {
                egui::ComboBox::from_label("Measure").selected_text(measure::KINDS[set.kind.min(6)]).show_ui(ui, |ui| {
                    for (value, label) in measure::KINDS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                ui.small(match set.kind {
                    0 => "Each primitive's area (prim attribute).",
                    1 => "Each primitive's perimeter; the Sum summary is the mesh's open boundary length.",
                    2 => "Each point's share of the surface area.",
                    3 => "Per point: mean curvature (1/radius on a sphere, + convex) or Gaussian (1/radius², 0 on flat or cylinders).",
                    4 => "Per point: the gradient (direction and rate of fastest increase) of a float attribute, as a vector.",
                    5 => "The volume of the closed mesh (detail attribute).",
                    _ => "Per point: the length of the curve it's on.",
                });
                if set.kind == 3 {
                    ui.horizontal(|ui| {
                        for (value, label) in measure::CURVATURES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.curvature, value, *label).changed();
                        }
                    });
                }
                if set.kind == 4 {
                    ui.horizontal(|ui| {
                        ui.label("Of attribute");
                        c |= ui.add(egui::TextEdit::singleline(&mut set.source).desired_width(120.0)).changed();
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Output name");
                    let hint = set.output_name();
                    c |= ui.add(egui::TextEdit::singleline(&mut set.name).hint_text(hint).desired_width(120.0)).changed();
                });
                if set.kind != 5 {
                    egui::ComboBox::from_label("Detail summary").selected_text(measure::SUMMARIES[set.summary.min(4)]).show_ui(ui, |ui| {
                        for (value, label) in measure::SUMMARIES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.summary, value, *label).changed();
                        }
                    });
                }
            }
            NodeKind::ForEachBegin | NodeKind::CompileBegin => {
                ui.small(if matches!(node.kind, NodeKind::ForEachBegin) {
                    "Puts out each iteration's geometry (detail attributes iteration, numiterations and value). Set the loop up on its For-Each End."
                } else {
                    "Start of a Compile Block: For-Each loops inside it run on several threads, and the block's result is cached."
                });
                if let Some(end) = &loop_begin_name {
                    ui.label(format!("Ends at {end}"));
                }
            }
            NodeKind::ForEachEnd(set) => {
                ui.small("Runs the nodes between its For-Each Begin and here once per iteration, merging the results (or feeding each result to the next, for feedback).");
                egui::ComboBox::from_label("Loop over").selected_text(flow::MODES[set.mode.min(5)]).show_ui(ui, |ui| {
                    for (value, label) in flow::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                match set.mode {
                    flow::NAMED => {
                        ui.horizontal(|ui| {
                            ui.label("Piece attribute");
                            c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).hint_text("name").desired_width(120.0)).changed();
                        });
                        ui.small("A primitive or point attribute, e.g. piece (Material Fracture) or name.");
                    }
                    flow::NUMBER | flow::FEEDBACK => {
                        c |= pfield(ui, pdrag(&mut set.iterations).range(0..=100_000).prefix("Iterations: ")).changed();
                        c |= pfield(ui, pdrag(&mut set.start).speed(0.1).prefix("Start value: ")).changed();
                        c |= pfield(ui, pdrag(&mut set.increment).speed(0.1).prefix("Increment: ")).changed();
                    }
                    _ => {}
                }
                let mut single = set.single_pass >= 0;
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut single, "Single pass").on_hover_text("Run only one iteration, to check it").changed();
                    if single {
                        let mut pass = set.single_pass.max(0);
                        c |= pfield(ui, pdrag(&mut pass).range(0..=1_000_000)).changed();
                        set.single_pass = pass;
                    } else {
                        set.single_pass = -1;
                    }
                });
                if set.mode != flow::FEEDBACK {
                    c |= ui.checkbox(&mut set.multithread, "Multithread when compiled").changed();
                    ui.small(if compiled_loop { "This loop is inside a Compile Block: iterations run in parallel." } else { "Put the loop inside a Compile Block to run its iterations in parallel." });
                }
                match &loop_begin_name {
                    Some(begin) => ui.label(format!("Begins at {begin}")),
                    None => ui.colored_label(egui::Color32::from_rgb(240, 120, 100), "No For-Each Begin: add the loop again from Flow Control."),
                };
            }
            NodeKind::CompileEnd { .. } => {
                ui.small("End of a Compile Block: the block cooks as a unit, its result is cached, and For-Each loops inside it run on several threads.");
                if let Some(begin) = &loop_begin_name {
                    ui.label(format!("Begins at {begin}"));
                }
            }
            NodeKind::ObjectMerge { sources, into_this } => {
                ui.small("Brings in an object's geometry (its display output), or any node's, in another object or this one.");
                let mut remove = None;
                for (i, source) in sources.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        // "object" for an object's display output, "object / node" for a node inside it.
                        let shown = source
                            .and_then(|s| {
                                merge_sources.iter().find_map(|(object, name, nodes)| {
                                    if *object == s {
                                        Some(name.clone())
                                    } else {
                                        nodes.iter().find(|(n, _)| *n == s).map(|(_, path)| format!("{name} / {path}"))
                                    }
                                })
                            })
                            .unwrap_or_else(|| "(choose)".to_owned());
                        ui.push_id(("object_merge", id, i), |ui| {
                            ui.menu_button(format!("{shown} ⏷"), |ui| {
                                if merge_sources.is_empty() {
                                    ui.weak("No geometry objects");
                                }
                                for (object, name, nodes) in &merge_sources {
                                    ui.menu_button(name, |ui| {
                                        let own = Some(*object) == merge_own_object;
                                        let display = ui
                                            .add_enabled(!own, egui::Button::selectable(*source == Some(*object), "Display output"))
                                            .on_hover_text("The object's display node, with its transform")
                                            .on_disabled_hover_text("An Object Merge can't merge its own object; pick a node in it instead");
                                        if display.clicked() {
                                            *source = Some(*object);
                                            c = true;
                                            ui.close();
                                        }
                                        if !nodes.is_empty() {
                                            ui.separator();
                                        }
                                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                                            for (n, path) in nodes {
                                                if ui.add(egui::Button::selectable(*source == Some(*n), path)).clicked() {
                                                    *source = Some(*n);
                                                    c = true;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                        });
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    sources.remove(i);
                    c = true;
                }
                if ui.button("➕ Add object").clicked() {
                    sources.push(None);
                    c = true;
                }
                c |= ui.checkbox(into_this, "Into this object").on_hover_text("Keep each object's position in the scene (off: its geometry as built, without its transform)").changed();
            }
            NodeKind::MaterialAssign { slots } => {
                ui.small("Assigns MAT materials to parts of the geometry: each slot's group (empty: everything) gets its material, overriding the object's. Later slots win.");
                let mut remove = None;
                for (i, (group, material)) in slots.iter_mut().enumerate() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(format!("Group {}", i + 1));
                        c |= ui.add(egui::TextEdit::singleline(group).hint_text("all").desired_width(120.0)).changed();
                        if ui.small_button("✕").on_hover_text("Remove this slot").clicked() {
                            remove = Some(i);
                        }
                    });
                    group_pick_ui(ui, self.group_pick.as_ref(), (self.selected_vertices.len(), self.selected_triangles.len()), id, PickTarget::MaterialSlot(i), true, &mut self.group_pick_request, &mut self.group_pick_domain, &mut self.group_pick_finish);
                    let shown = material.and_then(|m| material_ids.iter().find(|(mid, _)| *mid == m)).map_or("(object's material)".to_owned(), |(_, name)| name.clone());
                    ui.horizontal(|ui| {
                        ui.label("Material");
                        egui::ComboBox::from_id_salt(("material_slot", id, i)).selected_text(shown).show_ui(ui, |ui| {
                            c |= ui.selectable_value(material, None, "(object's material)").changed();
                            for (mid, name) in &material_ids {
                                c |= ui.selectable_value(material, Some(*mid), name).changed();
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    slots.remove(i);
                    c = true;
                }
                if ui.button("➕ Add slot").clicked() {
                    slots.push((String::new(), None));
                    c = true;
                }
                ui.small("Groups: names (from Group Create or Boolean's a_faces/b_faces), ranges like 0-99, or @Cd.r>0.5. Make materials in the MAT context.");
            }
            NodeKind::MaterialFracture(set) => {
                ui.small("Breaks a closed mesh into pieces for the RBD Solver. Pieces get `piece`; cut faces are in the `inside` group, the original surface in `outside`.");
                egui::ComboBox::from_label("Material").selected_text(fracture::MATERIALS[set.material.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in fracture::MATERIALS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.material, value, *label).changed();
                    }
                });
                match set.material {
                    1 => {
                        ui.horizontal(|ui| {
                            c |= group_label(ui, "Impact point", 0.01, &mut set.impact[..]);
                            for k in 0..3 {
                                c |= pfield(ui, pdrag(&mut set.impact[k]).speed(0.01)).changed();
                            }
                        });
                        c |= pfield(ui, pslider(&mut set.rings, 1..=20).text("Crack rings")).changed();
                        c |= pfield(ui, pslider(&mut set.spokes, 3..=64).text("Radial cracks")).changed();
                    }
                    m => {
                        c |= pfield(ui, pslider(&mut set.pieces, 2..=500).logarithmic(true).text("Pieces")).changed();
                        if m == 2 {
                            ui.horizontal(|ui| {
                                ui.label("Grain along");
                                for (value, label) in sops::AXES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut set.grain_axis, value, *label).changed();
                                }
                            });
                            c |= pfield(ui, pslider(&mut set.splinter, 1.0..=20.0).text("Splinter length")).changed();
                        }
                    }
                }
                c |= pfield(ui, pslider(&mut set.separate, 0.0..=1.0).text("Explode view")).on_hover_text("Moves pieces apart to see the cracks").changed();
                c |= pfield(ui, pslider(&mut set.glue_strength, 0.0..=100_000.0).logarithmic(true).text("Glue strength")).on_hover_text("How hard the constraints between pieces hold in the RBD Bullet Solver").changed();
                ui.small("Outputs: 1 pieces · 2 glue constraints (lines between neighbouring pieces) · 3 proxy collision shapes. Wire all three into an RBD Bullet Solver.");
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::RbdBullet(set) => {
                ui.small("Inputs: 1 pieces (piece attribute) · 2 glue constraints · 3 proxy collision shapes · 4 static collision geometry. Wire Material Fracture's three outputs straight in. Outputs the moved pieces, the constraints still holding, and the moved proxies.");
                c |= pfield(ui, pslider(&mut set.density, 0.01..=100.0).logarithmic(true).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=2.0).text("Friction")).changed();
                c |= pfield(ui, pslider(&mut set.bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(&mut set.gravity, -30.0..=30.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.glue_scale, 0.0..=100.0).logarithmic(true).text("Glue strength ×")).on_hover_text("Scales every constraint's strength; 0 breaks all glue at the start").changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=32).text("Substeps")).changed();
                c |= ui.checkbox(&mut set.ground, "Ground plane").changed();
                if set.ground {
                    c |= pfield(ui, pdrag(&mut set.ground_height).speed(0.01).prefix("Ground height: ")).changed();
                }
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Initial velocity", 0.05, &mut set.velocity[..]);
                    for k in 0..3 {
                        c |= pfield(ui, pdrag(&mut set.velocity[k]).speed(0.05)).changed();
                    }
                });
                ui.small("RBD Configure sets density, friction, bounce, active and velocity per piece.");
            }
            NodeKind::RbdConfigure(set) => {
                ui.small("Sets physical properties on the pieces in the node's group (all when empty), for the RBD Bullet Solver. Constraints and proxy pass through.");
                for (on, label, value, range) in [
                    (&mut set.set_density, "Density", &mut set.density, 0.01..=100.0),
                    (&mut set.set_friction, "Friction", &mut set.friction, 0.0..=2.0),
                    (&mut set.set_bounce, "Bounce", &mut set.bounce, 0.0..=1.0),
                ] {
                    ui.horizontal(|ui| {
                        c |= ui.checkbox(on, "").changed();
                        c |= ui.add_enabled(*on, pslider(value, range).text(label)).changed();
                    });
                }
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.set_active, "").changed();
                    c |= ui.add_enabled(set.set_active, egui::Checkbox::new(&mut set.active, "Active (off: static, others still hit it)")).changed();
                });
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.set_velocity, "").changed();
                    ui.add_enabled_ui(set.set_velocity, |ui| {
                        c |= group_label(ui, "Velocity", 0.05, &mut set.velocity[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut set.velocity[k]).speed(0.05)).changed();
                        }
                    });
                });
            }
            NodeKind::Pack(set) => {
                ui.small("Turns each piece into one point (P, transform, piece) whose geometry is stored once: cheap to move, copy and merge. The viewport and renders show the real geometry; Unpack turns it back.");
                egui::ComboBox::from_label("Pack").selected_text(packed::MODES[set.mode.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in packed::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Pivot");
                    for (value, label) in packed::PIVOTS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.pivot, value, *label).changed();
                    }
                });
            }
            NodeKind::Unpack { transfer } => {
                ui.small("Turns packed points back into geometry at their transform (P, transform, or orient and pscale).");
                c |= ui.checkbox(transfer, "Transfer the packed points' attributes (Cd, piece…)").changed();
            }
            NodeKind::CrowdSolver(set) => {
                ui.small("Input 1: start points (e.g. Scatter on a grid) • Input 2: optional ground to walk on • Input 3: optional obstacles.");
                ui.horizontal(|ui| {
                    ui.label("Display");
                    for (value, label) in crowd::DISPLAYS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.display, value, *label).changed();
                    }
                });
                if set.display == 0 {
                    ui.small("For your own characters: feed these points to Copy to Points's second input, with a character facing +Z, feet at the origin, into its first. orient turns each one to where its agent walks, and pscale sizes it to the agent height (for a character 1.75 tall).");
                }
                c |= pfield(ui, pslider(&mut set.speed, 0.0..=10.0).text("Walk speed")).changed();
                c |= pfield(ui, pslider(&mut set.speed_variance, 0.0..=1.0).text("Speed variance")).changed();
                c |= pfield(ui, pslider(&mut set.turn_rate, 1.0..=720.0).text("Turn rate°/s")).changed();
                c |= pfield(ui, pslider(&mut set.personal_space, 0.05..=5.0).text("Personal space")).changed();
                c |= pfield(ui, pslider(&mut set.separation, 0.0..=10.0).text("Separation")).changed();
                c |= pfield(ui, pslider(&mut set.alignment, 0.0..=5.0).text("Alignment")).changed();
                c |= pfield(ui, pslider(&mut set.cohesion, 0.0..=5.0).text("Cohesion")).changed();
                c |= pfield(ui, pslider(&mut set.wander, 0.0..=5.0).text("Wander")).changed();
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Goal", 0.1, &mut set.goal[..]);
                    for k in 0..3 {
                        c |= pfield(ui, pdrag(&mut set.goal[k]).speed(0.1)).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.goal_weight, 0.0..=10.0).text("Goal strength")).changed();
                c |= pfield(ui, pslider(&mut set.avoid_distance, 0.0..=10.0).text("Obstacle look-ahead")).changed();
                c |= pfield(ui, pslider(&mut set.agent_height, 0.1..=5.0).text("Agent height")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::Skeleton(set) => {
                ui.small("A skeleton: joints as points with `parent` and `transform`. Pose it with Rig Pose, Two Bone IK or Procedural Motion; skin a mesh with Joint Capture and Bone Deform.");
                egui::ComboBox::from_label("Preset").selected_text(kinefx::PRESETS[set.preset.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in kinefx::PRESETS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.preset, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.scale, 0.01..=10.0).logarithmic(true).text("Scale")).changed();
                if set.preset != 0 {
                    c |= pfield(ui, pslider(&mut set.joints, 2..=200).text("Joints")).changed();
                    c |= pfield(ui, pslider(&mut set.length, 0.01..=50.0).logarithmic(true).text("Length")).changed();
                }
            }
            NodeKind::RigPose(pose) => {
                ui.small("Rotates (degrees, in the joint's own axes) and moves joints; children follow. Every value can be keyframed.");
                for slot in 0..kinefx::POSE_SLOTS {
                    ui.separator();
                    c |= joint_menu(ui, ("pose", id, slot), &format!("Joint {}", slot + 1), &mut pose.joints[slot], &joint_names, true);
                    if pose.joints[slot].trim().is_empty() {
                        continue;
                    }
                    let v = &mut pose.values[slot];
                    for (label, range) in [("Rotate", 0..3), ("Translate", 3..6)] {
                        ui.horizontal(|ui| {
                            c |= group_label(ui, label, if range.start < 3 { 0.5 } else { 0.01 }, &mut v[range.clone()]);
                            for k in range {
                                c |= pfield(ui, pdrag(&mut v[k]).speed(if k < 3 { 0.5 } else { 0.01 })).changed();
                            }
                        });
                    }
                }
            }
            NodeKind::TwoBoneIk(set) => {
                ui.small("Bends a root → mid → tip chain (thigh, shin, foot) so the tip reaches the goal. Keyframe the goal to animate.");
                c |= joint_menu(ui, ("ik", id, 0), "Root", &mut set.root, &joint_names, false);
                c |= joint_menu(ui, ("ik", id, 1), "Mid", &mut set.mid, &joint_names, false);
                c |= joint_menu(ui, ("ik", id, 2), "Tip", &mut set.tip, &joint_names, false);
                for (label, value) in [("Goal", &mut set.goal), ("Pole", &mut set.pole)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut value[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut value[k]).speed(0.01)).changed();
                        }
                    });
                }
                c |= ui.checkbox(&mut set.use_pole, "Bend towards the pole").changed();
                c |= ui.checkbox(&mut set.keep_tip, "Keep the tip's orientation").changed();
                c |= pfield(ui, pslider(&mut set.blend, 0.0..=1.0).text("Blend")).changed();
            }
            NodeKind::ProceduralMotion(set) => {
                egui::ComboBox::from_label("Motion").selected_text(kinefx::MOTIONS[set.motion.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in kinefx::MOTIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.motion, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.rate, 0.0..=5.0).text("Cycles per second")).changed();
                c |= pfield(ui, pslider(&mut set.amplitude, 0.0..=3.0).text("Amplitude")).changed();
                match set.motion {
                    0 => c |= pfield(ui, pslider(&mut set.travel, 0.0..=5.0).text("Forward speed")).changed(),
                    1 => {
                        c |= pfield(ui, pslider(&mut set.lag, 0.0..=3.0).text("Lag per joint")).changed();
                        ui.horizontal(|ui| {
                            ui.label("Bend about");
                            for (value, label) in sops::AXES.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                            }
                        });
                    }
                    _ => {}
                }
            }
            NodeKind::SkeletonBlend { weight } => {
                ui.small("Blends two poses of the same skeleton (input 1 at 0, input 2 at 1).");
                c |= pfield(ui, pslider(weight, 0.0..=1.0).text("Blend")).changed();
            }
            NodeKind::JointCapture(set) => {
                ui.small("Input 1: mesh • Input 2: skeleton in its rest pose. Writes capture weights for Bone Deform.");
                c |= pfield(ui, pslider(&mut set.influences, 1..=kinefx::MAX_INFLUENCES).text("Joints per point")).changed();
                c |= pfield(ui, pslider(&mut set.falloff, 0.5..=12.0).text("Falloff")).on_hover_text("Higher: sharper transitions between bones").changed();
                c |= ui.checkbox(&mut set.visualize, "Colour by joint").changed();
            }
            NodeKind::BoneDeform => {
                ui.small("Input 1: captured mesh (Joint Capture) • Input 2: rest skeleton • Input 3: animated skeleton.");
            }
            NodeKind::Muscle(set) => {
                ui.small("A muscle from origin to insertion, or along a curve connected to input 1. Animate Activation to flex it.");
                for (label, point) in [("Origin", &mut set.origin), ("Insertion", &mut set.insertion)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut point[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut point[k]).speed(0.01)).changed();
                        }
                    });
                }
                egui::ComboBox::from_label("Shape").selected_text(muscle::PROFILES[set.profile.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in muscle::PROFILES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.profile, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.radius, 0.005..=2.0).logarithmic(true).text("Belly radius")).changed();
                if set.profile == 1 {
                    c |= pfield(ui, pslider(&mut set.flatness, 1.0..=10.0).text("Flatness")).changed();
                    c |= pfield(ui, pslider(&mut set.twist, -180.0..=180.0).text("Twist°")).changed();
                }
                c |= pfield(ui, pslider(&mut set.tendon, 0.0..=0.45).text("Tendon length")).changed();
                c |= pfield(ui, pslider(&mut set.activation, 0.0..=1.0).text("Activation")).changed();
                c |= pfield(ui, pslider(&mut set.contraction, 0.0..=0.8).text("Max contraction")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 2..=128).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.sides, 3..=64).text("Sides")).changed();
                c |= pfield(ui, pslider(&mut set.id, 0..=999).text("Muscle id")).changed();
            }
            NodeKind::MuscleSolver(set) => {
                ui.small("Jiggle on animated muscles (flexing or moved by their bones). The origin and insertion tips stay pinned.");
                c |= pfield(ui, pslider(&mut set.stiffness, 1.0..=2000.0).logarithmic(true).text("Firmness")).changed();
                c |= pfield(ui, pslider(&mut set.damping, 0.0..=50.0).text("Damping")).changed();
                c |= pfield(ui, pslider(&mut set.smoothing, 0..=50).text("Coherence")).on_hover_text("Higher wobbles more as one body").changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=16).text("Substeps")).changed();
            }
            NodeKind::MuscleSkin(set) => {
                ui.small("Input 1: skin • Input 2: muscles (animated). The skin follows how the muscles move from frame 0.");
                c |= pfield(ui, pslider(&mut set.falloff, 0.001..=5.0).logarithmic(true).text("Reach")).changed();
                c |= pfield(ui, pslider(&mut set.influences, 1..=16).text("Influences")).changed();
            }
            NodeKind::Terrain(set) => {
                ui.small("A flat heightfield grid. Shape it with Terrain Noise, Erode and Terrace.");
                ui.horizontal(|ui| {
                    if group_label(ui, "Size", 0.1, &mut set.size[..]) {
                        c = true;
                        set.size.iter_mut().for_each(|v| *v = v.clamp(0.01, 10_000.0));
                    }
                    c |= pfield(ui, pdrag(&mut set.size[0]).speed(0.1).range(0.01..=10_000.0)).changed();
                    c |= pfield(ui, pdrag(&mut set.size[1]).speed(0.1).range(0.01..=10_000.0)).changed();
                });
                c |= pfield(ui, pslider(&mut set.resolution, 2..=2048).logarithmic(true).text("Resolution")).on_hover_text("Grid cells along the longer side").changed();
            }
            NodeKind::TerrainNoise(set) => {
                egui::ComboBox::from_label("Noise").selected_text(terrain::NOISE_TYPES[set.kind.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::NOISE_TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Blend").selected_text(terrain::BLEND_MODES[set.blend.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::BLEND_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.blend, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.amplitude, -10.0..=10.0).text("Height")).changed();
                c |= pfield(ui, pslider(&mut set.element_size, 0.05..=100.0).logarithmic(true).text("Element size")).on_hover_text("Size of the largest features").changed();
                c |= pfield(ui, pslider(&mut set.octaves, 1..=12).text("Octaves")).changed();
                c |= pfield(ui, pslider(&mut set.roughness, 0.0..=1.0).text("Roughness")).changed();
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Offset", 0.05, &mut set.offset[..]);
                    c |= pfield(ui, pdrag(&mut set.offset[0]).speed(0.05)).changed();
                    c |= pfield(ui, pdrag(&mut set.offset[1]).speed(0.05)).changed();
                });
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                c |= ui.checkbox(&mut set.positive, "Positive only (0 to Height)").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").on_hover_text("Uses the `mask` attribute from Terrain Mask or Attribute Paint").changed();
            }
            NodeKind::TerrainErode(set) => {
                ui.small("Hydraulic erosion carves rivers and gullies; thermal erosion crumbles steep slopes. Writes `flow` and `debris` for Terrain Mask.");
                ui.label("Hydraulic");
                c |= pfield(ui, pslider(&mut set.droplets, 0..=2_000_000).logarithmic(true).text("Raindrops")).changed();
                c |= pfield(ui, pslider(&mut set.erosion, 0.0..=1.0).text("Erosion rate")).changed();
                c |= pfield(ui, pslider(&mut set.deposition, 0.0..=1.0).text("Deposition rate")).changed();
                c |= pfield(ui, pslider(&mut set.capacity, 0.0..=20.0).text("Sediment capacity")).changed();
                c |= pfield(ui, pslider(&mut set.evaporation, 0.0..=0.5).text("Evaporation")).changed();
                c |= pfield(ui, pslider(&mut set.inertia, 0.0..=0.95).text("Inertia")).on_hover_text("Higher: straighter, wider rivers").changed();
                c |= pfield(ui, pslider(&mut set.radius, 1..=8).text("Erosion radius (cells)")).changed();
                ui.label("Thermal");
                c |= pfield(ui, pslider(&mut set.thermal_iterations, 0..=500).text("Iterations")).changed();
                c |= pfield(ui, pslider(&mut set.talus_angle, 1.0..=89.0).text("Talus angle°")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::TerrainTerrace(set) => {
                c |= pfield(ui, pslider(&mut set.step, 0.01..=10.0).logarithmic(true).text("Step height")).changed();
                c |= pfield(ui, pslider(&mut set.offset, -5.0..=5.0).text("Offset")).changed();
                c |= pfield(ui, pslider(&mut set.sharpness, 0.0..=1.0).text("Sharpness")).on_hover_text("1: flat steps with sheer walls").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").changed();
            }
            NodeKind::TerrainMask(set) => {
                ui.small("Writes a 0–1 `mask` point attribute for Terrain Noise, Terrace or any other node.");
                egui::ComboBox::from_label("From").selected_text(terrain::MASK_SOURCES[set.source.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_SOURCES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.source, value, *label).changed();
                    }
                });
                let unit = match set.source {
                    0 => "height",
                    1 => "slope°",
                    _ => "value",
                };
                c |= pfield(ui, pdrag(&mut set.min).speed(0.05).prefix(format!("min {unit} "))).changed();
                c |= pfield(ui, pdrag(&mut set.max).speed(0.05).prefix(format!("max {unit} "))).changed();
                c |= pfield(ui, pslider(&mut set.falloff, 0.0..=50.0).text("Falloff")).changed();
                c |= pfield(ui, pslider(&mut set.blur, 0..=100).text("Blur passes")).changed();
                egui::ComboBox::from_label("Combine with existing mask").selected_text(terrain::MASK_COMBINE[set.combine.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_COMBINE.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                c |= ui.checkbox(&mut set.visualize, "Show in viewport (Cd)").changed();
            }
            NodeKind::TerrainColorize(set) => {
                ui.small("Colours the terrain (Cd) by height and slope, for the viewport and MoonRay.");
                for (label, colour) in [("Sand", &mut set.sand), ("Grass", &mut set.grass), ("Rock", &mut set.rock), ("Snow", &mut set.snow)] {
                    ui.horizontal(|ui| {
                        c |= ui.color_edit_button_rgb(colour).changed();
                        ui.label(label);
                    });
                }
                c |= pfield(ui, pslider(&mut set.sand_level, 0.0..=1.0).text("Sand up to")).changed();
                c |= pfield(ui, pslider(&mut set.snow_level, 0.0..=1.0).text("Snow from")).changed();
                c |= pfield(ui, pslider(&mut set.rock_slope, 0.0..=90.0).text("Rock slope°")).changed();
                c |= pfield(ui, pslider(&mut set.blend, 0.0..=0.5).text("Blend")).changed();
            }
            NodeKind::UvTexture(set) => {
                egui::ComboBox::from_label("Projection").selected_text(sops::UV_PROJECTIONS[set.projection.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in sops::UV_PROJECTIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.projection, value, *label).changed();
                    }
                });
                if matches!(set.projection, 0 | 1) {
                    ui.horizontal(|ui| {
                        ui.label(if set.projection == 0 { "Project along" } else { "Cylinder axis" });
                        for (value, label) in sops::AXES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                        }
                    });
                }
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Scale", 0.01, &mut set.scale[..]);
                    c |= pfield(ui, pdrag(&mut set.scale[0]).speed(0.01)).changed();
                    c |= pfield(ui, pdrag(&mut set.scale[1]).speed(0.01)).changed();
                    c |= group_label(ui, "Offset", 0.01, &mut set.offset[..]);
                    c |= pfield(ui, pdrag(&mut set.offset[0]).speed(0.01)).changed();
                    c |= pfield(ui, pdrag(&mut set.offset[1]).speed(0.01)).changed();
                });
                ui.small("Writes vertex:uv. Follow with UV Quick Shade to see them.");
            }
            NodeKind::UvTransform(set) => {
                for (label, value) in [("Translate", &mut set.translate), ("Scale", &mut set.scale), ("Pivot", &mut set.pivot)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut value[..]);
                        c |= pfield(ui, pdrag(&mut value[0]).speed(0.01)).changed();
                        c |= pfield(ui, pdrag(&mut value[1]).speed(0.01)).changed();
                    });
                }
                c |= pfield(ui, pslider(&mut set.rotate, -180.0..=180.0).text("Rotate")).changed();
            }
            NodeKind::UvQuickShade { file } => {
                ui.small("Shows a texture on the UVs in the viewport. Leave the file empty for a checker grid.");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("checker grid").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg", "tga", "bmp", "exr", "tif", "tiff"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                    if !file.is_empty() && ui.button("Clear").clicked() {
                        file.clear();
                        c = true;
                    }
                });
            }
            NodeKind::VellumSolver(set) => {
                ui.heading("Cloth Solver");
                ui.small("Input 1: geometry (a mesh for cloth/softbody, curves for hair, points for grains) • Input 2: optional static collider. Pin points with f@pin = 1; hair roots are pinned automatically.");
                ui.horizontal(|ui| {
                    ui.label("Type");
                    for (value, label) in vellum::VELLUM_TYPES.iter().enumerate() {
                        if ui.selectable_label(set.kind == value, *label).clicked() && set.kind != value {
                            // Switching type loads that type's typical settings.
                            *set = vellum::VellumSettings::defaults(value);
                            c = true;
                        }
                    }
                });
                ui.strong("Constraints");
                if set.kind != 3 {
                    c |= pfield(ui, pslider(&mut set.stretch_stiffness, 1.0..=1e8).logarithmic(true).text("Stretch stiffness")).changed();
                }
                if matches!(set.kind, 0 | 1) {
                    c |= pfield(ui, pslider(&mut set.bend_stiffness, 1e-3..=1e5).logarithmic(true).text("Bend stiffness")).changed();
                }
                if set.kind == 2 {
                    c |= pfield(ui, pslider(&mut set.shape_stiffness, 0.0..=1.0).text("Shape stiffness")).changed();
                }
                c |= pfield(ui, pslider(&mut set.mass, 1e-3..=100.0).logarithmic(true).text("Mass per point")).changed();
                let thickness_label = if set.kind == 3 { "Grain radius (pscale overrides)" } else { "Thickness" };
                c |= pfield(ui, pslider(&mut set.thickness, 1e-3..=0.5).logarithmic(true).text(thickness_label)).changed();
                ui.strong("Forces and collisions");
                c |= pfield(ui, pslider(&mut set.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.damping, 0.0..=5.0).text("Damping")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=1.0).text("Friction")).changed();
                c |= ui.checkbox(&mut set.ground, "Ground plane (y = 0)").changed();
                c |= ui.checkbox(&mut set.self_collision, "Self collision").on_hover_text("Point-based: keeps points apart by their thickness").changed();
                c |= ui
                    .checkbox(&mut set.pin_to_animation, "Pins follow animation")
                    .on_hover_text("Pinned points track the animated input (e.g. hair roots on a moving head)")
                    .changed();
                ui.strong("Solver");
                c |= pfield(ui, pslider(&mut set.substeps, 1..=100).text("Substeps"))
                    .on_hover_text("Raise this first if cloth stretches: small steps converge better than extra iterations")
                    .changed();
                c |= pfield(ui, pslider(&mut set.iterations, 1..=200).text("Constraint iterations")).changed();
            }
            NodeKind::RbdSolver(set) => {
                ui.heading("RBD Bullet Solver");
                ui.small("Input 1: pieces — each connected part (or each value of a piece point attribute) is one rigid body; points become small spheres • Input 2: optional static collider.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Collision shape");
                    for (value, label) in sims::RbdSettings::SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.shape, value, *label).changed();
                    }
                });
                if set.shape == 1 {
                    ui.weak("Convex decomposition fits concave pieces but is slow to build.");
                }
                c |= pfield(ui, pslider(&mut set.density, 0.01..=10.0).logarithmic(true).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=2.0).text("Friction")).changed();
                c |= pfield(ui, pslider(&mut set.bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(&mut set.gravity, -20.0..=30.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=16).text("Substeps")).changed();
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.ground, "Ground plane").changed();
                    if set.ground {
                        c |= pfield(ui, pdrag(&mut set.ground_height).speed(0.05).prefix("Height ")).changed();
                    }
                });
                c |= vec3_ui(ui, "Initial velocity", &mut set.velocity, 0.05);
                c |= vec3_ui(ui, "Initial spin (rad/s)", &mut set.spin, 0.05);
                ui.small("A point attribute v adds per-piece initial velocity. Output adds point attributes v and piece.");
            }
            NodeKind::SopSolver { substeps } => {
                ui.heading("SOP Solver");
                if solver_display_is_input {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 190, 90),
                        "⚠ The display flag inside this solver is on Prev_Frame/Input_1, so each step changes nothing. Enter the network and set the display flag (double-click) on your last node.",
                    );
                }
                ui.label("Each step cooks this node's network with Prev_Frame set to the previous step's result, so changes accumulate over time. Input_1 is the solver's input at the current step.");
                c |= pfield(ui, pslider(substeps, 1..=16).text("Substeps per frame")).changed();
                if ui.button("Enter solver network").clicked() {
                    self.current_network = Some(id);
                    self.context = AppContext::Obj;
                    self.selected = None;
                    self.graph_pan = egui::Vec2::ZERO;
                    c = true;
                }
                ui.small("At time 0 the output is the input. Example: Prev_Frame → Wrangle (@P.y += 0.02;) with the display flag on the wrangle.");
            }
            NodeKind::Subnet { values, spec } => {
                ui.heading(spec.asset.as_ref().map_or_else(|| "Subnet".to_owned(), |(name, version)| format!("{name} · asset version {version}")));
                if !spec.description.is_empty() {
                    ui.label(spec.description.as_str());
                }
                if spec.params.is_empty() {
                    ui.small("A network packaged as one node: double-click it to build inside. Inputs 1-4 come in through Subnet Input nodes; the output is the display node inside. Promote parameters of the nodes inside, below, to set them here.");
                }
                for (p, value) in spec.params.iter().zip(values.iter_mut()) {
                    let (lo, hi) = (p.min.min(p.max), p.max.max(p.min));
                    c |= pfield(ui, pslider(value, lo..=hi).clamping(egui::SliderClamping::Never).text(p.label.as_str())).changed();
                }
            }
            NodeKind::SubnetInput { index } => {
                ui.heading(format!("Subnet Input {}", *index + 1));
                ui.horizontal(|ui| {
                    for i in 0..4 {
                        c |= ui.selectable_value(index, i, format!("Input {}", i + 1)).changed();
                    }
                });
                ui.small("Inside a Subnet: what's wired into that input of the Subnet.");
            }
            NodeKind::SolverInput { index } => {
                ui.heading(if *index == 0 { "Prev_Frame" } else { "Input_1" });
                ui.horizontal(|ui| {
                    c |= ui.selectable_value(index, 0, "Prev_Frame").changed();
                    c |= ui.selectable_value(index, 1, "Input_1").changed();
                });
                ui.small("Prev_Frame is the solver's result from the previous step; Input_1 is the geometry wired into the SOP Solver, at the current step time.");
            }
            NodeKind::LopSopImport { geometry, path } => {
                ui.heading("SOP Import");
                ui.small("Brings OBJ geometry objects into the stage as Mesh prims: attributes become primvars (Cd → displayColor) and the object's MAT material is bound.");
                egui::ComboBox::from_id_salt("sop_import_geometry")
                    .selected_text(geometry.map_or("All geometry objects".to_string(), |g| format!("geometry{g}")))
                    .show_ui(ui, |ui| {
                        c |= ui.selectable_value(geometry, None, "All geometry objects").changed();
                        for g in &geometry_objects {
                            c |= ui.selectable_value(geometry, Some(*g), format!("geometry{g}")).changed();
                        }
                    });
                c |= prim_path_ui(ui, if geometry.is_some() { "Prim path" } else { "Parent path" }, path);
            }
            NodeKind::LopReference { file, path, sublayer } => {
                ui.heading("Reference / Sublayer");
                if !usd::NATIVE {
                    ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "This build has no OpenUSD (native-usd feature), so USD files cannot be read.");
                }
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/asset.usd").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new().add_filter("USD", &["usd", "usda", "usdc", "usdz"]).pick_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    c |= ui.selectable_value(sublayer, false, "Reference under a prim").changed();
                    c |= ui.selectable_value(sublayer, true, "Sublayer at the root").changed();
                });
                if !*sublayer {
                    c |= prim_path_ui(ui, "Prim path", path);
                }
                if ui.button("Reload file").on_hover_text("Read the file again after it changed on disk").clicked() {
                    usd::forget_loaded_files();
                    c = true;
                }
                ui.small("Opens .usd, .usda, .usdc and .usdz with full composition; animated files follow the timeline.");
            }
            NodeKind::LopPrimitive { shape, path, size, t } => {
                ui.heading("Primitive");
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in LOP_SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(shape, value, *label).changed();
                    }
                });
                c |= prim_path_ui(ui, "Prim path", path);
                c |= pfield(ui, pslider(size, 0.01..=10.0).logarithmic(true).text("Size (radius)")).changed();
                c |= vec3_ui(ui, "Translate", t, 0.05);
            }
            NodeKind::LopTransform { pattern, t, r, s } => {
                ui.heading("Transform");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                c |= vec3_ui(ui, "Translate", t, 0.05);
                c |= vec3_ui(ui, "Rotate°", r, 0.5);
                c |= vec3_ui(ui, "Scale", s, 0.02);
                ui.small("Applied in each matching prim's parent space, after its own transform.");
            }
            NodeKind::LopLight { kind, path, position, target, color, intensity, size } => {
                ui.heading("Light");
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in ["Distant", "Sphere", "Dome", "Rect"].iter().enumerate() {
                        c |= ui.selectable_value(kind, value, *label).changed();
                    }
                });
                c |= prim_path_ui(ui, "Prim path", path);
                if *kind != 2 {
                    c |= vec3_ui(ui, "Position", position, 0.05);
                    c |= vec3_ui(ui, "Aim at", target, 0.05);
                }
                ui.horizontal(|ui| {
                    ui.label("Color");
                    c |= ui.color_edit_button_rgb(color).changed();
                });
                c |= pfield(ui, pslider(intensity, 0.0..=20.0).text("Intensity")).changed();
                let size_label = match *kind {
                    0 => Some("Angle (degrees)"),
                    1 => Some("Radius"),
                    3 => Some("Width / height"),
                    _ => None,
                };
                if let Some(label) = size_label {
                    c |= pfield(ui, pslider(size, 0.0..=20.0).text(label)).changed();
                }
            }
            NodeKind::LopCamera { path, position, target, focal, aperture } => {
                ui.heading("Camera");
                c |= prim_path_ui(ui, "Prim path", path);
                c |= vec3_ui(ui, "Position", position, 0.05);
                c |= vec3_ui(ui, "Look at", target, 0.05);
                c |= pfield(ui, pslider(focal, 8.0..=300.0).logarithmic(true).text("Focal length (mm)")).changed();
                c |= pfield(ui, pslider(aperture, 5.0..=70.0).text("Horizontal aperture (mm)")).changed();
                if ui.button("Copy from viewport").clicked() {
                    (*position, *target) = viewport_view;
                    c = true;
                }
                ui.small("Rendering the STAGE with the Auto camera uses the first stage camera.");
            }
            NodeKind::LopMaterial { pattern, material } => {
                ui.heading("Assign Material");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                let selected = material
                    .and_then(|m| material_ids.iter().find(|(id, _)| *id == m))
                    .map_or("Choose a material".to_string(), |(_, name)| name.clone());
                egui::ComboBox::from_id_salt("lop_material").selected_text(selected).show_ui(ui, |ui| {
                    for (mid, name) in &material_ids {
                        c |= ui.selectable_value(material, Some(*mid), name).changed();
                    }
                });
                ui.small("MAT materials become UsdPreviewSurface materials under /materials; MoonRay renders them with the full MAT settings.");
            }
            NodeKind::LopPrune { pattern, hide } => {
                ui.heading("Prune");
                c |= prim_pattern_ui(ui, pattern, &picked_prim);
                c |= ui.checkbox(hide, "Only hide (keep the prims active)").changed();
            }
            NodeKind::LopMerge => {
                ui.heading("Merge");
                ui.label("Layers each input's prims over the inputs before it (any number of inputs: drop wires on the node).");
            }
            NodeKind::LopExport { file } => {
                ui.heading("USD Export");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/scene.usda").desired_width(220.0)).changed();
                    if ui.button("Choose…").clicked() {
                        if let Some(chosen) = FileDialog::new().set_file_name("scene.usda").add_filter("USD", &["usda", "usdc", "usd", "usdz"]).save_file() {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if ui.add_enabled(!file.trim().is_empty(), egui::Button::new("💾 Save USD")).clicked() {
                    self.usd_export_request = Some(id);
                }
                ui.small(if usd::NATIVE {
                    "The extension picks the format: .usda (text), .usdc or .usd (binary), .usdz (package)."
                } else {
                    "Saves .usda; .usdc and .usdz need the native-usd build."
                });
            }
            NodeKind::ConvertVdb { mode, iso } => {
                ui.heading("Convert VDB");
                ui.horizontal(|ui| {
                    ui.label("Convert to");
                    c |= ui.selectable_value(mode, 0, "Polygons").changed();
                    c |= ui.selectable_value(mode, 1, "Volume (fog)").changed();
                });
                if *mode == 0 {
                    c |= pfield(ui, pslider(iso, -1.0..=1.0).text("Isovalue")).changed();
                    ui.small("Level sets: distance from the surface (0 = the surface). Fog volumes and Pyro: density threshold, e.g. 0.5.");
                } else {
                    ui.small("Level sets become fog densities (1 inside); fog volumes pass through. Drawn as a soft volume.");
                }
                ui.small("Input: a VDB from VDB from Polygons, a .vdb file read with the File node, or Pyro smoke.");
            }
            NodeKind::FileImport { file, take, looping } => {
                ui.heading("File");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/model.$F4.obj").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new()
                            .add_filter("Geometry", &["obj", "fbx", "abc", "stl", "usd", "usda", "usdc", "usdz", "vdb"])
                            .pick_file()
                        {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                if fileio::has_frame_token(file) {
                    ui.weak(format!("Frame now: {}", fileio::expand_frame(file, self.time, sims::SIM_FPS)));
                }
                // FBX: which take plays, from the file's own list.
                if fileio::extension(file.trim()) == "fbx" {
                    let takes = fileio::fbx_take_names(file.trim());
                    if takes.is_empty() {
                        ui.weak("No animation takes in this file.");
                    } else {
                        let label = |name: &str, seconds: f64| format!("{name} ({seconds:.2} s)");
                        let shown = if take.is_empty() {
                            format!("First: {}", takes[0].0)
                        } else {
                            takes.iter().find(|(n, _)| n == take).map_or_else(|| format!("{take} (not in the file)"), |(n, s)| label(n, *s))
                        };
                        egui::ComboBox::from_label("Take").selected_text(shown).show_ui(ui, |ui| {
                            if ui.selectable_label(take.is_empty(), format!("First take ({})", takes[0].0)).clicked() {
                                take.clear();
                                c = true;
                            }
                            for (name, seconds) in &takes {
                                if ui.selectable_label(take == name, label(name, *seconds)).clicked() {
                                    *take = name.clone();
                                    c = true;
                                }
                            }
                        });
                        c |= ui.checkbox(looping, "Loop (else hold the last pose)").changed();
                    }
                }
                if ui.button("Reload").on_hover_text("Read the file again after it changed on disk").clicked() {
                    usd::forget_loaded_files();
                    c = true;
                }
                ui.small("Reads .obj, FBX (.fbx: the chosen take's animation, skinning and blend shapes play from frame 1), Alembic (.abc, animated: read at the current time), .stl, USD (.usd .usda .usdc .usdz) and .vdb. $F / $F4 insert the frame number for sequences. Separate objects get a piece attribute.");
            }
            NodeKind::FileExport { file } => {
                ui.heading("File Export");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/out.$F4.fbx").desired_width(220.0)).changed();
                    if ui.button("Choose…").clicked() {
                        if let Some(chosen) = FileDialog::new()
                            .set_file_name("geometry.obj")
                            .add_filter("Geometry", &["obj", "fbx", "abc", "stl", "usd", "usda", "usdc", "vdb"])
                            .save_file()
                        {
                            *file = chosen.to_string_lossy().into_owned();
                            c = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(!file.trim().is_empty(), egui::Button::new("💾 Write current frame")).clicked() {
                        self.file_export_request = Some((id, false));
                    }
                    // Alembic keeps the whole range in one file; other formats need $F for one file per frame.
                    let can_range = fileio::has_frame_token(file) || fileio::extension(file.trim()) == "abc";
                    if ui
                        .add_enabled(can_range, egui::Button::new("Write timeline range"))
                        .on_disabled_hover_text("Add $F or $F4 to the file name to write one file per frame, or write an .abc (one animated file)")
                        .clicked()
                    {
                        self.file_export_request = Some((id, true));
                    }
                });
                ui.small("The extension picks the format: .obj, .fbx, .abc (Alembic: the timeline range goes in one animated file), .stl, .usd/.usda/.usdc or .vdb (VDB input). The geometry passes through unchanged.");
            }
            NodeKind::Merge | NodeKind::Null => {
                ui.label("No parameters.");
            }
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
