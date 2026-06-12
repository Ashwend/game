//! Pickup / interact targeting.
//!
//! `update_pickup_target_system` runs the throttled sweep over every
//! networked entity category (dropped items, resource nodes, deployables,
//! remote players, loot bags), picks the closest along the look ray, and
//! writes the winner into [`PickupTargetState`]. The per-category scoring
//! and `set_*` writers live in [`targets`]; this module owns the
//! dispatcher plus the cheap per-frame screen-reprojection helpers.

mod targets;

use targets::{
    best_deployable_target, best_loot_bag_target, best_player_target,
    refresh_dropped_target_anchor, set_deployable_pickup_target, set_dropped_pickup_target,
    set_loot_bag_pickup_target, set_player_pickup_target, set_resource_pickup_target,
};

use bevy::prelude::*;

use crate::{
    app::{
        EYE_HEIGHT,
        scene::{MainCamera, NetworkDroppedItem},
        state::{ClientRuntime, LookState, MenuState, PickupTargetState, Screen},
    },
    items::pickup_score_at_position,
    resources::resource_node_score_at,
    server::{
        Deployable, DeployableActive, DeployableStability, DeployableTransform, DroppedItem,
        DroppedItemTransform, LootBagEntity, LootBagTransform, Player, PlayerHealth, PlayerPose,
        PlayerProfile, PlayerSleeping, ResourceNode, ResourceNodeStorage,
    },
};

use targets::PlayerTargetCandidate;

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_pickup_target_system(
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    look: Res<LookState>,
    menu: Res<MenuState>,
    camera: Query<(&Camera, &Transform), With<MainCamera>>,
    dropped_entities: Query<(&NetworkDroppedItem, &Transform)>,
    dropped_replicated: Query<(&DroppedItem, &DroppedItemTransform)>,
    resource_nodes: Query<(&ResourceNode, &ResourceNodeStorage)>,
    deployables: Query<(
        &Deployable,
        &DeployableTransform,
        &DeployableStability,
        &DeployableActive,
    )>,
    remote_players: Query<(
        &Player,
        &PlayerProfile,
        &PlayerPose,
        &PlayerHealth,
        Option<&PlayerSleeping>,
    )>,
    loot_bags: Query<(&LootBagEntity, &LootBagTransform)>,
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
    // fixed cadence, tooltip targeting doesn't need to update every render
    // frame and the early-exit work in the score helpers still scales
    // with the replicated set size.
    pickup_target.elapsed_since_scan += time.delta_secs().max(0.0);
    if pickup_target.elapsed_since_scan < crate::app::state::PICKUP_TARGET_SCAN_INTERVAL_SECS {
        return;
    }
    pickup_target.elapsed_since_scan = 0.0;

    let _ = runtime;
    let Some(player) = runtime.local_view() else {
        pickup_target.clear();
        return;
    };

    let eye = player
        .position
        .plus(crate::protocol::Vec3Net::new(0.0, EYE_HEIGHT, 0.0));
    let dropped_target = dropped_replicated
        .iter()
        .filter_map(|(drop, transform)| {
            pickup_score_at_position(eye, look.yaw, look.pitch, transform.position)
                .map(|score| (drop, transform, score))
        })
        .min_by(|(_, _, a), (_, _, b)| a.total_cmp(b));
    let resource_target = resource_nodes
        .iter()
        .filter_map(|(node, storage)| {
            resource_node_score_at(
                eye,
                look.yaw,
                look.pitch,
                &node.definition_id,
                node.position,
            )
            .map(|score| (node, storage, score))
        })
        .min_by(|(_, _, a), (_, _, b)| a.total_cmp(b));
    let deployable_target = best_deployable_target(eye, look.yaw, look.pitch, deployables.iter());
    let local_client_id = runtime.client_id;
    let player_target = best_player_target(
        eye,
        look.yaw,
        look.pitch,
        local_client_id,
        remote_players
            .iter()
            .map(
                |(player, profile, pose, health, sleeping)| PlayerTargetCandidate {
                    player,
                    name: &profile.name,
                    position: pose.position,
                    health: health.0,
                    sleeping: matches!(sleeping, Some(PlayerSleeping(true))),
                },
            ),
    );
    let loot_bag_target = best_loot_bag_target(eye, look.yaw, look.pitch, loot_bags.iter());

    // Pick whichever option is closest along the look ray. Dropped
    // items + resource nodes both return projection-along-ray scores;
    // we treat the deployable's centre-distance the same way.
    let item_score = dropped_target.as_ref().map(|(_, _, score)| *score);
    let node_score = resource_target.as_ref().map(|(_, _, score)| *score);
    let deployable_score = deployable_target.as_ref().map(|(_, _, _, score, _)| *score);
    let player_score = player_target.as_ref().map(|(_, score)| *score);
    let loot_bag_score = loot_bag_target.as_ref().map(|(_, _, score)| *score);
    let best = [
        item_score,
        node_score,
        deployable_score,
        player_score,
        loot_bag_score,
    ]
    .into_iter()
    .flatten()
    .fold(f32::INFINITY, f32::min);

    pickup_target.clear();
    if best == f32::INFINITY {
        return;
    }

    if item_score == Some(best) {
        if let Some((drop, transform, _)) = dropped_target {
            set_dropped_pickup_target(
                &mut pickup_target,
                drop,
                transform,
                &camera,
                &dropped_entities,
            );
        }
    } else if node_score == Some(best) {
        if let Some((node, storage, _)) = resource_target {
            set_resource_pickup_target(&mut pickup_target, node, storage, &camera);
        }
    } else if player_score == Some(best) {
        if let Some((candidate, _)) = player_target {
            set_player_pickup_target(&mut pickup_target, &candidate, &camera);
        }
    } else if loot_bag_score == Some(best) {
        if let Some((meta, transform, _)) = loot_bag_target {
            set_loot_bag_pickup_target(&mut pickup_target, meta, transform, &camera);
        }
    } else if let Some((meta, _, stability, _, anchor)) = deployable_target {
        set_deployable_pickup_target(&mut pickup_target, meta, stability, anchor, &camera);
    }
}

fn reproject_screen_position(
    pickup_target: &mut PickupTargetState,
    camera: &Query<(&Camera, &Transform), With<MainCamera>>,
) {
    if let Some(anchor) = pickup_target.world_position {
        pickup_target.screen_position = viewport_position(camera, anchor);
    }
}

pub(super) fn viewport_position(
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

#[cfg(test)]
mod tests;
