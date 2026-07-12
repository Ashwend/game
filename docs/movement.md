---
title: Movement and the client-authoritative trust boundary
owns: The character controller (deterministic fixed-substep kinematic simulation), movement feel/tuning, the server movement trust boundary, and client reconciliation.
when_to_read: Before touching the character controller, movement feel/tuning, the server movement trust boundary, or debugging rubber-banding.
sources:
  - src/controller/mod.rs - PlayerController, simulate_with_grid, simulate_step, jump/coyote/step-up
  - src/controller/movement.rs - speed constants, desired_horizontal_velocity, accelerate_air air cap
  - src/server/movement.rs - accept_client_movement (server trust boundary)
  - src/server/dispatch.rs - ClientMessage::Movement handler, alive gate
  - src/app/state/runtime.rs - seed_local_prediction, apply_non_movement_correction (reconciliation)
  - src/app/systems/input/movement.rs - client_input_system, send pacing
  - src/app/systems/input/gating.rs - gameplay_simulation_allowed / gameplay_accepts_movement
related:
  - docs/gameplay-gating.md - when prediction runs vs when input is zeroed while a menu is open
  - docs/replication.md - PlayerPose mirror sync and per-component replication
  - docs/pvp-combat.md - combat range/LOS re-validation that backs the exploit-surface claim
  - docs/networking.md - the shared ClientMessage/ServerMessage transport both SP and MP use
---

# Movement and the client-authoritative trust boundary

> When to read this: before touching the character controller, movement feel/tuning, the server trust boundary, or debugging rubber-banding. Source of truth: `src/controller/`, `src/server/movement.rs`, `src/app/state/runtime.rs`, `src/app/systems/input/movement.rs`. Canonical invariants (singleplayer == multiplayer, gameplay-never-pauses, no-em-dashes) live in CLAUDE.md.

The movement slice has four parts that live in four different places. Get the geography right before editing:

1. **The deterministic simulator** (`src/controller/`): a fixed-substep kinematic controller, no reconciliation logic.
2. **Client prediction** (`src/app/systems/input/movement.rs`): runs the simulator every frame, paces the wire send.
3. **Client reconciliation** (`src/app/state/runtime.rs`): the Welcome seed and Correction snap. NOT in the controller.
4. **The server trust boundary** (`src/server/movement.rs` + `src/server/dispatch.rs`): two sites, accepts the client's reported pose, never re-simulates.

## Deterministic fixed-substep simulator

The controller is `PlayerController` (`src/controller/mod.rs - PlayerController`). It is a pure kinematic simulator: feed it input, call `simulate_*`, read the integrated pose. It has no networking and no reconciliation.

`simulate_with_grid` (`src/controller/mod.rs:203`) clamps the frame delta to `MAX_SIMULATION_DELTA` (0.1 s) then advances in fixed chunks of `MAX_SIMULATION_STEP` (`1.0 / 120.0` s):

```
remaining = delta.clamp(0.0, 0.1)
while remaining > 0: step = min(remaining, 1/120); simulate_step(step); remaining -= step
```

This makes physics frame-rate-independent and is load-bearing: the air-strafe cap, the jump arc, and the bunny-hop tests all assert substep-level behaviour. When writing a movement test, step at `1.0 / 120.0` to observe one substep; a single large delta is auto-split. `simulate(delta, world)` is the convenience entry that builds a `BlockGrid` per call; hot loops (client prediction) call `simulate_with_grid` with a cached grid instead.

Per substep (`simulate_step`, `src/controller/mod.rs - simulate_step`): refresh `grounded` via `is_supported`, tick the coyote/jump timers, fire a buffered jump if eligible, accelerate horizontally (ground vs air paths), then resolve collision **per axis, X then Z then Y** through `move_with_collisions`. Step-up (`try_step_up`) lets the player walk over obstacles up to `STEP_HEIGHT` (0.45 m) without jumping, and smooths the camera through `step_view_offset_y` without smoothing the physical collision.

