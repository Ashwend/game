# Lightyear Replication Migration

Living plan for moving the game off the custom `WorldSnapshot` broadcast and onto
Lightyear's native component replication with per-chunk interest management.

---

## How to use this document

This file is the source of truth for the multi-phase Lightyear migration. Every
phase below is sized to fit a **single focused chat session**. It is designed so
a fresh chat — with no memory of prior conversations — can pick up any pending
phase and execute it.

### Maintenance rules

1. **Update this file every time anything meaningful changes.** A merged phase, a
   newly-discovered design constraint, a deferred open question that got
   answered, a file rename — anything that would invalidate a section. Stale
   plans waste fresh-session context.
2. **Destructive / breaking changes are fine.** This is early development. There
   are no users to migrate, no on-disk worlds we have to preserve, no public API
   contracts to keep. Don't add migration paths, don't preserve old shapes, don't
   write compat shims. Delete and rewrite freely.
3. **Bump the save file version when the save shape changes.** That's the only
   versioning we care about. See `src/save/format.rs` —
   `SAVE_FORMAT_VERSION: u32`. Bumping invalidates existing `.save` files, which
   is the intended behaviour.
4. **Don't add features beyond the phase's stated goal.** Each phase has a tight
   scope. If something tempting comes up mid-phase, jot it in the relevant
   "Open / deferred" section and move on.
5. **Each phase ends shippable.** Tests pass, lint clean, `./cli check` clean,
   committed. If a phase can't be cleanly committed in one session, the right
   move is to stop and report — not to leave the tree half-broken.

### Reading order for a fresh session

1. Read **Project context** below for the architecture snapshot.
2. Read the **Phase index** for status.
3. Jump to the phase you're going to execute. Each phase section is
   self-contained (key files, design decisions, open items).
4. Glance at **Cross-phase reference** for the entity-type table and glossary.

---

## Project context

A Rust/Bevy first-person prototype. Singleplayer and multiplayer both use the
Lightyear-backed `ClientSession::Network` path; singleplayer adds loopback host
startup, host admin assignment, and local save persistence.

### Versions

- Bevy `0.18.1`
- Lightyear `0.26.4`, features `["client", "server", "netcode", "udp"]`

### Current network architecture

- **Server**: runs as a separate thread with its own Bevy `App` (`MinimalPlugins`
  + `server::ServerPlugins` + `LightyearProtocolPlugin`). Authoritative game
  state lives in `GameServer` (a Bevy `Resource` inside that App). 20 Hz fixed
  tick.
- **Client**: lives in the main rendering/UI Bevy app since Phase 3.
  `client::ClientPlugins` + `LightyearProtocolPlugin` + `ClientNetworkPlugin`
  are installed in `src/app.rs` alongside `DefaultPlugins`. The connection
  lifecycle (spawn entity → `Connect` → handshake → `Connected` → Welcome →
  `Disconnect`) runs in `Update` driven by the systems registered by
  `ClientNetworkPlugin`. Gameplay code talks to it through the shared
  `ClientNetwork` resource (Arc-backed `outbox` / `inbox` / `status`).
  `ClientSession` is now a thin handle stored in
  `ClientRuntime::session`.
- **Loopback host (SP)**: still its own dedicated-server thread, bound to
  `127.0.0.1`. Spawned by `start_singleplayer` and held by `ClientSession`
  for shutdown.
- **State delivery (current, dual-path)**: two channels run in parallel
  today. (1) **Lightyear per-component replication** ships entity
  spawns, despawns, and per-component diffs only when something
  changes, room-gated to the player's AoI chunk ring. Verified
  end-to-end with the `replication-trace` Cargo feature — server-side
  mutations produce matching client-side `RECV` log lines within
  ~10–20 ms. (2) **`ServerMessage::Snapshot(WorldSnapshot)`** still
  ships every 100 ms (10 Hz; halved from the original 20 Hz in commit
  `2a3d752`) with the full AoI state. Every UI / pickup / HUD consumer
  reads from `ClientRuntime::snapshot` — that's the source of truth
  they trust today. **Phase 6** migrates each consumer to read from
  the replicated components directly, then deletes the snapshot wire.
- **Entity storage** (server): all authoritative state in `HashMap`s on
  `GameServer` (`clients`, `resource_nodes`, `dropped_items`,
  `deployed_entities`). Mirror ECS entities now exist alongside
  (Phases 1, 2) — kept in sync by exclusive systems in `net/host.rs`.
  Mirror entities carry `Replicate::to_clients(NetworkTarget::All) +
  NetworkVisibility` and join their chunk's `Room` (Phases 4/5). The
  `NetworkTarget::All` is load-bearing — see the Phase 6 section for
  why `NetworkTarget::None` was wrong.

### Lightyear 0.26 replication model (what we're moving toward)

Key API surface (from `cBournhonesque/lightyear` `network_visibility` example):

```rust
// Mark an entity as replicated
.spawn((Position(...), Replicate::to_clients(NetworkTarget::All), Rooms::single(room_id)));

// Per-client visibility via rooms
let player_room: RoomId = app.world_mut().resource_mut::<RoomAllocator>().allocate();
commands.entity(client_sender_entity).insert(Rooms::single(room_id));

// Or immediate per-client visibility overrides
commands.gain_visibility(entity, sender_entity);
commands.lose_visibility(entity, sender_entity);

// Connection lifecycle on the server
fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}
fn handle_connected(trigger: On<Add, Connected>, ..., mut commands: Commands) {
    // Now safe to start replication for this client
}

// Per-client targeted replication (used for player private state)
PredictionTarget::to_clients(NetworkTarget::Single(client_id));
InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id));
```

Rooms are the primitive for per-chunk interest management. One `RoomId` per
`ChunkCoord` (lazy-allocated). Entities join rooms via the `Rooms` component.
Clients subscribe by adding `Rooms` to their sender entity. Lightyear handles
delta encoding, auto-spawn/despawn on the client when room membership changes,
and ack-based retransmit.

