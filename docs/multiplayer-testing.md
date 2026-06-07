# Multiplayer Testing

`./cli multiplayer-test` is the primary "spin up two clients and a server
end-to-end" iteration helper. It exists so you can verify interpolation,
networking, voice chat, and shared-state correctness without manually
launching three processes and arranging windows.

## What it does

1. Spawns the dedicated server bound to `127.0.0.1:<ephemeral>` with a
   throwaway world save under the OS temp dir.
2. Waits for the `Lightyear game server listening on …` line on the
   server's stdout, then sleeps one server tick for the netcode entity
   to finish initialising.
3. Launches **two client processes** in parallel, each with:
   - Distinct Steam IDs (`76561197960287001`, `76561197960287002`) and
     names (`player1`, `player2` by default; override via positional args).
   - `--connect <server addr>` so they skip the menu and auto-join.
   - A bundle of `GAME_TEST_*` environment variables that drive
     window placement, spawn placement, and the "start with inventory open"
     behaviour. See [Voice](voice.md) and below for the keys.
4. Polls the two child processes until both exit, then shuts the server
   down cleanly (its `Drop` flushes any save state).

## Test-mode behaviour the helper produces

- **One client per monitor, with a single-monitor fallback.** Each
  client receives `GAME_TEST_WINDOW_INDEX` (0 or 1) and resolves its
  display *after* the monitors become queryable (see
  `reposition_test_window_system` in `src/app/systems/test_mode.rs`).
  Monitors are sorted left-to-right by `monitor.physical_position`, so:
  - With **two or more monitors**, `player1` (index 0) goes
    borderless-fullscreen on the leftmost screen and `player2` (index 1)
    on the next one to the right. "Don't know which is which" degrades to
    enumeration order.
  - With a **single monitor**, it falls back to the old centered,
    side-by-side, never-overlapping windowed tiling
    (`GAME_TEST_WINDOW_WIDTH/HEIGHT/GAP`), Retina/HiDPI safe because the
    math divides by `monitor.scale_factor`.

  Because the test harness owns the window here, `apply_display_settings_system`
  is gated off in test mode (`multiplayer_test_owns_window`) so the player's
  saved display mode (now **Borderless Fullscreen** by default) can't reassert
  itself and stack both clients on the primary monitor.