The world floor is ANALYTIC, not a block: flat `y = 0` everywhere except over a live meteor shower crater, where `BlockGrid::floor_height` (`src/controller/grid.rs - floor_height`) follows the shared crater surface (`crate::world::crater_surface_height`) so players walk up and over the rim mound. The grid owner installs/clears the crater centre via `BlockGrid::set_crater` (client: `ClientRuntime::tick_world_time` + `rebuild_world_grid` off the event state). While grounded and not ascending, `simulate_step` glues the feet to that floor (terrain-follow) so slopes track smoothly instead of stair-stepping through the gravity/land cycle; the guard against ascending protects the high-fps jump (see gotchas). Guarded by `player_walks_up_and_over_the_crater_mound`, `crater_floor_supports_standing_inside_the_bowl`.

`PlayerController` stores: `position`, `velocity`, `yaw`, `pitch`, `health`, `grounded`, `last_processed_input`, plus internals an agent editing feel needs to know about: `last_input` (the held `PlayerInput`), `jump_buffer_timer`, `coyote_timer`, `step_view_offset_y`, and `speed_multiplier` (the `/speed` cheat, see below). Seed constructors only: `from_player_state` (Welcome/Correction) and `from_persisted` (save load). There is no `reconcile` method.

Module layout under `src/controller/`:
- `mod.rs`: `PlayerController`, substep loop, jump/coyote/leap, step-up.
- `movement.rs`: speed constants, `desired_horizontal_velocity`, `accelerate_air` (the air cap), camera-relative direction.
- `collision.rs`: swept world-block AABB collision, support checks, `player_overlaps_world`.
- `grid.rs`: `BlockGrid`, the coarse spatial index over `WorldData::blocks`.
- `tests.rs`: substep, step-up, coyote, air-cap, and bunny-hop regression tests. No reconciliation test (reconciliation is not in this module).

## Tuning constants

All feel constants live in `src/controller/mod.rs` and `src/controller/movement.rs`, **not** in `game_balance.rs` (that file owns gameplay balance: combat ranges, deployable/building/furnace values). The comments next to each constant hold the why; read them before changing a number, several encode a deliberate retune away from a value that read badly.

| Constant | Value | File | Note (paraphrased from the code comment) |
| --- | --- | --- | --- |
| `WALK_SPEED` | 5.2 m/s | movement.rs | |
| `RUN_SPEED` | 7.0 m/s | movement.rs | Dropped from 8.4; at 8.4 the terrain "whipped past" and read as teleporting. |
| `SIDE_WALK_SPEED` | 4.4 m/s | movement.rs | |
| `RUN_STRAFE_SPEED` | 5.3 m/s | movement.rs | Scaled with `RUN_SPEED` to keep diagonal-run character. |
| `BACKPEDAL_SPEED` | 3.8 m/s | movement.rs | |
| `AIR_MAX_HORIZONTAL_SPEED` | 7.4 m/s | movement.rs | Air-control ceiling; anti-bunnyhop. Uses `max(cap, entry_speed)`. |
| `GROUND_ACCELERATION` | 68.0 | movement.rs | |
| `GROUND_DECELERATION` | 76.0 | movement.rs | |
| `AIR_ACCELERATION` | 13.0 | movement.rs | |
| `JUMP_SPEED` | 6.8 m/s | mod.rs | |
| `GRAVITY` | 18.0 | mod.rs | |
| `MAX_FALL_SPEED` | 32.0 m/s | mod.rs | |
| `STEP_HEIGHT` | 0.45 m | mod.rs | Step-up ceiling. |
| `PLAYER_RADIUS` | 0.35 m | mod.rs | Capsule radius for collision. |
| `PLAYER_HEIGHT` | 1.8 m | mod.rs | |
| `LEAP_TAKEOFF_SPEED` | 7.25 m/s | mod.rs | Running-jump forward boost; sits just above `RUN_SPEED` so it isn't an exploit. |
| `LEAP_MAX_HORIZONTAL_SPEED` | 7.4 m/s | mod.rs | |
| `JUMP_BUFFER_SECONDS` | 0.18 s | mod.rs | Buffer window for a pressed jump. |
| `COYOTE_TIME_SECONDS` | 0.1 s | mod.rs | Grace window to still jump just after leaving ground. |
| `MAX_LOOK_PITCH` | `FRAC_PI_2 - 0.01` | mod.rs | Pitch clamp, also enforced server-side. |
| `SERVER_EYE_HEIGHT` | 1.62 m | server/movement.rs | Eye offset used for server-side range/LOS checks. |

