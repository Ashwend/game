"""Single source of truth for the release asset file names.

The Python release scripts in this directory (prepare-release.py and
update-release-asset-links.py) import ASSETS from here. The same file names
are hardcoded in places that cannot import this module; when renaming an
asset, update every one of these sites too:

- .github/workflows/release.yml (build matrix `asset:`/`dmg:`/`installer:` entries)
- .github/workflows/deploy-hetzner.yml (Linux ARM server artifact download/scp)
- src/update/asset.rs (self-updater HOST_ASSET_NAME constants and its test)
- website/src/data/content.ts (DOWNLOADS entries behind the download buttons)
- website/src/lib/config.test.ts (sample asset name in a unit test)
- website/public/install.sh (macOS installer ASSET variable)
- docs/updates.md (macOS packaging + installer section)

Two kinds of asset ship per desktop platform:
- the bare archive (.tar.gz / .zip), which is the SELF-UPDATE transport the
  in-app updater downloads and extracts (src/update/asset.rs HOST_ASSET_NAME);
- the friendly first-install package (.dmg drag-to-Applications on macOS, a
  per-user Inno Setup setup.exe on Windows) the website download buttons point
  at. These are built in addition to the archive and are NOT consumed by
  self-update (the updater reads zip/tar.gz, not a dmg or an installer).
Both are listed here so the release-notes "Release Assets" section links every
file and `--require-all` enforces that all of them were uploaded.
"""

ASSETS = [
    ("Linux Intel", "ashwend-x86_64-unknown-linux-gnu.tar.gz"),
    ("Linux ARM", "ashwend-aarch64-unknown-linux-gnu.tar.gz"),
    ("macOS ARM", "ashwend-aarch64-apple-darwin.zip"),
    ("macOS ARM installer", "ashwend-aarch64-apple-darwin.dmg"),
    ("Windows Intel", "ashwend-x86_64-pc-windows-msvc.zip"),
    ("Windows Intel installer", "ashwend-x86_64-pc-windows-msvc-setup.exe"),
]
