// The parameter panel: POP, COP and TOP nodes.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn contexts_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let vop_code = &info.vop_code;
        let vop_parameters = &info.vop_parameters;
        let fetch_targets = &info.fetch_targets;
        let wedge_targets = &info.wedge_targets;
        let top_preview = &info.top_preview;
        let top_states = &info.top_states;
        let top_busy = info.top_busy;
        let mut c = false;
        match &mut node.kind {
            NodeKind::Pop(pop_node) => {
                ui.heading(pop_node.def().label);
                ui.small(pop_node.def().help);
                let op = pop_node.op;
                if op == pop::WRANGLE {
                    if let Some(code) = pop_node.texts.get_mut(0) {
                        c |= vex_editor_ui(ui, code, 2, "particles", &mut node.channels, "v@force += set(0, 2, 0);");
                    }
                } else {
                    let labels = pop_node.def().texts;
                    let rule = pop_node.v("rule").round() as usize;
                    for (i, ((label, _), text)) in labels.iter().zip(pop_node.texts.iter_mut()).enumerate() {
                        // The expression only matters for the Expression rule.
                        let is_expression = (op == pop::GROUP && i == 1) || op == pop::KILL;
                        let expression_rule = (op == pop::GROUP && rule == 3) || (op == pop::KILL && rule == 0);
                        if is_expression && !expression_rule {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.label(*label);
                            let edit = egui::TextEdit::singleline(text).desired_width(220.0);
                            c |= ui.add(if is_expression { edit.font(egui::TextStyle::Monospace) } else { edit }).changed();
                        });
                    }
                }
                for (param, slot) in pop_node.param_mut() {
                    if let Some(choices) = pop::choices(op, param.key) {
                        let mut value = (slot[0].round().max(0.0) as usize).min(choices.len() - 1);
                        egui::ComboBox::from_label(param.label).selected_text(choices[value]).show_ui(ui, |ui| {
                            for (i, label) in choices.iter().enumerate() {
                                c |= ui.selectable_value(&mut value, i, *label).changed();
                            }
                        });
                        slot[0] = value as f32;
                        continue;
                    }
                    match param.key {
                        "input" | "seed" => {
                            let mut whole = slot[0].round().max(0.0) as usize;
                            let top = if param.key == "input" { 3 } else { 10_000 };
                            c |= pfield(ui, pslider(&mut whole, 0..=top).text(param.label)).changed();
                            slot[0] = whole as f32;
                        }
                        _ => c |= shader_param_ui(ui, param, slot),
                    }
                }
                if op == pop::VOP {
                    ui.small("Double-click to edit its VOP network. Bind reads the particles' attributes (P, v, force, age, Cd...) and Export writes them back.");
                    match &vop_code {
                        Some(Ok(code)) => {
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
            }
            NodeKind::Cop(cop) => {
                ui.small(cop.def().help);
                let labels = cop.def().texts;
                for (label, text) in labels.iter().zip(cop.texts.iter_mut()) {
                    ui.horizontal(|ui| {
                        ui.label(*label);
                        c |= ui.add(egui::TextEdit::singleline(text).desired_width(200.0)).changed();
                        if ui.button("…").clicked() {
                            let dialog = FileDialog::new().add_filter("Images", &["exr", "hdr", "png", "jpg", "jpeg", "tif", "tiff", "tga", "bmp"]);
                            let chosen = if cop.op == cops::OUTPUT { dialog.set_file_name("comp.$F4.exr").save_file() } else { dialog.pick_file() };
                            if let Some(path) = chosen {
                                *text = path.to_string_lossy().into_owned();
                                c = true;
                            }
                        }
                    });
                }
                let op = cop.op;
                for (param, slot) in cop.param_mut() {
                    match param.key {
                        "mode" => {
                            let mut mode = slot[0].round() as usize;
                            egui::ComboBox::from_label("Mode").selected_text(cops::BLEND_MODES[mode.min(9)]).show_ui(ui, |ui| {
                                for (value, label) in cops::BLEND_MODES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut mode, value, *label).changed();
                                }
                            });
                            slot[0] = mode as f32;
                        }
                        "type" if op == 3 => {
                            let mut kind = slot[0].round() as usize;
                            ui.horizontal(|ui| {
                                for (value, label) in cops::RAMP_TYPES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut kind, value, *label).changed();
                                }
                            });
                            slot[0] = kind as f32;
                        }
                        _ => c |= shader_param_ui(ui, param, slot),
                    }
                }
                if op == cops::OUTPUT {
                    ui.horizontal(|ui| {
                        if ui.button("💾 Write current frame").clicked() {
                            self.cop_write_request = Some((id, false));
                        }
                        if ui.button("Write timeline range").on_hover_text("Put $F or $F4 in the file name for one file per frame").clicked() {
                            self.cop_write_request = Some((id, true));
                        }
                    });
                }
            }
            NodeKind::Top(top) => {
                ui.small(top.def().help);
                let op = top.op;
                // Node pickers: what to fetch, or which node's parameter to wedge.
                if op == tops::FETCH {
                    let current = top.node();
                    let shown = current.and_then(|n| fetch_targets.iter().find(|(t, _)| *t == n)).map_or("(choose)".to_owned(), |(_, name)| name.clone());
                    egui::ComboBox::from_label("Fetch").selected_text(shown).show_ui(ui, |ui| {
                        for (target, name) in fetch_targets {
                            if ui.selectable_label(current == Some(*target), name).clicked() {
                                top.set("node", *target as f32);
                                c = true;
                            }
                        }
                    });
                    if fetch_targets.is_empty() {
                        ui.small("Add a MoonRay render node (OUT), or a File Export or Cache node (in a geometry network), to fetch.");
                    }
                }
                if op == tops::WEDGE {
                    let current = top.node();
                    let shown = current.and_then(|n| wedge_targets.iter().find(|(t, ..)| *t == n)).map_or("(none: attribute only)".to_owned(), |(_, name, _)| name.clone());
                    egui::ComboBox::from_label("Node").selected_text(shown).height(400.0).show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "(none: attribute only)").clicked() {
                            top.set("node", -1.0);
                            c = true;
                        }
                        for (target, name, _) in wedge_targets {
                            if ui.selectable_label(current == Some(*target), name).clicked() {
                                top.set("node", *target as f32);
                                c = true;
                            }
                        }
                    });
                    if let Some((_, _, params)) = current.and_then(|n| wedge_targets.iter().find(|(t, ..)| *t == n)) {
                        let mut param = top.v("param") as usize;
                        let shown = params.iter().find(|(i, _)| *i == param).map_or("(choose)".to_owned(), |(_, n)| n.clone());
                        egui::ComboBox::from_label("Parameter").selected_text(shown).height(400.0).show_ui(ui, |ui| {
                            for (i, name) in params {
                                c |= ui.selectable_value(&mut param, *i, name).changed();
                            }
                        });
                        top.set("param", param as f32);
                    }
                }
                let labels = top.def().texts;
                for (label, text) in labels.iter().zip(top.texts.iter_mut()) {
                    ui.label(*label);
                    c |= ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY)).changed();
                }
                if matches!(op, tops::FETCH | tops::COMMAND) {
                    ui.small("Tokens: $F / $F4 frame, @index, @wedgeindex, @<wedge attribute>, {input} (the item's input files).");
                }
                let def = top.def();
                for (i, param) in def.params.iter().enumerate() {
                    if param.key == "node" || param.key == "param" {
                        continue;
                    }
                    let mut v = top.values[i];
                    if let shading::Kind::Float(min, max) = param.kind {
                        if matches!(param.key, "count" | "start" | "end" | "step") && op != tops::WEDGE || param.key == "count" || param.key == "step" {
                            let mut whole = v.round() as i64;
                            c |= pfield(ui, pdrag(&mut whole).range(min as i64..=max as i64).prefix(format!("{}: ", param.label))).changed();
                            v = whole as f32;
                        } else {
                            c |= pfield(ui, pdrag(&mut v).speed(0.01).range(min..=max).prefix(format!("{}: ", param.label))).changed();
                        }
                    }
                    top.values[i] = v;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(!top_busy, egui::Button::new("▶ Cook")).on_hover_text("Runs this node's work items (and its inputs' first)").clicked() {
                        self.top_cook_request = Some(id);
                    }
                    if ui.add_enabled(top_busy, egui::Button::new("■ Cancel")).clicked() {
                        self.top_cancel_request = true;
                    }
                });
                match &top_preview {
                    Some(Err(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(240, 120, 100), e);
                    }
                    Some(Ok(items)) => {
                        let done = top_states.iter().filter(|s| s.0 == tops::State::Done).count();
                        let failed = top_states.iter().filter(|s| s.0 == tops::State::Failed).count();
                        ui.label(format!("{} work items{}", items.len(), if top_states.is_empty() { String::new() } else { format!(" · {done} done · {failed} failed") }));
                        egui::ScrollArea::vertical().max_height(260.0).id_salt(("top_items", id)).show(ui, |ui| {
                            for (i, item) in items.iter().take(500).enumerate() {
                                let (state, log) = top_states.get(i).cloned().unwrap_or((tops::State::Waiting, String::new()));
                                let colour = top_state_colour(state);
                                let mut line = format!("#{} · frame {}", item.index, item.frame);
                                for (name, value) in &item.attrs {
                                    line += &format!(" · {name} {value}");
                                }
                                ui.horizontal(|ui| {
                                    ui.colored_label(colour, "●");
                                    ui.label(line);
                                })
                                .response
                                .on_hover_text(match &item.job {
                                    tops::Job::Shell(cmd) => format!("{cmd}\n{log}"),
                                    _ if !log.is_empty() => log.clone(),
                                    _ => item.output.clone(),
                                });
                                if !item.output.is_empty() {
                                    ui.small(&item.output);
                                }
                            }
                        });
                    }
                    None => {}
                }
            }
            _ => return None,
        }
        Some(c)
    }
}
