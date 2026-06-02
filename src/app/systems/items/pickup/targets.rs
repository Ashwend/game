//! Per-target-type resolution helpers for the pickup/interact tooltip.
//!
//! Each `best_*_target` scores the closest entity of one category along
//! the look ray (or inside its interact cone); the matching `set_*` helper
//! writes the winning entity's id and world/screen anchor into
//! [`PickupTargetState`]. The dispatcher lives in the parent module's
//! `update_pickup_target_system`.

use bevy::prelude::*;

use crate::{
    app::{
        scene::{MainCamera, NetworkDroppedItem},
        state::PickupTargetState,
    },
    items::{item_definition, look_forward, pickup_anchor_from_position},
    protocol::Vec3Net,
    resources::resource_node_anchor_for,
    server::{
        Deployable, DeployableTransform, DroppedItem, DroppedItemTransform, LootBagEntity,
        LootBagTransform, Player, PlayerPublic, ResourceNode, ResourceNodeStorage,
    },
};

use super::viewport_position;

/// Max range at which `E` lands on a placed structure. Matches the
/// furnace open-range so the tooltip never lies about reachability.
const DEPLOYABLE_INTERACT_RANGE_M: f32 = 5.5;
/// Cone half-angle (cosine) the player must aim through to lock onto
/// a deployable. Tight enough that the tooltip doesn't latch when the
/// player is mostly looking past the structure.
const DEPLOYABLE_INTERACT_CONE_COS: f32 = 0.92;
/// Max range at which a melee swing can reach another player. Tighter
/// than gather range, players are smaller targets than ore veins, so
/// we need them well inside arm's reach before "swing at player" wins
/// over "swing at the deployable behind them".
pub(super) const ATTACK_RANGE_M: f32 = 3.0;
/// Player body AABB half-extents used for the look-ray test. Roughly
/// matches the controller's `(PLAYER_RADIUS, PLAYER_HEIGHT/2,
/// PLAYER_RADIUS)` so "looking at the avatar" ≈ "swing hits".
/// Slightly larger than the collider so the hit volume is forgiving
/// at typical move/strafe speeds.
const PLAYER_BODY_HALF_WIDTH: f32 = 0.40;
const PLAYER_BODY_HALF_HEIGHT: f32 = 0.95;
/// Vertical offset from the player's feet to the AABB centre. Matches
/// `PLAYER_VISUAL_CENTER_Y` so the look-ray hits the same volume the
/// remote avatar visually occupies.
const PLAYER_BODY_CENTRE_Y: f32 = 0.95;
/// Max range, in metres, at which E latches onto a loot bag. Matches
/// the server's `LOOT_BAG_INTERACT_RANGE_M` so the tooltip never
/// lies about reachability.
const LOOT_BAG_INTERACT_RANGE_M: f32 = 4.5;
/// Cone cosine for loot bag interaction, same as deployables since
/// bags sit at roughly the same eye-level cone an aimed E would
/// expect to hit.
const LOOT_BAG_INTERACT_CONE_COS: f32 = 0.92;

/// Closest loot bag inside the player's interact cone. Score is the
/// straight-line distance from eye → bag origin, used directly
/// against the other category scores.
pub(super) fn best_loot_bag_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    bags: impl Iterator<Item = (&'a LootBagEntity, &'a LootBagTransform)>,
) -> Option<(&'a LootBagEntity, &'a LootBagTransform, f32)> {
    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let max_sq = LOOT_BAG_INTERACT_RANGE_M * LOOT_BAG_INTERACT_RANGE_M;
    let mut best: Option<(&LootBagEntity, &LootBagTransform, f32)> = None;
    for (meta, transform) in bags {
        let aim = Vec3Net::new(
            transform.position.x,
            transform.position.y + 0.4,
            transform.position.z,
        );
        let to = aim.minus(eye);
        let dist_sq = to.length_squared();
        if dist_sq > max_sq {
            continue;
        }
        let dist = dist_sq.sqrt();
        if dist <= 1e-3 {
            return Some((meta, transform, 0.0));
        }
        if to.dot(forward) / dist < LOOT_BAG_INTERACT_CONE_COS {
            continue;
        }
        if best.as_ref().map(|(_, _, s)| dist < *s).unwrap_or(true) {
            best = Some((meta, transform, dist));
        }
    }
    best
}

