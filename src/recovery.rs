// Autosave and crash reports, so a crash or a forced quit never loses more than a
// minute or so of work.
//
// Each running copy of the app is a session with its own files in the recovery folder:
//   <id>.alive  locked while it runs; holds the app's version
//   <id>.pfs    the unsaved scene, autosaved while it has changes
//   <id>.path   the scene file it belongs to (empty when untitled)
// A clean quit removes them. The system releases the lock however the app ends, so at
// start-up, files whose .alive file isn't locked belong to a session that crashed or
// was killed: its autosave is offered back, with its crash report if it wrote one.
//
// Crash reports (crashes/crash-<time>-<id>.txt) are written by a panic hook. They stay
// on this computer unless the user chooses to send one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

// How often unsaved changes are written to the autosave.
pub const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);
const MAX_CRASH_REPORTS: usize = 20;

pub struct Session {
    dir: PathBuf,
    id: String,
    // The .alive file, locked for as long as this session runs.
    alive: Option<std::fs::File>,
    last_check: Instant,
    // The fingerprint of what's in the autosave, if there is one.
    saved: Option<u64>,
}

// What a session that didn't quit cleanly left behind.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub id: String,
    pub autosave: Option<PathBuf>,
    // The scene file the autosave belongs to, if it had one.
    pub scene_path: Option<PathBuf>,
    pub when: Option<SystemTime>,
    pub crash_report: Option<PathBuf>,
}

impl Session {
    // Starts this session's files in `dir`, and lists what sessions that are gone left.
    pub fn start(dir: PathBuf) -> (Session, Vec<Leftover>) {
        let _ = std::fs::create_dir_all(&dir);
        let leftovers = leftovers(&dir);
        let id = format!("{}-{}", unix_seconds(SystemTime::now()), std::process::id());
        let mut session = Session { dir, id, alive: None, last_check: Instant::now(), saved: None };
        session.lock();
        set_crash_session(&session.dir, &session.id);
        (session, leftovers)
    }

    fn file(&self, ext: &str) -> PathBuf {
        self.dir.join(format!("{}.{ext}", self.id))
    }

    fn lock(&mut self) {
        use std::io::Write;
        let Ok(mut file) = std::fs::File::create(self.file("alive")) else { return };
        let _ = writeln!(file, "{}", env!("CARGO_PKG_VERSION"));
        if file.try_lock().is_ok() {
            self.alive = Some(file);
        }
    }

    // Whether it's time to check for changes to autosave.
    pub fn due(&self) -> bool {
        self.last_check.elapsed() >= AUTOSAVE_EVERY
    }

    // Called every frame: every AUTOSAVE_EVERY, writes the scene if it has unsaved
    // changes (`scene` is only built then), or removes the autosave once there's
    // nothing unsaved.
    pub fn tick(&mut self, unsaved: bool, scene_path: Option<&Path>, scene: impl FnOnce() -> String) {
        if !self.due() {
            return;
        }
        self.last_check = Instant::now();
        if unsaved {
            let text = scene();
            let fingerprint = fingerprint(&text);
            if self.saved != Some(fingerprint) {
                let path = scene_path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                // The path first, so an autosave never has the wrong scene's path.
                if std::fs::write(self.file("path"), path).is_ok() && write_atomically(&self.file("pfs"), &text).is_ok() {
                    self.saved = Some(fingerprint);
                }
            }
        } else if self.saved.is_some() {
            self.clear_autosave();
        }
    }

    // Called after a save, so the autosave doesn't outlive the changes it holds.
    pub fn clear_autosave(&mut self) {
        let _ = std::fs::remove_file(self.file("pfs"));
        let _ = std::fs::remove_file(self.file("path"));
        self.saved = None;
    }

    // A clean quit: nothing to recover.
    pub fn end(&mut self) {
        self.clear_autosave();
        // Unlocked first: Windows can't delete a file that's open.
        self.alive = None;
        let _ = std::fs::remove_file(self.file("alive"));
    }
}

