use crate::*;

fn inside(app: &mut ProceduralFxStudioApp, net: NodeId, context: AppContext, kind: NodeKind) -> NodeId {
    let id = app.graph.add(kind, egui::pos2(0.0, 0.0));
    let n = app.graph.nodes.get_mut(&id).unwrap();
    n.context = context;
    n.parent = Some(net);
    id
}

fn cop(label: &str) -> NodeKind {
    NodeKind::Cop(cops::CopNode::new(cops::OPS.iter().position(|o| o.label == label).unwrap()))
}

fn top(op: usize) -> tops::TopNode {
    tops::TopNode::new(op)
}

#[test]
fn cop_networks_composite_and_show_in_img() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    app.context = AppContext::Img;
    // Only the COP Network at the top of IMG.
    let top_menu: Vec<NodeKind> = app.templates_for_view().into_iter().flat_map(|(_, k)| k).collect();
    assert!(matches!(top_menu.as_slice(), [NodeKind::CopNetwork]));
    app.apply(Action::Add(NodeKind::CopNetwork, egui::pos2(0.0, 0.0)));
    let net = app.selected.unwrap();
    assert_eq!(app.graph.nodes[&net].context, AppContext::Img);
    app.apply(Action::Enter(net));
    assert_eq!(app.context, AppContext::Img);
    let menu: Vec<NodeKind> = app.templates_for_view().into_iter().flat_map(|(_, k)| k).collect();
    assert_eq!(menu.len(), cops::OPS.len());
    assert!(menu.iter().all(|k| matches!(k, NodeKind::Cop(_))));
    // Constant + Ramp, composited and blurred.
    let mut fg = cops::CopNode::new(2);
    fg.values = vec![1.0, 0.0, 0.0, 0.5, 64.0, 32.0];
    let a = inside(&mut app, net, AppContext::Img, NodeKind::Cop(fg));
    let b = inside(&mut app, net, AppContext::Img, cop("Ramp"));
    let comp = inside(&mut app, net, AppContext::Img, cop("Composite"));
    let blur = inside(&mut app, net, AppContext::Img, cop("Blur"));
    app.graph.connect(a, comp, 0);
    app.graph.connect(b, comp, 1);
    app.graph.connect(comp, blur, 0);
    app.graph.set_display(blur);
    let out = cops::Image::from_mesh(&app.graph.cook(net, 0.0, &mut HashMap::new())).expect("an image");
    assert_eq!((out.w, out.h), (64, 32));
    assert!(node_errors(|e| e.get(&blur).cloned()).is_none());
    assert_eq!(app.view_displays(), vec![blur]);
    // Missing input is an error; OBJ menus have no COPs or TOPs.
    let lonely = inside(&mut app, net, AppContext::Img, cop("Invert"));
    app.graph.cook(lonely, 0.0, &mut HashMap::new());
    assert!(node_errors(|e| e.get(&lonely).cloned()).unwrap().contains("input 1"));
    app.enter_network(None);
    app.context = AppContext::Obj;
    let obj: Vec<NodeKind> = app.templates_for_view().into_iter().flat_map(|(_, k)| k).collect();
    assert!(obj.iter().all(|k| !matches!(k, NodeKind::Cop(_) | NodeKind::Top(_) | NodeKind::CopNetwork | NodeKind::TopNetwork)));
    // Saved with contexts.
    let loaded = Graph::deserialize(&app.graph.serialize());
    assert_eq!(loaded.nodes[&blur].context, AppContext::Img);
    assert_eq!(format!("{:?}", loaded.nodes[&a].kind), format!("{:?}", app.graph.nodes[&a].kind));
}

#[test]
fn fetch_writes_a_cache_that_reads_back() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    // A grid that rises with time, cached over frames 1-3 (cache id 9871).
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let grid = inside(&mut app, geo, AppContext::Obj, NodeKind::Grid { size: 2.0, res: 3 });
    let rise = inside(&mut app, geo, AppContext::Obj, NodeKind::Wrangle { code: "@P.y = @Time;".into(), domain: 2 });
    let cache = inside(&mut app, geo, AppContext::Obj, NodeKind::Cache { cache_id: 9871, mode: 0, range: [1.0, 3.0, 1.0] });
    app.graph.connect(grid, rise, 0);
    app.graph.connect(rise, cache, 0);
    let net = app.graph.add(NodeKind::TopNetwork, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&net).unwrap().context = AppContext::Tasks;
    let mut fetch = top(tops::FETCH);
    fetch.set("node", cache as f32);
    let fetch = inside(&mut app, net, AppContext::Tasks, NodeKind::Top(fetch));
    let items = app.graph.top_items(fetch).unwrap();
    assert_eq!(items.len(), 3, "the cache's own frame range");
    for item in &items {
        let _ = std::fs::remove_file(&item.output);
    }
    app.start_top_cook(fetch);
    for _ in 0..500 {
        if app.top_run.is_none() {
            break;
        }
        app.advance_top_run();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.top_results[&fetch].iter().all(|s| s.0 == tops::State::Done), "{:?}", app.top_results[&fetch]);
    // Each frame's file has the risen grid, polygons included.
    let frame3 = load_mesh_cache(std::path::Path::new(&items[2].output)).unwrap();
    let t3 = 2.0 / sims::SIM_FPS;
    assert!(frame3.vertices.iter().all(|p| (p.y - t3).abs() < 1e-5));
    assert!(frame3.point_attributes.contains_key(poly::KEY), "polygons survive the cache");
    // Read mode plays the cache back.
    let read = app.graph.cook(cache, t3, &mut HashMap::new());
    assert!(read.vertices.iter().all(|p| (p.y - t3).abs() < 1e-5));
    for item in &items {
        let _ = std::fs::remove_file(&item.output);
    }
}

