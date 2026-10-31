// MAT shaders and the Material Builder.
//
// - Principled Shader: one physically based surface (DwaBaseMaterial in
//   MoonRay) covering metals, glass and water, skin, cloth, clearcoat and
//   emission, with presets for common materials.
// - Pyro Shader: smoke and fire for volumes (VdbVolume).
// - Hair Shader: groom curves (HairMaterial_v3).
// - Material Builder: a network of texture nodes (image, noise, ramp, mix…)
//   wired into a Surface Output, compiled to a MoonRay map network.
//
// Parameters live in flat f32 tables (colours take three slots) described by
// PRINCIPLED, PYRO and HAIR, so saving, the UI and presets are generic.

use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Float(f32, f32),
    Color,
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub kind: Kind,
    pub default: [f32; 3],
}

pub(crate) const fn f(key: &'static str, label: &'static str, group: &'static str, min: f32, max: f32, value: f32) -> Param {
    Param { key, label, group, kind: Kind::Float(min, max), default: [value, 0.0, 0.0] }
}
pub(crate) const fn c(key: &'static str, label: &'static str, group: &'static str, value: [f32; 3]) -> Param {
    Param { key, label, group, kind: Kind::Color, default: value }
}
pub(crate) const fn t(key: &'static str, label: &'static str, group: &'static str, on: bool) -> Param {
    Param { key, label, group, kind: Kind::Toggle, default: [on as u8 as f32, 0.0, 0.0] }
}

impl Param {
    pub fn width(&self) -> usize {
        if self.kind == Kind::Color { 3 } else { 1 }
    }
}

pub const PRINCIPLED: &[Param] = &[
    c("base_color", "Base color", "Base", [0.8, 0.8, 0.8]),
    t("use_point_color", "Use point colour (Cd)", "Base", false),
    f("metallic", "Metallic", "Base", 0.0, 1.0, 0.0),
    f("roughness", "Roughness", "Base", 0.0, 1.0, 0.4),
    f("specular", "Specular", "Base", 0.0, 1.0, 1.0),
    f("ior", "IOR", "Base", 1.0, 3.0, 1.5),
    f("transmission", "Transmission", "Transmission", 0.0, 1.0, 0.0),
    c("transmission_color", "Transmission color", "Transmission", [1.0, 1.0, 1.0]),
    t("thin_walled", "Thin walled (bubbles, leaves)", "Transmission", false),
    f("subsurface", "Subsurface", "Subsurface", 0.0, 1.0, 0.0),
    c("subsurface_color", "Scatter color", "Subsurface", [1.0, 0.45, 0.3]),
    f("subsurface_radius", "Scatter distance", "Subsurface", 0.0, 10.0, 1.0),
    f("sheen", "Sheen", "Sheen", 0.0, 1.0, 0.0),
    c("sheen_color", "Sheen color", "Sheen", [1.0, 1.0, 1.0]),
    f("sheen_roughness", "Sheen roughness", "Sheen", 0.0, 1.0, 0.3),
    f("clearcoat", "Clearcoat", "Clearcoat", 0.0, 1.0, 0.0),
    f("clearcoat_roughness", "Clearcoat roughness", "Clearcoat", 0.0, 1.0, 0.05),
    c("emission_color", "Emission color", "Emission", [1.0, 1.0, 1.0]),
    f("emission_intensity", "Emission intensity", "Emission", 0.0, 100.0, 0.0),
    f("opacity", "Opacity", "Other", 0.0, 1.0, 1.0),
    f("anisotropy", "Anisotropy", "Other", -1.0, 1.0, 0.0),
    f("iridescence", "Iridescence", "Other", 0.0, 1.0, 0.0),
    f("texture_scale_u", "Texture repeat U", "Textures", 0.01, 100.0, 1.0),
    f("texture_scale_v", "Texture repeat V", "Textures", 0.01, 100.0, 1.0),
    // Added later: kept at the end so older scenes load their values in place.
    t("use_point_alpha", "Use point alpha (Alpha)", "Other", false),
];

// Text parameters of the Principled Shader, in texts() order.
pub const PRINCIPLED_TEXTURES: [&str; 3] = ["Base color texture", "Roughness texture", "Normal map"];

pub const PYRO: &[Param] = &[
    c("smoke_color", "Smoke color", "Smoke", [0.2, 0.2, 0.2]),
    f("smoke_density", "Smoke density", "Smoke", 0.0, 20.0, 1.0),
    f("anisotropy", "Scattering direction", "Smoke", -0.95, 0.95, 0.3),
    c("fire_color", "Fire tint", "Fire", [1.0, 1.0, 1.0]),
    f("fire_intensity", "Fire intensity", "Fire", 0.0, 100.0, 1.0),
];

pub const HAIR: &[Param] = &[
    c("hair_color", "Hair color", "Hair", [0.3, 0.17, 0.08]),
    f("roughness", "Roughness", "Hair", 0.0, 1.0, 0.35),
    f("ior", "IOR", "Hair", 1.0, 2.5, 1.55),
    c("specular_tint", "Highlight tint", "Hair", [1.0, 1.0, 1.0]),
    f("glint", "Glints", "Hair", 0.0, 1.0, 0.0),
];

pub const TOON: &[Param] = &[
    c("base_color", "Lit color", "Toon", [0.9, 0.35, 0.25]),
    c("shadow_color", "Shadow tint", "Toon", [0.25, 0.2, 0.35]),
    f("bands", "Shading bands", "Toon", 2.0, 8.0, 2.0),
    f("softness", "Band softness", "Toon", 0.0, 1.0, 0.0),
    f("terminator", "Shadow size", "Toon", -0.5, 0.5, 0.05),
    f("highlight", "Highlight", "Highlight", 0.0, 2.0, 1.0),
    f("highlight_size", "Highlight size", "Highlight", 0.01, 1.0, 0.3),
    c("highlight_color", "Highlight color", "Highlight", [1.0, 1.0, 1.0]),
    c("emission_color", "Emission color", "Emission", [1.0, 1.0, 1.0]),
    f("emission_intensity", "Emission intensity", "Emission", 0.0, 50.0, 0.0),
    // Added later: kept at the end so older scenes load their values in place.
    t("use_point_color", "Use point colour (Cd)", "Toon", false),
    f("outline_width", "Outline width (pixels)", "Outline", 0.0, 12.0, 2.0),
    c("outline_color", "Outline color", "Outline", [0.06, 0.05, 0.08]),
];

