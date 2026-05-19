use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::*;
use directories::ProjectDirs;

use super::data::ClientSettings;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Game";
const APPLICATION: &str = "Game";
const SETTINGS_FILE: &str = "settings.json";

#[derive(Resource, Debug, Clone)]
pub(crate) struct ClientSettingsStore {
    path: PathBuf,
}

impl ClientSettingsStore {
    pub(crate) fn platform_default() -> Result<Self> {
        let project_dirs = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
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

        let json = fs::read_to_string(&self.path)
            .with_context(|| format!("could not read settings {}", self.path.display()))?;
        let settings = serde_json::from_str::<ClientSettings>(&json)
            .with_context(|| format!("could not parse settings {}", self.path.display()))?;
        Ok(settings.sanitized())
    }

    pub(crate) fn save(&self, settings: &ClientSettings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("could not create settings directory {}", parent.display())
            })?;
        }
        let json = serde_json::to_string_pretty(&settings.clone().sanitized())
            .context("could not serialize client settings")?;
        fs::write(&self.path, json)
            .with_context(|| format!("could not write settings {}", self.path.display()))
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}
