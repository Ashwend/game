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
    rebind_player_owner_if_changed,
};
use super::routing::ServerConnections;

/// Max number of *fresh* resource-node mirror entities to spawn in a single
/// sync pass. World-load-on-connect seeds every node id dirty at once (~1800
/// entities); spawning them all in one tick is a ~200ms `&mut World` stall
/// (two archetype moves + a room observer each). Capping new spawns per tick
/// and requeueing the overflow spreads the initial fill over a handful of
/// ~20Hz ticks (~700ms for 1800 nodes) instead. Only the fresh-spawn arm is
/// budgeted; refreshes and despawns stay uncapped so live gather diffs are
/// never delayed. Far nodes are AoI-room-gated, so a just-connected player
/// never notices the ones still streaming in beyond their chunk ring.
const MAX_RESOURCE_NODE_SPAWNS_PER_SYNC: usize = 128;

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
    use crate::protocol::ResourceNodeId;

    let _span = info_span!("sync_resource_node_entities").entered();

    // Drain the delta as *ids only* (no state clone yet). The authoritative map
    // records which node ids changed (`dirty`) or were removed since the last
    // pass, so we only touch the delta instead of walking every live node.
    let (dirty_ids, removed): (Vec<ResourceNodeId>, Vec<ResourceNodeId>) = world
        .resource_mut::<AuthoritativeServer>()
        .0
        .drain_resource_node_sync();

    // 1. Despawn the mirror entities for removed ids (no-op if one was added
    //    and removed within the same sync window, it never got an entity).
    for id in removed {
        crate::server::despawn_resource_node_entity(world, id);
    }

    // 2. Classify dirty ids into already-mirrored (refresh in place, uncapped)
    //    vs new (spawn, budgeted) using only the cheap index lookup. Doing this
    //    BEFORE cloning any state bounds the expensive per-tick work (state
    //    clones + archetype-moving spawns) to the budget, not the whole backlog:
    //    world-load seeds *every* node id dirty and the budget requeues the
    //    overflow, so a naive `drain -> clone all -> spawn budget` re-clones the
    //    entire requeued backlog every tick (O(n²) allocations). Here only the
    //    entities actually touched this tick are cloned. (The requeued tail is
    //    still re-classified each tick, but that's an allocation-free index
    //    lookup, ~cheap; a fully O(n) drain would need a persistent pending
    //    queue, not worth the extra state at current node counts.)
    let mut existing: Vec<(ResourceNodeId, Entity)> = Vec::new();
    let mut new_ids: Vec<ResourceNodeId> = Vec::new();
    {
        let index = world.resource::<crate::server::ResourceNodeIndex>();
        for id in dirty_ids {
            match index.get(id) {
                Some(entity) => existing.push((id, entity)),
                None => new_ids.push(id),
            }
        }
    }

    // Budget fresh spawns; the overflow ids are requeued (cheap, no clone) and
    // drained next tick. Refreshes are never capped so live gather / regrow
    // diffs ship without delay even while the initial fill is still draining.
    let spawn_now = new_ids.len().min(MAX_RESOURCE_NODE_SPAWNS_PER_SYNC);
    let requeue = new_ids.split_off(spawn_now);

    // 3. Snapshot authoritative state for *only* the refreshes + budgeted spawns,
    //    releasing the server borrow before the spawn / despawn calls need
    //    `&mut World`.
    #[allow(clippy::type_complexity)]
    let (refreshes, spawns): (
        Vec<(Entity, ResourceNodeId, crate::protocol::ResourceNodeState)>,
        Vec<(crate::protocol::ResourceNodeState, Option<ChunkCoord>)>,
    ) = {
        let server = world.resource::<AuthoritativeServer>();
        let refreshes = existing
            .iter()
            .filter_map(|&(id, entity)| {
                server
                    .0
                    .resource_node_state(id)
                    .map(|state| (entity, id, state.clone()))
            })
            .collect();
        let spawns = new_ids
            .iter()
            .filter_map(|&id| {
                server
                    .0
                    .resource_node_state(id)
                    .map(|state| (state.clone(), server.0.resource_node_chunk(id)))
            })
            .collect();
        (refreshes, spawns)
    };

    // 4. Refresh storage in place. Change detection will only mark it changed
    //    when the Vec actually differs, that's what triggers Lightyear's
    //    per-component diff ship.
    for (entity, _id, state) in refreshes {
        if let Some(mut storage) = world.get_mut::<crate::server::ResourceNodeStorage>(entity)
            && storage.0 != state.storage
        {
            #[cfg(feature = "replication-trace")]
            {
                let before: u16 = storage.0.iter().map(|s| s.quantity).sum();
                let after: u16 = state.storage.iter().map(|s| s.quantity).sum();
                info!(
                    target: "replication_trace",
                    "server: ResourceNodeStorage MUTATE id={_id} entity={entity:?} {before} -> {after}"
                );
            }
            storage.0 = state.storage;
        }
    }

    // 5. Spawn the budgeted fresh entities.
    for (state, chunk) in spawns {
        // Find the chunk this node anchors to. If chunk_manager hasn't tracked
        // it yet (admin spawn arrived after the resource_nodes insert but before
        // track_resource_node), fall back to the position's chunk so the entity
        // still has a coord; the next tick resyncs the membership.
        let chunk = chunk.unwrap_or_else(|| {
            crate::world::ChunkCoord::from_world(state.position.x, state.position.z)
        });
        let entity = crate::server::spawn_resource_node_entity(world, state, chunk);
        attach_room_gated_replication(world, entity, chunk);
    }

    // 6. Requeue the overflow new ids so the next pass drains the next batch.
    if !requeue.is_empty() {
        world
            .resource_mut::<AuthoritativeServer>()
            .0
            .requeue_resource_node_sync(requeue);
    }
}

