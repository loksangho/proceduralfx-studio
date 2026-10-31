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
// Which way round an outward-facing triangle of the app's meshes winds on screen
// (the outline culls the faces toward the camera).
const TOON_FRONT: wgpu::FrontFace = wgpu::FrontFace::Cw;

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

// A corner of a toon-shaded face (Toon Shader): its base colour, how lit it is (0
// shadow side to 1 facing the light, before banding), its smooth normal (for the
// outline) and which entry of the toon table its material is. The banding happens
// per pixel, so band edges are crisp however coarse the mesh.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ToonVertex {
    pub position: [f32; 3],
    pub light: f32,
    pub color: [f32; 4],
    pub normal: [f32; 3],
    pub material: f32,
}

// A toon material as the shader reads it (see shading::Values::toon_look).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ToonMaterial {
    // Bands, softness, shadow size (terminator shift), highlight size.
    pub shade: [f32; 4],
    // Shadow tint, and outline width in pixels.
    pub shadow: [f32; 4],
    // Highlight colour times its strength.
    pub highlight: [f32; 4],
    // Emission colour times its intensity.
    pub emission: [f32; 4],
    pub outline: [f32; 4],
}

// Toon materials one scene can use; more share the last.
pub const MAX_TOONS: usize = 16;

impl ToonMaterial {
    // fs_toon at one point, without the antialiasing: for the CPU viewport.
    pub fn shade(&self, base: [f32; 3], light: f32) -> [f32; 3] {
        let bands = self.shade[0].max(2.0);
        let t = (light + self.shade[2]).clamp(0.0, 1.0);
        let x = t * bands;
        let w = self.shade[1].clamp(1e-4, 1.0);
        let f = x.fract();
        let k = ((f - (1.0 - w)) / w).clamp(0.0, 1.0);
        let band = ((x.floor() + k * k * (3.0 - 2.0 * k)) / (bands - 1.0)).clamp(0.0, 1.0);
        let spot = if t >= 1.0 - 0.3 * self.shade[3] { 1.0 } else { 0.0 };
        std::array::from_fn(|c| (base[c] * (self.shadow[c] + (1.0 - self.shadow[c]) * band) + self.highlight[c] * spot + self.emission[c]).clamp(0.0, 1.0))
    }
}

// A point drawn as a camera-facing sprite. Dots: an opaque disc `radius` points
// across the screen. Splats (smoke, fire, volumes): a soft disc `radius` world
// units across, with opacity color[3] at its centre, glowing by `emission`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Point {
    pub position: [f32; 3],
    pub radius: f32,
    pub color: [f32; 4],
    pub emission: [f32; 4],
}

// One piece of a hair or other curve, from `a` to `b`, `width` across at each end in
// world units (Hair Generate's width attribute). A negative width draws a plain line a
// pixel wide, for curves that don't say how thick they are.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Strand {
    pub a: [f32; 3],
    pub width_a: f32,
    pub b: [f32; 3],
    pub width_b: f32,
    pub color: [f32; 4],
}

