---
title: Networking: transport, channels, handshake, message inventory
owns: The networking transport layer (one-path/two-bootstraps), the channel registry, the version/auth handshake, the admin Unix socket, and the ClientMessage/ServerMessage wire inventory.
when_to_read: Before adding a ClientMessage/ServerMessage variant, changing a channel, touching the auth/version handshake, or wiring the admin socket.
sources:
  - src/net/channels.rs - LightyearProtocolPlugin, channel registry, LIGHTYEAR_PROTOCOL_ID, private_key
  - src/net/client.rs - ClientSession (struct), start_singleplayer, connect
  - src/net/host.rs - spawn_loopback_server, run_game_server, run_host, tick_authoritative_server
  - src/net/host/routing.rs - handle_unauthenticated_message, message routing
  - src/protocol/mod.rs - PROTOCOL_VERSION, SERVER_TICK_RATE_HZ, GAME_VERSION
  - src/protocol/messages.rs - ClientMessage, ServerMessage, delivery()
  - src/protocol/world_map.rs - WorldMapMarker, WorldMapMarkerCommand
  - src/net/dedicated/admin.rs - DedicatedAdminRequest, DedicatedAdminResponse
related:
  - docs/replication.md - per-component replication mechanics, the host mirror, rooms/AoI, the #740 group fix
  - docs/server-authority.md - GameServer, ClientMessage handlers, tick subsystems
  - docs/chunks-and-aoi.md - chunk membership, AoI ring math, the room-subscription system
  - docs/movement.md - the client-authoritative movement trust boundary
  - docs/voice.md - the rest of the VOIP pipeline behind the Voice channel
  - docs/build-and-dev.md - ./cli server, ./cli admin, ./cli multiplayer-test
---

# Networking

> When to read this: before adding a `ClientMessage`/`ServerMessage` variant, changing a channel, touching the version/auth handshake, or wiring the admin socket. Source of truth: `src/net/channels.rs`, `src/net/host/routing.rs`, `src/protocol/messages.rs`, `src/net/dedicated/admin.rs`. Canonical invariants (singleplayer==multiplayer, replicated-state rules, never reintroduce a full-state broadcast) live in CLAUDE.md.

