// Software updates from GitHub Releases. A release tagged `vX.Y.Z` newer than
// this build is offered to the user; nothing is downloaded until they agree.
//
// macOS: the release's `.zip` is unpacked next to the running app, its code
// signature is checked against the running app's team (plus Gatekeeper), and a
// small shell script swaps the bundles once the app has quit, then relaunches.
// Flatpak: the release's `.flatpak` bundle is installed through the host's
// flatpak (needs --talk-name=org.freedesktop.Flatpak), then the app relaunches.
// Windows: the release's `-windows-x64.zip` is unpacked inside the app's folder and
// its programs' Authenticode signatures are checked against the running app's
// signer. On restart the running files are renamed aside (Windows allows that, not
// overwriting them), the new ones moved in, and the leftovers removed next start.
// Other builds (e.g. `cargo run`) only get a link to the release page.

use eframe::egui;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPO: &str = "loksangho/proceduralfx-studio";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const FLATPAK_ID: &str = "com.loksangho.proceduralfxstudio";
const AUTO_CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub assets: Vec<Asset>,
}

// How this copy was installed, which decides how (and whether) it can update itself.
#[derive(Clone, Debug, PartialEq)]
enum InstallKind {
    MacApp(PathBuf),
    Flatpak { user: bool },
    // The folder unzipped from a Windows release (it has pfx-render.exe beside the app).
    WindowsFolder(PathBuf),
    Unmanaged,
}

impl InstallKind {
    fn detect() -> Self {
        if let Ok(info) = std::fs::read_to_string("/.flatpak-info") {
            // User installs live under ~/.local/share/flatpak; anything else is system-wide.
            return InstallKind::Flatpak { user: info.contains("/.local/share/flatpak/") };
        }
        if cfg!(target_os = "macos") {
            if let Some(app) = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf))
            {
                return InstallKind::MacApp(app);
            }
        }
        if cfg!(windows) {
            if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
                if dir.join(crate::wslrender::HELPER).is_file() {
                    return InstallKind::WindowsFolder(dir);
                }
            }
        }
        InstallKind::Unmanaged
    }

    fn pick_asset<'a>(&self, assets: &'a [Asset]) -> Option<&'a Asset> {
        match self {
            InstallKind::MacApp(_) => assets.iter().find(|a| {
                let name = a.name.to_ascii_lowercase();
                name.ends_with(".zip") && (name.contains("mac") || name == "proceduralfxstudio.zip")
            }),
            InstallKind::Flatpak { .. } => assets.iter().find(|a| a.name.ends_with(".flatpak") && a.name.contains("x86_64")),
            InstallKind::WindowsFolder(_) => assets.iter().find(|a| {
                let name = a.name.to_ascii_lowercase();
                name.ends_with(".zip") && name.contains("windows")
            }),
            InstallKind::Unmanaged => None,
        }
    }
}

// "v1.2.3" / "1.2.3-beta" -> [1, 2, 3]; None if it isn't a version.
fn parse_version(text: &str) -> Option<Vec<u64>> {
    let core = text.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(mut a), Some(mut b)) => {
            let len = a.len().max(b.len());
            a.resize(len, 0);
            b.resize(len, 0);
            a > b
        }
        _ => false,
    }
}

