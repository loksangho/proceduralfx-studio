// The parameter panel: Time: caches, trails, loops and retiming.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn timing_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, _info: &ParamsInfo) -> Option<bool> {

        let mut c = false;
        match &mut node.kind {
            NodeKind::Cache { cache_id, mode, range } => {
                ui.label("Persistent frame cache: cache/cache_ID_frame.mcache");
                c |= pfield(ui, pslider(cache_id, 0..=9999).text("Cache ID")).changed();
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    c |= ui.selectable_value(mode, 0, "Read").changed();
                    c |= ui.selectable_value(mode, 1, "Write").changed();
                });
                ui.small("Write saves each evaluated frame. Read reuses files and falls back to the input on a miss.");
                ui.label("Frame range (what a Fetch TOP writes when nothing is connected to it)");
                ui.horizontal(|ui| {
                    c |= pfield(ui, pdrag(&mut range[0]).speed(1.0).prefix("Start ")).changed();
                    c |= pfield(ui, pdrag(&mut range[1]).speed(1.0).prefix("End ")).changed();
                    c |= pfield(ui, pdrag(&mut range[2]).speed(0.1).range(0.001..=1000.0).prefix("Step ")).changed();
                });
                ui.small("To write the cache in the background, or across wedges: in TASKS, point a Fetch at this node and cook it. Then set the mode to Read.");
            }
            NodeKind::TimeShift { offset, hold, at } => {
                ui.heading("Time Shift");
                ui.small("Input 1 as it is at another time: later or earlier by Offset, or frozen at one moment. Merge shifted copies to build your own loops, or start an effect later.");
                c |= ui.checkbox(hold, "Hold at one time").changed();
                if *hold {
                    c |= pfield(ui, pslider(at, 0.0..=20.0).text("At (s)")).changed();
                    ui.weak(format!("frame {}", (*at * sims::SIM_FPS).round() as i32 + 1));
                } else {
                    c |= pfield(ui, pslider(offset, -10.0..=10.0).text("Offset (s)")).changed();
                    ui.weak(format!("{:+} frames", (*offset * sims::SIM_FPS).round() as i32));
                }
            }
            NodeKind::TimeWarp { speed, anchor } => {
                ui.heading("Time Warp");
                ui.small("Input 1 retimed: time runs Speed times as fast, counting from Anchor, where it's unchanged. 0.5 is half speed. Put a Time Blend after it to smooth a slowed-down simulation.");
                c |= pfield(ui, pslider(speed, -4.0..=4.0).text("Speed")).changed();
                c |= pfield(ui, pslider(anchor, 0.0..=20.0).text("Anchor (s)")).changed();
            }
            NodeKind::TimeBlend => {
                ui.heading("Time Blend");
                ui.small("Simulations step in whole frames, so slowed or shifted by part of a frame they stutter. Between whole frames this mixes the frame before with the one after: particles matched by id, volumes voxel by voxel, other geometry point by point. Put it after a Time Warp or Time Shift.");
            }
            NodeKind::Trail { samples, step, mode, fade } => {
                ui.heading("Trail");
                ui.small("Input 1 over its last few moments, for streaks behind sparks and slashes. Copies merges them, each point with a trail attribute (0 newest, 1 oldest); Curves draws a line through each point's past positions, matched by id; Velocity adds v from how far each point moved.");
                ui.horizontal(|ui| {
                    for (value, label) in [(0, "Copies"), (1, "Curves"), (2, "Velocity")] {
                        c |= ui.selectable_value(mode, value, label).changed();
                    }
                });
                if *mode != 2 {
                    c |= pfield(ui, pslider(samples, 2..=60).text("Samples")).changed();
                }
                c |= pfield(ui, pslider(step, 0.1..=10.0).text("Step (frames)")).changed();
                if *mode == 0 {
                    c |= ui.checkbox(fade, "Fade Alpha and pscale towards the oldest").changed();
                }
            }
            NodeKind::LoopParticles { start, length } => {
                ui.heading("Loop Particles");
                ui.small("Makes a particle simulation (input 1) loop exactly, for looping game effects: no blending, so nothing ghosts. Set the playback range to Start until Start + Length and export it as a sprite sheet. The particles mustn't affect each other, and by Start the emitter should have been running for at least a particle's lifetime.");
                c |= pfield(ui, pslider(start, 0.0..=20.0).text("Start (s)")).changed();
                c |= pfield(ui, pslider(length, 1.0 / sims::SIM_FPS..=10.0).text("Length (s)")).changed();
                ui.weak(format!("{} frames", (*length * sims::SIM_FPS).round()));
            }
            NodeKind::MakeLoop { start, length, blend } => {
                ui.heading("Make Loop");
                ui.small("Makes a simulation (input 1) loop, such as Pyro: it carries on past the end and blends that into the start, voxel by voxel or point by point, over Blend seconds. Set the playback range to Start until Start + Length. For particles, Loop Particles loops exactly.");
                c |= pfield(ui, pslider(start, 0.0..=20.0).text("Start (s)")).changed();
                c |= pfield(ui, pslider(length, 1.0 / sims::SIM_FPS..=10.0).text("Length (s)")).changed();
                c |= pfield(ui, pslider(blend, 0.0..=*length).text("Blend (s)")).changed();
                ui.weak(format!("{} frames, blending over the first {}", (*length * sims::SIM_FPS).round(), (*blend * sims::SIM_FPS).round()));
                if ui.button("Find best length").on_hover_text("Tries lengths from half to twice this one and picks the one where the effect comes back closest to how it started, so the blend has least to hide. Keeps this length if none comes back. Simulates up to twice the length past Start.").clicked() {
                    self.loop_search_request = Some(id);
                }
            }
            _ => return None,
        }
        Some(c)
    }
}
