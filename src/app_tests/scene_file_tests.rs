use crate::*;

#[test]
fn presets_load_and_cook_into_something_to_see() {
    for preset in presets::PRESETS {
        let g = Graph::deserialize(preset.scene);
        let cameras = g.nodes.values().filter(|n| matches!(n.kind, NodeKind::Camera { .. }) && n.name == "sheet_camera").count();
        assert_eq!(cameras, 1, "{}: one sheet camera", preset.name);
        let geo = g.geometry_ids()[0];
        let display = g.display_of(geo).unwrap_or_else(|| panic!("{}: a display node", preset.name));
        // Partway through its range it shows something, as splats (points with Alpha).
        let (start, end) = preset.range;
        let mesh = g.cook(display, start + (end - start) * 0.4, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&display).cloned()).is_none(), "{}: {:?}", preset.name, node_errors(|e| e.get(&display).cloned()));
        // Glowing points (Alpha), or a surface (Toon fire).
        if mesh.indices.is_empty() {
            let alpha = mesh.point_attributes.get("Alpha").unwrap_or_else(|| panic!("{}: no Alpha", preset.name));
            assert!(alpha.iter().filter(|a| **a > 0.05).count() > 20, "{}: too little shows", preset.name);
        } else {
            assert!(mesh.indices.len() > 100, "{}: too little shows", preset.name);
        }
        assert!(presets::find(&preset.name.to_uppercase()).is_some());
    }
}

#[test]
fn the_3d_presets_bake_as_their_kind_and_stay_small() {
    for (name, mode) in [("Flag", vat::Mode::Soft), ("Breaking wall", vat::Mode::Rigid), ("Splash", vat::Mode::Fluid)] {
        let preset = presets::find(name).unwrap();
        assert!(preset.vertex_animation);
        let g = Graph::deserialize(preset.scene);
        let geo = g.geometry_ids()[0];
        let (start, end) = preset.range;
        let times = frame_times(start, end, 1.0 / sims::SIM_FPS);
        let bake = vat::bake_object(&g, geo, &times, None, &mut |_, _| {}).unwrap();
        assert_eq!(bake.mode, mode, "{name}: Automatic picks its kind");
        let layout = vat::Layout::new(bake.elements, bake.frames).unwrap();
        // Both textures, at the precision each is stored in: under 8 MB.
        let bytes = layout.width * layout.height() * if mode == vat::Mode::Soft { 16 } else if mode == vat::Mode::Rigid { 32 } else { 24 };
        assert!(bytes < 8 << 20, "{name}: {bytes} bytes of textures");
    }
    // The flag's last frame leads straight back into its first.
    let flag = presets::find("Flag").unwrap();
    let g = Graph::deserialize(flag.scene);
    let geo = g.geometry_ids()[0];
    let at = |t: f32| g.cook(geo, t, &mut HashMap::new()).vertices;
    let (first, after_last) = (at(flag.range.0), at(flag.range.1 + 1.0 / sims::SIM_FPS));
    assert!(first.iter().zip(&after_last).all(|(a, b)| a.sub(*b).length() < 1e-4), "it loops exactly");
}

#[test]
fn the_toon_fire_preset_shows_as_toon_faces() {
    let preset = presets::find("Toon fire").unwrap();
    let g = Graph::deserialize(preset.scene);
    let geo = g.geometry_ids()[0];
    let shown = g.cook_display(&[geo], preset.range.0 + 0.2, &mut HashMap::new());
    let toon = shown.point_attributes.get(TOON_ATTR).expect("its faces are marked for toon shading");
    assert_eq!(toon.len(), shown.indices.len());
    let material = (toon[0] - 1.0).round() as usize;
    assert!(g.is_toon(material) && toon.iter().all(|t| *t == toon[0]));
    // Coloured by its own flame bands (the Toon FX preset uses Cd).
    let cd = &shown.point_attributes["Cd"];
    assert!(cd.chunks(3).any(|c| c[0] > 0.9 && c[1] > 0.5) && cd.chunks(3).any(|c| c[0] > 0.9 && c[1] < 0.3), "yellow and red bands");
}