pub(super) fn set_loot_bag_pickup_target(
    pickup_target: &mut PickupTargetState,
    meta: &LootBagEntity,
    transform: &LootBagTransform,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.loot_bag_id = Some(meta.id);
    let anchor = Vec3Net::new(
        transform.position.x,
        transform.position.y + 0.4,
        transform.position.z,
    );
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

/// Find the closest remote player whose body AABB is hit by the look
/// ray within [`ATTACK_RANGE_M`]. Score is the ray-AABB entry distance
/// so it slots into the same min-score pick as the other target
/// categories.
pub(super) fn best_player_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    local_client_id: Option<crate::protocol::ClientId>,
    players: impl Iterator<Item = (&'a Player, &'a PlayerPublic)>,
) -> Option<(&'a Player, &'a PlayerPublic, f32)> {
    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let mut best: Option<(&Player, &PlayerPublic, f32)> = None;
    for (player, public) in players {
        if Some(player.client_id) == local_client_id {
            continue;
        }
        // Dead targets don't count; the swing should fall through to
        // whatever is behind them. Health below 1 (saturating-clamped
        // at zero on the server) is the "dead" marker today; Phase 5
        // replaces this with `PlayerLifecycle::Dead`.
        if public.health <= 0.0 {
            continue;
        }
        let centre = Vec3Net::new(
            public.position.x,
            public.position.y + PLAYER_BODY_CENTRE_Y,
            public.position.z,
        );
        let Some(distance) = ray_aabb_entry_distance(
            eye,
            forward,
            centre,
            PLAYER_BODY_HALF_WIDTH,
            PLAYER_BODY_HALF_HEIGHT,
        ) else {
            continue;
        };
        if distance > ATTACK_RANGE_M {
            continue;
        }
        if best.as_ref().map(|(_, _, s)| distance < *s).unwrap_or(true) {
            best = Some((player, public, distance));
        }
    }
    best
}

/// Slab-method ray-AABB intersection. Returns the entry distance along
/// `direction` if the ray (with finite length cap) enters the box from
/// outside; `None` when there's no hit or the box is behind the eye.
/// `direction` is assumed normalised.
pub(super) fn ray_aabb_entry_distance(
    origin: Vec3Net,
    direction: Vec3Net,
    centre: Vec3Net,
    half_width: f32,
    half_height: f32,
) -> Option<f32> {
    let min = Vec3Net::new(
        centre.x - half_width,
        centre.y - half_height,
        centre.z - half_width,
    );
    let max = Vec3Net::new(
        centre.x + half_width,
        centre.y + half_height,
        centre.z + half_width,
    );

    let mut t_near: f32 = f32::NEG_INFINITY;
    let mut t_far: f32 = f32::INFINITY;
    for axis in 0..3 {
        let (o, d, mn, mx) = match axis {
            0 => (origin.x, direction.x, min.x, max.x),
            1 => (origin.y, direction.y, min.y, max.y),
            _ => (origin.z, direction.z, min.z, max.z),
        };
        if d.abs() < 1e-6 {
            if o < mn || o > mx {
                return None;
            }
            continue;
        }
        let inv_d = d.recip();
        let mut t1 = (mn - o) * inv_d;
        let mut t2 = (mx - o) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_near = t_near.max(t1);
        t_far = t_far.min(t2);
        if t_near > t_far {
            return None;
        }
    }
    if t_far < 0.0 {
        return None;
    }
    // Inside the box: return 0 so the cosmetic "you're poking the
    // target" case still resolves as a valid hit at point-blank.
    Some(t_near.max(0.0))
}

