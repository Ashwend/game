use std::time::Duration;

use bevy::prelude::*;
use lightyear::prelude::{
    AppChannelExt, AppMessageExt, ChannelMode, ChannelSettings, MessageSender, NetworkDirection,
    ReliableSettings,
};

use crate::protocol::{ClientMessage, PROTOCOL_VERSION, PacketDelivery, ServerMessage};

pub(super) const LIGHTYEAR_PROTOCOL_ID: u64 = PROTOCOL_VERSION as u64;
const LIGHTYEAR_PRIVATE_KEY: [u8; 32] = [0; 32];

#[derive(Clone)]
pub(super) struct LightyearProtocolPlugin;

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

        app.register_message::<ClientMessage>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ServerMessage>()
            .add_direction(NetworkDirection::ServerToClient);
    }
}

pub(super) struct ReliableChannel;
pub(super) struct UnreliableChannel;

pub(super) fn send_client_message(
    sender: &mut MessageSender<ClientMessage>,
    message: ClientMessage,
) {
    match message.delivery() {
        PacketDelivery::Reliable => sender.send::<ReliableChannel>(message),
        PacketDelivery::Unreliable => sender.send::<UnreliableChannel>(message),
    }
}

pub(super) fn send_server_message(
    sender: &mut MessageSender<ServerMessage>,
    message: ServerMessage,
) {
    match message.delivery() {
        PacketDelivery::Reliable => sender.send::<ReliableChannel>(message),
        PacketDelivery::Unreliable => sender.send::<UnreliableChannel>(message),
    }
}

pub(super) fn private_key() -> [u8; 32] {
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
}
