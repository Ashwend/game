use std::{
    ffi::OsString,
    fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};

use super::types::WorldSave;

pub(super) const SAVE_EXTENSION: &str = "save";
pub(super) const SAVE_MAGIC: &[u8; 8] = b"GAMESAVE";
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
///
/// `4` added `next_resource_node_id` on `WorldStateSave` so the server can
/// hand out IDs in O(1) instead of scanning the live node map for the max.
///
/// `5` switched the test/procedural worlds to a chunk-based generator and
/// embeds `ChunkManagerSave` (per-chunk capacities + pending fresh-position
/// regrows) on `WorldStateSave`. Old saves don't carry the chunk state,
/// and the test-world layout changed, so older saves wouldn't map onto
/// the new world geometry, they're rejected at load.
///
/// `6` added persisted deployable entities (workbenches, furnaces) on
/// `WorldStateSave::deployed_entities` plus the `next_deployed_entity_id`
/// counter. Postcard is positional so old saves wouldn't line up.
///
/// `7` added per-deployable furnace state (fuel slot + smelt slots +
/// active flag + burn/smelt timers). Old v6 saves don't carry this
/// field, rejected and surfaced in the worlds-screen "couldn't load"
/// banner.
///
/// `8` (Phase 7 of the Lightyear migration) dropped the vestigial
/// `ResourceNodeState::respawn_progress` field. The server never wrote
/// `Some(_)` to it post-Phase-1, depleted nodes are removed entirely
/// and regrow as fresh entities, so the field was always `None`. Old
/// v7 saves carry the trailing `Option<f32>` and would deserialise
/// wrong; rejected at load.
///
/// `9` added `PersistedDeployedEntity::owner: Option<AccountId>` so
/// damage gating can survive reloads. Old v8 saves are rejected.
///
/// `10` added `ItemStack::durability: Option<u32>` (tool wear). Every
/// persisted inventory, furnace slot, dropped item, and loot bag embeds
/// `ItemStack`, and postcard is positional, so old v9 saves would
/// deserialise wrong; rejected at load.
///
/// `11` added the base-building fields on `PersistedDeployedEntity`:
/// `placed_at_tick` (hammer demolish window), `door`
/// (`PersistedDoorState`: lock code, authorized accounts, open flag,
/// parent doorway), and `label` (sleeping-bag names). `DeployableKind`
/// also grew the `Building`/`Door`/`SleepingBag` variants. Positional
/// postcard layout drift on both, so old v10 saves are rejected.
///
/// `12` added `PersistedDeployedEntity::storage`
/// (`PersistedStorageBoxState`: the slot grid of a placed storage box)
/// and the `DeployableKind::StorageBox` variant. Positional postcard
/// layout drift again, so old v11 saves are rejected.
///
/// `13` added `PersistedDeployedEntity::torch` (`PersistedTorchState`: the
/// lit flag + burn countdown of a placed torch) and the
/// `DeployableKind::Torch { wall }` variant. The new enum variant shifts
/// the positional postcard layout of every `DeployableKind`, so old v12
/// saves are rejected at load.
///
/// `14` added `WorldStateSave::world_map_markers` (per-account
/// `PersistedAccountMarkers`: the player-placed map pins). Appending a field
/// shifts the positional postcard layout, so old v13 saves are rejected.
///
/// `15` added `ResourceNodeState::dead`, the authoritative bare-dead-tree flag
/// (decided at generation from the seed + position and frozen on the node so it
/// replicates + persists rather than being re-derived per client). Every
/// persisted resource node embeds `ResourceNodeState`, and postcard is positional,
/// so old v14 saves would deserialise wrong; rejected at load.
///
/// `16` added `PersistedDeployedEntity::cupboard` (the Tool Cupboard
/// authorized-account list). Postcard is positional, so the new trailing
/// field shifts every later byte; old v15 saves are rejected at load.
pub(super) const SAVE_FORMAT_VERSION: u32 = 16;
/// zstd level 5 sits in the sweet spot for save files: ~70-75% size reduction
/// at >100MB/s compression and ~1GB/s decompression.
const ZSTD_LEVEL: i32 = 5;
/// Hard ceiling on the decompressed payload size. Save files are local (a
/// singleplayer file the player owns, or an operator-controlled dedicated
/// world), never attacker-delivered over the wire, so this is defense in depth
/// rather than a live threat: it stops a hand-crafted or corrupted blob from
/// driving an unbounded allocation (a zstd decompression bomb). Sized far above
/// any real world (1 GiB); raise it if a legitimate save ever approaches it.
pub(super) const MAX_DECOMPRESSED_SAVE_BYTES: u64 = 1 << 30;

