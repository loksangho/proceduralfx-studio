// Starter effects for 2D games: whole scenes to load and tweak rather than build from
// scratch. Each is an ordinary .pfs scene (assets/presets) with a camera node framing it
// from the side, plus how it's meant to be exported: its playback range, frame size, and
// whether it loops. The AI assistant can load them too, and customise them.

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
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Fire",
        description: "A looping campfire flame: a Pyro simulation burning fuel from a small source, looped by a Make Loop node.",
        scene: include_str!("../assets/presets/fire.pfs"),
        range: (2.0, 3.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
    },
    Preset {
        name: "Toon fire",
        description: "A looping cartoon flame: the Fire simulation meshed into a solid shape, coloured in flat yellow, orange and red bands with a grey smoky tip, cel-shaded and outlined by the Toon FX shader.",
        scene: include_str!("../assets/presets/toon_fire.pfs"),
        range: (2.0, 3.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
    },
    Preset {
        name: "Smoke",
        description: "A looping wisp of soft grey smoke: puffs that rise, spread, sway and fade.",
        scene: include_str!("../assets/presets/smoke.pfs"),
        range: (0.0, 1.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
    },
    Preset {
        name: "Impact",
        description: "A one-shot hit: a bright flash and a fan of sparks that fly out, slow, fall and fade.",
        scene: include_str!("../assets/presets/impact.pfs"),
        range: (0.0, 0.5),
        frame: [256, 192],
        looping: false,
    },
    Preset {
        name: "Magic burst",
        description: "A one-shot spell: glowing motes spiral out from a swelling core, in violet and cyan.",
        scene: include_str!("../assets/presets/magic_burst.pfs"),
        range: (0.0, 0.75),
        frame: [256, 256],
        looping: false,
    },
    Preset {
        name: "Slash",
        description: "A one-shot sword slash: a bright arc that sweeps across, trails off and fades.",
        scene: include_str!("../assets/presets/slash.pfs"),
        range: (0.0, 0.4),
        frame: [256, 256],
        looping: false,
    },
];

pub fn find(name: &str) -> Option<&'static Preset> {
    let name = name.trim();
    PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}
