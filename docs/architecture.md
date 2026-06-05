# Architecture

One Rust binary, `game`, defaults to the `client` subcommand. `server` runs the dedicated Lightyear host. `admin` sends one-shot administrative commands to a running dedicated server over a Unix socket.

Modules:
- `app`: Bevy client, scene, input, audio, local prediction, session polling, and egui UI. `src/app.rs` wires runtime systems through named `ClientSystemSet`s instead of long per-system ordering chains.
  - `app/state`: client resources split by concern: menu state, dialogs, runtime session state, look state, inventory UI state, toast queue, client settings, and menu backdrop fade state.
  - `app/scene`: first-person scene setup, generated floor/block geometry, and resource-node and held-item mesh builders under `app/scene/mesh/`.
  - `app/systems`: scheduled gameplay systems split by concern, input, camera, network tick, effects, players, items, node death, audio, display, and quit.
  - `app/ui/modal.rs`: reusable animated modal shell and confirmation modal behavior.
  - `app/ui/hud.rs`, `app/ui/chat.rs`, `app/ui/toast.rs`: HUD, chat panel, and toast feed.
  - `app/ui/inventory`: inventory slot rendering, drag/drop command dispatch, and pickup tooltip helpers.
  - `app/ui/multiplayer`: direct-connect dialog orchestration, connection attempts, and address parsing helpers.
  - `app/ui/worlds`: singleplayer worlds screen split into the screen shell, table rendering, create/edit dialogs, and session actions.
  - `app/ui/theme`: shared egui colors, frames, text, buttons, and tooltips.
  - `app/ui/options`: tabbed options panel, display/audio/voice/controls/keybindings/general subscreens plus shared widgets and the per-tab grid layout.
  - `app/voice`: client voice chat subsystem, Opus codec wrappers, cpal mic capture, cpal output mixer with per-speaker jitter buffer, and the Bevy systems that bridge the capture/playback worker threads to the network protocol. See [Voice](voice.md).
- `server`: shared authoritative game state for loopback singleplayer and dedicated multiplayer, including auth, connected players, movement acceptance, inventory, dropped items, resource nodes, deployables, chat, admin state, and voice frame routing. Connection/auth code lives in `server/connection.rs`; inventory, movement, dropped-item, resource-node, deployable, and voice helpers live under `server/`. The authoritative state lives in `HashMap`s on `GameServer`; exclusive sync systems in `src/net/host.rs` mirror those maps into ECS entities carrying the per-component replicated state Lightyear ships to clients.
  - `server/chunk_manager.rs`: server-side owner of the chunk grid. Anchors every networked entity (resource nodes, dropped items, eventually buildings) to its containing 64 m chunk and is the AoI gate the room-subscription system queries (`which chunks does this player see?`) when adding/removing the client's sender from per-chunk Lightyear `Room`s. Also schedules 5–15 min jittered respawns for depleted nodes, applies an outer-ring density falloff during initial population, and persists per-chunk live counts plus pending regrows into `ChunkManagerSave` (embedded in `WorldStateSave`).
- `controller`: shared player movement simulation. `mod.rs` owns `PlayerController`, `movement.rs` owns horizontal movement tuning/math, `collision.rs` owns world-block AABB collision, and `grid.rs` owns a coarse spatial grid built from `WorldData` for fast block queries.
- `items` and `resources`: item registry, tool profiles, dropped-item helpers, resource-node definitions, and gather-rule logic shared by client UI and server gather processing.
- `protocol`: serializable client/server messages, packets, replicated component types, and `PROTOCOL_VERSION`.
- `net`: Lightyear client and host adapters.
  - `client.rs`: `ClientSession::Network`, client app thread, auth send, outgoing `ClientMessage`, and incoming `ServerMessage`.
  - `host.rs`: Lightyear server app thread, connection mapping, message routing, fixed server ticking, loopback host spawning, and dedicated host running.
  - `host/admin.rs` (Unix only): admin Unix socket listener used by the `./cli admin` subcommand for announce and shutdown.
  - `protocol.rs`: Lightyear channel/message registration and delivery-channel helpers.
  - `dedicated/mod.rs`: small CLI-facing wrapper around the shared host plus the `DedicatedAdminRequest`/`DedicatedAdminResponse` JSON contract used by `./cli admin`.
