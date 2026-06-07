use bevy::{asset::io::embedded::EmbeddedAssetRegistry, prelude::*};
use include_dir::{Dir, include_dir};
use std::path::{Path, PathBuf};

// `include_dir!` (below) is a proc macro, so on stable Rust it can't tell Cargo
// which files it embedded; editing an asset would otherwise leave the *stale*
// bytes baked in until this module happened to recompile for another reason.
// This `include!` pulls in a fingerprint of the `assets/` tree that `build.rs`
// regenerates whenever any asset changes, making this module a recompile
// dependency of that fingerprint and forcing `include_dir!` to re-read the tree.
// Do not remove it. (Plain comment, not a doc comment: the target is a macro
// invocation, which can't carry docs.)
include!(concat!(env!("OUT_DIR"), "/embedded_assets_fingerprint.rs"));

/// Compile-time snapshot of the entire `assets/` tree, baked into the
/// binary. Every file under `assets/` is registered into Bevy's
/// [`EmbeddedAssetRegistry`] at startup, exposed to the rest of the engine
/// as `embedded://<relative-path>`.
///
/// To embed a new asset:
/// 1. Drop the file under `assets/<subdir>/<name>.<ext>`.
/// 2. That's it, `include_dir!` picks it up at the next build.
///
/// The previous per-file `include_bytes!` table was a maintenance tax that
/// scaled linearly with the clip count. With a directory tree of footstep
/// pools, item impact pools, ambient loops, and UI cues, that table would
/// have grown unbounded; the `include_dir` crate gives us the whole tree
/// for one line of source.
static EMBEDDED_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");

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

/// Iterate every embedded asset's `(relative_path, bytes)` pair. Used by
/// the audio manifest to glob pools at startup (e.g. all
/// `movement/footstep-dirt-*.wav`) without re-listing files in source.
pub(crate) fn iter_embedded_assets() -> impl Iterator<Item = (&'static str, &'static [u8])> {
    fn walk(dir: &'static Dir<'static>, out: &mut Vec<(&'static str, &'static [u8])>) {
        for file in dir.files() {
            if let Some(path) = file.path().to_str() {
                out.push((path, file.contents()));
            }
        }
        for sub in dir.dirs() {
            walk(sub, out);
        }
    }
    let mut entries = Vec::new();
    walk(&EMBEDDED_DIR, &mut entries);
    entries.into_iter()
}

/// Look up a single embedded asset's bytes by its `assets/`-relative path
/// (e.g. `"fonts/Cinzel-Bold.ttf"`). Returns the compile-time `&'static`
/// slice from the `include_dir!` tree.
///
/// egui builds its `FontDefinitions` synchronously and wants the raw
/// `&'static [u8]` up front, which the async `AssetServer` path can't hand
/// back, so the title font is pulled straight from the embedded tree here
/// instead of round-tripping through `embedded://`.
pub(crate) fn embedded_bytes(path: &str) -> Option<&'static [u8]> {
    iter_embedded_assets()
        .find(|(p, _)| *p == path)
        .map(|(_, bytes)| bytes)
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
            .expect("EmbeddedAssetRegistry missing, add EmbeddedAssetsPlugin after DefaultPlugins");
        for (path, bytes) in iter_embedded_assets() {
            // Skip macOS finder droppings that sneak in from working
            // directories; they're harmless but pollute the registry.
            if Path::new(path).file_name() == Some(std::ffi::OsStr::new(".DS_Store")) {
                continue;
            }
            // `full_path` only matters when the `embedded_watcher` feature is
            // enabled (we never enable it). Passing the asset path twice
            // keeps the entry self-describing in any debug dump.
            let asset_path = Path::new(path);
            registry.insert_asset(PathBuf::from(path), asset_path, bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_path_prepends_embedded_scheme() {
        assert_eq!(asset_path("ui/click.wav"), "embedded://ui/click.wav");
    }

    #[test]
    fn embedded_tree_contains_at_least_one_audio_asset() {
        // Smoke check: the include_dir! macro found *something* under
        // assets/. If this fails, the macro path is wrong or assets/ is
        // empty in this checkout.
        let has_audio = iter_embedded_assets()
            .any(|(path, _)| path.ends_with(".wav") || path.ends_with(".ogg"));
        assert!(
            has_audio,
            "include_dir!(assets) produced no audio files, check CARGO_MANIFEST_DIR/assets exists"
        );
    }

    #[test]
    fn embedded_asset_bytes_are_non_empty() {
        for (path, bytes) in iter_embedded_assets() {
            // .DS_Store is skipped at registry time but `include_dir!`
            // still picks it up; tolerate it here.
            if path.ends_with(".DS_Store") {
                continue;
            }
            assert!(!bytes.is_empty(), "{path} embedded with zero bytes");
        }
    }
}
