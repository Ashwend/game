---
title: "Playbook: add a networked entity"
owns: The end-to-end procedure for introducing a new per-entity authoritative state type that the client renders or queries.
when_to_read: When introducing any new per-entity authoritative state the client renders or queries (a new world prop, structure, or mobile entity that peers must see), or when wiring a new replicated component onto an existing mirror entity.
sources:
  - src/server/dirty_tracked_map.rs - DirtyTrackedMap (mutation-marks-dirty map)
  - src/server/queries.rs - GameServer mandatory-entry helpers (insert_/remove_/_state_mut/drain_*_sync)
  - src/server/player_ecs.rs - identity-vs-mutable component split (worked example)
  - src/net/host/mirror.rs - sync_* exclusive mirror systems
  - src/net/host/rooms.rs - attach_room_gated_replication / attach_player_replication / rebind_player_owner_if_changed
  - src/net/host.rs - mirror_systems tuple + server_tick_advanced gate
  - src/net/channels.rs - register_component registry + LIGHTYEAR_PROTOCOL_ID
  - src/app/systems/items/resource_nodes.rs - canonical event-driven client reconciler
  - src/app/systems/replication_trace.rs - client RECV trace logs
related:
  - docs/replication.md - the architecture and the WHY behind every step here
  - docs/networking.md - channels, handshake, the ClientMessage/ServerMessage wire (when you need a control message instead of replicated state)
  - docs/chunks-and-aoi.md - chunk anchoring and the room-subscription AoI ring this entity rides
  - docs/server-authority.md - GameServer state, receive/tick envelope contract
  - docs/profiling.md - the per-frame-over-N-entities cost this playbook's client step avoids
---

# Playbook: add a networked entity

> When to read this: you are adding a new per-entity authoritative state type the client must see (a prop, structure, or mobile entity), or a new replicated component on an existing mirror entity. Source of truth: `src/net/host/mirror.rs`, `src/net/host/rooms.rs`, `src/server/queries.rs`, `src/net/channels.rs`. Canonical invariants (per-entity ReplicationGroup, no periodic full-state broadcast, event-driven reconcilers) live in CLAUDE.md; this is the procedure, [docs/replication.md](../replication.md) is the why.

This is a clean checklist. Do not invent a new `ServerMessage` snapshot variant for per-entity state, that is the deleted-`WorldSnapshot` anti-pattern CLAUDE.md forbids; ship the state through Lightyear per-component replication using the steps below. Every claim here is pinned to a worked example already in the tree (resource nodes, players, deployables, loot bags).

## The shape you are building

Each networked entity is two halves kept in sync once per server tick:

1. **Authoritative state**: a map on `GameServer` (a `DirtyTrackedMap` if it mutates post-spawn, see step 1). This is the source of truth; the host's exclusive mirror system reads it.
2. **ECS mirror entity**: a Bevy entity carrying one identity component plus one component per mutable field. Lightyear replicates these components, room-gated to each client's AoI chunk ring. The client never sees the `GameServer` map; it sees the replicated components.

`GameServer` itself does zero networking. It mutates its own maps and returns `Vec<ServerEnvelope>`; the host layer in `src/net/host/` drives the mirror sync and the wire. See [docs/server-authority.md](../server-authority.md).

## Five-step server side

### Step 1: authoritative state on `GameServer`

Add a field to `GameServer` (`src/server.rs`) holding your map.

- If entries **mutate after spawn** (health changes, a flag toggles, a transform moves), use `DirtyTrackedMap<Id, T>` (`src/server/dirty_tracked_map.rs`), not a bare `HashMap`. `DirtyTrackedMap` makes "mutation marks dirty" a property of the type: there is no `DerefMut`, so you cannot get `&mut` to a value without the id being flagged for the next mirror sync. Skipping this is a silent stale-replication bug, the value changes server-side but the diff never ships, with no compile error and no failing test.
- A bare `HashMap` is only acceptable for short-lived / fully-recreated state. `loot_bags` gets away with a plain `HashMap` because bags are recreated rather than mutated in place; everything that mutates (`resource_nodes`, `dropped_items`, `deployed_entities`) is a `DirtyTrackedMap`. Check the existing field before copying a pattern.

