---
title: Server authoritative game state (GameServer)
owns: src/server/, the shared authoritative game-state crate (GameServer struct, receive/tick contracts, ServerEnvelope/DeliveryTarget output, the per-subsystem file map, and the ClientMessage dispatch table)
when_to_read: Before adding or changing a ClientMessage handler, a tick subsystem, or any server-side authoritative state; or to find which file owns a server concern.
sources:
  - src/server.rs - GameServer struct, ServerEnvelope, DeliveryTarget, ServerClient
  - src/server/dispatch.rs - GameServer::receive, the ClientMessage match
  - src/server/tick.rs - GameServer::tick, surviving periodic broadcasts
  - src/server/connection.rs - connect, wake_sleeper, hard_disconnect
  - src/server/voice.rs - apply_voice_frame (range constant: src/game_balance.rs - VOICE_AUDIBLE_RANGE_M)
  - src/server/commands.rs - apply_command, slash-command table
  - src/server/queries.rs - mandatory-entry helpers for DirtyTrackedMap fields
  - src/net/host.rs - run_host, tick_authoritative_server, mirror_systems wiring
  - src/net/host/routing.rs - receive_client_messages, route_envelopes
related:
  - docs/replication.md - the ECS-mirror bridge that turns GameServer HashMaps into replicated entities
  - docs/networking.md - the ClientMessage/ServerMessage wire inventory and channels
  - docs/chunks-and-aoi.md - chunk_manager, the AoI gate GameServer anchors entities to
  - docs/pvp-combat.md - combat/swing/death subsystem detail
  - docs/base-building-and-claims.md - building/door/claim/stability detail
  - docs/crafting-and-deployables.md - crafting/furnace/workbench/loot-bag/deployable/explosive detail
  - docs/meteor-shower.md - the meteor shower event that tick_world_events drives
  - docs/worlds-and-saves.md - WorldSave and the persistence boundary
---

# Server authoritative game state (GameServer)

> When to read this: before adding or changing a `ClientMessage` handler, a tick subsystem, or any server-side authoritative state, or to find which file owns a server concern. Source of truth: `src/server.rs`, `src/server/dispatch.rs`, `src/server/tick.rs`. Canonical invariants (singleplayer == multiplayer, gameplay-never-pauses, the replicated-state rules) live in CLAUDE.md.

`src/server/` is the shared authoritative game-state crate. Loopback singleplayer and dedicated multiplayer consume it through the exact same code path; see [the singleplayer/multiplayer invariant in CLAUDE.md](../CLAUDE.md). Never fork behavior between the two modes inside `src/server/`.

## GameServer is pure logic, zero I/O

`GameServer` (`src/server.rs` - `struct GameServer`) is a plain struct. It holds authoritative state in maps and exposes pure-ish methods that mutate those maps and return `Vec<ServerEnvelope>`. It performs **no networking and no disk I/O itself**. The thin host layer in `src/net/host/` drives it: drains inbound messages, calls `tick`, routes the returned envelopes onto the wire, and reconciles the maps into Lightyear-replicated ECS entities.

Hard rule for `src/server/`: never add a `MessageSender`, a socket, or a `commands.spawn` here. Returning an envelope is how this crate "sends." Persistence is the same shape: `tick` only sets `auto_save_pending`; the host drains it and does the disk write (`tick_authoritative_server` in `src/net/host.rs`). The HashMap-authority + ECS-mirror split is the **intended steady-state architecture**, not a migration scaffold. (Some code comments in `src/server.rs` still call `resource_nodes` a transitional "Phase 4" scaffold awaiting folding into entities. That framing is stale: the `WorldSnapshot` wire is deleted and Lightyear replication is the proven, sole path. The map+mirror split is deliberate.)

### Which fields replicate vs persist vs are runtime-only

Authoritative state on `GameServer` (`src/server.rs` - struct fields):

