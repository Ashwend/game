# CLAUDE.md

AI context for this repo.

Game is a Rust/Bevy first-person prototype. Singleplayer and multiplayer both use the Lightyear-backed `ClientSession::Network` path; singleplayer only adds loopback host startup, host admin assignment, and local save persistence. Worlds are compressed binary `.save` files (postcard + zstd, versioned `GAMESAVE` header).

Start here:
- `src/cli.rs`: `client`, `server`, and `admin` subcommands.
- `src/app.rs`: Bevy app wiring and named `ClientSystemSet` schedule ordering.
- `src/app/state/`: client resources and UI/runtime state.
- `src/app/ui/modal.rs`: reusable animated modal shell plus confirmation modal.
- `src/app/ui/worlds/`: singleplayer worlds screen, dialogs, table, and session actions.
- `src/app/ui/inventory/`: inventory slot rendering, drag handling, and pickup tooltip helpers.
- `src/app/ui/multiplayer/`: direct-connect dialog, address parsing, and connection attempt helpers.
- `src/server.rs` and `src/server/`: shared authoritative game state for both singleplayer loopback and dedicated multiplayer; keep connection/auth/snapshot, inventory, movement, dropped-item, and resource-node concerns split.
- `src/protocol.rs`: wire messages and shared state.
- `src/controller/`: movement simulation, movement tuning/math, collision, and the server-side block spatial grid.
- `src/items.rs` and `src/resources.rs`: item registry, tool profiles, and resource-node definitions/gather rules.
- `src/net/client.rs`: Lightyear client session wrapper used by singleplayer and direct multiplayer.
- `src/net/host.rs` and `src/net/host/`: Lightyear host wrapper, handle/shutdown helpers, routing around `GameServer`, and the optional Unix admin socket used by `./cli admin`.
- `src/net/dedicated/`: CLI-facing dedicated server entry point and admin request types.
- `src/save.rs`: world persistence (`WorldStore`, `WorldSave`, atomic writes, format version).
- `src/world.rs`: `MapType`, world block geometry, and resource node spawns.

Use `./cli check`, `./cli test`, and `./cli lint`.

Singleplayer/multiplayer invariant:
- Keep gameplay behavior in shared modules: `server`, `protocol`, `controller`, `items`, `world`, and shared app systems.
- Do not add a separate singleplayer gameplay implementation, direct in-process transport bypass, or duplicate movement/inventory/chat rules for local play.
- Singleplayer-specific code should stay limited to selecting/loading a save, starting a loopback host, marking the local host as admin, and saving the host world state on shutdown.
- Multiplayer-specific code should stay limited to remote address/server discovery, auth mode, transport setup, and dedicated-host lifecycle.
- When adding a feature, make it work through `ClientMessage`/`ServerMessage` and `GameServer` first, then let both loopback singleplayer and direct multiplayer consume that same path.
- Movement is intentionally client-authoritative for responsiveness. Clients send `PlayerMovement` state produced by local prediction; the server validates sequence/finite values and republishes snapshots. Do not convert to server-authoritative input simulation unless explicitly asked.
- Critical pause/menu invariant: the ESC pause menu must only block local player controls and cursor capture. It must not stop gameplay simulation, local prediction ticks, or network/session ticks while `Screen::InGame`; otherwise players and the loopback server appear frozen whenever pause UI is visible.

Clean-code rules:
- No monolithic files. If a file mixes transport, domain rules, UI layout, persistence, and tests, split by concern before extending it.
- Prefer small modules with clear ownership over broad helper files. Good splits already exist in `src/server/`, `src/controller/`, `src/app/systems/`, `src/app/state/`, and `src/app/ui/worlds/`.
- Keep UI rendering, UI state, session actions, and authoritative game rules separate.
- Put reusable modal/backdrop animation behavior in `src/app/ui/modal.rs`; individual screens should only provide form contents and choice mapping.
- Keep networking transport adapters thin; they should translate to shared protocol messages and delegate gameplay to `GameServer`.
- Add tests near the module that owns the behavior, especially for protocol changes, server authority, persistence, and layout/state helpers.
- Update the relevant existing doc when changing architecture. Do not create markdown summary files unless explicitly asked.

Open docs only when the task touches that area:
- [Architecture](docs/architecture.md)
- [Movement](docs/movement.md)
- [Networking](docs/networking.md)
- [Worlds and saves](docs/worlds-and-saves.md)
- [UI and client flow](docs/ui-and-client.md)

Keep changes small and preserve module boundaries.
