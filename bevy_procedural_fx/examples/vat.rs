//! Vertex animation exported from ProceduralFX Studio: a rippling sheet (soft) loops beside
//! a box breaking apart (rigid). Press Space to break another one.
//!
//! cargo run --example vat

use bevy::prelude::*;
use bevy_procedural_fx::{FxMesh, FxVat, ProceduralFxPlugin};

#[derive(Resource)]
struct Rubble(Handle<FxVat>);

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ProceduralFxPlugin))
        .insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.11)))
        .add_systems(Startup, setup)
        .add_systems(Update, break_on_space)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.spawn((Camera3d::default(), Transform::from_xyz(0.0, 4.5, 9.0).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y)));
    commands.spawn((
        DirectionalLight { shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(14.0, 8.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.25, 0.25, 0.3))),
        Transform::from_xyz(0.0, -0.9, 0.0),
    ));

    let sheet = StandardMaterial { base_color: Color::srgb(0.3, 0.6, 0.9), perceptual_roughness: 0.5, double_sided: true, cull_mode: None, ..default() };
    commands.spawn((FxMesh::looping(assets.load("fx/wave.vat.json")).with_material(sheet), Transform::from_xyz(-3.0, 0.0, 0.0).with_scale(Vec3::splat(0.9))));

    let rubble = assets.load("fx/rubble.vat.json");
    commands.spawn((FxMesh::looping(rubble.clone()).with_material(brick()), Transform::from_xyz(3.0, -0.9, 0.0)));
    commands.insert_resource(Rubble(rubble));
}

fn brick() -> StandardMaterial {
    StandardMaterial { base_color: Color::srgb(0.8, 0.45, 0.3), perceptual_roughness: 0.8, ..default() }
}

fn break_on_space(mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, rubble: Res<Rubble>) {
    if keys.just_pressed(KeyCode::Space) {
        // Plays once, then the entity goes.
        commands.spawn((FxMesh::one_shot(rubble.0.clone()).with_material(brick()), Transform::from_xyz(0.0, -0.9, 1.5)));
    }
}
