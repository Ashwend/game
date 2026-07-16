---
title: Runtime chunks and Lightyear room-based AoI
owns: The server-side ChunkManager runtime (entity-to-chunk membership, node regrow, density-falloff budget) and the chunk-room AoI subscription system that gates per-component replication.
when_to_read: Before touching chunk membership, AoI ring math, node regrow, or the room-subscription system.
sources:
  - src/server/chunk_manager.rs - ChunkManager, ActiveChunkState, RING_BUDGET, view_tier_radius, apply_ring_budget
  - src/server/chunk_manager/aoi.rs - visible_chunks, retained_chunks, chunks_within
  - src/server/chunk_manager/membership.rs - anchor_chunk_for, track_*/untrack_*/update_*_chunk
  - src/server/chunk_manager/regrow.rs - handle_node_depleted, tick, place_fresh_node
  - src/server/chunk_manager/save.rs - ChunkManagerSave, PendingRegrowSave
  - src/net/host/rooms.rs - attach_room_gated_replication, update_client_room_subscriptions, ensure_chunk_room_*
  - src/net/host/mirror.rs - sync_projectile_entities
  - src/server/meteor_shower.rs - spawn_meteor_shower_crater_nodes, cleanup_expired_meteors
  - src/server/queries.rs - client_aoi_key, visible_chunks_for_client, retained_chunks_for_client
related:
  - docs/replication.md - the attach helpers, bug #740, the host mirror this AoI gates
  - docs/worlds-and-saves.md - the pure generation pipeline that feeds the initial spawn list (including the ruin footprints), and ChunkManagerSave persistence
  - docs/meteor-shower.md - the meteor event that splices crater nodes into the live map at runtime
  - docs/networking.md - SetViewRadius wire message, channels, ServerTickPulse gate
  - docs/profiling.md - the ~1800-visible-entity floor that AoI scale produces
---

# Runtime chunks and Lightyear room-based AoI

> When to read this: before touching chunk membership, AoI ring math, node regrow, or the chunk-room subscription system. Source of truth: `src/server/chunk_manager/` (runtime) and `src/net/host/rooms.rs` (replication gate). Canonical invariants (replicated-state rules, gameplay-never-pauses) live in CLAUDE.md.

`ChunkManager` is the server-authoritative runtime owner of the chunk grid: which chunk every networked entity is anchored to, when depleted nodes respawn, and which chunk rooms each client subscribes to. The pure, deterministic generation that produces the initial node spawn list lives separately under `src/world/chunk/` and is documented in [worlds-and-saves.md](worlds-and-saves.md); this doc covers the live mutation and the AoI replication gate.

## AoI is Lightyear room-based replication, not a snapshot builder

Read this first. There is **no per-player snapshot system**. The old `WorldSnapshot` periodic full-state wire was deleted during the Lightyear migration and must not be reintroduced (CLAUDE.md replicated-state rule 5). If you grep the codebase the word "snapshot" still appears in a handful of stale docstrings (`src/server/chunk_manager.rs` module header, `aoi.rs` method comments, the `SetViewRadius` protocol comment in `src/protocol/messages.rs`); those refer to nothing that exists. Treat that terminology as drift, not a system to find.

How AoI actually works:

- Each `ChunkCoord` lazily owns one Lightyear `Room` entity, stored in `ChunkRoomMap.by_coord` (`src/net/host.rs - ChunkRoomMap`). Allocated on first use by `ensure_chunk_room_world` / `ensure_chunk_room_commands` in `src/net/host/rooms.rs`.
- Every chunk-anchored entity (resource node, dropped item, deployable, player, loot bag) **joins its chunk's room** at spawn via `attach_room_gated_replication` or `attach_player_replication` (`src/net/host/rooms.rs`), which trigger a `RoomEvent::AddEntity`.
- Each client's `ReplicationSender` joins the rooms covering its AoI ring. `update_client_room_subscriptions` (`src/net/host/rooms.rs:243 - update_client_room_subscriptions`) diffs the client's add/keep chunk sets each tick and emits `RoomEvent::AddSender` / `RemoveSender`.
- Lightyear then delta-ships per-component diffs only to senders that share a room with an entity, and auto-despawns on the client when the rooms diverge. `NetworkVisibility` on each entity narrows the `Replicate::to_clients(NetworkTarget::All)` target down to the in-room senders.

So "what a player sees" is the union of entities in the rooms that player's sender is subscribed to. The `ChunkManager` decides the ring of chunk coords; Lightyear does the actual per-component shipping. The spawn-side mechanics (`ReplicationGroup::new_from_entity()`, bug #740, owner-only player component gating) are owned by [replication.md](replication.md); this doc owns the membership and ring math that feed them.

