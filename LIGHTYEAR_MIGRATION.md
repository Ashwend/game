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
2. **Read the ⚠️ Lightyear known issue section** — it documents a
   subtle bug class that bit us hard during Phase 6 and influences
   every future change to replicated state. Future sessions will hit
   it again if they don't know about it.
3. Read the **Phase index** for status.
4. Jump to the phase you're going to execute. Each phase section is
   self-contained (key files, design decisions, open items).
5. Glance at **Cross-phase reference** for the entity-type table and glossary.

### Migration status (May 2026)

**Phases 0 through 6.6 are all complete and verified in-game.** The
custom `WorldSnapshot` broadcast is gone; every replicated entity
type (resource nodes, dropped items, deployables, players) flows
through Lightyear's per-component replication with chunk-room
visibility. State changes that Lightyear's post-spawn diff path
drops (a known Lightyear 0.26.4 bug — see the warning section) are
routed through reliable `ServerMessage` side-channels: see
`ResourceNodeStorageChanged` and `DeployableHealthChanged`.

**Only Phase 7 (post-migration audit) is pending.** It's a
cleanup pass — no behaviour change — focused on removing the
dead types, comments, and helpers the migration left behind.

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

## ⚠️ Lightyear 0.26.4 known issue: post-spawn component diffs can be silently dropped

**READ THIS BEFORE ADDING ANY NEW REPLICATED STATE.**

### Symptom

A server-side `Changed<T>` on a replicated component never reaches the
client. The initial entity spawn delivers the component value just
fine; later mutations to that same component on that same entity are
invisible to the client. Confirmed empirically with the
`replication-trace` feature: `server: <Component> MUTATE` log lines
fire, matching `client: <Component> RECV` lines never do.

### Root cause (Lightyear, not us)

Lightyear's `SendUpdatesMode::SinceLastAck` (the default) gates each
component update on `is_changed(send_tick, this_run)` where `send_tick`
is **per `(sender, ReplicationGroupId)`** — not per `(sender, entity)`
or `(sender, component)`. See
`lightyear_replication-0.26.4/src/send/sender.rs:155-166` and
`src/send/buffer.rs:678`.

