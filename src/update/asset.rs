//! Host → release-asset mapping.
//!
//! Names mirror the `asset:` entries in `.github/workflows/release.yml`, whose
//! source of truth is `.github/scripts/release_assets.py` (its docstring lists
//! every hardcoded site). They are deliberately hardcoded here, the constants
//! are baked into shipped binaries at compile time, so when renaming an asset
//! update these alongside the script. Each constant is gated to exactly one
//! host so a build only ever knows about the archive that carries *its*
//! binary; on any host we don't publish a build for, the name is empty and
//! self-update degrades to "open the download page".

/// The release-asset file name for the host this binary was built for.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) const HOST_ASSET_NAME: &str = "ashwend-aarch64-apple-darwin.zip";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(crate) const HOST_ASSET_NAME: &str = "ashwend-x86_64-unknown-linux-gnu.tar.gz";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) const HOST_ASSET_NAME: &str = "ashwend-aarch64-unknown-linux-gnu.tar.gz";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) const HOST_ASSET_NAME: &str = "ashwend-x86_64-pc-windows-msvc.zip";
#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
pub(crate) const HOST_ASSET_NAME: &str = "";

/// Path of the **game** binary *inside* the host's release archive. The macOS
/// asset wraps an `.app`; Linux/Windows are flat archives of the bare binary.
#[cfg(target_os = "macos")]
pub(crate) const ARCHIVE_GAME_MEMBER: &str = "Ashwend.app/Contents/MacOS/ashwend";
#[cfg(target_os = "linux")]
pub(crate) const ARCHIVE_GAME_MEMBER: &str = "ashwend";
#[cfg(target_os = "windows")]
pub(crate) const ARCHIVE_GAME_MEMBER: &str = "ashwend.exe";

/// File name of the sibling updater binary next to the running game binary.
#[cfg(not(target_os = "windows"))]
pub(crate) const UPDATER_BINARY: &str = "ashwend-updater";
#[cfg(target_os = "windows")]
pub(crate) const UPDATER_BINARY: &str = "ashwend-updater.exe";

/// Whether this host publishes a build we know how to self-update.
pub(crate) fn host_is_supported() -> bool {
    !HOST_ASSET_NAME.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_asset_name_is_a_known_release_artifact_or_empty() {
        // On any platform the tests run on, the name must either be empty
        // (unsupported host) or one of the published artifact names, and never
        // contain path separators.
        const KNOWN: &[&str] = &[
            "",
            "ashwend-aarch64-apple-darwin.zip",
            "ashwend-x86_64-unknown-linux-gnu.tar.gz",
            "ashwend-aarch64-unknown-linux-gnu.tar.gz",
            "ashwend-x86_64-pc-windows-msvc.zip",
        ];
        assert!(KNOWN.contains(&HOST_ASSET_NAME));
        assert!(!HOST_ASSET_NAME.contains('/'));
    }

    #[test]
    fn supported_host_has_a_member_and_an_asset() {
        // The hosts we build for must agree across all three constants.
        if host_is_supported() {
            assert!(!ARCHIVE_GAME_MEMBER.is_empty());
            assert!(UPDATER_BINARY.starts_with("ashwend-updater"));
        }
    }
}
