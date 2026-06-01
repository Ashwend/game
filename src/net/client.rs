use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use bevy::prelude::*;
use lightyear::prelude::{
    Authentication, Connected, LocalAddr, MessageReceiver, MessageSender, ReplicationReceiver,
    UdpIo, client,
};

use crate::{
    net::{
        channels::{LIGHTYEAR_PROTOCOL_ID, PrivateKeyContext, private_key, send_client_message},
        host::{GameServerHandle, spawn_loopback_server},
    },
    protocol::{ClientMessage, GAME_VERSION, PROTOCOL_VERSION, SERVER_TICK_RATE_HZ, ServerMessage},
    save::{WorldSave, WorldStore},
    server::ServerSettings,
    steam::{AuthMode, AuthenticatedUser},
};

const CLIENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum wall-clock time the background shutdown thread will wait for the
/// main app's graceful disconnect to complete. The actual disconnect is
/// bounded by `USER_DISCONNECT_FLUSH_TICKS + NETCODE_DISCONNECT_FLUSH_TICKS`
/// frames at whatever the main app's frame rate is; 5 seconds is comfortably
/// above any realistic frame-time even if the main app is briefly stalled.
const SHUTDOWN_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Update ticks to wait between queuing the app-level `Disconnect` message
/// and triggering the netcode `Disconnect`. The reliable channel needs one
/// PostUpdate to write the message into the UDP socket; the netcode layer
/// stops accepting user packets the moment we call its `disconnect()`, so
/// we must drain the user message first.
const USER_DISCONNECT_FLUSH_TICKS: u32 = 2;

/// Update ticks to wait after triggering the netcode `Disconnect` so the
/// 10 redundant DISCONNECT packets can be drained from netcode's internal
/// send queue into the UDP transport.
const NETCODE_DISCONNECT_FLUSH_TICKS: u32 = 4;

/// Lightyear client lifecycle as observed by the main app. The UI polls
/// this to decide when a singleplayer or direct-connect attempt has
/// actually finished its handshake and is safe to drive gameplay against.
/// Some variants are read only by the systems that write them today;
/// they're part of the public state surface so allow the dead-code lint.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ClientConnectionStatus {
    #[default]
    Idle,
    Connecting,
    Connected,
    Disconnected(String),
}

/// Shared state between [`ClientSession`] (held by the runtime / worker
/// threads) and the Bevy systems that drive Lightyear inside the main app.
/// Everything is wrapped in `Arc` so a `ClientSession` can be moved into a
/// background shutdown thread while the main app keeps reading and writing
/// the same queues.
#[derive(Resource, Clone, Default)]
pub(crate) struct ClientNetwork(Arc<ClientNetworkInner>);

#[derive(Default)]
struct ClientNetworkInner {
    /// Messages produced by gameplay code waiting to be forwarded to
    /// Lightyear. The first entry after each `pending_connect` is the
    /// `ClientMessage::Auth` handshake — it is only drained once the
    /// server-side handshake completes (`Connected` is observed).
    outbox: Mutex<VecDeque<ClientMessage>>,
    /// Server-originated messages received by Lightyear, awaiting drain
    /// from `network_tick_system` via `ClientSession::tick`.
    inbox: Mutex<VecDeque<ServerMessage>>,
    /// Public connection state used by the UI to decide when to flip from
    /// the loading splash into `Screen::InGame`.
    status: Mutex<ClientConnectionStatus>,
    /// Connection request set by `ClientSession::connect_inner`; consumed
    /// by `process_pending_connect_system` on its next tick.
    pending_connect: Mutex<Option<PendingConnect>>,
    /// Set by `ClientSession::shutdown` to start the graceful disconnect.
    shutdown_request: AtomicBool,
    /// Flipped by `drive_shutdown_system` once the netcode disconnect has
    /// had a chance to flush. The shutdown worker thread polls this.
    shutdown_complete: AtomicBool,
}

