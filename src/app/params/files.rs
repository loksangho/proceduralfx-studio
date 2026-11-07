// The parameter panel: Files in and out.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn files_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, _info: &ParamsInfo) -> Option<bool> {

        let mut c = false;
        match &mut node.kind {
            NodeKind::FileImport { file, take, looping } => {
                ui.heading("File");
                ui.horizontal(|ui| {
                    c |= ui.add(egui::TextEdit::singleline(file).hint_text("/path/to/model.$F4.obj").desired_width(220.0)).changed();
                    if ui.button("Browse…").clicked() {
                        if let Some(chosen) = FileDialog::new()
                            .add_filter("Geometry", &["obj", "fbx", "glb", "gltf", "abc", "stl", "usd", "usda", "usdc", "usdz", "vdb"])
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
                            .add_filter("Geometry", &["obj", "fbx", "glb", "gltf", "abc", "stl", "usd", "usda", "usdc", "vdb"])
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
                ui.small("The extension picks the format: .obj, .fbx, .glb or .gltf (glTF, for game engines and the web), .abc (Alembic: the timeline range goes in one animated file), .stl, .usd/.usda/.usdc or .vdb (VDB input). The geometry passes through unchanged.");
            }
            _ => return None,
        }
        Some(c)
    }
}
