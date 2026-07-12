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

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_loot_bags_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    time: Res<Time>,
    assets: Res<ItemVisualAssets>,
    mut entities: ResMut<LootBagEntities>,
    mut stream: ResMut<crate::app::state::WorldStreamState>,
    visuals: Query<&Transform, With<NetworkLootBag>>,
    replicated: Query<(&LootBagEntity, &LootBagTransform)>,
) {
    if runtime.client_id.is_none() {
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        stream.reset();
        return;
    }

    let entities = &mut *entities;
    stream.note_connected(time.elapsed_secs());
    // Replicated arrivals this frame (bags we have no visual for yet),
    // reported to the world-entry stream tracker so the loading gate can
    // wait for the server to finish the initial send.
    let mut arrivals = 0usize;
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
            // Bags are static, so the recomputed transform matches the
            // spawned one every frame; only write on a real change to
            // avoid per-frame change-detection churn on at-rest bags.
            if visuals
                .get(entity)
                .is_ok_and(|current| *current != world_transform)
            {
                commands.entity(entity).insert(world_transform);
            }
        } else {
            arrivals += 1;
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
    stream.note_arrivals(time.elapsed_secs(), arrivals);

    entities.0.retain(|id, entity| {
        if visible.contains(id) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
}