pub type Preset = (&'static str, &'static [(&'static str, [f32; 3])]);

const fn v(x: f32) -> [f32; 3] {
    [x, 0.0, 0.0]
}

pub const PRINCIPLED_PRESETS: &[Preset] = &[
    ("Principled", &[]),
    // Liquids and transparent solids
    ("Water", &[("transmission", v(1.0)), ("ior", v(1.333)), ("roughness", v(0.02)), ("transmission_color", [0.86, 0.95, 1.0]), ("base_color", [1.0, 1.0, 1.0])]),
    ("Glass", &[("transmission", v(1.0)), ("ior", v(1.5)), ("roughness", v(0.0)), ("base_color", [1.0, 1.0, 1.0])]),
    ("Frosted Glass", &[("transmission", v(1.0)), ("ior", v(1.5)), ("roughness", v(0.3)), ("base_color", [1.0, 1.0, 1.0])]),
    ("Ice", &[("transmission", v(1.0)), ("ior", v(1.31)), ("roughness", v(0.12)), ("transmission_color", [0.88, 0.96, 1.0])]),
    ("Diamond", &[("transmission", v(1.0)), ("ior", v(2.42)), ("roughness", v(0.0)), ("base_color", [1.0, 1.0, 1.0])]),
    ("Honey", &[("transmission", v(1.0)), ("ior", v(1.5)), ("roughness", v(0.05)), ("transmission_color", [1.0, 0.55, 0.08])]),
    ("Soap Bubble", &[("transmission", v(1.0)), ("ior", v(1.33)), ("roughness", v(0.0)), ("thin_walled", v(1.0)), ("iridescence", v(1.0))]),
    // Metals
    ("Gold", &[("metallic", v(1.0)), ("base_color", [1.0, 0.77, 0.34]), ("roughness", v(0.18))]),
    ("Silver", &[("metallic", v(1.0)), ("base_color", [0.97, 0.96, 0.91]), ("roughness", v(0.12))]),
    ("Copper", &[("metallic", v(1.0)), ("base_color", [0.95, 0.64, 0.54]), ("roughness", v(0.25))]),
    ("Brass", &[("metallic", v(1.0)), ("base_color", [0.91, 0.78, 0.42]), ("roughness", v(0.25))]),
    ("Aluminum", &[("metallic", v(1.0)), ("base_color", [0.91, 0.92, 0.92]), ("roughness", v(0.3))]),
    ("Brushed Aluminum", &[("metallic", v(1.0)), ("base_color", [0.91, 0.92, 0.92]), ("roughness", v(0.35)), ("anisotropy", v(0.75))]),
    ("Chrome", &[("metallic", v(1.0)), ("base_color", [0.62, 0.63, 0.64]), ("roughness", v(0.03))]),
    ("Steel", &[("metallic", v(1.0)), ("base_color", [0.56, 0.57, 0.58]), ("roughness", v(0.35))]),
    ("Mirror", &[("metallic", v(1.0)), ("base_color", [0.96, 0.96, 0.97]), ("roughness", v(0.0))]),
    // Plastics and paints
    ("Plastic", &[("base_color", [0.8, 0.12, 0.1]), ("roughness", v(0.3))]),
    ("Glossy Plastic", &[("base_color", [0.1, 0.3, 0.8]), ("roughness", v(0.08)), ("clearcoat", v(0.5))]),
    ("Rubber", &[("base_color", [0.04, 0.04, 0.04]), ("roughness", v(0.75)), ("specular", v(0.4))]),
    ("Car Paint", &[("base_color", [0.55, 0.02, 0.03]), ("roughness", v(0.35)), ("clearcoat", v(1.0)), ("clearcoat_roughness", v(0.02))]),
    // Ceramics and stone
    ("Porcelain", &[("base_color", [0.95, 0.94, 0.9]), ("roughness", v(0.1)), ("clearcoat", v(1.0)), ("clearcoat_roughness", v(0.02)), ("subsurface", v(0.25)), ("subsurface_color", [1.0, 0.95, 0.9]), ("subsurface_radius", v(0.3))]),
    ("Ceramic", &[("base_color", [0.72, 0.36, 0.2]), ("roughness", v(0.25)), ("clearcoat", v(0.8))]),
    ("Marble", &[("base_color", [0.92, 0.91, 0.88]), ("roughness", v(0.1)), ("subsurface", v(0.6)), ("subsurface_color", [0.95, 0.9, 0.85]), ("subsurface_radius", v(0.5))]),
    ("Jade", &[("base_color", [0.3, 0.6, 0.35]), ("roughness", v(0.15)), ("subsurface", v(1.0)), ("subsurface_color", [0.4, 0.9, 0.5]), ("subsurface_radius", v(1.0))]),
    ("Concrete", &[("base_color", [0.55, 0.55, 0.52]), ("roughness", v(0.9)), ("specular", v(0.3))]),
    ("Clay", &[("base_color", [0.7, 0.42, 0.3]), ("roughness", v(0.85)), ("specular", v(0.2))]),
    // Organic
    ("Skin", &[("base_color", [0.85, 0.6, 0.5]), ("roughness", v(0.45)), ("subsurface", v(1.0)), ("subsurface_color", [1.0, 0.35, 0.2]), ("subsurface_radius", v(1.0))]),
    ("Wax", &[("base_color", [0.95, 0.9, 0.75]), ("roughness", v(0.3)), ("subsurface", v(1.0)), ("subsurface_color", [1.0, 0.8, 0.5]), ("subsurface_radius", v(2.0))]),
    ("Milk", &[("base_color", [0.95, 0.95, 0.92]), ("roughness", v(0.15)), ("subsurface", v(1.0)), ("subsurface_color", [1.0, 0.97, 0.9]), ("subsurface_radius", v(2.0))]),
    ("Chocolate", &[("base_color", [0.25, 0.12, 0.06]), ("roughness", v(0.3)), ("subsurface", v(0.3)), ("subsurface_color", [0.6, 0.3, 0.1]), ("subsurface_radius", v(0.3))]),
    ("Wood", &[("base_color", [0.45, 0.28, 0.15]), ("roughness", v(0.55)), ("clearcoat", v(0.3))]),
    ("Leather", &[("base_color", [0.3, 0.15, 0.08]), ("roughness", v(0.5)), ("specular", v(0.5))]),
    ("Mud", &[("base_color", [0.25, 0.18, 0.12]), ("roughness", v(0.7)), ("clearcoat", v(0.3)), ("clearcoat_roughness", v(0.2))]),
    // Cloth
    ("Velvet", &[("base_color", [0.4, 0.02, 0.1]), ("roughness", v(0.9)), ("specular", v(0.2)), ("sheen", v(1.0)), ("sheen_color", [1.0, 0.5, 0.6])]),
    ("Cotton", &[("base_color", [0.8, 0.8, 0.78]), ("roughness", v(0.9)), ("specular", v(0.3)), ("sheen", v(0.5))]),
    ("Paper", &[("base_color", [0.9, 0.9, 0.86]), ("roughness", v(0.9)), ("specular", v(0.3)), ("subsurface", v(0.2)), ("subsurface_radius", v(0.2))]),
    // Light emitters
    ("Lava", &[("base_color", [0.05, 0.04, 0.04]), ("roughness", v(0.8)), ("emission_color", [1.0, 0.35, 0.05]), ("emission_intensity", v(8.0))]),
    ("Neon", &[("base_color", [0.1, 0.1, 0.1]), ("emission_color", [0.2, 0.8, 1.0]), ("emission_intensity", v(20.0))]),
];

// Add-menu categories for PRINCIPLED_PRESETS (index ranges, in order).
pub const PRESET_CATEGORIES: [(&str, std::ops::Range<usize>); 7] = [
    ("Liquids & Glass", 1..8),
    ("Metals", 8..17),
    ("Plastics & Paint", 17..21),
    ("Ceramics & Stone", 21..27),
    ("Organic", 27..34),
    ("Cloth & Paper", 34..37),
    ("Emissive", 37..39),
];

pub const PYRO_PRESETS: &[Preset] = &[
    ("Pyro Shader", &[]),
    ("Fire", &[("smoke_color", [0.12, 0.12, 0.12]), ("smoke_density", v(1.0)), ("fire_intensity", v(2.0)), ("fire_color", [1.0, 0.85, 0.7])]),
    ("Smoke", &[("smoke_color", [0.55, 0.55, 0.55]), ("smoke_density", v(1.0)), ("fire_intensity", v(0.0))]),
    ("Explosion", &[("smoke_color", [0.08, 0.08, 0.08]), ("smoke_density", v(1.5)), ("fire_intensity", v(6.0)), ("fire_color", [1.0, 0.6, 0.3])]),
    ("Clouds", &[("smoke_color", [0.95, 0.95, 0.97]), ("smoke_density", v(0.5)), ("fire_intensity", v(0.0)), ("anisotropy", v(0.6))]),
    ("Steam", &[("smoke_color", [0.9, 0.9, 0.9]), ("smoke_density", v(0.3)), ("fire_intensity", v(0.0)), ("anisotropy", v(0.5))]),
];

pub const TOON_PRESETS: &[Preset] = &[
    ("Toon Shader", &[]),
    ("Anime", &[("bands", v(2.0)), ("softness", v(0.0)), ("highlight", v(1.2)), ("highlight_size", v(0.15))]),
    ("Cel 3 Tone", &[("bands", v(3.0)), ("softness", v(0.0))]),
    ("Soft Toon", &[("bands", v(3.0)), ("softness", v(1.0)), ("highlight", v(0.6)), ("highlight_size", v(0.5))]),
    ("Toon Metal", &[("base_color", [0.8, 0.8, 0.85]), ("shadow_color", [0.15, 0.15, 0.25]), ("bands", v(4.0)), ("highlight", v(2.0)), ("highlight_size", v(0.08))]),
    // For effects coloured by their own Cd (e.g. Toon Fire's flame bands): flat, outlined.
    ("Toon FX", &[("use_point_color", v(1.0)), ("shadow_color", [0.72, 0.6, 0.78]), ("bands", v(2.0)), ("terminator", v(0.35)), ("highlight", v(0.0)), ("outline_width", v(2.5))]),
];

pub const HAIR_PRESETS: &[Preset] = &[
    ("Hair Shader", &[]),
    ("Brown Hair", &[("hair_color", [0.3, 0.17, 0.08])]),
    ("Blonde Hair", &[("hair_color", [0.75, 0.58, 0.33])]),
    ("Black Hair", &[("hair_color", [0.03, 0.02, 0.015]), ("roughness", v(0.25))]),
    ("Red Hair", &[("hair_color", [0.55, 0.15, 0.05])]),
    ("Grey Hair", &[("hair_color", [0.75, 0.75, 0.75])]),
    ("Fur", &[("hair_color", [0.45, 0.32, 0.2]), ("roughness", v(0.55))]),
];

// A parameter table's values with lookups by key.
#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    pub table: Table,
    pub values: Vec<f32>,
    // Index into the table's presets (0: none chosen; shown as the node's name).
    pub preset: usize,
    // Texture files (Principled only).
    pub textures: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Principled,
    Pyro,
    Hair,
    Toon,
}

impl Table {
    pub fn params(self) -> &'static [Param] {
        match self {
            Table::Principled => PRINCIPLED,
            Table::Pyro => PYRO,
            Table::Hair => HAIR,
            Table::Toon => TOON,
        }
    }

    pub fn presets(self) -> &'static [Preset] {
        match self {
            Table::Principled => PRINCIPLED_PRESETS,
            Table::Pyro => PYRO_PRESETS,
            Table::Hair => HAIR_PRESETS,
            Table::Toon => TOON_PRESETS,
        }
    }

    pub fn len(self) -> usize {
        self.params().iter().map(Param::width).sum()
    }

    fn offset(self, key: &str) -> Option<(usize, &'static Param)> {
        let mut at = 0;
        for p in self.params() {
            if p.key == key {
                return Some((at, p));
            }
            at += p.width();
        }
        None
    }
}