fn parse_release(json: &serde_json::Value) -> Option<Release> {
    let tag = json["tag_name"].as_str()?;
    parse_version(tag)?;
    let assets = json["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a["name"].as_str()?.to_owned(),
                        url: a["browser_download_url"].as_str()?.to_owned(),
                        size: a["size"].as_u64().unwrap_or(0),
                        sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_owned(),
        notes: json["body"].as_str().unwrap_or("").trim().to_owned(),
        page: json["html_url"].as_str().unwrap_or("").to_owned(),
        assets,
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(format!("ProceduralFxStudio/{CURRENT_VERSION}"))
        .build()
        .into()
}

fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = match agent().get(&url).header("Accept", "application/vnd.github+json").call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Err("No releases have been published yet.".to_owned()),
        // GitHub limits API requests without a login to 60 an hour per network address
        // (403 or 429 once used up); the release pages themselves aren't limited that way.
        Err(ureq::Error::StatusCode(403 | 429)) => {
            return fetch_latest_from_pages().map_err(|e| format!("GitHub's API is rate limited for this network right now, and the release page couldn't be read either: {e}"));
        }
        Err(other) => return Err(format!("Could not reach GitHub: {other}")),
    };
    let json: serde_json::Value = response.body_mut().read_json().map_err(|e| format!("Unexpected reply from GitHub: {e}"))?;
    parse_release(&json).ok_or_else(|| "The latest release has no version tag.".to_owned())
}

// The latest release from github.com's pages: releases/latest redirects to the
// newest tag, and its asset list links each download. No notes or checksums
// (those come from the API); Mac updates are still checked for the signing team.
fn fetch_latest_from_pages() -> Result<Release, String> {
    use ureq::ResponseExt;
    let response = agent().get(&format!("https://github.com/{REPO}/releases/latest")).call().map_err(|e| e.to_string())?;
    let page = response.get_uri().to_string();
    let tag = page.rsplit_once("/releases/tag/").map(|(_, tag)| tag.trim_end_matches('/').to_owned()).ok_or("No releases have been published yet.")?;
    parse_version(&tag).ok_or("The latest release has no version tag.")?;
    let html = agent()
        .get(&format!("https://github.com/{REPO}/releases/expanded_assets/{tag}"))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    Ok(Release { version: tag.trim_start_matches(['v', 'V']).to_owned(), notes: String::new(), page, assets: parse_asset_links(&html, &tag) })
}

// Download links in a release's asset list.
fn parse_asset_links(html: &str, tag: &str) -> Vec<Asset> {
    let prefix = format!("/{REPO}/releases/download/{tag}/");
    let mut assets: Vec<Asset> = Vec::new();
    for piece in html.split("href=\"").skip(1) {
        let Some(link) = piece.split('"').next().filter(|link| link.starts_with(&prefix)) else { continue };
        let name = &link[prefix.len()..];
        if !name.is_empty() && !assets.iter().any(|a| a.name == name) {
            assets.push(Asset { name: name.to_owned(), url: format!("https://github.com{link}"), size: 0, sha256: None });
        }
    }
    assets
}

// Streams `asset` to `dest`, reporting progress and checking GitHub's SHA-256 digest.
fn download(asset: &Asset, dest: &Path, tx: &Sender<Msg>, cancel: &AtomicBool) -> Result<(), String> {
    let mut response = agent().get(&asset.url).call().map_err(|e| format!("Download failed: {e}"))?;
    let total = response.body().content_length().unwrap_or(asset.size);
    let mut reader = response.body_mut().as_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| format!("Could not save the download: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err("Download cancelled.".to_owned());
        }
        let n = reader.read(&mut buffer).map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(|e| format!("Could not save the download: {e}"))?;
        done += n as u64;
        let _ = tx.send(Msg::Progress(done, total));
    }
    file.flush().map_err(|e| e.to_string())?;
    if let Some(expected) = &asset.sha256 {
        let actual = format!("{:x}", hasher.finalize());
        if &actual != expected {
            let _ = std::fs::remove_file(dest);
            return Err("The download is corrupt or was tampered with (checksum mismatch).".to_owned());
        }
    }
    Ok(())
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let output = cmd.output().map_err(|e| format!("Could not run {:?}: {e}", cmd.get_program()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if output.status.success() { Ok(text) } else { Err(text.trim().to_owned()) }
}

// The code-signing team of a bundle, or None for unsigned/ad-hoc builds.
fn signing_team(app: &Path) -> Option<String> {
    let text = run(Command::new("codesign").args(["-dv", "--verbose=2"]).arg(app)).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|team| *team != "not set")
        .map(str::to_owned)
}

