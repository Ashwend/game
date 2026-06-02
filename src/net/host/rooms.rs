//! Chunk-room / AoI replication helpers for the host.
//!
//! Each `ChunkCoord` lazily owns one Lightyear `Room` entity. Networked
//! entities (resource nodes, dropped items, deployables, players, loot bags)
//! join their chunk's room, and client senders join the rooms covering their
//! AoI ring. Lightyear delta-ships components to senders that share a room
//! with an entity and auto-despawns on the client when the rooms diverge.

use std::collections::HashSet;

use bevy::{log::info_span, prelude::*};
use lightyear::prelude::{
    ComponentReplicationOverrides, LinkOf, NetworkTarget, NetworkVisibility, Replicate,
    ReplicationGroup, ReplicationSender, Room, RoomEvent, RoomTarget,
};

use crate::{protocol::ClientId, world::ChunkCoord};

use super::routing::ServerConnections;
use super::{AuthoritativeServer, ChunkRoomMap, ClientChunkSubs};

/// Attach the room-gated replication marker to a freshly-spawned
/// world-entity (resource node, dropped item, deployable). Adds
/// `Replicate::to_clients(NetworkTarget::All) + NetworkVisibility +
/// ReplicationGroup::new_from_entity()` and then joins the chunk's room.
/// `NetworkVisibility` narrows the `All` target down to the senders
/// currently in a shared room with the entity, without it, every
/// client would see every node.
///
/// `ReplicationGroup::new_from_entity()` is the fix for the upstream
/// Lightyear 0.26.4 post-spawn-diff dropout bug. By default Lightyear
/// puts every replicated entity in `DEFAULT_GROUP = ReplicationGroupId(0)`
/// and gates change-detection sends on a per-group ack tick, so a
/// frequently-updated entity in the group can advance the shared ack
/// past a slowly-changing entity's local `Changed` mark and Lightyear
/// concludes "nothing new to send" for the slow entity even though it
/// just changed. Giving each entity its own group (derived from
/// `Entity::to_bits()`) means each entity has its own ack tick and the
/// share-the-tick race goes away. See [Networking § Replication](../../docs/networking.md#replication).
///
/// `NetworkTarget::All` (not `None`) is load-bearing: the Phase 6a
/// diagnostic showed Lightyear shipping the initial spawn but not
/// subsequent component updates with `None + room`. The room machinery
/// uses `gain_visibility` which inserts a fresh `PerSenderReplicationState`
/// when the sender isn't already in the entity's targets, that path
/// admits the sender for the spawn message but apparently does not
/// register the sender for the subsequent change-detection update
/// pipeline. Listing the sender in the `Replicate` target up front
/// avoids that ambiguity; `NetworkVisibility` still gates the actual
/// visibility per the room state, so peers in unrelated chunks
/// receive nothing.
pub(super) fn attach_room_gated_replication(world: &mut World, entity: Entity, chunk: ChunkCoord) {
    let room_entity = ensure_chunk_room_world(world, chunk);
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert((
            Replicate::to_clients(NetworkTarget::All),
            ReplicationGroup::new_from_entity(),
            NetworkVisibility,
        ));
    }
    world.trigger(RoomEvent {
        room: room_entity,
        target: RoomTarget::AddEntity(entity),
    });
}

/// Move an already-replicated entity between two chunk rooms. No-op when
/// the coords are equal. Used by dropped items and players whose anchor
/// chunk can change after spawn (physics rollover, footsteps).
pub(super) fn move_entity_between_rooms(
    world: &mut World,
    entity: Entity,
    from: ChunkCoord,
    to: ChunkCoord,
) {
    if from == to {
        return;
    }
    let from_room = world
        .resource::<ChunkRoomMap>()
        .by_coord
        .get(&from)
        .copied();
    let to_room = ensure_chunk_room_world(world, to);
    if let Some(from_room) = from_room {
        world.trigger(RoomEvent {
            room: from_room,
            target: RoomTarget::RemoveEntity(entity),
        });
    }
    world.trigger(RoomEvent {
        room: to_room,
        target: RoomTarget::AddEntity(entity),
    });
}