impl Values {
    pub fn new(table: Table) -> Self {
        let values = table.params().iter().flat_map(|p| p.default[..p.width()].to_vec()).collect();
        let textures = if table == Table::Principled { vec![String::new(); PRINCIPLED_TEXTURES.len()] } else { Vec::new() };
        Values { table, values, preset: 0, textures }
    }

    pub fn preset(table: Table, preset: usize) -> Self {
        let mut v = Values::new(table);
        v.apply_preset(preset);
        v
    }

    // Resets to the defaults with the preset's values on top.
    pub fn apply_preset(&mut self, preset: usize) {
        let textures = std::mem::take(&mut self.textures);
        *self = Values::new(self.table);
        self.textures = textures;
        self.preset = preset.min(self.table.presets().len() - 1);
        for (key, value) in self.table.presets()[self.preset].1 {
            self.set(key, *value);
        }
    }

    // Saved as [preset, values...]; missing trailing values keep their defaults.
    pub fn to_params(&self) -> Vec<f32> {
        let mut p = vec![self.preset as f32];
        p.extend_from_slice(&self.values);
        p
    }

    pub fn from_params(table: Table, p: &[f32]) -> Self {
        let mut v = Values::new(table);
        v.preset = p.first().map_or(0, |x| (x.max(0.0) as usize).min(table.presets().len() - 1));
        for (slot, value) in v.values.iter_mut().zip(p.iter().skip(1)) {
            *slot = *value;
        }
        v
    }

    pub fn float(&self, key: &str) -> f32 {
        self.table.offset(key).map_or(0.0, |(at, _)| self.values[at])
    }

    pub fn color(&self, key: &str) -> [f32; 3] {
        self.table.offset(key).map_or([0.0; 3], |(at, p)| {
            if p.width() == 3 { [self.values[at], self.values[at + 1], self.values[at + 2]] } else { [self.values[at]; 3] }
        })
    }

    pub fn flag(&self, key: &str) -> bool {
        self.float(key) > 0.5
    }

    pub fn set(&mut self, key: &str, value: [f32; 3]) {
        if let Some((at, p)) = self.table.offset(key) {
            self.values[at..at + p.width()].copy_from_slice(&value[..p.width()]);
        }
    }

    pub fn name(&self) -> &'static str {
        self.table.presets()[self.preset.min(self.table.presets().len() - 1)].0
    }

    pub fn texture(&self, index: usize) -> &str {
        self.textures.get(index).map_or("", |t| t.trim())
    }

    // The colour the viewport shows for this material.
    pub fn viewport_color(&self) -> [f32; 3] {
        match self.table {
            Table::Principled => {
                let emission = self.float("emission_intensity");
                if emission > 0.5 {
                    return self.color("emission_color").map(|c| (c * emission.min(4.0) / 2.0).min(1.0));
                }
                let base = self.color("base_color");
                let transmission = self.float("transmission");
                let tint = self.color("transmission_color");
                // Clear materials show as a pale tint of their transmission colour.
                std::array::from_fn(|i| base[i] * (1.0 - transmission) + tint[i] * 0.85 * transmission)
            }
            Table::Pyro => self.color("smoke_color"),
            Table::Hair => self.color("hair_color"),
            Table::Toon => self.color("base_color"),
        }
    }
}

