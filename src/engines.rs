// The game engine an export is for: which files the export windows write for it, and a
// short note on using them there (for Bevy, the bevy_procedural_fx crate and a snippet to
// copy).

pub const BEVY_CRATE: &str = "https://crates.io/crates/bevy_procedural_fx";
pub const BEVY_DOCS: &str = "https://docs.rs/bevy_procedural_fx";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Bevy,
    Godot,
    Unity,
    // Just the image (or mesh and textures) and the JSON beside it.
    Other,
}

impl Engine {
    pub const ALL: [Engine; 4] = [Engine::Bevy, Engine::Godot, Engine::Unity, Engine::Other];

    pub fn name(self) -> &'static str {
        match self {
            Engine::Bevy => "Bevy",
            Engine::Godot => "Godot 4",
            Engine::Unity => "Unity (URP)",
            Engine::Other => "Other",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Engine::Bevy => "Plays with the bevy_procedural_fx crate",
            Engine::Godot => "Also writes Godot resources (.tres, shaders) beside the files",
            Engine::Unity => "Also writes Unity import settings, a shader and a material",
            Engine::Other => "Only the files themselves, with a JSON description any engine can read",
        }
    }

    // Whether the export writes Godot's and Unity's extra files.
    pub fn writes(self) -> (bool, bool) {
        (self == Engine::Godot, self == Engine::Unity)
    }
}

// What a sprite sheet export needs on the engine's side: a few steps, and code to copy
// (Bevy). `stem` is the file name without its extension ("fire" for fire.png).
pub fn sheet_note(engine: Engine, stem: &str, s: &crate::spritesheet::Settings) -> (Vec<String>, Option<String>) {
    match engine {
        Engine::Bevy => {
            let flipbook = s.motion_vectors || s.glow || s.normal_map;
            let mut steps = vec![
                format!("Add bevy_procedural_fx to your game's Cargo.toml (cargo add bevy_procedural_fx), then export into its assets/fx folder: {stem}.png and {stem}.pfx.json."),
                if flipbook {
                    "FxFlipbook plays it with the extras ticked here (motion vectors, glow, normal map).".to_owned()
                } else {
                    "FxSprite plays it as a Sprite. Tick Motion vectors, Glow sheet or Normal map for FxFlipbook's smoother, glowing or lit playback.".to_owned()
                },
            ];
            if s.live {
                steps.push("Keep updating: turn on Bevy's file_watcher feature and the running game follows each export.".to_owned());
            }
            let spawn = match (flipbook, s.glow) {
                (true, true) => format!("    commands.spawn((Camera2d, Hdr, Bloom::default()));\n    commands.spawn(FxFlipbook::looping(assets.load(\"fx/{stem}.pfx.json\")).with_glow(1.0));"),
                (true, false) => format!("    commands.spawn(Camera2d);\n    commands.spawn(FxFlipbook::looping(assets.load(\"fx/{stem}.pfx.json\")));"),
                _ => format!("    commands.spawn(Camera2d);\n    commands.spawn(FxSprite::looping(assets.load(\"fx/{stem}.pfx.json\")));"),
            };
            let (component, extra) = match (flipbook, s.glow) {
                (true, true) => ("FxFlipbook", "\nuse bevy::camera::Hdr;\nuse bevy::post_process::bloom::Bloom;"),
                (true, false) => ("FxFlipbook", ""),
                _ => ("FxSprite", ""),
            };
            let code = format!(
                "use bevy::prelude::*;\nuse bevy_procedural_fx::{{{component}, ProceduralFxPlugin}};{extra}\n\nfn main() {{\n    App::new()\n        .add_plugins((DefaultPlugins, ProceduralFxPlugin))\n        .add_systems(Startup, setup)\n        .run();\n}}\n\nfn setup(mut commands: Commands, assets: Res<AssetServer>) {{\n{spawn}\n}}\n"
            );
            (steps, Some(code))
        }
        Engine::Godot => {
            let mut steps = vec![format!("Export into your Godot project. Put {stem}.tres (SpriteFrames) on an AnimatedSprite2D: it plays the sheet, looping, at its frame rate.")];
            if s.fill_grid {
                steps.push(format!("For particles: {stem}_particles.tres is a material for GPUParticles2D, with the sheet as its texture."));
            }
            if s.motion_vectors {
                steps.push(format!("Smooth playback: {stem}_flipbook.tscn is a node using {stem}_flipbook.gdshader, which slides along the motion vectors."));
            }
            if s.normal_map {
                steps.push(format!("Lit by Light2D: {stem}_sprite.tres gives the AnimatedSprite2D the normal map."));
            }
            (steps, None)
        }
        Engine::Unity => {
            let mut steps = vec![format!("Export into your project's Assets folder. {stem}.png.meta makes Unity slice it into one sprite per frame ({stem}_0, {stem}_1...): select them and drag them into the scene to make an animation.")];
            if s.fill_grid {
                steps.push("For particles: Texture Sheet Animation in Grid mode, with the columns and rows from the sheet.".to_owned());
            }
            if s.motion_vectors {
                steps.push(format!("Smooth playback: {stem}_flipbook.mat, with its shader, slides along the motion vectors."));
            }
            (steps, None)
        }
        Engine::Other => (vec![format!("{stem}.pfx.json describes the grid: frame size, columns and rows, frame count and frame rate (the format is in the bevy_procedural_fx README).")], None),
    }
}

