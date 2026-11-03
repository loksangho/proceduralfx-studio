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
    // Also write what Godot and Unity need to play it (see `godot_sprite_frames`, `unity_meta`).
    pub godot: bool,
    pub unity: bool,
    // For particle systems: a grid with no empty cells (they'd show as blank frames).
    pub fill_grid: bool,
    // Also write name_motion.png: how each pixel moves to the next frame, so games can
    // slide between frames rather than cross-fade (see `motion`).
    pub motion_vectors: bool,
    // Also write name_normal.png: each frame's shape, for engines' 2D lights (see `normals`).
    pub normal_map: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { open: false, width: 256, height: 256, every: 1, columns: 0, trim: true, live: false, seamless: false, blend: 8, godot: false, unity: false, fill_grid: false, motion_vectors: false, normal_map: false }
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
    pub godot: bool,
    pub unity: bool,
    pub fill_grid: bool,
    pub motion_vectors: bool,
    pub normal_map: bool,
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

// Cells across that leave no empty cells (a divisor of the frame count), nearest the
// layout `columns` would choose. Particle systems play every cell of the grid.
pub fn filled_columns(count: usize, size: [u32; 2], requested: u32) -> u32 {
    let count = count.max(1) as u32;
    let want = columns(count as usize, size, requested) as f32;
    (1..=count).filter(|c| count % c == 0).min_by(|a, b| ((*a as f32 / want).ln().abs()).total_cmp(&(*b as f32 / want).ln().abs())).unwrap_or(1)
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

// The frame edges ("top", "bottom", "left", "right") that something visible touches in
// any frame: the effect runs off them, so the sheet cuts it off.
pub fn edges_touched(frames: &[egui::ColorImage]) -> Vec<&'static str> {
    let Some([w, h]) = frames.first().map(|f| f.size) else { return Vec::new() };
    let mut touched = [false; 4];
    for frame in frames.iter().filter(|f| f.size == [w, h]) {
        // Faint wisps that barely show don't count.
        let shows = |x: usize, y: usize| frame.pixels[y * w + x].a() > 24;
        touched[0] |= (0..w).any(|x| shows(x, 0));
        touched[1] |= (0..w).any(|x| shows(x, h - 1));
        touched[2] |= (0..h).any(|y| shows(0, y));
        touched[3] |= (0..h).any(|y| shows(w - 1, y));
    }
    ["top", "bottom", "left", "right"].into_iter().zip(touched).filter(|(_, t)| *t).map(|(e, _)| e).collect()
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
    let crop = if trim { trim_box(frames) } else { [0, 0, first.size[0] as u32, first.size[1] as u32] };
    pack_in(frames, requested, crop)
}

// Packs the frames cropped to `crop` ([x, y, width, height] in the full frame).
pub fn pack_in(frames: &[egui::ColorImage], requested: u32, crop: [u32; 4]) -> Result<Sheet, String> {
    let first = frames.first().ok_or("no frames were rendered")?;
    let full = [first.size[0] as u32, first.size[1] as u32];
    let [x, y, w, h] = crop;
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
pub fn metadata(sheet: &Sheet, image_name: &str, frame_count: usize, fps: f32, source_size: [u32; 2], motion: Option<&Motion>, normal_map: Option<&str>) -> serde_json::Value {
    let [w, h] = sheet.cell;
    let frames: Vec<[u32; 4]> = (0..frame_count as u32).map(|i| [(i % sheet.columns) * w, (i / sheet.columns) * h, w, h]).collect();
    let mut json = serde_json::json!({
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
    });
    if let Some(name) = normal_map {
        json["normal_map"] = serde_json::json!({
            "image": name,
            "convention": "tangent space, laid out like the sheet: red right, green up, blue towards the viewer, each (n + 1) / 2",
        });
    }
    if let Some(m) = motion {
        json["motion_vectors"] = serde_json::json!({
            "image": m.image_name,
            "strength": (m.strength as f64 * 1000.0).round() / 1000.0,
            "looping": m.looping,
            "encoding": "red and green: each pixel's shift to the next frame, across and down, in fractions of a frame's size: (value - 0.5) * 2 * strength. Draw a frame t of the way to the next by sampling this frame at uv - shift * t and the next at uv + shift * (1 - t), and blending",
        });
    }
    json
}

// A sheet's motion vectors (name_motion.png): laid out like the sheet.
#[derive(Debug)]
pub struct Motion {
    pub sheet: Sheet,
    pub image_name: String,
    // The largest shift the colours can hold, in fractions of a frame.
    pub strength: f32,
    // Whether the last frame's vectors lead into the first.
    pub looping: bool,
}

// name_normal.png beside name.png.
pub fn normal_path(png: &Path) -> PathBuf {
    let stem = png.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    png.with_file_name(format!("{stem}_normal.png"))
}

// name_motion.png beside name.png.
pub fn motion_path(png: &Path) -> PathBuf {
    let stem = png.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    png.with_file_name(format!("{stem}_motion.png"))
}

// The motion vectors of `frames`, packed like `sheet` (which they were packed into).
pub fn motion_sheet(frames: &[egui::ColorImage], sheet: &Sheet, looping: bool, image_name: String) -> Result<Motion, String> {
    let flows = crate::motion::flows(frames, looping);
    let (images, strength) = crate::motion::encode(&flows, sheet.cell);
    let crop = [sheet.offset[0], sheet.offset[1], sheet.cell[0], sheet.cell[1]];
    let packed = pack_in(&images, sheet.columns, crop)?;
    Ok(Motion { sheet: packed, image_name, strength, looping })
}

fn save_png(image: &image::RgbaImage, png: &Path) -> Result<(), String> {
    let partial = png.with_extension("png.partial");
    image.save_with_format(&partial, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, png).map_err(|e| e.to_string())
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
    let requested = match frames.first() {
        Some(f) if output.fill_grid => filled_columns(frames.len(), [f.size[0] as u32, f.size[1] as u32], output.columns),
        _ => output.columns,
    };
    let sheet = pack(frames, requested, output.trim)?;
    let png = output.path.with_extension("png");
    if let Some(dir) = png.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Written beside the file and renamed over it, so a game watching the files never
    // reads half of one. The images go first: the JSON's reload reloads them too.
    save_png(&sheet.image, &png)?;
    let file_name = |p: &Path| p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let name = file_name(&png);
    let motion = if output.motion_vectors {
        let path = motion_path(&png);
        let looping = output.blend > 0 || crate::motion::looks_looped(frames);
        let motion = motion_sheet(frames, &sheet, looping, file_name(&path))?;
        save_png(&motion.sheet.image, &path)?;
        Some(motion)
    } else {
        None
    };
    let normal = if output.normal_map {
        let path = normal_path(&png);
        let maps = crate::normals::normal_maps(frames);
        let packed = pack_in(&maps, sheet.columns, [sheet.offset[0], sheet.offset[1], sheet.cell[0], sheet.cell[1]])?;
        save_png(&packed.image, &path)?;
        Some(file_name(&path))
    } else {
        None
    };
    let size = frames.first().map_or([0, 0], |f| [f.size[0] as u32, f.size[1] as u32]);
    let json = serde_json::to_string_pretty(&metadata(&sheet, &name, frames.len(), output.fps, size, motion.as_ref(), normal.as_deref())).map_err(|e| e.to_string())?;
    let partial = png.with_extension("pfx.json.partial");
    std::fs::write(&partial, json + "\n").map_err(|e| e.to_string())?;
    std::fs::rename(&partial, json_path(&png)).map_err(|e| e.to_string())?;
    let count = frames.len();
    if output.godot {
        write_beside(&godot_path(&png), &godot_sprite_frames(&sheet, &name, count, output.fps))?;
        if let Some(normal_name) = &normal {
            write_beside(&with_suffix(&normal_path(&png), ".import"), &crate::vat::godot_texture_import())?;
            let [shader, material] = godot_lit_paths(&png);
            write_beside(&shader, GODOT_LIT_SHADER)?;
            write_beside(&material, &godot_lit_material(&stem_of(&png), normal_name))?;
        }
        if output.fill_grid {
            write_beside(&godot_particles_path(&png), &godot_particle_material(&sheet))?;
        }
    }
    if output.unity {
        let meta = unity_meta_path(&png);
        // Re-exports keep the same guid, so Unity keeps what uses the sprites.
        let guid = std::fs::read_to_string(&meta).ok().and_then(|text| meta_guid(&text)).unwrap_or_else(|| new_guid(&png));
        let normal_guid = match &normal {
            Some(_) => {
                let path = normal_path(&png);
                let g = crate::vat::unity_guid(&unity_meta_path(&path), &stem_of(&png));
                write_beside(&unity_meta_path(&path), &unity_normal_meta(&g, sheet.image.width().max(sheet.image.height())))?;
                Some(g)
            }
            None => None,
        };
        write_beside(&meta, &unity_meta(&sheet, &name, count, &guid, normal_guid.as_deref()))?;
    }
    if let Some(motion) = &motion {
        write_flipbook_engines(output, &png, &sheet, count, size, motion, normal.as_deref())?;
    }
    Ok((png, sheet))
}

fn write_beside(path: &Path, text: &str) -> Result<(), String> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, path).map_err(|e| e.to_string())
}

