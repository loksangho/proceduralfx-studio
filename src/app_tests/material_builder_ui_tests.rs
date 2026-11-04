use crate::*;

#[test]
fn inside_a_material_builder_only_shader_nodes_are_offered() {
    let mut app = ProceduralFxStudioApp::default();
    app.graph = Graph::default();
    app.context = AppContext::Mat;
    app.apply(Action::Add(NodeKind::MaterialBuilder, egui::pos2(0.0, 0.0)));
    let builder = app.selected.unwrap();
    app.apply(Action::Enter(builder));
    assert_eq!(app.context, AppContext::Mat, "the builder's network is a MAT network");
    assert_eq!(app.view_parent(), Some(builder));
    let menus = app.templates_for_view();
    assert_eq!(menus.iter().map(|(c, _)| *c).collect::<Vec<_>>(), vec![shading::CATEGORY]);
    assert!(menus.iter().flat_map(|(_, k)| k).all(|k| matches!(k, NodeKind::ShaderNode(_))));
    assert_eq!(menus[0].1.len(), shading::SHADER_OPS.len());
    // The Surface Output made with the builder is in the network shown, and new nodes join it.
    let shown: Vec<NodeId> = app.graph.nodes.iter().filter(|(_, n)| n.context == app.context && n.parent == app.view_parent()).map(|(id, _)| *id).collect();
    assert_eq!(shown.len(), 1);
    assert!(matches!(&app.graph.nodes[&shown[0]].kind, NodeKind::ShaderNode(n) if n.op == shading::SURFACE_OUTPUT));
    app.apply(Action::Add(NodeKind::ShaderNode(shading::ShaderNode::new(4)), egui::pos2(0.0, 0.0)));
    let noise = app.selected.unwrap();
    assert_eq!((app.graph.nodes[&noise].context, app.graph.nodes[&noise].parent), (AppContext::Mat, Some(builder)));
    // Up goes back to the top of MAT, with materials (not shader nodes) on offer.
    app.enter_network(None);
    assert_eq!((app.context, app.network()), (AppContext::Mat, None));
    let top = app.templates_for_view();
    assert!(top.iter().flat_map(|(_, k)| k).all(|k| k.is_material()));
    // Switching to OBJ from inside the builder leaves it.
    app.apply(Action::Enter(builder));
    app.context = AppContext::Obj;
    assert_eq!(app.view_parent(), None);
}
