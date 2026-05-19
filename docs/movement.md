# Movement

`PlayerController` stores feet position, velocity, yaw/pitch, health, grounded state, and input sequence.

Flow:
- Client builds `PlayerInput` from WASD, shift, space, and mouse look.
- The client predicts locally with `PlayerController`, then sends `PlayerMovement` through the shared `ClientSession::Network` path.
- Loopback singleplayer and direct multiplayer use the same Lightyear client/host message flow.
- `GameServer` accepts newer finite movement states, normalizes/clamps view angles, and broadcasts authoritative snapshots.
- Future movement authority changes should happen in `PlayerController`, `ClientMessage`/`ServerMessage`, and `GameServer` so singleplayer and multiplayer keep exercising the same code.

Movement lives in `src/controller/`:
- `mod.rs`: `PlayerController`, fixed-step simulation, jumping, coyote time, reconciliation, and step-up handling.
- `movement.rs`: walk/sprint speeds, horizontal acceleration, air acceleration, and camera-relative movement vectors.
- `collision.rs`: world-block AABB collision and support checks.
- `grid.rs`: `BlockGrid`, a coarse spatial index over `WorldData::blocks` used by collision and held by `GameServer` for future server-side checks.
- `tests.rs`: substep, step-up, coyote-time, and reconciliation regression tests.
