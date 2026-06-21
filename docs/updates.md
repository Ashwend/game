# Updates and self-update

How the shipped client tells the player a newer build exists, shows the
changelog, and updates itself in place. Also covers the macOS `.app` packaging
that this depends on.

## Overview

- On boot, a background thread (the app has no async runtime, so this mirrors
  the [analytics](../src/analytics) worker: one OS thread + blocking `ureq`)
  asks the **public** GitHub releases API for `Ashwend/game`. Dev builds can
  set `GAME_SKIP_UPDATE_CHECK` to disable the check entirely (used by the
  agent test harness so the modal can't cover screenshots; compiled out of
  release builds).
- If the newest stable release is newer than `CARGO_PKG_VERSION`
  (`crate::protocol::GAME_VERSION`), the player sees a modal with the changelog
  for every version between theirs and latest, rendered as markdown
  (`egui_commonmark`).
- They can **Update now**, **Skip this version** (persisted, won't auto-pop
  again), or **Later**. A corner pill (menu screens) / pause-menu row (in-game)
  re-opens the modal at any time.
- **Update now** downloads the host's release archive, verifies its SHA-256
  against GitHub's published `digest`, extracts the new game binary, stages it
  beside the running binary, and hands off to a separate `ashwend-updater`
  process which swaps the binary and relaunches the game after this process
  exits.

Failures never block the game: a flaky network or unparseable response is
treated as "up to date" and logged once.

## Module layout

- [`src/update/`](../src/update), everything that doesn't touch app session
  state:
  - `version.rs`, tiny `MAJOR.MINOR.PATCH` parse/compare (no `semver` dep).
  - `github.rs`, releases client + the pure "what's newer / changelog since"
    logic. The auto-generated **Release Assets** link section is stripped for
    the in-app view.
  - `asset.rs`, host → release-asset name + the binary's path inside the
    archive, all `cfg`-gated per platform.
  - `download.rs`, stream + SHA-256 verify + extract (zip on macOS/Windows,
    tar.gz on Linux) into a staging file **on the install volume** (so the
    swap is an atomic same-filesystem rename).
  - `apply.rs`, locate the sibling updater, compute the relaunch target
    (`.app` on macOS, bare binary elsewhere), spawn it; browser fallback.
  - `skipped.rs`, `<data_dir>/skipped_version` persistence (same
    `ProjectDirs` + atomic-write pattern as the analytics id).
  - `mod.rs`, `UpdatePlugin`, the `UpdateState` resource, the worker, and the
    message pump.
- [`src/bin/ashwend-updater.rs`](../src/bin/ashwend-updater.rs), the helper
  binary. `std`-only, no network, no decompression. Waits for the game to exit,
  swaps the binary (retrying transient locks), and relaunches.
- [`src/app/ui/update.rs`](../src/app/ui/update.rs), the modal + corner pill +
  pause-menu row.
- [`src/app/systems/update.rs`](../src/app/systems/update.rs), 
  `apply_update_system`, which lives in `app` (not `crate::update`) because it
  must save any open world via `ClientRuntime`/`SessionShutdownTasks` before the
  process exits.

## Why a separate updater binary

The process doing the overwrite must not be the file being overwritten, and it
has to outlive the game to relaunch it. Shipping `ashwend-updater` as a distinct
file means it never replaces *itself*, so on Windows the (now-exited) game's
`.exe` is no longer locked and the swap is a plain `std::fs::rename`
(`MOVEFILE_REPLACE_EXISTING`) on every platform, retried briefly for transient
AV/indexer locks. On Unix it also waits for the game's PID to exit (via
`kill -0`) before relaunching so there's never a double instance.

## macOS packaging (`.app`)

The macOS release asset is `ashwend-aarch64-apple-darwin.zip`, a zipped
`Ashwend.app` bundle (built by [`package-release.py`](../.github/scripts/package-release.py)
with `ditto`), not a bare binary. Layout:

