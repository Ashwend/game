---
title: World generation and persistence
owns: Procedural world generation (map sizing, biome classification, the deterministic spawn pipeline) and the on-disk save format (GAMESAVE binary, the persisted struct inventory, load/seed-vs-restore).
when_to_read: Before changing world generation, biome classification, a persisted struct, or the save format.
sources:
  - src/world/mod.rs - MapType, ProceduralMapSize, WorldData, TEST_WORLD_SEED
  - src/world/chunk/classification.rs - ChunkClassification, ClassificationChannels, BIOME_BIAS, base_capacity
  - src/world/chunk/generator.rs - kind_target, chunk_kind_target, generate_world_spawns, PlayableBounds
  - src/resources.rs - spawn_resource_node, tree_is_dead (dead-snag decision)
  - src/save/format.rs - SAVE_FORMAT_VERSION, encode/decode, world_save_postcard_layout_is_stable
  - src/save/types.rs - WorldSave, WorldStateSave, PersistedPlayer
  - src/server/chunk_manager/save.rs - ChunkManagerSave
  - src/server/lifecycle.rs - GameServer::new (seed-vs-restore, sleeping bodies, next_id_floor)
related:
  - docs/chunks-and-aoi.md - runtime chunk membership, regrow scheduling, and Lightyear room-based AoI (the live half of the same chunk system)
  - docs/items-and-resources.md - the NodeKind/resource-node registry the generator places
  - docs/replication.md - how the generated nodes reach clients (per-component replication, not a snapshot)
  - docs/server-authority.md - GameServer ownership and the tick loop that drives regrow + auto-save
---

# World generation and persistence

> When to read this: before changing world generation, biome classification, a persisted struct, or the save format. Source of truth: `src/world/` (generation) and `src/save/` (persistence). Canonical invariants (replicated-state rules, singleplayer==multiplayer, balance-in-game_balance.rs) live in CLAUDE.md.

This doc owns two coupled concerns: how a `(seed, size)` pair turns into a deterministic world, and how that world is persisted. The *runtime* chunk system (live membership, node regrow, Lightyear room AoI) lives in [docs/chunks-and-aoi.md](chunks-and-aoi.md); this doc covers only the pure generation pipeline and the disk format.

## Map sizing and the bounded arena

A world is a single `MapType::Procedural { seed, size }` (`src/world/mod.rs` - `MapType`). There is currently no other map variant.

- `ProceduralMapSize` is `Small | Medium | Large`, mapping to **15 / 31 / 63** chunk cells per side (`ProceduralMapSize::dims`). Each is forced odd via `| 1` so a single center chunk sits over the origin where the player spawns. `LARGE_DIMS = 63`; medium and small derive as `LARGE_DIMS / 2` and `LARGE_DIMS / 4` then `| 1`.
- Chunk edge is `CHUNK_SIZE_M = 64.0` m, so the three sizes are **960 / 1984 / 4032** m square playable areas.
- **The default is `Medium` (31x31 = 1984 m)** (`ProceduralMapSize` `#[default] Medium`). `MapType::default()`, `WorldData::test_world()`, and a fresh `WorldSave` all produce a 31x31 world. Some module docstrings/tests still describe a "5x5 chunks span -2..=2" test world (e.g. `ChunkDims::new(5)` in generation tests); that 5x5 framing is a legacy test fixture, **not** the real default-world size.
- `TEST_WORLD_SEED = 0x7E57_5EED_5EED_5EED` (`src/world/mod.rs` - `TEST_WORLD_SEED`) is the default map seed, used by tests and the loopback menu backdrop.

`WorldData` for a grid world (`WorldData::chunk_world`) carries only the **4 perimeter stone walls** (`build_world_blocks`, four `BlockKind::Stone` blocks centred on the origin) and an **always-empty `resource_nodes`** vector. The server's `ChunkManager` owns every resource node; `WorldData.resource_nodes` is populated only for the hand-authored `WorldData::menu_backdrop_world` (the main-menu splash scene), never for grid worlds.

