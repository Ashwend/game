#[cfg(unix)]
mod admin;
mod handle;
mod mirror;
mod rooms;
mod routing;

use std::{
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use bevy::{app::TerminalCtrlCHandlerPlugin, log::info_span, prelude::*};
use lightyear::prelude::{
    LocalAddr, MessageSender, RoomPlugin,
    server::{self, ClientOf},
};

use crate::{
    auth::{AuthMode, WorkosVerifier},
    protocol::{ClientId, SERVER_TICK_RATE_HZ, ServerMessage},
    save::WorldSave,
    server::{GameServer, ServerSettings},
    world::ChunkCoord,
};

#[cfg(unix)]
use self::admin::{HostAdminSocket, drain_admin_socket};
pub(super) use self::handle::{GameServerHandle, SpawnedGameServer};
use self::{
    handle::HostCommand,
    mirror::{
        sync_deployable_entities, sync_dropped_item_entities, sync_loot_bag_entities,
        sync_player_entities, sync_projectile_entities, sync_resource_node_entities,
    },
    rooms::{install_replication_sender_on_link, update_client_room_subscriptions},
    routing::{
        ServerConnections, handle_disconnected_clients, receive_client_messages, route_envelopes,
    },
};
use super::channels::{
    LIGHTYEAR_PROTOCOL_ID, LightyearProtocolPlugin, PrivateKeyContext, private_key,
};

const HOST_SLEEP: Duration = Duration::from_millis(1);
/// How long [`spawn_loopback_server`] waits for the host thread to finish its
/// first update and report readiness. A healthy host comes up in well under
/// 100 ms, so this is purely a safety bound for a host that never starts. It is
/// generous on purpose: under coverage instrumentation (`cargo llvm-cov`) and on
/// loaded CI runners the instrumented host app builds its world several times
/// slower, and a tight bound here made the loopback integration tests flake
/// (the code is correct; only startup wall-clock is slow). A real never-starts
/// failure is still caught, just a few seconds later.
const HOST_START_TIMEOUT: Duration = Duration::from_secs(20);
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

/// Set true by [`tick_authoritative_server`] on any `app.update()` where the
/// fixed-step accumulator actually crossed a tick boundary, false otherwise.
///
/// The host loop sleeps [`HOST_SLEEP`] (1 ms) then calls `app.update()`, so the
/// `Update` schedule runs ~500-1000x/second, but authoritative state only
/// changes when a 20 Hz tick advances. The mirror-sync systems and the room
/// subscription system reconcile that authoritative state into the ECS, so they
/// only have work to do on updates where a tick advanced. Gating them on this
/// pulse turns ~98% of their passes (the pure no-op allocate/clone/compare
/// churn) into a single bool read. This is the host-loop analogue of the
/// "don't iterate every frame to discover nothing changed" rule in
/// docs/profiling.md.
#[derive(Resource, Default)]
struct ServerTickPulse {
    advanced: bool,
}

/// Run condition: only let the mirror/room systems run on updates where a tick
/// advanced (see [`ServerTickPulse`]).
fn server_tick_advanced(pulse: Res<ServerTickPulse>) -> bool {
    pulse.advanced
}

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

/// Per-client cache of the last AoI key (anchor chunk + view tier) the room
/// subscription system reconciled. The loaded-chunk grid is fixed after world
/// construction, so when a client's key is unchanged their add/keep chunk sets
/// are identical to last tick and the whole grid scan + set diff can be skipped.
/// A player wobbling within one chunk costs a single map lookup here.
#[derive(Resource, Default)]
struct ClientAoiAnchors {
    by_client: HashMap<ClientId, (ChunkCoord, crate::protocol::ViewRadiusTier)>,
}

/// Persists a snapshotted world during a periodic auto-save. Boxed so the
/// dedicated runner can close over its persistence target (a `WorldStore` or a
/// file path) without the host knowing which.
pub(crate) type AutoSaveWriter = Box<dyn Fn(&WorldSave) -> Result<()> + Send + Sync>;

/// Host-side disk-write target for periodic auto-saves. Present only on hosts
/// that persist while running (dedicated servers); loopback singleplayer omits
/// it and saves on exit instead. The `GameServer` schedules and announces the
/// save; this closure performs the actual write so I/O stays out of the
/// game-state module. `pub(crate)` so the dedicated runner can construct one.
#[derive(Resource)]
pub(crate) struct AutoSaveSink(pub(crate) AutoSaveWriter);

/// Full auto-save configuration handed to [`run_host`]: where to write
/// ([`AutoSaveSink`]), how often, and whether each routine save announces
/// itself. Dedicated hosts use a long interval that announces; the
/// singleplayer loopback host uses a short, silent interval. `None` disables
/// auto-save entirely (the world then persists only on a clean shutdown).
pub(crate) struct AutoSave {
    sink: AutoSaveSink,
    interval_ticks: u64,
    quiet: bool,
}

pub(super) fn spawn_loopback_server(
    save: WorldSave,
    settings: ServerSettings,
    auto_save: Option<AutoSaveSink>,
) -> Result<SpawnedGameServer> {
    let reserved_addr = reserve_udp_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .context("could not reserve loopback Lightyear server address")?;
    let addr = reserved_addr.addr();
    let (command_tx, command_rx) = mpsc::channel();
    let (startup_tx, startup_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("lightyear-game-server".to_owned())
        .spawn(move || {
            // The singleplayer loopback host auto-saves silently on a short
            // cadence so a crash loses at most one interval instead of the whole
            // session; the dedicated path uses the longer announced cadence.
            let auto_save = auto_save.map(|sink| AutoSave {
                sink,
                interval_ticks: crate::server::SINGLEPLAYER_AUTO_SAVE_INTERVAL_TICKS,
                quiet: true,
            });
            if let Err(error) = run_host(
                reserved_addr,
                save,
                settings,
                None,
                command_rx,
                None,
                false,
                Some(startup_tx.clone()),
                PrivateKeyContext::Loopback,
                auto_save,
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
    workos: Option<Arc<WorkosVerifier>>,
    admin_socket: Option<PathBuf>,
    auto_save: Option<AutoSaveSink>,
) -> Result<WorldSave> {
    let reserved_addr = reserve_udp_addr(bind_addr)
        .with_context(|| format!("could not reserve Lightyear server address {bind_addr}"))?;
    let bind_addr = reserved_addr.addr();
    let (_command_tx, command_rx) = mpsc::channel();
    println!("Lightyear game server listening on {bind_addr} ({auth_mode:?})");
    // Dedicated hosts auto-save on the long, announced cadence so every
    // connected player can brace for the brief write hitch.
    let auto_save = auto_save.map(|sink| AutoSave {
        sink,
        interval_ticks: crate::server::AUTO_SAVE_INTERVAL_TICKS,
        quiet: false,
    });
    run_host(
        reserved_addr,
        save,
        ServerSettings {
            auth_mode,
            singleplayer_host: None,
        },
        workos,
        command_rx,
        admin_socket,
        true,
        None,
        PrivateKeyContext::NetworkExposed,
        auto_save,
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
    workos: Option<Arc<WorkosVerifier>>,
    command_rx: mpsc::Receiver<HostCommand>,
    admin_socket: Option<PathBuf>,
    install_terminal_shutdown: bool,
    mut startup_tx: Option<mpsc::Sender<std::result::Result<(), String>>>,
    key_context: PrivateKeyContext,
    auto_save: Option<AutoSave>,
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
    app.insert_resource(ClientAoiAnchors::default());
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
    // Enable the auto-save schedule only when a write sink is present. The
    // caller picks the cadence and whether saves announce themselves: dedicated
    // hosts announce on a long interval, the singleplayer loopback host saves
    // silently on a short one. With no sink the `GameServer` keeps the interval
    // at 0 and never schedules a save (it persists on exit instead).
    let mut game_server = GameServer::new(save, settings).with_workos(workos);
    if let Some(auto_save) = auto_save {
        game_server = if auto_save.quiet {
            game_server.with_auto_save_silent(auto_save.interval_ticks)
        } else {
            game_server.with_auto_save(auto_save.interval_ticks)
        };
        app.insert_resource(auto_save.sink);
    }
    app.insert_resource(AuthoritativeServer(game_server));
    app.insert_resource(ServerConnections::default());
    app.insert_resource(TickAccumulator::default());
    app.insert_resource(ServerTickPulse::default());
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
    app.insert_resource(crate::server::ProjectileIndex::default());
    install_admin_socket(&mut app, admin_socket)?;

    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(server::Start {
            entity: server_entity,
        });
    });
    // The mirror-sync + room systems only have work when a tick advanced, so
    // they share a `server_tick_advanced` run condition. Everything before
    // `tick_authoritative_server` (command/admin drains, message receive,
    // disconnect handling) must run every update and stays ungated.
    let mirror_systems = (
        sync_resource_node_entities,
        sync_dropped_item_entities,
        sync_deployable_entities,
        sync_projectile_entities,
        sync_player_entities,
        sync_loot_bag_entities,
        update_client_room_subscriptions,
    )
        .chain()
        .run_if(server_tick_advanced);
    #[cfg(unix)]
    app.add_systems(
        Update,
        (
            drain_host_commands,
            drain_admin_socket,
            receive_client_messages,
            handle_disconnected_clients,
            tick_authoritative_server,
            mirror_systems,
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
            mirror_systems,
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

#[allow(clippy::too_many_arguments)]
fn tick_authoritative_server(
    mut commands: Commands,
    time: Res<Time>,
    mut accumulator: ResMut<TickAccumulator>,
    mut pulse: ResMut<ServerTickPulse>,
    mut server: ResMut<AuthoritativeServer>,
    mut connections: ResMut<ServerConnections>,
    mut senders: Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
    auto_save: Option<Res<AutoSaveSink>>,
) {
    let fixed_delta = Duration::from_secs_f32(1.0 / SERVER_TICK_RATE_HZ);
    let max_accumulator = fixed_delta.mul_f32(MAX_SERVER_TICKS_PER_LOOP);
    accumulator.0 = (accumulator.0 + time.delta()).min(max_accumulator);

    // Default to "no tick this update"; flip true below if the accumulator
    // crosses at least one fixed step. The gated mirror/room systems read this.
    pulse.advanced = accumulator.0 >= fixed_delta;

    while accumulator.0 >= fixed_delta {
        let _span = info_span!("host_fixed_tick").entered();
        let envelopes = server.0.tick(fixed_delta.as_secs_f32());
        let route_span = info_span!("route_envelopes", count = envelopes.len());
        route_span.in_scope(|| {
            route_envelopes(&mut commands, &mut connections, &mut senders, envelopes);
        });
        // The server flags a save when its schedule comes due (after emitting
        // the "Auto-saving..." line above). Perform the synchronous write here,
        // off the game-state module, then announce completion. The write is
        // intentionally inline: the brief hitch is what the heads-up warned of.
        if server.0.take_auto_save_pending() {
            let done = run_auto_save(&mut server.0, auto_save.as_deref());
            route_envelopes(&mut commands, &mut connections, &mut senders, done);
        }
        accumulator.0 -= fixed_delta;
    }
}

/// Snapshot + write the world for a due auto-save, returning the completion (or
/// failure) announcement envelopes for the host to route.
fn run_auto_save(
    server: &mut GameServer,
    sink: Option<&AutoSaveSink>,
) -> Vec<crate::server::ServerEnvelope> {
    let Some(sink) = sink else {
        return Vec::new();
    };
    let save = server.world_save();
    match (sink.0)(&save) {
        // A successful save only announces on hosts that opted into chatter
        // (dedicated); the silent singleplayer host stays quiet on success.
        Ok(()) if server.auto_save_announces() => server.announce("World saved."),
        Ok(()) => Vec::new(),
        // A failure is always surfaced, even in silent mode, so a player whose
        // disk is full learns their world is no longer being saved.
        Err(error) => {
            eprintln!("auto-save failed: {error:#}");
            server.announce("Auto-save failed, see server logs.")
        }
    }
}