// name.tres beside name.png: a Godot 4 SpriteFrames resource.
pub fn godot_path(png: &Path) -> PathBuf {
    png.with_extension("tres")
}

// name_particles.tres beside name.png: a material for GPUParticles2D.
pub fn godot_particles_path(png: &Path) -> PathBuf {
    let stem = png.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    png.with_file_name(format!("{stem}_particles.tres"))
}

// A Godot 4 material that plays the sheet on each particle of a GPUParticles2D (or
// CPUParticles2D), one cell after another: set it as the node's material, with the sheet
// as its texture. How fast the frames go is the process material's Animation > Speed (1
// plays them once over a particle's life).
pub fn godot_particle_material(sheet: &Sheet) -> String {
    format!(
        "[gd_resource type=\"CanvasItemMaterial\" format=3]\n\n[resource]\nparticles_animation = true\nparticles_anim_h_frames = {}\nparticles_anim_v_frames = {}\nparticles_anim_loop = true\n",
        sheet.columns, sheet.rows
    )
}

// name.png.meta: Unity's import settings for name.png.
pub fn unity_meta_path(png: &Path) -> PathBuf {
    let mut name = png.as_os_str().to_owned();
    name.push(".meta");
    PathBuf::from(name)
}

// The frames' rectangles in the sheet, left to right, top to bottom: [x, y, width, height]
// from the top-left corner.
fn frame_rects(sheet: &Sheet, count: usize) -> Vec<[u32; 4]> {
    let [w, h] = sheet.cell;
    (0..count as u32).map(|i| [(i % sheet.columns) * w, (i / sheet.columns) * h, w, h]).collect()
}

// A Godot 4 SpriteFrames resource playing the sheet's frames, looping, at its frame rate:
// put it on an AnimatedSprite2D (or AnimatedSprite3D). The sheet is found beside it.
pub fn godot_sprite_frames(sheet: &Sheet, image_name: &str, count: usize, fps: f32) -> String {
    let rects = frame_rects(sheet, count);
    let mut out = format!("[gd_resource type=\"SpriteFrames\" load_steps={} format=3]\n\n", rects.len() + 2);
    out += &format!("[ext_resource type=\"Texture2D\" path=\"{image_name}\" id=\"1_sheet\"]\n\n");
    for (i, [x, y, w, h]) in rects.iter().enumerate() {
        out += &format!("[sub_resource type=\"AtlasTexture\" id=\"AtlasTexture_{i}\"]\natlas = ExtResource(\"1_sheet\")\nregion = Rect2({x}, {y}, {w}, {h})\n\n");
    }
    let frames: Vec<String> = (0..rects.len()).map(|i| format!("{{\n\"duration\": 1.0,\n\"texture\": SubResource(\"AtlasTexture_{i}\")\n}}")).collect();
    out += &format!("[resource]\nanimations = [{{\n\"frames\": [{}],\n\"loop\": true,\n\"name\": &\"default\",\n\"speed\": {fps:.1}\n}}]\n", frames.join(", "));
    out
}