impl Point {
    pub fn new(position: [f32; 3], radius: f32, color: [f32; 4], emission: [f32; 3]) -> Self {
        Point { position, radius, color, emission: [emission[0], emission[1], emission[2], 0.0] }
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

    // The shader's camera; `far` is the orthographic depth range's far end, and
    // `splat_depth` the camera depths the splats span (for their blending weights).
    fn uniform(&self, far: f32, splat_depth: [f32; 2]) -> [f32; 20] {
        let (t, r) = (self.target, self.rotation);
        [
            t[0], t[1], t[2], self.zoom,
            r[0], r[1], r[2], r[3],
            self.center.x, self.center.y, self.size.x.max(1.0), self.size.y.max(1.0),
            self.focal, self.ortho_scale, self.near, far.max(self.near + 1e-3),
            self.orthographic as u8 as f32, splat_depth[0], splat_depth[1], 0.0,
        ]
    }
}

const SHADER: &str = r#"
struct Camera {
    look_at: vec4<f32>, // xyz, zoom
    rotate: vec4<f32>, // sin x, cos x, sin y, cos y
    screen: vec4<f32>, // centre, size
    lens: vec4<f32>,   // focal, ortho scale, near, far
    mode: vec4<f32>,   // orthographic, the splats' nearest and farthest camera depth
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

// main.rs's to_cam: world to camera space, +z in front.
fn to_cam(p: vec3<f32>) -> vec3<f32> {
    let v = p - camera.look_at.xyz;
    let r = camera.rotate;
    let rx = v.x * r.w - v.z * r.z;
    let rz = v.x * r.z + v.z * r.w;
    let ry = v.y * r.y - rz * r.x;
    return vec3<f32>(rx, ry, v.y * r.x + rz * r.y + camera.look_at.w);
}

fn to_clip(p: vec3<f32>) -> vec4<f32> {
    return cam_to_clip(to_cam(p));
}

// main.rs's project_cam, then clip space: depth 1 - near/z under perspective (so
// it's cut at the near plane, and the same surface wins as on the CPU), linear in
// z between near and far under orthographic.
fn cam_to_clip(c: vec3<f32>) -> vec4<f32> {
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

// Points: a quad around each, two triangles, corners at (-1..1, -1..1).
struct PointIn {
    @location(0) position: vec3<f32>,
    @location(1) radius: f32,
    @location(2) color: vec4<f32>,
    @location(3) emission: vec4<f32>,
};
struct PointOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) emission: vec3<f32>,
    @location(2) corner: vec2<f32>,
    // Dots: radius in points. Splats: camera depth.
    @location(3) extra: f32,
};
fn corner(i: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    return corners[i];
}

// Dots keep their size on screen, and are pulled toward the camera (more than
// edges) so a point on a surface shows over it.
@vertex
fn vs_dot(@builtin(vertex_index) i: u32, point: PointIn) -> PointOut {
    let k = corner(i);
    var p = to_clip(point.position);
    p.x += k.x * point.radius / camera.screen.z * 2.0 * p.w;
    p.y += k.y * point.radius / camera.screen.w * 2.0 * p.w;
    if (p.w > 0.0) {
        let d = p.z / p.w;
        p.z = (d - max((1.0 - d) * 2e-3, 1e-5)) * p.w;
    }
    return PointOut(p, point.color, vec3<f32>(0.0), k, point.radius);
}

// Partial coverage, written premultiplied like the rest of the image. Alpha to coverage
// already keeps about `a` of the pixel's samples, and each keeps the alpha written, so
// writing the square root gives the pixel `a` in all, and its colour times `a`.
fn covered(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    let s = sqrt(clamp(a, 0.0, 1.0));
    return vec4<f32>(rgb * s, s);
}

// Antialiased by coverage: the alpha at the disc's rim thins the samples drawn.
@fragment
fn fs_dot(in: PointOut) -> @location(0) vec4<f32> {
    let edge = clamp((1.0 - length(in.corner)) * in.extra + 0.5, 0.0, 1.0);
    return covered(in.color.rgb, edge);
}

// Strands: each piece of a curve as a ribbon facing the camera, as wide as the hair at
// each end, so it has thickness however it's turned. Hair finer than a pixel stays a pixel
// wide but shows only as much as it covers, so fine, dense hair looks soft, not wiry.
struct StrandIn {
    @location(0) a: vec3<f32>,
    @location(1) width_a: f32,
    @location(2) b: vec3<f32>,
    @location(3) width_b: f32,
    @location(4) color: vec4<f32>,
};
struct StrandOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Across the ribbon, -1 to 1; its width in pixels; how much of a pixel it covers.
    @location(1) side: f32,
    @location(2) shown: f32,
    @location(3) coverage: f32,
};
@vertex
fn vs_strand(@builtin(vertex_index) i: u32, s: StrandIn) -> StrandOut {
    var ends = array<vec2<f32>, 6>(vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let k = ends[i];
    let ca = to_cam(s.a);
    let cb = to_cam(s.b);
    let near = camera.lens.z;
    // Behind the camera: nothing.
    if (ca.z < near && cb.z < near) {
        return StrandOut(vec4<f32>(0.0, 0.0, 2.0, 1.0), s.color, 0.0, 1.0, 0.0);
    }
    let size = camera.screen.zw;
    let pa = cam_to_clip(ca);
    let pb = cam_to_clip(cb);
    var along = (pb.xy / pb.w - pa.xy / pa.w) * size;
    if (length(along) < 1e-6) {
        along = vec2<f32>(1.0, 0.0);
    }
    let dir = normalize(along);
    let normal = vec2<f32>(-dir.y, dir.x);
    let c = select(ca, cb, k.x > 0.5);
    var p = select(pa, pb, k.x > 0.5);
    let width = select(s.width_a, s.width_b, k.x > 0.5);
    let scale = select(camera.lens.x / max(c.z, 1e-6), camera.lens.y, camera.mode.x > 0.5);
    let pixels = select(width * scale, 1.0, width < 0.0);
    let shown = max(pixels, 1.0);
    // Half a pixel more on each side, to fade the edge over (see fs_strand).
    p.x += normal.x * k.y * (shown + 1.0) / size.x * p.w;
    p.y += normal.y * k.y * (shown + 1.0) / size.y * p.w;
    // Pulled toward the camera a little, as edges are, so roots don't sink into the skin.
    if (p.w > 0.0) {
        let d = p.z / p.w;
        p.z = (d - max((1.0 - d) * 2e-4, 1e-6)) * p.w;
    }
    return StrandOut(p, s.color, k.y, shown, min(pixels, 1.0));
}

// Solid inside, fading over the last pixel at each edge (the ribbon is drawn a pixel wider
// than the hair for that), so it covers its width in pixels; and faded by how much of a
// pixel fine hair covers.
@fragment
fn fs_strand(in: StrandOut) -> @location(0) vec4<f32> {
    let to_edge = (1.0 - abs(in.side)) * (in.shown + 1.0) * 0.5;
    return covered(in.color.rgb, clamp(to_edge, 0.0, 1.0) * in.coverage);
}

// Splats are sized in the world, kept between half a point and 120 points across
// (as on the CPU), facing the camera.
@vertex
fn vs_splat(@builtin(vertex_index) i: u32, point: PointIn) -> PointOut {
    let k = corner(i);
    let c = to_cam(point.position);
    let scale = select(camera.lens.x / max(c.z, 1e-6), camera.lens.y, camera.mode.x > 0.5);
    let shown = point.radius * scale;
    let radius = point.radius * clamp(shown, 0.5, 120.0) / max(shown, 1e-6);
    let p = cam_to_clip(c + vec3<f32>(k * radius, 0.0));
    return PointOut(p, point.color, point.emission.rgb, k, c.z);
}

// How strongly the emission glows in the viewport: chosen by eye so a flame keeps its
// orange edge and hot core (the render has its own, physical, strength).
const GLOW: f32 = 0.12;

// Weighted blended transparency (McGuire and Bavoil 2013): no sorting. Nearer
// splats weigh more; the glow adds up regardless of order.
struct Blended {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: vec4<f32>,
    @location(2) glow: vec4<f32>,
};
@fragment
fn fs_splat(in: PointOut) -> Blended {
    let r2 = dot(in.corner, in.corner);
    if (r2 > 1.0) {
        discard;
    }
    let falloff = (1.0 - r2) * (1.0 - r2);
    let a = clamp(in.color.a * falloff, 0.0, 0.99);
    let span = max(camera.mode.z - camera.mode.y, 1e-6);
    let t = clamp((in.extra - camera.mode.y) / span, 0.0, 1.0);
    let w = a * clamp(3e3 * pow(1.0 - 0.95 * t, 3.0), 1e-2, 3e3);
    // Glow as much as the splat is there (many overlap along each view ray).
    return Blended(vec4<f32>(in.color.rgb * a, a) * w, vec4<f32>(a), vec4<f32>(in.emission * a * GLOW, 0.0));
}

// Toon shading (Toon Shader): the light quantised into bands from the shadow tint up to
// the base colour, a hard highlight where it's brightest, and outlines drawn as the
// back faces pushed out along their normals by a number of pixels (an inverted hull).
struct Toon {
    shade: vec4<f32>,     // bands, softness, shadow size, highlight size
    shadow: vec4<f32>,    // shadow tint, outline width (pixels)
    highlight: vec4<f32>, // colour × strength
    emission: vec4<f32>,  // colour × intensity
    outline: vec4<f32>,   // colour
};
@group(2) @binding(0) var<uniform> toons: array<Toon, 16>;

struct ToonIn {
    @location(0) position: vec3<f32>,
    @location(1) light: f32,
    @location(2) color: vec4<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) material: f32,
};
struct ToonOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) light: f32,
    @location(2) @interpolate(flat) material: u32,
};

