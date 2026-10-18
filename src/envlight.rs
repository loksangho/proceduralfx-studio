// HDRI environment lights: lat-long images for MoonRay's EnvLight, plus what
// the viewport needs to match the render (background and image-based diffuse).
//
// The direction-to-image mapping reproduces MoonRay's EnvLight (EnvLight.cc:
// sLocalOrientation and local2uv), converted from MoonRay's right-handed space
// to the app's left-handed one (z mirrored, as for meshes).

use super::Vec3;
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

// Background lookups use this resolution; diffuse lighting a much smaller one.
const BACKGROUND_WIDTH: usize = 512;
const IRRADIANCE_WIDTH: usize = 32;

pub struct EnvMap {
    width: usize,
    height: usize,
    radiance: Vec<[f32; 3]>,
    irr_width: usize,
    irr_height: usize,
    // Cosine-weighted radiance (irradiance / pi) for each direction of a small lat-long grid.
    irradiance: Vec<[f32; 3]>,
}

// The light's local frame after a rotation of `rotation` degrees about +Y,
// as the rows MoonRay's node_xform uses (in MoonRay space).
pub fn rotation_rows(rotation: f32) -> [Vec3; 3] {
    let (s, c) = rotation.to_radians().sin_cos();
    [Vec3::new(c, 0.0, -s), Vec3::new(0.0, 1.0, 0.0), Vec3::new(s, 0.0, c)]
}

// Image coordinates (u to the right, v down from the top row, both 0..1) seen along app-space `dir`.
pub fn uv_of(dir: Vec3, rotation: f32) -> (f32, f32) {
    let d = Vec3::new(dir.x, dir.y, -dir.z);
    let r = rotation_rows(rotation);
    let l = Vec3::new(d.dot(r[0]), d.dot(r[1]), d.dot(r[2]));
    // MoonRay's z-up local frame: x = -l.z, y = -l.x, z = l.y.
    let (lx, ly, lz) = (-l.z, -l.x, l.y);
    let len = (lx * lx + ly * ly + lz * lz).sqrt().max(1e-20);
    let theta = (lz / len).clamp(-1.0, 1.0).acos();
    let mut u = ly.atan2(lx) / TAU;
    if u < 0.0 {
        u += 1.0;
    }
    (1.0 - u, theta / PI)
}

