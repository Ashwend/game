# Movement

`PlayerController` stores feet position, velocity, yaw/pitch, health, grounded state, and input sequence.

Flow:
- Client builds `PlayerInput` from WASD, shift, space, and mouse look.
- Lightyear sends native inputs and owns prediction/interpolation.
- Server simulates authoritative player controllers and replicates player components.
- Local singleplayer still uses direct in-process messages.

Rules live in `src/controller.rs`: walk/sprint speed, gravity, jump buffer, coyote time, and fixed-step collision. Collision uses world-block AABBs in `src/controller/collision.rs`.
