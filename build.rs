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

fn main() {
    // The build script itself.
    println!("cargo::rerun-if-changed=build.rs");

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
