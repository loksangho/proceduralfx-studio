//! Vertex animation for 3D: cloth, destruction and liquids baked in ProceduralFX Studio,
//! played on the GPU.
//!
//! **Render → Export Vertex Animation…** writes `name.vat.json`, `name.obj` and two EXR
//! textures. Put them in your `assets` folder and spawn an [`FxMesh`] with the
//! `.vat.json` file:
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_procedural_fx::{FxMesh, ProceduralFxPlugin};
//!
//! fn setup(mut commands: Commands, assets: Res<AssetServer>) {
//!     commands.spawn(FxMesh::looping(assets.load("fx/wave.vat.json")));
//! }
//! ```
//!
//! The entity gets a [`Mesh3d`] and a [`VatMaterial`]: a [`StandardMaterial`] whose vertex
//! shader moves the mesh, in the main pass and in shadows alike.

use crate::Playback;
use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages, embedded_asset};
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use serde::Deserialize;
use std::collections::HashMap;

const SHADER: &str = "embedded://bevy_procedural_fx/vat.wesl";

/// The material an [`FxMesh`] is drawn with: your [`StandardMaterial`], moved by the bake.
pub type VatMaterial = ExtendedMaterial<StandardMaterial, VatExtension>;

pub(crate) fn build(app: &mut App) {
    embedded_asset!(app, "vat.wesl");
    app.add_plugins(MaterialPlugin::<VatMaterial>::default())
        .init_asset::<FxVat>()
        .register_asset_loader(FxVatLoader)
        .add_systems(PostUpdate, play.before(bevy::camera::visibility::VisibilitySystems::CalculateBounds));
}

/// What a bake's elements are, and so how the shader moves the mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum VatMode {
    /// Points that stay the same and move freely, such as cloth and deformers.
    Soft,
    /// Pieces that move and turn without bending, such as destruction.
    Rigid,
    /// A mesh whose points and polygons change every frame, such as a liquid.
    Fluid,
}

/// A vertex animation bake exported from ProceduralFX Studio.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct FxVat {
    pub mode: VatMode,
    /// The mesh, as it is on the first frame (fluids: spare triangles the shader places).
    pub mesh: Handle<Mesh>,
    /// Soft: offsets and normals. Rigid: positions and rotations. Fluid: positions and
    /// normals.
    pub textures: [Handle<Image>; 2],
    pub frame_count: usize,
    pub fps: f32,
    pub element_count: usize,
    /// Elements to a texture row.
    pub width: usize,
    /// Texture rows each frame takes.
    pub rows_per_frame: usize,
    /// Everything the animation covers, in the mesh's space.
    pub bounds: (Vec3, Vec3),
}

impl FxVat {
    /// How long one pass through the frames takes, in seconds.
    pub fn duration(&self) -> f32 {
        self.frame_count as f32 / self.fps.max(1e-3)
    }

    /// The frame shown `elapsed` seconds in, with fractions blending to the next frame
    /// (fluids show whole frames).
    pub fn frame_at(&self, elapsed: f32, playback: Playback) -> f32 {
        let frame = elapsed.max(0.0) * self.fps;
        let last = self.frame_count.saturating_sub(1) as f32;
        match playback {
            Playback::Loop => frame % self.frame_count.max(1) as f32,
            Playback::Once | Playback::OnceThenDespawn => frame.min(last),
        }
    }
}

/// The bake's settings and textures, beside the [`StandardMaterial`] in a [`VatMaterial`].
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct VatExtension {
    #[uniform(100)]
    pub settings: VatSettings,
    #[texture(101, sample_type = "float", filterable = false)]
    pub first: Handle<Image>,
    #[texture(102, sample_type = "float", filterable = false)]
    pub second: Handle<Image>,
}

/// What the shader needs to know about a bake, and the frame to show.
#[derive(ShaderType, Reflect, Debug, Clone, Copy, Default)]
pub struct VatSettings {
    pub frame: f32,
    /// Last frame's `frame`, for motion vectors.
    pub previous_frame: f32,
    pub frame_count: u32,
    pub element_count: u32,
    pub width: u32,
    pub rows_per_frame: u32,
    /// 0 soft, 1 rigid, 2 fluid.
    pub mode: u32,
    pub looping: u32,
}

