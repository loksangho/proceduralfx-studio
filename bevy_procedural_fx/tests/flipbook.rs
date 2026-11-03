// Loads assets/fx/smoke.pfx.json (exported from ProceduralFX Studio with motion vectors)
// in a headless app and plays it with FxFlipbook.
#![cfg(feature = "flipbook")]

use bevy::camera::primitives::MeshAabb;
use bevy::image::{CompressedImageFormats, ImageLoader};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::time::TimeUpdateStrategy;
use bevy_procedural_fx::{Blend, FlipbookMaterial, FxFlipbook, FxSheet, ProceduralFxPlugin};
use std::time::Duration;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ImagePlugin::default(), ProceduralFxPlugin))
        .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(1.0 / 24.0)));
    app
}

fn load(app: &mut App) -> Handle<FxSheet> {
    let handle: Handle<FxSheet> = app.world().resource::<AssetServer>().load("fx/smoke.pfx.json");
    for _ in 0..500 {
        app.update();
        if app.world().resource::<AssetServer>().is_loaded_with_dependencies(&handle) {
            return handle;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the sheet didn't load");
}

#[test]
fn motion_vectors_load_as_data_beside_the_sheet() {
    let mut app = app();
    let handle = load(&mut app);
    let sheet = app.world().resource::<Assets<FxSheet>>().get(&handle).unwrap().clone();
    let motion = sheet.motion.clone().expect("exported with motion vectors");
    assert!(motion.strength > 0.0 && motion.strength < 1.0);
    let images = app.world().resource::<Assets<Image>>();
    let (colour, vectors) = (images.get(&sheet.image).unwrap(), images.get(&motion.image).unwrap());
    assert_eq!(colour.size(), vectors.size(), "laid out like the sheet");
    assert_eq!(vectors.texture_descriptor.format, TextureFormat::Rgba8Unorm, "numbers, not sRGB colour");
    assert_eq!(colour.texture_descriptor.format, TextureFormat::Rgba8UnormSrgb);
}

#[test]
fn flipbooks_play_between_frames() {
    let mut app = app();
    let handle = load(&mut app);
    let fx = app.world_mut().spawn(FxFlipbook::looping(handle.clone()).with_speed(0.5)).id();
    // 13 updates of a 24th of a second at half speed: 13 / 48 s at 6 frames a second.
    for _ in 0..13 {
        app.update();
    }
    let world = app.world();
    assert!(world.get::<Mesh2d>(fx).is_some());
    let material = world.get::<MeshMaterial2d<FlipbookMaterial>>(fx).unwrap();
    let settings = world.resource::<Assets<FlipbookMaterial>>().get(&material.0).unwrap().settings;
    assert!((settings.frame - 12.0 / 48.0 * 6.0).abs() < 1e-3, "frame {}", settings.frame);
    assert_eq!((settings.blend, settings.columns, settings.frame_count), (2, 5, 12));
    // The quad is a frame's size.
    let sheet = world.resource::<Assets<FxSheet>>().get(&handle).unwrap();
    let mesh = world.resource::<Assets<Mesh>>().get(&world.get::<Mesh2d>(fx).unwrap().0).unwrap();
    let aabb = mesh.get_aabb().unwrap();
    assert_eq!((aabb.half_extents.x * 2.0, aabb.half_extents.y * 2.0), (sheet.frame_size.x as f32, sheet.frame_size.y as f32));
}

#[test]
fn fades_and_one_shots() {
    let mut app = app();
    let handle = load(&mut app);
    let fx = app.world_mut().spawn(FxFlipbook::one_shot(handle).with_blend(Blend::Fade).with_speed(4.0)).id();
    app.update();
    let material = app.world().get::<MeshMaterial2d<FlipbookMaterial>>(fx).unwrap().0.clone();
    assert_eq!(app.world().resource::<Assets<FlipbookMaterial>>().get(&material).unwrap().settings.blend, 1);
    // 12 frames at 6 fps, four times as fast: half a second, 12 updates.
    for _ in 0..14 {
        app.update();
    }
    assert!(app.world().get_entity(fx).is_err(), "despawned after the last frame");
}
