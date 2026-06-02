//! Ashwend self-update helper.
//!
//! A tiny, `std`-only sidecar shipped next to the game binary (inside
//! `Ashwend.app/Contents/MacOS/` on macOS, beside the binary on Linux/Windows).
//! The game stages a verified new binary, spawns this process, and quits; this
//! process then swaps the new binary over the old one and relaunches the game.
//!
//! It exists as a separate executable so the program doing the overwrite is
//! never the file being overwritten, which also means on Windows the (now
//! exited) game's `.exe` is no longer locked, so the swap is a plain rename on
//! every platform. No networking, no decompression: all of that already
//! happened in the game before we were launched.
//!
//! Usage (not user-facing):
//!   ashwend-updater --staged <path> --target <path> --relaunch <path> [--wait-pid <pid>]

use std::{
    path::{Path, PathBuf},
    process::Command,
    thread::sleep,
    time::{Duration, Instant},
};

/// How long to keep retrying the swap. On Windows the target stays locked until
/// the game process exits; the retry loop is what waits it out. Generous to
/// survive a slow shutdown or an antivirus holding the file briefly.
const SWAP_TIMEOUT: Duration = Duration::from_secs(30);
const SWAP_RETRY_DELAY: Duration = Duration::from_millis(200);

/// How long to wait for the parent game process to exit before relaunching, so
/// we never end up with two instances (Unix `rename` succeeds even while the
/// old binary is mapped, so without this we could relaunch too early).
const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(15);
const PARENT_POLL_DELAY: Duration = Duration::from_millis(100);

struct Args {
    staged: PathBuf,
    target: PathBuf,
    relaunch: PathBuf,
    wait_pid: Option<u32>,
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ashwend-updater: {error}");
            std::process::exit(2);
        }
    };

    // 1. Wait for the game to exit (best-effort; the swap retry also waits).
    if let Some(pid) = args.wait_pid {
        wait_for_exit(pid, PARENT_EXIT_TIMEOUT);
    }

    // 2. Swap the new binary into place, retrying transient locks.
    if let Err(error) = swap_into_place(&args.staged, &args.target) {
        eprintln!("ashwend-updater: swap failed: {error}");
        // Leave the old binary untouched and still try to relaunch it so the
        // player isn't left without a runnable game.
        relaunch(&args.relaunch);
        std::process::exit(1);
    }

    // 3. Restore the executable bit (rename preserves it, copy fallback may not).
    set_executable(&args.target);

    // 4. macOS: the `.app` is ad-hoc signed, and swapping the inner binary just
    //    broke the bundle seal. Re-sign before relaunch so it stays valid.
    resign_app_bundle(&args.relaunch);

    // 5. Relaunch the (now updated) game.
    relaunch(&args.relaunch);
}

fn next_value<I: Iterator<Item = String>>(argv: &mut I, flag: &str) -> Result<String, String> {
    argv.next()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_args(mut argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut staged = None;
    let mut target = None;
    let mut relaunch = None;
    let mut wait_pid = None;
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--staged" => staged = Some(PathBuf::from(next_value(&mut argv, &flag)?)),
            "--target" => target = Some(PathBuf::from(next_value(&mut argv, &flag)?)),
            "--relaunch" => relaunch = Some(PathBuf::from(next_value(&mut argv, &flag)?)),
            "--wait-pid" => {
                wait_pid = Some(
                    next_value(&mut argv, &flag)?
                        .parse::<u32>()
                        .map_err(|_| "invalid --wait-pid".to_owned())?,
                )
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        staged: staged.ok_or("missing --staged")?,
        target: target.ok_or("missing --target")?,
        relaunch: relaunch.ok_or("missing --relaunch")?,
        wait_pid,
    })
}