Docs:
- [Lightyear book](https://cbournhonesque.github.io/lightyear/)
- [Lightyear 0.26 docs](https://docs.rs/lightyear/0.26.4/lightyear/)
- [`network_visibility` example](https://github.com/cBournhonesque/lightyear/tree/main/examples/network_visibility)

---

## Phase index

| #   | Title                                                          | Status      |
| --- | -------------------------------------------------------------- | ----------- |
| 0   | Tracing spans for server snapshot path                         | ✅ done     |
| 1   | ECS mirror for resource nodes                                  | ✅ done     |
| 2   | ECS mirror for dropped items, deployables, players             | ✅ done     |
| 3   | Merge Lightyear client into main Bevy app                      | ✅ done     |
| 4   | Wire chunk rooms for resource nodes                            | ✅ done     |
| 5   | Chunk rooms for dropped items, deployables, players            | ✅ done     |
| 6   | Migrate consumers off `WorldSnapshot`, delete it               | ⏳ pending  |
| 7   | Post-migration audit & cleanup                                 | ⏳ pending  |
| 1b  | Fold `resource_nodes` HashMap into entities (cleanup)          | ⏸ deferred  |

Future cleanups (analogous to 1b) will likely emerge for dropped items,
deployables, and players once Phase 5 lands. They aren't tracked yet to keep the
list honest.

---

## Phase 0 — Tracing spans for the server snapshot path

**Status**: ✅ done · commit `23af478`

### What landed

`tracing::info_span!` wrappers around the server hot paths so the existing
Chrome trace from `./cli profile` shows where snapshot CPU goes:

- `server_tick` (whole `GameServer::tick`)
- `chunk_manager_tick` (regrow scheduling)
- `snapshot_broadcast` (the per-client snapshot loop)
- `snapshot_inner` (the AoI-filtered build), split into sub-spans:
  - `snapshot_players`
  - `snapshot_dropped_items`
  - `snapshot_resource_nodes`
  - `snapshot_deployables`
- `host_fixed_tick` (the host App's update loop)
- `route_envelopes` (the message routing pass)

Files touched: [src/server.rs](src/server.rs),
[src/server/connection.rs](src/server/connection.rs),
[src/net/host.rs](src/net/host.rs).

### Why this came first

Every later phase wants to compare bandwidth/CPU before and after. Adding
spans before any restructuring gives us a baseline that's directly comparable
across commits. Cheap (≈30 min), no behaviour change, ships independently.

---

## Phase 1 — ECS mirror for resource nodes

**Status**: ✅ done · commit `f47c2cd`

### What landed

A parallel ECS representation of `GameServer::resource_nodes`, kept in sync
each Update by an exclusive system. The `HashMap` stays authoritative; Phase 4
attaches `Replicate` markers to the mirrored entities without needing to flip
ownership first.

New module: [src/server/resource_node_ecs.rs](src/server/resource_node_ecs.rs).

Components:

- `ResourceNode { id, definition_id, position, yaw }` — identity, immutable
  post-spawn.
- `ResourceNodeStorage(Vec<ItemStack>)` — mutable; decremented by gather.
- `ResourceNodeChunk(ChunkCoord)` — anchor for room subscription.

Resource:

- `ResourceNodeIndex` — `HashMap<ResourceNodeId, Entity>` for O(1) gameplay-side
  lookup.

System (in [src/net/host.rs](src/net/host.rs)):

- `sync_resource_node_entities(world: &mut World)` — reconciles
  `GameServer.resource_nodes` (HashMap) into ECS entities every Update.
  Despawn ids that left the live map, spawn fresh entities for new ids,
  refresh storage in place when the inner `Vec` actually differs.

GameServer accessors added: `resource_nodes_iter`, `has_resource_node`,
`resource_node_chunk`. ChunkManager gained `node_chunk` reverse lookup.

### Design decisions

- **Mirror over clean flip.** The "clean flip" Option A would have removed the
  HashMap entirely. That's ~45 call sites of cascading `&mut World` parameters
  and a near-rewrite of the test surface (`src/server/tests/resource_nodes.rs`
  alone has ~15 sites directly manipulating `server.resource_nodes`). It
  doesn't fit a single session reliably. The mirror gets us the same Phase 4
  unblock with O(N) less code change and N+1 fewer ways to break tests.
- **Split components.** `ResourceNode` (identity/pose) is separate from
  `ResourceNodeStorage` (mutable). Lightyear's per-component delta replication
  ships only the changed component, so a gather impulse will be 1 storage diff
  instead of a re-send of the whole entity.
- **Change-detection equality guards.** The mirror only writes
  `ResourceNodeStorage` when the value differs from the entity's current
  state. This keeps Bevy's `Changed<T>` ticks from firing every Update when
  nothing actually changed — which is what Lightyear keys off for "send a
  diff" once Phase 4 lands.

### Open / deferred

- HashMap removal lives in **Phase 1b** (post Phase 4). The mirror approach
  means the cleanup is independent and small.

---

## Phase 2 — ECS mirror for dropped items, deployables, players

**Status**: ✅ done · commit `c2d8104`

### What landed

Same mirror pattern as Phase 1, extended to the remaining authoritative entity
types. Three new modules, three new exclusive sync systems.

[src/server/dropped_item_ecs.rs](src/server/dropped_item_ecs.rs):

- `DroppedItem { id, stack }`
- `DroppedItemTransform { position, yaw, rotation }` — split out because the
  physics body writes it every tick the body is settling.
- `DroppedItemChunk(ChunkCoord)`
- `DroppedItemIndex`

[src/server/deployable_ecs.rs](src/server/deployable_ecs.rs):

- `Deployable { id, item_id, kind, max_health }` — identity / immutable.
- `DeployableTransform { position, yaw }`
- `DeployableHealth(u32)`
- `DeployableActive(bool)` — furnace on/off, split because it toggles
  independently of the rest of the state.
- `DeployableChunk(ChunkCoord)`
- `DeployableIndex`

[src/server/player_ecs.rs](src/server/player_ecs.rs):

- `Player { client_id, steam_id }` — identity.
- `PlayerPublic { name, position, velocity, yaw, pitch, health, grounded, is_admin, chat_bubble }`
  — replicated to all in same room (Phase 5).
- `PlayerPrivate { inventory, crafting, open_furnace, last_processed_input }`
  — replicated only to the owning client (Phase 5).
- `PlayerChunk(ChunkCoord)`
- `PlayerIndex`

Sync systems in [src/net/host.rs](src/net/host.rs):

- `sync_dropped_item_entities`
- `sync_deployable_entities`
- `sync_player_entities`

GameServer iterators added: `dropped_items_iter`, `deployables_iter`,
`players_iter`. Chunk lookups: `dropped_item_chunk`, `deployable_chunk`,
`player_chunk`. ChunkManager gained `dropped_item_chunk` and
`deployed_entity_chunk` reverse lookups.

### Design decisions

- **Players split into public / private now.** Phase 5 needs this split for
  Lightyear's per-target replication (`NetworkTarget::All` for public,
  `Single(client_id)` for private). Doing the split during the mirror means
  Phase 5 is just adding `Replicate` markers, not another data refactor.
- **`DeployableActive` is its own component.** Furnaces toggle smelt state
  independently of position/health. Isolating it means a furnace turning on
  ships one boolean delta in Phase 5, not the whole deployable.
- **`DroppedItemTransform` is split from `DroppedItem`.** Physics writes
  transform every tick a body is settling; stack changes only on merge.
  Splitting matches the change frequency.

### Open / deferred

- Cleanup of the underlying HashMaps (`dropped_items`, `deployed_entities`,
  `clients`) is the analogue of Phase 1b. Deferred to after Phase 5 lands.

---

## Phase 3 — Merge Lightyear client into main Bevy app

**Status**: ✅ done

### What landed

The Lightyear client now lives in the main Bevy `App`. The dedicated
`lightyear-game-client` thread, MPSC command channel, and the
Welcome-blocking-on-startup are all gone. The connection lifecycle runs
in the main `Update` schedule and gameplay code talks to it through a
shared `ClientNetwork` resource.

New module surface in [src/net/client.rs](src/net/client.rs):

- `ClientNetwork` — `Resource + Clone` (it's an `Arc<Inner>`). Holds the
  `outbox`, `inbox`, `status`, `pending_connect`, and the two shutdown
  flags. Cloning the resource gives worker threads
  (`singleplayer-start`, `direct-connect-attempt`, `auto-connect-attempt`,
  `game-session-shutdown`) a handle to the same shared state.
- `ClientConnectionStatus` enum
  (`Idle | Connecting | Connected | Disconnected(reason)`). Flipped by the
  Lightyear-driving systems; available to the UI via
  `ClientNetwork::status()` (no UI consumer yet — see "Open / deferred").
- `ClientNetworkPlugin` — registers the resource and the chained `Update`
  systems: `process_pending_connect_system`, `send_client_messages_system`,
  `receive_server_messages_system`, `report_client_disconnect_system`,
  `drive_shutdown_system`.
- `client_plugins()` — returns the configured `client::ClientPlugins` so
  `app.rs` doesn't need to know the protocol tick-rate constant.

`ClientSession` shrinks to a thin handle: the shared `ClientNetwork` clone
plus an `Option<GameServerHandle>` for the loopback server. Its `send`,
`tick`, `shutdown` methods are preserved (Option B from the plan, minimal
call-site churn): `send` pushes to the shared outbox, `tick` drains the
shared inbox, `shutdown` sets the shutdown flag and blocks the worker
thread on `shutdown_complete` while the main app drives the multi-tick
flush.

`start_singleplayer` / `connect` now return as soon as the loopback server
is up; they install a `PendingConnect` in the shared state and the main
app's `process_pending_connect_system` picks it up on the next `Update` to
spawn the Lightyear client entity and trigger `client::Connect`.

`app.rs` installs the new plugins alongside `DefaultPlugins`:

```rust
.add_plugins(client_plugins())
.add_plugins(LightyearProtocolPlugin)
.add_plugins(ClientNetworkPlugin);
```

`LIGHTYEAR_PROTOCOL_ID`, `LightyearProtocolPlugin`, `PrivateKeyContext`,
`private_key`, `send_client_message`, `send_server_message`, and the
channel markers were promoted from `pub(super)` to `pub(crate)` so the
plugin install and the new systems can see them.

UI flow: `UiResources` gained a `client_network: Res<ClientNetwork>` field
and threads it through `worlds_ui` / `multiplayer_ui`. The three connect
entry points (`worlds/session.rs::start_singleplayer_in_background`,
`multiplayer/direct_connect.rs::start_direct_connect_attempt`,
`systems/auto_connect.rs::auto_connect_start_system`) clone the resource
and hand it to the worker.

Test fixtures in [src/net/tests.rs](src/net/tests.rs) were rewritten
around a small `TestRig` that pairs a `ClientSession` with a minimal
Bevy `App` (`MinimalPlugins` + `client_plugins()` +
`LightyearProtocolPlugin` + `ClientNetworkPlugin`), then drives
`app.update()` to advance the handshake.

Files touched:

- [src/net/client.rs](src/net/client.rs) — full rewrite (thread removed)
- [src/net/channels.rs](src/net/channels.rs) — visibility bumps
- [src/net.rs](src/net.rs) — re-exports
- [src/app.rs](src/app.rs) — plugin install
- [src/app/ui.rs](src/app/ui.rs),
  [src/app/ui/worlds/{mod,session,table,tests}.rs](src/app/ui/worlds/),
  [src/app/ui/multiplayer.rs](src/app/ui/multiplayer.rs),
  [src/app/ui/multiplayer/direct_connect.rs](src/app/ui/multiplayer/direct_connect.rs),
  [src/app/systems/auto_connect.rs](src/app/systems/auto_connect.rs) —
  thread `ClientNetwork` through UI / connect entry points
- [src/net/tests.rs](src/net/tests.rs) — `TestRig` rewrite

466 tests pass; `./cli check`, `./cli lint`, `./cli test` all clean.

### Design decisions

- **Option B for `session.send`.** Kept `ClientSession::send/tick/shutdown`
  as methods (Option B from the plan): `send` pushes to the shared
  `Arc<Mutex<VecDeque<ClientMessage>>>` outbox; `tick` drains the inbox
  side. The alternative was a `ClientMessageSender` `SystemParam` wrapper
  at every call site (~5 sites). Option B is less idiomatic Bevy but
  zero churn for `voice/systems.rs`, `input/movement.rs`,
  `input/inventory_shortcuts.rs`, `ui/chat.rs`, `systems/settings.rs`,
  and `apply_message` consumption inside `network_tick_system`.
- **Auth is queued, not pumped separately.** `process_pending_connect`
  `push_front`s the `ClientMessage::Auth` onto the outbox. Once the
  `Connected` component appears on the client entity,
  `send_client_messages_system` drains the outbox in order, so auth
  rides ahead of anything gameplay queued during the handshake.
- **`network_tick_allowed` still gates on `Screen::InGame`.** The Welcome
  arrives via the inbox while the loading splash is still up; it just
  sits there until the UI transitions to `InGame`, at which point
  `network_tick_system` drains and applies it. No need for a "process
  Welcome out-of-screen" branch.
- **`app.finish()` + `app.cleanup()` in the test rig is load-bearing.**
  Without them Lightyear's UDP receive path never delivers inbound
  packets, even though sends still work. The original standalone client
  App called them too — easy to miss when porting to a test rig.
- **`ClientSession` kept as a thin marker.** Per the original plan's
  recommendation. `ClientRuntime::session: Option<ClientSession>` is
  still the "are we in a session?" gate everywhere; ripping it out would
  touch ~20 sites for no benefit.

### Open / deferred

- **UI doesn't poll `ClientConnectionStatus` yet.** Today the UI calls
  `menu.enter_in_game()` the moment the worker thread returns OK
  (loopback server up). The handshake then completes a few frames later
  while the loading splash is still at full opacity. This is fine for
  loopback SP and direct MP on a local network, but for slow remote MP
  the splash will hold visibly longer than today's "block until Welcome"
  worker did. When that becomes noticeable, add a system that watches
  `ClientNetwork::status()` and either (a) flips
  `LoadingSplash::ready = true` only on `Connected` or (b) reverts to
  the menu on `Disconnected(reason)`. Hook the same status to drive a
  "Connecting…" indicator on the direct-connect modal.
- **Shutdown timeout fallback.** `ClientSession::shutdown` polls
  `shutdown_complete` for up to 5 s. If the main app is genuinely stuck
  (paused frame loop, hung system), the worker still bails after the
  timeout and tears down the loopback server. No retry — the on-disk
  save is the only durable state. Revisit if real bug reports show up
  here.

---

## Phase 4 — Wire chunk rooms for resource nodes

**Status**: ✅ done

### What landed

Lightyear room-based replication is now live for resource nodes, gated on a
`replicated-nodes` Cargo feature (default off) so the existing snapshot path
keeps shipping in parallel for A/B testing.

`lightyear` features now include `replication`; `Cargo.toml` defines the
new `replicated-nodes` opt-in.

Server side ([src/net/host.rs](src/net/host.rs)):

- `RoomPlugin` is installed alongside `LightyearProtocolPlugin`.
- `ChunkRoomMap` (a `HashMap<ChunkCoord, Entity>`) lazily owns one
  `Room`-marked entity per chunk. The two lazy helpers
  `ensure_chunk_room_world` and `ensure_chunk_room_commands` cover the
  exclusive-system and the per-tick-system call sites without sharing
  borrows. Rooms are spawned on first need (entity placement or sender
  subscription) and live for the lifetime of the host app.
- `sync_resource_node_entities`, on the new-entity branch, calls
  `attach_node_replication(world, entity, chunk)` which attaches
  `Replicate::to_clients(NetworkTarget::None) + NetworkVisibility` and
  triggers `RoomEvent { room, target: AddEntity(entity) }`. `None` plus
  `NetworkVisibility` makes the room the sole driver of per-client
  visibility — no broadcast fallback.
- `install_replication_sender_on_link` is a `On<Add, LinkOf>` observer
  that inserts `ReplicationSender::default()` on every fresh client link
  so Lightyear can actually ship component diffs. `RoomPlugin`'s built-in
  `handle_disconnect` observer scrubs the sender from every room when
  `Disconnected` is added, so we don't bookkeep that manually.
- `update_client_room_subscriptions` runs every Update (after the mirror
  syncs). It calls `GameServer::visible_chunks_for_client` for every
  connected client, diffs against the cached `ClientChunkSubs` set, and
  triggers `RoomEvent { target: AddSender/RemoveSender }` only for the
  delta. Idle players emit zero events; chunk-crossing players emit a
  single boundary's worth of swaps.

GameServer accessors added: `client_view`, `connected_client_ids`,
`visible_chunks_for_client`. `ServerConnections::entity_for_client`
exposes the `ClientOf` (sender) entity for an id.

Client side ([src/net/client.rs](src/net/client.rs)):

- The Lightyear client entity is spawned with `ReplicationReceiver::default()`
  so incoming entity/component diffs are buffered and applied.

Protocol registration ([src/net/channels.rs](src/net/channels.rs)):

- `ResourceNode` and `ResourceNodeStorage` are registered via
  `AppComponentExt::register_component`. Both gained `PartialEq +
  Serialize + Deserialize` so they meet the replication trait bounds.

Consumer wiring ([src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs)):

- `apply_resource_nodes_system` now takes a
  `Query<(&ResourceNode, &ResourceNodeStorage)>` parameter and routes the
  per-tick input through `collect_resource_node_states`. Without the
  feature, that helper returns the existing `runtime.snapshot.resource_nodes`
  clone. With `--features replicated-nodes`, it materialises the same
  `Vec<ResourceNodeState>` shape from the replicated entities. Pop-in,
  death-effect, depleted-id, and AoI-leave handling are unchanged either
  way. `respawn_progress` is forced to `None` under replication for now —
  regen visuals stay snapshot-only until Phase 6.

Tests, lint, and `./cli check` are clean on both feature configurations
(466 passed, default; 466 passed, `--features replicated-nodes`).

### Key design decisions

- **`NetworkTarget::None` + `NetworkVisibility` + Room.** Reviewed the
  `network_visibility` example and `lightyear_replication::visibility::room`
  source; the room machinery calls `gain_visibility(sender)` on the
  entity's `ReplicationState` for every sender in a shared room, so
  `NetworkTarget::None` (entity defaults to nobody) plus rooms (room
  drives gain/lose) gives the wanted "room is the sole visibility gate"
  shape. Late-joining clients pick up existing entities automatically
  through the same path.
- **`Room` is an Entity, not a `RoomId`.** Lightyear 0.26 switched the
  Room model: `Room` is now a regular Bevy component on a spawned entity,
  and `RoomEvent` is triggered against it. `ChunkRoomMap` therefore maps
  `ChunkCoord -> Entity`.
- **Change-driven sender subscriptions.** `update_client_room_subscriptions`
  diffs against `ClientChunkSubs`, so it only fires `RoomEvent`s on the
  boundary delta — the system itself runs every Update but emits work
  proportional to player movement, not player count.
- **A/B via feature flag, both paths server-side.** Snapshot still ships
  the node list; only the client consumer switches based on the flag.
  Phase 6 deletes the snapshot path.

### Open / deferred

- **`respawn_progress` is not replicated yet.** The mirror does not
  expose the field on the ECS entity, so under `replicated-nodes` the
  regenerating-state visual (the "node is mid-respawn" hint) does not
  appear. Snapshot path still has it. Plan to either add a
  `ResourceNodeRegrow` component carrying `progress: f32` and replicate
  that, or accept the loss when Phase 6 removes snapshot. Decision in
  Phase 6.
- **Reliability tuning.** Default mode (sequenced unreliable with
  retransmit-on-ack on the replication side) is fine for sparse,
  rarely-changing nodes. Revisit if dropouts appear during soak.
- **Initial state delivery cost.** When a client first subscribes to a
  chunk room, Lightyear ships the full spawn for every entity in it.
  That's a burst on chunk crossings (5–20 entities) vs the previous 20
  Hz full snapshot — net win on idle, similar peak.

### Files touched

- [Cargo.toml](Cargo.toml) — `replication` feature on lightyear; new
  `replicated-nodes` feature flag
- [src/net/host.rs](src/net/host.rs) — RoomPlugin install, ChunkRoomMap,
  ClientChunkSubs, attach_node_replication, install_replication_sender_on_link,
  update_client_room_subscriptions
- [src/net/host/routing.rs](src/net/host/routing.rs) — `entity_for_client`
- [src/net/channels.rs](src/net/channels.rs) — component registration
- [src/net/client.rs](src/net/client.rs) — `ReplicationReceiver` on client
- [src/server.rs](src/server.rs) — `client_view`, `connected_client_ids`,
  `visible_chunks_for_client`
- [src/server/resource_node_ecs.rs](src/server/resource_node_ecs.rs) —
  `PartialEq + Serialize + Deserialize` on the two replicated components
- [src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs) —
  feature-gated input source via `collect_resource_node_states`

### Verification

- `./cli check`, `./cli test` (466 ok), `./cli lint` clean on default.
- `cargo check --features replicated-nodes`,
  `cargo test --features replicated-nodes` (466 ok),
  `cargo clippy --features replicated-nodes --all-targets -- -D warnings` clean.
- Manual 2-client MP soak (still pending): walk out of chunk → nodes
  despawn; walk back → reappear.

---

## Phase 5 — Chunk rooms for dropped items, deployables, players

**Status**: ✅ done

### What landed

Phase 4's room infrastructure now covers every networked entity type. The
`replicated-nodes` feature flag still toggles the client consumers; with
the flag, every visual driver (resource nodes, dropped items, deployables,
remote players) reads from Lightyear-replicated entities rather than
`WorldSnapshot`. The snapshot path keeps shipping in parallel until
Phase 6 deletes it.

Server side ([src/net/host.rs](src/net/host.rs)):

- `attach_node_replication` was generalised and renamed to
  `attach_room_gated_replication`. It now backs every static / room-only
  entity type (resource nodes, dropped items, deployables) —
  `Replicate::to_clients(NetworkTarget::None) + NetworkVisibility +
  RoomEvent::AddEntity`.
- `attach_player_replication` is new and handles the player public/private
  split. The entity uses `Replicate::to_clients(NetworkTarget::All)` (so
  the universe of senders is "everyone connected") and
  `NetworkVisibility` (so the room narrows to the chunk). Then a
  `ComponentReplicationOverrides<PlayerPrivate>` component is attached on
  the entity, configured `.disable_all().enable_for(owner_sender)`.
  Result: peers in the chunk room receive `PlayerPublic` but never the
  inventory/crafting bytes of someone else's `PlayerPrivate`.
- `move_entity_between_rooms` handles dynamic chunk transitions
  (`RemoveEntity` from the old chunk's Room, `AddEntity` to the new
  chunk's Room). `sync_dropped_item_entities` calls it when the
  authoritative `dropped_item_chunk` for a live id changes (physics
  body rolled); `sync_player_entities` calls it on player chunk
  crossings. Each call also updates the local `*Chunk` mirror component
  on the entity. Resource nodes and deployables are static and skip
  this path.

Protocol registration ([src/net/channels.rs](src/net/channels.rs)):

- Registered `DroppedItem`, `DroppedItemTransform`, `Deployable`,
  `DeployableTransform`, `DeployableHealth`, `DeployableActive`, `Player`,
  `PlayerPublic`, `PlayerPrivate` alongside the Phase 4 resource-node
  pair. Every component now Serde+PartialEq (added via derive) so they
  meet Lightyear's `register_component` trait bounds. `Deployable` uses
  the same `deserialize_interned_item_id` helper as the other
  `ItemId`-bearing wire types so peers and host agree on the interned
  string.

Client consumers (all under `#[cfg(feature = "replicated-nodes")]`):

- [src/app/systems/items/dropped.rs](src/app/systems/items/dropped.rs) —
  `collect_dropped_items` returns `Vec<(DroppedWorldItem, tick)>` from
  either source. Under the flag the per-id tick is
  `Ref::<DroppedItemTransform>::last_changed().get() as u64`, so
  interpolation `retarget` fires only on real transform changes. Without
  the flag the tick is `snapshot.tick` for every item (legacy behaviour).
- [src/app/systems/deployables.rs](src/app/systems/deployables.rs) —
  `collect_deployed_entities` materialises a `Vec<DeployedEntityState>`
  from the four-component replicated query (`Deployable`,
  `DeployableTransform`, `DeployableHealth`, `DeployableActive`) under
  the flag, or copies `snapshot.deployed_entities` without it.
- [src/app/systems/players.rs](src/app/systems/players.rs) —
  `collect_remote_players` returns a minimal `RemotePlayerSample` (id,
  position, yaw, per-id tick) from either source. The retired
  snapshot-driven unit test
  (`apply_snapshot_spawns_updates_and_removes_remote_players`) is gated
  off with `#[cfg(not(feature = "replicated-nodes"))]` — the equivalent
  test against the replicated path needs a real Lightyear plugin set up
  and is out of scope for a unit test; Phase 6 deletes that test along
  with the snapshot path.

Tests / lint / check pass on both feature configurations:
- default: 466 tests pass; `./cli check` / `./cli lint` clean.
- `--features replicated-nodes`: 465 tests pass (one snapshot-only test
  gated); `cargo check` / `cargo clippy --all-targets -- -D warnings`
  clean.

### Key design decisions

- **`ComponentReplicationOverrides<PlayerPrivate>` rather than two
  entities.** Lightyear 0.26's `Replicate` is per-entity; per-component
  per-sender control is handled via the overrides component. That kept
  the player ECS shape we already had from Phase 2 (one entity, two
  payload components, one chunk component) instead of needing a second
  "private mirror" entity.
- **Owner sender is captured at spawn.** The override stores a sender
  entity, so the overrides are recomputed when a player respawns (which
  happens any time `sync_player_entities` despawns and re-spawns the
  entity, e.g. reconnect). For the steady-state case where the sender
  entity hasn't changed the overrides stay valid.
- **Move events, not despawn/respawn.** When an entity changes chunks
  we trigger `RemoveEntity` then `AddEntity` on the room machinery.
  Lightyear's `RoomEvents::shared_counts` makes simultaneous
  remove/add a no-op visibility-wise for senders subscribed to both
  rooms — peers walking across the same boundary see no flicker.
- **Per-id ticks via `Ref::last_changed()`.** Avoids the
  always-retarget pitfall (elapsed resets every frame → broken
  interpolation timing). Bevy's change tick advances only when the
  component is mutated, so `retarget`'s `tick <= self.snapshot_tick`
  guard is now correct under replication too.

### Open / deferred

- **Inventory / crafting UI still reads `runtime.snapshot`.** The owner's
  `PlayerPrivate` is on the replicated entity but the inventory hotbar
  consumer, crafting queue UI, and open-furnace UI still pull from the
  snapshot path. Phase 6 retargets these to query the replicated
  `PlayerPrivate` component of the local player.
- **Resource node `respawn_progress`** is still snapshot-only, same
  caveat as Phase 4.
- **Voice frames** stay on the unreliable `VoiceChannel` — distance-
  gated, not chunk-gated. No change.

### Files touched

- [src/net/channels.rs](src/net/channels.rs) — register the 9 new
  components.
- [src/net/host.rs](src/net/host.rs) — generalise
  `attach_room_gated_replication`, add `attach_player_replication`, add
  `move_entity_between_rooms`, handle chunk transitions in the dropped-
  item and player mirror systems.
- [src/server/dropped_item_ecs.rs](src/server/dropped_item_ecs.rs),
  [src/server/deployable_ecs.rs](src/server/deployable_ecs.rs),
  [src/server/player_ecs.rs](src/server/player_ecs.rs) — `PartialEq +
  Serialize + Deserialize` derives, `deserialize_interned_item_id` for
  `Deployable.item_id`.
- [src/app/systems/items/dropped.rs](src/app/systems/items/dropped.rs),
  [src/app/systems/deployables.rs](src/app/systems/deployables.rs),
  [src/app/systems/players.rs](src/app/systems/players.rs) — feature-
  gated input collectors and per-id tick wiring.

### Verification (still pending manual)

- 2-client MP: drop an item; second client walks into the chunk → drop
  appears, walks out → drop despawns.
- 2-client MP: place a workbench; second client enters chunk → workbench
  visible with health; first client damages it → second client's health
  reading updates.
- 2-client MP: open inventory locally → still works. Peer's player
  entity on the local world has `PlayerPrivate` absent (`Query::<&PlayerPrivate>`
  returns one entry — your own).

---

## Phase 6 — Migrate consumers off `WorldSnapshot`, delete it

**Status**: ⏳ pending — split into six step-sized sub-phases. Each one
is a single chat session, ends with a commit that compiles, lints, and
passes tests, and is **independently verifiable in-game** before the
next sub-phase starts.

### What the wire looks like today

After commit `44eb81f` ("`fix: use NetworkTarget::All so per-component
replication actually ships`"), two paths run in parallel:

- **Per-component replication** (Lightyear): ships entity spawns,
  despawns, and per-component diffs only when something changes,
  room-gated to the player's AoI chunk ring. Empirically verified
  with the `replication-trace` feature flag — server-side mutations
  produce matching client-side `RECV` log lines within ~10–20 ms.
- **`WorldSnapshot` at 10 Hz** (legacy): still ships every 100 ms
  with the full AoI state. Every UI / pickup / HUD consumer reads
  from `ClientRuntime::snapshot`; that's the source of truth they
  trust.

Both paths are correct. The snapshot is purely duplicate bandwidth —
removing it is the goal of this phase.

### Lesson from the failed Phase 6a attempt

The original Phase 6a (commit `96c8b31`) tried to delete the snapshot
wire in one shot and synthesise `ClientRuntime::snapshot` locally from
the replicated entities. It surfaced six gameplay regressions (no
tree-count countdown, no furnace on/off, no deployable HP nameplate,
etc.). After reverting and adding the `replication-trace` diagnostic,
the root cause turned out to be **`Replicate::to_clients(NetworkTarget::None)`**:
Lightyear's room machinery added senders via `gain_visibility` and
shipped the initial spawn message, but the post-spawn component-diff
buffer in `lightyear_replication/src/send/buffer.rs` apparently
treats senders that weren't in `Replicate`'s original target
differently — diffs never went out. Switching to
`NetworkTarget::All + NetworkVisibility` fixed it: `All` lists every
connected client in the entity's targets up front; `NetworkVisibility`
(driven by the room state) still filters down to clients in a shared
chunk room.

Two takeaways for this phase:

1. **Migrate one consumer at a time**, not all at once. Each step
   below ends with the snapshot wire still active, so a regression
   only breaks the one consumer that was migrated and the player can
   keep playing while you debug.
2. **Use `--features replication-trace`** for any sub-phase where you
   suspect a Lightyear delivery problem. The diagnostic already
   covers `ResourceNodeStorage`, `DeployableHealth`, and
   `DeployableActive`; extend it for new components as you go.

### Goal

Every client-side reader of `runtime.snapshot.*` is repointed at a
Bevy `Query` over the matching replicated components. The
`WorldSnapshot` type, `ClientRuntime::snapshot`, the
`Welcome.snapshot` field, and the `ServerMessage::Snapshot` variant
all disappear. The per-tick snapshot broadcast loop in
`GameServer::tick` is gone. Idle clients are near-silent on the wire.

### Sub-phase ordering

The order is "smallest blast radius first" so a bug only ever affects
one consumer at a time. After each sub-phase, manually verify the
in-game behaviour listed under **Verification** before moving on.

#### Phase 6.1 — `PlayerPrivate` (local inventory / crafting / furnace UI)

Smallest first: this only affects what the local player sees in their
own UI. No peer state involved.

**Consumers to retarget:**
- [src/app/ui/inventory.rs](src/app/ui/inventory.rs) — reads
  `runtime.local_player().and_then(PlayerState::inventory)`.
- [src/app/ui/crafting.rs](src/app/ui/crafting.rs) — reads
  `runtime.local_player()` and its `.crafting`.
- [src/app/ui/furnace.rs](src/app/ui/furnace.rs) — reads
  `local_player.open_furnace`.
- [src/app/systems/input/menu_toggles.rs](src/app/systems/input/menu_toggles.rs)
  — reads `local_player.open_furnace.is_some()`.
- [src/app/systems/items/tool_swap.rs](src/app/systems/items/tool_swap.rs)
  — reads `local_player.inventory`.

**Approach:**
1. Add `LocalPlayerComponents` system param (or a `LocalPlayerSnapshot`
   resource refreshed from a query) that exposes the local player's
   replicated `(PlayerPublic, PlayerPrivate)` via the
   `PlayerIndex.get(runtime.client_id?)` lookup we already have on
   the server side… wait, that's server-side. On the client side
   the local player entity is whichever replicated `Player` entity
   matches `runtime.client_id`. Add a small client-side
   `LocalPlayerIndex` resource keyed on `client_id`, populated by a
   system that runs on `Added<Player>`.
2. Retarget each consumer one at a time. After each, in-game-test
   that the corresponding UI still works.
3. Remove `inventory: Option<PlayerInventoryState>`,
   `crafting: Option<PlayerCraftingState>`, and
   `open_furnace: Option<OpenFurnaceView>` from `PlayerState` (the
   wire type). The snapshot's player entries shrink to the
   peer-visible public fields.

**Verification:**
- Inventory UI shows and updates correctly when picking up / crafting
  items.
- Crafting queue shows in-flight jobs and ticks down progress bars.
- Furnace UI opens with the right contents when interacting with a
  furnace; smelt + fuel progress bars animate.

#### Phase 6.2 — `PlayerPublic` (peer avatars)

**Consumers to retarget:**
- [src/app/systems/players.rs](src/app/systems/players.rs)
  `apply_snapshot_system` — peer avatar spawn / despawn / interpolation.
- Wherever `local_view()` reads from snapshot — replace the
  fallback path to read from the local player's replicated
  `PlayerPublic` via the `LocalPlayerIndex` from Phase 6.1.

**Approach:** mirror the existing system shape but iterate
`Query<(&Player, Ref<PlayerPublic>)>` instead of
`snapshot.players.iter()`. Use `Ref::last_changed().get() as u64` as
the per-id interpolation tick (we already proved this works in the
Phase 5 collectors).

**Verification:** in MP, a second client's avatar appears, moves
smoothly, despawns when they walk out of the AoI ring. Chat bubbles
show on peers. Health-bar nameplate updates when peers take damage.

#### Phase 6.3 — `Deployable` + family

**Consumers to retarget:**
- [src/app/systems/deployables.rs](src/app/systems/deployables.rs)
  `apply_deployed_entities_system` — workbench/furnace spawn/despawn
  and furnace-light toggle.
- [src/app/state/runtime.rs](src/app/state/runtime.rs)
  `rebuild_world_grid` — reads `snapshot.deployed_entities` for
  collision colliders. Switch to reading replicated `Deployable +
  DeployableTransform` directly; trigger a rebuild on
  `Added<Deployable>` / `RemovedComponents<Deployable>`.
- [src/app/systems/items/pickup.rs](src/app/systems/items/pickup.rs)
  `update_pickup_target_system` — `snapshot.deployed_entities.iter()`
  for raycast targeting.

**Verification:** place a workbench → it appears with correct
visuals. Damage it → HP nameplate counts down. Place a furnace,
toggle it on → glow + smoke animations fire and stay on.

#### Phase 6.4 — `DroppedItem`

**Consumers to retarget:**
- [src/app/systems/items/dropped.rs](src/app/systems/items/dropped.rs)
  `apply_dropped_items_system` — visual interpolation.
- The dropped-item branch of `update_pickup_target_system`.

**Verification:** drop an item → it appears. Pick it up → particle
burst fires (the existing `depleted_node_ids`-style signal is
ResourceNode-only, but item-picked is similar — confirm pickup feels
right). Drop a stack of items, watch them settle from the air
smoothly.

#### Phase 6.5 — `ResourceNode`

**Consumers to retarget:**
- [src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs)
  `apply_resource_nodes_system` — node spawn, pop-in animation,
  depletion death-effect.
- The resource-node branch of `update_pickup_target_system`.
- The resource-node-collider portion of `rebuild_world_grid`.

**Tricky bit:** `ResourceNodeState.respawn_progress` is currently a
server-driven field on the wire. The mirror does not put it on the
ECS entity. Decide between (a) adding a `ResourceNodeRegrow {
progress: f32 }` component the server replicates while the node is
regenerating, or (b) shipping respawn as a discrete
`ServerMessage::ResourceNodeRegrowProgress` event. (a) is cleaner if
we're keeping replication as the only path; (b) is a smaller diff.

**Verification:** gather a tree → count-down updates in real time.
Fell it → death animation plays. Watch a node regenerate (5–15 min
wait, or admin-spawn one with low storage and gather it to test the
short cycle).

#### Phase 6.6 — Cleanup: delete the snapshot path

Everything below should now be dead code. Delete it in one commit:

1. **Server**:
   - The per-tick snapshot broadcast loop in
     [src/server.rs](src/server.rs) `GameServer::tick`.
   - `snapshot()`, `snapshot_for()`, `snapshot_inner()` in
     [src/server/connection.rs](src/server/connection.rs).
   - Tracing spans `snapshot_broadcast`, `snapshot_inner`,
     `snapshot_players`, `snapshot_dropped_items`,
     `snapshot_resource_nodes`, `snapshot_deployables` from Phase 0.
2. **Welcome**: replace the `snapshot: WorldSnapshot` field with a
   lean `local_seed: PlayerSpawnSeed { position, yaw, pitch,
   health, last_processed_input }` for prediction bootstrap.
3. **Protocol**: delete `ServerMessage::Snapshot` variant, the
   `WorldSnapshot` type, `PlayerState` (or trim it to only the
   inputs-correction shape if `ServerMessage::Correction` still
   uses it), `OpenFurnaceView` if it's only on the snapshot,
   `DroppedWorldItem` if it's only on the snapshot,
   `ResourceNodeState`, `DeployedEntityState`.
4. **ClientRuntime**: delete the `snapshot` field; delete
   `resource_node_collider_set_version`; delete the `is_stale_snapshot`
   helper (was already removed in 6a, may have come back via the
   revert).
5. **Tests**: the existing `src/server/tests/*` use `server.snapshot()`
   as a state accessor. Replace each with the equivalent direct
   GameServer accessor (`dropped_items_iter`, `resource_nodes_iter`,
   `players_iter`, `deployables_iter`). About 30 sites — wrap them
   in a small `TestFixture` helper if the diff is unwieldy.
6. **Save-format bump**: bump `SAVE_FORMAT_VERSION` if the on-disk
   shape changes (probably not — the save format doesn't carry
   `WorldSnapshot`, but double-check).
7. **`replication-trace` feature**: keep as-is; it's useful for
   debugging future replication issues.
8. **Snapshot-frequency stopgap**: delete
   `SNAPSHOT_BROADCAST_INTERVAL_TICKS` from
   [src/server.rs](src/server.rs); it's a no-op once the broadcast
   loop is gone.

**Verification:**
- `./cli check` / `./cli test` / `./cli lint` clean.
- Singleplayer + 2-client multiplayer play normally.
- `RUST_LOG=replication_trace=info` shows continuous activity
  during play but is silent when nobody is doing anything.
- Wireshark / pcap on the loopback port: a sitting-still client
  sends only heartbeats + occasional `WorldTime`. No periodic
  snapshot burst.

### Open / deferred

- **`PerfStats` message** stays. It's not entity state, it's a perf
  HUD payload.
- **`WorldTime` broadcast** stays. Single global value, 1 Hz.
- **`Toast`, `ResourceImpact`, `ItemMerged`, `ResourceNodeDepleted`,
  `Correction`** — these are events, not state. They stay on the
  message channel.

---

## Phase 7 — Post-migration audit & cleanup

**Status**: ⏳ pending · blocked by Phase 6

### Goal

After Phase 6 lands, the migration is functionally complete but the
tree probably carries leftovers: dead code paths, comments that still
reference the snapshot, helper types that exist only to feed
serialization that no longer exists, and so on. Phase 7 is a single
focused audit pass: read the diff, find the cruft, remove it.

### What to look for

This is a **judgment / exploration phase**, not a step-by-step recipe.
The agent picking this up should explicitly review each of the items
below and write up findings before changing anything. Some items will
be "no action needed", and that's a valid outcome.

1. **Dead types and re-exports.**
   - `PlayerState`, `DroppedWorldItem`, `ResourceNodeState`,
     `DeployedEntityState`, `OpenFurnaceView` — these were wire
     shapes for the snapshot. After Phase 6.6, are any still
     reachable? If only the `Correction` path uses something like
     `PlayerState`, the type can be trimmed. If a type has no
     remaining caller, delete it.
   - `DeployableView`, `PlayerView` — Phase 1/2 introduced these as
     server-internal "wire shape" structs to feed the mirror sync.
     If the ECS entities are now the source of truth, the mirror
     can read directly from `GameServer` fields. Verify whether
     these views still earn their keep.
   - `read_dropped_item`, `read_resource_node_state`,
     `collect_resource_node_states` — helpers introduced to
     translate the ECS components back into the wire shape. If
     nothing reads the wire shape, these go.

2. **The ECS mirror systems themselves.**
   - `sync_resource_node_entities`, `sync_dropped_item_entities`,
     `sync_deployable_entities`, `sync_player_entities` — these
     copy `GameServer` HashMap state into ECS components every
     tick. They exist because the HashMap is still authoritative
     server-side.
   - Phase 1b (`Fold resource_nodes HashMap into entities`) and
     the analogous never-started Phase 2b are the path to making
     ECS authoritative and deleting the mirror systems entirely.
     Phase 7 is the right place to decide: do we want to schedule
     Phase 1b + 2b now (multi-session refactor each, no behaviour
     change), defer them indefinitely, or pick just the resource-
     nodes one because that's where regrow + gather hit-rate
     concentrates the most mutations per tick?

3. **The legacy snapshot tests.**
   - `src/server/tests/*` use `server.snapshot()` extensively as a
     state accessor. Phase 6.6 will have replaced those. Look for
     any helper introduced during the migration that's now
     redundant.
   - `src/net/tests.rs` — the singleplayer/MP integration tests
     used to assert on `ServerMessage::Snapshot` arrivals. The
     `ServerMessage::Welcome` assertion is the right replacement;
     verify the wire-coverage stayed equivalent.

4. **Comments that reference the snapshot path.**
   - `grep -rn 'snapshot' src/` after Phase 6 will return ~hundreds
     of comments referencing the (now-gone) wire snapshot. Walk
     them and either update or delete.
   - `WorldSnapshot` in code comments — should be replaced with
     references to the specific replicated components.

5. **The chunk-room subscription cost.**
   - `update_client_room_subscriptions` runs every tick and
     diffs per-client `visible_chunks`. Most ticks it's a no-op
     (the player didn't cross a boundary). Worth measuring with
     `./cli profile` and checking the trace — if the
     `update_client_room_subscriptions` span is hot, gate it on
     a "player crossed chunk this tick" event from
     `ChunkManager::update_player_chunk` instead of running
     unconditionally.

6. **The `replication-trace` feature.**
   - It's a dev-only diagnostic. Keep it in tree for future
     replication debugging, but make sure it doesn't get baked
     into release builds (already gated by `#[cfg(feature = ...)]`).
   - Consider extending it to also log `DroppedItem` /
     `DroppedItemTransform` / `Player` / `PlayerPublic` /
     `PlayerPrivate` mutations once those are all migrated, so
     the diagnostic stays useful for the full surface.

7. **CLAUDE.md and docs/networking.md.**
   - `CLAUDE.md` references the snapshot path in the
     server-side description ("State delivery: per-client
     `ServerMessage::Snapshot(WorldSnapshot)` every tick"). Update
     to describe room-based replication.
   - `docs/networking.md` — same.
   - `docs/architecture.md` — same.

8. **`SNAPSHOT_BROADCAST_INTERVAL_TICKS` stopgap.**
   - Sanity-check this constant was deleted in 6.6 along with the
     broadcast loop.

9. **`PERF_STATS_BROADCAST_INTERVAL_TICKS`.**
   - This stays — perf stats aren't entity state — but its
     comment may still reference the snapshot path.

10. **Save format bookkeeping.**
    - Verify `SAVE_FORMAT_VERSION` reflects the cumulative breaking
      changes from the migration. If 6.6 didn't bump it but
      something on-disk did change, fix it now.

11. **The `Welcome.local_seed` field (from 6.6).**
    - Verify the prediction seed contains the minimum fields
      needed (position, yaw, pitch, health, last_processed_input).
      Anything more is dead weight.

12. **Phase 1b / 2b reconsideration.**
    - With the migration complete, is the `GameServer.resource_nodes`
      / `dropped_items` / `deployed_entities` / `clients` HashMap
      doing any work the ECS entities couldn't do? Decide whether
      to schedule 1b/2b or close them as won't-fix.

### Output

Phase 7 ends with:

- One commit removing dead code identified in the audit.
- One commit updating CLAUDE.md and `docs/*.md` to reflect the new
  architecture.
- A short "Phase 7 findings" section appended below this one
  documenting what was deleted, what was kept and why, and any
  follow-ups that didn't fit this session.

### Verification

- `./cli check && ./cli test && ./cli lint` clean.
- `grep -rn 'WorldSnapshot\|ServerMessage::Snapshot\|runtime\.snapshot' src/`
  returns zero hits.
- Singleplayer + 2-client MP soak still feels right.

---

## Phase 1b — Fold `resource_nodes` HashMap into entities (cleanup)

**Status**: ⏸ deferred — no functional impact, ~multi-session refactor

### Why deferred (intentional)

Phase 1b is a pure code-cleanliness pass: remove the `resource_nodes:
HashMap` field on `GameServer` so the ECS mirror entities (alive since
Phase 1) become authoritative on their own. It ships zero user-facing
or wire-facing behaviour change — the replication migration that
mattered (Phases 4–6) is done. Reasons for deferral:

1. **No bandwidth, latency, or correctness win.** The HashMap +
   `sync_resource_node_entities` mirror runs in well under 1 ms per
   tick on the host App. ECS-as-authoritative would save ~80 bytes
   per node of duplicate state and one exclusive system, but neither
   is on any hot path.
2. **The migration's working set is intact.** Phase 6 ended with
   replication-only on the wire and a synthesised
   `runtime.snapshot` driving the consumers. Phase 1b would not
   change either of those — it's strictly server-internal.
3. **Surgery is sprawling.** Every `&mut self GameServer` method
   that touches `self.resource_nodes` needs either a `&mut World`
   parameter or an explicit `Commands + Query` pair. Affected
   production sites:
   - `src/server.rs` — init load + regrow splice
   - `src/server/resource_nodes.rs` — gather (3 sites)
   - `src/server/inventory.rs` — pickup payouts (3 sites)
   - `src/server/persistence.rs` — `world_save` builder
   - `src/server/commands.rs` — admin spawn
   - `src/server/connection.rs` — `snapshot_inner` still reads
     the HashMap to bootstrap Welcome (Phase 6b retires this when
     the snapshot path is fully removed)
   - `src/server/chunk_manager.rs` — `tick(now, &resource_nodes)`
     signature; the regrow loop needs the live position set
   - `src/server/tests/resource_nodes.rs` — ~30 direct
     `server.resource_nodes.{clear,insert,iter,len,contains_key}`
     touches across the gather / pickup / regrow / admin tests
4. **Phase 2b (analogues for dropped items, deployables, players)
   has the same shape and the same lack of impact.** Without
   Phase 1b establishing the pattern (e.g. `TestFixture` rig and
   the `&mut World` threading convention), Phase 2b can't land
   either. Better to wait until there's a concrete reason to do
   the refactor at all.

### When to revisit

- A future feature actually needs ECS-only queries on resource
  nodes (e.g. running multiple parallel regrow workers, or
  attaching new components a non-ECS HashMap can't carry).
- A refactor of `GameServer` for unrelated reasons opens the door
  to `&mut World` threading at a lower marginal cost.
- The duplicate state surfaces a real consistency bug.

Until then, the HashMap + mirror is the production-supported path.

### Recipe (from the original plan) when picked up

1. Each method that touches `self.resource_nodes` gains a
   `world: &mut World` parameter (or a more focused `Commands +
   Query` pair).
2. Bevy system call sites in `net/host.rs`
   (`tick_authoritative_server`, `receive_client_messages`) wrap in
   `world.resource_scope::<AuthoritativeServer, _>(|world, mut
   server| { ... })`.
3. Introduce `struct TestFixture { world: World, server: GameServer }`
   in `src/server/tests/mod.rs` so the ~30 test sites flip from
   `server.resource_nodes.clear()` to `fixture.clear_resource_nodes()`
   without touching every test body.
4. `chunk_manager::tick` becomes
   `tick(now, existing_positions: impl Iterator<Item = &ResourceNodeState>)`
   so callers can feed it a query iterator instead of a HashMap.
5. Bump `SAVE_FORMAT_VERSION` again — query-iteration order vs.
   HashMap-iteration order may shift the on-disk Vec layout.
6. Delete the `sync_resource_node_entities` exclusive system from
   `net/host.rs`; ECS entities are now spawned by the gather /
   admin / regrow paths directly.

---

## Cross-phase reference

### Entity-type table

| Entity        | HashMap (server)               | ECS entity                   | Components                                                                            | Index                | Sync system                       |
| ------------- | ------------------------------ | ---------------------------- | ------------------------------------------------------------------------------------- | -------------------- | --------------------------------- |
| Resource node | `GameServer.resource_nodes`    | yes (Phase 1)                | `ResourceNode`, `ResourceNodeStorage`, `ResourceNodeChunk`                            | `ResourceNodeIndex`  | `sync_resource_node_entities`     |
| Dropped item  | `GameServer.dropped_items`     | yes (Phase 2)                | `DroppedItem`, `DroppedItemTransform`, `DroppedItemChunk`                             | `DroppedItemIndex`   | `sync_dropped_item_entities`      |
| Deployable    | `GameServer.deployed_entities` | yes (Phase 2)                | `Deployable`, `DeployableTransform`, `DeployableHealth`, `DeployableActive`, `DeployableChunk` | `DeployableIndex`    | `sync_deployable_entities`        |
| Player        | `GameServer.clients`           | yes (Phase 2)                | `Player`, `PlayerPublic`, `PlayerPrivate`, `PlayerChunk`                              | `PlayerIndex`        | `sync_player_entities`            |

### Glossary

- **AoI** — Area of Interest. The chunk ring around a player that determines
  which entities they see.
- **`ChunkCoord`** — Integer (x, z) of a 64 m × 64 m world chunk.
  See [src/world/chunk/mod.rs](src/world/chunk/mod.rs).
- **`ChunkManager`** — Server-side owner of per-chunk membership, AoI
  visibility, and regrow scheduling.
- **`ClientId`** — Server-assigned `u64` per connected client. Wire-stable
  for the session.
- **`ClientConnectionStatus`** — Lightyear lifecycle as observed by the
  main app: `Idle | Connecting | Connected | Disconnected(reason)`.
  Defined in `src/net/client.rs`. Polled via `ClientNetwork::status()`.
- **`ClientNetwork`** — `Resource + Clone` (Arc-backed) that
  `ClientNetworkPlugin` registers. Holds the shared `outbox`, `inbox`,
  `status`, `pending_connect`, and shutdown flags between the main app's
  Lightyear-driving systems and `ClientSession` (which is also held by
  worker threads). Defined in `src/net/client.rs`.
- **`ClientSession`** — Thin handle stored in `ClientRuntime::session`
  with three methods (`send`, `tick`, `shutdown`) that go through
  `ClientNetwork`. Owns the loopback server's `GameServerHandle` in SP.
- **`GameServer`** — The authoritative server-side game state struct,
  currently stored as a `Resource` inside `AuthoritativeServer`.
- **Mirror system** — An exclusive Bevy system (`fn(&mut World)`) that
  reconciles a `GameServer` HashMap into ECS entities every Update. Spawns
  fresh ids, despawns dropped ids, refreshes mutable components on
  surviving ids with change-detection equality guards.
- **`RoomId`** — Lightyear's primitive for per-client visibility groups.
  One `RoomId` per `ChunkCoord` once Phase 4 lands.
- **`WorldSnapshot`** — The custom per-tick full-state message we're moving
  away from. Removed in Phase 6.

### Useful commands

```bash
./cli check              # cargo check
./cli test               # full test suite
./cli lint               # cargo fmt --check && cargo clippy --all-targets -- -D warnings
./cli profile            # build with the `profile` feature and run; emits trace-*.json
```

### Useful greps

```bash
# Every site that touches the resource_nodes HashMap
grep -rn '\.resource_nodes\b' src/

# Every site that reads from the snapshot on the client (Phase 6 removes these)
grep -rn 'runtime\.snapshot' src/

# Every ClientSession call site (Phase 3 audits these)
grep -rn 'ClientSession\|session\.send\|session\.tick' src/
```