| Field | Type | Replicated? | Persisted? | Notes |
|---|---|---|---|---|
| `clients` | `HashMap<ClientId, ServerClient>` | yes (player mirror) | yes (into `persisted_players` on disconnect/shutdown) | Includes sleeping bodies (`online == false`). |
| `dropped_items` | `DirtyTrackedMap<DroppedItemId, DroppedItemBody>` | yes | yes | Physics step marks only moved bodies dirty. |
| `resource_nodes` | `DirtyTrackedMap<ResourceNodeId, ResourceNodeState>` | yes | yes (live counts via `chunk_manager`) | |
| `deployed_entities` | `DirtyTrackedMap<DeployedEntityId, DeployedEntity>` | yes | yes | Buildings, furnaces, torches, boxes, cupboards, bags-on-frames. |
| `projectiles` | `DirtyTrackedMap<ProjectileId, Projectile>` | yes | **no** | In-flight arrows, transient (cleared on restart); the per-tick integration marks movers dirty, re-anchored via `chunk_manager` like dropped items. |
| `stuck_projectiles` | `HashMap<ProjectileId, u64>` | no | no | Server-only TTL bookkeeping for cosmetic stuck arrows (projectile id -> despawn tick); the client renders the projectile mirror entity meanwhile. |
| `loot_bags` | `HashMap<LootBagId, LootBag>` | yes | yes | Plain `HashMap` (full-walk mirror sync), not a `DirtyTrackedMap`. |
| `persisted_players` | `HashMap<AccountId, PersistedPlayer>` | no | yes | Offline inventory/position/admin snapshots. |
| `world_time` | `WorldTime` | broadcast (`WorldTime` msg), not per-component | yes | |
| `world_map_markers` | `WorldMapMarkerStore` | reply-only (`WorldMapMarkers`) | yes | Per-account, private to owner. |
| `chunk_manager` | `ChunkManager` | no | yes (`ChunkManagerSave`) | Owns anchor chunks + regrow schedule. See [chunks-and-aoi.md](chunks-and-aoi.md). |
| `claim_footprints` | `HashMap<DeployedEntityId, Vec<(f32,f32)>>` | **no** | **no** | Server-only derived cache. Rebuilt by `recompute_claim_footprints` on structural change. |
| `world` / `world_grid` | `WorldData` / `BlockGrid` | no | yes (`world`) | `world_grid` is a block spatial index used **only** for combat line-of-sight, not movement (movement is client-authoritative). |
| `meteor_shower` | `MeteorShowerState` | no | no | Runtime-only meteor-event engine (scheduler + live event). Deliberately neither replicated nor persisted: world load rolls a fresh next event, an in-flight event does not survive restart. |
| `next_*_id`, `tick`, `auto_save_*` | scalars | no | partly | Bookkeeping counters. |

`claim_footprints` is the one to remember: it is never persisted and never replicated. It is a pure runtime cache derived from placed cupboards + connected building footprint, recomputed on every structural change. Do not try to ship or save it.

## Two execution paths

There are exactly two ways state changes, and both end at `route_envelopes`.

**1. Message receive (per client message, every host update, ungated):**

```
ClientMessage arrives on the Lightyear connection entity
  -> receive_client_messages            (src/net/host/routing.rs)
  -> GameServer::receive(client_id, msg) (src/server/dispatch.rs), mutates a map
  -> returns Vec<ServerEnvelope>
  -> route_envelopes maps each DeliveryTarget to a MessageSender (routing.rs)
```

**2. Per-tick (20 Hz fixed step, gated):**

```
tick_authoritative_server crosses a fixed step (src/net/host.rs)
  -> GameServer::tick(delta) (src/server/tick.rs), mutates maps, advances clock
  -> returns Vec<ServerEnvelope> -> route_envelopes
  -> mirror_systems reconcile the maps into ECS entities (src/net/host/mirror.rs)
  -> Lightyear replicates per-component diffs to in-AoI clients
```

