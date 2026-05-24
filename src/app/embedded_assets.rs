use bevy::{asset::io::embedded::EmbeddedAssetRegistry, prelude::*};
use std::path::{Path, PathBuf};

/// Bakes every audio file into the binary at compile time so a published
/// build is a single self-contained executable — no sibling `assets/`
/// folder to ship, no platform-specific resource bundles. Each file is
/// registered into Bevy's `embedded` [`AssetSource`] under its original
/// path, exposed to the rest of the engine via the `embedded://<path>`
/// URI.
///
/// To embed a new asset:
/// 1. Drop the file under `assets/<subdir>/<name>.<ext>`.
/// 2. Add a row to [`EMBEDDED_ASSETS`] below.
/// 3. Reference it from gameplay code as `embedded://<subdir>/<name>.<ext>`
///    via [`asset_path`].
///
/// The `include_bytes!` paths are relative to *this* file
/// (`src/app/embedded_assets.rs`), hence the `../../assets/...` prefix.
struct EmbeddedAsset {
    asset_path: &'static str,
    bytes: &'static [u8],
}

const EMBEDDED_ASSETS: &[EmbeddedAsset] = &[
    EmbeddedAsset {
        asset_path: "ui/button-click.wav",
        bytes: include_bytes!("../../assets/ui/button-click.wav"),
    },
    EmbeddedAsset {
        asset_path: "ui/button-hover.wav",
        bytes: include_bytes!("../../assets/ui/button-hover.wav"),
    },
    EmbeddedAsset {
        asset_path: "main-screen/ambient-music.wav",
        bytes: include_bytes!("../../assets/main-screen/ambient-music.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/hatchet-tree-1.wav",
        bytes: include_bytes!("../../assets/items/hatchet-tree-1.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/hatchet-tree-2.wav",
        bytes: include_bytes!("../../assets/items/hatchet-tree-2.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/hatchet-tree-3.wav",
        bytes: include_bytes!("../../assets/items/hatchet-tree-3.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/pickaxe-ore-node-1.wav",
        bytes: include_bytes!("../../assets/items/pickaxe-ore-node-1.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/pickaxe-ore-node-2.wav",
        bytes: include_bytes!("../../assets/items/pickaxe-ore-node-2.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/pickaxe-ore-node-3.wav",
        bytes: include_bytes!("../../assets/items/pickaxe-ore-node-3.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/miss-1.wav",
        bytes: include_bytes!("../../assets/items/miss-1.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/miss-2.wav",
        bytes: include_bytes!("../../assets/items/miss-2.wav"),
    },
    EmbeddedAsset {
        asset_path: "items/miss-3.wav",
        bytes: include_bytes!("../../assets/items/miss-3.wav"),
    },
    EmbeddedAsset {
        asset_path: "world/tree-fall.wav",
        bytes: include_bytes!("../../assets/world/tree-fall.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-01.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-01.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-02.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-02.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-03.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-03.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-04.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-04.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-05.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-05.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-06.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-06.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-07.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-07.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-08.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-08.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-09.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-09.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-10.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-10.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-11.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-11.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-dirt-12.wav",
        bytes: include_bytes!("../../assets/movement/footstep-dirt-12.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-01.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-01.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-02.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-02.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-03.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-03.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-04.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-04.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-05.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-05.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-06.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-06.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-07.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-07.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-08.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-08.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-09.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-09.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-10.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-10.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-11.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-11.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-wood-12.wav",
        bytes: include_bytes!("../../assets/movement/footstep-wood-12.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-01.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-01.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-02.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-02.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-03.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-03.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-04.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-04.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-05.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-05.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-06.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-06.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-07.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-07.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-08.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-08.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-09.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-09.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-10.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-10.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-11.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-11.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-concrete-12.wav",
        bytes: include_bytes!("../../assets/movement/footstep-concrete-12.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-01.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-01.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-02.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-02.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-03.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-03.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-04.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-04.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-05.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-05.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-06.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-06.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-07.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-07.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-08.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-08.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-09.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-09.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-10.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-10.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-11.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-11.wav"),
    },
    EmbeddedAsset {
        asset_path: "movement/footstep-sand-12.wav",
        bytes: include_bytes!("../../assets/movement/footstep-sand-12.wav"),
    },
];

/// URI prefix all embedded asset paths share. Loading `embedded://foo.wav`
/// routes through [`EmbeddedAssetRegistry`] instead of the filesystem.
pub(crate) const EMBEDDED_ASSET_PREFIX: &str = "embedded://";

/// Helper for code that needs to hand a load path to `asset_server.load(...)`.
/// Returns a `String` because the asset paths must include the prefix at
/// load time and the original constants live in code as plain
/// "subdir/file.ext" tokens.
pub(crate) fn asset_path(path: &str) -> String {
    format!("{EMBEDDED_ASSET_PREFIX}{path}")
}

pub(crate) struct EmbeddedAssetsPlugin;

impl Plugin for EmbeddedAssetsPlugin {
    fn build(&self, app: &mut App) {
        // `AssetPlugin` (added by `DefaultPlugins`) initialises
        // `EmbeddedAssetRegistry`; this plugin must therefore be added
        // *after* `DefaultPlugins` or the resource won't exist yet.
        let registry = app
            .world_mut()
            .get_resource_mut::<EmbeddedAssetRegistry>()
            .expect(
                "EmbeddedAssetRegistry missing — add EmbeddedAssetsPlugin after DefaultPlugins",
            );
        for asset in EMBEDDED_ASSETS {
            // `full_path` only matters when the `embedded_watcher` feature is
            // enabled (we never enable it). Passing the asset path twice
            // keeps the entry self-describing in any debug dump.
            let asset_path = Path::new(asset.asset_path);
            registry.insert_asset(PathBuf::from(asset.asset_path), asset_path, asset.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_asset_carries_some_bytes() {
        for asset in EMBEDDED_ASSETS {
            assert!(
                !asset.bytes.is_empty(),
                "{} embedded with zero bytes — include_bytes! source likely missing",
                asset.asset_path
            );
        }
    }

    #[test]
    fn asset_path_prepends_embedded_scheme() {
        assert_eq!(asset_path("ui/click.wav"), "embedded://ui/click.wav");
    }

    #[test]
    fn embedded_asset_paths_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for asset in EMBEDDED_ASSETS {
            assert!(
                seen.insert(asset.asset_path),
                "duplicate embedded asset path: {}",
                asset.asset_path
            );
        }
    }
}
