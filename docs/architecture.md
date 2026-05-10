# Architecture

One Rust binary, `game`, defaults to `client`; `server` runs the dedicated Lightyear host.

Modules:
- `app`: Bevy client, scene, input, audio, local prediction, session polling, and egui UI. `src/app.rs` wires runtime systems through named `ClientSystemSet`s instead of long per-system ordering chains.
  - `app/state`: client resources split by concern: menu state, dialogs, runtime session state, look state, and menu backdrop fade state.
  - `app/ui/modal.rs`: reusable animated modal shell and confirmation modal behavior.
  - `app/ui/inventory`: inventory slot rendering, drag/drop command dispatch, and pickup tooltip helpers.
  - `app/ui/multiplayer`: direct-connect dialog orchestration, connection attempts, and address parsing helpers.
  - `app/ui/worlds`: singleplayer worlds screen split into the screen shell, table rendering, create/edit dialogs, and session actions.
- `server`: shared authoritative game state for loopback singleplayer and dedicated multiplayer, including auth, connected players, movement acceptance, inventory, dropped items, chat, admin state, and snapshots. Connection/auth/snapshot code lives in `server/connection.rs`; inventory and dropped-item helpers live under `server/`.
- `controller`: shared player movement simulation. `mod.rs` owns `PlayerController`, `movement.rs` owns horizontal movement tuning/math, and `collision.rs` owns world-block collision.
- `protocol`: serializable client/server messages, packets, snapshots.
- `net`: Lightyear client and host adapters.
  - `client.rs`: `ClientSession::Network`, client app thread, auth send, outgoing `ClientMessage`, and incoming `ServerMessage`.
  - `host.rs`: Lightyear server app thread, connection mapping, message routing, fixed server ticking, loopback host spawning, and dedicated host running.
  - `protocol.rs`: Lightyear channel/message registration and delivery-channel helpers.
  - `dedicated/mod.rs`: small CLI-facing wrapper around the shared host.
- `save` + `world`: persistent world metadata and generated geometry.
- `steam`: offline auth shim and feature-gated Steam hook points.

Singleplayer and multiplayer are intentionally the same gameplay path. `ClientSession::start_singleplayer` starts a loopback Lightyear host with `GameServer`, then connects the normal Lightyear client to it. `ClientSession::connect` connects that same client to a remote Lightyear host. Both paths send `ClientMessage`, receive `ServerMessage`, and consume snapshots through `ClientRuntime`.

Movement is intentionally client-authoritative for responsiveness. The client predicts locally and sends `PlayerMovement`; the server accepts finite, increasing movement sequences and republishes snapshots. This trades cheat resistance for movement feel and should not be replaced with server-side input simulation unless that product decision changes.

Singleplayer-only responsibilities are save selection/loading, loopback host lifecycle, local host admin assignment, and saving the host world state on shutdown. Multiplayer-only responsibilities are remote address/server discovery, auth mode, and dedicated host lifecycle. Do not fork gameplay rules between those paths.

Dedicated multiplayer runs the same host wrapper as singleplayer loopback. On graceful terminal shutdown, it persists to the supplied `--world` file or to the platform `Dedicated` world save. Direct UDP connect is wired through the multiplayer UI. Steam auth/server-browser work is still incomplete.

Client audio is split between `src/app/systems/audio.rs` for main-menu ambience and `src/app/ui.rs` plus `src/app/ui/theme/buttons.rs` for UI one-shots. Runtime audio assets are WAV files so Bevy/rodio can decode them reliably and button effects can start exactly at the intended transient.
