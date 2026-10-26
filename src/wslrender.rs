// Rendering on Windows: MoonRay runs in a Linux container through pfx-render.exe
// (packaging/windows/render), which ships next to the app. It's given "moonray" or
// "hd_render" and their arguments, and maps Windows paths in the arguments into the
// container itself. The paths inside the scene files the app writes (textures, HDR
// maps) are rewritten here with the same rule: C:\Users\me\a.png is
// /mnt/host/c/Users/me/a.png in the container.

use std::path::{Path, PathBuf};

pub const HELPER: &str = "pfx-render.exe";

// The helper, on Windows: next to the app, or PFX_RENDER_HELPER.
pub fn helper() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    if let Some(path) = std::env::var_os("PFX_RENDER_HELPER").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(path);
    }
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name(HELPER)).filter(|p| p.is_file())
}

// Whether a render command runs through the helper.
pub fn is_helper(executable: &str) -> bool {
    // Either kind of slash, so a Windows path reads the same on every system.
    let name = executable.trim().rsplit(['\\', '/']).next().unwrap_or("");
    name.eq_ignore_ascii_case(HELPER)
}

// A Windows path's place in the container, or None if it isn't a drive path.
pub fn container_path(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let (drive, colon, slash) = (chars.next()?, chars.next()?, chars.next()?);
    if !drive.is_ascii_alphabetic() || colon != ':' || !(slash == '\\' || slash == '/') {
        return None;
    }
    let rest = path[2..].replace("\\\\", "/").replace('\\', "/");
    Some(format!("/mnt/host/{}{rest}", drive.to_ascii_lowercase()))
}

// A scene file's text with the Windows paths in its strings (and USD's @asset@
// paths) moved into the container. Lua strings in .rdla files escape backslashes.
pub fn container_scene(text: &str) -> String {
    let pattern = regex::Regex::new(r#"(["'@])([A-Za-z]:(?:\\\\|\\|/)[^"'@\r\n]*)"#).expect("valid pattern");
    pattern
        .replace_all(text, |caps: &regex::Captures| match container_path(&caps[2]) {
            Some(path) => format!("{}{path}", &caps[1]),
            None => caps[0].to_string(),
        })
        .into_owned()
}

// Rewrites a scene file in place for the container.
pub fn prepare_scene(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("couldn't read the scene {}: {e}", path.display()))?;
    let moved = container_scene(&text);
    if moved != text {
        std::fs::write(path, moved).map_err(|e| format!("couldn't write the scene {}: {e}", path.display()))?;
    }
    Ok(())
}

// What the helper's last progress line in a render log says, for the status bar:
// downloading the renderer (with a fraction), or another step.
pub fn progress(log: &str) -> Option<String> {
    let line = log.lines().rev().find(|l| l.starts_with("[pfx-render] "))?;
    let rest = line.trim_start_matches("[pfx-render] ");
    if let Some(numbers) = rest.strip_prefix("download ") {
        let mut parts = numbers.split_whitespace().filter_map(|n| n.parse::<f64>().ok());
        let (done, total) = (parts.next()?, parts.next()?);
        let gb = |b: f64| b / (1u64 << 30) as f64;
        return Some(if total > 0.0 {
            format!("Downloading MoonRay (once): {:.0}% of {:.1} GB", done / total * 100.0, gb(total))
        } else {
            "Downloading MoonRay (once)…".to_string()
        });
    }
    rest.strip_prefix("status ").map(|s| {
        let mut s = s.to_string();
        if let Some(first) = s.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        s
    })
}

// What a failed render's log means for the user, when the helper explains it.
pub fn failure_hint(log: &str) -> Option<&'static str> {
    if log.contains("WSL containers aren't set up") || log.contains("missing: ") {
        Some("Rendering on Windows needs WSL: choose Render → Set Up Rendering on Windows…")
    } else if log.contains("couldn't download the MoonRay image") {
        Some("MoonRay couldn't be downloaded: check your internet connection, then render again")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_move_into_the_container() {
        assert_eq!(container_path(r"C:\Users\me\a.png").as_deref(), Some("/mnt/host/c/Users/me/a.png"));
        assert_eq!(container_path("D:/tex/b.exr").as_deref(), Some("/mnt/host/d/tex/b.exr"));
        assert_eq!(container_path("/tmp/x"), None);
        assert_eq!(container_path("C:relative"), None);
        // An .rdla file: Lua strings with escaped backslashes; other strings stay.
        let rdla = "ImageMap(\"/m/tex\") {\n    [\"texture\"] = \"C:\\\\Users\\\\me\\\\wood.png\",\n    [\"name\"] = \"wood\",\n}\nEnvLight(\"/env\") { [\"texture\"] = 'E:\\\\hdri\\\\sky.exr' }";
        let moved = container_scene(rdla);
        assert!(moved.contains("\"/mnt/host/c/Users/me/wood.png\""), "{moved}");
        assert!(moved.contains("'/mnt/host/e/hdri/sky.exr'"), "{moved}");
        assert!(moved.contains("ImageMap(\"/m/tex\")") && moved.contains("\"wood\""));
        // USD asset paths.
        let usda = "asset inputs:file = @C:\\Users\\me\\wood.png@\nstring name = \"plain\"";
        assert!(container_scene(usda).contains("@/mnt/host/c/Users/me/wood.png@"));
        // Nothing to change: the same text.
        assert_eq!(container_scene("Camera(\"/cam\") {}"), "Camera(\"/cam\") {}");
    }

    #[test]
    fn helper_progress_reads_for_the_status_bar() {
        let log = "[pfx-render] status starting the renderer\n[pfx-render] download 536870912 2147483648\n";
        assert_eq!(progress(log).as_deref(), Some("Downloading MoonRay (once): 25% of 2.0 GB"));
        assert_eq!(progress("[pfx-render] status rendering\nRendering frame 1").as_deref(), Some("Rendering"));
        assert_eq!(progress("plain moonray output"), None);
        assert!(failure_hint("[pfx-render] error WSL containers aren't set up: run pfx-render.exe --install").is_some());
        assert!(is_helper(r"C:\Program Files\ProceduralFX Studio\pfx-render.exe") && is_helper("PFX-RENDER.EXE") && !is_helper("/usr/bin/moonray"));
        assert!(helper().is_none() || cfg!(windows));
    }
}
