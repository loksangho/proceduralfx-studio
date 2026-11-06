// Volumes in the GPU viewport: smoke and fire (Pyro, fog VDBs) ray-marched through 3D
// textures instead of drawn as one splat per voxel, so they look smooth and continuous
// at any distance. The march stops at surfaces (the splats' depth buffer) and writes into
// the splats' blended targets, so a volume mixes with particles and glow the same way.
//
// The grid comes from the voxel points volumes travel as through the network (see
// volume.rs): each point's colour, opacity (Alpha, for one voxel's length) and emission.

use eframe::egui_wgpu;
use eframe::wgpu;
use wgpu::util::DeviceExt;

// A volume's voxels, ready to upload.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeGrid {
    // World centre of voxel (0, 0, 0), and the voxel's edge length.
    pub origin: [f32; 3],
    pub voxel: f32,
    pub dims: [u32; 3],
    // Per voxel, x fastest: colour premultiplied by opacity, and opacity; in display encoding.
    pub colour: Vec<[u8; 4]>,
    // Per voxel: emission e stored as e / (1 + e), so any brightness fits.
    pub glow: Vec<[u8; 4]>,
}

// Volumes bigger than this many voxels are left to the splats.
pub const MAX_VOXELS: usize = 8_000_000;

impl VolumeGrid {
    // The grid for points sitting in the voxels of a grid with this origin and voxel size
    // (Pyro jitters them inside their voxel), or None if they don't: moved off the grid by
    // a rotation, say, or too many voxels.
    pub fn from_points(origin: [f32; 3], voxel: f32, points: &[([f32; 3], [f32; 3], f32, [f32; 3])]) -> Option<VolumeGrid> {
        if points.is_empty() || !(voxel > 0.0) {
            return None;
        }
        let index = |p: [f32; 3]| -> Option<[i64; 3]> {
            let mut out = [0i64; 3];
            for a in 0..3 {
                let f = (p[a] - origin[a]) / voxel;
                let i = f.round();
                if !f.is_finite() || (f - i).abs() > 0.45 {
                    return None;
                }
                out[a] = i as i64;
            }
            Some(out)
        };
        let indices: Vec<[i64; 3]> = points.iter().map(|(p, ..)| index(*p)).collect::<Option<_>>()?;
        let (mut lo, mut hi) = ([i64::MAX; 3], [i64::MIN; 3]);
        for i in &indices {
            for a in 0..3 {
                lo[a] = lo[a].min(i[a]);
                hi[a] = hi[a].max(i[a]);
            }
        }
        // A voxel of empty space all round, so the edges fade out when filtered.
        let dims = [0, 1, 2].map(|a| (hi[a] - lo[a] + 3) as u32);
        if dims.iter().map(|d| *d as usize).product::<usize>() > MAX_VOXELS {
            return None;
        }
        let at = |i: [i64; 3]| ((i[0] - lo[0] + 1) + dims[0] as i64 * ((i[1] - lo[1] + 1) + dims[1] as i64 * (i[2] - lo[2] + 1))) as usize;
        let count = dims.iter().map(|d| *d as usize).product();
        let (mut colour, mut glow) = (vec![[0u8; 4]; count], vec![[0u8; 4]; count]);
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        for (i, (_, c, a, e)) in indices.into_iter().zip(points) {
            let a = a.clamp(0.0, 1.0);
            colour[at(i)] = [byte(c[0] * a), byte(c[1] * a), byte(c[2] * a), byte(a)];
            glow[at(i)] = [byte(e[0] / (1.0 + e[0].max(0.0))), byte(e[1] / (1.0 + e[1].max(0.0))), byte(e[2] / (1.0 + e[2].max(0.0))), 0];
        }
        let origin = [0, 1, 2].map(|a| origin[a] + (lo[a] - 1) as f32 * voxel);
        Some(VolumeGrid { origin, voxel, dims, colour, glow })
    }

    // The world box the voxels fill: from the first voxel's near corner to the last's far one.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let lo = self.origin.map(|o| o - self.voxel * 0.5);
        let hi = [0, 1, 2].map(|a| lo[a] + self.dims[a] as f32 * self.voxel);
        (lo, hi)
    }

    pub fn corners(&self) -> Vec<[f32; 3]> {
        let (lo, hi) = self.bounds();
        (0..8).map(|k| [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }]).collect()
    }
}

// The pipeline that marches volumes.
pub struct Pass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

// An uploaded volume.
pub struct Upload {
    colour: wgpu::TextureView,
    glow: wgpu::TextureView,
    uniform: wgpu::Buffer,
    pub corners: Vec<[f32; 3]>,
}

// How strongly volumes glow, beside the splats' GLOW (0.12, gpu_viewport.rs): set so a
// flame's core comes out as hot as it did drawn as overlapping splats.
const GLOW: f32 = 0.18;

