//! Build script: tell Cargo to recompile when any compile-time-baked config
//! variable changes.
//!
//! The WorkOS client id (`src/auth/workos/config.rs`) and the PostHog analytics
//! config (`src/analytics/config.rs`) are embedded at build time via
//! `option_env!`. Cargo does NOT track `env!`/`option_env!` inputs on its own,
//! so without these directives a cached/incremental build keeps the previously
//! baked value even after the variable changes, exactly the trap where a CI
//! variable is updated but the produced binary still points at the old
//! environment. Listing each variable here forces a rebuild when it changes, on
//! CI and locally alike.
//!
//! Emitting any `rerun-if-*` directive also switches Cargo off its default
//! "rerun if any tracked file changed" heuristic for this script, so we add the
//! file watch back explicitly.

use std::{
    hash::{Hash, Hasher},
    path::Path,
};

fn main() {
    // The build script itself.
    println!("cargo::rerun-if-changed=build.rs");

    // Windows: embed the application icon as a Win32 resource so Explorer, the
    // taskbar, Alt-Tab, and installer shortcuts show the Ashwend logo. This
    // links into both `ashwend.exe` and `ashwend-updater.exe` (shared build
    // script). Host-gated via `#[cfg(windows)]` (the build script runs on the
    // build host): CI builds the Windows target on a Windows runner, so host ==
    // target. If we ever cross-compile Windows from another host, key this off
    // `CARGO_CFG_WINDOWS` and make `winresource` an unconditional build-dep.
    println!("cargo::rerun-if-changed=.github/assets/ashwend.ico");
    #[cfg(windows)]
    embed_windows_icon();

    // Assets are baked into the binary at compile time by `include_dir!` in
    // `src/app/embedded_assets.rs`. That's a proc macro, and on stable Rust it
    // can't report which files it read, so Cargo doesn't know to rebuild when an
    // asset changes: edit an item icon and a plain `cargo build`/`cargo run`
    // keeps the *stale* bytes embedded. We close that gap here. We watch the
    // assets tree and write a fingerprint of it into OUT_DIR; `embedded_assets.rs`
    // `include!`s that file, so when any asset changes the fingerprint changes,
    // which forces `embedded_assets.rs` to recompile and `include_dir!` to
    // re-read the tree. Without this, asset edits silently no-op until something
    // unrelated triggers a rebuild of that module.
    println!("cargo::rerun-if-changed=assets");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    fingerprint_dir(Path::new("assets"), &mut hasher);
    let fingerprint = hasher.finish();
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo");
    let dest = Path::new(&out_dir).join("embedded_assets_fingerprint.rs");
    // Anonymous const: exists only so `include!`ing this file makes the
    // fingerprint a recompile dependency. Never read, never warns.
    std::fs::write(&dest, format!("const _: u64 = {fingerprint};\n"))
        .expect("write embedded asset fingerprint");

    // Every variable consumed by `option_env!` in the crate. Keep this in sync
    // with the `mod build { ... option_env!(...) }` blocks in
    // `src/auth/workos/config.rs` and `src/analytics/config.rs`.
    const BAKED_VARS: &[&str] = &[
        "GAME_WORKOS_CLIENT_ID",
        "GAME_WORKOS_REDIRECT_PORT",
        "POSTHOG_API_KEY",
        "POSTHOG_HOST",
        "POSTHOG_ENVIRONMENT",
        "POSTHOG_ENABLED",
        "POSTHOG_DISABLE_GEOIP",
    ];
    for var in BAKED_VARS {
        println!("cargo::rerun-if-env-changed={var}");
    }

    // Surface the common misconfiguration loudly, but only for release builds:
    // a shipped binary with no WorkOS client id baked in silently falls back to
    // the in-crate default. A warning in the build log makes "why is it pointing
    // at the wrong environment?" answerable without spelunking. Debug builds are
    // skipped because local dev points at its environment via `workos.local.toml`
    // (which this script can't see), so warning there would be a false alarm.
    let is_release = std::env::var("PROFILE").as_deref() == Ok("release");
    let client_id_set = std::env::var("GAME_WORKOS_CLIENT_ID")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if is_release && !client_id_set {
        println!(
            "cargo::warning=GAME_WORKOS_CLIENT_ID is unset or empty for this release build; baking the default WorkOS client id from src/auth/workos/config.rs"
        );
    }
}

/// Compile the multi-resolution `.github/assets/ashwend.ico` into the binary as
/// a Win32 icon resource. A missing `rc.exe`/`llvm-rc` or a malformed icon must
/// fail a release build rather than silently shipping an iconless `.exe`; for a
/// local/dev host build it is only a warning so a dev without the Windows SDK
/// isn't blocked.
#[cfg(windows)]
fn embed_windows_icon() {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(".github/assets/ashwend.ico");
    resource.set("ProductName", "Ashwend");
    resource.set("FileDescription", "Ashwend");
    if let Err(error) = resource.compile() {
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            panic!("failed to embed the Windows application icon: {error}");
        }
        println!("cargo::warning=failed to embed the Windows application icon: {error}");
    }
}

/// Hash every file under `dir` (path + length + mtime) into `hasher`, recursively
/// and in a stable order, so the result changes iff an asset is added, removed,
/// or edited. Uses the fixed-key `DefaultHasher`, so identical trees produce the
/// same value across builds (no spurious recompiles).
fn fingerprint_dir(dir: &Path, hasher: &mut impl Hasher) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read_dir.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            fingerprint_dir(&path, hasher);
        } else if let Ok(meta) = std::fs::metadata(&path) {
            path.to_string_lossy().hash(hasher);
            meta.len().hash(hasher);
            if let Ok(mtime) = meta.modified().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| std::io::Error::other(e.to_string()))
            }) {
                mtime.as_nanos().hash(hasher);
            }
        }
    }
}
