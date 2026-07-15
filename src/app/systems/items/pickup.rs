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
    best_deployable_target, best_loot_bag_target, best_player_target, player_attack_target_range,
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
    resource_nodes::resource_node_score_at,
    server::{
        Deployable, DeployableActive, DeployableAuth, DeployableStability, DeployableTransform,
        DroppedItem, DroppedItemTransform, LootBagEntity, LootBagTransform, Player, PlayerHealth,
        PlayerPose, PlayerProfile, PlayerSleeping, Projectile, ProjectileTransform, ResourceNode,
        ResourceNodeStorage,
    },
};

use targets::PlayerTargetCandidate;

#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
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
        &DeployableAuth,
    )>,
    remote_players: Query<(
        &Player,
        &PlayerProfile,
        &PlayerPose,
        &PlayerHealth,
        Option<&PlayerSleeping>,
    )>,
    loot_bags: Query<(&LootBagEntity, &LootBagTransform)>,
    projectiles: Query<(&Projectile, &ProjectileTransform)>,
    user: Option<Res<crate::app::state::CurrentUser>>,
    local_player: Res<crate::app::state::LocalPlayerState>,
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
    let deployable_target = best_deployable_target(
        eye,
        look.yaw,
        look.pitch,
        deployables.iter().map(|(a, b, c, d, _)| (a, b, c, d)),
    );
    let local_client_id = runtime.client_id;
    // Player-attack targeting range is the active item's reach minus the fixed
    // margin (RULE): the spear reaches players at 4.0 m, tools and the other
    // weapons at 3.0 m. Node/deployable targeting keeps its own ranges.
    let player_attack_range = player_attack_target_range(&local_player);
    let player_target = best_player_target(
        eye,
        look.yaw,
        look.pitch,
        local_client_id,
        player_attack_range,
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
    // Stuck (at-rest) arrows are E-recoverable before their despawn TTL. Rest
    // is signalled by a near-zero replicated speed (the server snaps a stuck
    // arrow to a tiny direction-only epsilon velocity); an in-flight arrow
    // (tens of m/s) is never a target. Same look-ray scoring as a dropped item.
    const PROJECTILE_REST_SPEED_MPS: f32 = 0.5;
    let projectile_target = projectiles
        .iter()
        .filter(|(_, transform)| {
            transform.velocity.length_squared()
                < PROJECTILE_REST_SPEED_MPS * PROJECTILE_REST_SPEED_MPS
        })
        .filter_map(|(projectile, transform)| {
            pickup_score_at_position(eye, look.yaw, look.pitch, transform.position)
                .map(|score| (projectile, transform, score))
        })
        .min_by(|(_, _, a), (_, _, b)| a.total_cmp(b));

    // Pick whichever option is closest along the look ray. Dropped
    // items + resource nodes both return projection-along-ray scores;
    // we treat the deployable's centre-distance the same way.
    let item_score = dropped_target.as_ref().map(|(_, _, score)| *score);
    let node_score = resource_target.as_ref().map(|(_, _, score)| *score);
    let deployable_score = deployable_target.as_ref().map(|(_, _, _, score, _)| *score);
    let player_score = player_target.as_ref().map(|(_, score)| *score);
    let loot_bag_score = loot_bag_target.as_ref().map(|(_, _, score)| *score);
    let projectile_score = projectile_target.as_ref().map(|(_, _, score)| *score);
    let best = [
        item_score,
        node_score,
        deployable_score,
        player_score,
        loot_bag_score,
        projectile_score,
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
    } else if projectile_score == Some(best) {
        if let Some((projectile, transform, _)) = projectile_target {
            targets::set_projectile_pickup_target(
                &mut pickup_target,
                projectile,
                transform,
                &camera,
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
    } else if let Some((meta, transform, stability, _, anchor)) = deployable_target {
        let my_account = user.as_ref().map(|user| user.0.account_id);
        let authorized = deployables
            .iter()
            .find(|(deployable, ..)| deployable.id == meta.id)
            .map(|(_, _, _, _, auth)| auth.0.as_slice())
            .unwrap_or(&[]);
        set_deployable_pickup_target(
            &mut pickup_target,
            meta,
            stability,
            anchor,
            authorized,
            my_account,
            &camera,
        );
        // Claim-aware modify rights + demolish-window prediction, so the
        // hammer wheel only ever offers actions the server will accept.
        pickup_target.deployable_can_modify = client_building_modify_allowed(
            transform.position,
            meta.owner,
            my_account,
            &deployables,
        );
        pickup_target.deployable_demolishable =
            runtime.server_tick().saturating_sub(meta.placed_at_tick)
                <= crate::game_balance::BUILDING_DEMOLISH_WINDOW_TICKS;
    }
}

/// Client mirror of the server's `building_modify_allowed`: whether
/// `account` may upgrade/demolish the building piece at `position`. Inside
/// a Tool Cupboard claim, authorization governs; outside any claim, only
/// the original builder. Uses the same shared footprint geometry so the
/// hammer wheel matches the server's verdict.
fn client_building_modify_allowed(
    position: crate::protocol::Vec3Net,
    owner: Option<crate::protocol::AccountId>,
    account: Option<crate::protocol::AccountId>,
    deployables: &Query<(
        &Deployable,
        &DeployableTransform,
        &DeployableStability,
        &DeployableActive,
        &DeployableAuth,
    )>,
) -> bool {
    use crate::building::{
        ClaimPlatform, claim_cells_cover, claim_footprint_cells, platform_top_offset,
    };
    use crate::game_balance::BUILDING_PRIVILEGE_MARGIN_CELLS;
    use crate::items::DeployableKind;

    let Some(account) = account else {
        return false;
    };
    let platforms: Vec<ClaimPlatform> = deployables
        .iter()
        .filter_map(|(meta, transform, _, _, _)| {
            let DeployableKind::Building { piece, .. } = meta.kind else {
                return None;
            };
            let top = platform_top_offset(piece)?;
            Some(ClaimPlatform {
                position: transform.position,
                top: transform.position.y + top,
            })
        })
        .collect();
    let mut covered = false;
    for (meta, transform, _, _, auth) in deployables {
        if !matches!(meta.kind, DeployableKind::ToolCupboard) {
            continue;
        }
        let cells = claim_footprint_cells(
            &platforms,
            transform.position,
            BUILDING_PRIVILEGE_MARGIN_CELLS,
        );
        if !claim_cells_cover(&cells, position) {
            continue;
        }
        covered = true;
        if auth.0.contains(&account) {
            return true;
        }
    }
    if covered {
        false
    } else {
        owner == Some(account)
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
