// The parameter panel: LOPs: the USD stage.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn stage_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let geometry_objects = &info.geometry_objects;
        let picked_prim = &info.picked_prim;
        let viewport_view = info.viewport_view;
        let material_ids = &info.material_ids;
        let mut c = false;
        match &mut node.kind {
            NodeKind::LopSopImport { geometry, path } => {
                ui.heading("SOP Import");
                ui.small("Brings OBJ geometry objects into the stage as Mesh prims: attributes become primvars (Cd → displayColor) and the object's MAT material is bound.");
                egui::ComboBox::from_id_salt("sop_import_geometry")
                    .selected_text(geometry.map_or("All geometry objects".to_string(), |g| format!("geometry{g}")))
                    .show_ui(ui, |ui| {
                        c |= ui.selectable_value(geometry, None, "All geometry objects").changed();
                        for g in geometry_objects {
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
                    for (mid, name) in material_ids {
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
            _ => return None,
        }
        Some(c)
    }
}
