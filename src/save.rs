use std::{
    ffi::OsString,
    fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{protocol::SteamId, world::MapType};

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Game";
const APPLICATION: &str = "Game";

#[derive(Debug, Clone)]
pub struct WorldStore {
    root: PathBuf,
}

impl WorldStore {
    pub fn platform_default() -> Result<Self> {
        let project_dirs = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
            .context("could not resolve the platform data directory")?;
        Ok(Self::new(project_dirs.data_dir().join("worlds")))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure_exists(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("could not create world directory {}", self.root.display()))
    }

    pub fn list_worlds(&self) -> Result<Vec<WorldSummary>> {
        self.ensure_exists()?;

        let mut worlds = Vec::new();
        for entry in fs::read_dir(&self.root)
            .with_context(|| format!("could not read world directory {}", self.root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let save = self.load_world_file(&path)?;
            worlds.push(WorldSummary::from_save(&save, path));
        }

        worlds.sort_by(|a, b| {
            b.created_at_unix
                .cmp(&a.created_at_unix)
                .then(a.name.cmp(&b.name))
        });
        Ok(worlds)
    }

    pub fn create_world(&self, name: &str, owner_steam_id: Option<SteamId>) -> Result<WorldSave> {
        self.create_world_with_map(name, owner_steam_id, MapType::Test)
    }

    pub fn create_world_with_map(
        &self,
        name: &str,
        owner_steam_id: Option<SteamId>,
        map: MapType,
    ) -> Result<WorldSave> {
        self.ensure_exists()?;

        let save = WorldSave::new_with_map(name, owner_steam_id, map);
        self.save_world(&save)?;
        Ok(save)
    }

    pub fn load_world(&self, id: Uuid) -> Result<WorldSave> {
        self.load_world_file(&self.world_path(id))
    }

    pub fn save_world(&self, save: &WorldSave) -> Result<()> {
        self.ensure_exists()?;

        let path = self.world_path(save.id);
        let json = serde_json::to_string_pretty(save).context("could not serialize world save")?;
        write_file_atomically(&path, json.as_bytes())
            .with_context(|| format!("could not write world {}", path.display()))
    }

    pub fn rename_world(&self, id: Uuid, name: &str) -> Result<WorldSave> {
        let mut save = self.load_world(id)?;
        save.name = normalize_world_name(name);
        self.save_world(&save)?;
        Ok(save)
    }

    pub fn delete_world(&self, id: Uuid) -> Result<()> {
        let path = self.world_path(id);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("could not delete world {}", path.display()))?;
        }
        Ok(())
    }

    pub fn load_or_create_dedicated(&self, owner_steam_id: Option<SteamId>) -> Result<WorldSave> {
        let worlds = self.list_worlds()?;
        if let Some(world) = worlds.into_iter().find(|world| world.name == "Dedicated") {
            return self.load_world(world.id);
        }

        self.create_world("Dedicated", owner_steam_id)
    }

    fn world_path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    fn load_world_file(&self, path: &Path) -> Result<WorldSave> {
        let json = fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        serde_json::from_str(&json).with_context(|| format!("could not parse {}", path.display()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldSave {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub map: MapType,
    pub created_at_unix: u64,
    pub admins: Vec<SteamId>,
    pub state: WorldStateSave,
}

impl WorldSave {
    pub fn new(name: &str, owner_steam_id: Option<SteamId>) -> Self {
        Self::new_with_map(name, owner_steam_id, MapType::Test)
    }

    pub fn new_with_map(name: &str, owner_steam_id: Option<SteamId>, map: MapType) -> Self {
        let id = Uuid::new_v4();
        let mut admins = Vec::new();
        if let Some(owner_steam_id) = owner_steam_id {
            admins.push(owner_steam_id);
        }

        Self {
            id,
            name: normalize_world_name(name),
            map,
            created_at_unix: now_unix(),
            admins,
            state: WorldStateSave::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorldStateSave {
    pub last_authoritative_tick: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorldSummary {
    pub id: Uuid,
    pub name: String,
    pub map: MapType,
    pub created_at_unix: u64,
    pub path: PathBuf,
}

impl WorldSummary {
    fn from_save(save: &WorldSave, path: PathBuf) -> Self {
        Self {
            id: save.id,
            name: save.name.clone(),
            map: save.map.clone(),
            created_at_unix: save.created_at_unix,
            path,
        }
    }
}

fn normalize_world_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "New World".to_owned()
    } else {
        trimmed.chars().take(64).collect()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let temp_path = atomic_temp_path(path)?;
    let result = (|| -> Result<()> {
        let mut file = File::create(&temp_path)
            .with_context(|| format!("could not create temp save {}", temp_path.display()))?;
        file.write_all(contents)
            .with_context(|| format!("could not write temp save {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("could not sync temp save {}", temp_path.display()))?;
        replace_file(&temp_path, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn atomic_temp_path(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .context("could not build temp save path without a file name")?;
    let mut temp_name = OsString::from(file_name);
    temp_name.push(format!(".tmp-{}", std::process::id()));
    Ok(path.with_file_name(temp_name))
}

#[cfg(not(windows))]
fn replace_file(temp_path: &Path, path: &Path) -> Result<()> {
    fs::rename(temp_path, path).with_context(|| {
        format!(
            "could not replace {} with {}",
            path.display(),
            temp_path.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(temp_path: &Path, path: &Path) -> Result<()> {
    let backup_path = atomic_backup_path(path)?;
    if path.exists() {
        let _ = fs::remove_file(&backup_path);
        fs::rename(path, &backup_path).with_context(|| {
            format!(
                "could not move existing save {} to {}",
                path.display(),
                backup_path.display()
            )
        })?;
    }

    match fs::rename(temp_path, path) {
        Ok(()) => {
            let _ = fs::remove_file(&backup_path);
            Ok(())
        }
        Err(error) => {
            if backup_path.exists() {
                let _ = fs::rename(&backup_path, path);
            }
            Err(error).with_context(|| {
                format!(
                    "could not replace {} with {}",
                    path.display(),
                    temp_path.display()
                )
            })
        }
    }
}

#[cfg(windows)]
fn atomic_backup_path(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .context("could not build backup save path without a file name")?;
    let mut backup_name = OsString::from(file_name);
    backup_name.push(format!(".bak-{}", std::process::id()));
    Ok(path.with_file_name(backup_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::ProceduralMapSize;

    fn temp_store() -> WorldStore {
        WorldStore::new(std::env::temp_dir().join(format!("game-save-test-{}", Uuid::new_v4())))
    }

    #[test]
    fn create_load_and_delete_world() {
        let store = temp_store();
        let save = store
            .create_world("  Test World  ", Some(123))
            .expect("world should be created");

        assert_eq!(save.name, "Test World");
        assert_eq!(save.map, MapType::Test);
        assert_eq!(save.admins, vec![123]);
        assert!(!save.map.world_data().blocks.is_empty());

        let loaded = store.load_world(save.id).expect("world should load");
        assert_eq!(loaded.id, save.id);

        let listed = store.list_worlds().expect("world list should load");
        assert_eq!(listed.len(), 1);

        store.delete_world(save.id).expect("world should delete");
        assert!(
            store
                .list_worlds()
                .expect("world list should load")
                .is_empty()
        );

        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn create_world_with_map_persists_map_settings() {
        let store = temp_store();
        let save = store
            .create_world_with_map(
                "Procedural",
                Some(123),
                MapType::Procedural {
                    seed: 99,
                    size: ProceduralMapSize::Large,
                },
            )
            .expect("world should be created");

        let loaded = store.load_world(save.id).expect("world should load");
        assert_eq!(
            loaded.map,
            MapType::Procedural {
                seed: 99,
                size: ProceduralMapSize::Large,
            }
        );

        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn rename_world_preserves_other_save_fields() {
        let store = temp_store();
        let save = store
            .create_world_with_map(
                "Original",
                Some(123),
                MapType::Procedural {
                    seed: 99,
                    size: ProceduralMapSize::Large,
                },
            )
            .expect("world should be created");

        let renamed = store
            .rename_world(save.id, "  Renamed  ")
            .expect("world should rename");

        assert_eq!(renamed.name, "Renamed");
        assert_eq!(renamed.id, save.id);
        assert_eq!(renamed.map, save.map);
        assert_eq!(renamed.admins, save.admins);

        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn failed_temp_write_keeps_existing_world_file() {
        let store = temp_store();
        let mut save = store
            .create_world("Original", Some(123))
            .expect("world should be created");
        let path = store.world_path(save.id);
        let temp_path = atomic_temp_path(&path).expect("temp path should resolve");
        fs::create_dir_all(&temp_path).expect("temp blocker should be created");

        save.name = "Updated".to_owned();
        assert!(store.save_world(&save).is_err());

        fs::remove_dir_all(&temp_path).expect("temp blocker should be removed");
        let loaded = store.load_world(save.id).expect("world should still load");
        assert_eq!(loaded.name, "Original");

        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn old_seeded_saves_load_as_test_maps() {
        let id = Uuid::new_v4();
        let json = format!(
            r#"{{
                "id": "{id}",
                "name": "Old World",
                "seed": 123,
                "created_at_unix": 1,
                "admins": [123],
                "world": {{"floor_size": 80.0, "blocks": []}},
                "state": {{"last_authoritative_tick": 5}}
            }}"#
        );

        let save: WorldSave = serde_json::from_str(&json).expect("old save should load");

        assert_eq!(save.id, id);
        assert_eq!(save.map, MapType::Test);
        assert_eq!(save.state.last_authoritative_tick, 5);
    }

    #[test]
    fn old_procedural_maps_default_to_medium_size() {
        let id = Uuid::new_v4();
        let json = format!(
            r#"{{
                "id": "{id}",
                "name": "Old Procedural",
                "map": {{"procedural": {{"seed": 123}}}},
                "created_at_unix": 1,
                "admins": [123],
                "state": {{"last_authoritative_tick": 5}}
            }}"#
        );

        let save: WorldSave = serde_json::from_str(&json).expect("old save should load");

        assert_eq!(
            save.map,
            MapType::Procedural {
                seed: 123,
                size: ProceduralMapSize::Medium,
            }
        );
    }
}