- **Players spawn 2.5 m apart facing each other.** `GAME_TEST_SPAWN_OFFSET_X`
  pushes each player ±1.25 m from the world spawn point along the
  X axis; `GAME_TEST_SPAWN_YAW` sets each player's initial yaw so they
  look at each other (player1 = +π/2 → faces +X, player2 = −π/2 → faces −X
  on the controller's mouse-look convention). The override has to write
  *both* `predicted.yaw` and `LookState.yaw` because `client_input_system`
  echoes `LookState.yaw` back into the controller every input tick.
  Movement is client-authoritative, so the server accepts the new pose
  on the next outbound `Movement` packet.
- **Inventory panel open on join.** `GAME_TEST_INVENTORY_OPEN=1` flips
  `MenuState::inventory_open` the first frame the client reaches the
  in-game screen.
- **Both clients are admins with a full kit.** `multiplayer-test`
  pre-seeds the temp world save with both test Steam IDs in
  `WorldSave.admins`, and `GAME_TEST_AUTO_KIT=1` makes each client send
  `/test-kit` once on the first in-game frame. Boots both windows with
  the full early-game tool + resource + workbench + furnace set so PvP
  / death / crafting paths are immediately exercisable. The admin flag
  also unlocks `/tp` (teleport every other connected player to you) and
  the rest of the admin-only slash commands.

All overrides run exactly once, gated by a `Local<bool>` in the
test-mode systems. Production builds (no `GAME_TEST_*` vars set) see
`TestModeConfig::default()` and the systems short-circuit immediately.

## Environment variable contract

The producer side (`src/cli/multiplayer_test.rs`) and the consumer side
(`src/app/state/test_mode.rs`) share key names through a single
`mod env { … }` block in the consumer so the two halves can't drift.

| Variable | Type | Purpose |
|---|---|---|
| `GAME_PLAYER_NAME` | string | Display name. |
| `GAME_STEAM_ID` | u64 | Stable identity per client. |
| `GAME_TEST_WINDOW_WIDTH` | u32 | Window logical width (px), single-monitor fallback tiling only. |
| `GAME_TEST_WINDOW_HEIGHT` | u32 | Window logical height (px), single-monitor fallback tiling only. |
| `GAME_TEST_WINDOW_INDEX` | u32 | 0-based client index. Selects the monitor (0 = leftmost) on multi-monitor; the tile slot on single-monitor. |
| `GAME_TEST_WINDOW_COUNT` | u32 | Total clients/tile slots (always 2 today). |
| `GAME_TEST_WINDOW_GAP` | i32 | Pixel gap between sibling windows, single-monitor fallback tiling only. |
| `GAME_TEST_SPAWN_OFFSET_X` | f32 | Meters added to spawn position along X. |
| `GAME_TEST_SPAWN_OFFSET_Z` | f32 | Meters added to spawn position along Z. |
| `GAME_TEST_SPAWN_YAW` | f32 | Initial yaw in radians (set after Welcome). |
| `GAME_TEST_INVENTORY_OPEN` | u8 | `1` → open the inventory on first in-game frame. |
| `GAME_TEST_AUTO_KIT` | u8 | `1` → fire `/test-kit` once after Welcome lands. Paired with admin steam IDs pre-seeded into the save. |

## Tuning knobs

Defaults live as constants in `src/cli/multiplayer_test.rs`:

- `TEST_WINDOW_WIDTH` / `TEST_WINDOW_HEIGHT`, single-monitor fallback
  only: sized to fit two windows side-by-side on a 1920-wide display with
  comfortable margins, and tall enough to show the inventory panel without
  scrolling. Ignored on multi-monitor (each client is borderless-fullscreen).
- `TEST_WINDOW_GAP`, single-monitor fallback gap between the two windows.
- `TEST_PLAYER_OFFSET_X`, half the spawn separation between the two
  players (so they end up `2 × TEST_PLAYER_OFFSET_X` apart). Tuned so
  voice indicators / nameplates are clearly visible without making
  interpolation jitter hard to spot.
- The names array (`DEFAULT_NAMES = ["player1", "player2"]`) is the
  positional default for `./cli multiplayer-test`; `player1` lands on the
  left monitor and `player2` on the right. Pass `Tom Echo` to override.

## Agent-driven control socket

For autonomous testing (an AI agent that launches the game, acts, screenshots,
and asserts on the result), the client exposes a dev-only Unix control socket.
It is bound only when `GAME_CONTROL_SOCKET` names a path, so a normal
`./cli client` launch never opens it. The socket is a thin transport adapter
(`src/app/systems/control_socket.rs`) that mirrors the server admin socket
(`src/net/host/admin.rs`): it owns no gameplay rules, it only pokes existing
client resources or forwards a `ClientMessage::Command`.

**Dev builds only.** The control socket and the off-screen capture
(`GAME_HEADLESS_CAPTURE`, below) are gated on `debug_assertions`, so they are
compiled out of release builds entirely. A shipped game (`cargo build
--release`, as `./cli publish` produces) does not contain this code, setting
`GAME_CONTROL_SOCKET` or `GAME_HEADLESS_CAPTURE` on a final build does nothing,
so a bot can't drive it. Both are available in every dev/debug build
(`cargo run`, `cargo build`, `./cli client`) with no extra flags. This does not
affect the server-side admin socket (`./cli admin`), which is a production ops
tool and stays available in release builds.

### Launch recipe

No real login is needed. A dedicated server in `no-auth` mode plus a client on
the `--connect` bypass path (which injects an identity from `GAME_ACCOUNT_ID` /
`GAME_PLAYER_NAME` and skips WorkOS, see `bypass_identity_from_env` in
`src/auth/identity.rs`) lands straight in-world:

```bash
# 1. Dedicated server, no auth, throwaway deterministic world (auto-created).
#    `--admin <id>` grants admin to the agent's GAME_ACCOUNT_ID so admin-gated
#    slash commands (test-kit, spawn-ore, time, speed) work over the socket.
./cli server --bind 127.0.0.1:7777 --auth no-auth --world /tmp/agent.save \
  --map-size small --admin 1

# 2. Client: bypasses login, auto-connects, opens the control socket:
GAME_CONTROL_SOCKET=/tmp/ashwend-control.sock \
GAME_ACCOUNT_ID=1 GAME_PLAYER_NAME=Agent \
./cli client --connect 127.0.0.1:7777
```

The `GAME_ACCOUNT_ID` passed to the client must match an id passed to the
server's `--admin`; otherwise admin-gated commands reply `"admin only"` and the
kit/spawns are silently skipped. Without `--admin` the agent still spawns,
moves, and screenshots fine, it just can't issue admin commands.

### Requests

Line-delimited JSON, one request per connection; the reply is
`{"ok": bool, "message": string}`. `dump_state` returns its snapshot as JSON in
`message`.

| Request | Effect |
|---|---|
| `{"command":"dump_state"}` | JSON snapshot: `client_id`, `in_world`, `screen`, the `*_open` flags, player position/yaw/pitch/health, ping, roster. |
| `{"command":"screenshot","path":"/tmp/shot.png"}` | Capture the primary window (3D scene + egui UI) to PNG. Async: the file lands a frame or two later, so poll for it. |
| `{"command":"send_command","text":"test-kit"}` | Forward a slash command (no leading `/`) to the server. |
| `{"command":"set_screen","screen":"worlds"}` | Navigate menu screens. Does not start a session; connect via `--connect`. |
| `{"command":"set_inventory_open","open":true}` | Open/close the inventory panel. |

### Driver

`scripts/ashwend-control.py` is a stdlib-only driver (no deps):

```bash
scripts/ashwend-control.py /tmp/ashwend-control.sock wait-in-world 30
scripts/ashwend-control.py /tmp/ashwend-control.sock screenshot /tmp/spawn.png
scripts/ashwend-control.py /tmp/ashwend-control.sock send-command test-kit
scripts/ashwend-control.py /tmp/ashwend-control.sock set-inventory-open true
scripts/ashwend-control.py /tmp/ashwend-control.sock dump-state
```

Prefer asserting on `dump_state` JSON over pixel-reading; use screenshots for
human review and visual regression. Gate the agent on `in_world` (the script's
`wait-in-world` polls it) rather than a fixed sleep.

### Headless off-screen capture (recommended for agents)

Set `GAME_HEADLESS_CAPTURE` to render the primary camera into an off-screen
image instead of the window, and the window comes up hidden. The screenshot
command then captures that image rather than the live window framebuffer, so
capture no longer depends on the window being visible/foregrounded. Because
`bevy_egui` attaches the primary egui context to the same camera, the captured
frame includes the full UI (inventory, menus, hotbar), not just the 3D scene.

```bash
GAME_HEADLESS_CAPTURE=1280x720 \
GAME_CONTROL_SOCKET=/tmp/ashwend-control.sock \
GAME_ACCOUNT_ID=1 GAME_PLAYER_NAME=Agent \
./cli client --connect 127.0.0.1:7777
```

`GAME_HEADLESS_CAPTURE` accepts `WIDTHxHEIGHT` or a bare `1` for the default
1280x720. Implementation: `src/app/systems/headless_capture.rs` (target image +
camera redirect) and the screenshot branch in `control_socket.rs`. With the
window hidden, winit runs the schedule each cycle (its `all_invisible` path), so
frames keep advancing and the image stays fresh without an on-screen surface.

### Background launch (no focus stealing)

Agent-driven sessions, anything with `GAME_CONTROL_SOCKET` and/or
`GAME_HEADLESS_CAPTURE` set, come up in the background and do not steal focus:
the primary window is created unfocused (`Window::focused = false`), and on
macOS the process drops to an accessory app on the first frame
(`src/app/systems/agent_window.rs`) and resigns the active status winit grabs on
launch, so focus returns to whatever you were doing. (The window flag alone is
not enough on macOS, winit activates the app on launch regardless of it.) This
is dev-only and macOS-only for the accessory part; a normal `./cli client` play
session is untouched and focuses as usual.

Agent-driven sessions also **disable voice chat entirely** (`VoiceDisabled` in
`src/app/voice/systems.rs`): neither the playback nor the microphone capture
stream is opened, so an automated run never grabs the mic, which on macOS would
otherwise force a Bluetooth headset out of A2DP into low-quality HFP. Voice
isn't part of what the harness exercises. Normal play keeps voice as usual.

They also **mute game audio**: `app.rs` inserts a zeroed `GlobalVolume` so a
headless/automated run is silent (no one is listening). This mutes everything
without tearing down the audio pipeline, so sinks still despawn normally. Normal
play is unaffected. Both the voice and audio toggles key off the same
`agent_driven` flag (control socket and/or `GAME_HEADLESS_CAPTURE` set).

### macOS windowing caveat (live-window path only)

Without `GAME_HEADLESS_CAPTURE`, the screenshot renders the live window
framebuffer, so the client window must actually be rendering. macOS throttles
(and can eventually close) a fully occluded / background winit window even with
`WinitSettings::continuous()`, so a client launched into the background may
answer the socket slowly (the driver allows 20 s per request for this) or exit
with `No windows are open`. For the live-window path, keep the client window
foregrounded/visible while the agent drives it (the request/response round-trip
itself works regardless; only the rendering throttles). Prefer the headless
capture mode above to sidestep this entirely.

## Voice testing caveats

Both client processes share the same default microphone and the same
default output device, because cpal/CoreAudio multiplex inputs but
outputs come straight from the OS mixer. Two practical consequences
when testing voice with `./cli multiplayer-test`:

1. Both clients will pick up the same speech from your mic (each one
   independently captures, encodes, and sends). The server forwards
   each speaker to the other; the receive system skips packets whose
   `speaker` matches its own `client_id`, so you don't hear yourself
   echoed back through your own speakers.
2. If you're testing without headphones, the *other* client's speaker
   output gets captured by your mic on the *first* client and sent back.
   The first client then plays the round-trip in its speakers and the
   second client captures it again, and so on. Discord-style echo
   suppression is not implemented; for voice debugging, use headphones
   on at least one of the two windows.

See [Voice](voice.md) for the rest of the audio pipeline.

## Module map

- `src/cli/multiplayer_test.rs`: the helper itself, server spawn, client
  spawn, `TestClientLayout` tile-index/spawn-offset computation, and the
  unit tests for the symmetry of the layout.
- `src/app/state/test_mode.rs`: `TestModeConfig`, `TestWindowLayout`,
  `env::*` key constants, and the screen-coords math
  (`TestWindowLayout::position_in_screen`).
- `src/app/systems/test_mode.rs`: `apply_test_mode_overrides_system`
  (spawn + yaw + inventory) and `reposition_test_window_system` (the
  post-monitor window placement).

## When *not* to use this

For end-to-end tests of a single concern (auth, chat round-trip,
gather, etc.), prefer the in-process tests under `src/net/tests.rs`
and `src/server/tests/` that drive `ClientSession` against a
`LightyearGameSession` without spawning child processes.
`./cli multiplayer-test` is for the cases where you actually need to
*see* two clients render the same world, voice chat, interpolation,
animation, nameplate behaviour, UI synchronisation, or visual
verification of replicated state on chunk crossings.
