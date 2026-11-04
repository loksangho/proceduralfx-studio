use crate::*;

#[test]
fn wrangle_examples_and_lsystem_presets_run() {
    let mut voxels = Vec::new();
    for x in -3..3 {
        for y in 0..5 {
            for z in -3..3 {
                voxels.push(([x, y, z], 1.0));
            }
        }
    }
    let fog = volume::encode_voxels(&voxels, volume::Class::Fog, 0.4, Vec3::zero(), 0.0);
    for (label, code) in VOLUME_WRANGLE_EXAMPLES {
        vex::check(code).unwrap_or_else(|e| panic!("{label}: {e}"));
        let out = fields::volume_wrangle(fog.clone(), &[], code, 0.0, 1.0 / 24.0, None).unwrap();
        assert!(vex::last_report(code, 2).unwrap().error.is_none(), "{label}: {:?}", vex::last_report(code, 2).unwrap().error);
        assert_ne!(out.point_attributes["density"], fog.point_attributes["density"], "{label} changed nothing");
    }
    let terrain = terrain::terrain_noise(&terrain::terrain(&terrain::TerrainSettings { size: [10.0, 10.0], resolution: 32 }), &terrain::NoiseSettings::defaults()).unwrap();
    for (label, code) in HEIGHTFIELD_WRANGLE_EXAMPLES {
        vex::check(code).unwrap_or_else(|e| panic!("{label}: {e}"));
        let out = fields::heightfield_wrangle(terrain.clone(), &[], code, 0.0, 1.0 / 24.0, None, None).unwrap();
        assert!(vex::last_report(code, 2).unwrap().error.is_none(), "{label}: {:?}", vex::last_report(code, 2).unwrap().error);
        let changed = out.vertices != terrain.vertices || out.point_attributes.get("mask") != terrain.point_attributes.get("mask");
        assert!(changed, "{label} changed nothing");
    }
    for (label, premise, rules, generations, angle, step) in lsystem::PRESETS {
        let set = lsystem::LSystemSettings { premise: premise.to_string(), rules: rules.to_string(), generations: *generations, angle: *angle, step: *step, ..lsystem::LSystemSettings::defaults() };
        let mesh = lsystem::generate(&set).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert!(!groom::curves(&mesh).is_empty(), "{label}");
    }
}

#[test]
fn new_nodes_save_and_load() {
    let mut g = Graph::default();
    let mut set = lsystem::LSystemSettings::defaults();
    set.rules = "A=F[+A]".into();
    set.premise = "A".into();
    set.generations = 3;
    let l = g.add(NodeKind::LSystem(set.clone()), egui::pos2(0.0, 0.0));
    let v = g.add(NodeKind::VolumeWrangle { code: "@density = 2;".into() }, egui::pos2(0.0, 80.0));
    let h = g.add(NodeKind::HeightfieldWrangle { code: "@height = 1;".into() }, egui::pos2(0.0, 160.0));
    let loaded = Graph::deserialize(&g.serialize());
    assert!(matches!(&loaded.nodes[&l].kind, NodeKind::LSystem(s) if *s == set));
    assert!(matches!(&loaded.nodes[&v].kind, NodeKind::VolumeWrangle { code } if code == "@density = 2;"));
    assert!(matches!(&loaded.nodes[&h].kind, NodeKind::HeightfieldWrangle { code } if code == "@height = 1;"));
    // The L-system cooks on its own; the wrangles explain what they need.
    let curves = g.cook(l, 0.0, &mut HashMap::new());
    assert!(groom::has_curves(&curves));
    g.cook(v, 0.0, &mut HashMap::new());
    let error = node_errors(|errors| errors.get(&v).cloned());
    assert!(error.as_deref().is_some_and(|e| e.contains("volume")), "{error:?}");
}
