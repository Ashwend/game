use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{
        Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bevy::prelude::*;
use lightyear::prelude::{
    Authentication, Connected, LocalAddr, MessageReceiver, MessageSender, UdpIo, client,
};

use crate::{
    net::{
        host::{GameServerHandle, spawn_loopback_server},
        protocol::{
            LIGHTYEAR_PROTOCOL_ID, LightyearProtocolPlugin, private_key, send_client_message,
        },
    },
    protocol::{ClientMessage, GAME_VERSION, PROTOCOL_VERSION, ServerMessage},
    save::{WorldSave, WorldStore},
    server::ServerSettings,
    steam::{AuthMode, AuthenticatedUser},
};

const CLIENT_SLEEP: Duration = Duration::from_millis(1);
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

pub enum ClientSession {
    Network(Box<LightyearGameSession>),
}

impl std::fmt::Debug for ClientSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(_) => formatter.write_str("ClientSession::Network"),
        }
    }
}

impl ClientSession {
    pub fn start_singleplayer(save: WorldSave, user: &AuthenticatedUser) -> Result<Self> {
        LightyearGameSession::start_singleplayer(save, user)
            .map(Box::new)
            .map(Self::Network)
    }

    pub fn connect(addr: SocketAddr, user: &AuthenticatedUser) -> Result<Self> {
        LightyearGameSession::connect(addr, user)
            .map(Box::new)
            .map(Self::Network)
    }

    pub fn send(&mut self, message: ClientMessage) -> Result<()> {
        match self {
            Self::Network(session) => session.send(message),
        }
    }

    pub fn tick(&mut self, delta_seconds: f32) -> Result<Vec<ServerMessage>> {
        match self {
            Self::Network(session) => session.tick(delta_seconds),
        }
    }

    pub fn shutdown(&mut self, store: &WorldStore) -> Result<()> {
        let _ = self.send(ClientMessage::Disconnect);
        match self {
            Self::Network(session) => session.shutdown(store)?,
        }
        Ok(())
    }
}

pub struct LightyearGameSession {
    command_tx: Sender<ClientCommand>,
    incoming: Mutex<Receiver<ServerMessage>>,
    inbox: VecDeque<ServerMessage>,
    thread: Mutex<Option<JoinHandle<()>>>,
    local_server: Mutex<Option<GameServerHandle>>,
}

impl std::fmt::Debug for LightyearGameSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let local_server_running = self
            .local_server
            .lock()
            .is_ok_and(|local_server| local_server.is_some());
        formatter
            .debug_struct("LightyearGameSession")
            .field("local_server", &local_server_running)
            .field("inbox_len", &self.inbox.len())
            .finish_non_exhaustive()
    }
}

impl LightyearGameSession {
    pub fn start_singleplayer(save: WorldSave, user: &AuthenticatedUser) -> Result<Self> {
        let spawned = spawn_loopback_server(
            save,
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(user.steam_id),
            },
        )?;
        Self::connect_inner(spawned.addr, user, Some(spawned.handle))
    }

    pub fn connect(addr: SocketAddr, user: &AuthenticatedUser) -> Result<Self> {
        Self::connect_inner(addr, user, None)
    }

    fn connect_inner(
        addr: SocketAddr,
        user: &AuthenticatedUser,
        mut local_server: Option<GameServerHandle>,
    ) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (incoming_tx, incoming) = mpsc::channel();
        let (startup_tx, startup_rx) = mpsc::channel();
        let steam_id = user.steam_id;
        let auth_message = ClientMessage::Auth {
            protocol_version: PROTOCOL_VERSION,
            client_version: Some(GAME_VERSION.to_owned()),
            steam_id,
            display_name: user.display_name.clone(),
            token: user.token.clone(),
        };
        let thread = thread::Builder::new()
            .name("lightyear-game-client".to_owned())
            .spawn(move || {
                match build_client_app(addr, steam_id, auth_message, command_rx, incoming_tx) {
                    Ok(mut app) => {
                        let _ = startup_tx.send(Ok(()));
                        run_client_app(&mut app);
                    }
                    Err(error) => {
                        let _ = startup_tx.send(Err(format!("{error:#}")));
                    }
                }
            })
            .context("could not spawn Lightyear game client")?;

        match startup_rx
            .recv_timeout(AUTH_TIMEOUT)
            .context("Lightyear game client did not start")?
        {
            Ok(()) => {}
            Err(error) => {
                let _ = thread.join();
                if let Some(mut local_server) = local_server.take() {
                    let _ = local_server.shutdown();
                }
                bail!("{error}");
            }
        }

        let inbox = match wait_for_welcome(&incoming) {
            Ok(inbox) => inbox,
            Err(error) => {
                let _ = command_tx.send(ClientCommand::Shutdown);
                let _ = thread.join();
                if let Some(mut local_server) = local_server.take() {
                    let _ = local_server.shutdown();
                }
                return Err(error);
            }
        };

        Ok(Self {
            command_tx,
            incoming: Mutex::new(incoming),
            inbox,
            thread: Mutex::new(Some(thread)),
            local_server: Mutex::new(local_server),
        })
    }

    pub fn send(&mut self, message: ClientMessage) -> Result<()> {
        self.command_tx
            .send(ClientCommand::Send(message))
            .context("Lightyear game client is not running")
    }

    pub fn tick(&mut self, _delta_seconds: f32) -> Result<Vec<ServerMessage>> {
        let incoming = self
            .incoming
            .lock()
            .map_err(|_| anyhow::anyhow!("Lightyear receiver lock is poisoned"))?;
        self.inbox.extend(incoming.try_iter());
        Ok(self.inbox.drain(..).collect())
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
        let _ = self.command_tx.send(ClientCommand::Shutdown);

        let mut local_server = self
            .local_server
            .lock()
            .map_err(|_| anyhow::anyhow!("game server handle lock is poisoned"))?;
        if let Some(mut local_server) = local_server.take() {
            local_server.shutdown()?;
        }
        drop(local_server);

        let mut thread = self
            .thread
            .lock()
            .map_err(|_| anyhow::anyhow!("Lightyear game client thread lock is poisoned"))?;
        if let Some(thread) = thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("Lightyear game client thread panicked"))?;
        }
        Ok(())
    }
}

