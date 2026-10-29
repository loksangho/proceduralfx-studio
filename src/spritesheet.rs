// Sprite sheets for 2D games: the real-time renderer draws each frame of the playback
// range through the render camera, then the frames are packed into one PNG atlas in a
// grid, with a JSON file beside it describing the grid. Every cell is the same size,
// so Bevy can load it with TextureAtlasLayout::from_grid(frame_size, columns, rows, None, None).
//
// Trimming crops every frame to the same box: the union of what's visible in any frame,
// plus a 1-pixel transparent border so neighbouring cells don't bleed when filtered.
//
// Seamless loops render a few frames past the end of the range and cross-fade them into
// the first frames, so the last frame leads into the first as smoothly as into the next.

use eframe::egui;
use std::path::{Path, PathBuf};

// GPUs commonly allow textures up to this size on a side.
pub const MAX_SIDE: u32 = 16384;

// The Export Sprite Sheet window's choices.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub open: bool,
    // Each frame's render size, before trimming.
    pub width: u32,
    pub height: u32,
    // Render every Nth frame of the playback range.
    pub every: u32,
    // Cells across; 0 picks a near-square sheet.
    pub columns: u32,
    pub trim: bool,
    // Export again to the same files whenever the scene changes.
    pub live: bool,
    // Cross-fade the end into the start over `blend` frames.
    pub seamless: bool,
    pub blend: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { open: false, width: 256, height: 256, every: 1, columns: 0, trim: true, live: false, seamless: false, blend: 8 }
    }
}

impl Settings {
    // How many extra frames a seamless loop of `count` frames renders past the end.
    pub fn extra_frames(&self, count: usize) -> usize {
        if self.seamless && count > 1 { (self.blend as usize).clamp(1, count) } else { 0 }
    }
}

// Where and how a recording's frames are packed when it finishes.
#[derive(Clone, Debug)]
pub struct Output {
    pub path: PathBuf,
    pub columns: u32,
    pub trim: bool,
    pub fps: f32,
    // A live re-export: quieter (no Render Viewer entry).
    pub live: bool,
    // The last `blend` frames were rendered past the end, to cross-fade into the start.
    pub blend: usize,
}

// Cells across for `count` frames of `size`: as set, or what makes the sheet closest to square.
pub fn columns(count: usize, size: [u32; 2], requested: u32) -> u32 {
    let count = count.max(1) as u32;
    if requested > 0 {
        return requested.min(count);
    }
    let ideal = ((count as f32 * size[1] as f32 / size[0].max(1) as f32).sqrt()).round() as u32;
    ideal.clamp(1, count)
}