// Whether a session is still running: its .alive file is locked.
fn running(dir: &Path, id: &str) -> bool {
    let Ok(file) = std::fs::File::open(dir.join(format!("{id}.alive"))) else { return false };
    // Locking it ourselves means nobody holds it; the lock goes with `file`.
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

// The sessions in `dir` that are gone and left an autosave or a crash report, newest
// first.
pub fn leftovers(dir: &Path) -> Vec<Leftover> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?;
            matches!(ext, "alive" | "pfs" | "path").then(|| path.file_stem()?.to_str().map(str::to_owned)).flatten()
        })
        .collect();
    ids.sort();
    ids.dedup();
    let mut out = Vec::new();
    for id in ids {
        if running(dir, &id) {
            continue;
        }
        let autosave = Some(dir.join(format!("{id}.pfs"))).filter(|p| p.is_file());
        let scene_path = std::fs::read_to_string(dir.join(format!("{id}.path")))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let when = autosave.as_ref().and_then(|p| std::fs::metadata(p).ok()?.modified().ok());
        // A session that ended without a panic (killed, power cut) has no report.
        let crash_report = crash_report_of(dir, &id);
        if autosave.is_none() && crash_report.is_none() {
            forget(dir, &id);
            continue;
        }
        out.push(Leftover { id, autosave, scene_path, when, crash_report });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

// Removes a gone session's files (its crash report stays).
pub fn forget(dir: &Path, id: &str) {
    for ext in ["alive", "pfs", "path"] {
        let _ = std::fs::remove_file(dir.join(format!("{id}.{ext}")));
    }
}

pub fn crash_dir(dir: &Path) -> PathBuf {
    dir.join("crashes")
}

fn crash_report_of(dir: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.txt");
    let mut reports: Vec<PathBuf> = std::fs::read_dir(crash_dir(dir))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(&suffix)))
        .collect();
    // The last panic is the one that ended the session.
    reports.sort();
    reports.pop()
}

// ---------------------------------------------------------------------------
// Crash reports
// ---------------------------------------------------------------------------

static CRASH_SESSION: OnceLock<(PathBuf, String)> = OnceLock::new();

fn set_crash_session(dir: &Path, id: &str) {
    let _ = CRASH_SESSION.set((dir.to_owned(), id.to_owned()));
}

// Writes a crash report for every panic, then does what Rust does anyway (prints it).
// Some panics are caught and recovered from; their reports are only shown if the
// session then ends without quitting.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some((dir, id)) = CRASH_SESSION.get() {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(no message)".into());
            let location = info.location().map_or_else(|| "unknown".into(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
            let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
            log::error!("Crashed: {message} at {location} (thread {thread})");
            log::logger().flush();
            let backtrace = std::backtrace::Backtrace::force_capture();
            let report = crash_report_text(SystemTime::now(), &thread, &message, &location, &backtrace.to_string());
            let _ = write_crash_report(&crash_dir(dir), id, &report);
        }
        default(info);
    }));
}

// A crash report as it would be sent: the user's home folder (in paths in the panic
// message) shown as ~, so their account name doesn't leave the computer.
// The user's home folder, as crash reports name it: HOME, or USERPROFILE on Windows.
pub fn home_folder() -> Option<String> {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok()
}

pub fn redacted(report: &str, home: Option<&str>) -> String {
    match home.map(|h| h.trim_end_matches(['/', '\\'])).filter(|h| h.len() > 1) {
        Some(home) => report.replace(home, "~"),
        None => report.to_owned(),
    }
}

// Sends a crash report (already redacted, as the user saw it) and their note to the
// server's /v1/crash-reports.
pub fn send_report(server: &str, report: &str, note: &str) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .timeout_recv_response(Some(std::time::Duration::from_secs(30)))
        .user_agent(format!("ProceduralFxStudio/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let body = serde_json::json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "report": report,
        "note": note,
    });
    match agent.post(&format!("{server}/v1/crash-reports")).send_json(body) {
        Ok(_) => Ok(()),
        Err(ureq::Error::StatusCode(429)) => Err("too many reports were sent from here just now; try again later".into()),
        Err(ureq::Error::StatusCode(code)) => Err(format!("the server answered {code}")),
        Err(e) => Err(format!("couldn't reach the server ({e})")),
    }
}

pub fn crash_report_text(now: SystemTime, thread: &str, message: &str, location: &str, backtrace: &str) -> String {
    format!(
        "ProceduralFX Studio crash report\n\
         Version: {}\n\
         System: {} {}\n\
         Time: {} UTC\n\
         Thread: {thread}\n\
         Panic: {message}\n\
         At: {location}\n\
         \n\
         Backtrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        utc_text(now),
    )
}

