use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use bevy::{
    diagnostic::DiagnosticsPlugin,
    ecs::entity::{EntityMapper, MapEntities},
    ecs::system::SystemParam,
    math::Curve,
    prelude::*,
    state::app::StatesPlugin,
};
#[cfg(feature = "steam")]
use lightyear::prelude::server::{ListenTarget, SteamServerIo};
use lightyear::{
    connection::client::Connected,
    netcode::NetcodeServer,
    prelude::{
        input::native::{ActionState, InputPlugin},
        server::{ClientOf, NetcodeConfig, ServerPlugins, ServerUdpIo, Start},
        *,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    controller::PlayerController,
    protocol::{MAX_HEALTH, PROTOCOL_VERSION, PlayerInput, SERVER_TICK_RATE_HZ, SteamId, Vec3Net},
    save::WorldSave,
    steam::AuthMode,
    world::WorldData,
};

const LIGHTYEAR_PROTOCOL_ID: u64 = PROTOCOL_VERSION as u64;
const LIGHTYEAR_PRIVATE_KEY: [u8; 32] = [0; 32];
const SEND_INTERVAL: Duration = Duration::from_millis(50);
#[cfg(feature = "steam")]
const DEFAULT_STEAM_APP_ID: u32 = 480;

pub fn run_dedicated_server(
    bind_addr: SocketAddr,
    save: WorldSave,
    auth_mode: AuthMode,
) -> Result<()> {
    let fixed_delta = Duration::from_secs_f64(1.0 / f64::from(SERVER_TICK_RATE_HZ));
    let mut app = App::new();

    #[cfg(feature = "steam")]
    if auth_mode == AuthMode::Steam {
        app.add_steam_resources(steam_app_id());
    }

    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        DiagnosticsPlugin,
        ServerPlugins {
            tick_duration: fixed_delta,
        },
    ));
    app.add_plugins(LightyearProtocolPlugin);
    app.insert_resource(NetworkWorld(save.map.world_data()));
    app.add_observer(handle_new_client);
    app.add_observer(handle_connected_client);
    app.add_systems(FixedUpdate, authoritative_movement_system);

    spawn_server_transport(&mut app, bind_addr, auth_mode)?;
    app.add_systems(Startup, start_server);

    println!("lightyear server listening on {bind_addr} ({auth_mode:?})");
    app.run();
    Ok(())
}

fn spawn_server_transport(app: &mut App, bind_addr: SocketAddr, auth_mode: AuthMode) -> Result<()> {
    let mut entity = app.world_mut().spawn(Name::new("Lightyear Server"));

    match auth_mode {
        AuthMode::Offline => {
            entity.insert((
                LocalAddr(bind_addr),
                ServerUdpIo::default(),
                NetcodeServer::new(NetcodeConfig {
                    protocol_id: LIGHTYEAR_PROTOCOL_ID,
                    private_key: private_key(),
                    ..default()
                }),
            ));
        }
        AuthMode::Steam => spawn_steam_server_transport(&mut entity, bind_addr)?,
    }

    Ok(())
}

fn private_key() -> [u8; 32] {
    std::env::var("LIGHTYEAR_PRIVATE_KEY")
        .ok()
        .and_then(|value| parse_private_key(&value))
        .unwrap_or(LIGHTYEAR_PRIVATE_KEY)
}

fn parse_private_key(value: &str) -> Option<[u8; 32]> {
    let bytes = value
        .split(',')
        .map(str::trim)
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    Some(bytes)
}

#[cfg(feature = "steam")]
fn steam_app_id() -> u32 {
    std::env::var("GAME_STEAM_APP_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_STEAM_APP_ID)
}

#[cfg(feature = "steam")]
fn spawn_steam_server_transport(entity: &mut EntityWorldMut, bind_addr: SocketAddr) -> Result<()> {
    entity.insert(SteamServerIo {
        target: ListenTarget::Addr(bind_addr),
        config: SessionConfig::default(),
    });
    Ok(())
}

#[cfg(not(feature = "steam"))]
fn spawn_steam_server_transport(
    _entity: &mut EntityWorldMut,
    _bind_addr: SocketAddr,
) -> Result<()> {
    anyhow::bail!("Steam networking requires building with --features steam");
}

fn start_server(mut commands: Commands, server: Single<Entity, With<Server>>) {
    commands.trigger(Start {
        entity: server.into_inner(),
    });
}

fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::new("Client Link"),
    ));
}

fn handle_connected_client(
    trigger: On<Add, Connected>,
    clients: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(remote_id) = clients.get(trigger.entity) else {
        return;
    };
    let client_id = remote_id.0;
    commands.spawn((
        NetworkPlayerBundle::new(client_id, Vec3Net::ZERO),
        NetworkController(PlayerController::spawn()),
        NetworkInputSequence::default(),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: trigger.entity,
            lifetime: Default::default(),
        },
    ));
}

type NetworkMovementData = (
    &'static mut NetworkController,
    &'static mut NetworkInputSequence,
    &'static ActionState<NetworkInput>,
    &'static mut NetworkPosition,
    &'static mut NetworkVelocity,
    &'static mut NetworkLook,
    &'static mut NetworkHealth,
    &'static mut NetworkGrounded,
);

