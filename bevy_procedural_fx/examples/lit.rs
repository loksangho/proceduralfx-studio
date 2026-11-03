//! Smoke lit by a 2D light that follows the mouse. The sheet was exported from
//! ProceduralFX Studio with Normal map ticked; the light needs nothing else.
//!
//! cargo run --example lit

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_procedural_fx::{FxFlipbook, FxLight2d, ProceduralFxPlugin};

#[derive(Component)]
struct FollowMouse;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ProceduralFxPlugin))
        .insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.07)))
        .add_systems(Startup, setup)
        .add_systems(Update, follow_mouse)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn(Camera2d);
    let smoke = assets.load("fx/smoke.pfx.json");
    for x in [-250.0, 0.0, 250.0] {
        commands.spawn((FxFlipbook::looping(smoke.clone()).with_speed(0.5), Transform::from_xyz(x, 0.0, 0.0).with_scale(Vec3::splat(1.8))));
    }
    // A warm light where the mouse is, and a dim cool one low on the right.
    commands.spawn((FxLight2d { color: Color::srgb(1.0, 0.8, 0.55), intensity: 2.5, radius: 600.0, height: 70.0 }, Transform::from_xyz(-300.0, 0.0, 0.0), FollowMouse));
    commands.spawn((FxLight2d { color: Color::srgb(0.4, 0.55, 1.0), intensity: 1.0, radius: 700.0, height: 120.0 }, Transform::from_xyz(450.0, -250.0, 0.0)));
}

fn follow_mouse(window: Single<&Window, With<PrimaryWindow>>, camera: Single<(&Camera, &GlobalTransform)>, mut light: Single<&mut Transform, With<FollowMouse>>) {
    let (camera, at) = *camera;
    if let Some(point) = window.cursor_position().and_then(|p| camera.viewport_to_world_2d(at, p).ok()) {
        light.translation = point.extend(0.0);
    }
}
