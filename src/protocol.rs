use bevy::prelude::{Reflect, Vec3};
use serde::{Deserialize, Serialize};

use crate::{
    world::{MapType, WorldData},
    world_time::WorldTimeSnapshot,
};

pub type ClientId = u64;
pub type SteamId = u64;

pub const PROTOCOL_VERSION: u32 = 20;
pub const GAME_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SERVER_TICK_RATE_HZ: f32 = 20.0;
pub const MAX_CHAT_LEN: usize = 240;
pub const MAX_HEALTH: f32 = 100.0;
/// How long a chat bubble floats above a player after they send a chat
/// message. Long enough to read a sentence at a glance, short enough that
/// idle chatter doesn't permanently clutter the world.
pub const CHAT_BUBBLE_DURATION_SECONDS: f32 = 6.0;
pub const INVENTORY_SLOT_COUNT: usize = 40;
pub const ACTIONBAR_SLOT_COUNT: usize = 9;

/// Sample rate the voice pipeline encodes/decodes at end-to-end. 48 kHz is
/// the only rate libopus supports natively without resampling at its highest
/// quality tier, so we standardise both sides on it.
pub const VOICE_SAMPLE_RATE_HZ: u32 = 48_000;
/// Number of audio samples in one Opus frame. 960 samples @ 48 kHz = 20 ms,
/// which is the standard VoIP frame length — long enough to keep the codec
/// overhead reasonable, short enough to keep mouth-to-ear latency under the
/// audible-glass-cliff threshold.
pub const VOICE_FRAME_SAMPLES: usize = 960;
/// Hard cap on the encoded Opus payload, well above the ~120 byte high-water
/// mark for the bit-rates we target. Defends the snapshot/voice mux against
/// a misbehaving (or malicious) client trying to flood the wire.
pub const MAX_VOICE_FRAME_BYTES: usize = 512;

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

impl From<Vec3Net> for Vec3 {
    fn from(value: Vec3Net) -> Self {
        Vec3::new(value.x, value.y, value.z)
    }
}

impl From<Vec3> for Vec3Net {
    fn from(value: Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
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
    /// Server-evaluated slash command. The full text (without the leading
    /// `/`) is shipped verbatim — the server is the source of truth for
    /// parsing, validation, and the admin check.
    Command {
        text: String,
    },
    Inventory(InventoryCommand),
    Gather(ResourceGatherCommand),
    /// Client's view-radius preference (Low/Medium/High). The server uses
    /// this to decide how many concentric chunk rings to include in this
    /// client's per-tick snapshot. Sent on connect and whenever the
    /// player changes the setting in-game.
    SetViewRadius {
        tier: ViewRadiusTier,
    },
    /// One Opus-encoded voice frame. Unreliable — losing a 20 ms frame is
    /// better than waiting for a retransmit. The server routes these to
    /// peers within audible range only.
    Voice(VoiceFrame),
    Heartbeat,
    Disconnect,
}

/// Player-controlled view radius for chunk AoI streaming. Resolved to a
/// concentric chunk-ring size server-side. Living in the protocol layer
/// keeps the wire type free of server-internal details.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ViewRadiusTier {
    Low,
    #[default]
    Medium,
    High,
}

impl ViewRadiusTier {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

impl ClientMessage {
    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Auth { .. }
            | Self::Chat { .. }
            | Self::Command { .. }
            | Self::Inventory(_)
            | Self::Gather(_)
            | Self::SetViewRadius { .. }
            | Self::Disconnect => PacketDelivery::Reliable,
            // Voice frames are each independent (Opus packets carry their own
            // decoder state) so we want every delivered frame played — *not*
            // dropped for being slightly out-of-order, which is what
            // `Sequenced` would do. Movement is the opposite: a newer pose
            // makes an older one obsolete.
            Self::Voice(_) => PacketDelivery::UnreliableUnordered,
            Self::Movement(_) | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

/// One Opus-encoded voice packet. `sequence` lets the receiver drop reordered
/// frames; `frame` holds the raw codec bytes (capped at
/// [`MAX_VOICE_FRAME_BYTES`]). Sample-rate/frame-length are global constants
/// so the wire format only needs to carry the codec payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceFrame {
    pub sequence: u16,
    pub frame: Vec<u8>,
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
    /// Quick-pick a crude (hand-harvestable) resource node — surface
    /// stones, branch piles, grass tufts. Server treats this as an
    /// instant full drain: as much of the node's storage as fits flows
    /// straight into the player's inventory, and the node despawns if
    /// fully emptied. Rejected server-side for non-crude nodes (trees,
    /// ore veins) — those still require a tool swing.
    PickUpResourceNode {
        resource_node_id: ResourceNodeId,
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
    /// `None` when the node is ready to be gathered. `Some(p)` while it's
    /// regenerating after being depleted, with `p` in `0.0..1.0` — the
    /// server ticks this up to 1.0 over the configured respawn window and
    /// then resets storage and clears the flag.
    ///
    /// The client renders nodes with `Some(_)` as translucent ghosts that
    /// fade up to full opacity. Gather attempts are rejected server-side
    /// during this window.
    pub respawn_progress: Option<f32>,
}

/// Per-frame intent emitted by the client controller. Never serialized — the
/// wire format is `PlayerMovement` (the *result* of integrating the input),
/// not the input itself. The simulator reads `time.delta_secs()` for the
/// integration step, so the input carries no time field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerInput {
    pub sequence: u64,
    pub direction: Vec3Net,
    pub run: bool,
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
        world_time: WorldTimeSnapshot,
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
    Toast(ToastMessage),
    /// A remote player landed a successful gather hit. The server sends this
    /// to every client except the swinger (whose client already triggered
    /// the impact locally via prediction) so all nearby players hear and
    /// see the same hit feedback. Position is the gathered node's center
    /// so spatial audio attenuates naturally with distance.
    ResourceImpact {
        position: Vec3Net,
        kind: ResourceImpactKind,
    },
    /// A resource node was actually depleted (storage drained, node
    /// removed) — distinct from "the node just left this player's
    /// AoI". The client uses this to decide whether a node disappearing
    /// from the snapshot deserves a death animation (tree felling, ore
    /// shatter, crude pickup burst, …). Without this signal, every
    /// chunk-boundary crossing animated the death of every node leaving
    /// the player's view ring.
    ResourceNodeDepleted {
        id: ResourceNodeId,
    },
    /// Authoritative day/night clock. Sent every ~60 s as a routine drift
    /// realignment, and immediately after an admin command changes the
    /// clock or speed. Clients integrate locally between broadcasts using
    /// the same multiplier, so the visible cycle stays smooth.
    WorldTime(WorldTimeSnapshot),
    /// A voice frame forwarded from `speaker` after the server confirmed
    /// the listener is within audible range. The position is the speaker's
    /// authoritative position at send time so the client can apply spatial
    /// gain even when its last `Snapshot` is a few frames stale.
    Voice {
        speaker: ClientId,
        sequence: u16,
        position: Vec3Net,
        frame: Vec<u8>,
    },
    /// Server-side perf stats payload, broadcast on a slow tick (~1 Hz)
    /// when the perf HUD is being shown. The client uses this for the
    /// overlay panel; it doesn't affect gameplay.
    PerfStats(PerfStatsSnapshot),
    Heartbeat,
}

/// Snapshot of server-side perf counters relevant to the chunk system —
/// loaded chunks, live nodes, scheduled regrows, plus the requesting
/// player's classification and AoI count.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PerfStatsSnapshot {
    pub loaded_chunks: u32,
    pub live_nodes: u32,
    pub pending_regrows: u32,
    pub aoi_visible_nodes: u32,
    pub player_chunk_x: i32,
    pub player_chunk_z: i32,
    pub player_classification: PerfClassificationId,
}

