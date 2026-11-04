// The app's log: a file on the user's computer (in the settings folder, under logs/) of
// what happened in each session: start-up, the GPU, what the status line said, warnings
// and errors, so that when something goes wrong short of a crash, there's a record.
// Nothing leaves the computer: Help → Copy diagnostics copies the recent lines (with the
// home folder shown as ~) for the user to send if they choose.
//
// Our own messages are kept from Info up; other libraries' (wgpu, eframe) from Warn up.
// The file is started afresh when it passes 1 MB, keeping the previous one beside it.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// The recent lines kept in memory, for Copy diagnostics.
const RECENT: usize = 400;
const MAX_FILE: u64 = 1_000_000;

struct AppLog {
    file: Mutex<Option<std::fs::File>>,
    recent: Mutex<VecDeque<String>>,
    path: Option<PathBuf>,
}

static LOG: std::sync::OnceLock<AppLog> = std::sync::OnceLock::new();

impl log::Log for AppLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let ours = metadata.target().starts_with(env!("CARGO_CRATE_NAME"));
        metadata.level() <= if ours { log::Level::Info } else { log::Level::Warn }
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let target = record.target();
        let source = if target.starts_with(env!("CARGO_CRATE_NAME")) { String::new() } else { format!(" [{}]", target.split("::").next().unwrap_or(target)) };
        let line = format!("{} {:<5}{source} {}", clock(std::time::SystemTime::now()), record.level(), record.args());
        if let Ok(mut file) = self.file.lock() {
            if let Some(file) = file.as_mut() {
                let _ = writeln!(file, "{line}");
            }
        }
        if let Ok(mut recent) = self.recent.lock() {
            if recent.len() >= RECENT {
                recent.pop_front();
            }
            recent.push_back(line);
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            if let Some(file) = file.as_mut() {
                let _ = file.flush();
            }
        }
    }
}

// Days since 1970-01-01 as a civil date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (month <= 2) as i64, month, day)
}

// "2026-11-05 07:41:09" (UTC).
pub fn date_time(at: std::time::SystemTime) -> String {
    let secs = at.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

// "07:41:09.123" (UTC), for each line.
fn clock(at: std::time::SystemTime) -> String {
    let since = at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let s = since.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}.{:03}", s / 3600, s / 60 % 60, s % 60, since.subsec_millis())
}

// Opens the log in `dir` (logs/proceduralfx.log in the settings folder) and starts the
// session. Without a folder (or if it can't be written), it keeps the recent lines only.
pub fn start(dir: Option<&Path>) {
    let path = dir.map(|d| d.join("logs").join("proceduralfx.log"));
    let file = path.as_ref().and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE) {
            let _ = std::fs::rename(path, path.with_extension("1.log"));
        }
        std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
    });
    let log = LOG.get_or_init(|| AppLog { file: Mutex::new(file), recent: Mutex::new(VecDeque::new()), path });
    if log::set_logger(log).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    log::info!("── ProceduralFX Studio {} started {} UTC on {} ──", env!("CARGO_PKG_VERSION"), date_time(std::time::SystemTime::now()), system());
}

// The log file, if there is one.
pub fn path() -> Option<PathBuf> {
    LOG.get().and_then(|l| l.path.clone())
}

// The lines logged this session, most recent last (at most RECENT).
pub fn recent() -> Vec<String> {
    LOG.get().and_then(|l| l.recent.lock().ok().map(|r| r.iter().cloned().collect())).unwrap_or_default()
}

// The operating system and its version, as well as can be told without asking much.
pub fn system() -> String {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let version = if cfg!(target_os = "macos") {
        std::process::Command::new("sw_vers").arg("-productVersion").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned())))
    } else if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "ver"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    } else {
        None
    };
    match version.filter(|v| !v.is_empty()) {
        Some(v) => format!("{os} {v} ({arch})"),
        None => format!("{os} ({arch})"),
    }
}

// Status-line messages worth a line in the log: not the ones that change every frame.
pub fn worth_logging(status: &str) -> bool {
    let status = status.trim();
    !status.is_empty()
        && !status.starts_with("Frame from cache")
        && !status.contains("(Esc cancels)")
        && !status.ends_with("press Escape to cancel")
        && !status.starts_with("frame ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times_are_readable() {
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_793_864_469_123);
        assert_eq!(date_time(at), "2026-11-05 07:41:09");
        assert_eq!(clock(at), "07:41:09.123");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn busy_status_messages_are_left_out() {
        assert!(worth_logging("Sprite sheet: 24 frames in 6×4 cells, saved to ~/fx/fire.png"));
        assert!(!worth_logging("Sprite sheet: frame 3 of 24 (Esc cancels)"));
        assert!(!worth_logging("Simulating Pyro… press Escape to cancel"));
        assert!(!worth_logging("Frame from cache · 12 cached frames"));
        assert!(!worth_logging("  "));
    }

    #[test]
    fn lines_go_to_the_file_and_the_recent_list() {
        let dir = std::env::temp_dir().join(format!("pfx_log_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        start(Some(&dir));
        log::info!("exported fire.png");
        log::warn!(target: "wgpu_core", "a warning from a library");
        log::info!(target: "wgpu_core", "library chatter");
        log::logger().flush();
        // The first test to start the log decides its file; this one checks the list, and
        // the file if it was this one.
        let recent = recent();
        assert!(recent.iter().any(|l| l.ends_with("exported fire.png") && l.contains("INFO")), "{recent:?}");
        assert!(recent.iter().any(|l| l.contains("WARN  [wgpu_core] a warning from a library")));
        assert!(!recent.iter().any(|l| l.contains("library chatter")), "other libraries' info is left out");
        if path().is_some_and(|p| p.starts_with(&dir)) {
            let text = std::fs::read_to_string(dir.join("logs/proceduralfx.log")).unwrap();
            assert!(text.contains("started") && text.contains("exported fire.png"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
