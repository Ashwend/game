use std::{
    ffi::OsString,
    fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    protocol::{
        ClientId, DroppedItemId, DroppedWorldItem, PlayerInventoryState, ResourceNodeState,
        SteamId, Vec3Net,
    },
    world::MapType,
    world_time::{DEFAULT_START_SECONDS, WorldTime},
};

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Game";
const APPLICATION: &str = "Game";

const SAVE_EXTENSION: &str = "save";
const SAVE_MAGIC: &[u8; 8] = b"GAMESAVE";
/// Bump on every breaking change to the on-disk schema. Old saves with a
/// different version are rejected; there is no migration path.
///
/// `2` added `ResourceNodeState::respawn_progress` for the regenerating-node
/// flow. Older v1 saves don't include that field and would be misread
/// (postcard is positional), so they are rejected at load time and surfaced
/// in the worlds-screen "couldn't load" banner.
///
/// `3` added the persistent day/night clock (`world_time_seconds_of_day` and
/// `world_time_multiplier`) on `WorldStateSave`. Same story as v2: postcard
/// layout drift, so older saves are rejected with a "couldn't load" banner.
const SAVE_FORMAT_VERSION: u32 = 3;
/// zstd level 5 sits in the sweet spot for save files: ~70-75% size reduction
/// at >100MB/s compression and ~1GB/s decompression.
const ZSTD_LEVEL: i32 = 5;

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

    fn world_path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.{SAVE_EXTENSION}"))
    }

    fn load_world_file(&self, path: &Path) -> Result<WorldSave> {
        let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
        decode_world_save(&bytes).with_context(|| format!("could not parse {}", path.display()))
    }
}

pub fn save_world_file(path: &Path, save: &WorldSave) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create world directory {}", parent.display()))?;
    }

    let bytes = encode_world_save(save).context("could not serialize world save")?;
    write_file_atomically(path, &bytes)
        .with_context(|| format!("could not write world {}", path.display()))
}

pub fn load_world_file(path: &Path) -> Result<WorldSave> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    decode_world_save(&bytes).with_context(|| format!("could not parse {}", path.display()))
}

