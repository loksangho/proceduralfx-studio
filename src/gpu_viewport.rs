// The viewport's shaded surfaces and wireframe on the GPU (wgpu). Used whenever the
// app runs on wgpu; raster.rs draws them otherwise. The image goes into a texture
// egui shows like any other, so the points, curves and handles drawn over it are
// unchanged.
//
// The scene is uploaded once, in world space, with each corner's lit colour: the
// viewport's lighting doesn't depend on the camera, so orbiting, panning and
// zooming only change a few camera numbers, and the vertex shader does the
// viewport's own camera transform (to_cam and project_cam in main.rs). It's
// uploaded again only when the mesh, the lights or the display mode change.
//
// The image matches raster.rs: colours and texture coordinates are interpolated
// perspective-correctly, the nearest surface wins at each pixel, triangles are cut
// at the near plane, and edges are nudged toward the camera so they show on the
// faces they outline. Colours stay in the display's (sRGB) encoding, as in
// raster.rs and egui's own textures, so egui shows the same values a CPU image
// would give it.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;
use wgpu::util::DeviceExt;

use crate::raster;

const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;

// One corner: world position, colour (0..1, display encoding) and texture coordinates.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    // 1 where the texture applies.
    pub textured: f32,
    pub color: [f32; 4],
    pub uv: [f32; 2],
    pub _pad: [f32; 2],
}

impl Vertex {
    pub fn new(position: [f32; 3], color: [f32; 4], uv: Option<[f32; 2]>) -> Self {
        Vertex { position, textured: uv.is_some() as u8 as f32, color, uv: uv.unwrap_or_default(), _pad: [0.0; 2] }
    }
}

// The viewport camera, as main.rs's viewport sets it up.
#[derive(Clone, Copy)]
pub struct Camera {
    pub target: [f32; 3],
    // sin and cos of cam_rot_x, then of cam_rot_y.
    pub rotation: [f32; 4],
    pub zoom: f32,
    // The projection centre, from the viewport's top-left corner, and the viewport's size (points).
    pub center: egui::Vec2,
    pub size: egui::Vec2,
    pub focal: f32,
    pub ortho_scale: f32,
    pub orthographic: bool,
    // Triangles are cut here, as in the CPU path.
    pub near: f32,
}

impl Camera {
    // The rotation field for cam_rot_x and cam_rot_y.
    pub fn rotation(x: f32, y: f32) -> [f32; 4] {
        let ((sin_x, cos_x), (sin_y, cos_y)) = (x.sin_cos(), y.sin_cos());
        [sin_x, cos_x, sin_y, cos_y]
    }

    // main.rs's to_cam: world to camera space, +z in front.
    pub fn to_cam(&self, p: [f32; 3]) -> [f32; 3] {
        let [sin_x, cos_x, sin_y, cos_y] = self.rotation;
        let v = [p[0] - self.target[0], p[1] - self.target[1], p[2] - self.target[2]];
        let rx = v[0] * cos_y - v[2] * sin_y;
        let rz = v[0] * sin_y + v[2] * cos_y;
        let ry = v[1] * cos_x - rz * sin_x;
        let rrz = v[1] * sin_x + rz * cos_x;
        [rx, ry, rrz + self.zoom]
    }

    // main.rs's project_cam, from the viewport's top-left corner.
    pub fn project(&self, c: [f32; 3]) -> egui::Vec2 {
        if self.orthographic {
            self.center + egui::vec2(c[0], -c[1]) * self.ortho_scale
        } else {
            self.center + egui::vec2(c[0], -c[1]) / c[2] * self.focal
        }
    }

    // The shader's camera; `far` is the orthographic depth range's far end.
    fn uniform(&self, far: f32) -> [f32; 20] {
        let (t, r) = (self.target, self.rotation);
        [
            t[0], t[1], t[2], self.zoom,
            r[0], r[1], r[2], r[3],
            self.center.x, self.center.y, self.size.x.max(1.0), self.size.y.max(1.0),
            self.focal, self.ortho_scale, self.near, far.max(self.near + 1e-3),
            self.orthographic as u8 as f32, 0.0, 0.0, 0.0,
        ]
    }
}

