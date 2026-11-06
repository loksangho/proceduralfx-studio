// Render viewer: shows finished MoonRay images (EXR, PNG, ...) with exposure,
// channel and zoom controls, a pixel inspector and a history of this session's renders.
// A rendered sequence is one entry in the history: it plays as an animation, and saves
// as a movie or a GIF, as displayed (exposure, gamma and channel applied).

use eframe::egui;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

// A render in progress, filled in bucket by bucket by the render worker.
pub struct RenderProgress {
    pub width: usize,
    pub height: usize,
    // Linear RGBA, top row first.
    pub pixels: Vec<[f32; 4]>,
    // Buckets being rendered now: (x0, y0, x1, y1), exclusive, top-down pixels.
    pub active: Vec<[usize; 4]>,
    pub done: usize,
    pub total: usize,
    pub version: u64,
}

impl RenderProgress {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, pixels: vec![[0.0; 4]; width * height], active: Vec::new(), done: 0, total: 0, version: 0 }
    }
}

pub type SharedProgress = Arc<Mutex<RenderProgress>>;

// Larger images are refused rather than risking a texture the GPU cannot hold.
const MAX_PREVIEW_SIDE: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Channel {
    Rgb,
    R,
    G,
    B,
    A,
    Luma,
}

// How linear pixels are shown: exposure, transfer curve and channel.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Display {
    exposure: f32,
    gamma: f32,
    srgb: bool,
    channel: Channel,
}

impl Display {
    fn value(&self, v: f32) -> f32 {
        let v = v.max(0.0);
        let encoded = if self.srgb { linear_to_srgb(v) } else { v.powf(1.0 / self.gamma.max(0.1)) };
        encoded.clamp(0.0, 1.0)
    }

    // Display-encoded RGB of one pixel.
    fn rgb(&self, p: [f32; 4]) -> [f32; 3] {
        let scale = 2f32.powf(self.exposure);
        let luma = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let linear = match self.channel {
            Channel::Rgb => [p[0] * scale, p[1] * scale, p[2] * scale],
            Channel::R => [p[0] * scale; 3],
            Channel::G => [p[1] * scale; 3],
            Channel::B => [p[2] * scale; 3],
            Channel::Luma => [luma * scale; 3],
            // Alpha is coverage, not light: no exposure and no transfer curve.
            Channel::A => return [p[3].clamp(0.0, 1.0); 3],
        };
        linear.map(|v| self.value(v))
    }

    fn bytes(&self, p: [f32; 4]) -> [u8; 3] {
        self.rgb(p).map(|v| (v * 255.0 + 0.5) as u8)
    }

    // The image as shown, opaque.
    fn image(&self, img: &LoadedImage) -> egui::ColorImage {
        let pixels = img.pixels.iter().map(|p| {
            let [r, g, b] = self.bytes(*p);
            egui::Color32::from_rgb(r, g, b)
        });
        egui::ColorImage { size: [img.width, img.height], source_size: egui::vec2(img.width as f32, img.height as f32), pixels: pixels.collect() }
    }
}

// One item in the history: a single image, or a sequence's frames played as one animation.
#[derive(Debug, Clone, PartialEq)]
enum Entry {
    Image(PathBuf),
    Sequence { frames: Vec<PathBuf>, fps: f32 },
}

impl Entry {
    fn first(&self) -> &Path {
        match self {
            Entry::Image(path) => path,
            Entry::Sequence { frames, .. } => &frames[0],
        }
    }

    fn label(&self) -> String {
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match self {
            Entry::Image(path) => name(path),
            Entry::Sequence { frames, .. } => format!("{} ({} frames)", sequence_name(&frames[0]), frames.len()),
        }
    }
}

// A sequence's name from its first frame: "render.0001.exr" is "render.####.exr".
fn sequence_name(first: &Path) -> String {
    let name = first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_owned(), format!(".{ext}")),
        None => (name.clone(), String::new()),
    };
    let digits = stem.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return name;
    }
    format!("{}{}{ext}", &stem[..stem.len() - digits], "#".repeat(digits))
}

// Frames kept ready to play, at most this many bytes of them.
const PLAYBACK_CACHE_BYTES: usize = 1536 << 20;

