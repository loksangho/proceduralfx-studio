// The parameter panel: Loops, switches and merges.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn routing_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let switch_inputs = info.switch_inputs;
        let merge_sources = &info.merge_sources;
        let merge_own_object = info.merge_own_object;
        let loop_begin_name = &info.loop_begin_name;
        let compiled_loop = info.compiled_loop;
        let mut c = false;
        match &mut node.kind {
            NodeKind::Switch { index } => {
                c |= pfield(ui, pslider(index, 0..=switch_inputs.max(2) - 1).text("Input")).changed();
                ui.small(format!(
                    "{switch_inputs} input(s) connected; 0 passes the first, 1 the second, and so on (past the last, the last). Type an expression to choose by frame, e.g. $F > 24 or $F % {}.",
                    switch_inputs.max(2)
                ));
            }
            NodeKind::ForEachBegin | NodeKind::CompileBegin => {
                ui.small(if matches!(node.kind, NodeKind::ForEachBegin) {
                    "Puts out each iteration's geometry (detail attributes iteration, numiterations and value). Set the loop up on its For-Each End."
                } else {
                    "Start of a Compile Block: For-Each loops inside it run on several threads, and the block's result is cached."
                });
                if let Some(end) = &loop_begin_name {
                    ui.label(format!("Ends at {end}"));
                }
            }
            NodeKind::ForEachEnd(set) => {
                ui.small("Runs the nodes between its For-Each Begin and here once per iteration, merging the results (or feeding each result to the next, for feedback).");
                egui::ComboBox::from_label("Loop over").selected_text(flow::MODES[set.mode.min(5)]).show_ui(ui, |ui| {
                    for (value, label) in flow::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                match set.mode {
                    flow::NAMED => {
                        ui.horizontal(|ui| {
                            ui.label("Piece attribute");
                            c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).hint_text("name").desired_width(120.0)).changed();
                        });
                        ui.small("A primitive or point attribute, e.g. piece (Material Fracture) or name.");
                    }
                    flow::NUMBER | flow::FEEDBACK => {
                        c |= pfield(ui, pdrag(&mut set.iterations).range(0..=100_000).prefix("Iterations: ")).changed();
                        c |= pfield(ui, pdrag(&mut set.start).speed(0.1).prefix("Start value: ")).changed();
                        c |= pfield(ui, pdrag(&mut set.increment).speed(0.1).prefix("Increment: ")).changed();
                    }
                    _ => {}
                }
                let mut single = set.single_pass >= 0;
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut single, "Single pass").on_hover_text("Run only one iteration, to check it").changed();
                    if single {
                        let mut pass = set.single_pass.max(0);
                        c |= pfield(ui, pdrag(&mut pass).range(0..=1_000_000)).changed();
                        set.single_pass = pass;
                    } else {
                        set.single_pass = -1;
                    }
                });
                if set.mode != flow::FEEDBACK {
                    c |= ui.checkbox(&mut set.multithread, "Multithread when compiled").changed();
                    ui.small(if compiled_loop { "This loop is inside a Compile Block: iterations run in parallel." } else { "Put the loop inside a Compile Block to run its iterations in parallel." });
                }
                match &loop_begin_name {
                    Some(begin) => ui.label(format!("Begins at {begin}")),
                    None => ui.colored_label(egui::Color32::from_rgb(240, 120, 100), "No For-Each Begin: add the loop again from Flow Control."),
                };
            }
            NodeKind::CompileEnd { .. } => {
                ui.small("End of a Compile Block: the block cooks as a unit, its result is cached, and For-Each loops inside it run on several threads.");
                if let Some(begin) = &loop_begin_name {
                    ui.label(format!("Begins at {begin}"));
                }
            }
            NodeKind::ObjectMerge { sources, into_this } => {
                ui.small("Brings in an object's geometry (its display output), or any node's, in another object or this one.");
                let mut remove = None;
                for (i, source) in sources.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        // "object" for an object's display output, "object / node" for a node inside it.
                        let shown = source
                            .and_then(|s| {
                                merge_sources.iter().find_map(|(object, name, nodes)| {
                                    if *object == s {
                                        Some(name.clone())
                                    } else {
                                        nodes.iter().find(|(n, _)| *n == s).map(|(_, path)| format!("{name} / {path}"))
                                    }
                                })
                            })
                            .unwrap_or_else(|| "(choose)".to_owned());
                        ui.push_id(("object_merge", id, i), |ui| {
                            ui.menu_button(format!("{shown} ⏷"), |ui| {
                                if merge_sources.is_empty() {
                                    ui.weak("No geometry objects");
                                }
                                for (object, name, nodes) in merge_sources {
                                    ui.menu_button(name, |ui| {
                                        let own = Some(*object) == merge_own_object;
                                        let display = ui
                                            .add_enabled(!own, egui::Button::selectable(*source == Some(*object), "Display output"))
                                            .on_hover_text("The object's display node, with its transform")
                                            .on_disabled_hover_text("An Object Merge can't merge its own object; pick a node in it instead");
                                        if display.clicked() {
                                            *source = Some(*object);
                                            c = true;
                                            ui.close();
                                        }
                                        if !nodes.is_empty() {
                                            ui.separator();
                                        }
                                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                                            for (n, path) in nodes {
                                                if ui.add(egui::Button::selectable(*source == Some(*n), path)).clicked() {
                                                    *source = Some(*n);
                                                    c = true;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                        });
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    sources.remove(i);
                    c = true;
                }
                if ui.button("➕ Add object").clicked() {
                    sources.push(None);
                    c = true;
                }
                c |= ui.checkbox(into_this, "Into this object").on_hover_text("Keep each object's position in the scene (off: its geometry as built, without its transform)").changed();
            }
            _ => return None,
        }
        Some(c)
    }
}