- `save` + `world`: persistent world metadata, perimeter geometry, and the chunk-based generation pipeline. `world/chunk/` is the pure side of generation, `classification.rs` samples four seeded noise channels per chunk and labels it forest/rocky-outcrop/ore-vein/plains/mixed, `generator.rs` Poisson-disk-samples resource-node spawns inside each chunk against per-classification base-capacity tables, and `noise.rs` owns the value-noise + fbm + splitmix64 primitives both passes share. Both passes are pure functions of `(world_seed, chunk_coord)` so the same world generates identically every load. `WorldSave` is a binary `.save` file (postcard payload + zstd, behind a `GAMESAVE` magic header and a `u32` format version), see [Worlds and saves](worlds-and-saves.md).
- `world_time`: authoritative day/night clock shared by server and client. The server advances `seconds_of_day` per tick using a `multiplier`, persists both into `WorldStateSave`, and broadcasts a `ServerMessage::WorldTime` snapshot every minute (and immediately after `/time` or `/speed` admin changes). The client integrates locally between snapshots so the sun/moon stay smooth between drift realignments. Visual realisation (sun + moon directional lights, ambient curve, sky color, fog, sun/moon discs) lives in `app/scene/sky.rs`.
- `auth`: WorkOS-backed identity. The server verifies a client's access-token JWT (RS256) against the WorkOS JWKS (`auth/verify.rs`) and derives the stable `u64` account id from the WorkOS subject claim (`auth/identity.rs`); the client owns PKCE login, token refresh, and sealed local refresh-token storage (`auth/workos/token_store.rs`, encrypted at rest via `src/local_crypto.rs`) under `auth/workos/`. `AuthMode` is `Workos` (default for dedicated) or `NoAuth` (loopback/localhost only).

Singleplayer and multiplayer are intentionally the same gameplay path. `ClientSession::start_singleplayer` starts a loopback Lightyear host with `GameServer`, then connects the normal Lightyear client to it. `ClientSession::connect` connects that same client to a remote Lightyear host. Both paths send `ClientMessage`, receive `ServerMessage`, and consume replicated entity state through Lightyear's per-component replication, room-gated to the chunk ring around the local player.

Movement is intentionally client-authoritative for responsiveness. The client predicts locally and sends `PlayerMovement`; the server accepts finite, increasing movement sequences, writes them onto the player's mirror entity, and lets Lightyear replicate the resulting `PlayerPublic` to other clients. This trades cheat resistance for movement feel and should not be replaced with server-side input simulation unless that product decision changes.

Singleplayer-only responsibilities are save selection/loading, loopback host lifecycle, local host admin assignment, and saving the host world state on shutdown. Multiplayer-only responsibilities are remote address/server discovery, auth mode, dedicated host lifecycle, and the admin socket. Do not fork gameplay rules between those paths.

Dedicated multiplayer runs the same host wrapper as singleplayer loopback. On graceful terminal shutdown, or in response to an admin shutdown command, it persists to the supplied `--world` file or to the platform `Dedicated` world save. Direct UDP connect is wired through the multiplayer UI.

Client audio is split between `src/app/systems/audio.rs` for main-menu ambience and `src/app/ui.rs` plus `src/app/ui/theme/buttons.rs` for UI one-shots. Runtime audio assets are WAV files so Bevy/rodio can decode them reliably and button effects can start exactly at the intended transient. Voice chat is a separate subsystem under `src/app/voice/`, it runs its own cpal worker threads (cpal `Stream` is `!Send` on macOS) and ships Opus-encoded frames over a dedicated `VoiceChannel`.

Iteration tooling:
- `./cli multiplayer-test` spawns a dedicated server and two side-by-side client windows that auto-connect, spawn facing each other, and start with the inventory open. See [Multiplayer testing](multiplayer-testing.md). The corresponding `TestModeConfig` resource and apply-once systems live under `src/app/state/test_mode.rs` and `src/app/systems/test_mode.rs`.