pub(super) fn set_player_pickup_target(
    pickup_target: &mut PickupTargetState,
    meta: &Player,
    public: &PlayerPublic,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.player_id = Some(meta.client_id);
    let anchor = Vec3Net::new(
        public.position.x,
        public.position.y + PLAYER_BODY_CENTRE_Y,
        public.position.z,
    );
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

/// Find the closest placed structure inside the player's look cone.
/// `score` is the distance from the eye to the structure centre so the
/// caller can compare it directly against the dropped-item / resource-
/// node ray scores.
pub(super) fn best_deployable_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    deployables: impl Iterator<Item = (&'a Deployable, &'a DeployableTransform)>,
) -> Option<(&'a Deployable, &'a DeployableTransform, f32)> {
    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let max_sq = DEPLOYABLE_INTERACT_RANGE_M * DEPLOYABLE_INTERACT_RANGE_M;
    let mut best: Option<(&Deployable, &DeployableTransform, f32)> = None;
    for (meta, transform) in deployables {
        // Aim point sits at half the entity's collider height so the
        // cone test isn't biased toward the floor.
        let aim = deployable_aim_point(meta, transform);
        let to = aim.minus(eye);
        let dist_sq = to.length_squared();
        if dist_sq > max_sq {
            continue;
        }
        let dist = dist_sq.sqrt();
        if dist <= 1e-3 {
            return Some((meta, transform, 0.0));
        }
        let cosine = to.dot(forward) / dist;
        if cosine < DEPLOYABLE_INTERACT_CONE_COS {
            continue;
        }
        let score = dist;
        if best.as_ref().map(|(_, _, s)| score < *s).unwrap_or(true) {
            best = Some((meta, transform, score));
        }
    }
    best
}

pub(super) fn deployable_aim_point(meta: &Deployable, transform: &DeployableTransform) -> Vec3Net {
    // Approximate the structure's optical centre. 0.6 m up reads well
    // for both the workbench tabletop and the furnace mouth; the
    // profile-based half-height is preferred when we can resolve it.
    let mut aim = transform.position;
    aim.y += 0.6;
    if let Some(profile) = item_definition(&meta.item_id).and_then(|def| def.deployable) {
        aim.y = transform.position.y + profile.collider_half_height;
    }
    aim
}

pub(super) fn set_deployable_pickup_target(
    pickup_target: &mut PickupTargetState,
    meta: &Deployable,
    transform: &DeployableTransform,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.deployable_id = Some(meta.id);
    pickup_target.deployable_kind = Some(meta.kind);
    let anchor = deployable_aim_point(meta, transform);
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

/// Follow a dropped item's interpolated transform every frame so the tooltip
/// doesn't lag behind a falling stack. The target *selection* still runs on
/// the throttled scan, but as long as the same item stays selected we re-read
/// its current entity transform here. Resource nodes don't move, so their
/// cached anchor is left alone.
pub(super) fn refresh_dropped_target_anchor(
    pickup_target: &mut PickupTargetState,
    dropped_entities: &Query<(&NetworkDroppedItem, &Transform)>,
) {
    let Some(id) = pickup_target.dropped_item_id else {
        return;
    };
    let Some((_, transform)) = dropped_entities
        .iter()
        .find(|(dropped, _)| dropped.id == id)
    else {
        return;
    };
    pickup_target.world_position =
        Some(pickup_anchor_from_position(crate::protocol::Vec3Net::new(
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        )));
}

pub(super) fn set_dropped_pickup_target(
    pickup_target: &mut PickupTargetState,
    drop: &DroppedItem,
    transform: &DroppedItemTransform,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
    dropped_entities: &Query<(&NetworkDroppedItem, &Transform)>,
) {
    pickup_target.clear();
    pickup_target.dropped_item_id = Some(drop.id);
    pickup_target.stack = Some(drop.stack.clone());
    // Prefer the visual entity's interpolated transform when present so
    // the tooltip glues to a still-settling drop. Falls back to the
    // authoritative replicated position if the visual hasn't been
    // spawned yet this frame (rate-limited spawn budget).
    let anchor = dropped_entities
        .iter()
        .find(|(dropped, _)| dropped.id == drop.id)
        .map(|(_, visual)| {
            pickup_anchor_from_position(crate::protocol::Vec3Net::new(
                visual.translation.x,
                visual.translation.y,
                visual.translation.z,
            ))
        })
        .unwrap_or_else(|| pickup_anchor_from_position(transform.position));
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

pub(super) fn set_resource_pickup_target(
    pickup_target: &mut PickupTargetState,
    node: &ResourceNode,
    storage: &ResourceNodeStorage,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.resource_node_id = Some(node.id);
    pickup_target.resource_definition_id = Some(node.definition_id.clone());
    pickup_target.resource_storage = storage.0.clone();
    let anchor = resource_node_anchor_for(&node.definition_id, node.position);
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}