fn encode_world_save(save: &WorldSave) -> Result<Vec<u8>> {
    let payload = postcard::to_allocvec(save).context("could not postcard-encode world save")?;
    let compressed = zstd::stream::encode_all(payload.as_slice(), ZSTD_LEVEL)
        .context("could not zstd-compress world save")?;

    let mut out = Vec::with_capacity(SAVE_MAGIC.len() + 4 + compressed.len());
    out.extend_from_slice(SAVE_MAGIC);
    out.extend_from_slice(&SAVE_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

fn decode_world_save(bytes: &[u8]) -> Result<WorldSave> {
    if bytes.len() < SAVE_MAGIC.len() + 4 {
        bail!("save file is truncated");
    }
    if &bytes[..SAVE_MAGIC.len()] != SAVE_MAGIC {
        bail!("save file does not have a GAMESAVE header");
    }
    let version_bytes: [u8; 4] = bytes[SAVE_MAGIC.len()..SAVE_MAGIC.len() + 4]
        .try_into()
        .map_err(|_| anyhow!("save file version field is malformed"))?;
    let version = u32::from_le_bytes(version_bytes);
    if version != SAVE_FORMAT_VERSION {
        bail!("save file version {version} is not supported (expected {SAVE_FORMAT_VERSION})");
    }

    let compressed = &bytes[SAVE_MAGIC.len() + 4..];
    let payload =
        zstd::stream::decode_all(compressed).context("could not zstd-decompress world save")?;
    postcard::from_bytes(&payload).context("could not postcard-decode world save")
}

/// Minimal prefix of [`WorldSave`] used to recover a name for unloadable
/// saves. Postcard is positional, so as long as the on-disk schema still
/// starts with `id` then `name` (which it has for every shipped format
/// version), this can deserialize the first two fields even when the full
/// `WorldSave` decode fails because of a later field change or a version
/// mismatch. Anything after `name` is left in the trailing bytes and
/// ignored.
#[derive(Debug, Clone, Deserialize)]
struct WorldSaveNamePrefix {
    #[allow(dead_code)]
    id: Uuid,
    name: String,
}

fn uuid_from_save_file_name(file_name: &str) -> Option<Uuid> {
    let stem = file_name.strip_suffix(&format!(".{SAVE_EXTENSION}"))?;
    Uuid::parse_str(stem).ok()
}

fn read_world_name_best_effort(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    decode_world_name_best_effort(&bytes)
}

/// Try to recover just the world's display name from a save file, ignoring
/// version mismatches and trailing payload errors. Returns `None` if even
/// the header/compression layer can't be peeled back, or if the recovered
/// name is empty / nothing but control characters (which happens when the
/// postcard layout itself has drifted and the decode is reading garbage
/// bytes as a string length + payload).
fn decode_world_name_best_effort(bytes: &[u8]) -> Option<String> {
    if bytes.len() < SAVE_MAGIC.len() + 4 {
        return None;
    }
    if &bytes[..SAVE_MAGIC.len()] != SAVE_MAGIC {
        return None;
    }
    let compressed = &bytes[SAVE_MAGIC.len() + 4..];
    let payload = zstd::stream::decode_all(compressed).ok()?;
    let (prefix, _rest) = postcard::take_from_bytes::<WorldSaveNamePrefix>(&payload).ok()?;
    let name = prefix.name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_owned())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldSave {
    pub id: Uuid,
    pub name: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldStateSave {
    pub last_authoritative_tick: u64,
    pub players: Vec<PersistedPlayer>,
    pub dropped_items: Vec<DroppedWorldItem>,
    /// `None` while the world has never been hosted; once a server runs, this
    /// is always `Some` (even if empty) so harvested resources don't respawn.
    pub resource_nodes: Option<Vec<ResourceNodeState>>,
    #[serde(default = "default_next_id")]
    pub next_dropped_item_id: DroppedItemId,
    #[serde(default = "default_next_id")]
    pub next_client_id: ClientId,
    /// Persisted day/night clock — wall-clock seconds within the in-game
    /// day. Reload picks up wherever the last session left off so the world
    /// doesn't jump back to morning every restart.
    #[serde(default = "default_world_time_seconds")]
    pub world_time_seconds_of_day: f32,
    /// Persisted day/night multiplier. Admins can change it via the
    /// `/speed` command; the value survives a save round-trip.
    #[serde(default = "default_world_time_multiplier")]
    pub world_time_multiplier: f32,
}

impl Default for WorldStateSave {
    fn default() -> Self {
        Self {
            last_authoritative_tick: 0,
            players: Vec::new(),
            dropped_items: Vec::new(),
            resource_nodes: None,
            next_dropped_item_id: default_next_id(),
            next_client_id: default_next_id(),
            world_time_seconds_of_day: default_world_time_seconds(),
            world_time_multiplier: default_world_time_multiplier(),
        }
    }
}

impl WorldStateSave {
    pub fn world_time(&self) -> WorldTime {
        let mut time = WorldTime {
            seconds_of_day: self.world_time_seconds_of_day,
            multiplier: self.world_time_multiplier,
        };
        // Re-clamp on load — a save edited by hand or produced by a future
        // version we tolerate-via-default could carry a value outside
        // the safe range. Cheaper to fix once on load than on every tick.
        time.set_seconds(time.seconds_of_day);
        time.set_multiplier(time.multiplier);
        time
    }
}

fn default_next_id() -> u64 {
    1
}

fn default_world_time_seconds() -> f32 {
    DEFAULT_START_SECONDS
}

fn default_world_time_multiplier() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedPlayer {
    pub steam_id: SteamId,
    pub name: String,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub grounded: bool,
    pub last_processed_input: u64,
    pub is_admin: bool,
    pub inventory: PlayerInventoryState,
}

/// Result of a `list_worlds()` call. Loadable saves are returned in `worlds`;
/// any save files that failed to parse (truncated, wrong magic, bad format
/// version, corrupted zstd payload, …) are surfaced separately in
/// `corrupted` so the player can see what's broken instead of being shown
/// an empty list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldListing {
    pub worlds: Vec<WorldSummary>,
    pub corrupted: Vec<CorruptedWorld>,
}

/// A save file that was present on disk but could not be loaded. The file
/// name is preserved (rather than the parsed UUID) because the parse is
/// exactly what failed — there's no save struct to extract an id from.
///
/// `id` is recovered from the file name (`{uuid}.save`) when possible so the
/// UI can still wire up a Delete action against the same `WorldStore::delete_world`
/// path it uses for healthy worlds. `recovered_name` is a best-effort decode
/// of the save's name field; it lets the worlds list show something
/// human-readable instead of a raw file name when the failure is something
/// other than a postcard layout change.
#[derive(Debug, Clone, PartialEq)]
pub struct CorruptedWorld {
    pub file_name: String,
    pub id: Option<Uuid>,
    pub recovered_name: Option<String>,
    pub error: String,
}

impl CorruptedWorld {
    /// Display name for the worlds list. Falls back to the file name if the
    /// best-effort recovery turned up nothing (or the recovered name is
    /// empty/control-only — postcard layout drift can produce junk bytes
    /// that decode but aren't human-readable).
    pub fn display_name(&self) -> &str {
        self.recovered_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.file_name)
    }
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

/// Maximum number of characters allowed in a player-supplied world name.
/// Saves themselves can hold the historical 64-character form (via
/// `normalize_world_name`), but the UI rejects new inputs above this cap so
/// names stay legible in the worlds list.
pub const MAX_WORLD_NAME_LEN: usize = 48;

/// Validate a player-supplied world name. Returns the canonical (trimmed)
/// form on success, or a human-readable error otherwise.
///
/// The rules are intentionally tighter than `normalize_world_name`'s fallback
/// behaviour: callers that surface validation to the player should reject
/// rather than silently fixing up the input.
pub fn validate_world_name(name: &str) -> Result<&str, &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Name cannot be empty.");
    }
    let char_count = trimmed.chars().count();
    if char_count > MAX_WORLD_NAME_LEN {
        return Err("Name is too long (48 characters max).");
    }
    for ch in trimmed.chars() {
        if ch.is_control() {
            return Err("Name cannot contain control characters.");
        }
        if matches!(ch, '/' | '\\') {
            return Err("Name cannot contain '/' or '\\'.");
        }
    }
    Ok(trimmed)
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
    use crate::{
        protocol::{ItemStack, PlayerInventoryState},
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
        assert_eq!(save.map, MapType::Test);
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
    fn save_world_file_writes_custom_paths() {
        let root = std::env::temp_dir().join(format!("game-save-file-test-{}", Uuid::new_v4()));
        let path = root.join("nested").join("world.save");
        let save = WorldSave::new("Dedicated File", Some(123));

        save_world_file(&path, &save).expect("world file should save");

        let bytes = fs::read(&path).expect("world file should exist");
        let loaded = decode_world_save(&bytes).expect("world file should parse");
        assert_eq!(loaded.id, save.id);
        assert_eq!(loaded.name, "Dedicated File");

        let _ = fs::remove_dir_all(root);
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

    #[test]
    fn rejects_files_without_magic_header() {
        let err = decode_world_save(b"not a save file at all").unwrap_err();
        assert!(err.to_string().contains("GAMESAVE"));
    }

    #[test]
    fn rejects_mismatched_format_version() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(SAVE_MAGIC);
        bytes.extend_from_slice(&999u32.to_le_bytes());
        let err = decode_world_save(&bytes).unwrap_err();
        assert!(err.to_string().contains("version 999"));
    }

    #[test]
    fn validate_world_name_accepts_normal_input_and_trims() {
        assert_eq!(
            validate_world_name("  Spruce Valley  "),
            Ok("Spruce Valley")
        );
        assert_eq!(validate_world_name("a"), Ok("a"));
    }

    #[test]
    fn validate_world_name_rejects_empty_and_whitespace_only() {
        assert!(validate_world_name("").is_err());
        assert!(validate_world_name("   \t  ").is_err());
    }

    #[test]
    fn validate_world_name_rejects_overflowing_names() {
        let too_long: String = "a".repeat(MAX_WORLD_NAME_LEN + 1);
        assert!(validate_world_name(&too_long).is_err());
        let at_cap: String = "a".repeat(MAX_WORLD_NAME_LEN);
        assert!(validate_world_name(&at_cap).is_ok());
    }

    #[test]
    fn validate_world_name_rejects_path_separators_and_control_chars() {
        assert!(validate_world_name("nice/name").is_err());
        assert!(validate_world_name("nice\\name").is_err());
        assert!(validate_world_name("nice\nname").is_err());
        assert!(validate_world_name("nice\tname").is_err());
    }
}
