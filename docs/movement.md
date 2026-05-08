# Movement

`PlayerController` stores feet position, velocity, yaw/pitch, health, grounded state, and input sequence.

Flow:
- Client builds `PlayerInput` from WASD, shift, space, and mouse look.
- Local singleplayer predicts locally, sends `PlayerMovement` through direct in-process messages, and receives snapshots/corrections from `GameServer`.
- The Lightyear dedicated server registers native inputs and replicated player components.
- The Lightyear dedicated server simulates authoritative player controllers from `NetworkInput`.

Movement lives in `src/controller/`:
- `mod.rs`: `PlayerController`, fixed-step simulation, jumping, coyote time, reconciliation, and step-up handling.
- `movement.rs`: walk/sprint speeds, horizontal acceleration, air acceleration, and camera-relative movement vectors.
- `collision.rs`: world-block AABB collision and support checks.
