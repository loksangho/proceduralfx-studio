// The panes: layout, timeline and keys, spreadsheet, node info, performance, scene graph, image view.

use crate::*;

impl ProceduralFxStudioApp {
    pub(crate) fn pane_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pane: Pane) {
        match pane {
            Pane::Viewport => self.viewport(ui, ctx),
            Pane::Network => self.node_editor(ui),
            Pane::Parameters => self.parameters_pane(ui),
            Pane::Timeline => self.timeline_pane(ui),
            Pane::Spreadsheet => self.spreadsheet_pane(ui),
            Pane::GraphEditor => self.graph_editor_pane(ui),
            Pane::Performance => self.performance_pane(ui),
        }
    }

    pub(crate) fn apply_pane_requests(&mut self, dock: &mut egui_dock::DockState<Pane>) {
        for request in std::mem::take(&mut self.pane_requests) {
            match request {
                PaneRequest::Reset => {
                    *dock = default_layout();
                    self.popped.clear();
                }
                PaneRequest::SaveAs(name) => {
                    let saved = safe_layout_name(&name).zip(layouts_dir()).zip(layout_to_text(dock)).map(|((name, dir), text)| {
                        let _ = std::fs::create_dir_all(&dir);
                        (std::fs::write(dir.join(format!("{name}.ron")), text), name)
                    });
                    self.status = match saved {
                        Some((Ok(()), name)) => format!("Layout \"{name}\" saved"),
                        Some((Err(e), _)) => format!("Couldn't save the layout: {e}"),
                        None => "Give the layout a name (letters, numbers, spaces, - or _)".into(),
                    };
                }
                PaneRequest::Load(name) => {
                    match layouts_dir().and_then(|dir| std::fs::read_to_string(dir.join(format!("{name}.ron"))).ok()).and_then(|t| layout_from_text(&t)) {
                        Some(loaded) => {
                            *dock = loaded;
                            self.popped.clear();
                            self.status = format!("Layout \"{name}\"");
                        }
                        None => self.status = format!("Couldn't read layout \"{name}\""),
                    }
                }
                PaneRequest::Delete(name) => {
                    if let Some(dir) = layouts_dir() {
                        let _ = std::fs::remove_file(dir.join(format!("{name}.ron")));
                    }
                    self.status = format!("Layout \"{name}\" deleted");
                }
                PaneRequest::Show(pane, true) => {
                    if dock.find_tab(&pane).is_none() && !self.popped.contains(&pane) {
                        // A new pane goes next to the viewport.
                        let at = dock.find_tab(&Pane::Viewport).map(|(surface, node, _)| (surface, node));
                        match at {
                            Some((surface, node)) if surface == egui_dock::SurfaceIndex::main() => {
                                dock.main_surface_mut().split_below(node, 0.62, vec![pane]);
                            }
                            _ => dock.push_to_first_leaf(pane),
                        }
                    }
                }
                PaneRequest::Show(pane, false) => {
                    if pane != Pane::Viewport {
                        if let Some(at) = dock.find_tab(&pane) {
                            dock.remove_tab(at);
                        }
                        self.popped.retain(|p| *p != pane);
                    }
                }
                PaneRequest::PopOut(pane) => {
                    if !self.popped.contains(&pane) {
                        if let Some(at) = dock.find_tab(&pane) {
                            dock.remove_tab(at);
                        }
                        self.popped.push(pane);
                    }
                }
            }
        }
    }

    pub(crate) fn show_panes(&mut self, ctx: &egui::Context) {
        if let Some(mut name) = self.layout_name_prompt.take() {
            let (mut keep, mut open) = (true, true);
            own_window(ctx, "save_layout", "Save Layout", [340.0, 130.0], [300.0, 110.0], &mut open, true, |ui| {
                ui.label("Layout name");
                let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text("e.g. Modelling, Rendering"));
                field.request_focus();
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || enter {
                        self.pane_requests.push(PaneRequest::SaveAs(name.clone()));
                        keep = false;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        keep = false;
                    }
                });
                if saved_layouts().iter().any(|n| Some(n) == safe_layout_name(&name).as_ref()) {
                    ui.weak("Saving replaces the layout with this name.");
                }
            });
            // Closing its window cancels it too.
            if keep && open {
                self.layout_name_prompt = Some(name);
            }
        }
        let mut dock = self.dock.take().unwrap_or_else(default_layout);
        self.apply_pane_requests(&mut dock);
        let pop_out = {
            let mut viewer = PaneViewer { app: self, ctx: ctx.clone(), pop_out: None };
            egui_dock::DockArea::new(&mut dock)
                .style(egui_dock::Style::from_egui(ctx.style().as_ref()))
                .show_leaf_collapse_buttons(false)
                .show(ctx, &mut viewer);
            viewer.pop_out
        };
        if let Some(pane) = pop_out {
            self.pane_requests.push(PaneRequest::PopOut(pane));
        }
        // Popped-out panes: real OS windows; closing one puts the pane back in the dock.
        let mut redock = Vec::new();
        for pane in self.popped.clone() {
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{} — ProceduralFX Studio", pane.title()))
                .with_inner_size(match pane {
                    Pane::Viewport => [900.0, 640.0],
                    Pane::Timeline => [900.0, 160.0],
                    _ => [560.0, 520.0],
                });
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("pane", pane.title())), builder, |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| self.pane_ui(ui, ctx, pane));
                if ctx.input(|i| i.viewport().close_requested()) {
                    redock.push(pane);
                }
            });
        }
        for pane in redock {
            self.popped.retain(|p| *p != pane);
            self.pane_requests.push(PaneRequest::Show(pane, true));
        }
        self.open_panes = dock.iter_all_tabs().map(|(_, tab)| *tab).chain(self.popped.iter().copied()).collect();
        // Keep the arrangement for next time.
        if let Some(text) = layout_to_text(&dock) {
            if text != self.saved_layout {
                if let Some(file) = layout_file() {
                    if let Some(dir) = file.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(file, &text);
                }
                self.saved_layout = text;
            }
        }
        self.dock = Some(dock);
    }

    pub(crate) fn parameters_pane(&mut self, ui: &mut egui::Ui) {
        let before = self.selected.and_then(|id| self.graph.nodes.get(&id).map(|n| (id, n.kind.params(), n.xform)));
        TYPED.with(|t| *t.borrow_mut() = None);
        EXPRESSION_TIME.with(|t| t.set(self.time));
        let quick = self.preset_controls(ui);
        // The selected node's expressions, for its sliders to show (see PSlider).
        let fields: Vec<ExpressionField> = self.selected.map_or_else(Vec::new, |id| {
            let names = self.graph.nodes.get(&id).map(|n| n.kind.param_names()).unwrap_or_default();
            self.graph
                .node_channels(id)
                .filter_map(|(c, ch)| {
                    let key = match c.target {
                        ChannelTarget::Param(i) => names.get(i).map(|n| name_key(n)).unwrap_or_default(),
                        ChannelTarget::Xform(i) => XFORM_NAMES.get(i).map(|n| name_key(n)).unwrap_or_default(),
                    };
                    Some(ExpressionField { channel: *c, value: self.graph.channel_value(*c)?, text: ch.expression.clone()?, shown: self.shown_expressions.contains(c), taken: false, key, named: self.named_expressions.contains(c) })
                })
                .collect()
        });
        EXPRESSION_FIELDS.with(|f| *f.borrow_mut() = fields);
        EXPRESSION_EDITS.with(|e| e.borrow_mut().clear());
        EXPRESSION_NAMED.with(|n| n.borrow_mut().clear());
        let edited = self.params_ui(ui) | quick;
        EXPRESSION_FIELDS.with(|f| f.borrow_mut().clear());
        self.named_expressions = EXPRESSION_NAMED.with(|n| std::mem::take(&mut *n.borrow_mut())).into_iter().collect();
        for change in EXPRESSION_EDITS.with(|e| std::mem::take(&mut *e.borrow_mut())) {
            match change {
                ExpressionEdit::Toggle(c) => {
                    if !self.shown_expressions.remove(&c) {
                        self.shown_expressions.insert(c);
                    }
                }
                ExpressionEdit::Set(c, text) => self.set_expression(c, text.trim()),
            }
        }
        if let (Some(typed), Some(before)) = (TYPED.with(|t| t.borrow_mut().take()), before) {
            self.apply_typed(before, typed);
        }
        if edited {
            if self.pause_on_edit {
                self.animate = false;
            }
            self.dirty = true;
        }
        // The shortcut notes sit at the bottom of the pane when there's room, and after
        // the parameters when there isn't (so they never cover them).
        let notes = egui::RichText::new(
            "Network: RMB add node • drag output→input to wire\n\
                     click input to disconnect • dbl-click = display\n\
                     OBJ: Geometry / Light / Camera nodes need no wiring •\n\
                     dbl-click Geometry to enter • U = up • dbl-click Camera = look through • dbl-click Light = on/off\n\
                     MMB hold on a node = Node Info\n\
                     TAB search/add • H home network • T template\n\
                     B bypass • Ctrl+D duplicate • Del delete\n\
                     Ctrl+Z / Ctrl+Shift+Z undo/redo • Space play\n\
                     Selection: All • None • Invert • Grow • Shrink • Boundary • Non-manifold • Isolated • Connected\n\
                     Viewport: LMB orbit • RMB pan • wheel zoom • F frame",
        )
        .small()
        .weak();
        let galley = egui::WidgetText::from(notes.clone()).into_galley(ui, Some(egui::TextWrapMode::Wrap), ui.available_width(), egui::TextStyle::Small);
        let spare = ui.available_height() - galley.size().y - ui.spacing().item_spacing.y * 2.0;
        if spare > 0.0 && spare.is_finite() {
            ui.add_space(spare);
        } else {
            ui.add_space(12.0);
        }
        ui.label(notes);
    }

    pub(crate) fn timeline_pane(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                if ui.button("⏮ Start").on_hover_text("Jump to the playback start").clicked() {
                    self.time = self.playback_start;
                    self.dirty = true;
                }
                if ui.add(egui::Button::new(if self.animate { "⏸ Pause" } else { "▶ Play" })).on_hover_text("Space toggles playback").clicked() {
                    self.animate = !self.animate;
                    self.dirty = true;
                }
                if ui.button("⏭ End").on_hover_text("Jump to the playback end").clicked() {
                    self.time = self.playback_end;
                    self.dirty = true;
                }
                let flipbook = ui
                    .add_enabled(self.flipbook_recording.is_none(), egui::Button::new("🎞 Flipbook"))
                    .on_hover_text("Plays the playback range in the viewport, captures every frame, and plays them back (save as PNG frames or a movie)");
                if flipbook.clicked() {
                    self.start_flipbook();
                }
                ui.label(format!("Time: {:.2}", self.time));
                pfield(ui, pdrag(&mut self.playback_fps).range(1.0..=120.0).speed(0.5).fixed_decimals(0).suffix(" fps"))
                    .on_hover_text(format!("Playback rate. Scenes are {} frames a second, so {} plays in real time; frames that take longer to cook are waited for, never skipped", sims::SIM_FPS, sims::SIM_FPS));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.status.is_empty() {
                        // Only as wide as what's left of the row: a long message is cut short
                        // (with all of it on hover) instead of running over the buttons.
                        editor_helpers::copyable_status(ui, &self.status, true);
                    }
                });
            });

            // Scrub bar: dragging pauses playback and the viewport follows the time.
            ui.horizontal(|ui| {
                let frame = (self.time * sims::SIM_FPS).round() as i64 + 1;
                ui.monospace(format!("Frame {frame:>4}"));
                ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(120.0);
                let scrub = ui
                    .add(pslider(&mut self.time, self.playback_start..=self.playback_end).suffix(" s").show_value(true))
                    .on_hover_text("Drag to scrub the timeline (← / → step one frame)");
                if scrub.drag_started() {
                    self.animate = false;
                }
                // Ticks at the selected node's keys.
                if let Some(id) = self.selected {
                    let span = (self.playback_end - self.playback_start).max(1e-6);
                    let rail = scrub.rect.height() * 0.5;
                    let width = ui.spacing().slider_width - rail * 2.0;
                    let times: Vec<f32> = self.graph.node_channels(id).flat_map(|(_, ch)| ch.keys.iter().map(|k| k.t)).collect();
                    for t in times.into_iter().filter(|t| (self.playback_start..=self.playback_end).contains(t)) {
                        let x = scrub.rect.left() + rail + (t - self.playback_start) / span * width;
                        ui.painter().line_segment(
                            [egui::pos2(x, scrub.rect.top()), egui::pos2(x, scrub.rect.top() + 6.0)],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 200, 60)),
                        );
                    }
                }
                if scrub.changed() {
                    self.dirty = true;
                }
            });
            let typing = ui.ctx().wants_keyboard_input();
            let step = ui.input(|i| {
                (i.key_pressed(egui::Key::ArrowRight) as i32) - (i.key_pressed(egui::Key::ArrowLeft) as i32)
            });
            if step != 0 && !typing {
                self.animate = false;
                self.time = (self.time + step as f32 / sims::SIM_FPS).clamp(self.playback_start, self.playback_end);
                self.dirty = true;
            }

            ui.horizontal(|ui| {
                let range_changed = pfield(ui, pdrag(&mut self.playback_start).speed(0.1).prefix("Start ")).changed()
                    | pfield(ui, pdrag(&mut self.playback_end).speed(0.1).prefix("End   ")).changed();
                if self.playback_end < self.playback_start {
                    self.playback_end = self.playback_start;
                }
                if range_changed {
                    self.time = self.time.clamp(self.playback_start, self.playback_end);
                    self.dirty = true;
                }
                ui.checkbox(&mut self.pause_on_edit, "Pause on edit").on_hover_text("Pause playback whenever the scene is edited: parameters, wiring, flags or gizmo drags");
            });
        });
    }

    // Node Info: the node itself (type, flags, inputs, cook time, errors), then what its
    // geometry holds.
    pub(crate) fn node_info_ui(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let title = node.title(id);
        ui.heading(&title);
        let mut rows: Vec<(String, String)> = vec![("Type".into(), node.type_label())];
        let path = self.graph.path_in_object(id);
        if let Some(object) = self.graph.enclosing_geometry(id) {
            rows.push(("Path".into(), format!("{} / {path}", self.graph.nodes[&object].title(object))));
        }
        let mut flags = Vec::new();
        if self.graph.is_display(id) {
            flags.push("display");
        }
        if node.bypass {
            flags.push("bypassed");
        }
        if self.graph.time_dependent(id) {
            flags.push("time dependent");
        }
        if !flags.is_empty() {
            rows.push(("Flags".into(), flags.join(", ")));
        }
        let inputs = node.kind.num_inputs();
        if inputs > 0 || node.kind.is_multi_input() {
            let connected = self.graph.links.iter().filter(|l| l.to == id).count();
            rows.push(("Inputs".into(), if node.kind.is_multi_input() { format!("{connected} connected (any number)") } else { format!("{connected} of {inputs} connected") }));
        }
        if let Some(stat) = perf::get(id) {
            rows.push(("Cook".into(), format!("{} last, {} on average, {} cooks", perf::ms(stat.last), perf::ms(stat.average()), stat.cooks)));
        }
        egui::Grid::new(("node_info_node", id)).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
            for (label, value) in &rows {
                ui.weak(label);
                ui.label(value);
                ui.end_row();
            }
        });
        if let Some(error) = node_errors(|e| e.get(&id).cloned()) {
            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {error}"));
        }
        // Geometry nodes: what they make at this frame.
        let makes_geometry = !matches!(node.context, AppContext::Out | AppContext::Tasks | AppContext::Img | AppContext::Mat) || node.kind.is_object();
        if !makes_geometry {
            return;
        }
        let fresh = self.info_mesh.as_ref().is_some_and(|(n, t, _)| *n == id && *t == self.time);
        if !fresh {
            let mut cache = self.valid_cache(&self.graph.cook_fingerprints(scene_fingerprint(&self.graph.serialize())));
            let mesh = self.graph.cook(id, self.time, &mut cache);
            self.info_mesh = Some((id, self.time, mesh));
        }
        let Some((_, _, mesh)) = self.info_mesh.as_ref() else { return };
        for section in nodeinfo::summarize(mesh) {
            ui.add_space(6.0);
            ui.strong(section.title);
            egui::Grid::new(("node_info", id, section.title)).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                for (label, value) in &section.rows {
                    ui.weak(label);
                    ui.add(egui::Label::new(value).wrap());
                    ui.end_row();
                }
            });
        }
        let warnings = mesh.validation_warnings();
        if !warnings.is_empty() {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", warnings.join(" · ")));
        }
    }

    // The performance monitor: each node's cook times, slowest first.
    pub(crate) fn performance_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut recording = perf::recording();
            let label = if recording { "⏺ Recording" } else { "⏸ Paused" };
            if ui.toggle_value(&mut recording, label).on_hover_text("Time every node cook").changed() {
                perf::set_recording(recording);
            }
            if ui.button("Clear").on_hover_text("Forget the times so far").clicked() {
                perf::clear();
            }
            ui.checkbox(&mut self.show_cook_times, "Show cook times on nodes");
            let dt = self.frame_time.max(1e-4);
            ui.weak(format!("Interface: {:.0} fps ({:.1} ms a frame)", 1.0 / dt, dt * 1000.0));
        });
        let mut stats: Vec<(NodeId, perf::Stat)> = perf::snapshot().into_iter().filter(|(id, _)| self.graph.nodes.contains_key(id)).collect();
        if stats.is_empty() {
            ui.add_space(8.0);
            ui.weak("Nothing has cooked since the times were cleared. Change a parameter, or play the timeline, and each node's cook time shows here.");
            return;
        }
        let total: f64 = stats.iter().map(|(_, s)| s.total).sum();
        let cooks: u64 = stats.iter().map(|(_, s)| s.cooks).sum();
        ui.label(format!("{} nodes cooked {cooks} times: {} of their own work in all.", stats.len(), perf::ms(total)));
        ui.weak("A node's own time leaves out the time its inputs took, so the slowest node is the one to look at. Click a node to go to it.");
        match self.perf_sort {
            1 => stats.sort_by(|a, b| b.1.last.total_cmp(&a.1.last)),
            2 => stats.sort_by(|a, b| b.1.average().total_cmp(&a.1.average())),
            3 => stats.sort_by(|a, b| b.1.max.total_cmp(&a.1.max)),
            4 => stats.sort_by(|a, b| b.1.cooks.cmp(&a.1.cooks)),
            _ => {}
        }
        let mut jump = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("performance_grid").striped(true).num_columns(8).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.strong("Node");
                ui.strong("Type");
                for (column, label) in [(4, "Cooks"), (1, "Last"), (2, "Average"), (3, "Slowest"), (0, "Total")] {
                    let text = if self.perf_sort == column { format!("{label} ▼") } else { label.to_string() };
                    if ui.add(egui::Label::new(egui::RichText::new(text).strong()).sense(egui::Sense::click())).on_hover_text("Sort by this").clicked() {
                        self.perf_sort = column;
                    }
                }
                ui.strong("Share");
                ui.end_row();
                for (id, stat) in &stats {
                    let node = &self.graph.nodes[id];
                    let path = match self.graph.enclosing_geometry(*id) {
                        Some(object) => format!("{} / {}", self.graph.nodes[&object].title(object), self.graph.path_in_object(*id)),
                        None => node.title(*id),
                    };
                    if ui.link(path).on_hover_text("Go to this node").clicked() {
                        jump = Some(*id);
                    }
                    ui.label(node.type_label());
                    ui.label(stat.cooks.to_string());
                    ui.label(perf::ms(stat.last)).on_hover_text(format!("{} including its inputs", perf::ms(stat.last_with_inputs)));
                    ui.label(perf::ms(stat.average()));
                    ui.label(perf::ms(stat.max));
                    ui.label(perf::ms(stat.total));
                    let share = if total > 0.0 { (stat.total / total) as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(share).desired_width(90.0).text(format!("{:.0}%", share * 100.0)));
                    ui.end_row();
                }
            });
        });
        if let Some(id) = jump {
            let (parent, context) = self.graph.nodes.get(&id).map(|n| (n.parent, n.context)).unwrap_or((None, AppContext::Obj));
            self.enter_network(parent);
            if parent.is_none() {
                self.context = context;
            }
            self.selected = Some(id);
        }
    }

    pub(crate) fn spreadsheet_pane(&mut self, ui: &mut egui::Ui) {
        let source_node = if self.sheet_from_selected { self.selected } else { None };
        if let Some(id) = source_node {
            let fresh = matches!(&self.sheet_cache, Some((cached, generation, _)) if *cached == id && *generation == self.cook_generation);
            if !fresh {
                let mut cache = HashMap::new();
                let mesh = self.graph.cook(id, self.time, &mut cache);
                self.sheet_cache = Some((id, self.cook_generation, mesh));
            }
        }
        let source_label = match source_node {
            Some(id) => self
                .graph
                .nodes
                .get(&id)
                .map(|node| node.title(id))
                .unwrap_or_else(|| format!("node {id}")),
            None if self.sheet_from_selected => "no node selected".into(),
            None => "final geometry in the viewport".into(),
        };
        // Borrow the mesh out of self for the frame instead of cloning it.
        let mesh = match (source_node, self.sheet_cache.as_mut()) {
            (Some(_), Some((_, _, cached))) => std::mem::take(cached),
            _ if self.sheet_from_selected => Mesh::default(),
            _ => std::mem::take(&mut self.current_mesh),
        };
        self.spreadsheet_ui(ui, &mesh, source_node.is_none() && !self.sheet_from_selected, &source_label);
        match (source_node, self.sheet_cache.as_mut()) {
            (Some(_), Some((_, _, cached))) => *cached = mesh,
            _ if self.sheet_from_selected => {}
            _ => self.current_mesh = mesh,
        }
    }

    // `selectable`: rows map to the viewport geometry, so clicking them selects components.
    pub(crate) fn spreadsheet_ui(&mut self, ui: &mut egui::Ui, mesh: &Mesh, selectable: bool, source_label: &str) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Geometry");
            ui.selectable_value(&mut self.sheet_from_selected, false, "Displayed output");
            ui.selectable_value(&mut self.sheet_from_selected, true, "Selected node")
                .on_hover_text("Inspect the output of the node selected in the network, e.g. a wrangle mid-chain");
            ui.weak(source_label);
        });
        ui.horizontal_wrapped(|ui| {
            for domain in [SheetDomain::Points, SheetDomain::Vertices, SheetDomain::Primitives, SheetDomain::Detail] {
                let label = match domain {
                    SheetDomain::Points => format!("Points ({})", mesh.vertices.len()),
                    SheetDomain::Vertices => format!("Vertices ({})", mesh.indices.len() * 3),
                    SheetDomain::Primitives => format!("Primitives ({})", mesh.indices.len()),
                    SheetDomain::Detail => "Detail".to_string(),
                };
                ui.selectable_value(&mut self.sheet_domain, domain, label);
            }
        });

        let domain = self.sheet_domain;
        let columns = sheet_columns(mesh, domain);
        let normals = if columns.iter().any(|c| matches!(c.source, SheetSource::ComputedNormal(_))) {
            mesh.compute_normals()
        } else {
            Vec::new()
        };
        let count = domain.count(mesh);
        let polys = SheetPolys::new(mesh);
        let selection: Option<&Vec<usize>> = match domain {
            SheetDomain::Points if selectable => Some(&self.selected_vertices),
            SheetDomain::Primitives if selectable => Some(&self.selected_triangles),
            _ => None,
        };
        // Selected primitives are triangles; their rows are polygons.
        let selected: HashSet<usize> = selection
            .map(|s| s.iter().map(|&i| if domain == SheetDomain::Primitives { polys.of.get(i).copied().unwrap_or(i) } else { i }).collect())
            .unwrap_or_default();
        let filter = self.sheet_selected_only && selection.is_some();
        let rows: Vec<usize> = if domain == SheetDomain::Detail {
            if columns.is_empty() { Vec::new() } else { vec![0] }
        } else if filter {
            let mut rows: Vec<usize> = selected.iter().copied().filter(|i| *i < count).collect();
            rows.sort_unstable();
            rows
        } else {
            (0..count).collect()
        };

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(selection.is_some(), |ui| {
                ui.checkbox(&mut self.sheet_selected_only, "Selected only")
                    .on_hover_text("Points and primitives of the displayed output; click rows to select them (Shift adds)");
            });
            if ui.add_enabled(!rows.is_empty(), egui::Button::new("Copy CSV")).on_hover_text("Copy the rows shown as comma-separated values").clicked() {
                let quote = |text: &str| if text.contains(',') || text.contains('"') { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() };
                let mut csv = columns.iter().map(|c| quote(&c.header)).collect::<Vec<_>>().join(",");
                for &row in &rows {
                    csv.push('\n');
                    csv.push_str(&columns.iter().map(|c| quote(&sheet_cell(mesh, &normals, &polys, &c.source, row))).collect::<Vec<_>>().join(","));
                }
                ui.ctx().copy_text(csv);
                self.status = format!("Copied {} rows", rows.len());
            }
            if let Some((lo, hi)) = mesh.bounds() {
                ui.weak(format!("Bounds ({:.2}, {:.2}, {:.2}) to ({:.2}, {:.2}, {:.2})", lo.x, lo.y, lo.z, hi.x, hi.y, hi.z));
            }
            let warnings = mesh.validation_warnings();
            if !warnings.is_empty() {
                ui.colored_label(egui::Color32::YELLOW, format!("⚠ {}", warnings.join(" · ")));
            }
        });

        egui::CollapsingHeader::new("Attribute summary").id_salt("sheet_summary").show(ui, |ui| {
            egui::Grid::new("sheet_summary_grid").striped(true).num_columns(4).show(ui, |ui| {
                for header in ["Attribute", "Class", "Size", "Range"] {
                    ui.strong(header);
                }
                ui.end_row();
                for summary_domain in [SheetDomain::Points, SheetDomain::Vertices, SheetDomain::Primitives, SheetDomain::Detail] {
                    let class = match summary_domain {
                        SheetDomain::Points => "point",
                        SheetDomain::Vertices => "vertex",
                        SheetDomain::Primitives => "primitive",
                        SheetDomain::Detail => "detail",
                    };
                    for attribute in sheet_attributes(mesh, summary_domain) {
                        let values = &mesh.point_attributes[&attribute.key];
                        // Text: how many different values, and a few of them.
                        let ranges = if crate::strings::is_string_key(&attribute.key) {
                            let mut texts: Vec<String> = values.iter().map(|v| crate::strings::text(*v).to_string()).collect();
                            texts.sort();
                            texts.dedup();
                            let shown: Vec<String> = texts.iter().take(4).map(|t| format!("{t:?}")).collect();
                            format!("{} different: {}{}", texts.len(), shown.join(" "), if texts.len() > 4 { " …" } else { "" })
                        } else {
                            (0..attribute.components.min(4))
                            .map(|c| {
                                let lane = values.iter().skip(c).step_by(attribute.components);
                                let (lo, hi) = lane.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
                                if lo > hi { "-".to_string() } else { format!("{lo:.3} … {hi:.3}") }
                            })
                            .collect::<Vec<_>>()
                            .join("   ")
                        };
                        let name = ui.monospace(&attribute.name);
                        if let Some(meaning) = attribute_meaning(&attribute.name) {
                            name.on_hover_text(meaning);
                        }
                        ui.label(class);
                        ui.label(format!("{}{}", attribute.components, if attribute.size_ok { "" } else { " ⚠" }));
                        ui.monospace(ranges);
                        ui.end_row();
                    }
                }
            });
        });
        ui.separator();

        if rows.is_empty() {
            ui.weak(match domain {
                SheetDomain::Detail => "No detail attributes. Create one with a Detail wrangle, e.g. f@total = npoints(0);",
                _ if filter => "Nothing selected.",
                _ => "No elements.",
            });
            return;
        }
        let shift = ui.input(|i| i.modifiers.shift);
        let mut clicked = None;
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            let mut table = egui_extras::TableBuilder::new(ui)
                .id_salt(("geometry_sheet", domain as u8, columns.len()))
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .min_scrolled_height(0.0)
                .max_scroll_height(f32::INFINITY);
            for (i, column) in columns.iter().enumerate() {
                table = table.column(if i == 0 && matches!(column.source, SheetSource::Index) {
                    egui_extras::Column::auto().at_least(44.0)
                } else {
                    egui_extras::Column::initial(74.0).at_least(36.0).clip(true)
                });
            }
            table
                .header(20.0, |mut header| {
                    for column in &columns {
                        header.col(|ui| {
                            ui.strong(&column.header).on_hover_text(&column.tooltip);
                        });
                    }
                })
                .body(|body| {
                    body.rows(18.0, rows.len(), |mut row| {
                        let element = rows[row.index()];
                        row.set_selected(selected.contains(&element));
                        for column in &columns {
                            row.col(|ui| {
                                ui.label(egui::RichText::new(sheet_cell(mesh, &normals, &polys, &column.source, element)).monospace());
                            });
                        }
                        if row.response().clicked() {
                            clicked = Some(element);
                        }
                    });
                });
        });

        if let (Some(element), true) = (clicked, selection.is_some()) {
            let (list, mode) = match domain {
                SheetDomain::Points => (&mut self.selected_vertices, SelectionMode::Vertex),
                _ => (&mut self.selected_triangles, SelectionMode::Triangle),
            };
            // A polygon row selects all its triangles.
            let members: Vec<usize> = match domain {
                SheetDomain::Primitives => polys.runs.get(element).map_or_else(Vec::new, |run| run.clone().collect()),
                _ => vec![element],
            };
            if shift {
                if selected.contains(&element) {
                    list.retain(|i| !members.contains(i));
                } else {
                    list.extend(members);
                }
            } else {
                *list = members;
            }
            if self.selection_mode != mode {
                self.selection_mode = mode;
                self.selected_edges.clear();
                match mode {
                    SelectionMode::Vertex => self.selected_triangles.clear(),
                    _ => self.selected_vertices.clear(),
                }
            }
        }
    }

    // Applies the animation at the current time; an animated camera being looked through drives the view.
    pub(crate) fn begin_animation_frame(&mut self, ctx: &egui::Context) {
        self.graph.apply_animation(self.time);
        if let Some(request) = self.look_through_request.take() {
            self.look_through(request);
        }
        if self.looking_through {
            // Anything that moved the view since the camera set it (orbit, pan, zoom,
            // framing) leaves the camera; otherwise the camera, edits and animation
            // included, keeps driving the view.
            let moved = self.camera_view.is_some_and(|set| set.iter().zip(self.view_state()).any(|(a, b)| (a - b).abs() > 1e-5));
            let camera = self.active_camera;
            if moved || !camera.is_some_and(|c| self.view_from_camera(c)) {
                self.look_through(None);
                if moved {
                    self.status = "Left the camera's view (the camera is unchanged)".into();
                }
            } else {
                self.camera_view = Some(self.view_state());
            }
        } else if let Some(camera) = self.active_camera.filter(|c| self.graph.is_animated(*c)) {
            self.view_from_camera(camera);
        }
        self.anim_snapshot = self
            .selected
            .and_then(|id| self.graph.nodes.get(&id).map(|n| (id, n.kind.params(), n.xform)));
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::K) && !i.modifiers.command) {
            self.key_selected();
        }
    }

    // Keys this frame's edits: animated parameters always, anything else with Auto Key.
    pub(crate) fn end_animation_frame(&mut self) {
        let mut changed = self.graph.commit_animation_edits(self.time, self.auto_key);
        if changed && !self.auto_key && !self.graph.pending.is_empty() {
            self.status = "Changed without a key (Auto Key is off): press K to key it, or it goes back to the animation on another frame".into();
        }
        if self.auto_key {
            if let Some((id, params, xform)) = self.anim_snapshot.take() {
                let targets = self.graph.nodes.get(&id).map(|n| n.kind.animatable()).unwrap_or_default();
                for (target, _) in targets {
                    let channel_id = ChannelId { node: id, target };
                    if self.graph.channels.contains_key(&channel_id) {
                        continue;
                    }
                    let before = match target {
                        ChannelTarget::Param(i) => params.get(i).copied(),
                        ChannelTarget::Xform(i) => Some(xform_component(&xform, i)),
                    };
                    let now = self.graph.channel_value(channel_id);
                    if let (Some(before), Some(now)) = (before, now) {
                        if (now - before).abs() > 1e-4 * before.abs().max(1.0) {
                            self.graph.set_key(channel_id, self.time);
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    pub(crate) fn frame_number(&self) -> i64 {
        (self.time * sims::SIM_FPS).round() as i64 + 1
    }

    // Keys the selected node at the current time: an object's transform, or all its animatable parameters.
    pub(crate) fn key_selected(&mut self) {
        let Some(id) = self.selected else {
            self.status = "Select a node to key".into();
            return;
        };
        let Some(node) = self.graph.nodes.get(&id) else { return };
        let mut targets = node.kind.animatable();
        if matches!(node.kind, NodeKind::Light { .. } | NodeKind::Camera { .. }) {
            targets.truncate(6);
        }
        if targets.is_empty() {
            self.status = format!("{} has no parameters that can be animated", node.title(id));
            return;
        }
        let label = node.title(id);
        for (target, _) in &targets {
            self.graph.set_key(ChannelId { node: id, target: *target }, self.time);
        }
        self.dirty = true;
        self.status = format!("Keyed {} channels of {label} at frame {}", targets.len(), self.frame_number());
    }

    pub(crate) fn graph_editor_pane(&mut self, ui: &mut egui::Ui) {
        // The selected node's curves when it is animated, otherwise every animated channel.
        let focus = self.selected.filter(|id| self.graph.is_animated(*id));
        let mut wanted: HashMap<ChannelId, (String, String)> = HashMap::new();
        for channel_id in self.graph.channels.keys() {
            let Some(node) = self.graph.nodes.get(&channel_id.node) else { continue };
            if focus.is_some_and(|f| f != channel_id.node) {
                continue;
            }
            let group = node.title(channel_id.node);
            let name = node
                .kind
                .animatable()
                .into_iter()
                .find(|(t, _)| *t == channel_id.target)
                .map_or_else(|| format!("{:?}", channel_id.target), |(_, n)| n);
            wanted.insert(*channel_id, (format!("{group} · {name}"), group));
        }
        let mut rows: Vec<anim::ChannelRow> = self
            .graph
            .channels
            .iter_mut()
            .filter_map(|(id, channel)| wanted.get(id).map(|(label, group)| anim::ChannelRow { label: label.clone(), group: group.clone(), channel }))
            .collect();
        let result = self.graph_editor.ui_inside(ui, &mut rows, self.time, (self.playback_start, self.playback_end), sims::SIM_FPS);
        drop(rows);
        if result.changed {
            self.graph.channels.retain(|_, channel| !channel.is_empty());
            self.dirty = true;
        }
        if let Some(t) = result.scrub {
            self.time = t.clamp(self.playback_start, self.playback_end.max(self.playback_start));
            self.animate = false;
            self.dirty = true;
        }
    }

    // Prim hierarchy of a LOP's stage, with the picked prim's properties.
    pub(crate) fn scene_graph_ui(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let fresh = matches!(&self.lop_stage_cache, Some((node, generation, _)) if *node == id && *generation == self.cook_generation);
        if !fresh {
            let stage = self.graph.cook_stage(id, self.time, &mut HashMap::new());
            self.lop_stage_cache = Some((id, self.cook_generation, stage));
        }
        let Some((_, _, stage)) = self.lop_stage_cache.clone() else { return };
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Scene Graph");
            ui.weak(format!("{} prims", stage.prims.len()));
        });
        egui::ScrollArea::vertical().id_salt("scene_graph").max_height(240.0).show(ui, |ui| {
            if stage.prims.is_empty() {
                ui.weak("Empty stage. Add SOP Import, Reference or Primitive nodes.");
            }
            let roots: Vec<String> = stage.prims.keys().filter(|p| usd::parent_path(p) == Some("/")).cloned().collect();
            for root in roots {
                self.prim_tree_row(ui, &stage, &root, 0);
            }
        });
        let Some(path) = self.stage_selected_prim.clone() else { return };
        let Some(prim) = stage.prims.get(&path) else { return };
        ui.separator();
        egui::Grid::new("prim_properties").num_columns(2).striped(true).show(ui, |ui| {
            ui.weak("Path");
            ui.horizontal(|ui| {
                ui.monospace(&path);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(path.clone());
                }
            });
            ui.end_row();
            ui.weak("Type");
            ui.label(&prim.type_name);
            ui.end_row();
            let world = stage.world_xform(&path);
            ui.weak("World position");
            ui.monospace(format!("{:.3}, {:.3}, {:.3}", world[3][0], world[3][1], world[3][2]));
            ui.end_row();
            ui.weak("State");
            ui.label(match (prim.active, prim.visible) {
                (false, _) => "inactive",
                (true, false) => "invisible",
                _ => "active, visible",
            });
            ui.end_row();
            if let Some(mesh) = &prim.mesh {
                if prim.type_name == "Mesh" {
                    ui.weak("Geometry");
                    ui.label(format!("{} points, {} primitives", mesh.vertices.len(), poly::count(mesh)));
                    ui.end_row();
                    let mut names: Vec<&String> = mesh.point_attributes.keys().filter(|k| !poly::is_internal(k)).collect();
                    names.sort();
                    ui.weak("Primvars");
                    ui.label(names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", "));
                    ui.end_row();
                }
            }
            if let Some(material) = stage.bound_material(&path) {
                ui.weak("Material");
                ui.monospace(material);
                ui.end_row();
            }
            for (name, value) in &prim.attrs {
                ui.weak(name);
                ui.monospace(match value {
                    usd::AttrValue::Float(f) => format!("{f:.4}"),
                    usd::AttrValue::Vec3(v) => format!("{:.3}, {:.3}, {:.3}", v[0], v[1], v[2]),
                    usd::AttrValue::Token(t) => t.clone(),
                    usd::AttrValue::Bool(b) => b.to_string(),
                });
                ui.end_row();
            }
        });
    }

    pub(crate) fn prim_tree_row(&mut self, ui: &mut egui::Ui, stage: &usd::Stage, path: &str, depth: usize) {
        let Some(prim) = stage.prims.get(path) else { return };
        let children: Vec<String> = stage.children(path).map(|(p, _)| p.clone()).collect();
        let selected = self.stage_selected_prim.as_deref() == Some(path);
        let mut text = egui::RichText::new(format!("{}  ", usd::prim_name(path)));
        if !prim.active {
            text = text.strikethrough().weak();
        } else if !prim.visible {
            text = text.weak();
        }
        let row = |ui: &mut egui::Ui, this: &mut Self| {
            ui.horizontal(|ui| {
                if ui.selectable_label(selected, text.clone()).clicked() {
                    this.stage_selected_prim = Some(path.to_string());
                }
                ui.weak(&prim.type_name);
            });
        };
        if children.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                row(ui, self);
            });
            return;
        }
        let state_id = ui.make_persistent_id(("scene_graph_prim", path));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), state_id, depth < 2)
            .show_header(ui, |ui| row(ui, self))
            .body(|ui| {
                for child in &children {
                    self.prim_tree_row(ui, stage, child, depth + 1);
                }
            });
    }

    // The IMG viewer: the COP network's output, fitted to the pane, over a checkerboard.
    pub(crate) fn image_view(&mut self, ui: &egui::Ui, response: &egui::Response, painter: &egui::Painter) {
        let rect = response.rect;
        let Some(img) = cops::Image::from_mesh(&self.current_mesh) else {
            let message = if self.view_displays().is_empty() { "Add a COP Network, open it and build a composite" } else { "Nothing to show: check the display node's errors" };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, message, egui::FontId::proportional(15.0), egui::Color32::from_gray(170));
            return;
        };
        let raw = &self.current_mesh.point_attributes[cops::RGBA];
        let key = (img.w, img.h, raw.as_ptr() as usize, raw.len());
        if self.cop_texture.as_ref().is_none_or(|(k, _)| *k != key) {
            let pixels: Vec<egui::Color32> = img
                .px
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let (x, y) = (i % img.w, i / img.w);
                    let check = if (x / 8 + y / 8) % 2 == 0 { 0.22 } else { 0.32 };
                    let a = p[3].clamp(0.0, 1.0);
                    let c = |k: usize| ((cops::linear_to_srgb(p[k]) * a + check * (1.0 - a)).clamp(0.0, 1.0) * 255.0) as u8;
                    egui::Color32::from_rgb(c(0), c(1), c(2))
                })
                .collect();
            let image = egui::ColorImage { size: [img.w, img.h], pixels, source_size: egui::vec2(img.w as f32, img.h as f32) };
            let texture = ui.ctx().load_texture("cop_view", image, egui::TextureOptions::LINEAR);
            self.cop_texture = Some((key, texture));
        }
        let Some((_, texture)) = &self.cop_texture else { return };
        let scale = ((rect.width() - 24.0) / img.w as f32).min((rect.height() - 48.0) / img.h as f32).max(0.01);
        let shown = egui::Rect::from_center_size(rect.center(), egui::vec2(img.w as f32, img.h as f32) * scale);
        painter.image(texture.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        painter.rect_stroke(shown, 0.0, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(90)), egui::StrokeKind::Outside);
        let mut info = format!("{} × {}", img.w, img.h);
        if let Some(p) = response.hover_pos().filter(|p| shown.contains(*p)) {
            let (x, y) = (((p.x - shown.min.x) / scale) as usize, ((p.y - shown.min.y) / scale) as usize);
            let v = img.px[y.min(img.h - 1) * img.w + x.min(img.w - 1)];
            info += &format!("   ({x}, {y})  R {:.3}  G {:.3}  B {:.3}  A {:.3}", v[0], v[1], v[2], v[3]);
        }
        painter.text(rect.left_top() + egui::vec2(10.0, 10.0), egui::Align2::LEFT_TOP, info, egui::FontId::monospace(12.0), egui::Color32::from_gray(210));
    }
}
