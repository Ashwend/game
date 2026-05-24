use bevy::prelude::*;

use crate::{
    app::{
        EYE_HEIGHT,
        scene::{MainCamera, NetworkDroppedItem},
        state::{ClientRuntime, LookState, MenuState, PickupTargetState, Screen},
    },
    items::{pickup_anchor, pickup_anchor_from_position, pickup_score},
    protocol::{DroppedWorldItem, ResourceNodeState},
    resources::{best_resource_node_target, resource_node_anchor},
};

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

    match (dropped_target, resource_target) {
        (Some((item, item_score)), Some((_, node_score))) if item_score <= node_score => {
            set_dropped_pickup_target(&mut pickup_target, item, &camera, &dropped_entities);
        }
        (Some((item, _)), None) => {
            set_dropped_pickup_target(&mut pickup_target, item, &camera, &dropped_entities);
        }
        (_, Some((node, _))) => {
            set_resource_pickup_target(&mut pickup_target, node, &camera);
        }
        (None, None) => {
            pickup_target.clear();
        }
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
