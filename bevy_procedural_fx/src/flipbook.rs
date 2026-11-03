//! Smooth sprite-sheet playback with motion vectors.
//!
//! Tick **Motion vectors** in ProceduralFX Studio's Export Sprite Sheet window, and it also
//! writes `name_motion.png`: how each pixel moves on its way to the next frame. An
//! [`FxFlipbook`] uses it to slide each frame towards the next instead of jumping, so a
//! sheet with a quarter of the frames still plays smoothly, even slowed right down.
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_procedural_fx::FxFlipbook;
//!
//! fn setup(mut commands: Commands, assets: Res<AssetServer>) {
//!     commands.spawn(FxFlipbook::looping(assets.load("fx/smoke.pfx.json")).with_speed(0.25));
//! }
//! ```

use crate::{FxSheet, Playback};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{AsBindGroup, BlendState, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dKey, Material2dPipeline, Material2dPlugin};

const SHADER: &str = "embedded://bevy_procedural_fx/flipbook.wesl";

pub(crate) fn build(app: &mut App) {
    embedded_asset!(app, "flipbook.wesl");
    // DefaultPlugins register meshes; a minimal app may not have.
    if !app.world().contains_resource::<Assets<Mesh>>() {
        app.init_asset::<Mesh>();
    }
    app.add_plugins(Material2dPlugin::<FlipbookMaterial>::default()).init_resource::<FxAmbient2d>()
        // After transforms are worked out, so lights are where they are this frame.
        .add_systems(PostUpdate, play.after(bevy::transform::TransformSystems::Propagate));
}

/// How an [`FxFlipbook`] goes from one frame to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Blend {
    /// Slide along the sheet's motion vectors; cross-fade if it has none.
    #[default]
    Motion,
    /// Cross-fade.
    Fade,
    /// Show whole frames, like [`FxSprite`](crate::FxSprite).
    None,
}

/// Plays an [`FxSheet`] on a 2D mesh with a shader that blends between frames: it gets a
/// [`Mesh2d`] the size of a frame and a [`FlipbookMaterial`].
#[derive(Component, Clone, Debug, Reflect)]
#[require(Transform, Visibility)]
pub struct FxFlipbook {
    pub sheet: Handle<FxSheet>,
    pub playback: Playback,
    pub blend: Blend,
    /// 1 plays at the exported frame rate, 0.25 at a quarter of it.
    pub speed: f32,
    /// Seconds played so far.
    pub elapsed: f32,
    pub paused: bool,
    /// Multiplies the effect's colour and opacity.
    pub color: Color,
    /// Lit by [`FxLight2d`]s, when the sheet has a normal map and there are any lights.
    /// Turn it off for effects that glow, such as fire, which light shouldn't darken.
    pub lit: bool,
    /// How strongly a glow sheet's glow shows: 1 at full brightness (more than the sheet's
    /// colour alone holds), more for stronger bloom, 0 for none at all. Bloom needs `Hdr`
    /// and `Bloom` on the camera.
    pub glow: f32,
}

impl FxFlipbook {
    pub fn new(sheet: Handle<FxSheet>, playback: Playback) -> Self {
        FxFlipbook { sheet, playback, blend: Blend::Motion, speed: 1.0, elapsed: 0.0, paused: false, color: Color::WHITE, lit: true, glow: 1.0 }
    }

    pub fn looping(sheet: Handle<FxSheet>) -> Self {
        Self::new(sheet, Playback::Loop)
    }

    pub fn once(sheet: Handle<FxSheet>) -> Self {
        Self::new(sheet, Playback::Once)
    }