/// Reconciles `GameServer::dropped_items` into ECS entities. Same shape
/// as `sync_resource_node_entities`: the authoritative map records which
/// item ids changed (`dirty`) or were removed since the last pass, so we
/// only touch the delta instead of walking every live drop each tick.
/// The physics step marks an id dirty only while its transform actually
/// changes, so settled items cost nothing here. Stack writes are
/// change-detected so the `Changed<DroppedItem>` signal only fires on
/// real merges.
pub(super) fn sync_dropped_item_entities(world: &mut World) {
    let _span = info_span!("sync_dropped_item_entities").entered();
    // Snapshot the (small) set of changed states + anchor chunks up front
    // so the `Res` borrow is released before the spawn/despawn calls need
    // `&mut World`.
    #[allow(clippy::type_complexity)]
    let (dirty_states, removed): (
        Vec<(
            crate::protocol::DroppedItemId,
            crate::protocol::DroppedWorldItem,
            Option<ChunkCoord>,
        )>,
        Vec<crate::protocol::DroppedItemId>,
    ) = {
        let mut server = world.resource_mut::<AuthoritativeServer>();
        let (dirty_ids, removed_ids) = server.0.drain_dropped_item_sync();
        let dirty_states = dirty_ids
            .into_iter()
            .filter_map(|id| {
                server
                    .0
                    .dropped_item_state(id)
                    .map(|state| (id, state.clone(), server.0.dropped_item_chunk(id)))
            })
            .collect();
        (dirty_states, removed_ids)
    };

    // 1. Despawn the mirror entities for removed ids (no-op if one was added
    //    and removed within the same sync window, it never got an entity).
    for id in removed {
        crate::server::despawn_dropped_item_entity(world, id);
    }

    // 2. Spawn fresh entities for new ids; refresh transform + stack for
    //    changed ones.
    for (id, item, live_chunk) in dirty_states {
        let existing = world.resource::<crate::server::DroppedItemIndex>().get(id);
        match existing {
            Some(entity) => {
                // Transform changes every physics tick while the body is
                // settling; the value compare suppresses no-op writes so
                // only real moves emit a `Changed` tick.
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
                    #[cfg(feature = "replication-trace")]
                    info!(
                        target: "replication_trace",
                        "server: DroppedItem          MUTATE id={id} entity={entity:?} stack {:?} -> {:?}",
                        drop.stack, item.stack
                    );
                    drop.stack = item.stack;
                }
                // Dropped items can roll between chunks while their physics
                // body settles. Keep the room membership and the
                // `DroppedItemChunk` mirror in step so observing clients
                // gain/lose visibility at the boundary instead of seeing
                // the entity disappear off-screen.
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
                // If chunk_manager hasn't tracked the drop yet, fall back
                // to the position's chunk so the entity still has a coord;
                // the next dirty mark will resync the membership.
                let chunk = live_chunk.unwrap_or_else(|| {
                    crate::world::ChunkCoord::from_world(item.position.x, item.position.z)
                });
                let entity = crate::server::spawn_dropped_item_entity(world, item, chunk);
                attach_room_gated_replication(world, entity, chunk);
            }
        }
    }
}

