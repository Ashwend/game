---
title: Driving the game headless (agent testing)
owns: The dev-only client automation harness (control socket, headless capture, the agent test loop) and how an agent asserts on a running client.
when_to_read: Whenever you need to launch, drive, screenshot, or assert on the running game to verify a change.
sources:
  - src/app/systems/control_socket.rs - ControlRequest enum, ClientStateDump, drain_control_socket
  - src/app/systems/control_socket/targeting.rs - nearest_deployable_id, resolve_building_pose, building_piece_needle
  - src/app/systems/headless_capture.rs - HeadlessCapture, resolution_from_env, redirect_camera_to_capture
  - src/app.rs - agent_driven flag, window focused/visible, voice+audio mute, socket+capture wiring
  - scripts/ashwend-control.py - stdlib driver
  - src/auth/identity.rs - bypass_identity_from_env
  - src/cli.rs - seed_admin_accounts, admin subcommand, DEFAULT_ADMIN_SOCKET
  - src/net/host/admin.rs - host admin socket
related:
  - docs/multiplayer-testing.md - the two-client ./cli multiplayer-test helper and its GAME_TEST_HEADLESS path
  - docs/updates-and-distribution.md - GAME_SKIP_UPDATE_CHECK and the boot update modal
  - docs/networking.md - the host admin socket and ./cli admin ops surface
  - docs/build-and-dev.md - ./cli subcommands (server, client, multiplayer-test, admin)
---

# Driving the game headless (agent testing)

> When to read this: launch, drive, screenshot, or assert on a running client to verify a change. Source of truth: `src/app/systems/control_socket.rs`, `src/app/systems/headless_capture.rs`, `src/app.rs`, `scripts/ashwend-control.py`. Canonical invariants live in CLAUDE.md.

The client exposes a dev-only automation harness so an agent can launch the game, act, and assert on JSON state instead of pixels. Two pieces: a per-client Unix **control socket** (`GAME_CONTROL_SOCKET`) that forwards commands / dumps state / queues screenshots, and an off-screen **headless capture** target (`GAME_HEADLESS_CAPTURE`) so screenshots work with a hidden, unfocused window. Both are gated on `debug_assertions` and compiled out of release builds entirely.

## Dev-only, zero-cost, release-stripped

Both the control socket and headless capture live behind `#[cfg(debug_assertions)]` (and the socket additionally behind `#[cfg(unix)]`). In a release build (`./cli publish`, `cargo build --release`) the code is not compiled in: setting `GAME_CONTROL_SOCKET` or `GAME_HEADLESS_CAPTURE` on a shipped binary does nothing, so a bot cannot drive the final game. Wiring is in `src/app.rs` (`agent_driven` flag, capture block, socket bind block).

- Socket bound only when `GAME_CONTROL_SOCKET` names a path, in a dev unix build (`src/app.rs` socket block). A normal `./cli dev` launch never opens it.
- The socket file is created with mode `0o660` (owner+group only); see `CONTROL_SOCKET_MODE` in `src/app/systems/control_socket.rs`. Keep it in a user-private dir.
- Build with `cargo build` / `./cli` (debug) and drive `./target/debug/ashwend`. **Do not try to automate a release binary.**

This is separate from the **host admin socket** (`src/net/host/admin.rs`), which is a production ops tool and stays in release builds. See the three-socket map below.

## Full end-to-end loop

No real login is needed. A dedicated server in `no-auth` mode plus a client on the `--connect` bypass path lands straight in-world. The bypass injects an identity from `GAME_ACCOUNT_ID` / `GAME_PLAYER_NAME` and skips WorkOS (`bypass_identity_from_env` in `src/auth/identity.rs`).