@vertex
fn vs_toon(in: ToonIn) -> ToonOut {
    return ToonOut(to_clip(in.position), in.color, in.light, u32(in.material));
}

@fragment
fn fs_toon(in: ToonOut) -> @location(0) vec4<f32> {
    let m = toons[min(in.material, 15u)];
    let bands = max(m.shade.x, 2.0);
    let t = clamp(in.light + m.shade.z, 0.0, 1.0);
    // The band (0: shadow, bands - 1: fully lit), stepping up at each band's top over
    // the softness, or over a pixel so the edge is antialiased.
    let x = t * bands;
    let w = clamp(max(m.shade.y, fwidth(x)), 1e-4, 1.0);
    let level = floor(x) + smoothstep(1.0 - w, 1.0, fract(x));
    let band = clamp(level / (bands - 1.0), 0.0, 1.0);
    var rgb = in.color.rgb * mix(m.shadow.rgb, vec3<f32>(1.0), band);
    let edge = 1.0 - 0.3 * m.shade.w;
    let spot = smoothstep(edge - fwidth(t), edge, t);
    rgb = rgb + m.highlight.rgb * spot + m.emission.rgb;
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

// The outline's hull: each corner moved out across the screen, along its normal,
// by the outline's width in pixels (so it's as thick near as far, at any render size).
@vertex
fn vs_outline(in: ToonIn) -> ToonOut {
    let material = min(u32(in.material), 15u);
    let m = toons[material];
    let width = m.shadow.w;
    let c = to_cam(in.position);
    var p = cam_to_clip(c);
    if (width <= 0.0) {
        // No outline: off screen.
        return ToonOut(vec4<f32>(0.0, 0.0, 2.0, 1.0), m.outline, 0.0, material);
    }
    // The normal's direction on screen (camera space is linear in the position).
    let n = to_cam(in.position + in.normal) - c;
    var d = n.xy;
    if (camera.mode.x < 0.5) {
        d = (n.xy * c.z - c.xy * n.z) / max(c.z * c.z, 1e-8);
    }
    if (length(d) > 1e-8) {
        let dir = normalize(d);
        p.x += dir.x * width * 2.0 / camera.screen.z * p.w;
        p.y += dir.y * width * 2.0 / camera.screen.w * p.w;
    }
    return ToonOut(p, m.outline, 0.0, material);
}

@fragment
fn fs_outline(in: ToonOut) -> @location(0) vec4<f32> {
    return vec4<f32>(clamp(in.color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
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

// The splats, blended (above), over the surfaces: premultiplied colour and glow.
const COMPOSITE: &str = r#"
@group(0) @binding(0) var accum: texture_2d<f32>;
@group(0) @binding(1) var reveal: texture_2d<f32>;
@group(0) @binding(2) var glow: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(position.xy);
    let sum = textureLoad(accum, at, 0);
    let alpha = 1.0 - textureLoad(reveal, at, 0).r;
    let colour = clamp(sum.rgb / max(sum.a, 1e-5), vec3<f32>(0.0), vec3<f32>(1.0));
    // Bright glow rolls off toward white instead of clipping.
    let light = vec3<f32>(1.0) - exp(-textureLoad(glow, at, 0).rgb);
    return vec4<f32>(colour * alpha + light, alpha);
}
"#;

pub struct GpuViewport {
    render_state: egui_wgpu::RenderState,
    surfaces: wgpu::RenderPipeline,
    edges: wgpu::RenderPipeline,
    dots: wgpu::RenderPipeline,
    strands: wgpu::RenderPipeline,
    // The splats' passes: the surfaces' depth again (without antialiasing), the
    // splats, then the composite over the image.
    depth_only: wgpu::RenderPipeline,
    // See-through faces: depth-tested, not written, so they don't hide each other.
    see_through: wgpu::RenderPipeline,
    // Toon faces, their outlines, and their depth for the splats.
    toon: wgpu::RenderPipeline,
    outline: wgpu::RenderPipeline,
    toon_depth: wgpu::RenderPipeline,
    toon_table: wgpu::Buffer,
    toon_group: wgpu::BindGroup,
    splats: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    camera: wgpu::Buffer,
    camera_group: wgpu::BindGroup,
    // The scene's texture (Quickshade), by the CPU texture it came from.
    texture: Option<(Arc<raster::Texture>, wgpu::BindGroup)>,
    // Untextured draws bind a 1 x 1 white texture.
    blank: wgpu::BindGroup,
    scene: Option<Scene>,
    points: Option<Points>,
    // The viewport's image, and the egui texture showing it.
    target: Option<(Target, egui::TextureId)>,
    blended: Option<BlendTargets>,
}

// The uploaded points: dots and splats, one instance each, and curves (hair), one
// instance per strand piece.
struct Points {
    key: u64,
    dots: Option<(wgpu::Buffer, u32)>,
    splats: Option<(wgpu::Buffer, u32)>,
    curves: Option<(wgpu::Buffer, u32)>,
    // The splats' positions' bounding box corners (for their blending weights).
    corners: Vec<[f32; 3]>,
    // Every dot's, splat's and strand's bounding box corners, so the depth range
    // reaches them (an orthographic view's far plane is fitted to what's drawn).
    extent: Vec<[f32; 3]>,
}

// The 8 corners of the box around `positions` (none when there are none).
fn box_corners(positions: impl Iterator<Item = [f32; 3]>) -> Vec<[f32; 3]> {
    let (lo, hi) = positions.fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(lo, hi), p| (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k]))));
    if lo[0] <= hi[0] { (0..8).map(|i| std::array::from_fn(|k| if i >> k & 1 == 0 { lo[k] } else { hi[k] })).collect() } else { Vec::new() }
}

// The splats' buffers at one size: depth, the weighted colour sum, how much shows
// through, and the glow, without antialiasing.
struct BlendTargets {
    size: [u32; 2],
    depth: wgpu::TextureView,
    accum: wgpu::TextureView,
    reveal: wgpu::TextureView,
    glow: wgpu::TextureView,
    group: wgpu::BindGroup,
}