/// Reconciles `GameServer::deployed_entities` into ECS entities. Same
/// delta shape as `sync_resource_node_entities`: drain the dirty/removed
/// sets and only touch the changed ids. Each surviving id has its
/// `DeployableHealth` and `DeployableActive` refreshed in place so a
/// furnace switching on/off or a wall taking a hit ships exactly one
/// component delta.
pub(super) fn sync_deployable_entities(world: &mut World) {
    let _span = info_span!("sync_deployable_entities").entered();
    // Snapshot the (small) set of changed views + anchor chunks up front
    // so the `Res` borrow is released before the spawn/despawn calls need
    // `&mut World`.
    #[allow(clippy::type_complexity)]
    let (dirty_views, removed): (
        Vec<(crate::server::DeployableView, Option<ChunkCoord>)>,
        Vec<crate::protocol::DeployedEntityId>,
    ) = {
        let mut server = world.resource_mut::<AuthoritativeServer>();
        let (dirty_ids, removed_ids) = server.0.drain_deployable_sync();
        let dirty_views = dirty_ids
            .into_iter()
            .filter_map(|id| {
                server
                    .0
                    .deployable_view(id)
                    .map(|view| (view, server.0.deployable_chunk(id)))
            })
            .collect();
        (dirty_views, removed_ids)
    };

    // 1. Despawn the mirror entities for removed ids (no-op if one was added
    //    and removed within the same sync window, it never got an entity).
    for id in removed {
        crate::server::despawn_deployable_entity(world, id);
    }

    // 2. Spawn fresh entities for new ids; refresh health/active/label
    //    for changed ones. A kind change (hammer tier upgrade) can't be
    //    expressed as a diff, `Deployable` identity is immutable
    //    post-spawn by design, so the mirror entity is despawned and
    //    respawned; clients see a remove + add through the normal
    //    `Added`/`RemovedComponents` lifecycle.
    for (view, live_chunk) in dirty_views {
        let existing = world
            .resource::<crate::server::DeployableIndex>()
            .get(view.id);
        let existing = match existing {
            Some(entity) => {
                let kind_changed = world
                    .get::<crate::server::Deployable>(entity)
                    .is_some_and(|meta| meta.kind != view.kind);
                if kind_changed {
                    #[cfg(feature = "replication-trace")]
                    info!(
                        target: "replication_trace",
                        "server: Deployable         RESPAWN id={} entity={entity:?} kind changed to {:?}",
                        view.id, view.kind
                    );
                    crate::server::despawn_deployable_entity(world, view.id);
                    None
                } else {
                    Some(entity)
                }
            }
            None => None,
        };
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
                if let Some(mut label) = world.get_mut::<crate::server::DeployableLabel>(entity)
                    && label.0 != view.label
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        info!(
                            target: "replication_trace",
                            "server: DeployableLabel    MUTATE id={} entity={entity:?} {:?} -> {:?}",
                            view.id, label.0, view.label
                        );
                    }
                    label.0 = view.label;
                }
                if let Some(mut stability) =
                    world.get_mut::<crate::server::DeployableStability>(entity)
                    && stability.0 != view.stability
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = stability.0;
                        info!(
                            target: "replication_trace",
                            "server: DeployableStability MUTATE id={} entity={entity:?} {before} -> {}",
                            view.id, view.stability
                        );
                    }
                    stability.0 = view.stability;
                }
            }
            None => {
                // If chunk_manager hasn't tracked the placement yet, fall
                // back to the position's chunk so the entity still has a
                // coord; the next dirty mark will resync the membership.
                let chunk = live_chunk.unwrap_or_else(|| {
                    crate::world::ChunkCoord::from_world(view.position.x, view.position.z)
                });
                let entity = crate::server::spawn_deployable_entity(world, view, chunk);
                attach_room_gated_replication(world, entity, chunk);
            }
        }
    }
}

