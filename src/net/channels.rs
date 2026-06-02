use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use bevy::prelude::*;
use lightyear::prelude::{
    AppChannelExt, AppComponentExt, AppMessageExt, ChannelMode, ChannelSettings, MessageSender,
    NetworkDirection, ReliableSettings,
};

use crate::{
    protocol::{ClientMessage, PacketDelivery, ServerMessage},
    server::{
        Deployable, DeployableActive, DeployableHealth, DeployableTransform, DroppedItem,
        DroppedItemTransform, LootBagContents, LootBagEntity, LootBagTransform, Player,
        PlayerArmor, PlayerLifecycle, PlayerPrivate, PlayerPublic, ResourceNode,
        ResourceNodeStorage,
    },
};

/// Fixed netcode `protocol_id` for the Ashwend transport, deliberately
/// **independent of [`crate::protocol::PROTOCOL_VERSION`]**.
///
/// The netcode bakes this id into the encrypted connect token and rejects any
/// token whose id doesn't match, at the transport layer, before a single
/// application message is exchanged. If it tracked `PROTOCOL_VERSION`, a
/// version-bumped client would be bounced there and could never learn *which*
/// version the server runs. Keeping it fixed lets the connection always reach
/// the application-level `Auth` handshake, where `GameServer::connect` compares
/// versions and answers with a structured `ServerMessage::VersionMismatch`
/// carrying the server's version, so the client can show a "you're
/// newer/older" modal. The `Auth` / `AuthRejected` / `VersionMismatch` wire
/// shapes must therefore stay stable across versions; bump this id only on a
/// genuinely incompatible *transport* change.
pub(crate) const LIGHTYEAR_PROTOCOL_ID: u64 = 0x4153_4857_454E_4401; // b"ASHWEND\x01"
const LIGHTYEAR_PRIVATE_KEY: [u8; 32] = [0; 32];

#[derive(Clone)]
pub(crate) struct LightyearProtocolPlugin;

impl Plugin for LightyearProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<ReliableChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            send_frequency: Duration::default(),
            priority: 10.0,
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.add_channel::<UnreliableChannel>(ChannelSettings {
            mode: ChannelMode::SequencedUnreliable,
            send_frequency: Duration::default(),
            priority: 5.0,
        })
        .add_direction(NetworkDirection::Bidirectional);

        // Dedicated channel for voice frames. `UnorderedUnreliable` is the
        // standard VOIP-over-UDP pick: every delivered Opus packet is
        // surfaced to the receiver regardless of arrival order, so a frame
        // that races slightly past its neighbours still gets played rather
        // than being silently dropped (which is what `Sequenced` would do
        // and what produced the periodic-flicker symptom in earlier tests).
        // Higher priority than non-voice unreliable traffic so a busy
        // replication or movement stream doesn't shoulder voice off the
        // wire under load.
        app.add_channel::<VoiceChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            send_frequency: Duration::default(),
            priority: 8.0,
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.register_message::<ClientMessage>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ServerMessage>()
            .add_direction(NetworkDirection::ServerToClient);

        // Per-component replication: every networked entity ships through
        // Lightyear's room-gated replication. The server spawns the mirror
        // entity carrying these components (see `src/net/host.rs`); the
        // client receives them automatically once it subscribes to the
        // chunk room. Both sides need an identical registry here so the
        // wire bytes round-trip.
        app.register_component::<ResourceNode>();
        app.register_component::<ResourceNodeStorage>();
        app.register_component::<DroppedItem>();
        app.register_component::<DroppedItemTransform>();
        app.register_component::<Deployable>();
        app.register_component::<DeployableTransform>();
        app.register_component::<DeployableHealth>();
        app.register_component::<DeployableActive>();
        app.register_component::<Player>();
        app.register_component::<PlayerPublic>();
        app.register_component::<PlayerPrivate>();
        app.register_component::<PlayerArmor>();
        app.register_component::<PlayerLifecycle>();
        app.register_component::<LootBagEntity>();
        app.register_component::<LootBagTransform>();
        app.register_component::<LootBagContents>();
    }
}

pub(crate) struct ReliableChannel;
pub(crate) struct UnreliableChannel;
pub(crate) struct VoiceChannel;

pub(crate) fn send_client_message(
    sender: &mut MessageSender<ClientMessage>,
    message: ClientMessage,
) {
    match message.delivery() {
        PacketDelivery::Reliable => sender.send::<ReliableChannel>(message),
        PacketDelivery::Unreliable => sender.send::<UnreliableChannel>(message),
        PacketDelivery::UnreliableUnordered => sender.send::<VoiceChannel>(message),
    }
}

pub(crate) fn send_server_message(
    sender: &mut MessageSender<ServerMessage>,
    message: ServerMessage,
) {
    match message.delivery() {
        PacketDelivery::Reliable => sender.send::<ReliableChannel>(message),
        PacketDelivery::Unreliable => sender.send::<UnreliableChannel>(message),
        PacketDelivery::UnreliableUnordered => sender.send::<VoiceChannel>(message),
    }
}

/// Context the caller can pass so the warning only fires when it matters.
/// Loopback servers (singleplayer host) bind to 127.0.0.1 and can't be
/// reached over the network, so they don't need the warning. Dedicated
/// servers and remote clients do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateKeyContext {
    /// Singleplayer loopback, listener bound to localhost.
    Loopback,
    /// Anything reachable from another host (dedicated server, remote
    /// client). The all-zero default is dangerous here.
    NetworkExposed,
}

pub(crate) fn private_key(context: PrivateKeyContext) -> [u8; 32] {
    resolve_private_key(
        std::env::var("LIGHTYEAR_PRIVATE_KEY").ok().as_deref(),
        context,
    )
}

fn resolve_private_key(env_value: Option<&str>, context: PrivateKeyContext) -> [u8; 32] {
    if let Some(value) = env_value
        && let Some(key) = parse_private_key(value)
    {
        return key;
    }
    if context == PrivateKeyContext::NetworkExposed {
        // Print once per process so repeat callers (`run_game_server`,
        // `build_client_app`) don't spam the log.
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "warning: LIGHTYEAR_PRIVATE_KEY is unset or unparseable, using the all-zero default. \
                 Anyone on the network can forge connections with this key; set \
                 LIGHTYEAR_PRIVATE_KEY (32 comma-separated bytes) before exposing this server."
            );
        }
    }
    LIGHTYEAR_PRIVATE_KEY
}

fn parse_private_key(value: &str) -> Option<[u8; 32]> {
    let bytes = value
        .split(',')
        .map(str::trim)
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    bytes.try_into().ok()
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
    fn resolve_private_key_uses_parsed_env_when_available() {
        let key = (0..32).map(|_| "9").collect::<Vec<_>>().join(",");
        assert_eq!(
            resolve_private_key(Some(&key), PrivateKeyContext::NetworkExposed),
            [9; 32]
        );
    }

    #[test]
    fn resolve_private_key_falls_back_to_default_when_env_missing() {
        assert_eq!(
            resolve_private_key(None, PrivateKeyContext::Loopback),
            LIGHTYEAR_PRIVATE_KEY
        );
        assert_eq!(
            resolve_private_key(Some("bogus"), PrivateKeyContext::Loopback),
            LIGHTYEAR_PRIVATE_KEY
        );
    }
}
