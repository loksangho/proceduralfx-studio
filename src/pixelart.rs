// Pixel art from rendered frames, for sprite sheets: each block of `scale` × `scale`
// pixels becomes one pixel (averaged), every pixel is either opaque or clear, and the
// colours come from one palette shared by every frame, so nothing flickers between
// frames. The palette is picked from the frames' own colours (median cut in OKLab, then
// refined) or loaded from a palette file. Optionally an ordered (Bayer) dither, which
// also dithers soft edges such as smoke fading out, and a one-pixel outline.

use eframe::egui;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct PixelArt {
    // Each output pixel covers scale × scale rendered pixels.
    pub scale: u32,
    // How many colours to pick from the frames; 0 keeps their colours.
    pub colors: u32,
    // A palette to use instead (sRGB); empty picks one.
    pub palette: Vec<[u8; 3]>,
    pub dither: bool,
    pub outline: bool,
}

impl Default for PixelArt {
    fn default() -> Self {
        PixelArt { scale: 4, colors: 16, palette: Vec::new(), dither: false, outline: false }
    }
}

// 4×4 Bayer thresholds, in (0, 1).
const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];

fn bayer(x: usize, y: usize) -> f32 {
    (BAYER[y % 4][x % 4] + 0.5) / 16.0
}

// sRGB (0–255) to OKLab, a space where distance matches how different colours look.
fn oklab(c: [u8; 3]) -> [f32; 3] {
    let lin = |v: u8| egui::ecolor::linear_f32_from_gamma_u8(v);
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn distance2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

// One frame shrunk by `scale`: each pixel the average of its block, in linear
// premultiplied colour (so clear pixels don't darken the edges).
pub fn downscale(frame: &egui::ColorImage, scale: u32) -> egui::ColorImage {
    let k = scale.max(1) as usize;
    let (w, h) = ((frame.size[0] / k).max(1), (frame.size[1] / k).max(1));
    let mut out = egui::ColorImage::filled([w, h], egui::Color32::TRANSPARENT);
    for y in 0..h {
        for x in 0..w {
            let mut sum = egui::Rgba::TRANSPARENT;
            let mut n = 0.0f32;
            for yy in y * k..((y + 1) * k).min(frame.size[1]) {
                for xx in x * k..((x + 1) * k).min(frame.size[0]) {
                    sum = sum + egui::Rgba::from(frame.pixels[yy * frame.size[0] + xx]);
                    n += 1.0;
                }
            }
            // Coverage counts for more than its share, so a spark thinner than a pixel
            // still makes one (half a pixel covered reads as about 70%).
            let mean = sum * (1.0 / n.max(1.0));
            let alpha = mean.a();
            let boosted = if alpha > 0.0 { mean * (alpha.sqrt() / alpha) } else { mean };
            out.pixels[y * w + x] = boosted.into();
        }
    }
    out
}

// A frame made `scale` times bigger, each pixel a solid block: for showing pixel art.
pub fn upscale(frame: &egui::ColorImage, scale: u32) -> egui::ColorImage {
    let k = scale.max(1) as usize;
    let [w, h] = frame.size;
    let mut out = egui::ColorImage::filled([w * k, h * k], egui::Color32::TRANSPARENT);
    for y in 0..h * k {
        for x in 0..w * k {
            out.pixels[y * w * k + x] = frame.pixels[(y / k) * w + x / k];
        }
    }
    out
}

// A pixel's colour without its alpha, in sRGB.
fn opaque_colour(c: egui::Color32) -> [u8; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r, g, b]
}

