// COPs: compositing nodes inside a COP Network (the IMG context).
//
// An image travels through the node graph as a Mesh with no points: its size
// in `cop:size` ([width, height]) and linear RGBA pixels, row by row from the
// top, in `cop:rgba`. That way COP nodes get the graph's caching, bypass,
// display flags and keyframes for free.
//
// Alpha is straight (not premultiplied). Colour values are linear; File reads
// sRGB images as sRGB and the viewer shows sRGB.

use super::{hash01, Mesh};
use crate::shading::{c, f, t, Param};

pub const SIZE: &str = "cop:size";

static LAST_RENDER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

// The image file of the latest finished MoonRay render, for the Render COP.
pub fn set_last_render(path: &str) {
    *LAST_RENDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.to_owned());
}

pub fn last_render() -> Option<String> {
    LAST_RENDER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
pub const RGBA: &str = "cop:rgba";

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

impl Image {
    pub fn new(w: usize, h: usize, fill: [f32; 4]) -> Image {
        let (w, h) = (w.clamp(1, 16384), h.clamp(1, 16384));
        Image { w, h, px: vec![fill; w * h] }
    }

    pub fn is_image(mesh: &Mesh) -> bool {
        mesh.point_attributes.contains_key(SIZE)
    }

    pub fn from_mesh(mesh: &Mesh) -> Option<Image> {
        let size = mesh.point_attributes.get(SIZE).filter(|s| s.len() >= 2)?;
        let (w, h) = (size[0] as usize, size[1] as usize);
        let raw = mesh.point_attributes.get(RGBA).filter(|p| p.len() == w * h * 4)?;
        Some(Image { w, h, px: raw.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect() })
    }

    pub fn to_mesh(&self) -> Mesh {
        let mut mesh = Mesh::default();
        mesh.point_attributes.insert(SIZE.into(), vec![self.w as f32, self.h as f32]);
        mesh.point_attributes.insert(RGBA.into(), self.px.iter().flatten().copied().collect());
        mesh
    }

    fn at(&self, x: usize, y: usize) -> [f32; 4] {
        self.px[y.min(self.h - 1) * self.w + x.min(self.w - 1)]
    }

    // Bilinear sample at pixel coordinates (pixel centres at +0.5); outside is transparent.
    pub fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        if fx < -1.0 || fy < -1.0 || fx > self.w as f32 || fy > self.h as f32 {
            return [0.0; 4];
        }
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let get = |x: f32, y: f32| {
            if x < 0.0 || y < 0.0 || x >= self.w as f32 || y >= self.h as f32 { [0.0; 4] } else { self.at(x as usize, y as usize) }
        };
        let (a, b, c, d) = (get(x0, y0), get(x0 + 1.0, y0), get(x0, y0 + 1.0), get(x0 + 1.0, y0 + 1.0));
        std::array::from_fn(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
    }

    // The same picture at another size.
    pub fn resized(&self, w: usize, h: usize) -> Image {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let mut out = Image::new(w, h, [0.0; 4]);
        let (sx, sy) = (self.w as f32 / out.w as f32, self.h as f32 / out.h as f32);
        for y in 0..out.h {
            for x in 0..out.w {
                // Clamp to the edge so resizing never fades the border.
                let p = self.sample(((x as f32 + 0.5) * sx).clamp(0.5, self.w as f32 - 0.5), ((y as f32 + 0.5) * sy).clamp(0.5, self.h as f32 - 0.5));
                out.px[y * out.w + x] = p;
            }
        }
        out
    }

    fn map(mut self, f: impl Fn([f32; 4]) -> [f32; 4]) -> Image {
        self.px.iter_mut().for_each(|p| *p = f(*p));
        self
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.max(0.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn luma(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

// --- Operations -----------------------------------------------------------------

pub struct CopOp {
    pub label: &'static str,
    pub category: &'static str,
    pub help: &'static str,
    pub inputs: usize,
    pub params: &'static [Param],
    pub texts: &'static [&'static str],
}

pub const CATEGORIES: [&str; 6] = ["Generators", "Composite", "Color", "Filters", "Transform", "Output"];
pub const BLEND_MODES: [&str; 10] = ["Over", "Add", "Multiply", "Screen", "Subtract", "Difference", "Minimum", "Maximum", "Inside", "Outside"];
pub const RAMP_TYPES: [&str; 3] = ["Horizontal", "Vertical", "Radial"];

const SIZE_PARAMS: [Param; 2] = [f("width", "Width", "Size", 1.0, 8192.0, 1280.0), f("height", "Height", "Size", 1.0, 8192.0, 720.0)];

pub const FILE: usize = 0;
pub const RENDER: usize = 1;
pub const OUTPUT: usize = 19;

pub const OPS: &[CopOp] = &[
    CopOp {
        label: "File",
        category: "Generators",
        help: "Reads an image (EXR, HDR, PNG, JPG, TIFF…). $F in the name reads one file per frame.",
        inputs: 0,
        params: &[],
        texts: &["File"],
    },
    CopOp {
        label: "Render",
        category: "Generators",
        help: "The latest MoonRay render (read again after every render).",
        inputs: 0,
        params: &[],
        texts: &[],
    },
    CopOp {
        label: "Constant",
        category: "Generators",
        help: "A solid colour.",
        inputs: 0,
        params: &[c("color", "Color", "", [0.18, 0.18, 0.18]), f("alpha", "Alpha", "", 0.0, 1.0, 1.0), SIZE_PARAMS[0], SIZE_PARAMS[1]],
        texts: &[],
    },
    CopOp {
        label: "Ramp",
        category: "Generators",
        help: "A gradient between two colours.",
        inputs: 0,
        params: &[
            c("color_a", "Color A", "", [0.0, 0.0, 0.0]),
            c("color_b", "Color B", "", [1.0, 1.0, 1.0]),
            f("type", "Type (0 horizontal, 1 vertical, 2 radial)", "", 0.0, 2.0, 0.0),
            SIZE_PARAMS[0],
            SIZE_PARAMS[1],
        ],
        texts: &[],
    },
    CopOp {
        label: "Noise",
        category: "Generators",
        help: "Fractal noise between two colours. Speed animates it over time.",
        inputs: 0,
        params: &[
            c("color_a", "Color A", "", [0.0, 0.0, 0.0]),
            c("color_b", "Color B", "", [1.0, 1.0, 1.0]),
            f("size", "Element size (px)", "", 1.0, 2000.0, 120.0),
            f("octaves", "Octaves", "", 1.0, 8.0, 4.0),
            f("speed", "Speed", "", 0.0, 10.0, 0.0),
            f("seed", "Seed", "", 0.0, 9999.0, 0.0),
            SIZE_PARAMS[0],
            SIZE_PARAMS[1],
        ],
        texts: &[],
    },
    CopOp {
        label: "Composite",
        category: "Composite",
        help: "Combines input 1 (A, foreground) with input 2 (B, background): over, add, multiply, screen…",
        inputs: 2,
        params: &[f("mode", "Mode", "", 0.0, 9.0, 0.0), f("mix", "Mix", "", 0.0, 1.0, 1.0)],
        texts: &[],
    },
    CopOp {
        label: "Color Correct",
        category: "Color",
        help: "Exposure, contrast, gamma, saturation, hue, gain and offset.",
        inputs: 1,
        params: &[
            f("exposure", "Exposure (stops)", "", -10.0, 10.0, 0.0),
            f("contrast", "Contrast", "", 0.0, 4.0, 1.0),
            f("gamma", "Gamma", "", 0.1, 5.0, 1.0),
            f("saturation", "Saturation", "", 0.0, 4.0, 1.0),
            f("hue", "Hue shift", "", -1.0, 1.0, 0.0),
            c("gain", "Gain", "", [1.0, 1.0, 1.0]),
            c("offset", "Offset", "", [0.0, 0.0, 0.0]),
        ],
        texts: &[],
    },
    CopOp {
        label: "Invert",
        category: "Color",
        help: "Inverts the colour (alpha kept).",
        inputs: 1,
        params: &[],
        texts: &[],
    },
    CopOp {
        label: "Levels",
        category: "Color",
        help: "Remaps the input range (black and white points) to the output range.",
        inputs: 1,
        params: &[
            f("in_black", "Input black", "", 0.0, 1.0, 0.0),
            f("in_white", "Input white", "", 0.0, 4.0, 1.0),
            f("out_black", "Output black", "", 0.0, 1.0, 0.0),
            f("out_white", "Output white", "", 0.0, 4.0, 1.0),
        ],
        texts: &[],
    },
    CopOp {
        label: "Blur",
        category: "Filters",
        help: "Gaussian blur.",
        inputs: 1,
        params: &[f("radius", "Radius (px)", "", 0.0, 200.0, 4.0)],
        texts: &[],
    },
    CopOp {
        label: "Sharpen",
        category: "Filters",
        help: "Unsharp mask.",
        inputs: 1,
        params: &[f("amount", "Amount", "", 0.0, 5.0, 0.8), f("radius", "Radius (px)", "", 0.5, 20.0, 1.5)],
        texts: &[],
    },
    CopOp {
        label: "Glow",
        category: "Filters",
        help: "Bright areas bloom: pixels above the threshold are blurred and added back.",
        inputs: 1,
        params: &[f("threshold", "Threshold", "", 0.0, 10.0, 0.8), f("radius", "Radius (px)", "", 1.0, 200.0, 20.0), f("intensity", "Intensity", "", 0.0, 10.0, 1.0)],
        texts: &[],
    },
    CopOp {
        label: "Edge Detect",
        category: "Filters",
        help: "Sobel edges of the brightness.",
        inputs: 1,
        params: &[f("strength", "Strength", "", 0.0, 10.0, 1.0)],
        texts: &[],
    },
    CopOp {
        label: "Vignette",
        category: "Filters",
        help: "Darkens towards the corners.",
        inputs: 1,
        params: &[f("amount", "Amount", "", 0.0, 1.0, 0.5), f("softness", "Softness", "", 0.05, 2.0, 0.6)],
        texts: &[],
    },
    CopOp {
        label: "Grain",
        category: "Filters",
        help: "Film grain, different every frame.",
        inputs: 1,
        params: &[f("amount", "Amount", "", 0.0, 1.0, 0.05), f("seed", "Seed", "", 0.0, 9999.0, 0.0)],
        texts: &[],
    },
    CopOp {
        label: "Transform",
        category: "Transform",
        help: "Moves, rotates and scales the image about its centre.",
        inputs: 1,
        params: &[
            f("tx", "Translate X (px)", "", -8192.0, 8192.0, 0.0),
            f("ty", "Translate Y (px)", "", -8192.0, 8192.0, 0.0),
            f("rotate", "Rotate°", "", -360.0, 360.0, 0.0),
            f("scale", "Scale", "", 0.01, 20.0, 1.0),
        ],
        texts: &[],
    },
    CopOp {
        label: "Crop",
        category: "Transform",
        help: "Cuts the image down (fractions of each side).",
        inputs: 1,
        params: &[f("left", "Left", "", 0.0, 1.0, 0.0), f("right", "Right", "", 0.0, 1.0, 0.0), f("top", "Top", "", 0.0, 1.0, 0.0), f("bottom", "Bottom", "", 0.0, 1.0, 0.0)],
        texts: &[],
    },
    CopOp {
        label: "Resize",
        category: "Transform",
        help: "Scales the image to a new size.",
        inputs: 1,
        params: &[SIZE_PARAMS[0], SIZE_PARAMS[1]],
        texts: &[],
    },
    CopOp {
        label: "Chroma Key",
        category: "Composite",
        help: "Makes pixels close to the key colour transparent (green or blue screen), and removes its spill.",
        inputs: 1,
        params: &[c("key", "Key color", "", [0.1, 0.8, 0.2]), f("tolerance", "Tolerance", "", 0.0, 1.0, 0.25), f("softness", "Softness", "", 0.0, 1.0, 0.15), f("despill", "Despill", "", 0.0, 1.0, 1.0)],
        texts: &[],
    },
    CopOp {
        label: "File Output",
        category: "Output",
        help: "Writes the image (EXR keeps full range; PNG/JPG are sRGB). $F or $F4 in the name for one file per frame.",
        inputs: 1,
        params: &[],
        texts: &["File"],
    },
    CopOp {
        label: "Switch",
        category: "Composite",
        help: "Passes input 1 or input 2 through.",
        inputs: 2,
        params: &[f("index", "Input (0 or 1)", "", 0.0, 1.0, 0.0)],
        texts: &[],
    },
];

#[derive(Clone, Debug, PartialEq)]
pub struct CopNode {
    pub op: usize,
    pub values: Vec<f32>,
    pub texts: Vec<String>,
}

impl CopNode {
    pub fn new(op: usize) -> CopNode {
        let op = op.min(OPS.len() - 1);
        let def = &OPS[op];
        CopNode { op, values: def.params.iter().flat_map(|p| p.default[..p.width()].to_vec()).collect(), texts: vec![String::new(); def.texts.len()] }
    }

    pub fn def(&self) -> &'static CopOp {
        &OPS[self.op.min(OPS.len() - 1)]
    }

    pub fn to_params(&self) -> Vec<f32> {
        let mut p = vec![self.op as f32];
        p.extend_from_slice(&self.values);
        p
    }

    pub fn from_params(p: &[f32]) -> CopNode {
        let mut node = CopNode::new(p.first().map_or(0, |o| o.max(0.0) as usize));
        for (slot, v) in node.values.iter_mut().zip(p.iter().skip(1)) {
            *slot = *v;
        }
        node
    }

    pub fn get(&self, key: &str) -> [f32; 3] {
        let mut at = 0;
        for p in self.def().params {
            if p.key == key {
                return if p.width() == 3 { [self.values[at], self.values[at + 1], self.values[at + 2]] } else { [self.values[at]; 3] };
            }
            at += p.width();
        }
        [0.0; 3]
    }

    fn v(&self, key: &str) -> f32 {
        self.get(key)[0]
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

    pub fn label(&self, id: usize) -> String {
        format!("{}{id}", self.def().label.to_lowercase().replace(' ', "_"))
    }

    // Noise with speed, grain and per-frame files change over time.
    pub fn time_dependent(&self) -> bool {
        match self.op {
            4 => self.v("speed") > 0.0,
            14 => true,
            FILE => crate::fileio::has_frame_token(self.texts.first().map_or("", |s| s.as_str())),
            RENDER => true,
            _ => false,
        }
    }
}

// --- Reading and writing files --------------------------------------------------------

pub fn read_image(path: &str) -> Result<Image, String> {
    let img = image::open(path.trim()).map_err(|e| format!("{}: {e}", path.trim()))?;
    let linear = matches!(img, image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_));
    let rgba = img.to_rgba32f();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let decode = |c: f32| if linear { c } else { srgb_to_linear(c) };
    Ok(Image { w, h, px: rgba.pixels().map(|p| [decode(p[0]), decode(p[1]), decode(p[2]), p[3]]).collect() })
}

pub fn write_image(img: &Image, path: &str) -> Result<(), String> {
    let path = path.trim();
    if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (w, h) = (img.w as u32, img.h as u32);
    let result = if matches!(ext.as_str(), "exr" | "hdr") {
        let buf = image::Rgba32FImage::from_fn(w, h, |x, y| image::Rgba(img.px[(y * w + x) as usize]));
        image::DynamicImage::ImageRgba32F(buf).save(path)
    } else {
        let enc = |c: f32| (linear_to_srgb(c).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        let buf = image::RgbaImage::from_fn(w, h, |x, y| {
            let p = img.px[(y * w + x) as usize];
            image::Rgba([enc(p[0]), enc(p[1]), enc(p[2]), (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8])
        });
        if matches!(ext.as_str(), "jpg" | "jpeg") {
            image::DynamicImage::ImageRgba8(buf).to_rgb8().save(path)
        } else {
            buf.save(path)
        }
    };
    result.map_err(|e| format!("{path}: {e}"))
}

// --- Cooking ----------------------------------------------------------------------

fn gaussian(img: &Image, radius: f32) -> Image {
    if radius < 0.3 {
        return img.clone();
    }
    let sigma = radius / 2.0;
    let r = (sigma * 3.0).ceil() as i32;
    let kernel: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let total: f32 = kernel.iter().sum();
    let pass = |src: &Image, horizontal: bool| {
        let mut out = src.clone();
        for y in 0..src.h {
            for x in 0..src.w {
                let mut acc = [0.0f32; 4];
                for (k, w) in kernel.iter().enumerate() {
                    let o = k as i32 - r;
                    let (sx, sy) = if horizontal { (x as i32 + o, y as i32) } else { (x as i32, y as i32 + o) };
                    let p = src.at(sx.clamp(0, src.w as i32 - 1) as usize, sy.clamp(0, src.h as i32 - 1) as usize);
                    (0..4).for_each(|c| acc[c] += p[c] * w);
                }
                out.px[y * src.w + x] = acc.map(|a| a / total);
            }
        }
        out
    };
    pass(&pass(img, true), false)
}

fn fbm(x: f32, y: f32, octaves: usize, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves.clamp(1, 8) {
        let s = seed.wrapping_add(o as u32 * 97);
        let (px, py) = (x * freq, y * freq);
        let (x0, y0) = (px.floor(), py.floor());
        let (fx, fy) = (px - x0, py - y0);
        let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let h = |i: f32, j: f32| hash01(((i as i32).wrapping_mul(73856093) ^ (j as i32).wrapping_mul(19349663)) as u32, s);
        let a = h(x0, y0) + (h(x0 + 1.0, y0) - h(x0, y0)) * u;
        let b = h(x0, y0 + 1.0) + (h(x0 + 1.0, y0 + 1.0) - h(x0, y0 + 1.0)) * u;
        sum += (a + (b - a) * v) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

fn hsv_shift(p: [f32; 4], hue: f32, sat: f32) -> [f32; 4] {
    let l = luma(p);
    // Saturation around the luminance; hue as a rotation about the grey axis.
    let mut rgb = [l + (p[0] - l) * sat, l + (p[1] - l) * sat, l + (p[2] - l) * sat];
    if hue != 0.0 {
        let a = hue * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let k = 1.0 / 3.0f32.sqrt();
        let v = rgb;
        let dot = (v[0] + v[1] + v[2]) * k;
        let cross = [k * (v[2] - v[1]), k * (v[0] - v[2]), k * (v[1] - v[0])];
        rgb = std::array::from_fn(|i| v[i] * c + cross[i] * s + k * dot * (1.0 - c));
    }
    [rgb[0], rgb[1], rgb[2], p[3]]
}

fn blend(a: [f32; 4], b: [f32; 4], mode: usize) -> [f32; 4] {
    let mix = |f: &dyn Fn(f32, f32) -> f32| -> [f32; 4] { [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), (a[3] + b[3] * (1.0 - a[3])).min(1.0)] };
    match mode {
        0 => {
            // A over B (straight alpha).
            let alpha = a[3] + b[3] * (1.0 - a[3]);
            if alpha <= 1e-6 {
                return [0.0; 4];
            }
            let c = |k: usize| (a[k] * a[3] + b[k] * b[3] * (1.0 - a[3])) / alpha;
            [c(0), c(1), c(2), alpha]
        }
        1 => mix(&|x, y| x + y),
        2 => mix(&|x, y| x * y),
        3 => mix(&|x, y| 1.0 - (1.0 - x) * (1.0 - y)),
        4 => mix(&|x, y| y - x),
        5 => mix(&|x, y| (x - y).abs()),
        6 => mix(&|x, y| x.min(y)),
        7 => mix(&|x, y| x.max(y)),
        // Inside / outside: A kept where B is (or isn't) opaque.
        8 => [a[0], a[1], a[2], a[3] * b[3]],
        _ => [a[0], a[1], a[2], a[3] * (1.0 - b[3])],
    }
}

// Cooks a COP. `inputs` are the connected images (None when unconnected),
// `file` the resolved file to read (File and Render), `time` in seconds.
pub fn cook(node: &CopNode, inputs: &[Option<Image>], file: Option<&str>, time: f32) -> Result<Image, String> {
    let input = |i: usize| -> Result<Image, String> {
        inputs.get(i).cloned().flatten().ok_or_else(|| format!("connect an image to input {}", i + 1))
    };
    let size = || (node.v("width").round().max(1.0) as usize, node.v("height").round().max(1.0) as usize);
    Ok(match node.op {
        FILE | RENDER => {
            let path = file.filter(|f| !f.trim().is_empty()).ok_or(if node.op == FILE { "choose an image file" } else { "render with MoonRay first (OUT context)" })?;
            read_image(path)?
        }
        2 => {
            let (w, h) = size();
            let c = node.get("color");
            Image::new(w, h, [c[0], c[1], c[2], node.v("alpha")])
        }
        3 => {
            let (w, h) = size();
            let (a, b) = (node.get("color_a"), node.get("color_b"));
            let mut img = Image::new(w, h, [0.0; 4]);
            let kind = node.v("type").round() as usize;
            for y in 0..h {
                for x in 0..w {
                    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                    let t = match kind {
                        1 => 1.0 - v,
                        2 => (((u - 0.5) * w as f32 / h as f32).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0,
                        _ => u,
                    }
                    .clamp(0.0, 1.0);
                    img.px[y * w + x] = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, 1.0];
                }
            }
            img
        }
        4 => {
            let (w, h) = size();
            let (a, b) = (node.get("color_a"), node.get("color_b"));
            let scale = 1.0 / node.v("size").max(1.0);
            let drift = time * node.v("speed");
            let (octaves, seed) = (node.v("octaves").round() as usize, node.v("seed") as u32);
            let mut img = Image::new(w, h, [0.0; 4]);
            for y in 0..h {
                for x in 0..w {
                    let n = fbm(x as f32 * scale + drift, y as f32 * scale + drift * 0.7, octaves, seed);
                    img.px[y * w + x] = [a[0] + (b[0] - a[0]) * n, a[1] + (b[1] - a[1]) * n, a[2] + (b[2] - a[2]) * n, 1.0];
                }
            }
            img
        }
        5 => {
            let a = input(0)?;
            let b = input(1)?.resized(a.w, a.h);
            let (mode, mix) = (node.v("mode").round() as usize, node.v("mix").clamp(0.0, 1.0));
            let mut out = b.clone();
            for (i, p) in out.px.iter_mut().enumerate() {
                let c = blend(a.px[i], b.px[i], mode);
                *p = std::array::from_fn(|k| b.px[i][k] + (c[k] - b.px[i][k]) * mix);
            }
            out
        }
        6 => {
            let gain = node.get("gain");
            let offset = node.get("offset");
            let exposure = 2f32.powf(node.v("exposure"));
            let (contrast, gamma, sat, hue) = (node.v("contrast"), node.v("gamma").max(0.01), node.v("saturation"), node.v("hue"));
            input(0)?.map(|p| {
                let mut q = hsv_shift([p[0] * exposure, p[1] * exposure, p[2] * exposure, p[3]], hue, sat);
                for k in 0..3 {
                    // Contrast pivots on mid-grey (0.18 linear).
                    let c = 0.18 * (q[k].max(0.0) / 0.18).powf(contrast);
                    q[k] = c.max(0.0).powf(1.0 / gamma) * gain[k] + offset[k];
                }
                q
            })
        }
        7 => input(0)?.map(|p| [1.0 - p[0], 1.0 - p[1], 1.0 - p[2], p[3]]),
        8 => {
            let (ib, iw, ob, ow) = (node.v("in_black"), node.v("in_white"), node.v("out_black"), node.v("out_white"));
            input(0)?.map(|p| {
                let r = |c: f32| ob + ((c - ib) / (iw - ib).max(1e-6)).clamp(0.0, 1.0) * (ow - ob);
                [r(p[0]), r(p[1]), r(p[2]), p[3]]
            })
        }
        9 => gaussian(&input(0)?, node.v("radius")),
        10 => {
            let img = input(0)?;
            let soft = gaussian(&img, node.v("radius"));
            let amount = node.v("amount");
            let mut out = img.clone();
            for (i, p) in out.px.iter_mut().enumerate() {
                for k in 0..3 {
                    p[k] += (img.px[i][k] - soft.px[i][k]) * amount;
                }
            }
            out
        }
        11 => {
            let img = input(0)?;
            let threshold = node.v("threshold");
            let bright = img.clone().map(|p| {
                let excess = (luma(p) - threshold).max(0.0) / luma(p).max(1e-6);
                [p[0] * excess, p[1] * excess, p[2] * excess, p[3]]
            });
            let glow = gaussian(&bright, node.v("radius"));
            let i = node.v("intensity");
            let mut out = img.clone();
            for (n, p) in out.px.iter_mut().enumerate() {
                for k in 0..3 {
                    p[k] += glow.px[n][k] * i;
                }
            }
            out
        }
        12 => {
            let img = input(0)?;
            let s = node.v("strength");
            let mut out = img.clone();
            let l = |x: i32, y: i32| luma(img.at(x.clamp(0, img.w as i32 - 1) as usize, y.clamp(0, img.h as i32 - 1) as usize));
            for y in 0..img.h as i32 {
                for x in 0..img.w as i32 {
                    let gx = l(x + 1, y - 1) + 2.0 * l(x + 1, y) + l(x + 1, y + 1) - l(x - 1, y - 1) - 2.0 * l(x - 1, y) - l(x - 1, y + 1);
                    let gy = l(x - 1, y + 1) + 2.0 * l(x, y + 1) + l(x + 1, y + 1) - l(x - 1, y - 1) - 2.0 * l(x, y - 1) - l(x + 1, y - 1);
                    let e = (gx * gx + gy * gy).sqrt() * s;
                    out.px[y as usize * img.w + x as usize] = [e, e, e, 1.0];
                }
            }
            out
        }
        13 => {
            let img = input(0)?;
            let (amount, softness) = (node.v("amount"), node.v("softness").max(0.01));
            let (w, h) = (img.w as f32, img.h as f32);
            let mut out = img.clone();
            for y in 0..img.h {
                for x in 0..img.w {
                    let (u, v) = ((x as f32 + 0.5) / w - 0.5, (y as f32 + 0.5) / h - 0.5);
                    let d = ((u * u + v * v).sqrt() / 0.7071).clamp(0.0, 1.0);
                    let t = ((d - (1.0 - softness)) / softness).clamp(0.0, 1.0);
                    let k = 1.0 - amount * t * t * (3.0 - 2.0 * t);
                    let p = &mut out.px[y * img.w + x];
                    (0..3).for_each(|c| p[c] *= k);
                }
            }
            out
        }
        14 => {
            let img = input(0)?;
            let amount = node.v("amount");
            let seed = node.v("seed") as u32 ^ ((time * 24.0).round() as i32 as u32).wrapping_mul(2654435761);
            let mut out = img.clone();
            for (i, p) in out.px.iter_mut().enumerate() {
                let n = (hash01(i as u32, seed) - 0.5) * 2.0 * amount;
                (0..3).for_each(|c| p[c] = (p[c] + n * p[c].max(0.05).sqrt()).max(0.0));
            }
            out
        }
        15 => {
            let img = input(0)?;
            let (tx, ty, scale) = (node.v("tx"), node.v("ty"), node.v("scale").max(1e-3));
            let (s, c) = (-node.v("rotate").to_radians()).sin_cos();
            let (cx, cy) = (img.w as f32 / 2.0, img.h as f32 / 2.0);
            let mut out = Image::new(img.w, img.h, [0.0; 4]);
            for y in 0..img.h {
                for x in 0..img.w {
                    // Inverse map: output pixel -> source pixel (y up for positive ty).
                    let (dx, dy) = (x as f32 + 0.5 - cx - tx, y as f32 + 0.5 - cy + ty);
                    let (rx, ry) = ((dx * c - dy * s) / scale, (dx * s + dy * c) / scale);
                    out.px[y * img.w + x] = img.sample(rx + cx, ry + cy);
                }
            }
            out
        }
        16 => {
            let img = input(0)?;
            let x0 = (node.v("left").clamp(0.0, 0.99) * img.w as f32) as usize;
            let x1 = img.w - (node.v("right").clamp(0.0, 0.99) * img.w as f32) as usize;
            let y0 = (node.v("top").clamp(0.0, 0.99) * img.h as f32) as usize;
            let y1 = img.h - (node.v("bottom").clamp(0.0, 0.99) * img.h as f32) as usize;
            let (w, h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
            let mut out = Image::new(w, h, [0.0; 4]);
            for y in 0..h {
                for x in 0..w {
                    out.px[y * w + x] = img.at(x0 + x, y0 + y);
                }
            }
            out
        }
        17 => {
            let (w, h) = size();
            input(0)?.resized(w, h)
        }
        18 => {
            let key = node.get("key");
            let (tol, soft, despill) = (node.v("tolerance"), node.v("softness").max(1e-4), node.v("despill"));
            // Distance in chroma (colour minus its brightness), so shadows on the screen key too.
            let chroma = |p: [f32; 3]| {
                let l = (p[0] + p[1] + p[2]) / 3.0 + 1e-4;
                [p[0] / l, p[1] / l, p[2] / l]
            };
            let kc = chroma(key);
            let dominant = (0..3).max_by(|a, b| key[*a].partial_cmp(&key[*b]).unwrap()).unwrap_or(1);
            input(0)?.map(|p| {
                let pc = chroma([p[0], p[1], p[2]]);
                let d = ((pc[0] - kc[0]).powi(2) + (pc[1] - kc[1]).powi(2) + (pc[2] - kc[2]).powi(2)).sqrt() / 1.7;
                let alpha = ((d - tol) / soft).clamp(0.0, 1.0);
                let mut q = [p[0], p[1], p[2], p[3] * alpha];
                // Despill: the key channel can't exceed the other two's average.
                let others: f32 = (0..3).filter(|c| *c != dominant).map(|c| q[c]).sum::<f32>() / 2.0;
                q[dominant] -= (q[dominant] - others).max(0.0) * despill;
                q
            })
        }
        OUTPUT => input(0)?,
        _ => {
            let index = node.v("index").round() as usize;
            input(index.min(1))?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(label: &str) -> CopNode {
        CopNode::new(OPS.iter().position(|o| o.label == label).unwrap())
    }

    fn set(node: &mut CopNode, key: &str, value: [f32; 3]) {
        let mut at = 0;
        for p in node.def().params {
            if p.key == key {
                node.values[at..at + p.width()].copy_from_slice(&value[..p.width()]);
                return;
            }
            at += p.width();
        }
        panic!("no {key}");
    }

    fn small(label: &str) -> CopNode {
        let mut n = op(label);
        set(&mut n, "width", [32.0; 3]);
        set(&mut n, "height", [16.0; 3]);
        n
    }

    #[test]
    fn every_op_cooks_and_round_trips() {
        let mut constant = small("Constant");
        set(&mut constant, "color", [0.5, 0.25, 0.1]);
        let base = cook(&constant, &[], None, 0.0).unwrap();
        assert_eq!((base.w, base.h), (32, 16));
        let ramp = cook(&small("Ramp"), &[], None, 0.0).unwrap();
        for (i, def) in OPS.iter().enumerate() {
            let node = CopNode::new(i);
            assert_eq!(CopNode::from_params(&node.to_params()), node);
            if matches!(i, FILE | RENDER) {
                continue;
            }
            let inputs = vec![Some(ramp.clone()), Some(base.clone())];
            let out = cook(&node, &inputs[..def.inputs.max(0)], None, 0.5).unwrap_or_else(|e| panic!("{}: {e}", def.label));
            assert!(out.px.iter().flatten().all(|v| v.is_finite()), "{}", def.label);
            assert_eq!(Image::from_mesh(&out.to_mesh()).unwrap(), out);
        }
        assert!(cook(&op("Blur"), &[None], None, 0.0).is_err());
    }

    #[test]
    fn composite_and_colour_maths() {
        let mut red = small("Constant");
        set(&mut red, "color", [1.0, 0.0, 0.0]);
        set(&mut red, "alpha", [0.5; 3]);
        let mut blue = small("Constant");
        set(&mut blue, "color", [0.0, 0.0, 1.0]);
        let (r, b) = (cook(&red, &[], None, 0.0).unwrap(), cook(&blue, &[], None, 0.0).unwrap());
        let over = cook(&op("Composite"), &[Some(r.clone()), Some(b.clone())], None, 0.0).unwrap();
        let p = over.px[0];
        assert!((p[0] - 0.5).abs() < 1e-5 && (p[2] - 0.5).abs() < 1e-5 && (p[3] - 1.0).abs() < 1e-5, "{p:?}");
        // Exposure +1 doubles.
        let mut cc = op("Color Correct");
        set(&mut cc, "exposure", [1.0; 3]);
        let bright = cook(&cc, &[Some(b.clone())], None, 0.0).unwrap();
        assert!((bright.px[0][2] - 2.0).abs() < 1e-4);
        // Blur keeps a flat image flat and spreads a dot.
        let mut dot = Image::new(21, 21, [0.0, 0.0, 0.0, 1.0]);
        dot.px[10 * 21 + 10] = [1.0, 1.0, 1.0, 1.0];
        let blurred = cook(&op("Blur"), &[Some(dot.clone())], None, 0.0).unwrap();
        assert!(blurred.px[10 * 21 + 10][0] < 0.5 && blurred.px[10 * 21 + 12][0] > 0.0);
        let total: f32 = blurred.px.iter().map(|p| p[0]).sum();
        assert!((total - 1.0).abs() < 0.02, "energy {total}");
        // Chroma key removes green, keeps red.
        let mut screen = Image::new(2, 1, [0.0; 4]);
        screen.px = vec![[0.1, 0.8, 0.2, 1.0], [0.9, 0.1, 0.1, 1.0]];
        let keyed = cook(&op("Chroma Key"), &[Some(screen)], None, 0.0).unwrap();
        assert!(keyed.px[0][3] < 0.05 && keyed.px[1][3] > 0.95, "{:?}", keyed.px);
        // Transform by +4 px in x moves a dot right.
        let mut tf = op("Transform");
        set(&mut tf, "tx", [4.0; 3]);
        let moved = cook(&tf, &[Some(dot)], None, 0.0).unwrap();
        assert!(moved.px[10 * 21 + 14][0] > 0.9);
    }

    #[test]
    fn files_round_trip() {
        let dir = std::env::temp_dir();
        let mut img = Image::new(8, 4, [0.0; 4]);
        for (i, p) in img.px.iter_mut().enumerate() {
            *p = [i as f32 / 32.0, 0.5, 2.0, 1.0];
        }
        let exr = dir.join(format!("pfx_cop_{}.exr", std::process::id()));
        write_image(&img, exr.to_str().unwrap()).unwrap();
        let back = read_image(exr.to_str().unwrap()).unwrap();
        assert!((back.px[5][0] - img.px[5][0]).abs() < 1e-3 && (back.px[0][2] - 2.0).abs() < 1e-3, "EXR keeps full range");
        let png = dir.join(format!("pfx_cop_{}.png", std::process::id()));
        write_image(&img, png.to_str().unwrap()).unwrap();
        let back = read_image(png.to_str().unwrap()).unwrap();
        assert!((back.px[0][1] - 0.5).abs() < 0.01 && (back.px[0][2] - 1.0).abs() < 0.01, "PNG is sRGB, clipped");
    }
}
