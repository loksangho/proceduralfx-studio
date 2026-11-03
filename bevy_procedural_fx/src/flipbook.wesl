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
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> settings: FlipbookSettings;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var sheet: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var sheet_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var motion: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var motion_sampler: sampler;

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
    var color: vec4<f32>;
    if settings.blend == 0u {
        color = premultiplied(textureSample(sheet, sheet_sampler, in_sheet(f0, local)));
    } else {
        var shift = vec2<f32>(0.0);
        // The last frame's motion only leads into the first if the sheet loops.
        let wraps = f1 < f0;
        if settings.blend == 2u && (!wraps || settings.motion_loops != 0u) {
            shift = (textureSample(motion, motion_sampler, in_sheet(f0, local)).rg - vec2<f32>(0.5)) * 2.0 * settings.strength;
        }
        let a = premultiplied(textureSample(sheet, sheet_sampler, in_sheet(f0, local - shift * t)));
        let b = premultiplied(textureSample(sheet, sheet_sampler, in_sheet(f1, local + shift * (1.0 - t))));
        color = mix(a, b, t);
    }
    var out = vec4<f32>(color.rgb / max(color.a, 1e-5), color.a) * settings.color;
@if(TONEMAP_IN_SHADER)
    out = tonemapping::tone_mapping(out, view.color_grading);
@if(SRGB_OUTPUT)
    out = vec4<f32>(linear_to_srgb(out.rgb), out.a);
    return out;
}
