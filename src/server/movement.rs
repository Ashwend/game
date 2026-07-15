use std::f32::consts::{PI, TAU};

use crate::{
    controller::{MAX_LOOK_PITCH, PlayerController},
    items::look_forward,
    protocol::{ClientId, PlayerMovement, Vec3Net},
};

use super::ServerClient;

use crate::game_balance::{
    DROPPED_ITEM_TOSS_FORWARD_DISTANCE_M as DROP_FORWARD_DISTANCE,
    DROPPED_ITEM_TOSS_FORWARD_SPEED_MPS as DROP_FORWARD_SPEED,
    DROPPED_ITEM_TOSS_INHERITED_VELOCITY_SCALE as DROP_INHERITED_VELOCITY_SCALE,
    DROPPED_ITEM_TOSS_UP_SPEED_MPS as DROP_UP_SPEED,
};

pub(super) const SERVER_EYE_HEIGHT: f32 = 1.62;
const DROPPED_ITEM_DROP_HEIGHT: f32 = SERVER_EYE_HEIGHT + 0.04;

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

/// Where a stack a player drops originates: their feet-plus-forward toss
/// position, the inherited+forward toss velocity, and their facing yaw. One
/// type so every "drop these stacks at the player" site (inventory drop, craft
/// refund/overflow, furnace eject) shares the same physics instead of
/// re-inlining the `(drop_position, drop_velocity, yaw)` tuple.
#[derive(Debug, Clone, Copy)]
pub(super) struct DropOrigin {
    pub(super) position: Vec3Net,
    pub(super) velocity: Vec3Net,
    pub(super) yaw: f32,
}

pub(super) fn drop_origin_for(client: &ServerClient) -> DropOrigin {
    DropOrigin {
        position: drop_position(&client.controller),
        velocity: drop_velocity(&client.controller),
        yaw: client.controller.yaw,
    }
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
    // Strip control characters before the length cap, matching `sanitize_chat`
    // and `sanitize_marker_name`. The resolved name is broadcast to every peer
    // and rendered in nametags and the roster, so a control char is never
    // wanted and would otherwise be a peer-to-peer UI-corruption vector. Re-trim
    // afterwards in case removing one exposed surrounding whitespace, then fall
    // back to a numbered name if nothing legible remains.
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(32)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        format!("Player {fallback_id}")
    } else {
        cleaned.to_owned()
    }
}
