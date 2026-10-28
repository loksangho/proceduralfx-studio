// The viewport's shaded surfaces and wireframe on the GPU (wgpu), drawing the same
// screen-space triangles and edges raster.rs does on the CPU. The image goes into
// a texture egui shows like any other, so the points, curves and handles drawn
// over it are unchanged. Used whenever the app runs on wgpu; raster.rs otherwise.
//
// The image matches raster.rs: colours and texture coordinates are interpolated
// perspective-correctly (each vertex carries its camera depth as w), the nearest
// surface wins at each pixel, and edges are nudged toward the camera so they show
// on the faces they outline. Colours stay in the display's (sRGB) encoding, as in
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

// One corner: clip-space position, colour (0..1, display encoding) and texture coordinates.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
    pub uv: [f32; 2],
    // 1 where the texture applies.
    pub textured: f32,
    pub _pad: f32,
}

// How screen positions and camera depths become clip space.
pub struct Projection {
    // The viewport's top-left corner and size, in points.
    pub origin: egui::Pos2,
    pub size: egui::Vec2,
    pub orthographic: bool,
    // Perspective: the near plane (every depth is at least this). Orthographic: the depth range.
    pub near: f32,
    pub far: f32,
}

impl Projection {
    pub fn vertex(&self, p: egui::Pos2, z: f32, rgba: [f32; 4], uv: Option<[f32; 2]>) -> Vertex {
        let x = (p.x - self.origin.x) / self.size.x.max(1.0) * 2.0 - 1.0;
        let y = 1.0 - (p.y - self.origin.y) / self.size.y.max(1.0) * 2.0;
        // Perspective: depth 1 - near/z (the CPU's 1/z key, so the same surface wins), w = z.
        // Orthographic: depth linear in z, w = 1.
        let position = if self.orthographic {
            let d = ((z - self.near) / (self.far - self.near).max(1e-6)).clamp(0.0, 1.0);
            [x, y, d, 1.0]
        } else {
            let w = z.max(self.near * 0.5).max(1e-6);
            [x * w, y * w, (w - self.near * 0.5).max(0.0), w]
        };
        Vertex { position, color: rgba, uv: uv.unwrap_or_default(), textured: uv.is_some() as u8 as f32, _pad: 0.0 }
    }
}

const SHADER: &str = r#"
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) textured: f32,
};

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

@vertex
fn vs_surface(@location(0) position: vec4<f32>, @location(1) color: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) textured: f32) -> Out {
    return Out(position, color, uv, textured);
}

