use std::f32::consts::{PI, TAU};

use crate::{
    controller::{MAX_LOOK_PITCH, PlayerController},
    items::look_forward,
    protocol::{ClientId, PlayerMovement, Vec3Net},
};

pub(super) const SERVER_EYE_HEIGHT: f32 = 1.62;
const DROP_FORWARD_DISTANCE: f32 = 0.48;
const DROPPED_ITEM_DROP_HEIGHT: f32 = SERVER_EYE_HEIGHT + 0.04;
const DROP_INHERITED_VELOCITY_SCALE: f32 = 0.65;
const DROP_FORWARD_SPEED: f32 = 1.6;
const DROP_UP_SPEED: f32 = 0.45;

pub(super) fn drop_position(controller: &PlayerController) -> Vec3Net {
    let forward = Vec3Net::new(-controller.yaw.sin(), 0.0, -controller.yaw.cos());
    controller
        .position
        .plus(forward.scale(DROP_FORWARD_DISTANCE))
        .plus(Vec3Net::new(0.0, DROPPED_ITEM_DROP_HEIGHT, 0.0))
}

pub(super) fn drop_velocity(controller: &PlayerController) -> Vec3Net {
    let forward = look_forward(controller.yaw, controller.pitch).normalize_or_zero();
    controller
        .velocity
        .scale(DROP_INHERITED_VELOCITY_SCALE)
        .plus(forward.scale(DROP_FORWARD_SPEED))
        .plus(Vec3Net::new(0.0, DROP_UP_SPEED, 0.0))
}

pub(super) fn player_eye_position(position: Vec3Net) -> Vec3Net {
    position.plus(Vec3Net::new(0.0, SERVER_EYE_HEIGHT, 0.0))
}

pub(super) fn accept_client_movement(controller: &mut PlayerController, movement: PlayerMovement) {
    if movement.sequence <= controller.last_processed_input || !movement_is_finite(movement) {
        return;
    }

    controller.position = movement.position;
    controller.velocity = movement.velocity;
    controller.yaw = normalize_yaw(movement.yaw);
    controller.pitch = movement.pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
    controller.grounded = movement.grounded;
    controller.last_processed_input = movement.sequence;
}

fn movement_is_finite(movement: PlayerMovement) -> bool {
    vec3_is_finite(movement.position)
        && vec3_is_finite(movement.velocity)
        && movement.yaw.is_finite()
        && movement.pitch.is_finite()
}

fn vec3_is_finite(value: Vec3Net) -> bool {
    value.x.is_finite() && value.y.is_finite() && value.z.is_finite()
}

fn normalize_yaw(yaw: f32) -> f32 {
    (yaw + PI).rem_euclid(TAU) - PI
}

pub(super) fn clean_player_name(name: &str, fallback_id: ClientId) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("Player {fallback_id}")
    } else {
        trimmed.chars().take(32).collect()
    }
}
