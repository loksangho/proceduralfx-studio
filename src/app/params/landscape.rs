// The parameter panel: Terrain.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn landscape_params(&mut self, ui: &mut egui::Ui, _id: NodeId, node: &mut Node, _info: &ParamsInfo) -> Option<bool> {

        let mut c = false;
        match &mut node.kind {
            NodeKind::Terrain(set) => {
                ui.small("A flat heightfield grid. Shape it with Terrain Noise, Erode and Terrace.");
                ui.horizontal(|ui| {
                    if group_label(ui, "Size", 0.1, &mut set.size[..]) {
                        c = true;
                        set.size.iter_mut().for_each(|v| *v = v.clamp(0.01, 10_000.0));
                    }
                    c |= pfield(ui, pdrag(&mut set.size[0]).speed(0.1).range(0.01..=10_000.0)).changed();
                    c |= pfield(ui, pdrag(&mut set.size[1]).speed(0.1).range(0.01..=10_000.0)).changed();
                });
                c |= pfield(ui, pslider(&mut set.resolution, 2..=2048).logarithmic(true).text("Resolution")).on_hover_text("Grid cells along the longer side").changed();
            }
            NodeKind::TerrainNoise(set) => {
                egui::ComboBox::from_label("Noise").selected_text(terrain::NOISE_TYPES[set.kind.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::NOISE_TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Blend").selected_text(terrain::BLEND_MODES[set.blend.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::BLEND_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.blend, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.amplitude, -10.0..=10.0).text("Height")).changed();
                c |= pfield(ui, pslider(&mut set.element_size, 0.05..=100.0).logarithmic(true).text("Element size")).on_hover_text("Size of the largest features").changed();
                c |= pfield(ui, pslider(&mut set.octaves, 1..=12).text("Octaves")).changed();
                c |= pfield(ui, pslider(&mut set.roughness, 0.0..=1.0).text("Roughness")).changed();
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Offset", 0.05, &mut set.offset[..]);
                    c |= pfield(ui, pdrag(&mut set.offset[0]).speed(0.05)).changed();
                    c |= pfield(ui, pdrag(&mut set.offset[1]).speed(0.05)).changed();
                });
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                c |= ui.checkbox(&mut set.positive, "Positive only (0 to Height)").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").on_hover_text("Uses the `mask` attribute from Terrain Mask or Attribute Paint").changed();
            }
            NodeKind::TerrainErode(set) => {
                ui.small("Hydraulic erosion carves rivers and gullies; thermal erosion crumbles steep slopes. Writes `flow` and `debris` for Terrain Mask.");
                ui.label("Hydraulic");
                c |= pfield(ui, pslider(&mut set.droplets, 0..=2_000_000).logarithmic(true).text("Raindrops")).changed();
                c |= pfield(ui, pslider(&mut set.erosion, 0.0..=1.0).text("Erosion rate")).changed();
                c |= pfield(ui, pslider(&mut set.deposition, 0.0..=1.0).text("Deposition rate")).changed();
                c |= pfield(ui, pslider(&mut set.capacity, 0.0..=20.0).text("Sediment capacity")).changed();
                c |= pfield(ui, pslider(&mut set.evaporation, 0.0..=0.5).text("Evaporation")).changed();
                c |= pfield(ui, pslider(&mut set.inertia, 0.0..=0.95).text("Inertia")).on_hover_text("Higher: straighter, wider rivers").changed();
                c |= pfield(ui, pslider(&mut set.radius, 1..=8).text("Erosion radius (cells)")).changed();
                ui.label("Thermal");
                c |= pfield(ui, pslider(&mut set.thermal_iterations, 0..=500).text("Iterations")).changed();
                c |= pfield(ui, pslider(&mut set.talus_angle, 1.0..=89.0).text("Talus angle°")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::TerrainTerrace(set) => {
                c |= pfield(ui, pslider(&mut set.step, 0.01..=10.0).logarithmic(true).text("Step height")).changed();
                c |= pfield(ui, pslider(&mut set.offset, -5.0..=5.0).text("Offset")).changed();
                c |= pfield(ui, pslider(&mut set.sharpness, 0.0..=1.0).text("Sharpness")).on_hover_text("1: flat steps with sheer walls").changed();
                c |= ui.checkbox(&mut set.use_mask, "Only where masked").changed();
            }
            NodeKind::TerrainMask(set) => {
                ui.small("Writes a 0–1 `mask` point attribute for Terrain Noise, Terrace or any other node.");
                egui::ComboBox::from_label("From").selected_text(terrain::MASK_SOURCES[set.source.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_SOURCES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.source, value, *label).changed();
                    }
                });
                let unit = match set.source {
                    0 => "height",
                    1 => "slope°",
                    _ => "value",
                };
                c |= pfield(ui, pdrag(&mut set.min).speed(0.05).prefix(format!("min {unit} "))).changed();
                c |= pfield(ui, pdrag(&mut set.max).speed(0.05).prefix(format!("max {unit} "))).changed();
                c |= pfield(ui, pslider(&mut set.falloff, 0.0..=50.0).text("Falloff")).changed();
                c |= pfield(ui, pslider(&mut set.blur, 0..=100).text("Blur passes")).changed();
                egui::ComboBox::from_label("Combine with existing mask").selected_text(terrain::MASK_COMBINE[set.combine.min(4)]).show_ui(ui, |ui| {
                    for (value, label) in terrain::MASK_COMBINE.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                c |= ui.checkbox(&mut set.visualize, "Show in viewport (Cd)").changed();
            }
            NodeKind::TerrainColorize(set) => {
                ui.small("Colours the terrain (Cd) by height and slope, for the viewport and MoonRay.");
                for (label, colour) in [("Sand", &mut set.sand), ("Grass", &mut set.grass), ("Rock", &mut set.rock), ("Snow", &mut set.snow)] {
                    ui.horizontal(|ui| {
                        c |= ui.color_edit_button_rgb(colour).changed();
                        ui.label(label);
                    });
                }
                c |= pfield(ui, pslider(&mut set.sand_level, 0.0..=1.0).text("Sand up to")).changed();
                c |= pfield(ui, pslider(&mut set.snow_level, 0.0..=1.0).text("Snow from")).changed();
                c |= pfield(ui, pslider(&mut set.rock_slope, 0.0..=90.0).text("Rock slope°")).changed();
                c |= pfield(ui, pslider(&mut set.blend, 0.0..=0.5).text("Blend")).changed();
            }
            _ => return None,
        }
        Some(c)
    }
}