/// Compare-and-write one replicated player component. Writes only when
/// the freshly built view value differs from the live component, so
/// Lightyear sees a change tick (and ships a diff) only for fields that
/// actually moved. Each component carries one cadence: the pose ticks
/// at 20 Hz while moving, the input ack with every accepted input, the
/// inventory only on real mutations, so splitting the writes is what
/// keeps the heavyweight components off the per-tick wire.
macro_rules! refresh_player_component {
    ($world:expr, $entity:expr, $client_id:expr, $label:literal, $ty:ty, $value:expr) => {
        if let Some(mut current) = $world.get_mut::<$ty>($entity)
            && *current != $value
        {
            #[cfg(feature = "replication-trace")]
            info!(
                target: "replication_trace",
                concat!("server: ", $label, " MUTATE client={} entity={:?}"),
                $client_id, $entity
            );
            *current = $value;
        }
    };
}

/// Reconciles `GameServer::clients` into ECS entities. Spawns one entity
/// per connected client and keeps each replicated component in sync
/// with the authoritative `ServerClient`, one compare-and-write per
/// component so every field group diffs at its own cadence. The
/// owner-only components (inventory, crafting, containers, input ack)
/// are gated to the owning sender via `ComponentReplicationOverrides`
/// (see `attach_player_replication`).
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
                // Re-point the owner-only PlayerPrivate override if this
                // player's sender changed (a reconnect that woke a sleeping
                // body keeps this same mirror entity but gets a brand-new
                // sender), otherwise the woken player's inventory/crafting
                // never replicates to their new connection.
                let current_sender = world
                    .resource::<ServerConnections>()
                    .entity_for_client(view.client_id);
                rebind_player_owner_if_changed(world, entity, current_sender);
                // Peer-visible components. The pose ticks every tick
                // while moving; profile/health/bubble only on real
                // changes.
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerPose         ",
                    crate::server::PlayerPose,
                    view.pose
                );
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerProfile      ",
                    crate::server::PlayerProfile,
                    view.profile
                );
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerHealth       ",
                    crate::server::PlayerHealth,
                    view.health
                );
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerChatBubble   ",
                    crate::server::PlayerChatBubble,
                    view.chat_bubble
                );
                // Peer-visible cosmetic state for the rigged body. NOT
                // owner-gated (every peer in the room renders these): the held
                // mesh changes on a tool swap, the action seq on every swing.
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerHeldItem     ",
                    crate::server::PlayerHeldItem,
                    view.held
                );
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerAction       ",
                    crate::server::PlayerAction,
                    view.action
                );
                // Owner-only components, replicated to the owning
                // sender only.
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerInventory    ",
                    crate::server::PlayerInventory,
                    view.inventory
                );
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerCrafting     ",
                    crate::server::PlayerCrafting,
                    view.crafting
                );
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerOpenContainers",
                    crate::server::PlayerOpenContainers,
                    view.containers
                );
                refresh_player_component!(
                    world,
                    entity,
                    view.client_id,
                    "PlayerInputAck     ",
                    crate::server::PlayerInputAck,
                    view.input_ack
                );
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
                // Refresh the sleeping flag. Flips when a player logs out
                // (their body stays as a sleeping body) or reconnects (the
                // body wakes). Peers render the sleeping pose + tooltip off
                // this.
                if let Some(mut sleeping) = world.get_mut::<crate::server::PlayerSleeping>(entity)
                    && *sleeping != view.sleeping
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before = *sleeping;
                        info!(
                            target: "replication_trace",
                            "server: PlayerSleeping     MUTATE client={} entity={entity:?} {before:?} -> {:?}",
                            view.client_id, view.sleeping
                        );
                    }
                    *sleeping = view.sleeping;
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
                            view.pose.position.x,
                            view.pose.position.z,
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

