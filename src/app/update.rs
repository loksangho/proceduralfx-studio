// The update loop: what the app does each frame (eframe::App).

use crate::*;

impl eframe::App for ProceduralFxStudioApp {
    // A clean quit (saved, or "Don't Save"): nothing to recover next time.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(session) = &mut self.recovery {
            session.end();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

impl ProceduralFxStudioApp {
    // One frame of the whole app. Separate from eframe's update so tests can drive the
    // real UI without a window (see app_tests::gui_flows).
    pub(crate) fn frame(&mut self, ctx: &egui::Context) {
        // The GPU refused the viewport's work (or ran out of memory): carry on with the CPU.
        if let Some(error) = self.gpu_viewport.as_ref().and_then(|gpu| gpu.failure()) {
            self.gpu_fallback(error);
        }
        // What the status line says goes in the log (see applog.rs), but not every frame's.
        if self.status != self.logged_status {
            if applog::worth_logging(&self.status) {
                log::info!("{}", self.status);
            }
            self.logged_status = self.status.clone();
        }
        // A dialog in its own window locks this one.
        lock_for_modal(ctx);
        // Windows rendering set-up: show what the helper is doing.
        if let Some(rx) = &self.render_setup {
            let mut lines = Vec::new();
            let mut finished = false;
            loop {
                match rx.try_recv() {
                    Ok(line) => lines.push(line),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
            if let Some(line) = lines.last() {
                self.status = wslrender::progress(line).unwrap_or_else(|| line.trim_start_matches("[pfx-render] ").replacen("error ", "Error: ", 1));
            }
            for line in lines {
                if line == "[pfx-render] needs-admin" {
                    self.render_setup_needs_admin = true;
                    self.status = "Rendering on Windows needs WSL: see Set Up Rendering on Windows".into();
                    continue;
                }
                // Download progress replaces the previous progress line.
                let progress = line.starts_with("[pfx-render] download ");
                if progress && self.render_setup_log.last().is_some_and(|l| l.starts_with("[pfx-render] download ")) {
                    self.render_setup_log.pop();
                }
                self.render_setup_log.push(line);
            }
            if finished {
                self.render_setup = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        if self.render_setup_open {
            let mut open = true;
            let running = self.render_setup.is_some();
            own_window(ctx, "render_setup", "Set Up Rendering on Windows", [500.0, 420.0], [400.0, 300.0], &mut open, true, |ui| {
                ui.label("Installs WSL containers if they're missing, then downloads MoonRay (about 1.3 GB, once).");
                ui.separator();
                egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.render_setup_log {
                        let text = wslrender::progress(line).unwrap_or_else(|| line.trim_start_matches("[pfx-render] ").to_string());
                        if line.contains("] error ") {
                            ui.colored_label(egui::Color32::from_rgb(240, 110, 110), text.replacen("error ", "", 1));
                        } else {
                            ui.label(text);
                        }
                    }
                });
                if running {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("Working… a Windows permission prompt may appear.");
                    });
                }
                let mut next = None;
                if self.render_setup_needs_admin && !running {
                    ui.separator();
                    ui.label(egui::RichText::new("This PC needs WSL (the Windows Subsystem for Linux) to render.").strong());
                    ui.label("Installing it needs an administrator, once. Turning on Virtual Machine Platform also needs a restart. After that, downloading MoonRay and rendering work without administrator rights.");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Install now").on_hover_text("Windows asks for an administrator's approval").clicked() {
                            next = Some(true);
                        }
                        if ui.button("Copy the command for an administrator").on_hover_text(format!("Run in a Terminal opened as administrator: {WSL_ADMIN_COMMAND}")).clicked() {
                            ui.ctx().copy_text(WSL_ADMIN_COMMAND.to_string());
                            self.render_setup_log.push(format!("[pfx-render] status copied: {WSL_ADMIN_COMMAND} (an administrator runs it once in a Terminal opened as administrator, then restarts)"));
                        }
                        if ui.button("Check again").on_hover_text("After WSL was installed (and Windows restarted)").clicked() {
                            next = Some(false);
                        }
                    });
                }
                if let (Some(install), Some(helper)) = (next, wslrender::helper()) {
                    self.render_setup_needs_admin = false;
                    self.render_setup = Some(start_render_setup(helper, install));
                }
            });
            if !open {
                self.render_setup_open = false;
            }
        }
        let dt = ctx.input(|i| i.unstable_dt);
        if dt.is_finite() && dt > 0.0 && dt < 1.0 {
            self.frame_time += (dt - self.frame_time) * 0.1;
        }
        self.shortcuts(ctx);

        // Keep the requested simulation time stable until the asynchronous
        // FLIP cook completes. Otherwise playback changes the cache key every
        // frame and cancels the job before it can produce a result.
        if self.animate && self.simulation_rx.is_none() {
            // Whole frames at the playback rate. A slow cook holds playback back
            // rather than making it skip frames, so at most one frame is owed.
            if dt.is_finite() && dt > 0.0 {
                self.playback_due = (self.playback_due + dt.min(0.25) * self.playback_fps.clamp(1.0, 120.0)).min(1.0);
            }
            if self.playback_due >= 1.0 {
                self.playback_due -= 1.0;
                let frame = (self.time * sims::SIM_FPS).round() + 1.0;
                self.time = frame / sims::SIM_FPS;
                if self.time > self.playback_end + 1e-4 {
                    self.time = self.playback_start;
                }
                self.dirty = true;
            }
            ctx.request_repaint();
        }

        self.begin_animation_frame(ctx);
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| self.toolbar(ui));
        self.updater.poll(ctx);
        if let Some((id, target)) = self.group_pick_request.take() {
            self.start_group_pick(id, target);
        }
        if let Some(domain) = self.group_pick_domain.take() {
            self.set_group_pick_domain(domain);
        }
        if let Some(accept) = self.group_pick_finish.take() {
            self.finish_group_pick(accept);
        }
        if self.group_pick.is_some() {
            let (enter, escape) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if enter || escape {
                self.finish_group_pick(enter);
            }
            // Picking is tied to the node it was started from.
            if self.group_pick.as_ref().is_some_and(|p| Some(p.node) != self.selected) {
                self.finish_group_pick(false);
            }
        }
        if let Some(id) = self.group_choices_request.take() {
            let input = self.graph.input_of(id, 0).map(|i| self.graph.cook(i, self.time, &mut HashMap::new())).unwrap_or_default();
            let (points, prims) = groups::names(&input);
            self.group_choices = Some((id, points, prims));
        }
        if let Some(id) = self.paint_fill.take() {
            let count = self.graph.input_of(id, 0).map_or(0, |i| self.graph.cook(i, self.time, &mut HashMap::new()).vertices.len());
            if let Some(NodeKind::AttributePaint { values, settings, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                *values = vec![settings.value; count];
                self.dirty = true;
            }
        }
        if self.updater.show(ctx) == updater::UpdaterAction::SaveScene {
            self.save_scene();
        }
        self.update_window_title(ctx);
        self.confirm_quit(ctx);
        self.autosave(ctx);
        self.recovery_prompt(ctx);

        self.poll_moonray_render();
        self.advance_sequence();
        if let Some(id) = self.top_cook_request.take() {
            self.start_top_cook(id);
        }
        if self.top_run.is_some() || self.top_cancel_request {
            self.advance_top_run();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some((id, range)) = self.cop_write_request.take() {
            self.status = self.write_cop_output(id, range).unwrap_or_else(|e| format!("File Output failed: {e}"));
        }

        self.live_sheet_step(ctx);
        self.flipbook_step(ctx);
        if self.dirty || self.simulation_rx.is_some() {
            self.rebuild_graph();
            self.dirty = false;
        }
        self.flipbook_request_capture(ctx);
        if self.simulation_rx.is_some() || self.moonray_rx.is_some() || self.sequence.is_some() {
            ctx.request_repaint();
        }

        // The panes: a dock the user can rearrange, split, float and pop out (see Pane).
        self.show_panes(ctx);
        if let Some((scene, preset)) = self.assistant.show(ctx, &self.graph, self.time) {
            self.graph = scene;
            self.dirty = true;
            self.status = "AI changes applied (Ctrl+Z undoes them)".into();
            // Started from a preset: its range, sheet settings and (newest) camera come too.
            if let Some(preset) = preset {
                let camera = self.graph.nodes.iter().filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. }) && n.name == "sheet_camera").map(|(id, _)| *id).max();
                self.use_preset_settings(preset, camera);
            }
        }
        if let Some(viewer) = self.flipbook_viewer.as_mut() {
            viewer.show(ctx);
            if !viewer.open {
                self.flipbook_viewer = None;
            }
        }
        if self.dirty {
            ctx.request_repaint();
        }

        self.use_requested_camera();
        if let Some((id, range)) = self.file_export_request.take() {
            self.status = self.write_file_export(id, range).unwrap_or_else(|e| format!("File Export failed: {e}"));
        }
        if let Some(id) = self.usd_export_request.take() {
            if let Some(NodeKind::LopExport { file }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) {
                let stage = self.graph.cook_stage(id, self.time, &mut HashMap::new());
                self.status = match usd::save(&stage, file.trim()) {
                    Ok(()) => format!("Saved USD ({} prims) to {}", stage.prims.len(), file.trim()),
                    Err(e) => format!("USD export failed: {e}"),
                };
            }
        }
        if let Some(id) = self.pyro_fit_request.take() {
            let source = self.graph.cook_fresh(self.graph.input_of(id, 0), 0.0);
            match (source.bounds(), self.graph.nodes.get_mut(&id).map(|n| &mut n.kind)) {
                (Some((lo, hi)), Some(NodeKind::Pyro(set))) => {
                    let extent = hi.sub(lo);
                    let width = (extent.x.max(extent.z) * 3.0).max(2.0);
                    let height = (extent.y * 2.0 + width * 1.5).max(3.0);
                    set.size = [width.max(extent.x * 3.0), height, width.max(extent.z * 3.0)];
                    set.center = [(lo.x + hi.x) * 0.5, lo.y - 0.1 * height + height * 0.5, (lo.z + hi.z) * 0.5];
                    self.dirty = true;
                }
                _ => self.status = "Pyro: connect a source to input 1 first".into(),
            }
        }
        if let Some(id) = self.loop_search_request.take() {
            if let Some(&NodeKind::MakeLoop { start, length, .. }) = self.graph.nodes.get(&id).map(|n| &n.kind) {
                let (best, diff) = looping::best_length(|t| self.graph.cook_input_fresh(id, 0, t), start, length * 0.5, length * 2.0, length);
                if let Some(NodeKind::MakeLoop { length, blend, .. }) = self.graph.nodes.get_mut(&id).map(|n| &mut n.kind) {
                    *length = best;
                    *blend = blend.min(best);
                }
                self.playback_end = start + best - 1.0 / sims::SIM_FPS;
                self.dirty = true;
                self.status = format!("Make Loop: {} frames ({best:.2} s) loops best, its end {:.0}% different from its start. The playback range now ends there.", (best * sims::SIM_FPS).round(), diff * 100.0);
            }
        }
        if self.moonray_render_requested {
            self.moonray_render_requested = false;
            self.start_moonray_render();
        }

        self.render_settings_window(ctx);
        self.sprite_sheet_window(ctx);
        self.vat_window(ctx);
        self.viewer.ui(ctx);


        // Node Info: a pinned window, and the popup while the middle button is held on a node.
        if let Some(node_id) = self.inspect_node.filter(|id| self.graph.nodes.contains_key(id)) {
            let mut open = true;
            let title = self.graph.nodes[&node_id].title(node_id);
            own_window(ctx, "node_info_window", &format!("Node Info · {title}"), [380.0, 560.0], [280.0, 200.0], &mut open, false, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| self.node_info_ui(ui, node_id));
            });
            if !open {
                self.inspect_node = None;
            }
        } else {
            self.inspect_node = None;
        }
        if let Some((node_id, at)) = self.info_hold {
            if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)) || !self.graph.nodes.contains_key(&node_id) {
                self.info_hold = None;
            } else {
                egui::Area::new(egui::Id::new("node_info_hold")).order(egui::Order::Tooltip).fixed_pos(at + egui::vec2(14.0, 14.0)).show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(380.0);
                        self.node_info_ui(ui, node_id);
                    });
                });
            }
        }

        self.end_animation_frame();

        // Pause on edit: any change to the scene (parameters, wiring, flags, gizmo drags, undo)
        // stops playback, not only edits in the parameter panel.
        let signature = self.graph.content_signature(self.active_camera);
        if self.last_content_signature.is_some_and(|last| last != signature) && self.pause_on_edit && self.animate {
            self.animate = false;
            self.status = "Playback paused: the scene was edited (turn off Pause on edit to keep playing)".into();
        }
        self.last_content_signature = Some(signature);

        // Undo snapshots: commit once the mouse is released (one step per drag)
        if !ctx.input(|i| i.pointer.any_down()) {
            let s = self.graph.serialize();
            if s != self.last_state {
                self.undo.push(std::mem::replace(&mut self.last_state, s));
                self.redo.clear();
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
            }
        }
        // A dialog opened in its own window this frame: cover this one until it closes.
        if let Some((_, title)) = ctx.data(|d| d.get_temp::<(egui::ViewportId, String)>(modal_key())) {
            modal_cover(ctx, &title);
        }
    }

    // Stops using the GPU viewport after `error`, and says so: the viewport is drawn on the
    // CPU from now on (slower, without volumes); what needs the GPU says it does.
    pub(crate) fn gpu_fallback(&mut self, error: String) {
        log::error!("GPU viewport given up, drawing on the CPU: {error}");
        self.gpu_viewport = None;
        let first_line = error.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("an error").to_owned();
        self.status = format!("The GPU couldn't draw the viewport ({first_line}), so it's drawn on the CPU now. Sprite sheets and real-time renders need the GPU: restart the app to try it again.");
        self.gpu_failure = Some(error);
        self.dirty = true;
    }
}
