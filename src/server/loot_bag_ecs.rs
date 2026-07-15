//! ECS mirror for authoritative loot bags (death drop containers).
//!
//! Companion to [`crate::server::dropped_item_ecs`]. The authoritative
//! state lives in `GameServer::loot_bags`; the `sync_loot_bag_entities`
//! system in `net/host.rs` reconciles it into ECS entities so chunk-
//! room replication can attach `Replicate` to them and the client
//! receives per-component diffs of the bag's slot list.
//!
//! Split into:
//!   - [`LootBag`], identity (immutable post-spawn).
//!   - [`LootBagTransform`], placement pose (static after spawn).
//!   - [`LootBagContents`], the mutable slot list. Changes when a
//!     player drags items in or out; per-component replication keeps
//!     wire traffic to just the contents diff.

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

/// Slot grid snapshot. Stack list of fixed length
/// (`LOOT_BAG_SLOT_COUNT`); a slot is `None` when empty.
///
/// **Release builds neither replicate nor refresh this component
/// post-spawn.** Nothing client-side consumes it, the bag UI renders
/// from the owner-only `PlayerOpenContainers::open_loot_bag` view, so
/// shipping every bag's full contents to every client in the chunk
/// room (~1-1.5 KB per packed death bag, re-sent on every loot move)
/// was pure bandwidth waste plus an information leak. The
/// `replication-trace` build re-enables both the per-tick refresh and
/// the wire path so MUTATE/RECV coverage stays available. Do not query
/// this client-side for gameplay.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LootBagContents(pub Vec<Option<ItemStack>>);

/// Anchor chunk for room subscription.
#[derive(Component, Debug, Clone, Copy)]
pub struct LootBagChunk(pub ChunkCoord);

crate::server::entity_index::entity_index! {
    /// `LootBagId → Entity` lookup for gameplay-side O(1) reads.
    LootBagIndex, LootBagId;
    despawn_loot_bag_entity
}

/// Wire-shape view used by the mirror to spawn or refresh a bag
/// entity. `slots` is `None` when the caller skipped the clone (the
/// release-build steady state; see [`LootBagContents`]).
pub struct LootBagView {
    pub id: LootBagId,
    pub position: Vec3Net,
    pub yaw: f32,
    pub slots: Option<Vec<Option<ItemStack>>>,
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
            LootBagContents(view.slots.unwrap_or_default()),
            LootBagChunk(chunk),
        ))
        .id();
    world.resource_mut::<LootBagIndex>().insert(id, entity);
    entity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ItemStack;

    fn world_with_index() -> World {
        let mut world = World::new();
        world.init_resource::<LootBagIndex>();
        world
    }

    #[test]
    fn index_round_trips_inserted_id() {
        // Spawn a throwaway entity rather than fabricating one, the
        // public Entity constructor surface changes between bevy
        // releases, but `World::spawn_empty()` is stable.
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut idx = LootBagIndex::default();
        idx.insert(crate::protocol::LootBagId(42), entity);
        assert_eq!(idx.get(crate::protocol::LootBagId(42)), Some(entity));
        assert_eq!(idx.iter().count(), 1);
        assert_eq!(idx.remove(crate::protocol::LootBagId(42)), Some(entity));
        assert!(
            idx.get(crate::protocol::LootBagId(42)).is_none(),
            "removed ids should be gone"
        );
        assert_eq!(idx.iter().count(), 0);
    }

    #[test]
    fn spawn_loot_bag_entity_attaches_components_and_indexes_id() {
        let mut world = world_with_index();
        let view = LootBagView {
            id: crate::protocol::LootBagId(9),
            position: Vec3Net::new(1.0, 2.0, 3.0),
            yaw: 0.25,
            slots: Some(vec![Some(ItemStack::new("wood", 4)), None]),
        };
        let chunk = ChunkCoord::new(0, 0);
        let entity = spawn_loot_bag_entity(&mut world, view, chunk);

        // Every replicated component should be present on the entity.
        let id = world
            .entity(entity)
            .get::<LootBag>()
            .copied()
            .expect("spawn attaches LootBag identity");
        assert_eq!(id.id, crate::protocol::LootBagId(9));
        let transform = world
            .entity(entity)
            .get::<LootBagTransform>()
            .copied()
            .expect("spawn attaches LootBagTransform pose");
        assert_eq!(transform.position, Vec3Net::new(1.0, 2.0, 3.0));
        assert_eq!(transform.yaw, 0.25);
        let contents = world
            .entity(entity)
            .get::<LootBagContents>()
            .cloned()
            .expect("spawn attaches LootBagContents slots");
        assert_eq!(contents.0.len(), 2);
        let LootBagChunk(coord) = world
            .entity(entity)
            .get::<LootBagChunk>()
            .copied()
            .expect("spawn attaches LootBagChunk anchor");
        assert_eq!(coord, chunk);

        // The index should know about the new bag for O(1) ECS lookup.
        assert_eq!(
            world
                .resource::<LootBagIndex>()
                .get(crate::protocol::LootBagId(9)),
            Some(entity)
        );
    }

    #[test]
    fn despawn_removes_entity_and_index_entry() {
        let mut world = world_with_index();
        let view = LootBagView {
            id: crate::protocol::LootBagId(3),
            position: Vec3Net::ZERO,
            yaw: 0.0,
            slots: None,
        };
        let _ = spawn_loot_bag_entity(&mut world, view, ChunkCoord::new(0, 0));
        assert!(
            world
                .resource::<LootBagIndex>()
                .get(crate::protocol::LootBagId(3))
                .is_some()
        );

        let removed = despawn_loot_bag_entity(&mut world, crate::protocol::LootBagId(3));
        assert!(removed.is_some());
        assert!(
            world
                .resource::<LootBagIndex>()
                .get(crate::protocol::LootBagId(3))
                .is_none(),
            "despawn must clear the index so a fresh spawn doesn't clash with a stale entity"
        );
    }

    #[test]
    fn despawn_unknown_id_is_noop() {
        let mut world = world_with_index();
        assert!(despawn_loot_bag_entity(&mut world, crate::protocol::LootBagId(999)).is_none());
    }
}
