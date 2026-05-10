# Game

[![Quality Gate](https://github.com/danniehansen/game/actions/workflows/quality-gate.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/quality-gate.yml)
[![Coverage Gate](https://github.com/danniehansen/game/actions/workflows/coverage.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/coverage.yml)

Rust/Bevy first-person game prototype with loopback singleplayer, direct UDP multiplayer, JSON world saves, and a Lightyear host/client path shared by both play modes.

## Run

- `./cli dev` - run the Bevy client.
- `./cli server --bind 127.0.0.1:7777 --auth offline` - run a dedicated server.
- `./cli check` - run `cargo check --all-targets`.
- `./cli test` - run tests.
- `./cli lint` - run rustfmt and clippy.
- `./cli coverage` - run tests under `cargo-llvm-cov` when installed.

## Shape

- Client: Bevy scene, egui menus/HUD/chat, local prediction.
- Shared server: auth, sessions, movement state acceptance, inventory, dropped items, chat, admin state, snapshots.
- Networking: singleplayer starts a loopback Lightyear host; direct multiplayer connects to the same host path over UDP/netcode.
- Movement: shared first-person controller with collision, jump buffering, coyote time.
- Worlds: platform-local JSON saves backed by generated world data; loopback and dedicated hosts persist final save state on graceful shutdown.
- Audio: runtime audio uses WAV assets for reliable Bevy/rodio playback; main-menu ambience loops until a world loads, and egui buttons emit click/hover one-shots.
- Steam: offline dev backend now; `steam` feature is the transport integration hook.

See `CLAUDE.md` for AI context.