#[test]
fn the_fire_preset_loops_in_the_scene_itself() {
    let preset = presets::find("Fire").unwrap();
    let g = Graph::deserialize(preset.scene);
    let display = g.display_of(g.geometry_ids()[0]).unwrap();
    assert!(matches!(g.nodes[&display].kind, NodeKind::MakeLoop { .. }));
    // One loop past the first frame is the first frame again, voxel for voxel.
    let (start, end) = preset.range;
    let length = end - start + 1.0 / sims::SIM_FPS;
    let (a, b) = (g.cook(display, start, &mut HashMap::new()), g.cook(display, start + length, &mut HashMap::new()));
    assert!(!a.vertices.is_empty());
    assert_eq!(a.vertices, b.vertices);
    assert_eq!(a.point_attributes["density"], b.point_attributes["density"]);
}

#[test]
fn connecting_a_solver_clears_its_connect_error() {
    // Whitewater and Crowd say what to connect, and stop saying it once it is.
    let mut g = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 4\nnode 2 40 100 0 obj whitewater_solver\nnode 3 40 100 0 obj crowd_solver\nparent 1 0\nparent 2 0\nparent 3 0\ndisplay 0 2\nnext 4\n");
    for solver in [2, 3] {
        g.cook(solver, 0.0, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&solver).cloned()).is_some_and(|e| e.contains("connect")));
        g.connect(1, solver, 0);
        g.cook(solver, 0.0, &mut HashMap::new());
        assert!(node_errors(|e| e.get(&solver).cloned()).is_none(), "{:?}", node_errors(|e| e.get(&solver).cloned()));
    }
}

#[test]
fn cooked_results_stay_good_until_something_they_depend_on_changes() {
    // box -> transform -> noise, and a separate sphere; object 3 merges object 0.
    let text = "pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj box 1.5 4\nnode 2 40 100 0 obj xform 0 1 0 0 0 0 1 1 1\nnode 4 40 180 0 obj sphere 1 8 16\nlink 1 2 0\nparent 1 0\nparent 2 0\nparent 4 0\ndisplay 0 2\nnode 3 200 20 0 obj geometry -1\nnode 5 200 20 0 obj object_merge\nparent 5 3\ndisplay 3 5\nnext 6\n";
    let mut g = Graph::deserialize(text);
    let prints = |g: &Graph| g.cook_fingerprints(scene_fingerprint(&g.serialize()));
    let before = prints(&g);
    // Moving a node or the display flag changes nothing any SOP depends on.
    g.nodes.get_mut(&1).unwrap().pos.x += 50.0;
    g.set_display(4);
    let after = prints(&g);
    for id in [1, 2, 4] {
        assert_eq!(before[&id], after[&id], "node {id}");
    }
    // But Object Merge (and the objects) read the whole scene, so they follow it.
    assert_ne!(before[&5], after[&5]);
    // Editing the box changes it and what's below, not the sphere beside it.
    if let Some(NodeKind::Box { size, .. }) = g.nodes.get_mut(&1).map(|n| &mut n.kind) {
        *size = 3.0;
    }
    let edited = prints(&g);
    assert_ne!(after[&1], edited[&1]);
    assert_ne!(after[&2], edited[&2]);
    assert_eq!(after[&4], edited[&4]);
}

#[test]
fn the_viewport_shows_edits_and_reuses_what_it_cooked() {
    let mut app = ProceduralFxStudioApp::default();
    app.load_state("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj box 1.5 4\nnode 2 40 100 0 obj xform 0 1 0 0 0 0 1 1 1\nlink 1 2 0\nparent 1 0\nparent 2 0\ndisplay 0 2\nnext 3\n".into());
    app.rebuild_graph();
    let height = |app: &ProceduralFxStudioApp| { let (lo, hi) = app.current_mesh.bounds().unwrap(); hi.y - lo.y };
    assert!((height(&app) - 1.5).abs() < 1e-4);
    // Display the box, then the transform again: both come from what was cooked.
    app.graph.set_display(1);
    app.rebuild_graph();
    assert!(app.static_cache.contains_key(&1) && app.static_cache.contains_key(&2), "kept across the display change");
    app.graph.set_display(2);
    app.rebuild_graph();
    // An edit upstream shows straight away, never the old result.
    if let Some(NodeKind::Box { size, .. }) = app.graph.nodes.get_mut(&1).map(|n| &mut n.kind) {
        *size = 4.0;
    }
    app.rebuild_graph();
    assert!((height(&app) - 4.0).abs() < 1e-4, "{}", height(&app));
}

