//! Chunk-room / AoI replication helpers for the host.
//!
//! Each `ChunkCoord` lazily owns one lightyear [`RoomId`] (allocated from the
//! [`RoomAllocator`] resource). Networked entities (resource nodes, dropped
//! items, deployables, players, loot bags) carry a [`Rooms`] component naming
//! their chunk's room, and each client sender carries a [`Rooms`] component
//! naming the rooms covering its AoI ring. lightyear ships an entity's
//! components to a sender iff the two share at least one room, and auto-despawns
//! on the client when the rooms diverge.
//!
//! Owner-only player state lives on a **separate private mirror entity** (see
//! [`crate::server::PlayerPrivateState`]) that sits in a per-client private room
//! which only that client's sender joins, so its inventory/crafting never reach
//! peers. lightyear 0.28 (bevy_replicon-backed) removed the old
//! `ComponentReplicationOverrides` per-component gate; the private room replaces
//! it. That backend also removed `ReplicationGroup`: it tracks change detection
//! per entity per client, so the shared-group ack race that
//! `ReplicationGroup::new_from_entity()` used to work around (upstream #740)
//! cannot occur, and no per-entity group is needed.

use std::collections::HashSet;

use bevy::{log::info_span, prelude::*};
use lightyear::prelude::{
    LinkOf, NetworkTarget, Replicate, ReplicationSender, RoomAllocator, RoomId, Rooms,
};

use crate::{protocol::ClientId, world::ChunkCoord};

use super::routing::ServerConnections;
use super::{AuthoritativeServer, ChunkRoomMap, ClientChunkSubs, ClientPrivateRooms};

/// Attach room-gated replication to a freshly-spawned world entity (resource
/// node, dropped item, deployable, loot bag): `Replicate::to_clients(All)` plus
/// a [`Rooms`] component naming the entity's chunk room. The `Rooms` component
/// is itself the visibility filter, so senders in unrelated chunks receive
/// nothing.
pub(super) fn attach_room_gated_replication(world: &mut World, entity: Entity, chunk: ChunkCoord) {
    let room = ensure_chunk_room_world(world, chunk);
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert((
            Replicate::to_clients(NetworkTarget::All),
            Rooms::single(room),
        ));
    }
}

/// Move an already-replicated single-room entity to a different chunk room.
/// No-op when the coords are equal. Used by dropped items and players' public
/// entities whose anchor chunk can change after spawn (physics rollover,
/// footsteps). `Rooms` is an immutable component, so this re-inserts a fresh
/// single-room membership rather than mutating in place.
pub(super) fn move_entity_between_rooms(
    world: &mut World,
    entity: Entity,
    from: ChunkCoord,
    to: ChunkCoord,
) {
    if from == to {
        return;
    }
    let to_room = ensure_chunk_room_world(world, to);
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert(Rooms::single(to_room));
    }
}

/// Re-insert an entity's single-chunk [`Rooms`] membership WITHOUT changing its
/// chunk, re-firing lightyear's `on_insert` visibility observer against the
/// current sender subscriptions. The 0.28 room model latches per-client
/// visibility only when an entity's or a client's `Rooms` is (re)inserted and
/// never recomputes it per tick, so a settled entity that stops moving (a rested
/// arrow) would otherwise freeze its visibility at its last move with no recovery
/// path. Re-affirming for a short window after it settles lets a client whose
/// subscriptions settled a tick later still gain it. This restores the
/// self-healing the pre-0.28 per-tick room model had.
pub(super) fn reaffirm_entity_room(world: &mut World, entity: Entity, chunk: ChunkCoord) {
    let room = ensure_chunk_room_world(world, chunk);
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert(Rooms::single(room));
    }
}

