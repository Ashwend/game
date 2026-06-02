//! Exclusive ECS-mirror systems that reconcile `GameServer` authoritative
//! state into the replicated ECS entities Lightyear ships to clients.
//!
//! Each system walks (the delta of) one authoritative `HashMap` on
//! `GameServer` and spawns / despawns / refreshes the matching mirror entity
//! and its per-component replicated fields. They run as exclusive systems
//! because spawning and despawning need `&mut World`.

use bevy::{log::info_span, prelude::*};

use crate::world::ChunkCoord;

use super::AuthoritativeServer;
use super::rooms::{
    attach_player_replication, attach_room_gated_replication, move_entity_between_rooms,
};
use super::routing::ServerConnections;

/// Reconciles the live `GameServer::resource_nodes` map into ECS entities
/// once per Update. New ids spawn fresh entities; missing ids despawn the
/// tracked entity; surviving ids get their `ResourceNodeStorage` refreshed
/// in place so the per-component value tracks the authoritative HashMap.
///
/// Runs as an exclusive system because spawning / despawning needs
/// `&mut World`. Cheap in steady state (no allocations when the id set
/// is unchanged); the storage refresh writes are change-detected by Bevy
/// so they only emit `Changed` ticks when the inner Vec actually differs.
pub(super) fn sync_resource_node_entities(world: &mut World) {
    let _span = info_span!("sync_resource_node_entities").entered();
    // Incremental sync: the authoritative map records which node ids changed
    // (`dirty`) or were removed since the last pass, so we only touch the delta
    // instead of walking all live nodes every tick. We snapshot the (small) set
    // of changed states + anchor chunks up front so the `Res` borrow is
    // released before the spawn/despawn calls need `&mut World`.
    #[allow(clippy::type_complexity)]
    let (dirty_states, removed): (
        Vec<(
            crate::protocol::ResourceNodeId,
            crate::protocol::ResourceNodeState,
            Option<ChunkCoord>,
        )>,
        Vec<crate::protocol::ResourceNodeId>,
    ) = {
        let mut server = world.resource_mut::<AuthoritativeServer>();
        let (dirty_ids, removed_ids) = server.0.drain_resource_node_sync();
        let dirty_states = dirty_ids
            .into_iter()
            .filter_map(|id| {
                server
                    .0
                    .resource_node_state(id)
                    .map(|state| (id, state.clone(), server.0.resource_node_chunk(id)))
            })
            .collect();
        (dirty_states, removed_ids)
    };

    // 1. Despawn the mirror entities for removed ids (no-op if one was added
    //    and removed within the same sync window, it never got an entity).
    for id in removed {
        crate::server::despawn_resource_node_entity(world, id);
    }

    // 2. Spawn fresh entities for new ids; refresh storage for changed ones.
    for (id, state, chunk) in dirty_states {
        let existing = world.resource::<crate::server::ResourceNodeIndex>().get(id);
        match existing {
            Some(entity) => {
                // Refresh storage in place. Change detection will only
                // mark it changed when the Vec actually differs, that's
                // what triggers Lightyear's per-component diff ship.
                if let Some(mut storage) =
                    world.get_mut::<crate::server::ResourceNodeStorage>(entity)
                    && storage.0 != state.storage
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before: u16 = storage.0.iter().map(|s| s.quantity).sum();
                        let after: u16 = state.storage.iter().map(|s| s.quantity).sum();
                        info!(
                            target: "replication_trace",
                            "server: ResourceNodeStorage MUTATE id={id} entity={entity:?} {before} -> {after}"
                        );
                    }
                    storage.0 = state.storage;
                }
            }
            None => {
                // Find the chunk this node anchors to. If chunk_manager
                // hasn't tracked it yet (admin spawn arrived after the
                // resource_nodes insert but before track_resource_node),
                // fall back to the position's chunk so the entity still
                // has a coord; the next tick will resync the membership.
                let chunk = chunk.unwrap_or_else(|| {
                    crate::world::ChunkCoord::from_world(state.position.x, state.position.z)
                });
                let entity = crate::server::spawn_resource_node_entity(world, state, chunk);
                attach_room_gated_replication(world, entity, chunk);
            }
        }
    }
}

