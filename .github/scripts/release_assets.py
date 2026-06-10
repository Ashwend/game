"""Single source of truth for the release asset file names.

The Python release scripts in this directory (prepare-release.py and
update-release-asset-links.py) import ASSETS from here. The same file names
are hardcoded in places that cannot import this module; when renaming an
asset, update every one of these sites too:

- .github/workflows/release.yml (build matrix `asset:` entries)
- .github/workflows/deploy-hetzner.yml (Linux ARM server artifact download/scp)
- src/update/asset.rs (self-updater HOST_ASSET_NAME constants and its test)
- website/src/data/content.ts (DOWNLOADS entries behind the download buttons)
- website/src/lib/config.test.ts (sample asset name in a unit test)
- website/public/install.sh (macOS installer ASSET variable)
- docs/updates.md (macOS packaging section)
"""

ASSETS = [
    ("Linux Intel", "ashwend-x86_64-unknown-linux-gnu.tar.gz"),
    ("Linux ARM", "ashwend-aarch64-unknown-linux-gnu.tar.gz"),
    ("macOS ARM", "ashwend-aarch64-apple-darwin.zip"),
    ("Windows Intel", "ashwend-x86_64-pc-windows-msvc.zip"),
]