```bash
# 1. Dedicated server: no auth, throwaway world (auto-created), agent is admin.
#    --admin <id> must equal the client's GAME_ACCOUNT_ID or admin commands no-op.
./target/debug/ashwend server --bind 127.0.0.1:7777 --auth no-auth \
  --world /tmp/agent.save --map-size small --admin 1

# 2. Headless client: bypass login, auto-connect, off-screen capture + control socket.
GAME_HEADLESS_CAPTURE=1280x720 \
GAME_CONTROL_SOCKET=/tmp/ashwend-control.sock \
GAME_SKIP_UPDATE_CHECK=1 \
GAME_ACCOUNT_ID=1 GAME_PLAYER_NAME=Agent \
./target/debug/ashwend client --connect 127.0.0.1:7777

# 3. Wait until the world is fully loaded (poll, never a fixed sleep).
scripts/ashwend-control.py /tmp/ashwend-control.sock wait-in-world 30

# 4. Act: grant a kit, put a tool in hand, aim, place, etc.
scripts/ashwend-control.py /tmp/ashwend-control.sock send-command test-kit
scripts/ashwend-control.py /tmp/ashwend-control.sock select-actionbar-item iron_pickaxe
scripts/ashwend-control.py /tmp/ashwend-control.sock set-look 0.0 -0.37

# 5. Assert on dump_state JSON (the primary assertion surface, not pixels).
scripts/ashwend-control.py /tmp/ashwend-control.sock dump-state

# 6. Screenshot is async: queue it, then poll for the PNG to appear.
scripts/ashwend-control.py /tmp/ashwend-control.sock screenshot /tmp/spawn.png
```

`GAME_SKIP_UPDATE_CHECK=1` disables the boot-time GitHub release check (`UpdateState::SKIP_ENV`, dev-only; see `docs/updates-and-distribution.md`). Set it for any screenshot session, otherwise the "update available" modal opens a few seconds after boot and covers the scene.

### Admin id must match (the most common silent failure)

`GAME_ACCOUNT_ID` on the client **must equal** an id passed to the server's `--admin`. Admin status is `GameServer::is_admin` checking `save.admins` (`src/server/connection.rs - is_admin`), which the server seeds from `--admin` via `seed_admin_accounts` (`src/cli.rs - seed_admin_accounts`; ids of `0` and duplicates are dropped). On a mismatch, every admin-gated slash command replies `"admin only"` and the kit / spawn / give / time effects are silently skipped, which looks exactly like "the command did nothing." Confirm it took by reading `is_admin` in `dump_state`. Without `--admin` the agent still spawns, moves, looks, and screenshots fine; it just cannot run admin commands.

## Async / timing contract

- **Gate on `wait-in-world`, never a fixed sleep.** `in_world` is true only when `client_id`, the world, and the local player entity are all present (`build_dump`: `client_id.is_some() && world.is_some() && local_player.entity.is_some()`). The driver's `wait-in-world` polls `dump_state` every 0.25s until `in_world` or timeout.
- **Screenshots are asynchronous.** The screenshot command spawns a `Screenshot` entity with a `save_to_disk` observer; the PNG lands a frame or two later. Poll for the file before reading it. The reply (`"screenshot queued to ... (lands within a frame or two)"`) does not mean the file exists yet.
- **Socket timeouts.** The server reads/writes each request with a 2s timeout (`handle_stream`); the Python driver uses a 20.0s connect/recv timeout per request (`send(..., timeout=20.0)`). `wait-in-world`'s own default budget is 30.0s.
- **Session-gated commands.** Anything that forwards a `ClientMessage` (`send_command`, `select_actionbar_*`, `place_*`, `door_*`, `open_storage_box`, `close_container`, `upgrade_building`, `demolish_building`, `warp`, `swing`, `add_world_map_marker`) requires an active session and returns `"no active session (not in a world)"` (or a similar "not in a world" message) off-menu. Of the world-map commands, only `add_world_map_marker` is session-gated. `set_screen` / `set_inventory_open` / `set_look` / `set_world_map_open` / `set_world_map_view` mutate client UI/map state directly and can run pre-session (`set_world_map_open` only forwards `RequestWorldMap` when a session is already present, and returns Ok otherwise), but they do not start one (connect via `--connect`).

## Control-command catalog

Wire format: line-delimited JSON, one request per connection, reply `{"ok": bool, "message": string}`. `dump_state` returns its snapshot as a JSON string in `message`. The catalog below is the full `ControlRequest` enum (`src/app/systems/control_socket.rs - ControlRequest`); `serde` renames variants to `snake_case` so the `command` value is the snake_case form.

