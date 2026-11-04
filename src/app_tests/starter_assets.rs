// The assets that come with the app (assets/library), built here from nodes so they can be
// rebuilt when nodes change: PFX_WRITE_ASSETS=1 cargo test --release -- --ignored write_starter_assets
use crate::*;

struct Builder {
    app: ProceduralFxStudioApp,
    subnet: NodeId,
    out: NodeId,
}

impl Builder {
    // A Subnet inside a geometry object, entered, with Input_1 wired to OUT.
    fn new() -> Builder {
        let mut app = ProceduralFxStudioApp::default();
        app.graph = Graph::default();
        let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        app.current_network = Some(geo);
        app.apply(Action::Add(NodeKind::Subnet { values: Vec::new(), spec: assets::Spec::default() }, egui::pos2(0.0, 0.0)));
        let subnet = app.selected.unwrap();
        app.current_network = Some(subnet);
        let out = app.graph.display_of(subnet).unwrap();
        Builder { app, subnet, out }
    }
    fn input(&self) -> NodeId {
        self.app.graph.children(self.subnet).into_iter().find(|c| matches!(self.app.graph.nodes[c].kind, NodeKind::SubnetInput { .. })).unwrap()
    }
    // OUT keeps the display flag (adding a simulation would take it).
    fn out(&self) -> NodeId {
        self.out
    }
    // A node in the network being built (its own default network, if it has one).
    fn add(&mut self, kind: NodeKind, at: (f32, f32)) -> NodeId {
        self.app.apply(Action::Add(kind, egui::pos2(at.0, at.1)));
        self.app.selected.unwrap()
    }
    fn index(&self, node: NodeId, name: &str) -> usize {
        let names = self.app.graph.nodes[&node].kind.param_names();
        names.iter().position(|n| n == name).unwrap_or_else(|| panic!("no parameter {name} in {names:?}"))
    }
    fn set(&mut self, node: NodeId, name: &str, value: f32) {
        let index = self.index(node, name);
        self.app.graph.set_param(node, index, value);
    }
    fn promote(&mut self, label: &str, node: NodeId, name: &str, min: f32, max: f32) {
        let index = self.index(node, name);
        let value = self.app.graph.nodes[&node].kind.params()[index];
        if let Some(NodeKind::Subnet { values, spec }) = self.app.graph.nodes.get_mut(&self.subnet).map(|n| &mut n.kind) {
            spec.params.push(assets::Promoted { label: label.into(), node, index, min, max });
            values.push(value);
        }
    }
    // Puts `chain` between Input_1 and OUT.
    fn wire(&mut self, chain: &[NodeId]) {
        let (input, out) = (self.input(), self.out());
        self.app.graph.disconnect(out, 0);
        let mut from = input;
        for &n in chain {
            self.app.graph.connect(from, n, 0);
            from = n;
        }
        self.app.graph.connect(from, out, 0);
    }
    fn pop(&self, net: NodeId, op: usize) -> NodeId {
        self.app.graph.children(net).into_iter().find(|c| matches!(&self.app.graph.nodes[c].kind, NodeKind::Pop(p) if p.op == op)).unwrap()
    }
    fn text(&mut self, name: &str, description: &str) -> String {
        self.app.graph.displays.insert(self.subnet, self.out);
        self.app.graph.asset_text(self.subnet, name, 1, description)
    }
}

