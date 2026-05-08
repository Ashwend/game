# CLAUDE.md

AI context for this repo.

Game is a Rust/Bevy first-person prototype. Local singleplayer uses an in-process server; the dedicated Lightyear server path is experimental and server-side first; worlds are JSON saves.

Start here:
- `src/cli.rs`: commands.
- `src/app.rs`: Bevy app wiring.
- `src/server.rs`: authoritative game state.
- `src/protocol.rs`: wire messages and shared state.
- `src/controller.rs`: movement simulation.
- `src/save.rs`: world persistence.

Use `./cli check`, `./cli test`, and `./cli lint`.

Open docs only when the task touches that area:
- [Architecture](docs/architecture.md)
- [Movement](docs/movement.md)
- [Networking](docs/networking.md)
- [Worlds and saves](docs/worlds-and-saves.md)
- [UI and client flow](docs/ui-and-client.md)

Keep changes small, preserve module boundaries, and do not add markdown summaries unless asked.
