// Command-line rendering, for build scripts and render farms: cooks a scene and renders
// it with the real-time renderer, without opening a window, then exits.
//
//   proceduralfx_studio render scene.pfs [--out DIR] [--frames START END [STEP]] ...
//   proceduralfx_studio sprite-sheet scene.pfs sheet.png [--every N] [--columns N] ...
//   proceduralfx_studio animation scene.pfs fire.gif (or fire.mp4) ...
//
// Frames are numbered as the timeline numbers them (frame 1 is time 0). Exit codes:
// 0 done, 1 the render failed, 2 the command was wrong.

use super::*;

pub const USAGE: &str = "\
Renders a scene without opening a window.

  proceduralfx_studio render <scene.pfs> [options]
      PNG frames, named render.0001.png, render.0002.png...
      --out <folder>               where the frames go (default: <scene>_render beside the scene)

  proceduralfx_studio sprite-sheet <scene.pfs> <sheet.png> [options]
      A sprite sheet PNG and its .pfx.json, as File → Export Sprite Sheet makes
      --every <n>                  render every nth frame (default 1)
      --columns <n>                cells across (default: as square as possible)
      --no-trim                    keep the full frame, empty space included
      --seamless [--blend <n>]     fade the end into the start (default blend 8)
      --godot                      also write sheet.tres, a Godot 4 SpriteFrames resource
      --unity                      also write sheet.png.meta, slicing it into sprites in Unity
      --fill-grid                  no empty cells, for particle systems (and with --godot,
                                   also sheet_particles.tres, a GPUParticles2D material)

  proceduralfx_studio animation <scene.pfs> <fire.gif | fire.mp4> [options]
      A looping GIF, or an MP4 movie (needs FFmpeg), over a dark background:
      for posting an effect online

  All of them:
      --frames <start> <end> [<step>]   a range of frames (default: the scene's timeline)
      --frame <n>                  one frame
      --size <width>x<height>      the frame size in pixels (default: 1280x720 for
                                   renders, 256x256 for sprite sheets, 480x480
                                   for animations)
      --camera <name>              the Camera node to render through (default: the
                                   scene's first camera, or a view of everything)

  Frame 1 is the start of the timeline (time 0), at 24 frames a second.
";

#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    Frames { dir: Option<PathBuf> },
    // A GIF or MP4, by the file's extension.
    Animation { file: PathBuf },
    SpriteSheet { png: PathBuf, every: u32, columns: u32, trim: bool, seamless: bool, blend: u32, godot: bool, unity: bool, fill_grid: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub scene: PathBuf,
    pub output: Output,
    // First and last frame, and the step, in frame numbers.
    pub frames: Option<(f32, f32, f32)>,
    pub size: Option<[u32; 2]>,
    pub camera: Option<String>,
}

// A render command, if the arguments are one (None: start the app as usual).
pub fn parse(args: &[String]) -> Option<Result<Job, String>> {
    let command = args.get(1)?.as_str();
    if !matches!(command, "render" | "sprite-sheet" | "animation") {
        return None;
    }
    Some(parse_job(command, &args[2..]))
}

fn parse_job(command: &str, args: &[String]) -> Result<Job, String> {
    let mut positional = Vec::new();
    let (mut out, mut frames, mut size, mut camera) = (None, None, None, None);
    let (mut every, mut columns, mut trim, mut seamless, mut blend) = (1, 0, true, false, 8);
    let (mut godot, mut unity, mut fill_grid) = (false, false, false);
    let mut i = 0;
    let value = |i: usize, flag: &str| args.get(i + 1).cloned().ok_or_else(|| format!("{flag} needs a value"));
    let number = |text: &str, flag: &str| text.parse::<f32>().ok().filter(|n| n.is_finite()).ok_or_else(|| format!("{flag}: \"{text}\" isn't a number"));
    let count = |text: &str, flag: &str| text.parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(|| format!("{flag}: \"{text}\" isn't a whole number above 0"));
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--out" => {
                out = Some(PathBuf::from(value(i, arg)?));
                i += 1;
            }
            "--frame" => {
                let n = number(&value(i, arg)?, arg)?;
                frames = Some((n, n, 1.0));
                i += 1;
            }
            "--frames" => {
                let start = number(&value(i, arg)?, arg)?;
                let end = number(&value(i + 1, arg)?, arg)?;
                i += 2;
                // An optional step: the next argument, if it's a number.
                let step = match args.get(i + 1).and_then(|s| s.parse::<f32>().ok()) {
                    Some(step) if step > 0.0 => {
                        i += 1;
                        step
                    }
                    _ => 1.0,
                };
                if end < start {
                    return Err(format!("--frames: the end ({end}) is before the start ({start})"));
                }
                frames = Some((start, end, step));
            }
            "--size" => {
                let text = value(i, arg)?;
                let parsed = text.split_once(['x', 'X']).and_then(|(w, h)| Some([w.parse::<u32>().ok()?, h.parse::<u32>().ok()?]));
                size = Some(parsed.filter(|[w, h]| (8..=16384).contains(w) && (8..=16384).contains(h)).ok_or_else(|| format!("--size: \"{text}\" should be like 1280x720, from 8 to 16384 a side"))?);
                i += 1;
            }
            "--camera" => {
                camera = Some(value(i, arg)?);
                i += 1;
            }
            "--every" => {
                every = count(&value(i, arg)?, arg)?;
                i += 1;
            }
            "--columns" => {
                columns = count(&value(i, arg)?, arg)?;
                i += 1;
            }
            "--blend" => {
                blend = count(&value(i, arg)?, arg)?;
                i += 1;
            }
            "--no-trim" => trim = false,
            "--seamless" => seamless = true,
            "--godot" => godot = true,
            "--unity" => unity = true,
            "--fill-grid" => fill_grid = true,
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => positional.push(PathBuf::from(arg)),
        }
        i += 1;
    }
    let sheet = command == "sprite-sheet";
    let animation = command == "animation";
    let wanted = if sheet || animation { 2 } else { 1 };
    if positional.len() != wanted {
        return Err(match command {
            "sprite-sheet" => "sprite-sheet needs a scene and the sheet to write: sprite-sheet <scene.pfs> <sheet.png>".into(),
            "animation" => "animation needs a scene and the file to write: animation <scene.pfs> <fire.gif>".into(),
            _ => "render needs one scene: render <scene.pfs>".into(),
        });
    }
    if animation && !positional[1].extension().is_some_and(|e| e.eq_ignore_ascii_case("gif") || e.eq_ignore_ascii_case("mp4")) {
        return Err("animation writes a .gif or an .mp4: end the file name with one of them".into());
    }
    let sheet_only = [("--every", every != 1), ("--columns", columns != 0), ("--no-trim", !trim), ("--seamless", seamless), ("--godot", godot), ("--unity", unity), ("--fill-grid", fill_grid)];
    if !sheet {
        if let Some((flag, _)) = sheet_only.iter().find(|(_, set)| *set) {
            return Err(format!("{flag} is for sprite sheets"));
        }
    }
    if (sheet || animation) && out.is_some() {
        return Err(format!("--out is for render; {} goes where its second argument says", if sheet { "a sprite sheet" } else { "an animation" }));
    }
    let scene = positional.remove(0);
    let output = if animation {
        Output::Animation { file: positional.remove(0) }
    } else if sheet {
        Output::SpriteSheet { png: positional.remove(0).with_extension("png"), every, columns, trim, seamless, blend, godot, unity, fill_grid }
    } else {
        Output::Frames { dir: out }
    };
    Ok(Job { scene, output, frames, size, camera })
}