/// Phase 5 player replication: broadcast `PlayerPublic` to every sender
/// in the same room (peer-visible), and gate `PlayerPrivate` behind a
/// per-component override so only the owning client receives the
/// inventory/crafting wire bytes. The owner's prediction supplies their
/// own `PlayerPublic` locally, so them re-receiving it is a small,
/// acceptable redundancy.
pub(super) fn attach_player_replication(
    world: &mut World,
    entity: Entity,
    chunk: ChunkCoord,
    owner_sender: Option<Entity>,
) {
    let room_entity = ensure_chunk_room_world(world, chunk);
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        // See `attach_room_gated_replication` for why
        // `ReplicationGroup::new_from_entity()` is load-bearing.
        entity_mut.insert((
            Replicate::to_clients(NetworkTarget::All),
            ReplicationGroup::new_from_entity(),
            NetworkVisibility,
        ));
        let mut overrides =
            ComponentReplicationOverrides::<crate::server::PlayerPrivate>::default().disable_all();
        if let Some(sender) = owner_sender {
            overrides = overrides.enable_for(sender);
        }
        entity_mut.insert(overrides);
    }
    world.trigger(RoomEvent {
        room: room_entity,
        target: RoomTarget::AddEntity(entity),
    });
}

/// World-side lazy lookup: returns the Room entity for `chunk`, spawning
/// one if it does not yet exist. The mirror sync system uses this; the
/// per-tick subscription update uses the Commands-side
/// `ensure_chunk_room_commands` instead so it can defer the spawn.
pub(super) fn ensure_chunk_room_world(world: &mut World, chunk: ChunkCoord) -> Entity {
    if let Some(entity) = world
        .resource::<ChunkRoomMap>()
        .by_coord
        .get(&chunk)
        .copied()
    {
        return entity;
    }
    let entity = world
        .spawn((
            Name::new(format!("Chunk Room {}/{}", chunk.x, chunk.z)),
            Room::default(),
        ))
        .id();
    world
        .resource_mut::<ChunkRoomMap>()
        .by_coord
        .insert(chunk, entity);
    entity
}

fn ensure_chunk_room_commands(
    commands: &mut Commands,
    rooms: &mut ChunkRoomMap,
    chunk: ChunkCoord,
) -> Entity {
    if let Some(entity) = rooms.by_coord.get(&chunk).copied() {
        return entity;
    }
    let entity = commands
        .spawn((
            Name::new(format!("Chunk Room {}/{}", chunk.x, chunk.z)),
            Room::default(),
        ))
        .id();
    rooms.by_coord.insert(chunk, entity);
    entity
}

/// Observer that fires when Lightyear's link layer spawns a `LinkOf`
/// entity (a new pending or connected client). Adds the
/// `ReplicationSender` Lightyear needs to actually ship per-component
/// updates to that client. Connection plugins handle the
/// `Disconnected` tear-down for us, `RoomPlugin::handle_disconnect`
/// removes the sender from all rooms automatically.
pub(super) fn install_replication_sender_on_link(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert(ReplicationSender::default());
}

/// Reconciles each connected client's chunk-room subscriptions with their AoI
/// ring, using **spatial hysteresis** to stop boundary thrash. A chunk is
/// subscribed as soon as it enters the *add* radius (`visible_chunks_for_client`)
/// but only unsubscribed once it falls outside the wider *keep* radius
/// (`retained_chunks_for_client`). The gap between the two thresholds means a
/// player wobbling across a chunk boundary never crosses both, so nothing
/// loads/unloads/reloads. On disconnect, RoomPlugin scrubs the sender from
/// every room; we just drop our cached set.
pub(super) fn update_client_room_subscriptions(
    server: Res<AuthoritativeServer>,
    connections: Res<ServerConnections>,
    mut subs: ResMut<ClientChunkSubs>,
    mut chunk_rooms: ResMut<ChunkRoomMap>,
    mut commands: Commands,
) {
    let _span = info_span!("update_client_room_subscriptions").entered();
    let live_clients: HashSet<ClientId> = server.0.connected_client_ids().collect();
    subs.by_client.retain(|id, _| live_clients.contains(id));

    for client_id in live_clients {
        let Some(sender_entity) = connections.entity_for_client(client_id) else {
            continue;
        };
        let add_set: HashSet<ChunkCoord> = server.0.visible_chunks_for_client(client_id);
        let keep_set: HashSet<ChunkCoord> = server.0.retained_chunks_for_client(client_id);
        let subscribed = subs.by_client.entry(client_id).or_default();

        // Subscribe chunks that entered the add radius.
        for coord in &add_set {
            if subscribed.insert(*coord) {
                let room = ensure_chunk_room_commands(&mut commands, &mut chunk_rooms, *coord);
                commands.trigger(RoomEvent {
                    room,
                    target: RoomTarget::AddSender(sender_entity),
                });
            }
        }

        // Unsubscribe chunks that fell outside the wider keep radius.
        let to_remove: Vec<ChunkCoord> = subscribed.difference(&keep_set).copied().collect();
        for coord in to_remove {
            subscribed.remove(&coord);
            if let Some(room) = chunk_rooms.by_coord.get(&coord).copied() {
                commands.trigger(RoomEvent {
                    room,
                    target: RoomTarget::RemoveSender(sender_entity),
                });
            }
        }
    }
}