The grid extends past the centred perimeter walls on the positive axes, so the generator clips any spawn outside `PlayableBounds` (`src/world/chunk/generator.rs` - `PlayableBounds::from_dims`) to keep nodes on the player's side of the wall.

## The deterministic generation pipeline

Everything under `src/world/chunk/` is a pure function of `(world_seed, coord)` (and kind/counter). No I/O, no `std::rand`, no wall clock. Determinism is load-bearing and tested (`generate_world_spawns_is_deterministic`, `classification_is_deterministic_per_seed`); `ChunkRng` / `splitmix64` / `fbm` exist precisely to stay reproducible without a `rand` dependency. Keep every new generation path pure.

### Noise primitives (`src/world/chunk/noise.rs`)

`value_noise_2d`, `fbm` (octave-summed value noise), `splitmix64` (the integer mixer used for per-channel/per-node seeds), and `ChunkRng` (the per-chunk stream the placement loop draws from). All exported through `src/world/mod.rs`.

### Classification (recomputed, not persisted)

`ClassificationChannels::sample(world_seed, coord)` (`src/world/chunk/classification.rs`) reads **four `fbm` channels** (forest, stone, ore, hay) at the chunk centre, each with its own seed offset, at `CLASSIFICATION_BASE_FREQUENCY = 1/600` over `CLASSIFICATION_FBM_OCTAVES = 2`. `classify()` argmaxes the channels and labels the chunk `Forest | RockyOutcrop | OreVein | Plains | Mixed`, falling to `Mixed` if no channel clears `CLASSIFICATION_THRESHOLD = 0.42`.

Two facts that bite agents:

1. **The label uses biased channels; capacity uses raw channels.** `BIOME_BIAS = { forest: 1.19, stone: 0.92, ore: 0.89, hay: 1.08 }` is applied in `ClassificationChannels::biased()` and feeds **only** the label (which `base_capacity` row a chunk gets) and the ground-texture splat, leaning the map green. Per-kind density scaling reads the **raw, unbiased** channels via `channel_for`, so a forest chunk keeps its tuned tree count regardless of the bias.
2. **Classification is never persisted.** It is recomputed from the seed on every load. Editing `BIOME_BIAS`, `CLASSIFICATION_THRESHOLD`, the frequencies, or `base_capacity` **re-labels every existing world**. A chunk that flips away from ore/rocky loses that kind's capacity, and already-placed ore there stops respawning once mined. Start a fresh world to validate a generation change cleanly; the save only stores node ids, not labels.

`base_capacity(classification, kind)` is the per-`NodeKind` ceiling table for a "pure" example of each classification (e.g. `Forest` holds 8 medium trees and 0 ore; `RockyOutcrop` holds 14 surface stones and 4 stone veins).

### Spawn generator (`src/world/chunk/generator.rs`)

`generate_world_spawns(world_seed, dims)` walks every chunk and `generate_chunk_spawns` places nodes per kind:

- **Per-chunk target** comes from `chunk_kind_target(classification, channels, kind)`, which wraps `kind_target(base_capacity, channel)`. `kind_target = round(base * (0.55 + channel * 0.7) * DENSITY_MULTIPLIER)` with `DENSITY_MULTIPLIER = 2.0`.
- **`kind_target` / `chunk_kind_target` are the single shared capacity formula** used by both world-gen and the runtime regrow ceilings (`ChunkManager::build_empty_grids`). If you change one, change both or the world over/under-fills. This is the most important coupling in the system; the doc-comments at `kind_target` and in `chunk_manager` call it out explicitly.
- **Forest-fringe ore rule** (folded into `chunk_kind_target`): a `Forest` chunk holds no coal or sulfur (those stay in the high-risk barren biomes), gets a single lucky iron node only where the raw ore channel `>= FOREST_IRON_ORE_CHANNEL (0.64)`, and an occasional stone vein where the stone channel `>= FOREST_STONE_VEIN_CHANNEL (0.56)`. Qualifying forest chunks cluster on the edge of nearby barren biomes, so the strike reads as a lucky fringe find.
- **Placement** is Poisson-disk rejection sampling: candidates are drawn inside the chunk with `EDGE_MARGIN_M = 0.5` from the boundary, accepted against a per-kind noise mask (`KIND_MASK_FREQUENCY = 1/28`, `KIND_MASK_OCTAVES = 3`, `accept_floor = 0.25`) so nodes cluster, and rejected if they violate the kind's own `min_spacing_m()` or the global `CROSS_KIND_MIN_SPACING_M = 0.7`. Up to `MAX_CANDIDATES_PER_NODE = 18` candidates are tried per node before giving up. Candidates outside `PlayableBounds` are dropped.
- **Tree variants** alternate pine/birch deterministically via a `splitmix64` variant counter (`tree_variant_counter`), resolved through `NodeKind::variant_definition_id`. `NodeKind` collapses small/medium/large tree variants into three kinds; both pine and birch definition ids map back to the same kind.
- Node ids are dense and contiguous starting at `1`, in chunk-iteration order; the server adopts the high-water mark for its `ResourceNodeId` counter.

### Dead-tree snags (save format v15)

Whether a tree is a bare dead snag is decided **at spawn**, authoritatively, in `spawn_resource_node` (`src/resources.rs`), and frozen on `ResourceNodeState.dead` so it replicates and persists rather than being re-derived per client. `tree_is_dead(seed, id, position)` samples the forest channel at the node's exact position and runs a **smoothstep** over `DEAD_TREE_FOREST_LOW = 0.40` to `DEAD_TREE_FOREST_HIGH = 0.60`: forest interiors stay lush, the open is bare snags, the edge thins into a mix. A per-node `splitmix64(id)` hash makes a given tree stable. `world_seed = None` (the menu backdrop, which neither replicates nor saves) leaves all trees alive.

## Save model

A `WorldSave` (`src/save/types.rs`) on disk is, in order (`src/save/format.rs` - `encode_world_save`):

1. 8-byte magic `b"GAMESAVE"`.
2. `u32` little-endian format version.
3. zstd-compressed (`ZSTD_LEVEL = 5`) postcard payload of the `WorldSave` struct.

`WorldSave` itself is `{ id: Uuid, name, map: MapType, created_at_unix, admins: Vec<AccountId>, state: WorldStateSave }`.

### Format version and the no-migration contract

`SAVE_FORMAT_VERSION = 17` (`src/save/format.rs`). A version mismatch is **rejected**, never migrated (`decode_world_save` bails on `version != SAVE_FORMAT_VERSION`), and the worlds-screen surfaces it as a "couldn't load" banner. The full v1..v17 changelog is documented inline above the constant.

postcard is **positional and non-self-describing**: any field add, remove, reorder, or retype on `WorldSave` or any nested persisted struct silently changes the byte layout. Two guards exist:

- `WorldStateSave` deliberately has **no `#[serde(default)]`** on its fields, since the loader gates on an exact version match a missing field can never occur; defaults would imply a forward-compat path that does not exist.
- The `world_save_postcard_layout_is_stable` test pins the **SHA-256 of the uncompressed postcard payload** of a fixed `WorldSave`. A silent layout change fails CI instead of corrupting shipped saves.

**Procedure for any persisted-struct change:** add/reorder the field, bump `SAVE_FORMAT_VERSION` (with a changelog line), then regenerate the golden hash in `world_save_postcard_layout_is_stable`. There is no upgrade path; old saves are rejected.

