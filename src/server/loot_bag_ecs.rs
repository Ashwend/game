//! ECS mirror for authoritative loot bags (death drop containers).
//!
//! Companion to [`crate::server::dropped_item_ecs`]. The authoritative
//! state lives in `GameServer::loot_bags`; the `sync_loot_bag_entities`
//! system in `net/host.rs` reconciles it into ECS entities so chunk-
//! room replication can attach `Replicate` to them and the client
//! receives per-component diffs of the bag's slot list.
//!
//! Split into:
//!   - [`LootBag`] — identity (immutable post-spawn).
//!   - [`LootBagTransform`] — placement pose (static after spawn).
//!   - [`LootBagContents`] — the mutable slot list. Changes when a
//!     player drags items in or out; per-component replication keeps
//!     wire traffic to just the contents diff.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    protocol::{ItemStack, LootBagId, Vec3Net},
    world::ChunkCoord,
};

/// Identity. Immutable after spawn.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LootBag {
    pub id: LootBagId,
}

/// Placement pose. Bags don't move after spawn, but the transform is
/// kept on its own component so per-component replication ships it
/// once on spawn without pulling the contents along.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LootBagTransform {
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Mutable slot grid. Stack list of fixed length
/// (`LOOT_BAG_SLOT_COUNT`); a slot is `None` when empty. The client
/// renders the grid directly off this component.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LootBagContents(pub Vec<Option<ItemStack>>);

/// Anchor chunk for room subscription.
#[derive(Component, Debug, Clone, Copy)]
pub struct LootBagChunk(pub ChunkCoord);

/// `LootBagId → Entity` lookup for gameplay-side O(1) reads.
#[derive(Resource, Default, Debug)]
pub struct LootBagIndex {
    by_id: HashMap<LootBagId, Entity>,
}

impl LootBagIndex {
    pub fn get(&self, id: LootBagId) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }

    pub fn insert(&mut self, id: LootBagId, entity: Entity) {
        self.by_id.insert(id, entity);
    }

    pub fn remove(&mut self, id: LootBagId) -> Option<Entity> {
        self.by_id.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (LootBagId, Entity)> + '_ {
        self.by_id.iter().map(|(id, entity)| (*id, *entity))
    }
}

/// Wire-shape view used by the mirror to spawn or refresh a bag
/// entity.
pub struct LootBagView {
    pub id: LootBagId,
    pub position: Vec3Net,
    pub yaw: f32,
    pub slots: Vec<Option<ItemStack>>,
}

pub fn spawn_loot_bag_entity(world: &mut World, view: LootBagView, chunk: ChunkCoord) -> Entity {
    let id = view.id;
    let entity = world
        .spawn((
            LootBag { id: view.id },
            LootBagTransform {
                position: view.position,
                yaw: view.yaw,
            },
            LootBagContents(view.slots),
            LootBagChunk(chunk),
        ))
        .id();
    world.resource_mut::<LootBagIndex>().insert(id, entity);
    entity
}

pub fn despawn_loot_bag_entity(world: &mut World, id: LootBagId) -> Option<Entity> {
    let entity = world.resource_mut::<LootBagIndex>().remove(id)?;
    if let Ok(entity_world) = world.get_entity_mut(entity) {
        entity_world.despawn();
    }
    Some(entity)
}
