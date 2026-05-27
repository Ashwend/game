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
- **State delivery**: per-client `ServerMessage::Snapshot(WorldSnapshot)`
  every tick. Full state vectors for visible players, dropped items,
  resource nodes, deployables. No delta encoding. AoI by chunk ring.
  Phases 4 / 5 move entity state onto Lightyear's room replication;
  Phase 6 deletes the snapshot path.
- **Entity storage** (server): all authoritative state in `HashMap`s on
  `GameServer` (`clients`, `resource_nodes`, `dropped_items`,
  `deployed_entities`). Mirror ECS entities now exist alongside
  (Phases 1, 2) — kept in sync by exclusive systems in `net/host.rs`.

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
| 6   | Delete `WorldSnapshot`, save-version bump, cleanup             | ✅ done (6a) — 6b deferred |
| 1b  | Fold `resource_nodes` HashMap into entities (cleanup)          | ⏳ pending  |

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

## Phase 6 — Delete `WorldSnapshot`, save-version bump, cleanup

**Status**: ✅ done (Phase 6a) · 6b deferred

The wire-level `WorldSnapshot` broadcast is gone. Phase 6 landed in two
parts: 6a (this session) achieves the bandwidth win and removes the
feature flag; 6b (a follow-up phase) deletes the type itself and ends
the snapshot-shaped indirection.

### What landed in 6a

**Wire snapshot deleted.** Server side
([src/server.rs](src/server.rs)): the per-tick snapshot broadcast loop
and the `info_span!("snapshot_broadcast")` are gone. The pre-existing
`perf_stats_for` 1 Hz broadcast is unaffected. Protocol
([src/protocol.rs](src/protocol.rs)): `ServerMessage::Snapshot(WorldSnapshot)`
variant deleted, and the `PacketDelivery::Unreliable` mapping it shared
collapses to the remaining variants.

**Synthesised local snapshot.** New module
[src/app/systems/snapshot_synth.rs](src/app/systems/snapshot_synth.rs).
A single Bevy system, registered in `ClientSystemSet::SnapshotSynth`
between `Network` and `ToolSwap`, rebuilds `ClientRuntime::snapshot`
every frame from the Lightyear-replicated entity queries (resource
nodes, dropped items, deployables, players + their public/private
split). The synthetic `tick` is a monotonic `Local<u64>`. Consumers
(`apply_resource_nodes_system`, `apply_dropped_items_system`,
`apply_deployed_entities_system`, `apply_snapshot_system`, plus the
inventory / crafting / furnace UIs, pickup raycast, world-grid
rebuild) keep reading `runtime.snapshot` — the system above is the only
writer now.

**`PlayerPrivate.open_furnace` carries `OpenFurnaceView`.** Was
`Option<DeployedEntityId>`; now `Option<OpenFurnaceView>`. The server's
`players_iter` populates the full view via
`open_furnace_view_for(client_id)` so the owner-side furnace UI works
without a separate wire message. Privacy is preserved because
`PlayerPrivate` only replicates to the owner.

**Feature flag retired.** `replicated-nodes` is gone from `Cargo.toml`.
The four feature-gated collectors (`collect_resource_node_states`,
`collect_dropped_items`, `collect_deployed_entities`,
`collect_remote_players`) collapsed to a single snapshot-reading branch
each — and the snapshot is now the locally-synthesised one. The
`apply_snapshot_spawns_updates_and_removes_remote_players` unit test
that was gated off in Phase 5 is re-enabled (it manually builds a
snapshot in the test rig, which is unaffected by the synth system).

**Save version bump.** [src/save/format.rs](src/save/format.rs):
`SAVE_FORMAT_VERSION` 7 → 8. The on-disk shape is identical; the bump
invalidates v7 worlds so the replication-bootstrapped grid doesn't
inherit apply-order subtleties from the old snapshot-bootstrapped one.

**Stale apply paths removed.** `ClientRuntime::apply_message` no longer
has a `Snapshot` arm (deleted). `is_stale_snapshot` helper deleted. The
three legacy snapshot-driven unit tests
(`snapshots_do_not_overwrite_existing_local_prediction`,
`snapshots_do_not_seed_local_prediction_after_welcome`,
`stale_snapshots_are_ignored`) are deleted with the apply path. The
older `singleplayer_session_receives_authoritative_snapshots_from_loopback_host`
network test (which only verified that the wire variant arrived) is
also deleted. 462 tests pass on `./cli test`.

