# Game

Rust/Bevy first-person game prototype with local singleplayer, JSON world saves, and an authoritative UDP server path.

## Run

- `./cli dev` - run the Bevy client.
- `./cli server --bind 127.0.0.1:7777 --auth offline` - run a dedicated server.
- `./cli check` - run `cargo check --all-targets`.
- `./cli test` - run tests.
- `./cli lint` - run rustfmt and clippy.

## Shape

- Client: Bevy scene, egui menus/HUD/chat, local prediction.
- Server: auth, sessions, chat, admin state, snapshots.
- Movement: shared first-person controller with collision, jump buffering, coyote time.
- Network: Lightyear replication over UDP, with Steam transport available behind `--features steam`.
- Worlds: platform-local JSON saves backed by generated world data.
- Steam: offline dev backend now; `steam` feature is the integration hook.

See `CLAUDE.md` for AI context.