## The trust boundary: two sites

Movement is intentionally client-authoritative for responsiveness (CLAUDE.md owns that invariant; do not convert to server-authoritative input simulation unless explicitly asked). The client runs the full `PlayerController` locally and ships the **integrated result** as `PlayerMovement` (`src/protocol/world.rs - PlayerMovement`: sequence, position, velocity, yaw, pitch, grounded). It does NOT ship `PlayerInput`; `PlayerInput` is client-local and never serialized. The server does not re-simulate; it accepts the reported pose after a small set of guards spread across **two files**.

**Site 1, `accept_client_movement` (`src/server/movement.rs:59`)** does two checks and then assigns:
- **Sequence monotonicity**: rejects if `movement.sequence <= controller.last_processed_input` (strictly-increasing required, drops replays/stale frames).
- **Finiteness**: `movement_is_finite` rejects any non-finite position/velocity/yaw/pitch.
- On accept it assigns `position`, `velocity`, `grounded` verbatim, then post-processes view only: `normalize_yaw` (wraps to `[-PI, PI)`) and `pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH)`.

**Site 2, the alive gate (`src/server/dispatch.rs:36`)** is in the `ClientMessage::Movement` handler, the caller. It only invokes `accept_client_movement` when `client.lifecycle.is_alive()`. A dead client's movement is silently dropped so the corpse stays pinned at its death position (stable anchor for the tilt-and-fade and the loot pile). On accept, the handler also calls `chunk_manager.update_player_chunk` to keep the player's AoI ring current.

Death-pinning is NOT in `movement.rs`. An agent grepping `movement.rs` for the dead-player logic will not find it; it is in `dispatch.rs`. To change what the server accepts, edit both sites.

What the server deliberately does **not** validate (confirmed: no AABB or delta-distance guard exists in `dispatch.rs` or `movement.rs`):
- **Collision validity**: no world-block AABB check on the reported position. A modified client can clip walls.
- **Teleport / speed plausibility**: no max-step distance between accepted movements. A modified client can teleport or move arbitrarily fast.
- **Look-direction physics**: yaw/pitch are clamped to a finite range but otherwise trusted.

This is a conscious trade-off. Server-authoritative movement would require either re-simulating the controller server-side and reconciling (adds end-to-end latency to every step) or plausibility checks (max delta per tick, anti-teleport) that bite legitimately fast network conditions with false positives. **When to revisit**: competitive PvP where speed/wallhack cheats affect outcomes, or a sustained cheating incident. Re-introducing server-side simulation is a non-trivial architecture change; discuss before starting.

**Why the exploit surface is narrow:** every gameplay action re-validates the client's reported feet position (`client.controller.position`) at the moment it fires, against `game_balance.rs` ranges:
- Combat: `attacker_pos.within_horizontal_range(target_pos, ATTACK_RANGE_M)` plus a `BlockGrid` line-of-sight check (`src/server/combat.rs - line_of_sight_clear`).
- Gather: `player_eye_position(client.controller.position)` distance to the node (`src/server/resource_nodes.rs:52`).
- Furnace/loot-bag interact: `player_pos.within_horizontal_range(entity.position, FURNACE_INTERACT_RANGE_M)` (`src/server/furnace/commands.rs - within_horizontal_range`).

So a wallhack can make you *appear* somewhere implausible, but you cannot gather a tree you are not next to or punch a player across the map. When you add a new ranged action, re-validate against `client.controller.position` yourself; do not assume movement was validated upstream, it was not.

The accepted pose is replicated to peers as the `PlayerPose` component on the player's mirror entity (refreshed every tick while moving in `src/net/host/mirror.rs`), through per-component Lightyear replication, NOT a `ServerMessage` snapshot. See [replication.md](replication.md).

## Client prediction and send pacing

