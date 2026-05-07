# Networking

Multiplayer networking uses Lightyear.

Lightyear provides transport, replication, prediction, reliable channels, and snapshot interpolation. UDP uses netcode; Steam transport is available with `--features steam`.

The game protocol registers replicated player components and native inputs in `src/net/dedicated.rs`.

Dedicated server:
- runs a headless Bevy app.
- starts Lightyear `ServerPlugins` at 20 Hz.
- uses UDP/netcode in offline mode.
- uses Lightyear Steam transport in Steam mode when built with `--features steam`.

Local singleplayer bypasses packet networking and uses `LocalGameSession`.
