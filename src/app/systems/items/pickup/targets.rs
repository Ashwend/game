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
        state::{LocalPlayerState, PickupTargetState},
    },
    // The interact ranges come straight from the authoritative balance
    // constants (not redefined here) so the client tooltip/targeting can never
    // disagree with what the server will accept, no "tooltip says reachable,
    // server says no" pops, and there is a single tuning knob per range.
    game_balance::{
        COMBAT_ATTACK_RANGE_M, COMBAT_PLAYER_BODY_CENTRE_Y as PLAYER_BODY_CENTRE_Y,
        COMBAT_PLAYER_TARGET_REACH_MARGIN_M,
        COMBAT_SLEEPING_BODY_CENTRE_Y as SLEEPING_BODY_CENTRE_Y,
        DEPLOYABLE_DAMAGE_RANGE_M as DEPLOYABLE_INTERACT_RANGE_M, LOOT_BAG_INTERACT_RANGE_M,
    },
    items::{item_definition, look_forward, pickup_anchor_from_position},
    protocol::Vec3Net,
    resource_nodes::resource_node_anchor_for,
    server::{
        Deployable, DeployableActive, DeployableStability, DeployableTransform, DroppedItem,
        DroppedItemTransform, LootBagEntity, LootBagTransform, Player, ResourceNode,
        ResourceNodeStorage,
    },
};

use super::viewport_position;

/// Player-attack targeting range for the active item, as a RULE: the swing's
/// authoritative reach (the active [`crate::combat::AttackProfile`]'s `reach_m`)
/// minus a fixed [`COMBAT_PLAYER_TARGET_REACH_MARGIN_M`] margin. The server
/// validates the hit at the full `reach_m`, and the client only marks a player
/// as the target inside that value minus the margin, so a hit the client shows
/// as landing is one the server still accepts (the movement-prediction delta
/// lives in the gap). A standard-reach weapon or tool (3.5 m) therefore targets
/// players at 3.0 m and the spear (4.5 m reach) at 4.0 m. When the held item has
/// no attack profile (bare hands, a non-combat item), there is no swing anyway,
/// so we fall back to the standard-reach value (3.0 m) for a sane tooltip range.
/// Non-player targeting (nodes, deployables) keeps its own ranges.
pub(super) fn player_attack_target_range(local_player: &LocalPlayerState) -> f32 {
    let reach = local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(crate::combat::resolve_attack_profile)
        .map(|profile| profile.reach_m)
        .unwrap_or(COMBAT_ATTACK_RANGE_M);
    (reach - COMBAT_PLAYER_TARGET_REACH_MARGIN_M).max(0.0)
}
/// Cone cosine for loot bag interaction, same as deployables since
/// bags sit at roughly the same eye-level cone an aimed E would
/// expect to hit.
const LOOT_BAG_INTERACT_CONE_COS: f32 = 0.92;
/// Conservative bound on how far any collider box of a deployable can
/// reach from the entity origin. The largest pieces (foundation 3 m
/// footprint, wall/stairs 3 m tall) anchor at one edge, so the far
/// corner sits at most ~5.2 m from the origin; 6 m adds slack. Used as
/// a cheap squared-distance reject in [`best_deployable_target`] so
/// the 30 Hz scan doesn't build collider boxes for every structure in
/// the AoI when only pieces within interact range can ever win.
const DEPLOYABLE_COLLIDER_REACH_M: f32 = 6.0;

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

/// Everything the player-target scan needs about one remote player.
/// Assembled by the caller from the split replicated components
/// (`PlayerPose` + `PlayerHealth` + `PlayerProfile` + `PlayerSleeping`)
/// so this module stays decoupled from the wire shapes.
pub(super) struct PlayerTargetCandidate<'a> {
    pub(super) player: &'a Player,
    /// Display name, used only for the sleeping-body tooltip.
    pub(super) name: &'a str,
    pub(super) position: Vec3Net,
    pub(super) health: f32,
    pub(super) sleeping: bool,
}