/// Reconciles `GameServer::loot_bags` into ECS entities. Sync per
/// tick covers:
///   - `LootBagTransform`: changes while the spawn-time gravity
///     settle is still in flight (the bag falls from chest height
///     to the ground over ~0.4 s). Without refreshing the
///     replicated transform the client would see the bag frozen at
///     its spawn position.
///   - `LootBagContents`: **trace builds only.** Nothing in release
///     builds consumes the replicated contents (the bag UI rides the
///     owner-only `PlayerOpenContainers::open_loot_bag` view), so the
///     release path neither clones the slot list per tick nor ships it
///     to peers; see the component's doc in `loot_bag_ecs.rs`.
///
/// Loot bags deliberately use a full walk (no dirty set): death bags are far
/// rarer than nodes/drops, and the settling-transform bulk path would need
/// per-tick dirty marking to avoid freezing a falling bag.
pub(super) fn sync_loot_bag_entities(world: &mut World) {
    let _span = info_span!("sync_loot_bag_entities").entered();
    let known: std::collections::HashSet<crate::protocol::LootBagId> = world
        .resource::<crate::server::LootBagIndex>()
        .iter()
        .map(|(id, _)| id)
        .collect();
    let authoritative: Vec<crate::server::LootBagView> = {
        let server = world.resource::<AuthoritativeServer>();
        server
            .0
            .loot_bags_iter()
            .map(|(id, bag)| crate::server::LootBagView {
                id,
                position: bag.position,
                yaw: bag.yaw,
                // Slot clones are only needed where the contents
                // component is actually written: at spawn, and per tick
                // in trace builds. The release steady state skips the
                // per-bag Vec clone entirely.
                slots: (cfg!(feature = "replication-trace") || !known.contains(&id))
                    .then(|| bag.slots.clone()),
            })
            .collect()
    };
    let live_ids: std::collections::HashSet<crate::protocol::LootBagId> =
        authoritative.iter().map(|view| view.id).collect();

    let stale: Vec<crate::protocol::LootBagId> = known
        .iter()
        .copied()
        .filter(|id| !live_ids.contains(id))
        .collect();
    for id in stale {
        crate::server::despawn_loot_bag_entity(world, id);
    }

    for view in authoritative {
        let existing = world.resource::<crate::server::LootBagIndex>().get(view.id);
        match existing {
            Some(entity) => {
                #[cfg(feature = "replication-trace")]
                if let Some(slots) = view.slots.clone()
                    && let Some(mut contents) =
                        world.get_mut::<crate::server::LootBagContents>(entity)
                    && contents.0 != slots
                {
                    {
                        let before: usize = contents.0.iter().filter(|s| s.is_some()).count();
                        let after: usize = slots.iter().filter(|s| s.is_some()).count();
                        info!(
                            target: "replication_trace",
                            "server: LootBagContents    MUTATE id={} entity={entity:?} occupied {before} -> {after}",
                            view.id
                        );
                    }
                    contents.0 = slots;
                }
                // Refresh transform while the bag is still settling.
                // Change detection suppresses no-op writes, so once
                // the bag is at rest this short-circuits.
                let new_transform = crate::server::LootBagTransform {
                    position: view.position,
                    yaw: view.yaw,
                };
                if let Some(mut transform) =
                    world.get_mut::<crate::server::LootBagTransform>(entity)
                    && (transform.position != new_transform.position
                        || transform.yaw != new_transform.yaw)
                {
                    #[cfg(feature = "replication-trace")]
                    info!(
                        target: "replication_trace",
                        "server: LootBagTransform     MUTATE id={} entity={entity:?} pos {:?} -> {:?}",
                        view.id, transform.position, new_transform.position
                    );
                    *transform = new_transform;
                }
            }
            None => {
                let chunk = world
                    .resource::<AuthoritativeServer>()
                    .0
                    .loot_bag_chunk(view.id)
                    .unwrap_or_else(|| {
                        crate::world::ChunkCoord::from_world(view.position.x, view.position.z)
                    });
                let entity = crate::server::spawn_loot_bag_entity(world, view, chunk);
                attach_room_gated_replication(world, entity, chunk);
                // Release builds keep the contents off the wire: no
                // client-side consumer exists and replicating every
                // bag's full slot list to every peer in the room is
                // both bandwidth waste and an information leak. Trace
                // builds leave it enabled for MUTATE/RECV coverage.
                #[cfg(not(feature = "replication-trace"))]
                if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
                    entity_mut.insert(
                        lightyear::prelude::ComponentReplicationOverrides::<
                            crate::server::LootBagContents,
                        >::default()
                        .disable_all(),
                    );
                }
            }
        }
    }
}
