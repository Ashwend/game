# Worlds And Saves

`WorldSave` is a binary `.save` file: a `GAMESAVE` magic header, a `u32` format version, then a zstd-compressed [postcard](https://docs.rs/postcard) payload. The deserialized struct holds id, name, map, created time, admins, and the full `WorldStateSave`.

`WorldStateSave` captures everything the authoritative server owns:
- `last_authoritative_tick`
- `players: Vec<PersistedPlayer>` keyed in-memory by Steam ID; on reconnect a returning player keeps their position, velocity, look, health, admin flag, inventory, and active actionbar slot
- `dropped_items: Vec<DroppedWorldItem>`, re-spawned with fresh physics bodies at load time
- `resource_nodes: Option<Vec<ResourceNodeState>>`, `None` for a freshly created world (initial nodes are seeded from the world definition); `Some(_)` once the world has been hosted, so harvested nodes don't respawn
- `next_dropped_item_id`, `next_client_id`

Bump `SAVE_FORMAT_VERSION` in `src/save.rs` on any breaking schema change. There is no migration, older saves are rejected.

`MapType::Procedural { seed, size }` builds a chunk-generated world sized by `ProceduralMapSize::{Small, Medium, Large}` (3×3 / 5×5 / 9×9 chunks at `CHUNK_SIZE_M = 64` m per side, so 192 / 320 / 576 m playable squares). `WorldData::test_world()` is a convenience helper used by tests and the menu backdrop fallback, it returns the default procedural world.

## Chunk pipeline

Pure generation (no I/O, deterministic from the seed) lives under `src/world/chunk/`:

- `classification.rs` samples four seeded noise channels at each chunk's centre and labels the chunk as `Forest`, `RockyOutcrop`, `OreVein`, `Plains`, or `Mixed`. Each classification carries a `base_capacity` table describing the per-`NodeKind` ceiling (trees, surface stones, branch piles, hay grass, coal/iron/sulfur ore, stone veins).
- `generator.rs` Poisson-disk-samples node spawn positions inside each chunk, scaled by the local channel intensity and the classification's base capacity. Edge margin keeps spawns at least 0.5 m away from the chunk boundary so the client's collider grid doesn't pop when crossing.
- `noise.rs` owns `value_noise_2d`, `fbm`, `splitmix64`, and the per-chunk `ChunkRng` both passes share.

The server side is `src/server/chunk_manager.rs`:

- **Membership**, every networked entity (resource node ids, dropped item ids, eventually building ids) is registered against the chunk that contains its position; the entity itself stays in its owning collection (`GameServer::resource_nodes`, `dropped_items`, `clients`).
- **AoI streaming**, given a player position and `ViewRadiusTier`, the manager returns the Chebyshev ring of chunk coords the player should see (rings of 1 / 2 / 3 cells for Low / Medium / High, plus a `LOAD_BUFFER_RINGS = 1` outer ring used purely as a collider-stability buffer to avoid jitter when crossing boundaries). The room-subscription system in `src/net/host.rs` diffs that set against the client's last-known subscriptions and emits `AddSender`/`RemoveSender` events on the per-chunk Lightyear `Room` entities; Lightyear then auto-ships entity spawns/despawns for everything anchored to those chunks. There is no parallel per-entity AoI path.
- **Regrow scheduling**, depleted nodes are queued to respawn 5–15 minutes later at a noise-valid position in the same chunk, up to the chunk's capacity ceiling.
- **Density falloff**, during the initial world population, outer-ring chunks keep only a fraction of their generated capacity so distant areas read as populated without paying the full per-node cost. This is a fixed spawn budget, not a sliding cull, so players don't see chunks fade in/out.
- **Persistence**, the manager serializes per-chunk live counts and pending regrow events into `ChunkManagerSave`, which `WorldStateSave` embeds. Reload reconstitutes the manager from `(world_seed, dims, saved_state)`.

`WorldStore::platform_default()` stores saves under the platform app-data directory in `worlds/`.

Singleplayer loads a selected save, runs the loopback host, and on quit the pause menu calls `ClientRuntime::shutdown_in_background` which retrieves the final `WorldSave` from `GameServerHandle::world_save()` and writes it. Disconnect also writes each client's live state into `GameServer::persisted_players` so the next save (or reconnect) sees the latest pose, inventory, and health. Dedicated server loads `--world` or creates/reuses `Dedicated`; on graceful terminal shutdown it persists the final `WorldSave` back to the source file or store.
