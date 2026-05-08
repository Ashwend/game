# Architecture

One Rust binary, `game`, defaults to `client`; `server` runs the experimental dedicated Lightyear server.

Modules:
- `app`: Bevy client, scene, input, audio, local prediction, local session polling, and egui UI.
  - `app/state`: client resources split by concern: menu state, dialogs, runtime session state, look state, and menu backdrop fade state.
  - `app/ui/worlds`: singleplayer worlds screen split into the screen shell, table rendering, create/edit dialogs, and session actions.
- `server`: in-process authoritative game state for local singleplayer, including auth, connected players, chat, admin state, and snapshots.
- `controller`: shared player movement simulation. `mod.rs` owns `PlayerController`, `movement.rs` owns horizontal movement tuning/math, and `collision.rs` owns world-block collision.
- `protocol`: serializable client/server messages, packets, snapshots.
- `net`: local in-process session plus the server-side Lightyear dedicated path.
  - `net/dedicated`: headless server app wiring, transport setup, connection spawning, authoritative movement, and replicated component protocol are split into separate files.
- `save` + `world`: persistent world metadata and generated geometry.
- `steam`: offline auth shim and feature-gated Steam hook points.

Singleplayer runs the same `GameServer` through `LocalGameSession`, then persists on shutdown.

Dedicated multiplayer runs a headless Bevy app with Lightyear server plugins. It currently covers server transport, replicated player components, native input, and authoritative movement; the playable Lightyear client path is not wired yet.

Client audio is split between `src/app/systems/audio.rs` for main-menu ambience and `src/app/ui.rs` plus `src/app/ui/theme/buttons.rs` for UI one-shots. Runtime audio assets are WAV files so Bevy/rodio can decode them reliably and button effects can start exactly at the intended transient.