`DirtyTrackedMap` API you will use: `insert` / `remove` / `get_mut` (auto-mark dirty), `mark_dirty` (flag without handing out `&mut`), `for_each_mut_then_mark` (the per-tick escape hatch for ticks that mutate server-only fields every tick and must mark only the entries whose *replicated* field actually flipped, used by the furnace / torch / dropped-item-physics ticks), `drain_sync` (drain `(dirty, removed)` once per tick), `seed_all_dirty` (world-load: flag every live id so the first sync spawns all mirrors), `requeue_dirty` (re-flag for a later pass, the spawn-budget overflow path).

### Step 2: the `*_ecs.rs` component split

Add a `src/server/<entity>_ecs.rs` module defining the mirror components. Split by cadence:

- **One identity component**, immutable after spawn, carrying the wire-stable id and any never-changing fields. Example: `ResourceNode { id, definition_id, ..., dead }` (`src/server/resource_node_ecs.rs` - `ResourceNode`); `Player { client_id, account_id }` (`src/server/player_ecs.rs` - `Player`).
- **One component per mutable field group**, each changing at its own cadence. Lightyear ships *whole-component values*, not field diffs, so bundling a slow field with a fast one re-ships the slow field at the fast field's rate. The player split is the canonical lesson: pose ticks at 20 Hz while moving, so `PlayerPose` is its own tiny component (`src/server/player_ecs.rs` - `PlayerPose`); profile/health/chat-bubble/held-item/action each sit apart so they ship one diff per real change, not per movement tick. The old mega-component re-shipped the full inventory at 20 Hz because the per-tick input ack made the bundled value compare unequal.

The deployable family is the fullest example of the split: `Deployable` (identity, `kind` immutable) plus `DeployableTransform`, `DeployableHealth`, `DeployableActive`, `DeployableLabel`, `DeployableStability`, `DeployableAuth`, each its own component.