// The folder a render writes to: --out, or <scene>_render beside the scene.
fn frames_dir(job: &Job, dir: &Option<PathBuf>) -> PathBuf {
    dir.clone().unwrap_or_else(|| {
        let stem = job.scene.file_stem().map_or_else(|| "scene".into(), |s| s.to_string_lossy().into_owned());
        job.scene.with_file_name(format!("{stem}_render"))
    })
}

// Runs a render command; Ok says what was written, Err why it couldn't be.
pub fn run(job: &Job, progress: &mut dyn FnMut(usize, usize)) -> Result<String, String> {
    let text = std::fs::read_to_string(&job.scene).map_err(|e| format!("couldn't read {}: {e}", job.scene.display()))?;
    if let Some(problem) = scene_problem(&text) {
        return Err(format!("can't open {}: {problem}", job.scene.display()));
    }
    let gpu = gpu_viewport::headless().ok_or("no GPU was found for the real-time renderer")?;
    let mut app = ProceduralFxStudioApp::default();
    app.gpu_viewport = Some(gpu);
    let unread = unread_nodes(&text, &Graph::deserialize(&text));
    app.use_saved_timeline(&text);
    app.load_state(text);
    app.selected = None;
    let mut notes = Vec::new();
    if unread > 0 {
        notes.push(format!("{unread} node{} couldn't be read and were left out", if unread == 1 { "" } else { "s" }));
    }

    // The camera: the one named, the scene's first, or a view framing everything.
    let cameras: Vec<(NodeId, String)> = {
        let mut ids: Vec<NodeId> = app.graph.nodes.iter().filter(|(_, n)| matches!(n.kind, NodeKind::Camera { .. })).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.into_iter().map(|id| (id, app.graph.nodes[&id].title(id))).collect()
    };
    app.render_camera = match &job.camera {
        Some(name) => match cameras.iter().find(|(_, title)| title == name) {
            Some((id, _)) => RenderCamera::Node(*id),
            None if cameras.is_empty() => return Err(format!("there's no camera called \"{name}\": the scene has no cameras")),
            None => return Err(format!("there's no camera called \"{name}\"; the scene's cameras are {}", cameras.iter().map(|(_, t)| format!("\"{t}\"")).collect::<Vec<_>>().join(", "))),
        },
        None => match cameras.first() {
            Some((id, _)) => RenderCamera::Node(*id),
            None => {
                app.rebuild_graph();
                app.dirty = false;
                app.frame_all();
                notes.push("the scene has no camera, so it's rendered from a view of everything".into());
                RenderCamera::Viewport
            }
        },
    };

    if let Some((start, end, _)) = job.frames {
        app.playback_start = (start - 1.0) / sims::SIM_FPS;
        app.playback_end = (end - 1.0) / sims::SIM_FPS;
        app.time = app.playback_start;
    }
    let started = std::time::SystemTime::now();
    let written = match &job.output {
        Output::Frames { dir } => {
            let dir = frames_dir(job, dir);
            let step = job.frames.map_or(1.0, |f| f.2) / sims::SIM_FPS;
            let times = frame_times(app.playback_start, app.playback_end, step);
            if times.len() > 10_000 {
                return Err(format!("{} frames is too many to render at once; render at most 10000", times.len()));
            }
            let size = job.size.unwrap_or([1280, 720]);
            app.render_camera_in_use();
            app.animate = false;
            let capture = flipbook::Capture { render: Some(size), save_to: Some(dir.clone()), sheet: None, preview: None };
            app.flipbook_recording = Some(flipbook::Recording::new(times, app.time, Some(capture)));
            dir.join("render.0001.png")
        }
        Output::Animation { file } => {
            let step = job.frames.map_or(1.0, |f| f.2) / sims::SIM_FPS;
            let times = frame_times(app.playback_start, app.playback_end, step);
            if times.len() > 1000 {
                return Err(format!("{} frames is too many for an animation; make one of at most 1000", times.len()));
            }
            app.render_camera_in_use();
            app.animate = false;
            let capture = flipbook::Capture { render: Some(job.size.unwrap_or([480, 480])), save_to: None, sheet: None, preview: None };
            app.flipbook_recording = Some(flipbook::Recording::new(times, app.time, Some(capture)));
            file.clone()
        }
        Output::SpriteSheet { png, every, columns, trim, seamless, blend, godot, unity, fill_grid } => {
            let [width, height] = job.size.unwrap_or([256, 256]);
            app.sprite_sheet = spritesheet::Settings { open: false, width, height, every: *every, columns: *columns, trim: *trim, live: false, seamless: *seamless, blend: *blend, godot: *godot, unity: *unity, fill_grid: *fill_grid };
            app.start_sprite_sheet(png.clone(), false);
            png.clone()
        }
    };
    if app.flipbook_recording.is_none() {
        return Err(app.status.clone());
    }
    let mut finished = drive_recording(&mut app, None, progress).unwrap_or_else(|| app.status.clone());
    // An animation's frames end up in the flipbook viewer: save them.
    if let Output::Animation { file } = &job.output {
        let viewer = app.flipbook_viewer.take().ok_or(finished.clone())?;
        let fps = sims::SIM_FPS / job.frames.map_or(1.0, |f| f.2);
        let is_gif = file.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
        if is_gif { flipbook::save_gif(viewer.frames(), file, fps)? } else { flipbook::save_movie(viewer.frames(), file, fps)? }
        finished = format!("Saved a {}-frame {} to {}", viewer.frames().len(), if is_gif { "GIF" } else { "movie" }, file.display());
    }
    let fresh = std::fs::metadata(&written).and_then(|m| m.modified()).is_ok_and(|t| t >= started);
    if !fresh {
        return Err(finished);
    }
    notes.insert(0, finished);
    Ok(notes.join("\n"))
}