const SHADER: &str = r#"
struct Camera {
    look_at: vec4<f32>, // xyz, zoom
    rotate: vec4<f32>, // sin x, cos x, sin y, cos y
    screen: vec4<f32>, // centre, size
    lens: vec4<f32>,   // focal, ortho scale, near, far
    mode: vec4<f32>,   // orthographic
};
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(1) @binding(0) var<uniform> camera: Camera;

struct In {
    @location(0) position: vec3<f32>,
    @location(1) textured: f32,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) textured: f32,
};

// main.rs's to_cam and project_cam, then clip space: depth 1 - near/z under
// perspective (so it's cut at the near plane, and the same surface wins as on the
// CPU), linear in z between near and far under orthographic.
fn to_clip(p: vec3<f32>) -> vec4<f32> {
    let v = p - camera.look_at.xyz;
    let r = camera.rotate;
    let rx = v.x * r.w - v.z * r.z;
    let rz = v.x * r.z + v.z * r.w;
    let ry = v.y * r.y - rz * r.x;
    let c = vec3<f32>(rx, ry, v.y * r.x + rz * r.y + camera.look_at.w);
    let size = camera.screen.zw;
    let near = camera.lens.z;
    if (camera.mode.x > 0.5) {
        let s = camera.screen.xy + vec2<f32>(c.x, -c.y) * camera.lens.y;
        let depth = (c.z - near) / (camera.lens.w - near);
        return vec4<f32>(s.x / size.x * 2.0 - 1.0, 1.0 - s.y / size.y * 2.0, depth, 1.0);
    }
    let w = c.z;
    let s = camera.screen.xy * w + vec2<f32>(c.x, -c.y) * camera.lens.x;
    return vec4<f32>(s.x / size.x * 2.0 - w, w - s.y / size.y * 2.0, w - near, w);
}

@vertex
fn vs_surface(in: In) -> Out {
    return Out(to_clip(in.position), in.color, in.uv, in.textured);
}

// Edges: pulled toward the camera so they win against the faces they outline
// (raster.rs's constant bias; the faces' pipeline adds the slope-scaled part).
@vertex
fn vs_edge(in: In) -> Out {
    var p = to_clip(in.position);
    if (p.w > 0.0) {
        let d = p.z / p.w;
        p.z = (d - max((1.0 - d) * 2e-4, 1e-6)) * p.w;
    }
    return Out(p, in.color, in.uv, in.textured);
}

// Premultiplied output, as egui's textures are.
@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    var rgb = in.color.rgb;
    if (in.textured > 0.5) {
        rgb = rgb * textureSample(image, image_sampler, in.uv).rgb;
    }
    let a = in.color.a;
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * a, a);
}
"#;

pub struct GpuViewport {
    render_state: egui_wgpu::RenderState,
    surfaces: wgpu::RenderPipeline,
    edges: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    camera: wgpu::Buffer,
    camera_group: wgpu::BindGroup,
    // The scene's texture (Quickshade), by the CPU texture it came from.
    texture: Option<(Arc<raster::Texture>, wgpu::BindGroup)>,
    // Untextured draws bind a 1 x 1 white texture.
    blank: wgpu::BindGroup,
    scene: Option<Scene>,
    target: Option<Target>,
}

// The uploaded scene: faces (three vertices each) and edges (two each), the texture
// the faces use, and the corners of its bounding box (for the orthographic depth range).
struct Scene {
    key: u64,
    faces: Option<(wgpu::Buffer, u32)>,
    edges: Option<(wgpu::Buffer, u32)>,
    texture: Option<Arc<raster::Texture>>,
    corners: Vec<[f32; 3]>,
}