// Unpacks the downloaded zip beside the running app and checks the new bundle
// is signed by the same team and accepted by Gatekeeper. Returns the new bundle.
fn stage_mac_update(zip: &Path, current: &Path) -> Result<PathBuf, String> {
    let parent = current.parent().ok_or("The app has no parent folder.")?;
    let staging = parent.join(".ProceduralFxStudio-update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("Could not unpack the update next to the app: {e}"))?;
    run(Command::new("ditto").args(["-x", "-k"]).arg(zip).arg(&staging)).map_err(|e| format!("Could not unpack the update: {e}"))?;
    let new_app = std::fs::read_dir(&staging)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or("The update doesn't contain an app.")?;
    run(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&new_app))
        .map_err(|e| format!("The update's code signature is invalid: {e}"))?;
    if let Some(team) = signing_team(current) {
        if signing_team(&new_app).as_deref() != Some(team.as_str()) {
            return Err("The update isn't signed by the same developer as this app.".to_owned());
        }
        run(Command::new("spctl").args(["--assess", "--type", "execute"]).arg(&new_app))
            .map_err(|e| format!("Gatekeeper rejected the update: {e}"))?;
    }
    Ok(new_app)
}

// The sandbox's /tmp is private, so the host's `flatpak install` (run through
// flatpak-spawn) couldn't see a bundle saved there. The app's cache directory
// (~/.var/app/<id>/cache) has the same path inside and outside the sandbox.
fn download_dir(install: &InstallKind) -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|d| d.is_absolute());
    match (install, cache) {
        (InstallKind::Flatpak { .. }, Some(dir)) if std::fs::create_dir_all(&dir).is_ok() => dir,
        _ => std::env::temp_dir(),
    }
}

fn flatpak_install(bundle: &Path, user: bool) -> Result<(), String> {
    let mut cmd = Command::new("flatpak-spawn");
    cmd.args(["--host", "flatpak", "install", "--reinstall", "--noninteractive", "-y"]);
    cmd.arg(if user { "--user" } else { "--system" });
    cmd.arg(bundle);
    run(&mut cmd).map(|_| ()).map_err(|e| {
        format!(
            "Flatpak couldn't install the update: {e}\n\nYou can install it yourself with:\nflatpak install {}--reinstall {}",
            if user { "--user " } else { "" },
            bundle.display()
        )
    })
}

const WINDOWS_APP: &str = "ProceduralFX Studio.exe";
const WINDOWS_STAGING: &str = ".update";
const WINDOWS_OLD: &str = ".old";

fn windows_tool(name: &str) -> PathBuf {
    // The system's own tar.exe and PowerShell, not others found first on PATH.
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    match name {
        "powershell" => root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
        _ => root.join("System32").join(format!("{name}.exe")),
    }
}

// An exe's Authenticode status ("Valid", "NotSigned", ...) and signer.
fn authenticode(exe: &Path) -> Result<(String, String), String> {
    let mut cmd = Command::new(windows_tool("powershell"));
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$s = Get-AuthenticodeSignature -LiteralPath $env:PFX_SIGNED_FILE; \"$($s.Status)|$($s.SignerCertificate.Subject)\"",
    ])
    .env("PFX_SIGNED_FILE", exe);
    crate::platform::no_window(&mut cmd);
    let text = run(&mut cmd).map_err(|e| format!("Could not check the update's signature: {e}"))?;
    let (status, signer) = text.trim().split_once('|').ok_or("Could not check the update's signature.")?;
    Ok((status.to_owned(), signer.to_owned()))
}

