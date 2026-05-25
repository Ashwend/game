use bevy::prelude::*;

use crate::{
    app::{
        EYE_HEIGHT,
        scene::{MainCamera, NetworkDroppedItem},
        state::{ClientRuntime, LookState, MenuState, PickupTargetState, Screen},
    },
    items::{
        item_definition, look_forward, pickup_anchor, pickup_anchor_from_position, pickup_score,
    },
    protocol::{DeployedEntityState, DroppedWorldItem, ResourceNodeState, Vec3Net},
    resources::{best_resource_node_target, resource_node_anchor},
};

/// Max range at which `E` lands on a placed structure. Matches the
/// furnace open-range so the tooltip never lies about reachability.
const DEPLOYABLE_INTERACT_RANGE_M: f32 = 5.5;
/// Cone half-angle (cosine) the player must aim through to lock onto
/// a deployable. Tight enough that the tooltip doesn't latch when the
/// player is mostly looking past the structure.
const DEPLOYABLE_INTERACT_CONE_COS: f32 = 0.92;

pub(crate) fn update_pickup_target_system(
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    look: Res<LookState>,
    menu: Res<MenuState>,
    camera: Query<(&Camera, &Transform), With<MainCamera>>,
    dropped_entities: Query<(&NetworkDroppedItem, &Transform)>,
    mut pickup_target: ResMut<PickupTargetState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.inventory_open || menu.chat_open {
        pickup_target.clear();
        pickup_target.elapsed_since_scan = 0.0;
        return;
    }

    // Re-project the existing target's world anchor every frame so the
    // tooltip stays glued to the world position as the camera moves. The
    // O(N×M) target selection below stays throttled; only the cheap
    // viewport projection (and, for dropped items, a single entity lookup
    // to pick up the interpolated transform) runs each frame.
    refresh_dropped_target_anchor(&mut pickup_target, &dropped_entities);
    reproject_screen_position(&mut pickup_target, &camera);

    // Throttle the O(N×M) sweep over dropped items and resource nodes to a
    // fixed cadence — tooltip targeting doesn't need to update every render
    // frame and the early-exit work in `pickup_score`/`resource_node_score`
    // still scales with the snapshot size.
    pickup_target.elapsed_since_scan += time.delta_secs().max(0.0);
    if pickup_target.elapsed_since_scan < crate::app::state::PICKUP_TARGET_SCAN_INTERVAL_SECS {
        return;
    }
    pickup_target.elapsed_since_scan = 0.0;

    let Some(snapshot) = &runtime.snapshot else {
        pickup_target.clear();
        return;
    };
    let Some(player) = runtime.local_view() else {
        pickup_target.clear();
        return;
    };

    let eye = player
        .position
        .plus(crate::protocol::Vec3Net::new(0.0, EYE_HEIGHT, 0.0));
    let dropped_target = snapshot
        .dropped_items
        .iter()
        .filter_map(|item| pickup_score(eye, look.yaw, look.pitch, item).map(|score| (item, score)))
        .min_by(|(_, a), (_, b)| a.total_cmp(b));
    let resource_target =
        best_resource_node_target(eye, look.yaw, look.pitch, snapshot.resource_nodes.iter());
    let deployable_target =
        best_deployable_target(eye, look.yaw, look.pitch, snapshot.deployed_entities.iter());

    // Pick whichever option is closest along the look ray. Dropped
    // items + resource nodes both return projection-along-ray scores;
    // we treat the deployable's centre-distance the same way.
    let item_score = dropped_target.map(|(_, score)| score);
    let node_score = resource_target.map(|(_, score)| score);
    let deployable_score = deployable_target.map(|(_, score)| score);
    let best = [item_score, node_score, deployable_score]
        .into_iter()
        .flatten()
        .fold(f32::INFINITY, f32::min);

    pickup_target.clear();
    if best == f32::INFINITY {
        return;
    }

    if item_score == Some(best) {
        if let Some((item, _)) = dropped_target {
            set_dropped_pickup_target(&mut pickup_target, item, &camera, &dropped_entities);
        }
    } else if node_score == Some(best) {
        if let Some((node, _)) = resource_target {
            set_resource_pickup_target(&mut pickup_target, node, &camera);
        }
    } else if let Some((entity, _)) = deployable_target {
        set_deployable_pickup_target(&mut pickup_target, entity, &camera);
    }
}