impl VatExtension {
    pub fn new(vat: &FxVat, playback: Playback) -> VatExtension {
        VatExtension {
            settings: VatSettings {
                frame: 0.0,
                previous_frame: 0.0,
                frame_count: vat.frame_count as u32,
                element_count: vat.element_count as u32,
                width: vat.width as u32,
                rows_per_frame: vat.rows_per_frame as u32,
                mode: vat.mode as u32,
                looping: (playback == Playback::Loop) as u32,
            },
            first: vat.textures[0].clone(),
            second: vat.textures[1].clone(),
        }
    }
}

impl MaterialExtension for VatExtension {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn deferred_vertex_shader() -> ShaderRef {
        SHADER.into()
    }
}

/// Plays an [`FxVat`] on this entity: it gets the bake's [`Mesh3d`] and a [`VatMaterial`].
#[derive(Component, Clone, Debug, Reflect)]
#[require(Transform, Visibility)]
pub struct FxMesh {
    pub vat: Handle<FxVat>,
    pub playback: Playback,
    /// 1 plays at the exported frame rate, 2 twice as fast.
    pub speed: f32,
    /// Seconds played so far.
    pub elapsed: f32,
    pub paused: bool,
    /// The surface: colour, roughness, metalness and so on. Its textures can't use the
    /// mesh's UVs, which pick each vertex's texels.
    pub material: StandardMaterial,
}

impl FxMesh {
    pub fn new(vat: Handle<FxVat>, playback: Playback) -> Self {
        FxMesh {
            vat,
            playback,
            speed: 1.0,
            elapsed: 0.0,
            paused: false,
            material: StandardMaterial { perceptual_roughness: 0.6, ..default() },
        }
    }

    pub fn looping(vat: Handle<FxVat>) -> Self {
        Self::new(vat, Playback::Loop)
    }

    pub fn once(vat: Handle<FxVat>) -> Self {
        Self::new(vat, Playback::Once)
    }

