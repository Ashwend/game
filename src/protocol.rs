use serde::{Deserialize, Serialize};

use crate::world::{MapType, WorldData};

pub type ClientId = u64;
pub type PacketSequence = u64;
pub type SteamId = u64;

pub const PROTOCOL_VERSION: u32 = 8;
pub const SERVER_TICK_RATE_HZ: f32 = 20.0;
pub const MAX_INPUT_DELTA_SECONDS: f32 = 1.0 / SERVER_TICK_RATE_HZ;
pub const MAX_CHAT_LEN: usize = 240;
pub const MAX_HEALTH: f32 = 100.0;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Vec3Net {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3Net {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length_squared(self) -> f32 {
        self.x
            .mul_add(self.x, self.y.mul_add(self.y, self.z * self.z))
    }

    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq <= f32::EPSILON {
            return Self::ZERO;
        }

        let inv_len = len_sq.sqrt().recip();
        Self::new(self.x * inv_len, self.y * inv_len, self.z * inv_len)
    }

    pub fn scale(self, value: f32) -> Self {
        Self::new(self.x * value, self.y * value, self.z * value)
    }

    pub fn plus(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientMessage {
    Auth {
        protocol_version: u32,
        steam_id: SteamId,
        display_name: String,
        token: String,
    },
    Movement(PlayerMovement),
    Chat {
        text: String,
    },
    Heartbeat,
    Disconnect,
}

impl ClientMessage {
    pub fn kind(&self) -> ClientMessageKind {
        match self {
            Self::Auth { .. } => ClientMessageKind::Auth,
            Self::Movement(_) => ClientMessageKind::Movement,
            Self::Chat { .. } => ClientMessageKind::Chat,
            Self::Heartbeat => ClientMessageKind::Heartbeat,
            Self::Disconnect => ClientMessageKind::Disconnect,
        }
    }

    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Auth { .. } | Self::Chat { .. } | Self::Disconnect => PacketDelivery::Reliable,
            Self::Movement(_) | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PlayerInput {
    pub sequence: u64,
    pub delta_seconds: f32,
    pub direction: Vec3Net,
    pub sprint: bool,
    pub jump: bool,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PlayerMovement {
    pub sequence: u64,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ServerMessage {
    Welcome {
        client_id: ClientId,
        map: MapType,
        world: WorldData,
        is_admin: bool,
        snapshot: WorldSnapshot,
    },
    AuthRejected {
        reason: String,
    },
    PlayerEvent(PlayerEvent),
    Snapshot(WorldSnapshot),
    Correction(PlayerState),
    Chat(ChatMessage),
    Heartbeat,
}

impl ServerMessage {
    pub fn kind(&self) -> ServerMessageKind {
        match self {
            Self::Welcome { .. } => ServerMessageKind::Welcome,
            Self::AuthRejected { .. } => ServerMessageKind::AuthRejected,
            Self::PlayerEvent(_) => ServerMessageKind::PlayerEvent,
            Self::Snapshot(_) => ServerMessageKind::Snapshot,
            Self::Correction(_) => ServerMessageKind::Correction,
            Self::Chat(_) => ServerMessageKind::Chat,
            Self::Heartbeat => ServerMessageKind::Heartbeat,
        }
    }

    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Welcome { .. }
            | Self::AuthRejected { .. }
            | Self::PlayerEvent(_)
            | Self::Chat(_) => PacketDelivery::Reliable,
            Self::Snapshot(_) | Self::Correction(_) | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PacketDelivery {
    Unreliable,
    Reliable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClientMessageKind {
    Auth,
    Movement,
    Chat,
    Heartbeat,
    Disconnect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServerMessageKind {
    Welcome,
    AuthRejected,
    PlayerEvent,
    Snapshot,
    Correction,
    Chat,
    Heartbeat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientPacket {
    pub protocol_version: u32,
    pub sequence: PacketSequence,
    pub ack: PacketSequence,
    pub delivery: PacketDelivery,
    pub kind: ClientMessageKind,
    pub message: ClientMessage,
}

impl ClientPacket {
    pub fn new(sequence: PacketSequence, ack: PacketSequence, message: ClientMessage) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            sequence,
            ack,
            delivery: message.delivery(),
            kind: message.kind(),
            message,
        }
    }

    pub fn into_message(self) -> Option<ClientMessage> {
        (self.protocol_version == PROTOCOL_VERSION
            && self.delivery == self.message.delivery()
            && self.kind == self.message.kind())
        .then_some(self.message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerPacket {
    pub protocol_version: u32,
    pub sequence: PacketSequence,
    pub ack: PacketSequence,
    pub delivery: PacketDelivery,
    pub kind: ServerMessageKind,
    pub message: ServerMessage,
}

impl ServerPacket {
    pub fn new(sequence: PacketSequence, ack: PacketSequence, message: ServerMessage) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            sequence,
            ack,
            delivery: message.delivery(),
            kind: message.kind(),
            message,
        }
    }

    pub fn into_message(self) -> Option<ServerMessage> {
        (self.protocol_version == PROTOCOL_VERSION
            && self.delivery == self.message.delivery()
            && self.kind == self.message.kind())
        .then_some(self.message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PlayerEvent {
    Joined { client_id: ClientId, name: String },
    Left { client_id: ClientId, name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub from: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorldSnapshot {
    pub tick: u64,
    pub players: Vec<PlayerState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerState {
    pub client_id: ClientId,
    pub steam_id: SteamId,
    pub name: String,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub grounded: bool,
    pub last_processed_input: u64,
    pub is_admin: bool,
}

pub fn sanitize_chat(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.chars().take(MAX_CHAT_LEN).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_zero_stays_zero() {
        assert_eq!(Vec3Net::ZERO.normalize_or_zero(), Vec3Net::ZERO);
    }

    #[test]
    fn normalize_regular_vector() {
        let normalized = Vec3Net::new(3.0, 0.0, 4.0).normalize_or_zero();
        assert!((normalized.x - 0.6).abs() < 0.0001);
        assert!((normalized.z - 0.8).abs() < 0.0001);
    }

    #[test]
    fn chat_is_trimmed_and_limited() {
        let long = format!("  {}  ", "a".repeat(MAX_CHAT_LEN + 50));
        let sanitized = sanitize_chat(&long).expect("chat should be valid");
        assert_eq!(sanitized.len(), MAX_CHAT_LEN);
        assert!(sanitize_chat("   ").is_none());
    }

    #[test]
    fn packets_validate_version_kind_and_delivery() {
        let packet = ClientPacket::new(7, 3, ClientMessage::Heartbeat);
        assert_eq!(packet.kind, ClientMessageKind::Heartbeat);
        assert_eq!(packet.delivery, PacketDelivery::Unreliable);
        assert!(packet.into_message().is_some());

        let mut packet = ServerPacket::new(4, 2, ServerMessage::Heartbeat);
        packet.kind = ServerMessageKind::Snapshot;
        assert!(packet.into_message().is_none());
    }
}