### Files touched

- [Cargo.toml](Cargo.toml) — `replicated-nodes` feature removed
- [src/protocol.rs](src/protocol.rs) — `ServerMessage::Snapshot` variant deleted; delivery match updated
- [src/server.rs](src/server.rs) — per-tick snapshot broadcast deleted;
  `players_iter` now populates `PlayerPrivate.open_furnace` via
  `open_furnace_view_for`
- [src/server/player_ecs.rs](src/server/player_ecs.rs) —
  `PlayerPrivate.open_furnace` is now `Option<OpenFurnaceView>`
- [src/save/format.rs](src/save/format.rs) — `SAVE_FORMAT_VERSION` bumped to 8
- [src/app/state/runtime.rs](src/app/state/runtime.rs) — `Snapshot`
  apply-message arm deleted; `is_stale_snapshot` deleted;
  `rebuild_world_grid` and `resource_node_collider_set_version` exposed
  to the synth module
- [src/app/state/mod.rs](src/app/state/mod.rs) — `runtime` module
  visibility lifted to `pub(crate)` for synth access
- [src/app/state/tests.rs](src/app/state/tests.rs) — three
  snapshot-apply unit tests removed
- [src/app/systems.rs](src/app/systems.rs) — `snapshot_synth` module
  registered; new `ClientSystemSet::SnapshotSynth` variant added
- [src/app/systems/snapshot_synth.rs](src/app/systems/snapshot_synth.rs)
  — new file; the synth system itself
- [src/app.rs](src/app.rs) — synth set inserted between `Network` and
  `ToolSwap`; system registered
- [src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs),
  [src/app/systems/items/dropped.rs](src/app/systems/items/dropped.rs),
  [src/app/systems/deployables.rs](src/app/systems/deployables.rs),
  [src/app/systems/players.rs](src/app/systems/players.rs) — feature-flag
  blocks collapsed; the collectors read from synthesized snapshot
- [src/net/tests.rs](src/net/tests.rs) — wire-snapshot test deleted

### Key design decisions

- **Synthesised snapshot vs. consumer rewrite.** The migration doc's
  original goal was "consumers read from ECS queries against
  replicated entities". That would have rewritten the inventory UI,
  crafting UI, furnace UI, pickup raycast, hud, camera-follow, and
  the world-grid rebuild path in a single session — too risky. Synth
  preserves the consumer shape and ships the bandwidth win
  immediately; the consumer rewrite becomes a follow-up (6b) that
  doesn't block any further phases.
- **`PlayerPrivate.open_furnace` carries `OpenFurnaceView`.** This is
  the one place where a strict "components only carry their own
  state" reading would have forced a new server-pushed wire message
  for the smelt timer. Embedding the full view in `PlayerPrivate`
  keeps the data flow simple at the cost of pushing the furnace
  view's ~30 bytes every tick the smelt timer changes — only to the
  owner.
- **Tests for the deleted apply-message arm are deleted, not ported.**
  The behaviour they tested (stale-tick suppression, lazy
  prediction seeding from a fresh snapshot) belongs to the gone wire
  path; the equivalent under replication is "the replicated
  components are the source of truth" and is tested by the
  integration tests in `src/net/tests.rs`.

### Phase 6b — deferred follow-up

The remaining doc-stated work that didn't fit this session, captured
here so a fresh chat can pick it up:

1. **Delete `WorldSnapshot` type entirely.** The synth uses it as the
   container shape because all the consumers read from
   `ClientRuntime::snapshot`. A 6b pass replaces the consumers
   (one by one) with `Query<...>` based readers, then drops the
   `WorldSnapshot` type and the `snapshot()` / `snapshot_for` /
   `snapshot_inner` helpers on the server (these stay alive for now
   because the server tests still use `server.snapshot()` as a
   read-only accessor).
2. **Replace `Welcome.snapshot` with a lean local-player seed.** The
   Welcome message still carries a full `WorldSnapshot` for
   prediction seeding (`seed_local_prediction_from_snapshot`).
   Replace with a `PlayerSpawnSeed { position, yaw, pitch, health,
   last_processed_input }` once the consumers don't need the
   bootstrap snapshot.