// A POP Network fed by Input_1, with drag and a colour ramp after gravity: sparks.
fn sparks(burst: bool) -> String {
    let mut b = Builder::new();
    let net = b.add(NodeKind::PopNet { substeps: 1 }, (0.0, 80.0));
    b.wire(&[net]);
    b.app.current_network = Some(net);
    let (source, gravity) = (b.pop(net, pop::SOURCE), b.pop(net, pop::GRAVITY));
    let drag = b.add(NodeKind::Pop(pop::PopNode::new(pop::DRAG)), (0.0, 320.0));
    let color = b.add(NodeKind::Pop(pop::PopNode::new(pop::COLOR)), (0.0, 400.0));
    b.app.graph.connect(gravity, drag, 0);
    b.app.graph.connect(drag, color, 0);
    b.app.graph.displays.insert(net, color);
    // From any points of what's wired in (a single point works too).
    b.set(source, "emission", 1.0);
    b.set(source, "life", if burst { 1.0 } else { 1.2 });
    b.set(source, "life_variance", 0.4);
    b.set(source, "velocity.y", if burst { 2.0 } else { 4.0 });
    for (axis, spread) in [("variance.x", 1.5), ("variance.y", if burst { 2.5 } else { 0.8 }), ("variance.z", 1.5)] {
        b.set(source, axis, if burst { 3.0 } else { spread });
    }
    b.set(drag, "air_resistance", 0.6);
    b.set(color, "mode", 2.0);
    for (c, v) in [("color.r", 1.0), ("color.g", 0.85), ("color.b", 0.3), ("color2.r", 0.9), ("color2.g", 0.15), ("color2.b", 0.02)] {
        b.set(color, c, v);
    }
    if burst {
        // Everything on the first frame, nothing after.
        b.set(source, "birth_rate", 0.0);
        let impulse = b.index(source, "impulse");
        b.app.graph.channels.entry(ChannelId { node: source, target: ChannelTarget::Param(impulse) }).or_default().expression = Some("$F < 2.5 ? 400 : 0".into());
    } else {
        b.set(source, "birth_rate", 300.0);
        b.promote("Birth rate", source, "birth_rate", 0.0, 2000.0);
    }
    b.promote("Speed", source, "velocity.y", 0.0, 20.0);
    b.promote("Spread", source, "variance.x", 0.0, 10.0);
    b.promote("Life", source, "life", 0.1, 5.0);
    b.promote("Gravity", gravity, "force.y", -30.0, 0.0);
    b.promote("Drag", drag, "air_resistance", 0.0, 5.0);
    b.app.current_network = Some(b.subnet);
    if burst {
        b.text("Spark Burst", "A burst of 400 sparks on the first frame, from the points of what's wired in (a single point works), arcing down and fading from yellow to red. Copy to Points or a sprite sheet next.")
    } else {
        b.text("Sparks", "A steady fountain of sparks from the points of what's wired in (a single point works), arcing down and fading from yellow to red.")
    }
}

// Pyro smoke rising from what's wired in.
fn smoke_puff() -> String {
    let mut b = Builder::new();
    let pyro = NodeKind::templates().into_iter().flat_map(|(_, k)| k).find(|k| k.name() == "pyro").unwrap();
    let pyro = b.add(pyro, (0.0, 80.0));
    b.wire(&[pyro]);
    b.set(pyro, "density", 1.5);
    b.set(pyro, "dissipation", 0.4);
    b.set(pyro, "turbulence", 0.6);
    b.promote("Density", pyro, "density", 0.0, 5.0);
    b.promote("Rise", pyro, "buoyancy", 0.0, 5.0);
    b.promote("Fade", pyro, "dissipation", 0.0, 3.0);
    b.promote("Turbulence", pyro, "turbulence", 0.0, 3.0);
    b.promote("Detail", pyro, "resolution", 16.0, 96.0);
    b.text("Smoke Puff", "Smoke rising from the shape wired in (a small sphere works well), in a box 4 wide and 5 tall around the origin: set the Pyro inside to move it.")
}