// `count` colours that represent `colours` (with how often each appears): median cut in
// OKLab, then a few rounds of k-means. The same input always gives the same palette.
pub fn pick_palette(colours: &[([u8; 3], u32)], count: usize) -> Vec<[u8; 3]> {
    if colours.is_empty() || count == 0 {
        return Vec::new();
    }
    if colours.len() <= count {
        return colours.iter().map(|(c, _)| *c).collect();
    }
    let points: Vec<([f32; 3], f32, [u8; 3])> = colours.iter().map(|(c, n)| (oklab(*c), *n as f32, *c)).collect();
    // Median cut: split the box with the most spread along its widest axis.
    let mut boxes: Vec<Vec<usize>> = vec![(0..points.len()).collect()];
    while boxes.len() < count {
        let spread = |b: &Vec<usize>| -> (f32, usize) {
            let mut best = (0.0, 0);
            for axis in 0..3 {
                let (lo, hi) = b.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &i| (lo.min(points[i].0[axis]), hi.max(points[i].0[axis])));
                let weight: f32 = b.iter().map(|&i| points[i].1).sum::<f32>().sqrt();
                if (hi - lo) * weight > best.0 {
                    best = ((hi - lo) * weight, axis);
                }
            }
            best
        };
        let Some((index, axis)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| (i, spread(b)))
            .max_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
            .map(|(i, (_, axis))| (i, axis))
        else {
            break;
        };
        let mut b = boxes.swap_remove(index);
        b.sort_by(|&x, &y| points[x].0[axis].total_cmp(&points[y].0[axis]).then(points[x].2.cmp(&points[y].2)));
        // Split where half the weight is on each side.
        let total: f32 = b.iter().map(|&i| points[i].1).sum();
        let mut acc = 0.0;
        let mut at = 1;
        for (k, &i) in b.iter().enumerate() {
            acc += points[i].1;
            if acc >= total * 0.5 {
                at = (k + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let rest = b.split_off(at);
        boxes.push(b);
        boxes.push(rest);
    }
    let mean = |members: &[usize]| -> [f32; 3] {
        let mut sum = [0.0f32; 3];
        let mut weight = 0.0;
        for &i in members {
            for a in 0..3 {
                sum[a] += points[i].0[a] * points[i].1;
            }
            weight += points[i].1;
        }
        sum.map(|v| v / weight.max(1e-6))
    };
    let mut centres: Vec<[f32; 3]> = boxes.iter().map(|b| mean(b)).collect();
    // k-means: each colour to its nearest centre, each centre to its colours' mean.
    for _ in 0..6 {
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); centres.len()];
        for (i, p) in points.iter().enumerate() {
            let nearest = (0..centres.len()).min_by(|&a, &b| distance2(p.0, centres[a]).total_cmp(&distance2(p.0, centres[b]))).unwrap();
            groups[nearest].push(i);
        }
        for (centre, group) in centres.iter_mut().zip(&groups) {
            if !group.is_empty() {
                *centre = mean(group);
            }
        }
    }
    // Each centre as the real colour nearest it, so the palette holds colours that appear.
    let mut palette: Vec<[u8; 3]> = centres
        .iter()
        .map(|c| points.iter().min_by(|a, b| distance2(a.0, *c).total_cmp(&distance2(b.0, *c)).then(a.2.cmp(&b.2))).unwrap().2)
        .collect();
    palette.sort_by(|a, b| oklab(*a)[0].total_cmp(&oklab(*b)[0]).then(a.cmp(b)));
    palette.dedup();
    palette
}

// The palette's colours in OKLab, and how far apart neighbouring colours typically are
// (how strongly a dither moves a colour).
struct Lookup {
    lab: Vec<[f32; 3]>,
    spread: f32,
}

impl Lookup {
    fn new(palette: &[[u8; 3]]) -> Self {
        let lab: Vec<[f32; 3]> = palette.iter().map(|c| oklab(*c)).collect();
        let nearest_gaps: Vec<f32> = lab
            .iter()
            .enumerate()
            .map(|(i, a)| lab.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, b)| distance2(*a, *b)).fold(f32::MAX, f32::min).sqrt())
            .filter(|d| d.is_finite() && *d < f32::MAX)
            .collect();
        let spread = if nearest_gaps.is_empty() { 0.0 } else { nearest_gaps.iter().sum::<f32>() / nearest_gaps.len() as f32 };
        Lookup { lab, spread }
    }

    fn nearest(&self, c: [f32; 3]) -> usize {
        (0..self.lab.len()).min_by(|&a, &b| distance2(c, self.lab[a]).total_cmp(&distance2(c, self.lab[b]))).unwrap_or(0)
    }
}

