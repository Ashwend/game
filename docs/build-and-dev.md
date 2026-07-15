---
title: Build, run, test, ship (the ./cli surface)
owns: The ./cli wrapper, feature flags, the local CI gate, build prerequisites, and the toolchain pin.
when_to_read: Before running, building, testing, profiling, or releasing, or to find which ./cli subcommand does what.
sources:
  - cli - the bash wrapper that fronts every cargo invocation
  - src/cli.rs - clap Command enum (Client/Server/Admin/MultiplayerTest)
  - src/cli/multiplayer_test.rs - run_multiplayer_test
  - Cargo.toml - [features], [lints], [profile.*], rust-version
  - rust-toolchain.toml - pinned channel 1.95.0
  - .cargo/config.toml - CMAKE_POLICY_VERSION_MINIMUM workaround
  - .github/workflows/quality-gate.yml - the CI matrix ./cli ci mirrors
  - src/app/embedded_assets.rs - EmbeddedAssetsPlugin (why published builds need no sibling assets/)
related:
  - docs/profiling.md - the profile feature and trace capture/analysis
  - docs/headless-agent-testing.md - launching server/client to verify a change
  - docs/multiplayer-testing.md - the two-client multiplayer-test helper
  - docs/code-style.md - lint policy, commit conventions, balance-constant idiom
  - docs/updates-and-distribution.md - publish targets, release-asset names, packaging
---

# Build, run, test, ship (the ./cli surface)

> When to read this: before running, building, testing, profiling, or releasing, or to find which `./cli` subcommand does what. Source of truth: `cli`, `Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml`, `.github/workflows/quality-gate.yml`. Canonical invariants live in CLAUDE.md.

Always drive builds through `./cli`, never raw `cargo`. The wrapper does two things a bare `cargo run` skips:

1. Exports `BEVY_ASSET_ROOT=<repo root>` (`cli` near the top). A binary launched directly (not via `cargo run`) has no `CARGO_MANIFEST_DIR`, so Bevy's `AssetServer` falls back to the executable's dir (`target/debug/`, which has no `assets/`). Without this var, assets do not load. The var is also inherited by child processes the script spawns (the `multiplayer-test` client windows).
2. Kills stale Ashwend windows before launch (`close_existing_game` in `cli`: `pkill -x Ashwend`, `pkill -x ashwend`, `pkill -f target/debug/ashwend`) for `dev`, `dev-fast`, `profile`, and `multiplayer-test`.

## ./cli subcommand to cargo invocation

Every subcommand is a `case` arm in the `cli` bash script. The table is the contract; read the script for the exact flags.

| Subcommand | cargo invocation | When to use |
| --- | --- | --- |
| `dev` | `cargo run --bin ashwend -- client` | Default dev loop: launch the client (loopback singleplayer or connect). |
| `dev-fast` | `cargo run --features dev-fast --bin ashwend -- client` | Same, faster incremental rebuilds via `bevy/dynamic_linking`. Local iteration only, never ship. |
| `profile` | `cargo run --features profile --bin ashwend -- client` | Capture a Chrome trace + extra diagnostics. Writes `trace-<unix-ms>.json` to cwd on exit. See [docs/profiling.md](profiling.md). |
| `server` | `cargo run --bin ashwend -- server` | Run a dedicated authoritative server. `./cli server --help` for flags (auth mode, map size, admin socket). |
| `admin` | `cargo run --bin ashwend -- admin` | Send announce / shutdown / set-time / time-speed to a dedicated server's admin socket. |
| `multiplayer-test` | `cargo build --bin ashwend` then `./target/debug/ashwend multiplayer-test` | Spawn a fresh local server + two auto-connecting client windows. See [docs/multiplayer-testing.md](multiplayer-testing.md). |
| `check` | `cargo check --locked --all-targets` | Fast compile check (default features). |
| `test` | `cargo test --locked --all-targets` | Run the unit + integration suite. |
| `lint` | `cargo fmt --all -- --check` then `cargo clippy --locked --all-targets --all-features -- -D warnings` | rustfmt check + clippy with warnings denied. The pre-push hook runs exactly this. |
| `ci` | fmt check, clippy `--all-features -D warnings`, `check --all-features`, `test` | The canonical local pre-merge gate. Reproduces the CI Quality Gate (and slightly exceeds it, see below). |
| `setup-hooks` | `git config core.hooksPath .githooks` | Opt in to the pre-push hook (runs `./cli lint` on `git push`). Not auto-installed. |
| `audit` | `cargo audit --deny warnings` (auto-installs `cargo-audit` if missing) | Run the RustSec advisory check. |
| `coverage` | `cargo llvm-cov --all-targets --all-features --text` if present, else `cargo test --all-targets` | Print line-coverage text. |
| `build` | `cargo build --locked` | Plain debug build (passes extra args through). |
| `sweep [days]` | `cargo sweep --time <days>` (auto-installs `cargo-sweep`; default 30, override via arg or `ASHWEND_SWEEP_KEEP_DAYS`) | Manually trim stale `target/` artifacts. Deliberately not wired into `dev`. |
| `publish` | `cargo build --release --target <T>` for each of four targets, copies to `builds/<T>/Ashwend[.exe]` | Cross-build release binaries. Targets below. See [docs/updates-and-distribution.md](updates-and-distribution.md). |
| `web-build` | `npm run build` in `website/`, zips `dist/client` contents to `./ashwend-web.zip` | Package the marketing site for a Cloudflare Pages direct upload. |

