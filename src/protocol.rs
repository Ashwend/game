use bevy::prelude::Reflect;
use serde::{Deserialize, Serialize};

use crate::world::{MapType, WorldData};

pub type ClientId = u64;
pub type SteamId = u64;

pub const PROTOCOL_VERSION: u32 = 12;
pub const GAME_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SERVER_TICK_RATE_HZ: f32 = 20.0;
pub const MAX_CHAT_LEN: usize = 240;
pub const MAX_HEALTH: f32 = 100.0;
pub const INVENTORY_SLOT_COUNT: usize = 40;
pub const ACTIONBAR_SLOT_COUNT: usize = 9;

pub type DroppedItemId = u64;
pub type ResourceNodeId = u64;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Reflect)]
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

    pub fn minus(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x
            .mul_add(other.x, self.y.mul_add(other.y, self.z * other.z))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Reflect)]
pub struct QuatNet {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl QuatNet {
    pub const IDENTITY: Self = Self::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}

impl Default for QuatNet {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientMessage {
    Auth {
        protocol_version: u32,
        #[serde(default)]
        client_version: Option<String>,
        steam_id: SteamId,
        display_name: String,
        token: String,
    },
    Movement(PlayerMovement),
    Chat {
        text: String,
    },
    Inventory(InventoryCommand),
    Gather(ResourceGatherCommand),
    Heartbeat,
    Disconnect,
}

impl ClientMessage {
    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Auth { .. }
            | Self::Chat { .. }
            | Self::Inventory(_)
            | Self::Gather(_)
            | Self::Disconnect => PacketDelivery::Reliable,
            Self::Movement(_) | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemStack {
    #[serde(deserialize_with = "deserialize_interned_item_id")]
    pub item_id: crate::items::ItemId,
    pub quantity: u16,
}

impl ItemStack {
    pub fn new(item_id: impl AsRef<str>, quantity: u16) -> Self {
        Self {
            item_id: crate::items::intern_item_id(item_id.as_ref()),
            quantity,
        }
    }
}

fn deserialize_interned_item_id<'de, D>(deserializer: D) -> Result<crate::items::ItemId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(crate::items::intern_item_id(&raw))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ItemContainer {
    Inventory,
    Actionbar,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ItemContainerSlot {
    pub container: ItemContainer,
    pub slot: usize,
}

impl ItemContainerSlot {
    pub const fn inventory(slot: usize) -> Self {
        Self {
            container: ItemContainer::Inventory,
            slot,
        }
    }

    pub const fn actionbar(slot: usize) -> Self {
        Self {
            container: ItemContainer::Actionbar,
            slot,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InventoryCommand {
    Move {
        from: ItemContainerSlot,
        to: ItemContainerSlot,
        quantity: Option<u16>,
    },
    Drop {
        from: ItemContainerSlot,
        quantity: Option<u16>,
    },
    PickUp {
        dropped_item_id: DroppedItemId,
    },
    SelectActionbarSlot {
        slot: usize,
    },
    SelectActionbarOffset {
        offset: i8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceGatherCommand {
    pub resource_node_id: ResourceNodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerInventoryState {
    pub inventory_slots: Vec<Option<ItemStack>>,
    pub actionbar_slots: Vec<Option<ItemStack>>,
    pub active_actionbar_slot: usize,
}

impl Default for PlayerInventoryState {
    fn default() -> Self {
        Self::empty()
    }
}

impl PlayerInventoryState {
    pub fn empty() -> Self {
        Self {
            inventory_slots: vec![None; INVENTORY_SLOT_COUNT],
            actionbar_slots: vec![None; ACTIONBAR_SLOT_COUNT],
            active_actionbar_slot: 0,
        }
    }

    pub fn active_actionbar_stack(&self) -> Option<&ItemStack> {
        self.actionbar_slots
            .get(self.active_actionbar_slot)
            .and_then(Option::as_ref)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DroppedWorldItem {
    pub id: DroppedItemId,
    pub stack: ItemStack,
    pub position: Vec3Net,
    pub yaw: f32,
    #[serde(default)]
    pub rotation: QuatNet,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceNodeState {
    pub id: ResourceNodeId,
    pub definition_id: String,
    pub position: Vec3Net,
    pub yaw: f32,
    pub storage: Vec<ItemStack>,
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
    Kicked {
        reason: String,
    },
    PlayerEvent(PlayerEvent),
    Snapshot(WorldSnapshot),
    Correction(PlayerState),
    Chat(ChatMessage),
    ItemMerged {
        #[serde(deserialize_with = "deserialize_interned_item_id")]
        item_id: crate::items::ItemId,
        quantity: u16,
    },
    Heartbeat,
}

impl ServerMessage {
    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Welcome { .. }
            | Self::AuthRejected { .. }
            | Self::Kicked { .. }
            | Self::PlayerEvent(_)
            | Self::Chat(_)
            | Self::ItemMerged { .. } => PacketDelivery::Reliable,
            Self::Snapshot(_) | Self::Correction(_) | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PacketDelivery {
    Unreliable,
    Reliable,
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
    pub dropped_items: Vec<DroppedWorldItem>,
    #[serde(default)]
    pub resource_nodes: Vec<ResourceNodeState>,
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
    /// Only populated for the receiving client. Peer entries omit the
    /// inventory to keep snapshots small (49 slots × N players × 20 Hz
    /// adds up fast) and to avoid leaking other players' contents.
    #[serde(default)]
    pub inventory: Option<PlayerInventoryState>,
}

impl PlayerState {
    pub fn inventory(&self) -> Option<&PlayerInventoryState> {
        self.inventory.as_ref()
    }
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
    fn message_delivery_maps_network_channels() {
        assert_eq!(
            ClientMessage::Heartbeat.delivery(),
            PacketDelivery::Unreliable
        );
        assert_eq!(
            ClientMessage::Chat {
                text: "hello".to_owned(),
            }
            .delivery(),
            PacketDelivery::Reliable
        );
        assert_eq!(
            ServerMessage::Heartbeat.delivery(),
            PacketDelivery::Unreliable
        );
        assert_eq!(
            ServerMessage::Kicked {
                reason: "restart".to_owned(),
            }
            .delivery(),
            PacketDelivery::Reliable
        );
    }
}
