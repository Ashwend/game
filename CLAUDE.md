# CLAUDE.md

AI context for this repo.

Game is a Rust/Bevy first-person prototype. Singleplayer and multiplayer both use the Lightyear-backed `ClientSession::Network` path; singleplayer only adds loopback host startup, host admin assignment, and local save persistence. Worlds are compressed binary `.save` files (postcard + zstd, versioned `GAMESAVE` header).

Start here:
- `src/cli.rs`: `client`, `server`, and `admin` subcommands.
- `src/app.rs`: Bevy app wiring and named `ClientSystemSet` schedule ordering.
- `src/app/state/`: client resources and UI/runtime state.
- `src/app/ui/modal.rs`: reusable animated modal shell plus confirmation modal.
- `src/app/ui/worlds/`: singleplayer worlds screen, dialogs, table, and session actions.
- `src/app/ui/inventory/`: inventory slot rendering, drag handling, and pickup tooltip helpers.
- `src/app/ui/multiplayer/`: direct-connect dialog, address parsing, and connection attempt helpers.
- `src/server.rs` and `src/server/`: shared authoritative game state for both singleplayer loopback and dedicated multiplayer; keep connection/auth, inventory, movement, dropped-item, resource-node, and deployable concerns split. The `HashMap`s on `GameServer` are authoritative; the `*_ecs.rs` modules define the matching ECS mirror components that Lightyear replicates to clients (see [Networking § Replication](docs/networking.md#replication)).
- `src/protocol.rs`: `ClientMessage` / `ServerMessage` wire variants, channel delivery preferences, and a handful of server-internal shape types (`ResourceNodeState`, `DroppedWorldItem`, `DeployedEntityState`, `OpenFurnaceView`) kept here because the save layer serialises them.
- `src/controller/`: movement simulation, movement tuning/math, collision, and the server-side block spatial grid.
- `src/items.rs` and `src/resources.rs`: item registry, tool profiles, and resource-node definitions/gather rules. See [Items and Resources](docs/items-and-resources.md) for the walkthrough.
- `src/game_balance.rs`: every gameplay tuning constant (combat ranges, deployable damage/placement, furnace timings, loot-bag interact range, respawn radius). New balance values live here, not inline in subsystem modules.
- `src/server/furnace/` (split): `state.rs` (types + pure helpers), `tick.rs` (smelt loop), `commands.rs` (open/move/quick-transfer). Tests in `src/server/tests/furnace.rs`. See [Crafting and Deployables](docs/crafting.md).
- `src/net/client.rs`: Lightyear client session wrapper used by singleplayer and direct multiplayer.
- `src/net/host.rs` and `src/net/host/`: Lightyear host wrapper, handle/shutdown helpers, routing around `GameServer`, the room/AoI subscription system that drives per-component replication, and the optional Unix admin socket used by `./cli admin`. See [Networking § Replication](docs/networking.md#replication).
- `src/net/channels.rs`: Lightyear channel registration plus the `app.register_component::<T>()` calls for every replicated component. Both server and client install the same `LightyearProtocolPlugin` so registries match.
- `src/net/dedicated/`: CLI-facing dedicated server entry point and admin request types.
- `src/save/`: world persistence (`WorldStore`, `WorldSave`, atomic writes, format version).
- `src/world/`: `MapType`, world block geometry, perimeter walls, and the chunk-based generation pipeline under `src/world/chunk/` (classification, value noise, Poisson-disk spawn generator).
- `src/server/chunk_manager.rs`: server-side owner of the chunk grid — every networked entity (resource nodes, drops, eventually buildings) is anchored to a chunk; the room-subscription system in `src/net/host.rs` adds the client's sender to the matching chunk's Lightyear `Room` so replication is AoI-filtered. Also schedules 5–15 min node respawns and persists per-chunk live counts.
- `src/app/scene/assets.rs` and `src/app/scene/world.rs`: shared `StandardMaterial` setup for players, items, resource nodes, ground, and stone walls. See [Materials](docs/materials.md) before adding or tuning a material.

Use `./cli check`, `./cli test`, and `./cli lint`.

Singleplayer/multiplayer invariant:
- Keep gameplay behavior in shared modules: `server`, `protocol`, `controller`, `items`, `world`, and shared app systems.
- Do not add a separate singleplayer gameplay implementation, direct in-process transport bypass, or duplicate movement/inventory/chat rules for local play.
- Singleplayer-specific code should stay limited to selecting/loading a save, starting a loopback host, marking the local host as admin, and saving the host world state on shutdown.
- Multiplayer-specific code should stay limited to remote address/server discovery, auth mode, transport setup, and dedicated-host lifecycle.
- When adding a feature, make it work through `ClientMessage`/`ServerMessage` and `GameServer` first, then let both loopback singleplayer and direct multiplayer consume that same path. If the feature introduces new per-entity authoritative state (something the client renders or queries), ship it through Lightyear's per-component replication — not a new `ServerMessage` snapshot variant. Read **⚠️ Replicated state** below before designing the wire shape.
- Movement is intentionally client-authoritative for responsiveness. Clients send `PlayerMovement` state produced by local prediction; the server validates sequence/finite values and writes them onto the player's mirror entity so Lightyear replicates the result to peers. Do not convert to server-authoritative input simulation unless explicitly asked.
- **Gameplay never pauses.** The game runs an authoritative server (loopback or dedicated) at all times, so simulation, local prediction, and network ticks keep ticking *as long as the local screen is in-game* — regardless of which UI overlay is up (pause menu, pause-options, inventory, chat, crafting, furnace, death splash, anything). Overlays only gate local **controls** (movement, look, swing). Knockback, replication diffs, death/respawn, world-time advancement — all of it must keep happening while a menu is open, otherwise effects pile up and fire en masse when the menu closes. If you add a new overlay, gate it through `gameplay_accepts_controls` in `src/app/systems/input/gating.rs`, never through `gameplay_simulation_allowed`.

Clean-code rules:
- No monolithic files. If a file mixes transport, domain rules, UI layout, persistence, and tests, split by concern before extending it.
- Prefer small modules with clear ownership over broad helper files. Good splits already exist in `src/server/`, `src/controller/`, `src/app/systems/`, `src/app/state/`, and `src/app/ui/worlds/`.
- Keep UI rendering, UI state, session actions, and authoritative game rules separate.
- Put reusable modal/backdrop animation behavior in `src/app/ui/modal.rs`; individual screens should only provide form contents and choice mapping.
- Keep networking transport adapters thin; they should translate to shared protocol messages and delegate gameplay to `GameServer`.
- Add tests near the module that owns the behavior, especially for protocol changes, server authority, persistence, and layout/state helpers.
- Update the relevant existing doc when changing architecture. Do not create markdown summary files unless explicitly asked.

Open docs only when the task touches that area:
- [Architecture](docs/architecture.md)
- [Movement](docs/movement.md) — includes the client-authoritative trust boundary.
- [Networking](docs/networking.md) — includes the **Replication** section that documents per-component replication, chunk-room AoI, the Lightyear 0.26.4 known-issue pattern, and the procedure for adding new replicated state.
- [Items and resources](docs/items-and-resources.md) — registries, tool profiles, resource-node spawn rules, and "how to add a new tool / ore / recipe".
- [Crafting and deployables](docs/crafting.md) — recipe queue, furnace state machine, loot bags, deployable damage + ownership.
- [Voice](docs/voice.md)
- [Worlds and saves](docs/worlds-and-saves.md)
- [UI and client flow](docs/ui-and-client.md)
- [Multiplayer testing](docs/multiplayer-testing.md)
- [Materials](docs/materials.md) — PBR conventions for the scene (reflectance, roughness, metallic). Consult before adding a new `StandardMaterial` or tweaking an existing one.
- [Profiling](docs/profiling.md) — Chrome-trace capture, Perfetto `trace_processor` SQL queries, and the diagnostic patterns that surfaced during the frame-pacing investigation. Consult before reaching for "rewrite to make it faster" — the canonical bugs (per-frame iteration over N replicated entities, spurious change-detection, `Ref::is_changed()` lying for Lightyear-touched components) all have cheap fixes documented here.

Keep changes small and preserve module boundaries.

## Replicated state — rules

Every networked entity ships through Lightyear's per-component replication, room-gated to the AoI chunk ring around each player. Full architecture in [Networking § Replication](docs/networking.md#replication). The constraints below are the ones it's easy to get wrong:

1. **Per-entity authoritative state goes through Lightyear replication, not `ServerMessage`.** Resource nodes, dropped items, deployables, players — that's the established pattern. New networked entities follow the same shape: `HashMap` on `GameServer` for authoritative state + ECS mirror entity carrying the replicated components, kept in sync by an exclusive system in `src/net/host.rs`.
2. **Every spawn must attach `ReplicationGroup::new_from_entity()`.** Both spawn helpers in `src/net/host.rs` (`attach_room_gated_replication` and `attach_player_replication`) already do this. **Don't bypass them with a bare `Replicate::to_clients(...)`** — without an explicit group, Lightyear puts the entity in `ReplicationGroupId(0)` along with every other group-less entity and the per-group ack tick can advance past a slowly-changing entity's local `Changed` mark, silently dropping the diff. This is upstream bug [#740](https://github.com/cBournhonesque/lightyear/issues/740); per-entity groups sidestep it.
3. **Split each entity into one identity component (immutable post-spawn) and one component per mutable field that changes at a different cadence.** Lightyear ships per-component diffs, so this keeps wire traffic minimal.
4. **Add `replication-trace` coverage for new replicated components** that mutate post-spawn. `server: <Component> MUTATE` log in the mirror sync + `client: <Component> RECV` log on the client. Run with `--features replication-trace` and `RUST_LOG=replication_trace=info` to confirm post-spawn diffs ship before merging.
5. **Never re-introduce a periodic full-state broadcast.** The original `WorldSnapshot` wire was deleted during the migration; replacing it with a similar message because "replication is hard" is the wrong move — fix the replication path instead.
6. **Client reconciliation systems are event-driven, not polling.** When writing a `apply_*` system that mirrors replicated entities into local-only visuals, react to `Added<T>` and `RemovedComponents<T>` — do not iterate the full replicated query every frame. Iteration at AoI scale (~1800 nodes) costs 1-4 ms per frame for the noop case alone. See [src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs) for the canonical pattern: pending-spawn `VecDeque` (preserves the per-frame spawn budget across frames), reverse `Entity → Id` map (lets `RemovedComponents` find the local mirror), and a one-time catch-up scan on the first run after connect (the `Added` filter's `last_run` tick misses entities that arrived during early-returning `client_id == None` frames). `Ref::is_changed()` will fire on every replication tick for Lightyear-touched components even when the value is identical (Lightyear's receive path uses `insert_by_ids` which always bumps the change tick) — don't gate work behind it.

If you see stale replicated state in-game: build with `--features replication-trace`, reproduce, and check the log. `MUTATE` without a matching `RECV` means a replication failure (likely a missing `ReplicationGroup` at the spawn site, or a new Lightyear bug). `MUTATE` and `RECV` both firing but UI still stale means a consumer bug — look at the `Query<&Component>` reader.