    /// Plays once, then despawns the entity.
    pub fn one_shot(sheet: Handle<FxSheet>) -> Self {
        Self::new(sheet, Playback::OnceThenDespawn)
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn with_blend(mut self, blend: Blend) -> Self {
        self.blend = blend;
        self
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// How strongly a glow sheet glows (1: as rendered).
    pub fn with_glow(mut self, glow: f32) -> Self {
        self.glow = glow;
        self
    }

    /// Never lit, even with a normal map and lights: for effects that glow.
    pub fn unlit(mut self) -> Self {
        self.lit = false;
        self
    }
}

/// The most [`FxLight2d`]s that light one flipbook: the nearest ones.
pub const MAX_LIGHTS: usize = 8;

/// A point light for 2D: lights [`FxFlipbook`]s whose sheets have a normal map, from where
/// its transform puts it.
#[derive(Component, Clone, Debug, Reflect)]
#[require(Transform)]
pub struct FxLight2d {
    pub color: Color,
    /// 1 lights a surface facing it fully.
    pub intensity: f32,
    /// How far it reaches, in world units (pixels, with the default 2D camera); it fades to
    /// nothing there.
    pub radius: f32,
    /// How far in front of the scene it is: higher lights the effect's front more, lower
    /// lights its edges more, from the side.
    pub height: f32,
}

impl Default for FxLight2d {
    fn default() -> Self {
        FxLight2d { color: Color::WHITE, intensity: 1.0, radius: 400.0, height: 80.0 }
    }
}

/// The light everything lit gets anyway, from every side. Without any [`FxLight2d`]s,
/// flipbooks are drawn unlit.
#[derive(Resource, Clone, Debug, Reflect)]
pub struct FxAmbient2d(pub Color);

impl Default for FxAmbient2d {
    fn default() -> Self {
        FxAmbient2d(Color::srgb(0.25, 0.25, 0.25))
    }
}

/// The material an [`FxFlipbook`] is drawn with.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct FlipbookMaterial {
    #[uniform(0)]
    pub settings: FlipbookSettings,
    #[texture(1)]
    #[sampler(2)]
    pub sheet: Handle<Image>,
    /// The motion vectors; the sheet itself when there are none (they're then unused).
    #[texture(3)]
    #[sampler(4)]
    pub motion: Handle<Image>,
    /// The normal map; the sheet itself when there's none (it's then unused).
    #[texture(5)]
    #[sampler(6)]
    pub normal_map: Handle<Image>,
    /// The glow sheet; the sheet itself when there's none (it's then unused).
    #[texture(7)]
    #[sampler(8)]
    pub glow: Handle<Image>,
}

/// What the shader needs to know, and the frame to show.
#[derive(ShaderType, Reflect, Debug, Clone, Copy)]
pub struct FlipbookSettings {
    pub color: Vec4,
    pub frame: f32,
    pub strength: f32,
    pub columns: u32,
    pub rows: u32,
    pub frame_count: u32,
    pub looping: u32,
    /// 0 whole frames, 1 cross-fade, 2 motion vectors.
    pub blend: u32,
    pub motion_loops: u32,
    /// How many of `lights` light it (0: unlit).
    pub light_count: u32,
    pub ambient: Vec4,
    /// Each light's world position (z: its height) and radius.
    pub light_positions: [Vec4; MAX_LIGHTS],
    /// Each light's colour times its intensity.
    pub light_colors: [Vec4; MAX_LIGHTS],
    /// The glow sheet's strength (1: as rendered), and whether there is one.
    pub glow: f32,
    pub has_glow: u32,
}

impl Material2d for FlipbookMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }

    // The shader returns premultiplied colour, so glow can add light where the effect is
    // faint.
    fn specialize(_: &Material2dPipeline, descriptor: &mut RenderPipelineDescriptor, _: &MeshVertexBufferLayoutRef, _: Material2dKey<Self>) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(target) = descriptor.fragment.as_mut().and_then(|f| f.targets.first_mut()).and_then(|t| t.as_mut()) {
            target.blend = Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        }
        Ok(())
    }
}

impl FlipbookMaterial {
    fn new(sheet: &FxSheet, fx: &FxFlipbook) -> FlipbookMaterial {
        let motion = sheet.motion.as_ref();
        let blend = match (fx.blend, motion) {
            (Blend::None, _) => 0,
            (Blend::Fade, _) | (Blend::Motion, None) => 1,
            (Blend::Motion, Some(_)) => 2,
        };
        FlipbookMaterial {
            settings: FlipbookSettings {
                color: fx.color.to_linear().to_vec4(),
                frame: sheet.fps * fx.elapsed,
                strength: motion.map_or(0.0, |m| m.strength),
                columns: sheet.columns,
                rows: sheet.rows,
                frame_count: sheet.frame_count as u32,
                looping: (fx.playback == Playback::Loop) as u32,
                blend,
                motion_loops: motion.is_some_and(|m| m.looping) as u32,
                light_count: 0,
                ambient: Vec4::ONE,
                light_positions: [Vec4::ZERO; MAX_LIGHTS],
                light_colors: [Vec4::ZERO; MAX_LIGHTS],
                glow: fx.glow.max(0.0),
                has_glow: sheet.glow.is_some() as u32,
            },
            sheet: sheet.image.clone(),
            motion: motion.map_or_else(|| sheet.image.clone(), |m| m.image.clone()),
            normal_map: sheet.normal_map.clone().unwrap_or_else(|| sheet.image.clone()),
            glow: sheet.glow.clone().unwrap_or_else(|| sheet.image.clone()),
        }
    }
}

