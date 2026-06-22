---
title: Self-update, changelog modals, and packaging
owns: The client-only update subsystem (src/update/ + the sibling ashwend-updater binary) and the release packaging/asset-rename pipeline.
when_to_read: Before changing release-asset names, the self-update flow, either changelog modal, or installer/signing config.
sources:
  - src/update/mod.rs - UpdatePlugin, UpdateState, UpdateStatus, the worker + message pump
  - src/update/github.rs - releases client, latest_stable, changelog_since, changelog_for
  - src/update/asset.rs - HOST_ASSET_NAME / ARCHIVE_GAME_MEMBER / UPDATER_BINARY per-host constants
  - src/update/download.rs - download + sha256 verify + extract + stage
  - src/update/apply.rs - spawn_updater, relaunch_target, open_download_page
  - src/bin/ashwend-updater.rs - the std-only swap/relaunch sidecar
  - src/app/systems/update.rs - apply_update_system (saves the open SP world before relaunch)
  - src/app/ui/update.rs - update_modal, current_changelog_modal, update_corner_pill, pause_update_row
  - .github/scripts/release_assets.py - canonical asset-name source of truth (its docstring is the rename checklist)
  - .github/scripts/package-release.py - bundle/zip/tar/dmg/installer builder + ad-hoc signing
  - .github/installer/ashwend.iss - per-user Inno Setup installer
related:
  - docs/ui-and-client.md - where the update widgets register in the egui frame
  - docs/voice.md - why Info.plist carries NSMicrophoneUsageDescription
  - docs/build-and-dev.md - the ./cli surface that drives a release
  - docs/headless-agent-testing.md - the harness that sets GAME_SKIP_UPDATE_CHECK
---

# Self-update, changelog modals, and packaging

> When to read this: before changing release-asset names, the self-update transport, either changelog modal, or installer/signing config. Source of truth: `src/update/`, `src/bin/ashwend-updater.rs`, `.github/scripts/release_assets.py`, `.github/scripts/package-release.py`, `.github/installer/ashwend.iss`. Canonical invariants live in CLAUDE.md.

The update subsystem is **client-only**. `UpdatePlugin` is added in `src/app.rs` (`app.rs:530`) on the client run path only; the dedicated server and the `admin` CLI never load it. It does two independent things from one boot-time GitHub fetch:

1. Detects a newer stable release and offers an in-place self-update (download to a sibling `ashwend-updater` process that swaps the binary and relaunches).
2. Captures the **running build's own** release notes for the title-screen "What's new" modal, independent of whether anything newer exists.

Both changelog views live on the same `UpdateState` resource with two independent open flags. Conflating them is the most common drift; they are separate features.

## Boot check flow and the UpdateStatus state machine

`UpdateState::spawn()` (`src/update/mod.rs` - `spawn`) starts one named OS thread `ashwend-update-worker` running `run_worker`. There is no async runtime in the app; this mirrors the analytics worker (`src/analytics/client.rs` - `game-analytics-worker`): one thread + blocking `ureq`. The worker does the boot check once, then blocks on a command channel to serve a later `Download`.

The check (`check_latest` -> `github::fetch_releases`) hits the public GitHub Releases API for `Ashwend/game` (`src/update/github.rs` - `REPO_OWNER`/`REPO_NAME`). The repo is public, so no auth, only the `User-Agent: ashwend/<GAME_VERSION>` header GitHub requires. Network parameters worth knowing before tuning:

- `per_page=30` (`RELEASES_PER_PAGE`). A client more than 30 releases behind gets a changelog truncated to the latest 30 (still strictly correct about *which* is newest).
- 4s connect timeout, 10s overall (`HTTP_CONNECT_TIMEOUT` / `HTTP_TIMEOUT` via `build_agent`).

**Any failure is treated as "up to date" and logged once** (`check_latest` returns `Checked { available: None, current_changelog: None }` on error). A flaky network never blocks or nags. `GameServer`-side gameplay is unaffected; this is purely a client UI probe.

`UpdateStatus` (`src/update/mod.rs` - `enum UpdateStatus`):