impl Values {
    // A Toon Shader as the real-time renderer draws it (viewport and sprite sheets).
    pub fn toon_look(&self) -> crate::gpu_viewport::ToonMaterial {
        let [sr, sg, sb] = self.color("shadow_color");
        let highlight = self.color("highlight_color").map(|c| c * self.float("highlight"));
        let emission = self.color("emission_color").map(|c| c * self.float("emission_intensity"));
        let [or, og, ob] = self.color("outline_color");
        crate::gpu_viewport::ToonMaterial {
            shade: [self.float("bands").round().clamp(2.0, 8.0), self.float("softness").clamp(0.0, 1.0), self.float("terminator"), self.float("highlight_size").clamp(0.0, 1.0)],
            shadow: [sr, sg, sb, self.float("outline_width").max(0.0)],
            highlight: [highlight[0], highlight[1], highlight[2], 0.0],
            emission: [emission[0], emission[1], emission[2], 0.0],
            outline: [or, og, ob, 1.0],
        }
    }
}

// --- MoonRay output ------------------------------------------------------------

pub fn rgb(c: [f32; 3]) -> String {
    format!("Rgb({:.4}, {:.4}, {:.4})", c[0], c[1], c[2])
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "/").replace('"', "\\\""))
}

// An attribute's value: bound to a map when one drives it, else the constant.
#[derive(Clone, Debug)]
pub enum Input {
    Value(String),
    Map(String),
}

impl Input {
    fn rdla(&self) -> String {
        match self {
            Input::Value(v) => v.clone(),
            Input::Map(m) => format!("bind({m})"),
        }
    }
}

// Writes `objects` (maps the material needs) and returns (class, body) of a
// DwaBaseMaterial for Principled values. `inputs` overrides attributes with
// maps (from a Material Builder); `name` is the material's RDLA path.
pub fn principled_rdla(name: &str, p: &Values, inputs: &HashMap<&str, Input>, objects: &mut String) -> (&'static str, String) {
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut put = |key: &str, value: String| attrs.push((key.to_owned(), value));
    let scale = [p.float("texture_scale_u"), p.float("texture_scale_v")];
    let base = p.color("base_color");
    // Base colour: a builder input, a texture (tinted by the base colour), point Cd, or the constant.
    let base_input = inputs.get("base_color").cloned().unwrap_or_else(|| {
        if !p.texture(0).is_empty() {
            let map = format!("{name}_base_texture");
            objects.push_str(&image_map(&map, p.texture(0), scale, true, Some(base)));
            Input::Map(format!("ImageMap(\"{map}\")"))
        } else if p.flag("use_point_color") {
            let map = format!("{name}_point_color");
            objects.push_str(&attribute_map(&map, "Cd", 3, base));
            Input::Map(format!("AttributeMap(\"{map}\")"))
        } else {
            Input::Value(rgb(base))
        }
    });
    let metallic = p.float("metallic");
    put("albedo", base_input.rdla());
    put("metallic", inputs.get("metallic").map_or(format!("{metallic:.4}"), Input::rdla));
    put("metallic_color", base_input.rdla());
    put("metallic_edge_color", base_input.rdla());
    let roughness = inputs.get("roughness").cloned().unwrap_or_else(|| {
        if !p.texture(1).is_empty() {
            let map = format!("{name}_roughness_texture");
            objects.push_str(&image_map(&map, p.texture(1), scale, false, None));
            Input::Map(format!("ImageMap(\"{map}\")"))
        } else {
            Input::Value(format!("{:.4}", p.float("roughness")))
        }
    });
    put("roughness", roughness.rdla());
    put("specular", inputs.get("specular").map_or(format!("{:.4}", p.float("specular")), Input::rdla));
    put("refractive_index", format!("{:.4}", p.float("ior").max(1.0)));
    let transmission = p.float("transmission");
    put("transmission", inputs.get("transmission").map_or(format!("{transmission:.4}"), Input::rdla));
    put("transmission_color", inputs.get("transmission_color").map_or(rgb(p.color("transmission_color")), Input::rdla));
    if p.flag("thin_walled") {
        put("thin_geometry", "true".into());
    }
    let subsurface = p.float("subsurface");
    if subsurface > 0.0 {
        put("scattering_color", inputs.get("subsurface_color").map_or(rgb(p.color("subsurface_color")), Input::rdla));
        put("scattering_radius", format!("{:.4}", subsurface * p.float("subsurface_radius")));
    }
    let sheen = p.float("sheen");
    if sheen > 0.0 {
        put("show_fuzz", "true".into());
        put("fuzz", format!("{sheen:.4}"));
        put("fuzz_albedo", rgb(p.color("sheen_color")));
        put("fuzz_roughness", format!("{:.4}", p.float("sheen_roughness")));
    }
    let clearcoat = p.float("clearcoat");
    if clearcoat > 0.0 || inputs.contains_key("clearcoat") {
        put("show_clearcoat", "true".into());
        put("clearcoat", inputs.get("clearcoat").map_or(format!("{clearcoat:.4}"), Input::rdla));
        put("clearcoat_roughness", format!("{:.4}", p.float("clearcoat_roughness")));
    }
    let emission = p.float("emission_intensity");
    if emission > 0.0 || inputs.contains_key("emission") {
        put("show_emission", "true".into());
        put("emission", inputs.get("emission").map_or(rgb(p.color("emission_color").map(|c| c * emission)), Input::rdla));
    }
    let opacity = p.float("opacity");
    if let Some(input) = inputs.get("opacity") {
        put("presence", input.rdla());
    } else if p.flag("use_point_alpha") {
        // The geometry's Alpha (points or primitives); Opacity where it has none.
        let map = format!("{name}_point_alpha");
        objects.push_str(&attribute_map(&map, "Alpha", 0, [opacity; 3]));
        put("presence", Input::Map(format!("AttributeMap(\"{map}\")")).rdla());
    } else if opacity < 1.0 {
        put("presence", format!("{opacity:.4}"));
    }
    if p.float("anisotropy") != 0.0 {
        put("anisotropy", format!("{:.4}", p.float("anisotropy")));
    }
    if p.float("iridescence") > 0.0 {
        put("iridescence", format!("{:.4}", p.float("iridescence")));
    }
    // Normal maps are scene objects, not bindings.
    match inputs.get("normal") {
        Some(Input::Map(m)) => put("input_normal", m.clone()),
        _ if !p.texture(2).is_empty() => {
            let map = format!("{name}_normal_map");
            objects.push_str(&normal_map(&map, p.texture(2), scale));
            put("input_normal", format!("ImageNormalMap(\"{map}\")"));
        }
        _ => {}
    }
    let body: String = attrs.iter().map(|(k, v)| format!("    [\"{k}\"] = {v},\n")).collect();
    ("DwaBaseMaterial", format!("DwaBaseMaterial(\"{name}\") {{\n{body}}}\n"))
}

pub fn hair_rdla(name: &str, p: &Values) -> (&'static str, String) {
    let mut body = String::new();
    body.push_str(&format!("    [\"hair_color\"] = {},\n", rgb(p.color("hair_color"))));
    body.push_str(&format!("    [\"primary_specular_roughness\"] = {:.4},\n", p.float("roughness")));
    body.push_str(&format!("    [\"refractive_index\"] = {:.4},\n", p.float("ior").max(1.0)));
    body.push_str(&format!("    [\"primary_specular_tint\"] = {},\n", rgb(p.color("specular_tint"))));
    if p.float("glint") > 0.0 {
        body.push_str("    [\"show_hair_glint\"] = true,\n");
        body.push_str(&format!("    [\"glint_saturation\"] = {:.4},\n", p.float("glint")));
    }
    ("HairMaterial_v3", format!("HairMaterial_v3(\"{name}\") {{\n{body}}}\n"))
}

