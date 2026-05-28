#[cfg(unix)]
mod admin;
mod handle;
mod routing;

use std::{
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{Mutex, mpsc},
    thread,
    time::Duration,
};

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use bevy::{app::TerminalCtrlCHandlerPlugin, log::info_span, prelude::*};
use lightyear::prelude::{
    ComponentReplicationOverrides, LinkOf, LocalAddr, MessageSender, NetworkTarget,
    NetworkVisibility, Replicate, ReplicationGroup, ReplicationSender, Room, RoomEvent, RoomPlugin,
    RoomTarget,
    server::{self, ClientOf},
};

use crate::{
    protocol::{ClientId, SERVER_TICK_RATE_HZ, ServerMessage},
    save::WorldSave,
    server::{GameServer, ServerSettings},
    steam::AuthMode,
    world::ChunkCoord,
};

#[cfg(unix)]
use self::admin::{HostAdminSocket, drain_admin_socket};
pub(super) use self::handle::{GameServerHandle, SpawnedGameServer};
use self::{
    handle::HostCommand,
    routing::{
        ServerConnections, handle_disconnected_clients, receive_client_messages, route_envelopes,
    },
};
use super::channels::{
    LIGHTYEAR_PROTOCOL_ID, LightyearProtocolPlugin, PrivateKeyContext, private_key,
};

const HOST_SLEEP: Duration = Duration::from_millis(1);
const HOST_START_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_SERVER_TICKS_PER_LOOP: f32 = 5.0;

#[derive(Debug)]
struct ReservedUdpAddr {
    addr: SocketAddr,
    socket: Option<UdpSocket>,
}

impl ReservedUdpAddr {
    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn release(&mut self) {
        self.socket.take();
    }
}

#[derive(Resource)]
pub(super) struct AuthoritativeServer(GameServer);

#[derive(Resource)]
struct HostCommandInbox(Mutex<mpsc::Receiver<HostCommand>>);

#[derive(Resource, Default)]
struct TickAccumulator(Duration);

#[derive(Resource, Default)]
struct HostShutdown {
    requested: bool,
}

/// Lazy `ChunkCoord -> RoomEntity` allocator. A `Room` is a regular Bevy
/// entity in Lightyear 0.26; we spawn one the first time we need to attach
/// a node or subscribe a client to that chunk, then cache it here. Server
/// shutdown drops the world entirely so no explicit cleanup is required.
#[derive(Resource, Default)]
struct ChunkRoomMap {
    by_coord: HashMap<ChunkCoord, Entity>,
}

/// Per-client snapshot of which chunk rooms the client currently has a sender
/// in. We diff the AoI add/keep radii against this every tick to emit the
/// minimal Add/RemoveSender RoomEvents. RoomPlugin's `handle_disconnect`
/// observer removes the sender from every room on `Disconnected`, so we only
/// need to drop our local bookkeeping there.
#[derive(Resource, Default)]
struct ClientChunkSubs {
    by_client: HashMap<ClientId, HashSet<ChunkCoord>>,
}

