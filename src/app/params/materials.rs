// The parameter panel: Materials, shaders and UVs.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn materials_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let material_ids = &info.material_ids;
        let mut c = false;
        match &mut node.kind {
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
                            for (mid, name) in material_ids {
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
            _ => return None,
        }
        Some(c)
    }
}
