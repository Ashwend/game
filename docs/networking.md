# Networking

Networking intentionally has one gameplay path with two bootstraps.

Shared runtime:
- `ClientSession` has one active variant: `ClientSession::Network(Box<LightyearGameSession>)`.
- Both local singleplayer and direct multiplayer send `ClientMessage` values over Lightyear channels (`Auth`, `Movement`, `Chat`, `Inventory`, `Gather`, `Voice`, `Heartbeat`, `Disconnect`).
- Both receive `ServerMessage` values from the same host wrapper (`Welcome`, `AuthRejected`, `VersionMismatch`, `Kicked`, `PlayerEvent`, `Correction`, `Chat`, `ItemMerged`, `Toast`, `ResourceImpact`, `ResourceNodeDepleted`, `WorldTime`, `Voice`, `Heartbeat`) and apply them through `ClientRuntime`. Per-entity authoritative state (resource nodes, dropped items, deployables, players) does **not** flow through `ServerMessage`, it flows through Lightyear's room-gated per-component replication. See the **Replication** section below.

## Version handshake

The netcode `protocol_id` (`LIGHTYEAR_PROTOCOL_ID` in `src/net/channels.rs`) is a **fixed constant**, deliberately *not* tied to `PROTOCOL_VERSION`. netcode rejects a mismatched `protocol_id` at the transport layer before any application message is exchanged, so tying it to the version would mean a version-bumped client gets bounced before it can ever learn *which* version the server runs. Keeping it fixed lets every connection reach the application-level `Auth` handshake.

Version checking therefore happens entirely in `GameServer::connect`: it compares the client's `protocol_version` (the app-level `PROTOCOL_VERSION`) and human-readable `client_version` (`GAME_VERSION`) against its own. A mismatch returns a typed `VersionMismatchRejection`, which `routing.rs` turns into `ServerMessage::VersionMismatch { server_version, server_protocol }`. The client pairs that with its own compiled-in `GAME_VERSION` to show a "you're newer/older" modal and disconnects gracefully back to the main menu (see `network_tick_system` and `NoticeDialog::version_mismatch`). Generic auth failures (bad ticket) still use `AuthRejected { reason }`.

Consequence: `ClientMessage::Auth`, `ServerMessage::AuthRejected`, and `ServerMessage::VersionMismatch` are the **stable handshake surface**, keep their wire shapes (and enum positions) stable so a future server can always tell an older client why it was turned away. Bump `PROTOCOL_VERSION` on any breaking wire change (mismatched builds are then rejected cleanly at `Auth`); bump `LIGHTYEAR_PROTOCOL_ID` only on a genuinely incompatible *transport* change.
- Each `ClientMessage`/`ServerMessage` declares its delivery preference via `PacketDelivery::Reliable`, `Unreliable`, or `UnreliableUnordered`; the protocol module maps that to the right Lightyear channel.
- `GameServer` owns the authoritative domain state for auth, players, movement state acceptance, inventory, dropped items, resource nodes, deployables, chat, voice routing, and save tick state. The ECS mirror entities (one per live id) carry the replicated components Lightyear ships to clients; the `HashMap`s on `GameServer` stay authoritative.
- Movement is client-authoritative by design. Clients send predicted `PlayerMovement` state; the server rejects stale or non-finite movement and writes the accepted pose onto the player's mirror entity so Lightyear replicates `PlayerPublic` to peers in the same chunk room. This keeps first-person movement responsive at the cost of stronger cheat resistance.

Channels (registered in `src/net/channels.rs`):
- `ReliableChannel` (`OrderedReliable`, priority 10): auth, chat, inventory, gather, kick, disconnect, depletion events, reliable side-channel state patches, anything where dropping or reordering would corrupt domain state.
- `UnreliableChannel` (`SequencedUnreliable`, priority 5): movement, corrections, heartbeats, resource impacts, world-time broadcasts. Sequenced because for these messages a newer value supersedes the older one, playing back a stale movement after a fresher one has arrived is worse than dropping it.
- `VoiceChannel` (`UnorderedUnreliable`, priority 8): voice frames only. Unordered because each Opus packet contains unique speech, dropping one because it arrived a few milliseconds late produces audible holes. Higher priority than other unreliable traffic so a busy replication or movement stream can't shoulder voice off the wire under load. See [Voice](voice.md) for the rest of the VOIP pipeline.
- Lightyear's internal replication channel (not registered here) carries entity spawns, despawns, and per-component diffs for replicated state. Room-gated to the player's AoI chunk ring, see the **Replication** section below.