const SHADER: &str = r#"
struct Camera {
    look_at: vec4<f32>,
    rotate: vec4<f32>,
    screen: vec4<f32>,
    lens: vec4<f32>,
    mode: vec4<f32>,
};
struct Volume {
    lo: vec4<f32>,   // the box's near corner, voxel size
    hi: vec4<f32>,   // far corner, glow strength
};
@group(0) @binding(0) var colour: texture_3d<f32>;
@group(0) @binding(1) var glow: texture_3d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
// The depth buffer read as plain numbers: OpenGL can't textureLoad a depth texture.
@group(0) @binding(3) var depth: texture_2d<f32>;
@group(0) @binding(4) var<uniform> volume: Volume;
@group(1) @binding(0) var<uniform> camera: Camera;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

// Camera space back to the world (the inverse of gpu_viewport's to_cam).
fn to_world(c: vec3<f32>) -> vec3<f32> {
    let r = camera.rotate;
    let z = c.z - camera.look_at.w;
    let y = c.y * r.y + z * r.x;
    let rz = -c.y * r.x + z * r.y;
    return vec3<f32>(c.x * r.w + rz * r.z, y, -c.x * r.z + rz * r.w) + camera.look_at.xyz;
}

// The point on the pixel's ray at camera depth t.
fn on_ray(pixel: vec2<f32>, t: f32) -> vec3<f32> {
    let d = vec2<f32>(pixel.x - camera.screen.x, camera.screen.y - pixel.y);
    if (camera.mode.x > 0.5) {
        return to_world(vec3<f32>(d / camera.lens.y, t));
    }
    return to_world(vec3<f32>(d / camera.lens.x * t, t));
}