3. **Direct ECS readers for inventory / crafting / furnace UI.** The
   UI currently goes `runtime.local_player().and_then(inventory)`.
   Phase 6b adds a `LocalPlayerComponents` system param or resource
   that pulls the data straight from the local
   `PlayerPublic + PlayerPrivate` entity.
4. **Pickup raycast direct queries.** `pickup.rs` reads
   `snapshot.resource_nodes` and `snapshot.deployed_entities` for
   target selection — repointing to `Query<&ResourceNode>` /
   `Query<&Deployable>` is a small focused change.
5. **Server-side test refactor.** `src/server/tests/*` use
   `server.snapshot()` as a state accessor. Replace with direct
   `GameServer::*_iter()` calls or thin test-only helpers; then
   delete `snapshot_for` / `snapshot` / `snapshot_inner` on the
   server.

### Verification

- `./cli check`, `./cli test`, `./cli lint` clean (462 tests pass).
- `cargo test --lib` clean.
- Existing `.save` files older than v8 fail to load (expected).
- Manual MP soak still pending: 2-client world, both players drop
  items / mine nodes / open furnaces; the wire is silent on entity
  state between chunk crossings.

---

## Phase 1b — Fold `resource_nodes` HashMap into entities (cleanup)

**Status**: ⏳ pending · best done after Phase 4 is proven

### Goal

Remove the `resource_nodes: HashMap<ResourceNodeId, ResourceNodeState>` field
from `GameServer`. The ECS entities (alive since Phase 1) become authoritative
on their own.

### Why deferred

The mirror approach (Phase 1) ships in one session by keeping the HashMap
authoritative. Flipping ownership cleanly requires changing every site that
reads or mutates `self.resource_nodes` — roughly:

- [src/server.rs](src/server.rs) — init load + regrow insert
- [src/server/resource_nodes.rs](src/server/resource_nodes.rs) — gather (3 sites)
- [src/server/persistence.rs](src/server/persistence.rs) — save build
- [src/server/commands.rs](src/server/commands.rs) — admin spawn
- [src/server/inventory.rs](src/server/inventory.rs) — pickup payouts (3 sites)
- [src/server/connection.rs](src/server/connection.rs) — snapshot read (if Phase 6 hasn't already removed it)
- [src/server/tests/](src/server/tests/) — ~15 direct field touches across `commands.rs` and `resource_nodes.rs`

These methods are on `&mut self GameServer`. Conversion requires either
threading `&mut World` through them or using `world.resource_scope` at the
Bevy system entry point. See the "Required surgery" section of Phase 3 — same
pattern.

### Approach

1. Each method that touches `self.resource_nodes` gains a `world: &mut World`
   parameter (or a more focused `Commands` + `Query` pair).
2. Bevy system call sites (in `tick_authoritative_server`,
   `receive_client_messages`, etc.) wrap in
   `world.resource_scope::<AuthoritativeServer, _>(|world, mut server| { ... })`
   so both the resource and the world are accessible.
3. **TestFixture pattern** to minimize test churn: introduce
   `struct TestFixture { world: World, server: GameServer }` in
   [src/server/tests/mod.rs](src/server/tests/mod.rs) that wraps `GameServer`
   methods, hiding the `&mut World` parameter. Tests change from
   `server.resource_nodes.clear()` to `fixture.clear_resource_nodes()` —
   same shape, different impl.
4. Save format change: serialize from query iteration instead of HashMap
   iteration. The Vec order may differ → bump `SAVE_FORMAT_VERSION`.
5. Remove the mirror sync system from
   [src/net/host.rs](src/net/host.rs) (`sync_resource_node_entities`) — no
   longer needed, entities are authoritative.

### Open / deferred

- **Should this happen before or after Phase 6?** They're independent. If
  Phase 6 already removed the snapshot's resource node consumer, only the
  gather / pickup / admin paths still touch `resource_nodes`. Slightly
  smaller diff. **Recommendation: do this after Phase 6** so the snapshot
  path doesn't need touching here.
- The analogous cleanups for dropped items, deployables, players (let's
  call them **Phase 2b**) are even bigger and have similar shapes. Track
  them as separate phases when we get there.

### Verification

- `./cli check && ./cli test && ./cli lint` clean
- Save a world, reload it — content matches (modulo Vec order if that's
  what triggered the save-version bump).

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