| `command` | Fields | Effect |
|---|---|---|
| `dump_state` | (none) | JSON snapshot for assertions (schema below). |
| `screenshot` | `path` | Queue a PNG of the capture image (headless) or live window. Async. |
| `send_command` | `text` | Forward a slash command (no leading `/`) to the server. |
| `select_actionbar_slot` | `slot` (usize, 0-based) | Put that slot's item in hand (e.g. after `test-kit` the iron pickaxe is slot 3). |
| `select_actionbar_item` | `item_id` | Find the actionbar slot holding `item_id` and select it; resilient to loadout shifts. Holding a deployable or `building_plan` raises its placement ghost. |
| `place_deployable` | `item_id`, `distance?` (default 2.2), `height?` (default 0.0) | Drop a carried structure that far ahead along the **view yaw**, front (`+Z`) turned back toward the camera. Server still validates inventory/ground/overlap. |
| `place_building` | `piece`, `distance?` (default 3.0), `height?` | Place a building piece. Foundations ride the raw aim; other pieces re-derive the nearest replicated socket near the aim point (`resolve_building_pose`). |
| `place_door` | `code`, `flip?` (default false), `iron?` (default false) | Hang a carried door in the nearest doorway with a lock code. `iron` picks `DoorVariant::Iron` else `HewnLog`. |
| `door_interact` | (none) | E-press the nearest door (toggle, or get the code prompt when unauthorized). |
| `door_enter_code` | `code` | Enter a code at the nearest door (authorize-only; door stays shut until a `door_interact`). |
| `door_pick_up` | (none) | Pick the nearest door back into inventory (server enforces claim auth + that you unlocked it). |
| `open_storage_box` | (none) | Open the nearest storage box's transfer UI. |
| `close_container` | (none) | Close whatever container panel (loot bag / sleeper / storage box) is open. |
| `upgrade_building` | `piece?` | Hammer-upgrade the nearest building block (optional piece-kind filter). Select the hammer first; server enforces hammer/ownership/cost. |
| `demolish_building` | `piece?` | Hammer-demolish the nearest building block (optional piece-kind filter). Server enforces hammer/ownership/demolish-window; cascade follows. |
| `set_look` | `yaw`, `pitch` (radians) | Point the camera absolutely. Pitch clamped to `MAX_LOOK_PITCH`. Use to aim at ground targets and view-ray commands like `/drain`. |
| `set_screen` | `screen` | Navigate menu screens (`main_menu`/`worlds`/`multiplayer`/`options`/`in_game`; tolerant of case and `_`/`-`/space). Does not start a session. |
| `set_inventory_open` | `open` | Open/close the inventory panel. |
| `set_world_map_open` | `open` | Open/close the world-map overlay, bypassing the focus + toggle-key gate. Opening also sends `RequestWorldMap` so terrain + markers stream in for the screenshot. |
| `add_world_map_marker` | `x`, `z` | Drop a map marker at world `(x, z)` as if right-clicking the map. Server assigns the id and persists it. |
| `set_world_map_view` | `zoom`, `center_x`, `center_z` | Set map pan/zoom directly (`zoom` 1.0 fits the whole world; `center_*` is the world point at the map centre). |
| `warp` | `x`, `z` | Teleport the local player to absolute `(x, z)`, keeping the current height; zeroes velocity. Movement is client-authoritative, so the next send carries it to peers. |
| `swing` | (none) | Fire one cosmetic swing of the held tool (empty hand → `Hands`). Captures the third-person remote swing headless (the LMB path is focus-gated). Uses a monotonic per-process seq so the server never rejects it as stale. |

Notes the catalog can't fit in a cell:

- **Scripted placement uses view yaw, not the look ray.** `place_deployable` / `place_building` compute forward as `(-sin yaw, 0, -cos yaw)` (see `controller::movement`) and put the structure straight ahead regardless of pitch, so you don't have to aim at the ground. Place a foundation first, read its id from `dump_state.deployables`, then stack: non-foundation pieces snap to the nearest replicated socket within `1.6` m of the aim point (`resolve_building_pose` in `src/app/systems/control_socket/targeting.rs`).
- **Target resolution is by nearest deployable over the dump's `kind` debug string.** Doors match `kind.starts_with("Door {")`; building piece filters match a `"piece: <Kind>"` substring with longest-name-first ordering so `Wall` doesn't swallow `WindowWall` (`nearest_deployable_id`, `resolve_building_pose`, `building_piece_needle`).
- **The Python driver lags the enum in two spots.** `place-door` only forwards `code` and `flip`, never `iron`, so the iron variant can only be hung via raw JSON (`{"command":"place_door","code":"...","iron":true}`). And the driver's `--help` docstring omits `warp` and `swing` even though both are wired in its dispatch table; they work via the driver.

