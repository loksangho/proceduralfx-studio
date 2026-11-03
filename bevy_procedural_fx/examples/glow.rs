//! A campfire whose hot core glows under bloom, at three strengths: no glow, as rendered,
//! and three times as strong. The sheet was exported from ProceduralFX Studio with Glow
//! sheet ticked; bloom needs `Hdr` and `Bloom` on the camera.
//!
//! cargo run --example glow

use bevy::camera::Hdr;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy_procedural_fx::{FxFlipbook, ProceduralFxPlugin};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ProceduralFxPlugin))
        .insert_resource(ClearColor(Color::srgb(0.03, 0.03, 0.05)))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((Camera2d, Hdr, Bloom::default()));
    let fire = assets.load("fx/campfire.pfx.json");
    for (x, glow, label) in [(-300.0, 0.0, "No glow"), (0.0, 1.0, "As rendered"), (300.0, 3.0, "Three times")] {
        commands.spawn((FxFlipbook::looping(fire.clone()).with_glow(glow), Transform::from_xyz(x, 30.0, 0.0).with_scale(Vec3::splat(1.6))));
        commands.spawn((Text2d::new(label), TextFont { font_size: FontSize::Px(22.0), ..default() }, Transform::from_xyz(x, -260.0, 0.0)));
    }
}