#[derive(SystemParam)]
struct NetworkMovementParams<'w, 's> {
    players: Query<'w, 's, NetworkMovementData, Without<Predicted>>,
}

fn authoritative_movement_system(world: Res<NetworkWorld>, mut params: NetworkMovementParams) {
    for (
        mut controller,
        mut sequence,
        input,
        mut position,
        mut velocity,
        mut look,
        mut health,
        mut grounded,
    ) in &mut params.players
    {
        apply_network_input(
            &mut controller.0,
            &mut sequence,
            &input.0,
            &world.0,
            1.0 / SERVER_TICK_RATE_HZ,
        );
        write_controller_state(
            &controller.0,
            &mut position,
            &mut velocity,
            &mut look,
            &mut health,
            &mut grounded,
        );
    }
}

fn apply_network_input(
    controller: &mut PlayerController,
    sequence: &mut NetworkInputSequence,
    input: &NetworkInput,
    world: &WorldData,
    delta_seconds: f32,
) {
    sequence.0 += 1;
    controller.apply_input(PlayerInput {
        sequence: sequence.0,
        delta_seconds,
        direction: input.direction,
        sprint: input.sprint,
        jump: input.jump,
        yaw: input.yaw,
        pitch: input.pitch,
    });
    controller.simulate(delta_seconds, world);
}

fn write_controller_state(
    controller: &PlayerController,
    position: &mut NetworkPosition,
    velocity: &mut NetworkVelocity,
    look: &mut NetworkLook,
    health: &mut NetworkHealth,
    grounded: &mut NetworkGrounded,
) {
    position.0 = controller.position;
    velocity.0 = controller.velocity;
    look.yaw = controller.yaw;
    look.pitch = controller.pitch;
    health.0 = controller.health;
    grounded.0 = controller.grounded;
}

#[derive(Clone)]
pub struct LightyearProtocolPlugin;

impl Plugin for LightyearProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputPlugin::<NetworkInput>::default());

        app.register_component::<NetworkPlayerId>();
        app.register_component::<NetworkPlayerName>();
        app.register_component::<NetworkSteamId>();
        app.register_component::<NetworkAdmin>();

        app.register_component::<NetworkPosition>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<NetworkVelocity>().add_prediction();
        app.register_component::<NetworkLook>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<NetworkHealth>().add_prediction();
        app.register_component::<NetworkGrounded>().add_prediction();
    }
}

#[derive(Resource, Clone)]
struct NetworkWorld(WorldData);

#[derive(Component)]
struct NetworkController(PlayerController);

#[derive(Component, Default)]
struct NetworkInputSequence(u64);

#[derive(Bundle)]
struct NetworkPlayerBundle {
    id: NetworkPlayerId,
    steam_id: NetworkSteamId,
    name: NetworkPlayerName,
    admin: NetworkAdmin,
    position: NetworkPosition,
    velocity: NetworkVelocity,
    look: NetworkLook,
    health: NetworkHealth,
    grounded: NetworkGrounded,
}

impl NetworkPlayerBundle {
    fn new(id: PeerId, position: Vec3Net) -> Self {
        let steam_id = id.to_bits();
        Self {
            id: NetworkPlayerId(id),
            steam_id: NetworkSteamId(steam_id),
            name: NetworkPlayerName(clean_network_name(steam_id)),
            admin: NetworkAdmin(false),
            position: NetworkPosition(position),
            velocity: NetworkVelocity(Vec3Net::ZERO),
            look: NetworkLook::default(),
            health: NetworkHealth(MAX_HEALTH),
            grounded: NetworkGrounded(true),
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct NetworkPlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkSteamId(pub SteamId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkPlayerName(pub String);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkAdmin(pub bool);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkPosition(pub Vec3Net);

impl Ease for NetworkPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            NetworkPosition(lerp_vec3(start.0, end.0, t))
        })
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkVelocity(pub Vec3Net);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkLook {
    pub yaw: f32,
    pub pitch: f32,
}

impl Ease for NetworkLook {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| Self {
            yaw: lerp_f32(start.yaw, end.yaw, t),
            pitch: lerp_f32(start.pitch, end.pitch, t),
        })
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct NetworkHealth(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkGrounded(pub bool);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkInput {
    pub direction: Vec3Net,
    pub sprint: bool,
    pub jump: bool,
    pub yaw: f32,
    pub pitch: f32,
}

impl MapEntities for NetworkInput {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
}

fn lerp_vec3(start: Vec3Net, end: Vec3Net, t: f32) -> Vec3Net {
    Vec3Net::new(
        lerp_f32(start.x, end.x, t),
        lerp_f32(start.y, end.y, t),
        lerp_f32(start.z, end.z, t),
    )
}

fn lerp_f32(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn clean_network_name(steam_id: SteamId) -> String {
    format!("Player {steam_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_key_parser_requires_32_bytes() {
        let key = (0..32).map(|_| "7").collect::<Vec<_>>().join(",");
        assert_eq!(parse_private_key(&key), Some([7; 32]));
        assert!(parse_private_key("1,2,3").is_none());
    }

    #[test]
    fn network_position_interpolates_linearly() {
        let halfway = lerp_vec3(Vec3Net::ZERO, Vec3Net::new(2.0, 4.0, 6.0), 0.5);
        assert_eq!(halfway, Vec3Net::new(1.0, 2.0, 3.0));
    }
}