// DwaToonMaterial: banded ramp diffuse from the shadow tint up to the lit colour,
// with a hard-edged toon highlight.
pub fn toon_rdla(name: &str, p: &Values, objects: &mut String) -> (&'static str, String) {
    let bands = (p.float("bands").round() as usize).clamp(2, 8);
    let shadow = p.color("shadow_color");
    let mut positions = Vec::new();
    let mut colours = Vec::new();
    for i in 0..bands {
        let t = i as f32 / (bands - 1) as f32;
        positions.push(format!("{:.4}", i as f32 / bands as f32));
        colours.push(rgb(std::array::from_fn(|k| shadow[k] + (1.0 - shadow[k]) * t)));
    }
    let interpolation = if p.float("softness") > 0.5 { 1 } else { 0 };
    let mut body = String::new();
    let albedo = if p.flag("use_point_color") {
        let map = format!("{name}_point_color");
        objects.push_str(&attribute_map(&map, "Cd", 3, p.color("base_color")));
        format!("AttributeMap(\"{map}\")")
    } else {
        rgb(p.color("base_color"))
    };
    body.push_str(&format!("    [\"albedo\"] = {albedo},\n"));
    body.push_str("    [\"diffuse_model\"] = 1,\n");
    body.push_str(&format!("    [\"positions\"] = {{{}}},\n", positions.join(", ")));
    body.push_str(&format!("    [\"colors\"] = {{{}}},\n", colours.join(", ")));
    body.push_str(&format!("    [\"interpolations\"] = {{{}}},\n", vec![interpolation.to_string(); bands].join(", ")));
    body.push_str(&format!("    [\"terminator_shift\"] = {:.4},\n", p.float("terminator")));
    let highlight = p.float("highlight");
    if highlight > 0.0 {
        body.push_str("    [\"specular_model\"] = 2,\n");
        body.push_str(&format!("    [\"roughness\"] = {:.4},\n", p.float("highlight_size")));
        body.push_str(&format!("    [\"toon_specular_intensity\"] = {highlight:.4},\n"));
        body.push_str(&format!("    [\"toon_specular_tint\"] = {},\n", rgb(p.color("highlight_color"))));
    } else {
        body.push_str("    [\"show_specular\"] = false,\n");
    }
    let emission = p.float("emission_intensity");
    if emission > 0.0 {
        body.push_str("    [\"show_emission\"] = true,\n");
        body.push_str(&format!("    [\"emission\"] = {},\n", rgb(p.color("emission_color").map(|c| c * emission))));
    }
    ("DwaToonMaterial", format!("DwaToonMaterial(\"{name}\") {{\n{body}}}\n"))
}

// The texture file MoonRay should read, converted to a tiled, mip-mapped EXR
// when needed, and its gamma mode (converted files are already linear).
fn render_texture(file: &str, colour: bool) -> (String, i32) {
    match crate::textures::prepare(file, colour) {
        Ok(path) if path != file.trim() => (path, 0),
        // .tx files, or files that couldn't be converted (MoonRay reports them).
        _ => (file.trim().to_owned(), if colour { 2 } else { 0 }),
    }
}

fn image_map(name: &str, file: &str, scale: [f32; 2], colour: bool, tint: Option<[f32; 3]>) -> String {
    let (file, gamma) = render_texture(file, colour);
    let mut s = format!("ImageMap(\"{name}\") {{\n    [\"texture\"] = {},\n", quote(&file));
    s.push_str(&format!("    [\"scale\"] = Vec2({:.4}, {:.4}),\n", scale[0], scale[1]));
    s.push_str(&format!("    [\"gamma\"] = {gamma},\n"));
    if let Some(tint) = tint.filter(|t| *t != [1.0; 3]) {
        s.push_str("    [\"gain_offset_enabled\"] = true,\n");
        s.push_str(&format!("    [\"gain\"] = {},\n", rgb(tint)));
    }
    s.push_str("}\n\n");
    s
}

fn normal_map(name: &str, file: &str, scale: [f32; 2]) -> String {
    let (file, _) = render_texture(file, false);
    format!(
        "ImageNormalMap(\"{name}\") {{\n    [\"tangent_space_normal_texture\"] = {},\n    [\"scale\"] = Vec2({:.4}, {:.4}),\n}}\n\n",
        quote(&file),
        scale[0],
        scale[1]
    )
}

// AttributeMap reading a primitive attribute (type: 0 float, 3 rgb).
fn attribute_map(name: &str, attribute: &str, ty: i32, default: [f32; 3]) -> String {
    format!(
        "AttributeMap(\"{name}\") {{\n    [\"primitive_attribute_name\"] = {},\n    [\"primitive_attribute_type\"] = {ty},\n    [\"default_value\"] = {},\n}}\n\n",
        quote(attribute),
        rgb(default)
    )
}

// --- Material Builder ------------------------------------------------------------

// What an input takes; unconnected inputs use the node's own value of the same name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Socket {
    Color,
    Float,
    Normal,
}

pub struct ShaderOp {
    pub label: &'static str,
    pub help: &'static str,
    // Inputs: (label, Principled key or param key, socket).
    pub inputs: &'static [(&'static str, &'static str, Socket)],
    pub params: &'static [Param],
    // Text fields (file names, attribute names).
    pub texts: &'static [&'static str],
    pub output: Socket,
}

pub const SURFACE_OUTPUT: usize = 0;