#[test]
fn dragging_a_rows_label_changes_all_its_values() {
    // Draws "Scale" and its three fields; returns the label's rect.
    let ctx = egui::Context::default();
    let mut v = [1.0f32, 2.0, 3.0];
    let frame = |ctx: &egui::Context, v: &mut [f32; 3], input: egui::RawInput| {
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let before = ui.cursor().min;
                    group_label(ui, "Scale", 0.01, v);
                    rect = egui::Rect::from_min_max(before, ui.min_rect().max);
                });
            });
        });
        rect
    };
    let screen = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0)));
    let input = |events: Vec<egui::Event>, shift: bool| egui::RawInput { screen_rect: screen, events, modifiers: egui::Modifiers { shift, ..Default::default() }, ..Default::default() };
    let rect = frame(&ctx, &mut v, input(vec![], false));
    let at = egui::pos2(rect.min.x + 8.0, rect.center().y);
    let press = |pos: egui::Pos2, pressed: bool, shift: bool| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers { shift, ..Default::default() } };
    // Drag 100 points right: each value goes up by 100 × 0.01.
    frame(&ctx, &mut v, input(vec![egui::Event::PointerMoved(at)], false));
    frame(&ctx, &mut v, input(vec![press(at, true, false)], false));
    for step in 1..=10 {
        frame(&ctx, &mut v, input(vec![egui::Event::PointerMoved(at + egui::vec2(10.0 * step as f32, 0.0))], false));
    }
    frame(&ctx, &mut v, input(vec![press(at + egui::vec2(100.0, 0.0), false, false)], false));
    for (got, want) in v.iter().zip([2.0, 3.0, 4.0]) {
        assert!((got - want).abs() < 0.05, "{v:?}");
    }
    // With Shift, a tenth as much.
    let start = v;
    frame(&ctx, &mut v, input(vec![egui::Event::PointerMoved(at)], true));
    frame(&ctx, &mut v, input(vec![press(at, true, true)], true));
    for step in 1..=10 {
        frame(&ctx, &mut v, input(vec![egui::Event::PointerMoved(at + egui::vec2(10.0 * step as f32, 0.0))], true));
    }
    frame(&ctx, &mut v, input(vec![press(at + egui::vec2(100.0, 0.0), false, true)], true));
    assert!(v.iter().zip(start).all(|(a, b)| (a - b - 0.1).abs() < 0.02), "{start:?} -> {v:?}");
    // A click without dragging changes nothing.
    let still = v;
    frame(&ctx, &mut v, input(vec![press(at, true, false)], false));
    frame(&ctx, &mut v, input(vec![press(at, false, false)], false));
    assert_eq!(v, still);
}

#[test]
fn merging_a_scene_keeps_both_and_renumbers_the_new_one() {
    // A scene whose object refers to its material by id.
    let mut g = Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj box 1.5 4\nparent 1 0\ndisplay 0 1\nnext 2\n");
    let mut other = Graph::deserialize(presets::find("slash").unwrap().scene);
    let material = other.add(NodeKind::Material { kind: 0, color: [1.0; 3], roughness: 0.5, value: 0.5 }, egui::pos2(0.0, 0.0));
    let geo = other.geometry_ids()[0];
    if let Some(NodeKind::Geometry { material: m }) = other.nodes.get_mut(&geo).map(|n| &mut n.kind) {
        *m = Some(material);
    }
    let ids = g.merge(&other);
    assert_eq!(g.nodes.len(), 2 + other.nodes.len());
    assert!(ids.values().all(|id| *id >= 2 && *id < g.next_id));
    let new_geo = ids[&geo];
    assert!(matches!(g.nodes[&new_geo].kind, NodeKind::Geometry { material: Some(m) } if m == ids[&material]));
    assert!(g.nodes[&new_geo].pos.x > g.nodes[&0].pos.x, "placed beside the old nodes");
    // The merged copy cooks the same as the original, channels and all.
    let (a, b) = (other.cook(other.display_of(geo).unwrap(), 0.1, &mut HashMap::new()), g.cook(g.display_of(new_geo).unwrap(), 0.1, &mut HashMap::new()));
    assert!(a.vertices.len() > 100 && a.vertices.len() == b.vertices.len());
    assert_eq!(g.display_of(0), Some(1), "the original object is untouched");
}

