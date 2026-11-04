// Exports: flipbooks, real-time renders, sprite sheets (live too), vertex animation, File nodes.

use crate::*;

impl ProceduralFxStudioApp {
    // Writes a File Export node's input at the current time, or every timeline frame ($F in the name).
    pub(crate) fn write_file_export(&mut self, id: NodeId, range: bool) -> Result<String, String> {
        let Some(NodeKind::FileExport { file }) = self.graph.nodes.get(&id).map(|n| n.kind.clone()) else {
            return Err("not a File Export node".into());
        };
        let source = self.graph.input_of(id, 0).ok_or("connect geometry to the input")?;
        let times = if range {
            frame_times(self.playback_start.max(0.0), self.playback_end, 1.0 / sims::SIM_FPS)
        } else {
            vec![self.time]
        };
        // Alembic keeps every frame in one file.
        if fileio::extension(file.trim()) == "abc" && !fileio::has_frame_token(file.trim()) {
            let frames: Vec<(f32, Mesh)> = times.iter().map(|t| (*t, self.graph.cook_fresh(Some(source), *t))).collect();
            let path = file.trim();
            if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            abc::write(path, &frames, sims::SIM_FPS).map_err(|e| format!("{path}: {e}"))?;
            return Ok(format!("Wrote {path} ({} frame{})", frames.len(), if frames.len() == 1 { "" } else { "s" }));
        }
        let mut written = Vec::new();
        for time in &times {
            let mesh = self.graph.cook_fresh(Some(source), *time);
            let path = fileio::expand_frame(file.trim(), *time, sims::SIM_FPS);
            write_geometry_file(&mesh, &path).map_err(|e| format!("{path}: {e}"))?;
            written.push(path);
        }
        Ok(match written.as_slice() {
            [one] => format!("Wrote {one}"),
            many => format!("Wrote {} files ({} … {})", many.len(), many[0], many[many.len() - 1]),
        })
    }