## ActiveChunkState: membership for every entity type

`ChunkManager` (`src/server/chunk_manager.rs - ChunkManager`) holds a `HashMap<ChunkCoord, ActiveChunkState>` (`grids`) plus a reverse index per entity type so any membership move is O(1):

| Entity type | Per-chunk set (in `ActiveChunkState`) | Reverse index (`ChunkManager`) | Re-anchored after spawn? |
| --- | --- | --- | --- |
| Resource nodes | `live_by_kind: HashMap<NodeKind, HashSet<ResourceNodeId>>` | `node_chunks: HashMap<ResourceNodeId, (ChunkCoord, NodeKind)>` | No (depletion/regrow only) |
| Dropped items | `dropped_items: HashSet<DroppedItemId>` | `dropped_item_chunks` | **Yes, per physics step** |
| Players | `players: HashSet<ClientId>` | `player_chunks` | Yes, on accepted movement |
| Deployables | `deployed_entities: HashSet<DeployedEntityId>` | `deployed_entity_chunks` | No (anchored once at place) |
| Loot bags | (id-only, no per-chunk set) | `loot_bag_chunks` | No (anchored once at spawn) |
| Projectiles | (none, never enters `ChunkManager`) | (none) | **Yes, room-only, in the mirror sync** |

Entities themselves live in their owning `GameServer` collections (`resource_nodes`, `dropped_items`, `clients`, etc.); `ChunkManager` stores only ids. `live_by_kind` is grouped by kind so the regrow scheduler can answer "is this kind at cap?" in O(1) (`ActiveChunkState::live_count`).

Projectiles are the deliberate exception: an in-flight arrow is never tracked by `ChunkManager` (no per-chunk set, no reverse index). Its anchor is a pure `ChunkCoord::from_world` of the current position, recomputed by the mirror sync (`sync_projectile_entities`, `src/net/host/mirror.rs`) for every dirty projectile each server tick and cached on the ECS-side `ProjectileChunk` marker; when it changes, the sync calls `move_entity_between_rooms` directly so observing clients gain/lose visibility at the boundary. Projectiles are few and short-lived, so recomputing per sync is cheaper than membership bookkeeping. The projectile component split lives in [replication.md](replication.md).

Membership mutators live in `src/server/chunk_manager/membership.rs` and follow a uniform `track_` / `untrack_` / `update_*_chunk` shape (`track_player`, `track_deployed_entity`, `track_loot_bag`, `track_resource_node`, etc.). Call sites of note:

- Players: `track_player` on connect (`src/server/connection.rs`), `untrack_player` on disconnect, `update_player_chunk` on each accepted `PlayerMovement` (`src/server/dispatch.rs:49`) and on respawn / sleeping-bag move (`combat.rs`, `sleeping_bag.rs`).
- Deployables: `track_deployed_entity` from building / deployable / door place paths, `untrack_deployed_entity` on destroy.
- Admin-spawned nodes: `track_resource_node` exists specifically so a `/spawn` node added to `resource_nodes` directly also enters the per-chunk membership set, otherwise it would be invisible to AoI (membership is the source of truth, not the entity map).

### anchor_chunk_for clamps out-of-bounds entities

Every "where does this entity anchor?" call routes through `anchor_chunk_for` (`src/server/chunk_manager/membership.rs:18 - anchor_chunk_for`). It converts a world position to a `ChunkCoord` via `ChunkCoord::from_world`, and if that coord is outside the loaded grid it **clamps to the nearest loaded ring** rather than returning a coord with no `ActiveChunkState`. The clamp:

```
let half = self.dims.dims as i32 / 2;
let max = half - (1 - self.dims.dims as i32 % 2);
ChunkCoord::new(raw.x.clamp(-half, max), raw.z.clamp(-half, max))
```

This is a safety net for a dropped item physics-launched past the perimeter wall: it stays findable in some legal chunk instead of silently vanishing from AoI. Do not bypass it; a bare `from_world` can produce a coord the grid has no room for.

### Per-physics-step dropped-item re-anchoring

Dropped items are the only `ChunkManager`-tracked entity that drifts across chunk boundaries under simulation (in-flight projectiles also drift, but re-anchor room-only in the mirror sync; see the exception note above). After the physics step, `GameServer::tick` (`src/server/tick.rs`) re-anchors exactly the items the step actually moved, which are precisely the ids the dropped-item store flagged dirty:

```
let moved = self.dropped_items.dirty_ids().collect();
for id in moved { self.chunk_manager.update_dropped_item_chunk(id, position); }
```