#[test]
fn top_networks_wedge_and_fetch_exports() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    // Geometry: a box whose size is wedged, exported per work item.
    let geo = app.graph.add(NodeKind::Geometry { material: None }, egui::pos2(0.0, 0.0));
    let cube = inside(&mut app, geo, AppContext::Obj, NodeKind::Box { size: 1.0, div: 1 });
    let export = inside(&mut app, geo, AppContext::Obj, NodeKind::FileExport { file: String::new() });
    app.graph.connect(cube, export, 0);
    app.context = AppContext::Tasks;
    let menu: Vec<NodeKind> = app.templates_for_view().into_iter().flat_map(|(_, k)| k).collect();
    assert!(matches!(menu.as_slice(), [NodeKind::TopNetwork]));
    let net = app.graph.add(NodeKind::TopNetwork, egui::pos2(0.0, 0.0));
    app.graph.nodes.get_mut(&net).unwrap().context = AppContext::Tasks;
    let mut range = top(tops::FRAME_RANGE);
    range.set("start", 1.0);
    range.set("end", 2.0);
    let range = inside(&mut app, net, AppContext::Tasks, NodeKind::Top(range));
    let mut wedge = top(tops::WEDGE);
    wedge.set("count", 2.0);
    wedge.set("start", 1.0);
    wedge.set("end", 3.0);
    wedge.set("node", cube as f32);
    wedge.set("param", 0.0);
    let wedge = inside(&mut app, net, AppContext::Tasks, NodeKind::Top(wedge));
    let dir = std::env::temp_dir().join(format!("pfx_tops_run_{}", std::process::id()));
    let mut fetch = top(tops::FETCH);
    fetch.set("node", export as f32);
    fetch.texts[0] = dir.join("box_w@wedgeindex.$F4.obj").to_string_lossy().into_owned();
    let fetch = inside(&mut app, net, AppContext::Tasks, NodeKind::Top(fetch));
    app.graph.connect(range, wedge, 0);
    app.graph.connect(wedge, fetch, 0);
    let mut cmd = top(tops::COMMAND);
    cmd.texts[0] = "test -f {input}".into();
    let cmd = inside(&mut app, net, AppContext::Tasks, NodeKind::Top(cmd));
    app.graph.connect(fetch, cmd, 0);
    assert_eq!(app.graph.top_items(cmd).unwrap().len(), 4);
    app.start_top_cook(cmd);
    for _ in 0..500 {
        if app.top_run.is_none() {
            break;
        }
        app.advance_top_run();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.top_run.is_none(), "the cook finished");
    for node in [range, wedge, fetch, cmd] {
        let states = &app.top_results[&node];
        assert!(states.iter().all(|s| s.0 == tops::State::Done), "{:?}: {states:?}", app.graph.nodes[&node].kind);
    }
    // The wedge changed the box: wedge 1 (size 3) is three times as wide as wedge 0 (size 1).
    let width = |name: &str| {
        let text = std::fs::read_to_string(dir.join(name)).unwrap();
        let xs: Vec<f32> = text.lines().filter_map(|l| l.strip_prefix("v ")).map(|l| l.split_whitespace().next().unwrap().parse().unwrap()).collect();
        xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min)
    };
    assert!((width("box_w0.0001.obj") - 1.0).abs() < 1e-3 && (width("box_w1.0002.obj") - 3.0).abs() < 1e-3);
    // The box is back to its own size afterwards.
    assert!(matches!(app.graph.nodes[&cube].kind, NodeKind::Box { size, .. } if size == 1.0));
    // A failing command fails its item; cancelling stops the rest.
    if let Some(NodeKind::Top(c)) = app.graph.nodes.get_mut(&cmd).map(|n| &mut n.kind) {
        c.texts[0] = "exit 3".into();
    }
    app.start_top_cook(cmd);
    for _ in 0..500 {
        if app.top_run.is_none() {
            break;
        }
        app.advance_top_run();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.top_results[&cmd].iter().all(|s| s.0 == tops::State::Failed));
    app.start_top_cook(cmd);
    app.top_cancel_request = true;
    app.advance_top_run();
    assert!(app.top_run.is_none());
}
