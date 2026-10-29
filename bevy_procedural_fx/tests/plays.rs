// Loads assets/fx/fire.pfx.json (exported from ProceduralFX Studio) in a headless app and
// plays it.

use bevy::image::{CompressedImageFormats, ImageLoader};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy_procedural_fx::{FxSheet, FxSprite, ProceduralFxPlugin};
use std::time::Duration;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ImagePlugin::default(), ProceduralFxPlugin))
        // The renderer registers the PNG loader; there's no renderer here.
        .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(1.0 / 24.0)));
    app
}

fn load(app: &mut App) -> Handle<FxSheet> {
    load_path(app, "fx/fire.pfx.json")
}

fn load_path(app: &mut App, path: &str) -> Handle<FxSheet> {
    let handle: Handle<FxSheet> = app.world().resource::<AssetServer>().load(path.to_string());
    for _ in 0..500 {
        app.update();
        if app.world().resource::<AssetServer>().is_loaded_with_dependencies(&handle) {
            return handle;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let server = app.world().resource::<AssetServer>();
    panic!("the sheet didn't load: {:?} / {:?}", server.load_state(&handle), server.recursive_dependency_load_state(&handle));
}

#[test]
fn the_sheet_loads_with_its_image_and_grid() {
    let mut app = app();
    let handle = load(&mut app);
    let sheet = app.world().resource::<Assets<FxSheet>>().get(&handle).unwrap().clone();
    assert_eq!((sheet.frame_count, sheet.fps), (24, 24.0));
    let layout = app.world().resource::<Assets<TextureAtlasLayout>>().get(&sheet.layout).unwrap();
    assert_eq!(layout.textures.len(), 24);
    // Six across: frame 7 is the second in the second row.
    assert_eq!(layout.textures[7].min, sheet.frame_size);
    let image = app.world().resource::<Assets<Image>>().get(&sheet.image).unwrap();
    assert_eq!(image.size(), UVec2::new(sheet.frame_size.x * 6, sheet.frame_size.y * 4));
}

#[test]
fn one_shots_play_every_frame_then_despawn() {
    let mut app = app();
    let handle = load(&mut app);
    let fx = app.world_mut().spawn(FxSprite::one_shot(handle)).id();
    let mut seen = Vec::new();
    for _ in 0..40 {
        app.update();
        let Some(sprite) = app.world().get::<Sprite>(fx) else { break };
        let index = sprite.texture_atlas.as_ref().expect("the atlas is set once loaded").index;
        if seen.last() != Some(&index) {
            seen.push(index);
        }
    }
    assert_eq!(seen, (0..24).collect::<Vec<_>>(), "every frame once, in order");
    assert!(!app.world().entities().contains(fx), "despawned after the last frame");
}

#[test]
fn sheets_built_into_the_program_load_their_image_beside_them() {
    // Games with no assets folder (phones, the web) build their sheets in with
    // embedded_asset!. The .pfx.json names its PNG by a relative path, which has to resolve
    // inside the embedded source, not the assets folder.
    let mut app = app();
    bevy::asset::embedded_asset!(app, "tests", "embedded/fx/spark.pfx.json");
    bevy::asset::embedded_asset!(app, "tests", "embedded/fx/spark.png");
    let handle = load_path(&mut app, "embedded://plays/embedded/fx/spark.pfx.json");
    let sheet = app.world().resource::<Assets<FxSheet>>().get(&handle).unwrap().clone();
    assert_eq!((sheet.frame_count, sheet.fps, sheet.frame_size), (4, 12.0, UVec2::splat(8)));
    assert_eq!(sheet.image.path().map(|p| p.to_string()).as_deref(), Some("embedded://plays/embedded/fx/spark.png"));
    let image = app.world().resource::<Assets<Image>>().get(&sheet.image).unwrap();
    assert_eq!(image.size(), UVec2::new(16, 16));
}