`update_dropped_item_chunk` (`membership.rs`) is one HashMap lookup + comparison when the chunk hasn't changed, so calling it once per moved item per step is cheap. At-rest items are not dirty and skip the walk entirely. When the chunk does change, the matching mirror-sync system calls `move_entity_between_rooms` so the entity leaves the old chunk room and joins the new one (`src/net/host/mirror.rs`, `src/net/host/rooms.rs:70 - move_entity_between_rooms`). Players move rooms the same way on `update_player_chunk`.

## The AoI ring: two-threshold spatial hysteresis

Ring math is centralized in `src/server/chunk_manager/aoi.rs` so every entity type flows through one visibility decision. The server resolves the client's `ViewRadiusTier` to a Chebyshev grid radius via `view_tier_radius` (`src/server/chunk_manager.rs:122 - view_tier_radius`):

| `ViewRadiusTier` | View radius (chunks) |
| --- | --- |
| `Low` | 1 |
| `Medium` (default) | 2 |
| `High` | 3 |

`ViewRadiusTier::Medium` is the default (`src/protocol/messages.rs - ViewRadiusTier`, `#[default]` on `Medium`). The client sends `ClientMessage::SetViewRadius { tier }` on connect and whenever the player changes the setting; `src/server/dispatch.rs:123` writes it onto `client.view_tier`. That tier is the only knob the player has over replication volume.

Two radii drive subscriptions, and the gap between them is the hysteresis that stops boundary thrash:

- **Add radius** = `view_tier_radius(tier) + LOAD_BUFFER_RINGS` (`LOAD_BUFFER_RINGS = 1`). Computed by `visible_chunks` (`aoi.rs:23 - visible_chunks`). A chunk is **subscribed** as soon as it enters this radius. The extra buffer ring exists so the client's collider grid is already populated one full cell (64 m) away before the player crosses a boundary; without it, freshly-loaded tree/ore colliders placed as close as `EDGE_MARGIN_M` (0.5 m) from the boundary would overlap the player and the next prediction step would shove them upward (a visible vertical spasm). See the long comment on `LOAD_BUFFER_RINGS` in `chunk_manager.rs`.
- **Keep radius** = add radius `+ KEEP_MARGIN_RINGS` (`KEEP_MARGIN_RINGS = 2`). Computed by `retained_chunks` (`aoi.rs:31 - retained_chunks`), always a strict superset of `visible_chunks`. A subscribed chunk is **only unsubscribed** once it falls outside this wider radius.

Because add and keep differ by 2 rings, a player wobbling 1 chunk back and forth across a boundary never crosses both thresholds, so nothing thrashes load -> unload -> reload. This is deterministic (no timer); it costs only the extra fringe rings' replication while the player lingers near an edge.

Both radii are computed from the player's anchor chunk (`ChunkCoord::from_world` of the player position), not from a re-walk of every entity. `chunks_within` (`aoi.rs:43 - chunks_within`) walks the Chebyshev neighbourhood and keeps only coords that exist in `grids`.

### Wiring through GameServer and the subscription loop

`src/server/queries.rs` exposes the per-client wrappers the room system reads:

- `client_aoi_key` (`queries.rs:62`) returns `(anchor_chunk, view_tier)`, the cheap key that decides the subscription set. Because the loaded-chunk grid is fixed after world construction, an unchanged key means the add/keep sets are identical to last tick.
- `visible_chunks_for_client` (`queries.rs:72`) and `retained_chunks_for_client` (`queries.rs:86`) delegate to the `ChunkManager` ring methods using the client's current position and tier.

`update_client_room_subscriptions` (`src/net/host/rooms.rs:243`) runs the reconcile each server tick:

1. Retain only live clients in `ClientChunkSubs` and `ClientAoiAnchors`.
2. For each client, if its `client_aoi_key` is unchanged since the cached anchor, **short-circuit** (skip the grid scan and set diff entirely). This is the common idle-player path.
3. Otherwise diff `add_set` (visible) and `keep_set` (retained) against the cached `subscribed` set: `AddSender` for newly-entered chunks, `RemoveSender` for chunks that fell outside the keep radius.
4. Cache the new key.

A subtlety the code documents: a client id can be "in the world" with **no live sender** (a sleeping body, where the player logged out but the body remains so the id stays in `connected_client_ids`). When `entity_for_client` returns `None`, the loop drops that client's cached anchor and subscribed set so a later reconnect (which reuses the same id via `wake_sleeper` with a brand-new sender) re-subscribes from scratch. Skipping this leaves the woken player receiving nothing, not even its own entity.