`tick` and the mirror/room systems are gated behind `server_tick_advanced` (the `ServerTickPulse.advanced` flag, `src/net/host.rs` - `tick_authoritative_server`). The host loop calls `app.update()` hundreds of times a second; the gate ensures the 20 Hz subsystems run only on updates where a fixed step actually crossed. The message/command/disconnect drains run every update, ungated. When you add a new mirror-sync system, chain it into the `mirror_systems` tuple in `run_host` so it inherits the gate; an ungated mirror system runs ~1000x/s. The mirror bridge itself is documented in [replication.md](replication.md), not here.

### receive() and tick() contracts

- `GameServer::receive(client_id, ClientMessage) -> Vec<ServerEnvelope>` (`src/server/dispatch.rs` - `receive`). Calls `mark_client_seen` first, then dispatches one `ClientMessage` variant to an `apply_*` handler. Never sends on the wire; returns envelopes.
- `GameServer::tick(delta_seconds: f32) -> Vec<ServerEnvelope>` (`src/server/tick.rs` - `tick`). Advances `self.tick` and the world clock, steps dropped-item physics, runs the chunk-manager regrow tick, ticks furnaces/torches/loot-bags/crafting, expires chat bubbles, runs the gameplay subsystems (`tick_ruin_caches` ruin-cache refills, `tick_reload_slows` lifting the crossbow reload movement slow once the reload window elapses, `tick_fuses` counting armed charges down to detonation envelopes, `tick_world_events` driving the meteor shower schedule/announce/impact/cleanup on real tick time so `/time-speed` does not accelerate meteors, and `tick_projectiles` stepping the ballistic sim), sweeps stale clients, and emits the surviving periodic broadcasts. Returns envelopes.

### ServerEnvelope and DeliveryTarget

`ServerEnvelope { target: DeliveryTarget, message: ServerMessage }` (`src/server.rs`) is the entire output contract. `DeliveryTarget` has four variants (`src/server.rs` - `enum DeliveryTarget`):

| Variant | Routing behavior (`route_envelopes`, `src/net/host/routing.rs`) |
|---|---|
| `Client(ClientId)` | Send to one client's sender. |
| `Broadcast` | Send a clone to every connected client. |
| `BroadcastExcept(ClientId)` | Send to everyone except the named client. For "echo to peers" payloads where the originator already produced the effect via local prediction; a second server copy would double-trigger it. |
| `Disconnect(ClientId)` | **Control signal, not a message.** Tears down the transport session: inserts Lightyear's `Disconnecting` component (`try_insert`, load-bearing against a race where Lightyear already despawned the entity) and clears the connection map via `forget_connection`. The envelope's `message` field is ignored. Without this, a kicked or stale client holds its entity until the netcode token timeout and a reconnect is rejected as "already connected." |

`GameServer::disconnect` (the `ClientMessage::Disconnect` and stale-sweep paths) emits a trailing `DeliveryTarget::Disconnect`. Do not route a `Disconnect` target by trying to send `message` on a sender; it is a teardown.

## Subsystem inventory

Every server concern owns a file (or directory) under `src/server/`. Open the owning file, not `architecture.md`'s old short list. Deep behavior for the gameplay-heavy ones lives in the cross-linked docs.