## Replication

This is how every networked entity reaches the client. Read this before adding any new server-authoritative state.

### Architecture

Every replicated entity type (resource nodes, dropped items, deployables, players) lives in two places on the server:

1. **`HashMap` on `GameServer`**, the authoritative state. Gather, pickup, damage, movement, etc. all mutate the HashMap directly. This is the source of truth and the shape persisted to `WorldSave`.
2. **ECS mirror entity** in the host App's `World`. Carries the replicated components Lightyear ships (`ResourceNode` + `ResourceNodeStorage`, `Player` + `PlayerPublic` + `PlayerPrivate`, etc.) and a `*Chunk` anchor component pointing at its containing chunk.

An exclusive sync system per type (`sync_resource_node_entities`, `sync_player_entities`, …) in [src/net/host.rs](../src/net/host.rs) reconciles `HashMap → ECS` every Update: spawn entities for new ids, despawn entities for dropped ids, refresh the replicated components when the underlying value changed. Equality guards prevent spurious `Changed<T>` ticks so Lightyear only ships a diff when something actually moved.

**The resource-node sync is delta-driven, not a full walk.** A world can hold tens of thousands of live nodes, and reconciling all of them every tick cost ~100ms/tick (it pinned the host loop to ~10Hz). Instead, `GameServer` records the affected id whenever the node map mutates, every mutation goes through the `insert_resource_node` / `remove_resource_node` / `resource_node_state_mut` helpers, which push into a `node_sync_dirty` / `node_sync_removed` set (the constructor seeds `dirty` with all initial ids so the first sync still spawns everything once). `sync_resource_node_entities` drains those sets and processes only the per-tick delta, O(changed), not O(live nodes). If you add a new resource-node mutation site, it **must** go through those helpers or the mirror will silently go stale; the `replication-trace` feature is how you verify a `MUTATE` still pairs with a client `RECV`.

Each replicated entity gets its own `ReplicationGroup::new_from_entity()` at spawn (attached by `attach_room_gated_replication` / `attach_player_replication`). This is **load-bearing**, see the "Per-entity replication groups" section below for the bug it sidesteps.

Visibility is controlled by **rooms**. One Lightyear `Room` entity per `ChunkCoord` (lazily allocated, lives in `ChunkRoomMap`). When a mirror entity is spawned it triggers `RoomEvent::AddEntity` for its anchor chunk. When a client's AoI ring changes (player crossed a chunk boundary, view tier changed), `update_client_room_subscriptions` triggers `AddSender` / `RemoveSender` for just the boundary delta. Lightyear handles the rest: clients in a shared room receive spawns/despawns/diffs automatically.

Subscriptions use **spatial hysteresis** to stop boundary thrash. There are two radii: a chunk is *added* when it enters the load radius (`visible_chunks` = view tier + `LOAD_BUFFER_RINGS`) but only *removed* once it falls outside the wider keep radius (`retained_chunks` = load radius + `KEEP_MARGIN_RINGS`). Because the keep set is a strict superset of the add set, a player wobbling across a chunk boundary never crosses both thresholds, so no chunk loads → unloads → reloads (the churn that causes visible hitches). This is deterministic, no timer, and costs only the extra fringe rings' replication while the player lingers near an edge. `update_client_room_subscriptions` diffs the cached subscribed set against both radii each tick: subscribe `add − subscribed`, unsubscribe `subscribed − keep`.

The `replication-trace` Cargo feature (default off) adds `server: <Component> MUTATE` / `client: <Component> RECV` log lines for the load-bearing replicated components. Build with `--features replication-trace` and `RUST_LOG=replication_trace=info` to verify a mutation actually reaches the client.

### Player public / private split

A player's entity carries two payload components:

- **`PlayerPublic`**, name, pose, health, chat-bubble, admin flag. Visible to every client in the same chunk room (peers see your avatar and nameplate).
- **`PlayerPrivate`**, inventory, crafting queue, open-furnace view, last-input ack. Visible only to the owning client.

The split uses `ComponentReplicationOverrides<PlayerPrivate>` on the entity, configured `.disable_all().enable_for(owner_sender)`. The owner sender is captured at spawn (see `attach_player_replication` in `host.rs`); on a reconnect the mirror despawns and respawns, which recaptures the new sender.

