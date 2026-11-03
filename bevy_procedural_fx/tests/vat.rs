// Loads the vertex animation bakes in assets/fx (exported from ProceduralFX Studio) in a
// headless app and plays them.
#![cfg(feature = "vat")]

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_procedural_fx::{FxMesh, FxVat, ProceduralFxPlugin, VatMaterial, VatMode};
use std::time::Duration;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ImagePlugin::default(), ProceduralFxPlugin))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(1.0 / 24.0)));
    app
}

fn load(app: &mut App, path: &'static str) -> Handle<FxVat> {
    let handle: Handle<FxVat> = app.world().resource::<AssetServer>().load(path);
    for _ in 0..500 {
        app.update();
        if app.world().resource::<AssetServer>().is_loaded_with_dependencies(&handle) {
            return handle;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let server = app.world().resource::<AssetServer>();
    panic!("{path} didn't load: {:?} / {:?}", server.load_state(&handle), server.recursive_dependency_load_state(&handle));
}

#[test]
fn bakes_load_with_their_mesh_and_textures() {
    let mut app = app();
    for (path, mode, elements) in [("fx/wave.vat.json", VatMode::Soft, 400), ("fx/rubble.vat.json", VatMode::Rigid, 20)] {
        let handle = load(&mut app, path);
        let vat = app.world().resource::<Assets<FxVat>>().get(&handle).unwrap().clone();
        assert_eq!((vat.mode, vat.element_count, vat.frame_count, vat.fps), (mode, elements, 48, 24.0), "{path}");
        let images = app.world().resource::<Assets<Image>>();
        for texture in &vat.textures {
            assert_eq!(images.get(texture).unwrap().size(), UVec2::new(vat.width as u32, (vat.frame_count * vat.rows_per_frame) as u32), "{path}");
        }
        let mesh = app.world().resource::<Assets<Mesh>>().get(&vat.mesh).unwrap();
        // Every corner in the OBJ shares its position, UV and normal number, so the mesh
        // has as many vertices as the OBJ has positions.
        let obj = std::fs::read_to_string(format!("assets/{}", path.replace(".vat.json", ".obj"))).unwrap();
        assert_eq!(mesh.count_vertices(), obj.lines().filter(|l| l.starts_with("v ")).count(), "{path}");
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some() && mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some());
    }
}

#[test]
fn meshes_play_with_the_whole_animations_bounds() {
    let mut app = app();
    let handle = load(&mut app, "fx/wave.vat.json");
    let fx = app.world_mut().spawn(FxMesh::looping(handle.clone())).id();
    for _ in 0..13 {
        app.update();
    }
    let world = app.world();
    let vat = world.resource::<Assets<FxVat>>().get(&handle).unwrap();
    assert_eq!(world.get::<Mesh3d>(fx).unwrap().0, vat.mesh);
    let aabb = world.get::<Aabb>(fx).unwrap();
    assert_eq!(*aabb, Aabb::from_min_max(vat.bounds.0, vat.bounds.1), "culled by every frame's extent, not the first's");
    let material = world.get::<MeshMaterial3d<VatMaterial>>(fx).unwrap();
    let settings = world.resource::<Assets<VatMaterial>>().get(&material.0).unwrap().extension.settings;
    assert!(settings.frame > 10.0 && settings.frame < 13.0, "the clock moves the frame: {}", settings.frame);
    assert!(settings.previous_frame < settings.frame);
    assert_eq!((settings.mode, settings.looping, settings.element_count), (0, 1, 400));
}

#[test]
fn one_shots_despawn_after_the_last_frame() {
    let mut app = app();
    let handle = load(&mut app, "fx/rubble.vat.json");
    let fx = app.world_mut().spawn(FxMesh::one_shot(handle).with_speed(4.0)).id();
    // 48 frames at four times speed: 12 updates, and one more to notice.
    for _ in 0..11 {
        app.update();
    }
    assert!(app.world().get_entity(fx).is_ok(), "still playing");
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().get_entity(fx).is_err(), "despawned after the last frame");
}