```
Checking ──► UpToDate          (nothing newer, or check failed: same UX)
         └─► Available ──► Downloading{received,total} ──► Ready
                                                            │
                                            (player clicks Restart & update)
                                                            ▼
                                                         Applying
         (any download/extract/verify error) ──► Failed(String)
```

The worker pushes `Msg` values; `poll_update_messages_system` (`mod.rs` - `poll_update_messages_system`) drains them into `UpdateState` each frame:

- `Checked { available, current_changelog }`: stores `current_changelog` unconditionally (drives the "What's new" modal); if `available` is `Some` and not already skipped, sets `status = Available` and auto-opens the update modal (`modal_open = !skipped`).
- `Progress { received, total }`: refreshes the `Downloading` payload.
- `Staged { path }`: records the staged binary path and sets `status = Ready`.
- `Failed(error)`: logs and sets `status = Failed`.

`UpdateState::has_update()` is true only when an `available` exists and status is past `Checking`/`UpToDate`; it gates the corner pill and pause row.

### GAME_SKIP_UPDATE_CHECK (debug builds only)

`const SKIP_ENV = "GAME_SKIP_UPDATE_CHECK"` and the early return in `spawn()` are both behind `#[cfg(debug_assertions)]` (`mod.rs` - `SKIP_ENV` / `spawn`), so the env var is genuinely compiled out of release builds. When set in a dev build the state is constructed as `UpToDate` with no worker. The agent test harness sets it so the update modal can't cover the scene mid-screenshot (see `docs/headless-agent-testing.md`). Do not document or rely on it for release builds.

## The two changelog modals

Both render from `src/app/ui/update.rs` and are pure presentation over `UpdateState`. They are called from the main egui frame in `src/app/ui.rs` (`ui.rs:301-306`) with a `Local<CommonMarkCache>` (markdown is rendered via `egui_commonmark`).

**1. Available-update modal** (`update.rs` - `update_modal`). Shows when `modal_open && available.is_some()`. Header `Update available`, the line `You're on v<GAME_VERSION>. Latest is v<version>.`, the scrollable changelog (concatenated notes for every stable release strictly newer than the current build), and status-driven actions (`render_actions`):

- *Available*: `Update now` (or `Open download page` when `can_self_update()` is false) / `Skip this version` / `Later`.
- *Downloading*: a progress bar.
- *Ready*: `Restart & update` (calls `request_apply`) / `Later`.
- *Applying*: a spinner.
- *Failed*: the error text + `Open download page` / `Close`.

Re-entry points when the modal is dismissed (soft dismiss keeps the entry point; never dismisses mid-`Downloading`/`Applying`):
- `update_corner_pill` (`update.rs` - `update_corner_pill`): top-right pill on menu screens, label switches per status (`Update available: vX`, `Updating… N%`, `Update ready to install`). Suppressed in-game.
- `pause_update_row` (`update.rs` - `pause_update_row`): a pause-menu row in-game, called from `src/app/ui/pause.rs` (`pause.rs:65`).

`Skip this version` persists the version string to `<data_dir>/skipped_version` (`src/update/skipped.rs`, same `ProjectDirs` + atomic-write pattern as the analytics distinct id) so the modal won't auto-pop for that exact version again. The pill still appears.

**2. Title-screen "What's new" modal** (`update.rs` - `current_changelog_modal`). Shows the release notes for the build the player is *currently* running. Opened from the **clickable white version label** in the bottom-right of the main menu (`src/app/ui/menu.rs` - `draw_version_indicator`, `menu.rs:111` calls `open_current_changelog`). Header `What's new in v<GAME_VERSION>`, the running build's notes (or a graceful fallback when offline / a dev build ahead of any tag), and `Close` / `All releases`. It is driven by `current_changelog` / `current_changelog_open`, which are wholly separate from the available-update modal's `available` / `modal_open`.