#[derive(Debug)]
struct PendingConnect {
    server_addr: SocketAddr,
    /// Transport-level netcode connection id. Deliberately a fresh random
    /// nonce per attempt, NOT the player's `steam_id`: netcode refuses a
    /// connection whose client id is already in its table (`ClientIdInUse`)
    /// and only releases the slot after its ~10s client timeout, so reusing a
    /// stable id made quick reconnects fail until the old link timed out. The
    /// real identity rides in `auth_message` and the server keys players off
    /// that, so a per-connection nonce here is purely a transport detail.
    netcode_client_id: u64,
    auth_message: ClientMessage,
    key_context: PrivateKeyContext,
}

/// Bevy plugin that wires the Lightyear client into the main app: registers
/// the shared `ClientNetwork` resource and the systems that drive the
/// connection lifecycle. Pairs with [`LightyearProtocolPlugin`] which
/// installs Lightyear's own channels and message types.
pub(crate) struct ClientNetworkPlugin;

impl Plugin for ClientNetworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientNetwork>()
            .init_resource::<ClientHeartbeat>()
            .init_resource::<ClientShutdownState>()
            .add_systems(
                Update,
                (
                    process_pending_connect_system,
                    send_client_messages_system,
                    receive_server_messages_system,
                    report_client_disconnect_system,
                    drive_shutdown_system,
                )
                    .chain(),
            );
    }
}

/// Returns the `client::ClientPlugins` configured for our protocol tick
/// rate. Kept as a free function so `app.rs` doesn't need to know the tick
/// rate constant directly.
pub(crate) fn client_plugins() -> client::ClientPlugins {
    client::ClientPlugins {
        tick_duration: Duration::from_secs_f32(1.0 / SERVER_TICK_RATE_HZ),
    }
}

/// Thin handle stored in `ClientRuntime::session`. Sending and ticking the
/// network now both route through the main app's `ClientNetwork`; this
/// struct just keeps the loopback server alive (for singleplayer) and
/// signals shutdown.
pub struct ClientSession {
    network: ClientNetwork,
    local_server: Mutex<Option<GameServerHandle>>,
}

impl std::fmt::Debug for ClientSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let local_server_running = self.local_server.lock().is_ok_and(|guard| guard.is_some());
        formatter
            .debug_struct("ClientSession")
            .field("local_server", &local_server_running)
            .finish_non_exhaustive()
    }
}

