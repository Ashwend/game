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
| 4   | Wire chunk rooms for resource nodes                            | ⏳ pending  |
| 5   | Chunk rooms for dropped items, deployables, players            | ⏳ pending  |
| 6   | Delete `WorldSnapshot`, save-version bump, cleanup             | ⏳ pending  |
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

**Status**: ⏳ pending · unblocked by Phase 3

### Goal

First real bandwidth win. Add Lightyear's `RoomPlugin`, lazy-allocate one
`RoomId` per `ChunkCoord`, put resource node entities in their chunk's room,
subscribe each client's sender entity to its visible-chunk rooms. Lightyear
delta-encodes component changes and auto-spawn/despawns entities on the client
when room membership changes.

Phase 3 left the Lightyear client entity in the main app's `World`, so
the consumer side of this phase (rewriting `apply_resource_nodes_system`)
can query replicated entities directly via `Query<&ResourceNode,
&ResourceNodeStorage>` instead of reading `ClientRuntime::snapshot`.

A/B against the existing `WorldSnapshot.resource_nodes` path under a feature
flag during the soak; Phase 6 removes the snapshot path once Phase 5 is in.

### Approach

1. **Plugin install** — add `RoomPlugin` to the server app in
   [src/net/host.rs](src/net/host.rs). Insert a `ChunkRoomMap` resource:
   `HashMap<ChunkCoord, RoomId>`, lazy-allocated via `RoomAllocator`.
2. **Mark entities `Replicate`** — when `sync_resource_node_entities` spawns
   a fresh entity, attach
   `Replicate::to_clients(NetworkTarget::None)` (no broadcast — visibility
   is by room) and `Rooms::single(chunk_room_id)`. Despawn removes it
   automatically.
3. **Client sender subscriptions** — when a client connects (the
   `handle_connected` observer in [src/net/host/routing.rs](src/net/host/routing.rs)
   or equivalent), set its initial `Rooms` based on
   `chunk_manager.visible_chunks(...)`. When the client's chunk changes
   (Movement message in [src/server.rs:262](src/server.rs:262)), recompute
   visible chunks and update the sender's `Rooms` component (add new, drop
   stale).
4. **Client-side wiring** — once Phase 3 has merged the client into the main
   app, replicated entities appear directly with `ResourceNode` +
   `ResourceNodeStorage` components. Rewrite
   [src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs)
   to query those components instead of reading
   `ClientRuntime::snapshot.resource_nodes`. Initially, run **both** paths
   under a feature flag (`replicated-nodes` or similar) so we can A/B.
5. **Snapshot path stays** — `WorldSnapshot` still ships resource nodes for
   the rollback case. Phase 6 removes them.

### Key design decisions

- **Lazy room allocation.** A 5×5 chunk world has 25 chunks. Allocating all
  rooms at startup is fine but wasteful for larger worlds. Lazy-allocate on
  first entity placement; the cost is negligible.
- **Per-tick room recompute or change-driven?** Lightyear's `Rooms`
  component is observed; adding/removing room ids triggers replication
  changes. Most efficient: **change-driven** — only update sender `Rooms`
  when the player's anchor chunk changes (already tracked by
  `chunk_manager.update_player_chunk`). Per-tick recompute would be wasteful.
- **A/B feature flag**: `replicated-nodes` Cargo feature, default off until
  soaked. Both paths run server-side (snapshot still ships nodes), only one
  is consumed client-side.

### Open / deferred

- **Reliability mode for replication.** Lightyear default is unreliable
  with retransmit-on-ack. Resource nodes are sparse and rarely change, so
  this is appropriate. No tuning needed initially; revisit if dropouts
  appear during soak.
- **Initial state delivery.** When a client first subscribes to a room,
  Lightyear sends the full snapshot for that room's entities — the bandwidth
  pattern is "burst on chunk crossing, near-zero between". That's better
  than the current 20 Hz full snapshot.

### Files touched (expected)

- [src/net/host.rs](src/net/host.rs)
- [src/server.rs](src/server.rs) — player movement → chunk update
- [src/server/resource_node_ecs.rs](src/server/resource_node_ecs.rs) — `Replicate` marker on spawn
- [src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs) — consumer rewrite (gated)
- [Cargo.toml](Cargo.toml) — new feature flag

### Verification

- Run a 2-client multiplayer test: walking out of chunk → resource nodes
  despawn on client; walking back → they reappear.
- Wireshark / pcap: total bytes/sec on the wire should drop sharply once
  Phase 6 removes the snapshot duplicate.
- 460+ tests pass throughout.

---

## Phase 5 — Chunk rooms for dropped items, deployables, players

**Status**: ⏳ pending · blocked by Phase 4

### Goal

Apply the same Phase 4 pattern to the remaining replicated entity types. The
infrastructure (`RoomPlugin`, `ChunkRoomMap`, sender subscriptions) already
exists from Phase 4 — only the per-entity-type wiring is new.

### Approach