pub const SHADER_OPS: &[ShaderOp] = &[
    ShaderOp {
        label: "Surface Output",
        help: "The material: a Principled surface whose inputs can be driven by texture nodes.",
        inputs: &[
            ("Base Color", "base_color", Socket::Color),
            ("Roughness", "roughness", Socket::Float),
            ("Metallic", "metallic", Socket::Float),
            ("Specular", "specular", Socket::Float),
            ("Transmission", "transmission", Socket::Float),
            ("Transmission Color", "transmission_color", Socket::Color),
            ("Subsurface Color", "subsurface_color", Socket::Color),
            ("Clearcoat", "clearcoat", Socket::Float),
            ("Emission", "emission", Socket::Color),
            ("Opacity", "opacity", Socket::Float),
            ("Normal", "normal", Socket::Normal),
        ],
        params: &[],
        texts: &[],
        output: Socket::Color,
    },
    ShaderOp {
        label: "Image Texture",
        help: "An image file mapped by the mesh's UVs.",
        inputs: &[],
        params: &[
            f("repeat_u", "Repeat U", "", 0.01, 100.0, 1.0),
            f("repeat_v", "Repeat V", "", 0.01, 100.0, 1.0),
            c("tint", "Tint", "", [1.0, 1.0, 1.0]),
            t("color_data", "Colour image (sRGB)", "", true),
        ],
        texts: &["File"],
        output: Socket::Color,
    },
    ShaderOp {
        label: "Normal Map",
        help: "A tangent-space normal map image (connect to the Surface Output's Normal).",
        inputs: &[],
        params: &[f("repeat_u", "Repeat U", "", 0.01, 100.0, 1.0), f("repeat_v", "Repeat V", "", 0.01, 100.0, 1.0)],
        texts: &["File"],
        output: Socket::Normal,
    },
    ShaderOp {
        label: "Point Attribute",
        help: "Reads a geometry attribute (Cd, mask, flow…) at the shading point.",
        inputs: &[],
        params: &[t("is_float", "Float attribute", "", false), c("default", "When missing", "", [1.0, 1.0, 1.0])],
        texts: &["Attribute"],
        output: Socket::Color,
    },
    ShaderOp {
        label: "Noise",
        help: "Fractal Perlin noise between two colours, in object space.",
        inputs: &[],
        params: &[
            c("color_a", "Color A", "", [0.0, 0.0, 0.0]),
            c("color_b", "Color B", "", [1.0, 1.0, 1.0]),
            f("frequency", "Frequency", "", 0.01, 100.0, 4.0),
            f("octaves", "Octaves", "", 1.0, 10.0, 4.0),
            f("roughness", "Roughness", "", 0.0, 1.0, 0.5),
            f("distortion", "Distortion", "", 0.0, 5.0, 0.0),
            f("seed", "Seed", "", 0.0, 1000.0, 0.0),
        ],
        texts: &[],
        output: Socket::Color,
    },
    ShaderOp {
        label: "Cells",
        help: "Worley (cellular) noise: stones, scales, cracks.",
        inputs: &[],
        params: &[
            c("color_a", "Color A", "", [0.0, 0.0, 0.0]),
            c("color_b", "Color B", "", [1.0, 1.0, 1.0]),
            f("frequency", "Frequency", "", 0.01, 100.0, 4.0),
            f("jitter", "Jitter", "", 0.0, 1.0, 1.0),
            t("edges", "Cell edges", "", false),
            f("seed", "Seed", "", 0.0, 1000.0, 0.0),
        ],
        texts: &[],
        output: Socket::Color,
    },
    ShaderOp {
        label: "Checker",
        help: "A checkerboard in UV space.",
        inputs: &[],
        params: &[
            c("color_a", "Color A", "", [0.1, 0.1, 0.1]),
            c("color_b", "Color B", "", [0.9, 0.9, 0.9]),
            f("tiles_u", "Tiles U", "", 1.0, 256.0, 8.0),
            f("tiles_v", "Tiles V", "", 1.0, 256.0, 8.0),
        ],
        texts: &[],
        output: Socket::Color,
    },
    ShaderOp {
        label: "Color Ramp",
        help: "Maps a value (from another node, or along V) to a gradient between two colours.",
        inputs: &[("Value", "value", Socket::Float)],
        params: &[c("color_a", "Color A", "", [0.0, 0.0, 0.0]), c("color_b", "Color B", "", [1.0, 1.0, 1.0])],
        texts: &[],
        output: Socket::Color,
    },
    ShaderOp {
        label: "Mix",
        help: "Blends A and B by Factor.",
        inputs: &[("A", "color_a", Socket::Color), ("B", "color_b", Socket::Color), ("Factor", "factor", Socket::Float)],
        params: &[c("color_a", "A", "", [0.0, 0.0, 0.0]), c("color_b", "B", "", [1.0, 1.0, 1.0]), f("factor", "Factor", "", 0.0, 1.0, 0.5)],
        texts: &[],
        output: Socket::Color,
    },
    ShaderOp {
        label: "HSV Adjust",
        help: "Shifts hue and scales saturation and brightness.",
        inputs: &[("Color", "color", Socket::Color)],
        params: &[
            c("color", "Color", "", [0.5, 0.5, 0.5]),
            f("hue", "Hue shift", "", -1.0, 1.0, 0.0),
            f("saturation", "Saturation", "", 0.0, 4.0, 1.0),
            f("value", "Brightness", "", 0.0, 4.0, 1.0),
        ],
        texts: &[],
        output: Socket::Color,
    },
];

pub const CATEGORY: &str = "Shader Nodes";

#[derive(Clone, Debug, PartialEq)]
pub struct ShaderNode {
    pub op: usize,
    pub values: Vec<f32>,
    pub texts: Vec<String>,
    // The Surface Output's Principled values (unused by other ops).
    pub surface: Values,
}

impl ShaderNode {
    pub fn new(op: usize) -> Self {
        let op = op.min(SHADER_OPS.len() - 1);
        let def = &SHADER_OPS[op];
        ShaderNode {
            op,
            values: def.params.iter().flat_map(|p| p.default[..p.width()].to_vec()).collect(),
            texts: vec![String::new(); def.texts.len()],
            surface: Values::new(Table::Principled),
        }
    }

    pub fn def(&self) -> &'static ShaderOp {
        &SHADER_OPS[self.op.min(SHADER_OPS.len() - 1)]
    }

    pub fn to_params(&self) -> Vec<f32> {
        let mut p = vec![self.op as f32];
        if self.op == SURFACE_OUTPUT {
            p.extend(self.surface.to_params());
        } else {
            p.extend_from_slice(&self.values);
        }
        p
    }

    pub fn from_params(p: &[f32]) -> Self {
        let mut node = ShaderNode::new(p.first().map_or(0, |o| o.max(0.0) as usize));
        if node.op == SURFACE_OUTPUT {
            node.surface = Values::from_params(Table::Principled, &p[1.min(p.len())..]);
        } else {
            for (slot, value) in node.values.iter_mut().zip(p.iter().skip(1)) {
                *slot = *value;
            }
        }
        node
    }

    fn param(&self, key: &str) -> [f32; 3] {
        let mut at = 0;
        for p in self.def().params {
            if p.key == key {
                return if p.width() == 3 { [self.values[at], self.values[at + 1], self.values[at + 2]] } else { [self.values[at]; 3] };
            }
            at += p.width();
        }
        [0.0; 3]
    }

    pub fn param_mut(&mut self) -> impl Iterator<Item = (&'static Param, &mut [f32])> {
        let def = self.def();
        let mut rest: &mut [f32] = &mut self.values;
        def.params.iter().map(move |p| {
            let (head, tail) = std::mem::take(&mut rest).split_at_mut(p.width());
            rest = tail;
            (p, head)
        })
    }

    pub fn text(&self, i: usize) -> &str {
        self.texts.get(i).map_or("", |t| t.trim())
    }

    pub fn label(&self, id: usize) -> String {
        format!("{}{id}", self.def().label.to_lowercase().replace(' ', "_"))
    }
}

// Compiles a Material Builder network to RDLA: the maps, then the material.
// `nodes` are the builder's children, `input_of(node, slot)` its wiring, and
// `output` the Surface Output to compile. Returns (class, all text).
pub fn compile_builder(
    name: &str,
    nodes: &HashMap<usize, ShaderNode>,
    input_of: impl Fn(usize, usize) -> Option<usize>,
    output: usize,
) -> Result<(&'static str, String), String> {
    let out = nodes.get(&output).filter(|n| n.op == SURFACE_OUTPUT).ok_or("the Material Builder has no Surface Output node")?;
    let mut objects = String::new();
    let mut written: HashSet<usize> = HashSet::new();
    let mut visiting: HashSet<usize> = HashSet::new();
    let mut inputs: HashMap<&str, Input> = HashMap::new();
    for (slot, (_, key, socket)) in out.def().inputs.iter().enumerate() {
        if let Some(src) = input_of(output, slot) {
            let map = emit_map(name, src, nodes, &input_of, &mut written, &mut visiting, &mut objects)?;
            let src_socket = nodes[&src].def().output;
            if (*socket == Socket::Normal) != (src_socket == Socket::Normal) {
                return Err(format!("{} can only take a Normal Map", if *socket == Socket::Normal { "Normal" } else { "this input" }));
            }
            inputs.insert(key, Input::Map(map));
        }
    }
    let (class, material) = principled_rdla(name, &out.surface, &inputs, &mut objects);
    Ok((class, format!("{objects}{material}")))
}

