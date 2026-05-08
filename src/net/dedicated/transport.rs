use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use bevy::prelude::*;
#[cfg(feature = "steam")]
use lightyear::prelude::server::{ListenTarget, SteamServerIo};
use lightyear::{
    netcode::NetcodeServer,
    prelude::{
        server::{NetcodeConfig, ServerUdpIo, Start},
        *,
    },
};

use crate::{protocol::PROTOCOL_VERSION, steam::AuthMode};

const LIGHTYEAR_PROTOCOL_ID: u64 = PROTOCOL_VERSION as u64;
const LIGHTYEAR_PRIVATE_KEY: [u8; 32] = [0; 32];
const SEND_INTERVAL: Duration = Duration::from_millis(50);
#[cfg(feature = "steam")]
const DEFAULT_STEAM_APP_ID: u32 = 480;

pub(super) fn spawn_server_transport(
    app: &mut App,
    bind_addr: SocketAddr,
    auth_mode: AuthMode,
) -> Result<()> {
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
pub(super) fn steam_app_id() -> u32 {
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

pub(super) fn start_server(mut commands: Commands, server: Single<Entity, With<Server>>) {
    commands.trigger(Start {
        entity: server.into_inner(),
    });
}

pub(super) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::new("Client Link"),
    ));
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
}
