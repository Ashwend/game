//! Tiny cross-module helpers. Keep this module dependency-light: anything
//! in here should be reachable from `protocol`, `controller`, `server`, and
//! the client tree without pulling in heavy crates.

pub mod hash;
pub mod variation;

/// Open `url` in the system browser. Best-effort; returns the spawn error so a
/// caller can surface it if it cares. Shared by the menu's Discord link, the
/// WorkOS login flow, and the updater's download-page fallback so the per-OS
/// launch command lives in one place.
pub fn open_url(url: &str) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    command.spawn().map(|_| ())
}
