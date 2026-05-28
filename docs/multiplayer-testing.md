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
     names (`Alpha`, `Bravo` by default; override via positional args).
   - `--connect <server addr>` so they skip the menu and auto-join.
   - A bundle of `GAME_TEST_*` environment variables that drive
     window-tiling, spawn placement, and the "start with inventory open"
     behaviour. See [Voice](voice.md) and below for the keys.
4. Polls the two child processes until both exit, then shuts the server
   down cleanly (its `Drop` flushes any save state).

## Test-mode behaviour the helper produces

- **Side-by-side, centered, never-overlapping windows.** Each client
  receives `GAME_TEST_WINDOW_WIDTH/HEIGHT/INDEX/COUNT/GAP` and computes
  its own final pixel position *after* the primary monitor's logical
  size becomes queryable (see `reposition_test_window_system` in
  `src/app/systems/test_mode.rs`). Multi-monitor and Retina/HiDPI safe
  because the math uses `monitor.physical_position` and divides by
  `monitor.scale_factor`. Trying to compute the position before the
  window opens — by guessing screen width — is what produced the earlier
  "windows overlap on my display" regression.
- **Players spawn 2.5 m apart facing each other.** `GAME_TEST_SPAWN_OFFSET_X`
  pushes each player ±1.25 m from the world spawn point along the
  X axis; `GAME_TEST_SPAWN_YAW` sets each player's initial yaw so they
  look at each other (Alpha = +π/2 → faces +X, Bravo = −π/2 → faces −X
  on the controller's mouse-look convention). The override has to write
  *both* `predicted.yaw` and `LookState.yaw` because `client_input_system`
  echoes `LookState.yaw` back into the controller every input tick.
  Movement is client-authoritative, so the server accepts the new pose
  on the next outbound `Movement` packet.
- **Inventory panel open on join.** `GAME_TEST_INVENTORY_OPEN=1` flips
  `MenuState::inventory_open` the first frame the client reaches the
  in-game screen.

All three overrides run exactly once, gated by a `Local<bool>` in the
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
| `GAME_TEST_WINDOW_WIDTH` | u32 | Window logical width (px). |
| `GAME_TEST_WINDOW_HEIGHT` | u32 | Window logical height (px). |
| `GAME_TEST_WINDOW_INDEX` | u32 | 0-based slot in the tile row. |
| `GAME_TEST_WINDOW_COUNT` | u32 | Total tile slots (always 2 today). |
| `GAME_TEST_WINDOW_GAP` | i32 | Pixel gap between sibling windows. |
| `GAME_TEST_SPAWN_OFFSET_X` | f32 | Meters added to spawn position along X. |
| `GAME_TEST_SPAWN_OFFSET_Z` | f32 | Meters added to spawn position along Z. |
| `GAME_TEST_SPAWN_YAW` | f32 | Initial yaw in radians (set after Welcome). |
| `GAME_TEST_INVENTORY_OPEN` | u8 | `1` → open the inventory on first in-game frame. |

## Tuning knobs

Defaults live as constants in `src/cli/multiplayer_test.rs`:

- `TEST_WINDOW_WIDTH` / `TEST_WINDOW_HEIGHT` — sized to fit two windows
  side-by-side on a 1920-wide display with comfortable margins, and
  tall enough to show the inventory panel without scrolling.
- `TEST_WINDOW_GAP` — pixel gap between the two test windows.
- `TEST_PLAYER_OFFSET_X` — half the spawn separation between the two
  players (so they end up `2 × TEST_PLAYER_OFFSET_X` apart). Tuned so
  voice indicators / nameplates are clearly visible without making
  interpolation jitter hard to spot.
- The names array (`DEFAULT_NAMES = ["Alpha", "Bravo"]`) is the
  positional default for `./cli multiplayer-test`; pass `Tom Echo` to
  override.

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

- `src/cli/multiplayer_test.rs`: the helper itself — server spawn, client
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
*see* two clients render the same world — voice chat, interpolation,
animation, nameplate behaviour, UI synchronisation, or visual
verification of replicated state on chunk crossings.
