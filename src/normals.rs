// Normal maps for lit 2D effects: each frame's shape, so a game's 2D lights can light smoke
// and fire from the side they're on. The shape comes from the rendered picture itself:
// coverage (alpha), softened, is read as height, so a puff of smoke is a rounded mound,
// thickest where it's most opaque, and its edges face outwards. That suits smoke, dust and
// clouds best; flat or glowing effects get the same treatment, so leave it off for those
// if the lighting looks wrong.
//
// Tangent space, as engines' 2D normal maps are: red is right, green is up (the
// "OpenGL" convention, which Godot and Unity use), blue towards the viewer; each stored as
// (n + 1) / 2. Where nothing shows, the normal faces the viewer.

use eframe::egui;
use rayon::prelude::*;

// How soft the shape is, as a fraction of the frame's shorter side.
const SOFTNESS: f32 = 0.025;

// A frame's coverage, blurred by a box of radius `r` three times (close to a Gaussian).
fn height(frame: &egui::ColorImage, r: usize) -> Vec<f32> {
    let [w, h] = frame.size;
    let mut data: Vec<f32> = frame.pixels.iter().map(|c| c.a() as f32 / 255.0).collect();
    let blur = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0; w * h];
        let n = (2 * r + 1) as f32;
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for d in -(r as isize)..=r as isize {
                    let (sx, sy) = if horizontal { ((x as isize + d).clamp(0, w as isize - 1) as usize, y) } else { (x, (y as isize + d).clamp(0, h as isize - 1) as usize) };
                    sum += src[sy * w + sx];
                }
                out[y * w + x] = sum / n;
            }
        }
        out
    };
    for _ in 0..3 {
        data = blur(&data, true);
        data = blur(&data, false);
    }
    data
}

// The frame's normal map, `strength` times as steep as the default.
pub fn normal_map(frame: &egui::ColorImage, strength: f32) -> egui::ColorImage {
    let [w, h] = frame.size;
    let r = ((w.min(h) as f32 * SOFTNESS).round() as usize).max(1);
    let height = height(frame, r);
    let at = |x: isize, y: isize| height[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize];
    // A soft edge rises over about 2r pixels: this scale makes it about 45 degrees.
    let k = strength * 2.0 * r as f32;
    let pixels = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5;
            // Down the image is down the screen: up is minus y.
            let dy_up = -(at(x, y + 1) - at(x, y - 1)) * 0.5;
            let n = [-dx * k, -dy_up * k, 1.0];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let byte = |v: f32| ((v / len * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
            egui::Color32::from_rgba_premultiplied(byte(n[0]), byte(n[1]), byte(n[2]), 255)
        })
        .collect();
    egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels }
}

pub fn normal_maps(frames: &[egui::ColorImage]) -> Vec<egui::ColorImage> {
    frames.par_iter().map(|f| normal_map(f, 1.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> egui::ColorImage {
        let pixels = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let inside = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < r;
                if inside { egui::Color32::WHITE } else { egui::Color32::TRANSPARENT }
            })
            .collect();
        egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels }
    }

    fn normal(image: &egui::ColorImage, x: usize, y: usize) -> [f32; 3] {
        let c = image.pixels[y * image.size[0] + x].to_array();
        [0, 1, 2].map(|k| c[k] as f32 / 255.0 * 2.0 - 1.0)
    }

    #[test]
    fn a_puffs_edges_face_outwards_and_its_middle_faces_the_viewer() {
        let map = normal_map(&disc(128, 128, 64.0, 64.0, 30.0), 1.0);
        let [x, y, z] = normal(&map, 64, 64);
        assert!(x.abs() < 0.05 && y.abs() < 0.05 && z > 0.95, "middle: {x} {y} {z}");
        // The right edge faces right, the top edge faces up (green up), the bottom down.
        let right = normal(&map, 94, 64);
        assert!(right[0] > 0.4 && right[1].abs() < 0.1, "right: {right:?}");
        let top = normal(&map, 64, 34);
        assert!(top[1] > 0.4 && top[0].abs() < 0.1, "top: {top:?}");
        let bottom = normal(&map, 64, 94);
        assert!(bottom[1] < -0.4, "bottom: {bottom:?}");
        // Empty space faces the viewer.
        assert!(normal(&map, 2, 2)[2] > 0.99);
        // Every normal has unit length (to within 8-bit rounding).
        for p in [(70, 50), (90, 80), (40, 40)] {
            let n = normal(&map, p.0, p.1);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 0.02, "{len}");
        }
    }
}