### Per-entity replication groups, why every spawn must set one

**Read this before adding any new replicated state that mutates after the initial spawn.**

Lightyear 0.26.4 has a known upstream bug ([issue #740](https://github.com/cBournhonesque/lightyear/issues/740)) where post-spawn component diffs are silently dropped for slow-changing entities. The root cause: Lightyear's `SendUpdatesMode::SinceLastAck` (the default) gates each component update on a per-`(sender, ReplicationGroupId)` ack tick. The `DEFAULT_GROUP = ReplicationGroupId(0)` is shared by every entity that doesn't set its own group. A frequently-updated entity in the group can advance the shared ack past a slowly-changing entity's local `Changed` mark, after which Lightyear concludes "nothing new to send" for the slow entity even though it just changed.

**The fix in this codebase: every replicated entity gets its own `ReplicationGroup::new_from_entity()` at spawn.** That uses `Entity::to_bits()` as the group id, so each entity has its own ack tick and the shared-group race goes away. Empirically verified: with this in place, post-spawn `MUTATE` always pairs with a client-side `RECV` within ~3 ms, without it, the MUTATE→RECV pairing broke down for any entity not in the every-tick changeset.

Both spawn helpers in [src/net/host.rs](../src/net/host.rs), `attach_room_gated_replication` and `attach_player_replication`, already do this. **Don't add a new replication path that skips it.** A bare `Replicate::to_clients(...)` will pick up the default group and hit the bug.

The upstream fix is [PR #1361](https://github.com/cBournhonesque/lightyear/pull/1361), which replaces the entire replication subsystem with `bevy_replicon`. When that ships and we upgrade, the per-entity-group requirement may go away, but `new_from_entity()` is a safe choice regardless.

### Adding new replicated state, the procedure

1. **Define the component(s)** in the entity's `*_ecs.rs` module under `src/server/`. Derive `Component + Clone + PartialEq + Serialize + Deserialize`. Split mutable from immutable fields into separate components so per-component diffs stay cheap (see `ResourceNode` vs `ResourceNodeStorage`).
2. **Register the component** in [src/net/channels.rs](../src/net/channels.rs) via `app.register_component::<T>()` inside `LightyearProtocolPlugin::build`. Both server and client must register the same set.
3. **Wire the mirror sync** in [src/net/host.rs](../src/net/host.rs) so the `HashMap → ECS` reconciliation pass writes the new component and respects equality guards (no spurious `Changed<T>` ticks).
4. **Attach replication** at spawn via `attach_room_gated_replication` (static room-only entities) or `attach_player_replication` (player public/private split). Both helpers attach `ReplicationGroup::new_from_entity()`, don't bypass them.
5. **Consume on the client** with a `Query<&YourComponent>`, never via `ServerMessage`. Tear-down keys on `runtime.client_id.is_none()` (session ended).
6. **Add `replication-trace` coverage** for the new component in [src/app/systems/replication_trace.rs](../src/app/systems/replication_trace.rs) and a matching server-side MUTATE log in the mirror sync. Makes diagnosing the next dropout symptom a one-line check: `RUST_LOG=replication_trace=info cargo run --features replication-trace -- client`, reproduce the gameplay action, and confirm `MUTATE` pairs with `RECV` within a few ms.

### Reliability and recovery

The replication protocol is **eventually consistent with bounded convergence under loss**. We do not snapshot, we do not poll, we do not manually retry. The recovery behaviour below is what the protocol guarantees on its own, knowing it well is what lets us avoid reintroducing periodic state broadcasts.

**Room join (chunk crossing or fresh connect):** When a player walks into a chunk they didn't see before, `update_client_room_subscriptions` triggers `RoomEvent::AddSender(player_sender)` for that chunk's room. Lightyear's room plugin walks the entities in the room and calls `gain_visibility(sender)` on each, transitioning the per-sender visibility to `VisibilityState::Gained`. On the next replication tick, every `Gained` entity goes through the spawn code path again, Lightyear ships a fresh entity spawn message carrying **all currently-replicated components on that entity**, not a diff. The new observer is now caught up to the same state every prior observer sees. After that tick visibility flips to `Visible` and subsequent updates are diffs.

This is the catch-up mechanism. **No explicit "send me the room state" request is needed**, `RoomEvent::AddSender` is the trigger and Lightyear's spawn path is the payload.

**Dropped packets:** The `ReplicationSender` runs in `SendUpdatesMode::SinceLastAck` (the default). For each `ReplicationGroup` the server tracks the latest BevyTick the client acked. Every replication tick the server rebuilds the diff as "everything that changed in this group from `ack_tick` to `now`" and re-sends it. If a packet is lost the client doesn't ack; next tick the server builds a slightly larger diff covering the same window plus any new changes, and re-sends. The diff is **self-contained at every tick**, even four packets in a row can be lost and the fifth still carries the full delta from the last successful ack. Once the ack arrives, `ack_tick` advances and the next diff shrinks back.

The transport also surfaces NACKs when it detects a sequence-number gap. The replication layer resets the affected group's `send_tick` back to `ack_tick` so the next tick re-ships the unacked window without waiting for the ack timeout. See `ReplicationSender::handle_nacks` in `lightyear_replication`.

In practice with the `replication-trace` feature on a real loopback session: server `MUTATE` pairs with client `RECV` within ~3 ms. With realistic network loss (1-3 %) you'd occasionally see one missed tick followed by a slightly delayed `RECV`, visually imperceptible at the replication tick rate.

**Full disconnect and reconnect:** When the underlying Lightyear connection cycles through `Disconnected → Connected`, the `RoomPlugin::handle_disconnect` observer scrubs the dropped sender from every room it was in, and we drop our `ClientChunkSubs` entry for that client. On reconnect, `update_client_room_subscriptions` sees the empty subscription set and re-issues `AddSender` events for every chunk in the player's current AoI ring. Each chunk's entities go through the `Gained → spawn` path again. The client is fully repopulated within ~1 round trip of reconnect. No persistent-state catch-up logic is needed on our end.

**Failure-mode summary:**

| Failure | Recovery mechanism | Convergence |
| ------- | ----------------- | ----------- |
| Single dropped diff packet | Next replication tick re-ships unacked window | ~1 tick (50 ms) |
| Burst loss across multiple ticks | `SinceLastAck` window grows; every tick re-ships from last ack | ~1 RTT past the burst |
| Transport NACK | `send_tick` reset to `ack_tick`, immediate re-ship | ~1 tick |
| Chunk crossing into new room | `RoomEvent::AddSender` → `Gained` → fresh entity spawn with current state | Next replication tick |
| Full disconnect + reconnect | Room re-subscription on `Connected`, every entity re-spawned | ~1 round trip |
| Server-authoritative mutation while client has entity in view | `Changed<T>` → Lightyear diff → ack | ~3 ms observed |

**Why we don't add periodic state broadcasts.** Every reliability failure mode above is already handled by the protocol. A periodic full-state ship-out would (a) be redundant, the replication layer is already re-sending unacked state every tick, (b) reintroduce the bandwidth waste the `WorldSnapshot` deletion eliminated, and (c) bypass `Changed<T>` by writing to components even when they didn't change, defeating Lightyear's per-component delta replication. The only periodic broadcast in tree is `ServerMessage::WorldTime` (1× per minute), and that exists because it's a global *clock* the client extrapolates locally between broadcasts, not entity state.

**Special case, `ResourceNodeDepleted`:** Lightyear's entity-despawn alone can't tell the client apart "entity depleted (play death animation)" from "entity left my AoI ring (silent despawn)". We ship `ServerMessage::ResourceNodeDepleted` on the reliable channel as the disambiguator. The client's `pending_depletion_check` grace window pairs it with the matching Lightyear despawn. This is the one place in the codebase where a reliable message complements replication rather than replacing it, it's signalling intent, not patching dropped diffs.

### If you see a post-spawn diff dropout in the future

If `replication-trace` shows server `MUTATE` without a matching client `RECV`, the most likely cause is a replicated entity that didn't get `ReplicationGroup::new_from_entity()` at spawn. Check the spawn site, `attach_room_gated_replication` and `attach_player_replication` attach it; a bare `Replicate::to_clients(...)` does not.

If the spawn site is correct and the dropout persists, the fallback is the reliable side-channel pattern: emit a `ServerMessage::*Changed { id, new_value }` after the mutation, write it onto the replicated component on the client side in `network_tick_system`. We used this across the migration and it lives in git history if you ever need it, but the per-entity-group fix made it redundant for every case we have today, so try that first.

Singleplayer bootstrap:
- `ClientSession::start_singleplayer` loads a `WorldSave`, starts `spawn_loopback_server`, and connects the normal Lightyear client to the reserved loopback UDP address.
- The loopback host runs the same `run_host` code as a dedicated server, with `ServerSettings { auth_mode: NoAuth, singleplayer_host: Some(user.account_id) }`.
- Ephemeral loopback ports are reserved until the host thread performs its first update and reports startup, so the client is not handed an address before the host is ready to bind it.
- On shutdown, the client asks the local host for `world_save()` and persists it through `WorldStore`.

Multiplayer bootstrap:
- `./cli server --bind ... --auth ... [--world ...] [--admin-socket ...]` loads a world and calls `run_dedicated_server`, which delegates to the same `host::run_game_server`/`run_host` path.
- The multiplayer UI calls `ClientSession::connect(addr, user)` and uses the same client thread, message channels, replicated entity state, prediction, chat, and inventory flow as singleplayer.
- On graceful terminal shutdown (Ctrl-C or admin shutdown), the dedicated host returns its final `WorldSave`; `--world` saves back to that file, while the default dedicated world saves through `WorldStore`.

Admin socket (Unix only):
- `--admin-socket <path>` binds a Unix datagram-style stream socket alongside the dedicated server.
- `./cli admin --socket <path> announce <text...>` sends a `DedicatedAdminRequest::Announce` over the socket; the host broadcasts it as a server chat message.
- `./cli admin --socket <path> shutdown [--reason ...]` sends `DedicatedAdminRequest::Shutdown`; the host kicks all clients with the reason, persists the save, and exits.
- The request/response wire format is line-delimited JSON over the Unix socket; see `src/net/dedicated/admin.rs` for the schema.

Networking files:
- `src/net/client.rs`: client session API, Lightyear client app, auth send, command queue, incoming message queue, and local-host shutdown/persistence hook.
- `src/net/host.rs`: loopback host spawn, dedicated host run, Lightyear server app, shutdown, and fixed server ticking.
- `src/net/host/handle.rs`: host command handle, final-save request, and thread shutdown.
- `src/net/host/routing.rs`: unauthenticated/authenticated message handling, connection maps, and envelope routing.
- `src/net/host/admin.rs` (Unix only): admin socket listener and `DedicatedAdminRequest` dispatch.
- `src/net/protocol.rs`: Lightyear channel setup and delivery selection for shared protocol messages.
- `src/net/dedicated/mod.rs`: CLI-facing dedicated server wrapper.
- `src/net/dedicated/admin.rs`: admin request/response types and client helper used by `./cli admin`.

Do not reintroduce a direct in-process singleplayer transport or a singleplayer-only gameplay server. If a feature needs networking, add it to the shared protocol and `GameServer` flow so loopback singleplayer and remote multiplayer exercise the same code.

## Authentication

Identity is WorkOS-only. The `src/auth/` module owns it: `authenticate()` resolves a client's `ClientMessage::Auth` handshake into a `VerifiedIdentity { account_id, display_name }` according to the server's `AuthMode`:

- `AuthMode::Workos` (the default for any dedicated server): the client presents a WorkOS access-token JWT, which `WorkosVerifier` validates offline against the WorkOS JWKS (RS256, signature + expiry; issuer/audience checks stay off until confirmed against a live token). The `account_id` is a stable truncated-SHA-256 of the token's `sub`, so client and server agree on identity and the save format stays byte-compatible. No API key or secret is needed, only the public client id.
- `AuthMode::NoAuth` (loopback singleplayer + `./cli multiplayer-test`): the server trusts the client's claimed `account_id` and display name with no token check. **Localhost only**, never expose a `NoAuth` server to the network, or any client could claim any identity.

The desktop client drives a native browser login (RFC 8252: system browser + loopback redirect + PKCE) in `src/auth/workos/`; the short-lived access token rides the `Auth` handshake and the refresh token is kept in the OS keychain. WorkOS config (just the public `client_id`) resolves like analytics, baked-in build default → `workos.local.toml` → `GAME_WORKOS_*` env, via `WorkosConfig::load()`; CI bakes `GAME_WORKOS_CLIENT_ID` into release builds. `./cli multiplayer-test` injects each window's identity through `GAME_ACCOUNT_ID` / `GAME_PLAYER_NAME` and starts its server with `--auth no-auth`.