Writes are **atomic** (`write_file_atomically`): temp file in the same directory, `write_all` + `sync_all`, then rename. Windows uses a backup-swap variant (move existing aside, rename in, restore on failure). Decompression is bounded to `MAX_DECOMPRESSED_SAVE_BYTES = 1 GiB` (`zstd_decompress_bounded`) as defense-in-depth against a corrupt/crafted zstd bomb; saves are local files, never attacker-delivered over the wire.

`WorldStore::platform_default()` (`src/save/store.rs`) stores saves under `<platform data dir>/worlds/` as `<uuid>.save` (`world_path`). `list_worlds` isolates per-file failures into a separate `corrupted` list and best-effort recovers name+id from a version-mismatched-but-otherwise-valid file.

### Complete `WorldStateSave` inventory

The persisted authoritative state (`src/save/types.rs` - `WorldStateSave`), in field order:

| Field | Type | Notes |
|---|---|---|
| `last_authoritative_tick` | `u64` | server tick at save time; load-tick base for chunk regrow + dropped-item timers |
| `players` | `Vec<PersistedPlayer>` | rebuilt as sleeping bodies on load (see below) |
| `dropped_items` | `Vec<DroppedWorldItem>` | re-spawned with fresh physics bodies, expiry timer reset to load tick |
| `resource_nodes` | `Option<Vec<ResourceNodeState>>` | `None` for a never-hosted world; `Some(_)` (even empty) once hosted, so harvested nodes don't respawn |
| `chunk_manager` | `Option<ChunkManagerSave>` | per-chunk identity + pending regrows; `None` for a brand-new world |
| `next_dropped_item_id` | `DroppedItemId` | floored on load |
| `next_client_id` | `ClientId` | floored on load |
| `next_resource_node_id` | `ResourceNodeId` | admin-spawn id counter; floored above the chunk high-water mark |
| `world_time_seconds_of_day` | `f32` | persisted day/night clock (v3) |
| `world_time_multiplier` | `f32` | persisted day/night speed (v3) |
| `deployed_entities` | `Vec<PersistedDeployedEntity>` | placed structures (workbenches, furnaces, doors, storage, torches, cupboards, sleeping bags) (v6+) |
| `next_deployed_entity_id` | `DeployedEntityId` | floored on load |
| `world_map_markers` | `Vec<PersistedAccountMarkers>` | per-account map pins (v14); marker-id counter re-derived on load |

`ResourceNodeState` (`src/protocol/world.rs`) is `{ id, definition_id, position, yaw, storage: Vec<ItemStack>, dead: bool }`. The old `respawn_progress` field was **removed in save format v8** and survives only as a changelog comment; depleted nodes are now removed entirely and regrow as fresh entities. `dead` was added in v15 (above).

