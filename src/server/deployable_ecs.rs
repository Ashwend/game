//! ECS mirror for authoritative placed structures (workbench, furnace, …).
//!
//! Companion to [`crate::server::resource_node_ecs`]. The authoritative
//! state lives in `GameServer::deployed_entities`; the
//! `sync_deployable_entities` system in `net/host.rs` reconciles it into
//! ECS entities so Phase 4 chunk-room replication can attach `Replicate`
//! to them.
//!
//! `active` is split out as its own component because for furnaces it
//! toggles independently of the rest of the state (smelt start/stop) and
//! splitting lets Lightyear's per-component delta replication ship just
//! the toggle without re-sending position/health.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    items::{DeployableKind, ItemId},
    protocol::{DeployedEntityId, Vec3Net},
    world::ChunkCoord,
};

/// Identity + immutable-after-spawn fields. `item_id`, `kind`, and
/// `max_health` never change after placement, so this component stays
/// quiet on the change-detection front.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deployable {
    pub id: DeployedEntityId,
    #[serde(deserialize_with = "deserialize_interned_item_id")]
    pub item_id: ItemId,
    pub kind: DeployableKind,
    pub max_health: u32,
}

fn deserialize_interned_item_id<'de, D>(deserializer: D) -> Result<ItemId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(crate::items::intern_item_id(&raw))
}

/// Placement pose. Static after placement (deployables don't move), but
/// kept on its own component so the snapshot/replication path can read
/// it without pulling the rest of the entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeployableTransform {
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Mutable HP — damage taken by the structure. Replicated to all players
/// in the same chunk room so they can see the destruction animation
/// trigger when it hits zero.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeployableHealth(pub u32);

/// Public "is it doing work?" flag — drives furnace glow/smoke on the
/// client. Always `false` for kinds that have no active state
/// (workbench).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeployableActive(pub bool);

/// Anchor chunk for room subscription.
#[derive(Component, Debug, Clone, Copy)]
pub struct DeployableChunk(pub ChunkCoord);

/// `DeployedEntityId → Entity` for O(1) gameplay-side lookup.
#[derive(Resource, Default, Debug)]
pub struct DeployableIndex {
    by_id: HashMap<DeployedEntityId, Entity>,
}

impl DeployableIndex {
    pub fn get(&self, id: DeployedEntityId) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }

    pub fn insert(&mut self, id: DeployedEntityId, entity: Entity) {
        self.by_id.insert(id, entity);
    }

    pub fn remove(&mut self, id: DeployedEntityId) -> Option<Entity> {
        self.by_id.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (DeployedEntityId, Entity)> + '_ {
        self.by_id.iter().map(|(id, entity)| (*id, *entity))
    }
}

/// Wire-shape view of a deployable, used by the mirror tests and the
/// future replication path. Mirrors [`crate::protocol::DeployedEntityState`]
/// without owning a copy of its wire serde shape.
pub struct DeployableView {
    pub id: DeployedEntityId,
    pub item_id: ItemId,
    pub kind: DeployableKind,
    pub position: Vec3Net,
    pub yaw: f32,
    pub health: u32,
    pub max_health: u32,
    pub active: bool,
}

pub fn spawn_deployable_entity(
    world: &mut World,
    view: DeployableView,
    chunk: ChunkCoord,
) -> Entity {
    let id = view.id;
    let entity = world
        .spawn((
            Deployable {
                id: view.id,
                item_id: view.item_id,
                kind: view.kind,
                max_health: view.max_health,
            },
            DeployableTransform {
                position: view.position,
                yaw: view.yaw,
            },
            DeployableHealth(view.health),
            DeployableActive(view.active),
            DeployableChunk(chunk),
        ))
        .id();
    world.resource_mut::<DeployableIndex>().insert(id, entity);
    entity
}

pub fn despawn_deployable_entity(world: &mut World, id: DeployedEntityId) -> Option<Entity> {
    let entity = world.resource_mut::<DeployableIndex>().remove(id)?;
    if let Ok(entity_world) = world.get_entity_mut(entity) {
        entity_world.despawn();
    }
    Some(entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::WORKBENCH_T1_ID;

    fn fresh_world() -> World {
        let mut world = World::new();
        world.init_resource::<DeployableIndex>();
        world
    }

    fn workbench_view(id: DeployedEntityId) -> DeployableView {
        DeployableView {
            id,
            item_id: WORKBENCH_T1_ID.into(),
            kind: DeployableKind::Workbench { tier: 1 },
            position: Vec3Net::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            health: 100,
            max_health: 100,
            active: false,
        }
    }

    #[test]
    fn spawn_and_despawn_round_trip_index() {
        let mut world = fresh_world();
        let entity = spawn_deployable_entity(&mut world, workbench_view(5), ChunkCoord::new(0, 0));
        assert_eq!(world.resource::<DeployableIndex>().get(5), Some(entity));

        // Components are populated.
        let identity = world.get::<Deployable>(entity).expect("identity");
        assert_eq!(identity.id, 5);
        let health = world.get::<DeployableHealth>(entity).expect("health");
        assert_eq!(health.0, 100);

        let despawned = despawn_deployable_entity(&mut world, 5);
        assert_eq!(despawned, Some(entity));
        assert!(world.resource::<DeployableIndex>().get(5).is_none());
    }
}
