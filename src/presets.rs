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
    // A few plain-language sliders shown above the Parameters (see Control), so the effect
    // can be changed without opening the network.
    pub controls: &'static [Control],
    // A picture of it, for the welcome screen: its picture test's (app_tests/preset_pictures).
    pub picture: &'static [u8],
}

// One of a preset's simple controls: a slider that sets one parameter of one node inside it.
pub struct Control {
    pub label: &'static str,
    pub help: &'static str,
    // The node, by its name, or "#<id>" for one the preset leaves unnamed.
    pub node: &'static str,
    // A parameter (as docs/scene-nodes.md names them) or one of a wrangle's ch() channels.
    pub param: &'static str,
    pub range: (f32, f32),
    // Whole numbers only (a count).
    pub whole: bool,
}

const fn c(label: &'static str, help: &'static str, node: &'static str, param: &'static str, range: (f32, f32)) -> Control {
    Control { label, help, node, param, range, whole: false }
}

const fn count(label: &'static str, help: &'static str, node: &'static str, param: &'static str, range: (f32, f32)) -> Control {
    Control { label, help, node, param, range, whole: true }
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
        controls: &[
            c("Size", "How big the fire is: the source it burns from", "source", "radius", (0.1, 0.6)),
            c("Height", "How fast the flames rise, so how tall they get", "fire_sim", "buoyancy", (0.4, 3.0)),
            c("Flicker", "How much the flames waver and curl", "fire_sim", "turbulence", (0.0, 1.5)),
            c("Smoke", "How much smoke it gives off", "fire_sim", "density", (0.0, 0.5)),
            c("Wind", "A breeze from the side (negative: from the right)", "fire_sim", "wind.x", (-2.0, 2.0)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/fire.png"),
    },
    Preset {
        name: "Toon fire",
        description: "A looping cartoon campfire: the Fire simulation meshed into solid tongues of flame (one simulation, shown three times a beat apart), coloured in flat yellow, orange and red bands with smoky tips, cel-shaded and outlined by the Toon FX shader.",
        scene: include_str!("../assets/presets/toon_fire.pfs"),
        range: (2.0, 3.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
        vertex_animation: false,
        controls: &[
            c("Size", "How big the fire is: the source it burns from", "source", "radius", (0.1, 0.6)),
            c("Height", "How fast the flames rise, so how tall they get", "fire_sim", "buoyancy", (0.4, 3.0)),
            c("Flicker", "How much the flames waver and curl", "fire_sim", "turbulence", (0.0, 1.5)),
            c("Wind", "A breeze from the side (negative: from the right)", "fire_sim", "wind.x", (-2.0, 2.0)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/toon_fire.png"),
    },
    Preset {
        name: "Smoke",
        description: "A looping wisp of soft grey smoke: a Pyro simulation rising from a small source, curling and fading, looped by a Make Loop node.",
        scene: include_str!("../assets/presets/smoke.pfs"),
        range: (2.0, 3.0 - 1.0 / 24.0),
        frame: [192, 256],
        looping: true,
        vertex_animation: false,
        controls: &[
            c("Size", "How wide the smoke starts: the source it rises from", "source", "radius", (0.05, 0.4)),
            c("Rise", "How fast it rises", "smoke_sim", "buoyancy", (0.3, 2.5)),
            c("Swirl", "How much it curls and spreads", "smoke_sim", "turbulence", (0.0, 1.5)),
            c("Thickness", "How dense the smoke is", "smoke_sim", "density", (0.1, 1.5)),
            c("Fade", "How quickly it thins out as it rises", "smoke_sim", "dissipation", (0.2, 3.0)),
            c("Wind", "A breeze from the side (negative: from the right)", "smoke_sim", "wind.x", (-2.0, 2.0)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/smoke.png"),
    },
    Preset {
        name: "Impact",
        description: "A one-shot hit: a bright flash and a fan of sparks that fly out, slow, fall and fade.",
        scene: include_str!("../assets/presets/impact.pfs"),
        range: (0.0, 0.5),
        frame: [256, 192],
        looping: false,
        vertex_animation: false,
        controls: &[
            count("Sparks", "How many sparks fly out", "impact_points", "count", (10.0, 200.0)),
            c("Speed", "How fast they fly out, so how far they go", "impact_points", "speed", (2.0, 20.0)),
            c("Life", "How long each spark lasts, in seconds", "impact_points", "life", (0.1, 1.5)),
            c("Spark size", "How big each spark is", "impact_points", "size", (0.01, 0.08)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/impact.png"),
    },
    Preset {
        name: "Magic burst",
        description: "A one-shot spell: glowing motes spiral out from a swelling core, in violet and cyan.",
        scene: include_str!("../assets/presets/magic_burst.pfs"),
        range: (0.0, 0.75),
        frame: [256, 256],
        looping: false,
        vertex_animation: false,
        controls: &[
            count("Motes", "How many glowing motes there are", "magic_burst_points", "count", (20.0, 400.0)),
            c("Radius", "How far they spiral out", "magic_burst_points", "radius", (0.4, 3.0)),
            c("Duration", "How long the burst lasts, in seconds", "magic_burst_points", "duration", (0.2, 2.0)),
            c("Swirl", "How much they spiral as they go", "magic_burst_points", "swirl", (0.0, 4.0)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/magic_burst.png"),
    },
    Preset {
        name: "Slash",
        description: "A one-shot sword slash: a bright arc that sweeps across, trails off and fades.",
        scene: include_str!("../assets/presets/slash.pfs"),
        range: (0.0, 0.4),
        frame: [256, 256],
        looping: false,
        vertex_animation: false,
        controls: &[
            c("Duration", "How long the swing takes, in seconds", "slash_points", "duration", (0.08, 0.6)),
            c("Radius", "How far the arc reaches", "slash_points", "radius", (0.4, 2.5)),
            c("Arc", "How far round it sweeps, in degrees", "slash_points", "arc", (60.0, 300.0)),
            c("Width", "How thick the trail is", "slash_points", "width", (0.05, 0.6)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/slash.png"),
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
        controls: &[
            c("Wave height", "How far the cloth ripples", "wave", "amplitude", (0.0, 0.5)),
            c("Waves", "How many ripples run along it", "wave", "waves", (0.5, 4.0)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/flag.png"),
    },
    Preset {
        name: "Breaking wall",
        description: "A one-shot for 3D games: a stone wall struck in the middle breaks into 24 pieces that fly back and tumble into rubble. Exports as rigid vertex animation.",
        scene: include_str!("../assets/presets/breaking_wall.pfs"),
        range: (0.0, 2.0),
        frame: [256, 256],
        looping: false,
        vertex_animation: true,
        controls: &[
            c("Blast force", "How hard the pieces are thrown", "blast", "force", (0.0, 8.0)),
            c("Blast radius", "How much of the wall the blast breaks loose", "blast", "radius", (0.2, 2.0)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/breaking_wall.png"),
    },
    Preset {
        name: "Splash",
        description: "A one-shot for 3D games: a blob of liquid thrown up that lands and spreads into a puddle, meshed by Particle Fluid Surface. Exports as fluid vertex animation, kept coarse so its textures stay small.",
        scene: include_str!("../assets/presets/splash.pfs"),
        range: (0.0, 1.0),
        frame: [256, 256],
        looping: false,
        vertex_animation: true,
        controls: &[
            c("Height", "How high it splashes", "#5", "velocity.y", (1.0, 6.0)),
            c("Thickness", "How thick the water's surface is around the drops", "#9", "particle_radius", (0.06, 0.25)),
        ],
        picture: include_bytes!("app_tests/preset_pictures/splash.png"),
    },
];

pub fn find(name: &str) -> Option<&'static Preset> {
    let name = name.trim();
    PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

impl Control {
    // The node the control changes, if the scene still has it.
    fn target(&self, graph: &crate::Graph) -> Option<crate::NodeId> {
        match self.node.strip_prefix('#') {
            Some(id) => id.parse().ok().filter(|id| graph.nodes.contains_key(id)),
            None => graph.nodes.iter().filter(|(_, n)| n.name == self.node).map(|(id, _)| *id).min(),
        }
    }

    // What the control reads now, or None if its node or parameter is gone.
    pub fn value(&self, graph: &crate::Graph) -> Option<f32> {
        let node = graph.nodes.get(&self.target(graph)?)?;
        if let Some(v) = node.channels.get(self.param) {
            return Some(v[0]);
        }
        let i = node.kind.param_names().iter().position(|n| n == self.param)?;
        node.kind.params().get(i).copied()
    }

    // Sets it (clamped to its range); false if its node or parameter is gone.
    pub fn set(&self, graph: &mut crate::Graph, value: f32) -> bool {
        let value = value.clamp(self.range.0, self.range.1);
        let value = if self.whole { value.round() } else { value };
        let Some(id) = self.target(graph) else { return false };
        let Some(node) = graph.nodes.get_mut(&id) else { return false };
        if let Some(v) = node.channels.get_mut(self.param) {
            v[0] = value;
            return true;
        }
        let Some(i) = node.kind.param_names().iter().position(|n| n == self.param) else { return false };
        let mut params = node.kind.params();
        params[i] = value;
        let texts = node.kind.texts();
        let Some(mut rebuilt) = crate::NodeKind::from_params(node.kind.name(), &params) else { return false };
        rebuilt.set_texts(&texts);
        node.kind = rebuilt;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every preset's controls find their node and parameter, read a value inside their range,
    // and change the scene when moved.
    #[test]
    fn every_control_reaches_its_parameter() {
        for preset in PRESETS {
            assert!(!preset.controls.is_empty(), "{} has controls", preset.name);
            assert!(image::load_from_memory(preset.picture).is_ok(), "{}'s picture", preset.name);
            for control in preset.controls {
                let mut graph = crate::Graph::deserialize(preset.scene);
                let now = control.value(&graph).unwrap_or_else(|| panic!("{}: {} ({} {}) isn't in the scene", preset.name, control.label, control.node, control.param));
                assert!(now >= control.range.0 - 1e-4 && now <= control.range.1 + 1e-4, "{}: {} is {now}, outside {:?}", preset.name, control.label, control.range);
                let before = graph.serialize();
                let other = if (now - control.range.1).abs() > 1e-3 { control.range.1 } else { control.range.0 };
                assert!(control.set(&mut graph, other));
                assert!((control.value(&graph).unwrap() - other).abs() < 1e-3, "{}: {}", preset.name, control.label);
                assert_ne!(graph.serialize(), before, "{}: {} changes the scene", preset.name, control.label);
            }
        }
    }

    #[test]
    fn controls_stay_in_range_and_whole() {
        let impact = find("Impact").unwrap();
        let sparks = impact.controls.iter().find(|c| c.label == "Sparks").unwrap();
        let mut graph = crate::Graph::deserialize(impact.scene);
        sparks.set(&mut graph, 1000.4);
        assert_eq!(sparks.value(&graph), Some(200.0));
        sparks.set(&mut graph, 42.6);
        assert_eq!(sparks.value(&graph), Some(43.0));
    }
}