| Concern | File(s) | Notes / doc |
|---|---|---|
| Connection / auth / wake-sleep | `connection.rs` | `connect`, `wake_sleeper`, `hard_disconnect`. Sleeping-body model below. |
| Message dispatch | `dispatch.rs` | The `receive` match. |
| Per-tick loop | `tick.rs` | Periodic broadcasts, stale sweep. |
| Movement acceptance | `movement.rs` | `accept_client_movement`. Client-authoritative; see [movement.md](movement.md). |
| Inventory | `inventory.rs`, `container_slots.rs` | Slot moves, drops, pickups; shared container-slot logic. |
| Dropped items | `dropped_items.rs`, `dropped_item_ecs.rs` | Physics + merge/cleanup; mirror components. |
| Resource nodes | `resource_nodes.rs`, `resource_node_ecs.rs` | Gather rules; mirror components. See [items-and-resources.md](items-and-resources.md). |
| Combat / PvP | `combat.rs`, `swing.rs` | `apply_attack_player_command`, `apply_swing_start`. See [pvp-combat.md](pvp-combat.md). |
| Ranged / projectiles | `projectiles.rs`, `projectile_ecs.rs` | Draw/fire validation, server ballistic sim, stuck arrows; mirror components. See [pvp-combat.md](pvp-combat.md). |
| Player lifecycle / death / respawn | `combat.rs`, `sleeping_bag.rs`, `lifecycle.rs` | Death pile and respawn in `combat.rs` (`kill_player`, `apply_respawn_command`); respawn-at-bag in `sleeping_bag.rs`. `lifecycle.rs` owns only `GameServer` construction and the `with_auto_save` / `with_workos` builders. |
| Base building | `building.rs`, `stability.rs`, `door.rs` | Placement, stability graph, doors. See [base-building-and-claims.md](base-building-and-claims.md). |
| Tool Cupboard claims | `claim.rs` | Footprint cache, per-object auth. See [base-building-and-claims.md](base-building-and-claims.md). |
| Crafting | `crafting.rs` | Queue tick. See [crafting-and-deployables.md](crafting-and-deployables.md). |
| Furnaces | `furnace/` (`state.rs`, `tick.rs`, `commands.rs`, `commands/`) | Smelt state machine. See [crafting-and-deployables.md](crafting-and-deployables.md). |
| Workbenches / tier upgrade | `workbench.rs` | Open/close + the generic in-place tier upgrade via the `DEPLOYABLE_UPGRADES` table. See [crafting-and-deployables.md](crafting-and-deployables.md). |
| Deployables | `deployables.rs`, `deployable_ecs.rs`, `deployables/` | Placement/damage/ownership; mirror components. |
| Explosive fuses | `fuse.rs` | Armed-charge countdown -> detonation (`tick_fuses`). See [crafting-and-deployables.md](crafting-and-deployables.md). |
| Explosion resolution | `explosion.rs` | `resolve_explosion` AoE against players and structures. See [pvp-combat.md](pvp-combat.md). |
| Charge defuse | `defuse.rs` | Claim-authorized defuse + half-materials refund. See [crafting-and-deployables.md](crafting-and-deployables.md). |
| Loot bags / death piles | `loot_bag.rs`, `loot_bag_ecs.rs`, `loot_bag/` | Spawned by the kill chain, despawned when emptied + closed. |
| Sleeping bags / respawn points | `sleeping_bag.rs` | `RespawnAtBag`. |
| Storage boxes | `storage_box.rs` | `OpenStorageBox`. |
| Torches | `torch.rs` | Burn timer, ticked in `tick`. |
| Ruin caches | `ruin_cache.rs` | Refill scheduler + seeded loot rolls for world-spawned caches. See [crafting-and-deployables.md](crafting-and-deployables.md). |
| Meteor shower event | `meteor_shower.rs` | Scheduler, siting, impact, crater cleanup (`tick_world_events`). See [meteor-shower.md](meteor-shower.md). |
| World map | `world_map.rs` | Marker store + `RequestWorldMap` -> `WorldMapMarkers`. (Terrain is client-side from seed; the raster code here is not on the wire.) |
| Tool wear | `tool_wear.rs` | Durability decrement. |
| Voice routing | `voice.rs` | `apply_voice_frame`, spatial fan-out, range from `game_balance::VOICE_AUDIBLE_RANGE_M` (50 m). See [voice.md](voice.md). |
| Slash commands | `commands.rs` + `commands/` (`kit.rs`, `player.rs`, `time.rs`, `world.rs`) | `apply_command`. Table below. |
| Chunk grid / AoI / regrow | `chunk_manager/` | See [chunks-and-aoi.md](chunks-and-aoi.md). |
| Persistence boundary | `persistence.rs`, `lifecycle.rs` | `world_save`, auto-save flag plumbing. See [worlds-and-saves.md](worlds-and-saves.md). |
| Toasts | `toasts.rs` | Issuer-targeted toast helpers. |
| DirtyTrackedMap | `dirty_tracked_map.rs` | The delta-tracking map type. |
| Mandatory-entry helpers | `queries.rs` | `insert_/remove_/_state_mut` for the delta-tracked maps. |

