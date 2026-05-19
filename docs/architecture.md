# Architecture

One Rust binary, `game`, defaults to the `client` subcommand. `server` runs the dedicated Lightyear host. `admin` sends one-shot administrative commands to a running dedicated server over a Unix socket.

Modules:
- `app`: Bevy client, scene, input, audio, local prediction, session polling, and egui UI. `src/app.rs` wires runtime systems through named `ClientSystemSet`s instead of long per-system ordering chains.
  - `app/state`: client resources split by concern: menu state, dialogs, runtime session state, look state, inventory UI state, toast queue, client settings, and menu backdrop fade state.
  - `app/scene`: first-person scene setup, generated floor/block geometry, and resource-node and held-item mesh builders under `app/scene/mesh/`.
  - `app/systems`: scheduled gameplay systems split by concern — input, camera, network tick, effects, players, items, node death, audio, display, and quit.
  - `app/ui/modal.rs`: reusable animated modal shell and confirmation modal behavior.
  - `app/ui/hud.rs`, `app/ui/chat.rs`, `app/ui/toast.rs`: HUD, chat panel, and toast feed.
  - `app/ui/inventory`: inventory slot rendering, drag/drop command dispatch, and pickup tooltip helpers.
  - `app/ui/multiplayer`: direct-connect dialog orchestration, connection attempts, and address parsing helpers.
  - `app/ui/worlds`: singleplayer worlds screen split into the screen shell, table rendering, create/edit dialogs, and session actions.
  - `app/ui/theme`: shared egui colors, frames, text, buttons, and tooltips.
- `server`: shared authoritative game state for loopback singleplayer and dedicated multiplayer, including auth, connected players, movement acceptance, inventory, dropped items, resource nodes, chat, admin state, and snapshots. Connection/auth/snapshot code lives in `server/connection.rs`; inventory, movement, dropped-item, and resource-node helpers live under `server/`.
- `controller`: shared player movement simulation. `mod.rs` owns `PlayerController`, `movement.rs` owns horizontal movement tuning/math, `collision.rs` owns world-block AABB collision, and `grid.rs` owns a coarse spatial grid built from `WorldData` for fast block queries.
- `items` and `resources`: item registry, tool profiles, dropped-item helpers, resource-node definitions, and gather-rule logic shared by client UI and server gather processing.
- `protocol`: serializable client/server messages, packets, snapshots, and `PROTOCOL_VERSION`.
- `net`: Lightyear client and host adapters.
  - `client.rs`: `ClientSession::Network`, client app thread, auth send, outgoing `ClientMessage`, and incoming `ServerMessage`.
  - `host.rs`: Lightyear server app thread, connection mapping, message routing, fixed server ticking, loopback host spawning, and dedicated host running.
  - `host/admin.rs` (Unix only): admin Unix socket listener used by the `./cli admin` subcommand for announce and shutdown.
  - `protocol.rs`: Lightyear channel/message registration and delivery-channel helpers.
  - `dedicated/mod.rs`: small CLI-facing wrapper around the shared host plus the `DedicatedAdminRequest`/`DedicatedAdminResponse` JSON contract used by `./cli admin`.
- `save` + `world`: persistent world metadata, generated geometry, and resource-node spawns. `WorldSave` is a binary `.save` file (postcard payload + zstd, behind a `GAMESAVE` magic header and a `u32` format version).
- `steam`: offline auth shim and feature-gated Steam hook points.

Singleplayer and multiplayer are intentionally the same gameplay path. `ClientSession::start_singleplayer` starts a loopback Lightyear host with `GameServer`, then connects the normal Lightyear client to it. `ClientSession::connect` connects that same client to a remote Lightyear host. Both paths send `ClientMessage`, receive `ServerMessage`, and consume snapshots through `ClientRuntime`.

Movement is intentionally client-authoritative for responsiveness. The client predicts locally and sends `PlayerMovement`; the server accepts finite, increasing movement sequences and republishes snapshots. This trades cheat resistance for movement feel and should not be replaced with server-side input simulation unless that product decision changes.

Singleplayer-only responsibilities are save selection/loading, loopback host lifecycle, local host admin assignment, and saving the host world state on shutdown. Multiplayer-only responsibilities are remote address/server discovery, auth mode, dedicated host lifecycle, and the admin socket. Do not fork gameplay rules between those paths.

Dedicated multiplayer runs the same host wrapper as singleplayer loopback. On graceful terminal shutdown — or in response to an admin shutdown command — it persists to the supplied `--world` file or to the platform `Dedicated` world save. Direct UDP connect is wired through the multiplayer UI. Steam auth/server-browser work is still incomplete: `AuthMode::Steam` rejects until a live SteamGameServer verifier is wired.

Client audio is split between `src/app/systems/audio.rs` for main-menu ambience and `src/app/ui.rs` plus `src/app/ui/theme/buttons.rs` for UI one-shots. Runtime audio assets are WAV files so Bevy/rodio can decode them reliably and button effects can start exactly at the intended transient.
