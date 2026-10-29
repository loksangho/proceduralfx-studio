//! Three fires from one sprite sheet exported from ProceduralFX Studio, at different
//! speeds. Click to set off a one-shot burst where you click.
//!
//! cargo run --example fire

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_procedural_fx::{FxSheet, FxSprite, ProceduralFxPlugin};

#[derive(Resource)]
struct Fire(Handle<FxSheet>);

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ProceduralFxPlugin))
        .insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.11)))
        .add_systems(Startup, setup)
        .add_systems(Update, burst_on_click)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn(Camera2d);
    let fire = assets.load("fx/fire.pfx.json");
    for (x, speed) in [(-220.0, 0.6), (0.0, 1.0), (220.0, 1.6)] {
        commands.spawn((FxSprite::looping(fire.clone()).with_speed(speed), Transform::from_xyz(x, 0.0, 0.0).with_scale(Vec3::splat(2.5))));
    }
    commands.insert_resource(Fire(fire));
}

fn burst_on_click(
    mut commands: Commands,
    buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    fire: Res<Fire>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let (camera, at) = *camera;
    let Some(point) = window.cursor_position().and_then(|p| camera.viewport_to_world_2d(at, p).ok()) else { return };
    commands.spawn((FxSprite::one_shot(fire.0.clone()).with_speed(2.0), Transform::from_translation(point.extend(1.0)).with_scale(Vec3::splat(1.5))));
}