/// Atomically replace `target` with `staged`, retrying while the destination is
/// locked (Windows, until the game exits) and falling back to copy+remove if
/// the two end up on different filesystems.
fn swap_into_place(staged: &Path, target: &Path) -> std::io::Result<()> {
    let deadline = Instant::now() + SWAP_TIMEOUT;
    loop {
        match std::fs::rename(staged, target) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if Instant::now() >= deadline {
                    // Last resort: copy across (handles a cross-device staged
                    // dir) then drop the staged file.
                    std::fs::copy(staged, target)?;
                    let _ = std::fs::remove_file(staged);
                    return Ok(());
                }
                // Surface nothing on each retry, locks while the game shuts
                // down are expected. Keep the last error for the timeout path.
                let _ = error;
                sleep(SWAP_RETRY_DELAY);
            }
        }
    }
}

/// Relaunch the game. On macOS a `.app` bundle is launched via `open` so it
/// gets a proper LaunchServices identity (dock icon, single instance); a bare
/// binary is spawned directly.
fn relaunch(path: &Path) {
    if path.extension().is_some_and(|ext| ext == "app") {
        let _ = Command::new("open").arg(path).spawn();
        return;
    }
    let mut command = Command::new(path);
    if let Some(dir) = path.parent() {
        command.current_dir(dir);
    }
    let _ = command.spawn();
}

/// Re-apply the bundle's ad-hoc signature after swapping the inner binary.
/// Uses non-`--deep` on purpose: `--deep` would rewrite *this* updater binary
/// (we run from inside the same bundle), which can fail; non-`--deep` re-signs
/// the main executable and re-seals resources, leaving us untouched. Best
/// effort, a de-quarantined bundle still launches from the inner binary's own
/// signature if this fails.
#[cfg(target_os = "macos")]
fn resign_app_bundle(relaunch: &Path) {
    if relaunch.extension().is_none_or(|ext| ext != "app") {
        return;
    }
    let ok = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(relaunch)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("ashwend-updater: ad-hoc re-sign failed; relaunching anyway");
    }
}

#[cfg(not(target_os = "macos"))]
fn resign_app_bundle(_relaunch: &Path) {}

#[cfg(unix)]
fn wait_for_exit(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && process_alive(pid) {
        sleep(PARENT_POLL_DELAY);
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    use std::process::Stdio;
    // `kill -0` probes for existence without sending a signal. std-only, works
    // on macOS and Linux, avoids a libc dependency in this tiny binary.
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn wait_for_exit(_pid: u32, _timeout: Duration) {
    // On Windows the running game holds an exclusive lock on its `.exe`, so the
    // swap retry loop already waits for the game to exit before it can succeed.
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Result<Args, String> {
        parse_args(items.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_all_flags() {
        let parsed = args(&[
            "--staged",
            "/tmp/new",
            "--target",
            "/app/ashwend",
            "--relaunch",
            "/app/Ashwend.app",
            "--wait-pid",
            "4321",
        ])
        .unwrap();
        assert_eq!(parsed.staged, PathBuf::from("/tmp/new"));
        assert_eq!(parsed.target, PathBuf::from("/app/ashwend"));
        assert_eq!(parsed.relaunch, PathBuf::from("/app/Ashwend.app"));
        assert_eq!(parsed.wait_pid, Some(4321));
    }

    #[test]
    fn wait_pid_is_optional() {
        let parsed = args(&["--staged", "a", "--target", "b", "--relaunch", "c"]).unwrap();
        assert_eq!(parsed.wait_pid, None);
    }

    #[test]
    fn rejects_missing_required_flags_and_bad_values() {
        assert!(args(&["--staged", "a", "--target", "b"]).is_err());
        assert!(args(&["--target", "b", "--relaunch", "c"]).is_err());
        assert!(args(&["--staged"]).is_err());
        assert!(
            args(&[
                "--staged",
                "a",
                "--target",
                "b",
                "--relaunch",
                "c",
                "--wait-pid",
                "x"
            ])
            .is_err()
        );
        assert!(args(&["--bogus", "1"]).is_err());
    }
}
