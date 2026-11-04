// Scene files: opening, presets, saving, autosave and crash recovery, quitting.

use crate::*;

impl ProceduralFxStudioApp {
    pub(crate) fn open_scene_dialog(&mut self) {
        let mut dialog = FileDialog::new().add_filter("ProceduralFX Scene", &[SCENE_EXTENSION, "mhip", "json"]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.pick_file() {
            Some(path) => self.open_scene(path),
            None => self.status = "Load canceled".into(),
        }
    }

    pub(crate) fn open_scene(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(s) if scene_problem(&s).is_some() => {
                self.status = format!("Can't open {}: {}", path.display(), scene_problem(&s).unwrap_or_default());
            }
            Ok(s) => {
                self.undo.push(self.last_state.clone());
                self.use_saved_timeline(&s);
                let unread = unread_nodes(&s, &Graph::deserialize(&s));
                self.load_state(s);
                self.mark_saved();
                self.status = match unread {
                    0 => format!("Loaded {}", path.display()),
                    n => format!(
                        "Loaded {}, but {n} node{} couldn't be read (made by a newer version, or damaged). Saving over the file would lose them.",
                        path.display(),
                        if n == 1 { "" } else { "s" }
                    ),
                };
                push_recent_scene(&mut self.recent_scenes, path.clone());
                save_recent_scenes(&self.recent_scenes);
                self.scene_path = Some(path);
            }
            Err(e) => {
                self.status = format!("Load failed: {e}");
                if !path.exists() {
                    self.recent_scenes.retain(|p| p != &path);
                    save_recent_scenes(&self.recent_scenes);
                }
            }
        }
    }

