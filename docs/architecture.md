# Architecture

One Rust binary, `game`, defaults to `client`; `server` runs a dedicated UDP server.

Modules:
- `app`: Bevy client, egui UI, scene, input, prediction, network polling.
- `server`: auth, connected players, chat, admin state, snapshots.
- `controller`: shared player movement and collision simulation.
- `protocol`: serializable client/server messages, packets, snapshots.
- `net`: local in-process session plus Lightyear dedicated multiplayer transport.
- `save` + `world`: persistent world metadata and generated geometry.
- `steam`: offline auth shim and feature-gated Steam hook points.

Singleplayer runs the same `GameServer` through `LocalGameSession`, then persists on shutdown.

Dedicated multiplayer runs a headless Bevy app with Lightyear server plugins.
