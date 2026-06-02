//! The two top-level wire messages, [`ClientMessage`] and [`ServerMessage`],
//! plus the small payload enums/structs they carry and their channel-delivery
//! preferences.

use serde::{Deserialize, Serialize};

use crate::{
    world::{MapType, WorldData},
    world_time::WorldTimeSnapshot,
};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientMessage {
    Auth {
        protocol_version: u32,
        #[serde(default)]
        client_version: Option<String>,
        account_id: AccountId,
        display_name: String,
        token: String,
    },
    Movement(PlayerMovement),
    Chat {
        text: String,
    },
    /// Server-evaluated slash command. The full text (without the leading
    /// `/`) is shipped verbatim, the server is the source of truth for
    /// parsing, validation, and the admin check.
    Command {
        text: String,
    },
    Inventory(InventoryCommand),
    Crafting(CraftingCommand),
    Gather(ResourceGatherCommand),
    /// Place the active actionbar deployable at `position` with the given
    /// yaw. Server validates that the player actually holds a stack of
    /// that item, that the placement is in reach, and that nothing
    /// already occupies the footprint. One item is consumed on success.
    PlaceDeployable(PlaceDeployableCommand),
    /// Open/close/operate a furnace the player is standing next to. The
    /// server tracks at most one open furnace per client; opening a new
    /// one auto-closes the previous.
    Furnace(FurnaceCommand),
    /// Damage a placed structure (workbench, furnace, …). Server
    /// validates the active tool, the target's range/cone, and applies
    /// per-tool damage; the structure despawns when health reaches 0.
    DamageDeployable(DamageDeployableCommand),
    /// Swing the equipped tool at another player. Server re-validates
    /// tool, range, view cone, and line-of-sight against the world
    /// blocks; on success it applies armor-reduced damage and sends
    /// `PlayerImpact` + `Knockback`. See `docs/pvp.md`.
    AttackPlayer(AttackPlayerCommand),
    /// Respawn the calling client after death. Rejected unless the
    /// client is currently dead, the server is the authority on the
    /// lifecycle state.
    Respawn,
    /// Open / close / move-items in a loot bag (the container spawned
    /// at a dead player's feet). Server keeps the authoritative slots
    /// and gates the move on the player having the bag open.
    LootBag(LootBagCommand),
    /// Client's view-radius preference (Low/Medium/High). The server uses
    /// this to decide how many concentric chunk rings to include in this
    /// client's per-tick snapshot. Sent on connect and whenever the
    /// player changes the setting in-game.
    SetViewRadius {
        tier: ViewRadiusTier,
    },
    /// One Opus-encoded voice frame. Unreliable, losing a 20 ms frame is
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
            | Self::Crafting(_)
            | Self::Gather(_)
            | Self::PlaceDeployable(_)
            | Self::Furnace(_)
            | Self::DamageDeployable(_)
            | Self::AttackPlayer(_)
            | Self::Respawn
            | Self::LootBag(_)
            | Self::SetViewRadius { .. }
            | Self::Disconnect => PacketDelivery::Reliable,
            // Voice frames are each independent (Opus packets carry their own
            // decoder state) so we want every delivered frame played, *not*
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ServerMessage {
    Welcome {
        client_id: ClientId,
        map: MapType,
        world: WorldData,
        is_admin: bool,
        /// Prediction-seed for the connecting client's local
        /// controller, position, velocity, yaw, pitch, health,
        /// grounded, last_processed_input. Phase 6.6 retired the
        /// full-snapshot Welcome payload; everything else now flows
        /// through Lightyear replication.
        local_seed: PlayerState,
        world_time: WorldTimeSnapshot,
    },
    AuthRejected {
        reason: String,
    },
    /// The server refused the connection because the client's version didn't
    /// match. Carries the *server's* human-readable version and protocol so
    /// the client can pair them with its own compiled-in
    /// [`GAME_VERSION`]/[`PROTOCOL_VERSION`] and tell the player whether
    /// they're newer or older. Sent at the `Auth` handshake instead of a
    /// generic [`Self::AuthRejected`] string. Part of the stable handshake
    /// surface (see [`crate::net::channels::LIGHTYEAR_PROTOCOL_ID`]): keep this
    /// variant and its fields wire-stable so a future server can always tell an
    /// older client why it was turned away.
    VersionMismatch {
        server_version: String,
        server_protocol: u32,
    },
    Kicked {
        reason: String,
    },
    PlayerEvent(PlayerEvent),
    Correction(PlayerState),
    Chat(ChatMessage),
    ItemMerged {
        #[serde(deserialize_with = "super::deserialize_interned_item_id")]
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
    /// A PvP attack landed on a player. Broadcast to every client
    /// except the attacker (the attacker already produced their own
    /// feedback via prediction). Drives the chip burst, hit audio,
    /// floating damage number, and, on the target client only, the
    /// camera-kick hit reaction.
    PlayerImpact {
        attacker: ClientId,
        target: ClientId,
        /// Chest-height world position of the target at impact time,
        /// the visual + audio anchor.
        position: Vec3Net,
        tool: crate::items::ToolKind,
        /// Post-armor damage in HP. Used for the floating damage text.
        damage_dealt: u32,
    },
    /// Knockback impulse sent only to the target of a PvP hit. The
    /// target applies it to its local velocity predictor; a cheater
    /// ignoring this message only forfeits their own pushback.
    Knockback {
        impulse: Vec3Net,
    },
    /// Sent to the dying player when their HP reaches zero. Triggers the
    /// death splash and the respawn UI. `killer_name` is resolved
    /// server-side so the client doesn't have to look it up.
    PlayerKilled {
        killer: Option<ClientId>,
        killer_name: Option<String>,
    },
    /// A resource node was actually depleted (storage drained, node
    /// removed), distinct from "the node just left this player's
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

impl ServerMessage {
    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Welcome { .. }
            | Self::AuthRejected { .. }
            | Self::VersionMismatch { .. }
            | Self::Kicked { .. }
            | Self::PlayerEvent(_)
            | Self::Chat(_)
            | Self::ItemMerged { .. }
            | Self::ResourceNodeDepleted { .. }
            | Self::PlayerImpact { .. }
            | Self::Knockback { .. }
            | Self::PlayerKilled { .. }
            | Self::Toast(_) => PacketDelivery::Reliable,
            // Impact effects are pure cosmetic feedback. Dropping one is
            // far less bad than the extra latency of a reliable resend,
            // and the next swing will queue another regardless.
            // See the matching comment on `ClientMessage::delivery`,
            // voice rides an unordered unreliable channel so every
            // delivered frame is played even if it arrives out of order.
            Self::Voice { .. } => PacketDelivery::UnreliableUnordered,
            Self::Correction(_)
            | Self::ResourceImpact { .. }
            | Self::WorldTime(_)
            | Self::PerfStats(_)
            | Self::Heartbeat => PacketDelivery::Unreliable,
        }
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

/// Snapshot of server-side perf counters relevant to the chunk system,
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
            Self::None => "-",
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
    /// Bare-rock vein, same stone-shard burst as the ore variants but
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