`publish` targets (the `for target in ...` loop in `cli`): `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-gnu`. Each is `rustup target add`-ed first; a failing target prints a hint and the command exits non-zero.

`multiplayer-test`, `server`, and `admin` map to clap subcommands on the same `ashwend` binary (`src/cli.rs` `Command` enum: `Client`, `Server`, `Admin`, `MultiplayerTest`). The `multiplayer-test` flow itself lives in `src/cli/multiplayer_test.rs` (`run_multiplayer_test`).

## Feature flags (all three are dev-only)

`default = []` (`Cargo.toml` `[features]`). There are exactly three opt-in flags, none of which ships:

- `dev-fast = ["bevy/dynamic_linking"]`: faster incremental rebuilds by dynamically linking Bevy. Use via `./cli dev-fast`. Never produce a release or published build with it.
- `replication-trace = []`: logs every server-side mutation and client-side reception of replicated components, to verify Lightyear's per-component path actually delivers post-spawn diffs. Compile it in and run with `RUST_LOG=replication_trace=info` (CLAUDE.md's replicated-state rules call out the `MUTATE`/`RECV` pattern; see [docs/replication.md](replication.md)). It has no `./cli` arm: add `--features replication-trace` to a `cargo`/`./cli build` invocation yourself.
- `profile = ["bevy/trace_chrome"]`: Chrome-tracing spans (per-system, per-schedule, render-pass) plus runtime diagnostics (FPS, frame time, entity count, CPU, RAM) to stdout. Use via `./cli profile`; it writes a growing `trace-<unix-ms>.json` to cwd. Full capture/analysis workflow in [docs/profiling.md](profiling.md).

CI compiles the `--all-features` matrix leg precisely so these cfg-gated paths keep building. A plain `./cli check` (default features) never compiles them, so breakage in trace/profile code only surfaces under `--all-features`.

## ./cli ci is the local pre-merge gate

`./cli ci` runs four legs in order: fmt check, `clippy --all-targets --all-features -- -D warnings`, `check --all-targets --all-features`, `test --all-targets`. Run it before you push.

It mirrors `.github/workflows/quality-gate.yml`, but is a strict superset, not an exact copy. The CI matrix has two labels:

- `default` (no features): runs fmt, clippy, check, and tests.
- `all-features` (`--all-features`): runs clippy and check only. It sets `run_fmt: false` and `run_tests: false`, so CI does not run the test suite at `--all-features`.

`./cli ci` runs tests once (at default features) and clippy + check at `--all-features`, so it covers both CI legs. It does not run the test suite at `--all-features`, matching CI. Net effect: passing `./cli ci` locally is at least as strict as the Quality Gate.

Clippy runs with `-D warnings` in both CI legs and in `./cli lint`/`./cli ci`, so every `warn`-level lint in `Cargo.toml` `[lints.clippy]` (including `dbg_macro` and `todo`) is a hard build failure. Do not commit `dbg!()` or `todo!()`. The lint policy itself (where levels vs tunables live, the `unwrap_used`/`expect_used` story) is documented in [docs/code-style.md](code-style.md).