// The command-line entry point: Some(exit code) if the arguments were a render command.
pub fn main(args: &[String]) -> Option<i32> {
    if args.get(1).is_some_and(|a| matches!(a.as_str(), "--help" | "-h" | "help")) {
        println!("{USAGE}");
        return Some(0);
    }
    let job = match parse(args)? {
        Ok(job) => job,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return Some(2);
        }
    };
    let mut last = usize::MAX;
    let mut progress = |done: usize, total: usize| {
        if done != last {
            eprint!("\rframe {done} of {total}");
            last = done;
        }
    };
    let result = run(&job, &mut progress);
    eprintln!();
    Some(match result {
        Ok(summary) => {
            println!("{summary}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        std::iter::once("proceduralfx_studio").chain(line.split_whitespace()).map(str::to_owned).collect()
    }

    #[test]
    fn render_commands_are_read() {
        assert_eq!(parse(&args("")), None);
        assert_eq!(parse(&args("--usd-info x.usd")), None, "other flags start the app as before");
        let job = parse(&args("render fire.pfs --frames 1 48 2 --size 640x360 --camera cam1")).unwrap().unwrap();
        assert_eq!(job.scene, PathBuf::from("fire.pfs"));
        assert_eq!(job.output, Output::Frames { dir: None });
        assert_eq!(job.frames, Some((1.0, 48.0, 2.0)));
        assert_eq!(job.size, Some([640, 360]));
        assert_eq!(job.camera.as_deref(), Some("cam1"));
        assert_eq!(frames_dir(&job, &None), PathBuf::from("fire_render"));
        // The step is optional, and other options can follow the range.
        let job = parse(&args("render fire.pfs --frames 1 10 --out frames")).unwrap().unwrap();
        assert_eq!(job.frames, Some((1.0, 10.0, 1.0)));
        assert_eq!(job.output, Output::Frames { dir: Some("frames".into()) });

        let job = parse(&args("sprite-sheet fire.pfs out/fire --every 2 --columns 4 --no-trim --seamless --blend 3 --frame 5")).unwrap().unwrap();
        assert_eq!(job.output, Output::SpriteSheet { png: "out/fire.png".into(), every: 2, columns: 4, trim: false, seamless: true, blend: 3, godot: false, unity: false, fill_grid: false });
        let engines = parse(&args("sprite-sheet fire.pfs fire.png --godot --unity")).unwrap().unwrap();
        assert!(matches!(engines.output, Output::SpriteSheet { godot: true, unity: true, .. }));
        assert!(parse(&args("render fire.pfs --godot")).unwrap().unwrap_err().contains("for sprite sheets"));
        assert_eq!(job.frames, Some((5.0, 5.0, 1.0)));
    }

    #[test]
    fn wrong_commands_say_what_is_wrong() {
        let error = |line: &str| parse(&args(line)).unwrap().unwrap_err();
        assert!(error("render").contains("one scene"));
        assert!(error("sprite-sheet fire.pfs").contains("the sheet to write"));
        assert!(error("render fire.pfs --size 10").contains("1280x720"));
        assert!(error("render fire.pfs --frames 10 1").contains("before the start"));
        assert!(error("render fire.pfs --every 2").contains("for sprite sheets"));
        assert!(error("sprite-sheet a.pfs b.png --out x").contains("--out is for render"));
        assert!(error("render fire.pfs --wat").contains("unknown option"));
        assert!(error("animation fire.pfs fire.png").contains(".gif or an .mp4"));
        assert!(error("animation fire.pfs fire.gif --out x").contains("an animation"));
        assert!(error("animation fire.pfs fire.gif --godot").contains("for sprite sheets"));
        assert!(error("render fire.pfs --frame").contains("needs a value"));
        assert_eq!(main(&args("render")), Some(2));
        assert_eq!(main(&args("--help")), Some(0));
    }

    #[test]
    fn scenes_render_from_the_command_line() {
        if gpu_viewport::headless().is_none() {
            eprintln!("no GPU here; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pfx_cli_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A sphere and a camera looking at it.
        let scene = dir.join("ball.pfs");
        std::fs::write(&scene, "pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj sphere 0.5 12 24\nparent 1 0\ndisplay 0 1\nnext 2\n").unwrap();

        let job = parse(&args(&format!("render {} --frames 1 3 --size 64x48", scene.display()))).unwrap().unwrap();
        let mut seen = Vec::new();
        let summary = run(&job, &mut |done, total| seen.push((done, total))).unwrap();
        assert!(summary.contains("no camera"), "{summary}");
        for n in 1..=3 {
            let frame = image::open(dir.join(format!("ball_render/render.{n:04}.png"))).unwrap();
            assert_eq!((frame.width(), frame.height()), (64, 48));
        }
        assert_eq!(seen.last(), Some(&(3, 3)));
        // Something was drawn: the sphere covers the middle.
        let middle = image::open(dir.join("ball_render/render.0001.png")).unwrap().to_rgba8();
        assert!(middle.get_pixel(32, 24)[3] > 0, "the sphere is in the frame");

        let sheet = dir.join("ball_sheet.png");
        let job = parse(&args(&format!("sprite-sheet {} {} --frames 1 4 --size 32x32", scene.display(), sheet.display()))).unwrap().unwrap();
        run(&job, &mut |_, _| {}).unwrap();
        assert!(sheet.is_file() && spritesheet::json_path(&sheet).is_file());
        assert!(!spritesheet::godot_path(&sheet).exists() && !spritesheet::unity_meta_path(&sheet).exists(), "only when asked");
        let job = parse(&args(&format!("sprite-sheet {} {} --frames 1 4 --size 32x32 --godot --unity", scene.display(), sheet.display()))).unwrap().unwrap();
        run(&job, &mut |_, _| {}).unwrap();
        assert!(std::fs::read_to_string(spritesheet::godot_path(&sheet)).unwrap().contains("AtlasTexture_3"));
        assert!(std::fs::read_to_string(spritesheet::unity_meta_path(&sheet)).unwrap().contains("name: ball_sheet_3"));

        let gif = dir.join("ball.gif");
        let job = parse(&args(&format!("animation {} {} --frames 1 3 --size 32x24", scene.display(), gif.display()))).unwrap().unwrap();
        assert!(run(&job, &mut |_, _| {}).unwrap().contains("3-frame GIF"));
        assert!(std::fs::read(&gif).unwrap().starts_with(b"GIF89a"));

        let missing = parse(&args(&format!("render {} --camera nope", scene.display()))).unwrap().unwrap();
        assert!(run(&missing, &mut |_, _| {}).unwrap_err().contains("no cameras"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
