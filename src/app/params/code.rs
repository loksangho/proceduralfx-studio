// The parameter panel: Code: wrangles and VOPs.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn code_params(&mut self, ui: &mut egui::Ui, _id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let vop_code = &info.vop_code;
        let vop_parameters = &info.vop_parameters;
        let mut c = false;
        match &mut node.kind {
            NodeKind::AttributeVop { domain } => {
                ui.heading("Attribute VOP");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.small("Double-click the node to edit its network. Bind reads attributes, Export writes them.");
                match &vop_code {
                    Some(Ok(code)) => {
                        // Parameter VOPs start at their default value.
                        for (name, default) in vop_parameters {
                            node.channels.entry(name.clone()).or_insert(*default);
                        }
                        c |= channel_params_ui(ui, code, &mut node.channels);
                        egui::CollapsingHeader::new("Generated VEX").id_salt("vop_code").show(ui, |ui| {
                            ui.label(egui::RichText::new(code.as_str()).monospace().small());
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 110, 110), format!("✖ {e}"));
                    }
                    None => {}
                }
            }
            NodeKind::Vop(v) => {
                let def = v.def();
                ui.small(def.help);
                match v.op {
                    vop::BIND | vop::EXPORT | vop::PARAMETER | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        ui.horizontal(|ui| {
                            ui.label(if v.op == vop::PARAMETER { "Parameter" } else { "Attribute" });
                            c |= ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(120.0)).changed();
                            if matches!(v.op, vop::BIND | vop::GET_ATTRIB | vop::PC_AVERAGE) {
                                ui.menu_button("▾", |ui| {
                                    for (name, ty) in [("P", 1), ("N", 1), ("Cd", 1), ("v", 1), ("pscale", 0), ("ptnum", 2), ("primnum", 2), ("numpt", 2), ("Time", 0), ("Frame", 0), ("orient", 3)] {
                                        if ui.button(name).clicked() {
                                            v.name = name.into();
                                            v.ty = ty;
                                            c = true;
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        });
                    }
                    vop::SNIPPET => {
                        c |= ui.add(egui::TextEdit::singleline(&mut v.name).code_editor().desired_width(f32::INFINITY).hint_text("$0 * 2 + $1")).changed();
                        ui.small("$0 … $3 are the inputs.");
                    }
                    _ => {}
                }
                match v.op {
                    vop::BIND | vop::CONSTANT | vop::PARAMETER | vop::EXPORT | vop::SNIPPET | vop::GET_ATTRIB | vop::PC_AVERAGE => {
                        let choices: &[(&str, char)] = if v.op == vop::PARAMETER { &vop::TYPES[..3] } else { &vop::TYPES };
                        ui.horizontal(|ui| {
                            ui.label("Type");
                            for (i, (name, _)) in choices.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *name).changed();
                            }
                        });
                    }
                    vop::COMPONENT => {
                        ui.horizontal(|ui| {
                            ui.label("Component");
                            for (i, axis) in ["x", "y", "z"].iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *axis).changed();
                            }
                        });
                    }
                    vop::COMPARE => {
                        ui.horizontal(|ui| {
                            ui.label("a");
                            for (i, op) in vop::COMPARES.iter().enumerate() {
                                c |= ui.selectable_value(&mut v.ty, i, *op).changed();
                            }
                            ui.label("b");
                        });
                    }
                    _ => {}
                }
                if matches!(v.op, vop::CONSTANT | vop::PARAMETER) {
                    let label = if v.op == vop::CONSTANT { "Value" } else { "Default" };
                    if v.ty == 1 || v.ty == 3 {
                        c |= vec3_ui(ui, label, &mut v.value, 0.01);
                    } else {
                        c |= pfield(ui, pdrag(&mut v.value[0]).speed(0.01).prefix(format!("{label} "))).changed();
                    }
                }
                if v.reads_geometry() {
                    ui.horizontal(|ui| {
                        ui.label("Geometry");
                        for (i, label) in ["0: being changed", "1", "2", "3"].iter().enumerate() {
                            c |= ui.selectable_value(&mut v.input, i, *label).on_hover_text("Which of the Attribute VOP's inputs it reads").changed();
                        }
                    });
                }
                if !def.inputs.is_empty() {
                    let names: Vec<String> = def.inputs.iter().map(|(n, d)| format!("{n} ({d})")).collect();
                    ui.weak(format!("Inputs, left to right: {}", names.join(", ")));
                }
            }
            NodeKind::Wrangle { code, domain } => {
                ui.heading("VEX Wrangle");
                ui.horizontal(|ui| {
                    ui.label("Run over");
                    for (value, label) in [(0, "Detail"), (1, "Primitives"), (2, "Points"), (3, "Vertices")] {
                        c |= ui.selectable_value(domain, value, label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.menu_button("Insert example ▾", |ui| {
                        for (label, example_domain, example) in vex::EXAMPLES {
                            if ui.button(*label).on_hover_text("Replaces the current code").clicked() {
                                *code = example.to_string();
                                *domain = *example_domain;
                                c = true;
                                ui.close_kind(egui::UiKind::Menu);
                            }
                        }
                    });
                    ui.weak("Edits cook live");
                });
                let noun = match *domain {
                    0 => "detail",
                    1 => "primitives",
                    2 => "points",
                    _ => "vertices",
                };
                c |= vex_editor_ui(ui, code, *domain, noun, &mut node.channels, "@P.y += sin(@P.x * 3.0) * 0.2;");
            }
            NodeKind::VolumeWrangle { code } => {
                ui.heading("Volume Wrangle");
                ui.small("Runs over every voxel: @density (fog) or @surface (level set), with @P at the voxel's centre (read-only). volumesample(0, \"density\", pos) reads the volume anywhere; removepoint() switches a voxel off.");
                c |= examples_menu(ui, code, &VOLUME_WRANGLE_EXAMPLES);
                c |= vex_editor_ui(ui, code, 2, "voxels", &mut node.channels, "@density *= 0.5;");
            }
            NodeKind::HeightfieldWrangle { code } => {
                ui.heading("Heightfield Wrangle");
                ui.small("Runs over every point of a terrain: @height and @mask (and the terrain's other attributes), with @P's x and z fixed. volumesample(0, \"height\", pos) and volumeindex() read neighbouring heights.");
                c |= examples_menu(ui, code, &HEIGHTFIELD_WRANGLE_EXAMPLES);
                c |= vex_editor_ui(ui, code, 2, "points", &mut node.channels, "@height += 1;");
            }
            _ => return None,
        }
        Some(c)
    }
}