Changelog markdown is compacted by `clean_release_body` (`src/update/github.rs`) before display: it drops the `## Ashwend vX` title, the `Changes since vY.` preamble, the `Release Assets` link section, and the `Changelog` label, and renders category headings as **bold** lines instead of large headers. `changelog_since` concatenates newer releases newest-first (bold version labels only when more than one is stacked); `changelog_for` returns the single matching release's notes for the running build.

## Self-update transport

`begin_download` (`mod.rs` - `begin_download`) sends `Cmd::Download` to the worker, which calls `download::download_and_stage` (`src/update/download.rs`):

1. **Download** the host's release archive (`asset.browser_download_url`) to the system temp dir, streamed 64 KiB at a time with progress callbacks.
2. **Verify sha256** against GitHub's published `digest` (`"sha256:<hex>"`). **Warn-only when GitHub omits a digest** (`verify_sha256` logs and returns `Ok` on `None`). A non-`sha256:` prefix is rejected. Security note: this tolerates the brief window right after a release before GitHub computes a digest. If you tighten this to hard-fail, account for that window.
3. **Extract** the game binary out of the archive (`zip` crate on macOS/Windows, `tar` + `flate2` on Linux). The wanted member is `asset::ARCHIVE_GAME_MEMBER`; `entry_matches` tolerates a `./` prefix or one extra leading directory and refuses to match the sibling `ashwend-updater`.
4. **Stage** the extracted binary in the **install directory** as `.ashwend-update.<pid>.staged`, not in temp, because the updater's swap is a `rename` that is only atomic within one filesystem (`/tmp` can be a separate mount). Sets the exec bit on Unix.

On success the worker sends `Staged { path }` -> status `Ready`.

`can_self_update()` (`mod.rs` / `src/update/apply.rs` - `can_self_update`) is true only when the host publishes an asset *and* the sibling updater binary exists beside the running game (`updater_path`). When false the modal's primary button opens the releases page instead (`apply::open_download_page` -> `crate::util::open_url`, `src/util.rs:28`).

### Apply path (saves the open world first)

`Restart & update` flips status to `Applying`. `apply_update_system` (`src/app/systems/update.rs` - `apply_update_system`) takes over. It lives in `src/app/` (**not** `crate::update`) because it needs `ClientRuntime` / `MenuState` / `SessionShutdownTasks`, which are private to `app`, to save any open singleplayer world before the process exits:

1. If a live session exists, tear it down via the same background save the pause-menu Quit uses (`shutdown_in_background`) and reset menu state. Runs once; later frames fall through.
2. Wait for `shutdown_tasks.all_finished()` so the save is durable.
3. `crate::update::spawn_updater(staged)` then `AppExit::Success`.

`spawn_updater` (`src/update/apply.rs`) launches `ashwend-updater --staged <p> --target <current_exe> --relaunch <relaunch_target> --wait-pid <pid>`. `relaunch_target` resolves to the enclosing `.app` bundle on macOS (proper LaunchServices launch) and the bare binary elsewhere.

### The ashwend-updater sidecar

`src/bin/ashwend-updater.rs` is a tiny `std`-only sidecar: no network, no decompression (the game already did all of that). It exists as a separate executable so the program doing the overwrite is never the file being overwritten. Steps (`main`):

1. **Wait for the game to exit.** On Unix, poll the parent pid via `kill -0` for up to 15s (`PARENT_EXIT_TIMEOUT`); Unix `rename` succeeds even while the old binary is mapped, so without this we could relaunch a double instance. On Windows the running game holds an exclusive `.exe` lock, so the swap retry below is what waits it out (`wait_for_exit` is a no-op there).
2. **Swap** `staged` over `target` with `std::fs::rename`, retrying for 30s at 200ms intervals (`SWAP_TIMEOUT` / `SWAP_RETRY_DELAY`) to survive a slow shutdown or AV/indexer holding the file; cross-device fallback is `copy` + `remove_file`. On swap failure it still relaunches the old binary so the player isn't left without a runnable game.
3. Restore the exec bit (rename preserves it; the copy fallback may not).
4. **macOS only**: re-sign the bundle (see below).
5. **Relaunch** via `open` for an `.app`, or spawn the bare binary directly.

