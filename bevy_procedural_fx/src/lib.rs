//! Play sprite-sheet effects exported from [ProceduralFX Studio](https://github.com/loksangho/proceduralfx-studio)
//! in Bevy.
//!
//! In ProceduralFX Studio, **Render → Export Sprite Sheet…** writes `name.png` and
//! `name.pfx.json`. Put both in your `assets` folder, add [`ProceduralFxPlugin`], and spawn
//! an [`FxSprite`] with the `.pfx.json` file:
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_procedural_fx::{FxSprite, ProceduralFxPlugin};
//!
//! fn main() {
//!     App::new()
//!         .add_plugins((DefaultPlugins, ProceduralFxPlugin))
//!         .add_systems(Startup, setup)
//!         .run();
//! }
//!
//! fn setup(mut commands: Commands, assets: Res<AssetServer>) {
//!     commands.spawn(Camera2d);
//!     commands.spawn(FxSprite::looping(assets.load("fx/fire.pfx.json")));
//! }
//! ```
//!
//! The sprite plays at the frame rate it was exported at. Its transform sits where the
//! centre of the full rendered frame was, even when the sheet's frames were cropped, so an
//! effect lines up the same way however it was trimmed.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::sprite::Anchor;
use serde::Deserialize;

/// Registers the [`FxSheet`] asset, its `.pfx.json` loader, and the system that plays
/// [`FxSprite`]s.
pub struct ProceduralFxPlugin;

impl Plugin for ProceduralFxPlugin {
    fn build(&self, app: &mut App) {
        // DefaultPlugins register atlas layouts; a minimal app may not have.
        if !app.world().contains_resource::<Assets<TextureAtlasLayout>>() {
            app.init_asset::<TextureAtlasLayout>();
        }
        app.init_asset::<FxSheet>().register_asset_loader(FxSheetLoader).add_systems(Update, play);
    }
}

/// A sprite sheet exported from ProceduralFX Studio.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct FxSheet {
    /// The packed frames.
    pub image: Handle<Image>,
    /// The grid of frames in `image`, in playing order.
    pub layout: Handle<TextureAtlasLayout>,
    pub frame_count: usize,
    pub fps: f32,
    /// Each frame's size in pixels.
    pub frame_size: UVec2,
    /// The size each frame was rendered at, before cropping.
    pub source_size: UVec2,
    /// Where each cropped frame's top-left corner was in the full frame.
    pub offset: UVec2,
}

impl FxSheet {
    /// How long one pass through the frames takes, in seconds.
    pub fn duration(&self) -> f32 {
        self.frame_count as f32 / self.fps.max(1e-3)
    }

    /// The anchor that puts the centre of the full rendered frame at the sprite's transform.
    pub fn anchor(&self) -> Anchor {
        let size = self.frame_size.as_vec2().max(Vec2::ONE);
        let centre = self.source_size.as_vec2() / 2.0 - self.offset.as_vec2();
        Anchor(Vec2::new(centre.x / size.x - 0.5, 0.5 - centre.y / size.y))
    }

    /// The frame shown `elapsed` seconds in.
    pub fn frame_at(&self, elapsed: f32, playback: Playback) -> usize {
        // The small nudge keeps 13/24 s on frame 13 despite rounding.
        let frame = (elapsed.max(0.0) * self.fps + 1e-4).floor() as usize;
        match playback {
            Playback::Loop => frame % self.frame_count.max(1),
            Playback::Once | Playback::OnceThenDespawn => frame.min(self.frame_count.saturating_sub(1)),
        }
    }
}

/// What happens after the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Playback {
    /// Start again from the first frame.
    #[default]
    Loop,
    /// Stay on the last frame.
    Once,
    /// Despawn the entity, for one-shot effects such as impacts and explosions.
    OnceThenDespawn,
}

/// Plays an [`FxSheet`] on this entity's [`Sprite`].
#[derive(Component, Clone, Debug, Reflect)]
#[require(Sprite)]
pub struct FxSprite {
    pub sheet: Handle<FxSheet>,
    pub playback: Playback,
    /// 1 plays at the exported frame rate, 2 twice as fast.
    pub speed: f32,
    /// Seconds played so far.
    pub elapsed: f32,
    pub paused: bool,
}