impl ClientSession {
    pub(crate) fn start_singleplayer(
        save: WorldSave,
        user: &AuthenticatedUser,
        network: ClientNetwork,
    ) -> Result<Self> {
        // The loopback host runs in Offline mode and trusts the local player.
        // A signed-in player carries a WorkOS access-token JWT, which Offline
        // mode would reject (and re-validating it over the network would break
        // offline singleplayer) — so present the matching offline token for
        // this account id instead. Multiplayer keeps the real access token.
        let local_user = AuthenticatedUser {
            steam_id: user.steam_id,
            display_name: user.display_name.clone(),
            token: crate::steam::offline_auth_token(user.steam_id),
        };
        let spawned = spawn_loopback_server(
            save,
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(user.steam_id),
            },
        )?;
        Self::connect_inner(spawned.addr, &local_user, Some(spawned.handle), network)
    }

    pub(crate) fn connect(
        addr: SocketAddr,
        user: &AuthenticatedUser,
        network: ClientNetwork,
    ) -> Result<Self> {
        Self::connect_inner(addr, user, None, network)
    }

    fn connect_inner(
        addr: SocketAddr,
        user: &AuthenticatedUser,
        local_server: Option<GameServerHandle>,
        network: ClientNetwork,
    ) -> Result<Self> {
        let auth_message = ClientMessage::Auth {
            protocol_version: PROTOCOL_VERSION,
            client_version: Some(GAME_VERSION.to_owned()),
            steam_id: user.steam_id,
            display_name: user.display_name.clone(),
            token: user.token.clone(),
        };
        // Singleplayer pairs the client with a loopback server we just spun
        // up — both sides know the (default) key and the link doesn't leave
        // the box. Remote connections need real key material if the operator
        // sets `LIGHTYEAR_PRIVATE_KEY`, so they get the warning instead.
        let key_context = if local_server.is_some() {
            PrivateKeyContext::Loopback
        } else {
            PrivateKeyContext::NetworkExposed
        };
        network.0.shutdown_request.store(false, Ordering::Release);
        network.0.shutdown_complete.store(false, Ordering::Release);
        {
            let mut outbox = network
                .0
                .outbox
                .lock()
                .map_err(|_| anyhow::anyhow!("client outbox lock is poisoned"))?;
            outbox.clear();
        }
        {
            let mut inbox = network
                .0
                .inbox
                .lock()
                .map_err(|_| anyhow::anyhow!("client inbox lock is poisoned"))?;
            inbox.clear();
        }
        if let Ok(mut status) = network.0.status.lock() {
            *status = ClientConnectionStatus::Connecting;
        }
        {
            let mut pending = network
                .0
                .pending_connect
                .lock()
                .map_err(|_| anyhow::anyhow!("client pending-connect lock is poisoned"))?;
            *pending = Some(PendingConnect {
                server_addr: addr,
                netcode_client_id: uuid::Uuid::new_v4().as_u64_pair().0,
                auth_message,
                key_context,
            });
        }
        Ok(Self {
            network,
            local_server: Mutex::new(local_server),
        })
    }

    pub fn send(&mut self, message: ClientMessage) -> Result<()> {
        let mut outbox = self
            .network
            .0
            .outbox
            .lock()
            .map_err(|_| anyhow::anyhow!("client outbox lock is poisoned"))?;
        outbox.push_back(message);
        Ok(())
    }

    pub fn tick(&mut self, _delta_seconds: f32) -> Result<Vec<ServerMessage>> {
        let mut inbox = self
            .network
            .0
            .inbox
            .lock()
            .map_err(|_| anyhow::anyhow!("client inbox lock is poisoned"))?;
        Ok(inbox.drain(..).collect())
    }

    pub fn shutdown(&mut self, store: &WorldStore) -> Result<()> {
        let world_save = {
            let local_server = self
                .local_server
                .lock()
                .map_err(|_| anyhow::anyhow!("game server handle lock is poisoned"))?;
            local_server
                .as_ref()
                .map(GameServerHandle::world_save)
                .transpose()?
        };
        if let Some(world_save) = world_save {
            store.save_world(&world_save)?;
        }

        self.shutdown_transport()
    }

    fn shutdown_transport(&mut self) -> Result<()> {
        self.network
            .0
            .shutdown_request
            .store(true, Ordering::Release);

        // Poll for the main app's graceful disconnect. Sleeping is fine —
        // this runs on the dedicated `game-session-shutdown` worker, not
        // the main thread.
        let deadline = Instant::now() + SHUTDOWN_WAIT_TIMEOUT;
        while !self.network.0.shutdown_complete.load(Ordering::Acquire) && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }

        let mut local_server = self
            .local_server
            .lock()
            .map_err(|_| anyhow::anyhow!("game server handle lock is poisoned"))?;
        if let Some(mut handle) = local_server.take() {
            handle.shutdown()?;
        }
        Ok(())
    }
}

impl Drop for ClientSession {
    fn drop(&mut self) {
        let _ = self.shutdown_transport();
    }
}

/// Per-app heartbeat ticker. Lives outside `ClientNetwork` so the bg
/// shutdown worker doesn't have to touch it.
#[derive(Resource, Default)]
struct ClientHeartbeat {
    elapsed: Duration,
}

impl ClientHeartbeat {
    fn tick(&mut self, delta: Duration) -> bool {
        self.elapsed += delta;
        if self.elapsed < CLIENT_HEARTBEAT_INTERVAL {
            return false;
        }
        self.elapsed = Duration::ZERO;
        true
    }

    /// Reset the silence timer because a real user message went out this
    /// frame — no heartbeat is needed until we go quiet for a full interval
    /// again.
    fn note_traffic(&mut self) {
        self.elapsed = Duration::ZERO;
    }
}

/// Internal state for the graceful disconnect state machine. Mirrors the
/// shape of the old `ClientShutdown` resource in the standalone client App,
/// but tracks the additional "auth has been sent at least once" flag so we
/// can skip the user-disconnect flush when we never got past handshake.
#[derive(Resource, Default)]
struct ClientShutdownState {
    in_progress: bool,
    ticks_in_phase: u32,
    netcode_disconnect_issued: bool,
}