Both `src/main.rs:7` and `src/bin/ashwend-updater.rs:20` carry `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` so shipped Windows builds never flash a console between quit and relaunch. `src/console.rs` reattaches to the launching terminal so the `server`/`admin` CLI still prints when run from a shell.

## macOS bundle and signing

The macOS asset (`ashwend-aarch64-apple-darwin.zip`, `asset.rs` - `HOST_ASSET_NAME`) is a zipped `Ashwend.app`, not a bare binary. `package-release.py` (`build_app_bundle` -> `zip_app_bundle` via `ditto`) produces:

```
Ashwend.app/Contents/
  Info.plist            # CFBundleIdentifier=com.Ashwend.Ashwend, CFBundleIconFile=AppIcon,
                        #   NSMicrophoneUsageDescription (TCC kills a bundled app that opens the mic without it)
  MacOS/ashwend         # the game
  MacOS/ashwend-updater # the self-update sidecar
  Resources/AppIcon.icns
```

The icon source is the committed `.github/assets/AppIcon.icns` (`package-release.py` - `ICON_SRC`), copied directly into the bundle. There is no committed AppIcon regeneration script.

The bundle is **ad-hoc signed**, not notarized (notarization needs a paid Developer ID, deferred). Build-time signing is `codesign --force --deep --sign -` then `--verify --deep --strict` (`package-release.py` - `adhoc_sign`); `--deep` is fine here because nothing in the bundle is running at build time.

**The runtime re-sign is intentionally non-`--deep`** (`src/bin/ashwend-updater.rs` - `resign_app_bundle`: `codesign --force --sign -`). Self-update replaces only `Contents/MacOS/ashwend`, breaking the bundle seal; the updater re-seals before relaunch. It runs *from inside the same bundle*, so `--deep` would try to rewrite the live `ashwend-updater` binary. This asymmetry (build sign `--deep`, runtime re-sign non-`--deep`) is load-bearing; do not unify them. The re-sign is best-effort: a de-quarantined bundle still launches from the inner binary's own signature if it fails.

Because the app is not notarized, first launch of a browser-downloaded copy needs one trip through System Settings -> Privacy & Security -> Open Anyway. The `curl | sh install.sh` path avoids even that (curl does not set `com.apple.quarantine`).

## Packaging: the six release assets

`.github/scripts/release_assets.py` is the **single source of truth** for asset names; its module docstring is the canonical rename checklist. Do not maintain a parallel list here. The current set (`ASSETS`):

| Platform | Asset | Role |
| --- | --- | --- |
| Linux x86_64 | `ashwend-x86_64-unknown-linux-gnu.tar.gz` | self-update archive |
| Linux aarch64 | `ashwend-aarch64-unknown-linux-gnu.tar.gz` | self-update archive |
| macOS aarch64 | `ashwend-aarch64-apple-darwin.zip` | self-update archive (zipped `.app`) |
| macOS aarch64 | `ashwend-aarch64-apple-darwin.dmg` | website first-install only |
| Windows x86_64 | `ashwend-x86_64-pc-windows-msvc.zip` | self-update archive |
| Windows x86_64 | `ashwend-x86_64-pc-windows-msvc-setup.exe` | website first-install only |

The bare `.zip`/`.tar.gz` archives are the **self-update transport**. Each ships both binaries (`ashwend` + `ashwend-updater`) side by side. The `.dmg` and `setup.exe` are website-only first-install packages and are **never** consumed by self-update (the updater reads `zip`/`tar.gz`, not a dmg or installer). Never point `HOST_ASSET_NAME` at a `.dmg` or installer.

`ashwend-updater` is auto-discovered from `src/bin/` (no `[[bin]]` section); `Cargo.toml` only sets `default-run = "ashwend"`. The crate version is `0.21.0` (`Cargo.toml`), exposed at runtime as `GAME_VERSION = env!("CARGO_PKG_VERSION")` in `src/protocol/mod.rs:42`.

### Renaming an asset is a multi-file change

