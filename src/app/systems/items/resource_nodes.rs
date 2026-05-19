use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::{
    app::{
        scene::{NetworkResourceNode, ResourceVisualAssets},
        state::ClientRuntime,
    },
    protocol::{ResourceNodeId, ResourceNodeState},
    resources::{ResourceNodeModel, resource_node_definition},
};

/// Persistent `id → Entity` lookup for resource nodes. Mirrors the live
/// entity set so the snapshot-apply system can skip rebuilding the map
/// every frame.
#[derive(Resource, Default)]
pub(crate) struct ResourceNodeEntities(pub(crate) HashMap<ResourceNodeId, Entity>);

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_resource_nodes_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Res<ResourceVisualAssets>,
    impact_assets: Res<crate::app::scene::ImpactEffectAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut camera_kick: ResMut<crate::app::systems::CameraImpactKick>,
    mut entities: ResMut<ResourceNodeEntities>,
    resource_entities: Query<(
        &NetworkResourceNode,
        &Mesh3d,
        &MeshMaterial3d<StandardMaterial>,
        &Transform,
    )>,
) {
    let Some(snapshot) = &runtime.snapshot else {
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    };

    let snapshot_ids: HashSet<ResourceNodeId> =
        snapshot.resource_nodes.iter().map(|node| node.id).collect();
    let entities = &mut *entities;

    for node in &snapshot.resource_nodes {
        let Some(definition) = resource_node_definition(&node.definition_id) else {
            continue;
        };
        let transform = resource_node_transform(node, definition.model);
        if let Some(entity) = entities.0.get(&node.id).copied() {
            commands.entity(entity).insert(transform);
        } else {
            let (mesh, material) = resource_node_visual(&assets, node, definition.model);
            let entity = commands
                .spawn((
                    Name::new(format!("Resource Node {}", node.id)),
                    NetworkResourceNode {
                        id: node.id,
                        model: definition.model,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    transform,
                    Visibility::Visible,
                ))
                .id();
            entities.0.insert(node.id, entity);
        }
    }

    let player_position = runtime.local_view().map(|view| {
        Vec3::new(view.position.x, view.position.y, view.position.z)
            + Vec3::Y * crate::app::EYE_HEIGHT
    });

    // Despawn nodes that fell out of the snapshot, retaining the
    // spawn-node-death effect at their final transform.
    let mut to_remove: Vec<ResourceNodeId> = Vec::new();
    for (&id, &entity) in entities.0.iter() {
        if snapshot_ids.contains(&id) {
            continue;
        }
        if let Ok((resource, mesh, material, transform)) = resource_entities.get(entity) {
            crate::app::systems::node_death::spawn_node_death(
                &mut commands,
                &impact_assets,
                &mut materials,
                &mut camera_kick,
                resource.id,
                resource.model,
                *transform,
                mesh.0.clone(),
                material.0.clone(),
                player_position,
            );
        }
        commands.entity(entity).despawn();
        to_remove.push(id);
    }
    for id in to_remove {
        entities.0.remove(&id);
    }
}

pub(super) fn resource_node_transform(
    node: &ResourceNodeState,
    model: ResourceNodeModel,
) -> Transform {
    // Trees bake their full size into the mesh and sit on the ground at
    // unit scale, which keeps each variant a single canonical mesh that
    // can be GPU-instanced. Ore nodes keep their per-instance scale
    // jitter for shape variety. Both the tree trunks and the ore rock
    // lumps have their lowest vertices at local y=0, so no height offset
    // is needed — adding one would float the geometry above the floor.
    let (height_offset, scale) = match model {
        ResourceNodeModel::CoalOre => (0.0, Vec3::new(1.0, 1.0, 1.0)),
        ResourceNodeModel::IronOre => (0.0, Vec3::new(1.1, 1.05, 0.95)),
        ResourceNodeModel::SulfurOre => (0.0, Vec3::new(0.96, 0.92, 1.06)),
        ResourceNodeModel::PineTreeSmall
        | ResourceNodeModel::PineTreeMedium
        | ResourceNodeModel::PineTreeLarge
        | ResourceNodeModel::BirchTreeSmall
        | ResourceNodeModel::BirchTreeMedium
        | ResourceNodeModel::BirchTreeLarge => (0.0, Vec3::ONE),
    };
    Transform::from_xyz(
        node.position.x,
        node.position.y + height_offset,
        node.position.z,
    )
    .with_rotation(Quat::from_rotation_y(node.yaw))
    .with_scale(scale)
}

fn resource_node_visual(
    assets: &ResourceVisualAssets,
    _node: &ResourceNodeState,
    model: ResourceNodeModel,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    match model {
        ResourceNodeModel::CoalOre => (assets.coal_node_mesh.clone(), assets.coal_material.clone()),
        ResourceNodeModel::IronOre => (assets.iron_node_mesh.clone(), assets.iron_material.clone()),
        ResourceNodeModel::SulfurOre => (
            assets.sulfur_node_mesh.clone(),
            assets.sulfur_material.clone(),
        ),
        ResourceNodeModel::PineTreeSmall => (
            assets.pine_tree_small_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::PineTreeMedium => (
            assets.pine_tree_medium_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::PineTreeLarge => (
            assets.pine_tree_large_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeSmall => (
            assets.birch_tree_small_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeMedium => (
            assets.birch_tree_medium_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeLarge => (
            assets.birch_tree_large_mesh.clone(),
            assets.vertex_material.clone(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Vec3Net;
    use crate::resources::{COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID};

    fn node_at(id: &str, position: Vec3Net) -> ResourceNodeState {
        ResourceNodeState {
            id: 1,
            definition_id: id.to_owned(),
            position,
            yaw: 0.0,
            storage: Vec::new(),
        }
    }

    #[test]
    fn ore_transform_matches_spawn_y_so_rock_sits_on_ground() {
        // The ore meshes have their lowest vertex at local y=0, so the
        // transform must not raise them above the floor.
        for ore_id in [COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID] {
            let node = node_at(ore_id, Vec3Net::new(2.0, 0.0, -3.0));
            let definition = crate::resources::resource_node_definition(ore_id).unwrap();
            let transform = resource_node_transform(&node, definition.model);
            assert_eq!(
                transform.translation.y, node.position.y,
                "{ore_id} mesh must sit at the spawn y (no floating offset)"
            );
        }
    }
}