const ACCUM: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const REVEAL: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

// The uploaded scene: faces (three vertices each) and edges (two each), the texture
// the faces use, and the corners of its bounding box (for the orthographic depth range).
struct Scene {
    key: u64,
    faces: Option<(wgpu::Buffer, u32)>,
    // Faces with any corner less than opaque, drawn after the rest, blended.
    see_through: Option<(wgpu::Buffer, u32)>,
    edges: Option<(wgpu::Buffer, u32)>,
    toon: Option<(wgpu::Buffer, u32)>,
    texture: Option<Arc<raster::Texture>>,
    corners: Vec<[f32; 3]>,
}

// The image at one size: multisampled colour and depth, resolved into `image`.
struct Target {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    image: wgpu::TextureView,
    // The resolved image itself (off-screen renders read it back).
    texture: wgpu::Texture,
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
                // The splats' fragments read it too (their blending weights).
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
        let see_through = pipeline("viewport see-through faces", "vs_surface", wgpu::PrimitiveTopology::TriangleList, false, Default::default());
        let point_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Float32x4];
        let point_buffers = [wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Point>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &point_attributes }];
        let dots = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport dots"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_dot"), compilation_options: Default::default(), buffers: &point_buffers },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_dot"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::LessEqual, stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: SAMPLES, alpha_to_coverage_enabled: true, ..Default::default() },
            multiview: None,
            cache: None,
        });
        let strand_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Float32x3, 3 => Float32, 4 => Float32x4];
        let strands = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport strands"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_strand"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Strand>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &strand_attributes }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_strand"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::LessEqual, stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: SAMPLES, alpha_to_coverage_enabled: true, ..Default::default() },
            multiview: None,
            cache: None,
        });
        let depth_only = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport depth"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_surface"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }],
            },
            fragment: None,
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::LessEqual, stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        // Toon faces and outlines: their own vertices, and the toon table as group 2.
        let toon_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport toon"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let toon_table = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport toon"),
            size: (std::mem::size_of::<ToonMaterial>() * MAX_TOONS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let toon_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("viewport toon"), layout: &toon_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: toon_table.as_entire_binding() }] });
        let toon_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("viewport toon"), bind_group_layouts: &[&layout, &camera_layout, &toon_layout], push_constant_ranges: &[] });
        let toon_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Float32x3, 4 => Float32];
        let toon_buffers = [wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<ToonVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &toon_attributes }];
        let toon_pipeline = |label: &str, vertex: &str, fragment: Option<&str>, cull_mode: Option<wgpu::Face>, samples: u32, bias: wgpu::DepthBiasState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&toon_pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &toon_buffers },
                fragment: fragment.map(|entry| wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { cull_mode, front_face: TOON_FRONT, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::LessEqual, stencil: Default::default(), bias }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview: None,
                cache: None,
            })
        };
        let toon = toon_pipeline("viewport toon", "vs_toon", Some("fs_toon"), None, SAMPLES, wgpu::DepthBiasState { constant: 0, slope_scale: 1.5, clamp: 0.0 });
        // The hull shows only where it faces away, around and behind the surface.
        let outline = toon_pipeline("viewport outline", "vs_outline", Some("fs_outline"), Some(wgpu::Face::Front), SAMPLES, Default::default());
        let toon_depth = toon_pipeline("viewport toon depth", "vs_toon", None, None, 1, Default::default());
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let keep = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::OneMinusSrc, operation: wgpu::BlendOperation::Add };
        let splats = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport splats"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_splat"), compilation_options: Default::default(), buffers: &point_buffers },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_splat"),
                compilation_options: Default::default(),
                targets: &[
                    Some(wgpu::ColorTargetState { format: ACCUM, blend: Some(wgpu::BlendState { color: add, alpha: add }), write_mask: wgpu::ColorWrites::ALL }),
                    Some(wgpu::ColorTargetState { format: REVEAL, blend: Some(wgpu::BlendState { color: keep, alpha: keep }), write_mask: wgpu::ColorWrites::ALL }),
                    Some(wgpu::ColorTargetState { format: ACCUM, blend: Some(wgpu::BlendState { color: add, alpha: add }), write_mask: wgpu::ColorWrites::ALL }),
                ],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: false, depth_compare: wgpu::CompareFunction::LessEqual, stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("viewport composite"), source: wgpu::ShaderSource::Wgsl(COMPOSITE.into()) });
        let unfiltered = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("viewport composite"), entries: &[unfiltered(0), unfiltered(1), unfiltered(2)] });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport composite"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("viewport composite"), bind_group_layouts: &[&composite_layout], push_constant_ranges: &[] })),
            vertex: wgpu::VertexState { module: &composite_shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
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
        GpuViewport { render_state, surfaces, edges, dots, strands, depth_only, see_through, toon, outline, toon_depth, toon_table, toon_group, splats, composite, composite_layout, layout, sampler, camera, camera_group, texture: None, blank, scene: None, points: None, target: None, blended: None }
    }

    // Whether the scene uploaded last has this key (what the mesh, lights and display mode hash to).
    pub fn has_scene(&self, key: u64) -> bool {
        self.scene.as_ref().is_some_and(|s| s.key == key)
    }

    // Uploads faces (triangles, three vertices each) and edges (two each) under `key`,
    // with the texture the textured faces use.
    // Toon faces (three ToonVertex each) use the materials in `toons` (at most MAX_TOONS).
    pub fn set_scene(&mut self, key: u64, faces: &[Vertex], edges: &[Vertex], toon: &[ToonVertex], toons: &[ToonMaterial], texture: Option<Arc<raster::Texture>>) {
        let device = &self.render_state.device;
        let buffer = |label: &str, vertices: &[Vertex]| {
            (!vertices.is_empty()).then(|| (device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytemuck::cast_slice(vertices), usage: wgpu::BufferUsages::VERTEX }), vertices.len() as u32))
        };
        let toon_buffer = (!toon.is_empty()).then(|| (device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("viewport toon faces"), contents: bytemuck::cast_slice(toon), usage: wgpu::BufferUsages::VERTEX }), toon.len() as u32));
        if !toons.is_empty() {
            let mut table = [ToonMaterial::default(); MAX_TOONS];
            for (slot, material) in table.iter_mut().zip(toons) {
                *slot = *material;
            }
            self.render_state.queue.write_buffer(&self.toon_table, 0, bytemuck::cast_slice(&table));
        }
        let positions = faces.iter().chain(edges).map(|v| v.position).chain(toon.iter().map(|v| v.position));
        let (lo, hi) = positions.fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(lo, hi), v| (std::array::from_fn(|k| lo[k].min(v[k])), std::array::from_fn(|k| hi[k].max(v[k]))));
        let corners = if lo[0] <= hi[0] { (0..8).map(|i| std::array::from_fn(|k| if i >> k & 1 == 0 { lo[k] } else { hi[k] })).collect() } else { Vec::new() };
        if let Some(texture) = &texture {
            if self.texture.as_ref().is_none_or(|(t, _)| !Arc::ptr_eq(t, texture)) {
                self.texture = Some((texture.clone(), texture_bind_group(&self.render_state, &self.layout, &self.sampler, texture)));
            }
        }
        // See-through faces go last, blended over what's behind them without hiding it.
        let (opaque, see_through): (Vec<&[Vertex]>, Vec<&[Vertex]>) = faces.chunks(3).partition(|t| t.iter().all(|v| v.color[3] >= 0.999));
        let (opaque, see_through): (Vec<Vertex>, Vec<Vertex>) = (opaque.concat(), see_through.concat());
        self.scene = Some(Scene {
            key,
            faces: buffer("viewport faces", &opaque),
            see_through: buffer("viewport see-through faces", &see_through),
            edges: buffer("viewport edges", edges),
            toon: toon_buffer,
            texture,
            corners,
        });
    }

    // Whether the points uploaded last have this key.
    pub fn has_points(&self, key: u64) -> bool {
        self.points.as_ref().is_some_and(|p| p.key == key)
    }

    // Uploads the dots, splats and curve pieces (drawn as strands) under `key`.
    pub fn set_points(&mut self, key: u64, dots: &[Point], splats: &[Point], curves: &[Strand]) {
        let device = &self.render_state.device;
        let buffer = |label: &str, bytes: &[u8], count: usize| {
            (count > 0).then(|| (device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage: wgpu::BufferUsages::VERTEX }), count as u32))
        };
        let corners = box_corners(splats.iter().map(|p| p.position));
        let extent = box_corners(dots.iter().chain(splats).map(|p| p.position).chain(curves.iter().flat_map(|c| [c.a, c.b])));
        self.points = Some(Points {
            key,
            dots: buffer("viewport dots", bytemuck::cast_slice(dots), dots.len()),
            splats: buffer("viewport splats", bytemuck::cast_slice(splats), splats.len()),
            curves: buffer("viewport curves", bytemuck::cast_slice(curves), curves.len()),
            corners,
            extent,
        });
    }

    // Draws the uploaded scene through `camera` into an image of `size` pixels and
    // returns the egui texture showing it. Edges are drawn after the faces,
    // depth-tested against them but not each other.
    pub fn draw(&mut self, size: [u32; 2], camera: &Camera) -> egui::TextureId {
        let size = [size[0].max(1), size[1].max(1)];
        if self.target.as_ref().is_none_or(|(t, _)| t.size != size) {
            let target = Target::new(&self.render_state, size);
            let mut renderer = self.render_state.renderer.write();
            let id = match self.target.as_ref() {
                Some((_, id)) => {
                    renderer.update_egui_texture_from_wgpu_texture(&self.render_state.device, &target.image, wgpu::FilterMode::Linear, *id);
                    *id
                }
                None => renderer.register_native_texture(&self.render_state.device, &target.image, wgpu::FilterMode::Linear),
            };
            drop(renderer);
            self.target = Some((target, id));
        }
        if self.points.as_ref().is_some_and(|p| p.splats.is_some()) && self.blended.as_ref().is_none_or(|b| b.size != size) {
            self.blended = Some(BlendTargets::new(&self.render_state, &self.composite_layout, size));
        }
        let (target, id) = self.target.as_ref().expect("made above");
        self.encode(camera, target, self.blended.as_ref());
        *id
    }

    // The uploaded scene through `camera` as an image of `size` pixels, off screen:
    // premultiplied RGBA in display encoding, row by row, transparent where empty.
    pub fn render(&self, size: [u32; 2], camera: &Camera) -> Vec<[u8; 4]> {
        let size = [size[0].clamp(1, 16384), size[1].clamp(1, 16384)];
        let target = Target::new(&self.render_state, size);
        let blended = self.points.as_ref().is_some_and(|p| p.splats.is_some()).then(|| BlendTargets::new(&self.render_state, &self.composite_layout, size));
        self.encode(camera, &target, blended.as_ref());
        read_texture(&self.render_state, &target.texture, size)
    }

    // Draws the scene into `target` (and the splats through `blended`) and submits it.
    fn encode(&self, camera: &Camera, target: &Target, blended: Option<&BlendTargets>) {
        let state = &self.render_state;
        // Orthographic depth runs from the near plane to just past the farthest corner.
        let corners = self.scene.iter().flat_map(|s| &s.corners).chain(self.points.iter().flat_map(|p| &p.extent));
        let deepest = corners.map(|&p| camera.to_cam(p)[2]).fold(camera.near, f32::max);
        let far = deepest + (deepest - camera.near).abs().max(1e-3) * 0.01;
        let splat_depth = self.points.iter().flat_map(|p| &p.corners).map(|&p| camera.to_cam(p)[2]).fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], z| [lo.min(z), hi.max(z)]);
        state.queue.write_buffer(&self.camera, 0, bytemuck::cast_slice(&camera.uniform(far, splat_depth)));
        let splats = self.points.as_ref().and_then(|p| p.splats.as_ref());
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
                if let Some((buffer, count)) = &scene.faces {
                    pass.set_pipeline(&self.surfaces);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
                if let Some((buffer, count)) = &scene.toon {
                    pass.set_bind_group(2, &self.toon_group, &[]);
                    for pipeline in [&self.toon, &self.outline] {
                        pass.set_pipeline(pipeline);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..*count, 0..1);
                    }
                }
                if let Some((buffer, count)) = &scene.see_through {
                    pass.set_pipeline(&self.see_through);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
                if let Some((buffer, count)) = &scene.edges {
                    pass.set_pipeline(&self.edges);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
            if let Some((buffer, count)) = self.points.as_ref().and_then(|p| p.curves.as_ref()) {
                pass.set_pipeline(&self.strands);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..6, 0..*count);
            }
            if let Some((buffer, count)) = self.points.as_ref().and_then(|p| p.dots.as_ref()) {
                pass.set_pipeline(&self.dots);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..6, 0..*count);
            }
        }
        if let (Some((buffer, count)), Some(blended)) = (splats, blended) {
            let attachment = |view, clear: f64| Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: clear, g: clear, b: clear, a: clear }), store: wgpu::StoreOp::Store } });
            let depth = |load| Some(wgpu::RenderPassDepthStencilAttachment { view: &blended.depth, depth_ops: Some(wgpu::Operations { load, store: wgpu::StoreOp::Store }), stencil_ops: None });
            // The surfaces' depth, so splats behind them are hidden.
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("viewport depth"), color_attachments: &[], depth_stencil_attachment: depth(wgpu::LoadOp::Clear(1.0)), timestamp_writes: None, occlusion_query_set: None });
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_bind_group(1, &self.camera_group, &[]);
                if let Some((faces, faces_count)) = self.scene.as_ref().and_then(|s| s.faces.as_ref()) {
                    pass.set_pipeline(&self.depth_only);
                    pass.set_vertex_buffer(0, faces.slice(..));
                    pass.draw(0..*faces_count, 0..1);
                }
                if let Some((faces, faces_count)) = self.scene.as_ref().and_then(|s| s.toon.as_ref()) {
                    pass.set_bind_group(2, &self.toon_group, &[]);
                    pass.set_pipeline(&self.toon_depth);
                    pass.set_vertex_buffer(0, faces.slice(..));
                    pass.draw(0..*faces_count, 0..1);
                }
            }
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewport splats"),
                    color_attachments: &[attachment(&blended.accum, 0.0), attachment(&blended.reveal, 1.0), attachment(&blended.glow, 0.0)],
                    depth_stencil_attachment: depth(wgpu::LoadOp::Load),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_bind_group(1, &self.camera_group, &[]);
                pass.set_pipeline(&self.splats);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..6, 0..*count);
            }
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewport composite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target.image, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.composite);
                pass.set_bind_group(0, &blended.group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        state.queue.submit([encoder.finish()]);
    }
}