// A 32-hex-digit id Unity uses for assets and sprites, from a name (so the same name gives
// the same id, and nothing random is needed).
fn hex_id(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(seed.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn new_guid(png: &Path) -> String {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    hex_id(&format!("{}:{stamp}", png.display()))
}

fn meta_guid(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("guid: ")).map(|g| g.trim().to_owned()).filter(|g| g.len() == 32 && g.chars().all(|c| c.is_ascii_hexdigit()))
}

// Unity's import settings for the sheet: a sprite texture sliced into one sprite per frame
// (name_0, name_1...), uncompressed, without mipmaps, and not shrunk below its size. In
// Unity, select the sprites and drag them into the scene to make an animation.
pub fn unity_meta(sheet: &Sheet, image_name: &str, count: usize, guid: &str, normal_guid: Option<&str>) -> String {
    let stem = image_name.strip_suffix(".png").unwrap_or(image_name);
    let height = sheet.image.height();
    let size = sheet.image.width().max(height).next_power_of_two().clamp(32, MAX_SIDE);
    let sprites: Vec<(String, [u32; 4], i64)> = frame_rects(sheet, count)
        .into_iter()
        .enumerate()
        .map(|(i, [x, y, w, h])| {
            let name = format!("{stem}_{i}");
            // Unity's rectangles start at the bottom left.
            let id = i64::from_str_radix(&hex_id(&name)[..15], 16).unwrap_or(i as i64 + 1);
            (name, [x, height - y - h, w, h], id)
        })
        .collect();
    let mut out = format!(
        "fileFormatVersion: 2
guid: {guid}
TextureImporter:
  internalIDToNameTable: []
  externalObjects: {{}}
  serializedVersion: 13
  mipmaps:
    mipMapMode: 0
    enableMipMap: 0
    sRGBTexture: 1
    linearTexture: 0
    fadeOut: 0
    borderMipMap: 0
    mipMapsPreserveCoverage: 0
    alphaTestReferenceValue: 0.5
    mipMapFadeDistanceStart: 1
    mipMapFadeDistanceEnd: 3
  bumpmap:
    convertToNormalMap: 0
    externalNormalMap: 0
    heightScale: 0.25
    normalMapFilter: 0
    flipGreenChannel: 0
  isReadable: 0
  streamingMipmaps: 0
  streamingMipmapsPriority: 0
  vTOnly: 0
  ignoreMipmapLimit: 0
  grayScaleToAlpha: 0
  generateCubemap: 6
  cubemapConvolution: 0
  seamlessCubemap: 0
  textureFormat: 1
  maxTextureSize: {size}
  textureSettings:
    serializedVersion: 2
    filterMode: 1
    aniso: 1
    mipBias: 0
    wrapU: 1
    wrapV: 1
    wrapW: 1
  nPOTScale: 0
  lightmap: 0
  compressionQuality: 50
  spriteMode: 2
  spriteExtrude: 1
  spriteMeshType: 0
  alignment: 0
  spritePivot: {{x: 0.5, y: 0.5}}
  spritePixelsToUnits: 100
  spriteBorder: {{x: 0, y: 0, z: 0, w: 0}}
  spriteGenerateFallbackPhysicsShape: 0
  alphaUsage: 1
  alphaIsTransparency: 1
  spriteTessellationDetail: -1
  textureType: 8
  textureShape: 1
  singleChannelComponent: 0
  flipbookRows: 1
  flipbookColumns: 1
  maxTextureSizeSet: 0
  compressionQualitySet: 0
  textureFormatSet: 0
  ignorePngGamma: 0
  applyGammaDecoding: 0
  swizzle: 50462976
  cookieLightType: 0
  platformSettings:
  - serializedVersion: 4
    buildTarget: DefaultTexturePlatform
    maxTextureSize: {size}
    resizeAlgorithm: 0
    textureFormat: -1
    textureCompression: 0
    compressionQuality: 50
    crunchedCompression: 0
    allowsAlphaSplitting: 0
    overridden: 0
    ignorePlatformSupport: 0
    androidETC2FallbackOverride: 0
    forceMaximumCompressionQuality_BC6H_BC7: 0
  spriteSheet:
    serializedVersion: 2
    sprites:
"
    );
    for (name, [x, y, w, h], id) in &sprites {
        out += &format!(
            "    - serializedVersion: 2
      name: {name}
      rect:
        serializedVersion: 2
        x: {x}
        y: {y}
        width: {w}
        height: {h}
      alignment: 0
      pivot: {{x: 0.5, y: 0.5}}
      border: {{x: 0, y: 0, z: 0, w: 0}}
      customData: 
      outline: []
      physicsShape: []
      tessellationDetail: -1
      bones: []
      spriteID: {}
      internalID: {id}
      vertices: []
      indices: 
      edges: []
      weights: []
",
            hex_id(&format!("{guid}/{name}"))
        );
    }
    out += "    outline: []
    customData: 
    physicsShape: []
    bones: []
    spriteID: 
    internalID: 0
    vertices: []
    indices: 
    edges: []
    weights: []
";
    // The normal map, for URP's 2D lights (Sprite-Lit-Default reads _NormalMap).
    out += &match normal_guid {
        Some(g) => format!("    secondaryTextures:\n    - name: _NormalMap\n      texture: {{fileID: 2800000, guid: {g}, type: 3}}\n"),
        None => "    secondaryTextures: []\n".to_owned(),
    };
    out += "    spriteCustomMetadata:
      entries: []
    nameFileIdTable:
";
    for (name, _, id) in &sprites {
        out += &format!("      {name}: {id}\n");
    }
    out += "  mipmapLimitGroupName: 
  pSDRemoveMatte: 0
  userData: 
  assetBundleName: 
  assetBundleVariant: 
";
    out
}

// ---------------------------------------------------------------------------
// Smooth playback in Godot and Unity: a shader that, between two frames, slides the first
// forward along the motion vectors and the next one back, and blends them (as
// bevy_procedural_fx's FxFlipbook does). Without motion vectors the same shaders could
// only cross-fade, so they're written only with them.
// ---------------------------------------------------------------------------

fn stem_of(png: &Path) -> String {
    png.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

// name_lit.gdshader and name_lit.tres beside name.png: a material for the AnimatedSprite2D
// playing name.tres, so Light2D lights it with the normal map. (The normal sheet is laid
// out like the sheet, and an AtlasTexture's UV covers the whole sheet, so the same UV
// finds a pixel's normal. A CanvasTexture can't be used: AtlasTextures drop its normals.)
pub fn godot_lit_paths(png: &Path) -> [PathBuf; 2] {
    let stem = stem_of(png);
    [png.with_file_name(format!("{stem}_lit.gdshader")), png.with_file_name(format!("{stem}_lit.tres"))]
}

pub const GODOT_LIT_SHADER: &str = "// ProceduralFX Studio: lights a sprite sheet with its normal map (name_normal.png).
shader_type canvas_item;

uniform sampler2D normal_map : hint_normal, filter_linear, repeat_disable;

void fragment() {
	NORMAL_MAP = texture(normal_map, UV).rgb;
}
";

pub fn godot_lit_material(stem: &str, normal_name: &str) -> String {
    format!(
        "[gd_resource type=\"ShaderMaterial\" load_steps=3 format=3]

[ext_resource type=\"Shader\" path=\"{stem}_lit.gdshader\" id=\"1_shader\"]
[ext_resource type=\"Texture2D\" path=\"{normal_name}\" id=\"2_normal\"]

[resource]
shader = ExtResource(\"1_shader\")
shader_parameter/normal_map = ExtResource(\"2_normal\")
"
    )
}

// name_flipbook.gdshader and name_flipbook.tscn beside name.png.
pub fn godot_flipbook_paths(png: &Path) -> [PathBuf; 2] {
    let stem = stem_of(png);
    [png.with_file_name(format!("{stem}_flipbook.gdshader")), png.with_file_name(format!("{stem}_flipbook.tscn"))]
}

// A canvas_item shader for a Sprite2D showing the sheet with `hframes` and `vframes` set to
// its grid (so its UV covers the first cell); it picks the frame itself, from TIME or from
// `frame`.
pub fn godot_flipbook_shader(sheet: &Sheet, count: usize, fps: f32, motion: &Motion, normal_map: bool) -> String {
    format!(
        "// ProceduralFX Studio flipbook with motion vectors: slides between frames.
shader_type canvas_item;

uniform sampler2D motion_vectors : filter_linear, repeat_disable;
uniform int columns = {columns};
uniform int rows = {rows};
uniform int frame_count = {count};
uniform float fps = {fps:.3};
// The largest shift the motion colours hold, in fractions of a frame.
uniform float strength = {strength};
// Whether the last frame's motion leads into the first.
uniform bool motion_loops = {loops};
uniform bool use_time = true;
uniform bool looping = true;
// The frame to show when use_time is off (from 0; fractions are part of the way to the next).
uniform float frame = 0.0;
uniform int blending : hint_enum(\"Whole frames\", \"Cross-fade\", \"Motion vectors\") = 2;
{normal_uniform}
// Where `local` (0 to 1 across a frame, from its top left) is in the sheet for frame f,
// kept half a texel inside the cell.
vec2 in_sheet(int f, vec2 local, vec2 texel) {{
	vec2 grid = vec2(float(columns), float(rows));
	vec2 cell = vec2(float(f % columns), float(f / columns));
	vec2 half_texel = 0.5 * texel * grid;
	return (cell + clamp(local, half_texel, vec2(1.0) - half_texel)) / grid;
}}

vec4 premultiplied(vec4 c) {{
	return vec4(c.rgb * c.a, c.a);
}}

void fragment() {{
	vec2 local = UV * vec2(float(columns), float(rows));
	float count = float(frame_count);
	float at = use_time ? TIME * fps : frame;
	at = looping ? mod(at, count) : clamp(at, 0.0, count - 1.0);
	int f0 = min(int(floor(at)), frame_count - 1);
	int f1 = looping ? (f0 + 1) % frame_count : min(f0 + 1, frame_count - 1);
	float t = blending == 0 ? 0.0 : fract(at);
	vec2 shift = vec2(0.0);
	if (blending == 2 && (f1 > f0 || motion_loops)) {{
		shift = (texture(motion_vectors, in_sheet(f0, local, TEXTURE_PIXEL_SIZE)).rg - 0.5) * 2.0 * strength;
	}}
	// Where this pixel comes from in this frame and the next.
	vec2 ua = in_sheet(f0, local - shift * t, TEXTURE_PIXEL_SIZE);
	vec2 ub = in_sheet(f1, local + shift * (1.0 - t), TEXTURE_PIXEL_SIZE);
	vec4 c = mix(premultiplied(texture(TEXTURE, ua)), premultiplied(texture(TEXTURE, ub)), t);
{normal_line}	COLOR = vec4(c.rgb / max(c.a, 1e-5), c.a);
}}
",
        columns = sheet.columns,
        rows = sheet.rows,
        strength = (motion.strength as f64 * 1000.0).round() / 1000.0,
        loops = motion.looping,
        normal_uniform = if normal_map { "// Each frame's shape, for Light2D: name_normal.png.\nuniform sampler2D normal_map : hint_normal, filter_linear, repeat_disable;\n" } else { "" },
        normal_line = if normal_map { "\tNORMAL_MAP = mix(texture(normal_map, ua).rgb, texture(normal_map, ub).rgb, t);\n" } else { "" },
    )
}

// A ready scene: a Sprite2D showing the sheet through the shader, placed so the full
// rendered frame's centre is at the node, as the other exports do.
pub fn godot_flipbook_scene(sheet: &Sheet, png: &Path, source_size: [u32; 2], motion: &Motion, normal_map: Option<&str>) -> String {
    let stem = stem_of(png);
    let image = png.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let [cw, ch] = sheet.cell;
    // The cell's centre, from the full frame's centre (y down, as in Godot 2D).
    let dx = sheet.offset[0] as f32 + cw as f32 / 2.0 - source_size[0] as f32 / 2.0;
    let dy = sheet.offset[1] as f32 + ch as f32 / 2.0 - source_size[1] as f32 / 2.0;
    format!(
        "[gd_scene load_steps={steps} format=3]

[ext_resource type=\"Texture2D\" path=\"{image}\" id=\"1_sheet\"]
[ext_resource type=\"Texture2D\" path=\"{motion_image}\" id=\"2_motion\"]
[ext_resource type=\"Shader\" path=\"{stem}_flipbook.gdshader\" id=\"3_shader\"]
{normal_resource}
[sub_resource type=\"ShaderMaterial\" id=\"ShaderMaterial_flipbook\"]
shader = ExtResource(\"3_shader\")
shader_parameter/motion_vectors = ExtResource(\"2_motion\")
{normal_parameter}
[node name=\"{node}\" type=\"Sprite2D\"]
material = SubResource(\"ShaderMaterial_flipbook\")
texture = ExtResource(\"1_sheet\")
offset = Vector2({dx}, {dy})
hframes = {columns}
vframes = {rows}
",
        motion_image = motion.image_name,
        steps = if normal_map.is_some() { 6 } else { 5 },
        normal_resource = normal_map.map_or_else(String::new, |n| format!("[ext_resource type=\"Texture2D\" path=\"{n}\" id=\"4_normal\"]\n")),
        normal_parameter = if normal_map.is_some() { "shader_parameter/normal_map = ExtResource(\"4_normal\")\n" } else { "" },
        node = stem.replace(['.', ':', '@', '/', '"', '%'], "_"),
        columns = sheet.columns,
        rows = sheet.rows,
    )
}

// name_flipbook.shader and name_flipbook.mat beside name.png.
pub fn unity_flipbook_paths(png: &Path) -> [PathBuf; 2] {
    let stem = stem_of(png);
    [png.with_file_name(format!("{stem}_flipbook.shader")), png.with_file_name(format!("{stem}_flipbook.mat"))]
}

// An unlit, transparent shader for a Quad (or any mesh with 0-1 UVs): it draws one frame
// over the whole mesh and picks the frame itself. Its one pass has no LightMode, so it
// runs in URP as well as the built-in pipeline.
pub fn unity_flipbook_shader(stem: &str, sheet: &Sheet, count: usize, fps: f32, motion: &Motion) -> String {
    format!(
        "// ProceduralFX Studio flipbook with motion vectors: slides between frames.
// Put the material on a Quad; scale the Quad to the frame's shape ({cw} by {ch} pixels).
Shader \"ProceduralFX/Flipbook/{stem}\"
{{
    Properties
    {{
        [NoScaleOffset] _MainTex(\"Sheet\", 2D) = \"white\" {{}}
        [NoScaleOffset] _Motion(\"Motion vectors\", 2D) = \"gray\" {{}}
        _Color(\"Color\", Color) = (1, 1, 1, 1)
        _Fps(\"Frames per second\", Float) = {fps:.3}
        [Toggle] _UseTime(\"Play by itself\", Float) = 1
        [Toggle] _Loop(\"Loop\", Float) = 1
        _Frame(\"Frame (when not playing by itself)\", Float) = 0
        [Enum(Whole frames, 0, Cross fade, 1, Motion vectors, 2)] _Blending(\"Blending\", Float) = 2
        [HideInInspector] _Columns(\"Columns\", Float) = {columns}
        [HideInInspector] _Rows(\"Rows\", Float) = {rows}
        [HideInInspector] _FrameCount(\"Frame count\", Float) = {count}
        [HideInInspector] _Strength(\"Strength\", Float) = {strength}
        [HideInInspector] _MotionLoops(\"Motion loops\", Float) = {loops}
    }}
    SubShader
    {{
        Tags {{ \"Queue\" = \"Transparent\" \"RenderType\" = \"Transparent\" \"IgnoreProjector\" = \"True\" \"PreviewType\" = \"Plane\" }}
        // The fragment returns premultiplied colour.
        Blend One OneMinusSrcAlpha
        ZWrite Off
        Cull Off
        Pass
        {{
            CGPROGRAM
            #pragma vertex vert
            #pragma fragment frag
            #include \"UnityCG.cginc\"

            sampler2D _MainTex;
            sampler2D _Motion;
            float4 _MainTex_TexelSize;
            float4 _Color;
            float _Fps, _UseTime, _Loop, _Frame, _Blending, _Columns, _Rows, _FrameCount, _Strength, _MotionLoops;

            struct Attributes {{ float4 positionOS : POSITION; float2 uv : TEXCOORD0; }};
            struct Varyings {{ float4 positionCS : SV_POSITION; float2 uv : TEXCOORD0; }};

            Varyings vert(Attributes v)
            {{
                Varyings o;
                o.positionCS = UnityObjectToClipPos(v.positionOS);
                o.uv = v.uv;
                return o;
            }}

            // Where `local` (0 to 1 across a frame, from its top left) is in the sheet for
            // frame f, half a texel inside the cell; Unity's textures start at the bottom.
            float2 InSheet(float f, float2 local)
            {{
                float2 grid = float2(_Columns, _Rows);
                float2 cell = float2(fmod(f, _Columns), floor(f / _Columns));
                float2 halfTexel = 0.5 * _MainTex_TexelSize.xy * grid;
                float2 p = (cell + clamp(local, halfTexel, 1.0 - halfTexel)) / grid;
                return float2(p.x, 1.0 - p.y);
            }}

            float4 Premultiplied(float4 c) {{ return float4(c.rgb * c.a, c.a); }}

            float4 frag(Varyings i) : SV_Target
            {{
                float2 local = float2(i.uv.x, 1.0 - i.uv.y);
                float at = _UseTime > 0.5 ? _Time.y * _Fps : _Frame;
                at = _Loop > 0.5 ? fmod(at, _FrameCount) : clamp(at, 0.0, _FrameCount - 1.0);
                float f0 = min(floor(at), _FrameCount - 1.0);
                float f1 = _Loop > 0.5 ? fmod(f0 + 1.0, _FrameCount) : min(f0 + 1.0, _FrameCount - 1.0);
                float t = frac(at);
                float4 c;
                if (_Blending < 0.5)
                {{
                    c = Premultiplied(tex2D(_MainTex, InSheet(f0, local)));
                }}
                else
                {{
                    float2 shift = 0.0;
                    if (_Blending > 1.5 && (f1 > f0 || _MotionLoops > 0.5))
                    {{
                        // Red is across, green down the frame, as stored.
                        shift = (tex2D(_Motion, InSheet(f0, local)).rg - 0.5) * 2.0 * _Strength;
                    }}
                    float4 a = Premultiplied(tex2D(_MainTex, InSheet(f0, local - shift * t)));
                    float4 b = Premultiplied(tex2D(_MainTex, InSheet(f1, local + shift * (1.0 - t))));
                    c = lerp(a, b, t);
                }}
                return c * _Color * _Color.a;
            }}
            ENDCG
        }}
    }}
}}
",
        cw = sheet.cell[0],
        ch = sheet.cell[1],
        columns = sheet.columns,
        rows = sheet.rows,
        strength = (motion.strength as f64 * 1000.0).round() / 1000.0,
        loops = motion.looping as u8,
    )
}

fn unity_flipbook_material(stem: &str, shader: &str, sheet_guid: &str, motion_guid: &str, sheet: &Sheet, count: usize, fps: f32, motion: &Motion) -> String {
    let texture = |prop: &str, guid: &str| format!("    - {prop}:\n        m_Texture: {{fileID: 2800000, guid: {guid}, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n");
    format!(
        "%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!21 &2100000
Material:
  serializedVersion: 8
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_Name: {stem}_flipbook
  m_Shader: {{fileID: 4800000, guid: {shader}, type: 3}}
  m_Parent: {{fileID: 0}}
  m_ModifiedSerializedProperties: 0
  m_ValidKeywords: []
  m_InvalidKeywords: []
  m_LightmapFlags: 4
  m_EnableInstancingVariants: 0
  m_DoubleSidedGI: 0
  m_CustomRenderQueue: -1
  stringTagMap: {{}}
  disabledShaderPasses: []
  m_LockedProperties:
  m_SavedProperties:
    serializedVersion: 3
    m_TexEnvs:
{main}{motion_tex}    m_Ints: []
    m_Floats:
    - _Blending: 2
    - _Columns: {columns}
    - _Fps: {fps}
    - _Frame: 0
    - _FrameCount: {count}
    - _Loop: 1
    - _MotionLoops: {loops}
    - _Rows: {rows}
    - _Strength: {strength}
    - _UseTime: 1
    m_Colors:
    - _Color: {{r: 1, g: 1, b: 1, a: 1}}
  m_BuildTextureStacks: []
  m_AllowLocking: 1
",
        main = texture("_MainTex", sheet_guid),
        motion_tex = texture("_Motion", motion_guid),
        columns = sheet.columns,
        rows = sheet.rows,
        strength = (motion.strength as f64 * 1000.0).round() / 1000.0,
        loops = motion.looping as u8,
    )
}

// Import settings for name_motion.png: numbers, not colour (no sRGB), uncompressed, no
// mipmaps (they'd blend neighbouring cells' motion), filtered, clamped, never shrunk.
pub fn unity_motion_meta(guid: &str, largest: u32) -> String {
    unity_data_meta(guid, largest, false)
}

// The same for name_normal.png, imported as a normal map.
pub fn unity_normal_meta(guid: &str, largest: u32) -> String {
    unity_data_meta(guid, largest, true)
}

fn unity_data_meta(guid: &str, largest: u32, normal_map: bool) -> String {
    let size = largest.next_power_of_two().clamp(32, MAX_SIDE);
    let texture_type = if normal_map { 1 } else { 0 };
    format!(
        "fileFormatVersion: 2
guid: {guid}
TextureImporter:
  internalIDToNameTable: []
  externalObjects: {{}}
  serializedVersion: 13
  mipmaps:
    mipMapMode: 0
    enableMipMap: 0
    sRGBTexture: 0
    linearTexture: 0
    fadeOut: 0
    borderMipMap: 0
    mipMapsPreserveCoverage: 0
    alphaTestReferenceValue: 0.5
    mipMapFadeDistanceStart: 1
    mipMapFadeDistanceEnd: 3
  bumpmap:
    convertToNormalMap: 0
    externalNormalMap: 0
    heightScale: 0.25
    normalMapFilter: 0
    flipGreenChannel: 0
  isReadable: 0
  streamingMipmaps: 0
  streamingMipmapsPriority: 0
  vTOnly: 0
  ignoreMipmapLimit: 0
  grayScaleToAlpha: 0
  generateCubemap: 6
  cubemapConvolution: 0
  seamlessCubemap: 0
  textureFormat: 1
  maxTextureSize: {size}
  textureSettings:
    serializedVersion: 2
    filterMode: 1
    aniso: 1
    mipBias: 0
    wrapU: 1
    wrapV: 1
    wrapW: 1
  nPOTScale: 0
  lightmap: 0
  compressionQuality: 50
  spriteMode: 0
  spriteExtrude: 1
  spriteMeshType: 1
  alignment: 0
  spritePivot: {{x: 0.5, y: 0.5}}
  spritePixelsToUnits: 100
  spriteBorder: {{x: 0, y: 0, z: 0, w: 0}}
  spriteGenerateFallbackPhysicsShape: 1
  alphaUsage: 0
  alphaIsTransparency: 0
  spriteTessellationDetail: -1
  textureType: {texture_type}
  textureShape: 1
  singleChannelComponent: 0
  flipbookRows: 1
  flipbookColumns: 1
  maxTextureSizeSet: 0
  compressionQualitySet: 0
  textureFormatSet: 0
  ignorePngGamma: 0
  applyGammaDecoding: 0
  swizzle: 50462976
  cookieLightType: 0
  platformSettings:
  - serializedVersion: 4
    buildTarget: DefaultTexturePlatform
    maxTextureSize: {size}
    resizeAlgorithm: 0
    textureFormat: 4
    textureCompression: 0
    compressionQuality: 50
    crunchedCompression: 0
    allowsAlphaSplitting: 0
    overridden: 1
    ignorePlatformSupport: 0
    androidETC2FallbackOverride: 0
    forceMaximumCompressionQuality_BC6H_BC7: 0
  spriteSheet:
    serializedVersion: 2
    sprites: []
    outline: []
    physicsShape: []
    bones: []
    spriteID: 
    internalID: 0
    vertices: []
    indices: 
    edges: []
    weights: []
    secondaryTextures: []
    nameFileIdTable: {{}}
  mipmapLimitGroupName: 
  pSDRemoveMatte: 0
  userData: 
  assetBundleName: 
  assetBundleVariant: 
"
    )
}

// Writes the engines' smooth-playback files for a sheet saved at `png` with `motion`.
fn write_flipbook_engines(output: &Output, png: &Path, sheet: &Sheet, count: usize, source_size: [u32; 2], motion: &Motion, normal_map: Option<&str>) -> Result<(), String> {
    let stem = stem_of(png);
    let motion_png = motion_path(png);
    if output.godot {
        let [shader, scene] = godot_flipbook_paths(png);
        write_beside(&shader, &godot_flipbook_shader(sheet, count, output.fps, motion, normal_map.is_some()))?;
        write_beside(&with_suffix(&motion_png, ".import"), &crate::vat::godot_texture_import())?;
        write_beside(&scene, &godot_flipbook_scene(sheet, png, source_size, motion, normal_map))?;
    }
    if output.unity {
        let [shader, material] = unity_flipbook_paths(png);
        let guid = |p: &Path| crate::vat::unity_guid(&unity_meta_path(p), &stem);
        let (shader_guid, material_guid, motion_guid) = (guid(&shader), guid(&material), guid(&motion_png));
        let sheet_guid = std::fs::read_to_string(unity_meta_path(png)).ok().and_then(|t| meta_guid(&t)).ok_or("the sheet's Unity import settings are missing")?;
        let shader_meta = format!("fileFormatVersion: 2\nguid: {shader_guid}\nShaderImporter:\n  externalObjects: {{}}\n  defaultTextures: []\n  nonModifiableTextures: []\n  preprocessorOverride: 0\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n");
        let material_meta = format!("fileFormatVersion: 2\nguid: {material_guid}\nNativeFormatImporter:\n  externalObjects: {{}}\n  mainObjectFileID: 2100000\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n");
        let largest = motion.sheet.image.width().max(motion.sheet.image.height());
        write_beside(&unity_meta_path(&motion_png), &unity_motion_meta(&motion_guid, largest))?;
        write_beside(&shader, &unity_flipbook_shader(&stem, sheet, count, output.fps, motion))?;
        write_beside(&unity_meta_path(&shader), &shader_meta)?;
        write_beside(&material, &unity_flipbook_material(&stem, &shader_guid, &sheet_guid, &motion_guid, sheet, count, output.fps, motion))?;
        write_beside(&unity_meta_path(&material), &material_meta)?;
    }
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

// name.pfx.json beside name.png: the extension bevy_procedural_fx loads.
pub fn json_path(png: &Path) -> PathBuf {
    png.with_extension("pfx.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 5 frames of 10×20 in 3 columns: a 30×40 sheet.
    fn sheet() -> Sheet {
        Sheet { image: image::RgbaImage::new(30, 40), cell: [10, 20], columns: 3, rows: 2, offset: [0, 0] }
    }

    #[test]
    fn godot_sprite_frames_list_every_frame_from_the_top_left() {
        let text = godot_sprite_frames(&sheet(), "fire.png", 5, 12.0);
        assert!(text.starts_with("[gd_resource type=\"SpriteFrames\" load_steps=7 format=3]"));
        assert!(text.contains("path=\"fire.png\""));
        assert!(text.contains("region = Rect2(0, 0, 10, 20)"));
        assert!(text.contains("region = Rect2(20, 0, 10, 20)"));
        assert!(text.contains("region = Rect2(10, 20, 10, 20)"), "the fifth frame is the second of row two");
        assert!(!text.contains("AtlasTexture_5"), "only the frames there are");
        assert!(text.contains("\"speed\": 12.0") && text.contains("\"loop\": true"));
        assert_eq!(text.matches("SubResource(\"AtlasTexture_").count(), 5);
    }

    #[test]
    fn unity_meta_slices_from_the_bottom_left_at_full_size() {
        let text = unity_meta(&sheet(), "fire.png", 5, "0123456789abcdef0123456789abcdef", None);
        assert!(text.starts_with("fileFormatVersion: 2\nguid: 0123456789abcdef0123456789abcdef\n"));
        assert!(text.contains("spriteMode: 2") && text.contains("textureType: 8"));
        // The first frame is at the top: y = 40 - 20.
        let first = text.split("name: fire_0\n").nth(1).unwrap();
        assert!(first.contains("x: 0\n        y: 20\n        width: 10\n        height: 20"));
        let fifth = text.split("name: fire_4\n").nth(1).unwrap();
        assert!(fifth.contains("x: 10\n        y: 0\n"));
        assert!(!text.contains("fire_5"));
        // Never shrunk: the size limit is at least the sheet's.
        assert!(text.contains("maxTextureSize: 64"));
        // Each sprite has its own ids, and the table names them all.
        let ids: std::collections::HashSet<&str> = text.lines().filter_map(|l| l.trim().strip_prefix("internalID: ")).filter(|id| *id != "0").collect();
        assert_eq!(ids.len(), 5);
        assert_eq!(text.split("nameFileIdTable:").nth(1).unwrap().lines().filter(|l| l.trim().starts_with("fire_")).count(), 5);
        assert_eq!(meta_guid(&text).as_deref(), Some("0123456789abcdef0123456789abcdef"));
        // YAML: indentation in steps of two, no tabs.
        assert!(text.lines().all(|l| !l.contains('\t') && (l.len() - l.trim_start().len()) % 2 == 0));
    }

    #[test]
    fn filled_grids_have_no_empty_cells() {
        // 24 square frames: 6×4 rather than 5×5 with one empty.
        assert_eq!(filled_columns(24, [64, 64], 0), 6);
        assert_eq!(filled_columns(16, [64, 64], 0), 4);
        // Tall frames want more across.
        assert_eq!(filled_columns(12, [32, 128], 0), 6);
        // A prime count can only be one row.
        assert_eq!(filled_columns(7, [64, 64], 0), 7);
        // A column count asked for is the nearest divisor to it.
        assert_eq!(filled_columns(24, [64, 64], 5), 6);
        assert_eq!(filled_columns(24, [64, 64], 3), 3);
        for count in 1..60 {
            assert_eq!(count % filled_columns(count, [64, 48], 0) as usize, 0);
        }
        let material = godot_particle_material(&sheet());
        assert!(material.contains("particles_animation = true") && material.contains("particles_anim_h_frames = 3") && material.contains("particles_anim_v_frames = 2"));
        assert_eq!(godot_particles_path(Path::new("/fx/fire.png")), PathBuf::from("/fx/fire_particles.tres"));
    }

    #[test]
    fn exports_keep_their_unity_guid() {
        let dir = std::env::temp_dir().join(format!("pfx_engines_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let frames = vec![egui::ColorImage::filled([8, 8], egui::Color32::WHITE); 3];
        let output = Output { path: dir.join("puff.png"), columns: 0, trim: false, fps: 24.0, live: false, blend: 0, godot: true, unity: true, fill_grid: true, motion_vectors: false, normal_map: false };
        let (png, _) = save(&frames, &output).unwrap();
        let guid = |p: &Path| meta_guid(&std::fs::read_to_string(unity_meta_path(p)).unwrap()).unwrap();
        let first = guid(&png);
        save(&frames, &output).unwrap();
        assert_eq!(guid(&png), first, "Unity keeps the sprites' links");
        assert!(godot_path(&png).is_file() && godot_particles_path(&png).is_file());
        // 3 frames fill a 3×1 grid.
        assert!(std::fs::read_to_string(godot_particles_path(&png)).unwrap().contains("particles_anim_h_frames = 3"));
        assert_eq!(unity_meta_path(&png).file_name().unwrap(), "puff.png.meta");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let meta = metadata(&sheet, "fx.png", 5, 24.0, [20, 10], None, None);
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
    fn effects_that_run_off_the_frame_are_noticed() {
        assert!(edges_touched(&[frame(8, 6, 3, 3)]).is_empty());
        assert_eq!(edges_touched(&[frame(8, 6, 3, 0), frame(8, 6, 7, 2)]), ["top", "right"]);
        let mut faint = frame(8, 6, 3, 3);
        faint.pixels[5] = egui::Color32::from_white_alpha(10);
        assert!(edges_touched(&[faint]).is_empty(), "a faint wisp doesn't count");
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
        let output = Output { path: dir.join("spark.png"), columns: 0, trim: false, fps: 12.0, live: false, blend: 0, godot: false, unity: false, fill_grid: false, motion_vectors: false, normal_map: false };
        let (png, _) = save(&frames, &output).unwrap();
        assert_eq!(image::open(&png).unwrap().to_rgba8().dimensions(), (16, 16));
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("spark.pfx.json")).unwrap()).unwrap();
        assert_eq!((meta["image"].as_str(), meta["columns"].as_u64(), meta["fps"].as_f64()), (Some("spark.png"), Some(2), Some(12.0)));
        // Only the two files are left: nothing half-written.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn motion_vectors_are_packed_like_the_sheet() {
        let dir = std::env::temp_dir().join(format!("pfx_motion_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A soft dot moving right 3 pixels a frame, trimmed to what it covers.
        let dot = |cx: f32| {
            let pixels = (0..64 * 32)
                .map(|i| {
                    let (x, y) = ((i % 64) as f32 + 0.5, (i / 64) as f32 + 0.5);
                    let a = ((-((x - cx).powi(2) + (y - 16.0).powi(2)) / 20.0).exp() * 255.0) as u8;
                    egui::Color32::from_rgba_premultiplied(a, a, a, a)
                })
                .collect();
            egui::ColorImage { size: [64, 32], source_size: egui::vec2(64.0, 32.0), pixels }
        };
        let frames: Vec<_> = (0..5).map(|i| dot(20.0 + 3.0 * i as f32)).collect();
        let output = Output { path: dir.join("dot.png"), columns: 0, trim: true, fps: 12.0, live: false, blend: 0, godot: false, unity: false, fill_grid: false, motion_vectors: true, normal_map: false };
        let (png, sheet) = save(&frames, &output).unwrap();
        let motion = image::open(motion_path(&png)).unwrap().to_rgba8();
        assert_eq!(motion.dimensions(), sheet.image.dimensions(), "the same grid of the same cells");
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json_path(&png)).unwrap()).unwrap();
        let mv = &meta["motion_vectors"];
        assert_eq!(mv["image"], "dot_motion.png");
        assert_eq!(mv["looping"], false, "a dot moving one way isn't a loop");
        // At the dot's centre in the first cell, it moves about 3 pixels right and none down.
        let strength = mv["strength"].as_f64().unwrap() as f32;
        let [cx, cy] = [20 - sheet.offset[0], 16 - sheet.offset[1]];
        let px = motion.get_pixel(cx, cy).0;
        let shift = |byte: u8, size: u32| (byte as f32 / 255.0 - 0.5) * 2.0 * strength * size as f32;
        let (u, v) = (shift(px[0], sheet.cell[0]), shift(px[1], sheet.cell[1]));
        assert!((u - 3.0).abs() < 0.6 && v.abs() < 0.4, "({u}, {v})");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engines_get_a_flipbook_that_uses_the_motion_vectors() {
        let dir = std::env::temp_dir().join(format!("pfx_flipbook_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dot = |cx: usize| {
            let mut image = egui::ColorImage::new([32, 16], vec![egui::Color32::TRANSPARENT; 32 * 16]);
            for y in 6..10 {
                for x in cx..cx + 4 {
                    image.pixels[y * 32 + x] = egui::Color32::WHITE;
                }
            }
            image
        };
        let frames: Vec<_> = (0..4).map(|i| dot(4 + 5 * i)).collect();
        let output = Output { path: dir.join("puff.png"), columns: 0, trim: false, fps: 12.0, live: false, blend: 0, godot: true, unity: true, fill_grid: false, motion_vectors: true, normal_map: false };
        let (png, sheet) = save(&frames, &output).unwrap();
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        let [gd_shader, gd_scene] = godot_flipbook_paths(&png);
        let (shader, scene) = (read(&gd_shader), read(&gd_scene));
        assert!(shader.starts_with("// ProceduralFX Studio flipbook") && shader.contains("shader_type canvas_item;"));
        assert!(shader.contains(&format!("uniform int columns = {};", sheet.columns)) && shader.contains("uniform int frame_count = 4;") && shader.contains("uniform float fps = 12.000;"));
        assert!(scene.contains("path=\"puff.png\"") && scene.contains("path=\"puff_motion.png\"") && scene.contains("path=\"puff_flipbook.gdshader\""));
        assert!(scene.contains(&format!("hframes = {}", sheet.columns)) && scene.contains(&format!("vframes = {}", sheet.rows)));
        assert!(read(&dir.join("puff_motion.png.import")).contains("compress/mode=0"), "lossless, so the numbers survive");
        let [u_shader, u_material] = unity_flipbook_paths(&png);
        let guid = |p: &Path| meta_guid(&read(&unity_meta_path(p))).unwrap();
        let material = read(&u_material);
        assert!(material.contains(&format!("m_Shader: {{fileID: 4800000, guid: {}, type: 3}}", guid(&u_shader))));
        assert!(material.contains(&format!("guid: {}, type: 3}}", guid(&png))), "the material uses the sheet");
        assert!(material.contains(&format!("guid: {}, type: 3}}", guid(&motion_path(&png)))), "and its motion vectors");
        let motion_meta = read(&unity_meta_path(&motion_path(&png)));
        assert!(motion_meta.contains("sRGBTexture: 0") && motion_meta.contains("enableMipMap: 0") && motion_meta.contains("textureCompression: 0"));
        // Exporting again keeps every Unity link.
        let before = [guid(&u_shader), guid(&u_material), guid(&motion_path(&png))];
        save(&frames, &output).unwrap();
        assert_eq!([guid(&u_shader), guid(&u_material), guid(&motion_path(&png))], before);
        // Without motion vectors, none of it is written.
        let plain = Output { path: dir.join("plain.png"), motion_vectors: false, ..output };
        let (plain_png, _) = save(&frames, &plain).unwrap();
        assert!(!motion_path(&plain_png).exists() && !godot_flipbook_paths(&plain_png)[0].exists() && !unity_flipbook_paths(&plain_png)[0].exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normal_maps_come_with_what_each_engine_needs_to_light_them() {
        let dir = std::env::temp_dir().join(format!("pfx_normals_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let blob = |cx: usize| {
            let mut image = egui::ColorImage::new([40, 24], vec![egui::Color32::TRANSPARENT; 40 * 24]);
            for y in 6..18 {
                for x in cx..cx + 12 {
                    image.pixels[y * 40 + x] = egui::Color32::WHITE;
                }
            }
            image
        };
        let frames: Vec<_> = (0..3).map(|i| blob(6 + 4 * i)).collect();
        let output = Output { path: dir.join("puff.png"), columns: 0, trim: true, fps: 12.0, live: false, blend: 0, godot: true, unity: true, fill_grid: false, motion_vectors: true, normal_map: true };
        let (png, sheet) = save(&frames, &output).unwrap();
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        let normals = image::open(normal_path(&png)).unwrap().to_rgba8();
        assert_eq!(normals.dimensions(), sheet.image.dimensions(), "laid out like the sheet");
        let meta: serde_json::Value = serde_json::from_str(&read(&json_path(&png))).unwrap();
        assert_eq!(meta["normal_map"]["image"], "puff_normal.png");
        // Godot: a material for the AnimatedSprite2D, and the flipbook passes the normals on.
        let [lit_shader, lit_material] = godot_lit_paths(&png);
        assert!(read(&lit_shader).contains("NORMAL_MAP = texture(normal_map, UV).rgb;"));
        assert!(read(&lit_material).contains("path=\"puff_normal.png\"") && read(&lit_material).contains("path=\"puff_lit.gdshader\""));
        assert!(read(&dir.join("puff_normal.png.import")).contains("compress/mode=0"));
        let [flip_shader, flip_scene] = godot_flipbook_paths(&png);
        assert!(read(&flip_shader).contains("NORMAL_MAP = mix(") && read(&flip_scene).contains("shader_parameter/normal_map = ExtResource(\"4_normal\")"));
        // Unity: the sprites carry it as _NormalMap, imported as a normal map.
        let normal_meta = read(&unity_meta_path(&normal_path(&png)));
        let normal_guid = meta_guid(&normal_meta).unwrap();
        assert!(normal_meta.contains("textureType: 1") && normal_meta.contains("sRGBTexture: 0"));
        assert!(read(&unity_meta_path(&png)).contains(&format!("    - name: _NormalMap\n      texture: {{fileID: 2800000, guid: {normal_guid}, type: 3}}")));
        save(&frames, &output).unwrap();
        assert_eq!(meta_guid(&read(&unity_meta_path(&normal_path(&png)))).unwrap(), normal_guid, "re-exports keep the link");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