`ChunkManagerSave` (`src/server/chunk_manager/save.rs`) persists `world_seed`, `dims`, `next_node_id`, `node_chunks` (`node_id -> (coord, kind)`, replayed so live sets rebuild **without** re-running the placement RNG), and `pending_regrows` as `ticks_from_now` (re-clamped to `>= MIN_REGROW_TICKS` on load so a long-idle save doesn't fire a respawn backlog at `t+0`).

### World time

`world_time` is persisted as two scalars and re-clamped on load via `WorldStateSave::world_time()` (`set_seconds` wraps into `[0, SECONDS_PER_DAY)` via `rem_euclid`, `set_multiplier` clamps to `[MIN_MULTIPLIER, MAX_MULTIPLIER]`). Constants in `src/world_time.rs`: a full day is `REAL_SECONDS_PER_DAY = 30 min` at `multiplier = 1.0`, `MAX_MULTIPLIER = 240`, default start `DEFAULT_START_SECONDS = 07:00`. Admins change the multiplier with the `/time-speed` command and it survives a round-trip.

## Load: seed-vs-restore

`GameServer::new` (`src/server/lifecycle.rs`) decides whether to trust the save or generate fresh by matching on **both** `state.resource_nodes` and `state.chunk_manager`:

- **Both `Some`** (world was hosted before): adopt the saved nodes and `ChunkManager::from_save(saved_chunk, last_authoritative_tick)`. Harvested-then-not-regrown nodes stay gone.
- **Otherwise** (a brand-new world has both `None`): generate fresh via `ChunkManager::new_for_world(seed, dims)`. A partial save with only one set would also land here, but version bumps prevent partial same-version saves from existing.

On load:

- **Persisted players come back as logged-out sleeping bodies**, not despawned (`sleeping_body_from_persisted`): visible, lootable, killable, at their saved pose/health/inventory. A reconnect from the same account routes through the regular wake-sleeper path because `account_to_client` is seeded here. Don't assume a logged-out account has no world entity.
- Dropped items get fresh physics bodies and are re-anchored to their chunk so a returning player sees them via AoI immediately; their expiry timer resets to the load tick.
- Deployables are restored, re-anchored to their chunks, and their solid colliders re-synced into the dropped-item physics world.
- Every id counter is floored via `next_id_floor(saved, live_ids)` (`= saved.max(highest_live + 1).max(1)`) so a freshly issued id can never collide with a live entity. The resource-node counter additionally floors above the chunk generator's high-water mark so admin-spawned ids never collide with chunk-issued ones.

## Auto-save and save-on-quit

Both host kinds snapshot via `GameServer::world_save()` and write atomically; the difference is only whether the routine save is announced:

- **Dedicated** hosts use `GameServer::with_auto_save(interval_ticks)` (announced).
- **Singleplayer loopback** uses `with_auto_save_silent(interval_ticks)` (no chat announcement), per the singleplayer==multiplayer invariant: same persistence path, only the operator-facing notice differs.

On quit, singleplayer's pause menu drives `ClientRuntime::shutdown_in_background`, which pulls the final `WorldSave` from the host and writes it; disconnect also flushes each client's live state so the final save sees the latest pose/inventory/health. A dedicated server persists the final world on graceful shutdown.

## AoI is room-based, not a snapshot

Generation places nodes; the runtime ships them to clients through **Lightyear per-component replication, room-gated to the AoI chunk ring** ([docs/chunks-and-aoi.md](chunks-and-aoi.md), [docs/replication.md](replication.md)). The old per-player `WorldSnapshot` wire was deleted during the Lightyear migration. The word "snapshot" still appears in some `chunk_manager` docstrings, but it refers to **save-state summaries**, not a networking path. Do not reintroduce a periodic full-world broadcast; if generated state isn't reaching clients, fix the replication path (likely a missing `ReplicationGroup` at the spawn site), per the replicated-state rules in CLAUDE.md.

The runtime view tiers, for reference: `ViewRadiusTier` is `Low | Medium | High` (`#[default] Medium`, `src/protocol/messages.rs`), mapping to Chebyshev radius `1 / 2 / 3` chunks (`view_tier_radius`); regrow timing derives from `SERVER_TICK_RATE_HZ = 20`. The ring math, hysteresis, and room subscription live in [docs/chunks-and-aoi.md](chunks-and-aoi.md).

## Related docs

- [docs/chunks-and-aoi.md](chunks-and-aoi.md) - runtime chunk membership, 5-15 min node regrow, ring-budget density falloff, and the room-based AoI hysteresis (the live counterpart to this doc's generation pipeline).
- [docs/items-and-resources.md](items-and-resources.md) - the `NodeKind` and resource-node registry the generator places, plus tool/gather rules.
- [docs/replication.md](replication.md) - how generated nodes reach clients via per-component replication, and the `ReplicationGroup` per-entity-group requirement.
- [docs/server-authority.md](server-authority.md) - `GameServer` ownership and the tick loop that drives regrow and auto-save.
- [docs/playbooks/add-content.md](playbooks/add-content.md) - step-by-step for adding a new ore/tree/resource node the generator will place.