- **Dropped items**: identical to resource nodes. `Replicate::to_clients(NetworkTarget::None)`
  + `Rooms::single(chunk_room_id)` on entity spawn in
  [src/server/dropped_item_ecs.rs](src/server/dropped_item_ecs.rs). Chunk
  membership updates from the existing
  `chunk_manager.update_dropped_item_chunk`.
- **Deployables**: same.
- **Players** — the trickier case. Two layers:
  - **`PlayerPublic`** replicates to **all clients in the room**:
    `Replicate::to_clients(NetworkTarget::AllExcept(owner))` —
    everyone except the owning client (who reads their own state from
    `PlayerController` prediction, not from the wire).
  - **`PlayerPrivate`** replicates only to the **owning client**:
    `Replicate::to_clients(NetworkTarget::Single(client_id))`. Inventory,
    crafting, open furnace — never visible to peers.
  - Both components live on the same player entity, so the entity is in
    the chunk room; Lightyear's per-component target shapes who actually
    gets it.

### Key design decisions

- **Two-component player replication.** This is exactly why we did the
  public/private split in Phase 2. The wire shape becomes "any peer in
  range sees `PlayerPublic`; only you see your `PlayerPrivate`". This is
  more efficient than the current snapshot path (which still serializes
  the full `PlayerState` per peer, with `inventory: None` for peers — but
  the wire is paying for the `None`).
- **Local player visibility.** The owning client's own `PlayerPublic`
  doesn't need to come over the wire — local prediction owns position. Use
  `NetworkTarget::AllExcept(owner)` so the owner skips the public state for
  themselves.

### Open / deferred

- **Voice frames** are not in the room/replication path. They stay on the
  unreliable `VoiceChannel` and are gated by distance check, not chunk
  membership. No change needed here.
- **Chat bubbles** are inside `PlayerPublic.chat_bubble` so they ride along
  with the public component automatically.

### Files touched (expected)

- [src/server/dropped_item_ecs.rs](src/server/dropped_item_ecs.rs)
- [src/server/deployable_ecs.rs](src/server/deployable_ecs.rs)
- [src/server/player_ecs.rs](src/server/player_ecs.rs)
- [src/app/systems/players.rs](src/app/systems/players.rs) — consumer for `PlayerPublic`
- [src/app/systems/items/dropped.rs](src/app/systems/items/dropped.rs) — consumer for `DroppedItem`
- [src/app/systems/deployables.rs](src/app/systems/deployables.rs) — consumer for `Deployable`
- Wherever `runtime.snapshot.players` / `dropped_items` / `deployed_entities`
  is read on the client.

### Verification

- Connect 2 clients in MP; one drops an item, the other walks into the
  chunk → drop appears.
- One deploys a workbench, peer enters chunk → workbench appears with
  correct health.
- Owner sees their own inventory; peer entry for the same player has no
  inventory data (verify with debug print).

---

## Phase 6 — Delete `WorldSnapshot`, save-version bump, cleanup

**Status**: ⏳ pending · blocked by Phase 5

### Goal

Remove the custom snapshot path entirely now that Lightyear replication owns
all networked entity state.

### Approach

1. **Delete `ServerMessage::Snapshot` variant** in
   [src/protocol.rs](src/protocol.rs). And the corresponding `Welcome` field
   if any. The `WorldSnapshot` type goes away.
2. **Delete the per-tick snapshot loop** in
   [src/server.rs:391](src/server.rs:391) and `snapshot_inner` /
   `snapshot_for` / `snapshot` methods in
   [src/server/connection.rs](src/server/connection.rs).
3. **Delete `ClientRuntime::snapshot`** in
   [src/app/state/runtime.rs](src/app/state/runtime.rs). Consumers now read
   from ECS queries against replicated entities.
4. **Save-version bump**: this phase will likely change the saved-state
   shape (resource nodes serialized via entity iteration vs the existing
   HashMap iteration, possibly different order). Bump
   `SAVE_FORMAT_VERSION` in [src/save/format.rs:44](src/save/format.rs:44).
5. **Tracing spans**: the Phase 0 spans inside `snapshot_inner` are now
   dead and should be removed.
6. **Feature flag removal**: the `replicated-nodes` flag from Phase 4 goes
   away — replication is the only path.

### Open / deferred

- **`PerfStats` message** stays. It's not entity state, it's a perf HUD
  payload.
- **`WorldTime` broadcast** stays. World time is a single global value, not
  per-entity, and broadcasting it 1 Hz over an unreliable channel is fine.
- **`Toast`, `ResourceImpact`, `ItemMerged`, etc.** — these are events, not
  state, and stay on the message channel.

### Verification

- Existing world saves no longer load (expected — version bumped). Manual
  test: delete `~/.local/share/.../*.save` (or wherever) and create fresh.
- All 460+ tests pass after fixture updates.
- Bandwidth check: a sitting-still client should be near-silent on the
  wire (just heartbeats + occasional WorldTime).

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
