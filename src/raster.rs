// A small CPU rasterizer with a depth buffer for the viewport's shaded
// surfaces and wireframe. egui draws shapes in order with no depth test, so
// sorting whole triangles (the painter's algorithm) breaks on long or
// interpenetrating triangles; here every pixel keeps the nearest surface.
//
// Depth and attributes are interpolated perspective-correctly: 1/z is linear
// in screen space under a perspective camera, z itself under an orthographic one.

pub struct Texture {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
}

impl Texture {
    // Bilinear sample with repeat wrapping; (u, v) with v = 0 at the top row.
    fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        let (w, h) = (self.width as f32, self.height as f32);
        let x = (u * w - 0.5).rem_euclid(w);
        let y = (v * h - 0.5).rem_euclid(h);
        let (x0, y0) = (x.floor() as usize % self.width, y.floor() as usize % self.height);
        let (x1, y1) = ((x0 + 1) % self.width, (y0 + 1) % self.height);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let at = |x: usize, y: usize| self.pixels[y * self.width + x];
        std::array::from_fn(|c| {
            let top = at(x0, y0)[c] as f32 * (1.0 - fx) + at(x1, y0)[c] as f32 * fx;
            let bottom = at(x0, y1)[c] as f32 * (1.0 - fx) + at(x1, y1)[c] as f32 * fx;
            (top * (1.0 - fy) + bottom * fy) / 255.0
        })
    }
}

pub struct Raster {
    pub width: usize,
    pub height: usize,
    // Straight (non-premultiplied) RGBA; alpha 0 where nothing was drawn.
    pub color: Vec<[u8; 4]>,
    // Larger is nearer (1/z for perspective, -z for orthographic).
    pub depth: Vec<f32>,
    // How fast the depth key changes per pixel on the surface at each pixel (for edge bias).
    slope: Vec<f32>,
    orthographic: bool,
}

// A screen-space vertex: pixel position and camera-space depth (z > 0 in front).
#[derive(Clone, Copy)]
pub struct ScreenVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

// A triangle or edge to draw with Raster::draw.
pub struct Triangle<'a> {
    pub v: [ScreenVertex; 3],
    pub rgb: [[f32; 3]; 3],
    pub uv: Option<([[f32; 2]; 3], &'a Texture)>,
}

pub struct Line {
    pub a: ScreenVertex,
    pub b: ScreenVertex,
    pub rgba: [u8; 4],
}

// Rows y0..y0 + rows of the image; bands are drawn on separate threads.
struct Band<'a> {
    y0: usize,
    rows: usize,
    width: usize,
    color: &'a mut [[u8; 4]],
    depth: &'a mut [f32],
    slope: &'a mut [f32],
    orthographic: bool,
}

fn key(orthographic: bool, z: f32) -> f32 {
    if orthographic { -z } else { 1.0 / z.max(1e-6) }
}

