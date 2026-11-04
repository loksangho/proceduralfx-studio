use crate::*;

#[test]
fn every_parameter_has_a_name() {
    for (_, kinds) in NodeKind::templates() {
        for kind in kinds {
            let (params, names) = (kind.params(), kind.param_names());
            assert!(names.len() <= params.len(), "{}: {} names for {} params", kind.name(), names.len(), params.len());
            for (_, label) in kind.animatable() {
                assert!(!label.starts_with("parm"), "{} still has {label}", kind.name());
            }
            // Fixed-size nodes name every parameter (variable lists and table nodes may leave some unnamed).
            let variable = matches!(kind, NodeKind::Add { .. } | NodeKind::Blast { .. } | NodeKind::AttributePaint { .. } | NodeKind::ObjectMerge { .. } | NodeKind::MaterialAssign { .. } | NodeKind::Cache { .. } | NodeKind::Wrangle { .. } | NodeKind::MoonRayRender { .. } | NodeKind::Vop(_) | NodeKind::CompileEnd { .. } | NodeKind::Geometry { .. } | NodeKind::LopSopImport { .. } | NodeKind::LopMaterial { .. } | NodeKind::Light { .. } | NodeKind::Camera { .. } | NodeKind::EnvironmentLight { .. } | NodeKind::RigPose(_) | NodeKind::SolverInput { .. } | NodeKind::AttributeVop { .. });
            // Table nodes always store their operation first; only the rest needs names.
            let table_only_op = matches!(kind, NodeKind::Cop(_) | NodeKind::Top(_) | NodeKind::ShaderNode(_) | NodeKind::Pop(_));
            if !variable && !table_only_op && !params.is_empty() {
                assert!(!kind.animatable().is_empty(), "{} has parameters but none named", kind.name());
            }
        }
    }
    let names: Vec<String> = NodeKind::Box { size: 1.0, div: 1 }.animatable().into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, vec!["size", "divisions"]);
    let mut node = Graph::default();
    let id = node.add(NodeKind::Box { size: 1.0, div: 1 }, egui::pos2(0.0, 0.0));
    node.nodes.get_mut(&id).unwrap().name = "crate".into();
    assert_eq!(node.nodes[&id].type_label(), "Box");
}