/// Per-session bookkeeping kept on the Lightyear client entity. Records
/// whether the auth handshake has been sent yet so the send pump only
/// forwards it once per session.
#[derive(Component, Default)]
struct ClientAuthState {
    sent: bool,
}

fn process_pending_connect_system(
    mut commands: Commands,
    network: Res<ClientNetwork>,
    mut shutdown: ResMut<ClientShutdownState>,
    existing: Query<Entity, With<client::Client>>,
) {
    let pending = {
        let Ok(mut guard) = network.0.pending_connect.lock() else {
            return;
        };
        guard.take()
    };
    let Some(pending) = pending else {
        return;
    };

    // Tear down any prior session entity so a quick reconnect doesn't
    // pile up Lightyear clients.
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    *shutdown = ClientShutdownState::default();

    let netcode = client::NetcodeClient::new(
        Authentication::Manual {
            server_addr: pending.server_addr,
            client_id: pending.netcode_client_id,
            private_key: private_key(pending.key_context),
            protocol_id: LIGHTYEAR_PROTOCOL_ID,
        },
        client::NetcodeConfig::default(),
    );
    let netcode = match netcode {
        Ok(netcode) => netcode,
        Err(error) => {
            warn!("could not create Lightyear netcode client: {error}");
            if let Ok(mut status) = network.0.status.lock() {
                *status = ClientConnectionStatus::Disconnected(format!(
                    "could not create Lightyear netcode client: {error}"
                ));
            }
            return;
        }
    };

    {
        let Ok(mut outbox) = network.0.outbox.lock() else {
            return;
        };
        outbox.push_front(pending.auth_message);
    }

    let entity = commands
        .spawn((
            Name::new("Lightyear Game Client"),
            LocalAddr(SocketAddr::from(([0, 0, 0, 0], 0))),
            UdpIo::default(),
            netcode,
            // Phase 4: needed for Lightyear's replication machinery to
            // buffer and apply incoming entity/component diffs. The
            // sibling `ReplicationSender` is installed on the server's
            // `ClientOf` entity inside the host app.
            ReplicationReceiver::default(),
            ClientAuthState::default(),
        ))
        .id();
    commands.trigger(client::Connect { entity });
}

#[allow(clippy::type_complexity)]
fn send_client_messages_system(
    time: Res<Time>,
    network: Res<ClientNetwork>,
    mut heartbeat: ResMut<ClientHeartbeat>,
    mut clients: Query<
        (
            &mut MessageSender<ClientMessage>,
            &mut ClientAuthState,
            Has<Connected>,
        ),
        With<client::Client>,
    >,
) {
    let mut clients_iter = clients.iter_mut();
    let Some((mut sender, mut auth, connected)) = clients_iter.next() else {
        return;
    };
    if !connected {
        return;
    }

    // Pull out everything queued so we can decide heartbeat behaviour
    // before sending. Auth always rides at the head once we observe a
    // `Connected` client; this matches the old "ClientAuth resource"
    // sequence, and keeps reconnect deterministic.
    let messages: Vec<ClientMessage> = {
        let Ok(mut outbox) = network.0.outbox.lock() else {
            return;
        };
        outbox.drain(..).collect()
    };

    let sent_real_message = messages
        .iter()
        .any(|message| !matches!(message, ClientMessage::Heartbeat));

    for message in messages {
        if !auth.sent {
            // The auth message is pushed to the front of the outbox by
            // `process_pending_connect_system`. Just track that we have
            // sent at least one frame to the wire so the heartbeat
            // accounting kicks in.
            auth.sent = true;
        }
        send_client_message(&mut sender, message);
    }

    if sent_real_message {
        heartbeat.note_traffic();
    } else if auth.sent && heartbeat.tick(time.delta()) {
        send_client_message(&mut sender, ClientMessage::Heartbeat);
    }
}

