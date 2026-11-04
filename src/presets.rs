// Starter effects: whole scenes to load and tweak rather than build from scratch. Each is
// an ordinary .pfs scene (assets/presets) with a camera node framing it, plus how it's
// meant to be exported: its playback range, frame size, whether it loops, and whether it's
// for 2D games (a sprite sheet) or 3D ones (vertex animation). The AI assistant can load
// them too, and customise them.

pub struct Preset {
    // Short name, as on the menu and for the assistant's load_preset.
    pub name: &'static str,
    pub description: &'static str,
    pub scene: &'static str,
    // The playback range to export, in seconds.
    pub range: (f32, f32),
    // Sprite-sheet frame size, in pixels.
    pub frame: [u32; 2],
    // Loops (exported as a seamless loop) rather than playing once.
    pub looping: bool,
    // For 3D games: exported with Export Vertex Animation rather than as a sprite sheet.
    pub vertex_animation: bool,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Fire",
        description: "A looping campfire flame: a Pyro simulation burning fuel from a small source, looped by a Make Loop node.",
        scene: include_str!("../assets/presets/fire.pfs"),
        range: (2.0, 3.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
        vertex_animation: false,
    },
    Preset {
        name: "Toon fire",
        description: "A looping cartoon campfire: the Fire simulation meshed into solid tongues of flame (one simulation, shown three times a beat apart), coloured in flat yellow, orange and red bands with smoky tips, cel-shaded and outlined by the Toon FX shader.",
        scene: include_str!("../assets/presets/toon_fire.pfs"),
        range: (2.0, 3.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
        vertex_animation: false,
    },
    Preset {
        name: "Smoke",
        description: "A looping wisp of soft grey smoke: a Pyro simulation rising from a small source, curling and fading, looped by a Make Loop node.",
        scene: include_str!("../assets/presets/smoke.pfs"),
        range: (2.0, 3.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
        vertex_animation: false,
    },
    Preset {
        name: "Impact",
        description: "A one-shot hit: a bright flash and a fan of sparks that fly out, slow, fall and fade.",
        scene: include_str!("../assets/presets/impact.pfs"),
        range: (0.0, 0.5),
        frame: [256, 192],
        looping: false,
        vertex_animation: false,
    },
    Preset {
        name: "Magic burst",
        description: "A one-shot spell: glowing motes spiral out from a swelling core, in violet and cyan.",
        scene: include_str!("../assets/presets/magic_burst.pfs"),
        range: (0.0, 0.75),
        frame: [256, 256],
        looping: false,
        vertex_animation: false,
    },
    Preset {
        name: "Slash",
        description: "A one-shot sword slash: a bright arc that sweeps across, trails off and fades.",
        scene: include_str!("../assets/presets/slash.pfs"),
        range: (0.0, 0.4),
        frame: [256, 256],
        looping: false,
        vertex_animation: false,
    },
    // 3D: made by three_d_presets in main.rs.
    Preset {
        name: "Flag",
        description: "A looping flag for 3D games: a cloth pinned along one edge, rippling in waves that grow towards its free edge. Exports as soft vertex animation.",
        scene: include_str!("../assets/presets/flag.pfs"),
        range: (0.0, 1.0 - 1.0 / 24.0),
        frame: [256, 256],
        looping: true,
        vertex_animation: true,
    },
    Preset {
        name: "Breaking wall",
        description: "A one-shot for 3D games: a stone wall struck in the middle breaks into 24 pieces that fly back and tumble into rubble. Exports as rigid vertex animation.",
        scene: include_str!("../assets/presets/breaking_wall.pfs"),
        range: (0.0, 2.0),
        frame: [256, 256],
        looping: false,
        vertex_animation: true,
    },
    Preset {
        name: "Splash",
        description: "A one-shot for 3D games: a blob of liquid thrown up that lands and spreads into a puddle, meshed by Particle Fluid Surface. Exports as fluid vertex animation, kept coarse so its textures stay small.",
        scene: include_str!("../assets/presets/splash.pfs"),
        range: (0.0, 1.0),
        frame: [256, 256],
        looping: false,
        vertex_animation: true,
    },
];

pub fn find(name: &str) -> Option<&'static Preset> {
    let name = name.trim();
    PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}