## Pre-push hook (opt-in)

Git hooks are not auto-installed. `./cli setup-hooks` runs `git config core.hooksPath .githooks`; thereafter `.githooks/pre-push` runs `./cli lint` (fmt check + clippy `-D warnings`) before each `git push`. The hook runs only the lighter `lint` leg, not the full `ci` gate, so still run `./cli ci` before opening a PR if you want the test + `--all-features` coverage.

## Build prerequisites

A fresh machine needs a rustup toolchain (the pinned channel is picked up automatically from `rust-toolchain.toml`) plus the usual Bevy desktop system deps. The only third-party C library is **libopus** (voice chat). Per-OS install (full text in CONTRIBUTING.md "Build dependencies"):

- **Linux (Debian/Ubuntu):** `sudo apt-get install -y g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-x11-0 libwayland-dev libxkbcommon-dev libopus-dev`
- **macOS:** `brew install opus`. If the build falls back to compiling bundled Opus and fails under CMake 4.x, point pkg-config at the brew install: `export PKG_CONFIG_PATH="$(brew --prefix opus)/lib/pkgconfig:${PKG_CONFIG_PATH:-}"`.
- **Windows (MSVC):** `vcpkg install opus:x64-windows-static-md`, then set `OPUS_LIB_DIR=<vcpkg root>\installed\x64-windows-static-md` and `OPUS_STATIC=1`. The Opus bindings do not use pkg-config on Windows.

**CMake 4.x workaround.** `.cargo/config.toml` sets `CMAKE_POLICY_VERSION_MINIMUM = "3.5"` under `[env]`. The `audiopus_sys` crate bundles an old Opus source whose CMakeLists declares a `cmake_minimum_required` below 3.5; CMake 4.x removed compatibility with that and configuration fails. This env var (read by CMake 3.31+) sets the policy-version floor so the bundled Opus configures cleanly. It is inherited by the cmake subprocess the build script spawns.

**Embedded assets.** Published builds need no sibling `assets/` folder: every media file is baked into the binary by `EmbeddedAssetsPlugin` (`src/app/embedded_assets.rs`), which walks the compile-time tree of `assets/` (via the `include_dir` crate) and registers each file with Bevy's embedded asset source. `BEVY_ASSET_ROOT` (set by `cli`) only matters for binaries run directly from `target/` during local dev, where assets are read from disk.

## Toolchain and reproducibility

- **Pinned channel:** `1.95.0` with `clippy` + `rustfmt` components (`rust-toolchain.toml`). `Cargo.toml` also declares `rust-version = "1.95"` (MSRV) and `edition = "2024"`.
- **`--locked` everywhere:** `check`, `test`, `lint`, `ci`, `build` all pass `--locked` so builds resolve against the committed `Cargo.lock` and fail rather than silently bumping a dependency.
- **`[profile.dev.package."*"] opt-level = 3`** (`Cargo.toml`): dependencies (Bevy, rapier3d, lightyear) compile optimized even in dev so `./cli dev` is playable. The crate's own code stays at the default dev opt-level for fast incremental rebuilds. This is why `./cli profile` can capture a representative trace from a dev build.
- **`[profile.release]`:** `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`. Smaller binaries and no symbols leaked into release tarballs at the cost of a longer link step.

## Related docs

- [docs/profiling.md](profiling.md): the `profile` feature, Chrome-trace capture, and Perfetto analysis.
- [docs/headless-agent-testing.md](headless-agent-testing.md): launching `server`/`client` headless to screenshot and assert on a change.
- [docs/multiplayer-testing.md](multiplayer-testing.md): the `multiplayer-test` two-client helper in depth.
- [docs/code-style.md](code-style.md): lint policy locations, Conventional Commit + DCO rules, the `game_balance.rs` constant idiom, dependency/audit policy.
- [docs/updates-and-distribution.md](updates-and-distribution.md): `publish` targets, release-asset naming, and packaging/self-update.
- [CLAUDE.md](../CLAUDE.md): the invariants this doc does not restate (singleplayer == multiplayer, gameplay-never-pauses, replicated-state rules, balance-in-`game_balance.rs`, no em dashes, no monolithic files).