Every mirror component derives `Component, Clone, PartialEq, Serialize, Deserialize` (the `PartialEq` is what the mirror's compare-and-write uses to suppress no-op diffs). Provide a `spawn_*_entity(world, view, chunk) -> Entity` helper and an `Id -> Entity` index plus a `despawn_*_entity` (the `entity_index!` macro generates both: see `src/server/resource_node_ecs.rs:53` - `entity_index!` block).

**Identity fields cannot diff.** If an "immutable" identity field genuinely must change (e.g. `Deployable.kind` on a hammer upgrade), the mirror handles it by despawn + respawn of the mirror entity, not by mutating the immutable component. `sync_deployable_entities` compares `Deployable.kind`, despawns on mismatch, and the client sees a normal remove + add. Do not try to make identity components mutable.

**Owner-only fields** (state only the owning client should receive, e.g. a private inventory) are gated per component in step 4, not by a separate message. Mark them in the split so step 4 wraps them.

### Step 3: the `sync_*` exclusive mirror system

Add a `sync_<entity>_entities(world: &mut World)` exclusive system to `src/net/host/mirror.rs` (these are exclusive because spawning and despawning need `&mut World`). Use the delta-driven shape from `sync_resource_node_entities` (`src/net/host/mirror.rs` - `sync_resource_node_entities`):

1. `drain_*_sync()` the `(dirty, removed)` id delta (ids only, no state clone yet).
2. Despawn the mirror entity for each removed id via your generated `despawn_*_entity`.
3. Classify dirty ids into already-mirrored (refresh in place) vs new (spawn) using only the cheap `Id -> Entity` index lookup, *before* cloning any state, so the expensive per-tick work is bounded to what actually changed.
4. Refresh existing mirror components in place. Gate the write on a real value delta (`if storage.0 != state.storage`); Bevy's server-side change detection only marks `Changed` when the value actually differs, which is what triggers Lightyear's per-component ship.
5. Spawn fresh mirror entities and attach replication (step 4).

**Spawn budget**: world-load-on-connect seeds *every* id dirty at once (~1800 resource nodes). Spawning them all in one tick is a multi-hundred-millisecond `&mut World` stall. `sync_resource_node_entities` caps fresh spawns at `MAX_RESOURCE_NODE_SPAWNS_PER_SYNC = 128` per pass and `requeue_*_sync`s the overflow, spreading the fill over a few ticks. Refreshes and despawns stay uncapped so live diffs are never delayed. Adopt this only if your entity type can arrive in large bursts; a low-count type can skip it.

**Full-walk exception**: players and loot bags are *not* delta-driven, they full-walk their map every sync (`sync_player_entities`, `sync_loot_bag_entities`). Players because pose mutates every tick anyway; loot bags because the settling-transform bulk path would need per-tick dirty marking. New world-prop types should be delta-driven like resource nodes unless you have the same reason.

### Step 4: attach replication via the room helpers

In the spawn arm of your sync system, call exactly one of these (`src/net/host/rooms.rs`):

- `attach_room_gated_replication(world, entity, chunk)` for a static world entity (resource node, dropped item, deployable, loot bag). It inserts `Replicate::to_clients(NetworkTarget::All) + NetworkVisibility + ReplicationGroup::new_from_entity()` and joins the entity to its chunk's Lightyear `Room`.
- `attach_player_replication(world, entity, chunk, owner_sender)` for a player-shaped entity with owner-only components. Same as above plus a `ComponentReplicationOverrides<T>` per owner-only component (`disable_all().enable_for(owner_sender)`) so peers never receive those wire bytes.

**Never bypass these with a bare `Replicate::to_clients(...)`.** Both helpers attach `ReplicationGroup::new_from_entity()`; without it Lightyear puts the entity in `ReplicationGroupId(0)` alongside every other group-less entity, and the shared per-group ack tick can advance past a slowly-changing entity's local `Changed` mark, silently dropping the diff (upstream Lightyear bug [#740](https://github.com/cBournhonesque/lightyear/issues/740), found on 0.26.4 and still guarded after the 0.28 upgrade). A unit test in `src/net/host/rooms.rs` asserts the per-entity group id is `ReplicationGroupId(entity.to_bits())` and not `ReplicationGroupId(0)`. `NetworkTarget::All` (not `None`) plus `NetworkVisibility` is also load-bearing: see the long comment on `attach_room_gated_replication` for why `None + room` shipped the spawn but dropped subsequent updates.

If you add a **new owner-only component** to a player-shaped entity, add its override in **both** `attach_player_replication` and `rebind_player_owner_if_changed` (`src/net/host/rooms.rs` - `rebind_player_owner_if_changed`). A reconnect that wakes a sleeping body keeps the same mirror entity but gets a *new* sender; `rebind_player_owner_if_changed` must re-point every override or the woken player's owner-only state never reaches them.

### Step 5: register the components

Add `app.register_component::<YourComponent>()` for every replicated component to `src/net/channels.rs` (`LightyearProtocolPlugin::build`, after the existing `register_component` calls). Both server and client install the same `LightyearProtocolPlugin`, so one registration covers both sides; the registries must match exactly or the wire bytes will not round-trip. Identity and every mutable component need a line.

If this is a wire-breaking change (new components shift the protocol), bump `PROTOCOL_VERSION` in `src/protocol.rs` (currently `39`). Leave `LIGHTYEAR_PROTOCOL_ID` (`src/net/channels.rs` - `LIGHTYEAR_PROTOCOL_ID`, a fixed constant independent of `PROTOCOL_VERSION`) alone unless the transport itself becomes incompatible.

## Mutation must go through the mandatory-entry helpers

Every site that mutates a `DirtyTrackedMap`-backed authoritative map **must** go through the `GameServer` helpers in `src/server/queries.rs` (`insert_resource_node` / `remove_resource_node` / `resource_node_state_mut`, and the dropped-item / deployable equivalents). Those helpers are the single entry point that keeps the dirty set accurate. A direct `HashMap` write that bypasses them leaves the id un-flagged and the mirror silently goes stale. (Players and loot bags are full walks, so they have no dirty set to keep and do not need this.)

Per-tick ticks that mutate server-only fields every tick (furnace burn countdown, torch timer, dropped-item physics pose) use `for_each_mut_then_mark` and call `mark_dirty` only when a *replicated* field actually flips, so idle entities stay out of the delta.

## Chain the sync system under the tick gate

Add your `sync_*` system to the `mirror_systems` tuple in `run_host` (`src/net/host.rs` - `mirror_systems`). That tuple is `.chain().run_if(server_tick_advanced)`. The host loop calls `app.update()` hundreds to a thousand times per second, but the mirror and room systems only have work when a 20 Hz fixed tick crossed (`ServerTickPulse.advanced`). Adding your system anywhere else, ungated, runs it every ~1 ms and burns CPU. Everything before `tick_authoritative_server` (command/admin drains, message receive, disconnect handling) stays ungated and runs every update; the mirror systems run after it, gated.

## Client side: an event-driven reconciler

The client mirrors the replicated entities into local-only visuals. **Do not poll the full replicated query every frame.** Iterating ~1800 replicated nodes per frame just to detect "nothing changed" costs 1 to 4 ms/frame for the no-op case, the canonical frame-pacing bug (see [docs/profiling.md](../profiling.md)). React to `Added<T>` and `RemovedComponents<T>` instead. The canonical implementation is `apply_resource_nodes_system` in `src/app/systems/items/resource_nodes.rs`; copy its structure:

- **Pending-spawn `VecDeque`** so the per-frame spawn budget (`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME = 8`, distinct from the server-side per-sync budget) survives across frames. Crossing a chunk boundary can pull tens of entities into view at once; spawning them all in one frame is a command-buffer / GPU-upload hitch.
- **Reverse `Entity -> Id` map** so `RemovedComponents<T>` can find which local mirror to despawn without scanning.
- **First-run catch-up scan.** The `Added<T>` filter compares against the system's `last_run` tick, which keeps advancing every frame the system early-returns (while `client_id` is `None` on the main menu). By the time you connect and the replicated entities arrive, `Added` won't fire for them. On the first real run (`!applied_first_snapshot`), iterate the full query once to seed the queue and the reverse map; after that, `Added` / `Removed` handle everything.
- **Per-frame spawn budget** drained from the `VecDeque`; despawns and transform updates stay uncapped.

**Never gate work behind `Ref::is_changed()` for Lightyear-touched components.** Lightyear's receive path uses `insert_by_ids`, which bumps the change tick *every replication tick* even when the value is identical. Gate on a real before-to-after value delta instead (the `DeployableLabel` / `Stability` / `Auth`, `PlayerHeldItem`, `PlayerAction`, and `DroppedItem`-stack handlers all do this). Bevy's server-side change detection in step 3 *is* reliable, so the server compare-and-write is fine; only the client receive side has this trap.

## Verify with replication-trace before merging

Any new component that mutates post-spawn needs `replication-trace` coverage so you can prove the diff ships:

1. Add a server `MUTATE` log inline in your `sync_*` system in `mirror.rs`, gated `#[cfg(feature = "replication-trace")]`, logging the before-to-after value at `target: "replication_trace"`.
2. Add a client `RECV` log in `src/app/systems/replication_trace.rs`, also gated on a real value delta (not `Ref::is_changed()`).

Then build and run:

```
cargo run --features replication-trace
RUST_LOG=replication_trace=info
```

Mutate the state in-game and read the log:

- `MUTATE` followed by a matching `RECV` = replication is working.
- `MUTATE` with **no** `RECV` = a replication failure, almost always a missing `ReplicationGroup` at the spawn site (step 4), or a missing `register_component` (step 5), or a brand-new Lightyear bug.
- `RECV` but the UI is still stale = a consumer bug; look at the `Query<&Component>` reader, not the replication path.

## What not to do

- Do not add a periodic full-state `ServerMessage` broadcast. The original `WorldSnapshot` wire was deleted; per-entity state ships through replication. Lightyear's `SinceLastAck` re-ships unacked windows every tick and `AddSender` drives fresh-room catch-up, so a new client gets caught up without a broadcast. The only non-entity periodic messages allowed are presence (`PlayerList`), clocks (`WorldTime`), and diagnostics (`PerfStats`).
- Do not bypass the room helpers (loses the per-entity `ReplicationGroup`, reintroduces bug #740).
- Do not poll the full replicated query on the client, and do not gate client work behind `Ref::is_changed()`.
- Do not bypass the `queries.rs` mandatory-entry helpers when mutating a `DirtyTrackedMap`.
- If you need a *control message* (a request/response or a one-shot event, not per-entity state), that is a `ClientMessage` / `ServerMessage` variant on a channel, see [docs/networking.md](../networking.md), not this playbook.

## Related docs

- [docs/replication.md](../replication.md) - the architecture and the reasoning behind every step here; read it if any step is unclear.
- [docs/networking.md](../networking.md) - channels, the version handshake, and the `ClientMessage` / `ServerMessage` wire for when you need a control message instead.
- [docs/chunks-and-aoi.md](../chunks-and-aoi.md) - chunk anchoring and the room-subscription AoI ring this entity rides.
- [docs/server-authority.md](../server-authority.md) - `GameServer` state, the `receive` / `tick` envelope contract, and where each server concern lives.
- [docs/profiling.md](../profiling.md) - the per-frame-over-N-entities cost the client reconciler step exists to avoid.
- [docs/playbooks/add-content.md](add-content.md) - adding a tool / ore / recipe / deployable (which often pairs with this playbook when the content is a new networked entity).
