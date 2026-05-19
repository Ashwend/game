# Worlds And Saves

`WorldSave` is a binary `.save` file: a `GAMESAVE` magic header, a `u32` format version, then a zstd-compressed [postcard](https://docs.rs/postcard) payload. The deserialized struct holds id, name, map, created time, admins, and the full `WorldStateSave`.

`WorldStateSave` captures everything the authoritative server owns:
- `last_authoritative_tick`
- `players: Vec<PersistedPlayer>` keyed in-memory by Steam ID; on reconnect a returning player keeps their position, velocity, look, health, admin flag, inventory, and active actionbar slot
- `dropped_items: Vec<DroppedWorldItem>` — re-spawned with fresh physics bodies at load time
- `resource_nodes: Option<Vec<ResourceNodeState>>` — `None` for a freshly created world (initial nodes are seeded from the world definition); `Some(_)` once the world has been hosted, so harvested nodes don't respawn
- `next_dropped_item_id`, `next_client_id`

Bump `SAVE_FORMAT_VERSION` in `src/save.rs` on any breaking schema change. There is no migration — older saves are rejected.

`MapType::Test` builds `WorldData::test_world()`: an 80m floor with perimeter stone walls, a movement test obstacle course, ore clusters, and tree groves. `MapType::Procedural { seed, size }` currently builds a flat floor sized by `ProceduralMapSize::{Small, Medium, Large}` (64m / 128m / 256m); the seed is reserved for future generation work.

`WorldStore::platform_default()` stores saves under the platform app-data directory in `worlds/`.

Singleplayer loads a selected save, runs the loopback host, and on quit the pause menu calls `ClientRuntime::shutdown_in_background` which retrieves the final `WorldSave` from `GameServerHandle::world_save()` and writes it. Disconnect also writes each client's live state into `GameServer::persisted_players` so the next save (or reconnect) sees the latest snapshot. Dedicated server loads `--world` or creates/reuses `Dedicated`; on graceful terminal shutdown it persists the final `WorldSave` back to the source file or store.