/// Player replication. The player's **public** mirror entity carries the
/// peer-visible components (`PlayerProfile`/`PlayerPose`/... plus the cosmetic
/// rig state) and joins its chunk room, so every sender sharing that room sees
/// it. The player's **private** mirror entity (reached via
/// [`crate::server::PlayerPrivateLink`], holding inventory/crafting/containers/
/// input-ack) joins a per-client private room that only the owning sender ever
/// subscribes to (see [`update_client_room_subscriptions`]), so peers never
/// receive the private bytes.
pub(super) fn attach_player_replication(world: &mut World, entity: Entity, chunk: ChunkCoord) {
    let chunk_room = ensure_chunk_room_world(world, chunk);
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert((
            Replicate::to_clients(NetworkTarget::All),
            Rooms::single(chunk_room),
        ));
    }

    // Wire the private mirror entity into the owner's private room.
    let private_entity = world
        .get::<crate::server::PlayerPrivateLink>(entity)
        .map(|link| link.0);
    let client_id = world
        .get::<crate::server::Player>(entity)
        .map(|player| player.client_id);
    if let (Some(private_entity), Some(client_id)) = (private_entity, client_id) {
        let private_room = ensure_private_room(world, client_id);
        if let Ok(mut private_mut) = world.get_entity_mut(private_entity) {
            private_mut.insert((
                Replicate::to_clients(NetworkTarget::All),
                Rooms::single(private_room),
            ));
        }
    }
}

/// World-side lazy lookup: returns the [`RoomId`] for `chunk`, allocating one
/// from the [`RoomAllocator`] on first use. Server shutdown drops the world, so
/// no explicit cleanup is required.
pub(super) fn ensure_chunk_room_world(world: &mut World, chunk: ChunkCoord) -> RoomId {
    if let Some(id) = world
        .resource::<ChunkRoomMap>()
        .by_coord
        .get(&chunk)
        .copied()
    {
        return id;
    }
    let id = world.resource_mut::<RoomAllocator>().allocate();
    world
        .resource_mut::<ChunkRoomMap>()
        .by_coord
        .insert(chunk, id);
    id
}

/// Returns the per-client private [`RoomId`], allocating one on first use. The
/// owning sender joins this room in [`update_client_room_subscriptions`]; the
/// player's private mirror entity sits in it, so its owner-only components reach
/// that client alone.
fn ensure_private_room(world: &mut World, client_id: ClientId) -> RoomId {
    if let Some(id) = world
        .resource::<ClientPrivateRooms>()
        .by_client
        .get(&client_id)
        .copied()
    {
        return id;
    }
    let id = world.resource_mut::<RoomAllocator>().allocate();
    world
        .resource_mut::<ClientPrivateRooms>()
        .by_client
        .insert(client_id, id);
    id
}

/// Commands-side chunk-room lookup used by the per-tick subscription reconcile.
fn ensure_chunk_room_commands(
    chunk_rooms: &mut ChunkRoomMap,
    allocator: &mut RoomAllocator,
    chunk: ChunkCoord,
) -> RoomId {
    if let Some(id) = chunk_rooms.by_coord.get(&chunk).copied() {
        return id;
    }
    let id = allocator.allocate();
    chunk_rooms.by_coord.insert(chunk, id);
    id
}