/// Find the closest placed structure inside the player's look cone.
/// `score` is the distance from the eye to the structure centre so the
/// caller can compare it directly against the dropped-item / resource-
/// node ray scores.
fn best_deployable_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    deployables: impl Iterator<Item = &'a DeployedEntityState>,
) -> Option<(&'a DeployedEntityState, f32)> {
    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let max_sq = DEPLOYABLE_INTERACT_RANGE_M * DEPLOYABLE_INTERACT_RANGE_M;
    let mut best: Option<(&DeployedEntityState, f32)> = None;
    for entity in deployables {
        // Aim point sits at half the entity's collider height so the
        // cone test isn't biased toward the floor.
        let aim = deployable_aim_point(entity);
        let to = aim.minus(eye);
        let dist_sq = to.length_squared();
        if dist_sq > max_sq {
            continue;
        }
        let dist = dist_sq.sqrt();
        if dist <= 1e-3 {
            return Some((entity, 0.0));
        }
        let cosine = to.dot(forward) / dist;
        if cosine < DEPLOYABLE_INTERACT_CONE_COS {
            continue;
        }
        let score = dist;
        if best.map(|(_, s)| score < s).unwrap_or(true) {
            best = Some((entity, score));
        }
    }
    best
}

fn deployable_aim_point(entity: &DeployedEntityState) -> Vec3Net {
    // Approximate the structure's optical centre. We don't have the
    // profile here without a registry lookup; 0.6 m up reads well for
    // both the workbench tabletop and the furnace mouth.
    let mut aim = entity.position;
    aim.y += 0.6;
    if let Some(profile) = item_definition(&entity.item_id).and_then(|def| def.deployable) {
        aim.y = entity.position.y + profile.collider_half_height;
    }
    aim
}

fn set_deployable_pickup_target(
    pickup_target: &mut PickupTargetState,
    entity: &DeployedEntityState,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.deployable_id = Some(entity.id);
    pickup_target.deployable_kind = Some(entity.kind);
    let anchor = deployable_aim_point(entity);
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

fn reproject_screen_position(
    pickup_target: &mut PickupTargetState,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    if let Some(anchor) = pickup_target.world_position {
        pickup_target.screen_position = viewport_position(camera, anchor);
    }
}

/// Follow a dropped item's interpolated transform every frame so the tooltip
/// doesn't lag behind a falling stack. The target *selection* still runs on
/// the throttled scan, but as long as the same item stays selected we re-read
/// its current entity transform here. Resource nodes don't move, so their
/// cached anchor is left alone.
fn refresh_dropped_target_anchor(
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

fn set_dropped_pickup_target(
    pickup_target: &mut PickupTargetState,
    item: &DroppedWorldItem,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
    dropped_entities: &Query<(&NetworkDroppedItem, &Transform)>,
) {
    pickup_target.clear();
    pickup_target.dropped_item_id = Some(item.id);
    pickup_target.stack = Some(item.stack.clone());
    let anchor = dropped_entities
        .iter()
        .find(|(dropped, _)| dropped.id == item.id)
        .map(|(_, transform)| {
            pickup_anchor_from_position(crate::protocol::Vec3Net::new(
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            ))
        })
        .unwrap_or_else(|| pickup_anchor(item));
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

fn set_resource_pickup_target(
    pickup_target: &mut PickupTargetState,
    node: &ResourceNodeState,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    pickup_target.clear();
    pickup_target.resource_node_id = Some(node.id);
    pickup_target.resource_definition_id = Some(node.definition_id.clone());
    pickup_target.resource_storage = node.storage.clone();
    let anchor = resource_node_anchor(node);
    pickup_target.world_position = Some(anchor);
    pickup_target.screen_position = viewport_position(camera, anchor);
}

fn viewport_position(
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
    anchor: crate::protocol::Vec3Net,
) -> Option<Vec2> {
    camera.single().ok().and_then(|(camera, camera_transform)| {
        camera
            .world_to_viewport(
                &GlobalTransform::from(*camera_transform),
                Vec3::new(anchor.x, anchor.y, anchor.z),
            )
            .ok()
    })
}
