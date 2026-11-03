// Flipbook, as in Houdini: plays the timeline range, captures the viewport at
// every frame (once it has cooked), then plays the frames back in a viewer that
// can save them as PNG frames or, with FFmpeg installed, a movie.
//
// On the GPU viewport each frame is rendered off screen (see Capture); otherwise
// it's cut out of a window screenshot. Real-time renders use the same recording,
// through the render camera, and save the frames instead of showing them.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::time::Instant;

// Frames are scaled to at most this many pixels wide, to keep memory in check.
pub const MAX_WIDTH: u32 = 960;

pub struct Recording {
    pub times: Vec<f32>,
    pub next: usize,
    pub frames: Vec<egui::ColorImage>,
    // A screenshot has been asked for and hasn't arrived yet.
    pub waiting: bool,
    // The time to go back to when done.
    pub resume_time: f32,
    // Rendered by the GPU viewport, not screenshotted.
    pub capture: Option<Capture>,
}

// How the GPU viewport renders each frame of a recording.
pub struct Capture {
    // None: the viewport's own view and size (at most MAX_WIDTH wide), over its
    // background. Some: the render camera at this size, transparent where empty.
    pub render: Option<[u32; 2]>,
    // Real-time renders: where the frames are saved (name.0001.png…).
    pub save_to: Option<PathBuf>,
    // Sprite sheets: packed into one image when the recording finishes.
    pub sheet: Option<crate::spritesheet::Output>,
    // A sprite-sheet preview: the frames as they'll export (the last N faded into the start
    // for a seamless loop), shown looping in the viewer instead of saved.
    pub preview: Option<usize>,
}

impl Recording {
    pub fn new(times: Vec<f32>, resume_time: f32, capture: Option<Capture>) -> Self {
        Recording { times, next: 0, frames: Vec::new(), waiting: false, resume_time, capture }
    }
}

// The viewport's part of a window screenshot, scaled to at most MAX_WIDTH.
pub fn crop(shot: &egui::ColorImage, rect: egui::Rect, pixels_per_point: f32) -> Option<egui::ColorImage> {
    let [w, h] = shot.size;
    let x0 = ((rect.min.x * pixels_per_point).round().max(0.0) as usize).min(w);
    let y0 = ((rect.min.y * pixels_per_point).round().max(0.0) as usize).min(h);
    let x1 = ((rect.max.x * pixels_per_point).round().max(0.0) as usize).min(w);
    let y1 = ((rect.max.y * pixels_per_point).round().max(0.0) as usize).min(h);
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        return None;
    }
    let mut image = image::RgbaImage::new((x1 - x0) as u32, (y1 - y0) as u32);
    for (y, row) in (y0..y1).enumerate() {
        for (x, col) in (x0..x1).enumerate() {
            let c = shot.pixels[row * w + col];
            image.put_pixel(x as u32, y as u32, image::Rgba([c.r(), c.g(), c.b(), 255]));
        }
    }
    if image.width() > MAX_WIDTH {
        let height = (image.height() as f32 * MAX_WIDTH as f32 / image.width() as f32).round().max(1.0) as u32;
        image = image::imageops::resize(&image, MAX_WIDTH, height, image::imageops::FilterType::Triangle);
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
}

// Straight (not premultiplied) alpha, as PNG files have it.
fn to_rgba(frame: &egui::ColorImage) -> image::RgbaImage {
    let [w, h] = frame.size;
    image::RgbaImage::from_raw(w as u32, h as u32, frame.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect()).expect("sized from the frame")
}

// Writes frames as name.0001.png, name.0002.png… in `dir`; returns the first file.
pub fn save_pngs(frames: &[egui::ColorImage], dir: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (i, frame) in frames.iter().enumerate() {
        to_rgba(frame).save(dir.join(format!("{name}.{:04}.png", i + 1))).map_err(|e| e.to_string())?;
    }
    Ok(dir.join(format!("{name}.0001.png")))
}

// FFmpeg on PATH, or where Homebrew and Linux distributions put it (apps started
// from the Finder don't get the shell's PATH).
pub fn find_ffmpeg() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).collect()).unwrap_or_default();
    candidates.extend(["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"].map(PathBuf::from));
    candidates.into_iter().find(|p| p.is_file())
}

// What animations (GIFs and movies) are drawn over: neither can show soft transparency,
// and smoke fringes look ragged over anything but a plain dark background.
pub const BACKGROUND: [u8; 3] = [30, 30, 34];