// Unpacks the downloaded zip into the app folder's staging folder, and checks its
// programs are signed by the same publisher as this copy (when this copy is
// signed). Returns the folder holding the new files.
fn stage_windows_update(zip: &Path, current: &Path) -> Result<PathBuf, String> {
    let staging = current.join(WINDOWS_STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("Could not unpack the update in the app's folder: {e}"))?;
    let mut tar = Command::new(windows_tool("tar"));
    tar.arg("-xf").arg(zip).arg("-C").arg(&staging);
    crate::platform::no_window(&mut tar);
    run(&mut tar).map_err(|e| format!("Could not unpack the update: {e}"))?;
    // The zip holds one folder ("ProceduralFX Studio"), or the files themselves.
    let new = std::iter::once(staging.clone())
        .chain(std::fs::read_dir(&staging).map_err(|e| e.to_string())?.filter_map(Result::ok).map(|e| e.path()))
        .find(|dir| dir.join(WINDOWS_APP).is_file() && dir.join(crate::wslrender::HELPER).is_file())
        .ok_or("The update doesn't contain the app.")?;
    let (status, signer) = authenticode(&current.join(WINDOWS_APP))?;
    if status == "Valid" {
        for exe in [WINDOWS_APP, crate::wslrender::HELPER] {
            match authenticode(&new.join(exe))? {
                (s, _) if s != "Valid" => return Err(format!("The update's {exe} isn't validly signed ({s}).")),
                (_, by) if by != signer => return Err("The update isn't signed by the same publisher as this app.".to_owned()),
                _ => {}
            }
        }
    }
    Ok(new)
}

// Moves the staged files over the app's own: each existing one is renamed to
// "<name>.old" first (a running program can be renamed but not replaced). Undoes
// what it did if a move fails.
fn swap_windows_files(current: &Path, new: &Path) -> Result<(), String> {
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let undo = |done: &[(PathBuf, Option<PathBuf>)]| {
        for (target, old) in done.iter().rev() {
            let _ = std::fs::rename(target, new.join(target.file_name().unwrap_or_default()));
            if let Some(old) = old {
                let _ = std::fs::rename(old, target);
            }
        }
    };
    for entry in std::fs::read_dir(new).map_err(|e| e.to_string())?.filter_map(Result::ok) {
        let name = entry.file_name();
        let target = current.join(&name);
        let mut old = None;
        if target.exists() {
            let mut aside = name.clone();
            aside.push(WINDOWS_OLD);
            let aside = current.join(aside);
            let _ = std::fs::remove_dir_all(&aside);
            let _ = std::fs::remove_file(&aside);
            if let Err(e) = std::fs::rename(&target, &aside) {
                undo(&done);
                return Err(format!("Could not replace {}: {e}", name.to_string_lossy()));
            }
            old = Some(aside);
        }
        if let Err(e) = std::fs::rename(entry.path(), &target) {
            if let Some(old) = &old {
                let _ = std::fs::rename(old, &target);
            }
            undo(&done);
            return Err(format!("Could not install {}: {e}", name.to_string_lossy()));
        }
        done.push((target, old));
    }
    Ok(())
}

