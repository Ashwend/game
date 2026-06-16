use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use bevy::prelude::*;
use lightyear::prelude::{
    AppChannelExt, AppComponentExt, AppMessageExt, ChannelMode, ChannelSettings, Message,
    MessageSender, NetworkDirection, ReliableSettings,
};

use crate::{
    protocol::{ClientMessage, PacketDelivery, ServerMessage},
    server::{
        Deployable, DeployableActive, DeployableHealth, DeployableLabel, DeployableStability,
        DeployableTransform, DroppedItem, DroppedItemTransform, LootBagContents, LootBagEntity,
        LootBagTransform, Player, PlayerAction, PlayerArmor, PlayerChatBubble, PlayerCrafting,
        PlayerHealth, PlayerHeldItem, PlayerInputAck, PlayerInventory, PlayerLifecycle,
        PlayerOpenContainers, PlayerPose, PlayerProfile, PlayerSleeping, ResourceNode,
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
        app.register_component::<DeployableLabel>();
        app.register_component::<DeployableStability>();
        app.register_component::<Player>();
        // Peer-visible player state, one component per cadence: the
        // pose ticks at 20 Hz while moving, the rest only on real
        // changes (see `src/server/player_ecs.rs`).
        app.register_component::<PlayerProfile>();
        app.register_component::<PlayerPose>();
        app.register_component::<PlayerHealth>();
        app.register_component::<PlayerChatBubble>();
        // Peer-visible cosmetic state for the rigged remote body: what the
        // player holds, and their current swing. Both change far less often
        // than the pose, so they sit apart from it (one diff per tool swap /
        // per swing, not per movement tick).
        app.register_component::<PlayerHeldItem>();
        app.register_component::<PlayerAction>();
        // Owner-only player state, gated per component to the owning
        // sender in `attach_player_replication`.
        app.register_component::<PlayerInventory>();
        app.register_component::<PlayerCrafting>();
        app.register_component::<PlayerOpenContainers>();
        app.register_component::<PlayerInputAck>();
        app.register_component::<PlayerArmor>();
        app.register_component::<PlayerLifecycle>();
        app.register_component::<PlayerSleeping>();
        app.register_component::<LootBagEntity>();
        app.register_component::<LootBagTransform>();
        app.register_component::<LootBagContents>();
    }
}

pub(crate) struct ReliableChannel;
pub(crate) struct UnreliableChannel;
pub(crate) struct VoiceChannel;

/// Wire message that knows which delivery guarantee (and therefore which
/// channel) it wants. Lets [`send_over_channel`] own the single
/// `PacketDelivery -> channel` table for both message directions.
trait HasDelivery {
    fn packet_delivery(&self) -> PacketDelivery;
}

impl HasDelivery for ClientMessage {
    fn packet_delivery(&self) -> PacketDelivery {
        self.delivery()
    }
}

impl HasDelivery for ServerMessage {
    fn packet_delivery(&self) -> PacketDelivery {
        self.delivery()
    }
}

/// Route `message` onto the channel matching its requested delivery. The
/// delivery-to-channel mapping lives here exactly once; both directions share
/// it.
fn send_over_channel<M: Message + HasDelivery>(sender: &mut MessageSender<M>, message: M) {
    match message.packet_delivery() {
        PacketDelivery::Reliable => sender.send::<ReliableChannel>(message),
        PacketDelivery::Unreliable => sender.send::<UnreliableChannel>(message),
        PacketDelivery::UnreliableUnordered => sender.send::<VoiceChannel>(message),
    }
}

pub(crate) fn send_client_message(
    sender: &mut MessageSender<ClientMessage>,
    message: ClientMessage,
) {
    send_over_channel(sender, message);
}

pub(crate) fn send_server_message(
    sender: &mut MessageSender<ServerMessage>,
    message: ServerMessage,
) {
    send_over_channel(sender, message);
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
