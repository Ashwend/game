use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    app::{
        EYE_HEIGHT,
        scene::{HeldItemVisual, ItemVisualAssets, MainCamera, NetworkDroppedItem},
        state::{ClientRuntime, LookState, MenuState, PickupTargetState, Screen},
    },
    items::{best_pickup_target, item_definition, pickup_anchor},
    protocol::DroppedWorldItem,
};

const HELD_ITEM_FORWARD_OFFSET: f32 = 0.62;
const HELD_ITEM_RIGHT_OFFSET: f32 = 0.28;
const HELD_ITEM_DOWN_OFFSET: f32 = 0.24;

pub(crate) fn apply_dropped_items_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Res<ItemVisualAssets>,
    dropped_entities: Query<(Entity, &NetworkDroppedItem)>,
) {
    let Some(snapshot) = &runtime.snapshot else {
        for (entity, _) in &dropped_entities {
            commands.entity(entity).despawn();
        }
        return;
    };

    let existing = dropped_entities
        .iter()
        .map(|(entity, dropped)| (dropped.id, entity))
        .collect::<HashMap<_, _>>();

    for item in &snapshot.dropped_items {
        let transform = dropped_item_transform(item);
        if let Some(entity) = existing.get(&item.id) {
            commands.entity(*entity).insert((transform,));
        } else {
            commands.spawn((
                Name::new(format!("Dropped Item {}", item.id)),
                NetworkDroppedItem { id: item.id },
                Mesh3d(assets.dropped_mesh.clone()),
                MeshMaterial3d(assets.dropped_material.clone()),
                transform,
                Visibility::Visible,
            ));
        }
    }

    for (entity, dropped) in &dropped_entities {
        if !snapshot
            .dropped_items
            .iter()
            .any(|item| item.id == dropped.id)
        {
            commands.entity(entity).despawn();
        }
    }
}

pub(crate) fn update_pickup_target_system(
    runtime: Res<ClientRuntime>,
    look: Res<LookState>,
    menu: Res<MenuState>,
    camera: Query<(&Camera, &Transform), With<MainCamera>>,
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
    pickup_target.world_position = Some(pickup_anchor(item));
    pickup_target.screen_position = camera.single().ok().and_then(|(camera, camera_transform)| {
        let anchor = pickup_anchor(item);
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
    Transform::from_xyz(item.position.x, item.position.y + 0.22, item.position.z).with_rotation(
        Quat::from_rotation_y(item.yaw) * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
    )
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