Lightyear 0.26.0's PR
[#1330](https://github.com/cBournhonesque/lightyear/pull/1330)
introduced a `DEFAULT_GROUP = ReplicationGroupId(0)`, so every
replicated entity in this codebase shares the same ack tick. A
frequently-updated entity in the group can advance the shared ack
past a slowly-changing entity's local `Changed` mark, and Lightyear
concludes "nothing new to send" for the slow entity even though it
just changed.

The underlying design issue
([#740](https://github.com/cBournhonesque/lightyear/issues/740)) was
filed by the maintainer himself in Dec 2024 and is still open. The
fix lives on `main` as
[PR #1361](https://github.com/cBournhonesque/lightyear/pull/1361),
which **replaces the entire replication subsystem with
`bevy_replicon`**. No crates.io release yet.

### Why we can't downgrade

| Version | Bevy compat | Has the bug? |
| ------- | ----------- | ------------ |
| 0.26.x  | 0.18        | Yes — and only line that supports Bevy 0.18 |
| 0.25.x  | 0.17        | Yes (root-cause #740 predates 0.26 by ~year) |
| ≤ 0.24  | 0.16 or older | Yes |

There's no version of Lightyear that's both Bevy-0.18-compatible and
without the bug.

### Why we can't patch locally

The entire replication subsystem is being deleted on `main`. Any
local fix would be throwaway work, and the replicon-based replacement
will land sooner or later.

### The workaround pattern

For components whose post-spawn updates fail to ship, route the
update through a **reliable `ServerMessage`** instead. Pattern, as
applied for `ResourceNodeStorage` in Phase 6.5:

1. Define a server message variant carrying the entity id + the new
   value (e.g.
   `ServerMessage::ResourceNodeStorageChanged { id, storage }`).
   Add it to `ServerMessage::delivery()`'s `Reliable` arm.
2. After the server-side mutation in `GameServer`, push a
   `ServerEnvelope` with `DeliveryTarget::Broadcast` (or AoI-filtered
   if bandwidth matters). See `src/server/resource_nodes.rs`'s
   `apply_gather_command` for an example.
3. In the client's `network_tick_system`, take a
   `Query<(&MarkerComponent, &mut ReplicatedComponent)>`, find the
   entity by id, and write the new value directly. See
   `src/app/systems/network.rs` for the live example.
4. Make `ClientRuntime::apply_message` a no-op for the new variant
   (the work happens in the network tick system, not the runtime
   state).

The replicated component itself stays registered for replication —
the initial spawn delivery still works fine, and downstream readers
(tooltips, queries) keep using the ECS source of truth. The reliable
message is purely a patch on the post-spawn diff path.

### When to use the workaround

Use the reliable side-channel when **all** of these are true:

- The component is server-authoritative.
- The component is shipped through Lightyear replication.
- The component mutates after spawn.
- The mutations are infrequent or event-driven (gather impulses,
  inventory transactions, structure damage). One missed diff stays
  visible indefinitely because there's no follow-up update to
  re-converge.

You probably **don't** need the workaround for components that
mutate every tick (movement, smelt fractions, animations): the
flood of `Changed<T>` ticks masks individual dropouts and the client
converges within ~50 ms.

### Currently active workarounds in tree

- `ResourceNodeStorage` — reliable
  `ServerMessage::ResourceNodeStorageChanged` (Phase 6.5).
- `DeployableHealth` — reliable
  `ServerMessage::DeployableHealthChanged` (Phase 6.6). Surfaced
  immediately after Phase 6.6 deleted the snapshot wire that had
  been masking the drop. Symptom was "axe on furnace takes 2-3
  swings to update HP nameplate". Same pattern as the resource
  node storage workaround.
- `DeployableActive` — always-write at the server mirror so
  `Changed<T>` fires every tick (Phase 6.3 escape hatch). Less
  principled than the reliable-message pattern but works for a
  hot-toggling bool; consider migrating to the reliable-message
  pattern if it ever drops a transition.

### Suspected-but-untested at risk

These mutate post-spawn through replication and could exhibit the
same drop pattern; nobody has hit them yet but the next person who
adds gameplay touching them should plan for it:

- `PlayerPrivate.crafting` — only updates on craft enqueue/finish.
  Currently masked by the same `last_processed_input` field
  changing every tick alongside it, but verify after any future
  refactor.

### Long-term direction

Lightyear's `main` branch is migrating to `bevy_replicon`
([PR #1361](https://github.com/cBournhonesque/lightyear/pull/1361),
[PR #1486](https://github.com/cBournhonesque/lightyear/pulls)). When
that ships as a release, this whole class of bug should vanish and
we can revisit dropping the side-channel messages. Until then, treat
**reliable side-channel for slow-changing replicated state** as the
default pattern for this codebase.

### Diagnosing future occurrences

Add a `client: <Component> RECV` trace under the
`replication-trace` feature (see
`src/app/systems/replication_trace.rs`) and a matching
`server: <Component> MUTATE` trace in the mirror sync system in
`src/net/host.rs`. Reproduce the gameplay action that mutates the
component. If MUTATE fires but RECV doesn't, you're in this bug
class — apply the reliable-message workaround.

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
| 6.1 | Migrate `PlayerPrivate` consumers (inv/craft/furnace UI)       | ✅ done     |
| 6.2 | Migrate `PlayerPublic` consumers (peer avatars)                | ✅ done     |
| 6.3 | Migrate `Deployable` consumers                                 | ✅ done     |
| 6.4 | Migrate `DroppedItem` consumers                                | ✅ done     |
| 6.5 | Migrate `ResourceNode` consumers                               | ✅ done     |
| 6.6 | Delete the snapshot path                                       | ✅ done     |
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

**Status**: ✅ done

**What landed:**

- [src/server/player_ecs.rs](src/server/player_ecs.rs) — widened
  `PlayerPrivate.open_furnace` from `Option<DeployedEntityId>` to
  `Option<OpenFurnaceView>` so the furnace UI gets the whole view
  (slots + smelt/fuel progress) from one replicated component.
- [src/server.rs](src/server.rs) — `players_iter` now calls
  `self.open_furnace_view_for(client_id)` so the mirror writes the
  full view into the replicated component each tick.
- [src/app/state/local_player.rs](src/app/state/local_player.rs) —
  new `LocalPlayerState` resource (cached `(Entity, PlayerPublic,
  PlayerPrivate)`). Refreshed once per frame by
  `update_local_player_state_system` scanning `Query<(Entity,
  &Player, &PlayerPublic, Option<&PlayerPrivate>)>` for whichever
  entity matches `ClientRuntime::client_id`.
- New `ClientSystemSet::LocalPlayerSync` runs the cache refresh at
  the very start of `Update`, ahead of every consumer.
- Consumers retargeted:
  - [src/app/ui/inventory.rs](src/app/ui/inventory.rs),
    [src/app/ui/crafting.rs](src/app/ui/crafting.rs),
    [src/app/ui/furnace.rs](src/app/ui/furnace.rs) now read from
    `LocalPlayerState`. `inventory_ui` no longer takes `&mut
    ClientRuntime`.
  - [src/app/systems/input/menu_toggles.rs](src/app/systems/input/menu_toggles.rs)
    `sync_furnace_open_flag_system` reads
    `local_player.private.open_furnace.is_some()`.
  - [src/app/systems/items/tool_swap.rs](src/app/systems/items/tool_swap.rs)
    reads `local_player.private.inventory.active_actionbar_stack()`.

**Not yet removed** (intentionally — defer to Phase 6.6 so the diff
is small and the snapshot wire keeps the other consumers feeding
until they migrate): the wire `PlayerState.inventory`,
`PlayerState.crafting`, `PlayerState.open_furnace` fields. Server
still populates them; nothing reads them.

**Verified in-game:** inventory + actionbar render and update,
crafting queue HUD ticks, furnace fuel/smelt bars animate, tool-swap
animation fires, ESC closes the furnace modal.

**Bandwidth note:** `OpenFurnaceView.smelt_fraction` /
`fuel_fraction` change every tick while the furnace is running, so
`PlayerPrivate` ships a diff every tick under that condition.
Same as the legacy snapshot. Phase 7 can think about rounding /
fewer significant bits if it matters.

#### Phase 6.2 — `PlayerPublic` (peer avatars)

**Status**: ✅ done

**What landed:**

- [src/app/systems/players.rs](src/app/systems/players.rs)
  `apply_snapshot_system` now iterates the replicated `Query<(&Player,
  Ref<PlayerPublic>)>` directly — no more `WorldSnapshot` fallback or
  `replicated-nodes` cfg branches. Disconnect tear-down is keyed on
  `runtime.client_id.is_none()`. The snapshot-driven unit test was
  deleted; integration coverage lives in `src/net/tests.rs`.
- [src/app/state/runtime.rs](src/app/state/runtime.rs) — `local_view`
  and `local_player_position` drop the snapshot fallback. Until
  Welcome seeds `predicted_local`, both return `None`. Two test
  fixtures (`local_view_falls_back_to_snapshot_when_prediction_is_missing`
  → renamed `local_view_is_none_without_prediction`, plus the HUD
  render test) updated accordingly.
- [src/app/ui/peer_overlay.rs](src/app/ui/peer_overlay.rs) — overlay
  entries now borrow `&PlayerPublic` instead of `&PlayerState`.
  `collect_peer_overlay_entries` takes a replicated query iterator,
  and `PeerOverlayParams` gained a `Query<(&Player, &PlayerPublic)>`.

**Verified in-game:** in 2-client MP, peer capsule + nameplate + chat
bubble render; AoI ring controls visibility; local HUD continues to
read off `predicted_local`. PvP isn't implemented (no melee/fall
damage), so the health-bar tick on a peer wasn't directly tested —
the field replicates fine and renders at full HP.

#### Phase 6.3 — `Deployable` + family

**Status**: ✅ done

**What landed:**

- [src/app/systems/deployables.rs](src/app/systems/deployables.rs)
  `apply_deployed_entities_system` iterates `Query<(&Deployable,
  &DeployableTransform, &DeployableHealth, &DeployableActive)>`
  directly — no more `replicated-nodes` cfg gate or
  `collect_deployed_entities` indirection. Tear-down keys on
  `runtime.client_id.is_none()`. `current_deployable` now reads
  the active actionbar stack from `LocalPlayerState.private`.
- New `maintain_world_grid_system` (in the same file) runs in a
  new `ClientSystemSet::WorldGridRebuild` set right after
  `Network`. It fingerprints `(world_version,
  resource_node_collider_set_version,
  deployable_set_fingerprint)` via `Local<Option<(u64, u64,
  u64)>>` and rebuilds `ClientRuntime::world_grid` only when one
  of them changes. Removes the in-`apply_message` rebuild calls
  and the `resource_node_collider_version` field from
  `ClientRuntime`.
- [src/app/state/runtime.rs](src/app/state/runtime.rs)
  `rebuild_world_grid` now takes
  `deployable_colliders: impl IntoIterator<Item = WorldBlock>`
  and combines them with the snapshot's resource-node colliders.
  The deployable-side `deployable_collider` helper moved to
  `deployables.rs` and takes `&Deployable + &DeployableTransform`.
- [src/app/systems/items/pickup.rs](src/app/systems/items/pickup.rs)
  `update_pickup_target_system`'s deployable branch took on a
  `Query<(&Deployable, &DeployableTransform)>` and dropped the
  `DeployedEntityState` references.
  `best_deployable_target` / `deployable_aim_point` /
  `set_deployable_pickup_target` now take replicated components.

**Mid-phase bug found & worked around:**

`DeployableActive` diffs were getting dropped under rapid
furnace on/off toggling. The `replication-trace` server log
showed `MUTATE true→false` followed by `MUTATE false→true` but
the client only received one of every ~5 transitions, leaving
the furnace mouth light visually stuck. Workaround in
`sync_deployable_entities` ([src/net/host.rs](src/net/host.rs)):
write `DeployableActive` *unconditionally* every mirror pass,
so Bevy's `Changed<T>` fires every tick and Lightyear keeps
re-shipping the current value until the client acks. The
trace log still only fires on a real value flip so the
diagnostic stays readable. Bandwidth cost: ~1 byte/tick per
deployable (~20 bytes/sec at 20 Hz) — trivial.

The underlying Lightyear delta-replication race is something to
revisit in Phase 7 or report upstream — for now, this
workaround keeps the convergence guarantee where it matters.

**Verified in-game:** workbench/furnace placement visuals,
collision against placed structures (via the new maintain
system), furnace light toggling under repeated rapid clicks,
deployable pickup tooltip + interact, AoI ring despawn/respawn
all behave correctly.

**Pre-existing issue surfaced (not a 6.3 regression):**
Hatchet swings on a furnace play the "miss whoosh" because
`impact_sound_for(Axe, Stone)` returns `None` in
[src/app/audio/manifest.rs](src/app/audio/manifest.rs) — no
dedicated audio clip for that (tool, surface) combination.
The swing damage lands correctly. Same gap exists for
`(Pickaxe, Wood)` on a workbench. Worth filling in but
unrelated to the replication migration.

#### Phase 6.4 — `DroppedItem`

**Status**: ✅ done

**What landed:**

- [src/app/systems/items/dropped.rs](src/app/systems/items/dropped.rs)
  `apply_dropped_items_system` iterates
  `Query<(&DroppedItem, Ref<DroppedItemTransform>)>` directly. The
  `collect_dropped_items` indirection (and its `replicated-nodes`
  cfg gate) is gone. Tear-down keys on `runtime.client_id.is_none()`.
  Per-id retarget tick comes from `Ref::last_changed().get() as u64`
  so interpolation only restarts on real transform changes. The
  visual transform is built from `DroppedItemTransform` directly via
  a new `dropped_item_transform_from` helper (replaces the
  `DroppedWorldItem`-shaped one).
- [src/items.rs](src/items.rs) gained a position-keyed
  `pickup_score_at_position(eye, yaw, pitch, position)` so callers
  iterating replicated `DroppedItemTransform` don't have to
  materialise a `DroppedWorldItem`. `pickup_score` is now a thin
  shim that calls it.
- [src/app/systems/items/pickup.rs](src/app/systems/items/pickup.rs)
  `update_pickup_target_system` took on a
  `Query<(&DroppedItem, &DroppedItemTransform)>` and the dropped-
  item branch scores against the replicated position via the new
  helper. `set_dropped_pickup_target` now takes `&DroppedItem +
  &DroppedItemTransform`; it still prefers the visual entity's
  interpolated transform for the tooltip anchor and falls back to
  the authoritative replicated position only when the visual
  hasn't been spawned yet (rate-limited spawn budget).

**Verified in-game:** drops appear and settle, large-burst spawns
drain over a few frames as expected, pickup tooltip + E pick work,
AoI ring despawn/respawn behaves, and the multi-target priority
(dropped item vs. resource node vs. deployable) still picks the
closest one along the ray.

#### Phase 6.5 — `ResourceNode`

**Status**: ✅ done

**What landed:**

- [src/app/systems/items/resource_nodes.rs](src/app/systems/items/resource_nodes.rs)
  `apply_resource_nodes_system` iterates
  `Query<(&ResourceNode, &ResourceNodeStorage)>` directly. The
  `collect_resource_node_states` indirection + `replicated-nodes`
  cfg gate are gone. Tear-down keys on `runtime.client_id.is_none()`.
  The `respawn_progress` machinery turned out to be dead code (the
  server set it to `None` everywhere) so the `classify` /
  `commit_progress` helpers and the `previous_progress` map were
  deleted; pop-in is now just "this id wasn't tracked last frame
  and we've already applied at least one reconciliation pass".
- [src/resources.rs](src/resources.rs) gained position-keyed
  variants `resource_node_anchor_for`, `resource_node_score_at`,
  and `resource_node_collider_at` so callers iterating replicated
  components don't have to materialise a `ResourceNodeState`.
- [src/app/systems/items/pickup.rs](src/app/systems/items/pickup.rs)
  `update_pickup_target_system` took on a
  `Query<(&ResourceNode, &ResourceNodeStorage)>`. The resource-node
  branch scores against the replicated position via the new helper
  and the tooltip reads storage directly off the replicated
  component.
- [src/app/systems/deployables.rs](src/app/systems/deployables.rs)
  `maintain_world_grid_system` now takes
  `Query<&ResourceNode>` too; its fingerprint covers
  `(world_version, resource_node_set_fingerprint,
  deployable_set_fingerprint)` and the rebuild combines both
  collider sources. The
  `runtime.rs::resource_node_collider_set_version` helper +
  `ClientRuntime::resource_node_collider_version` field were
  deleted; the system carries its own `Local<Option<…>>` cache.
- `rebuild_world_grid` now takes both
  `resource_node_colliders` and `deployable_colliders` parameters
  — no more snapshot reads.

**Mid-phase bugs found:**

1. **Tooltip never updated** — `ResourceNodeStorage` post-spawn
   diffs were getting silently dropped by Lightyear (initial
   spawn ships fine but subsequent gather decrements never reach
   the client). Verified with `replication-trace`: ~6 server
   MUTATE log lines per gather session, zero client RECV lines.
   See the "⚠️ Lightyear 0.26.4 known issue" section at the top
   of this file for the root-cause writeup.

   **Workaround:** new reliable
   `ServerMessage::ResourceNodeStorageChanged { id, storage }`.
   Server emits it after every gather; client's
   `network_tick_system` applies it directly to the replicated
   `ResourceNodeStorage` component. Replication still owns the
   initial-spawn delivery; the reliable channel just patches the
   post-spawn diff path.

2. **Death animation race** — `ServerMessage::ResourceNodeDepleted`
   (reliable channel) and the Lightyear entity-despawn diff
   (separate channel) race; depending on order the client either
   plays the death animation or silently despawns the visual.

   **Fix:** new `pending_depletion_check` grace map on
   `ResourceNodeEntities`. When a replicated entity vanishes
   without the depleted id already in
   `runtime.depleted_node_ids`, the visual is held for
   [`DEPLETION_GRACE_FRAMES`] (3 frames ≈ 50 ms). If the
   message lands in that window, death effect fires; otherwise
   it's treated as an AoI-leave and silently despawned.

**Tricky bit (resolved):** `ResourceNodeState.respawn_progress`
turned out to be dead — the server never sets it to `Some`. The
client's old "just_entered_regen" / "just_finished_regen"
classification logic was entirely unreachable code. Deleted with
the rest of the snapshot reads. Regrow flow is now: server
depletes → broadcast `ResourceNodeDepleted` → entity despawns →
chunk manager schedules regrow → fresh entity spawns later
(possibly different id) → arrives as a brand-new replicated
entity on the client → standard pop-in animation. No regen-state
component needed.

**Verified in-game:** gather tooltip updates with each swing,
trees fall and shatter on depletion, ores shatter, crude pickups
fire pickup burst, AoI ring controls visibility with proper
silent despawn on leave / pop-in on return.

#### Phase 6.6 — Cleanup: delete the snapshot path

**Status**: ✅ done

**What landed:**

- **Wire**: `WorldSnapshot` type deleted from
  [src/protocol.rs](src/protocol.rs). `ServerMessage::Snapshot`
  variant deleted. `PlayerState` trimmed to the prediction-seed
  shape (`client_id`, `position`, `velocity`, `yaw`, `pitch`,
  `health`, `grounded`, `last_processed_input`) — used by
  `Welcome.local_seed` and `ServerMessage::Correction` only.
- **Welcome**: `snapshot: WorldSnapshot` field replaced by
  `local_seed: PlayerState`. Server constructs it from the
  connecting client's controller; client's
  `apply_message::Welcome` calls the new `seed_local_prediction`
  helper. No more per-player iteration through a snapshot players
  list.
- **Server**: `GameServer::snapshot()`, `snapshot_for()`,
  `snapshot_inner()`, `deployed_entities_for_snapshot()`, and
  `DeployedEntity::to_state()` removed from
  [src/server.rs](src/server.rs) /
  [src/server/connection.rs](src/server/connection.rs) /
  [src/server/deployables.rs](src/server/deployables.rs). The
  per-tick snapshot broadcast loop in `GameServer::tick` is
  gone — `tick()` returns ~0 envelopes per server tick when
  nothing is happening.
- **Tracing**: `snapshot_broadcast`, `snapshot_inner`,
  `snapshot_players`, `snapshot_dropped_items`,
  `snapshot_resource_nodes`, `snapshot_deployables` spans
  removed alongside their methods. The Phase 0 spans that
  measured CPU on the snapshot hot path are now also dead — the
  remaining `server_tick` / `host_fixed_tick` / `route_envelopes`
  / mirror-sync spans stay (those still see traffic).
- **Constants**: `SNAPSHOT_BROADCAST_INTERVAL_TICKS` deleted from
  [src/server.rs](src/server.rs).
  `PERF_STATS_BROADCAST_INTERVAL_TICKS` kept — perf stats aren't
  entity state, they're an HUD-only broadcast on its own slow
  tick.
- **`ClientRuntime`**: `snapshot: Option<WorldSnapshot>` field
  deleted. `local_player()`, `is_stale_snapshot()`, and
  `seed_local_prediction_from_snapshot()` removed. New
  `seed_local_prediction(&PlayerState)` takes a Welcome seed
  directly.
- **Last live consumer migrated**: the deployable nameplate
  overlay
  ([src/app/ui/deployable_overlay.rs](src/app/ui/deployable_overlay.rs))
  was reading `runtime.snapshot.deployed_entities`. It now takes
  a `Query<(&Deployable, &DeployableHealth)>` via
  `DeployableOverlayParams.replicated` and pairs IDs against the
  visual `NetworkDeployedEntity` set the same way. The held-item
  visual system and the gameplay-inventory shortcut handlers
  also moved off `runtime.local_player()` onto
  `LocalPlayerState`.
- **`PlayerController::from_player_state`** stayed — same name,
  reads the trimmed `PlayerState`. No call-site churn beyond the
  field-set update.
- **Feature flag**: `replicated-nodes` deleted from
  [Cargo.toml](Cargo.toml). Every consumer is now unconditionally
  on the replicated path.
- **Tests**: 460 tests pass after the port (was 465 before; the
  delta is 5 snapshot-only tests deleted with one-line reason
  comments).

**Wire types kept** (still used as the server's internal
authoritative HashMap value type — they're no longer wire shapes
but rewriting their callers is Phase 7 work):
- `ResourceNodeState`
- `DroppedWorldItem`
- `DeployedEntityState`
- `OpenFurnaceView`

**Skipped intentionally:**
- **Save-format bump** — the save format never carried
  `WorldSnapshot` to begin with, so deleting the wire type left
  the on-disk shape unchanged.
- **`replication-trace` feature** — kept as the documented
  diagnostic for the Lightyear-bug pattern (see the ⚠️ section
  at the top of this file).

**Late-breaking discovery, fixed in-phase:** `DeployableHealth`
hit the Lightyear post-spawn-diff bug as soon as the snapshot
wire was gone (the snapshot had been masking it). Symptom:
hatchet on a furnace took 2-3 swings to update the HP nameplate
on the swinger's own screen. Fix: applied the same reliable
`ServerMessage::DeployableHealthChanged` workaround as
`ResourceNodeStorage`; server's
`apply_damage_deployable_command` broadcasts the new health
after each hit, client's `network_tick_system` writes it to the
replicated `DeployableHealth` component. Recorded in the
"Currently active workarounds" list of the known-issues section
above.

**Verification:** `./cli check` / `./cli test` (460 ok) /
`./cli lint` clean. `--features replication-trace` builds clean
too.

---

Below is the original Phase 6.6 punch list, preserved for
reference. Everything in it landed except the wire-type group
(2), which was scoped down to keeping server-internal types in
place; Phase 7 can move them out of `protocol.rs` if it wants:

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

**Status**: ⏳ pending — fits a single focused session

### Goal

The migration is functionally complete and verified in-game. Phase 7
is a single audit pass to remove the cruft Phase 6 left behind: dead
fields, stale comments, helper types whose only job was feeding the
deleted snapshot wire, and docs that still describe the old
architecture. **No behaviour change** — this is purely a hygiene pass.

### Before you start — context

- **Read the ⚠️ Lightyear known issue section at the top of this
  file first.** It explains the active workarounds, why they exist,
  and the rule for new replicated state. Phase 7 should not touch
  those workarounds — they're load-bearing.
- **The migration is verified.** All 460 tests pass, lint is clean,
  singleplayer + multiplayer were play-tested end-to-end. Do not
  rip out machinery that "looks suspicious" without confirming it's
  dead — the live grep below already filtered the obviously-dead
  parts.
- **Per-entity `ReplicationGroup` was considered but not applied.**
  The research from "Known issue" section's referenced PRs notes
  switching from the default `ReplicationGroupId(0)` to
  `ReplicationGroup::new_from_entity()` *might* fix the post-spawn-
  diff bug. If you want to experiment, do it in isolation as a
  follow-up — don't bundle it into the Phase 7 cleanup commit.

### Punch list (concrete, executable)

Each item below has file paths + line numbers. Surveyed at the end
of Phase 6 — re-grep if you find drift.

#### 1. Delete `ResourceNodeState.respawn_progress` (vestigial field)

[src/protocol.rs:594](src/protocol.rs) defines
`pub respawn_progress: Option<f32>` on `ResourceNodeState`. Survey
confirmed it's **set to `None` everywhere** (8 sites) and **only
read via `.is_some()` checks** (3 sites, all in dead branches):

Writes (all `None`):
- `src/resources.rs:295,515,544,573,590`
- `src/server/resource_node_ecs.rs:140`
- `src/server/tests/resource_nodes.rs:10`

Reads (all `is_some()` → always false):
- `src/resources.rs:332` — `resource_node_anchor()` guard
- `src/resources.rs:445` — `resource_storage_is_empty()` guard
- `src/server/resource_nodes.rs:32` — `apply_gather_command()` guard

Delete the field, drop the three guard branches, remove the "this
is vestigial" comment at `src/server/resource_node_ecs.rs:136-139`.

#### 2. Move `OpenFurnaceView` out of `src/protocol.rs`

It's no longer a wire type — only the client side uses it (the
furnace UI reconstructs it from `PlayerPrivate.open_furnace`
locally). [src/protocol.rs:416](src/protocol.rs).

The server still constructs it via
`server.open_furnace_view_for(client_id)` in
[src/server/furnace.rs](src/server/furnace.rs) to populate
`PlayerPrivate.open_furnace`. So it's still bidirectional state.
Decide: keep where it is (it IS still on the wire, embedded inside
PlayerPrivate), or move to a `src/app/state/` module if you split
the wire/local concerns more strictly. Probably the right call is
to leave it in `protocol.rs` and just add a comment that it's a
replicated-component field, not a top-level wire type.

#### 3. Keep `DroppedWorldItem`, `ResourceNodeState`,
       `DeployedEntityState` in `src/protocol.rs`

All three are still used as the server's authoritative HashMap value
types (`GameServer.dropped_items`, `.resource_nodes`,
`.deployed_entities`). They're no longer wire types but they live
in `protocol.rs` because the persisted save layer also uses them.
Phase 1b/2b would move these out by folding the HashMaps into ECS
entities; until then, **leave them in place** but add a docstring
to each clarifying that they are server-internal post-Phase-6.

#### 4. Keep `DeployableView` and `PlayerView`

[src/server/deployable_ecs.rs:105-114](src/server/deployable_ecs.rs)
and [src/server/player_ecs.rs:114-119](src/server/player_ecs.rs).
Both still actively feed the mirror sync in
[src/net/host.rs](src/net/host.rs) via
`GameServer::deployables_iter()` and `GameServer::players_iter()`.
Cheap adapters with no logic — defer removal to Phase 1b/2b.

#### 5. Add docstring to `PlayerController::from_player_state`

[src/controller/mod.rs:86](src/controller/mod.rs). Missing one.
Suggested:

```rust
/// Seed a `PlayerController` from a `PlayerState`. Used at
/// `ServerMessage::Welcome` time (prediction bootstrap from
/// `local_seed`) and when applying a `ServerMessage::Correction`.
```

`PlayerState` itself doesn't need renaming — its trimmed shape
post-Phase-6 is "the prediction-seed view of the player's kinematic
state", which the existing name describes adequately.

#### 6. Update stale comments referencing the snapshot path

These survived Phase 6 and need updating to reference the
Lightyear replication path instead. Don't blow time on every
comment in tree — focus on these specific stragglers:

- `src/net/channels.rs:64` — comment says "riding `WorldSnapshot`".
  Update to describe per-component replication via rooms.
- `src/app/state/runtime.rs:427` — references "full
  `WorldSnapshot.players` list". Now reads `PlayerState` seed
  directly from Welcome.
- `src/app/state/local_player.rs:5` — refers to
  `runtime.snapshot.players[i]` path. Obsolete.
- `src/app/systems/players.rs:27` — "no `WorldSnapshot` dependency"
  is accurate but reads like a Phase-6-era brag; rephrase to just
  describe the Lightyear replication query.

Tests at `src/app/state/tests.rs:49,54` already have "Deleted: was
verifying ..." comments — those are fine as-is (they document why
the test is gone).

#### 7. Update `docs/*.md` to reflect the new architecture

Pre-Phase-6 paragraphs that still describe the snapshot wire:

- [docs/networking.md:8](docs/networking.md) — lists `Snapshot` as
  a `ServerMessage` variant. Remove it.
- [docs/networking.md:15](docs/networking.md) — mentions "snapshots"
  on `UnreliableChannel`. Remove; state now flows via Lightyear's
  internal replication channel, not user channels.
- [docs/architecture.md:18](docs/architecture.md) — lists snapshots
  as a `ReliableChannel` payload. Remove.
- [docs/architecture.md:19](docs/architecture.md) — "the snapshot
  builder asks which chunks does this player see". Rephrase as the
  Lightyear room-visibility filter.
- [docs/worlds-and-saves.md:27](docs/worlds-and-saves.md) — "the
  snapshot builder then collects all entities". Same rephrasing.
- [docs/ui-and-client.md:10](docs/ui-and-client.md) — lists
  `snapshots` as a `runtime.rs` field. The field is gone.

[CLAUDE.md](CLAUDE.md) is already clean — no snapshot references in
the architecture description.

#### 8. Add a "Lightyear known issue" pointer to CLAUDE.md or docs

The ⚠️ section at the top of this file documents the workaround
pattern future contributors need. Right now it only lives in this
migration doc, which a fresh session reads. Consider linking it
from `CLAUDE.md` (or moving it into a new
`docs/lightyear-known-issues.md`) so it survives this file
eventually being archived. The user explicitly asked for this at
session wrap-up.

#### 9. Optionally extend `replication-trace` coverage

[src/app/systems/replication_trace.rs](src/app/systems/replication_trace.rs)
currently logs `ResourceNodeStorage`, `DeployableHealth`,
`DeployableActive`. The same diagnostic would help if we ever hit
the post-spawn-diff bug for `DroppedItemTransform`, `PlayerPublic`,
or `PlayerPrivate`. Trivial to extend — add a similar query +
`Ref::is_changed()` log for each component. Server side mirrors
in `src/net/host.rs` already have matching MUTATE logs for these
where useful.

Not blocking Phase 7; only do this if you want to make the
diagnostic complete.

#### 10. Phase 1b / 2b reconsideration

Phase 6 didn't change the cost/benefit of folding the
`GameServer.resource_nodes` / `dropped_items` / `deployed_entities`
/ `clients` HashMaps into ECS entities. Still no runtime impact;
still ~8 production sites + ~30 test sites per HashMap. **Keep
deferred** unless a feature actually needs ECS-only queries on one
of these.

### Pre-flight checks (don't do, just verify)

Already verified as part of Phase 6 wrap-up — sanity-check before
starting:

- `SNAPSHOT_BROADCAST_INTERVAL_TICKS` — gone.
- `replicated-nodes` Cargo feature — gone.
- `WorldSnapshot` type — gone.
- `ServerMessage::Snapshot` variant — gone.
- `GameServer::snapshot()`, `snapshot_for()`, `snapshot_inner()` —
  gone.
- `ClientRuntime::snapshot`, `local_player()`,
  `is_stale_snapshot()`, `seed_local_prediction_from_snapshot()` —
  gone.
- `SAVE_FORMAT_VERSION` — not bumped (the save layer never carried
  `WorldSnapshot`).
- `Welcome.local_seed: PlayerState` carries exactly the prediction-
  seed fields (no dead weight).

### Output

Phase 7 ends with:

- One commit removing the dead `respawn_progress` field, its
  guards, and the vestigial-marker comment.
- One commit updating the comments listed in item 6 and the docs
  listed in item 7.
- A short "Phase 7 findings" section appended below this one
  documenting what was deleted, what was kept and why (especially
  the wire-type group in items 3 and 4), and any follow-ups that
  didn't fit this session.

### Verification

- `./cli check && ./cli test && ./cli lint` clean.
- `cargo check --features replication-trace` clean.
- Spot-check `grep -rn 'WorldSnapshot\|ServerMessage::Snapshot' src/`
  returns zero hits (already verified at end of Phase 6, but worth
  re-running).
- Singleplayer launches and renders a familiar gameplay scene.

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

# Sanity check: no remaining snapshot references (should be empty after Phase 6)
grep -rn 'WorldSnapshot\|ServerMessage::Snapshot\|runtime\.snapshot' src/

# Every ClientSession call site
grep -rn 'ClientSession\|session\.send\|session\.tick' src/

# Find any post-spawn-diff workaround broadcasts (the reliable
# side-channels we use to work around the Lightyear bug). When adding
# new replicated state that mutates after spawn, follow this pattern.
grep -rn 'ServerMessage::.*Changed' src/protocol.rs

# Replication-trace coverage: extend if adding new replicated components
grep -rn 'replication_trace' src/
```

### How to run with the replication-trace diagnostic

```bash
RUST_LOG=replication_trace=info cargo run --features replication-trace -- client
```

Then reproduce the suspect gameplay action. Look for:
- `server: <Component> MUTATE` line — confirms the server's mirror
  wrote a new value.
- `client: <Component> RECV` line — confirms Lightyear delivered the
  diff. **No matching RECV after a MUTATE = the Lightyear bug;
  workaround needed.**
