// Plays a sprite sheet from ProceduralFX Studio smoothly: between two frames, the first
// slides forward along its motion vectors and the next slides back, and the two blend.
// Without motion vectors (or with blending set to Fade), it cross-fades.

import bevy_sprite_render::mesh2d::{vertex_output::VertexOutput, view_bindings::view};
@if(TONEMAP_IN_SHADER)
import bevy_core_pipeline::tonemapping;
@if(SRGB_OUTPUT)
import bevy_render::color_operations::linear_to_srgb;

struct FlipbookSettings {
    color: vec4<f32>,
    // The frame to show, from 0; fractions are part of the way to the next.
    frame: f32,
    // The largest shift the motion colours hold, in fractions of a frame.
    strength: f32,
    columns: u32,
    rows: u32,
    frame_count: u32,
    looping: u32,
    // 0 shows whole frames, 1 cross-fades, 2 follows the motion vectors.
    blend: u32,
    // Whether the last frame's motion leads into the first.
    motion_loops: u32,
    light_count: u32,
    ambient: vec4<f32>,
    // World position (z: height) and radius; colour times intensity.
    light_positions: array<vec4<f32>, 8>,
    light_colors: array<vec4<f32>, 8>,
    // The glow sheet's strength (1: as rendered), and whether there is one.
    glow: f32,
    has_glow: u32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> settings: FlipbookSettings;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var sheet: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var sheet_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var motion: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var motion_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var normal_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var glow_sheet: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var glow_sampler: sampler;

// The light a glow sheet adds at `strength`: its glow g (stored as g / (1 + g)) that many
// times over, less the 1 - exp(-g) of it the sheet's colour already holds.
fn glow_light(x: vec3<f32>, strength: f32) -> vec3<f32> {
    let c = min(x, vec3<f32>(254.0 / 255.0));
    let g = c / (vec3<f32>(1.0) - c);
    return g * strength - (vec3<f32>(1.0) - exp(-g));
}

// The light reaching a surface facing `n` (red right, green up, blue out of the screen)
// at `at`, from the ambient and each light.
fn lighting(n: vec3<f32>, at: vec3<f32>) -> vec3<f32> {
    var total = settings.ambient.rgb;
    for (var i = 0u; i < settings.light_count; i++) {
        let light = settings.light_positions[i];
        let to_light = vec3<f32>(light.xy - at.xy, light.z);
        let fade = clamp(1.0 - length(to_light.xy) / light.w, 0.0, 1.0);
        total += settings.light_colors[i].rgb * max(dot(n, normalize(to_light)), 0.0) * fade * fade;
    }
    return total;
}

// Where `local` (0 to 1 across a frame, from its top left) is in the sheet for frame `f`,
// kept half a texel inside the cell so it never picks up the next one.
fn in_sheet(f: u32, local: vec2<f32>) -> vec2<f32> {
    let grid = vec2<f32>(f32(settings.columns), f32(settings.rows));
    let texel = 0.5 * grid / vec2<f32>(textureDimensions(sheet));
    let cell = vec2<f32>(f32(f % settings.columns), f32(f / settings.columns));
    return (cell + clamp(local, texel, vec2<f32>(1.0) - texel)) / grid;
}

// Premultiplied, so transparent edges blend without dark fringes.
fn premultiplied(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let count = f32(settings.frame_count);
    var at = clamp(settings.frame, 0.0, count - 1.0);
    if settings.looping != 0u {
        at = settings.frame - floor(settings.frame / count) * count;
    }
    let f0 = min(u32(floor(at)), settings.frame_count - 1u);
    var f1 = min(f0 + 1u, settings.frame_count - 1u);
    if settings.looping != 0u {
        f1 = (f0 + 1u) % settings.frame_count;
    }
    var t = at - floor(at);
    let local = in.uv;
    if settings.blend == 0u {
        t = 0.0;
    }
    var shift = vec2<f32>(0.0);
    // The last frame's motion only leads into the first if the sheet loops.
    let wraps = f1 < f0;
    if settings.blend == 2u && (!wraps || settings.motion_loops != 0u) {
        shift = (textureSample(motion, motion_sampler, in_sheet(f0, local)).rg - vec2<f32>(0.5)) * 2.0 * settings.strength;
    }
    // Where this pixel comes from in this frame and the next.
    let ua = in_sheet(f0, local - shift * t);
    let ub = in_sheet(f1, local + shift * (1.0 - t));
    let color = mix(premultiplied(textureSample(sheet, sheet_sampler, ua)), premultiplied(textureSample(sheet, sheet_sampler, ub)), t);
    var out = vec4<f32>(color.rgb / max(color.a, 1e-5), color.a) * settings.color;
    if settings.light_count > 0u {
        let encoded = mix(textureSample(normal_map, normal_sampler, ua).rgb, textureSample(normal_map, normal_sampler, ub).rgb, t);
        let n = normalize(encoded * 2.0 - vec3<f32>(1.0));
        out = vec4<f32>(out.rgb * lighting(n, in.world_position.xyz), out.a);
    }
    // Premultiplied (the pipeline blends it so), with the glow's light on top.
    var light = vec3<f32>(0.0);
    if settings.has_glow != 0u {
        let encoded = mix(textureSample(glow_sheet, glow_sampler, ua).rgb, textureSample(glow_sheet, glow_sampler, ub).rgb, t);
        light = glow_light(encoded, settings.glow) * settings.color.a;
    }
    out = vec4<f32>(max(out.rgb * out.a + light, vec3<f32>(0.0)), out.a);
@if(TONEMAP_IN_SHADER)
    out = tonemapping::tone_mapping(out, view.color_grading);
@if(SRGB_OUTPUT)
    out = vec4<f32>(linear_to_srgb(out.rgb), out.a);
    return out;
}