impl Target {
    fn new(state: &egui_wgpu::RenderState, size: [u32; 2]) -> Self {
        let device = &state.device;
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let texture = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: extent, mip_level_count: 1, sample_count: samples, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] })
        };
        let msaa = texture("viewport msaa", COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        let depth = texture("viewport depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        // Values in display encoding, like egui's own (Rgba8Unorm) textures, which it uses as sampled.
        let image = texture("viewport image", COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
        Target { size, msaa, depth, image: image.create_view(&Default::default()), texture: image }
    }
}

impl BlendTargets {
    fn new(state: &egui_wgpu::RenderState, layout: &wgpu::BindGroupLayout, size: [u32; 2]) -> Self {
        let device = &state.device;
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let view = |label: &str, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: extent, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] }).create_view(&Default::default())
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let depth = view("viewport splat depth", DEPTH, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let (accum, reveal, glow) = (view("viewport accum", ACCUM, usage), view("viewport reveal", REVEAL, usage), view("viewport glow", ACCUM, usage));
        let entry = |binding, view| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("viewport composite"), layout, entries: &[entry(0, &accum), entry(1, &reveal), entry(2, &glow)] });
        BlendTargets { size, depth, accum, reveal, glow, group }
    }
}

// An Rgba8 texture's pixels, row by row (waits for the GPU).
fn read_texture(state: &egui_wgpu::RenderState, texture: &wgpu::Texture, size: [u32; 2]) -> Vec<[u8; 4]> {
    let [w, h] = size;
    let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = state.device.create_buffer(&wgpu::BufferDescriptor { label: Some("viewport readback"), size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = state.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    state.queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    if state.device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
        return vec![[0; 4]; (w * h) as usize];
    }
    let data = buffer.slice(..).get_mapped_range();
    let (row, w) = (row as usize, w as usize);
    (0..h as usize).flat_map(|y| (0..w).map(move |x| (y, x))).map(|(y, x)| std::array::from_fn(|c| data[y * row + x * 4 + c])).collect()
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

// A GPU viewport without a window, for tests; None where there's no GPU (as on some CI machines).
#[cfg(test)]
pub fn headless() -> Option<GpuViewport> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let renderer = egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, Default::default());
    let state = egui_wgpu::RenderState { adapter, available_adapters: Vec::new(), device, queue, target_format: wgpu::TextureFormat::Rgba8Unorm, renderer: Arc::new(egui::mutex::RwLock::new(renderer)) };
    Some(GpuViewport::new(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The last image drawn, premultiplied, in display encoding.
    fn read(gpu: &GpuViewport) -> Vec<[u8; 4]> {
        let (target, _) = gpu.target.as_ref().expect("drawn");
        read_texture(&gpu.render_state, &target.texture, target.size)
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
        gpu.set_scene(1, &faces, &[], &[], &[], Some(checker.clone()));
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
        gpu.set_scene(1, &face, &edge(3.0), &[], &[], None);
        gpu.draw([20, 20], &camera);
        assert_eq!(read(&gpu)[5 * 20 + 3], [255, 255, 255, 255], "an edge behind the surface is hidden");
        gpu.set_scene(2, &face, &edge(0.0), &[], &[], None);
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
        gpu.set_scene(1, &floor, &[], &[], &[], None);
        gpu.draw([32, 32], &camera);
        let image = read(&gpu);
        assert!(image[31 * 32 + 16][3] > 0, "the floor just in front of the camera shows at the bottom");
        assert_eq!(image[2 * 32 + 16][3], 0, "nothing above the horizon");
    }

    // The edge test's camera: at z = -5 looking along +z, 20 x 20 points.
    fn straight_on() -> Camera {
        Camera { target: [0.0; 3], rotation: Camera::rotation(0.0, 0.0), zoom: 5.0, center: egui::vec2(10.0, 10.0), size: egui::vec2(20.0, 20.0), focal: 20.0, ortho_scale: 4.0, orthographic: false, near: 0.1 }
    }

    // A white face at depth z filling the view (corners well outside it).
    fn wall(z: f32) -> Vec<Vertex> {
        let r = (z + 5.0) * 2.0;
        [[-r, r, z], [r, r, z], [-r, -r, z], [r, r, z], [r, -r, z], [-r, -r, z]].map(|p| Vertex::new(p, [1.0; 4], None)).to_vec()
    }

    #[test]
    fn strands_are_as_thick_as_their_width_and_fine_ones_fade() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        // straight_on: 4 pixels per unit at the origin. A red strand across the view, through
        // the middle of a row of pixels (a quarter of a unit is a pixel).
        let strand = |width: f32, z: f32| Strand { a: [-2.0, -0.125, z], width_a: width, b: [2.0, -0.125, z], width_b: width, color: [1.0, 0.0, 0.0, 1.0] };
        // The middle column: how many pixels it covers, and the most it covers one.
        let column = |gpu: &mut GpuViewport, s: Strand| {
            gpu.set_scene(1, &[], &[], &[], &[], None);
            gpu.set_points(1, &[], &[], &[s]);
            gpu.draw([20, 20], &straight_on());
            let image = read(gpu);
            let alphas: Vec<u8> = (0..20).map(|y| image[y * 20 + 10][3]).collect();
            (alphas.iter().filter(|a| **a > 128).count(), *alphas.iter().max().unwrap())
        };
        let (one, _) = column(&mut gpu, strand(1.0, 0.0));
        let (two, _) = column(&mut gpu, strand(2.0, 0.0));
        assert!((3..=5).contains(&one) && (7..=9).contains(&two), "1 unit is 4 pixels, 2 units 8: {one}, {two}");
        // Finer than a pixel: still drawn, but faint.
        let (_, faint) = column(&mut gpu, strand(0.05, 0.0));
        assert!(faint > 0 && faint <= 128, "a fifth of a pixel shows faintly: {faint}");
        // And premultiplied like the rest of the image, so saving it (which divides by alpha)
        // keeps its colour: a faint red strand is red, not brightened past it to white.
        let image = read(&gpu);
        for p in image.iter().filter(|p| p[3] > 0) {
            assert!(p[0] <= p[3].saturating_add(2) && p[1] <= p[3].saturating_add(2), "not premultiplied: {p:?}");
        }
        // No width: a plain line a pixel wide.
        let (_, line) = column(&mut gpu, strand(-1.0, 0.0));
        assert_eq!(line, 255);
        // Behind a surface: hidden.
        gpu.set_scene(2, &wall(0.0), &[], &[], &[], None);
        gpu.set_points(2, &[], &[], &[strand(1.0, 2.0)]);
        gpu.draw([20, 20], &straight_on());
        assert_eq!(read(&gpu)[10 * 20 + 10], [255, 255, 255, 255], "a strand behind the surface is hidden");
    }

    #[test]
    fn dots_keep_their_size_and_hide_behind_surfaces() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let red = [1.0, 0.0, 0.0, 1.0];
        gpu.set_scene(1, &wall(0.0), &[], &[], &[], None);
        // On the surface, 3 points across: shows, and only near the middle.
        gpu.set_points(1, &[Point::new([0.0; 3], 3.0, red, [0.0; 3])], &[], &[]);
        gpu.draw([20, 20], &straight_on());
        let image = read(&gpu);
        assert_eq!(image[10 * 20 + 10], [255, 0, 0, 255], "a point on the surface shows over it");
        assert_eq!(image[10 * 20 + 16], [255, 255, 255, 255], "dots are a few points across, not more");
        // Behind the surface: hidden.
        gpu.set_points(2, &[Point::new([0.0, 0.0, 2.0], 3.0, red, [0.0; 3])], &[], &[]);
        gpu.draw([20, 20], &straight_on());
        assert_eq!(read(&gpu)[10 * 20 + 10], [255, 255, 255, 255], "a point behind the surface is hidden");
    }

    #[test]
    fn splats_blend_glow_and_hide_behind_surfaces() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let middle = |gpu: &GpuViewport| read(gpu)[10 * 20 + 10];
        // A dark wall at z = 1, so blending and glow show.
        let dark: Vec<Vertex> = wall(1.0).into_iter().map(|v| Vertex { color: [0.2, 0.2, 0.2, 1.0], ..v }).collect();
        gpu.set_scene(1, &dark, &[], &[], &[], None);
        let smoke = |z: f32, rgb: [f32; 3], emission: [f32; 3]| Point::new([0.0, 0.0, z], 2.0, [rgb[0], rgb[1], rgb[2], 0.5], emission);
        // In front: half covers the wall at its centre.
        gpu.set_points(1, &[], &[smoke(0.0, [1.0; 3], [0.0; 3])], &[]);
        gpu.draw([20, 20], &straight_on());
        let over = middle(&gpu);
        assert!(over[0] > 120 && over[0] < 160, "half-opaque white over dark grey: {over:?}");
        assert_eq!(over[3], 255, "still opaque where the wall is");
        // Behind the wall: hidden.
        gpu.set_points(2, &[], &[smoke(3.0, [1.0; 3], [0.0; 3])], &[]);
        gpu.draw([20, 20], &straight_on());
        assert_eq!(middle(&gpu), [51, 51, 51, 255], "a splat behind the wall is hidden");
        // Glow adds light, in its own colour: dark smoke glowing orange is brighter
        // than the same smoke not glowing, most in red, least in blue.
        gpu.set_points(3, &[], &[smoke(0.0, [0.0; 3], [0.0; 3])], &[]);
        gpu.draw([20, 20], &straight_on());
        let unlit = middle(&gpu);
        gpu.set_points(4, &[], &[smoke(0.0, [0.0; 3], [6.0, 3.0, 0.0])], &[]);
        gpu.draw([20, 20], &straight_on());
        let glowing = middle(&gpu);
        let gain: [i32; 3] = std::array::from_fn(|c| glowing[c] as i32 - unlit[c] as i32);
        assert!(gain[0] > gain[1] && gain[1] > gain[2] && gain[2] == 0 && gain[0] > 30, "fire glows orange: {unlit:?} to {glowing:?}");
        // Overlapping splats: the same image whichever is drawn first (no sorting needed).
        let (near, far) = (smoke(-0.5, [1.0, 0.0, 0.0], [0.0; 3]), smoke(0.5, [0.0, 0.0, 1.0], [0.0; 3]));
        gpu.set_points(5, &[], &[near, far], &[]);
        gpu.draw([20, 20], &straight_on());
        let one = middle(&gpu);
        gpu.set_points(6, &[], &[far, near], &[]);
        gpu.draw([20, 20], &straight_on());
        let other = middle(&gpu);
        assert!((0..4).all(|c| one[c].abs_diff(other[c]) <= 1), "order doesn't matter: {one:?} vs {other:?}");
        assert!(one[0] > one[2], "the nearer (red) splat weighs more: {one:?}");
    }

    #[test]
    fn see_through_faces_blend_over_what_is_behind() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        // A half-transparent white wall in front of a red one; drawn first, it still
        // doesn't hide the red (see-through faces go last and don't write depth).
        let front: Vec<Vertex> = wall(0.0).into_iter().map(|v| Vertex { color: [1.0, 1.0, 1.0, 0.5], ..v }).collect();
        let back: Vec<Vertex> = wall(2.0).into_iter().map(|v| Vertex { color: [1.0, 0.0, 0.0, 1.0], ..v }).collect();
        gpu.set_scene(1, &[front.clone(), back].concat(), &[], &[], &[], None);
        let image = gpu.render([20, 20], &straight_on());
        let p = image[10 * 20 + 10];
        assert!(p[0] == 255 && (120..=135).contains(&p[1]) && p[3] == 255, "pink: {p:?}");
        // Alone, it's half there (premultiplied).
        gpu.set_scene(2, &front, &[], &[], &[], None);
        let p = gpu.render([20, 20], &straight_on())[10 * 20 + 10];
        assert!((120..=135).contains(&p[3]) && p[0] == p[3], "half: {p:?}");
    }

    #[test]
    fn toon_faces_band_crisply_and_are_outlined() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        // A white sphere lit from the upper left, in three bands, with a blue outline.
        let sphere = crate::node_sphere(1.0, 32, 64);
        let normals = sphere.compute_normals();
        let to_light = crate::Vec3::new(-0.5, 0.5, -0.7).normalize();
        let toon: Vec<ToonVertex> = sphere.indices.iter().flatten().map(|&i| {
            let (p, n) = (sphere.vertices[i], normals[i]);
            ToonVertex { position: [p.x, p.y, p.z], light: n.dot(to_light).max(0.0), color: [1.0; 4], normal: [n.x, n.y, n.z], material: 0.0 }
        }).collect();
        let look = ToonMaterial { shade: [3.0, 0.0, 0.0, 0.0], shadow: [0.2, 0.2, 0.2, 3.0], highlight: [0.0; 4], emission: [0.0; 4], outline: [0.0, 0.0, 1.0, 1.0] };
        gpu.set_scene(1, &[], &[], &toon, &[look], None);
        let camera = Camera { size: egui::vec2(120.0, 120.0), center: egui::vec2(60.0, 60.0), focal: 150.0, ..straight_on() };
        let image = gpu.render([120, 120], &camera);
        let at = |x: usize, y: usize| image[y * 120 + x];
        let blue = |p: [u8; 4]| p[2] > 200 && p[0] < 40 && p[1] < 40 && p[3] > 200;
        // Inside: the bands, not the outline (the hull's faces toward the camera are culled).
        assert!(!blue(at(60, 60)) && at(60, 60)[3] == 255, "centre {:?}", at(60, 60));
        // Only three greys inside, apart from the antialiased band edges.
        let levels = [0.2f32, 0.6, 1.0].map(|l| (l * 255.0) as i32);
        let (mut inside, mut between) = (0, 0);
        for y in 35..85 {
            for x in 35..85 {
                let (dx, dy) = (x as f32 - 60.0, y as f32 - 60.0);
                if dx.hypot(dy) < 25.0 {
                    inside += 1;
                    if !levels.iter().any(|l| (at(x, y)[0] as i32 - l).abs() <= 6) {
                        between += 1;
                    }
                }
            }
        }
        assert!(between * 10 < inside, "{between} of {inside} pixels between the bands");
        // The lit side faces the light, the shadow side away from it.
        assert!(at(48, 48)[0] > at(72, 72)[0]);
        // Around the silhouette (radius about 30 pixels): a ring of outline, then nothing.
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let along = |r: i32| at((60 + dx * r) as usize, (60 + dy * r) as usize);
            let ring = (25..40).filter(|r| blue(along(*r))).count();
            assert!((2..=5).contains(&ring), "outline {ring} pixels wide toward {dx},{dy}");
            assert_eq!(along(45)[3], 0, "transparent beyond the outline");
        }
        // CPU shading gives the same bands.
        assert_eq!(look.shade([1.0; 3], 0.1), [0.2; 3]);
        assert_eq!(look.shade([1.0; 3], 0.5), [0.6; 3]);
        assert_eq!(look.shade([1.0; 3], 0.9), [1.0; 3]);
    }

    #[test]
    fn off_screen_renders_match_the_viewport_at_any_size() {
        let Some(mut gpu) = headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        // A small red square in the middle of an empty view.
        let red = [1.0, 0.0, 0.0, 1.0];
        let square: Vec<Vertex> = [[-1.0, 1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, -1.0, 0.0]].map(|p| Vertex::new(p, red, None)).to_vec();
        gpu.set_scene(1, &square, &[], &[], &[], None);
        let camera = Camera { size: egui::vec2(40.0, 30.0), center: egui::vec2(20.0, 15.0), focal: 30.0, ..straight_on() };
        gpu.draw([40, 30], &camera);
        let shown = read(&gpu);
        let small = gpu.render([40, 30], &camera);
        assert!(small == shown, "an off-screen render at the viewport's size is the viewport's image");
        assert_eq!(small[0], [0; 4], "transparent where nothing is drawn");
        // Twice the size: the same picture, in more pixels.
        let big = gpu.render([80, 60], &camera);
        let coverage = |image: &[[u8; 4]]| image.iter().filter(|p| p[3] > 127).count() as f32 / image.len() as f32;
        assert!((coverage(&small) - coverage(&big)).abs() < 0.02, "same framing: {} vs {}", coverage(&small), coverage(&big));
        assert_eq!(big[30 * 80 + 40], [255, 0, 0, 255], "the square is still in the middle");
    }
}
