# Movement

`PlayerController` stores feet position, velocity, yaw/pitch, health, grounded state, and input sequence.

Flow:
- Client builds `PlayerInput` from WASD, shift, space, and mouse look.
- Local singleplayer predicts locally, sends `PlayerMovement` through direct in-process messages, and receives snapshots/corrections from `GameServer`.
- The Lightyear dedicated server registers native inputs and replicated player components.
- The Lightyear dedicated server simulates authoritative player controllers from `NetworkInput`.

Rules live in `src/controller.rs`: walk/sprint speed, gravity, jump buffer, coyote time, and fixed-step collision. Collision uses world-block AABBs in `src/controller/collision.rs`.
