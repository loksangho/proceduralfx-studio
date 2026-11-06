// The parameter panel: Geometry: shapes, deformers, attributes and the rest of the SOPs.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn geometry_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let has_reference = info.has_reference;
        let mut c = false;
        match &mut node.kind {
            NodeKind::Add { count, spacing, layout, points, delete_geometry } => {
                ui.small("Adds points (to the input, if one is connected): at the positions below, and/or a line or grid of them.");
                c |= ui.checkbox(delete_geometry, "Delete geometry but keep the points").on_hover_text("Removes the input's polygons, leaving its points").changed();
                ui.separator();
                ui.strong("Points");
                let mut remove = None;
                for (i, p) in points.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, format!("{i}"), 0.01, &mut p[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut p[k]).speed(0.01)).changed();
                        }
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    points.remove(i);
                    c = true;
                }
                if ui.button("➕ Add point").clicked() {
                    let next = points.last().map_or([0.0; 3], |p| [p[0] + 1.0, p[1], p[2]]);
                    points.push(next);
                    c = true;
                }
                ui.separator();
                ui.strong("Pattern");
                c |= pfield(ui, pslider(count, 0..=100_000).logarithmic(true).text("Point count")).on_hover_text("0: no pattern").changed();
                if *count > 0 {
                    c |= pfield(ui, pslider(spacing, 0.01..=10.0).logarithmic(true).text("Spacing")).changed();
                    ui.horizontal(|ui| {
                        ui.label("Layout");
                        c |= ui.selectable_value(layout, 0, "Line").changed();
                        c |= ui.selectable_value(layout, 1, "Vertical").changed();
                        c |= ui.selectable_value(layout, 2, "Grid").changed();
                    });
                }
            }
            NodeKind::Grid { size, res } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Plane { size, res } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(res, 2..=120).text("Resolution")).changed();
            }
            NodeKind::Cone { radius, height, rows, cols, caps } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(height, 0.001..=100.0).logarithmic(true).text("Height")).changed();
                c |= pfield(ui, pslider(rows, 1..=32).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Capsule { radius, length, rows, cols } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(length, 0.0..=20.0).text("Length")).changed();
                c |= pfield(ui, pslider(rows, 4..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 8..=128).text("Columns")).changed();
            }
            NodeKind::Box { size, div } => {
                c |= pfield(ui, pslider(size, 0.001..=100.0).logarithmic(true).text("Size")).changed();
                c |= pfield(ui, pslider(div, 1..=30).text("Divisions")).changed();
            }
            NodeKind::Sphere { radius, rows, cols } => {
                c |= pfield(ui, pslider(radius, 0.001..=100.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(rows, 2..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Torus { major, minor, rows, cols } => {
                c |= pfield(ui, pslider(major, 0.001..=100.0).logarithmic(true).text("Major Radius")).changed();
                c |= pfield(ui, pslider(minor, 0.0005..=50.0).logarithmic(true).text("Minor Radius")).changed();
                c |= pfield(ui, pslider(rows, 3..=96).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
            }
            NodeKind::Transform { t, r, s } => {
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= vec3_ui(ui, "Scale    ", s, 0.01);
            }
            NodeKind::Noise { freq, amp } => {
                c |= pfield(ui, pslider(freq, 0.0..=10.0).text("Frequency")).changed();
                c |= pfield(ui, pslider(amp, 0.0..=2.0).text("Amplitude")).changed();
            }
            NodeKind::Taper { factor } => {
                c |= pfield(ui, pslider(factor, -2.0..=2.0).text("Taper")).changed();
            }
            NodeKind::Twist { angle } => {
                c |= pfield(ui, pslider(angle, -5.0..=5.0).text("Twist / unit")).changed();
            }
            NodeKind::Bend { angle } => {
                c |= pfield(ui, pslider(angle, -3.0..=3.0).text("Bend / unit")).changed();
            }
            NodeKind::Smooth { iterations, strength } => {
                c |= pfield(ui, pslider(iterations, 0..=50).text("Iterations")).changed();
                c |= pfield(ui, pslider(strength, 0.0..=1.0).text("Strength")).changed();
            }
            NodeKind::Bevel { amount, segments } => {
                c |= pfield(ui, pslider(amount, 0.0..=1.0).text("Amount")).changed();
                c |= pfield(ui, pslider(segments, 1..=8).text("Segments")).changed();
            }
            NodeKind::Fuse { tol } => {
                c |= pfield(ui, pslider(tol, 0.0001..=0.5).logarithmic(true).text("Distance"))
                    .changed();
            }
            NodeKind::Mirror { axis } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
            }
            NodeKind::Copy { count, t, r, s } => {
                c |= pfield(ui, pslider(count, 1..=100).text("Copies")).changed();
                c |= vec3_ui(ui, "Translate", t, 0.02);
                c |= vec3_ui(ui, "Rotate°  ", r, 0.5);
                c |= pfield(ui, pslider(s, 0.5..=1.5).text("Uniform Scale")).changed();
            }
            NodeKind::CopyToPoints { scale, rotation, piece_attribute } => {
                ui.label("Copy the first input onto every point from the second input.");
                c |= pfield(ui, pslider(scale, 0.01..=10.0).logarithmic(true).text("Copy scale")).changed();
                c |= vec3_ui(ui, "Rotation°", rotation, 0.5);
                let mut use_pieces = !piece_attribute.is_empty();
                if ui
                    .checkbox(&mut use_pieces, "Piece attribute")
                    .on_hover_text("Each point copies only the piece of the first input with its value of this attribute")
                    .changed()
                {
                    *piece_attribute = if use_pieces { "variant".into() } else { String::new() };
                    c = true;
                }
                if use_pieces {
                    c |= ui.add(egui::TextEdit::singleline(piece_attribute).desired_width(140.0)).changed();
                    ui.small("Pieces: number them with Enumerate (Primitives, piece attribute name) or Connectivity. Points: Attribute Randomize an integer from 0 to the number of pieces − 1 to choose randomly.");
                }
                ui.small("Use Add or a particle output as the point input.");
            }
            NodeKind::AttributeCreate(a) => {
                ui.label("Creates or replaces an attribute, of any name and type, with one value on every element.");
                let (changed, chosen) = attribute_target_ui(ui, id, &mut a.name, &mut a.class, &mut a.kind, &mut a.size);
                c |= changed;
                if let Some((name, kind, size, value)) = chosen {
                    (a.name, a.kind, a.size, a.value) = (name.into(), kind, size, value);
                }
                ui.horizontal(|ui| {
                    c |= attribute_value_ui(ui, if a.kind == attrib::QUATERNION { "Rotation°" } else { "Value" }, a.kind, a.size, &mut a.value);
                });
                if a.kind == attrib::QUATERNION {
                    ui.small("Stored as a quaternion (x y z w): turns around X, then Y, then Z, like Transform. Copy to Points uses orient to turn copies.");
                }
            }
            NodeKind::AttributeRandomize(r) if r.preset < randomize::CUSTOM => {
                ui.colored_label(egui::Color32::from_rgb(230, 190, 90), format!("This node uses an older preset: {}.", randomize::PRESETS[r.preset]));
                if ui.button("Switch to the full settings").on_hover_text("The same values, with any name, type and range").clicked() {
                    *r = r.converted();
                    c = true;
                }
                c |= pfield(ui, pslider(&mut r.amount, 0.0..=10.0).text("Amount")).changed();
                ui.small(match r.preset {
                    2 => "A random colour per point; Amount shifts the hues.",
                    3 => "A random rotation per point (a quaternion), blended in from none by Amount (0 to 1).",
                    _ => "Adds a random value between -Amount and +Amount to each point.",
                });
                c |= pfield(ui, pdrag(&mut r.seed).prefix("Seed ")).changed();
            }
            NodeKind::AttributeRandomize(r) => {
                ui.label("Random values for an attribute, the same every time for the same seed.");
                let (changed, chosen) = attribute_target_ui(ui, id, &mut r.name, &mut r.class, &mut r.kind, &mut r.size);
                c |= changed;
                if let Some((name, kind, size, value)) = chosen {
                    (r.name, r.kind, r.size) = (name.into(), kind, size);
                    // A useful range to start from.
                    (r.distribution, r.min, r.max) = match (name, kind) {
                        (_, attrib::QUATERNION) => (randomize::ROTATION, [0.0; 4], [0.0, 360.0, 0.0, 0.0]),
                        ("N" | "v" | "up", _) => (randomize::DIRECTION, [1.0; 4], [1.0; 4]),
                        ("pscale", _) => (0, [value[0] * 0.5; 4], [value[0] * 1.5; 4]),
                        ("id", _) => (0, [0.0; 4], [9.0; 4]),
                        _ => (0, [0.0; 4], [1.0; 4]),
                    };
                }
                let choices = randomize::RandomizeSettings::distributions(r.kind);
                if !choices.contains(&r.distribution) {
                    r.distribution = 0;
                    c = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Values");
                    egui::ComboBox::from_id_salt(("randomize_distribution", id)).selected_text(randomize::DISTRIBUTIONS[r.distribution]).show_ui(ui, |ui| {
                        for i in choices {
                            c |= ui.selectable_value(&mut r.distribution, i, randomize::DISTRIBUTIONS[i]).changed();
                        }
                    });
                });
                match r.distribution {
                    randomize::HUE => {
                        c |= pfield(ui, pslider(&mut r.amount, 0.0..=2.0).text("Hue shift")).changed();
                    }
                    randomize::ROTATION => {
                        c |= pfield(ui, pslider(&mut r.amount, 0.0..=1.0).text("Amount (0 none, 1 any rotation)")).changed();
                    }
                    randomize::DIRECTION => {
                        ui.horizontal(|ui| {
                            c |= pfield(ui, pdrag(&mut r.min[0]).speed(0.01).prefix("Length from ")).changed();
                            c |= pfield(ui, pdrag(&mut r.max[0]).speed(0.01).prefix("to ")).changed();
                        });
                    }
                    _ => {
                        let unit = if r.kind == attrib::QUATERNION { "°" } else { "" };
                        ui.horizontal(|ui| {
                            c |= attribute_value_ui(ui, &format!("Min{unit}"), r.kind, r.size, &mut r.min);
                        });
                        ui.horizontal(|ui| {
                            c |= attribute_value_ui(ui, &format!("Max{unit}"), r.kind, r.size, &mut r.max);
                        });
                        ui.small(match (r.kind, r.distribution) {
                            (attrib::QUATERNION, 1) => "A rotation on the way from Min to Max, in degrees around X, then Y, then Z.",
                            (attrib::QUATERNION, _) => "Each angle between its Min and Max, in degrees around X, then Y, then Z: Max 0 360 0 spins each point around Y.",
                            (_, 1) => "One random point on the way from Min to Max, all components together: a colour between two colours.",
                            (attrib::INTEGER, _) => "Whole numbers from Min to Max, both included.",
                            _ => "Each component on its own, between its Min and Max.",
                        });
                    }
                }
                ui.horizontal(|ui| {
                    for (i, name) in randomize::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut r.mode, i, *name).on_hover_text(match i {
                            0 => "Replace what's there",
                            1 => "Add to what's there (for rotations: turn it further)",
                            _ => "Multiply what's there (for rotations: turn it further)",
                        }).changed();
                    }
                });
                c |= pfield(ui, pdrag(&mut r.seed).prefix("Seed ")).changed();
            }
            NodeKind::AttributeDelete(d) => {
                ui.label("Removes attributes by name.");
                ui.horizontal(|ui| {
                    ui.label("Names");
                    c |= ui.add(egui::TextEdit::singleline(&mut d.pattern).desired_width(180.0).hint_text("Cd N fx_*")).changed();
                });
                ui.horizontal(|ui| {
                    for (i, name) in attrib::CLASSES.iter().chain(&["All"]).enumerate() {
                        c |= ui.selectable_value(&mut d.class, i, *name).changed();
                    }
                });
                ui.small("Names separated by spaces; * matches anything, and ^ in front keeps a name: \"* ^P ^N\" removes all but those.");
            }
            NodeKind::TestGeometry(set) => {
                egui::ComboBox::from_label("Shape").selected_text(testgeo::SHAPES[set.shape.min(testgeo::SHAPES.len() - 1)]).show_ui(ui, |ui| {
                    for (value, label) in testgeo::SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.shape, value, *label).changed();
                    }
                });
                if set.shape <= 2 {
                    ui.small("Keenan Crane's public-domain (CC0) models, quads with UVs.");
                    c |= ui.checkbox(&mut set.colours, "Painted colours (Cd from the model's texture)").changed();
                }
                if set.shape == testgeo::CHARACTER {
                    ui.small("Quaternius's public-domain (CC0) animated human: a skinned mesh with seven takes.");
                    let clips = testgeo::character_clips();
                    let shown = clips.get(set.clip).cloned().unwrap_or_default();
                    egui::ComboBox::from_label("Take").selected_text(shown).show_ui(ui, |ui| {
                        for (value, label) in clips.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.clip, value, label).changed();
                        }
                    });
                    c |= ui.checkbox(&mut set.looping, "Loop (else hold the last pose)").changed();
                    c |= ui.checkbox(&mut set.colours, "Colours").changed();
                    if set.colours {
                        egui::ComboBox::from_label("Look").selected_text(testgeo::LOOKS[set.look.min(testgeo::LOOKS.len() - 1)]).show_ui(ui, |ui| {
                            for (value, label) in testgeo::LOOKS.iter().enumerate() {
                                c |= ui.selectable_value(&mut set.look, value, *label).changed();
                            }
                        });
                    }
                    ui.small("The take plays from frame 1, at the file's 24 frames per second.");
                }
                c |= pfield(ui, pslider(&mut set.size, 0.01..=100.0).logarithmic(true).text("Size")).on_hover_text("The largest side, in scene units").changed();
                c |= ui.checkbox(&mut set.on_ground, "Sit on the ground (else centred on the origin)").changed();
                if set.shape == 7 {
                    c |= pfield(ui, pslider(&mut set.p, 1..=12).text("Windings (p)")).changed();
                    c |= pfield(ui, pslider(&mut set.q, 1..=12).text("Twists (q)")).changed();
                }
                if set.shape >= 7 {
                    c |= pfield(ui, pslider(&mut set.detail, 8..=256).text("Detail")).changed();
                }
            }
            NodeKind::LSystem(set) => {
                ui.small("Rewrites the premise by the rules, generation after generation, then draws it as curves.");
                ui.menu_button("Presets ▾", |ui| {
                    for (label, premise, rules, generations, angle, step) in lsystem::PRESETS {
                        if ui.button(*label).clicked() {
                            set.premise = premise.to_string();
                            set.rules = rules.to_string();
                            set.generations = *generations;
                            set.angle = *angle;
                            set.step = *step;
                            c = true;
                            ui.close_kind(egui::UiKind::Menu);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Premise");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.premise).code_editor().desired_width(220.0)).changed();
                });
                ui.label("Rules (one per line, e.g. A=F[+A][-A])");
                c |= ui.add(egui::TextEdit::multiline(&mut set.rules).code_editor().desired_rows(4).desired_width(f32::INFINITY)).changed();
                c |= pfield(ui, pslider(&mut set.generations, 0..=12).text("Generations")).changed();
                c |= pfield(ui, pslider(&mut set.angle, 0.0..=180.0).text("Angle")).changed();
                c |= pfield(ui, pslider(&mut set.angle_random, 0.0..=45.0).text("Angle Randomness")).changed();
                c |= pfield(ui, pslider(&mut set.step, 0.001..=5.0).logarithmic(true).text("Step Size")).changed();
                c |= pfield(ui, pslider(&mut set.step_scale, 0.0..=1.5).text("Step Scale")).changed();
                c |= pfield(ui, pslider(&mut set.thickness, 0.0..=1.0).text("Thickness")).changed();
                c |= pfield(ui, pslider(&mut set.thickness_scale, 0.0..=1.5).text("Thickness Scale")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                egui::CollapsingHeader::new("Symbols").id_salt("lsystem_symbols").show(ui, |ui| {
                    ui.label(egui::RichText::new(lsystem::SYMBOL_HELP).monospace().small());
                });
            }
            NodeKind::Tube { r_bot, r_top, height, rows, cols, caps } => {
                c |= pfield(ui, pslider(r_bot, 0.0..=5.0).text("Bottom Radius")).changed();
                c |= pfield(ui, pslider(r_top, 0.0..=5.0).text("Top Radius")).changed();
                c |= pfield(ui, pslider(height, 0.1..=10.0).text("Height")).changed();
                c |= pfield(ui, pslider(rows, 1..=64).text("Rows")).changed();
                c |= pfield(ui, pslider(cols, 3..=128).text("Columns")).changed();
                c |= ui.checkbox(caps, "End Caps").changed();
            }
            NodeKind::Peak { dist } => {
                c |= pfield(ui, pslider(dist, -2.0..=2.0).text("Distance")).changed();
            }
            NodeKind::Ripple { freq, amp, speed } => {
                c |= pfield(ui, pslider(freq, 0.0..=20.0).text("Frequency")).changed();
                c |= pfield(ui, pslider(amp, 0.0..=2.0).text("Amplitude")).changed();
                c |= pfield(ui, pslider(speed, 0.0..=20.0).text("Speed")).changed();
            }
            NodeKind::Jitter { amp, seed, scale } => {
                ui.small("Moves each point by a random offset of up to Amount, scaled per axis (0 keeps that axis still).");
                c |= pfield(ui, pslider(amp, 0.0..=1.0).text("Amount")).changed();
                ui.horizontal(|ui| {
                    ui.label("Axis scale");
                    for (axis, value) in ["x", "y", "z"].iter().zip(scale.iter_mut()) {
                        c |= pfield(ui, pdrag(value).speed(0.01).range(0.0..=10.0).prefix(format!("{axis} "))).changed();
                    }
                });
                c |= pfield(ui, pslider(seed, 0..=999).text("Seed")).changed();
            }
            NodeKind::Extrude(set) => {
                ui.small("Extrudes the node's group (or everything) along the normals. Faces outside the group stay as they are.");
                c |= ui.checkbox(&mut set.connected, "Keep shared points together").on_hover_text("On: connected faces extrude as one, with walls only around the outside. Off: every face on its own").changed();
                c |= pfield(ui, pslider(&mut set.distance, -10.0..=10.0).text("Distance")).changed();
                c |= pfield(ui, pslider(&mut set.inset, 0.0..=1.0).text("Inset")).changed();
                c |= pfield(ui, pslider(&mut set.divisions, 1..=50).text("Divisions")).changed();
                ui.separator();
                ui.strong("Output");
                c |= ui.checkbox(&mut set.output_front, "Output front").changed();
                c |= ui.checkbox(&mut set.output_back, "Output back").on_hover_text("Keep the original faces (flipped), closing the shape").changed();
                c |= ui.checkbox(&mut set.output_side, "Output side").changed();
                c |= ui.checkbox(&mut set.make_groups, "Output groups").changed();
                if set.make_groups {
                    for (label, name) in [("Front group", &mut set.front_group), ("Side group", &mut set.side_group), ("Back group", &mut set.back_group)] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            c |= ui.add(egui::TextEdit::singleline(name).desired_width(140.0)).changed();
                        });
                    }
                }
            }
            NodeKind::Clip { axis, offset, invert } => {
                ui.horizontal(|ui| {
                    ui.label("Axis:");
                    c |= ui.selectable_value(axis, 0, "X").changed();
                    c |= ui.selectable_value(axis, 1, "Y").changed();
                    c |= ui.selectable_value(axis, 2, "Z").changed();
                });
                c |= pfield(ui, pslider(offset, -5.0..=5.0).text("Offset")).changed();
                c |= ui.checkbox(invert, "Keep Below").changed();
            }
            NodeKind::Blast { component, indices, invert } => {
                ui.label(format!("Delete {} selected component(s)", indices.len()));
                ui.horizontal(|ui| {
                    ui.label("Component:");
                    c |= ui.selectable_value(component, 0, "Vertices").changed();
                    c |= ui.selectable_value(component, 1, "Faces").changed();
                    c |= ui.selectable_value(component, 2, "Edges").changed();
                });
                c |= ui.checkbox(invert, "Keep selected (delete outside)").changed();
                if ui.button("Clear stored selection").clicked() {
                    indices.clear();
                    c = true;
                }
                ui.small("Select components in the viewport and press Delete to create this node.");
            }
            NodeKind::Vdb { voxel_size, padding, operation, iterations, output } => {
                ui.heading("VDB from Polygons");
                ui.horizontal(|ui| {
                    ui.label("Output");
                    c |= ui.selectable_value(output, 1, "VDB (level set)").changed();
                    c |= ui.selectable_value(output, 0, "Polygons").changed();
                });
                ui.small(if *output == 1 {
                    "Outputs a signed-distance VDB; use Convert VDB to turn it into polygons or a fog volume."
                } else {
                    "Converts the mesh to a signed-distance volume and outputs its surface directly."
                });
                c |= pfield(ui, pslider(voxel_size, 0.03..=1.0).logarithmic(true).text("Voxel size"))
                    .changed();
                ui.small("Smaller voxels = more detail and slower cooking.");
                c |= pfield(ui, pslider(padding, 0..=4).text("Volume padding")).changed();
                ui.horizontal(|ui| {
                    ui.label("Surface operation");
                    egui::ComboBox::from_id_salt("vdb-operation")
                        .selected_text(match *operation {
                            1 => "Dilate",
                            2 => "Erode",
                            3 => "Smooth",
                            _ => "Rasterize",
                        })
                        .show_ui(ui, |ui| {
                            c |= ui.selectable_value(operation, 0, "Rasterize").changed();
                            c |= ui.selectable_value(operation, 1, "Dilate").changed();
                            c |= ui.selectable_value(operation, 2, "Erode").changed();
                            c |= ui.selectable_value(operation, 3, "Smooth").changed();
                        });
                });
                if *operation == 1 || *operation == 2 {
                    c |= pfield(ui, pslider(iterations, 1..=4).text("Surface offset (voxels)")).changed();
                    ui.small(if *operation == 1 {
                        "Moves the surface outward by the selected number of voxels."
                    } else {
                        "Moves the surface inward by the selected number of voxels."
                    });
                } else if *operation == 3 {
                    c |= pfield(ui, pslider(iterations, 1..=4).text("Smoothing passes")).changed();
                    ui.small("Removes small voxel-scale bumps.");
                } else {
                    ui.small("Creates the base VDB surface without changing its shape.");
                }
                ui.small("The VDB node is evaluated from its first input.");
            }
            NodeKind::Reverse => {
                ui.label("Flips triangle winding (normals).");
            }
            NodeKind::Subdivide { levels } => {
                c |= pfield(ui, pslider(levels, 1..=4).text("Levels")).changed();
            }
            NodeKind::Remesh { target } => {
                c |= pfield(ui, pslider(target, 4..=50_000).logarithmic(true).text("Target triangles"))
                    .changed();
                ui.label("Adds shared-edge subdivisions until the target is reached.");
            }
            NodeKind::GroupCreate(set) => {
                ui.horizontal(|ui| {
                    ui.label("Group name");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.name).desired_width(140.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Type");
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Points, "Points").changed();
                    c |= ui.selectable_value(&mut set.domain, groups::GroupDomain::Prims, "Primitives").changed();
                });
                egui::ComboBox::from_label("Method").selected_text(groups::CREATE_METHODS[set.method.min(3)]).show_ui(ui, |ui| {
                    for (value, label) in groups::CREATE_METHODS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, value, *label).changed();
                    }
                });
                match set.method {
                    0 => {
                        c |= vec3_ui(ui, "Min", &mut set.bbox_min, 0.05);
                        c |= vec3_ui(ui, "Max", &mut set.bbox_max, 0.05);
                    }
                    1 => {
                        c |= pfield(ui, pslider(&mut set.percent, 0.0..=100.0).text("Percent")).changed();
                        c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                    }
                    2 => {
                        c |= vec3_ui(ui, "Direction", &mut set.direction, 0.05);
                        c |= pfield(ui, pslider(&mut set.spread, 0.0..=180.0).text("Spread angle")).changed();
                    }
                    _ => {
                        c |= ui.add(egui::TextEdit::singleline(&mut set.expression).hint_text("@P.y>0  0-10  other ^skip").desired_width(220.0)).changed();
                    }
                }
                ui.horizontal(|ui| {
                    group_pick_ui(ui, self.group_pick.as_ref(), (self.selected_vertices.len(), self.selected_triangles.len()), id, PickTarget::GroupCreate, false, &mut self.group_pick_request, &mut self.group_pick_domain, &mut self.group_pick_finish);
                    ui.small("Picking sets the method to Expression, with the picked numbers.");
                });
                egui::ComboBox::from_label("Combine").selected_text(groups::COMBINE_MODES[set.combine.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in groups::COMBINE_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.combine, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.invert, "Invert").changed();
                ui.small("Stored as a 0/1 attribute (group:name / prim:group:name); use the name in any node's Group field.");
            }
            NodeKind::AttributePromote(set) => {
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.pattern).desired_width(160.0).hint_text("Cd"))
                        .on_hover_text("Attributes of the original class, separated by spaces: * for all, ^name to skip one. P (named) gives each polygon its centre")
                        .changed();
                });
                for (label, class) in [("From", &mut set.from), ("To", &mut set.to)] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        for (value, name) in promote::CLASSES.iter().enumerate() {
                            c |= ui.selectable_value(class, value, *name).changed();
                        }
                    });
                }
                egui::ComboBox::from_label("Promotion method").selected_text(promote::METHODS[set.method.min(promote::METHODS.len() - 1)]).show_ui(ui, |ui| {
                    for (value, name) in promote::METHODS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, value, *name).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("New name");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.rename).desired_width(140.0).hint_text("same name")).changed();
                });
                c |= ui.checkbox(&mut set.delete_original, "Delete original").changed();
                ui.small("Where several values meet (a point shared by polygons, a polygon's points, everything for the detail), the method combines them. A quad is one primitive with four vertices.");
            }
            NodeKind::AttributeBlur(set) => {
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.pattern).desired_width(160.0).hint_text("Cd mask"))
                        .on_hover_text("Point attributes, separated by spaces: * for all, ^name to skip one. P (named) smooths the shape itself")
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Neighbours");
                    for (value, label) in blur::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Weighting");
                    for (value, label) in blur::WEIGHTINGS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.weighting, value, *label).on_hover_text("Edge length: nearer neighbours count more").changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.iterations, 0..=200).logarithmic(true).text("Blurring iterations")).changed();
                c |= pfield(ui, pslider(&mut set.step, 0.0..=1.0).text("Step size")).on_hover_text("How far each pass moves toward the neighbours' average").changed();
                if set.mode == 1 {
                    c |= pfield(ui, pslider(&mut set.radius, 0.001..=10.0).logarithmic(true).text("Radius")).changed();
                    c |= pfield(ui, pslider(&mut set.max_points, 1..=100).text("Max points")).changed();
                } else {
                    c |= ui.checkbox(&mut set.pin_border, "Pin border").on_hover_text("Points on open edges keep their values").changed();
                }
                ui.small("Connectivity: points sharing a polygon edge. Proximity: the nearest points within the radius (works on loose points and particles). The group's points change; the others feed in but stay as they are.");
            }
            NodeKind::Enumerate(set) => {
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in enumerate::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).desired_width(140.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Piece attribute");
                    c |= ui
                        .add(egui::TextEdit::singleline(&mut set.piece_attribute).desired_width(140.0).hint_text("optional"))
                        .on_hover_text("Elements sharing a value of this attribute share a number: the pieces are numbered, in the order they first appear")
                        .changed();
                });
                c |= pfield(ui, pdrag(&mut set.start).speed(0.1).prefix("Start ")).changed();
                c |= pfield(ui, pdrag(&mut set.increment).speed(0.1).prefix("Increment ")).changed();
                ui.small("Writes 0, 1, 2… (from Start, by Increment) on each point or primitive (a polygon counts once). For Copy to Points' piece attribute: Class Primitives on the shapes to copy.");
            }
            NodeKind::Color(set) => {
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in color::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Color type").selected_text(color::MODES[set.mode.min(color::MODES.len() - 1)]).show_ui(ui, |ui| {
                    for (value, label) in color::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                match set.mode {
                    1 => {
                        c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
                        ui.horizontal(|ui| {
                            ui.label("One colour per");
                            c |= ui.add(egui::TextEdit::singleline(&mut set.random_by).hint_text("element, or e.g. piece").desired_width(130.0)).changed();
                        });
                    }
                    2 | 3 => {
                        if set.mode == 2 {
                            ui.horizontal(|ui| {
                                ui.label("Along");
                                for (value, label) in color::AXES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut set.axis, value, *label).changed();
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                ui.label("Attribute");
                                c |= ui.add(egui::TextEdit::singleline(&mut set.ramp_attribute).hint_text("pscale, mask, speed…").desired_width(130.0)).changed();
                            });
                            c |= ui.checkbox(&mut set.auto_range, "Use the attribute's own range").on_hover_text("Its lowest value at the ramp's left end, its highest at the right").changed();
                            if !set.auto_range {
                                ui.horizontal(|ui| {
                                    c |= pfield(ui, pdrag(&mut set.range[0]).speed(0.01).prefix("From ")).changed();
                                    c |= pfield(ui, pdrag(&mut set.range[1]).speed(0.01).prefix("to ")).changed();
                                });
                            }
                        }
                        let (from, to) = (set.color, set.color2);
                        let label = if set.mode == 2 { "Color ramp (low → high along the axis)" } else { "Color ramp (low → high values)" };
                        c |= ramp_editor(ui, (id, "color_ramp"), label, &mut set.ramp, 3, || ramp::Ramp::linear(from, to));
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.label("Color");
                            c |= ui.color_edit_button_rgb(&mut set.color).changed();
                        });
                    }
                }
                ui.small("Sets Cd (prim:Cd for primitives). Use the Group field above to colour only part of the geometry.");
            }
            NodeKind::AttributeTransfer(set) => {
                ui.small("Copies attributes from the second input onto the nearest points or primitives of the first.");
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in transfer::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Attributes");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.pattern).hint_text("* or Cd temperature ^id").desired_width(160.0)).changed();
                });
                c |= pfield(ui, pslider(&mut set.distance, 0.0..=100.0).logarithmic(true).smallest_positive(0.001).text("Distance threshold"))
                    .on_hover_text("Elements farther than this from every source keep what they had").changed();
                c |= pfield(ui, pslider(&mut set.blend_width, 0.0..=100.0).logarithmic(true).smallest_positive(0.001).text("Blend width"))
                    .on_hover_text("Past the threshold, the transfer fades out over this distance instead of stopping at a hard edge").changed();
                c |= pfield(ui, pslider(&mut set.samples, 1..=32).text("Max samples"))
                    .on_hover_text("How many nearby sources are blended, weighted by distance (1: the nearest only)").changed();
            }
            NodeKind::AttributePaint { attribute, values, settings: set } => {
                let painting = self.paint_active;
                if ui
                    .selectable_label(painting, if painting { "✏ Painting — drag in the viewport (Ctrl erases)" } else { "✏ Paint in viewport" })
                    .on_hover_text("While on, dragging in the viewport paints instead of orbiting. Right/middle drag still pans.")
                    .clicked()
                {
                    self.paint_active = !painting;
                }
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(attribute).desired_width(110.0)).changed();
                    egui::ComboBox::from_id_salt("paint_presets").selected_text("Groom ▾").show_ui(ui, |ui| {
                        for name in paint::GROOM_ATTRIBUTES {
                            if ui.selectable_label(attribute == name, name).clicked() {
                                *attribute = name.to_owned();
                                c = true;
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    for (value, label) in paint::PAINT_MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.value, -2.0..=4.0).text("Paint value")).changed();
                c |= pfield(ui, pslider(&mut set.default, -2.0..=4.0).text("Default (unpainted)")).changed();
                c |= pfield(ui, pslider(&mut set.radius, 2.0..=400.0).logarithmic(true).text("Brush radius (px)")).changed();
                c |= pfield(ui, pslider(&mut set.strength, 0.01..=1.0).text("Strength")).changed();
                c |= ui.checkbox(&mut set.visualize, "Show as colour (Cd)").changed();
                ui.horizontal(|ui| {
                    if ui.button("Fill with paint value").clicked() {
                        // Needs the input's point count; done at the start of the next frame.
                        self.paint_fill = Some(id);
                    }
                    if ui.button("Clear").clicked() {
                        values.clear();
                        c = true;
                    }
                });
                ui.small(format!(
                    "{} painted point(s). The groom reads density, length, clump, frizz and mask from the skin.",
                    values.len()
                ));
            }
            NodeKind::Normal(set) => {
                ui.horizontal(|ui| {
                    ui.label("Add normals to");
                    for (value, label) in sops::NORMAL_DOMAINS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.domain, value, *label).changed();
                    }
                });
                if set.domain == 1 {
                    c |= pfield(ui, pslider(&mut set.cusp_angle, 0.0..=180.0).text("Cusp angle"))
                        .on_hover_text("Faces meeting at a sharper angle than this keep a hard edge")
                        .changed();
                }
                egui::ComboBox::from_label("Weighting").selected_text(sops::NORMAL_WEIGHTS[set.weighting.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in sops::NORMAL_WEIGHTS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.weighting, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.reverse, "Reverse normals").changed();
                ui.small("The viewport shades with these normals (vertex N, then point N).");
            }
            NodeKind::Scatter(set) => {
                ui.small("Scatters points on the input's surface, or inside it when it's a closed mesh. Points get N, pscale and sourceprim.");
                c |= pfield(ui, pslider(&mut set.count, 1..=1_000_000).logarithmic(true).text("Points")).changed();
                c |= ui.checkbox(&mut set.volume, "Fill the inside (closed meshes)").changed();
                c |= pfield(ui, pslider(&mut set.relax_iterations, 0..=50).text("Relax iterations")).on_hover_text("Spreads the points out evenly").changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::Boolean(set) => {
                ui.small("Combines input A (left) with input B (right). Both should be closed meshes. Result faces are in the a_faces and b_faces primitive groups.");
                egui::ComboBox::from_label("Operation").selected_text(boolean::OPERATIONS[set.operation.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::OPERATIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.operation, value, *label).changed();
                    }
                });
                egui::ComboBox::from_label("Method").selected_text(boolean::MODES[set.mode.min(1)]).show_ui(ui, |ui| {
                    for (value, label) in boolean::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                if set.mode == 1 {
                    c |= pfield(ui, pslider(&mut set.resolution, 16..=400).text("Voxel resolution")).on_hover_text("Voxels along the longest side").changed();
                } else {
                    ui.small("Exact keeps sharp edges. Use Volume for very dense or leaky meshes.");
                }
            }
            NodeKind::PointDeform(set) => {
                ui.small("Input 1: geometry • Input 2: rest points • Input 3: the same points, moved (a simulation, a lattice, an animated cloud). The geometry follows them, bending and turning.");
                c |= pfield(ui, pslider(&mut set.influences, 1..=32).text("Points followed")).changed();
                c |= pfield(ui, pslider(&mut set.fit_neighbours, 3..=64).text("Rotation from neighbours")).changed();
                c |= ui.checkbox(&mut set.rotate, "Follow rotation").changed();
            }
            NodeKind::TransformPieces => {
                ui.small("Input 1: geometry with `piece` (or `name`) • Input 2: one point per piece with P, orient or transform, pscale • Input 3 (optional): the template points at rest. Each piece moves rigidly with its point.");
            }
            NodeKind::ExtractCentroid(set) => {
                ui.small("One point per piece, at its centre (with a piece attribute): feed it to Copy to Points or Transform Pieces.");
                egui::ComboBox::from_label("Pieces").selected_text(sopkit::PARTITIONS[set.partition.min(2)]).show_ui(ui, |ui| {
                    for (i, name) in sopkit::PARTITIONS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.partition, i, *name).changed();
                    }
                });
                egui::ComboBox::from_label("Centre").selected_text(sopkit::CENTRES[set.method.min(1)]).show_ui(ui, |ui| {
                    for (i, name) in sopkit::CENTRES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.method, i, *name).changed();
                    }
                });
            }
            NodeKind::MatchSize(set) => {
                ui.small(if has_reference { "Lines the input up with input 2's bounding box." } else { "Lines the input up with the target box below (or connect a reference to input 2)." });
                for (a, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Justify {axis}"));
                        for (j, name) in sopkit::JUSTIFY.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.justify[a], j, *name).changed();
                        }
                    });
                }
                c |= ui.checkbox(&mut set.scale_to_fit, "Scale to fit").changed();
                if set.scale_to_fit {
                    c |= ui.checkbox(&mut set.uniform, "Uniform scale (keep proportions)").changed();
                }
                if !has_reference {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, "Target centre", 0.01, &mut set.target_center[..]);
                        for v in set.target_center.iter_mut() {
                            c |= pfield(ui, pdrag(v).speed(0.01)).changed();
                        }
                    });
                    ui.horizontal(|ui| {
                        if group_label(ui, "Target size", 0.01, &mut set.target_size[..]) {
                            c = true;
                            set.target_size.iter_mut().for_each(|v| *v = v.max(0.0));
                        }
                        for v in set.target_size.iter_mut() {
                            c |= pfield(ui, pdrag(v).speed(0.01).range(0.0..=1e6)).changed();
                        }
                    });
                }
            }
            NodeKind::ConvertLine => {
                ui.small("Every polygon edge (not the diagonals inside quads) as a two-point line, with a restlength attribute. Lines are curves: Resample, Copy to Points and grooming work on them.");
            }
            NodeKind::Metaball(set) => {
                ui.small("Soft blobs around the input's points (pscale scales each), joined where they meet; with no input, one ball at the origin.");
                c |= pfield(ui, pslider(&mut set.radius, 0.01..=10.0).logarithmic(true).text("Radius")).changed();
                c |= pfield(ui, pslider(&mut set.threshold, 0.01..=0.99).text("Threshold")).on_hover_text("Lower is fatter: blobs reach out and blend sooner").changed();
                c |= pfield(ui, pslider(&mut set.resolution, 8..=256).text("Resolution")).changed();
            }
            NodeKind::Connectivity(set) => {
                ui.small("Numbers the connected pieces 0, 1, 2… in an attribute.");
                ui.horizontal(|ui| {
                    ui.label("Class");
                    c |= ui.selectable_value(&mut set.class, 0, "Points").changed();
                    c |= ui.selectable_value(&mut set.class, 1, "Primitives").changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).desired_width(120.0)).changed();
                });
            }
            NodeKind::Curve(set) => {
                ui.small("A curve through (or near) the points below, in order. Curves are points with curveu (0 at the start, 1 at the end): POP Curve Force, Resample and Copy to Points use them.");
                egui::ComboBox::from_label("Type").selected_text(curve::TYPES[set.kind.min(curve::TYPES.len() - 1)]).show_ui(ui, |ui| {
                    for (value, label) in curve::TYPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.closed, "Closed").on_hover_text("Joins the end back to the start (three points or more)").changed();
                if set.kind != 0 {
                    c |= pfield(ui, pslider(&mut set.divisions, 1..=64).text("Divisions")).on_hover_text("Points on the curve between two of yours").changed();
                }
                ui.separator();
                ui.strong("Points");
                let (mut remove, mut swap) = (None, None);
                let count = set.points.len();
                for (i, p) in set.points.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        c |= group_label(ui, format!("{i}"), 0.01, &mut p[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut p[k]).speed(0.01)).changed();
                        }
                        if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).on_hover_text("Move up").clicked() {
                            swap = Some(i - 1);
                        }
                        if ui.add_enabled(i + 1 < count, egui::Button::new("⏷").small()).on_hover_text("Move down").clicked() {
                            swap = Some(i);
                        }
                        if ui.add_enabled(count > 2, egui::Button::new("✕").small()).on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = swap {
                    set.points.swap(i, i + 1);
                    c = true;
                }
                if let Some(i) = remove {
                    set.points.remove(i);
                    c = true;
                }
                if ui.button("➕ Add point").on_hover_text("Adds a point past the last one, carrying on in its direction").clicked() {
                    let n = set.points.len();
                    let next = match n {
                        0 => [0.0; 3],
                        1 => [set.points[0][0] + 1.0, set.points[0][1], set.points[0][2]],
                        _ => {
                            let (a, b) = (set.points[n - 2], set.points[n - 1]);
                            [2.0 * b[0] - a[0], 2.0 * b[1] - a[1], 2.0 * b[2] - a[2]]
                        }
                    };
                    set.points.push(next);
                    c = true;
                }
            }
            NodeKind::AttributeRemap(set) => {
                ui.small("Reshapes an attribute's values through the ramp: each value's place in the input range (0 at its start, 1 at its end) is looked up on the ramp, and the result spans the output range. Vectors and colours have each component remapped.");
                ui.horizontal(|ui| {
                    ui.label("Attribute");
                    c |= ui.add(egui::TextEdit::singleline(&mut set.attribute).hint_text("pscale, mask, Alpha…").desired_width(130.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Class");
                    for (value, label) in remap::CLASSES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.class, value, *label).changed();
                    }
                });
                c |= ui.checkbox(&mut set.auto_range, "Input range: the attribute's own").on_hover_text("From its lowest value to its highest").changed();
                if !set.auto_range {
                    ui.horizontal(|ui| {
                        ui.label("Input range");
                        c |= pfield(ui, pdrag(&mut set.in_range[0]).speed(0.01)).changed();
                        c |= pfield(ui, pdrag(&mut set.in_range[1]).speed(0.01)).changed();
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Output range");
                    c |= pfield(ui, pdrag(&mut set.out_range[0]).speed(0.01)).changed();
                    c |= pfield(ui, pdrag(&mut set.out_range[1]).speed(0.01)).changed();
                });
                c |= ramp_editor(ui, (id, "remap"), "Ramp (input → output)", &mut set.ramp, 1, || ramp::Ramp::parse(remap::DEFAULT_RAMP, 1).unwrap());
            }
            NodeKind::Resample(set) => {
                ui.small("Evenly spaced points along curves (Convert Line, grooms…); attributes are interpolated.");
                c |= ui.checkbox(&mut set.by_length, "Maximum segment length").changed();
                if set.by_length {
                    c |= pfield(ui, pslider(&mut set.length, 0.001..=10.0).logarithmic(true).text("Length")).changed();
                } else {
                    c |= pfield(ui, pslider(&mut set.segments, 1..=500).text("Segments")).changed();
                }
            }
            NodeKind::Measure(set) => {
                egui::ComboBox::from_label("Measure").selected_text(measure::KINDS[set.kind.min(6)]).show_ui(ui, |ui| {
                    for (value, label) in measure::KINDS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.kind, value, *label).changed();
                    }
                });
                ui.small(match set.kind {
                    0 => "Each primitive's area (prim attribute).",
                    1 => "Each primitive's perimeter; the Sum summary is the mesh's open boundary length.",
                    2 => "Each point's share of the surface area.",
                    3 => "Per point: mean curvature (1/radius on a sphere, + convex) or Gaussian (1/radius², 0 on flat or cylinders).",
                    4 => "Per point: the gradient (direction and rate of fastest increase) of a float attribute, as a vector.",
                    5 => "The volume of the closed mesh (detail attribute).",
                    _ => "Per point: the length of the curve it's on.",
                });
                if set.kind == 3 {
                    ui.horizontal(|ui| {
                        for (value, label) in measure::CURVATURES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.curvature, value, *label).changed();
                        }
                    });
                }
                if set.kind == 4 {
                    ui.horizontal(|ui| {
                        ui.label("Of attribute");
                        c |= ui.add(egui::TextEdit::singleline(&mut set.source).desired_width(120.0)).changed();
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Output name");
                    let hint = set.output_name();
                    c |= ui.add(egui::TextEdit::singleline(&mut set.name).hint_text(hint).desired_width(120.0)).changed();
                });
                if set.kind != 5 {
                    egui::ComboBox::from_label("Detail summary").selected_text(measure::SUMMARIES[set.summary.min(4)]).show_ui(ui, |ui| {
                        for (value, label) in measure::SUMMARIES.iter().enumerate() {
                            c |= ui.selectable_value(&mut set.summary, value, *label).changed();
                        }
                    });
                }
            }
            NodeKind::ConvertVdb { mode, iso } => {
                ui.heading("Convert VDB");
                ui.horizontal(|ui| {
                    ui.label("Convert to");
                    c |= ui.selectable_value(mode, 0, "Polygons").changed();
                    c |= ui.selectable_value(mode, 1, "Volume (fog)").changed();
                });
                if *mode == 0 {
                    c |= pfield(ui, pslider(iso, -1.0..=1.0).text("Isovalue")).changed();
                    ui.small("Level sets: distance from the surface (0 = the surface). Fog volumes and Pyro: density threshold, e.g. 0.5.");
                } else {
                    ui.small("Level sets become fog densities (1 inside); fog volumes pass through. Drawn as a soft volume.");
                }
                ui.small("Input: a VDB from VDB from Polygons, a .vdb file read with the File node, or Pyro smoke.");
            }
            _ => return None,
        }
        Some(c)
    }
}
