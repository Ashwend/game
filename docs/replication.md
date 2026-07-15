---
title: Per-component replication and the host mirror
owns: How authoritative GameServer state reaches clients via Lightyear per-component replication and the ECS mirror.
when_to_read: Before adding a new replicated entity, a new replicated component, a mirror-sync system, or any client reconciliation system; or when debugging stale replicated state.
sources:
  - src/net/host.rs - run_host bootstrap, mirror_systems tuple, ServerTickPulse gate
  - src/net/host/mirror.rs - sync_* exclusive systems, refresh_player_component! macro
  - src/net/host/rooms.rs - attach_*_replication, owner_only_overrides, room subscriptions, install_replication_sender_on_link
  - src/net/host/routing.rs - receive_client_messages, route_envelopes, handshake
  - src/net/channels.rs - LightyearProtocolPlugin, register_component registry, LIGHTYEAR_PROTOCOL_ID
  - src/server/dirty_tracked_map.rs - DirtyTrackedMap delta tracking
  - src/server/queries.rs - mandatory-entry helpers + drain_*_sync
  - src/server/player_ecs.rs - player identity/public/owner-only component split
  - src/server/deployable_ecs.rs - Deployable identity + per-field components
  - src/app/systems/items/resource_nodes.rs - canonical event-driven client reconciler
  - src/app/systems/replication_trace.rs - client RECV trace logs
related:
  - docs/networking.md - transport, channels, ClientMessage/ServerMessage inventory, handshake
  - docs/server-authority.md - GameServer authoritative state, dispatch, tick, DeliveryTarget/ServerEnvelope
  - docs/chunks-and-aoi.md - chunk grid, AoI ring math, room-subscription system, node regrow
  - docs/playbooks/add-replicated-entity.md - step-by-step recipe
  - docs/profiling.md - the per-frame O(live-entities) traps this doc's patterns avoid
---

# Per-component replication and the host mirror

> When to read this: before adding a replicated entity/component, a mirror-sync system, or a client reconciler, or when debugging stale replicated state. Source of truth: `src/net/host/mirror.rs`, `src/net/host/rooms.rs`, `src/server/dirty_tracked_map.rs`, `src/server/queries.rs`. Canonical invariants live in CLAUDE.md (replicated-state rules 1-6).

This doc owns the replication internals. For the wire-message inventory, channels, and handshake see [docs/networking.md](networking.md); for the authoritative `GameServer` and its dispatch/tick contract see [docs/server-authority.md](server-authority.md); for the chunk grid and AoI ring math see [docs/chunks-and-aoi.md](chunks-and-aoi.md).

