# Networking

Networking currently has two paths.

Local singleplayer uses `ClientSession::Local` and `LocalGameSession`. It runs `GameServer` in-process, sends `ClientMessage` values directly, drains `ServerMessage` values, and persists the selected save on shutdown.

The dedicated server path uses Lightyear in `src/net/dedicated/`. It:
- runs a headless Bevy app.
- starts Lightyear `ServerPlugins` at 20 Hz.
- registers replicated player components and native `NetworkInput`.
- simulates authoritative movement from input on the server.
- uses UDP/netcode in offline mode.
- uses Lightyear Steam transport in Steam mode when built with `--features steam`.

Dedicated server files:
- `mod.rs`: headless Bevy app assembly and system registration.
- `transport.rs`: UDP/netcode and feature-gated Steam transport setup.
- `connections.rs`: new client observer and replicated player spawning.
- `movement.rs`: authoritative server movement from native input.
- `protocol.rs`: Lightyear component registration, replicated player components, interpolation, and `NetworkInput`.

The playable client is not yet connected to the Lightyear dedicated server path. The multiplayer UI is still gated from the main menu, and the direct UDP connect button reports that client networking is moving to Lightyear replication.

Steam mode is transport-only for now. Live Steam auth ticket validation and server browser registration still need final Steamworks server integration.
