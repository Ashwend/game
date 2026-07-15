//! ECS mirror for authoritative in-flight projectiles (arrows).
//!
//! Companion to [`crate::server::deployable_ecs`]. The authoritative state lives
//! in `GameServer::projectiles` (a [`crate::server::dirty_tracked_map::
//! DirtyTrackedMap`]); the `sync_projectile_entities` system in `net::host::
//! mirror` reconciles it into ECS entities so chunk-room replication ships each
//! projectile to clients in its AoI ring.
//!
//! The split follows the six replication rules: one identity component
//! ([`Projectile`], immutable post-spawn) plus one mutable component
//! ([`ProjectileTransform`], the per-tick position + velocity the client
//! extrapolates from). Unlike deployables a projectile MOVES every tick, so the
//! mirror sync re-anchors its chunk room as it flies (the dropped-item pattern).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    items::ItemModel,
    protocol::{ClientId, ProjectileId, Vec3Net},
    world::ChunkCoord,
};

/// Identity + immutable-after-spawn fields for a live projectile. The model
/// (the firing weapon's Bow/Crossbow archetype) drives the client's arrow VFX
/// and the impact cue; `owner` lets a client suppress double-rendering its own
/// predicted arrow (P3b) and lets the sim skip self-hits during the spawn grace
/// window. None of these change after launch.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Projectile {
    pub id: ProjectileId,
    /// Firing weapon archetype (Bow / Crossbow), for the impact identity.
    pub model: ItemModel,
    /// The client that fired this projectile.
    pub owner: ClientId,
}

/// Mutable flight state: current position and velocity, replicated together so
/// the client can extrapolate the arrow's path between the 20 Hz diffs (a fast
/// arrow moves metres per tick, so a client that only had the position would see
/// it teleport). Changes every tick while in flight.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProjectileTransform {
    pub position: Vec3Net,
    pub velocity: Vec3Net,
}

/// Anchor chunk for room subscription. Server-only (never replicated); updated
/// as the projectile flies so it rides the correct AoI room.
#[derive(Component, Debug, Clone, Copy)]
pub struct ProjectileChunk(pub ChunkCoord);

crate::server::entity_index::entity_index! {
    /// `ProjectileId -> Entity` for O(1) mirror-sync lookup.
    ProjectileIndex, ProjectileId;
    despawn_projectile_entity
}

/// Wire-shape view of a projectile, snapshotted by the mirror sync so the server
/// borrow is released before the spawn/refresh calls need `&mut World`.
pub struct ProjectileView {
    pub id: ProjectileId,
    pub model: ItemModel,
    pub owner: ClientId,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
}

pub fn spawn_projectile_entity(
    world: &mut World,
    view: ProjectileView,
    chunk: ChunkCoord,
) -> Entity {
    let id = view.id;
    let entity = world
        .spawn((
            Projectile {
                id: view.id,
                model: view.model,
                owner: view.owner,
            },
            ProjectileTransform {
                position: view.position,
                velocity: view.velocity,
            },
            ProjectileChunk(chunk),
        ))
        .id();
    world.resource_mut::<ProjectileIndex>().insert(id, entity);
    entity
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_world() -> World {
        let mut world = World::new();
        world.init_resource::<ProjectileIndex>();
        world
    }

    fn arrow_view(id: ProjectileId) -> ProjectileView {
        ProjectileView {
            id,
            model: ItemModel::Bow,
            owner: crate::protocol::ClientId(1),
            position: Vec3Net::new(0.0, 1.6, 0.0),
            velocity: Vec3Net::new(0.0, 0.0, -35.0),
        }
    }

    #[test]
    fn spawn_and_despawn_round_trip_index() {
        let mut world = fresh_world();
        let entity = spawn_projectile_entity(
            &mut world,
            arrow_view(crate::protocol::ProjectileId(7)),
            ChunkCoord::new(0, 0),
        );
        assert_eq!(
            world
                .resource::<ProjectileIndex>()
                .get(crate::protocol::ProjectileId(7)),
            Some(entity)
        );

        let identity = world.get::<Projectile>(entity).expect("identity");
        assert_eq!(identity.id, crate::protocol::ProjectileId(7));
        assert_eq!(identity.model, ItemModel::Bow);
        let transform = world.get::<ProjectileTransform>(entity).expect("transform");
        assert_eq!(transform.velocity.z, -35.0);

        let despawned = despawn_projectile_entity(&mut world, crate::protocol::ProjectileId(7));
        assert_eq!(despawned, Some(entity));
        assert!(
            world
                .resource::<ProjectileIndex>()
                .get(crate::protocol::ProjectileId(7))
                .is_none()
        );
    }
}
