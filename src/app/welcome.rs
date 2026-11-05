// For people new to the app (and to Houdini): a welcome window that starts from a
// ready-made effect, and each preset's simple controls at the top of Parameters, so an
// effect can be shaped without opening its network.

use crate::*;

// Where "don't show the welcome window at startup" is kept.
fn welcome_setting() -> Option<PathBuf> {
    Some(app_config_dir()?.join("welcome.cfg"))
}

// Whether the welcome window opens when the app starts (until it's turned off).
pub(crate) fn welcome_at_startup() -> bool {
    welcome_setting().is_none_or(|path| std::fs::read_to_string(path).map_or(true, |t| t.trim() != "hide"))
}

fn set_welcome_at_startup(show: bool) {
    let Some(path) = welcome_setting() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, if show { "show\n" } else { "hide\n" });
}

impl ProceduralFxStudioApp {
    // The preset's picture as a texture (made once).
    fn preset_picture(&mut self, ctx: &egui::Context, preset: &'static presets::Preset) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.preset_pictures.get(preset.name) {
            return Some(texture.clone());
        }
        let image = image::load_from_memory(preset.picture).ok()?.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let texture = ctx.load_texture(format!("preset {}", preset.name), egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()), egui::TextureOptions::LINEAR);
        self.preset_pictures.insert(preset.name, texture.clone());
        Some(texture)
    }

    // The welcome window: pick an effect to start from, describe one to the assistant,
    // open a scene, or start empty.
    pub(crate) fn welcome_window(&mut self, ctx: &egui::Context) {
        if !self.welcome_open {
            return;
        }
        let pictures: Vec<(&'static presets::Preset, Option<egui::TextureHandle>)> = presets::PRESETS.iter().map(|p| (p, self.preset_picture(ctx, p))).collect();
        let (mut chosen, mut describe, mut open_file, mut empty) = (None, false, false, false);
        let mut show_at_startup = welcome_at_startup();
        let mut open = true;
        egui::Window::new("Welcome to ProceduralFX Studio")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Start from a ready-made effect: it plays straight away. Shape it with the sliders at the top of Parameters, then export it for your game.");
                ui.add_space(6.0);
                for (three_d, heading) in [(false, "Effects for 2D games (sprite sheets)"), (true, "For 3D games (vertex animation)")] {
                    ui.strong(heading);
                    ui.horizontal_wrapped(|ui| {
                        for (preset, picture) in pictures.iter().filter(|(p, _)| p.vertex_animation == three_d) {
                            ui.vertical(|ui| {
                                ui.set_width(112.0);
                                let button = match picture {
                                    Some(texture) => egui::Button::image(egui::Image::new(texture).fit_to_exact_size(egui::vec2(104.0, 104.0))),
                                    None => egui::Button::new(preset.name).min_size(egui::vec2(104.0, 104.0)),
                                };
                                let response = ui.add(button.fill(egui::Color32::from_gray(28)).min_size(egui::vec2(112.0, 112.0)));
                                // Named for screen readers (and tests): the picture has no text.
                                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Start from {}", preset.name)));
                                if response.on_hover_text(preset.description).clicked() {
                                    chosen = Some(*preset);
                                }
                                ui.label(if preset.looping { format!("{} (loop)", preset.name) } else { preset.name.to_owned() });
                            });
                        }
                    });
                    ui.add_space(6.0);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    describe = ui.button("✨ Describe an effect…").on_hover_text("The AI assistant builds it from what you describe, as nodes you can change").clicked();
                    open_file = ui.button("📂 Open a scene…").clicked();
                    empty = ui.button("Start empty").on_hover_text("An empty scene, to build from nodes").clicked();
                });
                ui.add_space(4.0);
                if ui.checkbox(&mut show_at_startup, "Show this when the app starts").changed() {
                    set_welcome_at_startup(show_at_startup);
                }
                ui.weak("Reopen it any time from File > Welcome…");
            });
        if let Some(preset) = chosen {
            self.load_preset(preset);
            // Playing, so it's alive from the first moment.
            self.animate = true;
            self.welcome_open = false;
        }
        if describe {
            self.assistant.open = true;
            self.welcome_open = false;
        }
        if open_file {
            self.welcome_open = false;
            self.open_scene_dialog();
        }
        if empty || !open {
            self.welcome_open = false;
        }
    }

    // The scene's preset's simple controls, at the top of Parameters; whether one changed.
    pub(crate) fn preset_controls(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(preset) = self.preset else { return false };
        let live: Vec<(&'static presets::Control, f32)> = preset.controls.iter().filter_map(|c| c.value(&self.graph).map(|v| (c, v))).collect();
        if live.is_empty() {
            return false;
        }
        let mut changed = false;
        egui::CollapsingHeader::new(egui::RichText::new(format!("{} controls", preset.name)).strong())
            .id_salt("preset_controls")
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new("preset_controls_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for (control, mut value) in live {
                        ui.label(control.label).on_hover_text(control.help);
                        let slider = egui::Slider::new(&mut value, control.range.0..=control.range.1);
                        let slider = if control.whole { slider.integer() } else { slider };
                        if ui.add(slider).on_hover_text(control.help).changed() {
                            changed |= control.set(&mut self.graph, value);
                        }
                        ui.end_row();
                    }
                });
                ui.small("Each slider changes a node inside the network: select a node to see all of its settings.");
            });
        ui.separator();
        changed
    }
}
