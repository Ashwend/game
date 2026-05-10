# Networking

Networking intentionally has one gameplay path with two bootstraps.

Shared runtime:
- `ClientSession` has one active variant: `ClientSession::Network(Box<LightyearGameSession>)`.
- Both local singleplayer and direct multiplayer send `ClientMessage` values over Lightyear channels.
- Both receive `ServerMessage` values from the same host wrapper and apply them through `ClientRuntime`.
- `GameServer` owns the authoritative domain state for auth, players, movement state acceptance, inventory, dropped items, chat, snapshots, and save tick state.
- Movement is client-authoritative by design. Clients send predicted `PlayerMovement` state; the server rejects stale or non-finite movement and snapshots the accepted state. This keeps first-person movement responsive at the cost of stronger cheat resistance.

Singleplayer bootstrap:
- `ClientSession::start_singleplayer` loads a `WorldSave`, starts `spawn_loopback_server`, and connects the normal Lightyear client to the reserved loopback UDP address.
- The loopback host runs the same `run_host` code as a dedicated server, with `ServerSettings { auth_mode: Offline, singleplayer_host: Some(user.steam_id) }`.
- Ephemeral loopback ports are reserved until the host thread performs its first update and reports startup, so the client is not handed an address before the host is ready to bind it.
- On shutdown, the client asks the local host for `world_save()` and persists it through `WorldStore`.

Multiplayer bootstrap:
- `./cli server --bind ... --auth ...` loads a world and calls `run_dedicated_server`, which delegates to the same `host::run_game_server`/`run_host` path.
- The multiplayer UI calls `ClientSession::connect(addr, user)` and uses the same client thread, message channels, runtime snapshots, prediction, chat, and inventory flow as singleplayer.
- On graceful terminal shutdown, the dedicated host returns its final `WorldSave`; `--world` saves back to that file, while the default dedicated world saves through `WorldStore`.

Networking files:
- `src/net/client.rs`: client session API, Lightyear client app, auth send, command queue, incoming message queue, and local-host shutdown/persistence hook.
- `src/net/host.rs`: loopback host spawn, dedicated host run, Lightyear server app, shutdown, and fixed server ticking.
- `src/net/host/handle.rs`: host command handle, final-save request, and thread shutdown.
- `src/net/host/routing.rs`: unauthenticated/authenticated message handling, connection maps, and envelope routing.
- `src/net/protocol.rs`: Lightyear channel setup and delivery selection for shared protocol messages.
- `src/net/dedicated/mod.rs`: CLI-facing dedicated server wrapper.

Do not reintroduce a direct in-process singleplayer transport or a singleplayer-only gameplay server. If a feature needs networking, add it to the shared protocol and `GameServer` flow so loopback singleplayer and remote multiplayer exercise the same code.

Steam mode is not production-ready. `AuthMode::Steam` currently rejects until a live SteamGameServer verifier is wired; the server browser path opens the Steam UI through the offline backend but does not register a visible server.
