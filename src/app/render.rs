// Rendering with MoonRay and Hydra: the settings, the scene it's given, lights, and the render's progress.

use crate::*;

impl ProceduralFxStudioApp {
    pub(crate) fn start_moonray_render(&mut self) {
        if self.moonray_rx.is_some() || self.sequence.is_some() {
            self.status = "A MoonRay render is already running".into();
            return;
        }
        self.status.clear();
        self.render_at = None;
        // A selected render node supplies its own settings; otherwise the toolbar defaults apply.
        if let Some(id) = self.selected {
            if let Some(NodeKind::MoonRayRender { executable, arguments, output, width, height, samples, opts }) =
                self.graph.nodes.get(&id).map(|node| &node.kind)
            {
                self.render_opts = opts.clone();
                if opts.sequence {
                    let job = SequenceJob {
                        executable: executable.clone(),
                        arguments: arguments.clone(),
                        output: output.clone(),
                        width: *width,
                        height: *height,
                        samples: *samples,
                        opts: opts.clone(),
                        times: frame_times(opts.range_start, opts.range_end, opts.range_step),
                        next: 0,
                    };
                    self.begin_sequence(job);
                    return;
                }
                self.status = format!("Rendering with {} camera...", self.resolve_render_camera().1);
                self.spawn_moonray_render(
                    executable.clone(),
                    arguments.clone(),
                    output.clone(),
                    *width,
                    *height,
                    *samples,
                );
                return;
            }
        }
        // Otherwise the Render Settings dialog applies.
        let opts = RenderOpts { camera: self.render_camera, ..self.moonray_default_opts.clone() };
        self.render_opts = opts.clone();
        // Relative outputs are routed to the temp folder by spawn_moonray_render.
        let output = if self.render_to_temp {
            format!("render.{}", self.render_temp_ext)
        } else {
            self.moonray_default_output.clone()
        };
        if opts.sequence {
            let job = SequenceJob {
                executable: self.moonray_default_executable.clone(),
                arguments: self.moonray_default_arguments.clone(),
                output,
                width: self.moonray_default_width,
                height: self.moonray_default_height,
                samples: self.moonray_default_samples,
                times: frame_times(opts.range_start, opts.range_end, opts.range_step),
                opts,
                next: 0,
            };
            self.begin_sequence(job);
            return;
        }
        self.status = format!("Rendering with {} camera...", self.resolve_render_camera().1);
        self.spawn_moonray_render(
            self.moonray_default_executable.clone(),
            self.moonray_default_arguments.clone(),
            output,
            self.moonray_default_width,
            self.moonray_default_height,
            self.moonray_default_samples,
        );
    }

