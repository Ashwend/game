//! Client-side reconciliation of replicated `LootBag` entities to
//! visible scene entities.
//!
//! Bags are static (no physics, no interpolation), so this is the
//! simplest of the per-entity reconcilers, spawn a mesh on first
//! sight, despawn when the replicated entity leaves the AoI ring.

use std::collections::{HashMap, HashSet};

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        scene::{ItemVisualAssets, NetworkLootBag},
        state::ClientRuntime,
    },
    protocol::LootBagId,
    server::{LootBagEntity, LootBagTransform},
};

#[derive(Resource, Default)]
pub(crate) struct LootBagEntities(pub(crate) HashMap<LootBagId, Entity>);

pub(crate) fn apply_loot_bags_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Res<ItemVisualAssets>,
    mut entities: ResMut<LootBagEntities>,
    replicated: Query<(&LootBagEntity, &LootBagTransform)>,
) {
    if runtime.client_id.is_none() {
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    }

    let entities = &mut *entities;
    let mut visible: HashSet<LootBagId> = HashSet::new();
    for (bag, transform) in &replicated {
        visible.insert(bag.id);
        let world_transform = Transform::from_xyz(
            transform.position.x,
            transform.position.y + 0.18,
            transform.position.z,
        )
        .with_rotation(Quat::from_rotation_y(transform.yaw))
        .with_scale(Vec3::new(1.45, 1.45, 1.45));

        if let Some(entity) = entities.0.get(&bag.id).copied() {
            commands.entity(entity).insert(world_transform);
        } else {
            let entity = commands
                .spawn((
                    Name::new(format!("Loot Bag {}", bag.id)),
                    NetworkLootBag { id: bag.id },
                    Mesh3d(assets.dropped_mesh.clone()),
                    MeshMaterial3d(assets.dropped_material.clone()),
                    world_transform,
                    Visibility::Visible,
                    NotShadowCaster,
                ))
                .id();
            entities.0.insert(bag.id, entity);
        }
    }

    entities.0.retain(|id, entity| {
        if visible.contains(id) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
}
