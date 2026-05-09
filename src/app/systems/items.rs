use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    app::{
        EYE_HEIGHT,
        scene::{HeldItemVisual, ItemVisualAssets, MainCamera, NetworkDroppedItem},
        state::{ClientRuntime, LookState, MenuState, PickupTargetState, Screen},
    },
    items::{best_pickup_target, item_definition, pickup_anchor, pickup_anchor_from_position},
    protocol::{DroppedWorldItem, QuatNet},
};

const HELD_ITEM_FORWARD_OFFSET: f32 = 0.62;
const HELD_ITEM_RIGHT_OFFSET: f32 = 0.28;
const HELD_ITEM_DOWN_OFFSET: f32 = 0.24;
const DROPPED_ITEM_INTERPOLATION_SECONDS: f32 = 0.1;
const DROPPED_ITEM_INTERPOLATION_SNAP_DISTANCE: f32 = 6.0;

pub(crate) fn apply_dropped_items_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    assets: Res<ItemVisualAssets>,
    mut dropped_entities: Query<(
        Entity,
        &NetworkDroppedItem,
        &Transform,
        &mut DroppedItemInterpolation,
    )>,
) {
    let Some(snapshot) = &runtime.snapshot else {
        for (entity, _, _, _) in &dropped_entities {
            commands.entity(entity).despawn();
        }
        return;
    };

    let existing = dropped_entities
        .iter()
        .map(|(entity, dropped, _, _)| (dropped.id, entity))
        .collect::<HashMap<_, _>>();

    for item in &snapshot.dropped_items {
        let target = dropped_item_transform(item);
        if let Some(entity) = existing.get(&item.id) {
            if let Ok((_, _, current, mut interpolation)) = dropped_entities.get_mut(*entity) {
                interpolation.retarget(snapshot.tick, current, target);
                let transform = interpolation.advance(time.delta_secs());
                commands.entity(*entity).insert((transform,));
            }
        } else {
            commands.spawn((
                Name::new(format!("Dropped Item {}", item.id)),
                NetworkDroppedItem { id: item.id },
                DroppedItemInterpolation::new(snapshot.tick, target),
                Mesh3d(assets.dropped_mesh.clone()),
                MeshMaterial3d(assets.dropped_material.clone()),
                target,
                Visibility::Visible,
            ));
        }
    }

    for (entity, dropped, _, _) in &dropped_entities {
        if !snapshot
            .dropped_items
            .iter()
            .any(|item| item.id == dropped.id)
        {
            commands.entity(entity).despawn();
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct DroppedItemInterpolation {
    snapshot_tick: u64,
    from: Transform,
    to: Transform,
    elapsed: f32,
}

impl DroppedItemInterpolation {
    fn new(snapshot_tick: u64, transform: Transform) -> Self {
        Self {
            snapshot_tick,
            from: transform,
            to: transform,
            elapsed: DROPPED_ITEM_INTERPOLATION_SECONDS,
        }
    }

    fn retarget(&mut self, snapshot_tick: u64, current: &Transform, target: Transform) {
        if snapshot_tick <= self.snapshot_tick {
            return;
        }

        let distance = current.translation.distance(target.translation);
        self.from = if distance > DROPPED_ITEM_INTERPOLATION_SNAP_DISTANCE {
            target
        } else {
            *current
        };
        self.to = target;
        self.elapsed = 0.0;
        self.snapshot_tick = snapshot_tick;
    }

    fn advance(&mut self, delta_seconds: f32) -> Transform {
        self.elapsed += delta_seconds.max(0.0);
        let alpha = (self.elapsed / DROPPED_ITEM_INTERPOLATION_SECONDS).clamp(0.0, 1.0);
        Transform::from_translation(self.from.translation.lerp(self.to.translation, alpha))
            .with_rotation(self.from.rotation.slerp(self.to.rotation, alpha))
    }
}

pub(crate) fn update_pickup_target_system(
    runtime: Res<ClientRuntime>,
    look: Res<LookState>,
    menu: Res<MenuState>,
    camera: Query<(&Camera, &Transform), With<MainCamera>>,
    dropped_entities: Query<(&NetworkDroppedItem, &Transform)>,
    mut pickup_target: ResMut<PickupTargetState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.inventory_open || menu.chat_open {
        pickup_target.clear();
        return;
    }

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
    let Some(item) = best_pickup_target(eye, look.yaw, look.pitch, snapshot.dropped_items.iter())
    else {
        pickup_target.clear();
        return;
    };

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
    pickup_target.screen_position = camera.single().ok().and_then(|(camera, camera_transform)| {
        camera
            .world_to_viewport(
                &GlobalTransform::from(*camera_transform),
                Vec3::new(anchor.x, anchor.y, anchor.z),
            )
            .ok()
    });
}

pub(crate) fn apply_held_item_visual_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    assets: Res<ItemVisualAssets>,
    camera: Query<&Transform, With<MainCamera>>,
    held: Query<Entity, With<HeldItemVisual>>,
) {
    let should_show = menu.screen == Screen::InGame
        && !menu.pause_open
        && runtime
            .local_player()
            .and_then(|player| {
                player
                    .inventory
                    .active_actionbar_stack()
                    .and_then(|stack| item_definition(&stack.item_id))
            })
            .is_some_and(|definition| definition.equipable);

    if !should_show {
        for entity in &held {
            commands.entity(entity).despawn();
        }
        return;
    }

    let Ok(camera_transform) = camera.single() else {
        return;
    };
    let transform = held_item_transform(camera_transform);
    if let Some(entity) = held.iter().next() {
        commands
            .entity(entity)
            .insert((transform, Visibility::Visible));
    } else {
        commands.spawn((
            Name::new("Held Item"),
            HeldItemVisual,
            Mesh3d(assets.held_mesh.clone()),
            MeshMaterial3d(assets.held_material.clone()),
            transform,
            Visibility::Visible,
        ));
    }
}

fn dropped_item_transform(item: &DroppedWorldItem) -> Transform {
    Transform::from_xyz(item.position.x, item.position.y, item.position.z)
        .with_rotation(dropped_item_rotation(item.rotation, item.yaw))
}

fn dropped_item_rotation(rotation: QuatNet, fallback_yaw: f32) -> Quat {
    let len_sq = rotation.x.mul_add(
        rotation.x,
        rotation.y.mul_add(
            rotation.y,
            rotation.z.mul_add(rotation.z, rotation.w * rotation.w),
        ),
    );
    if len_sq.is_finite() && len_sq > f32::EPSILON {
        Quat::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w).normalize()
    } else {
        Quat::from_rotation_y(fallback_yaw)
    }
}

fn held_item_transform(camera_transform: &Transform) -> Transform {
    let forward = camera_transform.rotation.mul_vec3(Vec3::NEG_Z);
    let right = camera_transform.rotation.mul_vec3(Vec3::X);
    let up = camera_transform.rotation.mul_vec3(Vec3::Y);
    let translation = camera_transform.translation
        + forward * HELD_ITEM_FORWARD_OFFSET
        + right * HELD_ITEM_RIGHT_OFFSET
        - up * HELD_ITEM_DOWN_OFFSET;
    Transform::from_translation(translation).with_rotation(
        camera_transform.rotation * Quat::from_euler(EulerRot::XYZ, -0.35, 0.25, 0.18),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_item_interpolation_blends_between_snapshot_targets() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(4.0, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let mut interpolation = DroppedItemInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let halfway = interpolation.advance(DROPPED_ITEM_INTERPOLATION_SECONDS * 0.5);

        assert!((halfway.translation.x - 2.0).abs() < 0.001);
        assert!(halfway.rotation.angle_between(current.rotation) > 0.1);
        assert!(halfway.rotation.angle_between(target.rotation) > 0.1);
    }

    #[test]
    fn dropped_item_interpolation_snaps_extreme_corrections() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(DROPPED_ITEM_INTERPOLATION_SNAP_DISTANCE + 1.0, 0.0, 0.0);
        let mut interpolation = DroppedItemInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let corrected = interpolation.advance(0.0);

        assert_eq!(corrected.translation, target.translation);
    }
}
