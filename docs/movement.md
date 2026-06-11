# Movement

`PlayerController` stores feet position, velocity, yaw/pitch, health, grounded state, and input sequence.

Flow:
- Client builds `PlayerInput` from WASD, shift, space, and mouse look.
- The client predicts locally with `PlayerController` every render frame, then sends `PlayerMovement` through the shared `ClientSession::Network` path. Sends are paced, not per-frame: ~1.5x the server tick rate while the state is changing, a 1 Hz keep-alive while fully stationary (the server keeps only the newest movement per tick, so frame-rate-coupled sends were mostly discarded on arrival; see `MOVEMENT_SEND_RATE_HZ` in `src/app/systems/input/movement.rs`).
- Loopback singleplayer and direct multiplayer use the same Lightyear client/host message flow.
- `GameServer` accepts newer finite movement states, normalizes/clamps view angles, and writes the accepted pose onto the player's ECS mirror entity. Lightyear replicates the resulting `PlayerPose` to peers in the same chunk room, see [Networking § Replication](networking.md#replication).
- Future movement authority changes should happen in `PlayerController`, `ClientMessage`/`ServerMessage`, and `GameServer` so singleplayer and multiplayer keep exercising the same code.

Movement lives in `src/controller/`:
- `mod.rs`: `PlayerController`, fixed-step simulation, jumping, coyote time, reconciliation, and step-up handling.
- `movement.rs`: walk/run speeds, horizontal acceleration, air acceleration, and camera-relative movement vectors.
- `collision.rs`: world-block AABB collision and support checks.
- `grid.rs`: `BlockGrid`, a coarse spatial index over `WorldData::blocks` used by collision and held by `GameServer` for future server-side checks.
- `tests.rs`: substep, step-up, coyote-time, and reconciliation regression tests.

## Trust boundary

Movement is intentionally **client-authoritative for responsiveness**. The client runs its full `PlayerController` simulation locally, then ships the resulting `PlayerMovement` state to the server. The server does not re-simulate the input, it accepts the client's reported pose if it passes a small set of cheap guards.

What the server validates:
- **Input sequence monotonicity.** Accepts a movement only if its `sequence` is strictly greater than the last one applied for that client (`src/server/movement.rs`). Replays and stale frames are dropped.
- **Finiteness.** Position, velocity, yaw, and pitch must all be finite floats. NaN/Inf is rejected outright.
- **Death pinning.** Dead players are pinned at their death position; movement messages from a dead client are ignored until respawn.

What the server deliberately does **not** validate:
- **Collision validity.** No world-block AABB check on the reported position. A modified client can clip through walls.
- **Teleport / speed plausibility.** No max-step distance between consecutive accepted movements. A modified client can teleport or move arbitrarily fast.
- **Look-direction physics.** Yaw/pitch are clamped to a finite range but otherwise trusted.

This is a conscious trade-off. Server-authoritative movement would require either (a) running the full controller simulation server-side and reconciling client-side, which adds end-to-end latency to every step, or (b) running plausibility checks (max delta per tick, anti-teleport) that bite legitimately fast network conditions and add false-positive friction.

**When to revisit:** if the game adds competitive PvP modes where speed/wallhack cheats meaningfully affect outcomes, or if a sustained cheating incident is reported. Re-introducing server-side simulation is a non-trivial architecture change, discuss before starting.

**What you can rely on right now:** combat ranges, gather ranges, placement ranges, furnace/loot-bag interact ranges, and inventory operations all re-validate the client's reported position server-side at the moment they fire. So a wallhack can move you to the wrong place, but you can't gather a tree you're not actually next to, or punch a player from across the map. The exploit surface from client-authoritative movement is "I appear to be standing somewhere implausible", not "I can damage/loot/gather from there."
