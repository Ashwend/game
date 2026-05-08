# Game

Rust/Bevy first-person game prototype with local singleplayer, JSON world saves, and an experimental Lightyear dedicated server path.

## Run

- `./cli dev` - run the Bevy client.
- `./cli server --bind 127.0.0.1:7777 --auth offline` - run a dedicated server.
- `./cli check` - run `cargo check --all-targets`.
- `./cli test` - run tests.
- `./cli lint` - run rustfmt and clippy.

## Shape

- Client: Bevy scene, egui menus/HUD/chat, local prediction.
- Local server: auth, sessions, chat, admin state, snapshots.
- Dedicated server: headless Lightyear replication over UDP/netcode, with Steam transport available behind `--features steam`.
- Movement: shared first-person controller with collision, jump buffering, coyote time.
- Client networking: playable client sessions are local-only while the Lightyear client path is still being wired.
- Worlds: platform-local JSON saves backed by generated world data.
- Steam: offline dev backend now; `steam` feature is the transport integration hook.

See `CLAUDE.md` for AI context.
