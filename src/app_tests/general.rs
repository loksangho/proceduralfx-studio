use crate::*;

#[test]
fn materials_and_render_options_round_trip() {
    let mut g = Graph::default();
    let m = g.add(NodeKind::Material { kind: 2, color: [0.5, 0.6, 0.7], roughness: 0.1, value: 1.4 }, egui::pos2(0.0, 0.0));
    g.add(NodeKind::Geometry { material: Some(m) }, egui::pos2(0.0, 0.0));
    let opts = RenderOpts {
        camera: RenderCamera::Node(7),
        max_depth: 9,
        light_samples: 3,
        bsdf_samples: 4,
        background: [0.1, 0.2, 0.3],
        sequence: true,
        range_start: 0.5,
        range_end: 2.0,
        range_step: 0.25,
        stage: true,
        hydra: true,
        volume_samples: 3,
    };
    let r = g.add(
        NodeKind::MoonRayRender {
            executable: "moonray".into(),
            arguments: "{scene}".into(),
            output: "out.exr".into(),
            width: 320,
            height: 200,
            samples: 16,
            opts: opts.clone(),
        },
        egui::pos2(0.0, 0.0),
    );
    g.nodes.get_mut(&r).unwrap().context = AppContext::Out;
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(loaded.serialize(), g.serialize());
    assert!(matches!(&loaded.nodes[&r].kind, NodeKind::MoonRayRender { opts: o, .. } if *o == opts));
    assert!(matches!(loaded.nodes[&m].kind, NodeKind::Material { kind: 2, .. }));
}

#[test]
fn frame_ranges_and_output_names() {
    assert_eq!(frame_times(0.0, 1.0, 0.25), vec![0.0, 0.25, 0.5, 0.75, 1.0]);
    assert_eq!(frame_times(2.0, 1.0, 0.5).len(), 1);
    // The system's own separator before the new file name ("\\" on Windows).
    assert_eq!(frame_output("/out/render.exr", 7), std::path::Path::new("/out/render.exr").with_file_name("render.0007.exr").to_string_lossy());
    assert_eq!(frame_output("/out/shot_####.png", 12), "/out/shot_0012.png");
    assert_eq!(frame_output("beauty", 3), "beauty.0003");
}

#[test]
fn flat_scene_is_wrapped_in_a_geometry_container() {
    let legacy = "mhip 1\n\
            node 0 0 0 0 obj grid 4 20\n\
            node 1 0 80 0 obj twist 1.8\n\
            node 2 0 0 0 obj light 3 4 3 -35 35 0 1 0.95 0.85 0.9 0\n\
            link 0 1 0\n\
            display 1\n\
            next 3\n";
    let g = Graph::deserialize(legacy);
    let geo = g.geometry_ids();
    assert_eq!(geo.len(), 1);
    assert_eq!(g.nodes[&0].parent, Some(geo[0]));
    assert_eq!(g.nodes[&1].parent, Some(geo[0]));
    assert_eq!(g.nodes[&2].parent, None);
    assert_eq!(g.display_of(geo[0]), Some(1));
    assert!(!g.cook_many(&geo, 0.0, &mut HashMap::new()).vertices.is_empty());

    let round_trip = Graph::deserialize(&g.serialize());
    assert_eq!(round_trip.serialize(), g.serialize());
}

#[test]
fn duplicating_and_removing_geometry_carries_its_network() {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let grid = g.add(NodeKind::Grid { size: 1.0, res: 2 }, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
    g.set_display(grid);
    let copy = g.duplicate(geo).unwrap();
    assert_ne!(g.display_of(copy), Some(grid));
    assert!(g.display_of(copy).is_some());
    g.remove(geo);
    assert!(!g.nodes.contains_key(&grid));
    assert_eq!(g.nodes.len(), 2);
}

fn quad() -> Mesh {
    Mesh {
        vertices: vec![Vec3::zero(), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0)],
        indices: vec![[0, 1, 2], [0, 2, 3]],
        point_attributes: HashMap::new(),
    }
}

