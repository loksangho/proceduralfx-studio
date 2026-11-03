//! The same smoke, slowed to a quarter speed, three ways: whole frames, cross-faded, and
//! following the sheet's motion vectors. The sheet has only 12 frames (every 4th frame of
//! the effect), exported from ProceduralFX Studio with Motion vectors ticked.
//!
//! cargo run --example flipbook

use bevy::prelude::*;
use bevy_procedural_fx::{Blend, FxFlipbook, ProceduralFxPlugin};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ProceduralFxPlugin))
        .insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.11)))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn(Camera2d);
    let smoke = assets.load("fx/smoke.pfx.json");
    for (x, blend, label) in [(-300.0, Blend::None, "Whole frames"), (0.0, Blend::Fade, "Cross-fade"), (300.0, Blend::Motion, "Motion vectors")] {
        commands.spawn((FxFlipbook::looping(smoke.clone()).with_blend(blend).with_speed(0.25), Transform::from_xyz(x, 20.0, 0.0).with_scale(Vec3::splat(1.6))));
        commands.spawn((Text2d::new(label), TextFont { font_size: FontSize::Px(22.0), ..default() }, Transform::from_xyz(x, -250.0, 0.0)));
    }
}
