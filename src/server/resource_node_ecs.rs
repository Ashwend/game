//! ECS storage for authoritative resource nodes.
//!
//! Each live resource node is a Bevy entity carrying a [`ResourceNode`]
//! and [`ResourceNodeStorage`] component pair. [`ResourceNodeIndex`] is a
//! sibling resource that maps the wire-stable [`ResourceNodeId`] to the
//! owning entity so gather/admin paths can keep doing O(1) id lookups
//! without a query.
//!
//! Splitting position/yaw/definition (rarely changes) from storage
//! (mutated on every successful gather) keeps per-component change
//! detection cheap: only the changed component ships through Lightyear
//! per tick instead of the whole node.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    protocol::{ItemStack, ResourceNodeId, ResourceNodeState, Vec3Net},
    world::ChunkCoord,
};

/// Identity + immutable-after-spawn fields. `position`/`yaw` change only
/// on regrow (which deletes the old entity and spawns a new one), so this
/// component is effectively read-only post-spawn.
///
/// `Serialize`/`Deserialize` are required by Lightyear's component
/// replication, the component travels the wire as-is.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceNode {
    pub id: ResourceNodeId,
    pub definition_id: String,
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Per-node mutable inventory. The active storage list, gather decrements
/// entries, depletion is observed when this list is empty.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceNodeStorage(pub Vec<ItemStack>);

/// Anchor chunk for a node entity. Mirrors `ChunkManager::node_chunks` and
/// is kept in sync at spawn/despawn. Not strictly required for current
/// gameplay (chunk_manager owns the membership index) but lets future
/// chunk-room replication subscribe by component query without a side
/// lookup.
#[derive(Component, Debug, Clone, Copy)]
pub struct ResourceNodeChunk(pub ChunkCoord);

/// `ResourceNodeId → Entity` so the gather/admin paths can resolve a node
/// in O(1) without iterating a query.
#[derive(Resource, Default, Debug)]
pub struct ResourceNodeIndex {
    by_id: HashMap<ResourceNodeId, Entity>,
}

impl ResourceNodeIndex {
    pub fn get(&self, id: ResourceNodeId) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }

    pub fn insert(&mut self, id: ResourceNodeId, entity: Entity) {
        self.by_id.insert(id, entity);
    }

    pub fn remove(&mut self, id: ResourceNodeId) -> Option<Entity> {
        self.by_id.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ResourceNodeId, Entity)> + '_ {
        self.by_id.iter().map(|(id, entity)| (*id, *entity))
    }

    pub fn clear(&mut self) {
        self.by_id.clear();
    }
}

/// Spawn a fresh entity for `state`, register it in the index, and return
/// the entity. The caller is responsible for any chunk-manager tracking.
pub fn spawn_resource_node_entity(
    world: &mut World,
    state: ResourceNodeState,
    chunk: ChunkCoord,
) -> Entity {
    let id = state.id;
    let node = ResourceNode {
        id: state.id,
        definition_id: state.definition_id,
        position: state.position,
        yaw: state.yaw,
    };
    let entity = world
        .spawn((
            node,
            ResourceNodeStorage(state.storage),
            ResourceNodeChunk(chunk),
        ))
        .id();
    world.resource_mut::<ResourceNodeIndex>().insert(id, entity);
    entity
}

/// Despawn the entity for `id` if present, removing it from the index.
/// Returns the despawned entity (useful for tests / assertions).
pub fn despawn_resource_node_entity(world: &mut World, id: ResourceNodeId) -> Option<Entity> {
    let entity = world.resource_mut::<ResourceNodeIndex>().remove(id)?;
    if let Ok(entity_world) = world.get_entity_mut(entity) {
        entity_world.despawn();
    }
    Some(entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::COAL_NODE_ID;

    fn coal_state(id: ResourceNodeId, quantity: u16) -> ResourceNodeState {
        ResourceNodeState {
            id,
            definition_id: COAL_NODE_ID.to_owned(),
            position: Vec3Net::new(1.0, 0.0, 2.0),
            yaw: 0.5,
            storage: vec![ItemStack::new(crate::items::COAL_ID, quantity)],
        }
    }

    fn fresh_world() -> World {
        let mut world = World::new();
        world.init_resource::<ResourceNodeIndex>();
        world
    }

    #[test]
    fn spawn_then_despawn_round_trips_index_and_components() {
        let mut world = fresh_world();
        let entity =
            spawn_resource_node_entity(&mut world, coal_state(7, 3), ChunkCoord::new(0, 0));

        assert_eq!(world.resource::<ResourceNodeIndex>().get(7), Some(entity));
        let node = world.get::<ResourceNode>(entity).expect("node component");
        let storage = world
            .get::<ResourceNodeStorage>(entity)
            .expect("storage component");
        assert_eq!(node.id, 7);
        assert_eq!(storage.0[0].quantity, 3);

        let despawned = despawn_resource_node_entity(&mut world, 7);
        assert_eq!(despawned, Some(entity));
        assert!(world.resource::<ResourceNodeIndex>().get(7).is_none());
    }
}