/// Observer that fires when lightyear's link layer spawns a `LinkOf` entity (a
/// new pending or connected client). Adds the [`ReplicationSender`] lightyear
/// needs to actually ship per-component updates to that client. The sender's
/// [`Rooms`] membership is filled in by [`update_client_room_subscriptions`];
/// on disconnect the `LinkOf` entity is despawned, taking its `Rooms` with it.
pub(super) fn install_replication_sender_on_link(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

/// Reconciles each connected client's chunk-room subscriptions with their AoI
/// ring, using **spatial hysteresis** to stop boundary thrash. A chunk is
/// subscribed as soon as it enters the *add* radius (`visible_chunks_for_client`)
/// but only unsubscribed once it falls outside the wider *keep* radius
/// (`retained_chunks_for_client`). The gap between the two thresholds means a
/// player wobbling across a chunk boundary never crosses both, so nothing
/// loads/unloads/reloads.
///
/// Each reconcile rebuilds the sender's [`Rooms`] component (immutable, so a
/// full re-insert) from the accumulated chunk-room set plus the client's private
/// room. On disconnect the sender entity is despawned by lightyear; we just drop
/// our cached bookkeeping here.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(super) fn update_client_room_subscriptions(
    server: Res<AuthoritativeServer>,
    connections: Res<ServerConnections>,
    mut subs: ResMut<ClientChunkSubs>,
    mut anchors: ResMut<super::ClientAoiAnchors>,
    mut chunk_rooms: ResMut<ChunkRoomMap>,
    private_rooms: Res<ClientPrivateRooms>,
    mut allocator: ResMut<RoomAllocator>,
    mut commands: Commands,
) {
    let _span = info_span!("update_client_room_subscriptions").entered();
    let live_clients: HashSet<ClientId> = server.0.connected_client_ids().collect();
    subs.by_client.retain(|id, _| live_clients.contains(id));
    anchors.by_client.retain(|id, _| live_clients.contains(id));

    for client_id in live_clients {
        let Some(sender_entity) = connections.entity_for_client(client_id) else {
            // This client id is "in the world" but has no live sender right now,
            // the classic case is a sleeping body: the player logged out, their
            // body stays in the world (so the id remains in
            // `connected_client_ids`), but the transport, and thus the sender, is
            // gone. Drop the cached AoI anchor and subscribed-chunk set so that
            // when a sender reappears (the player reconnects and `wake_sleeper`
            // reuses this id with a brand-new sender) the next reconcile rebuilds
            // that new sender's `Rooms` from scratch.
            anchors.by_client.remove(&client_id);
            subs.by_client.remove(&client_id);
            continue;
        };
        // Spatial short-circuit: if the client's anchor chunk and view tier are
        // unchanged since last reconcile, the loaded-chunk grid being fixed means
        // the add/keep sets are identical to last time, so the sender's room set
        // hasn't changed. Skip the grid scan + re-insert entirely.
        let aoi_key = server.0.client_aoi_key(client_id);
        if let Some(key) = aoi_key
            && anchors.by_client.get(&client_id) == Some(&key)
        {
            continue;
        }
        let add_set: HashSet<ChunkCoord> = server.0.visible_chunks_for_client(client_id);
        let keep_set: HashSet<ChunkCoord> = server.0.retained_chunks_for_client(client_id);
        let subscribed = subs.by_client.entry(client_id).or_default();

        // Add chunks that entered the add radius, drop chunks that fell outside
        // the wider keep radius (add_set is within keep_set, so this is the
        // hysteresis band).
        subscribed.extend(add_set.iter().copied());
        subscribed.retain(|coord| keep_set.contains(coord));

        // Rebuild the sender's full room set: every subscribed chunk room plus
        // the client's own private room (so it keeps receiving its private
        // mirror entity regardless of where it stands).
        let mut rooms: Vec<RoomId> = subscribed
            .iter()
            .map(|coord| ensure_chunk_room_commands(&mut chunk_rooms, &mut allocator, *coord))
            .collect();
        if let Some(private) = private_rooms.by_client.get(&client_id).copied() {
            rooms.push(private);
        }
        commands
            .entity(sender_entity)
            .insert(Rooms::from(rooms.into_iter()));

        if let Some(key) = aoi_key {
            anchors.by_client.insert(client_id, key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::app::App;
    use lightyear::prelude::{RoomPlugin, server::ServerPlugins};

    use super::*;

    /// `attach_room_gated_replication` must give the spawned entity the
    /// `Replicate` marker and a `Rooms` component naming a freshly-allocated
    /// room for its chunk.
    ///
    /// Inserting `Replicate` fires lightyear component hooks that touch
    /// server-side replication resources, so a bare `World` panics. We stand up
    /// the same minimal plugin set the host uses (`ServerPlugins` + `RoomPlugin`,
    /// the latter providing `RoomAllocator`) so the hooks resolve.
    #[test]
    fn attach_room_gated_replication_adds_replicate_and_room() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Bevy 0.19: ServerPlugins calls init_state, which needs StatesPlugin.
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(ServerPlugins {
            tick_duration: Duration::from_secs_f32(1.0 / 60.0),
        });
        app.add_plugins(RoomPlugin);
        app.insert_resource(ChunkRoomMap::default());
        app.finish();
        app.cleanup();

        let world = app.world_mut();
        let entity = world.spawn_empty().id();
        let chunk = ChunkCoord::new(3, -1);

        attach_room_gated_replication(world, entity, chunk);
        world.flush();

        assert!(
            world.get::<Replicate>(entity).is_some(),
            "entity should carry the Replicate marker"
        );
        let room = world
            .resource::<ChunkRoomMap>()
            .by_coord
            .get(&chunk)
            .copied()
            .expect("a room should have been allocated for the chunk");
        let rooms = world
            .get::<Rooms>(entity)
            .expect("entity should carry a Rooms component");
        assert!(
            rooms.contains_room(room),
            "entity's Rooms should name its chunk room"
        );
    }
}
