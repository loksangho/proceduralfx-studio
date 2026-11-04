// The menus and the toolbar.

use crate::*;

impl ProceduralFxStudioApp {
    pub(crate) fn menu_contents(&mut self, ui: &mut egui::Ui, name: &str) {
        match name {
            "File" => self.file_menu(ui),
            "Edit" => self.edit_menu(ui),
            "Select" => self.select_menu(ui),
            "View" => self.view_menu(ui),
            "Render" => self.render_menu(ui),
            "Windows" => self.windows_menu(ui),
            _ => self.help_menu(ui),
        }
    }

    pub(crate) fn file_menu(&mut self, ui: &mut egui::Ui) {
        let menu_ctx = ui.ctx().clone();
        let shortcut = |s: &egui::KeyboardShortcut| menu_ctx.format_shortcut(s);
        if ui.add(egui::Button::new("Open…").shortcut_text(shortcut(&SHORTCUT_OPEN))).clicked() {
            self.open_scene_dialog();
            ui.close();
        }
        ui.add_enabled_ui(!self.recent_scenes.is_empty(), |ui| {
            ui.menu_button("Open Recent", |ui| {
                let mut chosen = None;
                for path in &self.recent_scenes {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
                    let button = ui
                        .add_enabled(path.exists(), egui::Button::new(name))
                        .on_hover_text(path.display().to_string())
                        .on_disabled_hover_text(format!("{} (not found)", path.display()));
                    if button.clicked() {
                        chosen = Some(path.clone());
                    }
                }
                ui.separator();
                if ui.button("Clear Recent").clicked() {
                    self.recent_scenes.clear();
                    save_recent_scenes(&self.recent_scenes);
                    ui.close();
                }
                if let Some(path) = chosen {
                    self.open_scene(path);
                    ui.close();
                }
            });
        });
        ui.menu_button("New from Preset", |ui| {
            for (three_d, heading) in [(false, "Starter effects for 2D games (sprite sheets)"), (true, "For 3D games (vertex animation)")] {
                if three_d {
                    ui.separator();
                }
                ui.weak(heading);
            for preset in presets::PRESETS.iter().filter(|p| p.vertex_animation == three_d) {
                let label = if preset.looping { format!("{} (loop)", preset.name) } else { preset.name.to_owned() };
                if ui.button(label).on_hover_text(preset.description).clicked() {
                    self.load_preset(preset);
                    ui.close();
                }
            }
            }
        });
        ui.separator();
        let save_hover = match &self.scene_path {
            Some(path) => format!("Save to {}", path.display()),
            None => "Choose where to save this scene".to_owned(),
        };
        if ui.add(egui::Button::new("Save").shortcut_text(shortcut(&SHORTCUT_SAVE))).on_hover_text(save_hover).clicked() {
            self.save_scene();
            ui.close();
        }
        if ui.add(egui::Button::new("Save As…").shortcut_text(shortcut(&SHORTCUT_SAVE_AS))).clicked() {
            self.save_scene_dialog();
            ui.close();
        }
        ui.separator();
        if ui.button("Export OBJ…").on_hover_text("Writes the displayed mesh to an OBJ file you choose").clicked() {
            ui.close();
            self.export_mesh_dialog("OBJ", "obj");
        }
        if ui.button("Export STL…").on_hover_text("Writes the displayed mesh to an STL file you choose").clicked() {
            ui.close();
            self.export_mesh_dialog("STL", "stl");
        }
    }