    // Flipbook: renders the playback range in the viewport, frame by frame.
    pub(crate) fn start_flipbook(&mut self) {
        // The GPU viewport renders the frames itself; screenshots need it in the main window.
        let capture = self.gpu_viewport.is_some().then_some(flipbook::Capture { render: None, save_to: None, sheet: None, preview: None });
        if capture.is_none() && self.viewport_rect.is_none() {
            self.status = "Flipbook: show the Viewport in the main window first".into();
            return;
        }
        let times = frame_times(self.playback_start, self.playback_end, 1.0 / sims::SIM_FPS);
        if times.len() > 1000 {
            self.status = format!("Flipbook: {} frames is too many; shorten the playback range to at most 1000", times.len());
            return;
        }
        self.animate = false;
        self.flipbook_viewer = None;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, capture));
    }

    // Renders the current frame, or Render Settings' sequence, with the GPU renderer:
    // through the render camera at the render size, to transparent PNGs in the temp
    // folder, shown in the Render Viewer. No MoonRay needed.
    pub(crate) fn start_realtime_render(&mut self) {
        if self.gpu_viewport.is_none() {
            self.status = "Real-time rendering needs the GPU viewport (it's off with PFX_RENDERER=opengl or PFX_VIEWPORT=cpu)".into();
            return;
        }
        if self.flipbook_recording.is_some() {
            self.status = "A flipbook or real-time render is already running".into();
            return;
        }
        let opts = &self.moonray_default_opts;
        let times = if opts.sequence { frame_times(opts.range_start, opts.range_end, opts.range_step) } else { vec![self.time] };
        if times.len() > 1000 {
            self.status = format!("Real-time render: {} frames is too many; render at most 1000 at a time", times.len());
            return;
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let dir = std::env::temp_dir().join(format!("pfx_realtime_{stamp}"));
        let size = [self.moonray_default_width.clamp(1, 16384) as u32, self.moonray_default_height.clamp(1, 16384) as u32];
        self.render_camera_in_use();
        self.animate = false;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, Some(flipbook::Capture { render: Some(size), save_to: Some(dir), sheet: None, preview: None })));
    }

    // The playback range's times for a sprite sheet: every Nth frame.
    pub(crate) fn sprite_sheet_times(&self) -> Vec<f32> {
        frame_times(self.playback_start, self.playback_end, self.sprite_sheet.every.max(1) as f32 / sims::SIM_FPS)
    }

    // The geometry object a bake uses by default: the one being viewed, else the first.
    pub(crate) fn vat_default_object(&self) -> Option<NodeId> {
        let objects = self.graph.geometry_ids();
        // Up from the network being viewed to its object.
        let mut at = self.current_network;
        for _ in 0..64 {
            match at.and_then(|n| self.graph.nodes.get(&n)).and_then(|n| n.parent) {
                Some(parent) => at = Some(parent),
                None => break,
            }
        }
        at.filter(|o| objects.contains(o)).or_else(|| objects.first().copied())
    }

    pub(crate) fn vat_window(&mut self, ctx: &egui::Context) {
        // A bake in the background: progress, then what it wrote.
        if let Some(rx) = &self.vat_job {
            loop {
                match rx.try_recv() {
                    Ok(VatMessage::Progress(done, total)) => self.status = format!("Baking vertex animation: frame {done} of {total}"),
                    Ok(VatMessage::Done(result)) => {
                        self.status = match result {
                            Ok(summary) => summary,
                            Err(e) => format!("Couldn't bake the vertex animation: {e}"),
                        };
                        self.vat_job = None;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.vat_job = None;
                        break;
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !self.vat_window.open {
            return;
        }
        let objects: Vec<(NodeId, String)> = self.graph.geometry_ids().into_iter().map(|id| (id, self.graph.nodes[&id].title(id))).collect();
        let default_object = self.vat_default_object();
        let mut open = true;
        let mut export = false;
        let busy = self.vat_job.is_some();
        let every = self.vat_window.every.max(1);
        let count = frame_times(self.playback_start, self.playback_end, every as f32 / sims::SIM_FPS).len();
        own_window(ctx, "export_vat", "Export Vertex Animation", [480.0, 360.0], [380.0, 260.0], &mut open, true, |ui| {
            let w = &mut self.vat_window;
            w.every = w.every.max(1);
            ui.label("Bakes an object's motion over the playback range into vertex animation textures: a mesh (OBJ) and two textures (EXR) of every frame's motion. A shader in the game moves the mesh on the GPU.");
            ui.add_space(4.0);
            egui::Grid::new("vat_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Object");
                let chosen = w.object.filter(|o| objects.iter().any(|(id, _)| id == o)).or(default_object);
                let title = objects.iter().find(|(id, _)| Some(*id) == chosen).map_or("(none)", |(_, t)| t.as_str());
                egui::ComboBox::from_id_salt("vat_object").selected_text(title).show_ui(ui, |ui| {
                    for (id, t) in &objects {
                        if ui.selectable_label(chosen == Some(*id), t.as_str()).clicked() {
                            w.object = Some(*id);
                        }
                    }
                });
                ui.end_row();
                ui.label("Kind");
                let label = |m: Option<vat::Mode>| match m {
                    None => "Automatic",
                    Some(vat::Mode::Soft) => "Soft: points that stay",
                    Some(vat::Mode::Rigid) => "Rigid: pieces",
                    Some(vat::Mode::Fluid) => "Fluid: a changing mesh",
                };
                egui::ComboBox::from_id_salt("vat_mode").selected_text(label(w.mode)).show_ui(ui, |ui| {
                    for (m, hint) in [
                        (None, "The kind that suits the motion: Fluid if the mesh changes, Rigid if it's pieces that keep their shape, else Soft"),
                        (Some(vat::Mode::Soft), "Points that stay the same and move freely: cloth, deformers, wobbling or twisting shapes"),
                        (Some(vat::Mode::Rigid), "Pieces that move and turn without bending: destruction and RBD simulations. Pieces come from the piece attribute, or what's connected"),
                        (Some(vat::Mode::Fluid), "A mesh whose points and polygons change: liquids (Particle Fluid Surface), growing shapes. Bigger textures, and frames don't blend"),
                    ] {
                        ui.selectable_value(&mut w.mode, m, label(m)).on_hover_text(hint);
                    }
                });
                ui.end_row();
                ui.label("Frames");
                ui.horizontal(|ui| {
                    ui.label("every");
                    ui.add(egui::DragValue::new(&mut w.every).range(1..=100));
                    ui.label(format!("frame{} of the playback range: {count} frame{}", if w.every == 1 { "" } else { "s" }, if count == 1 { "" } else { "s" }));
                });
                ui.end_row();
                ui.label("Also for");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut w.godot, "Godot").on_hover_text("A spatial shader and a ShaderMaterial (name_vat_material.tres), with import settings that keep the textures exact. Put name.obj on a MeshInstance3D and the material on it.");
                    ui.checkbox(&mut w.unity, "Unity").on_hover_text("A URP shader and material (name_vat.mat), with import settings that keep the textures exact. Put name.obj in the scene and the material on it. Needs the Linear colour space (URP's default).");
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && chosen_ok(&objects, default_object, w.object) && count > 0, egui::Button::new("Export…")).clicked() {
                    export = true;
                }
                if busy {
                    ui.spinner();
                    ui.weak("Baking…");
                }
            });
        });
        self.vat_window.open = open;
        if !export {
            return;
        }
        let Some(object) = self.vat_window.object.filter(|o| objects.iter().any(|(id, _)| id == o)).or(default_object) else { return };
        let name = self.graph.nodes.get(&object).map_or_else(|| "vat".to_owned(), |n| n.title(object));
        let Some(path) = FileDialog::new().set_title("Save the vertex animation").set_file_name(&format!("{name}.obj")).add_filter("OBJ mesh (with its textures beside it)", &["obj"]).save_file() else { return };
        self.start_vat_bake(object, path.with_extension(""));
    }

    // Bakes `object` with the window's settings to the files beside `stem`, in the
    // background; vat_window reports progress and the result.
    pub(crate) fn start_vat_bake(&mut self, object: NodeId, stem: PathBuf) {
        let every = self.vat_window.every.max(1);
        let times = frame_times(self.playback_start, self.playback_end, every as f32 / sims::SIM_FPS);
        let fps = sims::SIM_FPS / every as f32;
        let (godot, unity, mode) = (self.vat_window.godot, self.vat_window.unity, self.vat_window.mode);
        let graph = self.graph.clone();
        let (tx, rx) = mpsc::channel();
        self.vat_job = Some(rx);
        self.vat_window.open = false;
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = vat::bake_object(&graph, object, &times, mode, &mut |done, total| {
                let _ = progress_tx.send(VatMessage::Progress(done, total));
            })
            .and_then(|bake| vat::write(&bake, &stem, fps).map(|(files, layout)| (bake.mode, files, layout)))
            .and_then(|(kind, files, layout)| {
                if godot {
                    vat::write_godot(&files, &layout, fps)?;
                }
                if unity {
                    vat::write_unity(&files, &layout, fps)?;
                }
                Ok(format!("Baked as {}: {} elements over {} frames, to {} and its textures", kind.name(), layout.points, layout.frames, files.obj.display()))
            });
            let _ = tx.send(VatMessage::Done(result));
        });
    }

    pub(crate) fn sprite_sheet_window(&mut self, ctx: &egui::Context) {
        if !self.sprite_sheet.open {
            return;
        }
        let mut open = true;
        let (mut export, mut preview) = (false, false);
        let count = self.sprite_sheet_times().len();
        own_window(ctx, "export_sprite_sheet", "Export Sprite Sheet", [500.0, 460.0], [400.0, 320.0], &mut open, true, |ui| {
            let s = &mut self.sprite_sheet;
            ui.label("Renders the playback range through the render camera with the real-time renderer, then packs the frames into one PNG.");
            ui.add_space(4.0);
            egui::Grid::new("sprite_sheet_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Frame size");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.width).range(8..=4096).suffix(" px"));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut s.height).range(8..=4096).suffix(" px"));
                });
                ui.end_row();
                ui.label("Frames");
                ui.horizontal(|ui| {
                    ui.label("every");
                    ui.add(egui::DragValue::new(&mut s.every).range(1..=100));
                    ui.label(format!("frame{} of the playback range: {count} frame{}, {:.3} fps", if s.every == 1 { "" } else { "s" }, if count == 1 { "" } else { "s" }, sims::SIM_FPS / s.every.max(1) as f32));
                });
                ui.end_row();
                ui.label("Columns");
                ui.horizontal(|ui| {
                    let mut auto = s.columns == 0;
                    if ui.checkbox(&mut auto, "Automatic").on_hover_text("As close to square as the frames allow").changed() {
                        s.columns = if auto { 0 } else { spritesheet::columns(count, [s.width, s.height], 0) };
                    }
                    if !auto {
                        ui.add(egui::DragValue::new(&mut s.columns).range(1..=1000));
                    }
                });
                ui.end_row();
                ui.label("Particles");
                ui.checkbox(&mut s.fill_grid, "Fill the grid").on_hover_text(
                    "For engines' particle systems, which play every cell of the grid: picks a column count that leaves no empty cells (24 frames: 6×4), \
                     so particles never show a blank frame. With Godot ticked, also writes name_particles.tres, a material for GPUParticles2D. \
                     In Unity, use Texture Sheet Animation in Grid mode with the columns and rows (or Sprites mode with the sliced sprites).",
                );
                ui.end_row();
                ui.label("Smooth playback");
                ui.checkbox(&mut s.motion_vectors, "Motion vectors").on_hover_text(
                    "Also writes name_motion.png: how each pixel moves on its way to the next frame. Games then slide each frame towards the next \
                     instead of cross-fading or jumping, so a sheet with a quarter of the frames plays about as smoothly. \
                     bevy_procedural_fx plays it with FxFlipbook; with Godot or Unity ticked, you also get a shader and material that use it.",
                );
                ui.end_row();
                ui.label("Lighting");
                ui.checkbox(&mut s.normal_map, "Normal map").on_hover_text(
                    "Also writes name_normal.png: each frame's shape, read from how opaque it is, so a game's 2D lights light the effect from their side. \
                     Suits smoke, dust and clouds. With Godot ticked, name_sprite.tres (for the AnimatedSprite2D) gives it to Light2D; with Unity ticked, the sliced sprites get it as their \
                     _NormalMap, for URP's 2D lights. bevy_procedural_fx lights an FxFlipbook with FxLight2d lights.",
                );
                ui.end_row();
                ui.label("Glow");
                ui.checkbox(&mut s.glow, "Glow sheet").on_hover_text(
                    "Also writes name_glow.png: the light of glowing parts (fire, sparks, spells), which the sheet itself rolls off towards white. \
                     Games put it back at full brightness, or stronger, so their bloom makes the hot parts glow. bevy_procedural_fx's FxFlipbook adds it (turn on HDR and Bloom on the camera); \
                     with Godot or Unity ticked as well as Motion vectors, their flipbook shaders do too.",
                );
                ui.end_row();
                ui.label("Pixel art");
                ui.vertical(|ui| {
                    ui.checkbox(&mut s.pixel_art, "Pixel art").on_hover_text(
                        "Turns the frames into pixel art: each block of pixels becomes one, every pixel opaque or clear, in a few colours shared by every frame \
                         (so nothing flickers). Render bigger than you need: the frame size is divided by the pixel size. \
                         Bevy, Godot and Unity are told to draw it unfiltered, so the pixels stay sharp.",
                    );
                    if s.pixel_art {
                        ui.horizontal(|ui| {
                            ui.label("pixel size");
                            ui.add(egui::DragValue::new(&mut s.pixel_scale).range(1..=32).suffix(" px"));
                            let k = s.pixel_scale.max(1);
                            ui.weak(format!("→ {}×{} pixels", (s.width / k).max(1), (s.height / k).max(1)));
                        });
                        ui.horizontal(|ui| {
                            ui.label("palette");
                            let mut from_file = !s.pixel_palette.is_empty();
                            ui.radio_value(&mut from_file, false, "pick").on_hover_text("Picks the colours from the frames themselves");
                            ui.radio_value(&mut from_file, true, "from a file").on_hover_text("A palette from Lospec (.hex), GIMP or Aseprite (.gpl), or an image of swatches");
                            if from_file && s.pixel_palette.is_empty() {
                                if let Some(path) = rfd::FileDialog::new().set_title("Palette").add_filter("Palette", &["hex", "gpl", "png", "txt"]).pick_file() {
                                    s.pixel_palette = path.display().to_string();
                                }
                            } else if !from_file {
                                s.pixel_palette.clear();
                            }
                            if s.pixel_palette.is_empty() {
                                ui.add(egui::DragValue::new(&mut s.pixel_colors).range(0..=64)).on_hover_text("How many colours; 0 keeps the frames' own");
                                ui.label(if s.pixel_colors == 0 { "(all colours)" } else { "colours" });
                            } else {
                                let name = std::path::Path::new(&s.pixel_palette).file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                                ui.label(name).on_hover_text(&s.pixel_palette);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut s.pixel_dither, "Dither").on_hover_text("An ordered dither: gradients and soft edges (smoke fading out) become patterns of pixels instead of bands");
                            ui.checkbox(&mut s.pixel_outline, "Outline").on_hover_text("A one-pixel outline around the shape: near-black, or the palette's darkest colour if it has a dark one");
                        });
                    }
                });
                ui.end_row();
                ui.label("Trim");
                ui.checkbox(&mut s.trim, "Crop empty space").on_hover_text("Crops every frame to the same box: what any frame shows, plus a 1-pixel transparent border. The JSON records where the box was in the full frame.");
                ui.end_row();
                ui.label("Loop");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.seamless, "Seamless").on_hover_text(
                        "Renders a few frames past the end of the playback range and fades those pictures into the first frames, so the last frame leads into the first. \
                         The sheet still has one frame per frame of the range. For simulations, a Make Loop node (Pyro) or Loop Particles node in the network loops better: \
                         it blends the simulation itself rather than two pictures, or for particles loops exactly, and loops in the viewport too.",
                    );
                    if s.seamless {
                        ui.label("blend");
                        ui.add(egui::DragValue::new(&mut s.blend).range(1..=count.max(1) as u32));
                        ui.label(if s.blend == 1 { "frame" } else { "frames" });
                    }
                });
                ui.end_row();
                ui.label("Also for");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut s.godot, "Godot").on_hover_text("Also write name.tres, a Godot 4 SpriteFrames resource: put it on an AnimatedSprite2D and it plays the sheet, looping, at its frame rate. Export into your Godot project, or copy the PNG and .tres together.");
                    ui.checkbox(&mut s.unity, "Unity").on_hover_text("Also write name.png.meta, so Unity imports the PNG already sliced into one sprite per frame (name_0, name_1...), uncompressed and full size. Export into your project's Assets folder; select the sprites and drag them into the scene to make an animation.");
                });
                ui.end_row();
                ui.label("Live");
                ui.checkbox(&mut s.live, "Keep updating").on_hover_text(
                    "After exporting, export again to the same files whenever the scene, camera or these settings change. \
                     A Bevy game using bevy_procedural_fx with Bevy's file_watcher feature (embedded_watcher if the game builds its art in) then updates as you work.",
                );
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.weak("Uses the camera chosen in the toolbar's render camera menu. Beside the PNG, name.pfx.json gives the frame size, columns and rows, frame count and frame rate. The bevy_procedural_fx crate plays it in Bevy.");
            ui.add_space(4.0);
            let busy = self.flipbook_recording.is_some();
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && count > 0, egui::Button::new("Export…")).clicked() {
                    export = true;
                }
                if ui
                    .add_enabled(!busy && count > 0, egui::Button::new("Preview"))
                    .on_hover_text("Plays the frames as they'll be exported, seamless blend included, looping in the flipbook viewer. Nothing is saved.")
                    .clicked()
                {
                    preview = true;
                }
                if let Some(live) = &self.live_sheet {
                    let name = live.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    ui.weak(format!("Keeping {name} up to date"));
                    if ui.button("Stop").clicked() {
                        self.sprite_sheet.live = false;
                    }
                }
            });
        });
        if preview {
            self.preview_sprite_sheet();
        }
        if export {
            if let Some(path) = FileDialog::new().set_title("Save the sprite sheet").set_file_name("sprite_sheet.png").add_filter("PNG image", &["png"]).save_file() {
                self.live_sheet = None;
                self.start_sprite_sheet(path, false);
            }
        }
        self.sprite_sheet.open = open && !export;
    }

    // The times a sprite sheet renders, with a seamless loop's extra frames past the end, and
    // how many of them are extra. None (saying why) when it can't start.
    pub(crate) fn sprite_sheet_job(&mut self) -> Option<(Vec<f32>, usize)> {
        if self.gpu_viewport.is_none() || self.flipbook_recording.is_some() {
            self.status = "Sprite sheets need the GPU viewport, with no flipbook or render running".into();
            return None;
        }
        let mut times = self.sprite_sheet_times();
        if times.len() > 1000 {
            self.status = format!("Sprite sheet: {} frames is too many; render at most 1000", times.len());
            return None;
        }
        // A seamless loop renders on past the end, to fade into the start.
        let s = &self.sprite_sheet;
        let blend = s.extra_frames(times.len());
        let step = s.every.max(1) as f32 / sims::SIM_FPS;
        let last = times.last().copied().unwrap_or(self.playback_start);
        times.extend((1..=blend).map(|i| last + i as f32 * step));
        Some((times, blend))
    }

    // Plays the sheet's frames, as they'll export, looping in the flipbook viewer.
    pub(crate) fn preview_sprite_sheet(&mut self) {
        let Some((times, blend)) = self.sprite_sheet_job() else { return };
        let s = &self.sprite_sheet;
        let capture = flipbook::Capture { render: Some([s.width.clamp(8, 4096), s.height.clamp(8, 4096)]), save_to: None, sheet: None, preview: Some(blend) };
        self.render_camera_in_use();
        self.animate = false;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, Some(capture)));
    }

    // Live re-exports keep the files current without adding to the Render Viewer.
    pub(crate) fn start_sprite_sheet(&mut self, path: PathBuf, live: bool) {
        let Some((times, blend)) = self.sprite_sheet_job() else { return };
        let pixel_art = match self.sprite_sheet.pixel_art() {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("Sprite sheet not exported: {e}");
                return;
            }
        };
        let s = &self.sprite_sheet;
        if s.live && !live {
            let exported = self.live_sheet_fingerprint();
            self.live_sheet = Some(LiveSheet { path: path.clone(), exported, pending: None });
        }
        let s = &self.sprite_sheet;
        let output = spritesheet::Output { path, columns: s.columns, trim: s.trim, fps: sims::SIM_FPS / s.every.max(1) as f32, live, blend, godot: s.godot, unity: s.unity, fill_grid: s.fill_grid, motion_vectors: s.motion_vectors, normal_map: s.normal_map, glow: s.glow, pixel_art };
        let capture = flipbook::Capture { render: Some([s.width.clamp(8, 4096), s.height.clamp(8, 4096)]), save_to: None, sheet: Some(output), preview: None };
        self.render_camera_in_use();
        self.animate = false;
        self.flipbook_recording = Some(flipbook::Recording::new(times, self.time, Some(capture)));
    }

    // Everything a sprite sheet depends on: the scene, the camera, the range and the sheet settings.
    pub(crate) fn live_sheet_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        scene_fingerprint(&self.graph.serialize()).hash(&mut hasher);
        let s = &self.sprite_sheet;
        format!(
            "{:?} {:?} {} {} {} {} {} {} {} {} {} {} {}",
            self.render_camera, self.cam_target, self.cam_rot_x, self.cam_rot_y, self.cam_zoom, self.cam_pan_x, self.cam_pan_y,
            self.playback_start, self.playback_end, s.width, s.height, s.every, s.columns
        )
        .hash(&mut hasher);
        (s.trim, s.seamless, s.blend, s.godot, s.unity, s.fill_grid, s.motion_vectors, s.normal_map, s.glow).hash(&mut hasher);
        (s.pixel_art, s.pixel_scale, s.pixel_colors, &s.pixel_palette, s.pixel_dither, s.pixel_outline).hash(&mut hasher);
        hasher.finish()
    }

    // Exports a live sheet again once a change has settled. A change during a live export
    // stops it, and the next one starts once things are still again. Waits while the
    // timeline plays or a button is held, so it never fights with what you're doing.
    pub(crate) fn live_sheet_step(&mut self, ctx: &egui::Context) {
        if !self.sprite_sheet.live {
            if self.live_sheet.take().is_some() {
                self.status = "Live sprite sheet stopped".into();
            }
            return;
        }
        if self.live_sheet.is_none() {
            return;
        }
        let now = self.live_sheet_fingerprint();
        let exporting_live = self.flipbook_recording.as_ref().and_then(|r| r.capture.as_ref()).and_then(|c| c.sheet.as_ref()).is_some_and(|o| o.live);
        let Some(live) = self.live_sheet.as_mut() else { return };
        if exporting_live && now != live.exported {
            if let Some(stopped) = self.flipbook_recording.take() {
                self.time = stopped.resume_time;
                self.dirty = true;
            }
        } else if self.flipbook_recording.is_some() {
            return;
        }
        if now == live.exported {
            live.pending = None;
            return;
        }
        match live.pending {
            Some((seen, since)) if seen == now => {
                let busy = self.animate || ctx.input(|i| i.pointer.any_down());
                if since.elapsed() >= LIVE_SHEET_SETTLE && !busy {
                    live.exported = now;
                    live.pending = None;
                    let path = live.path.clone();
                    self.start_sprite_sheet(path, true);
                }
            }
            _ => live.pending = Some((now, Instant::now())),
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    // Keeps the render options' camera in step with the toolbar's, as MoonRay renders do.
    pub(crate) fn render_camera_in_use(&mut self) {
        self.render_opts = RenderOpts { camera: self.render_camera, ..self.moonray_default_opts.clone() };
    }

    // Once this frame has cooked (simulations too), asks for it to be captured: a
    // window screenshot, or the GPU viewport's own render as it draws.
    pub(crate) fn flipbook_request_capture(&mut self, ctx: &egui::Context) {
        if let Some(recording) = self.flipbook_recording.as_mut() {
            if !recording.waiting && self.simulation_rx.is_none() {
                if recording.capture.is_none() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                }
                recording.waiting = true;
            }
            ctx.request_repaint();
        }
    }

    // Takes each frame's screenshot as it arrives and moves on; Escape cancels.
    pub(crate) fn flipbook_step(&mut self, ctx: &egui::Context) {
        let Some(recording) = self.flipbook_recording.as_mut() else { return };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // Escape during a live export means stop keeping it up to date.
            if recording.capture.as_ref().and_then(|c| c.sheet.as_ref()).is_some_and(|o| o.live) {
                self.sprite_sheet.live = false;
            }
            self.time = recording.resume_time;
            self.flipbook_recording = None;
            self.dirty = true;
            self.status = "Flipbook cancelled".into();
            return;
        }
        // The GPU viewport renders the frame as it draws (see viewport).
        if recording.waiting && recording.capture.is_some() {
            return;
        }
        if recording.waiting {
            let shot = ctx.input(|i| i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            }));
            let Some(shot) = shot else { return };
            if let Some(frame) = self.viewport_rect.and_then(|rect| flipbook::crop(&shot, rect, ctx.pixels_per_point())) {
                recording.frames.push(frame);
            }
            recording.next += 1;
            recording.waiting = false;
        }
        if recording.next >= recording.times.len() {
            let Some(done) = self.flipbook_recording.take() else { return };
            self.time = done.resume_time;
            self.dirty = true;
            let cut_off = || {
                let edges = spritesheet::edges_touched(&done.frames);
                if edges.is_empty() {
                    String::new()
                } else {
                    format!(". The effect runs off the {} of the frame and is cut off there: zoom the render camera out, or make the frame bigger", edges.join(" and "))
                }
            };
            if done.frames.is_empty() {
                self.status = "Flipbook: nothing was captured".into();
            } else if let Some(blend) = done.capture.as_ref().and_then(|c| c.preview) {
                let cut_off = cut_off();
                let frames = if blend > 0 { spritesheet::seamless(&done.frames, blend) } else { done.frames.clone() };
                let times = done.times[..frames.len().min(done.times.len())].to_vec();
                let looped = if blend > 0 { ", with the seamless blend" } else { "" };
                self.status = format!("Previewing the sprite sheet: {} frames as they'll export{looped}{cut_off}", frames.len());
                self.flipbook_viewer = Some(flipbook::Viewer::new(frames, times, sims::SIM_FPS / self.sprite_sheet.every.max(1) as f32));
            } else if let Some(output) = done.capture.as_ref().and_then(|c| c.sheet.clone()) {
                let cut_off = cut_off();
                let glows = (output.glow && done.glows.len() == done.frames.len()).then_some(&done.glows[..]);
                self.status = match spritesheet::save_with_glow(&done.frames, glows, &output) {
                    Ok((png, _)) if output.live => format!("Live sprite sheet: updated {}{cut_off}", png.display()),
                    Ok((png, sheet)) => {
                        // With Live on, the game shows the effect: don't open a window over it.
                        self.viewer.add(png.clone(), !self.sprite_sheet.live);
                        let (w, h) = sheet.image.dimensions();
                        format!("Sprite sheet: {} frames in {}×{} cells of {}×{} ({w}×{h} pixels), saved to {}{cut_off}", done.frames.len().saturating_sub(output.blend), sheet.columns, sheet.rows, sheet.cell[0], sheet.cell[1], png.display())
                    }
                    Err(e) => format!("Sprite sheet: {e}"),
                };
            } else if let Some(dir) = done.capture.and_then(|c| c.save_to) {
                self.status = match flipbook::save_pngs(&done.frames, &dir, "render") {
                    Ok(first) => {
                        self.viewer.add(first, !(done.frames.len() > 1));
                        let count = done.frames.len();
                        // A range plays as an animation in the flipbook viewer, at its own rate.
                        if count > 1 {
                            let step = (done.times[1] - done.times[0]).abs().max(1e-3);
                            self.flipbook_viewer = Some(flipbook::Viewer::new(done.frames, done.times, (1.0 / step).clamp(1.0, 120.0)));
                        }
                        format!("Rendered {count} frame{} in real time to {}{}", if count == 1 { "" } else { "s" }, dir.display(), if count > 1 { "; playing them in the flipbook viewer" } else { "" })
                    }
                    Err(e) => format!("Real-time render: couldn't save the frames: {e}"),
                };
            } else {
                self.status = format!("Flipbook: {} frames", done.frames.len());
                self.flipbook_viewer = Some(flipbook::Viewer::new(done.frames, done.times, sims::SIM_FPS));
            }
            return;
        }
        let t = recording.times[recording.next];
        if self.time != t {
            self.time = t;
            self.dirty = true;
        }
        let what = match recording.capture.as_ref() {
            Some(c) if c.sheet.as_ref().is_some_and(|o| o.live) => "Live sprite sheet",
            Some(c) if c.sheet.is_some() => "Sprite sheet",
            Some(c) if c.save_to.is_some() => "Real-time render",
            _ => "Flipbook",
        };
        self.status = format!("{what}: frame {} of {} (Esc cancels)", recording.next + 1, recording.times.len());
    }
}
