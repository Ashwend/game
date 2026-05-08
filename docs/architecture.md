# Architecture

One Rust binary, `game`, defaults to `client`; `server` runs the experimental dedicated Lightyear server.

Modules:
- `app`: Bevy client, egui UI, scene, input, local prediction, local session polling.
- `server`: in-process authoritative game state for local singleplayer, including auth, connected players, chat, admin state, and snapshots.
- `controller`: shared player movement and collision simulation.
- `protocol`: serializable client/server messages, packets, snapshots.
- `net`: local in-process session plus the server-side Lightyear dedicated transport.
- `save` + `world`: persistent world metadata and generated geometry.
- `steam`: offline auth shim and feature-gated Steam hook points.

Singleplayer runs the same `GameServer` through `LocalGameSession`, then persists on shutdown.

Dedicated multiplayer runs a headless Bevy app with Lightyear server plugins. It currently covers server transport, replicated player components, native input, and authoritative movement; the playable Lightyear client path is not wired yet.