// Small boxes of random sizes scattered over what's wired in.
fn debris() -> String {
    let mut b = Builder::new();
    let kind = |name: &str| NodeKind::templates().into_iter().flat_map(|(_, k)| k).find(|k| k.name() == name).unwrap();
    let scatter = b.add(kind("scatter"), (0.0, 80.0));
    let random = b.add(kind("attribute_randomize"), (0.0, 160.0));
    let piece = b.add(NodeKind::Box { size: 0.08, div: 1 }, (200.0, 160.0));
    let copy = b.add(kind("copy_to_points"), (0.0, 240.0));
    b.wire(&[scatter, random]);
    let out = b.out();
    b.app.graph.disconnect(out, 0);
    b.app.graph.connect(piece, copy, 0);
    b.app.graph.connect(random, copy, 1);
    b.app.graph.connect(copy, out, 0);
    b.set(scatter, "count", 60.0);
    b.set(scatter, "relax_iterations", 2.0);
    if let Some(NodeKind::AttributeRandomize(r)) = b.app.graph.nodes.get_mut(&random).map(|n| &mut n.kind) {
        r.name = "pscale".into();
    }
    b.set(random, "size", 1.0);
    b.set(random, "min.x", 0.4);
    b.set(random, "max.x", 1.6);
    b.promote("Count", scatter, "count", 1.0, 1000.0);
    b.promote("Size", piece, "size", 0.01, 1.0);
    b.promote("Seed", scatter, "seed", 1.0, 100.0);
    b.text("Debris", "Small boxes of random sizes scattered over the surface wired in: rubble, rocks or confetti.")
}

fn all() -> Vec<(&'static str, String)> {
    vec![("sparks", sparks(false)), ("spark_burst", sparks(true)), ("smoke_puff", smoke_puff()), ("debris", debris())]
}

#[test]
#[ignore]
fn write_starter_assets() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/library");
    for (file, text) in all() {
        if std::env::var_os("PFX_WRITE_ASSETS").is_some() {
            std::fs::write(dir.join(format!("{file}.pfxasset")), &text).unwrap();
        }
        eprintln!("{file}: {} bytes", text.len());
    }
}

// Every built-in asset is listed, places from the TAB menu and makes geometry when fed
// a sphere, at the start and a second in.
#[test]
fn the_built_in_assets_place_and_cook() {
    let library = assets::library(None);
    assert_eq!(library.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["Debris", "Smoke Puff", "Spark Burst", "Sparks"]);
    for entry in &library {
        assert!(!entry.description.is_empty(), "{}", entry.name);
        let mut app = ProceduralFxStudioApp::default();
        app.graph = Graph::default();
        let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
        let sphere = app.graph.add(NodeKind::Sphere { radius: 0.3, rows: 8, cols: 12 }, egui::pos2(0.0, 0.0));
        app.graph.nodes.get_mut(&sphere).unwrap().parent = Some(geo);
        app.current_network = Some(geo);
        let kind = app.templates_for_view().into_iter().find(|(c, _)| *c == "Assets").unwrap().1.into_iter().find(|k| k.menu_name() == entry.name).unwrap();
        app.apply(Action::Add(kind, egui::pos2(0.0, 100.0)));
        let placed = app.selected.unwrap();
        assert!(app.status.starts_with("Placed"), "{}: {}", entry.name, app.status);
        app.graph.connect(sphere, placed, 0);
        let NodeKind::Subnet { spec, values } = &app.graph.nodes[&placed].kind else { panic!() };
        assert!(!spec.params.is_empty() && spec.params.len() == values.len(), "{} has sliders", entry.name);
        for p in &spec.params {
            assert_eq!(app.graph.nodes[&p.node].parent.map(|n| app.graph.subnets_around(p.node).contains(&placed) || n == placed), Some(true), "{}: {} drives a node inside", entry.name, p.label);
        }
        assert_eq!(app.graph.display_of(placed).map(|d| app.graph.nodes[&d].title(d)), Some("OUT".to_owned()), "{} shows OUT", entry.name);
        // Simulations have nothing at the start; everything has something a second in.
        for time in [0.0, 1.0] {
            let mut mesh = app.graph.cook(placed, time, &mut HashMap::new());
            // Simulations cook in the background; wait for them.
            for _ in 0..if time > 0.0 || entry.name == "Debris" { 2000 } else { 0 } {
                if !mesh.vertices.is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
                mesh = app.graph.cook(placed, time, &mut HashMap::new());
            }
            if time > 0.0 || entry.name == "Debris" {
                assert!(!mesh.vertices.is_empty(), "{} at {time}s makes something", entry.name);
            }
        }
    }
}