The mirror-sync systems that consume these maps live in `src/net/host/mirror.rs`, **not** in `src/server/` and **not** in `src/net/host.rs` (which only wires them). If you are looking for `sync_player_entities` or `sync_deployable_entities`, open `mirror.rs`.

## ClientMessage -> apply_* dispatch table

The `match` in `GameServer::receive` (`src/server/dispatch.rs`). This is the index for "where do I handle message X." Adding a handler means adding an `apply_*` method that returns `Vec<ServerEnvelope>` and wiring it here.

| ClientMessage | Handler | File |
|---|---|---|
| `Auth { .. }` | rejects (already authenticated) | `dispatch.rs` |
| `Movement(m)` | `accept_client_movement` (only if `is_alive()`), re-anchors chunk | `dispatch.rs` / `movement.rs` |
| `Chat { text }` | sanitize, set chat bubble, broadcast | `dispatch.rs` |
| `Command { text }` | `apply_command` | `commands.rs` |
| `Inventory(c)` | `note_action_seq` then `apply_inventory_command` | `inventory.rs` |
| `Crafting(c)` | `apply_crafting_command` | `crafting.rs` |
| `Gather(c)` | `note_action_seq` then `apply_gather_command` | `resource_nodes.rs` |
| `PlaceDeployable(c)` | `apply_place_deployable_command` | `deployables.rs` |
| `Furnace(c)` | `apply_furnace_command` | `furnace/commands.rs` |
| `Workbench(c)` | `apply_workbench_command` | `workbench.rs` |
| `Ranged(c)` | `apply_ranged_command` (draw start/cancel, fire) | `projectiles.rs` |
| `Explosive(c)` | `apply_explosive_command` (`Throw` -> `throw_explosive`, `Defuse` -> `defuse_charge`) | `projectiles.rs` / `defuse.rs` |
| `DamageDeployable(c)` | `apply_damage_deployable_command` | `deployables.rs` |
| `AttackPlayer(c)` | `apply_attack_player_command` | `combat.rs` |
| `SwingStart(c)` | `note_action_seq` then `apply_swing_start` | `swing.rs` |
| `Respawn` | `apply_respawn_command` | `combat.rs` |
| `RespawnAtBag { id }` | `apply_respawn_at_bag_command` | `sleeping_bag.rs` |
| `PlaceBuilding(c)` | `apply_place_building_command` | `building.rs` |
| `Building(c)` | `apply_building_command` | `building.rs` |
| `Door(c)` | `apply_door_command` | `door.rs` |
| `SleepingBag(c)` | `apply_sleeping_bag_command` | `sleeping_bag.rs` |
| `Claim(c)` | `apply_claim_command` | `claim.rs` |
| `LootBag(c)` | `apply_loot_bag_command` | `loot_bag.rs` |
| `LootSleeper { client_id }` | `apply_loot_sleeper` | `loot_bag.rs` |
| `OpenStorageBox { id }` | `apply_open_storage_box` | `storage_box.rs` |
| `RequestWorldMap` | `apply_world_map_request` | `world_map.rs` |
| `WorldMapMarker(c)` | `apply_world_map_marker_command` | `world_map.rs` |
| `SetViewRadius { tier }` | sets `client.view_tier` (drives AoI ring size) | `dispatch.rs` |
| `Voice(v)` | `apply_voice_frame` | `voice.rs` |
| `Heartbeat` | no-op (handled by `mark_client_seen`) | `dispatch.rs` |
| `Ping { .. }` | stores ping, echoes `Pong` | `dispatch.rs` |
| `Disconnect` | `disconnect` (emits `DeliveryTarget::Disconnect`) | `connection.rs` |