Asset names are baked into shipped binaries at compile time (`asset::HOST_ASSET_NAME`). A mismatch silently degrades self-update to "open download page" with no error. Renaming requires updating `release_assets.py` **and every mirror its docstring lists**: `release.yml` build matrix (`asset:`/`dmg:`/`installer:`), `deploy-hetzner.yml`, `src/update/asset.rs` (constants + test), `website/src/data/content.ts`, `website/src/lib/config.test.ts`, `website/public/install.sh`, and this doc. `update-release-asset-links.py` runs with `--require-all` (`release.yml`), so every listed asset (including the `.dmg`/`setup.exe`) must be uploaded or the release fails.

`ARCHIVE_GAME_MEMBER` (`src/update/asset.rs`) must keep matching what `package-release.py` produces, especially the macOS `Ashwend.app/Contents/MacOS/ashwend` path. If you rename the bundle or move binaries inside it, update both sides or extraction fails with `archive did not contain ...`.

### dmg and setup.exe specifics

- **macOS `.dmg`** (`package-release.py` - `build_dmg`): wraps the same ad-hoc-signed `Ashwend.app` with `appdmg` (`npx --yes appdmg`, spec `.github/installer/ashwend-dmg.json`). `appdmg` writes the volume `.DS_Store` directly, so it runs headless on CI where Finder/AppleScript dmg tools hang. It does not re-sign; a post-build `hdiutil` mount + `codesign --verify --deep --strict` fails the release if the bundle lost its seal.
- **Windows `setup.exe`** (`package-release.py` - `build_windows_installer`): compiles `.github/installer/ashwend.iss` with Inno Setup (`iscc`). **Installs per-user to `{localappdata}\Programs\Ashwend` with `PrivilegesRequired=lowest` and `DisableDirPage=yes`** precisely because the self-updater swaps `ashwend.exe` with a non-elevated `std::fs::rename`; Program Files is UAC-protected and would silently break auto-update, and `DisableDirPage` stops a user relocating into a protected dir. The `AppId` GUID (`307311E9-...`) must never change or upgrades stop recognizing prior installs. Inno over MSI: an MSI component table would fight the out-of-band binary swap.

The Windows app icon is embedded as a Win32 resource by `build.rs` via `winresource` from `.github/assets/ashwend.ico`. This embedding is host-gated on `#[cfg(windows)]` (the build script's host, not the target), so cross-compiling Windows from another host skips the icon, as `build.rs`'s own comment warns.

Neither installer is code-signed, so Gatekeeper ("Open Anyway") and SmartScreen ("unknown publisher") prompts remain until notarization / an Authenticode cert lands. The installers improve presentation, not the trust prompts.

The public domain is **ashwend.com**: `website/public/install.sh` documents `curl -fsSL https://www.ashwend.com/install.sh | sh`, `ashwend.iss` uses `https://ashwend.com`, README links `https://www.ashwend.com`. (Not `ashwend.game`.)

> Deploy ordering: `website/src/data/content.ts` points the macOS/Windows download buttons at the `.dmg`/`setup.exe`, which resolve to `releases/latest/download/...`. Deploy the website only after the first release that publishes those assets, or the buttons 404.

## When notarization lands

If a paid Developer ID is acquired: swap the ad-hoc `-` identity in `package-release.py` for the Developer ID plus an `xcrun notarytool` step, and change self-update to swap the **whole bundle** (so the relaunched app stays notarized) instead of the inner binary + ad-hoc re-sign. This is a proposal, not shipped.

## Related docs

- [docs/ui-and-client.md](ui-and-client.md) - where `update_modal` / `current_changelog_modal` / `update_corner_pill` register in the egui frame.
- [docs/voice.md](voice.md) - why the macOS `Info.plist` needs `NSMicrophoneUsageDescription`.
- [docs/build-and-dev.md](build-and-dev.md) - the `./cli` surface for building, testing, and cutting a release.
- [docs/headless-agent-testing.md](headless-agent-testing.md) - the harness that sets `GAME_SKIP_UPDATE_CHECK`.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - the singleplayer world save that `apply_update_system` flushes before relaunch.