// The frames of a sequence as displayed, loaded from their files.
fn displayed_frames(frames: &[PathBuf], display: Display) -> Result<Vec<egui::ColorImage>, String> {
    use rayon::prelude::*;
    let images: Vec<egui::ColorImage> = frames.par_iter().map(|path| load_image(path).map(|img| display.image(&img))).collect::<Result<_, _>>()?;
    if images.windows(2).any(|w| w[0].size != w[1].size) {
        return Err("the frames aren't all the same size".into());
    }
    Ok(images)
}

// Writes a sequence's frames, as displayed, as a GIF or a movie.
fn save_frames(frames: &[PathBuf], display: Display, fps: f32, target: &Path, gif: bool) -> Result<String, String> {
    let images = displayed_frames(frames, display)?;
    if gif {
        crate::flipbook::save_gif(&images, target, fps)?;
    } else {
        crate::flipbook::save_movie(&images, target, fps)?;
    }
    Ok(format!("Saved a {}-frame {} to {}", images.len(), if gif { "GIF" } else { "movie" }, target.display()))
}

struct LoadedImage {
    path: PathBuf,
    width: usize,
    height: usize,
    // Linear-light RGBA; 8-bit sources are decoded from sRGB on load.
    pixels: Vec<[f32; 4]>,
    hdr: bool,
    file_bytes: u64,
}