// Removes what a Windows update left behind: the renamed old files (no longer
// running once the new app starts) and the staging folder.
fn clean_windows_folder(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.ends_with(WINDOWS_OLD) || name == WINDOWS_STAGING {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".pfx-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

enum Msg {
    Checked(Result<Release, String>),
    Progress(u64, u64),
    // The update is ready: a staged .app (macOS) or folder (Windows), or nothing
    // more to do (Flatpak).
    Prepared(Result<Option<PathBuf>, String>),
}

enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading { release: Release, done: u64, total: u64 },
    Ready { release: Release, staged: Option<PathBuf> },
    Failed(String),
}

#[derive(PartialEq)]
pub enum UpdaterAction {
    None,
    SaveScene,
}

#[derive(Default, Debug, PartialEq)]
struct Prefs {
    auto_check: bool,
    skipped: Option<String>,
    last_check: u64,
}

impl Prefs {
    fn path() -> Option<PathBuf> {
        Some(crate::app_config_dir()?.join("updater.cfg"))
    }

    fn parse(text: &str) -> Self {
        let mut prefs = Prefs { auto_check: true, ..Prefs::default() };
        for (key, value) in text.lines().filter_map(|l| l.split_once('=')) {
            match key.trim() {
                "auto_check" => prefs.auto_check = value.trim() != "0",
                "skipped" if !value.trim().is_empty() => prefs.skipped = Some(value.trim().to_owned()),
                "last_check" => prefs.last_check = value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        prefs
    }

    fn to_text(&self) -> String {
        format!(
            "auto_check={}\nskipped={}\nlast_check={}\n",
            u8::from(self.auto_check),
            self.skipped.as_deref().unwrap_or(""),
            self.last_check
        )
    }

    fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_else(|| Self::parse(""))
    }

    fn save(&self) {
        if let Some(path) = Self::path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, self.to_text());
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct Updater {
    state: State,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    prefs: Prefs,
    install: InstallKind,
    open: bool,
    manual: bool,
    started: bool,
}

impl Updater {
    pub fn new() -> Self {
        let install = InstallKind::detect();
        if let InstallKind::WindowsFolder(dir) = &install {
            clean_windows_folder(dir);
        }
        Self {
            state: State::Idle,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            prefs: Prefs::load(),
            install,
            open: false,
            manual: false,
            started: false,
        }
    }

    pub fn check_now(&mut self, ctx: &egui::Context) {
        self.manual = true;
        self.open = true;
        self.start_check(ctx);
    }

    fn start_check(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        self.state = State::Checking;
        self.prefs.last_check = now_secs();
        self.prefs.save();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Checked(fetch_latest()));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    fn start_download(&mut self, release: Release, ctx: &egui::Context) {
        let Some(asset) = self.install.pick_asset(&release.assets).cloned() else { return };
        if let InstallKind::MacApp(app) = &self.install {
            if !app.parent().is_some_and(writable) {
                self.state = State::Failed(
                    "This copy of the app is in a folder it can't write to (for example the disk image). \
                     Drag it into Applications, open it from there, and try again."
                        .to_owned(),
                );
                return;
            }
        }
        if let InstallKind::WindowsFolder(dir) = &self.install {
            if !writable(dir) {
                self.state = State::Failed(format!(
                    "This copy of the app is in a folder it can't change ({}). Move the folder somewhere of your own, \
                     such as Documents, run it from there, and try again.",
                    dir.display()
                ));
                return;
            }
        }
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let install = self.install.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.state = State::Downloading { release, done: 0, total: asset.size };
        std::thread::spawn(move || {
            let dest = download_dir(&install).join(&asset.name);
            let result = download(&asset, &dest, &tx, &cancel).and_then(|()| {
                let staged = match &install {
                    InstallKind::MacApp(app) => stage_mac_update(&dest, app).map(Some),
                    InstallKind::Flatpak { user } => flatpak_install(&dest, *user).map(|()| None),
                    InstallKind::WindowsFolder(dir) => stage_windows_update(&dest, dir).map(Some),
                    InstallKind::Unmanaged => Err("This copy can't update itself.".to_owned()),
                };
                let _ = std::fs::remove_file(&dest);
                staged
            });
            let _ = tx.send(Msg::Prepared(result));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    // Swaps in the staged app (macOS, Windows) or relaunches (Flatpak), then quits.
    fn restart(&mut self, ctx: &egui::Context) {
        let State::Ready { staged, .. } = &self.state else { return };
        let launched = match (&self.install, staged) {
            (InstallKind::MacApp(app), Some(new_app)) => {
                let script = "pid=\"$1\"; old=\"$2\"; new=\"$3\"\n\
                    while kill -0 \"$pid\" 2>/dev/null; do sleep 0.2; done\n\
                    backup=\"$old.previous\"; rm -rf \"$backup\"\n\
                    if mv \"$old\" \"$backup\"; then\n\
                      if mv \"$new\" \"$old\"; then rm -rf \"$backup\"; else mv \"$backup\" \"$old\"; fi\n\
                    fi\n\
                    rm -rf \"$(dirname \"$new\")\"\n\
                    open \"$old\"\n";
                Command::new("/bin/sh")
                    .args(["-c", script, "pfx-update"])
                    .arg(std::process::id().to_string())
                    .arg(app)
                    .arg(new_app)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            (InstallKind::WindowsFolder(dir), Some(new)) => swap_windows_files(dir, new).and_then(|()| {
                let _ = std::fs::remove_dir_all(dir.join(WINDOWS_STAGING));
                Command::new(dir.join(WINDOWS_APP)).current_dir(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
            }),
            (InstallKind::Flatpak { .. }, _) => Command::new("flatpak-spawn")
                .args(["--host", "flatpak", "run", FLATPAK_ID])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string()),
            _ => return,
        };
        match launched {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => self.state = State::Failed(format!("Could not restart: {e}")),
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            if self.prefs.auto_check && now_secs().saturating_sub(self.prefs.last_check) >= AUTO_CHECK_INTERVAL_SECS {
                self.start_check(ctx);
            }
        }
        let messages: Vec<Msg> = match &self.rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        for msg in messages {
            match msg {
                Msg::Checked(result) => {
                    self.rx = None;
                    self.state = match result {
                        Ok(release) if is_newer(&release.version, CURRENT_VERSION) => {
                            // Automatic checks stay quiet about a version the user skipped.
                            if self.manual || self.prefs.skipped.as_deref() != Some(release.version.as_str()) {
                                self.open = true;
                            }
                            State::Available(release)
                        }
                        Ok(_) => State::UpToDate,
                        Err(e) if self.manual => State::Failed(e),
                        Err(_) => State::Idle,
                    };
                }
                Msg::Progress(done, total) => {
                    if let State::Downloading { done: d, total: t, .. } = &mut self.state {
                        *d = done;
                        *t = total.max(done);
                    }
                }
                Msg::Prepared(result) => {
                    self.rx = None;
                    let release = match std::mem::replace(&mut self.state, State::Idle) {
                        State::Downloading { release, .. } => release,
                        _ => return,
                    };
                    self.state = match result {
                        Ok(staged) => State::Ready { release, staged },
                        Err(e) => State::Failed(e),
                    };
                    self.open = true;
                }
            }
        }
    }

    // Menu-bar button shown while an update is waiting.
    // Whether indicator() shows a button (for the top bar's layout).
    pub fn has_indicator(&self) -> bool {
        matches!(self.state, State::Available(_) | State::Downloading { .. } | State::Ready { .. })
    }

    pub fn indicator(&mut self, ui: &mut egui::Ui) {
        let label = match &self.state {
            State::Available(r) => format!("⬆ Update to v{}", r.version),
            State::Downloading { .. } => "⬇ Downloading update…".to_owned(),
            State::Ready { .. } => "⟳ Restart to update".to_owned(),
            _ => return,
        };
        if ui.button(label).clicked() {
            self.open = true;
        }
    }

    pub fn menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Check for Updates…").clicked() {
            self.check_now(ui.ctx());
            ui.close();
        }
        if ui.checkbox(&mut self.prefs.auto_check, "Check for updates at startup").changed() {
            self.prefs.save();
        }
        ui.separator();
        ui.weak(format!("ProceduralFX Studio v{CURRENT_VERSION}"));
    }

    pub fn show(&mut self, ctx: &egui::Context) -> UpdaterAction {
        if !self.open {
            return UpdaterAction::None;
        }
        let mut action = UpdaterAction::None;
        let mut open = true;
        let mut close = false;
        let mut next: Option<State> = None;
        let mut download: Option<Release> = None;
        let mut restart = false;
        crate::own_window(ctx, "software_update", "Software Update", [460.0, 300.0], [380.0, 200.0], &mut open, true, |ui| match &self.state {
                State::Idle => {
                    close = true;
                }
                State::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking for updates…");
                    });
                }
                State::UpToDate => {
                    ui.label(format!("You're up to date: v{CURRENT_VERSION} is the latest version."));
                    if ui.button("OK").clicked() {
                        close = true;
                    }
                }
                State::Available(release) => {
                    ui.heading(format!("Version {} is available", release.version));
                    ui.label(format!("You have v{CURRENT_VERSION}."));
                    if !release.notes.is_empty() {
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            ui.label(&release.notes);
                        });
                    }
                    ui.add_space(6.0);
                    let asset = self.install.pick_asset(&release.assets);
                    if asset.is_none() {
                        ui.label(match self.install {
                            InstallKind::Unmanaged => "This copy wasn't installed from a release, so it can't update itself.",
                            _ => "This release has no download for this platform yet.",
                        });
                    }
                    ui.horizontal(|ui| {
                        if let Some(asset) = asset {
                            let size = if asset.size > 0 { format!(" ({:.0} MB)", asset.size as f64 / 1e6) } else { String::new() };
                            if ui.button(format!("Update now{size}")).clicked() {
                                download = Some(release.clone());
                            }
                        } else if !release.page.is_empty() && ui.button("Open download page").clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(&release.page));
                        }
                        if ui.button("Remind me later").clicked() {
                            close = true;
                        }
                        if ui.button("Skip this version").clicked() {
                            self.prefs.skipped = Some(release.version.clone());
                            self.prefs.save();
                            next = Some(State::Idle);
                            close = true;
                        }
                    });
                }
                State::Downloading { done, total, .. } => {
                    ui.label("Downloading and verifying the update…");
                    let fraction = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    if *done >= *total && *total > 0 {
                        ui.weak("Verifying…");
                    }
                    if ui.button("Cancel").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                }
                State::Ready { release, .. } => {
                    ui.heading(format!("Version {} is ready", release.version));
                    ui.label("The app will close and reopen to finish updating. Save your scene first if you need to.");
                    ui.horizontal(|ui| {
                        if ui.button("Save scene…").clicked() {
                            action = UpdaterAction::SaveScene;
                        }
                        if ui.button("Restart now").clicked() {
                            restart = true;
                        }
                        if ui.button("Later").clicked() {
                            close = true;
                        }
                    });
                }
                State::Failed(message) => {
                    ui.label(message);
                    ui.horizontal(|ui| {
                        if ui.button("Open download page").clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(format!("https://github.com/{REPO}/releases/latest")));
                        }
                        if ui.button("Close").clicked() {
                            next = Some(State::Idle);
                            close = true;
                        }
                    });
                }
            });
        if let Some(release) = download {
            self.start_download(release, ctx);
        }
        if restart {
            self.restart(ctx);
        }
        if let Some(state) = next {
            self.state = state;
        }
        if !open || close {
            self.open = false;
            self.manual = false;
            if matches!(self.state, State::UpToDate | State::Failed(_)) {
                self.state = State::Idle;
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert_eq!(parse_version("v2.3.4-beta.1"), Some(vec![2, 3, 4]));
    }

    #[test]
    fn parses_github_release() {
        let json = serde_json::json!({
            "tag_name": "v0.2.0",
            "body": " Faster pyro.\n",
            "html_url": "https://github.com/loksangho/proceduralfx-studio/releases/tag/v0.2.0",
            "assets": [
                {"name": "ProceduralFxStudio-macos.zip", "browser_download_url": "https://example.invalid/a.zip",
                 "size": 250000000, "digest": "sha256:ABCDEF"},
                {"name": "ProceduralFxStudio-x86_64.flatpak", "browser_download_url": "https://example.invalid/b.flatpak", "size": 9}
            ]
        });
        let release = parse_release(&json).unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.notes, "Faster pyro.");
        assert_eq!(release.assets[0].sha256.as_deref(), Some("abcdef"));
        assert_eq!(release.assets[1].sha256, None);

        let mac = InstallKind::MacApp(PathBuf::from("/Applications/ProceduralFxStudio.app"));
        assert_eq!(mac.pick_asset(&release.assets).unwrap().name, "ProceduralFxStudio-macos.zip");
        let flatpak = InstallKind::Flatpak { user: true };
        assert_eq!(flatpak.pick_asset(&release.assets).unwrap().name, "ProceduralFxStudio-x86_64.flatpak");
        assert!(InstallKind::Unmanaged.pick_asset(&release.assets).is_none());
        assert!(parse_release(&serde_json::json!({"tag_name": "latest"})).is_none());
    }

    #[test]
    fn reads_asset_links_from_the_release_page() {
        let html = format!(
            r#"<a href="/{REPO}/releases/download/v0.7.0/ProceduralFxStudio.zip" rel="nofollow">zip</a>
               <a href="/{REPO}/releases/download/v0.7.0/ProceduralFxStudio.zip">again</a>
               <a href="/{REPO}/archive/refs/tags/v0.7.0.zip">source</a>
               <a href="/{REPO}/releases/download/v0.7.0/ProceduralFxStudio-x86_64.flatpak">flatpak</a>"#
        );
        let assets = parse_asset_links(&html, "v0.7.0");
        let names: Vec<&str> = assets.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["ProceduralFxStudio.zip", "ProceduralFxStudio-x86_64.flatpak"]);
        assert_eq!(assets[0].url, format!("https://github.com/{REPO}/releases/download/v0.7.0/ProceduralFxStudio.zip"));
        assert!(assets.iter().all(|a| a.sha256.is_none()));
    }

    #[test]
    fn windows_update_swaps_files_and_cleans_up() {
        let assets = [
            Asset { name: "ProceduralFxStudio.zip".into(), url: String::new(), size: 0, sha256: None },
            Asset { name: "ProceduralFxStudio-windows-x64.zip".into(), url: String::new(), size: 0, sha256: None },
        ];
        let windows = InstallKind::WindowsFolder(PathBuf::from(r"C:\Apps\ProceduralFX Studio"));
        assert_eq!(windows.pick_asset(&assets).unwrap().name, "ProceduralFxStudio-windows-x64.zip");
        let mac = InstallKind::MacApp(PathBuf::from("/Applications/ProceduralFxStudio.app"));
        assert_eq!(mac.pick_asset(&assets).unwrap().name, "ProceduralFxStudio.zip");

        let dir = std::env::temp_dir().join(format!("pfx-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let new = dir.join(WINDOWS_STAGING).join("ProceduralFX Studio");
        std::fs::create_dir_all(dir.join("licenses")).unwrap();
        std::fs::create_dir_all(new.join("licenses")).unwrap();
        std::fs::write(dir.join(WINDOWS_APP), "old").unwrap();
        std::fs::write(dir.join("licenses/a.txt"), "old").unwrap();
        std::fs::write(new.join(WINDOWS_APP), "new").unwrap();
        std::fs::write(new.join("README.txt"), "new").unwrap();
        std::fs::write(new.join("licenses/a.txt"), "new").unwrap();

        swap_windows_files(&dir, &new).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(WINDOWS_APP)).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("README.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("licenses/a.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join(format!("{WINDOWS_APP}{WINDOWS_OLD}"))).unwrap(), "old");

        clean_windows_folder(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [WINDOWS_APP, "README.txt", "licenses"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefs_round_trip() {
        assert!(Prefs::parse("").auto_check);
        let prefs = Prefs { auto_check: false, skipped: Some("0.3.0".into()), last_check: 1234 };
        assert_eq!(Prefs::parse(&prefs.to_text()), prefs);
    }
}

#[cfg(test)]
mod live {
    // Talks to GitHub: `cargo test -- --ignored live_github_check`.
    #[test]
    #[ignore]
    fn live_github_check() {
        match super::fetch_latest() {
            Ok(release) => println!("latest: v{} with {} assets", release.version, release.assets.len()),
            Err(e) => println!("check result: {e}"),
        }
        let release = super::fetch_latest_from_pages().unwrap();
        println!("from the pages: v{} with {:?}", release.version, release.assets.iter().map(|a| &a.name).collect::<Vec<_>>());
    }
}
