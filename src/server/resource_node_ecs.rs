//! ECS storage for authoritative resource nodes.
//!
//! Each live resource node is a Bevy entity carrying a [`ResourceNode`]
//! and [`ResourceNodeStorage`] component pair. [`ResourceNodeIndex`] is a
//! sibling resource that maps the wire-stable [`ResourceNodeId`] to the
//! owning entity so gather/admin paths can keep doing O(1) id lookups
//! without a query.
//!
//! Splitting position/yaw/definition (rarely changes) from storage
//! (mutated on every successful gather) keeps the per-component change
//! detection useful when Lightyear replication is wired in a later
//! phase — only the changed component ships per tick instead of the
//! whole node.

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
/// replication (Phase 4) — the component travels the wire as-is.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceNode {
    pub id: ResourceNodeId,
    pub definition_id: String,
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Per-node mutable inventory. The active storage list — gather decrements
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

/// Materialise the wire-form snapshot state for a single node by entity.
/// Used by the snapshot path and persistence to translate the ECS storage
/// back to the protocol shape the client expects.
pub fn read_resource_node_state(world: &World, entity: Entity) -> Option<ResourceNodeState> {
    let node = world.get::<ResourceNode>(entity)?;
    let storage = world.get::<ResourceNodeStorage>(entity)?;
    Some(ResourceNodeState {
        id: node.id,
        definition_id: node.definition_id.clone(),
        position: node.position,
        yaw: node.yaw,
        storage: storage.0.clone(),
        // respawn_progress is a vestigial client-side hint that the
        // server never sets — depleted nodes are removed entirely and
        // re-emerge from chunk_manager.tick as a fresh entity at a
        // new position.
        respawn_progress: None,
    })
}

/// Update the storage of an indexed node entity in place. No-op if the
/// id isn't tracked or the stored value already matches. Returns `true`
/// if a write actually happened (used by tests / future change-detection
/// instrumentation).
pub fn refresh_resource_node_storage(
    world: &mut World,
    id: ResourceNodeId,
    storage: &[ItemStack],
) -> bool {
    let Some(entity) = world.resource::<ResourceNodeIndex>().get(id) else {
        return false;
    };
    let Some(mut component) = world.get_mut::<ResourceNodeStorage>(entity) else {
        return false;
    };
    if component.0 == storage {
        return false;
    }
    component.0 = storage.to_vec();
    true
}

/// Snapshot every live node into a HashMap keyed by id. Used by:
/// - `chunk_manager.tick` (collision check against existing positions)
/// - `world_save` (persistence — entire live set)
/// - snapshot when no AoI filter is active (tests / handshake fallback)
pub fn collect_resource_node_states(
    world: &mut World,
) -> HashMap<ResourceNodeId, ResourceNodeState> {
    let mut out = HashMap::new();
    let mut query = world.query::<(&ResourceNode, &ResourceNodeStorage)>();
    for (node, storage) in query.iter(world) {
        out.insert(
            node.id,
            ResourceNodeState {
                id: node.id,
                definition_id: node.definition_id.clone(),
                position: node.position,
                yaw: node.yaw,
                storage: storage.0.clone(),
                respawn_progress: None,
            },
        );
    }
    out
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
            respawn_progress: None,
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
        let state = read_resource_node_state(&world, entity).expect("state");
        assert_eq!(state.id, 7);
        assert_eq!(state.storage[0].quantity, 3);

        let despawned = despawn_resource_node_entity(&mut world, 7);
        assert_eq!(despawned, Some(entity));
        assert!(world.resource::<ResourceNodeIndex>().get(7).is_none());
    }

    #[test]
    fn collect_returns_all_live_nodes_keyed_by_id() {
        let mut world = fresh_world();
        spawn_resource_node_entity(&mut world, coal_state(1, 5), ChunkCoord::new(0, 0));
        spawn_resource_node_entity(&mut world, coal_state(2, 9), ChunkCoord::new(0, 0));

        let collected = collect_resource_node_states(&mut world);
        assert_eq!(collected.len(), 2);
        assert_eq!(collected.get(&1).unwrap().storage[0].quantity, 5);
        assert_eq!(collected.get(&2).unwrap().storage[0].quantity, 9);
    }

    #[test]
    fn refresh_storage_writes_only_on_real_change() {
        let mut world = fresh_world();
        spawn_resource_node_entity(&mut world, coal_state(1, 5), ChunkCoord::new(0, 0));

        // Same value → no write.
        let unchanged = refresh_resource_node_storage(
            &mut world,
            1,
            &[ItemStack::new(crate::items::COAL_ID, 5)],
        );
        assert!(!unchanged);

        // Different value → write.
        let changed = refresh_resource_node_storage(
            &mut world,
            1,
            &[ItemStack::new(crate::items::COAL_ID, 4)],
        );
        assert!(changed);
        let entity = world.resource::<ResourceNodeIndex>().get(1).unwrap();
        let storage = world.get::<ResourceNodeStorage>(entity).unwrap();
        assert_eq!(storage.0[0].quantity, 4);

        // Unknown id → no-op.
        let missing = refresh_resource_node_storage(&mut world, 999, &[]);
        assert!(!missing);
    }
}
