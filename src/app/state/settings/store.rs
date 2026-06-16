use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::*;

use super::data::ClientSettings;
use crate::local_crypto;

// Encrypted-at-rest config (see `save`). The `.dat` extension reflects the
// sealed binary contents; a pre-encryption `settings.json` is a different path
// and is simply ignored (the player starts from defaults).
const SETTINGS_FILE: &str = "settings.dat";

#[derive(Resource, Debug, Clone)]
pub(crate) struct ClientSettingsStore {
    path: PathBuf,
}

impl ClientSettingsStore {
    pub(crate) fn platform_default() -> Result<Self> {
        let project_dirs = crate::util::platform::project_dirs()
            .context("could not resolve the platform config directory")?;
        Ok(Self::new(project_dirs.config_dir().join(SETTINGS_FILE)))
    }

    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) fn load(&self) -> Result<ClientSettings> {
        if !self.path.exists() {
            return Ok(ClientSettings::default());
        }

        // The file is sealed (see `save`). A blob we can't open (a plain-text
        // file from a build that predates encryption, a tampered file, or one
        // written under a different key) returns `Err` here; the caller in
        // `app.rs` already turns that into "fall back to defaults", which
        // doubles as the reset path for the old config format.
        let sealed = fs::read(&self.path)
            .with_context(|| format!("could not read settings {}", self.path.display()))?;
        let json = local_crypto::open(&sealed)
            .with_context(|| format!("could not decrypt settings {}", self.path.display()))?;
        let settings = serde_json::from_slice::<ClientSettings>(&json)
            .with_context(|| format!("could not parse settings {}", self.path.display()))?;
        Ok(settings.sanitized())
    }

    pub(crate) fn save(&self, settings: &ClientSettings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("could not create settings directory {}", parent.display())
            })?;
        }
        let json = serde_json::to_vec(&settings.clone().sanitized())
            .context("could not serialize client settings")?;
        // Seal the whole config so it isn't readable plain text on disk.
        let sealed = local_crypto::seal(&json);
        fs::write(&self.path, sealed)
            .with_context(|| format!("could not write settings {}", self.path.display()))
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}
