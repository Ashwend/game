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

use anyhow::{Context, Result, bail};
use bevy::{app::TerminalCtrlCHandlerPlugin, log::info_span, prelude::*};
use lightyear::prelude::{
    LocalAddr, MessageSender,
    server::{self, ClientOf},
};

use crate::{
    protocol::{SERVER_TICK_RATE_HZ, ServerMessage},
    save::WorldSave,
    server::{GameServer, ServerSettings},
    steam::AuthMode,
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
    // Mirror of `GameServer::resource_nodes` into ECS entities. Phase 4
    // (Lightyear room replication) attaches `Replicate` to these entities;
    // the index lets the gather/admin paths find an entity in O(1) once
    // ownership is fully flipped in a later phase.
    app.insert_resource(crate::server::ResourceNodeIndex::default());
    app.insert_resource(crate::server::DroppedItemIndex::default());
    app.insert_resource(crate::server::DeployableIndex::default());
    app.insert_resource(crate::server::PlayerIndex::default());
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
                // mark it changed when the Vec actually differs.
                if let Some(mut storage) =
                    world.get_mut::<crate::server::ResourceNodeStorage>(entity)
                    && storage.0 != state.storage
                {
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
                crate::server::spawn_resource_node_entity(world, state, chunk);
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
                    *transform = new_transform;
                }
                if let Some(mut drop) = world.get_mut::<crate::server::DroppedItem>(entity)
                    && drop.stack != item.stack
                {
                    drop.stack = item.stack;
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
                crate::server::spawn_dropped_item_entity(world, item, chunk);
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
                    health.0 = view.health;
                }
                if let Some(mut active) = world.get_mut::<crate::server::DeployableActive>(entity)
                    && active.0 != view.active
                {
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
                crate::server::spawn_deployable_entity(world, view, chunk);
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
                    *public = view.public.clone();
                }
                // Refresh private — inventory/crafting change on user action.
                if let Some(mut private) = world.get_mut::<crate::server::PlayerPrivate>(entity)
                    && *private != view.private
                {
                    *private = view.private;
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
                crate::server::spawn_player_entity(world, view, chunk);
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