/// Find the closest remote player whose body AABB is hit by the look ray within
/// `max_range` (the active item's [`player_attack_target_range`]). Score is the
/// ray-AABB entry distance so it slots into the same min-score pick as the other
/// target categories.
pub(super) fn best_player_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    local_client_id: Option<crate::protocol::ClientId>,
    max_range: f32,
    players: impl Iterator<Item = PlayerTargetCandidate<'a>>,
) -> Option<(PlayerTargetCandidate<'a>, f32)> {
    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let mut best: Option<(PlayerTargetCandidate<'a>, f32)> = None;
    for candidate in players {
        if Some(candidate.player.client_id) == local_client_id {
            continue;
        }
        // Dead targets don't count; the swing should fall through to
        // whatever is behind them (a killed sleeper's loot is its dropped
        // bag, not the corpse). Health at zero is the "dead" marker.
        if candidate.health <= 0.0 {
            continue;
        }
        // Shared with the server's hit validation so targeting and acceptance
        // can't disagree: a standing player uses the upright column box, a
        // laid-out sleeper the low, wide box.
        let Some(distance) = crate::combat::player_body_ray_entry(
            eye,
            forward,
            candidate.position,
            candidate.sleeping,
        ) else {
            continue;
        };
        if distance > max_range {
            continue;
        }
        if best.as_ref().map(|(_, s)| distance < *s).unwrap_or(true) {
            best = Some((candidate, distance));
        }
    }
    best
}

pub(super) fn set_player_pickup_target(
    pickup_target: &mut PickupTargetState,
    candidate: &PlayerTargetCandidate<'_>,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.player_id = Some(candidate.player.client_id);
    // A sleeper also carries name + health so the tooltip can identify the
    // logged-out body and anchors low (over the laid-out mesh). A live player
    // is purely a swing target with no tooltip.
    let centre_y = if candidate.sleeping {
        pickup_target.sleeping_player = Some((candidate.name.to_owned(), candidate.health));
        SLEEPING_BODY_CENTRE_Y
    } else {
        PLAYER_BODY_CENTRE_Y
    };
    let anchor = Vec3Net::new(
        candidate.position.x,
        candidate.position.y + centre_y,
        candidate.position.z,
    );
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

/// Find the placed structure whose solid collider boxes the look ray
/// actually enters first, within [`DEPLOYABLE_INTERACT_RANGE_M`]. A real
/// ray test (not a cone toward the entity centre) is required for
/// building pieces: a 3 m wall's centre sits far off the look ray at
/// point-blank range, and a cone test would skip the wall in front of
/// the player to latch onto a piece behind it. Multi-box colliders also
/// give correct openings, a ray through a doorway hits whatever is
/// genuinely behind it.
///
/// Returns the entry distance (the cross-category score) plus a tooltip
/// anchor at the centre of the *hit box*. The box centre, unlike the ray
/// hit point, doesn't shift with every camera micro-movement, so the
/// tooltip glues to one world position per box instead of stuttering at
/// the throttled-rescan cadence.
pub(super) fn best_deployable_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    deployables: impl Iterator<
        Item = (
            &'a Deployable,
            &'a DeployableTransform,
            &'a DeployableStability,
            &'a DeployableActive,
        ),
    >,
) -> Option<(&'a Deployable, &'a DeployableTransform, u8, f32, Vec3Net)> {
    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let max_reach = DEPLOYABLE_INTERACT_RANGE_M + DEPLOYABLE_COLLIDER_REACH_M;
    let max_reach_sq = max_reach * max_reach;
    let mut best: Option<(&Deployable, &DeployableTransform, u8, f32, Vec3Net)> = None;
    for (meta, transform, stability, active) in deployables {
        // Cheap distance reject before the collider build: a structure
        // whose origin is beyond interact range plus the worst-case
        // collider reach can never be hit, and `deployable_colliders`
        // heap-allocates a box Vec per call, which adds up at 30 Hz
        // over a whole base.
        if transform.position.minus(eye).length_squared() > max_reach_sq {
            continue;
        }
        // Doors are targeted through their actual panel volume: closed,
        // the plane seated in the opening; open, the panel swung clear
        // on its hinge. E (close), repair taps, and damage swings all
        // land where the mesh visibly is.
        let mut hit: Option<(f32, Vec3Net)> = None;
        for block in
            crate::app::systems::deployables::deployable_colliders(meta, transform, active.0)
        {
            let Some(distance) = ray_block_entry_distance(eye, forward, &block) else {
                continue;
            };
            if hit.is_none_or(|(best_distance, _)| distance < best_distance) {
                hit = Some((distance, block.center));
            }
        }
        let Some((distance, anchor)) = hit else {
            continue;
        };
        if distance > DEPLOYABLE_INTERACT_RANGE_M {
            continue;
        }
        if best
            .as_ref()
            .map(|(_, _, _, s, _)| distance < *s)
            .unwrap_or(true)
        {
            best = Some((meta, transform, stability.0, distance, anchor));
        }
    }
    best
}

/// Slab-method ray entry distance against an arbitrary
/// [`WorldBlock`](crate::world::WorldBlock)
/// (per-axis half-extents, unlike the player body box's square footprint).
/// Returns 0 when the eye is inside the box.
fn ray_block_entry_distance(
    origin: Vec3Net,
    direction: Vec3Net,
    block: &crate::world::WorldBlock,
) -> Option<f32> {
    let min = block.min();
    let max = block.max();
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
    Some(t_near.max(0.0))
}

/// `anchor` is the centre of the collider box the look ray hit, not the
/// entity origin: for a 3 m wall the origin sits at the base of one edge
/// (off to the side and at foot height), while the hit box centre is on
/// the piece the player is actually looking at, and it stays put between
/// rescans so the tooltip tracks smoothly.
pub(super) fn set_deployable_pickup_target(
    pickup_target: &mut PickupTargetState,
    meta: &Deployable,
    stability: u8,
    anchor: Vec3Net,
    authorized: &[crate::protocol::AccountId],
    my_account: Option<crate::protocol::AccountId>,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    use crate::app::state::CupboardAuthState;
    use crate::items::DeployableKind;
    pickup_target.clear();
    pickup_target.deployable_id = Some(meta.id);
    pickup_target.deployable_kind = Some(meta.kind);
    pickup_target.deployable_stability =
        matches!(meta.kind, DeployableKind::Building { .. }).then_some(stability);
    pickup_target.deployable_cupboard_auth = matches!(meta.kind, DeployableKind::ToolCupboard)
        .then(|| {
            if my_account.is_some_and(|account| authorized.contains(&account)) {
                CupboardAuthState::Authorized
            } else {
                CupboardAuthState::Unauthorized
            }
        });
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

/// Write a stuck (at-rest) arrow as the pickup target. `stack` carries the
/// ammo item so the shared tooltip shows "Arrow / Press E to pick up" exactly
/// like a dropped item; the recovery command itself only ships the projectile
/// id (the server resolves the true ammo item authoritatively). Both ranged
/// weapons fire the one arrow item today, so the client-side label is the
/// plain [`crate::items::ARROW_ID`].
pub(super) fn set_projectile_pickup_target(
    pickup_target: &mut PickupTargetState,
    projectile: &crate::server::Projectile,
    transform: &crate::server::ProjectileTransform,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.projectile_id = Some(projectile.id);
    pickup_target.stack = Some(crate::protocol::ItemStack::new(crate::items::ARROW_ID, 1));
    let anchor = pickup_anchor_from_position(transform.position);
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
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