// The image at one size: multisampled colour and depth, resolved into `image`.
struct Target {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    image: wgpu::TextureView,
    // The resolved image itself (the tests read it back).
    #[cfg_attr(not(test), allow(dead_code))]
    texture: wgpu::Texture,
    id: egui::TextureId,
}

impl GpuViewport {
    pub fn new(render_state: egui_wgpu::RenderState) -> Self {
        let device = &render_state.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("viewport"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let camera = device.create_buffer(&wgpu::BufferDescriptor { label: Some("viewport camera"), size: 80, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("viewport camera"), layout: &camera_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera.as_entire_binding() }] });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("viewport"), bind_group_layouts: &[&layout, &camera_layout], push_constant_ranges: &[] });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Float32x2];
        let pipeline = |label: &str, entry: &str, topology: wgpu::PrimitiveTopology, depth_write: bool, bias: wgpu::DepthBiasState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: depth_write, depth_compare: wgpu::CompareFunction::LessEqual, stencil: Default::default(), bias }),
                multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
                multiview: None,
                cache: None,
            })
        };
        // Faces are pushed back by their depth slope (grazing faces change fast), as raster.rs biases its edges.
        let surfaces = pipeline("viewport surfaces", "vs_surface", wgpu::PrimitiveTopology::TriangleList, true, wgpu::DepthBiasState { constant: 0, slope_scale: 1.5, clamp: 0.0 });
        let edges = pipeline("viewport edges", "vs_edge", wgpu::PrimitiveTopology::LineList, false, Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viewport texture"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = raster::Texture { width: 1, height: 1, pixels: vec![[255; 4]] };
        let blank = texture_bind_group(&render_state, &layout, &sampler, &white);
        GpuViewport { render_state, surfaces, edges, layout, sampler, camera, camera_group, texture: None, blank, scene: None, target: None }
    }

    // Whether the scene uploaded last has this key (what the mesh, lights and display mode hash to).
    pub fn has_scene(&self, key: u64) -> bool {
        self.scene.as_ref().is_some_and(|s| s.key == key)
    }

    // Uploads faces (triangles, three vertices each) and edges (two each) under `key`,
    // with the texture the textured faces use.
    pub fn set_scene(&mut self, key: u64, faces: &[Vertex], edges: &[Vertex], texture: Option<Arc<raster::Texture>>) {
        let device = &self.render_state.device;
        let buffer = |label: &str, vertices: &[Vertex]| {
            (!vertices.is_empty()).then(|| (device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytemuck::cast_slice(vertices), usage: wgpu::BufferUsages::VERTEX }), vertices.len() as u32))
        };
        let (lo, hi) = faces.iter().chain(edges).fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(lo, hi), v| (std::array::from_fn(|k| lo[k].min(v.position[k])), std::array::from_fn(|k| hi[k].max(v.position[k]))));
        let corners = if lo[0] <= hi[0] { (0..8).map(|i| std::array::from_fn(|k| if i >> k & 1 == 0 { lo[k] } else { hi[k] })).collect() } else { Vec::new() };
        if let Some(texture) = &texture {
            if self.texture.as_ref().is_none_or(|(t, _)| !Arc::ptr_eq(t, texture)) {
                self.texture = Some((texture.clone(), texture_bind_group(&self.render_state, &self.layout, &self.sampler, texture)));
            }
        }
        self.scene = Some(Scene { key, faces: buffer("viewport faces", faces), edges: buffer("viewport edges", edges), texture, corners });
    }

    // Draws the uploaded scene through `camera` into an image of `size` pixels and
    // returns the egui texture showing it. Edges are drawn after the faces,
    // depth-tested against them but not each other.
    pub fn draw(&mut self, size: [u32; 2], camera: &Camera) -> egui::TextureId {
        let size = [size[0].max(1), size[1].max(1)];
        let state = &self.render_state;
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let target = Target::new(state, size, self.target.as_ref().map(|t| t.id));
            self.target = Some(target);
        }
        // Orthographic depth runs from the near plane to just past the scene's farthest corner.
        let deepest = self.scene.iter().flat_map(|s| &s.corners).map(|&p| camera.to_cam(p)[2]).fold(camera.near, f32::max);
        let far = deepest + (deepest - camera.near).abs().max(1e-3) * 0.01;
        state.queue.write_buffer(&self.camera, 0, bytemuck::cast_slice(&camera.uniform(far)));
        let target = self.target.as_ref().expect("made above");
        let textured = self.scene.as_ref().and_then(|s| s.texture.as_ref());
        let bind_group = textured.and(self.texture.as_ref()).map_or(&self.blank, |(_, b)| b);
        let mut encoder = state.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewport") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa,
                    resolve_target: Some(&target.image),
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, bind_group, &[]);
            pass.set_bind_group(1, &self.camera_group, &[]);
            if let Some(scene) = &self.scene {
                for ((buffer, count), pipeline) in [(&scene.faces, &self.surfaces), (&scene.edges, &self.edges)].into_iter().filter_map(|(b, p)| Some((b.as_ref()?, p))) {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        }
        state.queue.submit([encoder.finish()]);
        target.id
    }
}

