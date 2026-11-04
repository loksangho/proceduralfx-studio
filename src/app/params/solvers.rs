// The parameter panel: Simulations and packed pieces.

use super::ParamsInfo;
use crate::*;

impl ProceduralFxStudioApp {
    // This family's controls for the node, and whether they changed it; None for another family's node.
    pub(super) fn solvers_params(&mut self, ui: &mut egui::Ui, id: NodeId, node: &mut Node, info: &ParamsInfo) -> Option<bool> {
        let solver_display_is_input = info.solver_display_is_input;
        let mut c = false;
        match &mut node.kind {
            NodeKind::Particles {
                count,
                emission_rate,
                lifetime,
                speed,
                spread,
                gravity,
                seed,
                drag,
                bounce,
                friction,
                radius,
                substeps,
                along_normals,
            } => {
                ui.small("Input 1: optional source (emits from its surface, or its points; the origin when unconnected) • Input 2: optional static collider.");
                ui.strong("Emission");
                c |= pfield(ui, pslider(count, 1..=50_000).logarithmic(true).text("Max particles")).changed();
                c |= pfield(ui, pslider(emission_rate, 1.0..=10_000.0).logarithmic(true).text("Birth rate (per second)")).changed();
                c |= pfield(ui, pslider(lifetime, 0.1..=20.0).text("Lifetime")).changed();
                c |= pfield(ui, pslider(speed, 0.0..=20.0).text("Speed")).changed();
                c |= pfield(ui, pslider(spread, 0.0..=2.0).text("Spread")).changed();
                c |= ui
                    .checkbox(along_normals, "Emit along source normals")
                    .on_hover_text("Off: particles head up (+Y). Points inherit their v attribute either way.")
                    .changed();
                ui.strong("Forces");
                c |= pfield(ui, pslider(gravity, -20.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(drag, 0.0..=5.0).text("Drag")).changed();
                ui.strong("Collisions");
                c |= pfield(ui, pslider(bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(friction, 0.0..=1.0).text("Friction")).changed();
                c |= pfield(ui, pslider(radius, 0.0..=0.5).text("Particle radius")).changed();
                ui.strong("Solver");
                c |= pfield(ui, pslider(substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(seed, 0..=9999).text("Seed")).changed();
                ui.small("Outputs points with v, age, life and id attributes.");
            }
            NodeKind::Flip {
                particle_count,
                grid_resolution,
                flip_ratio,
                viscosity,
                gravity,
                max_dt,
                substeps,
                seed,
                domain,
            } => {
                ui.label("3D CPU FLIP solver");
                c |= pfield(ui, pslider(particle_count, 1..=100_000).logarithmic(true).text("Particles")).changed();
                c |= pfield(ui, pslider(grid_resolution, 16..=128).text("Grid resolution")).changed();
                c |= pfield(ui, pslider(flip_ratio, 0.0..=1.0).text("FLIP blend")).changed();
                c |= pfield(ui, pslider(viscosity, 0.0..=4.0).text("Viscosity")).changed();
                c |= pfield(ui, pslider(gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(max_dt, 0.001..=0.1).logarithmic(true).text("Max timestep")).changed();
                c |= pfield(ui, pslider(substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(seed, 0..=9999).text("Seed")).changed();
                let mut auto = domain[3] <= 0.0;
                if ui.checkbox(&mut auto, "Domain from the emitter's bounds").on_hover_text("Or drag the domain box's faces in the viewport").changed() {
                    *domain = if auto { [0.0; 6] } else { [0.0, 1.5, 0.0, 4.0, 3.0, 4.0] };
                    c = true;
                }
                if !auto {
                    for (label, range) in [("Domain centre", 0..3), ("Domain size", 3..6)] {
                        ui.horizontal(|ui| {
                            if group_label(ui, label, 0.02, &mut domain[range.clone()]) {
                                c = true;
                                for k in range.clone() {
                                    domain[k] = if k >= 3 { domain[k].clamp(0.01, 1000.0) } else { domain[k].clamp(-1000.0, 1000.0) };
                                }
                            }
                            for k in range {
                                c |= pfield(ui, pdrag(&mut domain[k]).speed(0.02).range(if k >= 3 { 0.01..=1000.0 } else { -1000.0..=1000.0 })).changed();
                            }
                        });
                    }
                }
                ui.small("Input 1: emitter surface • Input 2: optional static collider. Select the node to drag its domain box in the viewport.");
            }
            NodeKind::WhitewaterSolver(w) => {
                ui.small("Spray, foam and bubbles where the fluid input splashes. Input 1: FLIP fluid • Input 2: optional collider. Writes `type` and spray/foam/bubble groups; renders as small spheres.");
                c |= pfield(ui, pslider(&mut w.max_particles, 0..=2_000_000).logarithmic(true).text("Max particles")).changed();
                c |= pfield(ui, pslider(&mut w.emission_rate, 0.0..=10_000.0).logarithmic(true).text("Emission rate")).changed();
                c |= pfield(ui, pslider(&mut w.trapped_air, 0.0..=4.0).text("Trapped air")).on_hover_text("Emission where fluid collides or tears apart").changed();
                c |= pfield(ui, pslider(&mut w.wave_crest, 0.0..=4.0).text("Wave crests")).on_hover_text("Emission from breaking crests").changed();
                c |= pfield(ui, pslider(&mut w.min_speed, 0.0..=10.0).text("Min speed")).changed();
                c |= pfield(ui, pslider(&mut w.max_speed, 0.1..=30.0).text("Full emission speed")).changed();
                c |= pfield(ui, pslider(&mut w.lifespan, 0.05..=10.0).text("Foam lifespan (s)")).changed();
                c |= pfield(ui, pslider(&mut w.spray_drag, 0.0..=5.0).text("Spray drag")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_buoyancy, 0.0..=10.0).text("Bubble buoyancy")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_drag, 0.0..=1.0).text("Bubble drag")).changed();
                c |= pfield(ui, pslider(&mut w.spray_neighbours, 0..=50).text("Spray below (neighbours)")).changed();
                c |= pfield(ui, pslider(&mut w.bubble_neighbours, 1..=100).text("Bubbles above (neighbours)")).changed();
                c |= pfield(ui, pslider(&mut w.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut w.particle_size, 0.0005..=0.2).logarithmic(true).text("Particle size")).changed();
                c |= pfield(ui, pslider(&mut w.substeps, 1..=8).text("Substeps")).changed();
                c |= pfield(ui, pslider(&mut w.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::ParticleSurface { radius, join_distance, resolution, smoothing } => {
                c |= pfield(ui, pslider(radius, 0.001..=1.0).logarithmic(true).text("Particle radius"))
                    .on_hover_text("A lone droplet's radius. 1–1.5× the particles' spacing, which for FLIP is ∛(emitter volume ÷ particles): 100,000 particles in a 1 m cube → about 0.03")
                    .changed();
                c |= pfield(ui, pslider(join_distance, 0.0..=1.0).text("Join distance")).on_hover_text("Extra reach: particles this much further apart still join into one surface. More fills gaps but rounds off splashes").changed();
                c |= pfield(ui, pslider(resolution, 16..=512).text("Resolution")).on_hover_text("Voxels across the particles' longest side. Never finer than a quarter of the radius; very large grids are made coarser").changed();
                c |= pfield(ui, pslider(smoothing, 0..=8).text("Smoothing")).on_hover_text("Passes that even out the surface. 1–2 for water; more melts away thin sheets").changed();
                ui.small("Particles (e.g. FLIP) to one smooth, closed surface. The particles' v, Cd and other point attributes carry onto it. For water: a glass material with IOR 1.33.");
            }
            NodeKind::Pyro(set) => {
                ui.heading("Pyro Solver");
                ui.small("Smoke and fire on a voxel grid. Input 1: source (closed mesh or points, pscale sets point radius) • Input 2: optional collider (closed mesh).");
                ui.horizontal(|ui| {
                    ui.label("Preset");
                    for (label, preset) in set.presets() {
                        if ui.button(label).clicked() {
                            *set = preset;
                            c = true;
                        }
                    }
                });
                ui.separator();
                ui.strong("Domain");
                c |= vec3_ui(ui, "Center", &mut set.center, 0.05);
                c |= vec3_ui(ui, "Size", &mut set.size, 0.05);
                if ui.button("Fit around source").on_hover_text("Centre the box on the source and leave room for the smoke to rise").clicked() {
                    self.pyro_fit_request = Some(id);
                }
                c |= pfield(ui, pslider(&mut set.resolution, 8..=160).text("Resolution (voxels on longest side)")).changed();
                let voxels = {
                    let h = set.size.iter().cloned().fold(0.0, f32::max) / set.resolution.max(4) as f32;
                    set.size.iter().map(|s| ((s / h).ceil() as usize).max(4)).product::<usize>()
                };
                ui.weak(format!("{voxels} voxels"));
                c |= pfield(ui, pslider(&mut set.substeps, 1..=8).text("Substeps")).changed();
                ui.separator();
                ui.strong("Source");
                c |= pfield(ui, pslider(&mut set.density, 0.0..=4.0).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.temperature, 0.0..=4.0).text("Temperature")).changed();
                c |= pfield(ui, pslider(&mut set.fuel, 0.0..=4.0).text("Fuel (fire)")).changed();
                ui.strong("Shape");
                c |= pfield(ui, pslider(&mut set.buoyancy, -2.0..=5.0).text("Buoyancy")).changed();
                c |= pfield(ui, pslider(&mut set.dissipation, 0.0..=3.0).text("Dissipation")).changed();
                c |= pfield(ui, pslider(&mut set.cooling, 0.0..=5.0).text("Cooling")).changed();
                c |= pfield(ui, pslider(&mut set.burn_rate, 0.0..=5.0).text("Burn rate")).changed();
                c |= pfield(ui, pslider(&mut set.vorticity, 0.0..=3.0).text("Vorticity")).changed();
                c |= pfield(ui, pslider(&mut set.turbulence, 0.0..=3.0).text("Turbulence")).changed();
                c |= vec3_ui(ui, "Wind", &mut set.wind, 0.05);
                c |= pfield(ui, pslider(&mut set.opacity, 0.0..=4.0).text("Display opacity")).changed();
                ui.small("Output: points with density, temperature, flame, Cd, Alpha and pscale, drawn as soft volume splats. Edits restart the simulation.");
            }
            NodeKind::MaterialFracture(set) => {
                ui.small("Breaks a closed mesh into pieces for the RBD Solver. Pieces get `piece`; cut faces are in the `inside` group, the original surface in `outside`.");
                egui::ComboBox::from_label("Material").selected_text(fracture::MATERIALS[set.material.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in fracture::MATERIALS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.material, value, *label).changed();
                    }
                });
                match set.material {
                    1 => {
                        ui.horizontal(|ui| {
                            c |= group_label(ui, "Impact point", 0.01, &mut set.impact[..]);
                            for k in 0..3 {
                                c |= pfield(ui, pdrag(&mut set.impact[k]).speed(0.01)).changed();
                            }
                        });
                        c |= pfield(ui, pslider(&mut set.rings, 1..=20).text("Crack rings")).changed();
                        c |= pfield(ui, pslider(&mut set.spokes, 3..=64).text("Radial cracks")).changed();
                    }
                    m => {
                        c |= pfield(ui, pslider(&mut set.pieces, 2..=500).logarithmic(true).text("Pieces")).changed();
                        if m == 2 {
                            ui.horizontal(|ui| {
                                ui.label("Grain along");
                                for (value, label) in sops::AXES.iter().enumerate() {
                                    c |= ui.selectable_value(&mut set.grain_axis, value, *label).changed();
                                }
                            });
                            c |= pfield(ui, pslider(&mut set.splinter, 1.0..=20.0).text("Splinter length")).changed();
                        }
                    }
                }
                c |= pfield(ui, pslider(&mut set.separate, 0.0..=1.0).text("Explode view")).on_hover_text("Moves pieces apart to see the cracks").changed();
                c |= pfield(ui, pslider(&mut set.glue_strength, 0.0..=100_000.0).logarithmic(true).text("Glue strength")).on_hover_text("How hard the constraints between pieces hold in the RBD Bullet Solver").changed();
                ui.small("Outputs: 1 pieces · 2 glue constraints (lines between neighbouring pieces) · 3 proxy collision shapes. Wire all three into an RBD Bullet Solver.");
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::RbdBullet(set) => {
                ui.small("Inputs: 1 pieces (piece attribute) · 2 glue constraints · 3 proxy collision shapes · 4 static collision geometry. Wire Material Fracture's three outputs straight in. Outputs the moved pieces, the constraints still holding, and the moved proxies.");
                c |= pfield(ui, pslider(&mut set.density, 0.01..=100.0).logarithmic(true).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=2.0).text("Friction")).changed();
                c |= pfield(ui, pslider(&mut set.bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(&mut set.gravity, -30.0..=30.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.glue_scale, 0.0..=100.0).logarithmic(true).text("Glue strength ×")).on_hover_text("Scales every constraint's strength; 0 breaks all glue at the start").changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=32).text("Substeps")).changed();
                c |= ui.checkbox(&mut set.ground, "Ground plane").changed();
                if set.ground {
                    c |= pfield(ui, pdrag(&mut set.ground_height).speed(0.01).prefix("Ground height: ")).changed();
                }
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Initial velocity", 0.05, &mut set.velocity[..]);
                    for k in 0..3 {
                        c |= pfield(ui, pdrag(&mut set.velocity[k]).speed(0.05)).changed();
                    }
                });
                ui.small("RBD Configure sets density, friction, bounce, active and velocity per piece.");
            }
            NodeKind::RbdConfigure(set) => {
                ui.small("Sets physical properties on the pieces in the node's group (all when empty), for the RBD Bullet Solver. Constraints and proxy pass through.");
                for (on, label, value, range) in [
                    (&mut set.set_density, "Density", &mut set.density, 0.01..=100.0),
                    (&mut set.set_friction, "Friction", &mut set.friction, 0.0..=2.0),
                    (&mut set.set_bounce, "Bounce", &mut set.bounce, 0.0..=1.0),
                ] {
                    ui.horizontal(|ui| {
                        c |= ui.checkbox(on, "").changed();
                        c |= ui.add_enabled(*on, pslider(value, range).text(label)).changed();
                    });
                }
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.set_active, "").changed();
                    c |= ui.add_enabled(set.set_active, egui::Checkbox::new(&mut set.active, "Active (off: static, others still hit it)")).changed();
                });
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.set_velocity, "").changed();
                    ui.add_enabled_ui(set.set_velocity, |ui| {
                        c |= group_label(ui, "Velocity", 0.05, &mut set.velocity[..]);
                        for k in 0..3 {
                            c |= pfield(ui, pdrag(&mut set.velocity[k]).speed(0.05)).changed();
                        }
                    });
                });
            }
            NodeKind::Pack(set) => {
                ui.small("Turns each piece into one point (P, transform, piece) whose geometry is stored once: cheap to move, copy and merge. The viewport and renders show the real geometry; Unpack turns it back.");
                egui::ComboBox::from_label("Pack").selected_text(packed::MODES[set.mode.min(2)]).show_ui(ui, |ui| {
                    for (value, label) in packed::MODES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.mode, value, *label).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Pivot");
                    for (value, label) in packed::PIVOTS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.pivot, value, *label).changed();
                    }
                });
            }
            NodeKind::Unpack { transfer } => {
                ui.small("Turns packed points back into geometry at their transform (P, transform, or orient and pscale).");
                c |= ui.checkbox(transfer, "Transfer the packed points' attributes (Cd, piece…)").changed();
            }
            NodeKind::CrowdSolver(set) => {
                ui.small("Input 1: start points (e.g. Scatter on a grid) • Input 2: optional ground to walk on • Input 3: optional obstacles.");
                ui.horizontal(|ui| {
                    ui.label("Display");
                    for (value, label) in crowd::DISPLAYS.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.display, value, *label).changed();
                    }
                });
                if set.display == 0 {
                    ui.small("For your own characters: feed these points to Copy to Points's second input, with a character facing +Z, feet at the origin, into its first. orient turns each one to where its agent walks, and pscale sizes it to the agent height (for a character 1.75 tall).");
                }
                c |= pfield(ui, pslider(&mut set.speed, 0.0..=10.0).text("Walk speed")).changed();
                c |= pfield(ui, pslider(&mut set.speed_variance, 0.0..=1.0).text("Speed variance")).changed();
                c |= pfield(ui, pslider(&mut set.turn_rate, 1.0..=720.0).text("Turn rate°/s")).changed();
                c |= pfield(ui, pslider(&mut set.personal_space, 0.05..=5.0).text("Personal space")).changed();
                c |= pfield(ui, pslider(&mut set.separation, 0.0..=10.0).text("Separation")).changed();
                c |= pfield(ui, pslider(&mut set.alignment, 0.0..=5.0).text("Alignment")).changed();
                c |= pfield(ui, pslider(&mut set.cohesion, 0.0..=5.0).text("Cohesion")).changed();
                c |= pfield(ui, pslider(&mut set.wander, 0.0..=5.0).text("Wander")).changed();
                ui.horizontal(|ui| {
                    c |= group_label(ui, "Goal", 0.1, &mut set.goal[..]);
                    for k in 0..3 {
                        c |= pfield(ui, pdrag(&mut set.goal[k]).speed(0.1)).changed();
                    }
                });
                c |= pfield(ui, pslider(&mut set.goal_weight, 0.0..=10.0).text("Goal strength")).changed();
                c |= pfield(ui, pslider(&mut set.avoid_distance, 0.0..=10.0).text("Obstacle look-ahead")).changed();
                c |= pfield(ui, pslider(&mut set.agent_height, 0.1..=5.0).text("Agent height")).changed();
                c |= pfield(ui, pslider(&mut set.seed, 0..=9999).text("Seed")).changed();
            }
            NodeKind::VellumSolver(set) => {
                ui.heading("Cloth Solver");
                ui.small("Input 1: geometry (a mesh for cloth/softbody, curves for hair, points for grains) • Input 2: optional static collider. Pin points with f@pin = 1; hair roots are pinned automatically.");
                ui.horizontal(|ui| {
                    ui.label("Type");
                    for (value, label) in vellum::VELLUM_TYPES.iter().enumerate() {
                        if ui.selectable_label(set.kind == value, *label).clicked() && set.kind != value {
                            // Switching type loads that type's typical settings.
                            *set = vellum::VellumSettings::defaults(value);
                            c = true;
                        }
                    }
                });
                ui.strong("Constraints");
                if set.kind != 3 {
                    c |= pfield(ui, pslider(&mut set.stretch_stiffness, 1.0..=1e8).logarithmic(true).text("Stretch stiffness")).changed();
                }
                if matches!(set.kind, 0 | 1) {
                    c |= pfield(ui, pslider(&mut set.bend_stiffness, 1e-3..=1e5).logarithmic(true).text("Bend stiffness")).changed();
                }
                if set.kind == 2 {
                    c |= pfield(ui, pslider(&mut set.shape_stiffness, 0.0..=1.0).text("Shape stiffness")).changed();
                }
                c |= pfield(ui, pslider(&mut set.mass, 1e-3..=100.0).logarithmic(true).text("Mass per point")).changed();
                let thickness_label = if set.kind == 3 { "Grain radius (pscale overrides)" } else { "Thickness" };
                c |= pfield(ui, pslider(&mut set.thickness, 1e-3..=0.5).logarithmic(true).text(thickness_label)).changed();
                ui.strong("Forces and collisions");
                c |= pfield(ui, pslider(&mut set.gravity, -30.0..=10.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.damping, 0.0..=5.0).text("Damping")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=1.0).text("Friction")).changed();
                c |= ui.checkbox(&mut set.ground, "Ground plane (y = 0)").changed();
                c |= ui.checkbox(&mut set.self_collision, "Self collision").on_hover_text("Point-based: keeps points apart by their thickness").changed();
                c |= ui
                    .checkbox(&mut set.pin_to_animation, "Pins follow animation")
                    .on_hover_text("Pinned points track the animated input (e.g. hair roots on a moving head)")
                    .changed();
                ui.strong("Solver");
                c |= pfield(ui, pslider(&mut set.substeps, 1..=100).text("Substeps"))
                    .on_hover_text("Raise this first if cloth stretches: small steps converge better than extra iterations")
                    .changed();
                c |= pfield(ui, pslider(&mut set.iterations, 1..=200).text("Constraint iterations")).changed();
            }
            NodeKind::RbdSolver(set) => {
                ui.heading("RBD Bullet Solver");
                ui.small("Input 1: pieces — each connected part (or each value of a piece point attribute) is one rigid body; points become small spheres • Input 2: optional static collider.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Collision shape");
                    for (value, label) in sims::RbdSettings::SHAPES.iter().enumerate() {
                        c |= ui.selectable_value(&mut set.shape, value, *label).changed();
                    }
                });
                if set.shape == 1 {
                    ui.weak("Convex decomposition fits concave pieces but is slow to build.");
                }
                c |= pfield(ui, pslider(&mut set.density, 0.01..=10.0).logarithmic(true).text("Density")).changed();
                c |= pfield(ui, pslider(&mut set.friction, 0.0..=2.0).text("Friction")).changed();
                c |= pfield(ui, pslider(&mut set.bounce, 0.0..=1.0).text("Bounce")).changed();
                c |= pfield(ui, pslider(&mut set.gravity, -20.0..=30.0).text("Gravity")).changed();
                c |= pfield(ui, pslider(&mut set.substeps, 1..=16).text("Substeps")).changed();
                ui.horizontal(|ui| {
                    c |= ui.checkbox(&mut set.ground, "Ground plane").changed();
                    if set.ground {
                        c |= pfield(ui, pdrag(&mut set.ground_height).speed(0.05).prefix("Height ")).changed();
                    }
                });
                c |= vec3_ui(ui, "Initial velocity", &mut set.velocity, 0.05);
                c |= vec3_ui(ui, "Initial spin (rad/s)", &mut set.spin, 0.05);
                ui.small("A point attribute v adds per-piece initial velocity. Output adds point attributes v and piece.");
            }
            NodeKind::SopSolver { substeps } => {
                ui.heading("SOP Solver");
                if solver_display_is_input {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 190, 90),
                        "⚠ The display flag inside this solver is on Prev_Frame/Input_1, so each step changes nothing. Enter the network and set the display flag (double-click) on your last node.",
                    );
                }
                ui.label("Each step cooks this node's network with Prev_Frame set to the previous step's result, so changes accumulate over time. Input_1 is the solver's input at the current step.");
                c |= pfield(ui, pslider(substeps, 1..=16).text("Substeps per frame")).changed();
                if ui.button("Enter solver network").clicked() {
                    self.current_network = Some(id);
                    self.context = AppContext::Obj;
                    self.selected = None;
                    self.graph_pan = egui::Vec2::ZERO;
                    c = true;
                }
                ui.small("At time 0 the output is the input. Example: Prev_Frame → Wrangle (@P.y += 0.02;) with the display flag on the wrangle.");
            }
            _ => return None,
        }
        Some(c)
    }
}
