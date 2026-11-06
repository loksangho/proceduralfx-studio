// The parameter panel: Objects, networks, cameras, lights and rendering.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn objects_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let pop_display_missing = info.pop_display_missing;
        let camera_ids = &info.camera_ids;
        let material_ids = &info.material_ids;
        let mut c = false;
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
                            for (mid, name) in material_ids {
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
                ui.small("A particle simulation built from POP nodes: double-click it to edit. A new one has a POP Source in the POP Solver's third input (post-solve: particles are born at the source at the end of each step), with Gravity Force after it. Inputs 0-3 are geometry for POP Sources to birth from (input 0 by default), for POP Collide to collide with, and for POP Wrangles to read (point(1, ...) reads input 0).");
                ui.small("The output is points with v, age, life and id, plus what the POPs add (Cd, groups...): ready for Copy to Points, Particle Fluid Surface, Loop Particles or a sprite sheet.");
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
                    self.start_render_request = true;
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
            NodeKind::Merge | NodeKind::Null => {
                ui.label("No parameters.");
            }
            _ => return None,
        }
        Some(c)
    }
}