// docs/scene-nodes.md lists every node's saved parameters, generated from the code so it
// can't go stale. Rewrite it with PFX_WRITE_DOCS=1 cargo test node_reference.
fn node_reference() -> String {
    let mut out = String::from(
        "# Scene file node reference\n\n\
             Generated from the code by `node_reference_is_current` (see [scene-format.md](scene-format.md)). \
             Each `node` line stores its parameters in the order below, as numbers; the values shown are a new node's defaults. \
             Nodes with text fields store them first, as `t:` followed by hex-encoded UTF-8 strings joined by `|`.\n",
    );
    let mut seen = std::collections::HashSet::new();
    for (category, kinds) in NodeKind::templates() {
        let mut rows = String::new();
        for kind in kinds {
            if !seen.insert(kind.menu_name()) {
                continue;
            }
            let (params, names, texts) = (kind.params(), kind.param_names(), kind.texts());
            // Table-driven nodes share one saved name and store which operation they are first.
            let table = matches!(kind, NodeKind::Vop(_) | NodeKind::Cop(_) | NodeKind::Top(_) | NodeKind::ShaderNode(_) | NodeKind::Pop(_));
            let numbers = params
                .iter()
                .enumerate()
                .map(|(i, v)| match names.get(i).filter(|n| !n.is_empty()) {
                    _ if table && i == 0 => format!("operation={v}"),
                    Some(n) => format!("{n}={v}"),
                    None => format!("#{i}={v}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let special = match &kind {
                NodeKind::Wrangle { .. } => "the run-over domain (0–3), `:`, then the code in hex".to_owned(),
                NodeKind::MoonRayRender { .. } => "`mr:` then 20 fields separated by `\\|` (see scene-format.md)".to_owned(),
                _ if texts.is_empty() => numbers,
                _ if numbers.is_empty() => format!("{} text field(s)", texts.len()),
                _ => format!("{} text field(s); {numbers}", texts.len()),
            };
            rows += &format!("| {} | `{}` | {} |\n", kind.menu_name().replace('|', "/"), kind.name(), if special.is_empty() { "none".into() } else { special });
        }
        if !rows.is_empty() {
            out += &format!("\n## {category}\n\n| Node | Saved as | Parameters |\n| --- | --- | --- |\n{rows}");
        }
    }
    out
}

#[test]
fn the_scene_format_example_loads() {
    let doc = include_str!("../../docs/scene-format.md");
    let example = doc.split("```\n").nth(1).unwrap();
    let g = Graph::deserialize(example);
    assert!(matches!(g.nodes[&2].kind, NodeKind::Noise { amp, .. } if amp == 0.2));
    assert_eq!(g.nodes[&2].parent, Some(0));
    assert_eq!(g.nodes[&0].name, "rocks");
    assert_eq!(g.xform_at(0, 0.0).t, [0.0, 1.0, 0.0]);
    assert_eq!(g.xform_at(0, 0.0).r, [0.0, 45.0, 0.0]);
    assert!(matches!(g.kind_at(2, 2.0).as_ref(), NodeKind::Noise { amp, .. } if (*amp - 0.8).abs() < 1e-5));
    assert_eq!(g.next_id, 3);
    assert_eq!(g.serialize(), example);
}

#[test]
fn node_reference_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/scene-nodes.md");
    let text = node_reference();
    if std::env::var_os("PFX_WRITE_DOCS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert!(std::fs::read_to_string(&path).is_ok_and(|saved| saved == text), "docs/scene-nodes.md is out of date: run PFX_WRITE_DOCS=1 cargo test node_reference");
}

#[test]
fn recent_scenes_dedupe_and_cap() {
    let text = "/a.pfs\n\n/b.pfs\n/a.pfs\n".to_owned() + &(0..20).map(|i| format!("/s{i}.pfs\n")).collect::<String>();
    let list = parse_recent_scenes(&text);
    assert_eq!(list.len(), MAX_RECENT_SCENES);
    assert_eq!(list[0], PathBuf::from("/a.pfs"));
    assert_eq!(list[1], PathBuf::from("/b.pfs"));

    let mut list = vec![PathBuf::from("/a.pfs"), PathBuf::from("/b.pfs")];
    push_recent_scene(&mut list, PathBuf::from("/b.pfs"));
    assert_eq!(list, vec![PathBuf::from("/b.pfs"), PathBuf::from("/a.pfs")]);
}

#[test]
fn old_particle_nodes_load_with_defaults() {
    let old = NodeKind::from_params("particles", &[2000.0, 400.0, 4.0, 2.0, 0.35, -1.5, 1.0]).unwrap();
    let NodeKind::Particles { bounce, friction, radius, substeps, along_normals, drag, .. } = old else { panic!() };
    assert_eq!((bounce, friction, radius, substeps, along_normals, drag), (0.4, 0.1, 0.02, 2, false, 0.0));
    let round_trip = NodeKind::from_params("particles", &old.params()).unwrap();
    assert_eq!(format!("{round_trip:?}"), format!("{old:?}"));
}

#[test]
fn new_nodes_survive_save_and_load() {
    let mut g = Graph::default();
    let painted = g.add(
        NodeKind::AttributePaint {
            attribute: "clump".into(),
            values: vec![0.0, 0.25, 1.0],
            settings: paint::PaintSettings { value: 0.8, default: 0.1, radius: 55.0, strength: 0.3, mode: 2, visualize: false },
        },
        egui::pos2(0.0, 0.0),
    );
    let kinds = [
        NodeKind::VellumSolver(vellum::VellumSettings { iterations: 33, ..vellum::VellumSettings::defaults(1) }),
        NodeKind::Normal(sops::NormalSettings { domain: 1, cusp_angle: 45.0, weighting: 2, reverse: true }),
        NodeKind::Scatter(sops::ScatterSettings { count: 77, seed: 4, relax_iterations: 2, volume: true }),
        NodeKind::UvTexture(sops::UvTextureSettings { projection: 1, axis: 2, scale: [2.0, 3.0], offset: [0.1, 0.2] }),
        NodeKind::UvTransform(sops::UvTransformSettings { translate: [1.0, 2.0], rotate: 30.0, scale: [0.5, 0.5], pivot: [0.5, 0.5] }),
        NodeKind::UvQuickShade { file: "/textures/grid.png".into() },
        NodeKind::AttributeTransfer(transfer::TransferSettings { class: 1, pattern: "Cd ^id".into(), samples: 3, distance: 0.25, blend_width: 0.5 }),
        NodeKind::Color(color::ColorSettings { class: 1, mode: 1, seed: 7, random_by: "piece".into(), ..color::ColorSettings::defaults() }),
        NodeKind::Enumerate(enumerate::EnumerateSettings { class: 1, attribute: "variant".into(), piece_attribute: "name".into(), start: 3.0, increment: 2.0 }),
        NodeKind::AttributePromote(promote::PromoteSettings { pattern: "Cd mask".into(), from: 2, to: 3, method: 4, rename: String::new(), delete_original: false }),
        NodeKind::AttributeBlur(blur::BlurSettings { pattern: "Cd mask".into(), mode: 1, weighting: 1, iterations: 7, step: 0.4, radius: 0.3, max_points: 6, pin_border: true }),
        NodeKind::CopyToPoints { scale: 0.5, rotation: [0.0, 45.0, 0.0], piece_attribute: "variant".into() },
        NodeKind::Boolean(boolean::BooleanSettings { operation: 1, mode: 1, resolution: 90 }),
        NodeKind::MaterialFracture(fracture::FractureSettings { pieces: 33, splinter: 7.0, ..fracture::FractureSettings::defaults(2) }),
        NodeKind::CrowdSolver(crowd::CrowdSettings { goal: [1.0, 2.0, 3.0], display: 0, ..crowd::CrowdSettings::defaults() }),
        NodeKind::Muscle(muscle::MuscleSettings { activation: 0.4, profile: 1, ..muscle::MuscleSettings::defaults() }),
        NodeKind::MuscleSolver(muscle::JiggleSettings { stiffness: 77.0, ..muscle::JiggleSettings::defaults() }),
        NodeKind::MuscleSkin(muscle::SkinSettings { falloff: 0.7, influences: 3 }),
        NodeKind::Skeleton(kinefx::SkeletonSettings { preset: 2, scale: 2.0, joints: 9, length: 3.0 }),
        NodeKind::RigPose({
            let mut p = kinefx::RigPose::new();
            p.joints[2] = "forearm_l".into();
            p.values[2] = [10.0, 20.0, 30.0, 0.1, 0.2, 0.3];
            p
        }),
        NodeKind::TwoBoneIk(kinefx::IkSettings { root: "upperarm_r".into(), goal: [1.0, 2.0, 3.0], keep_tip: false, ..kinefx::IkSettings::defaults() }),
        NodeKind::ProceduralMotion(kinefx::MotionSettings { motion: 1, lag: 0.9, axis: 2, ..kinefx::MotionSettings::defaults() }),
        NodeKind::SkeletonBlend { weight: 0.3 },
        NodeKind::JointCapture(kinefx::CaptureSettings { influences: 2, falloff: 6.0, visualize: true }),
        NodeKind::BoneDeform,
        NodeKind::WhitewaterSolver(whitewater::WhitewaterSettings { lifespan: 3.5, ..whitewater::WhitewaterSettings::defaults() }),
        NodeKind::Shader(shading::Values::preset(shading::Table::Principled, 5)),
        NodeKind::Shader(shading::Values::preset(shading::Table::Pyro, 2)),
        NodeKind::Terrain(terrain::TerrainSettings { size: [12.0, 7.0], resolution: 300 }),
        NodeKind::TerrainNoise(terrain::NoiseSettings { kind: 3, blend: 2, offset: [1.0, -2.0], use_mask: true, ..terrain::NoiseSettings::defaults() }),
        NodeKind::TerrainErode(terrain::ErodeSettings { droplets: 1234, talus_angle: 33.0, ..terrain::ErodeSettings::defaults() }),
        NodeKind::TerrainTerrace(terrain::TerraceSettings { step: 0.3, offset: 0.1, sharpness: 0.9, use_mask: true }),
        NodeKind::TerrainMask(terrain::MaskSettings { source: 2, min: 0.2, max: 0.8, falloff: 0.1, blur: 3, invert: true, combine: 1, visualize: false }),
        NodeKind::TerrainColorize(terrain::ColorizeSettings { snow: [0.1, 0.2, 0.3], rock_slope: 50.0, ..terrain::ColorizeSettings::defaults() }),
    ];
    let ids: Vec<NodeId> = kinds.iter().cloned().map(|k| g.add(k, egui::pos2(0.0, 0.0))).collect();
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(format!("{:?}", loaded.nodes[&painted].kind), format!("{:?}", g.nodes[&painted].kind));
    for id in ids {
        assert_eq!(format!("{:?}", loaded.nodes[&id].kind), format!("{:?}", g.nodes[&id].kind));
    }
}

#[test]
fn boolean_and_terrain_nodes_cook_in_a_network() {
    let mut g = Graph::default();
    g.next_id += 9100;
    let a = g.add(NodeKind::Box { size: 2.0, div: 1 }, egui::pos2(0.0, 0.0));
    let b = g.add(NodeKind::Sphere { radius: 0.8, rows: 16, cols: 24 }, egui::pos2(0.0, 0.0));
    let cut = g.add(NodeKind::Boolean(boolean::BooleanSettings { operation: 2, mode: 0, resolution: 64 }), egui::pos2(0.0, 0.0));
    g.connect(a, cut, 0);
    let missing = g.cook(cut, 0.0, &mut HashMap::new());
    assert!(missing.indices.is_empty());
    assert!(node_errors(|e| e.get(&cut).cloned()).unwrap().contains("right input"));
    g.connect(b, cut, 1);
    let out = g.cook(cut, 0.0, &mut HashMap::new());
    assert!(!out.indices.is_empty() && node_errors(|e| e.get(&cut).cloned()).is_none());
    assert!(out.point_attributes.contains_key("prim:group:b_faces"));
    // Box minus sphere: 8 - 4/3·π·0.8³ (the sphere's facets make it a little smaller).
    let volume: f32 = out.indices.iter().map(|t| {
        let [p, q, r] = t.map(|i| out.vertices[i]);
        p.dot(q.cross(r)) / 6.0
    }).sum();
    assert!((volume - 5.86).abs() < 0.15, "volume {volume}");

    let land = g.add(NodeKind::Terrain(terrain::TerrainSettings { size: [10.0, 10.0], resolution: 64 }), egui::pos2(0.0, 0.0));
    let mut chain = vec![land];
    for kind in NodeKind::templates().into_iter().find(|(m, _)| *m == "Terrain").unwrap().1.into_iter().skip(1) {
        let node = g.add(kind, egui::pos2(0.0, 0.0));
        g.connect(*chain.last().unwrap(), node, 0);
        chain.push(node);
    }
    let out = g.cook(*chain.last().unwrap(), 0.0, &mut HashMap::new());
    for id in &chain {
        assert!(node_errors(|e| e.get(id).cloned()).is_none(), "{:?}", g.nodes[id].kind);
    }
    assert_eq!(out.vertices.len(), 65 * 65);
    assert!(out.vertices.iter().any(|v| v.y > 0.5));
    assert_eq!(out.point_attributes["Cd"].len(), out.vertices.len() * 3);
    assert!(out.point_attributes.contains_key("mask") && out.point_attributes.contains_key("flow"));
    // A non-terrain input is reported, not mangled.
    let grid = g.add(NodeKind::Grid { size: 1.0, res: 4 }, egui::pos2(0.0, 0.0));
    let noise = g.add(NodeKind::TerrainNoise(terrain::NoiseSettings::defaults()), egui::pos2(0.0, 0.0));
    g.connect(grid, noise, 0);
    g.cook(noise, 0.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&noise).cloned()).unwrap().contains("isn't a terrain"));
}