fn wait_for_welcome(incoming: &Receiver<ServerMessage>) -> Result<VecDeque<ServerMessage>> {
    let deadline = Instant::now() + AUTH_TIMEOUT;
    let mut buffered = VecDeque::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("Lightyear game server did not answer auth");
        }

        let message = incoming
            .recv_timeout(remaining)
            .context("Lightyear game server did not answer auth")?;
        match message {
            ServerMessage::Welcome { .. } => {
                let mut inbox = VecDeque::from([message]);
                inbox.extend(buffered);
                return Ok(inbox);
            }
            ServerMessage::AuthRejected { reason } => bail!("auth rejected: {reason}"),
            ServerMessage::Kicked { reason } => bail!("disconnected: {reason}"),
            message => buffered.push_back(message),
        }
    }
}

impl Drop for LightyearGameSession {
    fn drop(&mut self) {
        let _ = self.shutdown_transport();
    }
}

#[derive(Debug)]
enum ClientCommand {
    Send(ClientMessage),
    Shutdown,
}

#[derive(Resource)]
struct ClientCommandInbox(Mutex<Receiver<ClientCommand>>);

#[derive(Resource)]
struct ClientIncoming(Sender<ServerMessage>);

#[derive(Resource)]
struct ClientAuth {
    message: ClientMessage,
    sent: bool,
}

#[derive(Resource, Default)]
struct PendingClientMessages(VecDeque<ClientMessage>);

#[derive(Resource, Default)]
struct ClientHeartbeat {
    elapsed: Duration,
}

#[derive(Resource, Default)]
struct ClientShutdown {
    requested: bool,
}

fn build_client_app(
    server_addr: SocketAddr,
    steam_id: u64,
    auth_message: ClientMessage,
    command_rx: Receiver<ClientCommand>,
    incoming_tx: Sender<ServerMessage>,
) -> Result<App> {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(client::ClientPlugins {
        tick_duration: Duration::from_secs_f32(1.0 / crate::protocol::SERVER_TICK_RATE_HZ),
    });
    app.add_plugins(LightyearProtocolPlugin);

    let client_entity = app
        .world_mut()
        .spawn((
            Name::new("Lightyear Game Client"),
            LocalAddr(SocketAddr::from(([0, 0, 0, 0], 0))),
            UdpIo::default(),
            client::NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id: steam_id,
                    private_key: private_key(),
                    protocol_id: LIGHTYEAR_PROTOCOL_ID,
                },
                client::NetcodeConfig::default(),
            )
            .context("could not create Lightyear netcode client")?,
        ))
        .id();

    app.insert_resource(ClientCommandInbox(Mutex::new(command_rx)));
    app.insert_resource(ClientIncoming(incoming_tx));
    app.insert_resource(ClientAuth {
        message: auth_message,
        sent: false,
    });
    app.insert_resource(PendingClientMessages::default());
    app.insert_resource(ClientHeartbeat::default());
    app.insert_resource(ClientShutdown::default());

    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(client::Connect {
            entity: client_entity,
        });
    });
    app.add_systems(
        Update,
        (
            send_client_messages,
            receive_server_messages,
            report_client_disconnect,
        )
            .chain(),
    );
    app.finish();
    app.cleanup();

    Ok(app)
}

fn run_client_app(app: &mut App) {
    loop {
        app.update();
        if app.world().resource::<ClientShutdown>().requested {
            return;
        }
        thread::sleep(CLIENT_SLEEP);
    }
}

fn send_client_messages(
    time: Res<Time>,
    inbox: Res<ClientCommandInbox>,
    mut auth: ResMut<ClientAuth>,
    mut pending: ResMut<PendingClientMessages>,
    mut heartbeat: ResMut<ClientHeartbeat>,
    mut shutdown: ResMut<ClientShutdown>,
    mut clients: Query<(&mut MessageSender<ClientMessage>, Has<Connected>), With<client::Client>>,
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
            ClientCommand::Send(message) => pending.0.push_back(message),
            ClientCommand::Shutdown => shutdown.requested = true,
        }
    }

    for (mut sender, connected) in &mut clients {
        if !connected {
            continue;
        }
        if !auth.sent {
            send_client_message(&mut sender, auth.message.clone());
            auth.sent = true;
        }
        while let Some(message) = pending.0.pop_front() {
            send_client_message(&mut sender, message);
        }
        if auth.sent && heartbeat.tick(time.delta()) {
            send_client_message(&mut sender, ClientMessage::Heartbeat);
        }
    }
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
}

fn receive_server_messages(
    incoming: Res<ClientIncoming>,
    mut receivers: Query<&mut MessageReceiver<ServerMessage>, With<client::Client>>,
) {
    for mut receiver in &mut receivers {
        let messages: Vec<ServerMessage> = receiver.receive().collect();
        for message in messages {
            if incoming.0.send(message).is_err() {
                return;
            }
        }
    }
}

fn report_client_disconnect(
    incoming: Res<ClientIncoming>,
    disconnected: Query<&client::Disconnected, (With<client::Client>, Added<client::Disconnected>)>,
) {
    for disconnected in &disconnected {
        if let Some(reason) = &disconnected.reason {
            let _ = incoming.0.send(ServerMessage::AuthRejected {
                reason: reason.clone(),
            });
        }
    }
}