This whole system, like the mirror-sync systems, runs only when a 20 Hz fixed tick crossed: it is chained under the `server_tick_advanced` (`ServerTickPulse`) run condition in `run_host` (`src/net/host.rs`), not every ~1 ms host-loop iteration. See [replication.md](replication.md) and [networking.md](networking.md) for the tick gate.

## Node regrow

When a node is depleted (gather/inventory/admin removal), `handle_node_depleted` (`src/server/chunk_manager/regrow.rs:26 - handle_node_depleted`) removes it from membership and schedules a jittered respawn:

- Delay window is **5 to 15 minutes**, expressed in ticks: `MIN_REGROW_TICKS = 5 * 60 * SERVER_TICK_RATE_HZ` and `MAX_REGROW_TICKS = 15 * 60 * SERVER_TICK_RATE_HZ`, where `SERVER_TICK_RATE_HZ = 20` (`src/protocol.rs - SERVER_TICK_RATE_HZ`). So 6000 to 18000 ticks.
- The per-event jitter is deterministic: `placement_counter` is stirred with `splitmix64(counter ^ now_tick ^ node_id)` so the same `(coord, kind, tick)` round-trips identically across save and load. No `std::rand`, no wall clock.
- Events live in a `BinaryHeap<RegrowEvent>` ordered as a min-heap on `fire_tick` (the `Ord` impl flips the comparison).

`ChunkManager::tick` (`regrow.rs:54 - tick`) is driven from `GameServer::tick` each server tick. It drains every event whose `fire_tick` has arrived and calls `place_fresh_node` (`regrow.rs:77`) for each. Placement is **capacity-gated**: it refuses if the chunk is already at `capacity[kind]` (the same per-chunk ceiling the generator used; see below), picks a fresh Poisson-disk position via the per-chunk generator (`candidate_positions`, salted so repeats get fresh points), and rejects any candidate within 1.2 m of a surviving node (`collides_with_existing`). Better to drop a respawn than jam a node into an occupied square. The result is spliced back into `GameServer::resource_nodes` and the mirror sync turns it into a replicated entity on the next `Update`.

Ruin footprints are a node-rejection input to both placement passes: `ChunkManager` recomputes the seed-pure footprint list (`ruin_footprints`, `src/world/ruins.rs`) on construction and on load (never persisted) and passes it into `candidate_positions`, the same rejection input `generate_world_spawns` consumes at initial generation, so a regrow can never drop a node inside a ruin the initial pass kept clear. The ruin pipeline itself (site scatter, prefab layouts, cache footprints) is documented in [worlds-and-saves.md](worlds-and-saves.md).

### Runtime and rare node sources

Two sources feed nodes into this system differently:

- **Meteorite** is a normal world-gen kind. It rides the raw ore channel (`channel_for`, `src/world/chunk/classification.rs`), has base capacity 1 and only in `RockyOutcrop`/`OreVein` chunks (`base_capacity`, same file), and is further gated in `chunk_kind_target` (`src/world/chunk/generator.rs:87`) by a centre-distance ring (`METEORITE_MIN_CENTER_DISTANCE_FRACTION`) plus an ore-channel floor, so most eligible chunks hold none. It depletes and regrows like any other kind, capped by that same ceiling.
- **Meteor shower crater nodes** are spliced into the live map at runtime by the meteor event, per meteor (`spawn_meteor_shower_crater_nodes`, `src/server/meteor_shower.rs`; the count scales with the meteor's size): each crater node enters membership via `track_resource_node` so it is AoI-visible like any node. A crater node mined by a player depletes through the normal gather path, so its regrow event is subject to the same capacity gate as any other kind. Any crater node still unmined when its meteor's window ends is force-despawned by `cleanup_expired_meteors` (same file), which removes it and calls `untrack_resource_node` with **no regrow scheduled** (event spawns, not world nodes). See [meteor-shower.md](meteor-shower.md).

### Capacity ceiling is shared with world-gen

`build_empty_grids` (`src/server/chunk_manager.rs - build_empty_grids`) computes each chunk's per-kind `capacity` from `chunk_kind_target(classification, channels, kind, center_dist_frac)`, the **same** formula `src/world/chunk/generator.rs` uses to place the initial nodes. The fourth argument (the chunk centre's distance from the world origin as a fraction of the playable radius) exists for the meteorite ring gate; both sites compute it via the shared `chunk_center_distance_fraction`, so the meteorite ceiling cannot drift between generation and regrow. This is load-bearing: if regrow used a different ceiling than world-gen, the world would silently over- or under-fill on respawn. Change one, change both. Classification is recomputed from the seed on every load (not persisted), so this stays in sync automatically. See [worlds-and-saves.md](worlds-and-saves.md) for the classification/generator pipeline and the consequences of editing `BIOME_BIAS` or thresholds.