#[test]
fn vellum_node_cooks_cloth_falling_onto_the_ground() {
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 8 }, egui::pos2(0.0, 0.0));
    let lift = g.add(NodeKind::Wrangle { code: "@P.y += 1.0;".into(), domain: 2 }, egui::pos2(0.0, 0.0));
    let cloth = g.add(NodeKind::VellumSolver(vellum::VellumSettings::defaults(0)), egui::pos2(0.0, 0.0));
    g.connect(grid, lift, 0);
    g.connect(lift, cloth, 0);
    sims::clear_all();
    let start = g.cook(cloth, 0.0, &mut HashMap::new());
    let later = g.cook(cloth, 1.5, &mut HashMap::new());
    assert!(start.vertices.iter().all(|p| (p.y - 1.0).abs() < 1e-5));
    assert!(later.vertices.iter().all(|p| p.y < 0.2 && p.y > -1e-3), "lies on the ground");
    assert_eq!(later.indices, start.indices);
}

// The reported setup: a 4 x 4 grid (20 x 20 points) at y = 0 over a 1.5 box whose top is at y = -0.92.
fn cloth_over_box(ground: bool) -> (Graph, NodeId, NodeId) {
    let mut g = Graph::default();
    // Distinct node ids per variant: node notes are shared by tests running in parallel.
    g.next_id = if ground { 500 } else { 600 };
    let grid = g.add(NodeKind::Grid { size: 4.0, res: 20 }, egui::pos2(0.0, 0.0));
    let cloth = g.add(NodeKind::VellumSolver(vellum::VellumSettings { ground, ..vellum::VellumSettings::defaults(0) }), egui::pos2(0.0, 0.0));
    let block = g.add(NodeKind::Box { size: 1.5, div: 4 }, egui::pos2(0.0, 0.0));
    let lower = g.add(NodeKind::Transform { t: [0.0, -1.67, 0.0], r: [0.0; 3], s: [1.0; 3] }, egui::pos2(0.0, 0.0));
    g.connect(grid, cloth, 0);
    g.connect(block, lower, 0);
    g.connect(lower, cloth, 1);
    (g, grid, cloth)
}