`client_input_system` (`src/app/systems/input/movement.rs - client_input_system`) runs every render frame. It:
1. Early-returns unless `gameplay_simulation_allowed(menu)` (in-game screen) and a `client_id` exists.
2. Builds `PlayerInput` from WASD/run/jump/look, zeroing the gameplay bits when `gameplay_accepts_movement` is false (see gating below).
3. Sets `predicted.speed_multiplier` from the replicated `/speed` value, then `predicted.apply_input(input)` and `predicted.simulate_with_grid(delta, grid)` against the cached `runtime.world_grid` (no per-frame grid rebuild).
4. Emits a `PlayerMovement` from the integrated result and sends it, paced.

Prediction runs every frame; the **wire send is decoupled and throttled**. `MOVEMENT_SEND_RATE_HZ = SERVER_TICK_RATE_HZ * 1.5 = 30 Hz` while the state is changing, `MOVEMENT_IDLE_SEND_RATE_HZ = 1 Hz` when fully stationary (`SERVER_TICK_RATE_HZ = 20.0`, `src/protocol/mod.rs - SERVER_TICK_RATE_HZ`). The server keeps only the newest movement per tick and the sequence guard makes latest-state-wins; a dropped send loses nothing, the next send carries the integrated result. Do not "fix" the pacing back to per-frame: at 144 fps that was ~144 msgs/s with ~85% overwritten unread, which was the bug this pacing fixes. On a send error the system leaves `last_sent` stale so the next interval retries as a changed send.

Loopback singleplayer and direct multiplayer use this exact path (`client_input_system` always emits `ClientMessage::Movement` through `session.send`; the server handles it identically for loopback host and dedicated). There is no separate singleplayer movement code.

## Client reconciliation (in runtime.rs, not the controller)

Reconciliation lives in `src/app/state/runtime.rs`, not in the controller. If you are debugging rubber-banding, this is the code to read.

- **Welcome seed** (`seed_local_prediction`, `src/app/state/runtime.rs - seed_local_prediction`): on `ServerMessage::Welcome` the client builds `predicted_local` from `Welcome.local_seed` via `PlayerController::from_player_state` and bumps `input_sequence` to the seed's `last_processed_input`. This bootstraps prediction.
- **Correction** (`apply_non_movement_correction`, `src/app/state/runtime.rs:552`): on `ServerMessage::Correction(PlayerState)` the client **always** overwrites `health` (server owns damage). Position/velocity/yaw/pitch only snap when they diverge past `SNAP_THRESHOLD_M = 1.0` m; below that, small per-tick drift is left alone so the player is not yanked off-screen. 1 m is bigger than any single-tick run-speed step and small enough that a real desync still corrects. Past the threshold it rebuilds `predicted_local` from the corrected `PlayerState`. Teleport, respawn, and anti-cheat snap-backs work by sending a `PlayerState` that diverges past the threshold.

`PlayerState` (`src/protocol/world.rs - PlayerState`) is the prediction-seed wire shape (client_id, position, velocity, yaw, pitch, health, grounded, last_processed_input). All other per-player state moved off the wire to Lightyear component replication.

## Feel and anti-exploit details

These are the easy-to-break invariants. Each has a guarding test in `src/controller/tests.rs`.

- **Air-strafe cap with knockback preservation.** `accelerate_air` (`src/controller/movement.rs:127`) clamps the horizontal magnitude to `max(AIR_MAX_HORIZONTAL_SPEED * speed_multiplier, entry_speed)`. Air control can never *gain* speed past the cap (kills diagonal air-strafe bunny-hop ratcheting), but it never *crushes* a pre-existing over-speed (knockback, a forward leap). Clamping to a bare cap would silently kill knockback feel. Guarded by `air_strafing_cannot_ratchet_speed_past_the_air_cap` and `air_control_preserves_knockback_overspeed`.
- **Jump buffer decays only while grounded** (`src/controller/mod.rs - simulate_step`). A press anywhere in the jump arc persists until landing and fires on the first touch-down substep; that is what makes bunny-hopping work. The buffer is reset to 0 the instant a jump fires, so it cannot accumulate ghost jumps across long airtime. Do not "simplify" it to decay-always. Guarded by `early_air_press_still_fires_jump_on_landing`, `rapid_tap_bunny_hops_on_every_landing`, `buffer_does_not_auto_fire_without_a_press`.
- **High-framerate jump not smothered.** When grounded, downward velocity is clamped to 0 but upward velocity is left alone, because at high fps a fresh jump's first substep moves only a few cm and still reads `grounded`. Replacing upward `JUMP_SPEED` with 0 there would silently eat the jump. Guarded by `high_framerate_jump_is_not_smothered_by_grounded_clamp`.
- **Coyote time** (`COYOTE_TIME_SECONDS` 0.1 s): the jump remains available for a short grace window after leaving the ground.
- **Leap takeoff** (`apply_leap_takeoff`): a running jump (run held, forward input past `LEAP_FORWARD_INPUT_THRESHOLD`) gets a forward boost up to `LEAP_MAX_HORIZONTAL_SPEED`, tuned just above `RUN_SPEED` so it is a feel boost, not an exploit. Guarded by `run_jump_creates_modest_forward_boost`.
- **Diagonal speed cap** (`desired_horizontal_velocity`): forward and strafe use different per-axis speeds, so a raw diagonal would combine to a larger magnitude than either (`sqrt(5.3^2 + 7.0^2) ~= 8.8`). The combined magnitude is clamped to the faster axis so angled movement is never quicker than a straight run. Guarded by `running_is_forward_weighted_and_sidewalking_is_slower`.