// Writes the map for `id` (after the maps it reads) and returns its reference.
fn emit_map(
    material: &str,
    id: usize,
    nodes: &HashMap<usize, ShaderNode>,
    input_of: &impl Fn(usize, usize) -> Option<usize>,
    written: &mut HashSet<usize>,
    visiting: &mut HashSet<usize>,
    out: &mut String,
) -> Result<String, String> {
    let node = nodes.get(&id).ok_or("a shader node input is connected to something outside the Material Builder")?;
    let name = format!("{material}_n{id}");
    let class = match node.op {
        1 => "ImageMap",
        2 => "ImageNormalMap",
        3 => "AttributeMap",
        4 => "NoiseMap_v2",
        5 => "NoiseWorleyMap_v3",
        6 => "CheckerboardMap",
        7 => "RampMap",
        8 => "BlendMap",
        9 => "ColorCorrectHsvMap",
        _ => return Err("a Surface Output can't feed another node".into()),
    };
    let reference = format!("{class}(\"{name}\")");
    if written.contains(&id) {
        return Ok(reference);
    }
    if !visiting.insert(id) {
        return Err("the shader network has a loop".into());
    }
    // Inputs first, so every bound map is defined before it's used.
    let mut bound: HashMap<&str, String> = HashMap::new();
    for (slot, (_, key, _)) in node.def().inputs.iter().enumerate() {
        if let Some(src) = input_of(id, slot) {
            bound.insert(key, format!("bind({})", emit_map(material, src, nodes, input_of, written, visiting, out)?));
        }
    }
    let value = |key: &str, colour: bool| -> String {
        bound.get(key).cloned().unwrap_or_else(|| {
            let v = node.param(key);
            if colour { rgb(v) } else { format!("{:.4}", v[0]) }
        })
    };
    let p = |key: &str| node.param(key)[0];
    let mut body = String::new();
    match node.op {
        1 => {
            let (file, gamma) = render_texture(node.text(0), p("color_data") > 0.5);
            body.push_str(&format!("    [\"texture\"] = {},\n", quote(&file)));
            body.push_str(&format!("    [\"scale\"] = Vec2({:.4}, {:.4}),\n", p("repeat_u"), p("repeat_v")));
            body.push_str(&format!("    [\"gamma\"] = {gamma},\n"));
            if node.param("tint") != [1.0; 3] {
                body.push_str("    [\"gain_offset_enabled\"] = true,\n");
                body.push_str(&format!("    [\"gain\"] = {},\n", rgb(node.param("tint"))));
            }
        }
        2 => {
            let (file, _) = render_texture(node.text(0), false);
            body.push_str(&format!("    [\"tangent_space_normal_texture\"] = {},\n", quote(&file)));
            body.push_str(&format!("    [\"scale\"] = Vec2({:.4}, {:.4}),\n", p("repeat_u"), p("repeat_v")));
        }
        3 => {
            let attribute = if node.text(0).is_empty() { "Cd" } else { node.text(0) };
            body.push_str(&format!("    [\"primitive_attribute_name\"] = {},\n", quote(attribute)));
            body.push_str(&format!("    [\"primitive_attribute_type\"] = {},\n", if p("is_float") > 0.5 { 0 } else { 3 }));
            body.push_str(&format!("    [\"default_value\"] = {},\n", rgb(node.param("default"))));
        }
        4 => {
            body.push_str("    [\"color\"] = true,\n");
            body.push_str(&format!("    [\"color_A\"] = {},\n    [\"color_B\"] = {},\n", rgb(node.param("color_a")), rgb(node.param("color_b"))));
            body.push_str(&format!("    [\"frequency_multiplier\"] = {:.4},\n", p("frequency")));
            body.push_str(&format!("    [\"max_level\"] = {:.4},\n", p("octaves").max(1.0)));
            body.push_str(&format!("    [\"persistence\"] = {:.4},\n", p("roughness")));
            body.push_str(&format!("    [\"distortion\"] = {:.4},\n", p("distortion")));
            body.push_str(&format!("    [\"seed\"] = {},\n", p("seed") as i32));
        }
        5 => {
            body.push_str(&format!("    [\"color_A\"] = {},\n    [\"color_B\"] = {},\n", rgb(node.param("color_a")), rgb(node.param("color_b"))));
            body.push_str(&format!("    [\"frequency\"] = {:.4},\n", p("frequency")));
            body.push_str(&format!("    [\"jitter\"] = {:.4},\n", p("jitter")));
            body.push_str(&format!("    [\"output_mode\"] = {},\n", if p("edges") > 0.5 { 3 } else { 0 }));
            body.push_str(&format!("    [\"seed\"] = {},\n", p("seed") as i32));
            body.push_str("    [\"space\"] = 4,\n");
        }
        6 => {
            body.push_str(&format!("    [\"color_A\"] = {},\n    [\"color_B\"] = {},\n", rgb(node.param("color_a")), rgb(node.param("color_b"))));
            body.push_str(&format!("    [\"num_u_tiles\"] = {},\n    [\"num_v_tiles\"] = {},\n", p("tiles_u") as i32, p("tiles_v") as i32));
        }
        7 => {
            body.push_str("    [\"positions\"] = {0.0, 1.0},\n");
            body.push_str(&format!("    [\"colors\"] = {{{}, {}}},\n", rgb(node.param("color_a")), rgb(node.param("color_b"))));
            body.push_str("    [\"interpolations\"] = {1, 1},\n");
            if let Some(input) = bound.get("value") {
                // ramp_type "input" isn't a mode: MoonRay ramps read `input` when it's bound.
                body.push_str(&format!("    [\"input\"] = {input},\n"));
            }
        }
        8 => {
            body.push_str(&format!("    [\"color_A\"] = {},\n", value("color_a", true)));
            body.push_str(&format!("    [\"color_B\"] = {},\n", value("color_b", true)));
            body.push_str(&format!("    [\"blend_amount\"] = {},\n", value("factor", false)));
        }
        _ => {
            body.push_str(&format!("    [\"input\"] = {},\n", value("color", true)));
            body.push_str(&format!("    [\"hue_shift\"] = {:.4},\n", p("hue")));
            body.push_str(&format!("    [\"saturation_factor\"] = {:.4},\n", p("saturation")));
            body.push_str(&format!("    [\"value_factor\"] = {:.4},\n", p("value")));
        }
    }
    out.push_str(&format!("{class}(\"{name}\") {{\n{body}}}\n\n"));
    visiting.remove(&id);
    written.insert(id);
    Ok(reference)
}

