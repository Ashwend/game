//! Client-side deployable systems.
//!
//! - `apply_deployed_entities_system`, diffs the replicated deployable
//!   entities against the world: spawns new ones, despawns missing ones,
//!   updates kind/health if needed.
//! - `maintain_world_grid_system`, rebuilds the client collision grid
//!   from the replicated resource-node and deployable sets when they
//!   change.
//!
//! Diffing follows the same pattern as `apply_resource_nodes_system` /
//! `apply_dropped_items_system` so the lifecycle reads consistently
//! across all networked entities. Placement-ghost and input handling
//! live in [`placement`].

mod placement;

pub(crate) use placement::{placement_input_system, update_placement_ghost_system};

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::{
    app::{
        scene::{DeployableVisualAssets, NetworkDeployedEntity},
        state::ClientRuntime,
        systems::furnace_fire::{FurnaceFire, sync_furnace_fire},
    },
    items::{DeployableKind, item_definition},
    protocol::{DeployedEntityId, Vec3Net},
    resources::resource_node_collider_at,
    server::{Deployable, DeployableActive, DeployableHealth, DeployableTransform, ResourceNode},
    world::WorldBlock,
};

/// Reconcile the local `NetworkDeployedEntity` visuals against the
/// Lightyear-replicated `(Deployable, DeployableTransform,
/// DeployableActive)` entities. Spawn missing ones, refresh transforms,
/// despawn any that left the AoI ring. Toggles the furnace mouth light
/// to match `active`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_deployed_entities_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Option<Res<DeployableVisualAssets>>,
    existing: Query<(Entity, &NetworkDeployedEntity, &Transform)>,
    existing_fires: Query<(Entity, &ChildOf), With<FurnaceFire>>,
    replicated: Query<(
        &Deployable,
        &DeployableTransform,
        &DeployableHealth,
        &DeployableActive,
    )>,
    mut play_sound: MessageWriter<crate::app::audio::PlaySound>,
) {
    let Some(assets) = assets else {
        return;
    };
    if runtime.client_id.is_none() {
        // Not connected, tear down any visuals from a prior session.
        for (entity, _, _) in &existing {
            commands.entity(entity).despawn();
        }
        return;
    }

    let mut existing_by_id: HashMap<DeployedEntityId, (Entity, Transform)> = existing
        .iter()
        .map(|(entity, marker, transform)| (marker.id, (entity, *transform)))
        .collect();
    let mut visible_ids: HashSet<DeployedEntityId> = HashSet::new();

    // Map parent-entity → child-fire-rig-entity for the furnace fires
    // currently in the world. We compare per-furnace-entity below and either
    // spawn (active && missing) or despawn (inactive && present) the rig.
    let mut fires_by_parent: HashMap<Entity, Entity> = HashMap::new();
    for (fire_entity, child_of) in &existing_fires {
        fires_by_parent.insert(child_of.parent(), fire_entity);
    }

    for (meta, transform, _health, active) in &replicated {
        visible_ids.insert(meta.id);
        let visual_transform = deployable_transform(transform.position.into(), transform.yaw);
        let parent_entity = if let Some((entity, current)) = existing_by_id.remove(&meta.id) {
            // Only write the Transform when it actually moved. The replicated
            // transform stops changing once a deployable is placed, so an
            // unconditional insert would trip change detection (and the
            // renderer) every frame for a static structure, the spurious
            // change-detection pattern docs/profiling.md warns about.
            if current != visual_transform {
                commands.entity(entity).insert(visual_transform);
            }
            entity
        } else {
            let (mesh, material) = deployable_visual(&assets, meta.kind);
            commands
                .spawn((
                    Name::new(format!("Deployable {}", meta.id)),
                    NetworkDeployedEntity {
                        id: meta.id,
                        kind: meta.kind,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    visual_transform,
                    Visibility::Visible,
                ))
                .id()
        };

        sync_furnace_fire(
            &mut commands,
            parent_entity,
            meta.kind,
            active.0,
            fires_by_parent.remove(&parent_entity),
            visual_transform.translation,
            &mut play_sound,
        );
    }

    for (id, (entity, _)) in existing_by_id {
        if !visible_ids.contains(&id) {
            commands.entity(entity).despawn();
        }
    }
    // Any fire rig whose parent disappeared (out of AoI / destroyed)
    // would normally despawn alongside its parent via Bevy's hierarchy
    // cleanup, but a despawn-recursive isn't guaranteed everywhere,
    // sweep here just in case.
    for (_, fire_entity) in fires_by_parent {
        commands.entity(fire_entity).despawn();
    }
}

/// Knuth golden-ratio mix constant for the fingerprint helpers, the
/// XOR-of-ids accumulator gets multiplied by this to spread sequential
/// id values across the `u64`.
const FINGERPRINT_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// Per-frame maintainer for `ClientRuntime::world_grid`. Watches the
/// world version (Welcome bumps it), the replicated resource-node set,
/// and the replicated deployable set; rebuilds the grid when any of
/// them changes.
///
/// **Event-gated**, the previous fingerprint-based "idle frames cost a
/// fingerprint compare and nothing else" was a lie at scale: with 1811
/// resource nodes, the fingerprint scan was a 1-2 ms iteration *every
/// frame*, calling `resource_node_collider_at` (string-keyed HashMap
/// lookup) for each node just to detect "nothing changed". The probe
/// below short-circuits before that scan when `Added`/`Removed` events
/// confirm the entity set is unchanged.
#[allow(clippy::too_many_arguments)]
pub(crate) fn maintain_world_grid_system(
    mut runtime: ResMut<ClientRuntime>,
    resource_nodes: Query<&ResourceNode>,
    deployables: Query<(&Deployable, &DeployableTransform)>,
    added_nodes: Query<(), Added<ResourceNode>>,
    added_deps: Query<(), Added<Deployable>>,
    mut removed_nodes: RemovedComponents<ResourceNode>,
    mut removed_deps: RemovedComponents<Deployable>,
    mut last_fingerprint: Local<Option<(u64, u64, u64)>>,
    mut last_world_version: Local<u64>,
) {
    let world_version = runtime.world_version;
    // Cheap probe: skip the O(N) fingerprint scan when the entity sets
    // and world version are unchanged. `.count()` drains all events
    // so the cursor doesn't carry stale counts across frames.
    let world_changed = world_version != *last_world_version || last_fingerprint.is_none();
    let removed_count = removed_nodes.read().count() + removed_deps.read().count();
    let added_any = !added_nodes.is_empty() || !added_deps.is_empty();
    if !world_changed && !added_any && removed_count == 0 {
        return;
    }

    // At least one change is plausible. Compute the actual fingerprint
    // and bail if it matches (e.g. Added fired for an entity whose id
    // already contributed to the prior fingerprint, shouldn't happen
    // but cheap to verify).
    let resource_node_version = resource_node_set_fingerprint(resource_nodes.iter());
    let deployable_version = deployable_set_fingerprint(deployables.iter());
    let current = (world_version, resource_node_version, deployable_version);
    *last_world_version = world_version;

    if *last_fingerprint == Some(current) {
        return;
    }

    let resource_colliders: Vec<WorldBlock> = resource_nodes
        .iter()
        .filter_map(|node| resource_node_collider_at(&node.definition_id, node.position))
        .collect();
    let deployable_colliders: Vec<WorldBlock> = deployables
        .iter()
        .filter_map(|(meta, transform)| deployable_collider(meta, transform))
        .collect();
    runtime.rebuild_world_grid(resource_colliders, deployable_colliders);
    *last_fingerprint = Some(current);
}

fn resource_node_set_fingerprint<'a>(iter: impl IntoIterator<Item = &'a ResourceNode>) -> u64 {
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for node in iter {
        // Skip ids that contribute no collider so the fingerprint stays
        // tight to the actual collision set, crude clutter (surface
        // stones, branch piles, hay grass) doesn't move the grid.
        if resource_node_collider_at(&node.definition_id, node.position).is_none() {
            continue;
        }
        hash ^= node.id;
        count += 1;
    }
    hash.wrapping_mul(FINGERPRINT_MIX).wrapping_add(count)
}