pub struct RenderViewer {
    pub open: bool,
    history: Vec<Entry>,
    current: Option<usize>,
    image: Option<LoadedImage>,
    texture: Option<egui::TextureHandle>,
    texture_dirty: bool,
    error: Option<String>,
    exposure: f32,
    gamma: f32,
    srgb: bool,
    channel: Channel,
    // Screen points per image pixel.
    zoom: f32,
    pan: egui::Vec2,
    fit: bool,
    status: String,
    // Render being watched, the version last shown, and its buckets in flight.
    progress: Option<SharedProgress>,
    progress_version: u64,
    progress_overlay: Vec<[usize; 4]>,
    progress_counts: (usize, usize),
    // The frame of a sequence shown, whether it plays, and when it last moved on.
    frame: usize,
    playing: bool,
    last_step: Option<std::time::Instant>,
    // Displayed frames of the sequence shown, kept to play smoothly, and the display
    // settings they were made with.
    played: Vec<Option<egui::ColorImage>>,
    played_with: Option<Display>,
    // A movie or GIF being written.
    saving: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

impl Default for RenderViewer {
    fn default() -> Self {
        Self {
            open: false,
            history: Vec::new(),
            current: None,
            image: None,
            texture: None,
            texture_dirty: false,
            error: None,
            exposure: 0.0,
            gamma: 2.2,
            srgb: true,
            channel: Channel::Rgb,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fit: true,
            status: String::new(),
            progress: None,
            progress_version: u64::MAX,
            progress_overlay: Vec::new(),
            progress_counts: (0, 0),
            frame: 0,
            playing: false,
            last_step: None,
            played: Vec::new(),
            played_with: None,
            saving: None,
        }
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn load_image(path: &Path) -> Result<LoadedImage, String> {
    let img = image::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let hdr = matches!(
        img,
        image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_)
    );
    let rgba = img.to_rgba32f();
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    if width > MAX_PREVIEW_SIDE || height > MAX_PREVIEW_SIDE {
        return Err(format!(
            "{width}×{height} is too large to preview (limit {MAX_PREVIEW_SIDE} px per side)"
        ));
    }
    let pixels = rgba
        .pixels()
        .map(|p| {
            if hdr {
                p.0
            } else {
                [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3]]
            }
        })
        .collect();
    let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(LoadedImage { path: path.to_path_buf(), width, height, pixels, hdr, file_bytes })
}

// Opens the platform file browser with `path` selected (or its folder, where selection is unsupported).
pub fn reveal_in_file_browser(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = if path.is_dir() {
        Command::new("open").arg(path).spawn()
    } else {
        Command::new("open").arg("-R").arg(path).spawn()
    };
    #[cfg(target_os = "windows")]
    let result = if path.is_dir() {
        Command::new("explorer").arg(path).spawn()
    } else {
        Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open")
        .arg(if path.is_dir() { path } else { path.parent().unwrap_or(path) })
        .spawn();
    result.map(|_| ()).map_err(|e| format!("Could not open file browser: {e}"))
}

fn human_bytes(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

impl RenderViewer {
    // Shows a render as its buckets arrive.
    pub fn watch(&mut self, progress: SharedProgress) {
        self.progress = Some(progress);
        self.progress_version = u64::MAX;
        self.open = true;
    }

    // Copies new buckets into the displayed image.
    fn poll_progress(&mut self) {
        let Some(progress) = self.progress.clone() else { return };
        let Ok(state) = progress.lock() else { return };
        self.progress_overlay = state.active.clone();
        self.progress_counts = (state.done, state.total);
        if state.version == self.progress_version {
            return;
        }
        self.progress_version = state.version;
        let same_size = self.image.as_ref().is_some_and(|img| img.width == state.width && img.height == state.height);
        if !same_size {
            self.fit = true;
        }
        self.image = Some(LoadedImage {
            path: PathBuf::from("rendering…"),
            width: state.width,
            height: state.height,
            pixels: state.pixels.clone(),
            hdr: true,
            file_bytes: 0,
        });
        self.error = None;
        self.texture_dirty = true;
    }

    // Adds a finished render to the history; `focus` also opens the viewer on it.
    pub fn add(&mut self, path: PathBuf, focus: bool) {
        self.push(Entry::Image(path), focus);
    }

    // Adds a frame of a sequence being rendered: the first starts a new entry, later ones
    // join it. Shown as it arrives when `focus`.
    pub fn add_frame(&mut self, path: PathBuf, first: bool, fps: f32, focus: bool) {
        let last = self.history.len().checked_sub(1);
        let joins = !first && matches!(self.history.last(), Some(Entry::Sequence { .. }));
        if !joins {
            self.push(Entry::Sequence { frames: vec![path], fps }, focus);
            return;
        }
        self.progress = None;
        self.progress_overlay.clear();
        if let Some(Entry::Sequence { frames, .. }) = self.history.last_mut() {
            frames.retain(|p| p != &path);
            frames.push(path);
        }
        if self.current == last {
            self.open |= focus;
            self.played.push(None);
            // Playing, it carries on; otherwise the newest frame is shown.
            if !self.playing {
                self.show_frame(usize::MAX);
            }
        } else if let (true, Some(last)) = (focus, last) {
            self.open = true;
            self.select(last);
            self.show_frame(usize::MAX);
        }
    }

    // A sequence of image files, opened together.
    pub fn add_sequence(&mut self, mut frames: Vec<PathBuf>, fps: f32) {
        frames.sort();
        match frames.len() {
            0 => {}
            1 => self.add(frames.remove(0), true),
            _ => self.push(Entry::Sequence { frames, fps }, true),
        }
    }

    fn push(&mut self, entry: Entry, focus: bool) {
        self.progress = None;
        self.progress_overlay.clear();
        let shown = self.current.and_then(|i| self.history.get(i)).cloned();
        let first = entry.first().to_path_buf();
        self.history.retain(|e| e.first() != first);
        self.history.push(entry);
        let index = self.history.len() - 1;
        if focus {
            self.open = true;
            self.select(index);
        } else {
            // The retain above may have shifted the entry being shown.
            self.current = shown.and_then(|s| self.history.iter().position(|e| *e == s));
        }
    }

    // Opens the viewer, loading the newest render if nothing is shown yet.
    pub fn show(&mut self) {
        self.open = true;
        if self.image.is_none() && !self.history.is_empty() {
            self.select(self.history.len() - 1);
        }
    }

    fn select(&mut self, index: usize) {
        let Some(entry) = self.history.get(index).cloned() else {
            return;
        };
        self.current = Some(index);
        self.playing = false;
        self.played.clear();
        self.played_with = None;
        match entry {
            Entry::Image(path) => self.reload(&path),
            Entry::Sequence { frames, .. } => {
                self.played = vec![None; frames.len()];
                self.played_with = Some(self.display());
                self.frame = 0;
                self.reload(&frames[0]);
            }
        }
    }

    // The sequence shown: its frames and frame rate.
    fn sequence(&self) -> Option<(&[PathBuf], f32)> {
        match self.current.and_then(|i| self.history.get(i))? {
            Entry::Sequence { frames, fps } => Some((frames, *fps)),
            Entry::Image(_) => None,
        }
    }

    // Shows a frame of the sequence (usize::MAX: the last). While playing, from the frames
    // kept ready; otherwise loaded in full, for the pixel inspector.
    fn show_frame(&mut self, frame: usize) {
        let Some((frames, _)) = self.sequence() else { return };
        let frame = frame.min(frames.len() - 1);
        let path = frames[frame].clone();
        self.frame = frame;
        let display = self.display();
        if self.played_with != Some(display) {
            self.played.iter_mut().for_each(|f| *f = None);
            self.played_with = Some(display);
        }
        if self.playing {
            if let (Some(Some(ready)), Some(texture)) = (self.played.get(frame), self.texture.as_mut()) {
                texture.set(ready.clone(), Self::texture_options());
                return;
            }
        }
        self.reload(&path);
    }

    fn reload(&mut self, path: &Path) {
        match load_image(path) {
            Ok(img) => {
                self.image = Some(img);
                self.error = None;
                self.texture_dirty = true;
            }
            Err(error) => {
                self.image = None;
                self.texture = None;
                self.error = Some(error);
            }
        }
    }

    fn display(&self) -> Display {
        Display { exposure: self.exposure, gamma: self.gamma, srgb: self.srgb, channel: self.channel }
    }

    fn texture_options() -> egui::TextureOptions {
        egui::TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Linear, ..egui::TextureOptions::LINEAR }
    }

    // Moves a playing sequence on, at its frame rate, looping.
    fn play(&mut self, ctx: &egui::Context) {
        let Some((frames, fps)) = self.sequence() else {
            self.playing = false;
            return;
        };
        if !self.playing || frames.len() < 2 {
            self.last_step = None;
            return;
        }
        let (count, step) = (frames.len(), std::time::Duration::from_secs_f32(1.0 / fps.clamp(1.0, 120.0)));
        let now = std::time::Instant::now();
        let last = *self.last_step.get_or_insert(now);
        if now.duration_since(last) >= step {
            // Late frames are skipped rather than slowing the animation down.
            let behind = (now.duration_since(last).as_secs_f32() / step.as_secs_f32()) as usize;
            self.last_step = Some(last + step * behind as u32);
            self.show_frame((self.frame + behind.max(1)) % count);
        }
        ctx.request_repaint_after(step.saturating_sub(now.duration_since(self.last_step.unwrap_or(now))));
    }

    fn rebuild_texture(&mut self, ctx: &egui::Context) {
        self.texture_dirty = false;
        let Some(img) = self.image.as_ref() else {
            return;
        };
        let color = self.display().image(img);
        // Kept to play smoothly, while there's room.
        if self.sequence().is_some() && self.played_with == Some(self.display()) {
            let kept: usize = self.played.iter().flatten().map(|f| f.pixels.len() * 4).sum();
            if let Some(slot) = self.played.get_mut(self.frame).filter(|_| kept + color.pixels.len() * 4 <= PLAYBACK_CACHE_BYTES) {
                *slot = Some(color.clone());
            }
        }
        match self.texture.as_mut() {
            Some(texture) => texture.set(color, Self::texture_options()),
            None => self.texture = Some(ctx.load_texture("render_viewer_image", color, Self::texture_options())),
        }
    }

    // Writes the current image: a plain copy for the same format, linear floats for EXR,
    // otherwise the image as currently displayed (exposure, gamma and channel applied).
    fn save_as(&self, target: &Path) -> Result<(), String> {
        let img = self.image.as_ref().ok_or("No image loaded")?;
        let ext = |p: &Path| p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext(target) == ext(&img.path) {
            return std::fs::copy(&img.path, target).map(|_| ()).map_err(|e| e.to_string());
        }
        let (w, h) = (img.width as u32, img.height as u32);
        if ext(target) == "exr" {
            let raw = img.pixels.iter().flat_map(|p| *p).collect::<Vec<f32>>();
            let buffer = image::Rgba32FImage::from_raw(w, h, raw).ok_or("Image buffer size mismatch")?;
            return image::DynamicImage::ImageRgba32F(buffer).save(target).map_err(|e| e.to_string());
        }
        let display = self.display();
        let raw = img.pixels.iter().flat_map(|p| display.bytes(*p)).collect::<Vec<u8>>();
        let buffer = image::RgbImage::from_raw(w, h, raw).ok_or("Image buffer size mismatch")?;
        buffer.save(target).map_err(|e| e.to_string())
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        self.poll_progress();
        self.play(ctx);
        self.poll_saving();
        if self.progress.is_some() || self.saving.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.texture_dirty {
            self.rebuild_texture(ctx);
        }
        let mut open = self.open;
        let rendering = self.progress.is_some();
        crate::own_window(ctx, "render_viewer", "Render Viewer", [760.0, 560.0], [360.0, 260.0], &mut open, false, |ui| {
            self.toolbar(ui);
            self.display_controls(ui);
            self.sequence_controls(ui);
            ui.separator();
            self.footer(ui);
            self.image_area(ui);
            // Its own window redraws itself while a render comes in.
            if rendering {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        });
        self.open = open;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let count = self.history.len();
            let current = self.current.unwrap_or(0);
            if ui.add_enabled(count > 0 && current > 0, egui::Button::new("◀")).on_hover_text("Previous render").clicked() {
                self.select(current - 1);
            }
            let label = self
                .current
                .and_then(|i| self.history.get(i))
                .map(|e| format!("{}/{} · {}", current + 1, count, e.label()))
                .unwrap_or_else(|| "No renders yet".into());
            let mut pick = None;
            egui::ComboBox::from_id_salt("viewer_history")
                .width(260.0)
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (i, entry) in self.history.iter().enumerate().rev() {
                        if ui.selectable_label(self.current == Some(i), format!("{}. {}", i + 1, entry.label())).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.select(i);
            }
            if ui.add_enabled(count > 0 && current + 1 < count, egui::Button::new("▶")).on_hover_text("Next render").clicked() {
                self.select(current + 1);
            }
            ui.separator();
            if ui.button("Open…").on_hover_text("Open an image, or pick several to play them as a sequence").clicked() {
                if let Some(paths) = rfd::FileDialog::new()
                    .add_filter("Images", &["exr", "png", "jpg", "jpeg", "tif", "tiff", "hdr"])
                    .pick_files()
                {
                    self.add_sequence(paths, crate::sims::SIM_FPS);
                }
            }
            let has_image = self.image.is_some();
            if ui.add_enabled(self.current.is_some(), egui::Button::new("⟳ Reload")).clicked() {
                if let Some(path) = self.image.as_ref().map(|img| img.path.clone()) {
                    self.played.iter_mut().for_each(|f| *f = None);
                    self.reload(&path);
                }
            }
            if ui.add_enabled(has_image, egui::Button::new("💾 Save As…"))
                .on_hover_text("Same format: exact copy. EXR: linear floats. PNG/JPG/TIFF: the image as displayed.")
                .clicked()
            {
                let name = self
                    .image
                    .as_ref()
                    .and_then(|img| img.path.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "render.exr".into());
                if let Some(target) = rfd::FileDialog::new()
                    .set_file_name(&name)
                    .add_filter("Images", &["exr", "png", "jpg", "tif"])
                    .save_file()
                {
                    self.status = match self.save_as(&target) {
                        Ok(()) => format!("Saved {}", target.display()),
                        Err(e) => format!("Save failed: {e}"),
                    };
                }
            }
            if ui.add_enabled(has_image, egui::Button::new("Reveal")).on_hover_text("Show the file in the file browser").clicked() {
                if let Some(img) = &self.image {
                    if let Err(e) = reveal_in_file_browser(&img.path) {
                        self.status = e;
                    }
                }
            }
            if ui.add_enabled(has_image, egui::Button::new("Copy path")).clicked() {
                if let Some(img) = &self.image {
                    ui.ctx().copy_text(img.path.to_string_lossy().into_owned());
                    self.status = "Path copied".into();
                }
            }
            if ui.add_enabled(count > 0, egui::Button::new("Clear")).on_hover_text("Forget the render history (files are kept)").clicked() {
                self.history.clear();
                self.current = None;
                self.playing = false;
                self.played.clear();
                self.image = None;
                self.texture = None;
                self.error = None;
            }
        });
    }

    fn display_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut changed = false;
            changed |= ui
                .add(egui::Slider::new(&mut self.exposure, -8.0..=8.0).step_by(0.1).text("Exposure (stops)"))
                .changed();
            if ui.small_button("0").on_hover_text("Reset exposure").clicked() {
                self.exposure = 0.0;
                changed = true;
            }
            changed |= ui.checkbox(&mut self.srgb, "sRGB").on_hover_text("sRGB display transform; off uses a plain gamma curve").changed();
            if !self.srgb {
                changed |= ui.add(egui::Slider::new(&mut self.gamma, 1.0..=3.0).text("Gamma")).changed();
            }
            ui.separator();
            for (channel, label) in [
                (Channel::Rgb, "RGB"),
                (Channel::R, "R"),
                (Channel::G, "G"),
                (Channel::B, "B"),
                (Channel::A, "A"),
                (Channel::Luma, "Luma"),
            ] {
                changed |= ui.selectable_value(&mut self.channel, channel, label).changed();
            }
            ui.separator();
            if ui.selectable_label(self.fit, "Fit").on_hover_text("Fit to window (double-click image)").clicked() {
                self.fit = true;
            }
            if ui.button("1:1").on_hover_text("Actual pixels").clicked() {
                self.fit = false;
                self.zoom = 1.0 / ui.ctx().pixels_per_point();
                self.pan = egui::Vec2::ZERO;
            }
            ui.label(format!("{:.0}%", self.zoom * ui.ctx().pixels_per_point() * 100.0));
            if changed {
                self.texture_dirty = true;
            }
        });
    }

    // Play, the frame slider and the frame rate, and saving as a movie or GIF: shown for a
    // sequence.
    fn sequence_controls(&mut self, ui: &mut egui::Ui) {
        let Some((frames, fps)) = self.sequence() else { return };
        let (count, mut fps) = (frames.len(), fps);
        ui.horizontal_wrapped(|ui| {
            let label = if self.playing { "⏸ Pause" } else { "▶ Play" };
            if ui.add_enabled(count > 1, egui::Button::new(label)).on_hover_text("Play the frames as an animation, looping").clicked() {
                self.toggle_play();
            }
            let mut frame = self.frame + 1;
            if ui.add(egui::Slider::new(&mut frame, 1..=count.max(1)).text(format!("of {count}"))).changed() {
                self.playing = false;
                self.show_frame(frame - 1);
            }
            if ui.add(egui::DragValue::new(&mut fps).range(1.0..=120.0).speed(0.25).suffix(" fps")).on_hover_text("How fast it plays, and saves").changed() {
                if let Some(Some(Entry::Sequence { fps: kept, .. })) = self.current.map(|i| self.history.get_mut(i)) {
                    *kept = fps;
                }
            }
            ui.separator();
            let idle = self.saving.is_none();
            if ui.add_enabled(idle, egui::Button::new("🎬 Save movie…")).on_hover_text("An MP4 of the frames as displayed (needs FFmpeg)").clicked() {
                self.save_animation(false);
            }
            if ui.add_enabled(idle, egui::Button::new("Save GIF…")).on_hover_text("A looping GIF of the frames as displayed").clicked() {
                self.save_animation(true);
            }
            if !idle {
                ui.spinner();
                ui.label("Saving…");
            }
        });
    }

    fn toggle_play(&mut self) {
        self.playing = !self.playing;
        self.last_step = None;
        if !self.playing {
            // Paused: the frame in full, for the pixel inspector.
            self.show_frame(self.frame);
        }
    }

    // Asks where, then writes the sequence as displayed in the background.
    fn save_animation(&mut self, gif: bool) {
        let Some((frames, fps)) = self.sequence() else { return };
        let frames = frames.to_vec();
        let ext = if gif { "gif" } else { "mp4" };
        let name = sequence_name(&frames[0]).replace('#', "");
        let name = format!("{}.{ext}", name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem).trim_end_matches(['.', '_']));
        let Some(target) = rfd::FileDialog::new().set_file_name(&name).add_filter(if gif { "GIF" } else { "Movie" }, &[ext]).save_file() else {
            return;
        };
        let display = self.display();
        let (tx, rx) = std::sync::mpsc::channel();
        self.saving = Some(rx);
        self.status.clear();
        std::thread::spawn(move || {
            let _ = tx.send(save_frames(&frames, display, fps, &target, gif));
        });
    }

    fn poll_saving(&mut self) {
        let Some(rx) = self.saving.as_ref() else { return };
        match rx.try_recv() {
            Ok(result) => {
                self.status = result.unwrap_or_else(|e| format!("Save failed: {e}"));
                self.saving = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status = "Save failed".into();
                self.saving = None;
            }
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.progress.is_some() {
                let (done, total) = self.progress_counts;
                ui.spinner();
                ui.label(format!("Rendering: {done}/{total} buckets"));
                ui.separator();
            }
            if let Some(img) = &self.image {
                ui.label(format!(
                    "{}×{} · {} · {}",
                    img.width,
                    img.height,
                    if img.hdr { "float (HDR)" } else { "8-bit" },
                    human_bytes(img.file_bytes)
                ));
                ui.weak(img.path.display().to_string());
            }
            if !self.status.is_empty() {
                ui.separator();
                ui.colored_label(egui::Color32::LIGHT_GREEN, &self.status);
            }
        });
    }

    fn image_area(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size().max(egui::vec2(64.0, 64.0)), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(24));

        let (Some(img), Some(texture)) = (self.image.as_ref(), self.texture.as_ref()) else {
            let message = self.error.clone().unwrap_or_else(|| "Render with MoonRay, or Open… an image".into());
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, message, egui::FontId::proportional(14.0), egui::Color32::GRAY);
            return;
        };
        let image_size = egui::vec2(img.width as f32, img.height as f32);

        if response.double_clicked() {
            self.fit = true;
        }
        if self.fit {
            self.zoom = (rect.width() / image_size.x).min(rect.height() / image_size.y).max(0.01);
            self.pan = egui::Vec2::ZERO;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0);
            if scroll.abs() > 0.0 {
                self.fit = false;
                let old = self.zoom;
                self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(0.01, 64.0);
                // Zoom about the cursor so the pixel under it stays put.
                if let Some(cursor) = response.hover_pos() {
                    let offset = cursor - (rect.center() + self.pan);
                    self.pan += offset - offset * (self.zoom / old);
                }
            }
        }
        if response.dragged() {
            self.fit = false;
            self.pan += response.drag_delta();
        }

        let image_rect = egui::Rect::from_center_size(rect.center() + self.pan, image_size * self.zoom);
        painter.image(
            texture.id(),
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        // Buckets being rendered: corner brackets like Houdini and Maya.
        for [x0, y0, x1, y1] in &self.progress_overlay {
            let min = image_rect.min + egui::vec2(*x0 as f32, *y0 as f32) * self.zoom;
            let max = image_rect.min + egui::vec2(*x1 as f32, *y1 as f32) * self.zoom;
            let arm = ((max.x - min.x).min(max.y - min.y) * 0.25).clamp(3.0, 14.0);
            let stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(255, 190, 60));
            for (corner, dx, dy) in [(min, 1.0, 1.0), (egui::pos2(max.x, min.y), -1.0, 1.0), (egui::pos2(min.x, max.y), 1.0, -1.0), (max, -1.0, -1.0)] {
                painter.line_segment([corner, corner + egui::vec2(arm * dx, 0.0)], stroke);
                painter.line_segment([corner, corner + egui::vec2(0.0, arm * dy)], stroke);
            }
        }

        // Pixel inspector: raw linear values under the cursor.
        if let Some(cursor) = response.hover_pos().filter(|p| image_rect.contains(*p) && !self.playing) {
            let x = (((cursor.x - image_rect.min.x) / self.zoom) as usize).min(img.width - 1);
            let y = (((cursor.y - image_rect.min.y) / self.zoom) as usize).min(img.height - 1);
            let p = img.pixels[y * img.width + x];
            let text = format!("x {x}  y {y}   R {:.4}  G {:.4}  B {:.4}  A {:.3}", p[0], p[1], p[2], p[3]);
            let galley = painter.layout_no_wrap(text, egui::FontId::monospace(12.0), egui::Color32::WHITE);
            let pos = rect.left_bottom() + egui::vec2(8.0, -8.0 - galley.size().y);
            painter.rect_filled(egui::Rect::from_min_size(pos, galley.size()).expand(4.0), 3.0, egui::Color32::from_black_alpha(180));
            painter.galley(pos, galley, egui::Color32::WHITE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exr_round_trip_keeps_linear_values() {
        let path = std::env::temp_dir().join("proceduralfx_viewer_test.exr");
        let raw = vec![0.25f32, 2.0, 0.0, 1.0, 1.0, 0.5, 0.125, 0.0];
        let buffer = image::Rgba32FImage::from_raw(2, 1, raw).unwrap();
        image::DynamicImage::ImageRgba32F(buffer).save(&path).unwrap();
        let img = load_image(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(img.hdr);
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.pixels[0], [0.25, 2.0, 0.0, 1.0]);
    }

    fn write_frames(dir: &Path, count: usize) -> Vec<PathBuf> {
        let _ = std::fs::create_dir_all(dir);
        (0..count)
            .map(|i| {
                let path = dir.join(format!("render.{:04}.exr", i + 1));
                let raw = vec![i as f32 * 0.25, 0.0, 0.0, 1.0, 0.0, 0.5, 0.0, 1.0];
                image::DynamicImage::ImageRgba32F(image::Rgba32FImage::from_raw(2, 1, raw).unwrap()).save(&path).unwrap();
                path
            })
            .collect()
    }

    // A sequence's frames, rendered one after another, are one entry that plays.
    #[test]
    fn a_rendered_sequence_is_one_entry() {
        let dir = std::env::temp_dir().join(format!("pfx_viewer_sequence_{}", std::process::id()));
        let frames = write_frames(&dir, 3);
        let mut viewer = RenderViewer::default();
        viewer.add(dir.join("still.exr"), false);
        for (i, frame) in frames.iter().enumerate() {
            viewer.add_frame(frame.clone(), i == 0, 24.0, true);
        }
        assert_eq!(viewer.history.len(), 2, "the still and the sequence");
        assert_eq!(viewer.history[1], Entry::Sequence { frames: frames.clone(), fps: 24.0 });
        assert_eq!(viewer.history[1].label(), "render.####.exr (3 frames)");
        assert_eq!(viewer.frame, 2, "the newest frame is shown");
        assert_eq!(viewer.image.as_ref().unwrap().path, frames[2]);
        // Rendering it again replaces it, rather than adding another.
        viewer.add_frame(frames[0].clone(), true, 24.0, true);
        assert_eq!(viewer.history.len(), 2);
        // Opening several files plays them as a sequence too, in order.
        viewer.add_sequence(vec![frames[1].clone(), frames[0].clone()], 30.0);
        assert_eq!(viewer.history.last(), Some(&Entry::Sequence { frames: frames[..2].to_vec(), fps: 30.0 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Saved as displayed: exposure applied, and every frame the same size.
    #[test]
    fn a_sequence_saves_as_displayed() {
        let dir = std::env::temp_dir().join(format!("pfx_viewer_gif_{}", std::process::id()));
        let frames = write_frames(&dir, 3);
        let plain = Display { exposure: 0.0, gamma: 2.2, srgb: true, channel: Channel::Rgb };
        let images = displayed_frames(&frames, plain).unwrap();
        assert_eq!(images.len(), 3);
        assert_eq!(images[2].pixels[0].r(), (linear_to_srgb(0.5) * 255.0 + 0.5) as u8);
        let brighter = displayed_frames(&frames, Display { exposure: 1.0, ..plain }).unwrap();
        assert_eq!(brighter[2].pixels[0].r(), 255);
        let gif = dir.join("render.gif");
        let message = save_frames(&frames, plain, 24.0, &gif, true).unwrap();
        assert!(message.contains("3-frame GIF"), "{message}");
        let decoded = image::AnimationDecoder::into_frames(image::codecs::gif::GifDecoder::new(std::io::BufReader::new(std::fs::File::open(&gif).unwrap())).unwrap());
        assert_eq!(decoded.count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sequences_are_named_by_their_frames() {
        assert_eq!(sequence_name(Path::new("/r/smoke.0012.exr")), "smoke.####.exr");
        assert_eq!(sequence_name(Path::new("frame_007.png")), "frame_###.png");
        assert_eq!(sequence_name(Path::new("still.exr")), "still.exr");
    }

    #[test]
    fn srgb_transfer_round_trips() {
        for v in [0.0f32, 0.002, 0.18, 0.5, 1.0] {
            assert!((srgb_to_linear(linear_to_srgb(v)) - v).abs() < 1e-5);
        }
    }
}