impl Target {
    fn new(state: &egui_wgpu::RenderState, size: [u32; 2], id: Option<egui::TextureId>) -> Self {
        let device = &state.device;
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let texture = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: extent, mip_level_count: 1, sample_count: samples, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] })
        };
        let msaa = texture("viewport msaa", COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        let depth = texture("viewport depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        // Values in display encoding, like egui's own (Rgba8Unorm) textures, which it uses as sampled.
        let image = texture("viewport image", COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let shown = image.create_view(&Default::default());
        let mut renderer = state.renderer.write();
        let id = match id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(device, &shown, wgpu::FilterMode::Linear, id);
                id
            }
            None => renderer.register_native_texture(device, &shown, wgpu::FilterMode::Linear),
        };
        Target { size, msaa, depth, image: shown, texture: image, id }
    }
}

fn texture_bind_group(state: &egui_wgpu::RenderState, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, source: &raster::Texture) -> wgpu::BindGroup {
    let extent = wgpu::Extent3d { width: source.width.max(1) as u32, height: source.height.max(1) as u32, depth_or_array_layers: 1 };
    // Plain (not sRGB): raster.rs multiplies the texture's encoded values too.
    let texture = state.device.create_texture_with_data(
        &state.queue,
        &wgpu::TextureDescriptor { label: Some("viewport texture"), size: extent, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: COLOR, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&source.pixels),
    );
    let view = texture.create_view(&Default::default());
    state.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewport texture"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A GPU viewport without a window, or None where there's no GPU (as on some CI machines).
    fn headless() -> Option<GpuViewport> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
        let renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, Default::default());
        let state = egui_wgpu::RenderState { adapter, available_adapters: Vec::new(), device, queue, target_format: wgpu::TextureFormat::Rgba8Unorm, renderer: Arc::new(egui::mutex::RwLock::new(renderer)) };
        Some(GpuViewport::new(state))
    }

    // The last image drawn, premultiplied, in display encoding.
    fn read(gpu: &GpuViewport) -> Vec<[u8; 4]> {
        let target = gpu.target.as_ref().expect("drawn");
        let [w, h] = target.size;
        let row = (w * 4).div_ceil(256) * 256;
        let state = &gpu.render_state;
        let buffer = state.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut encoder = state.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        state.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("mapped"));
        state.device.poll(wgpu::PollType::wait_indefinitely()).expect("finished");
        let data = buffer.slice(..).get_mapped_range();
        (0..h as usize).flat_map(|y| (0..w as usize).map(move |x| (y, x))).map(|(y, x)| std::array::from_fn(|c| data[y * row as usize + x * 4 + c])).collect()
    }

    // raster.rs's straight colours, premultiplied as the GPU image is.
    fn premultiplied(c: [u8; 4]) -> [u8; 4] {
        let a = c[3] as u32;
        [((c[0] as u32 * a + 127) / 255) as u8, ((c[1] as u32 * a + 127) / 255) as u8, ((c[2] as u32 * a + 127) / 255) as u8, c[3]]
    }

    fn camera(orthographic: bool, w: f32, h: f32) -> Camera {
        Camera { target: [0.2, 0.1, -0.3], rotation: Camera::rotation(0.45, 0.7), zoom: 4.0, center: egui::vec2(w * 0.5 + 6.0, h * 0.5 - 4.0), size: egui::vec2(w, h), focal: h * 3.5, ortho_scale: h * 0.9, orthographic, near: 0.05 }
    }

    // The CPU path for comparison: main.rs projects each corner, and raster.rs draws it.
    // (Every corner here is in front of the near plane, so nothing needs cutting.)
    fn on_cpu(camera: &Camera, w: usize, h: usize, faces: &[Vertex], texture: &raster::Texture, flat: bool) -> raster::Raster {
        let mut cpu = raster::Raster::new(w, h, camera.orthographic);
        let screen = |v: &Vertex| {
            let c = camera.to_cam(v.position);
            let s = camera.project(c);
            raster::ScreenVertex { x: s.x, y: s.y, z: c[2] }
        };
        let triangles: Vec<raster::Triangle> = faces
            .chunks(3)
            .enumerate()
            .map(|(i, t)| raster::Triangle {
                v: [screen(&t[0]), screen(&t[1]), screen(&t[2])],
                rgb: if flat { [[(i + 1) as f32 / 32.0, 0.0, 0.0]; 3] } else { [0, 1, 2].map(|k| [t[k].color[0], t[k].color[1], t[k].color[2]]) },
                uv: (t[0].textured > 0.5 && !flat).then(|| ([t[0].uv, t[1].uv, t[2].uv], texture)),
            })
            .collect();
        cpu.draw(&triangles, &[]);
        cpu
    }

    #[test]
    fn gpu_matches_the_cpu_rasterizer() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let (w, h) = (192usize, 160usize);
        let checker = Arc::new(raster::Texture { width: 2, height: 2, pixels: vec![[255, 255, 255, 255], [40, 40, 40, 255], [40, 40, 40, 255], [255, 255, 255, 255]] });
        // Overlapping and interpenetrating triangles at different depths, one textured.
        let faces: Vec<Vertex> = (0..14)
            .flat_map(|i| {
                let f = i as f32;
                let o = [(f * 0.37) % 1.3 - 0.8, (f * 0.23) % 1.0 - 0.5, (f * 0.29) % 1.1 - 0.6];
                let corners = [[o[0], o[1], o[2]], [o[0] + 0.9, o[1] + 0.1, o[2] - 0.4 + (f % 3.0) * 0.3], [o[0] + 0.2, o[1] + 0.8, o[2] + 0.5 - (f % 4.0) * 0.2]];
                let rgb = [[(f % 5.0) / 5.0, 0.6, 0.3], [0.2, (f % 3.0) / 3.0, 0.9], [0.9, 0.4, (f % 7.0) / 7.0]];
                let uv = [[0.0, 0.0], [3.0, 0.0], [0.0, 3.0]];
                (0..3).map(move |k| Vertex::new(corners[k], [rgb[k][0], rgb[k][1], rgb[k][2], 1.0], (i == 3).then_some(uv[k])))
            })
            .collect();
        gpu.set_scene(1, &faces, &[], Some(checker.clone()));
        for orthographic in [false, true] {
            let camera = camera(orthographic, w as f32, h as f32);
            let cpu = on_cpu(&camera, w, h, &faces, &checker, false);
            // Which triangle covers each pixel: the same scene in flat colours.
            let ids = on_cpu(&camera, w, h, &faces, &checker, true);
            gpu.draw([w as u32, h as u32], &camera);
            let image = read(&gpu);
            // Compare away from edges, where the GPU's antialiasing differs on purpose.
            let (mut compared, mut close) = (0, 0);
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    if (0..9).any(|n| ids.color[(y + n / 3 - 1) * w + x + n % 3 - 1] != ids.color[y * w + x]) {
                        continue;
                    }
                    compared += 1;
                    let (a, b) = (premultiplied(cpu.color[y * w + x]), image[y * w + x]);
                    close += (0..4).all(|c| a[c].abs_diff(b[c]) <= 3) as usize;
                }
            }
            let covered = ids.color.iter().filter(|c| c[3] > 0).count();
            assert!(covered > w * h / 4, "orthographic {orthographic}: the triangles fill the view ({covered} pixels)");
            assert!(compared > w * h / 2, "most of the image is away from edges");
            let share = close as f64 / compared as f64;
            assert!(share > 0.995, "orthographic {orthographic}: {close} of {compared} pixels match");
        }
    }

    #[test]
    fn edges_show_on_their_faces_and_hide_behind_others() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        // No rotation: the camera sits at z = -5 looking along +z, at a face at z = 0 filling the view.
        let camera = Camera { target: [0.0; 3], rotation: Camera::rotation(0.0, 0.0), zoom: 5.0, center: egui::vec2(10.0, 10.0), size: egui::vec2(20.0, 20.0), focal: 20.0, ortho_scale: 4.0, orthographic: false, near: 0.1 };
        let white = [1.0, 1.0, 1.0, 1.0];
        let face: Vec<Vertex> = [[-9.0, 9.0, 0.0], [9.0, 9.0, 0.0], [-9.0, -9.0, 0.0]].map(|p| Vertex::new(p, white, None)).to_vec();
        // A row of pixels across the face (y = 5.5 on screen), on it or behind it.
        let edge = |z: f32| -> Vec<Vertex> {
            let depth = z + 5.0;
            [0.0, 10.0].map(|x: f32| Vertex::new([(x - 10.0) / 20.0 * depth, (10.0 - 5.5) / 20.0 * depth, z], [1.0, 0.0, 0.0, 1.0], None)).to_vec()
        };
        gpu.set_scene(1, &face, &edge(3.0), None);
        gpu.draw([20, 20], &camera);
        assert_eq!(read(&gpu)[5 * 20 + 3], [255, 255, 255, 255], "an edge behind the surface is hidden");
        gpu.set_scene(2, &face, &edge(0.0), None);
        gpu.draw([20, 20], &camera);
        assert_eq!(read(&gpu)[5 * 20 + 3], [255, 0, 0, 255], "an edge on the surface shows");
        assert_eq!(read(&gpu)[19 * 20 + 19][3], 0, "outside the face stays empty");
    }

    #[test]
    fn triangles_are_cut_at_the_near_plane() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        // A floor running from behind the camera to far ahead: the part in front still draws.
        let camera = Camera { target: [0.0; 3], rotation: Camera::rotation(0.0, 0.0), zoom: 5.0, center: egui::vec2(16.0, 16.0), size: egui::vec2(32.0, 32.0), focal: 32.0, ortho_scale: 4.0, orthographic: false, near: 0.1 };
        let grey = [0.5, 0.5, 0.5, 1.0];
        let floor: Vec<Vertex> = [[-4.0, -1.0, 8.0], [4.0, -1.0, 8.0], [0.0, -1.0, -30.0]].map(|p| Vertex::new(p, grey, None)).to_vec();
        gpu.set_scene(1, &floor, &[], None);
        gpu.draw([32, 32], &camera);
        let image = read(&gpu);
        assert!(image[31 * 32 + 16][3] > 0, "the floor just in front of the camera shows at the bottom");
        assert_eq!(image[2 * 32 + 16][3], 0, "nothing above the horizon");
    }
}
