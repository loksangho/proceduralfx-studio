use crate::*;

#[test]
fn file_import_plays_the_chosen_take() {
    let path = std::env::temp_dir().join(format!("pfx_takes_{}.fbx", std::process::id()));
    std::fs::write(&path, include_bytes!("../../assets/testgeo/character.fbx")).unwrap();
    let file = path.to_string_lossy().into_owned();
    let names: Vec<String> = fileio::fbx_take_names(&file).into_iter().map(|(n, _)| n).collect();
    assert!(names.contains(&"Walk".to_string()) && names.contains(&"Jump".to_string()), "{names:?}");
    let mut g = Graph::default();
    let reader = g.add(NodeKind::FileImport { file: file.clone(), take: "Walk".into(), looping: true }, egui::pos2(0.0, 0.0));
    assert!(g.time_dependent(reader));
    let walk = |g: &Graph, t: f32| g.cook(reader, t, &mut HashMap::new());
    let (a, b) = (walk(&g, 0.2), walk(&g, 0.6));
    assert!(!a.vertices.is_empty() && a.vertices != b.vertices, "the walk moves");
    // Looping: a take's length later, the same pose.
    let length = fileio::fbx_take_names(&file).into_iter().find(|(n, _)| n == "Walk").unwrap().1 as f32;
    let again = walk(&g, 0.2 + length);
    assert!(a.vertices.iter().zip(&again.vertices).all(|(p, q)| p.sub(*q).length() < 1e-3));
    // Another take gives other poses; a misspelt one says what the file has.
    if let NodeKind::FileImport { take, .. } = &mut g.nodes.get_mut(&reader).unwrap().kind {
        *take = "Jump".into();
    }
    assert_ne!(walk(&g, 0.2).vertices, a.vertices);
    if let NodeKind::FileImport { take, .. } = &mut g.nodes.get_mut(&reader).unwrap().kind {
        *take = "Moonwalk".into();
    }
    walk(&g, 0.2);
    let error = node_errors(|e| e.get(&reader).cloned()).unwrap_or_default();
    assert!(error.contains("Moonwalk") && error.contains("Walk"), "{error}");
    // The take and loop are saved with the scene.
    if let NodeKind::FileImport { take, .. } = &mut g.nodes.get_mut(&reader).unwrap().kind {
        *take = "Run".into();
    }
    let loaded = Graph::deserialize(&g.serialize());
    assert!(matches!(&loaded.nodes[&reader].kind, NodeKind::FileImport { take, looping: true, .. } if take == "Run"));
    let _ = std::fs::remove_file(&path);
}