Lightyear version is `0.28.0` (Cargo.toml, features `client, server, netcode, udp, replication`). The upstream-bug workarounds below were discovered and version-matched against `0.26.4` (bug #740) and deliberately retained through the 0.28 upgrade; the regression test in `src/net/tests.rs` guards the behavior, so drop a workaround only when that test proves it obsolete.

## The two execution paths

The host runs one `GameServer` (loopback singleplayer and dedicated multiplayer share the same `run_host`, see [docs/server-authority.md](server-authority.md)). Two distinct paths move data to clients. Do not conflate them.

1. **Message-receive (every Update, ungated).** A `ClientMessage` arrives on the Lightyear connection entity. `receive_client_messages` (`src/net/host/routing.rs`) drains it, calls `GameServer::receive` (`src/server/dispatch.rs`), which mutates a `HashMap`/`DirtyTrackedMap` and returns `Vec<ServerEnvelope>`. `route_envelopes` maps each `DeliveryTarget` to a `MessageSender`. The `GameServer` itself never touches the wire; it returns envelopes. This path carries control/auth/chat/commands, NOT per-entity world state.
2. **Per-tick replication (gated on a 20 Hz tick crossing).** `tick_authoritative_server` calls `GameServer::tick`, mutating the authoritative maps. Then the `mirror_systems` tuple reconciles those maps into per-entity ECS mirror entities carrying Lightyear-replicated components. Lightyear ships the per-component diffs to subscribed clients. This is how resource nodes, dropped items, deployables, players, and loot bags reach the client.

The system chain in `run_host` (`src/net/host.rs`, the `mirror_systems` tuple) is, in order: `drain_host_commands` -> (`drain_admin_socket`, unix only) -> `receive_client_messages` -> `handle_disconnected_clients` -> `tick_authoritative_server` -> `mirror_systems`. Everything up to and including `tick_authoritative_server` runs every Update; only `mirror_systems` is gated.

### The ServerTickPulse gate

The host loop calls `app.update()` roughly 500-1000 times per second, but the simulation tick is 20 Hz (`SERVER_TICK_RATE_HZ = 20.0`, `src/protocol.rs`). The mirror-sync and room-subscription systems only have work when a fixed tick actually crossed. They share a `run_if(server_tick_advanced)` condition driven by `ServerTickPulse.advanced` (`src/net/host.rs`, set in `tick_authoritative_server` when the accumulator passes `fixed_delta`).

The `mirror_systems` tuple is:

```
sync_resource_node_entities,
sync_dropped_item_entities,
sync_deployable_entities,
sync_projectile_entities,
sync_player_entities,
sync_loot_bag_entities,
update_client_room_subscriptions,
```

`.chain().run_if(server_tick_advanced)`.

**Any new mirror-sync system MUST be chained into this tuple** so it inherits the gate. Adding it ungated runs it ~1000x/s and burns CPU for nothing. See [docs/profiling.md](profiling.md) for why per-loop work at this rate is a frame killer.

## host/ submodule division

`src/net/host.rs` is the bootstrap only: `spawn_loopback_server`, `run_game_server`, `run_host`, `tick_authoritative_server`, the `ServerTickPulse` resource, and the `mirror_systems` wiring. The work lives in `src/net/host/`:

- `mirror.rs` - the six exclusive `sync_*` systems (`world: &mut World`) and the `refresh_player_component!` macro. This is where `HashMap -> ECS` reconciliation and the server-side `MUTATE` trace logs live. Five of the six ride the shared `reconcile_mirror_entities` skeleton (snapshot the delta, despawn removed ids, then refresh-or-spawn per item); only the per-entity refresh/spawn closures are bespoke. `sync_resource_node_entities` stays fully bespoke for its world-load spawn budget.
- `rooms.rs` - chunk-room AoI: `attach_room_gated_replication` / `attach_player_replication` (the only sanctioned spawn paths), `owner_only_overrides`, `rebind_player_owner_if_changed`, `update_client_room_subscriptions`, `install_replication_sender_on_link`, and the unit test guarding the per-entity `ReplicationGroup` contract.
- `routing.rs` - `receive_client_messages`, `route_envelopes`, the pre-auth handshake (`handle_unauthenticated_message`, `VersionMismatch`).
- `admin.rs` - the dedicated-only Unix admin socket.
- `handle.rs` - host handle / shutdown plumbing.

If you grep for `sync_player_entities` and look in `host.rs`, you will not find it; it is in `mirror.rs`. The old docs that say "in `src/net/host.rs`" drifted.

## Identity vs per-mutable-field split

Lightyear ships **whole-component values**, not field diffs. So bundling fields that change at different cadences re-ships the slow field at the fast field's rate. The rule (CLAUDE.md replicated-state rule 3): one **identity** component (immutable after spawn) plus one component per mutable field that changes at its own cadence. All components are registered in `src/net/channels.rs` (`LightyearProtocolPlugin::build`); both server and client install the same plugin so the registries match.

### Resource node

| Component | Mutability | Notes |
| --- | --- | --- |
| `ResourceNode` | identity | id, definition_id (the kind), position, yaw, dead (tree dead-snag flag); all immutable post-spawn (`src/server/resource_node_ecs.rs`) |
| `ResourceNodeStorage(Vec<ItemStack>)` | mutable | remaining yield; shrinks on each gather |

### Deployable (the full set, `src/server/deployable_ecs.rs`)

| Component | Mutability | Notes |
| --- | --- | --- |
| `Deployable` | identity | `kind` is immutable post-spawn; a kind change is handled by despawn+respawn (below) |
| `DeployableTransform` | mutable | position/orientation |
| `DeployableHealth(u32)` | mutable | damage/repair |
| `DeployableActive(bool)` | mutable | flips when a furnace lights / extinguishes |
| `DeployableLabel(Option<String>)` | mutable | sign/door label; value-delta trace-gated |
| `DeployableStability(u8)` | mutable | building stability; value-delta trace-gated |
| `DeployableAuth(Vec<AccountId>)` | mutable | Tool Cupboard auth list; value-delta trace-gated |

`DeployableLabel`, `DeployableStability`, and `DeployableAuth` are all registered and have full `MUTATE`/`RECV` trace coverage; do not assume only health/active replicate.

### Dropped item

| Component | Mutability | Notes |
| --- | --- | --- |
| `DroppedItem` | identity + stack | id and the `ItemStack`; the stack count is value-delta trace-gated |
| `DroppedItemTransform` | mutable | physics body transform; the drop step flags only bodies that actually moved |

### Projectile (`src/server/projectile_ecs.rs`)

The in-flight arrow (bow / crossbow). Transient: never persisted, and its authoritative state (`GameServer::projectiles`) is a `DirtyTrackedMap` cleared on restart. Unlike the other props a projectile MOVES every tick, so `sync_projectile_entities` re-anchors it to its current chunk room (via `move_entity_between_rooms`, the dropped-item pattern) as it flies. The anchor chunk is a pure function of the projectile's position (`ChunkCoord::from_world`), not tracked by `chunk_manager`, since projectiles are few and short-lived. `RECV` trace lives in its own `log_replicated_projectile_changes_system` (the main trace system is at Bevy's system-param cap).