struct Blended {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: vec4<f32>,
    @location(2) glow: vec4<f32>,
};

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> Blended {
    // The camera is in the view's own units (points), the target in pixels: on a Retina
    // screen, or a render bigger than the view, there are more pixels than points.
    let target_size = vec2<f32>(textureDimensions(depth));
    let pixel = position.xy * camera.screen.zw / target_size;
    let near = camera.lens.z;
    // The ray as world = a + b * t, t being camera depth.
    let a = on_ray(pixel, 0.0);
    let b = on_ray(pixel, 1.0) - a;
    let inv = 1.0 / b;
    let t0 = (volume.lo.xyz - a) * inv;
    let t1 = (volume.hi.xyz - a) * inv;
    let lo = min(t0, t1);
    let hi = max(t0, t1);
    var enter = max(max(max(lo.x, lo.y), lo.z), near);
    var leave = max(min(min(hi.x, hi.y), hi.z), near);
    // Stop at the surfaces in front.
    let d = textureLoad(depth, vec2<i32>(position.xy), 0).r;
    var surface = 1e30;
    if (camera.mode.x > 0.5) {
        surface = near + d * (camera.lens.w - near);
    } else if (d < 1.0) {
        surface = near / max(1.0 - d, 1e-7);
    }
    leave = min(leave, surface);
    if (leave <= enter) {
        discard;
    }
    let voxel = volume.lo.w;
    let march = (leave - enter) * sqrt(dot(b, b));
    let steps = clamp(i32(ceil(march / (voxel * 0.5))), 1, 768);
    let dt = (leave - enter) / f32(steps);
    let fraction = march / f32(steps) / voxel;
    // Each pixel starts a different way into its first step, so steps don't show as rings.
    let jitter = fract(sin(dot(pixel, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    let size = volume.hi.xyz - volume.lo.xyz;
    var through = 1.0;
    var light = vec3<f32>(0.0);
    var glowing = vec3<f32>(0.0);
    var depth_sum = 0.0;
    for (var k = 0; k < steps; k++) {
        let t = enter + (f32(k) + jitter) * dt;
        let uvw = (a + b * t - volume.lo.xyz) / size;
        let c = textureSampleLevel(colour, linear_clamp, uvw, 0.0);
        let e = textureSampleLevel(glow, linear_clamp, uvw, 0.0).rgb;
        if (c.a > 1e-4) {
            // Opacity over this step, from the opacity over a voxel's length.
            let o = 1.0 - pow(1.0 - min(c.a, 0.999), fraction);
            light += through * (c.rgb / c.a) * o;
            depth_sum += through * o * t;
            through *= 1.0 - o;
            glowing += e / max(vec3<f32>(1.0) - e, vec3<f32>(1e-3)) * o * volume.hi.w;
            if (through < 0.003) {
                break;
            }
        }
    }
    let alpha = 1.0 - through;
    if (alpha < 1e-4 && dot(glowing, glowing) < 1e-8) {
        discard;
    }
    // Weighted blended transparency, as the splats: weighed by how near it mostly is.
    let mean_t = depth_sum / max(alpha, 1e-6);
    let span = max(camera.mode.z - camera.mode.y, 1e-6);
    let s = clamp((mean_t - camera.mode.y) / span, 0.0, 1.0);
    let w = max(alpha, 1e-4) * clamp(3e3 * pow(1.0 - 0.95 * s, 3.0), 1e-2, 3e3);
    let a_out = min(alpha, 0.999);
    return Blended(vec4<f32>(light / max(alpha, 1e-6) * a_out, a_out) * w, vec4<f32>(a_out), vec4<f32>(glowing, 0.0));
}
"#;

impl Pass {
    // `camera_layout` is the viewport's camera bind group layout; `targets` the splats'
    // blended targets, blended as they are.
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, targets: &[Option<wgpu::ColorTargetState>]) -> Pass {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("viewport volumes"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport volumes"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("viewport volumes"), bind_group_layouts: &[&layout, camera_layout], push_constant_ranges: &[] });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport volumes"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viewport volumes"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Pass { pipeline, layout, sampler }
    }

    pub fn upload(&self, state: &egui_wgpu::RenderState, grid: &VolumeGrid) -> Upload {
        let device = &state.device;
        let size = wgpu::Extent3d { width: grid.dims[0], height: grid.dims[1], depth_or_array_layers: grid.dims[2] };
        let texture = |label: &str, texels: &[[u8; 4]]| {
            device
                .create_texture_with_data(
                    &state.queue,
                    &wgpu::TextureDescriptor { label: Some(label), size, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D3, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    bytemuck::cast_slice(texels),
                )
                .create_view(&Default::default())
        };
        let (lo, hi) = grid.bounds();
        let values: [f32; 8] = [lo[0], lo[1], lo[2], grid.voxel, hi[0], hi[1], hi[2], GLOW];
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("viewport volume"), contents: bytemuck::cast_slice(&values), usage: wgpu::BufferUsages::UNIFORM });
        Upload { colour: texture("viewport volume colour", &grid.colour), glow: texture("viewport volume glow", &grid.glow), uniform, corners: grid.corners() }
    }

    // Marches `volume` into the blended targets (`targets`, loaded), stopping at `depth`.
    pub fn draw(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, volume: &Upload, depth: &wgpu::TextureView, camera_group: &wgpu::BindGroup, targets: &[Option<wgpu::RenderPassColorAttachment>]) {
        let entry = |binding, resource| wgpu::BindGroupEntry { binding, resource };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport volume"),
            layout: &self.layout,
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&volume.colour)),
                entry(1, wgpu::BindingResource::TextureView(&volume.glow)),
                entry(2, wgpu::BindingResource::Sampler(&self.sampler)),
                entry(3, wgpu::BindingResource::TextureView(depth)),
                entry(4, volume.uniform.as_entire_binding()),
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("viewport volumes"), color_attachments: targets, depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, camera_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voxel_points_make_a_padded_grid_and_off_grid_points_dont() {
        let h = 0.5;
        let points = vec![([1.0, 2.0, 3.0], [1.0, 0.5, 0.0], 1.0, [0.0; 3]), ([1.5 + 0.1, 2.0, 3.0 - 0.15], [0.0, 0.0, 1.0], 0.5, [3.0, 0.0, 0.0])];
        let grid = VolumeGrid::from_points([0.0; 3], h, &points).unwrap();
        assert_eq!(grid.dims, [4, 3, 3], "two voxels across, a voxel of space all round");
        // The first point's voxel (index 2, 4, 6) is now at (1, 1, 1).
        assert_eq!(grid.origin, [0.5, 1.5, 2.5]);
        let at = |x: usize, y: usize, z: usize| x + 4 * (y + 3 * z);
        assert_eq!(grid.colour[at(1, 1, 1)], [255, 128, 0, 255]);
        assert_eq!(grid.colour[at(2, 1, 1)], [0, 0, 128, 128], "premultiplied");
        assert_eq!(grid.glow[at(2, 1, 1)][0], 191, "3 / (1 + 3)");
        assert_eq!(grid.colour[at(0, 0, 0)], [0; 4]);
        let (lo, hi) = grid.bounds();
        assert_eq!((lo, hi), ([0.25, 1.25, 2.25], [2.25, 2.75, 3.75]));
        // A point between voxel centres: not a grid.
        let off = vec![([1.0, 2.0, 3.0], [1.0; 3], 1.0, [0.0; 3]), ([1.25, 2.0, 3.0], [1.0; 3], 1.0, [0.0; 3])];
        assert!(VolumeGrid::from_points([0.0; 3], h, &off).is_none());
    }
}