    pub(crate) fn render_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_moonray_settings {
            return;
        }
        let cameras = self.camera_ids();
        let node_selected = self
            .selected
            .is_some_and(|id| matches!(self.graph.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::MoonRayRender { .. })));
        let rendering = self.moonray_rx.is_some() || self.sequence.is_some();
        // Live log while a render runs; the finished log afterwards.
        let log = match (&self.render_log_path, self.moonray_rx.is_some()) {
            (Some(path), true) => read_log_tail(path, 64 * 1024),
            _ => self.last_render_log.clone(),
        };
        let mut open = true;
        let mut start = false;
        let mut cancel = false;
        own_window(ctx, "render_settings", "Render Settings", [480.0, 720.0], [360.0, 320.0], &mut open, true, |ui| {
                if node_selected {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 190, 90),
                        "A MoonRay render node is selected: Render uses that node's settings. Resolution scale still applies.",
                    );
                }
                // As tall as the window leaves room for, above the buttons and the log.
                egui::ScrollArea::vertical().max_height((ui.available_height() - 200.0).max(240.0)).show(ui, |ui| {
                    egui::CollapsingHeader::new("Renderer").default_open(true).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Executable");
                            ui.add(egui::TextEdit::singleline(&mut self.moonray_default_executable).desired_width(260.0));
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Browse…").clicked() {
                                if let Some(path) = FileDialog::new().pick_file() {
                                    self.moonray_default_executable = path.to_string_lossy().into_owned();
                                }
                            }
                            if ui.button("Auto-detect").clicked() {
                                self.moonray_default_executable = detect_moonray_executable();
                            }
                            match resolve_executable(&self.moonray_default_executable) {
                                Some(path) => ui.colored_label(egui::Color32::LIGHT_GREEN, format!("✔ {}", path.display())),
                                None => ui.colored_label(egui::Color32::from_rgb(240, 110, 110), "✖ not found"),
                            };
                        });
                    });

                    egui::CollapsingHeader::new("Image").default_open(true).show(ui, |ui| {
                        let (w, h) = (&mut self.moonray_default_width, &mut self.moonray_default_height);
                        let current = RESOLUTION_PRESETS
                            .iter()
                            .find(|(_, pw, ph)| *pw == *w && *ph == *h)
                            .map(|(name, ..)| *name)
                            .unwrap_or("Custom");
                        ui.horizontal(|ui| {
                            ui.label("Resolution");
                            egui::ComboBox::from_id_salt("render_resolution_preset")
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    for (name, pw, ph) in RESOLUTION_PRESETS {
                                        if ui.selectable_label(current == name, format!("{name}  ({pw}×{ph})")).clicked() {
                                            (*w, *h) = (pw, ph);
                                        }
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            pfield(ui, pdrag(w).range(1..=16384).prefix("W "));
                            if ui.button("⇄").on_hover_text("Swap width and height").clicked() {
                                std::mem::swap(w, h);
                            }
                            pfield(ui, pdrag(h).range(1..=16384).prefix("H "));
                            ui.weak(format!("aspect {:.3}", *w as f32 / (*h).max(1) as f32));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Resolution scale");
                            for (scale, label) in [(1.0, "100%"), (0.5, "50%"), (0.25, "25%")] {
                                ui.selectable_value(&mut self.resolution_scale, scale, label);
                            }
                        })
                        .response
                        .on_hover_text("Quick previews: scales every render, render nodes included");
                        if self.resolution_scale < 1.0 {
                            ui.weak(format!(
                                "Renders at {}×{}",
                                ((*w as f32 * self.resolution_scale).round() as usize).max(1),
                                ((*h as f32 * self.resolution_scale).round() as usize).max(1)
                            ));
                        }
                        ui.horizontal(|ui| {
                            ui.label("Camera");
                            ui.push_id("settings_camera", |ui| render_camera_combo(ui, &mut self.render_camera, &cameras));
                        });
                        ui.checkbox(&mut self.moonray_default_opts.stage, "Render the STAGE (USD) instead of the OBJ objects")
                            .on_hover_text("Uses the displayed LOP's stage: its meshes, materials, lights and (with the Auto camera) its first camera");
                        hydra_checkbox(ui, &mut self.moonray_default_opts);
                    });

                    egui::CollapsingHeader::new("Sampling").default_open(true).show(ui, |ui| {
                        let opts = &mut self.moonray_default_opts;
                        let samples = &mut self.moonray_default_samples;
                        ui.horizontal(|ui| {
                            ui.label("Quality");
                            for (name, spp, depth, light, bsdf) in QUALITY_PRESETS {
                                let active = *samples == spp && opts.max_depth == depth && opts.light_samples == light && opts.bsdf_samples == bsdf;
                                if ui
                                    .selectable_label(active, name)
                                    .on_hover_text(format!("{spp} spp · depth {depth} · light {light} · BSDF {bsdf}"))
                                    .clicked()
                                {
                                    *samples = spp;
                                    (opts.max_depth, opts.light_samples, opts.bsdf_samples) = (depth, light, bsdf);
                                }
                            }
                        });
                        pfield(ui, pdrag(samples).range(1..=4096).prefix("Samples per pixel "));
                        pfield(ui, pslider(&mut opts.max_depth, 1..=32).text("Max ray depth"));
                        pfield(ui, pslider(&mut opts.light_samples, 1..=16).text("Light samples"));
                        pfield(ui, pslider(&mut opts.bsdf_samples, 1..=16).text("BSDF samples"));
                        pfield(ui, pslider(&mut opts.volume_samples, 1..=8).text("Volume lighting samples"))
                            .on_hover_text("Light samples per step inside smoke and fire. Render time grows roughly in proportion: 1 for previews, 4+ for clean finals.");
                        ui.horizontal(|ui| {
                            ui.label("Environment light");
                            ui.color_edit_button_rgb(&mut opts.background);
                        });
                        ui.small("A non-black color adds a uniform environment light (not visible in camera).");
                    });

                    egui::CollapsingHeader::new("Output").default_open(true).show(ui, |ui| {
                        ui.radio_value(&mut self.render_to_temp, true, "Temporary folder (auto-named, open in the viewer)");
                        if self.render_to_temp {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label("Format");
                                ui.selectable_value(&mut self.render_temp_ext, "exr", "EXR (HDR)");
                                ui.selectable_value(&mut self.render_temp_ext, "png", "PNG");
                                if ui.button("Open folder").clicked() {
                                    if let Err(error) = reveal_in_file_browser(&std::env::temp_dir()) {
                                        self.status = error;
                                    }
                                }
                            });
                        }
                        ui.radio_value(&mut self.render_to_temp, false, "Save to file");
                        if !self.render_to_temp {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.add(egui::TextEdit::singleline(&mut self.moonray_default_output).desired_width(260.0));
                                if ui.button("Choose…").clicked() {
                                    if let Some(path) = FileDialog::new()
                                        .set_file_name("render.exr")
                                        .add_filter("Images", &["exr", "png", "jpg", "tif"])
                                        .save_file()
                                    {
                                        self.moonray_default_output = path.to_string_lossy().into_owned();
                                    }
                                }
                            });
                            if PathBuf::from(&self.moonray_default_output).is_relative() {
                                ui.colored_label(egui::Color32::from_rgb(230, 190, 90), "Relative paths are written to the temp folder; choose a full path.");
                            }
                        }
                        ui.checkbox(&mut self.open_viewer_after_render, "Open in the Render Viewer when finished");
                        ui.checkbox(&mut self.bucket_render, "Progressive buckets")
                            .on_hover_text("Shows the image filling in tile by tile in the Render Viewer. Each tile restarts MoonRay, so turn this off for very heavy scenes.");
                    });

                    egui::CollapsingHeader::new("Frame range").default_open(false).show(ui, |ui| {
                        let opts = &mut self.moonray_default_opts;
                        ui.checkbox(&mut opts.sequence, "Render frame range");
                        if opts.sequence {
                            ui.horizontal(|ui| {
                                pfield(ui, pdrag(&mut opts.range_start).speed(0.05).range(0.0..=100_000.0).prefix("Start ").suffix(" s"));
                                pfield(ui, pdrag(&mut opts.range_end).speed(0.05).range(0.0..=100_000.0).prefix("End ").suffix(" s"));
                                pfield(ui, pdrag(&mut opts.range_step).speed(0.01).range(0.0001..=1000.0).prefix("Step ").suffix(" s"))
                                    .on_hover_text(format!("Seconds between rendered frames: {:.4} is every frame", 1.0 / sims::SIM_FPS));
                            });
                            if ui.button("Use timeline range").clicked() {
                                opts.range_start = self.playback_start.max(0.0);
                                opts.range_end = self.playback_end.max(opts.range_start);
                            }
                            let frames = frame_times(opts.range_start, opts.range_end, opts.range_step).len();
                            ui.small(format!("{frames} frames, numbered render.0001, render.0002, … (#### in a file name sets the position)."));
                        }
                    });

                    egui::CollapsingHeader::new("Advanced").default_open(false).show(ui, |ui| {
                        ui.label("Arguments ({scene} {output} {width} {height} {samples})");
                        ui.text_edit_singleline(&mut self.moonray_default_arguments);
                        ui.label("Extra MoonRay arguments");
                        ui.add(egui::TextEdit::singleline(&mut self.render_extra_args).hint_text("e.g. -threads 8"));
                        if ui.button("Reset all settings to defaults").clicked() {
                            let defaults = Self::default();
                            self.moonray_default_opts = RenderOpts::default();
                            self.moonray_default_arguments = defaults.moonray_default_arguments;
                            self.moonray_default_width = defaults.moonray_default_width;
                            self.moonray_default_height = defaults.moonray_default_height;
                            self.moonray_default_samples = defaults.moonray_default_samples;
                            self.render_to_temp = defaults.render_to_temp;
                            self.render_temp_ext = defaults.render_temp_ext;
                            self.resolution_scale = defaults.resolution_scale;
                            self.open_viewer_after_render = defaults.open_viewer_after_render;
                            self.render_extra_args.clear();
                        }
                    });

                    egui::CollapsingHeader::new("Render log").default_open(false).show(ui, |ui| {
                        if !self.last_render_summary.is_empty() {
                            ui.label(&self.last_render_summary);
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!log.is_empty(), egui::Button::new("Copy log")).clicked() {
                                ui.ctx().copy_text(log.clone());
                            }
                            if let Some(path) = &self.render_log_path {
                                if ui.button("Reveal log file").clicked() {
                                    if let Err(error) = reveal_in_file_browser(path) {
                                        self.status = error;
                                    }
                                }
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("render_log").max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                            let text = if log.is_empty() { "No render yet." } else { log.as_str() };
                            ui.add(egui::Label::new(egui::RichText::new(text).monospace().small()).wrap());
                        });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if rendering {
                        ui.spinner();
                        if let Some(started) = self.render_started {
                            ui.label(format!("Rendering… {:.0}s", started.elapsed().as_secs_f32()));
                        }
                        cancel = ui.button("■ Cancel").clicked();
                    } else {
                        let label = if self.moonray_default_opts.sequence && !node_selected { "Render sequence" } else { "Render" };
                        start = ui.button(label).clicked();
                    }
                    if ui.button("🖼 Render Viewer").clicked() {
                        self.viewer.show();
                    }
                });
            });
        self.show_moonray_settings = open;
        if start {
            self.start_moonray_render();
        }
        if cancel {
            self.cancel_render();
        }
    }

    // Stops the running render (killing MoonRay) and any sequence it belongs to.
    pub(crate) fn cancel_render(&mut self) {
        if let Some(cancel) = &self.render_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.sequence = None;
        self.render_at = None;
        self.status = "Cancelling render...".into();
    }

    pub(crate) fn spawn_moonray_render(&mut self, executable: String, arguments: String, output: String, width: usize, height: usize, samples: usize) {
        // Lights and cameras are read from the nodes: evaluate them at the frame being rendered.
        let render_time = self.render_at.unwrap_or(self.time);
        self.graph.apply_animation(render_time);
        // Those values are not user edits for Auto Key.
        self.anim_snapshot = None;
        if executable.trim().is_empty() || output.trim().is_empty() {
            self.status = "MoonRay executable and output image are required".into();
            return;
        }
        let scale = self.resolution_scale.clamp(0.05, 1.0);
        let width = ((width as f32 * scale).round() as usize).max(1);
        let height = ((height as f32 * scale).round() as usize).max(1);

        // Preserve the requested output path and export the scene to a writable
        // temporary file to avoid failures when the configured output path is
        // on a read-only filesystem (for example when running from inside a
        // .app bundle Resources directory).
        let _output_path = PathBuf::from(&output);
        
        // Export the scene to temp
        // (use a unique filename to avoid clobbering concurrent renders)
        // Export the scene to a writable temporary file to avoid failures when
        // the configured output path is on a read-only filesystem (for example
        // when running from inside a .app bundle Resources directory).
        let tmp = std::env::temp_dir();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        // Through Hydra: hd_render reads the stage as USD, and renders through its first camera.
        let hydra = self.render_opts.stage && self.render_opts.hydra;
        let mut hydra_camera = None;
        let scene_path = if hydra {
            let usda = tmp.join(format!("proceduralfx_stage_{ts}.usda"));
            match self.write_hydra_scene(&usda) {
                Ok(camera) => hydra_camera = camera,
                Err(error) => {
                    self.status = error;
                    return;
                }
            }
            usda
        } else {
        let scene_filename = format!("proceduralfx_scene_{}.obj", ts);
        let scene_path = tmp.join(scene_filename);
        let scene = match self.render_scene() {
            Ok(scene) => scene,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let scene_parts = &scene.parts;
        let scene_mesh = scene_parts
            .iter()
            .fold(Mesh::default(), |merged, (mesh, _)| node_merge(merged, mesh.clone()));
        if let Err(error) = scene_mesh.export_obj(scene_path.to_string_lossy().as_ref()) {
            self.status = format!("MoonRay scene export failed: {error}");
            return;
        }

        // Also attempt to generate a minimal RDLA wrapper that embeds the
        // exported mesh as vertex/index arrays. MoonRay accepts .rdla files
        // via `-in`, so prefer that when available. This is a best-effort
        // ASCII RDLA writer — the format may need adjustments depending on
        // the MoonRay build; we iterate as needed.
        let rdla_filename = format!("proceduralfx_scene_{}.rdla", ts);
        let rdla_path = tmp.join(rdla_filename);
        let rdla_result = self.export_scene_rdla(&rdla_path, &scene, width, height, samples);
        // If RDLA export succeeded, use it as the scene input; otherwise
        // fall back to the OBJ we wrote above.
        if rdla_result.is_ok() { rdla_path } else { scene_path }
        };

        let replace = |text: &str| {
            text.replace("{scene}", &scene_path.to_string_lossy())
                .replace("{width}", &width.to_string())
                .replace("{height}", &height.to_string())
                .replace("{samples}", &samples.to_string())
        };
        // Resolve executable: if the configured executable doesn't exist or
        // is relative, try to find a bundled MoonRay inside the app bundle
        // (Contents/Resources/moonray or Contents/MacOS/moonray).
        let mut executable_to_run = executable.clone();
        let mut exe_path = PathBuf::from(&executable_to_run);
        if !exe_path.is_absolute() || !exe_path.exists() {
            if let Ok(cur) = std::env::current_exe() {
                if let Some(bundle_dir) = cur.parent().and_then(|p| p.parent()) {
                    let candidates = [
                        bundle_dir.join("Resources").join("moonray"),
                        bundle_dir.join("MacOS").join("moonray"),
                        bundle_dir.join("bin").join("moonray"),
                    ];
                    for c in &candidates {
                        if c.exists() {
                            exe_path = c.clone();
                            break;
                        }
                    }
                }
            }
        }

        // Windows: the helper runs MoonRay or hd_render in a container, and the scene's
        // own file paths (textures, HDRs) are rewritten to the container's view.
        let wsl = wslrender::is_helper(&exe_path.to_string_lossy());
        if wsl {
            if let Err(error) = wslrender::prepare_scene(&scene_path) {
                self.status = error;
                return;
            }
        }
        if hydra && !wsl {
            match find_hd_render(&exe_path) {
                Some(path) => exe_path = path,
                None => {
                    self.status = "Render through Hydra: hd_render was not found (next to MoonRay, in the app, or on PATH)".into();
                    return;
                }
            }
        }

        // Ensure execute permission on Unix-like systems if the file exists.
        if exe_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&exe_path) {
                    let mut perms = meta.permissions();
                    let mode = perms.mode();
                    if mode & 0o111 == 0 {
                        perms.set_mode(mode | 0o111);
                        let _ = std::fs::set_permissions(&exe_path, perms);
                    }
                }
            }
            executable_to_run = exe_path.to_string_lossy().into_owned();
        }

        // Decide where MoonRay should write its output. If the requested
        // output path is not writable (for example inside the app bundle),
        // write to a temp file instead and copy the result back after render.
        let orig_output_pb = PathBuf::from(&output);
        let mut used_output_pb = orig_output_pb.clone();
        let mut copy_back_target = orig_output_pb.clone();
        let tmp = std::env::temp_dir();
        // Relative output paths are ambiguous in packaged apps (Finder launch
        // may set an unwritable working directory). Route them to temp.
        if orig_output_pb.is_relative() {
            let ext = orig_output_pb.extension().and_then(|s| s.to_str()).unwrap_or("png");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            used_output_pb = tmp.join(format!("proceduralfx_output_{}.{}", ts, ext));
            if self.render_at.is_some() {
                // Sequence frames keep their numbered names in one folder.
                let folder = tmp.join("proceduralfx_sequence");
                let _ = std::fs::create_dir_all(&folder);
                if let Some(name) = orig_output_pb.file_name() {
                    used_output_pb = folder.join(name);
                }
            }
            // No copy-back for relative paths; report the temp output path.
            copy_back_target = used_output_pb.clone();
        }
        // try to ensure parent exists and try creating the file to test writability
        let output_writable = (|| {
            if let Some(parent) = used_output_pb.parent() {
                // For relative paths with no explicit parent (e.g.
                // "procedural_output.png"), parent is empty; do not assume
                // writability — we still probe by attempting to create the
                // file below.
                if !parent.as_os_str().is_empty() {
                    if let Err(_) = std::fs::create_dir_all(parent) {
                        return false;
                    }
                }
            }
            match std::fs::OpenOptions::new().write(true).create(true).open(&used_output_pb) {
                Ok(mut f) => {
                    let _ = f.flush();
                    let _ = std::fs::remove_file(&used_output_pb);
                    true
                }
                Err(_) => false,
            }
        })();
        if !output_writable {
            let ext = orig_output_pb.extension().and_then(|s| s.to_str()).unwrap_or("png");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            used_output_pb = tmp.join(format!("proceduralfx_output_{}.{}", ts, ext));
            if copy_back_target.is_relative() {
                copy_back_target = used_output_pb.clone();
            }
        }

        // Now perform argument replacement with the (possibly) adjusted output path
        let arguments = replace(&arguments).replace("{output}", &used_output_pb.to_string_lossy());
        let output_path = used_output_pb.to_string_lossy().into_owned();
        self.pending_render_output = Some(copy_back_target.to_string_lossy().into_owned());
        let (tx, rx) = mpsc::channel();
        self.moonray_rx = Some(rx);
        let cancel = Arc::new(AtomicBool::new(false));
        self.render_cancel = Some(cancel.clone());
        let log_path = tmp.join(format!("proceduralfx_render_{ts}.log"));
        self.render_log_path = Some(log_path.clone());
        self.render_started = Some(Instant::now());
        let extra_args = self.render_extra_args.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        self.status = format!("Rendering with {}: {executable_to_run}", if hydra { "Hydra (hdMoonray)" } else { "MoonRay" });
        // The Render Viewer opens on every render; bucket mode streams the image into it.
        // hd_render only writes the finished image.
        // (Not through the Windows helper: every bucket would start a container.)
        let progress = (self.bucket_render && !hydra && !wsl).then(|| Arc::new(std::sync::Mutex::new(render_viewer::RenderProgress::new(width, height))));
        match &progress {
            Some(progress) => self.viewer.watch(progress.clone()),
            None => self.viewer.show(),
        }
        let exec_run = executable_to_run.clone();
        let copy_back_target_clone = copy_back_target.clone();
        let requested_output = output.clone();
        let scene_for_hydra = scene_path.to_string_lossy().into_owned();
        let hydra_env = hydra.then(|| hydra_plugin_path(&PathBuf::from(&exec_run))).flatten();
        std::thread::spawn(move || {
                // hd_render takes the stage and output directly; MoonRay's argument template doesn't apply.
                let raw_tokens = if hydra {
                    // hdMoonray's in-process renderer: the default one renders through Arras, which
                    // runs MoonRay as a separate compute process with its own session setup.
                    let mut tokens = vec!["-renderer".to_string(), "HdMoonrayRendererDebugPlugin".into(), "-in".into(), scene_for_hydra.clone(), "-out".into(), output_path.clone(), "-size".into(), width.to_string(), height.to_string()];
                    if let Some(camera) = &hydra_camera {
                        tokens.extend(["-camera".to_string(), camera.clone()]);
                    }
                    tokens
                } else {
                    arguments.split_whitespace().map(str::to_owned).collect::<Vec<_>>()
                };
                // Map common templates/flags to MoonRay CLI (-in, -out, -size).
                let mut args = Vec::new();
                let mut i = 0usize;
                while i < raw_tokens.len() {
                    let t = &raw_tokens[i];
                    if t.starts_with("--scene=") {
                        args.push("-in".to_string());
                        args.push(t[8..].to_string());
                    } else if t == "--scene" {
                        args.push("-in".to_string());
                        if i + 1 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); }
                    } else if t.starts_with("--output=") {
                        args.push("-out".to_string());
                        args.push(t[9..].to_string());
                    } else if t == "--output" {
                        args.push("-out".to_string());
                        if i + 1 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); }
                    } else if t.starts_with("--size=") {
                        // --size=WxH or --size W H; try to split by x or comma
                        let val = t[7..].to_string();
                        let mut parts: Vec<&str> = val.split(|c| c == 'x' || c == 'X' || c == ',').collect();
                        if parts.len() == 1 && i + 2 < raw_tokens.len() {
                            parts = vec![&raw_tokens[i+1], &raw_tokens[i+2]];
                            i += 2;
                        }
                        args.push("-size".to_string());
                        for p in parts { args.push(p.to_string()); }
                    } else if t == "--size" {
                        args.push("-size".to_string());
                        if i + 2 < raw_tokens.len() { i += 1; args.push(raw_tokens[i].clone()); i += 1; args.push(raw_tokens[i].clone()); }
                    } else {
                        args.push(t.clone());
                    }
                    i += 1;
                }

                // If the user-provided template produced bare positional
                // tokens (e.g. "{scene} {output} {width} {height} {samples}"),
                // insert MoonRay flags where missing so MoonRay sees `-in` and
                // `-out`. Also ensure a `-size` is present.
                if !args.iter().any(|a| a == "-in") {
                    for idx in 0..args.len() {
                        let t = args[idx].to_lowercase();
                        if t.ends_with(".rdla") || t.ends_with(".rdlb") || t.ends_with(".obj") || t.ends_with(".ply") || t.ends_with(".usd") {
                            args.insert(idx, "-in".to_string());
                            break;
                        }
                    }
                }
                if !args.iter().any(|a| a == "-out") {
                    for idx in 0..args.len() {
                        let t = args[idx].to_lowercase();
                        if t.ends_with(".png") || t.ends_with(".exr") || t.ends_with(".jpg") || t.ends_with(".jpeg") || t.ends_with(".tif") || t.ends_with(".tiff") {
                            args.insert(idx, "-out".to_string());
                            break;
                        }
                    }
                }
                if !args.iter().any(|a| a == "-size") {
                    args.push("-size".to_string());
                    args.push(width.to_string());
                    args.push(height.to_string());
                }

                // Final output-path normalization: regardless of what the
                // user template provided, make MoonRay write to our vetted
                // writable path.
                let mut out_flag_found = false;
                let mut idx = 0usize;
                while idx < args.len() {
                    if args[idx] == "-out" {
                        out_flag_found = true;
                        if idx + 1 < args.len() {
                            args[idx + 1] = output_path.clone();
                        } else {
                            args.push(output_path.clone());
                        }
                        idx += 2;
                        continue;
                    }
                    if args[idx] == requested_output {
                        args[idx] = output_path.clone();
                    }
                    idx += 1;
                }
                if !out_flag_found {
                    args.push("-out".to_string());
                    args.push(output_path.clone());
                }

                // Extra flags are MoonRay's; hd_render rejects flags it doesn't know.
                if !hydra {
                    args.extend(extra_args);
                }
                let mut dso_paths: Option<String> = None;
                // If running from inside an app bundle, help MoonRay find its
                // RDL DSOs by setting RDL2_DSO_PATH to the bundle Resources
                // directory (where packaging should copy DSOs). Also include
                // the executable parent as a fallback.
                if let Ok(cur_exe) = std::env::current_exe() {
                    if let Some(bundle_dir) = cur_exe.parent().and_then(|p| p.parent()) {
                        let mut candidates = Vec::new();
                        candidates.push(bundle_dir.join("Resources").join("rdl2dso"));
                        candidates.push(bundle_dir.join("Resources"));
                        candidates.push(bundle_dir.join("MacOS"));
                        candidates.push(bundle_dir.join("rdl2dso"));
                        if let Ok(existing) = std::env::var("RDL2_DSO_PATH") {
                            for p in existing.split(':') {
                                if !p.is_empty() {
                                    candidates.push(std::path::PathBuf::from(p));
                                }
                            }
                        }
                        // join as colon-separated paths
                        let paths = candidates.iter()
                            .filter(|p| p.exists())
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(":");
                        if !paths.is_empty() {
                            dso_paths = Some(paths);
                        }
                    }
                }
                let make_cmd = |args: &[String]| {
                    let mut cmd = Command::new(&exec_run);
                    platform::no_window(&mut cmd);
                    if wsl {
                        // The container has MoonRay's own environment.
                        cmd.arg(if hydra { "hd_render" } else { "moonray" });
                        cmd.args(args);
                        return cmd;
                    }
                    cmd.args(args);
                    if let Some(paths) = &dso_paths {
                        cmd.env("RDL2_DSO_PATH", paths);
                    }
                    // hdMoonray's USD plugin: the bundled one is found next to USD itself.
                    if let Some(plugins) = &hydra_env {
                        let existing = std::env::var("PXR_PLUGINPATH_NAME").unwrap_or_default();
                        cmd.env("PXR_PLUGINPATH_NAME", if existing.is_empty() { plugins.clone() } else { format!("{plugins}:{existing}") });
                    }
                    cmd
                };
                let outcome: Result<(), String> = match &progress {
                    Some(progress) => render_buckets(&make_cmd, &args, &output_path, width, height, &log_path, &cancel, progress),
                    None => run_logged(&mut make_cmd(&args), &log_path, &cancel)
                        .map_err(|error| if cancel.load(Ordering::Relaxed) { error } else { format!("Could not start {exec_run}: {error}") })
                        .and_then(|status| {
                            if status.success() {
                                Ok(())
                            } else {
                                let tail = read_log_tail(&log_path, 600);
                                let hint = wslrender::failure_hint(&tail).map(|h| format!("{h}. ")).unwrap_or_default();
                                Err(format!("{hint}{} exited with {status}: {}", if hydra { "hd_render" } else { "MoonRay" }, tail.trim()))
                            }
                        }),
                };
                let result = outcome.and_then(|()| {
                    {
                        let rendered_pb = PathBuf::from(&output_path);
                        let _ = make_png_opaque(&rendered_pb);
                        let replaced_preview = ensure_png_not_all_black(&rendered_pb).unwrap_or(false);
                        // If we rendered to a temp file because the requested
                        // output path was not writable, copy it back to the
                        // original requested location.
                        if copy_back_target_clone != PathBuf::from(&output_path) {
                            let _ = std::fs::create_dir_all(copy_back_target_clone.parent().unwrap_or_else(|| std::path::Path::new(".")));
                            match std::fs::copy(&output_path, &copy_back_target_clone) {
                                Ok(_) => {
                                    let _ = make_png_opaque(&copy_back_target_clone);
                                    let _ = ensure_png_not_all_black(&copy_back_target_clone);
                                    if replaced_preview {
                                        return Ok((format!("MoonRay render complete: {} (copied to {}) [fallback preview inserted because output was fully black]", output_path, copy_back_target_clone.display()), copy_back_target_clone.clone()));
                                    }
                                    return Ok((format!("MoonRay render complete: {} (copied to {})", output_path, copy_back_target_clone.display()), copy_back_target_clone.clone()));
                                }
                                Err(e) => return Err(format!("Render complete but failed to copy output to {}: {}", copy_back_target_clone.display(), e)),
                            }
                        }
                        if replaced_preview {
                            return Ok((format!("MoonRay render complete: {output_path} [fallback preview inserted because output was fully black]"), rendered_pb));
                        }
                        Ok((format!("MoonRay render complete: {output_path}"), rendered_pb))
                    }
                });
            let log = read_log_tail(&log_path, 256 * 1024);
            let _ = tx.send(RenderOutcome { result, log });
        });
    }

    pub(crate) fn poll_moonray_render(&mut self) {
        let Some(rx) = self.moonray_rx.as_ref() else {
            return;
        };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Disconnected) => RenderOutcome {
                result: Err("MoonRay render worker disconnected".to_string()),
                log: String::new(),
            },
            Err(mpsc::TryRecvError::Empty) => {
                // The Windows helper says what it's doing (downloading MoonRay the first time).
                if self.render_log_checked.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(500)) {
                    self.render_log_checked = Some(Instant::now());
                    if let Some(step) = self.render_log_path.as_ref().map(|p| read_log_tail(p, 4096)).as_deref().and_then(wslrender::progress) {
                        self.status = step;
                    }
                }
                return;
            }
        };
        self.moonray_rx = None;
        self.render_cancel = None;
        let seconds = self.render_started.take().map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
        self.last_render_log = outcome.log;
        self.render_ok = Some(outcome.result.is_ok());
        if outcome.result.is_ok() {
            if let Some(path) = self.pending_render_output.take() {
                cops::set_last_render(&path);
                // Render COPs read it again.
                self.dirty = true;
            }
        }
        // A sequence's frames are one entry in the viewer, played as an animation.
        let frame_of = self.sequence.as_ref().map(|job| (job.next <= 1, sequence_fps(&job.times)));
        let outcome = outcome.result.map(|(message, image)| {
            match frame_of {
                Some((first, fps)) => self.viewer.add_frame(image, first, fps, self.open_viewer_after_render),
                None => self.viewer.add(image, self.open_viewer_after_render),
            }
            format!("{message} in {seconds:.1}s")
        });
        self.last_render_summary = match &outcome {
            Ok(message) => message.clone(),
            Err(error) => format!("Failed after {seconds:.1}s: {error}"),
        };
        if self.sequence.is_none() {
            self.status = outcome.unwrap_or_else(|error| error);
            return;
        }
        // Within a sequence only failures surface; advance_sequence reports progress.
        if let Err(error) = outcome {
            self.sequence = None;
            self.render_at = None;
            self.status = format!("Sequence stopped: {error}");
        }
    }

    pub(crate) fn export_scene_rdla(
        &self,
        path: &std::path::Path,
        scene: &RenderScene,
        width: usize,
        height: usize,
        samples: usize,
    ) -> Result<(), String> {
        use std::fs::File;
        use std::io::Write;

        let mut f = File::create(path).map_err(|e| format!("Failed to create RDLA file {}: {}", path.display(), e))?;
        // Best-effort RDLA writer: one RdlMeshGeometry per active geometry,
        // MoonRay materials from MAT nodes, lights, and a camera.
        writeln!(f, "-- RDLA generated by ProceduralFX Studio") .map_err(|e| e.to_string())?;
        writeln!(f, "") .map_err(|e| e.to_string())?;

        let opts = &self.render_opts;
        writeln!(f, "SceneVariables {{") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"image_width\"] = {},", width).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"image_height\"] = {},", height).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"output_file\"] = \"\",") .map_err(|e| e.to_string())?;
        // MoonRay's pixel_samples is per axis, so N samples per pixel is sqrt(N) per side.
        writeln!(f, "    [\"pixel_samples\"] = {},", (samples.max(1) as f32).sqrt().ceil() as usize) .map_err(|e| e.to_string())?;
        // Adaptive sampling stops early where pixels have converged (measured: same image,
        // fewer samples); the sample count is the maximum.
        writeln!(f, "    [\"sampling_mode\"] = 2,") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"min_adaptive_samples\"] = {},", (samples / 4).max(4).min(samples.max(1))) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"max_adaptive_samples\"] = {},", samples.max(1)) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"target_adaptive_error\"] = 1.0,") .map_err(|e| e.to_string())?;
        if !scene.volumes.is_empty() {
            // Finer ray marching and one bounce inside volumes keep smoke from rendering too dark.
            writeln!(f, "    [\"volume_quality\"] = 1.0,") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"volume_indirect_samples\"] = 1,") .map_err(|e| e.to_string())?;
            // In-scattering samples dominate volume render time (each marches shadow rays).
            writeln!(f, "    [\"volume_illumination_samples\"] = {},", opts.volume_samples.max(1)) .map_err(|e| e.to_string())?;
        }
        writeln!(f, "    [\"max_depth\"] = {},", opts.max_depth) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"light_samples\"] = {},", opts.light_samples) .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"bsdf_samples\"] = {},", opts.bsdf_samples) .map_err(|e| e.to_string())?;
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        let parts = &scene.parts;
        // The viewport is left-handed and MoonRay right-handed, so z is mirrored
        // here and in the camera, and triangle winding is swapped to match.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            if mesh.indices.is_empty() {
                continue;
            }
            writeln!(f, "RdlMeshGeometry(\"/scene/objects/mesh{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"vertex_list_0\"] = {{") .map_err(|e| e.to_string())?;
            for vertex in &mesh.vertices {
                writeln!(f, "        Vec3({:.9}, {:.9}, {:.9}),", vertex.x, vertex.y, -vertex.z).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"vertices_by_index\"] = {{") .map_err(|e| e.to_string())?;
            for tri in &mesh.indices {
                writeln!(f, "        {},", tri[0]).map_err(|e| e.to_string())?;
                writeln!(f, "        {},", tri[2]).map_err(|e| e.to_string())?;
                writeln!(f, "        {},", tri[1]).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"face_vertex_count\"] = {{") .map_err(|e| e.to_string())?;
            for _ in &mesh.indices {
                writeln!(f, "        3,") .map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_list\"] = {{ \"tri_shape_0\" }},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_face_count_list\"] = {{ {} }},", mesh.indices.len()) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"part_face_indices\"] = {{") .map_err(|e| e.to_string())?;
            for i in 0..mesh.indices.len() {
                writeln!(f, "        {},", i).map_err(|e| e.to_string())?;
            }
            writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            // UVs per face-vertex (in the swapped winding) for image textures.
            if let Some(uvs) = sops::corner_uvs(mesh) {
                writeln!(f, "    [\"uv_list\"] = {{") .map_err(|e| e.to_string())?;
                for uv in &uvs {
                    for k in [0, 2, 1] {
                        writeln!(f, "        Vec2({:.6}, {:.6}),", uv[k][0], uv[k][1]).map_err(|e| e.to_string())?;
                    }
                }
                writeln!(f, "    }},") .map_err(|e| e.to_string())?;
            }
            // Cd as user data, for shaders that read point colour (rate guessed from the count).
            let cd = mesh
                .point_attributes
                .get("Cd")
                .filter(|c| c.len() == mesh.vertices.len() * 3)
                .or_else(|| mesh.point_attributes.get("prim:Cd").filter(|c| c.len() == mesh.indices.len() * 3));
            // Alpha too, for shaders using point alpha.
            let alpha = mesh
                .point_attributes
                .get("Alpha")
                .filter(|a| a.len() == mesh.vertices.len())
                .or_else(|| mesh.point_attributes.get("prim:Alpha").filter(|a| a.len() == mesh.indices.len()));
            let user_data: Vec<String> = [cd.map(|_| "Cd"), alpha.map(|_| "Alpha")].into_iter().flatten().map(|key| format!("UserData(\"/scene/objects/mesh{index}_{key}\")")).collect();
            if !user_data.is_empty() {
                writeln!(f, "    [\"primitive_attributes\"] = {{ {} }},", user_data.join(", ")) .map_err(|e| e.to_string())?;
            }
            writeln!(f, "    [\"is_subd\"] = false") .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            if let Some(cd) = cd {
                let mut body = format!("UserData(\"/scene/objects/mesh{index}_Cd\") {{\n    [\"color_key\"] = \"Cd\",\n    [\"color_values_0\"] = {{\n");
                for c in cd.chunks(3) {
                    body.push_str(&format!("        Rgb({:.4}, {:.4}, {:.4}),\n", c[0], c[1], c[2]));
                }
                body.push_str("    }\n}\n\n");
                f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            }
            if let Some(alpha) = alpha {
                let mut body = format!("UserData(\"/scene/objects/mesh{index}_Alpha\") {{\n    [\"float_key\"] = \"Alpha\",\n    [\"float_values_0\"] = {{\n");
                for a in alpha {
                    body.push_str(&format!("        {a:.4},\n"));
                }
                body.push_str("    }\n}\n\n");
                f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            }
        }

        // Hair and other curves: linear RdlCurveGeometry, radius from the width attribute.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            let curves = groom::curves(mesh);
            if curves.is_empty() {
                continue;
            }
            let width = mesh.point_attributes.get("width").filter(|w| w.len() == mesh.vertices.len());
            let mut body = format!("RdlCurveGeometry(\"/scene/objects/curves{index}\") {{\n    [\"curve_type\"] = 0,\n    [\"vertex_list_0\"] = {{\n");
            for range in &curves {
                for v in &mesh.vertices[range.clone()] {
                    body.push_str(&format!("        Vec3({:.7}, {:.7}, {:.7}),\n", v.x, v.y, -v.z));
                }
            }
            body.push_str("    },\n    [\"curves_vertex_count\"] = {\n");
            for range in &curves {
                body.push_str(&format!("        {},\n", range.len()));
            }
            body.push_str("    },\n    [\"radius_list\"] = {\n");
            for range in &curves {
                for i in range.clone() {
                    body.push_str(&format!("        {:.6},\n", width.map_or(0.005, |w| w[i] * 0.5).max(1e-5)));
                }
            }
            body.push_str("    }\n}\n\n");
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }

        // Whitewater and other marked points: spheres of radius pscale.
        for (index, (mesh, _)) in parts.iter().enumerate() {
            if !renders_points(mesh) {
                continue;
            }
            let pscale = mesh.point_attributes.get("pscale").filter(|p| p.len() == mesh.vertices.len());
            let mut body = format!("RdlPointGeometry(\"/scene/objects/points{index}\") {{\n    [\"vertex_list_0\"] = {{\n");
            for v in &mesh.vertices {
                body.push_str(&format!("        Vec3({:.6}, {:.6}, {:.6}),\n", v.x, v.y, -v.z));
            }
            body.push_str("    },\n    [\"radius_list\"] = {\n");
            for i in 0..mesh.vertices.len() {
                body.push_str(&format!("        {:.6},\n", pscale.map_or(0.01, |p| p[i]).max(1e-5)));
            }
            body.push_str("    }\n}\n\n");
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }

        // Fog volumes: VdbGeometry reading the written .vdb, shaded by VdbVolume.
        for (index, vol) in scene.volumes.iter().enumerate() {
            writeln!(f, "VdbGeometry(\"/scene/volumes/volume{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"model\"] = \"{}\",", vol.file.replace('\\', "/")) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"density_grid\"] = \"density\",") .map_err(|e| e.to_string())?;
            if vol.emission.is_some() {
                writeln!(f, "    [\"emission_grid\"] = \"emission\",") .map_err(|e| e.to_string())?;
                // Sample the fire densely so flames render smooth instead of speckled.
                writeln!(f, "    [\"emission_sample_rate\"] = 1.0,") .map_err(|e| e.to_string())?;
            }
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            let emission = vol.emission.unwrap_or(0.0);
            writeln!(f, "VdbVolume(\"/scene/volumes/shader{index}\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"opacity_gain_mult\"] = Rgb({0:.5}, {0:.5}, {0:.5}),", vol.extinction) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color_mult\"] = Rgb({:.4}, {:.4}, {:.4}),", vol.albedo[0], vol.albedo[1], vol.albedo[2]) .map_err(|e| e.to_string())?;
            let tint = vol.emission_tint.map(|t| t * emission);
            writeln!(f, "    [\"incandescence_gain_mult\"] = Rgb({:.4}, {:.4}, {:.4}),", tint[0], tint[1], tint[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"anisotropy\"] = {:.4}", vol.anisotropy) .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
        }

        // Each part renders as a mesh, curves, or both.
        let part_geometry: Vec<Vec<String>> = parts
            .iter()
            .enumerate()
            .map(|(index, (mesh, _))| {
                let mut refs = Vec::new();
                if !mesh.indices.is_empty() {
                    refs.push(format!("RdlMeshGeometry(\"/scene/objects/mesh{index}\")"));
                }
                if groom::has_curves(mesh) && !groom::curves(mesh).is_empty() {
                    refs.push(format!("RdlCurveGeometry(\"/scene/objects/curves{index}\")"));
                }
                if renders_points(mesh) {
                    refs.push(format!("RdlPointGeometry(\"/scene/objects/points{index}\")"));
                }
                refs
            })
            .collect();
        writeln!(f, "GeometrySet(\"/scene/geometrysets/GeometrySet1\") {{") .map_err(|e| e.to_string())?;
        for geometry in part_geometry.iter().flatten() {
            writeln!(f, "    {geometry},") .map_err(|e| e.to_string())?;
        }
        for index in 0..scene.volumes.len() {
            writeln!(f, "    VdbGeometry(\"/scene/volumes/volume{index}\"),") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        // Lights come from active Light nodes (or the default key/fill rig).
        // z is mirrored like the mesh; MoonRay lights emit along local -Z.
        let mat4 = |x: Vec3, y: Vec3, z: Vec3, p: Vec3| {
            let m = format!(
                "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},1)",
                x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z, p.x, p.y, p.z
            );
            format!("blur({m}, {m})")
        };
        let mut light_names: Vec<String> = Vec::new();
        for (index, (position, direction, color, intensity, kind)) in scene.lights.iter().enumerate() {
            let name = format!("/scene/lights/light{index}");
            let rgb = format!("Rgb({:.4}, {:.4}, {:.4})", color[0], color[1], color[2]);
            if *kind == 1 {
                let xform = mat4(
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(0.0, 0.0, 1.0),
                    Vec3::new(position.x, position.y, -position.z),
                );
                writeln!(f, "SphereLight(\"{name}\") {{") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"node_xform\"] = {xform},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"color\"] = {rgb},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"intensity\"] = {:.4},", intensity * POINT_LIGHT_SCALE) .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"radius\"] = 0.1,") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"visible_in_camera\"] = \"force off\",") .map_err(|e| e.to_string())?;
                writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            } else {
                let z = Vec3::new(-direction.x, -direction.y, direction.z).normalize();
                let hint = if z.y.abs() > 0.99 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
                let x = hint.cross(z).normalize();
                let y = z.cross(x);
                let xform = mat4(x, y, z, Vec3::zero());
                writeln!(f, "DistantLight(\"{name}\") {{") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"node_xform\"] = {xform},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"color\"] = {rgb},") .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"intensity\"] = {:.4},", intensity * DISTANT_LIGHT_SCALE) .map_err(|e| e.to_string())?;
                writeln!(f, "    [\"angular_extent\"] = {},", if *kind == 2 { 10.0 } else { 0.53 }) .map_err(|e| e.to_string())?;
                writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            }
            light_names.push(format!("{}(\"{name}\")", if *kind == 1 { "SphereLight" } else { "DistantLight" }));
        }

        // One MoonRay material per MAT node used by a geometry, plus a neutral default.
        let (default_class, default_body) = rdla_material("/scene/materials/default", 0, [0.8; 3], 0.5, 0.5);
        write!(f, "{default_body}\n") .map_err(|e| e.to_string())?;
        let mut written: Vec<NodeId> = Vec::new();
        let mut layer_materials: Vec<String> = Vec::new();
        for (index, (_, material)) in parts.iter().enumerate() {
            let node = match material {
                PartMaterial::Node(id) => self.graph.nodes.get(id).map(|n| (*id, &n.kind)),
                _ => None,
            };
            let reference = match (material, node) {
                (_, Some((id, NodeKind::Shader(v)))) if v.table != shading::Table::Pyro => {
                    let name = format!("/scene/materials/mat{id}");
                    let mut objects = String::new();
                    let (class, body) = match v.table {
                        shading::Table::Toon => shading::toon_rdla(&name, v, &mut objects),
                        shading::Table::Hair => shading::hair_rdla(&name, v),
                        _ => shading::principled_rdla(&name, v, &HashMap::new(), &mut objects),
                    };
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{objects}{body}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (_, Some((id, NodeKind::MaterialBuilder))) => {
                    let name = format!("/scene/materials/mat{id}");
                    let output = self.graph.builder_output(id).ok_or("Material Builder: add a Surface Output node inside it")?;
                    let nodes = self.graph.builder_nodes(id);
                    let (class, text) = shading::compile_builder(&name, &nodes, |to, slot| self.graph.input_of(to, slot), output)
                        .map_err(|e| format!("Material Builder {}: {e}", self.graph.nodes[&id].title(id)))?;
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{text}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (_, Some((id, NodeKind::Material { kind, color, roughness, value }))) => {
                    let name = format!("/scene/materials/mat{id}");
                    let (class, body) = rdla_material(&name, *kind, *color, *roughness, *value);
                    if !written.contains(&id) {
                        written.push(id);
                        write!(f, "{body}\n") .map_err(|e| e.to_string())?;
                    }
                    format!("{class}(\"{name}\")")
                }
                (PartMaterial::Preview { diffuse, roughness, metallic, opacity, emissive, ior }, _) => {
                    // A USD preview surface mapped onto the closest MAT material type.
                    let name = format!("/scene/materials/usd{index}");
                    let emission = emissive.iter().cloned().fold(0.0, f32::max);
                    let (kind, color, value) = if emission > 0.0 {
                        (3, emissive.map(|c| c / emission), emission)
                    } else if *metallic > 0.5 {
                        (1, *diffuse, 0.5)
                    } else if *opacity < 0.5 {
                        (2, *diffuse, *ior)
                    } else {
                        (0, *diffuse, 0.5)
                    };
                    let (class, body) = rdla_material(&name, kind, color, *roughness, value);
                    write!(f, "{body}\n") .map_err(|e| e.to_string())?;
                    format!("{class}(\"{name}\")")
                }
                _ => format!("{default_class}(\"/scene/materials/default\")"),
            };
            layer_materials.push(reference);
        }

        // An HDRI environment light replaces the uniform ambient environment below.
        let hdri = self.environment_light();
        if let Some((file, color, rotation, visible)) = &hdri {
            envlight::load(file).map_err(|e| format!("Environment light: {e}"))?;
            let r = envlight::rotation_rows(*rotation);
            let m = format!(
                "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, 0,0,0,1)",
                r[0].x, r[0].y, r[0].z, r[1].x, r[1].y, r[1].z, r[2].x, r[2].y, r[2].z
            );
            writeln!(f, "EnvLight(\"/scene/lights/hdri\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"texture\"] = \"{}\",", file.replace('\\', "/").replace('"', "\\\"")) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color\"] = Rgb({:.4}, {:.4}, {:.4}),", color[0], color[1], color[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"visible_in_camera\"] = {},", if *visible { 1 } else { 0 }) .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"node_xform\"] = blur({m}, {m})") .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            light_names.push("EnvLight(\"/scene/lights/hdri\")".to_string());
        }
        // The viewport's ambient term becomes a dim environment light (decoded from sRGB).
        let ambient = self.ambient_light.max(0.0).powf(2.2);
        let environment: [f32; 3] = std::array::from_fn(|i| opts.background[i] + scene.environment.map_or(0.0, |e| e[i]) + ambient);
        if hdri.is_none() && environment.iter().any(|c| *c > 0.0) {
            writeln!(f, "EnvLight(\"/scene/lights/env\") {{") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"color\"] = Rgb({:.4}, {:.4}, {:.4}),", environment[0], environment[1], environment[2]) .map_err(|e| e.to_string())?;
            writeln!(f, "}}\n") .map_err(|e| e.to_string())?;
            light_names.push("EnvLight(\"/scene/lights/env\")".to_string());
        }

        writeln!(f, "LightSet(\"/scene/lightsets/LightSet1\") {{") .map_err(|e| e.to_string())?;
        for light in &light_names {
            writeln!(f, "    {light},") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        writeln!(f, "Layer(\"/scene/layers/Layer1\") {{") .map_err(|e| e.to_string())?;
        for (geometry, material) in part_geometry.iter().zip(&layer_materials) {
            for geometry in geometry {
                writeln!(f, "    {{{geometry}, \"\", {material}, LightSet(\"/scene/lightsets/LightSet1\"), undef(), undef(), undef(), undef()}},") .map_err(|e| e.to_string())?;
            }
        }
        for index in 0..scene.volumes.len() {
            writeln!(f, "    {{VdbGeometry(\"/scene/volumes/volume{index}\"), \"\", LightSet(\"/scene/lightsets/LightSet1\"), VdbVolume(\"/scene/volumes/shader{index}\")}},") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}\n") .map_err(|e| e.to_string())?;

        // Camera from the stage, the selected/chosen camera node, or the viewport.
        let (rig, _) = self.resolve_render_camera();
        let (sx, cx) = rig.rot_x.sin_cos();
        let (sy, cy) = rig.rot_y.sin_cos();
        let mut right = Vec3::new(cy, 0.0, -sy);
        let mut up = Vec3::new(-sx * sy, cx, -sx * cy);
        let mut fwd = Vec3::new(cx * sy, sx, cx * cy);
        let pan_scale = rig.zoom / self.viewport_focal_px.max(1.0);
        let target = rig
            .target
            .sub(right.scale(rig.pan[0] * pan_scale))
            .add(up.scale(rig.pan[1] * pan_scale));
        let mut eye = target.sub(fwd.scale(rig.zoom));
        let mut lens = None;
        if let Some((stage_eye, stage_right, stage_up, stage_fwd, focal, aperture)) = scene.camera {
            (eye, right, up, fwd) = (stage_eye, stage_right, stage_up, stage_fwd);
            lens = Some((focal, aperture));
        }
        // z is mirrored (see mesh export); MoonRay cameras look down local -Z.
        let xform = format!(
            "Mat4({:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},0, {:.6},{:.6},{:.6},1)",
            right.x, right.y, -right.z,
            up.x, up.y, -up.z,
            -fwd.x, -fwd.y, fwd.z,
            eye.x, eye.y, -eye.z,
        );
        let min_dim = width.min(height).max(1) as f32;
        let aspect_w = width.max(1) as f32;
        let orthographic = rig.orthographic && lens.is_none();
        writeln!(f, "{}(\"/scene/cameras/Camera1\") {{", if orthographic { "OrthographicCamera" } else { "PerspectiveCamera" }).map_err(|e| e.to_string())?;
        writeln!(f, "    [\"node_xform\"] = blur({xform}, {xform}),") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"near\"] = 0.001,") .map_err(|e| e.to_string())?;
        writeln!(f, "    [\"far\"] = 100000,") .map_err(|e| e.to_string())?;
        if let Some((focal, aperture)) = lens {
            writeln!(f, "    [\"film_width_aperture\"] = {aperture:.6},") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"focal\"] = blur({focal:.6}, {focal:.6})") .map_err(|e| e.to_string())?;
        } else if rig.orthographic {
            writeln!(f, "    [\"film_width_aperture\"] = {:.6}", rig.zoom / rig.focal * aspect_w / min_dim) .map_err(|e| e.to_string())?;
        } else {
            // Match the viewport's field of view along the smaller image dimension.
            let focal_mm = 24.0 * min_dim * rig.focal / aspect_w;
            writeln!(f, "    [\"film_width_aperture\"] = 24,") .map_err(|e| e.to_string())?;
            writeln!(f, "    [\"focal\"] = blur({focal_mm:.6}, {focal_mm:.6})") .map_err(|e| e.to_string())?;
        }
        writeln!(f, "}}") .map_err(|e| e.to_string())?;
        Ok(())
    }

    // (position, travel direction, color, intensity, kind) for every active Light node,
    // or the viewport's default key/fill rig when the scene has none.
    // The active (first, not bypassed) environment light with a file: (file, color * intensity, rotation, visible).
    pub(crate) fn environment_light(&self) -> Option<(String, [f32; 3], f32, bool)> {
        let mut ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter().find_map(|id| {
            let node = &self.graph.nodes[&id];
            match &node.kind {
                NodeKind::EnvironmentLight { file, color, intensity, rotation, visible } if !node.bypass && node.parent.is_none() && !file.trim().is_empty() => {
                    Some((file.trim().to_owned(), color.map(|c| c * intensity), *rotation, *visible))
                }
                _ => None,
            }
        })
    }

    // The scene's lights for shading smoke in the real-time renderer: none when it has no
    // light nodes or environment light, so smoke keeps the solver's own overhead shading.
    pub(crate) fn volume_lights(&self) -> Vec<sims::VolumeLight> {
        let has_lights = self.graph.nodes.values().any(|n| !n.bypass && n.parent.is_none() && matches!(n.kind, NodeKind::Light { .. }));
        if !has_lights && self.environment_light().is_none() {
            return Vec::new();
        }
        let mut lights: Vec<sims::VolumeLight> = self
            .scene_lights()
            .into_iter()
            .map(|(position, direction, color, intensity, kind)| match kind {
                1 => sims::VolumeLight::Sphere { position, color, intensity, falloff: POINT_LIGHT_FALLOFF },
                2 => sims::VolumeLight::Dome { color, intensity },
                _ => sims::VolumeLight::Distant { to_light: direction.scale(-1.0), color, intensity },
            })
            .collect();
        if self.environment_light().is_some() {
            lights.push(sims::VolumeLight::Dome { color: [1.0; 3], intensity: 1.0 });
        }
        lights
    }

    pub(crate) fn scene_lights(&self) -> Vec<(Vec3, Vec3, [f32; 3], f32, usize)> {
        let mut ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        ids.sort_unstable();
        let lights: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                let node = &self.graph.nodes[&id];
                if node.bypass || node.parent.is_some() {
                    return None;
                }
                let NodeKind::Light { position, rotation, color, intensity, kind } = &node.kind else {
                    return None;
                };
                Some((
                    Vec3::from_arr(*position),
                    Vec3::new(0.0, 0.0, -1.0).rotate_deg(*rotation).normalize(),
                    *color,
                    *intensity,
                    *kind,
                ))
            })
            .collect();
        // Any light, including an HDRI, replaces the default key/fill rig.
        if !lights.is_empty() || self.environment_light().is_some() {
            return lights;
        }
        vec![
            (
                Vec3::zero(),
                Vec3::from_arr(self.key_light_dir).normalize(),
                [1.0; 3],
                self.key_light_intensity.max(0.0),
                0,
            ),
            (
                Vec3::zero(),
                Vec3::from_arr(self.fill_light_dir).normalize(),
                [1.0; 3],
                self.fill_light_intensity.max(0.0),
                2,
            ),
        ]
    }

    // Active geometries with their MAT assignment, regardless of which network the viewport shows.
    // The displayed USD stage written for hd_render; returns the camera to render
    // through (the stage's first camera with the Auto camera, else hd_render's own view).
    pub(crate) fn write_hydra_scene(&self, path: &std::path::Path) -> Result<Option<String>, String> {
        let display = self.graph.display_of(STAGE_NETWORK).ok_or("Render STAGE: no LOP node has the display flag in the STAGE network")?;
        let time = self.render_at.unwrap_or(self.time);
        let mut stage = (*self.graph.cook_stage(display, time, &mut HashMap::new())).clone();
        // Light the stage as the MoonRay render does: normalized lights (brightness doesn't
        // depend on their size; USD's default isn't), with the same sphere light scale.
        for prim in stage.prims.values_mut().filter(|prim| prim.is_light() && prim.type_name != "DomeLight") {
            prim.attrs.insert("normalize".into(), usd::AttrValue::Bool(true));
            if prim.type_name == "SphereLight" {
                if let Some(usd::AttrValue::Float(intensity)) = prim.attrs.get_mut("intensity") {
                    *intensity *= POINT_LIGHT_SCALE;
                }
            }
        }
        std::fs::write(path, usd::to_usda(&stage)).map_err(|error| format!("Could not write {}: {error}", path.display()))?;
        let auto = self.render_opts.camera == RenderCamera::Auto;
        // hd_render wants the camera path relative to the stage root.
        Ok(stage.prims.iter().find(|(p, prim)| auto && prim.type_name == "Camera" && stage.is_shown(p)).map(|(p, _)| p.trim_start_matches('/').to_string()))
    }

    pub(crate) fn render_scene_parts(&self) -> Vec<(Mesh, Option<NodeId>)> {
        let material_of = |id: NodeId| match self.graph.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Geometry { material }) => *material,
            _ => None,
        };
        let active: Vec<NodeId> = self
            .graph
            .geometry_ids()
            .into_iter()
            .filter(|id| !self.graph.nodes[id].bypass)
            .collect();
        let time = self.render_at.unwrap_or(self.time);
        if self.render_at.is_none() && active.len() == 1 && self.network().is_none() && self.context != AppContext::Stage {
            return vec![(self.current_mesh.clone(), material_of(active[0]))];
        }
        let mut cache = self.valid_cache(&self.graph.cook_fingerprints(scene_fingerprint(&self.graph.serialize())));
        active
            .into_iter()
            .map(|id| {
                let mesh = self.graph.cook(id, time, &mut cache);
                (if packed::is_packed(&mesh) { packed::unpack(&mesh, true) } else { mesh }, material_of(id))
            })
            .filter(|(mesh, _)| !mesh.indices.is_empty() || volume::is_volume(mesh) || groom::has_curves(mesh) || renders_points(mesh))
            .collect()
    }

    // The scene to render: the displayed USD stage when the render options ask for it, else the OBJ objects.
    pub(crate) fn render_scene(&self) -> Result<RenderScene, String> {
        if !self.render_opts.stage {
            let mut parts = Vec::new();
            let mut volumes = Vec::new();
            for (index, (mesh, material)) in self.render_scene_parts().into_iter().enumerate() {
                if !volume::is_volume(&mesh) {
                    // Material SOP assignments split the object into one part per material.
                    let is_material = |m: NodeId| self.graph.nodes.get(&m).is_some_and(|n| n.kind.is_material());
                    for (part, m) in split_by_material(mesh, material, is_material) {
                        parts.push((part, m.map_or(PartMaterial::Default, PartMaterial::Node)));
                    }
                    continue;
                }
                let material = material.map_or(PartMaterial::Default, PartMaterial::Node);
                let grid = volume::from_mesh(&mesh)?;
                if grid.class == volume::Class::LevelSet {
                    // Level sets render as their surface.
                    parts.push((volume::to_polygons(&grid, 0.0), material));
                } else {
                    let mut volume = write_render_volume(&mesh, index)?;
                    if let PartMaterial::Node(m) = material {
                        if let Some(NodeKind::Shader(v)) = self.graph.nodes.get(&m).map(|n| &n.kind).filter(|k| matches!(k, NodeKind::Shader(v) if v.table == shading::Table::Pyro)) {
                            volume.extinction *= v.float("smoke_density");
                            volume.albedo = v.color("smoke_color");
                            volume.emission = volume.emission.map(|e| e * v.float("fire_intensity"));
                            volume.emission_tint = v.color("fire_color");
                            volume.anisotropy = v.float("anisotropy");
                        }
                    }
                    volumes.push(volume);
                }
            }
            return Ok(RenderScene { parts, lights: self.scene_lights(), environment: None, camera: None, volumes });
        }
        let display = self.graph.display_of(STAGE_NETWORK).ok_or("Render STAGE: no LOP node has the display flag in the STAGE network")?;
        let time = self.render_at.unwrap_or(self.time);
        let stage = self.graph.cook_stage(display, time, &mut HashMap::new());
        let parts = stage
            .flatten()
            .into_iter()
            .filter(|(_, mesh, _)| !mesh.indices.is_empty())
            .map(|(_, mesh, material)| {
                let material = material.as_deref().and_then(|path| stage.material(path)).map_or(PartMaterial::Default, |m| {
                    match m.material_node.filter(|id| self.graph.nodes.contains_key(id)) {
                        Some(node) => PartMaterial::Node(node),
                        None => PartMaterial::Preview {
                            diffuse: m.vec3("diffuseColor").unwrap_or([0.18; 3]),
                            roughness: m.float("roughness").unwrap_or(0.5),
                            metallic: m.float("metallic").unwrap_or(0.0),
                            opacity: m.float("opacity").unwrap_or(1.0),
                            emissive: m.vec3("emissiveColor").unwrap_or([0.0; 3]),
                            ior: m.float("ior").unwrap_or(1.5),
                        },
                    }
                });
                (mesh, material)
            })
            .collect();
        let mut lights = Vec::new();
        let mut environment: Option<[f32; 3]> = None;
        let mut camera = None;
        for (path, prim) in &stage.prims {
            if !stage.is_shown(path) {
                continue;
            }
            let world = stage.world_xform(path);
            let colour = prim.vec3("color").unwrap_or([1.0; 3]);
            let intensity = prim.float("intensity").unwrap_or(1.0) * 2f32.powf(prim.float("exposure").unwrap_or(0.0));
            match prim.type_name.as_str() {
                "DomeLight" => {
                    let add = colour.map(|c| c * intensity);
                    let current = environment.unwrap_or([0.0; 3]);
                    environment = Some(std::array::from_fn(|i| current[i] + add[i]));
                }
                "DistantLight" => lights.push((usd::row(&world, 3), usd::row(&world, 2).normalize(), colour, intensity, 0)),
                kind if kind.ends_with("Light") => lights.push((usd::row(&world, 3), usd::row(&world, 2).normalize(), colour, intensity, 1)),
                "Camera" if camera.is_none() && self.render_opts.camera == RenderCamera::Auto => {
                    camera = Some((
                        usd::row(&world, 3),
                        usd::row(&world, 0).normalize(),
                        usd::row(&world, 1).normalize(),
                        usd::row(&world, 2).normalize(),
                        prim.float("focalLength").unwrap_or(50.0),
                        prim.float("horizontalAperture").unwrap_or(36.0),
                    ));
                }
                _ => {}
            }
        }
        if lights.is_empty() && environment.is_none() {
            lights = self.scene_lights();
        }
        Ok(RenderScene { parts, lights, environment, camera, volumes: Vec::new() })
    }
}

// The frame rate a sequence rendered at these times plays at: real time.
pub(crate) fn sequence_fps(times: &[f32]) -> f32 {
    match times {
        [a, b, ..] if b > a => (1.0 / (b - a)).clamp(1.0, 120.0),
        _ => sims::SIM_FPS,
    }
}
