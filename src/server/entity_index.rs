//! Shared scaffolding for the `Id -> Entity` indexes that sit next to each
//! networked entity's ECS mirror.
//!
//! Every replicated entity type (resource nodes, dropped items, deployables,
//! players, loot bags) keeps the authoritative state in a `HashMap` on
//! `GameServer` and mirrors it into ECS entities carrying the
//! Lightyear-replicated components. Each one also needs a sibling resource
//! mapping its wire-stable id back to the owning `Entity` so gather/admin/sync
//! paths can resolve `id -> entity` in O(1) without scanning a query.
//!
//! All five ids are `u64` aliases, so a single generic `EntityIndex<u64>`
//! would collapse into one shared resource. The [`entity_index!`] macro instead
//! stamps out a distinct, named resource type per entity kind (plus the
//! matching `despawn_*_entity` helper) from one definition, so adding a new
//! networked entity is a one-line invocation instead of another ~40 lines of
//! copy-pasted `get`/`insert`/`remove`/`iter`/despawn boilerplate.

/// Generate an `Id -> Entity` index resource named `$name` keyed by `$id`, plus
/// a `$despawn(world, id)` free function that removes the entry and despawns the
/// entity. See the module docs for why this is a macro rather than a generic.
macro_rules! entity_index {
    (
        $(#[$index_meta:meta])*
        $name:ident, $id:ty;
        $(#[$despawn_meta:meta])*
        $despawn:ident $(,)?
    ) => {
        $(#[$index_meta])*
        #[derive(bevy::prelude::Resource, Default, Debug)]
        pub struct $name {
            by_id: std::collections::HashMap<$id, bevy::prelude::Entity>,
        }

        impl $name {
            pub fn get(&self, id: $id) -> Option<bevy::prelude::Entity> {
                self.by_id.get(&id).copied()
            }

            pub fn insert(&mut self, id: $id, entity: bevy::prelude::Entity) {
                self.by_id.insert(id, entity);
            }

            pub fn remove(&mut self, id: $id) -> Option<bevy::prelude::Entity> {
                self.by_id.remove(&id)
            }

            pub fn len(&self) -> usize {
                self.by_id.len()
            }

            pub fn is_empty(&self) -> bool {
                self.by_id.is_empty()
            }

            pub fn iter(&self) -> impl Iterator<Item = ($id, bevy::prelude::Entity)> + '_ {
                self.by_id.iter().map(|(id, entity)| (*id, *entity))
            }

            pub fn clear(&mut self) {
                self.by_id.clear();
            }
        }

        $(#[$despawn_meta])*
        pub fn $despawn(
            world: &mut bevy::prelude::World,
            id: $id,
        ) -> Option<bevy::prelude::Entity> {
            let entity = world.resource_mut::<$name>().remove(id)?;
            if let Ok(entity_world) = world.get_entity_mut(entity) {
                entity_world.despawn();
            }
            Some(entity)
        }
    };
}

pub(crate) use entity_index;
