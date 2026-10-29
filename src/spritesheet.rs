// Sprite sheets for 2D games: the real-time renderer draws each frame of the playback
// range through the render camera, then the frames are packed into one PNG atlas in a
// grid, with a JSON file beside it describing the grid. Every cell is the same size,
// so Bevy can load it with TextureAtlasLayout::from_grid(frame_size, columns, rows, None, None).
//
// Trimming crops every frame to the same box: the union of what's visible in any frame,
// plus a 1-pixel transparent border so neighbouring cells don't bleed when filtered.

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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { open: false, width: 256, height: 256, every: 1, columns: 0, trim: true }
    }
}

// Where and how a recording's frames are packed when it finishes.
#[derive(Clone, Debug)]
pub struct Output {
    pub path: PathBuf,
    pub columns: u32,
    pub trim: bool,
    pub fps: f32,
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

// Writes the sheet PNG and its JSON (same name, .pfx.json); returns the PNG's path.
pub fn save(frames: &[egui::ColorImage], output: &Output) -> Result<(PathBuf, Sheet), String> {
    let sheet = pack(frames, output.columns, output.trim)?;
    let png = output.path.with_extension("png");
    if let Some(dir) = png.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    sheet.image.save(&png).map_err(|e| e.to_string())?;
    let name = png.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let size = frames.first().map_or([0, 0], |f| [f.size[0] as u32, f.size[1] as u32]);
    let json = serde_json::to_string_pretty(&metadata(&sheet, &name, frames.len(), output.fps, size)).map_err(|e| e.to_string())?;
    std::fs::write(json_path(&png), json + "\n").map_err(|e| e.to_string())?;
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
    fn oversized_sheets_are_refused() {
        let frames: Vec<_> = (0..3).map(|_| frame(10000, 1, 0, 0)).collect();
        assert!(pack(&frames, 3, false).unwrap_err().contains("16384"));
    }

    #[test]
    fn saving_writes_the_png_and_its_json() {
        let dir = std::env::temp_dir().join(format!("pfx_sheet_{}", std::process::id()));
        let frames: Vec<_> = (0..4).map(|i| frame(8, 8, i, i)).collect();
        let output = Output { path: dir.join("spark.png"), columns: 0, trim: false, fps: 12.0 };
        let (png, _) = save(&frames, &output).unwrap();
        assert_eq!(image::open(&png).unwrap().to_rgba8().dimensions(), (16, 16));
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("spark.pfx.json")).unwrap()).unwrap();
        assert_eq!((meta["image"].as_str(), meta["columns"].as_u64(), meta["fps"].as_f64()), (Some("spark.png"), Some(2), Some(12.0)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