// What a vertex animation export needs on the engine's side.
pub fn vat_note(engine: Engine, stem: &str) -> (Vec<String>, Option<String>) {
    match engine {
        Engine::Bevy => (
            vec![format!("Add bevy_procedural_fx to your game's Cargo.toml (cargo add bevy_procedural_fx), then export into its assets/fx folder: {stem}.obj, its textures and {stem}.vat.json.")],
            Some(format!(
                "use bevy::prelude::*;\nuse bevy_procedural_fx::{{FxMesh, ProceduralFxPlugin}};\n\nfn main() {{\n    App::new()\n        .add_plugins((DefaultPlugins, ProceduralFxPlugin))\n        .add_systems(Startup, setup)\n        .run();\n}}\n\nfn setup(mut commands: Commands, assets: Res<AssetServer>) {{\n    commands.spawn((Camera3d::default(), Transform::from_xyz(0.0, 2.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y)));\n    commands.spawn((DirectionalLight::default(), Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y)));\n    commands.spawn(FxMesh::looping(assets.load(\"fx/{stem}.vat.json\")));\n}}\n"
            )),
        ),
        Engine::Godot => (vec![format!("Export into your Godot project and open {stem}_vat.tscn: a MeshInstance3D with {stem}.obj and {stem}_vat_material.tres, playing the bake.")], None),
        Engine::Unity => (vec![format!("Export into your project's Assets folder. Put {stem}.obj in the scene and {stem}_vat.mat on it. Needs the Linear colour space (URP's default).")], None),
        Engine::Other => (vec![format!("{stem}.vat.json describes the textures' layout and frame rate (the format is in the bevy_procedural_fx README).")], None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bevy_snippet_names_the_files_and_the_right_component() {
        let mut s = crate::spritesheet::Settings::default();
        let (_, code) = sheet_note(Engine::Bevy, "fire", &s);
        let code = code.unwrap();
        assert!(code.contains("FxSprite::looping(assets.load(\"fx/fire.pfx.json\"))") && code.contains("use bevy_procedural_fx::{FxSprite, ProceduralFxPlugin};"), "{code}");
        s.glow = true;
        let code = sheet_note(Engine::Bevy, "fire", &s).1.unwrap();
        assert!(code.contains("FxFlipbook::looping") && code.contains("with_glow") && code.contains("Bloom"), "{code}");
        assert!(vat_note(Engine::Bevy, "wave").1.unwrap().contains("FxMesh::looping(assets.load(\"fx/wave.vat.json\"))"));
        assert!(sheet_note(Engine::Godot, "fire", &s).1.is_none());
    }

    // Writes the snippets where PFX_SNIPPETS says, to build them against the crate:
    // `PFX_SNIPPETS=bevy_procedural_fx/examples cargo test --release -- --ignored snippets`.
    #[test]
    #[ignore]
    fn write_the_snippets() {
        let dir = std::path::PathBuf::from(std::env::var("PFX_SNIPPETS").unwrap());
        let mut s = crate::spritesheet::Settings::default();
        std::fs::write(dir.join("snippet_sprite.rs"), sheet_note(Engine::Bevy, "fire", &s).1.unwrap()).unwrap();
        s.glow = true;
        std::fs::write(dir.join("snippet_glow.rs"), sheet_note(Engine::Bevy, "fire", &s).1.unwrap()).unwrap();
        s.glow = false;
        s.motion_vectors = true;
        std::fs::write(dir.join("snippet_flipbook.rs"), sheet_note(Engine::Bevy, "fire", &s).1.unwrap()).unwrap();
        std::fs::write(dir.join("snippet_vat.rs"), vat_note(Engine::Bevy, "wave").1.unwrap()).unwrap();
    }

    #[test]
    fn engines_choose_their_files() {
        assert_eq!(Engine::Godot.writes(), (true, false));
        assert_eq!(Engine::Unity.writes(), (false, true));
        assert_eq!(Engine::Bevy.writes(), (false, false));
    }
}
