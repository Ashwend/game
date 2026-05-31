use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use uuid::Uuid;

use crate::{protocol::SteamId, world::MapType};

use super::format::{SAVE_EXTENSION, decode_world_save, save_world_file};
use super::listing::{
    CorruptedWorld, WorldListing, WorldSummary, read_world_name_best_effort,
    uuid_from_save_file_name,
};
use super::types::WorldSave;
use super::validate::normalize_world_name;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Ashwend";
const APPLICATION: &str = "Ashwend";

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

    pub fn list_worlds(&self) -> Result<WorldListing> {
        self.ensure_exists()?;

        let mut worlds = Vec::new();
        let mut corrupted = Vec::new();
        for entry in fs::read_dir(&self.root)
            .with_context(|| format!("could not read world directory {}", self.root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(SAVE_EXTENSION) {
                continue;
            }

            // Per-file isolation: one unreadable save must not hide the rest
            // of the player's worlds. Surface the failure separately instead.
            match self.load_world_file(&path) {
                Ok(save) => worlds.push(WorldSummary::from_save(&save, path)),
                Err(error) => {
                    let file_name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    let id = uuid_from_save_file_name(&file_name);
                    let recovered_name = read_world_name_best_effort(&path);
                    corrupted.push(CorruptedWorld {
                        file_name,
                        id,
                        recovered_name,
                        error: format!("{error:#}"),
                    });
                }
            }
        }

        worlds.sort_by(|a, b| {
            b.created_at_unix
                .cmp(&a.created_at_unix)
                .then(a.name.cmp(&b.name))
        });
        corrupted.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        Ok(WorldListing { worlds, corrupted })
    }

    pub fn create_world(&self, name: &str, owner_steam_id: Option<SteamId>) -> Result<WorldSave> {
        self.create_world_with_map(name, owner_steam_id, MapType::default())
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
        save_world_file(&path, save)
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
        let listing = self.list_worlds()?;
        if let Some(world) = listing
            .worlds
            .into_iter()
            .find(|world| world.name == "Dedicated")
        {
            return self.load_world(world.id);
        }

        self.create_world("Dedicated", owner_steam_id)
    }

    pub(crate) fn world_path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.{SAVE_EXTENSION}"))
    }

    fn load_world_file(&self, path: &Path) -> Result<WorldSave> {
        let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
        decode_world_save(&bytes).with_context(|| format!("could not parse {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::{SAVE_MAGIC, atomic_temp_path, encode_world_save};
    use super::super::types::{PersistedPlayer, WorldSave};
    use super::*;
    use crate::{
        protocol::{ItemStack, PlayerInventoryState, Vec3Net},
        world::ProceduralMapSize,
    };

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
        assert_eq!(save.map, MapType::default());
        assert_eq!(save.admins, vec![123]);
        assert!(!save.map.world_data().blocks.is_empty());

        let loaded = store.load_world(save.id).expect("world should load");
        assert_eq!(loaded.id, save.id);

        let listed = store.list_worlds().expect("world list should load");
        assert_eq!(listed.worlds.len(), 1);
        assert!(listed.corrupted.is_empty());

        store.delete_world(save.id).expect("world should delete");
        let after_delete = store.list_worlds().expect("world list should load");
        assert!(after_delete.worlds.is_empty());
        assert!(after_delete.corrupted.is_empty());

        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn list_worlds_reports_corrupted_files_separately_from_valid_ones() {
        let store = temp_store();
        let good = store
            .create_world("Good", Some(123))
            .expect("good world should save");
        let bad_path = store.root().join(format!("broken.{SAVE_EXTENSION}"));
        std::fs::create_dir_all(store.root()).expect("store dir");
        std::fs::write(&bad_path, b"this is not a real save file")
            .expect("broken save should be written");

        let listing = store.list_worlds().expect("listing should still succeed");

        assert_eq!(listing.worlds.len(), 1);
        assert_eq!(listing.worlds[0].id, good.id);
        assert_eq!(listing.corrupted.len(), 1);
        assert_eq!(listing.corrupted[0].file_name, "broken.save");
        assert!(
            listing.corrupted[0].id.is_none(),
            "non-UUID file name should not yield an id"
        );
        assert!(listing.corrupted[0].recovered_name.is_none());
        assert_eq!(listing.corrupted[0].display_name(), "broken.save");
        assert!(
            !listing.corrupted[0].error.is_empty(),
            "corrupted entry should carry a human-readable error"
        );

        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn corrupted_entry_recovers_name_and_id_when_only_version_mismatches() {
        let store = temp_store();
        let save = WorldSave::new("Recovered Name", Some(123));
        let mut bytes = encode_world_save(&save).expect("save should encode");
        // Stomp the format version so the regular decode path rejects the
        // file, but the postcard payload itself is still well-formed.
        let version_offset = SAVE_MAGIC.len();
        bytes[version_offset..version_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());

        store.ensure_exists().expect("store dir");
        let path = store.root().join(format!("{}.{SAVE_EXTENSION}", save.id));
        std::fs::write(&path, &bytes).expect("bad save should be written");

        let listing = store.list_worlds().expect("listing should still succeed");

        assert!(listing.worlds.is_empty());
        assert_eq!(listing.corrupted.len(), 1);
        let corrupted = &listing.corrupted[0];
        assert_eq!(corrupted.id, Some(save.id));
        assert_eq!(corrupted.recovered_name.as_deref(), Some("Recovered Name"));
        assert_eq!(corrupted.display_name(), "Recovered Name");

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
    fn full_state_round_trips_through_binary_format() {
        let store = temp_store();
        let mut save = store
            .create_world("Stateful", Some(42))
            .expect("world should be created");

        let mut inventory = PlayerInventoryState::empty();
        inventory.active_actionbar_slot = 3;
        inventory.actionbar_slots[3] = Some(ItemStack {
            item_id: "test.item".into(),
            quantity: 7,
        });

        save.state.last_authoritative_tick = 12345;
        save.state.players.push(PersistedPlayer {
            steam_id: 42,
            name: "Tester".to_owned(),
            position: Vec3Net::new(1.0, 2.5, -3.0),
            velocity: Vec3Net::new(0.1, 0.0, 0.2),
            yaw: 1.2,
            pitch: -0.4,
            health: 87.5,
            grounded: true,
            last_processed_input: 9000,
            is_admin: true,
            inventory,
        });
        save.state.resource_nodes = Some(Vec::new());
        save.state.next_dropped_item_id = 99;
        save.state.next_client_id = 5;

        store.save_world(&save).expect("save should write");

        let loaded = store.load_world(save.id).expect("save should load");
        assert_eq!(loaded.state, save.state);
    }
}
