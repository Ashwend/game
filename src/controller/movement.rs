use crate::protocol::Vec3Net;

pub const WALK_SPEED: f32 = 5.2;
// Boosted speed when holding the run key. Sized to feel like a grounded
// run rather than a full-out sprint, at 8.4 m/s (the previous tuning)
// the camera and terrain whipped past in a way that read as
// teleporting. 7.0 m/s keeps a clear speed delta from `WALK_SPEED`
// without losing the player's sense of scale.
pub const RUN_SPEED: f32 = 7.0;
const SIDE_WALK_SPEED: f32 = 4.4;
// Scaled proportionally with `RUN_SPEED` so diagonal running keeps
// roughly the same forward-vs-strafe character it had before the
// run-speed reduction.
const RUN_STRAFE_SPEED: f32 = 5.3;
const BACKPEDAL_SPEED: f32 = 3.8;
const GROUND_ACCELERATION: f32 = 68.0;
const GROUND_DECELERATION: f32 = 76.0;
const AIR_ACCELERATION: f32 = 13.0;
// Ceiling on the speed air control is allowed to *build toward*. Kept in
// line with the leap takeoff cap (`LEAP_MAX_HORIZONTAL_SPEED`, 7.4) so a
// jump never lets you accelerate past what a forward leap already grants.
// The old 12.0 let diagonal air-strafe bunny-hopping ramp well past run
// speed, which read as a movement exploit rather than intended feel.
// Pre-existing over-speed (knockback, a leap) is preserved: `accelerate_air`
// clamps to `max(cap, entry speed)`, so it only ever prevents *gaining*
// speed past this ceiling.
pub(super) const AIR_MAX_HORIZONTAL_SPEED: f32 = 7.4;

pub fn first_person_move_direction(input: Vec3Net, yaw: f32) -> Vec3Net {
    let input = clamped_local_move_input(input).normalize_or_zero();
    if input.length_squared() == 0.0 {
        return Vec3Net::ZERO;
    }

    rotate_local_horizontal(input, yaw).normalize_or_zero()
}

pub(super) fn desired_horizontal_velocity(
    input: Vec3Net,
    yaw: f32,
    run: bool,
    speed_multiplier: f32,
) -> Vec3Net {
    let input = clamped_local_move_input(input);
    if input.length_squared() == 0.0 {
        return Vec3Net::ZERO;
    }

    // Admin `/speed` cheat: scale every gait. `1.0` (the default) leaves the
    // tuned speeds untouched.
    let forward_speed = speed_multiplier
        * if input.z < 0.0 {
            BACKPEDAL_SPEED
        } else if run && input.z > 0.0 {
            RUN_SPEED
        } else {
            WALK_SPEED
        };
    let side_speed = speed_multiplier
        * if run && input.z > 0.0 {
            RUN_STRAFE_SPEED
        } else {
            SIDE_WALK_SPEED
        };
    // Forward and strafe use different per-axis speeds, so a raw diagonal
    // input combines them into a magnitude larger than either, e.g. running
    // diagonally was sqrt(5.3^2 + 7.0^2) ~= 8.8 m/s, noticeably faster than
    // the 7.0 m/s straight run. Clamp the combined magnitude to the faster of
    // the two axes so moving at an angle is never quicker than moving straight
    // along your fastest direction.
    let max_speed = side_speed.max(forward_speed);
    let local_velocity = clamp_horizontal_speed(
        Vec3Net::new(input.x * side_speed, 0.0, input.z * forward_speed),
        max_speed,
    );
    rotate_local_horizontal(local_velocity, yaw)
}

pub(super) fn clamped_local_move_input(input: Vec3Net) -> Vec3Net {
    let input = Vec3Net::new(input.x.clamp(-1.0, 1.0), 0.0, input.z.clamp(-1.0, 1.0));
    if input.length_squared() > 1.0 {
        input.normalize_or_zero()
    } else {
        input
    }
}

fn rotate_local_horizontal(input: Vec3Net, yaw: f32) -> Vec3Net {
    let forward = Vec3Net::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3Net::new(yaw.cos(), 0.0, -yaw.sin());
    right.scale(input.x).plus(forward.scale(input.z))
}

pub(super) fn approach_horizontal(
    mut current: Vec3Net,
    target: Vec3Net,
    delta_seconds: f32,
    accelerating: bool,
) -> Vec3Net {
    let max_delta = if accelerating {
        GROUND_ACCELERATION
    } else {
        GROUND_DECELERATION
    } * delta_seconds;
    let difference = Vec3Net::new(target.x - current.x, 0.0, target.z - current.z);
    let distance = horizontal_length(difference);
    if distance <= max_delta || distance <= f32::EPSILON {
        current.x = target.x;
        current.z = target.z;
    } else {
        let scale = max_delta / distance;
        current.x += difference.x * scale;
        current.z += difference.z * scale;
    }
    current
}

pub(super) fn approach(current: f32, target: f32, max_delta: f32) -> f32 {
    let difference = target - current;
    if difference.abs() <= max_delta {
        target
    } else {
        current + difference.signum() * max_delta
    }
}

pub(super) fn accelerate_air(
    mut velocity: Vec3Net,
    target_velocity: Vec3Net,
    delta_seconds: f32,
    speed_multiplier: f32,
) -> Vec3Net {
    let target_speed = horizontal_length(target_velocity);
    if target_speed <= f32::EPSILON {
        return velocity;
    }

    // Speed we entered this substep with. Air control may not push the player
    // *past* `AIR_MAX_HORIZONTAL_SPEED`, but it must never slow a pre-existing
    // over-speed either: a server knockback or a fresh forward leap can
    // legitimately exceed the cap. Clamping the result to `max(cap, entry
    // speed)` enforces "air control can't speed you up past the cap" without
    // crushing that momentum the instant a movement key is held.
    let speed_before = horizontal_length(velocity);

    let target_direction = target_velocity.scale(target_speed.recip());
    let current_speed = horizontal_dot(velocity, target_direction);
    let added_speed = target_speed - current_speed;
    if added_speed <= 0.0 {
        return velocity;
    }

    let acceleration = (AIR_ACCELERATION * delta_seconds).min(added_speed);
    velocity.x += target_direction.x * acceleration;
    velocity.z += target_direction.z * acceleration;
    // Hard ceiling on the magnitude. Without it, repeatedly redirecting the
    // input mid-air (diagonal air-strafe bunny-hopping) ratchets the speed up
    // every frame, the exploit this guards against. The `max(.., speed_before)`
    // keeps knockback/leap over-speed intact.
    clamp_horizontal_speed(
        velocity,
        (AIR_MAX_HORIZONTAL_SPEED * speed_multiplier).max(speed_before),
    )
}

pub(super) fn clamp_horizontal_speed(mut velocity: Vec3Net, max_speed: f32) -> Vec3Net {
    let speed = horizontal_length(velocity);
    if speed > max_speed {
        let scale = max_speed / speed;
        velocity.x *= scale;
        velocity.z *= scale;
    }
    velocity
}

pub(super) fn horizontal_length(value: Vec3Net) -> f32 {
    horizontal_length_squared(value).sqrt()
}

fn horizontal_length_squared(value: Vec3Net) -> f32 {
    value.x.mul_add(value.x, value.z * value.z)
}

pub(super) fn horizontal_dot(a: Vec3Net, b: Vec3Net) -> f32 {
    a.x.mul_add(b.x, a.z * b.z)
}