| Component | Mutability | Notes |
| --- | --- | --- |
| `Projectile` | identity | `id`, `model` (the firing weapon's Bow/Crossbow archetype for impact VFX/cue), and `owner` (the shooter); all immutable post-spawn |
| `ProjectileTransform` | per-tick | position + velocity, replicated together so the client extrapolates the arrow's path between the 20 Hz diffs (a fast arrow moves metres per tick); trace-covered as `ProjectileTransform` |

### Loot bag (`src/server/loot_bag_ecs.rs`)

| Component | Mutability | Notes |
| --- | --- | --- |
| `LootBagEntity` | identity | the `LootBag` struct, re-exported as `LootBagEntity` and registered under that name |
| `LootBagTransform` | mutable | spawns `BAG_SPAWN_HEIGHT_M = 1.0` above the corpse's feet and falls under gravity (`BAG_GRAVITY = 18.0`) to its support surface, resting after a variable number of ticks |
| `LootBagContents(Vec<Option<ItemStack>>)` | mutable | **disabled for replication in release builds** (see below) |

### Player (`src/server/player_ecs.rs`)

Peer-visible (broadcast to every sender in the same chunk room):

| Component | Mutability | Notes |
| --- | --- | --- |
| `Player` | identity | `client_id` + `account_id`; immutable post-spawn |
| `PlayerProfile` | rare | name + admin flag |
| `PlayerPose` | per-tick | position/velocity/yaw/pitch/grounded; the 20 Hz component |
| `PlayerHealth` | on damage/heal | |
| `PlayerChatBubble` | on chat | live bubble text, `None` once expired |
| `PlayerHeldItem` | on swap | ships the 1-byte `HeldMesh` enum, NOT the item id string |
| `PlayerEquipmentVisual` | on equip/unequip | four 1-byte `ArmorMesh` selectors (head/chest/legs/feet) that drive the worn armor on the third-person rig; ships selectors, NOT item id strings (mirrors `PlayerHeldItem`); registered in `src/net/channels.rs`, refreshed in `sync_player_entities` |
| `PlayerAction` | on swing | `{ seq, tool }`; peers edge-detect a `seq` change (never `Ref::is_changed`) |
| `PlayerArmor` | on equipment change | the melee column of the worn set's `ArmorProtection` (percent mitigation), rebuilt server-side on every equip/unequip or armor durability wear (`src/server/queries.rs` builds `PlayerArmor(client.protection.melee)`); feeds the HUD armor readout. The full per-kind protection stays server-only. |
| `PlayerLifecycle` | on death/respawn | Alive / Dead, drives corpse anim + death splash |
| `PlayerSleeping(bool)` | on logout/wake | Rust-style logged-out body marker |

Owner-only (gated to the owning client's sender, never seen by peers):

| Component | Mutability | Notes |
| --- | --- | --- |
| `PlayerInventory` | on inventory change | |
| `PlayerCrafting` | while jobs run | |
| `PlayerOpenContainers` | while a furnace/loot-bag/workbench view is open | the loot-bag UI rides `open_loot_bag` here, not `LootBagContents`; the workbench upgrade UI rides `open_workbench: Option<OpenWorkbenchView>` (id + current tier, `src/server/player_ecs.rs`) |
| `PlayerInputAck` | per-tick while moving | last processed input + applied action seq |

The four owner-only components were split out because the old `PlayerPublic`/`PlayerPrivate` mega-components re-shipped the full inventory at 20 Hz (the input ack ticking every tick made the bundled value compare unequal). The client reassembles the four owner-only components into one view struct in `update_local_player_state_system`.

#### Owner-only gating and rebind-on-reconnect

Each owner-only component carries one `ComponentReplicationOverrides<T>`, built `.disable_all().enable_for(owner_sender)` by `owner_only_overrides` in `rooms.rs`. The owner sender is captured at spawn (`attach_player_replication`).

A reconnect that wakes a sleeping body keeps the **same mirror entity** but is handed a **brand-new sender**. `rebind_player_owner_if_changed` (called from `sync_player_entities`) re-points all four overrides at the new sender on the next tick; without it the woken player's inventory/crafting never reaches their new connection and the join splash hangs forever. **If you add a new owner-only component, add it to BOTH `attach_player_replication` and `rebind_player_owner_if_changed`.**

The plumbing that makes per-client senders work: `install_replication_sender_on_link` is an observer on `Add<LinkOf>` that inserts a `ReplicationSender` on every new client link; `RoomPlugin::handle_disconnect` scrubs that sender from all rooms on disconnect.

#### LootBagContents disabled in release

`LootBagContents` is registered for registry parity but a `ComponentReplicationOverrides::<LootBagContents>::default().disable_all()` is inserted at the bag spawn site under `#[cfg(not(feature = "replication-trace"))]` (`src/net/host/mirror.rs`). Nothing in release consumes the replicated contents (the bag UI rides the owner-only `PlayerOpenContainers::open_loot_bag` view), so release neither clones the slot list per tick nor ships it. The `replication-trace` build re-enables it for `MUTATE`/`RECV` coverage.

## ReplicationGroup and bug #740

**Every spawn MUST attach `ReplicationGroup::new_from_entity()`.** Both sanctioned spawn helpers do it: `attach_room_gated_replication` (static room-only entities) and `attach_player_replication` (player split), in `src/net/host/rooms.rs`.

The bug ([Lightyear #740](https://github.com/cBournhonesque/lightyear/issues/740)): `SendUpdatesMode::SinceLastAck` (the default) gates each component update on a per-`(sender, ReplicationGroupId)` ack tick. The shared `DEFAULT_GROUP = ReplicationGroupId(0)` is used by every entity that does not set its own group. A frequently-updated entity in that group advances the shared ack past a slowly-changing entity's local `Changed` mark, after which Lightyear concludes "nothing new to send" for the slow entity even though it just changed, and the diff is silently dropped. `new_from_entity()` uses `Entity::to_bits()` as the group id so each entity gets its own ack tick and the race goes away.

A bare `Replicate::to_clients(...)` without an explicit group lands in group 0 and hits the bug. Do not bypass the helpers.

A unit test in `rooms.rs` enforces the contract: it stands up the minimal `ServerPlugins + RoomPlugin` set, calls `attach_room_gated_replication`, and asserts `group.group_id(Some(entity)) == ReplicationGroupId(entity.to_bits())` and `!= ReplicationGroupId(0)`. `Replicate::to_clients(NetworkTarget::All)` (not `None`) is also load-bearing: listing the sender in the target up front avoids a separate room change-detection ambiguity; `NetworkVisibility` still gates actual visibility per room state, so peers in unrelated chunks receive nothing.

The upstream rewrite is [PR #1361](https://github.com/cBournhonesque/lightyear/pull/1361) (replaces the replication subsystem with `bevy_replicon`). When that ships the per-entity-group requirement may relax, but `new_from_entity()` stays safe.

## Delta-sync vs full-walk

The mirror-sync systems split into two strategies.

**Delta-driven (resource nodes, dropped items, deployables).** A world can hold tens of thousands of live entities; reconciling all of them every tick pinned the host loop to ~10 Hz (~100 ms/tick). Instead each map is a `DirtyTrackedMap` (`src/server/dirty_tracked_map.rs`): any `&mut` access or insert flags the id dirty, removal records the id removed, and the `seed_all_dirty()` method (called explicitly after the `from_map` constructor, e.g. on world load) flags every live id dirty so the first sync still spawns everything once. Each `sync_*` system drains its delta via `drain_*_sync` and processes only the changed ids: O(changed), not O(live entities).

**The mandatory-entry helpers in `src/server/queries.rs` are how the dirty set gets marked.** Any mutation of a delta-tracked map MUST route through them or the dirty flag is never set and the mirror silently goes stale:

- resource nodes: `insert_resource_node`, `remove_resource_node`, `resource_node_state_mut`
- dropped items: `insert_dropped_item`, `remove_dropped_item`, `dropped_item_body_mut`
- deployables: `insert_deployed_entity`, `remove_deployed_entity`, `deployed_entity_mut` (plus `mark_deployable_dirty` for in-place flags)

The two per-tick bulk paths mark selectively, not conservatively: the dropped-item physics step flags only bodies whose transform actually changed (items at rest cost nothing), and the furnace tick flags a deployable only when its replicated `active` flag flips.

**Full-walk (players AND loot bags).** Two types deliberately do NOT use a dirty set:

- `sync_player_entities`: pose mutates every tick while moving, so a dirty set would be marked for nearly every player every tick anyway. It walks all connected clients and does a per-component compare-and-write.
- `sync_loot_bag_entities`: death bags are far rarer than nodes/drops, and the spawn-time settling transform would need per-tick dirty marking to avoid freezing a falling bag. It walks all bags. The map is a plain `HashMap`, not a `DirtyTrackedMap`. Check this before copying the loot-bag pattern for a frequently-mutated type.

## The compare-and-write invariant

The server-side sync writes a component only when the authoritative value differs from the current replicated value: `if *current != value { *current = value }`. This keeps Bevy's `Changed<T>` honest so Lightyear only ships a diff when something actually moved. The player path uses the `refresh_player_component!` macro (`src/net/host/mirror.rs`) which does exactly this compare-and-write and emits the server `MUTATE` trace log on a real change.

Bevy's server-side change detection is reliable, so this compare-and-write is sound on the server.

**On the client, `Ref::is_changed()` lies.** Lightyear's receive path uses `insert_by_ids`, which bumps the change tick every replication tick even when the received value is identical. Any client-side work, and any client `RECV` trace log, MUST gate on a real before -> after value delta, never on `is_changed()`. The value-delta-tracked components (`DeployableLabel`/`Stability`/`Auth`, `PlayerHeldItem`, `PlayerAction`, the `DroppedItem` stack count) all do this.

## Despawn/respawn on identity-change

An identity field that cannot be expressed as a diff is handled by despawning and respawning the mirror entity. Example: a building-block tier upgrade changes `Deployable.kind`. `sync_deployable_entities` compares `Deployable.kind`, despawns the mirror on mismatch, and spawns a fresh one (logged as `RESPAWN` under `replication-trace`, `src/net/host/mirror.rs`). Clients see an ordinary remove + add through the `Added`/`RemovedComponents` lifecycle, no special-casing. Do not try to make an immutable identity component mutable.

## Chunk-room AoI and two-threshold hysteresis

Visibility is room-gated. One Lightyear `Room` entity per `ChunkCoord`, lazily allocated in `ChunkRoomMap`. A mirror entity joins its anchor chunk's room via a `RoomEvent { room, target: RoomTarget::AddEntity(entity) }` at spawn; a client subscribes via `RoomTarget::AddSender`/`RemoveSender`. Clients sharing a room receive that room's spawns/despawns/diffs automatically. The chunk grid, ring math, and regrow scheduling are owned by [docs/chunks-and-aoi.md](chunks-and-aoi.md); this section covers only how subscriptions feed replication.

`update_client_room_subscriptions` (`src/net/host/rooms.rs`) uses **spatial hysteresis** with two radii to stop boundary thrash:

- **add radius** (`visible_chunks_for_client`): view-tier radius + `LOAD_BUFFER_RINGS`. A chunk is subscribed as soon as it enters this radius.
- **keep radius** (`retained_chunks_for_client`): add radius + `KEEP_MARGIN_RINGS`. A chunk is only unsubscribed once it falls outside this wider radius.

Because the keep set is a strict superset of the add set, a player wobbling across a chunk boundary never crosses both thresholds, so no chunk loads -> unloads -> reloads. Deterministic, no timer. Each reconcile subscribes `add - subscribed` and unsubscribes `subscribed - keep`.

Two short-circuits and a sleeping-body case:

- **Idle short-circuit:** if the client's `client_aoi_key` (anchor chunk + view tier) is unchanged since last reconcile, the grid scan and set diff are skipped entirely.
- **Sleeping body:** a client id can be "in the world" (still in `connected_client_ids`) but have no live sender (logged-out body, transport gone). The reconcile drops that client's cached anchor and subscribed set so that when a sender reappears (the player reconnects and `wake_sleeper` reuses the same id with a new sender), the next reconcile re-subscribes the new sender from scratch. Without this the woken player receives nothing, not even its own entity.

Per-client AoI ring size is driven by `ClientMessage::SetViewRadius` (view tier -> `view_tier_radius`), which feeds the visible/retained chunk computation. That is the loop between the view-radius UI setting and how much state replicates to that client.

## Event-driven client reconciler

Client reconciliation systems that mirror replicated entities into local-only visuals MUST be event-driven (CLAUDE.md replicated-state rule 6). React to `Added<T>` and `RemovedComponents<T>`; never iterate the full replicated query every frame. At AoI scale (~1800 nodes) the noop full-iteration costs 1-4 ms per frame for nothing. See [docs/profiling.md](profiling.md).

The canonical pattern is `src/app/systems/items/resource_nodes.rs`. Copy its structure:

- **Pending-spawn `VecDeque`.** `Added<ResourceNode>` arrivals queue up and are drained against a per-frame budget (`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME = 8`), so a chunk full of fresh spawns never stalls a single frame. The queue preserves the budget across frames.
- **Reverse `Entity -> Id` map** (`replicated_to_id`). `RemovedComponents<ResourceNode>` only hands you the despawned `Entity`; the reverse map lets you find which local mirror to tear down.
- **First-run catch-up scan.** The `Added<T>` filter compares against the system's `last_run` tick, which misses entities that arrived during early-returning `client_id == None` frames before connect completed. On the first real run, scan the full query once to seed the queue and reverse map, then never again.
- **Value-delta gating**, not `Ref::is_changed()` (see the compare-and-write section).

### The ResourceNodeDepleted disambiguator

A Lightyear entity despawn alone cannot distinguish "node depleted (play the death effect)" from "node left my AoI ring (silent despawn)". The server sends `ServerMessage::ResourceNodeDepleted` on the reliable channel as the disambiguator. The client pairs it with the matching Lightyear despawn via `pending_depletion_check` + `DEPLETION_GRACE_FRAMES = 3` (`src/app/systems/items/resource_nodes.rs`). This is the one place a reliable message complements replication rather than replacing it: it signals intent, it is not patching a dropped diff. Do not generalize it into a state broadcast.

## Add a replicated entity (recipe)

The full step-by-step is [docs/playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md). The shape:

1. Authoritative state on `GameServer`: a `DirtyTrackedMap` (for post-spawn-mutating entities) or a plain `HashMap` (only if mutation is rare and you accept a full walk).
2. An `*_ecs.rs` module: one identity component + one component per mutable-cadence field. Derive `Component + Clone + PartialEq + Serialize + Deserialize`.
3. Mandatory-entry helpers in `src/server/queries.rs` (insert/remove/_mut) that mark the dirty set, plus a `drain_*_sync`. All mutation sites route through these.
4. A `sync_*` exclusive system in `src/net/host/mirror.rs` with the compare-and-write invariant; chain it into the `mirror_systems` tuple in `run_host` so it inherits the `server_tick_advanced` gate.
5. Spawn via `attach_room_gated_replication` (static) or `attach_player_replication` (player). Never a bare `Replicate`.
6. Register every component in `src/net/channels.rs` (`register_component::<T>()`); both server and client install the same plugin.
7. Consume on the client with `Query<&YourComponent>` in an event-driven reconciler, never via a `ServerMessage`.
8. Add `replication-trace` coverage (below).

## replication-trace verification

The `replication-trace` Cargo feature (default off) adds `server: <Component> MUTATE` logs inline in `src/net/host/mirror.rs` and `client: <Component> RECV` logs in `src/app/systems/replication_trace.rs`, both at `target: "replication_trace"`. Add both for any new post-spawn-mutating replicated component.

Run:

```
RUST_LOG=replication_trace=info cargo run --features replication-trace -- client
```

Reproduce the gameplay action and confirm each `MUTATE` pairs with a `RECV` within a few ms.

Diagnosis:

- `MUTATE` with no matching `RECV` = a replication failure. Most likely a missing `ReplicationGroup::new_from_entity()` at the spawn site (a bare `Replicate` bypassing the helpers), or a new Lightyear bug.
- `MUTATE` and `RECV` both firing but the UI still stale = a consumer bug. Look at the `Query<&Component>` reader, not the replication path.

## Reliability and what is allowed on the wire

The replication protocol is eventually consistent with bounded convergence under loss. We do not snapshot, poll, or manually retry. The protocol recovers on its own:

- **Room join (chunk crossing or fresh connect):** `AddSender` flips every entity in the room to `VisibilityState::Gained`; the next replication tick re-runs each through the spawn path, shipping a fresh entity spawn carrying ALL currently-replicated components (not a diff). The new observer is caught up. After that tick visibility is `Visible` and updates are diffs again.
- **Dropped packets:** `SinceLastAck` rebuilds each group's diff every tick as "everything changed from `ack_tick` to now" and re-sends. The diff is self-contained per tick; several lost packets in a row still converge once one arrives. NACKs reset the group's `send_tick` to re-ship the unacked window without waiting for ack timeout.

**Never re-introduce a periodic full-state broadcast.** The original `WorldSnapshot` wire was deleted during the migration and is genuinely gone from `src/`. Replacing it because "replication is hard" is the wrong move; fix the replication path instead (CLAUDE.md replicated-state rule 5). The only sanctioned periodic non-entity messages are presence (`PlayerList`, ~1 Hz, must include out-of-AoI players so it cannot ride the chunk-gated path), clocks (`WorldTime`, ~60 s), and diagnostics (`PerfStats`, ~1 Hz). If a new periodic message carries per-entity world state the client renders, it belongs in replication, not a message.

Note on the world map: the hold-M map's biome terrain is NOT on the wire and NOT rastered server-side at runtime; it is a pure function of the seed computed client-side (`src/protocol/world_map.rs`). The request/reply (`ClientMessage::RequestWorldMap` -> `ServerMessage::WorldMapMarkers`) ships only per-account markers. `src/server/world_map.rs` holds only per-account marker state and the request/command handlers; it contains no terrain-raster code (the terrain is computed purely client-side from the seed). See [docs/networking.md](networking.md) for the message-level detail.

## Related docs

- [docs/networking.md](networking.md) - transport, the three channels, full `ClientMessage`/`ServerMessage` inventory, auth/version handshake, admin socket.
- [docs/server-authority.md](server-authority.md) - `GameServer`, `receive`/`tick`, `ServerEnvelope`/`DeliveryTarget`, where each server concern lives.
- [docs/chunks-and-aoi.md](chunks-and-aoi.md) - chunk grid, AoI ring/tier math, node regrow, the room-subscription system in depth.
- [docs/playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md) - the step-by-step recipe.
- [docs/profiling.md](profiling.md) - the per-frame O(live-entities) traps the event-driven reconciler and the tick gate avoid.
