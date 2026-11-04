// The parameter panel: Characters: hair, skeletons and muscles.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn characters_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let joint_names = &info.joint_names;
        let mut c = false;
        match &mut node.kind {
            NodeKind::HairGuides(set) => {
                ui.small("Input 1: skin mesh. Grows guide curves from its surface along the normals.");
                c |= pfield(ui, pslider(&mut set.count, 1..=20_000).logarithmic(true).text("Guides")).changed();
                c |= pfield(ui, pslider(&mut set.length, 0.0..=5.0).text("Length")).changed();
                c |= pfield(ui, pslider(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.droop, 0.0..=3.0).text("Droop")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::GuideProcess(set) => {
                ui.small("Shapes the guide curves from input 1. Chain several to combine operations.");
                egui::ComboBox::from_label("Operation").selected_text(groom::GUIDE_OPS[set.op.min(groom::GUIDE_OPS.len() - 1)]).show_ui(ui, |ui| {
                    for (i, label) in groom::GUIDE_OPS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.op, i, *label).changed();
                    }
                });
                let (amount_label, range) = match set.op {
                    0 => ("Length", 0.0..=5.0),
                    1 => ("Bend", 0.0..=5.0),
                    2 => ("Frizz", 0.0..=2.0),
                    3 => ("Curl radius", 0.0..=2.0),
                    _ => ("Smoothing", 0.0..=2.0),
                };
                c |= pfield(ui, pslider(&mut set.amount, range).text(amount_label)).changed();
                if matches!(set.op, 2 | 3) {
                    let label = if set.op == 2 { "Frequency" } else { "Turns per unit length" };
                    c |= pfield(ui, pslider(&mut set.frequency, 0.0..=20.0).text(label)).changed();
                    c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                }
            }
            NodeKind::HairDeform(set) => {
                ui.small("Input 1: hair or guides • Input 2: animated skin. Each strand moves with the skin under its root.");
                c |= pfield(ui, pdrag(&mut set.rest_frame).speed(1.0).range(-10_000.0..=100_000.0).prefix("Rest frame "))
                    .on_hover_text("The frame the hair was groomed on. The skin must keep the same topology at every frame.")
                    .changed();
            }
            NodeKind::HairGenerate(set) => {
                ui.small("Input 1: skin • Input 2: guides. Interpolates dense hair from the three nearest guides.");
                c |= pfield(ui, pslider(&mut set.count, 1..=200_000).logarithmic(true).text("Hairs")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 1..=64).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.clump, 0.0..=1.0).text("Clumping")).changed();
                c |= pfield(ui, pslider(&mut set.length_variance, 0.0..=1.0).text("Length variance")).changed();
                c |= pfield(ui, pslider(&mut set.frizz, 0.0..=2.0).text("Frizz")).changed();
                c |= pfield(ui, pslider(&mut set.width_root, 0.0..=0.05).text("Root width")).changed();
                c |= pfield(ui, pslider(&mut set.width_tip, 0.0..=0.05).text("Tip width")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                ui.small("Merge the result with the skin to render both.");
            }
            NodeKind::Skeleton(set) => {
                ui.small("A skeleton: joints as points with `parent` and `transform`. Pose it with Rig Pose, Two Bone IK or Procedural Motion; skin a mesh with Joint Capture and Bone Deform.");
                egui::ComboBox::from_label("Preset").selected_text(kinefx::PRESETS[set.preset.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in kinefx::PRESETS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.preset, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.scale, 0.01..=10.0).logarithmic(true).text("Scale")).changed();
                if set.preset != 0 {
                    c |= pfield(ui, pslider(&mut set.joints, 2..=200).text("Joints")).changed();
                    c |= pfield(ui, pslider(&mut set.length, 0.01..=50.0).logarithmic(true).text("Length")).changed();
                }
            }
            NodeKind::RigPose(pose) => {
                ui.small("Rotates (degrees, in the joint's own axes) and moves joints; children follow. Every value can be keyframed.");
                for slot in 0..kinefx::POSE_SLOTS {
                    ui.separator();
                    c |= joint_menu(ui, ("pose", id, slot), &format!("Joint {}", slot + 1), &mut pose.joints[slot], &joint_names, true);
                    if pose.joints[slot].trim().is_empty() {
                        continue;
                    }
                    let v = &mut pose.values[slot];
                    for (label, range) in [("Rotate", 0..3), ("Translate", 3..6)] {
                        ui.horizontal(|ui| {
                            c |= group_label(ui, label, if range.start < 3 { 0.5 } else { 0.01 }, &mut v[range.clone()]);
                            for k in range {
                                c |= pfield(ui, pdrag(&mut v[k]).speed(if k < 3 { 0.5 } else { 0.01 })).changed();
                            }
                        });
                    }
                }
            }
            NodeKind::TwoBoneIk(set) => {
                ui.small("Bends a root → mid → tip chain (thigh, shin, foot) so the tip reaches the goal. Keyframe the goal to animate.");
                c |= joint_menu(ui, ("ik", id, 0), "Root", &mut set.root, &joint_names, false);
                c |= joint_menu(ui, ("ik", id, 1), "Mid", &mut set.mid, &joint_names, false);
                c |= joint_menu(ui, ("ik", id, 2), "Tip", &mut set.tip, &joint_names, false);
                for (label, value) in [("Goal", &mut set.goal), ("Pole", &mut set.pole)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut value[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut value[k]).speed(0.01)).changed();
                        }
                    });
                }
                c |= ui.checkbox(&mut set.use_pole, "Bend towards the pole").changed();
                c |= ui.checkbox(&mut set.keep_tip, "Keep the tip's orientation").changed();
                c |= pfield(ui, pslider(&mut set.blend, 0.0..=1.0).text("Blend")).changed();
            }
            NodeKind::ProceduralMotion(set) => {
                egui::ComboBox::from_label("Motion").selected_text(kinefx::MOTIONS[set.motion.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in kinefx::MOTIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.motion, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.rate, 0.0..=5.0).text("Cycles per second")).changed();
                c |= pfield(ui, pslider(&mut set.amplitude, 0.0..=3.0).text("Amplitude")).changed();
                match set.motion {
                    0 => c |= pfield(ui, pslider(&mut set.travel, 0.0..=5.0).text("Forward speed")).changed(),
                    1 => {
                        c |= pfield(ui, pslider(&mut set.lag, 0.0..=3.0).text("Lag per joint")).changed();
                        ui.horizontal(|ui| {
                            ui.label("Bend about");
                            for (value, label) in sops::AXES.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                            }
                        });
                    }
                    _ => {}
                }
            }
            NodeKind::SkeletonBlend { weight } => {
                ui.small("Blends two poses of the same skeleton (input 1 at 0, input 2 at 1).");
                c |= pfield(ui, pslider(weight, 0.0..=1.0).text("Blend")).changed();
            }
            NodeKind::JointCapture(set) => {
                ui.small("Input 1: mesh • Input 2: skeleton in its rest pose. Writes capture weights for Bone Deform.");
                c |= pfield(ui, pslider(&mut set.influences, 1..=kinefx::MAX_INFLUENCES).text("Joints per point")).changed();
                c |= pfield(ui, pslider(&mut set.falloff, 0.5..=12.0).text("Falloff")).on_hover_text("Higher: sharper transitions between bones").changed();
                c |= ui.checkbox(&mut set.visualize, "Colour by joint").changed();
            }
            NodeKind::BoneDeform => {
                ui.small("Input 1: captured mesh (Joint Capture) • Input 2: rest skeleton • Input 3: animated skeleton.");
            }
            NodeKind::Muscle(set) => {
                ui.small("A muscle from origin to insertion, or along a curve connected to input 1. Animate Activation to flex it.");
                for (label, point) in [("Origin", &mut set.origin), ("Insertion", &mut set.insertion)] {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, label, 0.01, &mut point[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut point[k]).speed(0.01)).changed();
                        }
                    });
                }
                egui::ComboBox::from_label("Shape").selected_text(muscle::PROFILES[set.profile.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in muscle::PROFILES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.profile, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.radius, 0.005..=2.0).logarithmic(true).text("Belly radius")).changed();
                if set.profile == 1 {
                    c |= pfield(ui, pslider(&mut set.flatness, 1.0..=10.0).text("Flatness")).changed();
                    c |= pfield(ui, pslider(&mut set.twist, -180.0..=180.0).text("Twist°")).changed();
                }
                c |= pfield(ui, pslider(&mut set.tendon, 0.0..=0.45).text("Tendon length")).changed();
                c |= pfield(ui, pslider(&mut set.activation, 0.0..=1.0).text("Activation")).changed();
                c |= pfield(ui, pslider(&mut set.contraction, 0.0..=0.8).text("Max contraction")).changed();
                c |= pfield(ui, pslider(&mut set.segments, 2..=128).text("Segments")).changed();
                c |= pfield(ui, pslider(&mut set.sides, 3..=64).text("Sides")).changed();
                c |= pfield(ui, pslider(&mut set.id, 0..=999).text("Muscle id")).changed();
            }
            NodeKind::MuscleSolver(set) => {
                ui.small("Jiggle on animated muscles (flexing or moved by their bones). The origin and insertion tips stay pinned.");
                c |= pfield(ui, pslider(&mut set.stiffness, 1.0..=2000.0).logarithmic(true).text("Firmness")).changed();
                c |= pfield(ui, pslider(&mut set.damping, 0.0..=50.0).text("Damping")).changed();
                c |= pfield(ui, pslider(&mut set.smoothing, 0..=50).text("Coherence")).on_hover_text("Higher wobbles more as one body").changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=16).text("Substeps")).changed();
            }
            NodeKind::MuscleSkin(set) => {
                ui.small("Input 1: skin • Input 2: muscles (animated). The skin follows how the muscles move from frame 0.");
                c |= pfield(ui, pslider(&mut set.falloff, 0.001..=5.0).logarithmic(true).text("Reach")).changed();
                c |= pfield(ui, pslider(&mut set.influences, 1..=16).text("Influences")).changed();
            }
            _ => return None,
        }
        Some(c)
    }
}