fn receive_server_messages_system(
    network: Res<ClientNetwork>,
    mut receivers: Query<&mut MessageReceiver<ServerMessage>, With<client::Client>>,
) {
    let mut inbox_collected: Vec<ServerMessage> = Vec::new();
    for mut receiver in &mut receivers {
        for message in receiver.receive() {
            inbox_collected.push(message);
        }
    }
    if inbox_collected.is_empty() {
        return;
    }

    // Flip status to `Connected` the moment a Welcome lands so the UI's
    // loading splash can transition to `Screen::InGame`. AuthRejected
    // and Kicked propagate via the inbox to `ClientRuntime::apply_message`.
    let mut connected_flip = false;
    for message in &inbox_collected {
        if matches!(message, ServerMessage::Welcome { .. }) {
            connected_flip = true;
            break;
        }
    }
    if connected_flip
        && let Ok(mut status) = network.0.status.lock()
        && !matches!(*status, ClientConnectionStatus::Connected)
    {
        *status = ClientConnectionStatus::Connected;
    }

    let Ok(mut inbox) = network.0.inbox.lock() else {
        return;
    };
    inbox.extend(inbox_collected);
}

fn report_client_disconnect_system(
    network: Res<ClientNetwork>,
    disconnected: Query<&client::Disconnected, (With<client::Client>, Added<client::Disconnected>)>,
) {
    for disconnected in &disconnected {
        let reason = disconnected
            .reason
            .clone()
            .unwrap_or_else(|| "disconnected".to_owned());
        if let Ok(mut status) = network.0.status.lock() {
            *status = ClientConnectionStatus::Disconnected(reason.clone());
        }
        // Surface as an AuthRejected event so existing UI handling
        // (kick notice / toast) picks it up via the inbox path.
        if let Ok(mut inbox) = network.0.inbox.lock() {
            inbox.push_back(ServerMessage::AuthRejected { reason });
        }
    }
}

/// Drives the multi-phase graceful disconnect once `shutdown_request` is
/// set. Same shape as the old standalone-app version: queue a reliable
/// `Disconnect`, wait a couple of ticks for the user message to flush,
/// trigger Lightyear's netcode disconnect, wait a few more ticks for the
/// redundant DISCONNECT packets to leave the socket, then flip
/// `shutdown_complete` so the worker thread can finish.
fn drive_shutdown_system(
    mut commands: Commands,
    network: Res<ClientNetwork>,
    mut shutdown: ResMut<ClientShutdownState>,
    clients: Query<Entity, With<client::Client>>,
) {
    if !network.0.shutdown_request.load(Ordering::Acquire) {
        return;
    }
    if !shutdown.in_progress {
        shutdown.in_progress = true;
        shutdown.ticks_in_phase = 0;
        shutdown.netcode_disconnect_issued = false;
        // Queue the app-level Disconnect so the server can clean us up
        // immediately on its message-handling path, in addition to the
        // netcode Disconnect we will issue a couple of ticks from now.
        if let Ok(mut outbox) = network.0.outbox.lock() {
            outbox.push_back(ClientMessage::Disconnect);
        }
    }

    shutdown.ticks_in_phase = shutdown.ticks_in_phase.saturating_add(1);

    if !shutdown.netcode_disconnect_issued && shutdown.ticks_in_phase >= USER_DISCONNECT_FLUSH_TICKS
    {
        for entity in &clients {
            commands.trigger(client::Disconnect { entity });
        }
        shutdown.netcode_disconnect_issued = true;
        shutdown.ticks_in_phase = 0;
    }

    if shutdown.netcode_disconnect_issued
        && shutdown.ticks_in_phase >= NETCODE_DISCONNECT_FLUSH_TICKS
    {
        for entity in &clients {
            commands.entity(entity).despawn();
        }
        network.0.shutdown_complete.store(true, Ordering::Release);
        // Leave `in_progress = true` so we stop firing on this session;
        // a fresh `process_pending_connect_system` invocation resets it.
        if let Ok(mut status) = network.0.status.lock() {
            *status = ClientConnectionStatus::Idle;
        }
    }
}

impl ClientNetwork {
    /// Snapshot of the current connection state. Polled by the UI to
    /// decide when a session has actually become live.
    #[allow(dead_code)]
    pub(crate) fn status(&self) -> ClientConnectionStatus {
        self.0
            .status
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}