## Density falloff: RING_BUDGET, applied once at creation

To make distant areas read as populated without paying the full per-node cost, outer-ring chunks keep only a fraction of their generated nodes. This is a **fixed spawn budget applied exactly once** at world creation, not a sliding cull, so players moving around never see neighbouring chunks fade in or out (only a brand-new world's outer rings are budgeted).

`apply_ring_budget` (`src/server/chunk_manager.rs:425 - apply_ring_budget`) runs inside `new_for_world` right after `generate_world_spawns`:

```
const RING_BUDGET: [f32; 5] = [1.0, 0.85, 0.65, 0.45, 0.30];
```

For each `(coord, kind)` group, the ring distance is the Chebyshev distance `coord.x.abs().max(coord.z.abs())`. The multiplier is `RING_BUDGET[ring]`, falling back to `OUTERMOST_RING_BUDGET` (the table's last entry, 0.30) for rings beyond index 4. It keeps the first `round(count * multiplier)` spawns per group, a **deterministic suffix-trim** of the spawn list rather than a re-run of the Poisson sampler with a scaled target. The spawns themselves are not persisted; only the surviving node ids end up in `node_chunks`.

## Persistence: ChunkManagerSave

`ChunkManagerSave` (`src/server/chunk_manager/save.rs - ChunkManagerSave`) is embedded in `WorldStateSave` and persists:

- `world_seed`, `dims`, `next_node_id`.
- `node_chunks: Vec<NodeChunkEntry>` (`node_id -> (coord, kind)`), so `from_save` rebuilds the per-chunk live sets by **replay** without re-running placement RNG.
- `pending_regrows: Vec<PendingRegrowSave>`, each stored as `ticks_from_now` (not an absolute fire tick). On load, `from_save` (`chunk_manager.rs` - `from_save`) re-clamps each to at least `MIN_REGROW_TICKS` so a save that sat idle for an hour does not dump a backlog of respawns at `t+0`. Classification and capacity are **not** persisted; `build_empty_grids` recomputes them from the seed.

`new_for_world` (fresh world) vs `from_save` (restore) is selected at world load by the lifecycle path based on whether both `resource_nodes` and `chunk_manager` are `Some` in the save. Full save-format detail (versioning, the golden-layout test, the field inventory) lives in [worlds-and-saves.md](worlds-and-saves.md).

## Gotchas

- **Membership is the AoI source of truth, not the entity map.** A node added straight to `GameServer::resource_nodes` without `track_resource_node` exists but is invisible to every client.
- **Every chunk-anchored spawn must go through `attach_room_gated_replication` (static) or `attach_player_replication` (players).** A bare `Replicate` lands in shared `ReplicationGroupId(0)` and silently drops post-spawn diffs (Lightyear bug #740, found on 0.26.4 and still guarded after the 0.28 upgrade). Owned by [replication.md](replication.md).
- **Do not re-anchor nodes, deployables, or loot bags per frame.** Only dropped items (physics drift), players (movement), and in-flight projectiles change chunks after spawn. Projectiles are the not-in-ChunkManager exception: `sync_projectile_entities` re-anchors them room-only from a pure `ChunkCoord::from_world` of the current position (see the membership exception note above). Nodes change membership only on depletion/regrow.
- **`RING_BUDGET` is creation-only.** Editing it changes only newly created worlds; existing saves keep their budgeted node ids.
- **Regrow capacity and world-gen capacity must agree** (`chunk_kind_target`). They share one function for exactly this reason.

## Related docs

- [docs/replication.md](replication.md) - the `attach_room_gated_replication` / `attach_player_replication` helpers, the per-entity `ReplicationGroup` fix for bug #740, the host mirror-sync systems, and the `ServerTickPulse` gate this AoI system shares.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - the deterministic `src/world/chunk/` generation pipeline (classification, Poisson-disk generator, noise), `chunk_kind_target`, the ruin pipeline behind `ruin_footprints`, and the full `WorldStateSave` / save-format story `ChunkManagerSave` embeds into.
- [docs/meteor-shower.md](meteor-shower.md) - the meteor event that splices meteorite crater nodes into the live map at runtime and force-despawns unmined ones at cleanup.
- [docs/networking.md](networking.md) - the `SetViewRadius` wire message, channel registration, and the `ServerTickPulse` / `server_tick_advanced` run condition.
- [docs/profiling.md](profiling.md) - the ~1800-visible-entity floor that AoI scale produces and the per-frame-iteration pitfalls when reconciling replicated entities client-side.