// A frame over the animation background, opaque.
fn over_background(frame: &egui::ColorImage) -> image::RgbaImage {
    let [w, h] = frame.size;
    let pixels = frame.pixels.iter().flat_map(|c| {
        // The frame's colours are premultiplied: background × (1 - alpha) + colour.
        let a = c.a() as f32 / 255.0;
        let [r, g, b] = [c.r(), c.g(), c.b()].map(|v| v as f32);
        let mix = |v: f32, bg: u8| (v + bg as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
        [mix(r, BACKGROUND[0]), mix(g, BACKGROUND[1]), mix(b, BACKGROUND[2]), 255]
    });
    image::RgbaImage::from_raw(w as u32, h as u32, pixels.collect()).expect("sized from the frame")
}

// A looping GIF of the frames at `fps`, over the animation background. Needs nothing
// installed, unlike movies.
pub fn save_gif(frames: &[egui::ColorImage], path: &Path, fps: f32) -> Result<(), String> {
    use image::codecs::gif::{GifEncoder, Repeat};
    if frames.is_empty() {
        return Err("there are no frames".into());
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let partial = path.with_extension("gif.partial");
    let file = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
    // Speed 10 of 30: good colours without taking long.
    let mut encoder = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 10);
    encoder.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;
    let delay = image::Delay::from_numer_denom_ms(1000, fps.clamp(1.0, 100.0).round() as u32);
    for frame in frames {
        encoder.encode_frame(image::Frame::from_parts(over_background(frame), 0, 0, delay)).map_err(|e| e.to_string())?;
    }
    drop(encoder);
    std::fs::rename(&partial, path).map_err(|e| e.to_string())
}

// An H.264 movie of the frames at `fps`, over the animation background.
pub fn save_movie(frames: &[egui::ColorImage], path: &Path, fps: f32) -> Result<(), String> {
    let ffmpeg = find_ffmpeg().ok_or("FFmpeg isn't installed (brew install ffmpeg), so only PNG frames can be saved")?;
    let dir = std::env::temp_dir().join(format!("pfx_flipbook_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (i, frame) in frames.iter().enumerate() {
        over_background(frame).save(dir.join(format!("frame.{:04}.png", i + 1))).map_err(|e| e.to_string())?;
    }
    let output = std::process::Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-framerate", &fps.to_string(), "-i"])
        .arg(dir.join("frame.%04d.png"))
        // Even sizes, as H.264 in yuv420p needs.
        .args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub struct Viewer {
    frames: Vec<egui::ColorImage>,
    times: Vec<f32>,
    textures: Vec<Option<egui::TextureHandle>>,
    current: usize,
    playing: bool,
    looping: bool,
    fps: f32,
    last_step: Option<Instant>,
    pub open: bool,
    status: String,
}

impl Viewer {
    pub fn new(frames: Vec<egui::ColorImage>, times: Vec<f32>, fps: f32) -> Self {
        let textures = (0..frames.len()).map(|_| None).collect();
        Viewer { frames, times, textures, current: 0, playing: true, looping: true, fps, last_step: None, open: true, status: String::new() }
    }

    // The frames, for saving (the animation command) and tests.
    pub fn frames(&self) -> &[egui::ColorImage] {
        &self.frames
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if self.frames.is_empty() {
            self.open = false;
            return;
        }
        // Playback at the chosen rate, independent of the app's redraw rate.
        if self.playing {
            let step = 1.0 / self.fps.max(1.0);
            let now = Instant::now();
            match self.last_step {
                Some(last) if now.duration_since(last).as_secs_f32() < step => {}
                _ => {
                    if self.last_step.is_some() {
                        if self.current + 1 < self.frames.len() {
                            self.current += 1;
                        } else if self.looping {
                            self.current = 0;
                        } else {
                            self.playing = false;
                        }
                    }
                    self.last_step = Some(now);
                }
            }
            ctx.request_repaint();
        }
        let current = self.current;
        let texture = self.textures[current]
            .get_or_insert_with(|| ctx.load_texture(format!("flipbook_{current}"), self.frames[current].clone(), egui::TextureOptions::LINEAR))
            .clone();
        // A window of its own, as big as you like, beside the app; where the platform can't
        // open one, a panel inside the app's window instead.
        let [w, h] = self.frames[0].size;
        let size = [(w as f32 * 0.8 + 20.0).max(360.0), h as f32 * 0.8 + 100.0];
        let mut open = self.open;
        crate::own_window(ctx, "flipbook_viewer", "Flipbook", size, [320.0, 240.0], &mut open, false, |ui| {
            self.contents(ui, &texture);
            // Its own window needs its own redraws to play.
            if self.playing {
                ui.ctx().request_repaint();
            }
        });
        self.open = open;
    }

    // The controls, the frame (fitted to the space, keeping its proportions) and saving.
    fn contents(&mut self, ui: &mut egui::Ui, texture: &egui::TextureHandle) {
        {
            ui.horizontal(|ui| {
                if ui.button(if self.playing { "⏸ Pause" } else { "▶ Play" }).clicked() {
                    self.playing = !self.playing;
                    if self.playing && self.current + 1 == self.frames.len() {
                        self.current = 0;
                    }
                    self.last_step = None;
                }
                if ui.button("⏮").on_hover_text("First frame").clicked() {
                    self.current = 0;
                }
                if ui.button("◀").on_hover_text("Previous frame").clicked() {
                    self.playing = false;
                    self.current = self.current.saturating_sub(1);
                }
                if ui.button("▶|").on_hover_text("Next frame").clicked() {
                    self.playing = false;
                    self.current = (self.current + 1).min(self.frames.len() - 1);
                }
                ui.checkbox(&mut self.looping, "Loop");
                ui.add(egui::DragValue::new(&mut self.fps).range(1.0..=120.0).speed(0.5).suffix(" fps"));
            });
            let mut index = self.current;
            let last = self.frames.len() - 1;
            let time = self.times.get(index).copied().unwrap_or(0.0);
            let label = format!("frame {} · {:.2} s", index + 1, time);
            if ui.add(egui::Slider::new(&mut index, 0..=last).text(label)).changed() {
                self.current = index;
                self.playing = false;
            }
            // The frame, fitted to the window and keeping its proportions.
            let [w, h] = self.frames[0].size;
            let available = ui.available_size() - egui::vec2(0.0, 32.0);
            let scale = (available.x / w as f32).min(available.y / h as f32).max(0.05);
            ui.image((texture.id(), egui::vec2(w as f32 * scale, h as f32 * scale)));
            ui.horizontal(|ui| {
                if ui.button("Save PNG frames…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Folder for the flipbook frames").pick_folder() {
                        self.status = match save_pngs(&self.frames, &dir, "flipbook") {
                            Ok(first) => format!("Saved {} frames ({} …)", self.frames.len(), first.display()),
                            Err(e) => format!("Couldn't save: {e}"),
                        };
                    }
                }
                if ui.button("Save GIF…").on_hover_text("A looping GIF over a dark background, for posting online").clicked() {
                    if let Some(path) = rfd::FileDialog::new().set_file_name("flipbook.gif").add_filter("GIF", &["gif"]).save_file() {
                        self.status = match save_gif(&self.frames, &path, self.fps) {
                            Ok(()) => format!("Saved {}", path.display()),
                            Err(e) => format!("Couldn't save the GIF: {e}"),
                        };
                    }
                }
                let ffmpeg = find_ffmpeg().is_some();
                let movie = ui.add_enabled(ffmpeg, egui::Button::new("Save movie…")).on_disabled_hover_text("Install FFmpeg (brew install ffmpeg) to save an .mp4");
                if movie.clicked() {
                    if let Some(path) = rfd::FileDialog::new().set_file_name("flipbook.mp4").add_filter("Movie", &["mp4"]).save_file() {
                        self.status = match save_movie(&self.frames, &path, self.fps) {
                            Ok(()) => format!("Saved {}", path.display()),
                            Err(e) => format!("Couldn't save the movie: {e}"),
                        };
                    }
                }
                ui.weak(&self.status);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gifs_loop_every_frame_over_the_background() {
        use image::AnimationDecoder;
        let dir = std::env::temp_dir().join(format!("pfx_gif_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Three frames: clear, half-covered by white, solid red.
        let clear = egui::ColorImage::filled([8, 4], egui::Color32::TRANSPARENT);
        let half = egui::ColorImage::filled([8, 4], egui::Color32::from_white_alpha(128));
        let red = egui::ColorImage::filled([8, 4], egui::Color32::RED);
        let path = dir.join("fx.gif");
        save_gif(&[clear, half, red], &path, 24.0).unwrap();
        let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap());
        let frames = image::codecs::gif::GifDecoder::new(file).unwrap().into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 3);
        let pixel = |f: usize| frames[f].buffer().get_pixel(3, 2).0;
        let near = |a: [u8; 4], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 8);
        assert!(near(pixel(0), BACKGROUND), "clear shows the background: {:?}", pixel(0));
        assert!(near(pixel(1), [142, 142, 144]), "half white over it: {:?}", pixel(1));
        assert!(near(pixel(2), [255, 0, 0]), "{:?}", pixel(2));
        let (numer, denom) = frames[0].delay().numer_denom_ms();
        // GIFs count in hundredths of a second: 24 fps is 40 ms a frame.
        assert!((numer as f32 / denom as f32 - 1000.0 / 24.0).abs() < 10.0);
        assert!(!path.with_extension("gif.partial").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crops_scales_and_saves() {
        // A 2000×1000 "window" with the viewport at points (100, 50)-(1100, 550) at 2 pixels per point.
        let mut shot = egui::ColorImage::filled([2000, 1000], egui::Color32::BLACK);
        for y in 100..1100usize.min(1000) {
            for x in 200..2200usize.min(2000) {
                shot.pixels[y * 2000 + x] = egui::Color32::from_rgb(200, 100, 50);
            }
        }
        let frame = crop(&shot, egui::Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(1100.0, 550.0)), 2.0).unwrap();
        assert_eq!(frame.size[0], MAX_WIDTH as usize, "scaled down to the maximum width");
        assert_eq!(frame.size[1], (900.0 * MAX_WIDTH as f32 / 1800.0).round() as usize);
        assert_eq!(frame.pixels[frame.size[0] * 10 + 10], egui::Color32::from_rgb(200, 100, 50));
        assert!(crop(&shot, egui::Rect::from_min_max(egui::pos2(5000.0, 0.0), egui::pos2(6000.0, 10.0)), 2.0).is_none());
        let dir = std::env::temp_dir().join("pfx_flipbook_test");
        let first = save_pngs(&[frame.clone(), frame], &dir, "shot").unwrap();
        assert!(first.ends_with("shot.0001.png") && dir.join("shot.0002.png").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }
}