impl FxSprite {
    pub fn new(sheet: Handle<FxSheet>, playback: Playback) -> Self {
        FxSprite { sheet, playback, speed: 1.0, elapsed: 0.0, paused: false }
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
}

fn play(mut commands: Commands, time: Res<Time>, sheets: Res<Assets<FxSheet>>, mut sprites: Query<(Entity, &mut FxSprite, &mut Sprite, &mut Anchor)>) {
    for (entity, mut fx, mut sprite, mut anchor) in &mut sprites {
        // Not loaded yet: nothing to show, and the clock waits.
        let Some(sheet) = sheets.get(&fx.sheet) else { continue };
        if fx.playback == Playback::OnceThenDespawn && fx.elapsed >= sheet.duration() - 1e-4 {
            commands.entity(entity).despawn();
            continue;
        }
        let index = sheet.frame_at(fx.elapsed, fx.playback);
        // Set every frame, so a sheet re-exported while the game runs (with Bevy's
        // file_watcher feature) takes effect straight away.
        if sprite.image != sheet.image {
            sprite.image = sheet.image.clone();
        }
        match &mut sprite.texture_atlas {
            Some(atlas) if atlas.layout == sheet.layout => {
                if atlas.index != index {
                    atlas.index = index;
                }
            }
            _ => sprite.texture_atlas = Some(TextureAtlas { layout: sheet.layout.clone(), index }),
        }
        let wanted = sheet.anchor();
        if *anchor != wanted {
            *anchor = wanted;
        }
        // Each frame shows for its whole time, the first included.
        if !fx.paused {
            fx.elapsed += time.delta_secs() * fx.speed;
        }
    }
}

// The .pfx.json file, as ProceduralFX Studio writes it.
#[derive(Deserialize)]
struct SheetFile {
    format: String,
    version: u32,
    image: String,
    frame_size: [u32; 2],
    columns: u32,
    rows: u32,
    frame_count: usize,
    fps: f32,
    #[serde(default)]
    source_size: Option<[u32; 2]>,
    #[serde(default)]
    offset: [u32; 2],
}

/// Why a `.pfx.json` file couldn't be loaded.
#[derive(Debug, thiserror::Error)]
pub enum FxSheetError {
    #[error("couldn't read the sprite sheet: {0}")]
    Io(#[from] std::io::Error),
    #[error("couldn't parse the sprite sheet: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a ProceduralFX Studio sprite sheet (format \"{0}\")")]
    Format(String),
    #[error("sprite sheet version {0} is newer than this plugin understands; update bevy_procedural_fx")]
    Version(u32),
    #[error("the sprite sheet has {frames} frames, more than its {columns}×{rows} grid holds")]
    Grid { frames: usize, columns: u32, rows: u32 },
    #[error("couldn't find the sprite sheet's image path: {0}")]
    Path(String),
}

/// Loads `.pfx.json` sprite sheets; the image they name loads beside them.
#[derive(Default, TypePath)]
pub struct FxSheetLoader;

impl AssetLoader for FxSheetLoader {
    type Asset = FxSheet;
    type Settings = ();
    type Error = FxSheetError;

    async fn load(&self, reader: &mut dyn Reader, _settings: &(), load_context: &mut LoadContext<'_>) -> Result<FxSheet, FxSheetError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let file: SheetFile = serde_json::from_slice(&bytes)?;
        parse(file, load_context)
    }

    fn extensions(&self) -> &[&str] {
        &["pfx.json"]
    }
}

fn parse(file: SheetFile, load_context: &mut LoadContext) -> Result<FxSheet, FxSheetError> {
    if file.format != "procedural-fx-sprite-sheet" {
        return Err(FxSheetError::Format(file.format));
    }
    if file.version > 1 {
        return Err(FxSheetError::Version(file.version));
    }
    let (columns, rows) = (file.columns.max(1), file.rows.max(1));
    if file.frame_count > (columns * rows) as usize {
        return Err(FxSheetError::Grid { frames: file.frame_count, columns, rows });
    }
    let frame_size = UVec2::from(file.frame_size);
    let mut layout = TextureAtlasLayout::from_grid(frame_size, columns, rows, None, None);
    layout.textures.truncate(file.frame_count.max(1));
    let image_path = load_context.path().resolve_embed_str(&file.image).map_err(|e| FxSheetError::Path(e.to_string()))?;
    Ok(FxSheet {
        image: load_context.load(image_path),
        layout: load_context.add_labeled_asset("layout", layout),
        frame_count: file.frame_count.max(1),
        fps: if file.fps > 0.0 { file.fps } else { 24.0 },
        frame_size,
        source_size: file.source_size.map_or(frame_size, UVec2::from),
        offset: UVec2::from(file.offset),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(frames: usize, fps: f32) -> FxSheet {
        FxSheet {
            image: Handle::default(),
            layout: Handle::default(),
            frame_count: frames,
            fps,
            frame_size: UVec2::new(40, 20),
            source_size: UVec2::new(100, 100),
            offset: UVec2::new(30, 40),
        }
    }

    #[test]
    fn frames_follow_the_clock() {
        let s = sheet(4, 10.0);
        assert_eq!(s.duration(), 0.4);
        assert_eq!(s.frame_at(0.0, Playback::Loop), 0);
        assert_eq!(s.frame_at(0.25, Playback::Loop), 2);
        assert_eq!(s.frame_at(0.45, Playback::Loop), 0);
        assert_eq!(s.frame_at(0.45, Playback::Once), 3);
    }

    #[test]
    fn the_anchor_keeps_the_full_frames_centre() {
        // Cropped to x 30..70, y 40..60 of a 100×100 frame: the centre (50, 50) is the
        // middle of the cell, so the anchor is (0, 0).
        assert_eq!(sheet(1, 24.0).anchor(), Anchor(Vec2::new(0.0, 0.0)));
        let mut s = sheet(1, 24.0);
        s.offset = UVec2::new(50, 50);
        // Now the centre is the cell's top-left corner.
        assert_eq!(s.anchor(), Anchor(Vec2::new(-0.5, 0.5)));
    }
}