// The box every frame is cropped to, as [x, y, width, height]: what any frame shows, plus a
// 1-pixel border, inside the frame. The whole frame when nothing shows.
pub fn trim_box(frames: &[egui::ColorImage]) -> [u32; 4] {
    let [w, h] = frames.first().map_or([1, 1], |f| f.size);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for frame in frames {
        for (i, c) in frame.pixels.iter().enumerate() {
            if c.a() > 0 {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    if x1 <= x0 {
        return [0, 0, w as u32, h as u32];
    }
    let (x0, y0, x1, y1) = (x0.saturating_sub(1), y0.saturating_sub(1), (x1 + 1).min(w), (y1 + 1).min(h));
    [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]
}

#[derive(Debug)]
pub struct Sheet {
    pub image: image::RgbaImage,
    pub cell: [u32; 2],
    pub columns: u32,
    pub rows: u32,
    // Where the cell's top-left corner was in the full frame.
    pub offset: [u32; 2],
}

// Packs the frames left to right, top to bottom, `requested` cells across (0: near square).
pub fn pack(frames: &[egui::ColorImage], requested: u32, trim: bool) -> Result<Sheet, String> {
    let first = frames.first().ok_or("no frames were rendered")?;
    let full = [first.size[0] as u32, first.size[1] as u32];
    let [x, y, w, h] = if trim { trim_box(frames) } else { [0, 0, full[0], full[1]] };
    let columns = columns(frames.len(), [w, h], requested);
    let rows = (frames.len() as u32).div_ceil(columns);
    let (sheet_w, sheet_h) = (w as u64 * columns as u64, h as u64 * rows as u64);
    if sheet_w > MAX_SIDE as u64 || sheet_h > MAX_SIDE as u64 {
        return Err(format!(
            "the sheet would be {sheet_w}×{sheet_h} pixels, over the {MAX_SIDE}-pixel limit most GPUs have: use fewer frames, a smaller frame size, or a different column count"
        ));
    }
    let mut image = image::RgbaImage::new(sheet_w as u32, sheet_h as u32);
    for (i, frame) in frames.iter().enumerate() {
        let (cx, cy) = ((i as u32 % columns) * w, (i as u32 / columns) * h);
        for row in 0..h {
            for col in 0..w {
                let c = frame.pixels[((y + row) * full[0] + x + col) as usize];
                image.put_pixel(cx + col, cy + row, image::Rgba(c.to_srgba_unmultiplied()));
            }
        }
    }
    Ok(Sheet { image, cell: [w, h], columns, rows, offset: [x, y] })
}

// The JSON written beside the sheet.
pub fn metadata(sheet: &Sheet, image_name: &str, frame_count: usize, fps: f32, source_size: [u32; 2]) -> serde_json::Value {
    let [w, h] = sheet.cell;
    let frames: Vec<[u32; 4]> = (0..frame_count as u32).map(|i| [(i % sheet.columns) * w, (i / sheet.columns) * h, w, h]).collect();
    serde_json::json!({
        "format": "procedural-fx-sprite-sheet",
        "version": 1,
        "image": image_name,
        "frame_size": [w, h],
        "columns": sheet.columns,
        "rows": sheet.rows,
        "frame_count": frame_count,
        "fps": fps,
        "source_size": source_size,
        "offset": sheet.offset,
        "frames": frames,
    })
}

// Turns `frames`, whose last `blend` were rendered just past the end of the loop, into a
// loop that wraps smoothly: frame i of the first `blend` fades from the frame `blend`
// past the end (which follows the last frame) towards its own, in even steps. Blends in
// linear premultiplied colour, so transparent edges fade without dark fringes.
pub fn seamless(frames: &[egui::ColorImage], blend: usize) -> Vec<egui::ColorImage> {
    let blend = blend.min(frames.len() / 2);
    let n = frames.len() - blend;
    let mut out = frames[..n].to_vec();
    for i in 0..blend {
        let own = (i + 1) as f32 / (blend + 1) as f32;
        let (head, tail) = (&frames[i], &frames[n + i]);
        if head.size != tail.size {
            continue;
        }
        for (px, (a, b)) in out[i].pixels.iter_mut().zip(head.pixels.iter().zip(&tail.pixels)) {
            *px = (egui::Rgba::from(*a) * own + egui::Rgba::from(*b) * (1.0 - own)).into();
        }
    }
    out
}

// Writes the sheet PNG and its JSON (same name, .pfx.json); returns the PNG's path.
pub fn save(frames: &[egui::ColorImage], output: &Output) -> Result<(PathBuf, Sheet), String> {
    let looped;
    let frames = if output.blend > 0 {
        looped = seamless(frames, output.blend);
        &looped[..]
    } else {
        frames
    };
    let sheet = pack(frames, output.columns, output.trim)?;
    let png = output.path.with_extension("png");
    if let Some(dir) = png.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Written beside the file and renamed over it, so a game watching the files never
    // reads half of one. The image goes first: the JSON's reload reloads it too.
    let partial = png.with_extension("png.partial");
    sheet.image.save_with_format(&partial, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &png).map_err(|e| e.to_string())?;
    let name = png.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let size = frames.first().map_or([0, 0], |f| [f.size[0] as u32, f.size[1] as u32]);
    let json = serde_json::to_string_pretty(&metadata(&sheet, &name, frames.len(), output.fps, size)).map_err(|e| e.to_string())?;
    let partial = png.with_extension("pfx.json.partial");
    std::fs::write(&partial, json + "\n").map_err(|e| e.to_string())?;
    std::fs::rename(&partial, json_path(&png)).map_err(|e| e.to_string())?;
    Ok((png, sheet))
}

// name.pfx.json beside name.png: the extension bevy_procedural_fx loads.
pub fn json_path(png: &Path) -> PathBuf {
    png.with_extension("pfx.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    // A w×h frame, transparent except for one opaque pixel at (x, y).
    fn frame(w: usize, h: usize, x: usize, y: usize) -> egui::ColorImage {
        let mut image = egui::ColorImage::new([w, h], vec![egui::Color32::TRANSPARENT; w * h]);
        image.pixels[y * w + x] = egui::Color32::from_rgb(255, 128, 0);
        image
    }

    #[test]
    fn columns_make_a_near_square_sheet() {
        assert_eq!(columns(16, [64, 64], 0), 4);
        assert_eq!(columns(10, [64, 64], 0), 3);
        assert_eq!(columns(8, [128, 32], 0), 1);
        assert_eq!(columns(1, [64, 64], 0), 1);
        assert_eq!(columns(5, [64, 64], 8), 5);
        assert_eq!(columns(5, [64, 64], 2), 2);
    }

    #[test]
    fn trimming_keeps_what_any_frame_shows_plus_a_border() {
        let frames = [frame(20, 10, 5, 2), frame(20, 10, 12, 6)];
        assert_eq!(trim_box(&frames), [4, 1, 10, 7]);
        // Touching the edge, the border stops at the frame.
        assert_eq!(trim_box(&[frame(20, 10, 0, 9)]), [0, 8, 2, 2]);
        // Nothing visible: the whole frame.
        let empty = egui::ColorImage::new([20, 10], vec![egui::Color32::TRANSPARENT; 200]);
        assert_eq!(trim_box(&[empty]), [0, 0, 20, 10]);
    }

    #[test]
    fn frames_pack_in_reading_order() {
        let frames: Vec<_> = (0..5).map(|i| frame(20, 10, 5 + i, 2)).collect();
        let sheet = pack(&frames, 2, true).unwrap();
        // Trimmed to x 4..11, y 1..4.
        assert_eq!((sheet.cell, sheet.columns, sheet.rows, sheet.offset), ([7, 3], 2, 3, [4, 1]));
        assert_eq!(sheet.image.dimensions(), (14, 9));
        // Frame 3 (index 2) is in row 1, column 0; its pixel was at x 7 → 7 - 4 = 3 in the cell.
        assert_eq!(sheet.image.get_pixel(3, 3 + 1).0, [255, 128, 0, 255]);
        assert_eq!(sheet.image.get_pixel(2, 3 + 1).0[3], 0);
        let meta = metadata(&sheet, "fx.png", 5, 24.0, [20, 10]);
        assert_eq!(meta["frames"][4], serde_json::json!([0, 6, 7, 3]));
        assert_eq!(meta["frame_size"], serde_json::json!([7, 3]));
        assert_eq!(pack(&frames, 2, false).unwrap().image.dimensions(), (40, 30));
    }

    #[test]
    fn seamless_loops_fade_the_frames_past_the_end_into_the_start() {
        // Six loop frames plus two rendered past the end, each one flat grey: 10, 20, … 80.
        let flat = |v: u8| egui::ColorImage::new([2, 2], vec![egui::Color32::from_gray(v); 4]);
        let frames: Vec<_> = (1..=8).map(|i| flat(i * 10)).collect();
        let looped = seamless(&frames, 2);
        assert_eq!(looped.len(), 6, "the extra frames don't become frames of their own");
        let linear = |c: egui::Color32| egui::Rgba::from(c).r();
        let mix = |own: f32, a: u8, b: u8| own * linear(egui::Color32::from_gray(a)) + (1.0 - own) * linear(egui::Color32::from_gray(b));
        // Frame 0 is mostly frame 6 (the one after the last), frame 1 mostly its own.
        assert!((linear(looped[0].pixels[0]) - mix(1.0 / 3.0, 10, 70)).abs() < 0.01);
        assert!((linear(looped[1].pixels[0]) - mix(2.0 / 3.0, 20, 80)).abs() < 0.01);
        // Past the blend, frames are untouched.
        assert_eq!(&looped[2..], &frames[2..6]);
        // Fully transparent frames stay fully transparent.
        let clear = vec![egui::ColorImage::new([2, 2], vec![egui::Color32::TRANSPARENT; 4]); 4];
        assert!(seamless(&clear, 2).iter().all(|f| f.pixels.iter().all(|p| *p == egui::Color32::TRANSPARENT)));
    }

    #[test]
    fn seamless_loops_need_extra_frames_only_when_asked() {
        let mut s = Settings::default();
        assert_eq!(s.extra_frames(24), 0);
        s.seamless = true;
        assert_eq!((s.extra_frames(24), s.extra_frames(5), s.extra_frames(1)), (8, 5, 0));
    }

    #[test]
    fn oversized_sheets_are_refused() {
        let frames: Vec<_> = (0..3).map(|_| frame(10000, 1, 0, 0)).collect();
        assert!(pack(&frames, 3, false).unwrap_err().contains("16384"));
    }

    #[test]
    fn saving_writes_the_png_and_its_json() {
        let dir = std::env::temp_dir().join(format!("pfx_sheet_{}", std::process::id()));
        let frames: Vec<_> = (0..4).map(|i| frame(8, 8, i, i)).collect();
        let output = Output { path: dir.join("spark.png"), columns: 0, trim: false, fps: 12.0, live: false, blend: 0 };
        let (png, _) = save(&frames, &output).unwrap();
        assert_eq!(image::open(&png).unwrap().to_rgba8().dimensions(), (16, 16));
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("spark.pfx.json")).unwrap()).unwrap();
        assert_eq!((meta["image"].as_str(), meta["columns"].as_u64(), meta["fps"].as_f64()), (Some("spark.png"), Some(2), Some(12.0)));
        // Only the two files are left: nothing half-written.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
