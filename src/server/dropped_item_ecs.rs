//! ECS mirror for authoritative dropped items.
//!
//! Companion to [`crate::server::resource_node_ecs`], the dropped item
//! analogue. `GameServer::dropped_items` (HashMap of physics-body-backed
//! [`crate::protocol::DroppedWorldItem`]) stays authoritative; the
//! `sync_dropped_item_entities` system in `net/host.rs` reconciles it
//! into ECS entities so Phase 4 room replication can attach `Replicate`
//! to the entities without changing the HashMap-driven gameplay paths.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    protocol::{DroppedItemId, DroppedWorldItem, ItemStack, QuatNet, Vec3Net},
    world::ChunkCoord,
};

/// Identity + the stack of items the drop carries. `item_id`/`quantity`
/// only change when the server merges nearby drops (rare, low-frequency
/// event), so keeping them on the same component is fine, Lightyear's
/// per-component change detection still fires only when a merge happens.
///
/// `Serialize`/`Deserialize`/`PartialEq` are required by Lightyear's
/// component replication (Phase 5).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DroppedItem {
    pub id: DroppedItemId,
    pub stack: ItemStack,
}

/// Pose for a dropped item. The physics simulation steps the body every
/// tick and the mirror writes its result here, so this component changes
/// every tick a drop is settling and stops changing once the drop comes
/// to rest. Split from [`DroppedItem`] so Lightyear's per-component delta
/// stream only ships transform updates while the body is moving.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DroppedItemTransform {
    pub position: Vec3Net,
    pub yaw: f32,
    pub rotation: QuatNet,
}

/// Anchor chunk for room subscription. Updated by the mirror whenever the
/// drop crosses a chunk boundary.
#[derive(Component, Debug, Clone, Copy)]
pub struct DroppedItemChunk(pub ChunkCoord);

crate::server::entity_index::entity_index! {
    /// `DroppedItemId → Entity` so the pickup path can resolve a drop in O(1)
    /// without scanning a query.
    DroppedItemIndex, DroppedItemId;
    despawn_dropped_item_entity
}

pub fn spawn_dropped_item_entity(
    world: &mut World,
    item: DroppedWorldItem,
    chunk: ChunkCoord,
) -> Entity {
    let id = item.id;
    let transform = DroppedItemTransform {
        position: item.position,
        yaw: item.yaw,
        rotation: item.rotation,
    };
    let entity = world
        .spawn((
            DroppedItem {
                id: item.id,
                stack: item.stack,
            },
            transform,
            DroppedItemChunk(chunk),
        ))
        .id();
    world.resource_mut::<DroppedItemIndex>().insert(id, entity);
    entity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::COAL_ID;

    fn drop_state(id: DroppedItemId, quantity: u16) -> DroppedWorldItem {
        DroppedWorldItem {
            id,
            stack: ItemStack::new(COAL_ID, quantity),
            position: Vec3Net::new(1.0, 0.0, 2.0),
            yaw: 0.3,
            rotation: QuatNet::IDENTITY,
        }
    }

    fn fresh_world() -> World {
        let mut world = World::new();
        world.init_resource::<DroppedItemIndex>();
        world
    }

    #[test]
    fn spawn_and_despawn_round_trips_index() {
        let mut world = fresh_world();
        let entity = spawn_dropped_item_entity(
            &mut world,
            drop_state(crate::protocol::DroppedItemId(3), 5),
            ChunkCoord::new(0, 0),
        );
        assert_eq!(
            world
                .resource::<DroppedItemIndex>()
                .get(crate::protocol::DroppedItemId(3)),
            Some(entity)
        );

        let drop = world.get::<DroppedItem>(entity).expect("drop component");
        assert_eq!(drop.id, crate::protocol::DroppedItemId(3));
        assert_eq!(drop.stack.quantity, 5);

        let despawned = despawn_dropped_item_entity(&mut world, crate::protocol::DroppedItemId(3));
        assert_eq!(despawned, Some(entity));
        assert!(
            world
                .resource::<DroppedItemIndex>()
                .get(crate::protocol::DroppedItemId(3))
                .is_none()
        );
    }
}