This doc owns the **transport half**: how bytes move, the channels, the handshake, and the message inventory. Per-component replication (the host mirror, replication groups, rooms/AoI, the catch-up mechanics) lives in `docs/replication.md`. The rule that decides which half a new piece of state belongs to is below in [Message inventory](#message-inventory): per-entity world state the client renders goes through replication; presence, clocks, and diagnostics go through a message.

Lightyear version is `0.26.4` (`Cargo.toml`, features `client, server, netcode, udp, replication`). Several upstream-bug workarounds in `docs/replication.md` are pinned to that exact version.

## One path, two bootstraps

There is **one** gameplay networking path. Both singleplayer and multiplayer are real Lightyear client sessions talking to a real Lightyear host over UDP; only the bootstrap differs. Do not add a direct in-process transport bypass or a singleplayer-only gameplay server (see the singleplayer/multiplayer invariant in CLAUDE.md).

`ClientSession` (`src/net/client.rs` - `ClientSession`, a plain struct, not an enum) holds the client network handle plus an optional `local_server` handle. Two constructors:

- **Singleplayer** (`ClientSession::start_singleplayer`): loads a `WorldSave`, calls `spawn_loopback_server` with `ServerSettings { auth_mode: AuthMode::NoAuth, singleplayer_host: Some(account_id) }` and an `AutoSaveSink` onto the same `WorldStore`, then `connect_inner` to the reserved loopback address. The ephemeral loopback port is not handed to the client until the host thread has performed its first update and reported startup, so the client never gets an address before the host can bind it. On shutdown (`ClientSession::shutdown`) the client asks the local host for its final `WorldSave` and persists it.
- **Multiplayer** (`ClientSession::connect`): just `connect_inner` to a remote `SocketAddr`, no local server.

Both constructors funnel through `connect_inner`, so message channels, replicated state, prediction, chat, and inventory are identical.

The host side is symmetric. `spawn_loopback_server` (singleplayer) and `run_game_server` (dedicated, reached via `run_dedicated_server` in `src/net/dedicated/mod.rs`) both delegate to the same `run_host` in `src/net/host.rs`. Dedicated entry is `./cli server --bind ... --auth ... [--world ...] [--admin-socket ...]`.

`run_host` builds the host App, registers the protocol plugin, and schedules a chained system pipeline (`src/net/host.rs` - `run_host`):

```
drain_host_commands → [drain_admin_socket (unix)] → receive_client_messages
  → handle_disconnected_clients → tick_authoritative_server → mirror_systems
```

`tick_authoritative_server` advances the fixed 20 Hz simulation and sets `ServerTickPulse.advanced` only on updates where a tick boundary crossed. The `mirror_systems` tuple (the five `sync_*_entities` mirror systems plus `update_client_room_subscriptions`) is `.run_if(server_tick_advanced)`, so it runs only on tick-advancing updates, not every host-loop iteration. Everything before `tick_authoritative_server` (command/admin drains, receive, disconnect handling) runs every update and stays ungated. The mirror systems themselves are documented in `docs/replication.md`.

## Channels

Three channels are registered in `src/net/channels.rs` - `LightyearProtocolPlugin::build`. Lightyear's internal replication channel is **not** registered here; it carries entity spawns/despawns/component diffs and is described in `docs/replication.md`.

| Channel | Mode | Priority | Carries |
| --- | --- | --- | --- |
| `ReliableChannel` | `OrderedReliable` | 10.0 | auth, chat, every command, depletion/kill/door events, world-map markers, anything where a drop or reorder corrupts domain state |
| `UnreliableChannel` | `SequencedUnreliable` | 5.0 | movement, corrections, ping, impacts, world-time, perf stats, player list, heartbeats; sequenced so a newer value supersedes a stale one |
| `VoiceChannel` | `UnorderedUnreliable` | 8.0 | Opus voice frames only; unordered so a slightly-late packet is still played, higher priority than other unreliable traffic so a busy stream can't shoulder voice off the wire |

Each message picks its delivery via `PacketDelivery` (`Reliable` / `Unreliable` / `UnreliableUnordered`); the `HasDelivery` trait in `channels.rs` maps that to the channel for both directions. The per-variant mapping is the `delivery()` match on each enum in `src/protocol/messages.rs`. See `docs/voice.md` for the rest of the VOIP pipeline.

## Version and auth handshake

Two distinct gates.

**Netcode `protocol_id`** is `LIGHTYEAR_PROTOCOL_ID = 0x4153_4857_454E_4401` (`src/net/channels.rs`, the bytes `b"ASHWEND\x01"`). It is a **fixed constant, deliberately independent of `PROTOCOL_VERSION`**. Netcode rejects a mismatched id at the transport layer before any application message is exchanged; if it tracked the app version, a version-bumped client would be bounced there and could never learn *which* version the server runs. Keeping it fixed lets every connection reach the app-level `Auth` handshake. Bump it only on a genuinely incompatible transport change.

**App-level `PROTOCOL_VERSION = 37`** (`src/protocol/mod.rs`) is the real version gate. It rides in `ClientMessage::Auth` alongside the human-readable `GAME_VERSION` (the crate version). `GameServer::connect` compares both against its own; a mismatch returns a typed `VersionMismatchRejection`, which `src/net/host/routing.rs` turns into `ServerMessage::VersionMismatch { server_version, server_protocol }`. The client pairs that with its compiled-in `GAME_VERSION` to show a "you're newer/older" modal and disconnects cleanly. Bump `PROTOCOL_VERSION` on any breaking wire change so mismatched builds are rejected cleanly at `Auth`.

**Wire-skew fallback** (`src/net/host/routing.rs` - `handle_unauthenticated_message`): postcard is not self-describing, so a genuinely version-skewed client's `Auth` can deserialize into a *different* `ClientMessage` variant. An unauthenticated client whose first message is not `Auth` is therefore answered with `VersionMismatch` too, and the surfaced variant is logged. That path **swallows** benign version-agnostic control messages (`Heartbeat`, `Ping`, `Disconnect`) instead of bouncing the player, because a same-version client can legitimately have one of those queued from a prior in-process session (singleplayer → main menu → multiplayer) or reordered ahead of its reliable `Auth`. On the client, a `VersionMismatch` arriving after a `Welcome` is dropped as stale, and one landing in the same receive batch as an `AuthRejected` is suppressed (the auth rejection is the real reason).

**Stable handshake surface:** keep the wire shapes *and the enum positions* of `ClientMessage::Auth`, `ServerMessage::AuthRejected`, and `ServerMessage::VersionMismatch` stable so a future server can always tell an older client why it was turned away.

**Auth modes** (the WorkOS identity flow itself is in `src/auth/`):
- `AuthMode::Workos` (dedicated default): the client presents a WorkOS access-token JWT, validated offline against the WorkOS JWKS.
- `AuthMode::NoAuth` (loopback singleplayer + `./cli multiplayer-test`): the server trusts the client's claimed `account_id` and display name with no token check. **Localhost only**, never expose a `NoAuth` server to the network.

The netcode private key (`LIGHTYEAR_PRIVATE_KEY`) defaults to all-zero. `private_key` (`src/net/channels.rs`) emits a one-shot warning to set `LIGHTYEAR_PRIVATE_KEY` only in the `PrivateKeyContext::NetworkExposed` case (dedicated/remote), never for `Loopback`.

## Message inventory

Pointer-level only; the per-variant payloads and delivery live in `src/protocol/messages.rs`. The wire-protocol module is the directory `src/protocol/` (`mod.rs`, `messages.rs`, `items.rs`, `world.rs`, `world_map.rs`, `commands.rs`, `math.rs`), re-exported flat as `crate::protocol::*`. There is no `src/net/protocol.rs`.

**The rule for which path new state takes:** per-entity world state the client renders or simulates against (a node, a drop, a deployable, a player, a loot bag) goes through Lightyear replication, **not** a `ServerMessage` variant. Presence, clocks, diagnostics, and one-shot intent signals go through a message. Never reintroduce a periodic full-state broadcast; the old `WorldSnapshot` wire was deleted and the replication path already re-ships unacked windows on its own (see CLAUDE.md replicated-state rules and `docs/replication.md`).

### `ClientMessage` (29 variants, `src/protocol/messages.rs`)

All ride **Reliable** except `Voice` (UnreliableUnordered) and `Movement` + `Ping` (Unreliable).

- Handshake / lifecycle: `Auth`, `Heartbeat`, `Disconnect`, `Ping { .. }`
- Movement: `Movement` (client-authoritative pose; see `docs/movement.md`)
- Social: `Chat`, `Command`
- Inventory / crafting: `Inventory`, `Crafting`, `OpenStorageBox { .. }`
- Gather / combat: `Gather`, `AttackPlayer`, `DamageDeployable`, `SwingStart` (cosmetic swing-start that drives the third-person swing animation; reliable so every swing animates on peers, including whiffs)
- Deployables / building: `PlaceDeployable`, `Furnace`, `PlaceBuilding`, `Building`, `Door`, `SleepingBag`, `Claim`
- Loot / death: `LootBag`, `LootSleeper { .. }`, `Respawn`, `RespawnAtBag { .. }`
- AoI / map: `SetViewRadius { .. }` (drives the per-client AoI ring size; see `docs/chunks-and-aoi.md`), `RequestWorldMap`, `WorldMapMarker(WorldMapMarkerCommand)`
- Voice: `Voice`

### `ServerMessage` (23 variants, `src/protocol/messages.rs`)

Reliable: `Welcome`, `AuthRejected`, `VersionMismatch`, `Kicked`, `PlayerEvent`, `Chat`, `ItemMerged`, `Toast`, `ResourceNodeDepleted`, `Knockback`, `PlayerKilled`, `DoorCodePrompt`, `DoorCodeResult`, `WorldMapMarkers`.
Unreliable: `Correction`, `ResourceImpact`, `PlayerImpact`, `WorldTime`, `PerfStats`, `Pong`, `PlayerList`, `Heartbeat`.
UnreliableUnordered: `Voice`.

- Handshake / lifecycle: `Welcome`, `AuthRejected`, `VersionMismatch`, `Kicked`, `Heartbeat`, `Pong`
- Movement: `Correction` (cosmetic; authoritative pose replicates as `PlayerPose`)
- Social: `Chat`, `PlayerEvent`, `Toast`
- Inventory: `ItemMerged`
- Combat / death: `ResourceImpact`, `PlayerImpact` (both cosmetic feedback; authoritative damage lands via the replicated health component, so they ride unreliable), `Knockback`, `PlayerKilled`
- Doors: `DoorCodePrompt`, `DoorCodeResult`
- Resource nodes: `ResourceNodeDepleted` (see below)
- Presence / clocks / diagnostics (periodic, deliberately **not** entity state): `WorldTime` (~1/min day-night clock the client extrapolates locally), `PerfStats` (~1 Hz per-client perf-HUD diagnostics), `PlayerList` (~1 Hz presence roster including players outside the receiver's AoI ring, so it can't ride chunk-gated replication)
- Map: `WorldMapMarkers`
- Voice: `Voice`

### Two messages that complement replication rather than carrying snapshots

- **`ServerMessage::ResourceNodeDepleted`** (reliable): Lightyear's bare entity-despawn can't tell the client "node depleted, play the death animation" apart from "node left my AoI ring, silent despawn". This message is the disambiguator; the client's depletion-grace window pairs it with the matching Lightyear despawn. It signals intent, it does not patch dropped diffs.
- **`ServerMessage::WorldMapMarkers`** (reliable): the hold-`M` world map. **The biome terrain image is NOT on the wire.** It is a pure function of `(world_seed, dims)`, both of which the client already gets in `Welcome`, so the client generates it locally (`src/world::map_texture`). Only the per-account, server-owned, persisted **markers** ride the wire. The client sends `ClientMessage::RequestWorldMap` on map open and `ClientMessage::WorldMapMarker(WorldMapMarkerCommand::{Add,Rename,Remove})` for edits; the server answers both with the caller's full updated marker list in `WorldMapMarkers { markers }`, filtered to `owner == account_id` so a shared map can't leak enemy markers. (Note: `src/server/world_map.rs` still contains a server-side terrain raster, but it is **not on the wire**; the live path ships markers only.)

## Admin socket (Unix only)

`./cli server --admin-socket <path>` binds a line-delimited JSON Unix stream socket alongside the dedicated server, drained by `drain_admin_socket` in the host pipeline. `./cli admin --socket <path> ...` is the client (`src/net/dedicated/admin.rs` - `send_admin_request`).

Wire schema: a `DedicatedAdminRequest` serialized with `serde_json::to_writer` plus a trailing newline; the reply is a `DedicatedAdminResponse { ok, message }`. `DedicatedAdminRequest` (`#[serde(tag = "command", rename_all = "snake_case")]`) has four variants:

- `Announce { text }` - broadcast as a server chat message
- `Shutdown { reason }` - kick all clients with the reason, persist the save, exit
- `SetTime { seconds_of_day }` - jump the day/night clock
- `SetTimeMultiplier { multiplier }` - change the day/night cycle speed (clamped server-side)

## Networking files

- `src/net/client.rs` - `ClientSession` (struct), `start_singleplayer`, `connect`, Lightyear client app, auth send, command/incoming queues, local-host shutdown hook.
- `src/net/channels.rs` - `LightyearProtocolPlugin`: channel registration, the `register_component::<T>()` registry, `LIGHTYEAR_PROTOCOL_ID`, the `PacketDelivery → channel` table, and the private-key context.
- `src/net/host.rs` - `spawn_loopback_server`, `run_game_server`, `run_host`, `tick_authoritative_server`, `ServerTickPulse`, the `mirror_systems` tuple wiring.
- `src/net/host/routing.rs` - `handle_unauthenticated_message`, authenticated routing, connection maps.
- `src/net/host/mirror.rs` - the five `sync_*_entities` mirror systems (see `docs/replication.md`).
- `src/net/host/rooms.rs` - room/AoI subscription helpers, `attach_room_gated_replication`, `attach_player_replication` (see `docs/replication.md` and `docs/chunks-and-aoi.md`).
- `src/net/host/handle.rs` - host command handle, final-save request, thread shutdown.
- `src/net/host/admin.rs` (Unix only) - admin socket listener and `DedicatedAdminRequest` dispatch.
- `src/net/dedicated/mod.rs` - CLI-facing `run_dedicated_server`.
- `src/net/dedicated/admin.rs` - `DedicatedAdminRequest` / `DedicatedAdminResponse` and the `./cli admin` client helper.

## Related docs

- `docs/replication.md` - per-component replication, the host mirror, the #740 per-entity-group fix, rooms/AoI, catch-up and recovery.
- `docs/server-authority.md` - `GameServer`, `ClientMessage` handlers, tick subsystems.
- `docs/chunks-and-aoi.md` - chunk membership, AoI ring math, the room-subscription system, `SetViewRadius` view tiers.
- `docs/movement.md` - the client-authoritative movement trust boundary and the ~1.5x tick wire pace.
- `docs/voice.md` - mic capture, Opus codec, the Voice channel, spatial mixing.
- `docs/build-and-dev.md` - `./cli server`, `./cli admin`, `./cli multiplayer-test`, `--features replication-trace`.