impl Band<'_> {
    fn triangle(&mut self, t: &Triangle) {
        let (v, rgb) = (t.v, t.rgb);
        let area = (v[1].x - v[0].x) * (v[2].y - v[0].y) - (v[1].y - v[0].y) * (v[2].x - v[0].x);
        if !area.is_finite() || area.abs() < 1e-9 {
            return;
        }
        let band_end = (self.y0 + self.rows) as isize - 1;
        let min_y = (v.iter().map(|p| p.y).fold(f32::INFINITY, f32::min).floor() as isize).max(self.y0 as isize);
        let max_y = (v.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max).ceil() as isize).min(band_end);
        let min_x = v.iter().map(|p| p.x).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let max_x = (v.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max).ceil() as isize).min(self.width as isize - 1);
        if max_x < 0 || max_y < min_y {
            return;
        }
        let keys = v.map(|p| key(self.orthographic, p.z));
        // The key is a plane in screen space: its gradient gives the slope-scaled edge bias.
        let dkx = ((keys[1] - keys[0]) * (v[2].y - v[0].y) - (keys[2] - keys[0]) * (v[1].y - v[0].y)) / area;
        let dky = ((keys[2] - keys[0]) * (v[1].x - v[0].x) - (keys[1] - keys[0]) * (v[2].x - v[0].x)) / area;
        let slope = dkx.abs() + dky.abs();
        // Perspective-correct weights divide by z; orthographic ones don't.
        let inv = v.map(|p| if self.orthographic { 1.0 } else { 1.0 / p.z.max(1e-6) });
        let edge = |a: ScreenVertex, b: ScreenVertex, x: f32, y: f32| (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x);
        for py in min_y as usize..=max_y as usize {
            let y = py as f32 + 0.5;
            let row = (py - self.y0) * self.width;
            for px in min_x..=max_x as usize {
                let x = px as f32 + 0.5;
                let l0 = edge(v[1], v[2], x, y) / area;
                let l1 = edge(v[2], v[0], x, y) / area;
                let l2 = 1.0 - l0 - l1;
                if l0 < -1e-5 || l1 < -1e-5 || l2 < -1e-5 {
                    continue;
                }
                let k = l0 * keys[0] + l1 * keys[1] + l2 * keys[2];
                let i = row + px;
                if k <= self.depth[i] {
                    continue;
                }
                self.depth[i] = k;
                self.slope[i] = slope;
                let (w0, w1, w2) = (l0 * inv[0], l1 * inv[1], l2 * inv[2]);
                let s = w0 + w1 + w2;
                let (b0, b1, b2) = (w0 / s, w1 / s, w2 / s);
                let mut c: [f32; 3] = std::array::from_fn(|k| rgb[0][k] * b0 + rgb[1][k] * b1 + rgb[2][k] * b2);
                if let Some((uvs, texture)) = t.uv {
                    let u = uvs[0][0] * b0 + uvs[1][0] * b1 + uvs[2][0] * b2;
                    let tv = uvs[0][1] * b0 + uvs[1][1] * b1 + uvs[2][1] * b2;
                    let tex = texture.sample(u, tv);
                    c = std::array::from_fn(|k| c[k] * tex[k]);
                }
                let c = c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                self.color[i] = [c[0], c[1], c[2], 255];
            }
        }
    }

    // A depth-tested line, nudged toward the camera so it shows on the surface it outlines.
    fn line(&mut self, l: &Line) {
        let (a, b) = (l.a, l.b);
        let (top, bottom) = (a.y.min(b.y), a.y.max(b.y));
        if bottom < self.y0 as f32 || top >= (self.y0 + self.rows) as f32 {
            return;
        }
        let steps = (b.x - a.x).abs().max((b.y - a.y).abs()).ceil().max(1.0) as usize;
        let (ka, kb) = (key(self.orthographic, a.z), key(self.orthographic, b.z));
        let alpha = l.rgba[3] as f32 / 255.0;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let (x, y) = (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            if x < 0.0 || y < self.y0 as f32 {
                continue;
            }
            let (px, py) = (x as usize, y as usize);
            if px >= self.width || py >= self.y0 + self.rows {
                continue;
            }
            let i = (py - self.y0) * self.width + px;
            let k = ka + (kb - ka) * t;
            // Edges win against their own faces: a constant part, plus the surface's
            // depth change across a pixel (grazing faces change fast).
            let bias = k.abs() * 2e-4 + self.slope[i] * 1.5 + 1e-7;
            if k + bias < self.depth[i] {
                continue;
            }
            let dst = self.color[i];
            self.color[i] = if dst[3] == 0 {
                l.rgba
            } else {
                std::array::from_fn(|c| if c == 3 { 255 } else { (dst[c] as f32 * (1.0 - alpha) + l.rgba[c] as f32 * alpha) as u8 })
            };
        }
    }
}

impl Raster {
    pub fn new(width: usize, height: usize, orthographic: bool) -> Self {
        let n = width * height;
        Raster { width, height, color: vec![[0; 4]; n], depth: vec![f32::NEG_INFINITY; n], slope: vec![0.0; n], orthographic }
    }

