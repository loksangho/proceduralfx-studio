// How each preset looks, exported: the middle frame of its range (the first, for rigid
// bodies: see `frame_time`), rendered as Export
// Sprite Sheet renders it, must still look like the picture in preset_pictures/.
//
// The pictures are compared shrunk to about 40 pixels across, so that platforms' small
// differences (antialiased edges, the volume renderer's per-pixel jitter) don't count,
// while a change in how an effect looks (its colour, brightness, shape or size) does.
// On a failure, the new picture and the difference are written to
// target/preset-pictures/ (CI keeps them with the run), to look at.
//
// When a preset's look changes on purpose, look at the new pictures, then write them again:
//     PFX_WRITE_PICTURES=1 cargo test --release preset_pictures
use crate::*;

// The shrunk pictures may differ by this much on average, and this much in any one place
// (out of 255, per channel).
const MEAN: f32 = 2.5;
const MOST: f32 = 48.0;
// Pictures are kept at most this big.
const KEPT: u32 = 160;
const COMPARED: u32 = 40;

fn picture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app_tests/preset_pictures")
}

fn file_name(preset: &presets::Preset) -> String {
    format!("{}.png", preset.name.to_lowercase().replace(' ', "_"))
}

// Which frame is pictured: the middle of the range. Rigid bodies bouncing off each other
// amplify the platforms' last-bit maths differences until, mid-collapse, the Breaking
// wall's pieces lie in different places on Linux, Windows and a Mac; so those are pictured
// at the start, before anything collides (old_scene_tests.rs checks where they land).
fn frame_time(preset: &presets::Preset) -> f32 {
    let (start, end) = preset.range;
    let rigid = preset.scene.lines().any(|l| l.starts_with("node ") && (l.contains(" obj rbd_") || l.contains(" obj material_fracture")));
    if rigid {
        start
    } else {
        (((start + end) * 0.5) * sims::SIM_FPS).round() / sims::SIM_FPS
    }
}

// The preset's pictured frame, as Preview (and so Export) renders it.
fn render(gpu: gpu_viewport::GpuViewport, preset: &presets::Preset) -> image::RgbaImage {
    sims::clear_all();
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    app.load_preset(preset);
    let at = frame_time(preset);
    (app.playback_start, app.playback_end) = (at, at);
    app.sprite_sheet.seamless = false;
    app.preview_sprite_sheet();
    super::realtime_render_tests::run_recording_for(&mut app, 2000);
    let frame = app.flipbook_viewer.as_ref().expect("the preview opens").frames()[0].clone();
    let mut image = image::RgbaImage::new(frame.size[0] as u32, frame.size[1] as u32);
    for (pixel, c) in image.pixels_mut().zip(&frame.pixels) {
        *pixel = image::Rgba(c.to_srgba_unmultiplied());
    }
    image
}

// The picture shrunk to fit `side`, averaging in premultiplied colour, as RGBA 0-255.
fn shrink(image: &image::RgbaImage, side: u32) -> (u32, u32, Vec<[f32; 4]>) {
    let (w, h) = image.dimensions();
    let k = (w.max(h) as f32 / side as f32).max(1.0);
    let (sw, sh) = (((w as f32 / k).round() as u32).max(1), ((h as f32 / k).round() as u32).max(1));
    let mut out = vec![[0.0f32; 4]; (sw * sh) as usize];
    let mut counts = vec![0.0f32; (sw * sh) as usize];
    for (x, y, p) in image.enumerate_pixels() {
        let (cx, cy) = (((x as f32 / k) as u32).min(sw - 1), ((y as f32 / k) as u32).min(sh - 1));
        let i = (cy * sw + cx) as usize;
        let a = p.0[3] as f32 / 255.0;
        for c in 0..3 {
            out[i][c] += p.0[c] as f32 * a;
        }
        out[i][3] += p.0[3] as f32;
        counts[i] += 1.0;
    }
    for (v, n) in out.iter_mut().zip(&counts) {
        *v = v.map(|x| x / n.max(1.0));
    }
    (sw, sh, out)
}

// How far apart two pictures look: the mean and the largest difference of their shrunk
// versions, per channel out of 255; None if they're different shapes.
fn difference(a: &image::RgbaImage, b: &image::RgbaImage) -> Option<(f32, f32, Vec<f32>)> {
    let (aw, ah, sa) = shrink(a, COMPARED);
    let (bw, bh, sb) = shrink(b, COMPARED);
    if (aw, ah) != (bw, bh) {
        return None;
    }
    let cells: Vec<f32> = sa.iter().zip(&sb).map(|(x, y)| (0..4).map(|c| (x[c] - y[c]).abs()).fold(0.0, f32::max)).collect();
    let mean = sa.iter().zip(&sb).map(|(x, y)| (0..4).map(|c| (x[c] - y[c]).abs()).sum::<f32>() / 4.0).sum::<f32>() / cells.len() as f32;
    Some((mean, cells.iter().cloned().fold(0.0, f32::max), cells))
}