`note_action_seq` (`src/server/dispatch.rs`) advances the client's optimistic-prediction high-water mark **before** the handler runs, so a rejected predicted command still lets the client prune and revert its optimistic overlay. Predicted/optimistic commands (inventory, gather, swing) must call it before dispatch.

### DirtyTrackedMap requirement

Any authoritative map whose entities mutate **post-spawn** must be a `DirtyTrackedMap`, and all mutation must go through the mandatory-entry helpers in `src/server/queries.rs` (`insert_resource_node`, `remove_resource_node`, `resource_node_state_mut`, `insert_dropped_item`, `insert_deployed_entity`, and siblings). Those helpers flag the affected id into the map's `dirty`/`removed` sets; the delta-driven mirror sync drains those sets and only touches changed entities (O(delta), not O(live entities)). If you mutate a `DirtyTrackedMap` entry without going through a helper (or `for_each_mut_then_mark` / `mark_dirty`), the mirror never sees the change and the entity goes silently stale.

`resource_nodes`, `dropped_items`, and `deployed_entities` are `DirtyTrackedMap`. `loot_bags` is a plain `HashMap` and is mirrored by a full walk every tick (`sync_loot_bag_entities`); players are likewise a full walk because the pose mutates every tick. Check before copying the loot-bag pattern; new mutating entity types should use `DirtyTrackedMap`.

## Surviving periodic broadcasts

`tick` emits exactly three periodic broadcasts, none of which is per-entity state:

- `ServerMessage::WorldTime`, once per real minute (plus an immediate out-of-band push after a `/time` or `/time-speed` change). Clients integrate locally between snapshots.
- `ServerMessage::PerfStats`, once per second, per client (own AoI node count + world chunk bookkeeping).
- `ServerMessage::PlayerList`, once per second, broadcast (the whole-server roster, AoI-independent, so the pause screen shows everyone).

The per-tick `ServerMessage::Snapshot` / `WorldSnapshot` full-state broadcast was **deleted** (the comment in `src/server/tick.rs` still notes the removal). Every per-entity state consumer reads from Lightyear-replicated components. **Never reintroduce a periodic full-state broadcast** to ship entity state; fix the replication path instead. This is a CLAUDE.md invariant; the replicated-entity procedure is in [replication.md](replication.md) and [docs/playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md).

## Sleeping bodies (online == false) gotcha

A logged-out player leaves a Rust-style sleeping body in the world. `ServerClient.online` (`src/server.rs` - `struct ServerClient`) is `false` for these: the body stays in `clients` (still replicated, lootable, killable, frozen) but is excluded from the online roster, the stale-timeout, and voice fan-out. Reconnect from the same account wakes the body in place (`wake_sleeper`, `src/server/connection.rs`); a second live login from the same account hard-disconnects the old session (`hard_disconnect`).

The constant gotcha: **any loop over `self.clients` that fans out network traffic or counts "online" players must filter `client.online`.** `apply_voice_frame` (`voice.rs`) and the `PlayerList` builder (`tick.rs`) both do. Forgetting it builds envelopes the router silently drops, or shows logged-out players as online. Stale-client sweep: `CLIENT_STALE_TIMEOUT_TICKS = 20 * 3` (`src/server.rs`), i.e. 3 missed 1 Hz heartbeats sweeps a live client to a sleeping body, via `disconnect_stale_clients` in `tick`.

Armor is live: `ServerClient.protection` (`src/server.rs` - `struct ServerClient`, an `ArmorProtection`) holds the authoritative per-damage-kind mitigation, recomputed via `crate::items::equipped_protection` from the worn equipment slots whenever equipment can change (equip/unequip moves, connect/restore, durability wear, death). The mirror ships only the melee component as the replicated `PlayerArmor` HUD value (`src/server/queries.rs`); the full per-kind protection stays server-only.

## Slash commands