    /// Plays once, then despawns the entity.
    pub fn one_shot(vat: Handle<FxVat>) -> Self {
        Self::new(vat, Playback::OnceThenDespawn)
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn with_material(mut self, material: StandardMaterial) -> Self {
        self.material = material;
        self
    }
}

type Playing<'a> = (Entity, &'a mut FxMesh, Option<&'a Mesh3d>, Option<&'a MeshMaterial3d<VatMaterial>>, Option<&'a Aabb>);

fn play(
    mut commands: Commands,
    time: Res<Time>,
    vats: Res<Assets<FxVat>>,
    mut materials: ResMut<Assets<VatMaterial>>,
    mut meshes: Query<Playing>,
) {
    for (entity, mut fx, mesh, material, aabb) in &mut meshes {
        // Not loaded yet: nothing to show, and the clock waits.
        let Some(vat) = vats.get(&fx.vat) else { continue };
        if fx.playback == Playback::OnceThenDespawn && fx.elapsed >= vat.duration() - 1e-4 {
            commands.entity(entity).despawn();
            continue;
        }
        let mut entity_commands = commands.entity(entity);
        // Checked every frame, so a bake exported again while the game runs (with Bevy's
        // file_watcher feature) takes effect straight away.
        if mesh.is_none_or(|m| m.0 != vat.mesh) {
            entity_commands.insert(Mesh3d(vat.mesh.clone()));
        }
        // The mesh's own bounds are only the first frame's: cull by the whole animation's.
        let bounds = Aabb::from_min_max(vat.bounds.0, vat.bounds.1);
        if aabb.is_none_or(|a| a.center != bounds.center || a.half_extents != bounds.half_extents) {
            entity_commands.insert((bounds, NoAutoAabb));
        }
        let frame = vat.frame_at(fx.elapsed, fx.playback);
        let wanted = VatExtension::new(vat, fx.playback);
        let existing = material.map(|m| m.0.clone()).filter(|m| materials.contains(m));
        match existing {
            Some(handle) => {
                let mut material = materials.get_mut(&handle).expect("checked above");
                let ext = &mut material.extension;
                let previous = ext.settings.frame;
                ext.settings = VatSettings { frame, previous_frame: previous, ..wanted.settings };
                if ext.first != wanted.first || ext.second != wanted.second {
                    ext.first = wanted.first;
                    ext.second = wanted.second;
                }
            }
            None => {
                let mut extension = wanted;
                extension.settings.frame = frame;
                extension.settings.previous_frame = frame;
                let handle = materials.add(VatMaterial { base: fx.material.clone(), extension });
                entity_commands.insert(MeshMaterial3d(handle));
            }
        }
        if !fx.paused {
            fx.elapsed += time.delta_secs() * fx.speed;
        }
    }
}

// The .vat.json file, as ProceduralFX Studio writes it.
#[derive(Deserialize)]
struct VatFile {
    format: String,
    version: u32,
    mode: String,
    mesh: String,
    frame_count: usize,
    fps: f32,
    element_count: usize,
    width: usize,
    rows_per_frame: usize,
    bounds: [[f32; 3]; 2],
    #[serde(flatten)]
    rest: HashMap<String, serde_json::Value>,
}

/// Why a `.vat.json` bake couldn't be loaded.
#[derive(Debug, thiserror::Error)]
pub enum FxVatError {
    #[error("couldn't read the vertex animation: {0}")]
    Io(#[from] std::io::Error),
    #[error("couldn't parse the vertex animation: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a ProceduralFX Studio vertex animation (format \"{0}\")")]
    Format(String),
    #[error("vertex animation version {0} is older or newer than this plugin understands; export it again, or update bevy_procedural_fx")]
    Version(u32),
    #[error("unknown kind of vertex animation \"{0}\"")]
    Mode(String),
    #[error("the vertex animation doesn't name its {0} texture")]
    Texture(&'static str),
    #[error("couldn't find a file the vertex animation names: {0}")]
    Path(String),
    #[error("couldn't read the vertex animation's mesh, line {line}: {problem}")]
    Obj { line: usize, problem: String },
    #[error("the vertex animation's layout doesn't add up: {0}")]
    Layout(String),
}

/// Loads `.vat.json` bakes; the mesh and textures they name load beside them.
#[derive(Default, TypePath)]
pub struct FxVatLoader;

impl AssetLoader for FxVatLoader {
    type Asset = FxVat;
    type Settings = ();
    type Error = FxVatError;

    async fn load(&self, reader: &mut dyn Reader, _settings: &(), load_context: &mut LoadContext<'_>) -> Result<FxVat, FxVatError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let file: VatFile = serde_json::from_slice(&bytes)?;
        let (mode, names) = check(&file)?;
        let mesh_path = load_context.path().resolve_embed_str(&file.mesh).map_err(|e| FxVatError::Path(e.to_string()))?;
        let obj = load_context.read_asset_bytes(mesh_path.clone()).await.map_err(|e| FxVatError::Path(format!("{mesh_path}: {e}")))?;
        let mesh = parse_obj(&String::from_utf8_lossy(&obj))?;
        let mut textures = Vec::with_capacity(2);
        for name in names {
            let path = load_context.path().resolve_embed_str(name).map_err(|e| FxVatError::Path(e.to_string()))?;
            textures.push(load_context.load(path));
        }
        let [first, second]: [Handle<Image>; 2] = textures.try_into().expect("two textures");
        Ok(FxVat {
            mode,
            mesh: load_context.add_labeled_asset("mesh", mesh),
            textures: [first, second],
            frame_count: file.frame_count,
            fps: if file.fps > 0.0 { file.fps } else { 24.0 },
            element_count: file.element_count,
            width: file.width,
            rows_per_frame: file.rows_per_frame,
            bounds: (Vec3::from(file.bounds[0]), Vec3::from(file.bounds[1])),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["vat.json"]
    }
}

// The kind of bake, and its two textures' file names.
fn check(file: &VatFile) -> Result<(VatMode, [&str; 2]), FxVatError> {
    if file.format != "procedural-fx-vat" {
        return Err(FxVatError::Format(file.format.clone()));
    }
    // Version 1 had UVs this plugin doesn't read.
    if file.version != 2 {
        return Err(FxVatError::Version(file.version));
    }
    let (mode, roles) = match file.mode.as_str() {
        "soft" => (VatMode::Soft, ["offsets", "normals"]),
        "rigid" => (VatMode::Rigid, ["positions", "rotations"]),
        "fluid" => (VatMode::Fluid, ["positions", "normals"]),
        other => return Err(FxVatError::Mode(other.into())),
    };
    let name = |role: &'static str| file.rest.get(role).and_then(|v| v.as_str()).ok_or(FxVatError::Texture(role));
    let names = [name(roles[0])?, name(roles[1])?];
    if file.frame_count == 0 || file.element_count == 0 || file.width == 0 || file.rows_per_frame == 0 || file.element_count > file.width * file.rows_per_frame {
        return Err(FxVatError::Layout(format!(
            "{} elements, {} to a row over {} rows, {} frames",
            file.element_count, file.width, file.rows_per_frame, file.frame_count
        )));
    }
    Ok((mode, names))
}

// The bake's OBJ: positions, UVs and normals, and triangles (or polygons, fanned).
// Each distinct position/UV/normal corner becomes one vertex.
fn parse_obj(text: &str) -> Result<Mesh, FxVatError> {
    let (mut v, mut vt, mut vn) = (Vec::new(), Vec::new(), Vec::new());
    let (mut positions, mut uvs, mut normals) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 2]>::new(), Vec::<[f32; 3]>::new());
    let mut indices = Vec::<u32>::new();
    let mut corners = HashMap::<(usize, usize, usize), u32>::new();
    for (n, line) in text.lines().enumerate() {
        let bad = |problem: String| FxVatError::Obj { line: n + 1, problem };
        let mut words = line.split_whitespace();
        let numbers = |words: std::str::SplitWhitespace, want: usize| -> Result<Vec<f32>, FxVatError> {
            let values: Vec<f32> = words.take(want).map(|w| w.parse::<f32>().map_err(|e| bad(format!("{w}: {e}")))).collect::<Result<_, _>>()?;
            if values.len() < want { Err(bad(format!("expected {want} numbers"))) } else { Ok(values) }
        };
        match words.next() {
            Some("v") => v.push(numbers(words, 3)?),
            Some("vt") => vt.push(numbers(words, 2)?),
            Some("vn") => vn.push(numbers(words, 3)?),
            Some("f") => {
                let mut face = Vec::new();
                for corner in words {
                    let mut parts = corner.split('/');
                    let mut index = |list_len: usize| -> Result<Option<usize>, FxVatError> {
                        match parts.next().filter(|p| !p.is_empty()) {
                            None => Ok(None),
                            Some(p) => {
                                let i: i64 = p.parse().map_err(|e| bad(format!("{corner}: {e}")))?;
                                let i = if i < 0 { list_len as i64 + i } else { i - 1 };
                                if i < 0 || i as usize >= list_len { Err(bad(format!("{corner} is out of range"))) } else { Ok(Some(i as usize)) }
                            }
                        }
                    };
                    let p = index(v.len())?.ok_or_else(|| bad(format!("{corner} has no position")))?;
                    let t = index(vt.len())?.ok_or_else(|| bad(format!("{corner} has no UV, which picks its texels")))?;
                    let nn = index(vn.len())?;
                    let key = (p, t, nn.unwrap_or(usize::MAX));
                    let at = *corners.entry(key).or_insert_with(|| {
                        positions.push([v[p][0], v[p][1], v[p][2]]);
                        uvs.push([vt[t][0], vt[t][1]]);
                        normals.push(nn.map_or([0.0, 1.0, 0.0], |i| [vn[i][0], vn[i][1], vn[i][2]]));
                        (positions.len() - 1) as u32
                    });
                    face.push(at);
                }
                for k in 1..face.len().saturating_sub(1) {
                    indices.extend([face[0], face[k], face[k + 1]]);
                }
            }
            _ => {}
        }
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vat(frames: usize, fps: f32) -> FxVat {
        FxVat {
            mode: VatMode::Soft,
            mesh: Handle::default(),
            textures: [Handle::default(), Handle::default()],
            frame_count: frames,
            fps,
            element_count: 1,
            width: 1,
            rows_per_frame: 1,
            bounds: (Vec3::ZERO, Vec3::ONE),
        }
    }

    #[test]
    fn frames_follow_the_clock_and_blend() {
        let v = vat(4, 10.0);
        assert_eq!(v.duration(), 0.4);
        assert!((v.frame_at(0.25, Playback::Loop) - 2.5).abs() < 1e-5);
        assert!((v.frame_at(0.45, Playback::Loop) - 0.5).abs() < 1e-5);
        assert_eq!(v.frame_at(0.45, Playback::Once), 3.0);
    }

    #[test]
    fn objs_become_meshes_with_each_corners_uv() {
        let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0.25 0.5\nvt 0.75 0.5\nvn 0 0 1\nf 1/1/1 2/2/1 4/2/1 3/1/1\n";
        let mesh = parse_obj(text).unwrap();
        assert_eq!(mesh.count_vertices(), 4);
        assert_eq!(mesh.indices().unwrap().len(), 6, "a quad fans into two triangles");
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0) else { panic!("UVs") };
        assert_eq!(uvs[2], [0.75, 0.5]);
        assert!(matches!(parse_obj("v 0 0 0\nf 1 1 1\n"), Err(FxVatError::Obj { line: 2, .. })), "corners need UVs");
    }
}