fn kept_size(image: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = image.dimensions();
    let k = (w.max(h) as f32 / KEPT as f32).max(1.0);
    image::imageops::resize(image, ((w as f32 / k).round() as u32).max(1), ((h as f32 / k).round() as u32).max(1), image::imageops::FilterType::Triangle)
}

#[test]
fn presets_still_look_as_they_did() {
    let write = std::env::var_os("PFX_WRITE_PICTURES").is_some();
    let failures_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/preset-pictures");
    let mut problems = Vec::new();
    for preset in presets::PRESETS {
        let Some(gpu) = gpu_viewport::headless() else {
            eprintln!("no GPU here; skipping");
            return;
        };
        let now = kept_size(&render(gpu, preset));
        let path = picture_dir().join(file_name(preset));
        if write {
            std::fs::create_dir_all(picture_dir()).unwrap();
            now.save(&path).unwrap();
            continue;
        }
        let Ok(before) = image::open(&path).map(|i| i.to_rgba8()) else {
            problems.push(format!("{}: no picture to compare with (write them, see the top of preset_pictures.rs)", preset.name));
            continue;
        };
        let verdict = match difference(&before, &now) {
            None => Some(format!("{}: now {}×{}, was {}×{}", preset.name, now.width(), now.height(), before.width(), before.height())),
            Some((mean, most, _)) if mean > MEAN || most > MOST => Some(format!("{}: looks different (mean {mean:.1}, most {most:.0}, of 255)", preset.name)),
            Some(_) => None,
        };
        if let Some(problem) = verdict {
            // The new picture and the difference, to look at.
            std::fs::create_dir_all(&failures_dir).unwrap();
            let stem = file_name(preset).replace(".png", "");
            now.save(failures_dir.join(format!("{stem}_now.png"))).unwrap();
            before.save(failures_dir.join(format!("{stem}_was.png"))).unwrap();
            if let Some((_, _, cells)) = difference(&before, &now) {
                let (w, h, _) = shrink(&now, COMPARED);
                let diff = image::GrayImage::from_fn(w, h, |x, y| image::Luma([(cells[(y * w + x) as usize] * 4.0).min(255.0) as u8]));
                image::imageops::resize(&diff, w * 4, h * 4, image::imageops::FilterType::Nearest).save(failures_dir.join(format!("{stem}_difference.png"))).unwrap();
            }
            problems.push(problem);
        }
    }
    assert!(problems.is_empty(), "{}\nThe new pictures are in {}", problems.join("\n"), failures_dir.display());
}

#[test]
fn small_differences_pass_and_changes_dont() {
    let base = image::RgbaImage::from_fn(120, 80, |x, y| if (x as i32 - 60).pow(2) + (y as i32 - 40).pow(2) < 900 { image::Rgba([250, 180, 60, 255]) } else { image::Rgba([0, 0, 0, 0]) });
    // A pixel's difference here and there (edges antialiased differently): passes.
    let mut noisy = base.clone();
    for i in (0..noisy.width()).step_by(7) {
        noisy.put_pixel(i, 40, image::Rgba([255, 190, 70, 255]));
    }
    let (mean, most, _) = difference(&base, &noisy).unwrap();
    assert!(mean <= MEAN && most <= MOST, "{mean} {most}");
    // Redder, or bigger: fails.
    let redder = image::RgbaImage::from_fn(120, 80, |x, y| if base.get_pixel(x, y).0[3] > 0 { image::Rgba([250, 90, 60, 255]) } else { image::Rgba([0, 0, 0, 0]) });
    let (mean, most, _) = difference(&base, &redder).unwrap();
    assert!(mean > MEAN || most > MOST);
    let bigger = image::RgbaImage::from_fn(120, 80, |x, y| if (x as i32 - 60).pow(2) + (y as i32 - 40).pow(2) < 1300 { image::Rgba([250, 180, 60, 255]) } else { image::Rgba([0, 0, 0, 0]) });
    let (mean, most, _) = difference(&base, &bigger).unwrap();
    assert!(mean > MEAN || most > MOST, "{mean} {most}");
    assert!(difference(&base, &image::RgbaImage::new(100, 80)).is_none(), "another shape");
}