// The frames as pixel art (see the top of this file). Every frame comes out the same size.
pub fn pixelate(frames: &[egui::ColorImage], settings: &PixelArt) -> Vec<egui::ColorImage> {
    let small: Vec<egui::ColorImage> = frames.iter().map(|f| downscale(f, settings.scale)).collect();
    // Opaque or clear: half alpha, or the dither's threshold.
    let solid = |x: usize, y: usize, c: egui::Color32| -> bool {
        let alpha = c.a() as f32 / 255.0;
        alpha > 0.0 && alpha >= if settings.dither { bayer(x, y) } else { 0.5 }
    };
    let palette = if !settings.palette.is_empty() {
        settings.palette.clone()
    } else if settings.colors > 0 {
        let mut counts: std::collections::BTreeMap<[u8; 3], u32> = std::collections::BTreeMap::new();
        for frame in &small {
            for (i, c) in frame.pixels.iter().enumerate() {
                if solid(i % frame.size[0], i / frame.size[0], *c) {
                    *counts.entry(opaque_colour(*c)).or_default() += 1;
                }
            }
        }
        let colours: Vec<([u8; 3], u32)> = counts.into_iter().collect();
        pick_palette(&colours, settings.colors as usize)
    } else {
        Vec::new()
    };
    let lookup = (!palette.is_empty()).then(|| Lookup::new(&palette));
    // The outline: the palette's darkest colour if it's dark, else near-black.
    let outline_colour = palette.iter().copied().min_by(|a, b| oklab(*a)[0].total_cmp(&oklab(*b)[0])).filter(|c| oklab(*c)[0] < 0.3).unwrap_or([20, 18, 28]);
    small
        .into_iter()
        .map(|frame| {
            let [w, h] = frame.size;
            let mut out = egui::ColorImage::filled([w, h], egui::Color32::TRANSPARENT);
            for y in 0..h {
                for x in 0..w {
                    let c = frame.pixels[y * w + x];
                    if !solid(x, y, c) {
                        continue;
                    }
                    let mut rgb = opaque_colour(c);
                    if let Some(lookup) = &lookup {
                        let mut lab = oklab(rgb);
                        if settings.dither {
                            lab[0] += (bayer(x, y) - 0.5) * lookup.spread;
                        }
                        rgb = palette[lookup.nearest(lab)];
                    }
                    out.pixels[y * w + x] = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                }
            }
            if settings.outline {
                let filled: Vec<bool> = out.pixels.iter().map(|p| p.a() > 0).collect();
                let [r, g, b] = outline_colour;
                for y in 0..h {
                    for x in 0..w {
                        if filled[y * w + x] {
                            continue;
                        }
                        let touches = (x > 0 && filled[y * w + x - 1]) || (x + 1 < w && filled[y * w + x + 1]) || (y > 0 && filled[(y - 1) * w + x]) || (y + 1 < h && filled[(y + 1) * w + x]);
                        if touches {
                            out.pixels[y * w + x] = egui::Color32::from_rgb(r, g, b);
                        }
                    }
                }
            }
            out
        })
        .collect()
}

