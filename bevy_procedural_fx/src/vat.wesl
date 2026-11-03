// Plays a vertex animation bake from ProceduralFX Studio: each vertex's UV picks its
// element, and the element's texels for the current frame move it. The same file serves
// the main pass and the prepass (shadows, depth, normals, motion vectors), so shadows move
// with the mesh.

@if(PREPASS_PIPELINE)
import bevy_pbr::prepass::io::{Vertex, VertexOutput, decompress_vertex};
@if(!PREPASS_PIPELINE)
import bevy_pbr::render::forward_io::{Vertex, VertexOutput, decompress_vertex};
import bevy_pbr::render::{mesh_functions, view_transformations::position_world_to_clip};

struct VatSettings {
    frame: f32,
    previous_frame: f32,
    frame_count: u32,
    element_count: u32,
    width: u32,
    rows_per_frame: u32,
    // 0 soft, 1 rigid, 2 fluid.
    mode: u32,
    looping: u32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> vat: VatSettings;
// Soft: offsets and normals. Rigid: centres and rotations. Fluid: positions and normals.
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var first: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var second: texture_2d<f32>;

fn fetch_first(e: u32, f: u32) -> vec4<f32> {
    return textureLoad(first, vec2<u32>(e % vat.width, f * vat.rows_per_frame + e / vat.width), 0);
}

fn fetch_second(e: u32, f: u32) -> vec4<f32> {
    return textureLoad(second, vec2<u32>(e % vat.width, f * vat.rows_per_frame + e / vat.width), 0);
}

// The element a vertex's UV picks: its column and row within a frame.
fn element(uv: vec2<f32>) -> u32 {
    let column = min(u32(max(uv.x * f32(vat.width), 0.0)), vat.width - 1u);
    let row = min(u32(max(uv.y * f32(vat.rows_per_frame), 0.0)), vat.rows_per_frame - 1u);
    return min(row * vat.width + column, vat.element_count - 1u);
}

// Turns v by the quaternion q (x, y, z, w).
fn turn(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

struct Moved {
    position: vec3<f32>,
    normal: vec3<f32>,
}

// Where a vertex is at `frame` (from 0; fractions blend to the next frame).
fn animate(position: vec3<f32>, normal: vec3<f32>, uv: vec2<f32>, frame: f32) -> Moved {
    let e = element(uv);
    let count = f32(vat.frame_count);
    var at = clamp(frame, 0.0, count - 1.0);
    if vat.looping != 0u {
        at = frame - floor(frame / count) * count;
    }
    let f0 = min(u32(floor(at)), vat.frame_count - 1u);
    var f1 = min(f0 + 1u, vat.frame_count - 1u);
    if vat.looping != 0u {
        f1 = (f0 + 1u) % vat.frame_count;
    }
    let blend = at - floor(at);
    var out: Moved;
    if vat.mode == 0u {
        out.position = position + mix(fetch_first(e, f0).xyz, fetch_first(e, f1).xyz, blend);
        out.normal = normalize(mix(fetch_second(e, f0).xyz, fetch_second(e, f1).xyz, blend));
    } else if vat.mode == 1u {
        let rest_centre = fetch_first(e, 0u).xyz;
        let centre = mix(fetch_first(e, f0).xyz, fetch_first(e, f1).xyz, blend);
        let q0 = fetch_second(e, f0);
        var q1 = fetch_second(e, f1);
        if dot(q0, q1) < 0.0 {
            q1 = -q1;
        }
        let q = normalize(mix(q0, q1, blend));
        out.position = turn(q, position - rest_centre) + centre;
        out.normal = turn(q, normal);
    } else {
        // A changing mesh can't blend between frames: each frame's corners are its own.
        out.position = fetch_first(e, f0).xyz;
        out.normal = fetch_second(e, f0).xyz;
    }
    return out;
}

@if(!PREPASS_PIPELINE)
@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    var normal = vec3<f32>(0.0, 1.0, 0.0);
@if(VERTEX_NORMALS)
    normal = vertex.normal;
    var uv = vec2<f32>(0.5);
@if(VERTEX_UVS_A)
    uv = vertex.uv;
    let moved = animate(vertex.position, normal, uv, vat.frame);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(moved.normal, vertex_in.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(moved.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
@if(VERTEX_UVS_A)
    out.uv = vertex.uv;
@if(VERTEX_UVS_B)
    out.uv_b = vertex.uv_b;
@if(VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, vertex.tangent, vertex_in.instance_index);
@if(VERTEX_COLORS)
    out.color = vertex.color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, world_from_local[3]);
    return out;
}

@if(PREPASS_PIPELINE)
@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    var normal = vec3<f32>(0.0, 1.0, 0.0);
@if(VERTEX_NORMALS)
    normal = vertex.normal;
    var uv = vec2<f32>(0.5);
@if(VERTEX_UVS_A)
    uv = vertex.uv;
    let moved = animate(vertex.position, normal, uv, vat.frame);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(moved.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}
@if(VERTEX_UVS_A)
    out.uv = vertex.uv;
@if(VERTEX_UVS_B)
    out.uv_b = vertex.uv_b;
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(moved.normal, vertex_in.instance_index);
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, vertex.tangent, vertex_in.instance_index);
@if(VERTEX_COLORS)
    out.color = vertex.color;
@if(MOTION_VECTOR_PREPASS) {
    let before = animate(vertex.position, normal, uv, vat.previous_frame);
    let previous_world_from_local = mesh_functions::get_previous_world_from_local(vertex_in.instance_index);
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(previous_world_from_local, vec4<f32>(before.position, 1.0));
}
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, world_from_local[3]);
    return out;
}