    // Starts a new scene from a starter effect: its nodes, the playback range it's made for,
    // its camera as the render camera, and sprite-sheet settings to suit (a seamless loop
    // for looping effects). Undoable, like opening a file; saving asks where.
    pub(crate) fn load_preset(&mut self, preset: &presets::Preset) {
        self.undo.push(self.last_state.clone());
        self.load_state(preset.scene.to_owned());
        self.scene_path = None;
        self.selected = None;
        self.current_network = None;
        let camera = self.graph.nodes.iter().filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. })).map(|(id, _)| *id).min();
        self.use_preset_settings(preset, camera);
        let export = if preset.vertex_animation { "Render → Export Vertex Animation… exports it for a 3D game" } else { "Render → Export Sprite Sheet… exports it for a game" };
        self.status = format!("New scene from the {} preset. {export}.", preset.name);
    }

    // A preset's playback range, sprite-sheet settings and, given its camera, render camera.
    pub(crate) fn use_preset_settings(&mut self, preset: &presets::Preset, camera: Option<NodeId>) {
        self.animate = false;
        (self.playback_start, self.playback_end) = preset.range;
        self.time = preset.range.0;
        if let Some(id) = camera {
            self.render_camera = RenderCamera::Node(id);
            // Look through it in the viewport too, so what you see is what exports.
            self.look_through(Some(id));
        }
        // Looping presets loop in the scene itself (Make Loop, or by construction), so the
        // rendered frames need no blending.
        let s = &mut self.sprite_sheet;
        (s.width, s.height, s.seamless) = (preset.frame[0], preset.frame[1], false);
        self.dirty = true;
    }

    // Saves to the current scene file, or asks where to save if there isn't one yet.
    pub(crate) fn save_scene(&mut self) {
        match self.scene_path.clone() {
            // An old .mhip scene is saved as .pfs beside it; the original is left alone.
            Some(path) if path.extension().is_some_and(|e| e == "mhip") => self.write_scene(path.with_extension(SCENE_EXTENSION)),
            Some(path) => self.write_scene(path),
            None => self.save_scene_dialog(),
        }
    }

    pub(crate) fn save_scene_dialog(&mut self) {
        let name = self.scene_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        // Saving an old .mhip scene writes a .pfs next to it.
        let name = name.map(|n| match n.strip_suffix(".mhip") {
            Some(stem) => format!("{stem}.{SCENE_EXTENSION}"),
            None => n,
        });
        let mut dialog = FileDialog::new().set_file_name(name.as_deref().unwrap_or(SCENE_FILE)).add_filter("ProceduralFX Scene", &[SCENE_EXTENSION]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.save_file() {
            Some(path) => self.write_scene(path),
            None => self.status = "Save canceled".into(),
        }
    }

    pub(crate) fn write_scene(&mut self, path: PathBuf) {
        match write_file_atomically(&path, &self.scene_text()) {
            Ok(()) => {
                self.mark_saved();
                if let Some(session) = &mut self.recovery {
                    session.clear_autosave();
                }
                self.status = format!("Saved {}", path.display());
                push_recent_scene(&mut self.recent_scenes, path.clone());
                save_recent_scenes(&self.recent_scenes);
                self.scene_path = Some(path);
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    // The scene now matches what's on disk.
    pub(crate) fn mark_saved(&mut self) {
        self.saved_fingerprint = scene_fingerprint(&self.scene_text());
        self.unsaved = false;
        self.unsaved_checked = Some(Instant::now());
    }

    pub(crate) fn has_unsaved_changes(&self) -> bool {
        scene_fingerprint(&self.scene_text()) != self.saved_fingerprint
    }

    // What a saved scene file holds: the graph, then the timeline.
    pub(crate) fn scene_text(&self) -> String {
        self.graph.serialize() + &timeline_line(self.playback_start, self.playback_end, self.playback_fps)
    }

    // Uses a scene file's timeline, if it saved one.
    pub(crate) fn use_saved_timeline(&mut self, text: &str) {
        if let Some((start, end, fps)) = parse_timeline(text) {
            (self.playback_start, self.playback_end, self.playback_fps) = (start, end, fps);
            self.time = self.time.clamp(start, end);
        }
    }

    // Writes unsaved changes to the session's autosave every so often.
    pub(crate) fn autosave(&mut self, ctx: &egui::Context) {
        let Some(mut session) = self.recovery.take() else { return };
        if session.due() {
            let unsaved = self.has_unsaved_changes();
            let scene_path = self.scene_path.clone();
            session.tick(unsaved, scene_path.as_deref(), || self.scene_text());
        }
        self.recovery = Some(session);
        // An idle window doesn't redraw: wake it for the next autosave.
        ctx.request_repaint_after(recovery::AUTOSAVE_EVERY);
    }

    // After a crash or a forced quit: offers back the unsaved work, and the crash report.
    pub(crate) fn recovery_prompt(&mut self, ctx: &egui::Context) {
        let Some(offer) = self.recovery_offer.clone() else { return };
        let Some(dir) = recovery_dir() else { return };
        let name = offer.scene_path.as_ref().and_then(|p| p.file_name()).map_or_else(|| "an untitled scene".to_owned(), |n| format!("\"{}\"", n.to_string_lossy()));
        let when = offer.when.map(|t| recovery::ago(t, std::time::SystemTime::now()));
        let mut choice = None;
        egui::Modal::new(egui::Id::new("recovery_prompt")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("ProceduralFX Studio didn't close properly last time");
            ui.add_space(4.0);
            if offer.autosave.is_some() {
                ui.label(format!("Unsaved changes to {name} were kept{}.", when.map(|w| format!(" (autosaved {w})")).unwrap_or_default()));
            }
            if let Some(report) = &offer.crash_report {
                if let Some(rx) = &self.crash_send {
                    if let Ok(result) = rx.try_recv() {
                        self.crash_sent = Some(result);
                        self.crash_send = None;
                    }
                }
                match &self.crash_sent {
                    Some(Ok(())) => ui.label("It crashed. A crash report was saved on this computer, and you sent it: thank you."),
                    _ => ui.label("It crashed. A crash report was saved on this computer; nothing has been sent."),
                };
                // Sending it: only what's shown here, only when asked.
                let server = self.crash_report_server.clone();
                if let (Some(server), Ok(text)) = (server, std::fs::read_to_string(report)) {
                    let home = recovery::home_folder();
                    let shown = recovery::redacted(&text, home.as_deref());
                    if self.crash_sent.as_ref().is_none_or(|r| r.is_err()) {
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new("What would be sent").id_salt("crash_report_preview").show(ui, |ui| {
                            ui.weak("This, and your note. Your home folder shows as ~. No account, scene or IP address is sent.");
                            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                                ui.add(egui::Label::new(egui::RichText::new(&shown).monospace().small()).wrap());
                            });
                        });
                        ui.add(egui::TextEdit::multiline(&mut self.crash_note).hint_text("What were you doing when it crashed? (optional)").desired_rows(2).desired_width(f32::INFINITY));
                        ui.horizontal(|ui| {
                            let sending = self.crash_send.is_some();
                            if ui.add_enabled(!sending, egui::Button::new(if sending { "Sending…" } else { "Send the report" })).clicked() {
                                let (tx, rx) = mpsc::channel();
                                let note = self.crash_note.trim().to_owned();
                                std::thread::spawn(move || {
                                    let _ = tx.send(recovery::send_report(&server, &shown, &note));
                                });
                                self.crash_send = Some(rx);
                                self.crash_sent = None;
                            }
                            if sending {
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                            }
                            if let Some(Err(e)) = &self.crash_sent {
                                ui.colored_label(egui::Color32::from_rgb(230, 120, 100), format!("Couldn't send it: {e}"));
                            }
                        });
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    ui.weak("Or email the report to");
                    ui.hyperlink_to("support@build3dstudio.com", "mailto:support@build3dstudio.com?subject=Crash%20report");
                });
                ui.horizontal(|ui| {
                    if ui.button("Copy the report").clicked() {
                        if let Ok(text) = std::fs::read_to_string(report) {
                            ui.ctx().copy_text(text);
                            self.status = "Crash report copied".into();
                        }
                    }
                    if ui.button("Show the file").clicked() {
                        let _ = platform::reveal(report);
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if offer.autosave.is_some() {
                    if ui.button("Discard").on_hover_text("Delete the unsaved changes").clicked() {
                        choice = Some(false);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Enter recovers, unless it's typing a note.
                        let typing = ui.memory(|m| m.focused().is_some());
                        if ui.button("Recover").clicked() || (!typing && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            choice = Some(true);
                        }
                    });
                } else if ui.button("OK").clicked() || (!ui.memory(|m| m.focused().is_some()) && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape))) {
                    choice = Some(false);
                }
            });
        });
        let Some(recover) = choice else { return };
        if recover {
            if let Some(text) = offer.autosave.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
                self.recover_scene(text, offer.scene_path.clone());
            }
        }
        recovery::forget(&dir, &offer.id);
        self.recovery_offer = None;
        self.crash_note.clear();
        self.crash_sent = None;
    }

    // Opens recovered work as unsaved changes to the scene it came from.
    pub(crate) fn recover_scene(&mut self, text: String, scene_path: Option<PathBuf>) {
        if let Some(problem) = scene_problem(&text) {
            self.status = format!("Couldn't recover the scene: {problem}");
            return;
        }
        self.undo.push(self.last_state.clone());
        self.use_saved_timeline(&text);
        self.load_state(text);
        // Unsaved against the file on disk (or against nothing, if it was untitled).
        let on_disk = scene_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        self.saved_fingerprint = on_disk.map_or(0, |t| scene_fingerprint(&t));
        self.unsaved = true;
        self.unsaved_checked = None;
        // Saving writes back to the scene it came from.
        self.scene_path = scene_path.filter(|p| p.extension().is_some_and(|e| e == SCENE_EXTENSION || e == "mhip"));
        self.status = "Recovered your unsaved changes. Save to keep them.".into();
    }

    // Quitting with unsaved changes asks first: Save, Don't Save or Cancel.
    pub(crate) fn confirm_quit(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.has_unsaved_changes() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.quit_prompt = true;
            }
        }
        if !self.quit_prompt {
            return;
        }
        let name = self.scene_path.as_ref().and_then(|p| p.file_name()).map_or_else(|| "this scene".to_owned(), |n| format!("\"{}\"", n.to_string_lossy()));
        let mut choice = None;
        egui::Modal::new(egui::Id::new("quit_prompt")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading("Save changes before quitting?");
            ui.label(format!("Your changes to {name} will be lost if you don't save them."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Don't Save").clicked() {
                    choice = Some(false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        choice = Some(true);
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.quit_prompt = false;
                    }
                });
            });
        });
        match choice {
            Some(true) => {
                self.save_scene();
                // Quit only once it's saved (the Save dialog can be cancelled).
                if !self.has_unsaved_changes() {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                self.quit_prompt = false;
            }
            Some(false) => {
                self.quitting = true;
                self.quit_prompt = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
    }

    pub(crate) fn update_window_title(&mut self, ctx: &egui::Context) {
        if self.unsaved_checked.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(500)) {
            self.unsaved = self.has_unsaved_changes();
            self.unsaved_checked = Some(Instant::now());
        }
        let edited = if self.unsaved { " •" } else { "" };
        let title = match self.scene_path.as_ref().and_then(|p| p.file_name()) {
            Some(name) => format!("{}{edited} — ProceduralFX Studio", name.to_string_lossy()),
            None => format!("Untitled{edited} — ProceduralFX Studio"),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    pub(crate) fn load_state(&mut self, s: String) {
        self.graph = Graph::deserialize(&s);
        self.last_state = s;
        if self.selected.map_or(false, |id| !self.graph.nodes.contains_key(&id)) {
            self.selected = None;
        }
        self.dragging_link = None;
        self.dirty = true;
    }
}