// A palette from a file: .hex (one RRGGBB per line, as Lospec exports), .gpl (GIMP), or
// an image (each different colour in it, in order of first appearance).
pub fn load_palette(path: &Path) -> Result<Vec<[u8; 3]>, String> {
    let name = path.display();
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut colours: Vec<[u8; 3]> = Vec::new();
    match extension.as_str() {
        "hex" | "txt" => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("couldn't read {name}: {e}"))?;
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(';')) {
                let hex = line.trim_start_matches('#');
                let hex = if hex.len() == 8 && hex[..2].eq_ignore_ascii_case("ff") { &hex[2..] } else { hex };
                let value = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6).ok_or_else(|| format!("{name}: \"{line}\" isn't a colour like 1a2b3c"))?;
                colours.push([(value >> 16) as u8, (value >> 8) as u8, value as u8]);
            }
        }
        "gpl" => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("couldn't read {name}: {e}"))?;
            for line in text.lines().map(str::trim) {
                let mut parts = line.split_whitespace();
                let rgb: Option<Vec<u8>> = (0..3).map(|_| parts.next().and_then(|p| p.parse().ok())).collect();
                if let Some(rgb) = rgb {
                    colours.push([rgb[0], rgb[1], rgb[2]]);
                }
            }
        }
        _ => {
            let image = image::open(path).map_err(|e| format!("couldn't read {name} as a palette (.hex, .gpl or an image): {e}"))?.to_rgba8();
            for p in image.pixels().filter(|p| p.0[3] > 0) {
                let c = [p.0[0], p.0[1], p.0[2]];
                if !colours.contains(&c) {
                    colours.push(c);
                    if colours.len() > 256 {
                        return Err(format!("{name} has more than 256 colours: use a palette image with one swatch per colour"));
                    }
                }
            }
        }
    }
    let mut seen = Vec::new();
    colours.retain(|c| if seen.contains(c) { false } else { seen.push(*c); true });
    if colours.is_empty() {
        return Err(format!("{name} has no colours in it"));
    }
    Ok(colours)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A soft orange-to-yellow disc, fading out at the edge, growing over the frames.
    fn frames() -> Vec<egui::ColorImage> {
        (0..4)
            .map(|f| {
                let size = 64;
                let mut image = egui::ColorImage::filled([size, size], egui::Color32::TRANSPARENT);
                for y in 0..size {
                    for x in 0..size {
                        let d = ((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt() / (16.0 + f as f32 * 4.0);
                        let alpha = (1.0 - d).clamp(0.0, 1.0);
                        let t = y as f32 / size as f32;
                        let colour = egui::Color32::from_rgb(255, (120.0 + 120.0 * t) as u8, (40.0 * t) as u8);
                        image.pixels[y * size + x] = colour.gamma_multiply(alpha);
                    }
                }
                image
            })
            .collect()
    }

    #[test]
    fn frames_shrink_to_hard_edged_pixels_from_one_palette() {
        let settings = PixelArt { scale: 4, colors: 6, ..Default::default() };
        let out = pixelate(&frames(), &settings);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|f| f.size == [16, 16]));
        let mut colours = std::collections::BTreeSet::new();
        for f in &out {
            for p in &f.pixels {
                assert!(p.a() == 0 || p.a() == 255, "every pixel opaque or clear");
                if p.a() == 255 {
                    colours.insert([p.r(), p.g(), p.b()]);
                }
            }
        }
        assert!(colours.len() <= 6 && colours.len() >= 3, "at most the palette's 6 colours, shared by every frame: {colours:?}");
        // The disc grows: later frames cover more pixels.
        let filled = |f: &egui::ColorImage| f.pixels.iter().filter(|p| p.a() > 0).count();
        assert!(filled(&out[3]) > filled(&out[0]));
        // The same frames give the same pixels.
        assert_eq!(pixelate(&frames(), &settings)[2].pixels, out[2].pixels);
    }

    #[test]
    fn a_dither_softens_the_edge_and_an_outline_rings_the_shape() {
        let plain = pixelate(&frames(), &PixelArt { scale: 2, colors: 4, ..Default::default() });
        let dithered = pixelate(&frames(), &PixelArt { scale: 2, colors: 4, dither: true, ..Default::default() });
        assert_ne!(plain[0].pixels, dithered[0].pixels);
        let outlined = pixelate(&frames(), &PixelArt { scale: 2, colors: 4, outline: true, ..Default::default() });
        let filled = |f: &egui::ColorImage| f.pixels.iter().filter(|p| p.a() > 0).count();
        assert!(filled(&outlined[0]) > filled(&plain[0]), "the outline adds a ring");
        // Every clear pixel beside the shape is now the outline.
        let [w, h] = plain[0].size;
        let shape = |x: usize, y: usize| plain[0].pixels[y * w + x].a() > 0;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if !shape(x, y) && (shape(x - 1, y) || shape(x + 1, y) || shape(x, y - 1) || shape(x, y + 1)) {
                    assert!(outlined[0].pixels[y * w + x].a() > 0, "outline at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn a_given_palette_is_used_exactly() {
        let palette = vec![[20, 12, 28], [210, 125, 44], [218, 212, 94], [222, 238, 214]];
        let out = pixelate(&frames(), &PixelArt { scale: 4, palette: palette.clone(), ..Default::default() });
        assert!(out.iter().flat_map(|f| &f.pixels).filter(|p| p.a() > 0).all(|p| palette.contains(&[p.r(), p.g(), p.b()])));
    }

    #[test]
    fn colours_kept_when_no_palette_is_asked_for() {
        let out = pixelate(&frames(), &PixelArt { scale: 4, colors: 0, ..Default::default() });
        let colours: std::collections::BTreeSet<_> = out.iter().flat_map(|f| &f.pixels).filter(|p| p.a() > 0).map(|p| [p.r(), p.g(), p.b()]).collect();
        assert!(colours.len() > 10, "the frames' own colours");
    }

    #[test]
    fn palettes_load_from_hex_gpl_and_images() {
        let dir = std::env::temp_dir().join(format!("pfx_palettes_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.hex"), "1a1c2c\n5D275D\n\nb13e53\n").unwrap();
        assert_eq!(load_palette(&dir.join("p.hex")).unwrap(), vec![[0x1a, 0x1c, 0x2c], [0x5d, 0x27, 0x5d], [0xb1, 0x3e, 0x53]]);
        std::fs::write(dir.join("p.gpl"), "GIMP Palette\nName: test\nColumns: 4\n#\n 26  28  44\tdark\n255 255 255\twhite\n").unwrap();
        assert_eq!(load_palette(&dir.join("p.gpl")).unwrap(), vec![[26, 28, 44], [255, 255, 255]]);
        let mut image = image::RgbaImage::new(3, 1);
        image.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        image.put_pixel(1, 0, image::Rgba([4, 5, 6, 255]));
        image.put_pixel(2, 0, image::Rgba([1, 2, 3, 255]));
        image.save(dir.join("p.png")).unwrap();
        assert_eq!(load_palette(&dir.join("p.png")).unwrap(), vec![[1, 2, 3], [4, 5, 6]]);
        std::fs::write(dir.join("bad.hex"), "12345\n").unwrap();
        assert!(load_palette(&dir.join("bad.hex")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
