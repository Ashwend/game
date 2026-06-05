//! Persistent anonymous identifier sent to PostHog as `distinct_id`.
//!
//! Stored as a plain UUID v4 string at `<data_dir>/analytics_id`, sibling to
//! [`crate::app::state::ClientSettingsStore`]'s settings file. Kept out of
//! the settings file so resetting display/audio settings does not also reset
//! the user's analytics identity, and so a player who deletes
//! `settings.dat` to "start fresh" does not have their analytics history
//! re-key.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use uuid::Uuid;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Ashwend";
const APPLICATION: &str = "Ashwend";
const FILE_NAME: &str = "analytics_id";

/// Resolve the platform-default location for the anonymous id file.
pub(crate) fn platform_default_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .context("could not resolve the platform data directory")?;
    Ok(dirs.data_dir().join(FILE_NAME))
}

/// Load the UUID from `path` if it exists and parses; otherwise mint a new
/// one and persist it atomically. Returns the resulting UUID.
pub(crate) fn load_or_create(path: &Path) -> Result<Uuid> {
    if let Some(existing) = read_existing(path) {
        return Ok(existing);
    }
    let id = Uuid::new_v4();
    write_atomic(path, &id)?;
    Ok(id)
}

fn read_existing(path: &Path) -> Option<Uuid> {
    let raw = fs::read_to_string(path).ok()?;
    Uuid::parse_str(raw.trim()).ok()
}

fn write_atomic(path: &Path, id: &Uuid) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "could not create analytics state directory {}",
                parent.display()
            )
        })?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("could not create {}", tmp.display()))?;
        file.write_all(id.as_hyphenated().to_string().as_bytes())
            .with_context(|| format!("could not write {}", tmp.display()))?;
        file.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| format!("could not finalize {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("game-distinct-id-{}", Uuid::new_v4()))
    }

    #[test]
    fn load_or_create_is_idempotent() {
        let path = temp_path();
        let first = load_or_create(&path).expect("first call mints id");
        let second = load_or_create(&path).expect("second call loads same id");
        assert_eq!(first, second);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_or_create_replaces_unparseable_file_with_fresh_id() {
        let path = temp_path();
        fs::write(&path, b"not a uuid").unwrap();
        let id = load_or_create(&path).expect("garbage file should be replaced");
        // The new id was written through, so a second read returns the same one.
        let again = load_or_create(&path).expect("subsequent load");
        assert_eq!(id, again);
        let _ = fs::remove_file(&path);
    }
}