#[test]
fn vellum_cloth_drapes_over_a_lowered_box() {
    sims::clear_all();
    let (g, grid, cloth) = cloth_over_box(false);
    let rest = g.cook(grid, 0.0, &mut HashMap::new());
    let out = g.cook(cloth, 3.0, &mut HashMap::new());
    let centre = (0..rest.vertices.len()).min_by(|&a, &b| (rest.vertices[a].x.abs() + rest.vertices[a].z.abs()).total_cmp(&(rest.vertices[b].x.abs() + rest.vertices[b].z.abs()))).unwrap();
    assert!((out.vertices[centre].y + 0.90).abs() < 0.03, "rests on the box top: {:?}", out.vertices[centre]);
    assert!(out.vertices.iter().all(|p| !(p.x.abs() < 0.74 && p.z.abs() < 0.74 && p.y < -0.93 && p.y > -2.4)), "nothing inside the box");
    let stretch = rest
        .indices
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| (out.vertices[a].sub(out.vertices[b]).length() / rest.vertices[a].sub(rest.vertices[b]).length() - 1.0).abs())
        .fold(0.0, f32::max);
    assert!(stretch < 0.08, "max stretch {:.1}%", stretch * 100.0);
    assert!(out.vertices.iter().map(|p| p.y).fold(f32::INFINITY, f32::min) < -2.0, "the sides hang down");
    assert!(node_notes(|n| n.get(&cloth).cloned()).is_none());
}

