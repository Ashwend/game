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

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    items::{DeployableKind, ItemId},
    protocol::{AccountId, DeployedEntityId, Vec3Net},
    world::ChunkCoord,
};

/// Identity + immutable-after-spawn fields. None of these change after
/// placement (a building-block tier upgrade despawns and respawns the
/// mirror entity, see `sync_deployable_entities`), so this component
/// stays quiet on the change-detection front. `owner` is replicated so
/// the client can gate owner-only affordances (hammer upgrade/demolish
/// wheel, bag rename/pickup) before the server's authoritative check.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deployable {
    pub id: DeployedEntityId,
    #[serde(deserialize_with = "deserialize_interned_item_id")]
    pub item_id: ItemId,
    pub kind: DeployableKind,
    pub max_health: u32,
    pub owner: Option<AccountId>,
}

fn deserialize_interned_item_id<'de, D>(deserializer: D) -> Result<ItemId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(crate::items::intern_item_id(&raw))
}

/// Placement pose. Static after placement (deployables don't move), but
/// kept on its own component so Lightyear's per-component replication
/// can ship it once on spawn without pulling the rest of the entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeployableTransform {
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Mutable HP, damage taken by the structure. Replicated to all players
/// in the same chunk room so they can see the destruction animation
/// trigger when it hits zero.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeployableHealth(pub u32);

/// Public "is it doing work?" flag, drives furnace glow/smoke on the
/// client, and doubles as the open/closed flag for doors (open = true,
/// drives the swing animation + drops the door's collider). Always
/// `false` for kinds that have no active state (workbench).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeployableActive(pub bool);

/// Player-given display name, `None` for everything except renamed
/// sleeping bags. Mutable post-spawn (the rename wheel), so it gets its
/// own component and its own replication diff.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployableLabel(pub Option<String>);

/// Structural stability percentage (0-100). Changes whenever the support
/// graph around the piece changes (a neighbour placed or destroyed), so
/// it's its own component and its own replication diff. The client uses
/// it to predict ghost validity and to show the tooltip readout. Always
/// 100 for free-standing deployables.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeployableStability(pub u8);

/// Anchor chunk for room subscription.
#[derive(Component, Debug, Clone, Copy)]
pub struct DeployableChunk(pub ChunkCoord);

crate::server::entity_index::entity_index! {
    /// `DeployedEntityId → Entity` for O(1) gameplay-side lookup.
    DeployableIndex, DeployedEntityId;
    despawn_deployable_entity
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
    pub owner: Option<AccountId>,
    pub label: Option<String>,
    pub stability: u8,
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
                owner: view.owner,
            },
            DeployableTransform {
                position: view.position,
                yaw: view.yaw,
            },
            DeployableHealth(view.health),
            DeployableActive(view.active),
            DeployableLabel(view.label),
            DeployableStability(view.stability),
            DeployableChunk(chunk),
        ))
        .id();
    world.resource_mut::<DeployableIndex>().insert(id, entity);
    entity
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
            owner: None,
            label: None,
            stability: 100,
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