## The `/speed` admin cheat

`speed_multiplier` on `PlayerController` scales every gait, the leap caps, and the air ceiling (default `1.0`). It is the only thing that makes the client controller's speeds non-constant. The admin `/speed` command sets `PlayerInputAck.run_speed_multiplier` server-side (`src/server/player_ecs.rs - PlayerInputAck`), Lightyear replicates that owner-only component, the client reassembles it into `PlayerPrivate.run_speed_multiplier`, and `client_input_system` re-applies it to `predicted.speed_multiplier` **every frame** so a Correction rebuild cannot strand it at 1.0. Because the server never simulates movement, the server's `PlayerController.speed_multiplier` copy stays `1.0`. The command clamps the multiplier to a safe range and never `0.0`.

## BlockGrid

`BlockGrid` (`src/controller/grid.rs - BlockGrid`) is a coarse spatial index over `WorldData::blocks`. It is used in production now, not "for future checks":
- Client prediction keeps it in `runtime.world_grid` and passes it to `simulate_with_grid` to avoid per-frame rebuilds.
- The server holds it on `GameServer` as `world_grid` (`src/server/lifecycle.rs - world_grid`) and uses it for combat line-of-sight (`src/server/combat.rs - line_of_sight_clear`) and spawn-placement overlap (`spawn_collision_grid` + `player_overlaps_world`, `src/server/combat.rs` and `src/server/sleeping_bag.rs`).

Use `BlockGrid::build_with_extras` to fold dynamic colliders (tree trunks, deployables) into the static `WorldData::blocks` so the grid matches what the client actually collides with.

## Gating: when prediction runs vs when input is zeroed

Two gates in `src/app/systems/input/gating.rs` decide movement behaviour while a menu is open. They implement the CLAUDE.md "gameplay never pauses" invariant for this slice:
- `gameplay_simulation_allowed(menu)`: true while the screen is `InGame`. Prediction and network ticks keep running regardless of which overlay is up. Only leaving the in-game screen halts simulation.
- `gameplay_accepts_movement(menu, focused)`: true only when focused and no blocking modal is open (the world map is navigable, so it does NOT block movement, only look/swing). When false, `client_input_system` zeroes the WASD/run/jump bits but still runs `simulate_with_grid` (so server-pushed effects keep applying).

If you add a new overlay, gate it through `gameplay_accepts_controls` / `gameplay_accepts_movement`, never through `gameplay_simulation_allowed`. See [gameplay-gating.md](gameplay-gating.md).

## Related docs

- [gameplay-gating.md](gameplay-gating.md): the full control-gating model and the gameplay-never-pauses invariant this slice obeys.
- [replication.md](replication.md): `PlayerPose` mirror sync and per-component replication of the accepted pose.
- [pvp-combat.md](pvp-combat.md): combat range and line-of-sight re-validation that backs the narrow-exploit-surface claim.
- [networking.md](networking.md): the shared `ClientMessage`/`ServerMessage` transport both singleplayer and multiplayer drive.
- [server-authority.md](server-authority.md): how `GameServer` owns and dispatches the `ClientMessage::Movement` handler.