/// zstd-decompress `compressed`, refusing to allocate past
/// [`MAX_DECOMPRESSED_SAVE_BYTES`]. Shared by the full loader and the
/// best-effort name recovery so both call sites are bounded.
pub(super) fn zstd_decompress_bounded(compressed: &[u8]) -> Result<Vec<u8>> {
    zstd_decompress_capped(compressed, MAX_DECOMPRESSED_SAVE_BYTES)
}

fn zstd_decompress_capped(compressed: &[u8], cap: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut decoder = zstd::stream::read::Decoder::new(compressed)
        .context("could not start zstd decode of world save")?;
    let mut out = Vec::new();
    // Read one byte past the cap so an exactly-at-cap payload still succeeds
    // while anything larger is detectable and rejected.
    decoder
        .by_ref()
        .take(cap + 1)
        .read_to_end(&mut out)
        .context("could not zstd-decompress world save")?;
    if out.len() as u64 > cap {
        bail!("world save decompresses beyond the {cap}-byte cap");
    }
    Ok(out)
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

pub(super) fn encode_world_save(save: &WorldSave) -> Result<Vec<u8>> {
    let payload = postcard::to_allocvec(save).context("could not postcard-encode world save")?;
    let compressed = zstd::stream::encode_all(payload.as_slice(), ZSTD_LEVEL)
        .context("could not zstd-compress world save")?;

    let mut out = Vec::with_capacity(SAVE_MAGIC.len() + 4 + compressed.len());
    out.extend_from_slice(SAVE_MAGIC);
    out.extend_from_slice(&SAVE_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

pub(super) fn decode_world_save(bytes: &[u8]) -> Result<WorldSave> {
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
    let payload = zstd_decompress_bounded(compressed)?;
    postcard::from_bytes(&payload).context("could not postcard-decode world save")
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

pub(super) fn atomic_temp_path(path: &Path) -> Result<PathBuf> {
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
    use super::super::types::WorldSave;
    use super::*;

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

    /// Golden-layout guard. postcard is a non-self-describing positional
    /// format, so reordering or retyping a field in `WorldSave` (or any nested
    /// save struct) silently changes the on-disk byte layout WITHOUT changing
    /// `SAVE_FORMAT_VERSION`, which would make every shipped `.save` fail to
    /// load with no test going red. This pins the SHA-256 of the uncompressed
    /// postcard payload of a fixed save. When it trips, the author must either
    /// revert the layout change or deliberately bump `SAVE_FORMAT_VERSION` and
    /// regenerate the hash below, turning silent corruption into an explicit
    /// decision. Hashing the postcard payload (not the zstd output) keeps a
    /// zstd version/level change from false-failing.
    #[test]
    fn world_save_postcard_layout_is_stable() {
        use super::super::types::WorldStateSave;
        use crate::protocol::{DroppedWorldItem, ItemStack, QuatNet, Vec3Net};
        use crate::world::{MapType, ProceduralMapSize};
        use sha2::{Digest, Sha256};

        let state = WorldStateSave {
            last_authoritative_tick: 123,
            dropped_items: vec![DroppedWorldItem {
                id: 5,
                stack: ItemStack::new("wood", 9),
                position: Vec3Net::new(1.0, 2.0, 3.0),
                yaw: 0.5,
                rotation: QuatNet::IDENTITY,
            }],
            resource_nodes: Some(Vec::new()),
            next_dropped_item_id: 6,
            next_client_id: 2,
            next_resource_node_id: 1000,
            world_time_seconds_of_day: 42.0,
            world_time_multiplier: 1.0,
            next_deployed_entity_id: 1,
            ..Default::default()
        };
        let save = WorldSave {
            id: uuid::Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            name: "Golden World".to_owned(),
            map: MapType::Procedural {
                seed: 42,
                size: ProceduralMapSize::Small,
            },
            created_at_unix: 1_700_000_000,
            admins: vec![7],
            state,
        };

        let payload = postcard::to_allocvec(&save).expect("postcard encode");
        let digest = Sha256::digest(&payload);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex, "8ae0b93968b625f9ada046d06399c0439aa53369ce28134ed3002ef88c1f5795",
            "WorldSave postcard layout changed. If intentional, bump \
             SAVE_FORMAT_VERSION and update this golden hash to {hex}."
        );
    }

    #[test]
    fn bounded_decompress_rejects_over_cap_payloads() {
        // A payload that decompresses past the cap must error out instead of
        // allocating the whole thing. Use a tiny cap so the test stays cheap.
        let payload = vec![0u8; 4096];
        let compressed = zstd::stream::encode_all(payload.as_slice(), ZSTD_LEVEL).expect("encode");
        let err = zstd_decompress_capped(&compressed, 64).unwrap_err();
        assert!(err.to_string().contains("cap"), "got: {err}");
        // The same blob is fine under a generous cap.
        let ok = zstd_decompress_capped(&compressed, 1 << 20).expect("under cap");
        assert_eq!(ok.len(), payload.len());
    }

    #[test]
    fn save_world_file_writes_custom_paths() {
        let root =
            std::env::temp_dir().join(format!("game-save-file-test-{}", uuid::Uuid::new_v4()));
        let path = root.join("nested").join("world.save");
        let save = WorldSave::new("Dedicated File", Some(123));

        save_world_file(&path, &save).expect("world file should save");

        let bytes = std::fs::read(&path).expect("world file should exist");
        let loaded = decode_world_save(&bytes).expect("world file should parse");
        assert_eq!(loaded.id, save.id);
        assert_eq!(loaded.name, "Dedicated File");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn round_trip_preserves_empty_world() {
        let save = WorldSave::new("Round Trip Empty", Some(42));
        let bytes = encode_world_save(&save).expect("encode");
        let decoded = decode_world_save(&bytes).expect("decode");
        assert_eq!(save, decoded, "empty round trip should be byte-identical");
    }

    #[test]
    fn round_trip_preserves_populated_world() {
        use crate::{
            items::{CRUDE_FURNACE_ID, DeployableKind, IRON_ORE_ID, WOOD_ID},
            protocol::{DroppedWorldItem, ItemStack, PlayerInventoryState, QuatNet, Vec3Net},
            save::{PersistedDeployedEntity, PersistedFurnaceState, PersistedPlayer},
        };

        let mut save = WorldSave::new("Round Trip Populated", Some(1));
        save.state.last_authoritative_tick = 1234;
        save.state.next_dropped_item_id = 17;
        save.state.next_client_id = 9;
        save.state.next_resource_node_id = 99;
        save.state.next_deployed_entity_id = 42;
        save.state.world_time_seconds_of_day = 4321.5;
        save.state.world_time_multiplier = 2.0;

        save.state.players.push(PersistedPlayer {
            account_id: 1,
            name: "Alice".to_owned(),
            position: Vec3Net::new(1.0, 0.0, 2.0),
            velocity: Vec3Net::ZERO,
            yaw: 0.5,
            pitch: 0.1,
            health: 80.0,
            grounded: true,
            last_processed_input: 7,
            is_admin: false,
            inventory: PlayerInventoryState::empty(),
        });

        save.state.dropped_items.push(DroppedWorldItem {
            id: 7,
            stack: ItemStack::new(IRON_ORE_ID, 4),
            position: Vec3Net::new(3.0, 0.0, 5.0),
            yaw: 0.0,
            rotation: QuatNet::IDENTITY,
        });

        save.state.resource_nodes = Some(Vec::new());

        save.state.deployed_entities.push(PersistedDeployedEntity {
            id: 11,
            item_id: CRUDE_FURNACE_ID.to_owned(),
            kind: DeployableKind::Furnace { tier: 1 },
            position: Vec3Net::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            health: 800,
            max_health: 800,
            owner: Some(1),
            placed_at_tick: 4_200,
            door: None,
            label: None,
            storage: None,
            cupboard: None,
            furnace: Some(PersistedFurnaceState {
                fuel: Some(ItemStack::new(WOOD_ID, 3)),
                items: vec![Some(ItemStack::new(IRON_ORE_ID, 2)), None, None],
                active: true,
                fuel_burn_ticks_left: 50,
                smelt_progress_ticks: 25,
            }),
            torch: None,
        });

        let bytes = encode_world_save(&save).expect("encode");
        let decoded = decode_world_save(&bytes).expect("decode");
        assert_eq!(
            save, decoded,
            "populated round trip should be byte-identical"
        );
    }

    #[test]
    fn rejects_truncated_compressed_payload() {
        let save = WorldSave::new("Truncate", Some(1));
        let bytes = encode_world_save(&save).expect("encode");
        // Snip 8 bytes off the end of the compressed payload, zstd
        // should refuse it on decode.
        let truncated = &bytes[..bytes.len() - 8];
        assert!(
            decode_world_save(truncated).is_err(),
            "truncated payload must not decode silently"
        );
    }

    #[test]
    fn rejects_corrupted_compressed_payload() {
        let save = WorldSave::new("Corrupt", Some(1));
        let mut bytes = encode_world_save(&save).expect("encode");
        // Flip a byte in the middle of the compressed payload.
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF;
        assert!(
            decode_world_save(&bytes).is_err(),
            "corrupt payload must not decode silently"
        );
    }
}