    fn whole(&mut self) -> Band<'_> {
        Band {
            y0: 0,
            rows: self.height,
            width: self.width,
            color: &mut self.color,
            depth: &mut self.depth,
            slope: &mut self.slope,
            orthographic: self.orthographic,
        }
    }

    // A shaded triangle: per-vertex RGB (0..1) and optional texture coordinates.
    pub fn triangle(&mut self, v: [ScreenVertex; 3], rgb: [[f32; 3]; 3], uv: Option<([[f32; 2]; 3], &Texture)>) {
        self.whole().triangle(&Triangle { v, rgb, uv });
    }

    pub fn line(&mut self, a: ScreenVertex, b: ScreenVertex, rgba: [u8; 4]) {
        self.whole().line(&Line { a, b, rgba });
    }

    // Draws all triangles, then all lines, splitting the image into horizontal
    // bands rendered in parallel (each band sees every primitive, in order).
    pub fn draw(&mut self, triangles: &[Triangle], lines: &[Line]) {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
        let rows_per_band = self.height.div_ceil(threads.max(1)).max(1);
        let chunk = rows_per_band * self.width;
        let (width, orthographic) = (self.width, self.orthographic);
        std::thread::scope(|scope| {
            let bands = self.color.chunks_mut(chunk).zip(self.depth.chunks_mut(chunk)).zip(self.slope.chunks_mut(chunk)).enumerate();
            for (b, ((color, depth), slope)) in bands {
                scope.spawn(move || {
                    let rows = color.len() / width;
                    let mut band = Band { y0: b * rows_per_band, rows, width, color, depth, slope, orthographic };
                    for t in triangles {
                        band.triangle(t);
                    }
                    for l in lines {
                        band.line(l);
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> ScreenVertex {
        ScreenVertex { x, y, z }
    }

    #[test]
    fn parallel_drawing_matches_one_band() {
        let tris: Vec<Triangle> = (0..60)
            .map(|i| {
                let f = i as f32;
                let o = (f * 7.3) % 90.0;
                Triangle {
                    v: [v(o, (f * 3.1) % 70.0, 1.0 + f * 0.1), v(o + 40.0, (f * 5.7) % 80.0, 3.0), v(o + 10.0, 95.0 - (f * 2.3) % 50.0, 2.0 + (f % 5.0))],
                    rgb: [[(f % 10.0) / 10.0, 0.5, 1.0 - (f % 7.0) / 7.0]; 3],
                    uv: None,
                }
            })
            .collect();
        let lines: Vec<Line> = tris.iter().map(|t| Line { a: t.v[0], b: t.v[1], rgba: [0, 0, 0, 110] }).collect();
        let mut serial = Raster::new(100, 97, false);
        for t in &tris {
            serial.triangle(t.v, t.rgb, None);
        }
        for l in &lines {
            serial.line(l.a, l.b, l.rgba);
        }
        let mut parallel = Raster::new(100, 97, false);
        parallel.draw(&tris, &lines);
        assert!(serial.color == parallel.color, "bands must reproduce the single-threaded image exactly");
    }

    #[test]
    fn nearer_triangles_win_regardless_of_draw_order() {
        for near_first in [true, false] {
            let mut r = Raster::new(20, 20, false);
            let near = || (v(0.0, 0.0, 1.0), v(20.0, 0.0, 1.0), v(0.0, 20.0, 1.0));
            let far = || (v(0.0, 0.0, 5.0), v(20.0, 0.0, 5.0), v(0.0, 20.0, 5.0));
            let mut draw = |r: &mut Raster, t: (ScreenVertex, ScreenVertex, ScreenVertex), c: f32| r.triangle([t.0, t.1, t.2], [[c; 3]; 3], None);
            if near_first {
                draw(&mut r, near(), 1.0);
                draw(&mut r, far(), 0.0);
            } else {
                draw(&mut r, far(), 0.0);
                draw(&mut r, near(), 1.0);
            }
            assert_eq!(r.color[2 * 20 + 2], [255, 255, 255, 255]);
            assert_eq!(r.color[19 * 20 + 19][3], 0, "outside both triangles stays empty");
        }
    }

    #[test]
    fn interpenetrating_triangles_split_per_pixel() {
        // Two triangles crossing along x = 10: each is nearer on one side.
        let mut r = Raster::new(20, 4, false);
        r.triangle([v(0.0, -10.0, 1.0), v(20.0, -10.0, 3.0), v(0.0, 30.0, 1.0)], [[1.0, 0.0, 0.0]; 3], None);
        r.triangle([v(0.0, -10.0, 3.0), v(20.0, -10.0, 1.0), v(0.0, 30.0, 3.0)], [[0.0, 0.0, 1.0]; 3], None);
        assert_eq!(r.color[20 + 2][0], 255, "red is nearer on the left");
        assert_eq!(r.color[20 + 12][2], 255, "blue is nearer on the right");
    }

    #[test]
    fn hidden_edges_stay_hidden() {
        let mut r = Raster::new(20, 20, false);
        r.triangle([v(0.0, 0.0, 1.0), v(20.0, 0.0, 1.0), v(0.0, 20.0, 1.0)], [[1.0; 3]; 3], None);
        r.line(v(0.0, 5.0, 4.0), v(10.0, 5.0, 4.0), [255, 0, 0, 255]);
        assert_eq!(r.color[5 * 20 + 3], [255, 255, 255, 255], "an edge behind the surface is hidden");
        r.line(v(0.0, 5.0, 1.0), v(10.0, 5.0, 1.0), [255, 0, 0, 255]);
        assert_eq!(r.color[5 * 20 + 3], [255, 0, 0, 255], "an edge on the surface shows");
    }

    #[test]
    fn texture_coordinates_are_perspective_correct() {
        // A 2 x 1 texture: left half black, right half white.
        let texture = Texture { width: 2, height: 1, pixels: vec![[0, 0, 0, 255], [255, 255, 255, 255]] };
        let mut r = Raster::new(100, 2, false);
        // u goes 0 -> 1 left to right while depth goes 1 -> 9, so the far (right) side is squashed:
        // u = 0.5 falls at 90% across the screen, not halfway.
        r.triangle([v(0.0, -10.0, 1.0), v(100.0, -10.0, 9.0), v(0.0, 1000.0, 1.0)], [[1.0; 3]; 3], Some(([[0.0, 0.5], [1.0, 0.5], [0.0, 0.5]], &texture)));
        let row = 100;
        assert!(r.color[row + 60][0] < 128, "at 60% across u is still ~0.14 (an affine mapping would give 0.6)");
        assert!(r.color[row + 97][0] > 128, "near the right edge u passes 0.5");
    }
}
