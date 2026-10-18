// Render textures. MoonRay's texture system only reads tiled, mip-mapped
// images (like maketx's .tx files), so ordinary images are converted to
// tiled, mip-mapped OpenEXR before rendering. Colour images are decoded
// from sRGB to linear on the way; data images (roughness, normals) are not.
// Converted files are cached by source path, size, modification time and mode.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("proceduralfx_textures")
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

// The file to give MoonRay for `file`: itself when it's already a .tx, else a
// converted EXR. `colour` linearises sRGB images.
pub fn prepare(file: &str, colour: bool) -> Result<String, String> {
    let source = Path::new(file.trim());
    if source.extension().is_some_and(|e| e.eq_ignore_ascii_case("tx")) {
        return Ok(file.trim().to_owned());
    }
    let meta = std::fs::metadata(source).map_err(|e| format!("texture {}: {e}", source.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (source, meta.len(), meta.modified().ok(), colour).hash(&mut hasher);
    let out = cache_dir().join(format!("{:016x}.exr", hasher.finish()));
    if out.exists() {
        return Ok(out.to_string_lossy().into_owned());
    }
    let img = image::open(source).map_err(|e| format!("texture {}: {e}", source.display()))?;
    // EXR and HDR images are already linear.
    let linear_source = matches!(img, image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_));
    let rgb = img.to_rgb32f();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let decode = |c: f32| if colour && !linear_source { srgb_to_linear(c) } else { c };
    let base: Vec<[f32; 3]> = rgb.pixels().map(|p| [decode(p[0]), decode(p[1]), decode(p[2])]).collect();
    std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    let tmp = out.with_extension("exr.part");
    write_mipmapped(&tmp, w, h, base)?;
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out.to_string_lossy().into_owned())
}

// Box-filters each level from the previous one, rounding sizes up like OpenEXR.
fn write_mipmapped(path: &Path, w: usize, h: usize, base: Vec<[f32; 3]>) -> Result<(), String> {
    use exr::math::RoundingMode;
    use exr::prelude::*;
    let rounding = RoundingMode::Up;
    let sizes: Vec<Vec2<usize>> = exr::meta::mip_map_levels(rounding, Vec2(w, h)).map(|(_, s)| s).collect();
    let mut levels = vec![base];
    let mut size = (w, h);
    for target in sizes.iter().skip(1) {
        let prev = levels.last().unwrap();
        let (tw, th) = (target.0, target.1);
        let mut next = Vec::with_capacity(tw * th);
        for y in 0..th {
            for x in 0..tw {
                let mut sum = [0.0f32; 3];
                let mut n = 0.0;
                for (sx, sy) in [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)] {
                    if sx < size.0 && sy < size.1 {
                        let p = prev[sy * size.0 + sx];
                        (0..3).for_each(|k| sum[k] += p[k]);
                        n += 1.0;
                    }
                }
                next.push(sum.map(|s| s / n));
            }
        }
        levels.push(next);
        size = (tw, th);
    }
    let channel = |k: usize, name: &str| {
        let data = levels.iter().map(|l| FlatSamples::F32(l.iter().map(|p| p[k]).collect())).collect();
        AnyChannel::new(name, Levels::Mip { level_data: data, rounding_mode: rounding })
    };
    let channels = AnyChannels::sort(SmallVec::from_vec(vec![channel(0, "R"), channel(1, "G"), channel(2, "B")]));
    let encoding = Encoding { compression: Compression::ZIP16, blocks: Blocks::Tiles(Vec2(64, 64)), line_order: LineOrder::Increasing };
    let layer = Layer::new(Vec2(w, h), LayerAttributes::default(), encoding, channels);
    let image = Image::from_layer(layer);
    image.write().to_file(path).map_err(|e| format!("writing {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_cached_tiled_mipmapped_exr() {
        let png = std::env::temp_dir().join(format!("pfx_tex_{}.png", std::process::id()));
        image::RgbImage::from_fn(40, 24, |x, _| image::Rgb([if x < 20 { 255 } else { 128 }, 0, 0])).save(&png).unwrap();
        let out = prepare(png.to_str().unwrap(), true).unwrap();
        assert!(out.ends_with(".exr"));
        assert_eq!(prepare(png.to_str().unwrap(), true).unwrap(), out, "cached");
        assert_ne!(prepare(png.to_str().unwrap(), false).unwrap(), out, "data and colour versions differ");
        let meta = exr::meta::MetaData::read_from_file(&out, false).unwrap();
        let header = &meta.headers[0];
        assert!(matches!(header.blocks, exr::meta::BlockDescription::Tiles(t) if t.level_mode == exr::meta::attribute::LevelMode::MipMap));
        // sRGB 128 decodes to about 0.216 linear.
        let img = image::open(&out).unwrap().to_rgb32f();
        assert!((img.get_pixel(0, 0)[0] - 1.0).abs() < 1e-3 && (img.get_pixel(30, 0)[0] - 0.216).abs() < 0.01);
        assert!(prepare("/no/such/texture.png", true).is_err());
        assert_eq!(prepare("/already/tiled.tx", true).unwrap(), "/already/tiled.tx");
    }
}