// The viewport colour and texture of a builder's output: the base colour
// input's constant, image or colour A, followed through Mix and HSV nodes.
pub fn builder_preview(nodes: &HashMap<usize, ShaderNode>, input_of: impl Fn(usize, usize) -> Option<usize>, output: usize) -> ([f32; 3], Option<String>) {
    let Some(out) = nodes.get(&output) else { return ([0.8; 3], None) };
    let mut colour = out.surface.viewport_color();
    let mut node = input_of(output, 0);
    for _ in 0..16 {
        let Some(n) = node.and_then(|id| nodes.get(&id).map(|n| (id, n))) else { break };
        match n.1.op {
            1 if !n.1.text(0).is_empty() => return (n.1.param("tint"), Some(n.1.text(0).to_owned())),
            3 | 4 | 5 | 6 | 7 => {
                let (a, b) = (n.1.param("color_a"), n.1.param("color_b"));
                colour = std::array::from_fn(|i| (a[i] + b[i]) * 0.5);
                break;
            }
            8 => {
                node = input_of(n.0, 0).or(input_of(n.0, 1));
                if node.is_none() {
                    let (a, b, t) = (n.1.param("color_a"), n.1.param("color_b"), n.1.param("factor")[0]);
                    colour = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
                }
            }
            9 => {
                node = input_of(n.0, 0);
                if node.is_none() {
                    colour = n.1.param("color");
                }
            }
            _ => break,
        }
    }
    (colour, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_set_their_values_and_round_trip() {
        for table in [Table::Principled, Table::Pyro, Table::Hair, Table::Toon] {
            for (i, (name, overrides)) in table.presets().iter().enumerate() {
                let v = Values::preset(table, i);
                assert_eq!(v.name(), *name);
                for (key, value) in *overrides {
                    let p = table.params().iter().find(|p| p.key == *key).unwrap_or_else(|| panic!("{name}: no parameter {key}"));
                    let got = if p.width() == 3 { v.color(key) } else { [v.float(key), 0.0, 0.0] };
                    assert_eq!(got, *value, "{name}.{key}");
                }
                assert_eq!(Values::from_params(table, &v.to_params()), Values { textures: v.textures.clone(), ..v.clone() });
            }
        }
        let names: Vec<&str> = PRINCIPLED_PRESETS.iter().map(|p| p.0).collect();
        for wanted in ["Water", "Aluminum", "Porcelain", "Rubber", "Gold", "Glass", "Skin"] {
            assert!(names.contains(&wanted), "{wanted}");
        }
        // Every preset is in exactly one menu, in order.
        let covered: Vec<usize> = PRESET_CATEGORIES.iter().flat_map(|(_, r)| r.clone()).collect();
        assert_eq!(covered, (1..PRINCIPLED_PRESETS.len()).collect::<Vec<_>>());
        assert_eq!(PRINCIPLED_PRESETS[PRESET_CATEGORIES[1].1.start].0, "Gold");
        assert_eq!(PRINCIPLED_PRESETS[PRESET_CATEGORIES[6].1.start].0, "Lava");
        // Older saves with fewer values keep the defaults for the rest.
        let short = Values::from_params(Table::Principled, &[0.0, 0.1, 0.2, 0.3]);
        assert_eq!(short.color("base_color"), [0.1, 0.2, 0.3]);
        assert_eq!(short.float("roughness"), 0.4);
    }

    #[test]
    fn principled_writes_moonray_attributes() {
        let water = Values::preset(Table::Principled, 1);
        let mut objects = String::new();
        let (class, body) = principled_rdla("/m/water", &water, &HashMap::new(), &mut objects);
        assert_eq!(class, "DwaBaseMaterial");
        assert!(body.contains("[\"transmission\"] = 1.0000"));
        assert!(body.contains("[\"refractive_index\"] = 1.3330"));
        assert!(objects.is_empty());
        let mut textured = Values::preset(Table::Principled, 0);
        textured.textures = vec!["/tex/a.png".into(), "".into(), "/tex/n.png".into()];
        let (_, body) = principled_rdla("/m/t", &textured, &HashMap::new(), &mut objects);
        assert!(body.contains("[\"albedo\"] = bind(ImageMap(\"/m/t_base_texture\"))"));
        assert!(body.contains("[\"input_normal\"] = ImageNormalMap(\"/m/t_normal_map\")"));
        assert!(objects.contains("ImageMap(\"/m/t_base_texture\") {") && objects.contains("\"/tex/a.png\""));
        let skin = Values::preset(Table::Principled, PRINCIPLED_PRESETS.iter().position(|p| p.0 == "Skin").unwrap());
        let (_, body) = principled_rdla("/m/s", &skin, &HashMap::new(), &mut String::new());
        assert!(body.contains("scattering_radius"));
        let (class, body) = toon_rdla("/m/toon", &Values::preset(Table::Toon, 2), &mut String::new());
        assert_eq!(class, "DwaToonMaterial");
        assert!(body.contains("[\"diffuse_model\"] = 1") && body.contains("[\"interpolations\"] = {0, 0, 0}"));
    }

    #[test]
    fn builder_compiles_a_map_network() {
        let mut nodes = HashMap::new();
        nodes.insert(1, ShaderNode::new(SURFACE_OUTPUT));
        nodes.insert(2, ShaderNode::new(4)); // noise
        nodes.insert(3, ShaderNode::new(8)); // mix
        nodes.insert(4, ShaderNode::new(5)); // cells
        let links: HashMap<(usize, usize), usize> = [((3, 0), 2), ((3, 1), 4), ((1, 0), 3), ((1, 1), 2)].into_iter().collect();
        let (class, text) = compile_builder("/m/b", &nodes, |to, slot| links.get(&(to, slot)).copied(), 1).unwrap();
        assert_eq!(class, "DwaBaseMaterial");
        // Each map once, defined before use, the material last.
        assert_eq!(text.matches("NoiseMap_v2(\"/m/b_n2\") {").count(), 1);
        let noise = text.find("NoiseMap_v2(\"/m/b_n2\") {").unwrap();
        let mix = text.find("BlendMap(\"/m/b_n3\") {").unwrap();
        let material = text.find("DwaBaseMaterial(\"/m/b\") {").unwrap();
        assert!(noise < mix && mix < material);
        assert!(text.contains("[\"color_A\"] = bind(NoiseMap_v2(\"/m/b_n2\"))"));
        assert!(text.contains("[\"albedo\"] = bind(BlendMap(\"/m/b_n3\"))"));
        assert!(text.contains("[\"roughness\"] = bind(NoiseMap_v2(\"/m/b_n2\"))"));
        // A loop is an error, as is a missing output.
        let looped: HashMap<(usize, usize), usize> = [((1, 0), 3), ((3, 0), 3)].into_iter().collect();
        assert!(compile_builder("/m/b", &nodes, |to, slot| looped.get(&(to, slot)).copied(), 1).is_err());
        assert!(compile_builder("/m/b", &nodes, |_, _| None, 2).is_err());
        // Normal input only takes a normal map.
        let bad: HashMap<(usize, usize), usize> = [((1, 10), 2)].into_iter().collect();
        assert!(compile_builder("/m/b", &nodes, |to, slot| bad.get(&(to, slot)).copied(), 1).is_err());
        let round = ShaderNode::from_params(&nodes[&2].to_params());
        assert_eq!(round.values, nodes[&2].values);
    }
}