`GameServer::apply_command` (`src/server/commands.rs`) parses the leading `/`, splits on whitespace, and dispatches. Submodules: `kit.rs`, `player.rs`, `time.rs`, `world.rs` (plus colocated `tests.rs`); the two `/meteor_shower` handlers live in `src/server/meteor_shower.rs`. Commands: `/spawn`, `/drain`, `/time`, `/speed`, `/knockback-scale` (alias `knockbackscale`), `/time-speed` (aliases `timespeed`, `timescale`), `/test-kit` (alias `testkit`), `/give`, `/tp` (alias `teleport`), `/ruins [tp]`, `/meteor_shower`, `/meteor_shower-here` (alias `meteor_showerhere`), `/help`. All except `/help` are admin-gated; `/help` lists every command and marks the gated ones for non-admins.

## Loopback vs dedicated: the only differences

Both modes call `run_host` (`src/net/host.rs`), which inserts the same `AuthoritativeServer(GameServer)`. The only injected differences:

- **Auto-save cadence.** Singleplayer: `with_auto_save_silent` at `SINGLEPLAYER_AUTO_SAVE_INTERVAL_TICKS` (5 minutes, no chat announce). Dedicated: `with_auto_save` at `AUTO_SAVE_INTERVAL_TICKS` (30 minutes, announced with a 30-second heads-up). Builders live in `src/server/lifecycle.rs`; constants in `src/server.rs`.
- **WorkOS verifier presence.** Dedicated `AuthMode::Workos` attaches one via `with_workos`; loopback and test runs leave it `None`.
- **Admin Unix socket.** Dedicated-only (`src/net/host/admin.rs`), driven by `./cli admin`.

Nothing else differs. Do not branch gameplay on the mode inside `src/server/`.

## Tests: submodule-per-subsystem

Server behavior is covered by `src/server/tests/` (`src/server/tests.rs` registers the submodules and re-exports the shared `connect_host` / `movement` / `server` / `equip_basic_tools` harness from `src/server/test_support.rs`). Current submodules: `building`, `claim`, `combat`, `commands`, `connection`, `deployables`, `door`, `dropped_items`, `explosives`, `furnace`, `heal`, `inventory`, `loot_bag`, `movement`, `projectiles`, `resource_nodes`, `ruins`, `sleeping_bag`, `storage_box`, `tool_wear`, `workbench`. Some modules also keep colocated `#[cfg(test)]` unit tests (e.g. `voice.rs`, `dirty_tracked_map.rs`); the `net` module's integration tests live in `src/net/tests.rs` (declared in `src/net.rs`).

Convention: add a test for new server behavior in the matching `tests/` submodule. Protocol changes, server authority, persistence, and the dispatch contract especially should be tested near the owning module.

## Related docs

- [docs/replication.md](replication.md) - the ECS-mirror bridge (`mirror.rs`) that turns these maps into Lightyear-replicated entities; the per-component split and bug #740 group rule.
- [docs/networking.md](networking.md) - the full `ClientMessage`/`ServerMessage` wire inventory, channels, and handshake.
- [docs/chunks-and-aoi.md](chunks-and-aoi.md) - `chunk_manager`, anchor chunks, and the room-based AoI gate.
- [docs/movement.md](movement.md) - the client-authoritative movement trust boundary.
- [docs/pvp-combat.md](pvp-combat.md) - combat, swing, death, respawn, loot bags.
- [docs/base-building-and-claims.md](base-building-and-claims.md) - building, stability, doors, Tool Cupboard claims.
- [docs/crafting-and-deployables.md](crafting-and-deployables.md) - crafting queue, furnaces, workbench tiers, the unified deployable system, charges/fuses/defuse.
- [docs/meteor-shower.md](meteor-shower.md) - the meteor shower event (scheduler, siting, impact, cleanup) that `tick_world_events` drives.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - `WorldSave` and the persistence boundary `tick` defers to.
- [docs/playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md) - the step-by-step for a new networked entity.
