use crate::*;

// Writes the 3D starter presets (assets/presets/flag.pfs, breaking_wall.pfs, splash.pfs):
//     PFX_WRITE_PRESETS=1 cargo test --release -- --ignored write_3d_presets
// src/presets.rs lists them with their playback range.
#[test]
#[ignore]
fn write_3d_presets() {
    if std::env::var_os("PFX_WRITE_PRESETS").is_none() {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/presets");
    for (file, text) in [("flag.pfs", flag()), ("breaking_wall.pfs", breaking_wall()), ("splash.pfs", splash())] {
        std::fs::write(dir.join(file), text).unwrap();
    }
}

fn kind(name: &str) -> NodeKind {
    NodeKind::templates().into_iter().flat_map(|(_, k)| k).find(|k| k.name() == name).unwrap_or_else(|| panic!("{name}"))
}

fn set(g: &mut Graph, node: NodeId, name: &str, value: f32) {
    let names = g.nodes[&node].kind.param_names();
    let index = names.iter().position(|n| n == name).unwrap_or_else(|| panic!("no parameter {name} in {names:?}"));
    g.set_param(node, index, value);
}

// A geometry object named `name` holding `chain` wired in order, the last displayed,
// and a camera looking down at `centre` from the front left, `span` units across.
fn scene(mut g: Graph, geo: NodeId, name: &str, chain: &[NodeId], centre: [f32; 3], span: f32) -> String {
    for pair in chain.windows(2) {
        g.connect(pair[0], pair[1], 0);
    }
    for (i, &n) in chain.iter().enumerate() {
        let node = g.nodes.get_mut(&n).unwrap();
        node.parent = Some(geo);
        node.pos = egui::pos2(40.0, 20.0 + 80.0 * i as f32);
    }
    g.displays.insert(geo, *chain.last().unwrap());
    g.nodes.get_mut(&geo).unwrap().name = name.into();
    let camera = g.add(NodeKind::Camera { position: centre, rotation: [20.0, 30.0, 0.0], focal: 1.2, near: 0.05, orthographic: false, pan: [0.0, 0.0], zoom: span * 2.2 }, egui::pos2(40.0, -80.0));
    g.nodes.get_mut(&camera).unwrap().name = "sheet_camera".into();
    g.serialize()
}

// A flag pinned along its left edge, waving in a loop that repeats every second.
pub fn flag() -> String {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(40.0, 20.0));
    let grid = g.add(NodeKind::Grid { size: 2.0, res: 24 }, egui::pos2(0.0, 0.0));
    // Upright, 2 wide and 1.3 high, its bottom edge a little off the ground.
    let stand = g.add(NodeKind::Transform { t: [0.0, 1.2, 0.0], r: [90.0, 0.0, 0.0], s: [1.0, 1.0, 0.65], p: [0.0; 3] }, egui::pos2(0.0, 0.0));
    let code = "// Waves run along the flag from the pole at x = -1, growing towards the free edge.\n\
                    // Whole waves per period, so it loops exactly.\n\
                    float t = @Time / ch(\"period\");\n\
                    float along = (@P.x + 1.0) / 2.0;\n\
                    float phase = @P.x * ch(\"waves\") * 3.14159 - t * 6.28318;\n\
                    @P.z += sin(phase) * ch(\"amplitude\") * along;\n\
                    @P.y += (sin(phase + 1.0) * 0.04 - 0.08 * along * along) * along;\n";
    let wave = g.add(NodeKind::Wrangle { code: code.into(), domain: 2 }, egui::pos2(0.0, 0.0));
    let node = g.nodes.get_mut(&wave).unwrap();
    node.name = "wave".into();
    for (name, value) in [("period", 1.0), ("waves", 1.5), ("amplitude", 0.22)] {
        node.channels.insert(name.into(), [value, 0.0, 0.0]);
    }
    scene(g, geo, "flag", &[grid, stand, wave], [0.0, 1.2, 0.0], 2.2)
}

// A stone wall struck in the middle: it breaks into pieces that fly back and tumble down.
pub fn breaking_wall() -> String {
    let mut g = Graph::default();
    let geo = g.add(NodeKind::Geometry { material: None }, egui::pos2(40.0, 20.0));
    let block = g.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    let shape = g.add(NodeKind::Transform { t: [0.0, 1.0, 0.0], r: [0.0; 3], s: [3.0, 2.0, 0.3], p: [0.0; 3] }, egui::pos2(0.0, 0.0));
    let fracture = g.add(kind("material_fracture"), egui::pos2(0.0, 0.0));
    set(&mut g, fracture, "pieces", 24.0);
    let code = "// The blow lands on the wall's middle from the front: pieces near it are\n\
                    // thrown back and out from there, the rest stay put. (A piece takes its\n\
                    // points' average v, so the push back is the same for all its points.)\n\
                    vector hit = set(0.0, 1.1, 0.0);\n\
                    vector out = set(@P.x - hit.x, @P.y - hit.y, 0.0);\n\
                    float near = exp(-dot(out, out) / ch(\"radius\"));\n\
                    @v = (out * 1.5 + set(0.0, 0.8, -2.0)) * ch(\"force\") * near;\n";
    let blast = g.add(NodeKind::Wrangle { code: code.into(), domain: 2 }, egui::pos2(0.0, 0.0));
    let node = g.nodes.get_mut(&blast).unwrap();
    node.name = "blast".into();
    for (name, value) in [("force", 2.5), ("radius", 0.8)] {
        node.channels.insert(name.into(), [value, 0.0, 0.0]);
    }
    let rbd = g.add(kind("rbd_solver"), egui::pos2(0.0, 0.0));
    scene(g, geo, "wall", &[block, shape, fracture, blast, rbd], [0.0, 1.2, 0.0], 3.6)
}

