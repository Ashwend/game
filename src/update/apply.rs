//! Hand-off to the external updater + the browser fallback.
//!
//! All this side does is locate the sibling `ashwend-updater`, work out what
//! the updater should relaunch (the `.app` bundle on macOS, the bare binary
//! elsewhere), spawn it, and let the caller quit. The swap + relaunch happen in
//! the separate `ashwend-updater` process so nothing is overwriting the file it
//! is running from.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use super::{asset, github};

/// Path to the sibling updater binary, if it exists next to the game binary.
pub(crate) fn updater_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(asset::UPDATER_BINARY);
    path.exists().then_some(path)
}

/// Whether an in-place self-update is possible on this install (supported host
/// *and* the updater binary is present beside us).
pub(crate) fn can_self_update() -> bool {
    asset::host_is_supported() && updater_path().is_some()
}

/// What the updater should relaunch after the swap. On macOS, if the game lives
/// inside an `.app`, relaunch the bundle (proper LaunchServices launch + dock
/// identity) rather than the bare inner binary.
fn relaunch_target(game_binary: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        for ancestor in game_binary.ancestors() {
            if ancestor.extension().is_some_and(|e| e == "app") {
                return ancestor.to_path_buf();
            }
        }
    }
    game_binary.to_path_buf()
}

/// Launch the updater to swap `staged` over the running game binary and
/// relaunch. The caller must trigger `AppExit` right after, so the old process
/// releases the binary (matters on Windows) and the updater relaunches exactly
/// one fresh instance — it waits for this pid to exit first.
pub(crate) fn spawn_updater(staged: &Path) -> Result<(), String> {
    let updater = updater_path().ok_or_else(|| "updater binary not found".to_owned())?;
    let target = std::env::current_exe().map_err(|e| format!("cannot locate current exe: {e}"))?;
    let relaunch = relaunch_target(&target);
    Command::new(updater)
        .arg("--staged")
        .arg(staged)
        .arg("--target")
        .arg(&target)
        .arg("--relaunch")
        .arg(&relaunch)
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start updater: {e}"))
}

/// Fallback when in-place update isn't possible (no updater binary or an
/// unsupported host): open the releases page in the browser.
pub(crate) fn open_download_page() {
    let _ = open_url(&github::releases_page_url());
}

/// Best-effort open `url` in the system browser. (Mirrors the helper in the
/// WorkOS login flow; kept local so this module has no cross-feature coupling.)
fn open_url(url: &str) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_target_returns_bare_binary_for_non_bundle_paths() {
        let p = Path::new("/usr/local/bin/ashwend");
        assert_eq!(relaunch_target(p), p.to_path_buf());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn relaunch_target_returns_app_bundle_on_macos() {
        let inner = Path::new("/Users/x/Ashwend.app/Contents/MacOS/ashwend");
        assert_eq!(
            relaunch_target(inner),
            PathBuf::from("/Users/x/Ashwend.app")
        );
    }
}