```
Ashwend.app/Contents/
  Info.plist               # CFBundleIdentifier=com.Ashwend.Ashwend, version, CFBundleIconFile=AppIcon
  MacOS/ashwend            # the game (CFBundleExecutable)
  MacOS/ashwend-updater    # the self-update helper
  Resources/AppIcon.icns   # dock / Finder icon
```

The icon is **pre-rendered and committed** at
[`.github/assets/AppIcon.icns`](../.github/assets/AppIcon.icns), generated from
the website favicon (`website/public/favicon.svg`). Regenerate only when the
logo changes, rasterize with the **native QuickLook renderer** (ImageMagick
silently drops the SVG's `linearGradient`), then `iconutil`:
`qlmanage -t -s 1024 -o . favicon.svg` → `sips` to each iconset size →
`iconutil -c icns AppIcon.iconset`. It's committed rather than built in CI
because `qlmanage` is unreliable headless. `codesign --deep` seals it; the
self-update re-sign re-seals it.

The bundle is **ad-hoc signed** (`codesign --force --deep --sign -` in
`package-release.py`), **not notarized**, notarization needs a paid Developer
ID, which is deferred. Why ad-hoc and not just unsigned: the Rust toolchain only
applies a *linker* ad-hoc signature to the bare binary, which is invalid as a
bundle's main executable (no sealed `_CodeSignature`), and a *broken* signature
is exactly what makes Gatekeeper say **"damaged, move to Trash"** with no
recourse. A proper ad-hoc bundle signature downgrades that to the ordinary
"Apple can't check it for malware" prompt, which has an **"Open Anyway"** button.

Because it isn't notarized, there's no clean *browser-download* double-click.
Two friction-free paths instead:

- **Install script**, `curl -fsSL https://ashwend.game/install.sh | sh`
  ([website/public/install.sh](../website/public/install.sh)). curl doesn't set
  the `com.apple.quarantine` flag (only browsers do), so the de-quarantined copy
  it drops in `/Applications` launches with **no prompt at all**.
- **Website download button**, quarantined, so first launch needs the one-click
  System Settings → Privacy & Security → **Open Anyway**.

Self-update replaces only `Contents/MacOS/ashwend`, which breaks the bundle
seal, so the updater **re-signs ad-hoc** afterwards with `codesign --force
--sign -` (note: *not* `--deep`, that would rewrite the running
`ashwend-updater` inside the same bundle; non-`--deep` re-signs the main exec and
re-seals resources, leaving the updater untouched). Self-downloaded updates
aren't quarantined, so the re-signed bundle relaunches cleanly.

`Info.plist` also carries `NSMicrophoneUsageDescription`, a bundled app that
opens the mic without it is killed by macOS TCC, and Ashwend captures voice.

**When notarization lands** (paid Developer ID): swap the ad-hoc `-` identity in
`package-release.py` for the Developer ID + an `xcrun notarytool` step, and
change self-update to swap the **whole bundle** (so the relaunched app stays
notarized) instead of the inner binary + ad-hoc re-sign.

Linux ships a `.tar.gz` and Windows a `.zip`, each now containing **both**
binaries side by side.

## Installers (`.dmg` and `setup.exe`)

The bare archives above are the **self-update transport**: the in-app updater
downloads the host's `.zip`/`.tar.gz` ([`asset::HOST_ASSET_NAME`](../src/update/asset.rs))
and extracts the binary with the `zip`/`tar` crates. Those never change. On top
of them, the release also builds **friendlier first-install packages** that the
website download buttons point at. Both kinds are listed in `release_assets.py`
so the release-notes "Release Assets" links cover every file and `--require-all`
enforces all of them were uploaded.

- **macOS, drag-to-Applications `.dmg`** (`ashwend-aarch64-apple-darwin.dmg`).
  Built by [`package-release.py`](../.github/scripts/package-release.py)
  `build_dmg` wrapping the same ad-hoc-signed `Ashwend.app` with
  [`appdmg`](../.github/installer/ashwend-dmg.json) (`npx --yes appdmg`). `appdmg`
  writes the volume's `.DS_Store` directly, so it runs reliably headless on a CI
  macOS runner where the Finder/AppleScript-driven `create-dmg` tools hang. The
  step does **not** re-sign (appdmg's signing is skipped); a post-build
  `hdiutil` mount + `codesign --verify --deep --strict` fails the release if the
  bundle lost its seal inside the dmg. The `.dmg` does **not** replace the
  `.zip`: the updater can't read a dmg, and the no-prompt `curl | sh install.sh`
  path still pulls the `.zip`. A browser-downloaded dmg is quarantined like the
  zip, so first launch of the non-notarized app still needs right-click → Open.
- **Windows, per-user `setup.exe`** (`ashwend-x86_64-pc-windows-msvc-setup.exe`).
  Built by `package-release.py` `build_windows_installer` compiling
  [`ashwend.iss`](../.github/installer/ashwend.iss) with Inno Setup
  (`choco install innosetup`). **Installs per-user to
  `%LOCALAPPDATA%\Programs\Ashwend` (`PrivilegesRequired=lowest`), not Program
  Files**, because the self-updater swaps `ashwend.exe` in place with a
  non-elevated `std::fs::rename`; Program Files is UAC-protected and would
  silently break auto-update. `DisableDirPage=yes` locks the location so a user
  can't relocate it into a protected directory. The installer adds a Start Menu
  shortcut, an optional desktop shortcut, and an uninstaller, all using the
  embedded icon. Like macOS, this is **additional** to the `.zip` the updater
  consumes. (Inno chosen over MSI: an MSI's component table would fight the
  out-of-band binary swap.)

Neither installer is code-signed, so Gatekeeper ("Open Anyway") and SmartScreen
("unknown publisher") prompts remain until notarization / an Authenticode cert
lands; the installers improve presentation, not the trust prompts.

**Windows console + icon.** Shipped Windows builds are GUI-subsystem
(`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` in
[`main.rs`](../src/main.rs) and the updater bin) so double-clicking never flashes
a console; [`crate::console`](../src/console.rs) reattaches to the launching
terminal so the `server`/`admin` CLI still prints when run from a shell. The app
icon is embedded as a Win32 resource by [`build.rs`](../build.rs) via
`winresource` from [`.github/assets/ashwend.ico`](../.github/assets/ashwend.ico)
(regenerate from `AppIcon.icns` with `magick ... -define icon:auto-resize=...`).

> **Deploy ordering:** [`content.ts`](../website/src/data/content.ts) now points
> the macOS/Windows download buttons at the `.dmg`/`setup.exe`. Those resolve to
> `releases/latest/download/...`, so deploy the website only **after** the first
> release that publishes the new assets, or the buttons 404.

## Adding/adjusting

- Release-asset names have a single source of truth:
  [`.github/scripts/release_assets.py`](../.github/scripts/release_assets.py).
  `prepare-release.py` and `update-release-asset-links.py` import it. The
  remaining sites cannot read it and stay hardcoded with a pointer comment;
  when renaming an asset, update all of them: `release.yml` (matrix
  `asset:`/`dmg:`/`installer:`), `deploy-hetzner.yml`, `src/update/asset.rs`
  (`HOST_ASSET_NAME` constants and their test), `website/src/data/content.ts`,
  `website/src/lib/config.test.ts`, `website/public/install.sh`, and the macOS
  asset name mentioned earlier in this doc. The installer output names also
  appear in the Inno script's `OutputBaseFilename`
  ([`ashwend.iss`](../.github/installer/ashwend.iss)) default, overridden per
  build by `package-release.py`'s `/F` flag.
- The friendly installers are built in addition to the bare archive by
  `package-release.py` (`--dmg-asset` / `--installer-asset`); their specs live in
  [`.github/installer/`](../.github/installer). Only the bare-archive names feed
  `src/update/asset.rs` (self-update); the `.dmg`/`setup.exe` are website-only.
- The in-archive binary path is `asset::ARCHIVE_GAME_MEMBER`, keep it matching
  what the packaging script produces.