// Edges: pulled toward the camera so they win against the faces they outline
// (raster.rs's constant bias; the faces' pipeline adds the slope-scaled part).
@vertex
fn vs_edge(@location(0) position: vec4<f32>, @location(1) color: vec4<f32>, @location(2) uv: vec2<f32>, @location(3) textured: f32) -> Out {
    var p = position;
    let d = p.z / p.w;
    p.z = max(d - max((1.0 - d) * 2e-4, 1e-6), 0.0) * p.w;
    return Out(p, color, uv, textured);
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
    // The scene's texture (Quickshade), by the CPU texture it came from.
    texture: Option<(Arc<raster::Texture>, wgpu::BindGroup)>,
    // Untextured draws bind a 1 x 1 white texture.
    blank: wgpu::BindGroup,
    target: Option<Target>,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("viewport"), bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x2, 3 => Float32];
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
        GpuViewport { render_state, surfaces, edges, layout, sampler, texture: None, blank, target: None }
    }

    // Draws the triangles (faces) and lines (edges, two vertices each) into an image of
    // `size` pixels and returns the egui texture showing it. Lines are drawn after the
    // faces, depth-tested against them but not each other.
    pub fn draw(&mut self, size: [u32; 2], faces: &[Vertex], edges: &[Vertex], texture: Option<&Arc<raster::Texture>>) -> egui::TextureId {
        let size = [size[0].max(1), size[1].max(1)];
        let state = &self.render_state;
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let target = Target::new(state, size, self.target.as_ref().map(|t| t.id));
            self.target = Some(target);
        }
        if let Some(texture) = texture {
            if self.texture.as_ref().is_none_or(|(t, _)| !Arc::ptr_eq(t, texture)) {
                self.texture = Some((texture.clone(), texture_bind_group(state, &self.layout, &self.sampler, texture)));
            }
        }
        let target = self.target.as_ref().expect("made above");
        let bind_group = texture.and(self.texture.as_ref()).map_or(&self.blank, |(_, b)| b);
        let buffer = |label: &str, vertices: &[Vertex]| {
            (!vertices.is_empty()).then(|| state.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytemuck::cast_slice(vertices), usage: wgpu::BufferUsages::VERTEX }))
        };
        let (face_buffer, edge_buffer) = (buffer("viewport faces", faces), buffer("viewport edges", edges));
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
            if let Some(buffer) = &face_buffer {
                pass.set_pipeline(&self.surfaces);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..faces.len() as u32, 0..1);
            }
            if let Some(buffer) = &edge_buffer {
                pass.set_pipeline(&self.edges);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..edges.len() as u32, 0..1);
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
        let texture = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages, view_formats: &[wgpu::TextureFormat]| {
            device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: extent, mip_level_count: 1, sample_count: samples, dimension: wgpu::TextureDimension::D2, format, usage, view_formats })
        };
        let msaa = texture("viewport msaa", COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT, &[]).create_view(&Default::default());
        let depth = texture("viewport depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT, &[]).create_view(&Default::default());
        // Values in display encoding, like egui's own (Rgba8Unorm) textures, which it uses as sampled.
        let image = texture("viewport image", COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, &[]);
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

    #[test]
    fn gpu_matches_the_cpu_rasterizer() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let (w, h) = (192usize, 160usize);
        let checker = Arc::new(raster::Texture { width: 2, height: 2, pixels: vec![[255, 255, 255, 255], [40, 40, 40, 255], [40, 40, 40, 255], [255, 255, 255, 255]] });
        // Overlapping and interpenetrating triangles at different depths, one textured.
        let s = |x: f32, y: f32, z: f32| raster::ScreenVertex { x, y, z };
        let scene: Vec<([raster::ScreenVertex; 3], [[f32; 3]; 3], Option<[[f32; 2]; 3]>)> = (0..14)
            .map(|i| {
                let f = i as f32;
                let (ox, oy) = ((f * 37.0) % 130.0, (f * 23.0) % 100.0);
                let v = [s(ox, oy, 1.5 + f * 0.3), s(ox + 70.0, oy + 10.0, 4.0 - (f % 3.0)), s(ox + 20.0, oy + 60.0, 2.0 + (f % 4.0) * 0.7)];
                let rgb = [[(f % 5.0) / 5.0, 0.6, 0.3], [0.2, (f % 3.0) / 3.0, 0.9], [0.9, 0.4, (f % 7.0) / 7.0]];
                (v, rgb, (i == 3).then_some([[0.0, 0.0], [3.0, 0.0], [0.0, 3.0]]))
            })
            .collect();
        for orthographic in [false, true] {
            let mut cpu = raster::Raster::new(w, h, orthographic);
            let triangles: Vec<raster::Triangle> = scene.iter().map(|(v, rgb, uv)| raster::Triangle { v: *v, rgb: *rgb, uv: uv.map(|uv| (uv, &*checker)) }).collect();
            cpu.draw(&triangles, &[]);
            // Which triangle covers each pixel: the same scene in flat colours.
            let mut ids = raster::Raster::new(w, h, orthographic);
            let flat: Vec<raster::Triangle> = scene.iter().enumerate().map(|(i, (v, _, _))| raster::Triangle { v: *v, rgb: [[(i + 1) as f32 / 32.0, 0.0, 0.0]; 3], uv: None }).collect();
            ids.draw(&flat, &[]);
            let projection = Projection { origin: egui::pos2(0.0, 0.0), size: egui::vec2(w as f32, h as f32), orthographic, near: if orthographic { 0.5 } else { 1.0 }, far: 8.0 };
            let faces: Vec<Vertex> = scene
                .iter()
                .flat_map(|(v, rgb, uv)| (0..3).map(move |k| (v[k], rgb[k], uv.map(|uv| uv[k]))))
                .map(|(v, c, uv)| projection.vertex(egui::pos2(v.x, v.y), v.z, [c[0], c[1], c[2], 1.0], uv))
                .collect();
            gpu.draw([w as u32, h as u32], &faces, &[], Some(&checker));
            let image = read(&gpu);
            // Compare away from edges, where the GPU's antialiasing differs on purpose.
            let (mut compared, mut close) = (0, 0);
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let here = cpu.color[y * w + x];
                    if (0..9).any(|n| ids.color[(y + n / 3 - 1) * w + x + n % 3 - 1] != ids.color[y * w + x]) {
                        continue;
                    }
                    compared += 1;
                    let (a, b) = (premultiplied(here), image[y * w + x]);
                    close += (0..4).all(|c| a[c].abs_diff(b[c]) <= 3) as usize;
                }
            }
            let share = close as f64 / compared as f64;
            assert!(compared > w * h / 2, "most of the image is away from edges");
            assert!(share > 0.995, "orthographic {orthographic}: {close} of {compared} pixels match");
        }
    }

    #[test]
    fn edges_show_on_their_faces_and_hide_behind_others() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let projection = Projection { origin: egui::pos2(0.0, 0.0), size: egui::vec2(20.0, 20.0), orthographic: false, near: 0.5, far: f32::INFINITY };
        let white = [1.0, 1.0, 1.0, 1.0];
        let face: Vec<Vertex> = [(0.0, 0.0), (20.0, 0.0), (0.0, 20.0)].iter().map(|&(x, y)| projection.vertex(egui::pos2(x, y), 1.0, white, None)).collect();
        let edge = |z: f32| -> Vec<Vertex> { [0.0, 10.0].iter().map(|&x| projection.vertex(egui::pos2(x, 5.5), z, [1.0, 0.0, 0.0, 1.0], None)).collect() };
        gpu.draw([20, 20], &face, &edge(4.0), None);
        assert_eq!(read(&gpu)[5 * 20 + 3], [255, 255, 255, 255], "an edge behind the surface is hidden");
        gpu.draw([20, 20], &face, &edge(1.0), None);
        assert_eq!(read(&gpu)[5 * 20 + 3], [255, 0, 0, 255], "an edge on the surface shows");
        assert_eq!(read(&gpu)[19 * 20 + 19][3], 0, "outside the face stays empty");
    }
}