// A splash: a burst of drops thrown up and out, falling back onto the ground, meshed
// into liquid by Particle Fluid Surface (coarse, so its vertex animation stays small).
pub fn splash() -> String {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(40.0, 20.0));
    let seed = app.graph.add(NodeKind::Sphere { radius: 0.15, rows: 6, cols: 8 }, egui::pos2(0.0, 0.0));
    let lift = app.graph.add(NodeKind::Transform { t: [0.0, 0.2, 0.0], r: [0.0; 3], s: [1.0; 3], p: [0.0; 3] }, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&lift).unwrap().parent = Some(geo);
    app.graph.connect(seed, lift, 0);
    app.graph.nodes.get_mut(&seed).unwrap().parent = Some(geo);
    app.current_network = Some(geo);
    app.apply(Action::Add(NodeKind::PopNet { substeps: 2 }, egui::pos2(0.0, 80.0)));
    let net = app.selected.unwrap();
    app.graph.connect(lift, net, 0);
    let pop = |app: &ProceduralFxStudioApp, op: usize| app.graph.children(net).into_iter().find(|c| matches!(&app.graph.nodes[c].kind, NodeKind::Pop(p) if p.op == op)).unwrap();
    let source = pop(&app, pop::SOURCE);
    let set_pop = |app: &mut ProceduralFxStudioApp, node: NodeId, name: &str, value: f32| {
        let names = app.graph.nodes[&node].kind.param_names();
        let index = names.iter().position(|n| n == name).unwrap_or_else(|| panic!("no parameter {name} in {names:?}"));
        app.graph.set_param(node, index, value);
    };
    // All of it in the first frames, from the seed's points.
    set_pop(&mut app, source, "emission", 1.0);
    set_pop(&mut app, source, "birth_rate", 0.0);
    set_pop(&mut app, source, "life", 3.0);
    set_pop(&mut app, source, "life_variance", 0.0);
    set_pop(&mut app, source, "velocity.y", 3.5);
    for (axis, spread) in [("variance.x", 1.0), ("variance.y", 0.8), ("variance.z", 1.0)] {
        set_pop(&mut app, source, axis, spread);
    }
    let names = app.graph.nodes[&source].kind.param_names();
    let impulse = names.iter().position(|n| n == "impulse").unwrap();
    app.graph.channels.entry(ChannelId { node: source, target: ChannelTarget::Param(impulse) }).or_default().expression = Some("$F < 3.5 ? 120 : 0".into());
    // A floor the drops land on and roll over.
    let gravity = pop(&app, pop::GRAVITY);
    let collide = app.graph.add(NodeKind::Pop(pop::PopNode::new(pop::COLLIDE)), egui::pos2(0.0, 320.0));
    let node = app.graph.nodes.get_mut(&collide).unwrap();
    node.context = AppContext::Obj;
    node.parent = Some(net);
    app.graph.connect(gravity, collide, 0);
    app.graph.displays.insert(net, collide);
    let mut g = app.graph;
    let surface = g.add(kind("particle_surface"), egui::pos2(0.0, 0.0));
    set(&mut g, surface, "particle_radius", 0.13);
    set(&mut g, surface, "resolution", 20.0);
    set(&mut g, surface, "smoothing", 2.0);
    g.connect(net, surface, 0);
    g.nodes.get_mut(&net).unwrap().pos = egui::pos2(40.0, 100.0);
    scene(g, geo, "splash", &[seed, lift, net, surface], [0.0, 0.8, 0.0], 2.4)
}

#[test]
#[ignore]
fn flag_moves() {
    let g = Graph::deserialize(&flag());
    let geo = g.geometry_ids()[0];
    let wave = g.nodes.iter().find(|(_, n)| n.name == "wave").map(|(id, _)| *id).unwrap();
    for t in [0.0, 0.25, 0.5] {
        let m = g.cook(geo, t, &mut HashMap::new());
        let zs: Vec<f32> = m.vertices.iter().map(|v| v.z).collect();
        eprintln!("t {t}: z range {:?} {:?} errors {:?}", zs.iter().cloned().fold(f32::MAX, f32::min), zs.iter().cloned().fold(f32::MIN, f32::max), node_errors(|e| e.get(&wave).cloned()));
    }
}

// How big each preset's vertex animation is, and how long baking takes.
#[test]
#[ignore]
fn preset_bake_sizes() {
    for (name, text, range, frames) in [("flag", flag(), 1.0, 24), ("wall", breaking_wall(), 2.0, 48), ("splash", splash(), 1.0, 24)] {
        let g = Graph::deserialize(&text);
        let geo = g.geometry_ids()[0];
        let start = std::time::Instant::now();
        let times: Vec<f32> = (0..frames).map(|i| i as f32 * range / frames as f32).collect();
        let bake = vat::bake_object(&g, geo, &times, None, &mut |_, _| {}).unwrap();
        let layout = vat::Layout::new(bake.elements, bake.frames).unwrap();
        eprintln!("{name}: {} as {}, {} elements, texture {}x{}, {:.1}s", bake.mesh.vertices.len(), bake.mode.name(), bake.elements, layout.width, layout.height(), start.elapsed().as_secs_f32());
    }
}