pub(super) fn spawn_loopback_server(
    save: WorldSave,
    settings: ServerSettings,
) -> Result<SpawnedGameServer> {
    let reserved_addr = reserve_udp_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .context("could not reserve loopback Lightyear server address")?;
    let addr = reserved_addr.addr();
    let (command_tx, command_rx) = mpsc::channel();
    let (startup_tx, startup_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("lightyear-game-server".to_owned())
        .spawn(move || {
            if let Err(error) = run_host(
                reserved_addr,
                save,
                settings,
                command_rx,
                None,
                false,
                Some(startup_tx.clone()),
                PrivateKeyContext::Loopback,
            ) {
                let _ = startup_tx.send(Err(format!("{error:#}")));
                eprintln!("Lightyear game server stopped: {error:#}");
            }
        })
        .context("could not spawn loopback Lightyear game server")?;

    match startup_rx.recv_timeout(HOST_START_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = thread.join();
            bail!("{error}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let (reply_tx, reply_rx) = mpsc::channel();
            let _ = command_tx.send(HostCommand::Shutdown(reply_tx));
            let _ = reply_rx.recv_timeout(HOST_START_TIMEOUT);
            let _ = thread.join();
            bail!("Lightyear game server did not start");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = thread.join();
            bail!("Lightyear game server stopped before startup");
        }
    }

    Ok(SpawnedGameServer {
        addr,
        handle: GameServerHandle::new(command_tx, thread),
    })
}

pub(super) fn run_game_server(
    bind_addr: SocketAddr,
    save: WorldSave,
    auth_mode: AuthMode,
    admin_socket: Option<PathBuf>,
) -> Result<WorldSave> {
    let reserved_addr = reserve_udp_addr(bind_addr)
        .with_context(|| format!("could not reserve Lightyear server address {bind_addr}"))?;
    let bind_addr = reserved_addr.addr();
    let (_command_tx, command_rx) = mpsc::channel();
    println!("Lightyear game server listening on {bind_addr} ({auth_mode:?})");
    run_host(
        reserved_addr,
        save,
        ServerSettings {
            auth_mode,
            singleplayer_host: None,
        },
        command_rx,
        admin_socket,
        true,
        None,
        PrivateKeyContext::NetworkExposed,
    )
}

fn reserve_udp_addr(addr: SocketAddr) -> Result<ReservedUdpAddr> {
    if addr.port() != 0 {
        return Ok(ReservedUdpAddr { addr, socket: None });
    }
    let socket = UdpSocket::bind(addr).with_context(|| format!("could not bind {addr}"))?;
    let addr = socket
        .local_addr()
        .context("could not read reserved UDP address")?;
    Ok(ReservedUdpAddr {
        addr,
        socket: Some(socket),
    })
}

#[cfg(unix)]
fn install_admin_socket(app: &mut App, admin_socket: Option<PathBuf>) -> Result<()> {
    if let Some(path) = admin_socket {
        app.insert_resource(HostAdminSocket::bind(path)?);
    }
    Ok(())
}

#[cfg(not(unix))]
fn install_admin_socket(_app: &mut App, admin_socket: Option<PathBuf>) -> Result<()> {
    if admin_socket.is_some() {
        bail!("dedicated server admin sockets require a Unix-like OS");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_host(
    mut reserved_addr: ReservedUdpAddr,
    save: WorldSave,
    settings: ServerSettings,
    command_rx: mpsc::Receiver<HostCommand>,
    admin_socket: Option<PathBuf>,
    install_terminal_shutdown: bool,
    mut startup_tx: Option<mpsc::Sender<std::result::Result<(), String>>>,
    key_context: PrivateKeyContext,
) -> Result<WorldSave> {
    let bind_addr = reserved_addr.addr();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    if install_terminal_shutdown {
        app.add_plugins(TerminalCtrlCHandlerPlugin);
    }
    app.add_plugins(server::ServerPlugins {
        tick_duration: Duration::from_secs_f32(1.0 / SERVER_TICK_RATE_HZ),
    });
    app.add_plugins(LightyearProtocolPlugin);
    // Phase 4: room-based interest management. Each `ChunkCoord` lazily
    // owns one Room entity; resource-node entities join their chunk's
    // room and client senders join the rooms covering their AoI ring.
    // Lightyear delta-ships components to senders in shared rooms and
    // auto-despawns on the client when rooms diverge.
    app.add_plugins(RoomPlugin);
    app.insert_resource(ChunkRoomMap::default());
    app.insert_resource(ClientChunkSubs::default());
    app.add_observer(install_replication_sender_on_link);

    let server_entity = app
        .world_mut()
        .spawn((
            Name::new("Lightyear Game Server"),
            LocalAddr(bind_addr),
            server::ServerUdpIo::default(),
            server::NetcodeServer::new(
                server::NetcodeConfig::default()
                    .with_protocol_id(LIGHTYEAR_PROTOCOL_ID)
                    .with_key(private_key(key_context)),
            ),
        ))
        .id();

    app.insert_resource(HostCommandInbox(Mutex::new(command_rx)));
    app.insert_resource(AuthoritativeServer(GameServer::new(save, settings)));
    app.insert_resource(ServerConnections::default());
    app.insert_resource(TickAccumulator::default());
    app.insert_resource(HostShutdown::default());
    // Per-id → ECS-entity lookup for each networked entity type. The
    // `HashMap`s on `GameServer` are still the authoritative store; the
    // mirror sync systems below reconcile them into ECS entities that
    // carry the Lightyear-replicated components, and these indexes let
    // gather/admin paths resolve `id → entity` in O(1).
    app.insert_resource(crate::server::ResourceNodeIndex::default());
    app.insert_resource(crate::server::DroppedItemIndex::default());
    app.insert_resource(crate::server::DeployableIndex::default());
    app.insert_resource(crate::server::PlayerIndex::default());
    app.insert_resource(crate::server::LootBagIndex::default());
    install_admin_socket(&mut app, admin_socket)?;

    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(server::Start {
            entity: server_entity,
        });
    });
    #[cfg(unix)]
    app.add_systems(
        Update,
        (
            drain_host_commands,
            drain_admin_socket,
            receive_client_messages,
            handle_disconnected_clients,
            tick_authoritative_server,
            sync_resource_node_entities,
            sync_dropped_item_entities,
            sync_deployable_entities,
            sync_player_entities,
            sync_loot_bag_entities,
            update_client_room_subscriptions,
        )
            .chain(),
    );
    #[cfg(not(unix))]
    app.add_systems(
        Update,
        (
            drain_host_commands,
            receive_client_messages,
            handle_disconnected_clients,
            tick_authoritative_server,
            sync_resource_node_entities,
            sync_dropped_item_entities,
            sync_deployable_entities,
            sync_player_entities,
            sync_loot_bag_entities,
            update_client_room_subscriptions,
        )
            .chain(),
    );
    app.finish();
    app.cleanup();

    reserved_addr.release();
    app.update();
    if let Some(startup_tx) = startup_tx.take() {
        let _ = startup_tx.send(Ok(()));
    }

    loop {
        if host_should_shutdown(&app) {
            return Ok(app.world().resource::<AuthoritativeServer>().0.world_save());
        }
        thread::sleep(HOST_SLEEP);
        app.update();
    }
}

fn host_should_shutdown(app: &App) -> bool {
    app.world().resource::<HostShutdown>().requested || app.should_exit().is_some()
}

fn drain_host_commands(
    inbox: Res<HostCommandInbox>,
    mut shutdown: ResMut<HostShutdown>,
    server: Res<AuthoritativeServer>,
) {
    let commands = {
        let Ok(receiver) = inbox.0.lock() else {
            shutdown.requested = true;
            return;
        };
        receiver.try_iter().collect::<Vec<_>>()
    };

    for command in commands {
        match command {
            HostCommand::WorldSave(reply_tx) => {
                let _ = reply_tx.send(server.0.world_save());
            }
            HostCommand::Shutdown(reply_tx) => {
                shutdown.requested = true;
                let _ = reply_tx.send(());
            }
        }
    }
}

/// Reconciles the live `GameServer::resource_nodes` map into ECS entities
/// once per Update. New ids spawn fresh entities; missing ids despawn the
/// tracked entity; surviving ids get their `ResourceNodeStorage` refreshed
/// in place so the per-component value tracks the authoritative HashMap.
///
/// Runs as an exclusive system because spawning / despawning needs
/// `&mut World`. Cheap in steady state (no allocations when the id set
/// is unchanged); the storage refresh writes are change-detected by Bevy
/// so they only emit `Changed` ticks when the inner Vec actually differs.
fn sync_resource_node_entities(world: &mut World) {
    let _span = info_span!("sync_resource_node_entities").entered();
    // Pull the authoritative state out as an owned snapshot so we can
    // release the borrow before mutating the world (spawn/despawn need
    // `&mut World` and would conflict with a live `Res<>` borrow).
    let authoritative: Vec<(
        crate::protocol::ResourceNodeId,
        crate::protocol::ResourceNodeState,
    )> = {
        let server = world.resource::<AuthoritativeServer>();
        server
            .0
            .resource_nodes_iter()
            .map(|(id, state)| (*id, state.clone()))
            .collect()
    };
    let authoritative_ids: std::collections::HashSet<crate::protocol::ResourceNodeId> =
        authoritative.iter().map(|(id, _)| *id).collect();

    // 1. Despawn entities whose node id is no longer in the live map.
    let stale: Vec<crate::protocol::ResourceNodeId> = {
        let index = world.resource::<crate::server::ResourceNodeIndex>();
        index
            .iter()
            .filter_map(|(id, _)| {
                if authoritative_ids.contains(&id) {
                    None
                } else {
                    Some(id)
                }
            })
            .collect()
    };
    for id in stale {
        crate::server::despawn_resource_node_entity(world, id);
    }

    // 2. Walk the live map; spawn fresh entities, update existing ones.
    for (id, state) in authoritative {
        let existing = world.resource::<crate::server::ResourceNodeIndex>().get(id);
        match existing {
            Some(entity) => {
                // Refresh storage in place. Change detection will only
                // mark it changed when the Vec actually differs — that's
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
                let chunk = world
                    .resource::<AuthoritativeServer>()
                    .0
                    .resource_node_chunk(id)
                    .unwrap_or_else(|| {
                        crate::world::ChunkCoord::from_world(state.position.x, state.position.z)
                    });
                let entity = crate::server::spawn_resource_node_entity(world, state, chunk);
                attach_room_gated_replication(world, entity, chunk);
            }
        }
    }
}

/// Attach the room-gated replication marker to a freshly-spawned
/// world-entity (resource node, dropped item, deployable). Adds
/// `Replicate::to_clients(NetworkTarget::All) + NetworkVisibility +
/// ReplicationGroup::new_from_entity()` and then joins the chunk's room.
/// `NetworkVisibility` narrows the `All` target down to the senders
/// currently in a shared room with the entity — without it, every
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
/// when the sender isn't already in the entity's targets — that path
/// admits the sender for the spawn message but apparently does not
/// register the sender for the subsequent change-detection update
/// pipeline. Listing the sender in the `Replicate` target up front
/// avoids that ambiguity; `NetworkVisibility` still gates the actual
/// visibility per the room state, so peers in unrelated chunks
/// receive nothing.
fn attach_room_gated_replication(world: &mut World, entity: Entity, chunk: ChunkCoord) {
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
fn move_entity_between_rooms(world: &mut World, entity: Entity, from: ChunkCoord, to: ChunkCoord) {
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
fn attach_player_replication(
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
fn ensure_chunk_room_world(world: &mut World, chunk: ChunkCoord) -> Entity {
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
/// `Disconnected` tear-down for us — `RoomPlugin::handle_disconnect`
/// removes the sender from all rooms automatically.
fn install_replication_sender_on_link(trigger: On<Add, LinkOf>, mut commands: Commands) {
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
fn update_client_room_subscriptions(
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

/// Reconciles `GameServer::dropped_items` into ECS entities. Same shape
/// as `sync_resource_node_entities`: despawn ids that left the live map,
/// spawn fresh entities for new ids, refresh transform + stack in place
/// for surviving ids. Stack writes are change-detected so the
/// `Changed<DroppedItem>` signal only fires on real merges.
fn sync_dropped_item_entities(world: &mut World) {
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
fn sync_deployable_entities(world: &mut World) {
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
/// targets — `NetworkTarget::All` for public, `Single(client_id)` for
/// private.
fn sync_player_entities(world: &mut World) {
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
                // Refresh public — position/velocity tick every frame.
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
                // Refresh private — inventory/crafting change on user action.
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
                // Refresh armor. Today only mutated by future systems —
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

/// Reconciles `GameServer::loot_bags` into ECS entities. Sync per
/// tick covers two diffable fields:
///   - `LootBagContents`: changes when a player drags items in/out.
///   - `LootBagTransform`: changes while the spawn-time gravity
///     settle is still in flight (the bag falls from chest height
///     to the ground over ~0.4 s). Without refreshing the
///     replicated transform the client would see the bag frozen at
///     its spawn position.
fn sync_loot_bag_entities(world: &mut World) {
    let _span = info_span!("sync_loot_bag_entities").entered();
    let authoritative: Vec<crate::server::LootBagView> = {
        let server = world.resource::<AuthoritativeServer>();
        server
            .0
            .loot_bags_iter()
            .map(|(id, bag)| crate::server::LootBagView {
                id,
                position: bag.position,
                yaw: bag.yaw,
                slots: bag.slots.clone(),
            })
            .collect()
    };
    let live_ids: std::collections::HashSet<crate::protocol::LootBagId> =
        authoritative.iter().map(|view| view.id).collect();

    let stale: Vec<crate::protocol::LootBagId> = {
        let index = world.resource::<crate::server::LootBagIndex>();
        index
            .iter()
            .filter_map(|(id, _)| (!live_ids.contains(&id)).then_some(id))
            .collect()
    };
    for id in stale {
        crate::server::despawn_loot_bag_entity(world, id);
    }

    for view in authoritative {
        let existing = world.resource::<crate::server::LootBagIndex>().get(view.id);
        match existing {
            Some(entity) => {
                if let Some(mut contents) = world.get_mut::<crate::server::LootBagContents>(entity)
                    && contents.0 != view.slots
                {
                    #[cfg(feature = "replication-trace")]
                    {
                        let before: usize = contents.0.iter().filter(|s| s.is_some()).count();
                        let after: usize = view.slots.iter().filter(|s| s.is_some()).count();
                        info!(
                            target: "replication_trace",
                            "server: LootBagContents    MUTATE id={} entity={entity:?} occupied {before} -> {after}",
                            view.id
                        );
                    }
                    contents.0 = view.slots;
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
            }
        }
    }
}

fn tick_authoritative_server(
    mut commands: Commands,
    time: Res<Time>,
    mut accumulator: ResMut<TickAccumulator>,
    mut server: ResMut<AuthoritativeServer>,
    mut connections: ResMut<ServerConnections>,
    mut senders: Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) {
    let fixed_delta = Duration::from_secs_f32(1.0 / SERVER_TICK_RATE_HZ);
    let max_accumulator = fixed_delta.mul_f32(MAX_SERVER_TICKS_PER_LOOP);
    accumulator.0 = (accumulator.0 + time.delta()).min(max_accumulator);

    while accumulator.0 >= fixed_delta {
        let _span = info_span!("host_fixed_tick").entered();
        let envelopes = server.0.tick(fixed_delta.as_secs_f32());
        let route_span = info_span!("route_envelopes", count = envelopes.len());
        route_span.in_scope(|| {
            route_envelopes(&mut commands, &mut connections, &mut senders, envelopes);
        });
        accumulator.0 -= fixed_delta;
    }
}