#[test]
fn merge_keeps_primitive_vertex_and_detail_attributes_sized() {
    let mut a = quad();
    a.point_attributes.insert("prim:Cd".into(), vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    a.point_attributes.insert("detail:total".into(), vec![7.0]);
    let mut b = quad();
    b.point_attributes.insert("pscale".into(), vec![2.0; 4]);
    b.point_attributes.insert("vertex:uvw".into(), vec![0.5; 6]);
    let merged = node_merge(a, b);
    let attrs = &merged.point_attributes;
    // Missing colour reads white and missing pscale 1, as in Houdini.
    assert_eq!(attrs["prim:Cd"], vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    assert_eq!(attrs["pscale"], vec![1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    assert_eq!(attrs["vertex:uvw"].len(), 12);
    assert_eq!(attrs["detail:total"], vec![7.0]);
}

#[test]
fn spreadsheet_lists_attributes_created_by_a_wrangle() {
    let mesh = node_wrangle(quad(), &[], "@Cd = {1, 0, 0}; f@mass = 2.5; i@group = @ptnum;", 0.0, 2, None, None);
    let mesh = node_wrangle(mesh, &[], "f@area = 0.5;", 0.0, 1, None, None);
    let mesh = node_wrangle(mesh, &[], "i@count = npoints(0);", 0.0, 0, None, None);
    let headers = |domain| sheet_columns(&mesh, domain).into_iter().map(|c| c.header).collect::<Vec<_>>();
    let points = headers(SheetDomain::Points);
    for expected in ["#", "P.x", "N.z", "Cd.r", "Cd.b", "group", "mass"] {
        assert!(points.contains(&expected.to_string()), "{expected} missing from {points:?}");
    }
    assert_eq!(headers(SheetDomain::Primitives), vec!["#", "Points", "area"]);
    assert_eq!(headers(SheetDomain::Detail), vec!["count"]);
    let columns = sheet_columns(&mesh, SheetDomain::Points);
    let group = columns.iter().find(|c| c.header == "group").unwrap();
    assert_eq!(sheet_cell(&mesh, &[], &SheetPolys::new(&mesh), &group.source, 3), "3");
    let mass = columns.iter().find(|c| c.header == "mass").unwrap();
    assert_eq!(sheet_cell(&mesh, &[], &SheetPolys::new(&mesh), &mass.source, 1), "2.500");
}


fn solver_scene() -> (Graph, NodeId) {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let grid = g.add(NodeKind::Grid { size: 1.0, res: 2 }, egui::pos2(0.0, 0.0));
    let solver = g.add(NodeKind::SopSolver { substeps: 1 }, egui::pos2(0.0, 80.0));
    let prev = g.add(NodeKind::SolverInput { index: 0 }, egui::pos2(0.0, 0.0));
    let wrangle = g.add(NodeKind::Wrangle { code: "@P.y += 1;".into(), domain: 2 }, egui::pos2(0.0, 80.0));
    for (node, parent) in [(grid, geo), (solver, geo), (prev, solver), (wrangle, solver)] {
        g.nodes.get_mut(&node).unwrap().parent = Some(parent);
    }
    g.connect(grid, solver, 0);
    g.connect(prev, wrangle, 0);
    g.set_display(wrangle);
    g.set_display(solver);
    (g, solver)
}

#[test]
fn sop_solver_feeds_its_output_back_every_step() {
    let (g, solver) = solver_scene();
    let height = |t: f32| g.cook(solver, t, &mut HashMap::new()).vertices[0].y;
    assert_eq!(height(0.0), 0.0, "time 0 is the input");
    assert_eq!(height(1.0), sims::SIM_FPS);
    assert_eq!(height(0.5), sims::SIM_FPS / 2.0, "scrubbing back resumes from a checkpoint");
    assert!(g.time_dependent(solver));
}

#[test]
fn solver_nodes_survive_save_and_load() {
    let (mut g, solver) = solver_scene();
    let geo = g.nodes[&solver].parent;
    let pyro = g.add(NodeKind::Pyro(sims::PyroSettings { fuel: 0.5, ..Default::default() }), egui::pos2(0.0, 0.0));
    let rbd = g.add(NodeKind::RbdSolver(sims::RbdSettings { shape: 2, ..Default::default() }), egui::pos2(0.0, 0.0));
    for node in [pyro, rbd] {
        g.nodes.get_mut(&node).unwrap().parent = geo;
    }
    let loaded = Graph::deserialize(&g.serialize());
    assert!(matches!(&loaded.nodes[&pyro].kind, NodeKind::Pyro(s) if s.fuel == 0.5));
    assert!(matches!(&loaded.nodes[&rbd].kind, NodeKind::RbdSolver(s) if s.shape == 2));
    assert!(matches!(loaded.nodes[&solver].kind, NodeKind::SopSolver { substeps: 1 }));
    assert_eq!(loaded.children(solver).len(), 2);
    assert_eq!(loaded.sim_key(solver), g.sim_key(solver), "same scene, same simulation");
}


#[test]
fn time_inc_inside_a_sop_solver_is_the_step_length() {
    let (mut g, solver) = solver_scene();
    let wrangle = g.display_of(solver).unwrap();
    if let Some(NodeKind::Wrangle { code, .. }) = g.nodes.get_mut(&wrangle).map(|n| &mut n.kind) {
        *code = "float speed = 2.0; // Units per second\n@P.y += speed * @TimeInc;".into();
    }
    if let Some(NodeKind::SopSolver { substeps }) = g.nodes.get_mut(&solver).map(|n| &mut n.kind) {
        *substeps = 4;
    }
    let height = g.cook(solver, 1.5, &mut HashMap::new()).vertices[0].y;
    assert!((height - 3.0).abs() < 1e-3, "2 units/s for 1.5 s, got {height}");
}


#[test]
fn scenes_with_the_removed_fluid_node_still_load() {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let grid = g.add(NodeKind::Grid { size: 1.0, res: 2 }, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
    g.set_display(grid);
    // An old file: a fluid node wired into the grid's network and holding the display flag.
    let old = g.serialize().replace(
        "next ",
        &format!("node 90 0 0 0 obj fluid 0 20 1 1 0.5 1 7\nparent 90 {geo}\nlink 90 {grid} 0\ndisplay {geo} 90\nnext "),
    );
    let loaded = Graph::deserialize(&old);
    assert!(!loaded.nodes.contains_key(&90));
    assert!(loaded.links.iter().all(|l| l.from != 90 && l.to != 90));
    assert!(loaded.nodes.contains_key(&grid));
}


// geometry object (grid) + Material, and a STAGE network importing it with a sphere, a light,
// a transform, a material assignment and a prune.
fn usd_scene() -> (Graph, NodeId) {
    let mut g = Graph::default();
    let material = g.add(NodeKind::Material { kind: 1, color: [0.9, 0.2, 0.1], roughness: 0.3, value: 0.5 }, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&material).unwrap().context = AppContext::Mat;
    let geo = g.add(NodeKind::Geometry { material: Some(material) }, egui::pos2(0.0, 0.0));
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 2 }, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
    g.set_display(grid);
    let lops = [
        NodeKind::LopSopImport { geometry: None, path: "/geo".into() },
        NodeKind::LopPrimitive { shape: 0, path: "/shapes/ball".into(), size: 0.5, t: [0.0, 2.0, 0.0] },
        NodeKind::LopLight { kind: 0, path: "/lights/key".into(), position: [0.0, 5.0, 0.0], target: [0.0; 3], color: [1.0; 3], intensity: 2.0, size: 1.0 },
        NodeKind::LopTransform { pattern: "/shapes/*".into(), t: [1.0, 0.0, 0.0], r: [0.0; 3], s: [1.0; 3] },
        NodeKind::LopMaterial { pattern: "/shapes/**".into(), material: Some(material) },
        NodeKind::LopPrune { pattern: "/lights/key".into(), hide: true },
    ];
    let mut previous = None;
    let mut last = 0;
    for kind in lops {
        let id = g.add(kind, egui::pos2(0.0, 0.0));
        g.nodes.get_mut(&id).unwrap().context = AppContext::Stage;
        if let Some(p) = previous {
            g.connect(p, id, 0);
        }
        previous = Some(id);
        last = id;
    }
    assert!(g.set_display(last));
    (g, last)
}

#[test]
fn lop_network_builds_a_stage() {
    let (g, last) = usd_scene();
    let stage = g.cook_stage(last, 0.0, &mut HashMap::new());
    let geo_prim = stage.prims.keys().find(|p| p.starts_with("/geo/geometry")).cloned().expect("imported mesh");
    assert_eq!(stage.prims[&geo_prim].type_name, "Mesh");
    assert!(stage.prims[&geo_prim].binding.as_deref().is_some_and(|b| b.starts_with("/materials/mat")));
    assert_eq!(stage.world_xform("/shapes/ball")[3], [1.0, 2.0, 0.0, 1.0]);
    assert_eq!(stage.prims["/shapes/ball"].binding, stage.prims[&geo_prim].binding);
    assert!(!stage.prims["/lights/key"].visible);
    // The node's mesh output is the flattened stage, coloured from the materials.
    let mesh = g.cook(last, 0.0, &mut HashMap::new());
    assert!(mesh.indices.len() > 8);
    assert!(mesh.point_attributes.contains_key("vertex:Cd"));
    assert_eq!(g.display_of(STAGE_NETWORK), Some(last));
}

#[test]
fn lop_nodes_save_and_load_with_their_text() {
    let (g, last) = usd_scene();
    let loaded = Graph::deserialize(&g.serialize());
    assert!(matches!(&loaded.nodes[&last].kind, NodeKind::LopPrune { pattern, hide: true } if pattern == "/lights/key"));
    assert_eq!(loaded.nodes[&last].context, AppContext::Stage);
    assert_eq!(loaded.display_of(STAGE_NETWORK), Some(last));
    let stage = loaded.cook_stage(last, 0.0, &mut HashMap::new());
    assert!(stage.prims.contains_key("/shapes/ball"));
}

#[test]
fn usd_files_round_trip_through_openusd() {
    let (g, last) = usd_scene();
    let stage = g.cook_stage(last, 0.0, &mut HashMap::new());
    let dir = std::env::temp_dir();
    for extension in ["usda", "usdc", "usdz"] {
        let path = dir.join(format!("pfx_round_trip.{extension}"));
        let path = path.to_string_lossy().into_owned();
        let saved = usd::save(&stage, &path);
        if !usd::NATIVE {
            assert_eq!(saved.is_ok(), extension == "usda");
            continue;
        }
        saved.unwrap_or_else(|e| panic!("save {extension}: {e}"));
        let loaded = usd::load_file(&path, 0.0).unwrap_or_else(|e| panic!("load {extension}: {e}"));
        let geo_prim = stage.prims.keys().find(|p| p.starts_with("/geo/geometry")).unwrap();
        let (a, b) = (stage.prims[geo_prim].mesh.as_ref().unwrap(), loaded.prims[geo_prim].mesh.as_ref().unwrap());
        assert_eq!(a.vertices, b.vertices, "{extension}: points come back in app space");
        // Faces come back as the same polygons, wound the same way (a quad may split along its other diagonal).
        let outlines = |m: &Mesh| -> Vec<Vec<usize>> {
            poly::faces(m).iter().map(|f| {
                let pts: Vec<usize> = f.corners.iter().map(|c| c.0).collect();
                let start = (0..pts.len()).min_by_key(|&i| pts[i]).unwrap_or(0);
                pts[start..].iter().chain(&pts[..start]).copied().collect()
            }).collect()
        };
        assert_eq!(outlines(a), outlines(b), "{extension}: polygons and their winding survive the mirror");
        assert_eq!(loaded.world_xform("/shapes/ball")[3], [1.0, 2.0, 0.0, 1.0], "{extension}");
        assert_eq!(loaded.prims["/shapes/ball"].float("radius"), Some(0.5), "{extension}");
        assert!(!loaded.prims["/lights/key"].visible, "{extension}");
        let material = loaded.prims[geo_prim].binding.clone().expect("binding survives");
        assert_eq!(loaded.prims[&material].vec3("diffuseColor"), Some([0.9, 0.2, 0.1]), "{extension}");
        assert_eq!(loaded.prims[&material].float("metallic"), Some(1.0), "{extension}");
        let _ = std::fs::remove_file(&path);
    }
}


#[test]
fn vdb_from_polygons_then_convert_vdb() {
    let mut g = Graph::default();
    let sphere = g.add(NodeKind::Sphere { radius: 1.0, rows: 24, cols: 48 }, egui::pos2(0.0, 0.0));
    let vdb = g.add(NodeKind::Vdb { voxel_size: 0.08, padding: 0, operation: 0, iterations: 1, output: 1 }, egui::pos2(0.0, 0.0));
    let to_polygons = g.add(NodeKind::ConvertVdb { mode: 0, iso: 0.0 }, egui::pos2(0.0, 0.0));
    let to_fog = g.add(NodeKind::ConvertVdb { mode: 1, iso: 0.0 }, egui::pos2(0.0, 0.0));
    g.connect(sphere, vdb, 0);
    g.connect(vdb, to_polygons, 0);
    g.connect(vdb, to_fog, 0);
    let level_set = g.cook(vdb, 0.0, &mut HashMap::new());
    assert!(volume::is_volume(&level_set) && level_set.point_attributes.contains_key("surface"));
    let surface = g.cook(to_polygons, 0.0, &mut HashMap::new());
    assert!(!surface.indices.is_empty());
    assert!(surface.vertices.iter().all(|v| (v.length() - 1.0).abs() < 0.12), "surface should match the sphere");
    let fog = g.cook(to_fog, 0.0, &mut HashMap::new());
    assert!(fog.point_attributes["density"].iter().any(|d| *d > 0.99));
    // Convert VDB on plain polygons reports an error instead of guessing.
    let bad = g.add(NodeKind::ConvertVdb { mode: 0, iso: 0.0 }, egui::pos2(0.0, 0.0));
    g.connect(sphere, bad, 0);
    assert!(g.cook(bad, 0.0, &mut HashMap::new()).vertices.is_empty());
    assert!(node_errors(|e| e.get(&bad).cloned()).is_some_and(|e| e.contains("not a VDB")));
}

#[test]
fn file_nodes_write_and_read_every_format() {
    let mut g = Graph::default();
    let source = g.add(NodeKind::Torus { major: 1.0, minor: 0.3, rows: 8, cols: 16 }, egui::pos2(0.0, 0.0));
    let original = g.cook(source, 0.0, &mut HashMap::new());
    let dir = std::env::temp_dir();
    let mut formats = vec!["obj", "fbx", "stl", "usda"];
    if usd::NATIVE {
        formats.push("usdc");
    }
    for extension in formats {
        let path = dir.join(format!("pfx_file_node.{extension}")).to_string_lossy().into_owned();
        write_geometry_file(&original, &path).unwrap_or_else(|e| panic!("write {extension}: {e}"));
        let reader = g.add(NodeKind::FileImport { file: path.clone(), take: String::new(), looping: false }, egui::pos2(0.0, 0.0));
        let back = g.cook(reader, 0.0, &mut HashMap::new());
        if extension.starts_with("usd") && !usd::NATIVE {
            continue;
        }
        assert_eq!(back.indices.len(), original.indices.len(), "{extension}");
        let (lo, hi) = (back.bounds().unwrap(), original.bounds().unwrap());
        assert!(lo.0.sub(hi.0).length() < 1e-4 && lo.1.sub(hi.1).length() < 1e-4, "{extension} bounds");
        let _ = std::fs::remove_file(&path);
    }
    // $F sequences.
    let pattern = dir.join("pfx_seq.$F4.obj").to_string_lossy().into_owned();
    assert!(fileio::expand_frame(&pattern, 1.0, sims::SIM_FPS).ends_with("pfx_seq.0025.obj"));
    let reader = g.add(NodeKind::FileImport { file: pattern, take: String::new(), looping: false }, egui::pos2(0.0, 0.0));
    assert!(g.time_dependent(reader));
}

#[test]
fn vdb_files_round_trip_through_openvdb() {
    let sphere = node_sphere(1.0, 16, 32);
    let level_set = volume::to_mesh(&volume::from_polygons(&sphere, 0.1, 3.0).unwrap());
    let path = std::env::temp_dir().join("pfx_round_trip.vdb").to_string_lossy().into_owned();
    let written = write_geometry_file(&level_set, &path);
    if !cfg!(feature = "native-openvdb") {
        assert!(written.is_err());
        return;
    }
    written.unwrap();
    let back = read_geometry_file(&path, 0.0).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.vertices.len(), level_set.vertices.len());
    let surface = volume::to_polygons(&volume::from_mesh(&back).unwrap(), 0.0);
    assert!(surface.vertices.iter().all(|v| (v.length() - 1.0).abs() < 0.12));
}


#[test]
fn object_transforms_apply_and_persist() {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 2 }, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&grid).unwrap().parent = Some(geo);
    g.set_display(grid);
    g.nodes.get_mut(&geo).unwrap().xform = ObjectXform { t: [0.0, 3.0, 0.0], r: [0.0; 3], s: [2.0; 3] };
    let (lo, hi) = g.cook(geo, 0.0, &mut HashMap::new()).bounds().unwrap();
    assert!((lo.y - 3.0).abs() < 1e-5 && (hi.x - 2.0).abs() < 1e-5, "{lo:?} {hi:?}");
    // The display inside the network is shown where the object is.
    assert_eq!(g.cook_display(&[grid], 0.0, &mut HashMap::new()).bounds(), Some((lo, hi)));
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(loaded.nodes[&geo].xform, g.nodes[&geo].xform);
    assert!(loaded.nodes[&grid].xform.is_identity());
}

#[test]
fn gizmo_drags_edit_the_selected_object() {
    let mut app = ProceduralFxStudioApp::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    app.selected = Some(geo);
    app.context = AppContext::Obj;
    let frame = GizmoFrame {
        node: geo,
        pivot: egui::pos2(100.0, 100.0),
        pixels_per_unit: 100.0,
        cam_right: Vec3::new(1.0, 0.0, 0.0),
        cam_up: Vec3::new(0.0, 1.0, 0.0),
        handle_px: 85.0,
        parts: Vec::new(),
    };
    let axis = GizmoPart { handle: GizmoHandle::Axis(0), points: vec![], axis_screen: egui::vec2(100.0, 0.0), ring_sign: 1.0 };
    let target = app.gizmo_target().unwrap();
    let drag = GizmoDrag { part: axis.clone(), frame: frame.clone(), start_pointer: egui::pos2(100.0, 100.0), target };
    app.gizmo_mode = GizmoMode::Translate;
    app.apply_gizmo_drag(&drag, egui::pos2(150.0, 130.0));
    assert!((app.graph.nodes[&geo].xform.t[0] - 0.5).abs() < 1e-5);
    app.gizmo_mode = GizmoMode::Scale;
    app.apply_gizmo_drag(&drag, egui::pos2(185.0, 100.0));
    assert!((app.graph.nodes[&geo].xform.s[0] - 2.0).abs() < 1e-4);
    let ring = GizmoPart { handle: GizmoHandle::Ring(1), points: vec![], axis_screen: egui::Vec2::ZERO, ring_sign: 1.0 };
    let drag = GizmoDrag { part: ring, frame, start_pointer: egui::pos2(200.0, 100.0), target };
    app.gizmo_mode = GizmoMode::Rotate;
    app.apply_gizmo_drag(&drag, egui::pos2(100.0, 200.0));
    assert!((app.graph.nodes[&geo].xform.r[1] - 90.0).abs() < 1e-3);
}


#[test]
fn level_set_vdbs_display_as_surfaces_and_fog_volumes_render() {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let sphere = g.add(NodeKind::Sphere { radius: 1.0, rows: 16, cols: 32 }, egui::pos2(0.0, 0.0));
    let vdb = g.add(NodeKind::Vdb { voxel_size: 0.1, padding: 0, operation: 0, iterations: 1, output: 1 }, egui::pos2(0.0, 0.0));
    for n in [sphere, vdb] {
        g.nodes.get_mut(&n).unwrap().parent = Some(geo);
    }
    g.connect(sphere, vdb, 0);
    g.set_display(vdb);
    let shown = g.cook_display(&[vdb], 0.0, &mut HashMap::new());
    assert!(!shown.indices.is_empty(), "a level set is displayed as its surface");
    assert!(!shown.point_attributes.contains_key("Alpha"));
    // Fog volumes go to MoonRay as .vdb files.
    let fog = g.add(NodeKind::ConvertVdb { mode: 1, iso: 0.0 }, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&fog).unwrap().parent = Some(geo);
    g.connect(vdb, fog, 0);
    g.set_display(fog);
    let mut app = ProceduralFxStudioApp::default();
    app.graph = g;
    app.current_network = None;
    app.render_opts = RenderOpts::default();
    // Cook for an explicit time rather than reusing the viewport mesh.
    app.render_at = Some(0.0);
    let scene = app.render_scene();
    if cfg!(feature = "native-openvdb") {
        let scene = scene.unwrap();
        assert_eq!((scene.volumes.len(), scene.parts.len()), (1, 0));
        assert!(std::path::Path::new(&scene.volumes[0].file).exists());
        let _ = std::fs::remove_file(&scene.volumes[0].file);
    } else {
        assert!(scene.is_err());
    }
}


#[test]
fn content_signature_ignores_layout_but_sees_edits() {
    let (mut g, solver) = solver_scene();
    let before = g.content_signature(None);
    g.nodes.get_mut(&solver).unwrap().pos += egui::vec2(50.0, 10.0);
    assert_eq!(g.content_signature(None), before, "moving a node in the editor is not an edit");
    g.nodes.get_mut(&solver).unwrap().bypass = true;
    assert_ne!(g.content_signature(None), before);
    g.nodes.get_mut(&solver).unwrap().bypass = false;
    g.nodes.get_mut(&solver).unwrap().xform.t[1] = 2.0;
    assert_ne!(g.content_signature(None), before);
    assert_eq!(g.content_signature(Some(solver)), solver_scene().0.content_signature(Some(solver)), "the skipped node is ignored");
}


fn animated_box() -> (Graph, NodeId, NodeId) {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let shape = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    g.nodes.get_mut(&shape).unwrap().parent = Some(geo);
    g.set_display(shape);
    let ty = ChannelId { node: geo, target: ChannelTarget::Xform(1) };
    let mut channel = anim::Channel::constant(0.0, 0.0);
    channel.set_key(1.0, 4.0);
    channel.keys.iter_mut().for_each(|k| k.interp = anim::Interp::Linear);
    g.channels.insert(ty, channel);
    let light = g.add(NodeKind::Light { position: [0.0; 3], rotation: [0.0; 3], color: [1.0; 3], intensity: 1.0, kind: 0 }, egui::pos2(0.0, 0.0));
    let mut fade = anim::Channel::constant(0.0, 1.0);
    fade.set_key(2.0, 0.0);
    g.channels.insert(ChannelId { node: light, target: ChannelTarget::Param(9) }, fade);
    (g, geo, light)
}

#[test]
fn keyframed_objects_animate_in_cooks() {
    let (g, geo, light) = animated_box();
    let centre_y = |t: f32| {
        let (lo, hi) = g.cook(geo, t, &mut HashMap::new()).bounds().unwrap();
        (lo.y + hi.y) * 0.5
    };
    assert!((centre_y(0.0) - 0.0).abs() < 1e-5);
    assert!((centre_y(0.5) - 2.0).abs() < 1e-5);
    assert!((centre_y(3.0) - 4.0).abs() < 1e-5, "held after the last key");
    assert!(g.time_dependent(geo));
    assert!(matches!(g.kind_at(light, 2.0).as_ref(), NodeKind::Light { intensity, .. } if intensity.abs() < 1e-5));
}

#[test]
fn playback_is_not_an_edit_but_changes_are_keyed() {
    let (mut g, geo, light) = animated_box();
    let signature = g.content_signature(None);
    let saved = g.serialize();
    g.apply_animation(0.75);
    assert_eq!(g.content_signature(None), signature, "evaluating animation is not an edit");
    assert_eq!(g.serialize(), saved, "the saved scene does not depend on the current time");
    assert!((g.nodes[&geo].xform.t[1] - 3.0).abs() < 1e-5, "panels and gizmos see the animated value");
    let height = ChannelId { node: geo, target: ChannelTarget::Xform(1) };
    // With Auto Key off, an edit of an animated parameter is pending: no key, but it
    // holds on this frame (playback re-applying the animation doesn't undo it)...
    g.nodes.get_mut(&geo).unwrap().xform.t[1] = 7.0;
    assert!(g.commit_animation_edits(0.75, false));
    assert!(!g.channels[&height].has_key_at(0.75));
    g.apply_animation(0.75);
    assert_eq!(g.nodes[&geo].xform.t[1], 7.0);
    assert_eq!(g.xform_at(geo, 0.75).t[1], 7.0, "what's cooked on this frame uses it");
    assert!(!g.commit_animation_edits(0.75, false), "nothing new");
    // ...until another frame, which goes back to the animation.
    g.apply_animation(0.5);
    g.apply_animation(0.75);
    assert!((g.nodes[&geo].xform.t[1] - 3.0).abs() < 1e-5, "the pending edit is dropped");
    // Keying (K) a pending edit makes it a key.
    g.nodes.get_mut(&geo).unwrap().xform.t[1] = 8.0;
    g.commit_animation_edits(0.75, false);
    g.set_key(height, 0.75);
    assert!(g.pending.is_empty() && (g.channels[&height].eval(0.75) - 8.0).abs() < 1e-5);
    // With Auto Key, an edit of an animated parameter keys the current frame.
    g.nodes.get_mut(&geo).unwrap().xform.t[1] = 10.0;
    assert!(g.commit_animation_edits(0.75, true));
    let channel = &g.channels[&height];
    assert!(channel.has_key_at(0.75) && (channel.eval(0.75) - 10.0).abs() < 1e-5);
    assert!(!g.commit_animation_edits(0.75, true), "nothing more to key");
    // Channels survive saving, loading and duplication.
    let loaded = Graph::deserialize(&g.serialize());
    assert_eq!(loaded.channels, g.channels);
    let copy = g.duplicate(light).unwrap();
    assert!(g.is_animated(copy));
    g.remove(light);
    assert!(!g.channels.keys().any(|c| c.node == light));
}


#[test]
fn buckets_cover_the_frame_once_from_the_centre() {
    for (w, h) in [(1280, 720), (333, 97), (64, 64)] {
        let buckets = bucket_layout(w, h);
        let mut covered = vec![0u8; w * h];
        for [x0, y0, x1, y1] in &buckets {
            for y in *y0..*y1 {
                for x in *x0..*x1 {
                    covered[y * w + x] += 1;
                }
            }
        }
        assert!(covered.iter().all(|c| *c == 1), "{w}x{h}");
        let first = buckets[0];
        assert!(first[0] <= w / 2 && w / 2 <= first[2] + 1 && first[1] <= h / 2 && h / 2 <= first[3] + 1, "starts in the centre");
    }
    assert_eq!(bucket_layout(1280, 720).len(), 24);
}