## dump_state schema

`dump_state` is the primary assertion surface. Shape from `ClientStateDump` (assembled in `build_dump`, `src/app/systems/control_socket.rs`):

| Field | Type | Meaning |
|---|---|---|
| `client_id` | `u64?` | Local net id, `null` before connect. |
| `is_admin` | `bool` | Whether `--admin` granted this account admin. **Check this to confirm admin gating took.** |
| `world_loaded` | `bool` | World installed (`runtime.world.is_some()`). |
| `world_version` | `u64` | Monotonic counter bumped on each world install/reset; use to detect a reconnect/reload. |
| `in_world` | `bool` | Strong "fully loaded" signal: `client_id` + world + local player entity all present. Gate on this. |
| `private_present` | `bool` | Whether the owner-only `PlayerPrivate` (inventory/crafting) replicated. Distinguishes a fresh-but-empty inventory (`true`) from one that never arrived (`false`), e.g. a stale owner override after a sleeping-body wake. |
| `screen` | `string` | Current `Screen` (`MainMenu`/`Worlds`/`Multiplayer`/`Options`/`InGame`). |
| `inventory_open` `crafting_open` `furnace_open` `loot_bag_open` `pause_open` `chat_open` | `bool` | Which overlays are up. |
| `death_splash` | `bool` | Whether the death splash is showing (`menu.death_splash.is_some()`). |
| `position` | `[f32;3]?` | Local player world position. |
| `yaw` `pitch` | `f32?` | View angles (radians). |
| `health` | `f32?` | Local player health. |
| `local_ping_ms` | `u16` | Local RTT estimate. |
| `players` | `[{client_id, name, ping_ms}]` | Connected roster. |
| `deployables` | `[DeployableDump]` | Replicated placed structures in AoI. |

`DeployableDump`: `{ id: u64, kind: String, position: [f32;3], yaw: f32, health: u32, max_health: u32, active: bool }`. `kind` is the `Debug` string of the deployable kind (this is what door/box/building target resolution matches against). Use `deployables[]` to resolve ids and verify placements: `id`/`kind`/`position` confirm a placement landed, `health`/`max_health` confirm an upgrade, `active` confirms e.g. a furnace lit.

Assert against this JSON, not pixels. Screenshots are for human / visual-regression review only.

## Headless capture

Set `GAME_HEADLESS_CAPTURE` to render the primary camera into an off-screen `Image` instead of the window swapchain, and the window comes up hidden (`visible: false`). The screenshot command then captures that image (`Screenshot::image(capture.image)`) instead of the live framebuffer, so capture no longer depends on the window being visible or foregrounded. Because `bevy_egui` attaches the primary egui context to `MainCamera`, redirecting that camera's target sends both the 3D scene and the full egui UI into the image, so a captured frame matches what a player would see. Implementation: `src/app/systems/headless_capture.rs` (`insert_capture_target`, `redirect_camera_to_capture`).

- **Value parsing** (`HeadlessCapture::resolution_from_env` → `parse_resolution`): accepts `WIDTHxHEIGHT` (case-insensitive `x`/`X`, surrounding whitespace tolerated) or a truthy alias (`1`/`true`/`on`/`yes`) for the default **1280x720** (`DEFAULT_WIDTH`/`DEFAULT_HEIGHT`).
- **Bad input degrades silently.** Empty, malformed, or `0`-dimension strings return `None`, so the client falls back to the live-window path with no error. If a screenshot looks like a live-window capture, re-check the resolution string.
- **Frames keep advancing with no window.** A hidden window makes the winit runner take its `all_invisible` path and run the schedule each cycle, so the capture image stays fresh without an on-screen surface. This sidesteps the macOS occluded-window throttle entirely.

### Live-window caveat (no capture set)

Without `GAME_HEADLESS_CAPTURE`, screenshots read the live window framebuffer, so the window must actually be rendering. macOS throttles (and can eventually close) a fully occluded background winit window even with continuous settings, so a backgrounded client may answer the socket slowly or exit with `No windows are open`. Keep the window foregrounded for the live-window path, or just use headless capture (recommended) to avoid it.

