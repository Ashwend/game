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

## Adding/adjusting

- Release-asset names have a single source of truth:
  [`.github/scripts/release_assets.py`](../.github/scripts/release_assets.py).
  `prepare-release.py` and `update-release-asset-links.py` import it. The
  remaining sites cannot read it and stay hardcoded with a pointer comment;
  when renaming an asset, update all of them: `release.yml` (matrix `asset:`),
  `deploy-hetzner.yml`, `src/update/asset.rs` (`HOST_ASSET_NAME` constants and
  their test), `website/src/data/content.ts`, `website/src/lib/config.test.ts`,
  `website/public/install.sh`, and the macOS asset name mentioned earlier in
  this doc.
- The in-archive binary path is `asset::ARCHIVE_GAME_MEMBER`, keep it matching
  what the packaging script produces.