fn write_crash_report(dir: &Path, id: &str, report: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    // Several panics in one second still get their own files, in order.
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("crash-{nanos:020}-{id}.txt"));
    std::fs::write(&path, report)?;
    // Keep the newest few.
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut reports: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-")))
            .collect();
        reports.sort();
        let excess = reports.len().saturating_sub(MAX_CRASH_REPORTS);
        for old in &reports[..excess] {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("saving");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn unix_seconds(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// "2026-11-03 09:41:07"
pub fn utc_text(t: SystemTime) -> String {
    let secs = unix_seconds(t);
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

// "3 minutes ago", for the recovery prompt.
pub fn ago(when: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(when).map_or(0, |d| d.as_secs());
    let (n, unit) = match secs {
        0..=59 => return "just now".into(),
        60..=3599 => (secs / 60, "minute"),
        3600..=86_399 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_hide_the_home_folder_before_they_leave() {
        let report = "Panic: couldn't read /Users/ann/Desktop/a.pfs\nAt: /Users/ann/x.rs:1:1\n";
        assert_eq!(redacted(report, Some("/Users/ann")), "Panic: couldn't read ~/Desktop/a.pfs\nAt: ~/x.rs:1:1\n");
        assert_eq!(redacted(report, Some("/Users/ann/")), redacted(report, Some("/Users/ann")), "with or without the last slash");
        assert_eq!(redacted("C:\\Users\\Ann\\a.pfs", Some("C:\\Users\\Ann")), "~\\a.pfs");
        assert_eq!(redacted(report, Some("/")), report, "a root home isn't replaced everywhere");
        assert_eq!(redacted(report, None), report);
    }


    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pfx-recovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session(dir: &Path, id: &str) -> Session {
        // Due for an autosave straight away.
        let long_ago = Instant::now().checked_sub(AUTOSAVE_EVERY * 2).unwrap_or_else(Instant::now);
        let mut s = Session { dir: dir.to_owned(), id: id.into(), alive: None, last_check: long_ago, saved: None };
        s.lock();
        assert!(s.alive.is_some(), "locked");
        s
    }

    fn due(s: &mut Session) {
        s.last_check = Instant::now().checked_sub(AUTOSAVE_EVERY * 2).unwrap_or_else(Instant::now);
    }

    #[test]
    fn unsaved_changes_are_autosaved_and_cleared() {
        let dir = temp_dir("autosave");
        let mut s = session(&dir, "1-1");
        s.tick(true, Some(Path::new("/scenes/fire.pfs")), || "pfs 2\nnode a\n".into());
        assert_eq!(std::fs::read_to_string(dir.join("1-1.pfs")).unwrap(), "pfs 2\nnode a\n");
        assert_eq!(std::fs::read_to_string(dir.join("1-1.path")).unwrap(), "/scenes/fire.pfs");
        // Not yet due: nothing is built.
        s.tick(true, None, || panic!("built too early"));
        // Saved: the autosave goes.
        due(&mut s);
        s.tick(false, None, || unreachable!());
        assert!(!dir.join("1-1.pfs").exists());
        // A clean quit leaves nothing.
        s.end();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn a_session_that_ended_without_quitting_leaves_its_autosave_and_crash_report() {
        let dir = temp_dir("leftover");
        let mut gone = session(&dir, "100-7");
        gone.tick(true, None, || "pfs 2\n".into());
        write_crash_report(&crash_dir(&dir), "100-7", "boom").unwrap();
        // A running session alongside it, with unsaved work too.
        let mut running = session(&dir, "200-8");
        running.tick(true, None, || "pfs 2\n".into());
        assert!(leftovers(&dir).is_empty(), "both are running");

        // Killed: its lock goes with it, without a clean quit. It's found at once.
        drop(gone);
        let found = leftovers(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "100-7");
        assert_eq!(found[0].autosave, Some(dir.join("100-7.pfs")));
        assert_eq!(found[0].scene_path, None);
        assert!(found[0].crash_report.as_ref().is_some_and(|p| std::fs::read_to_string(p).unwrap() == "boom"));

        forget(&dir, "100-7");
        assert!(leftovers(&dir).is_empty());
        running.end();
    }

    #[test]
    fn a_session_gone_with_nothing_to_recover_is_tidied_away() {
        let dir = temp_dir("tidy");
        drop(session(&dir, "5-5"));
        assert!(leftovers(&dir).is_empty());
        assert!(!dir.join("5-5.alive").exists());
    }

    #[test]
    fn only_the_newest_crash_reports_are_kept() {
        let dir = temp_dir("crashes");
        for i in 0..MAX_CRASH_REPORTS + 5 {
            write_crash_report(&dir, "1-1", &format!("{i}")).unwrap();
        }
        let mut kept: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
        kept.sort_by_key(|s| s.parse::<usize>().unwrap());
        assert_eq!(kept.len(), MAX_CRASH_REPORTS);
        assert_eq!(kept[0], "5");
    }

    #[test]
    fn crash_reports_say_what_and_where() {
        let report = crash_report_text(SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_700_000), "main", "index out of bounds", "src/main.rs:10:5", "  0: main");
        assert!(report.contains(concat!("Version: ", env!("CARGO_PKG_VERSION"))));
        assert!(report.contains("Panic: index out of bounds"));
        assert!(report.contains("At: src/main.rs:10:5"));
        assert!(report.contains("Time: 2026-11-03"));
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(utc_text(SystemTime::UNIX_EPOCH), "1970-01-01 00:00:00");
        assert_eq!(utc_text(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29 00:00:00");
        let now = SystemTime::now();
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now - Duration::from_secs(60), now), "1 minute ago");
        assert_eq!(ago(now - Duration::from_secs(7200), now), "2 hours ago");
    }
}