## agent_driven side effects

`agent_driven = headless_capture.is_some() || GAME_CONTROL_SOCKET set` (unix), or `headless_capture.is_some()` (non-unix), and always `false` in release (`src/app.rs - agent_driven`). When set:

- **Window comes up unfocused** (`focused: !agent_driven` on the primary window), so the agent launch doesn't steal focus. On macOS+debug an extra system sets the app to `NSApplicationActivationPolicy::Accessory` and resigns the active status winit grabs on launch (`relinquish_macos_focus_system` / agent-window system), since the `focused` flag alone isn't enough on macOS.
- **Voice chat disabled** (`VoiceDisabled` resource): neither playback nor mic capture opens, so an automated run never grabs the mic (on macOS that would force a Bluetooth headset out of A2DP into low-quality HFP). See `docs/voice.md`.
- **Game audio muted** (`GlobalVolume::new(Volume::Linear(0.0))`): a headless run is silent without tearing down the audio pipeline, so sinks still despawn normally.

Normal `./cli dev` play is untouched by all three.

## Slash commands the socket can forward

`send_command` forwards the text as `ClientMessage::Command`. Dispatch table (`src/server/commands/mod.rs`): `spawn`, `drain`, `time`, `speed` (run-speed cheat), `time-speed` / `timespeed` / `timescale`, `test-kit` / `testkit`, `give`, `tp` / `teleport`, `help`. Every command except `help` checks `client.is_admin` and replies `"admin only"` when false. `test-kit` grants the early-game kit (the four tools, workbench_t1, crude_furnace, building_plan, hammer, hewn_log_door, sleeping_bag, plus 100 of each of ten resources with wood appearing twice); `tp` teleports every other connected player to you (for PvP/death staging). See `docs/server-authority.md` for the command handlers.

## Three-socket map

| Socket | Env / path | Build availability | Scope | Drives |
|---|---|---|---|---|
| Client control socket | `GAME_CONTROL_SOCKET=<path>` | dev-only (`debug_assertions` + unix) | one client process | This doc: commands, `dump_state`, screenshots, forwarded slash commands. |
| Host admin socket | `/run/game-server/admin.sock` (override with `--admin-socket`) | production (not debug-gated) | the whole dedicated host | `./cli admin announce/shutdown/time/time-speed`: `Announce`, `Shutdown{reason}`, `SetTime{seconds_of_day}`, `SetTimeMultiplier{multiplier}` only. |
| Loopback host | n/a (in-process) | always | singleplayer host | The `GameServer` the singleplayer client runs in-process; no socket. |

The two named sockets are easy to conflate but do different jobs. The control socket acts **as the player** (forwards `ClientMessage`s, including slash commands the player could type). The host admin socket (`src/net/host/admin.rs`, mode `0o660`, default path `DEFAULT_ADMIN_SOCKET` in `src/cli.rs`) is a **host-wide ops tool** with a fixed four-request vocabulary; it is the production path and is documented in `docs/networking.md`. Don't reach for the admin socket to do player actions, and don't try to drive a production server with the dev control socket (it isn't there).

## Two agent-driven clients at once

To capture one player as seen from another (e.g. the remote rig's swing), run the two-client helper in its headless branch: `GAME_TEST_HEADLESS=1 ./cli multiplayer-test`. Both clients then get `GAME_HEADLESS_CAPTURE=1280x960` and per-client sockets `/tmp/ashwend-mptest-0.sock` and `/tmp/ashwend-mptest-1.sock`; drive each socket independently with `scripts/ashwend-control.py`. The default (GUI) path, the spawn/yaw/kit env contract, and the helper internals live in `docs/multiplayer-testing.md`.

## Related docs

- `docs/multiplayer-testing.md` - the `./cli multiplayer-test` two-client helper and its `GAME_TEST_HEADLESS` headless branch.
- `docs/updates-and-distribution.md` - `GAME_SKIP_UPDATE_CHECK` and the boot-time update modal.
- `docs/networking.md` - the host admin socket and `./cli admin` ops surface.
- `docs/server-authority.md` - the slash-command handlers `send_command` forwards.
- `docs/build-and-dev.md` - the `./cli` subcommands (`server`, `client`, `multiplayer-test`, `admin`).
- `docs/voice.md` - the voice subsystem the harness disables.
