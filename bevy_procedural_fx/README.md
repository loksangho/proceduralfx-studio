# bevy_procedural_fx

Play sprite-sheet effects exported from [ProceduralFX Studio](https://github.com/loksangho/proceduralfx-studio) in [Bevy](https://bevy.org).

Build an effect in ProceduralFX Studio, choose **Render → Export Sprite Sheet…**, and drop the
two files it writes into your game's `assets` folder:

- `fire.png`: the frames, packed in a grid, with a transparent background
- `fire.pfx.json`: the grid, frame count and frame rate

Then add the plugin and spawn an `FxSprite`:

```rust
use bevy::prelude::*;
use bevy_procedural_fx::{FxSprite, ProceduralFxPlugin};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ProceduralFxPlugin))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn(Camera2d);
    commands.spawn(FxSprite::looping(assets.load("fx/fire.pfx.json")));
}
```

## Playing effects

| | |
| --- | --- |
| `FxSprite::looping(sheet)` | Plays over and over, for fires, auras and ambient effects. |
| `FxSprite::once(sheet)` | Plays once and holds the last frame. |
| `FxSprite::one_shot(sheet)` | Plays once, then despawns the entity, for impacts and explosions. |
| `.with_speed(2.0)` | Plays at twice the exported frame rate. |

`FxSprite` adds a `Sprite`, so tint, flip and size it as usual, and place it with a
`Transform`. You can change `paused`, `speed` and `elapsed` while it plays.

The sprite's transform sits where the centre of the full rendered frame was, even when
the frames were cropped on export. An effect therefore lines up the same way however much
empty space was trimmed.

## Changing effects while the game runs

With Bevy's `file_watcher` feature on, exporting the sheet again to the same files
updates every sprite that uses it, without restarting the game:

```toml
bevy = { version = "0.20", features = ["file_watcher"] }
```

## Example

```sh
cargo run --example fire
```

This shows three fires from one sheet, playing at different speeds. Click to set off
one-shot bursts.

## The file format

`.pfx.json` is plain JSON. Other engines can read it too:

| Field | Meaning |
| --- | --- |
| `format`, `version` | `"procedural-fx-sprite-sheet"`, `1` |
| `image` | The PNG's file name, relative to the JSON file |
| `frame_size` | Each frame's width and height in pixels |
| `columns`, `rows` | The grid, filled left to right, top to bottom |
| `frame_count`, `fps` | How many frames there are, and how fast they play |
| `frames` | Each frame's `[x, y, width, height]` in the PNG |
| `source_size`, `offset` | The size each frame was rendered at, and where the cropped frame's top-left corner was in it |

## Bevy versions

| bevy_procedural_fx | Bevy |
| --- | --- |
| 0.1 | 0.20 |

## License

The plugin is MIT or Apache-2.0, at your option, the same as Bevy. ProceduralFX Studio
itself has [its own license](https://github.com/loksangho/proceduralfx-studio/blob/main/LICENSE).
Effects you make with it are yours.