fn deployable_set_fingerprint<'a>(
    iter: impl IntoIterator<Item = (&'a Deployable, &'a DeployableTransform)>,
) -> u64 {
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for (meta, _) in iter {
        // XOR ^ 0xD9E3_F1A7_5B6C_8024 ensures the deployable id space
        // (separate counter from resource nodes server-side) doesn't
        // accidentally cancel against a resource node id with the same
        // numeric value when the two fingerprints are tupled together.
        hash ^= meta.id ^ 0xD9E3_F1A7_5B6C_8024;
        count += 1;
    }
    hash.wrapping_mul(FINGERPRINT_MIX).wrapping_add(count)
}

/// Build the AABB collider for a placed structure from its replicated
/// components. Returns `None` if the item id no longer resolves (e.g.
/// a server using a newer item table than this client knows about, in
/// which case skip the collider rather than crash, the renderer will
/// still draw the structure).
pub(crate) fn deployable_collider(
    meta: &Deployable,
    transform: &DeployableTransform,
) -> Option<WorldBlock> {
    let profile = item_definition(&meta.item_id)?.deployable?;
    let center = Vec3Net::new(
        transform.position.x,
        transform.position.y + profile.collider_half_height,
        transform.position.z,
    );
    let half = Vec3Net::new(
        profile.collider_half_width,
        profile.collider_half_height,
        profile.collider_half_width,
    );
    Some(WorldBlock::new(center, half))
}

fn deployable_visual(
    assets: &DeployableVisualAssets,
    kind: DeployableKind,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    let mesh = match kind {
        DeployableKind::Workbench { .. } => assets.workbench_mesh.clone(),
        DeployableKind::Furnace { .. } => assets.furnace_mesh.clone(),
    };
    (mesh, assets.material.clone())
}

pub(super) fn deployable_transform(position: Vec3, yaw: f32) -> Transform {
    Transform::from_translation(position).with_rotation(Quat::from_rotation_y(yaw))
}

#[cfg(test)]
mod tests;