/// Reconciles `GameServer::dropped_items` into ECS entities. Same shape
/// as `sync_resource_node_entities`: despawn ids that left the live map,
/// spawn fresh entities for new ids, refresh transform + stack in place
/// for surviving ids. Stack writes are change-detected so the
/// `Changed<DroppedItem>` signal only fires on real merges.
pub(super) fn sync_dropped_item_entities(world: &mut World) {
    let _span = info_span!("sync_dropped_item_entities").entered();
    let authoritative: Vec<(
        crate::protocol::DroppedItemId,
        crate::protocol::DroppedWorldItem,
    )> = {
        let server = world.resource::<AuthoritativeServer>();
        server.0.dropped_items_iter().collect()
    };
    let live_ids: std::collections::HashSet<crate::protocol::DroppedItemId> =
        authoritative.iter().map(|(id, _)| *id).collect();

    let stale: Vec<crate::protocol::DroppedItemId> = {
        let index = world.resource::<crate::server::DroppedItemIndex>();
        index
            .iter()
            .filter_map(|(id, _)| (!live_ids.contains(&id)).then_some(id))
            .collect()
    };
    for id in stale {
        crate::server::despawn_dropped_item_entity(world, id);
    }

    for (id, item) in authoritative {
        let existing = world.resource::<crate::server::DroppedItemIndex>().get(id);
        match existing {
            Some(entity) => {
                // Transform changes every physics tick while the body is
                // settling; refresh unconditionally but rely on Bevy's
                // change tick model to suppress no-op writes.
                let new_transform = crate::server::DroppedItemTransform {
                    position: item.position,
                    yaw: item.yaw,
                    rotation: item.rotation,
                };
                if let Some(mut transform) =
                    world.get_mut::<crate::server::DroppedItemTransform>(entity)
                    && (transform.position != new_transform.position
                        || transform.yaw != new_transform.yaw
                        || transform.rotation != new_transform.rotation)
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = transform.position;
                        info!(
                            target: "replication_trace",
                            "server: DroppedItemTransform MUTATE id={id} entity={entity:?} pos {before:?} -> {:?}",
                            new_transform.position
                        );
                    }
                    *transform = new_transform;
                }
                if let Some(mut drop) = world.get_mut::<crate::server::DroppedItem>(entity)
                    && drop.stack != item.stack
                {
                    drop.stack = item.stack;
                }
                // Dropped items can roll between chunks while their physics
                // body settles. Keep the room membership and the
                // `DroppedItemChunk` mirror in step so observing clients
                // gain/lose visibility at the boundary instead of seeing
                // the entity disappear off-screen.
                let live_chunk = world
                    .resource::<AuthoritativeServer>()
                    .0
                    .dropped_item_chunk(id);
                let old_chunk = world
                    .get::<crate::server::DroppedItemChunk>(entity)
                    .map(|c| c.0);
                if let (Some(live), Some(prev)) = (live_chunk, old_chunk)
                    && live != prev
                {
                    move_entity_between_rooms(world, entity, prev, live);
                    if let Some(mut chunk_marker) =
                        world.get_mut::<crate::server::DroppedItemChunk>(entity)
                    {
                        chunk_marker.0 = live;
                    }
                }
            }
            None => {
                let chunk = world
                    .resource::<AuthoritativeServer>()
                    .0
                    .dropped_item_chunk(id)
                    .unwrap_or_else(|| {
                        crate::world::ChunkCoord::from_world(item.position.x, item.position.z)
                    });
                let entity = crate::server::spawn_dropped_item_entity(world, item, chunk);
                attach_room_gated_replication(world, entity, chunk);
            }
        }
    }
}

/// Reconciles `GameServer::deployed_entities` into ECS entities. Each
/// surviving id has its `DeployableHealth` and `DeployableActive`
/// refreshed in place so a furnace switching on/off or a wall taking a
/// hit ships exactly one component delta in the future replication path.
pub(super) fn sync_deployable_entities(world: &mut World) {
    let _span = info_span!("sync_deployable_entities").entered();
    let authoritative: Vec<crate::server::DeployableView> = {
        let server = world.resource::<AuthoritativeServer>();
        server.0.deployables_iter().collect()
    };
    let live_ids: std::collections::HashSet<crate::protocol::DeployedEntityId> =
        authoritative.iter().map(|view| view.id).collect();

    let stale: Vec<crate::protocol::DeployedEntityId> = {
        let index = world.resource::<crate::server::DeployableIndex>();
        index
            .iter()
            .filter_map(|(id, _)| (!live_ids.contains(&id)).then_some(id))
            .collect()
    };
    for id in stale {
        crate::server::despawn_deployable_entity(world, id);
    }

    for view in authoritative {
        let existing = world
            .resource::<crate::server::DeployableIndex>()
            .get(view.id);
        match existing {
            Some(entity) => {
                if let Some(mut health) = world.get_mut::<crate::server::DeployableHealth>(entity)
                    && health.0 != view.health
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = health.0;
                        info!(
                            target: "replication_trace",
                            "server: DeployableHealth   MUTATE id={} entity={entity:?} {before} -> {}",
                            view.id, view.health
                        );
                    }
                    health.0 = view.health;
                }
                if let Some(mut active) = world.get_mut::<crate::server::DeployableActive>(entity)
                    && active.0 != view.active
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = active.0;
                        info!(
                            target: "replication_trace",
                            "server: DeployableActive   MUTATE id={} entity={entity:?} {before} -> {}",
                            view.id, view.active
                        );
                    }
                    active.0 = view.active;
                }
            }
            None => {
                let chunk = world
                    .resource::<AuthoritativeServer>()
                    .0
                    .deployable_chunk(view.id)
                    .unwrap_or_else(|| {
                        crate::world::ChunkCoord::from_world(view.position.x, view.position.z)
                    });
                let entity = crate::server::spawn_deployable_entity(world, view, chunk);
                attach_room_gated_replication(world, entity, chunk);
            }
        }
    }
}