// The app-space direction an image coordinate shows (inverse of uv_of).
pub fn dir_of(u: f32, v: f32, rotation: f32) -> Vec3 {
    let phi = (1.0 - u) * TAU;
    let theta = v * PI;
    let (lx, ly, lz) = (theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
    // Back from the z-up frame (l = (-ly, lz, -lx)), then out of the light's rotation.
    let l = Vec3::new(-ly, lz, -lx);
    let r = rotation_rows(rotation);
    let d = r[0].scale(l.x).add(r[1].scale(l.y)).add(r[2].scale(l.z));
    Vec3::new(d.x, d.y, -d.z)
}

fn bilinear(map: &[[f32; 3]], w: usize, h: usize, u: f32, v: f32) -> [f32; 3] {
    let x = (u * w as f32 - 0.5).rem_euclid(w as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize % w, y.floor() as usize);
    let (x1, y1) = ((x0 + 1) % w, (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let at = |x: usize, y: usize| map[y * w + x];
    std::array::from_fn(|c| {
        let top = at(x0, y0)[c] * (1.0 - fx) + at(x1, y0)[c] * fx;
        let bottom = at(x0, y1)[c] * (1.0 - fx) + at(x1, y1)[c] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

// Box-filters a lat-long image down to `w` x `w / 2`.
fn downsample(pixels: &[[f32; 3]], sw: usize, sh: usize, w: usize) -> Vec<[f32; 3]> {
    let h = (w / 2).max(1);
    let mut out = vec![[0.0f32; 3]; w * h];
    let mut counts = vec![0u32; w * h];
    for y in 0..sh {
        let ty = (y * h / sh).min(h - 1);
        for x in 0..sw {
            let t = ty * w + (x * w / sw).min(w - 1);
            for c in 0..3 {
                out[t][c] += pixels[y * sw + x][c];
            }
            counts[t] += 1;
        }
    }
    for (texel, n) in out.iter_mut().zip(counts) {
        if n > 0 {
            *texel = texel.map(|v| v / n as f32);
        }
    }
    out
}

impl EnvMap {
    pub fn from_pixels(pixels: &[[f32; 3]], width: usize, height: usize) -> EnvMap {
        let bw = BACKGROUND_WIDTH.min(width.max(2));
        let radiance = if width <= bw { pixels.to_vec() } else { downsample(pixels, width, height, bw) };
        let (width, height) = if width <= bw { (width, height) } else { (bw, bw / 2) };
        let iw = IRRADIANCE_WIDTH;
        let ih = iw / 2;
        let small = downsample(&radiance, width, height, iw);
        // Directions and solid angles of the small map's texels (rotation 0; rotating the
        // map rotates its irradiance the same way).
        let texels: Vec<(Vec3, f32, [f32; 3])> = (0..ih)
            .flat_map(|y| (0..iw).map(move |x| (x, y)))
            .map(|(x, y)| {
                let (u, v) = ((x as f32 + 0.5) / iw as f32, (y as f32 + 0.5) / ih as f32);
                let solid = (TAU / iw as f32) * (PI / ih as f32) * (v * PI).sin();
                (dir_of(u, v, 0.0), solid, small[y * iw + x])
            })
            .collect();
        let irradiance = texels
            .iter()
            .map(|(n, _, _)| {
                let mut e = [0.0f32; 3];
                for (d, solid, l) in &texels {
                    let w = n.dot(*d).max(0.0) * solid;
                    for c in 0..3 {
                        e[c] += l[c] * w;
                    }
                }
                e.map(|v| v / PI)
            })
            .collect();
        EnvMap { width, height, radiance, irr_width: iw, irr_height: ih, irradiance }
    }

    pub fn radiance(&self, dir: Vec3, rotation: f32) -> [f32; 3] {
        let (u, v) = uv_of(dir, rotation);
        bilinear(&self.radiance, self.width, self.height, u, v)
    }

    // Diffuse lighting for a surface facing `normal`: for a white environment of
    // radiance 1 this is 1, like a full-strength ambient term.
    pub fn diffuse(&self, normal: Vec3, rotation: f32) -> [f32; 3] {
        let (u, v) = uv_of(normal, rotation);
        bilinear(&self.irradiance, self.irr_width, self.irr_height, u, v)
    }
}

// Loads (and caches) an HDRI; .exr and .hdr keep their full range.
pub fn load(path: &str) -> Result<Arc<EnvMap>, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<EnvMap>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(map) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return Ok(map.clone());
    }
    let image = image::open(path).map_err(|e| format!("could not read {path}: {e}"))?.to_rgb32f();
    let (w, h) = (image.width() as usize, image.height() as usize);
    if w < 2 || h < 1 {
        return Err(format!("{path} is too small to be an environment map"));
    }
    let pixels: Vec<[f32; 3]> = image.pixels().map(|p| [p[0], p[1], p[2]]).collect();
    let map = Arc::new(EnvMap::from_pixels(&pixels, w, h));
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_owned(), map.clone());
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_round_trips_and_poles() {
        for rotation in [0.0, 37.0, -120.0] {
            for (u, v) in [(0.1, 0.2), (0.5, 0.5), (0.83, 0.71), (0.33, 0.95)] {
                let d = dir_of(u, v, rotation);
                assert!((d.length() - 1.0).abs() < 1e-5);
                let (u2, v2) = uv_of(d, rotation);
                assert!((u - u2).abs() < 1e-4 && (v - v2).abs() < 1e-4, "{rotation}: ({u}, {v}) -> ({u2}, {v2})");
            }
        }
        // Up is the top row, down the bottom row.
        assert!(uv_of(Vec3::new(0.0, 1.0, 0.0), 0.0).1 < 1e-4);
        assert!((uv_of(Vec3::new(0.0, -1.0, 0.0), 0.0).1 - 1.0).abs() < 1e-4);
    }

    #[test]
    fn uniform_white_environment_lights_like_full_ambient() {
        let map = EnvMap::from_pixels(&vec![[1.0; 3]; 64 * 32], 64, 32);
        for n in [Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.3, -0.5, 0.8).normalize()] {
            let e = map.diffuse(n, 0.0);
            assert!((e[0] - 1.0).abs() < 0.03, "{n:?}: {e:?}");
        }
        assert!((map.radiance(Vec3::new(0.2, 0.3, 0.9).normalize(), 15.0)[1] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_bright_sky_lights_upward_faces() {
        // Top half bright, bottom half dark.
        let (w, h) = (64, 32);
        let pixels: Vec<[f32; 3]> = (0..w * h).map(|i| if i / w < h / 2 { [2.0; 3] } else { [0.0; 3] }).collect();
        let map = EnvMap::from_pixels(&pixels, w, h);
        let up = map.diffuse(Vec3::new(0.0, 1.0, 0.0), 0.0)[0];
        let down = map.diffuse(Vec3::new(0.0, -1.0, 0.0), 0.0)[0];
        let side = map.diffuse(Vec3::new(1.0, 0.0, 0.0), 0.0)[0];
        assert!((up - 2.0).abs() < 0.1 && down < 0.1 && (side - 1.0).abs() < 0.1, "{up} {down} {side}");
    }
}