// A frame-sized quad placed so the full rendered frame's centre is at the origin, as
// FxSprite's anchor does.
fn quad(sheet: &FxSheet) -> Mesh {
    let size = sheet.frame_size.as_vec2();
    let centre = sheet.offset.as_vec2() + size / 2.0 - sheet.source_size.as_vec2() / 2.0;
    let (x0, x1) = (centre.x - size.x / 2.0, centre.x + size.x / 2.0);
    // Down the image is down the screen.
    let (y0, y1) = (-centre.y + size.y / 2.0, -centre.y - size.y / 2.0);
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    mesh.insert_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]));
    mesh
}

// The quad an entity was given, and the sheet it was made for (a re-export may change
// the frame size).
#[derive(Component)]
struct Quad(AssetId<FxSheet>, UVec2, UVec2, UVec2);

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    time: Res<Time>,
    sheets: Res<Assets<FxSheet>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FlipbookMaterial>>,
    ambient: Option<Res<FxAmbient2d>>,
    lights: Query<(&FxLight2d, &GlobalTransform)>,
    mut books: Query<(Entity, &mut FxFlipbook, Option<&MeshMaterial2d<FlipbookMaterial>>, Option<&Quad>, Option<&GlobalTransform>)>,
) {
    let ambient = ambient.map_or_else(|| FxAmbient2d::default().0, |a| a.0).to_linear().to_vec4();
    let lights: Vec<(Vec3, &FxLight2d)> = lights.iter().map(|(l, at)| (at.translation(), l)).collect();
    for (entity, mut fx, material, quad_for, at) in &mut books {
        let Some(sheet) = sheets.get(&fx.sheet) else { continue };
        if fx.playback == Playback::OnceThenDespawn && fx.elapsed >= sheet.duration() - 1e-4 {
            commands.entity(entity).despawn();
            continue;
        }
        let mut entity_commands = commands.entity(entity);
        let wanted = Quad(fx.sheet.id(), sheet.frame_size, sheet.source_size, sheet.offset);
        if quad_for.is_none_or(|q| (q.0, q.1, q.2, q.3) != (wanted.0, wanted.1, wanted.2, wanted.3)) {
            entity_commands.insert((Mesh2d(meshes.add(quad(sheet))), wanted));
        }
        let mut settings = FlipbookMaterial::new(sheet, &fx);
        if fx.lit && sheet.normal_map.is_some() && !lights.is_empty() {
            let here = at.map_or(Vec3::ZERO, |t| t.translation());
            let mut nearest: Vec<&(Vec3, &FxLight2d)> = lights.iter().collect();
            nearest.sort_by(|a, b| a.0.truncate().distance_squared(here.truncate()).total_cmp(&b.0.truncate().distance_squared(here.truncate())));
            let s = &mut settings.settings;
            for (i, (p, light)) in nearest.into_iter().take(MAX_LIGHTS).enumerate() {
                s.light_positions[i] = Vec4::new(p.x, p.y, light.height, light.radius.max(1e-3));
                s.light_colors[i] = (light.color.to_linear().to_vec3() * light.intensity).extend(1.0);
                s.light_count = i as u32 + 1;
            }
            s.ambient = ambient;
        }
        match material.map(|m| m.0.clone()).filter(|m| materials.contains(m)) {
            Some(handle) => {
                let mut material = materials.get_mut(&handle).expect("checked above");
                material.settings = settings.settings;
                if material.sheet != settings.sheet || material.motion != settings.motion || material.normal_map != settings.normal_map || material.glow != settings.glow {
                    material.sheet = settings.sheet;
                    material.motion = settings.motion;
                    material.normal_map = settings.normal_map;
                    material.glow = settings.glow;
                }
            }
            None => {
                entity_commands.insert(MeshMaterial2d(materials.add(settings)));
            }
        }
        if !fx.paused {
            fx.elapsed += time.delta_secs() * fx.speed;
        }
    }
}