/// Wire-friendly enum mirror of [`crate::world::ChunkClassification`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PerfClassificationId {
    Forest,
    RockyOutcrop,
    OreVein,
    Plains,
    Mixed,
    /// Player isn't inside a loaded chunk (off-world / between chunks).
    None,
}

impl PerfClassificationId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Forest => "Forest",
            Self::RockyOutcrop => "Rocky outcrop",
            Self::OreVein => "Ore vein",
            Self::Plains => "Plains",
            Self::Mixed => "Mixed",
            Self::None => "—",
        }
    }
}

/// Which class of resource a `ResourceImpact` was produced on. The
/// client derives both the visual particle effect and the impact-audio
/// `(tool, surface)` pair from this. New ore types should each get their
/// own variant so they can have distinct audio without a follow-up
/// protocol change.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceImpactKind {
    Tree,
    CoalOre,
    IronOre,
    SulfurOre,
    /// Bare-rock vein — same stone-shard burst as the ore variants but
    /// the audio cue routes through the plain-stone surface instead of
    /// an ore-specific one.
    StoneVein,
    /// Crude wood material (branch pile). Lighter wood chip burst than a
    /// felled tree.
    Branches,
    /// Crude stone material (surface rock). Lighter stone shard burst
    /// than an ore vein.
    SurfaceStone,
    /// Plant fibres (hay tuft). Soft thud, no particle burst yet.
    HayGrass,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToastMessage {
    pub kind: ToastKind,
    pub text: String,
}

impl ToastMessage {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

impl ServerMessage {
    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Welcome { .. }
            | Self::AuthRejected { .. }
            | Self::Kicked { .. }
            | Self::PlayerEvent(_)
            | Self::Chat(_)
            | Self::ItemMerged { .. }
            | Self::ResourceNodeDepleted { .. }
            | Self::Toast(_) => PacketDelivery::Reliable,
            // Impact effects are pure cosmetic feedback. Dropping one is
            // far less bad than the extra latency of a reliable resend,
            // and the next swing will queue another regardless.
            // See the matching comment on `ClientMessage::delivery` —
            // voice rides an unordered unreliable channel so every
            // delivered frame is played even if it arrives out of order.
            Self::Voice { .. } => PacketDelivery::UnreliableUnordered,
            Self::Snapshot(_)
            | Self::Correction(_)
            | Self::ResourceImpact { .. }
            | Self::WorldTime(_)
            | Self::PerfStats(_)
            | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PacketDelivery {
    /// Sequenced-unreliable: drop older-than-newest. Right for state where
    /// only the latest value matters (movement, snapshots, world time).
    Unreliable,
    /// Reliable-ordered.
    Reliable,
    /// Unordered-unreliable: deliver every packet that survives the link in
    /// whatever order they arrive. Right for streams where each packet is
    /// independent — most notably voice frames, where dropping a frame
    /// because it arrived a few milliseconds late produces audible holes.
    UnreliableUnordered,
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
    /// Most recent in-world chat line, while it's still floating above the
    /// player. Cleared server-side after [`CHAT_BUBBLE_DURATION_SECONDS`].
    /// Populated on every snapshot entry — even peers — so remote players
    /// can render speech bubbles above each other's heads.
    #[serde(default)]
    pub chat_bubble: Option<String>,
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
