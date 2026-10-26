// What differs between macOS, Linux and Windows when the app hands things to the
// system: opening files and web pages, showing a file in the file manager, and
// running a shell command.

use std::process::Command;

// Opens a file or URL with whatever the system uses for it.
pub fn open(target: &str) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "macos") {
        Command::new("open").arg(target).spawn()
    } else if cfg!(windows) {
        // `start` needs an empty title before a quoted target.
        Command::new("cmd").args(["/C", "start", ""]).arg(target).spawn()
    } else {
        Command::new("xdg-open").arg(target).spawn()
    }
}

// Shows a file in the file manager (its folder, where it can't be selected).
pub fn reveal(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else if cfg!(windows) {
        let mut select = std::ffi::OsString::from("/select,");
        select.push(path.as_os_str());
        Command::new("explorer").arg(select).spawn()
    } else {
        Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn()
    }
}

// On Windows, a console program started from the app gets no console window of its
// own (renders and set-up would flash one up otherwise).
pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

// A command line run by the system's shell (sh, or cmd on Windows).
pub fn shell(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        no_window(&mut cmd);
        cmd
    } else {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", command]);
        cmd
    }
}