    // Asks where to write the displayed mesh, named after the scene and beside it by default.
    pub(crate) fn export_mesh_dialog(&mut self, format: &str, extension: &str) {
        let stem = self.scene_path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "procedural_output".to_owned(), |s| s.to_string_lossy().into_owned());
        let mut dialog = FileDialog::new().set_title(&format!("Export {format}")).set_file_name(&format!("{stem}.{extension}")).add_filter(&format!("{format} mesh"), &[extension]);
        if let Some(dir) = self.scene_path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            self.status = "Export canceled".into();
            return;
        };
        if path.extension().is_none() {
            path.set_extension(extension);
        }
        let file = path.to_string_lossy().into_owned();
        let result = if extension == "obj" { self.current_mesh.export_obj(&file) } else { self.current_mesh.export_stl(&file) };
        self.status = match result {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    pub(crate) fn edit_menu(&mut self, ui: &mut egui::Ui) {
        if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
            self.do_undo();
            ui.close();
        }
        if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z")).clicked() {
            self.do_redo();
            ui.close();
        }
    }

    pub(crate) fn select_menu(&mut self, ui: &mut egui::Ui) {
        let actions: [(&str, fn(&mut Self)); 9] = [
            ("All", Self::select_all_components),
            ("None", Self::clear_component_selection),
            ("Invert", Self::invert_component_selection),
            ("Grow", Self::grow_component_selection),
            ("Shrink", Self::shrink_component_selection),
            ("Boundary edges", Self::select_boundary_edges),
            ("Non-manifold edges", Self::select_non_manifold_edges),
            ("Isolated points", Self::select_isolated_vertices),
            ("Connected", Self::select_connected_components),
        ];
        for (i, (label, action)) in actions.into_iter().enumerate() {
            if i == 5 {
                ui.separator();
            }
            if ui.button(label).clicked() {
                action(self);
                ui.close();
            }
        }
        ui.separator();
        ui.checkbox(&mut self.isolate_selection, "Isolate selection");
        ui.weak("Shift-drag: box select • double-click an edge: loop");
    }

    pub(crate) fn view_menu(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.show_points, "Points");
        ui.checkbox(&mut self.show_floor, "Floor grid");
        ui.checkbox(&mut self.show_gizmos, "Gizmos");
        ui.checkbox(&mut self.orthographic, "Orthographic");
        ui.separator();
        for (label, rot_x, rot_y) in [
            ("Top", -1.52, 0.0),
            ("Front", 0.0, 0.0),
            ("Right", 0.0, std::f32::consts::FRAC_PI_2),
            ("Perspective", 0.4, -0.6),
        ] {
            if ui.button(format!("{label} view")).clicked() {
                self.set_camera_view(rot_x, rot_y);
                ui.close();
            }
        }
        if ui.add(egui::Button::new("Frame all").shortcut_text("F")).clicked() {
            self.frame_all();
            ui.close();
        }
        ui.separator();
        pfield(ui, pslider(&mut self.viewport_bg, 15..=80).text("Background"));
        pfield(ui, pslider(&mut self.grid_spacing, 0.1..=2.0).text("Grid spacing"));
        pfield(ui, pslider(&mut self.grid_extent, 1.0..=20.0).text("Grid extent"));
        ui.menu_button("Camera & lights", |ui| {
            pfield(ui, pslider(&mut self.camera_focal, 0.5..=2.5).text("Focal scale"));
            pfield(ui, pslider(&mut self.camera_near, 0.05..=2.0).text("Near clip"));
            ui.separator();
            pfield(ui, pslider(&mut self.ambient_light, 0.0..=1.0).text("Ambient")).on_hover_text(
                "Soft light everywhere, always added, with or without lights in the scene, so the sides facing away from them aren't black. \
                 MoonRay renders get it too, as a dim environment light. Set it to 0 to light the scene only with its own lights.",
            );
            ui.label("Default lights (used when the scene has no lights)");
            vec3_ui(ui, "Key direction", &mut self.key_light_dir, 0.05);
            pfield(ui, pslider(&mut self.key_light_intensity, 0.0..=2.0).text("Key intensity"));
            vec3_ui(ui, "Fill direction", &mut self.fill_light_dir, 0.05);
            pfield(ui, pslider(&mut self.fill_light_intensity, 0.0..=2.0).text("Fill intensity"));
        });
    }

    pub(crate) fn render_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Render with MoonRay").clicked() {
            self.start_moonray_render();
            ui.close();
        }
        let realtime = ui
            .add_enabled(self.gpu_viewport.is_some() && self.flipbook_recording.is_none(), egui::Button::new("Render Real-Time"))
            .on_hover_text("Renders like the viewport, in seconds, through the render camera at the Render Settings size (a sequence if that's ticked there): transparent PNGs, no MoonRay needed");
        if realtime.clicked() {
            self.start_realtime_render();
            ui.close();
        }
        if ui
            .add_enabled(self.gpu_viewport.is_some(), egui::Button::new("Export Sprite Sheet…"))
            .on_hover_text("Renders the playback range in real time and packs the frames into one PNG, with a JSON file describing the grid, ready for a 2D game")
            .clicked()
        {
            self.sprite_sheet.open = true;
            ui.close();
        }
        if ui
            .button("Export Vertex Animation…")
            .on_hover_text("Bakes a deforming object (cloth, deformers, wobbling shapes) over the playback range into vertex animation textures, with shaders for Godot and Unity: the mesh moves on the GPU in the game")
            .clicked()
        {
            self.vat_window.open = true;
            ui.close();
        }
        if ui.button("✨ AI").on_hover_text("Describe a setup and Claude builds it as nodes").clicked() {
            self.assistant.open = !self.assistant.open;
        }
        if ui.button("Render Settings…").clicked() {
            self.show_moonray_settings = true;
            ui.close();
        }
        if ui.button("Render Viewer").clicked() {
            self.viewer.show();
            ui.close();
        }
        if let Some(helper) = wslrender::helper() {
            ui.separator();
            let busy = self.render_setup.is_some();
            if ui.add_enabled(!busy, egui::Button::new("Set Up Rendering on Windows…")).on_hover_text("Installs WSL containers if needed and downloads MoonRay (a few GB, once)").clicked() {
                self.render_setup = Some(start_render_setup(helper, false));
                self.render_setup_log.clear();
                self.render_setup_needs_admin = false;
                self.render_setup_open = true;
                ui.close();
            }
        }
    }

    pub(crate) fn windows_menu(&mut self, ui: &mut egui::Ui) {
        for pane in Pane::ALL {
            let mut open = self.open_panes.contains(&pane);
            if ui.checkbox(&mut open, pane.title()).changed() {
                self.pane_requests.push(PaneRequest::Show(pane, open));
            }
        }
        ui.separator();
        ui.menu_button("Open in New Window", |ui| {
            for pane in Pane::ALL {
                if ui.button(pane.title()).clicked() {
                    self.pane_requests.push(PaneRequest::PopOut(pane));
                    ui.close();
                }
            }
        });
        ui.menu_button("Layouts", |ui| {
            if ui.button("Save Current Layout…").clicked() {
                self.layout_name_prompt = Some(String::new());
                ui.close();
            }
            let names = saved_layouts();
            if !names.is_empty() {
                ui.separator();
            }
            for name in names {
                ui.horizontal(|ui| {
                    if ui.button(&name).on_hover_text("Switch to this layout").clicked() {
                        self.pane_requests.push(PaneRequest::Load(name.clone()));
                        ui.close();
                    }
                    if ui.small_button("🗑").on_hover_text("Delete this layout").clicked() {
                        self.pane_requests.push(PaneRequest::Delete(name.clone()));
                    }
                });
            }
            ui.separator();
            if ui.button("Reset Layout").on_hover_text("Back to the default arrangement of panes").clicked() {
                self.pane_requests.push(PaneRequest::Reset);
                ui.close();
            }
        });
        ui.weak("Drag tabs to rearrange or split; drag one out to float it.");
        ui.separator();
        if ui.button("Render Viewer").clicked() {
            self.viewer.show();
            ui.close();
        }
        if ui.button("Render Settings").clicked() {
            self.show_moonray_settings = true;
            ui.close();
        }
    }

    pub(crate) fn help_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("📖 Documentation").on_hover_text("All nodes and VEX, as a web page in your browser").clicked() {
            self.status = match open_documentation() {
                Ok(path) => format!("Opened the documentation ({path})"),
                Err(e) => format!("Couldn't open the documentation: {e}"),
            };
            ui.close_kind(egui::UiKind::Menu);
        }
        ui.separator();
        if ui.button("📋 Copy diagnostics").on_hover_text("Copies what support needs to help: the version, your system and GPU, and what the app did recently (your home folder shows as ~). Nothing is sent: paste it into an email.").clicked() {
            ui.ctx().copy_text(self.diagnostics());
            self.status = "Copied the diagnostics: paste them into an email to support@build3dstudio.com".into();
            ui.close_kind(egui::UiKind::Menu);
        }
        if let Some(log) = applog::path().filter(|p| p.exists()) {
            if ui.button("📄 Show the log file").on_hover_text(log.display().to_string()).clicked() {
                if let Err(e) = platform::reveal(&log) {
                    self.status = format!("Couldn't show the log file: {e}");
                }
                ui.close_kind(egui::UiKind::Menu);
            }
        }
        ui.separator();
        self.updater.menu(ui);
        ui.separator();
        ui.weak("Houdini is a trademark of Side Effects Software Inc.\nProceduralFX Studio is not affiliated with or endorsed by SideFX.");
    }

    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            // Narrower windows drop the title, then fold the menus into one ☰ button.
            let width = ui.available_width();
            let updater = if self.updater.has_indicator() { 190.0 } else { 0.0 };
            if width - updater > 640.0 {
                ui.strong("ProceduralFX Studio");
                ui.separator();
            }
            if width - updater > 470.0 {
                for name in Self::MENUS {
                    ui.menu_button(name, |ui| self.menu_contents(ui, name));
                }
            } else {
                ui.menu_button("☰", |ui| {
                    for name in Self::MENUS {
                        ui.menu_button(name, |ui| self.menu_contents(ui, name));
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.updater.indicator(ui));
        });

        // The toolbar wraps a group at a time when the window is narrow, so every control
        // stays visible; the render controls keep to the right edge when they fit there.
        ui.horizontal_wrapped(|ui| {
            toolbar_group(ui, "contexts", |ui| {
                for (context, label, hover) in [
                    (AppContext::Obj, "OBJ", "Objects and their geometry (SOP) networks"),
                    (AppContext::Mat, "MAT", "Materials"),
                    (AppContext::Out, "OUT", "Render nodes"),
                    (AppContext::Stage, "STAGE", "USD stage (LOP) network"),
                    (AppContext::Img, "IMG", "Compositing (COP networks)"),
                    (AppContext::Tasks, "TASKS", "Task graphs: renders, wedges and batch jobs (TOP networks)"),
                ] {
                    if ui.selectable_label(self.context == context, egui::RichText::new(label).strong()).on_hover_text(hover).clicked()
                        && self.context != context
                    {
                        self.context = context;
                        // An open network belongs to its own context (a Material Builder to MAT).
                        if self.network().is_some_and(|id| self.graph.nodes.get(&id).is_some_and(|n| n.context != context)) {
                            self.current_network = None;
                        }
                        self.selected = None;
                        self.dirty = true;
                    }
                }
            });
            toolbar_group(ui, "shading", |ui| {
                ui.separator();
                ui.selectable_value(&mut self.render_mode, RenderMode::ShadedWireframe, "Shaded");
                ui.selectable_value(&mut self.render_mode, RenderMode::FlatShaded, "Flat");
                ui.selectable_value(&mut self.render_mode, RenderMode::Wireframe, "Wire");
                ui.selectable_value(&mut self.render_mode, RenderMode::Normals, "Normals");
            });
            toolbar_group(ui, "camera", |ui| {
                ui.separator();
                let cameras = self.camera_ids();
                let looking = self.active_camera.filter(|_| self.looking_through);
                let shown = looking.and_then(|id| cameras.iter().find(|(c, _)| *c == id)).map_or("Free view".to_owned(), |(_, name)| name.clone());
                egui::ComboBox::from_id_salt("look_through").selected_text(format!("👁 {shown}")).show_ui(ui, |ui| {
                    if ui.selectable_label(looking.is_none(), "Free view").clicked() && looking.is_some() {
                        self.look_through_request = Some(None);
                    }
                    for (id, name) in &cameras {
                        if ui.selectable_label(looking == Some(*id), name).clicked() {
                            self.look_through_request = Some(Some(*id));
                        }
                    }
                }).response.on_hover_text("Look through a camera without changing it");
            });
            toolbar_group(ui, "selection", |ui| {
                ui.separator();
                let mode_changed = ui.selectable_value(&mut self.selection_mode, SelectionMode::Vertex, "Points").changed()
                    || ui.selectable_value(&mut self.selection_mode, SelectionMode::Triangle, "Faces").changed()
                    || ui.selectable_value(&mut self.selection_mode, SelectionMode::Edge, "Edges").changed();
                if mode_changed {
                    self.clear_component_selection();
                }
            });
            toolbar_group(ui, "undo", |ui| {
                ui.separator();
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("↶")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                    self.do_undo();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("↷")).on_hover_text("Redo (Ctrl+Shift+Z)").clicked() {
                    self.do_redo();
                }
            });
            toolbar_group(ui, "transform", |ui| {
                ui.separator();
                for (mode, label, hover) in [
                    (GizmoMode::Translate, "✥ Move", "Move the selected object (W)"),
                    (GizmoMode::Rotate, "⟳ Rotate", "Rotate the selected object (E)"),
                    (GizmoMode::Scale, "⤢ Scale", "Scale the selected geometry object (R)"),
                ] {
                    ui.selectable_value(&mut self.gizmo_mode, mode, label).on_hover_text(hover);
                }
            });
            toolbar_group(ui, "keys", |ui| {
                ui.separator();
                if ui.button("◆ Key").on_hover_text("Key the selected node at this frame (K)").clicked() {
                    self.key_selected();
                }
                if !self.graph.pending.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(255, 170, 60), format!("● {} unkeyed", self.graph.pending.len()))
                        .on_hover_text("Changes to animated parameters, not keyed (Auto Key is off). Press K to key them; on another frame they go back to the animation");
                }
                ui.toggle_value(&mut self.auto_key, "Auto Key").on_hover_text("Key every parameter you change, not only animated ones");
                let mut graph_open = self.open_panes.contains(&Pane::GraphEditor);
                if ui.toggle_value(&mut graph_open, "📈 Graph").on_hover_text("Graph Editor").changed() {
                    self.pane_requests.push(PaneRequest::Show(Pane::GraphEditor, graph_open));
                }
            });
            if ui.available_size_before_wrap().x < self.toolbar_render_width + 8.0 && ui.cursor().min.x > ui.max_rect().left() + 1.0 {
                ui.end_row();
            }
            if ui.max_rect().width() < self.toolbar_render_width + 8.0 {
                // Narrower than the render controls themselves: left to right, wrapping.
                self.render_controls(ui, false);
            } else {
                let row = egui::vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let right = ui.max_rect().right();
                    self.render_controls(ui, true);
                    self.toolbar_render_width = right - ui.min_rect().left();
                });
            }
        });
    }

    // The toolbar's camera, Render and settings controls; `from_right` lays them out
    // right to left (⚙ at the edge).
    pub(crate) fn render_controls(&mut self, ui: &mut egui::Ui, from_right: bool) {
        let settings = |app: &mut Self, ui: &mut egui::Ui| {
            if ui.button("⚙").on_hover_text("Render Settings").clicked() {
                app.show_moonray_settings = true;
            }
        };
        let render = |app: &mut Self, ui: &mut egui::Ui| {
            if app.moonray_rx.is_some() || app.sequence.is_some() {
                if ui.button("■ Cancel").clicked() {
                    app.cancel_render();
                }
                if let Some(started) = app.render_started {
                    ui.label(format!("{:.0}s", started.elapsed().as_secs_f32()));
                }
                ui.spinner();
            } else if ui.button(egui::RichText::new("▶ Render").strong()).on_hover_text("Render with MoonRay").clicked() {
                app.start_moonray_render();
            }
        };
        let camera = |app: &mut Self, ui: &mut egui::Ui| {
            let cameras = app.camera_ids();
            render_camera_combo(ui, &mut app.render_camera, &cameras);
        };
        if from_right {
            settings(self, ui);
            render(self, ui);
            camera(self, ui);
            ui.weak("Camera");
        } else {
            ui.weak("Camera");
            camera(self, ui);
            render(self, ui);
            settings(self, ui);
        }
    }
}