#[test]
fn vellum_explains_geometry_resting_on_the_ground() {
    sims::clear_all();
    let (g, _, cloth) = cloth_over_box(true);
    let out = g.cook(cloth, 1.0, &mut HashMap::new());
    // It stays on the ground, without being launched off it...
    assert!(out.vertices.iter().all(|p| (p.y - 0.02).abs() < 1e-3), "{:?}", out.vertices[0]);
    // ...and the node says why it doesn't fall.
    assert!(node_notes(|n| n.get(&cloth).cloned()).is_some_and(|n| n.contains("ground plane")));
}

#[test]
fn background_cook_names_its_solvers() {
    let (g, _, cloth) = cloth_over_box(false);
    assert_eq!(g.cooking_label(&[cloth]), "Simulating Cloth");
    let mut g = Graph::default();
    let grid = g.add(NodeKind::Grid { size: 1.0, res: 4 }, egui::pos2(0.0, 0.0));
    let wave = g.add(NodeKind::Ripple { freq: 1.0, amp: 0.1, speed: 1.0 }, egui::pos2(0.0, 0.0));
    g.connect(grid, wave, 0);
    assert_eq!(g.cooking_label(&[wave]), "Cooking the animated frame");
    let pyro = g.add(NodeKind::Pyro(sims::PyroSettings::default()), egui::pos2(0.0, 0.0));
    g.connect(wave, pyro, 0);
    let vellum = g.add(NodeKind::VellumSolver(vellum::VellumSettings::defaults(0)), egui::pos2(0.0, 0.0));
    g.connect(pyro, vellum, 0);
    assert_eq!(g.cooking_label(&[vellum]), "Simulating Pyro + Cloth");
}

