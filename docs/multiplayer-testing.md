---
title: Two-client multiplayer-test helper
owns: The `./cli multiplayer-test` helper (server + admin-seeded temp save + two auto-connecting clients) and its GAME_TEST_* spawn/window contract.
when_to_read: Before running or modifying the two-client multiplayer-test helper, or capturing one player as seen from another.
sources:
  - src/cli/multiplayer_test.rs - run_multiplayer_test, spawn_server, spawn_client, test_client_layouts
  - src/app/state/test_mode.rs - TestModeConfig, TestWindowLayout, env::* keys
  - src/app/systems/test_mode.rs - apply_test_mode_overrides_system, reposition_test_window_system, multiplayer_test_owns_window
related:
  - docs/headless-agent-testing.md - single-client control-socket primitives the headless mode reuses
  - docs/voice.md - the voice pipeline these caveats are about
  - docs/build-and-dev.md - the ./cli surface this subcommand lives on
---

# Two-client multiplayer-test helper

> When to read this: before running or modifying `./cli multiplayer-test`, or when you need to capture one player as seen from another. Source of truth: `src/cli/multiplayer_test.rs`, `src/app/state/test_mode.rs`, `src/app/systems/test_mode.rs`. Canonical invariants live in CLAUDE.md.

`./cli multiplayer-test` spins up a complete two-client session (one server, two auto-connecting clients) in one command, so you can eyeball interpolation, nameplates, chat bubbles, player rigs, and shared-state correctness without launching three processes and arranging windows by hand. It is a developer-iteration helper, not a test in the `./cli test` sense.

This doc owns the helper itself. For driving a **single** headless client through its control socket (slash commands, `dump_state`, screenshots, placement), see [headless-agent-testing.md](headless-agent-testing.md). The `GAME_TEST_HEADLESS=1` mode below reuses exactly those control-socket primitives, one socket per client.

## What it spawns

`run_multiplayer_test` (`src/cli/multiplayer_test.rs`) does, in order:

1. **Seeds an ephemeral world save.** A throwaway `test.save` is written under the OS temp dir (`game-multiplayer-test-<pid>/test.save`), created with `WorldSave::new_with_map` at `MapType::Procedural { seed: 0, size: Small }` (`TEST_MAP_SIZE = ProceduralMapSize::Small`, the smallest procedural map so it boots fast and streams cheaply). Both test account IDs are pushed into `seeded.admins` before the save is written, so admin-gated slash commands work from the first frame.
2. **Spawns the dedicated server.** `spawn_server` runs the current executable as `server --bind 127.0.0.1:<port> --world <temp>/test.save --auth no-auth --map-size small`. `--auth no-auth` bypasses WorkOS and admits each client by the account id + name it claims via the environment; `--map-size small` is derived from `TEST_MAP_SIZE` via `map_size_cli_token` so it can't drift from the seed save and trip the size guard. Server stdout is piped and prefixed `[server]`.
3. **Waits for the server-ready handshake.** A reader thread scans server stdout for the line containing `listening on` and signals readiness. `wait_for_server_ready` blocks on that signal with `SERVER_READY_TIMEOUT = 45s`. On a warm rebuild this is a few hundred ms. The listening line fires when the UDP socket is reserved, which is a tick or two before the Lightyear netcode entity accepts sessions, so `wait_for_tcp_canary` then sleeps a fixed `150ms` (a UDP server can't be TCP-probed). Skipping that canary makes the first client occasionally hit "connection refused".
4. **Launches two clients in parallel.** `spawn_client` runs the current executable as `client --connect 127.0.0.1:<port>` per layout (see below). `--connect` skips the menu and auto-joins.
5. **Waits and tears down.** `wait_for_clients` polls both child processes until both exit (or Ctrl-C / closed stdin trips the exit flag), then `ServerProcess::shutdown` kills + reaps the server and the `TempDir` `Drop` removes the temp save directory.

Port: `--port 0` (the default) reserves a free port by binding and dropping a TCP listener, then hands that port to the UDP server. Pass `--port <n>` to pin it.

Names: defaults are `player1` (left) and `player2` (right), from `DEFAULT_NAMES`. Override with `--names`, one or two values, blank/whitespace values fall back to the default for that slot. (The `MultiplayerTest` clap doc-comment in `src/cli.rs` still says the defaults are `Alpha`/`Bravo`; that comment is stale, the real defaults are `player1`/`player2`.)

Account IDs are fixed and distinct: `TEST_ACCOUNT_IDS = [76561197960287001, 76561197960287002]`. They differ from the default local-dev bypass id so a test run does not collide with a real local session, and they are the ids seeded into `save.admins`.

## GAME_TEST_* spawn/window contract

The helper communicates per-client setup through `GAME_TEST_*` environment variables. The producer is `spawn_client`; the consumer is `TestModeConfig::from_env` (`src/app/state/test_mode.rs`), which reads them once into a `TestModeConfig` resource. The env-var name constants live in `test_mode::env` so the two sides can't drift. Production runs set none of these, so `TestModeConfig::default` is a no-op and both consumer systems short-circuit.

Always set by `spawn_client`:

| Env var | Value | Effect |
| --- | --- | --- |
| `GAME_PLAYER_NAME` | the resolved name | display name / claimed identity |
| `GAME_ACCOUNT_ID` | one of `TEST_ACCOUNT_IDS` | claimed account id (matches a seeded admin) |
| `GAME_TEST_SPAWN_OFFSET_X` | ±1.25 | meters added to the predicted spawn x after Welcome |
| `GAME_TEST_SPAWN_YAW` | ±π/2 | yaw (radians) forced on the predicted controller, makes the two face each other |
| `GAME_TEST_INVENTORY_OPEN` | `1` GUI / `0` headless | force the inventory panel open on the first in-game frame |
| `GAME_TEST_AUTO_KIT` | `1` | fire `/test-kit` once on the first in-game frame |

`TestModeConfig` also reads `GAME_TEST_SPAWN_OFFSET_Z` (defaults to 0; the helper never sets it).

### Spawn placement and facing

`apply_test_mode_overrides_system` (`src/app/systems/test_mode.rs`) applies the runtime overrides exactly once, the first frame the client is `Screen::InGame` with a predicted local controller. Movement is client-authoritative (see [movement.md](movement.md)), so it bumps the predicted controller's pose and lets the next outbound packet carry it. It adds `spawn_offset_x`/`spawn_offset_z` to `predicted.position`, and when `spawn_yaw` is set it writes that yaw to **both** `predicted.yaw` and `LookState.yaw`. Both are required: `client_input_system` reads `LookState.yaw` and writes it back through `apply_input`, so setting only the controller yaw would be clobbered the same tick.

The two layouts from `test_client_layouts` are equal-and-opposite:

- `player1`: `window_index 0`, `spawn_offset_x = -1.25`, `spawn_yaw = +π/2`.
- `player2`: `window_index 1`, `spawn_offset_x = +1.25`, `spawn_yaw = -π/2`.

Yaw convention (matches the live mouse-look code, `look.yaw -= delta.x`):

- yaw `0` looks toward `-Z`.
- yaw `+π/2` looks toward `+X`.
- yaw `-π/2` looks toward `-X`.

So `player1` sits at `-X` and faces `+X` (toward `player2`); `player2` mirrors it. They land roughly `2 × 1.25 = 2.5m` apart, close enough to read nameplates and voice indicators, far enough that interpolation is legible.

If `GAME_TEST_AUTO_KIT=1`, the same system fires one `ClientMessage::Command { text: "test-kit" }` once in-game. `/test-kit` is admin-gated, which is why the helper seeds both account ids into `save.admins`; without that seeding the command would silently no-op. The send is fire-and-forget (the player can re-issue it from chat if it fails to land).

### Monitor-aware window placement (GUI mode)

In GUI mode `spawn_client` also sets the window-geometry keys so the client can tile itself once the real monitors are queryable:

| Env var | Value |
| --- | --- |
| `GAME_TEST_WINDOW_WIDTH` | `880` (`TEST_WINDOW_WIDTH`) |
| `GAME_TEST_WINDOW_HEIGHT` | `620` (`TEST_WINDOW_HEIGHT`) |
| `GAME_TEST_WINDOW_INDEX` | `0` or `1` |
| `GAME_TEST_WINDOW_COUNT` | `2` |
| `GAME_TEST_WINDOW_GAP` | `24` (`TEST_WINDOW_GAP`) |

`TestModeConfig::from_env` only builds a `TestWindowLayout` when all four of width/height/index/count parse, are nonzero, and `index < count`; otherwise `window` is `None`.

`reposition_test_window_system` (`src/app/systems/test_mode.rs`) runs once per session, retrying until `Query<&Monitor>` is non-empty (winit surfaces monitors a frame or two after the window opens). Monitors are sorted left-to-right by `monitor.physical_position` (x then y), so index 0 is the leftmost screen; ties keep a stable enumeration order.

- **Two or more monitors:** each client takes its own screen `BorderlessFullscreen`, `player1` (index 0) on the leftmost monitor, `player2` (index 1) on the next one right. An out-of-range index clamps to the last monitor.
- **Single monitor:** falls back to the centered, side-by-side windowed tiling via `TestWindowLayout::position_in_screen`. Bevy reports monitor size in physical pixels but `Window.position` is logical, so the math divides by `scale_factor` (Retina/HiDPI safe) and adds the monitor's `physical_position` offset.

While `config.window.is_some()`, `multiplayer_test_owns_window` returns true and the normal `apply_display_settings_system` is gated off, so the player's saved display settings can't fight the reposition system.

## GAME_TEST_HEADLESS=1 mode

`GAME_TEST_HEADLESS=1 ./cli multiplayer-test` is a real opt-in branch in `spawn_client` (it checks `GAME_TEST_HEADLESS` via `env::var_os`). It is the intended way to drive both clients programmatically and capture one player as seen from the other (for example, verifying the third-person swing rig: one player swinging, screenshotted from the other).

In this mode each client instead gets:

| Env var | Value |
| --- | --- |
| `GAME_HEADLESS_CAPTURE` | `1280x960` (off-screen render target) |
| `GAME_CONTROL_SOCKET` | `/tmp/ashwend-mptest-0.sock` and `/tmp/ashwend-mptest-1.sock` |
| `GAME_TEST_INVENTORY_OPEN` | `0` (inventory forced closed so it never covers the other player) |

The `GAME_TEST_WINDOW_*` geometry keys are deliberately **omitted** in headless mode, the on-screen reposition path fights the hidden capture window, so `config.window` is `None` and `reposition_test_window_system` no-ops.

`GAME_TEST_SPAWN_OFFSET_X`, `GAME_TEST_SPAWN_YAW`, and `GAME_TEST_AUTO_KIT` are still set, so the two players still spawn facing each other with the full kit. Drive each socket independently with `scripts/ashwend-control.py`, exactly as for a single headless client. The full control-command catalog, `dump_state` schema, the wait-in-world / async-screenshot timing contract, and the `GAME_ACCOUNT_ID` must-match `--admin` gotcha all live in [headless-agent-testing.md](headless-agent-testing.md). Here the admin gating is already satisfied: both account ids are seeded admins.

Headless capture is debug-only (compiled out of release builds), and an agent-driven session inserts `VoiceDisabled` and sets `GlobalVolume` to `Linear(0.0)` (`src/app.rs`), so a `GAME_TEST_HEADLESS=1` run never opens the mic or plays audio. That sidesteps the voice-echo caveats below entirely.

## Voice-echo caveats (GUI mode only)

In GUI mode both client processes run real audio and share the same default microphone and speakers, so testing voice on one machine has feedback hazards:

1. Both clients capture the same speech from your mic and send it. The server does **not** echo your own voice back, the receive filter skips packets whose `speaker` matches the listener's own `client_id` (`src/server/voice.rs`, `client.client_id != speaker`), so you don't hear yourself directly.
2. Without headphones you still get a round-trip loop: client A's speaker output is captured by the mic and sent, client B plays it, B's output is captured again, and so on. There is no Discord-style echo suppression. For voice debugging on one machine, use headphones.

Spatial attenuation uses `VOICE_AUDIBLE_RANGE = 50.0` meters (`src/server/voice.rs`); the two test players spawn ~2.5m apart, well inside range. See [voice.md](voice.md) for the rest of the pipeline.

## Related docs

- [docs/headless-agent-testing.md](headless-agent-testing.md): single-client control-socket primitives (commands, `dump_state`, screenshots, placement) that the headless mode reuses.
- [docs/voice.md](voice.md): the voice subsystem these caveats are about.
- [docs/movement.md](movement.md): the client-authoritative trust boundary the spawn-offset override relies on.
- [docs/build-and-dev.md](build-and-dev.md): the full `./cli` surface this subcommand lives on.
