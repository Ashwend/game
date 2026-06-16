//! "Skip this version" persistence.
//!
//! When the player dismisses an update with **Skip this version**, we store
//! that version string so the modal doesn't auto-pop for it again. Kept in its
//! own file (`<data_dir>/skipped_version`) sibling to the settings + analytics
//! id, same `ProjectDirs` + atomic-write pattern as
//! [`crate::analytics::distinct_id`], so clearing display/audio settings
//! doesn't also un-skip an update, and vice versa. The persistent corner pill
//! still shows regardless; only the unprompted boot popup is suppressed.

use std::{
    fs,
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "skipped_version";

fn default_path() -> Option<PathBuf> {
    crate::util::platform::project_dirs().map(|dirs| dirs.data_dir().join(FILE_NAME))
}

/// The version the player last chose to skip, if any.
pub(crate) fn load() -> Option<String> {
    let path = default_path()?;
    read_from(&path)
}

/// Persist `version` as skipped. Best-effort; a write failure just means we'll
/// prompt again next boot, which is harmless.
pub(crate) fn save(version: &str) {
    if let Some(path) = default_path() {
        let _ = write_atomic(&path, version);
    }
}

fn read_from(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn write_atomic(path: &Path, version: &str) -> std::io::Result<()> {
    crate::util::fs::write_atomic(path, version.trim().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("ashwend-skip-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn round_trips_a_version_string() {
        let path = temp_path();
        assert_eq!(read_from(&path), None, "missing file reads as None");
        write_atomic(&path, "0.17.0").unwrap();
        assert_eq!(read_from(&path), Some("0.17.0".to_owned()));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn trims_whitespace_and_treats_blank_as_none() {
        let path = temp_path();
        write_atomic(&path, "  0.18.1\n").unwrap();
        assert_eq!(read_from(&path), Some("0.18.1".to_owned()));
        fs::write(&path, "   \n").unwrap();
        assert_eq!(read_from(&path), None);
        let _ = fs::remove_file(&path);
    }
}