/// Reconciles `GameServer::clients` into ECS entities. Spawns one entity
/// per connected client and keeps its public + private components in
/// sync with the authoritative `ServerClient`. The public/private split
/// is what Phase 5 uses to ship per-component `Replicate::to_clients`
/// targets, `NetworkTarget::All` for public, `Single(client_id)` for
/// private.
pub(super) fn sync_player_entities(world: &mut World) {
    let _span = info_span!("sync_player_entities").entered();
    let authoritative: Vec<crate::server::PlayerView> = {
        let server = world.resource::<AuthoritativeServer>();
        server.0.players_iter().collect()
    };
    let live_ids: std::collections::HashSet<crate::protocol::ClientId> =
        authoritative.iter().map(|view| view.client_id).collect();

    let stale: Vec<crate::protocol::ClientId> = {
        let index = world.resource::<crate::server::PlayerIndex>();
        index
            .iter()
            .filter_map(|(id, _)| (!live_ids.contains(&id)).then_some(id))
            .collect()
    };
    for id in stale {
        crate::server::despawn_player_entity(world, id);
    }

    for view in authoritative {
        let existing = world
            .resource::<crate::server::PlayerIndex>()
            .get(view.client_id);
        match existing {
            Some(entity) => {
                // Refresh public, position/velocity tick every frame.
                if let Some(mut public) = world.get_mut::<crate::server::PlayerPublic>(entity)
                    && *public != view.public
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = public.position;
                        info!(
                            target: "replication_trace",
                            "server: PlayerPublic       MUTATE client={} entity={entity:?} pos {before:?} -> {:?}",
                            view.client_id, view.public.position
                        );
                    }
                    *public = view.public;
                }
                // Refresh private, inventory/crafting change on user action.
                if let Some(mut private) = world.get_mut::<crate::server::PlayerPrivate>(entity)
                    && *private != view.private
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        info!(
                            target: "replication_trace",
                            "server: PlayerPrivate      MUTATE client={} entity={entity:?} last_input={}",
                            view.client_id, view.private.last_processed_input
                        );
                    }
                    *private = view.private;
                }
                // Refresh armor. Today only mutated by future systems,
                // change detection still tracks it so the wire diff is
                // ready the moment armor items start landing.
                if let Some(mut armor) = world.get_mut::<crate::server::PlayerArmor>(entity)
                    && *armor != view.armor
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = armor.0;
                        info!(
                            target: "replication_trace",
                            "server: PlayerArmor        MUTATE client={} entity={entity:?} {before} -> {}",
                            view.client_id, view.armor.0
                        );
                    }
                    *armor = view.armor;
                }
                // Refresh lifecycle. Flips on every death / respawn.
                // Triggers the corpse animation on peers and the death
                // splash on the owner.
                if let Some(mut lifecycle) = world.get_mut::<crate::server::PlayerLifecycle>(entity)
                    && *lifecycle != view.lifecycle
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = *lifecycle;
                        info!(
                            target: "replication_trace",
                            "server: PlayerLifecycle    MUTATE client={} entity={entity:?} {before:?} -> {:?}",
                            view.client_id, view.lifecycle
                        );
                    }
                    *lifecycle = view.lifecycle;
                }
                // Players walk; keep their room subscription aligned with
                // chunk_manager so peers gain/lose visibility at the
                // boundary instead of seeing the avatar pop out of view.
                let live_chunk = world
                    .resource::<AuthoritativeServer>()
                    .0
                    .player_chunk(view.client_id);
                let old_chunk = world.get::<crate::server::PlayerChunk>(entity).map(|c| c.0);
                if let (Some(live), Some(prev)) = (live_chunk, old_chunk)
                    && live != prev
                {
                    move_entity_between_rooms(world, entity, prev, live);
                    if let Some(mut chunk_marker) =
                        world.get_mut::<crate::server::PlayerChunk>(entity)
                    {
                        chunk_marker.0 = live;
                    }
                }
            }
            None => {
                let chunk = world
                    .resource::<AuthoritativeServer>()
                    .0
                    .player_chunk(view.client_id)
                    .unwrap_or_else(|| {
                        crate::world::ChunkCoord::from_world(
                            view.public.position.x,
                            view.public.position.z,
                        )
                    });
                let owner_sender = world
                    .resource::<ServerConnections>()
                    .entity_for_client(view.client_id);
                let entity = crate::server::spawn_player_entity(world, view, chunk);
                attach_player_replication(world, entity, chunk, owner_sender);
            }
        }
    }
}