#[test]
fn atomic_save_replaces_file() {
    let path = std::env::temp_dir().join(format!("pfx_atomic_{}.pfs", std::process::id()));
    write_file_atomically(&path, "first").unwrap();
    write_file_atomically(&path, "second").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    assert!(!PathBuf::from(format!("{}.saving", path.display())).exists());
    let _ = std::fs::remove_file(&path);
}

// String attributes go through the Cache node's files as text, and come back as the same
// strings (ids only mean something while the app runs).
#[test]
fn cached_geometry_keeps_its_string_attributes() {
    let mut mesh = Mesh::default();
    mesh.vertices = vec![Vec3::zero(), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)];
    mesh.indices = vec![[0, 1, 2]];
    mesh.point_attributes.insert("str:name".into(), ["piece one", "", "piece \"two\""].iter().map(|t| strings::id(t)).collect());
    mesh.point_attributes.insert("prim:str:path".into(), vec![strings::id("/geo/wall")]);
    let path = std::env::temp_dir().join(format!("pfx_strings_{}.mcache", std::process::id()));
    save_mesh_cache(&path, &mesh).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("@sattr:str:name") && !text.contains("piece one"), "saved as text, hex-encoded");
    let back = load_mesh_cache(&path).unwrap();
    let texts = |key: &str| back.point_attributes[key].iter().map(|v| strings::text(*v).to_string()).collect::<Vec<_>>();
    assert_eq!(texts("str:name"), vec!["piece one", "", "piece \"two\""]);
    assert_eq!(texts("prim:str:path"), vec!["/geo/wall"]);
    let _ = std::fs::remove_file(&path);
    // The spreadsheet shows text, under the attribute's own name.
    let columns = sheet_columns(&mesh, SheetDomain::Points);
    let name = columns.iter().find(|c| c.header == "name").expect("a name column");
    let polys = SheetPolys::new(&mesh);
    assert_eq!(sheet_cell(&mesh, &[], &polys, &name.source, 2), "piece \"two\"");
}
