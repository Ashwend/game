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
    /// Respawn at one of the caller's own sleeping bags. Same lifecycle
    /// gate as [`Self::Respawn`]; additionally rejected when the bag is
    /// gone or belongs to someone else (falls back to nothing, the
    /// client can still pick the random respawn).
    RespawnAtBag {
        id: DeployedEntityId,
    },
    /// Place a building block via the building plan. The server is the
    /// authority on snapping, costs, and overlap; see
    /// [`PlaceBuildingCommand`].
    PlaceBuilding(PlaceBuildingCommand),
    /// Hammer repair / upgrade / demolish on a building block.
    Building(BuildingCommand),
    /// Door placement, interaction, and code-lock management.
    Door(DoorCommand),
    /// Sleeping-bag rename + pickup.
    SleepingBag(SleepingBagCommand),
    /// Open / close / move-items in a loot bag (the container spawned
    /// at a dead player's feet). Server keeps the authoritative slots
    /// and gates the move on the player having the bag open.
    LootBag(LootBagCommand),
    /// Loot a logged-out sleeping body. The server spills the sleeper's
    /// inventory into a fresh loot bag at their feet and opens it for the
    /// looter, so the rest of the transfer flows through the normal loot-bag
    /// path. Validated server-side: the target must be a sleeper in range.
    LootSleeper {
        client_id: ClientId,
    },
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
    /// Lightweight RTT probe. `client_time_ms` is the client's monotonic send
    /// timestamp, which the server echoes back in [`ServerMessage::Pong`] so the
    /// client can measure round-trip latency. `rtt_ms` is the client's most
    /// recently measured RTT, reported so the server can surface every player's
    /// ping in the roster ([`ServerMessage::PlayerList`]).
    Ping {
        client_time_ms: u32,
        rtt_ms: u16,
    },
    Disconnect,
    /// Open a placed storage box's container UI. The server validates
    /// kind + range and replies by populating
    /// `PlayerPrivate.open_loot_bag` (the shared container view);
    /// close/move/quick-transfer then ride [`LootBagCommand`] like any
    /// other open container.
    OpenStorageBox {
        id: DeployedEntityId,
    },
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
            | Self::RespawnAtBag { .. }
            | Self::PlaceBuilding(_)
            | Self::Building(_)
            | Self::Door(_)
            | Self::SleepingBag(_)
            | Self::LootBag(_)
            | Self::LootSleeper { .. }
            | Self::OpenStorageBox { .. }
            | Self::SetViewRadius { .. }
            // Heartbeat is the server's liveness signal: it drives the
            // stale-client sweep. Sending it reliably means a single dropped
            // packet can't look like a vanished client, so the timeout can be
            // tightened without false-disconnecting players on a lossy link.
            | Self::Heartbeat
            | Self::Disconnect => PacketDelivery::Reliable,
            // Voice frames are each independent (Opus packets carry their own
            // decoder state) so we want every delivered frame played, *not*
            // dropped for being slightly out-of-order, which is what
            // `Sequenced` would do. Movement is the opposite: a newer pose
            // makes an older one obsolete.
            Self::Voice(_) => PacketDelivery::UnreliableUnordered,
            Self::Movement(_) | Self::Ping { .. } => PacketDelivery::Unreliable,
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
        /// Attacker's world position at impact time. The target client uses
        /// this to point a damage-direction indicator at the source; peers
        /// ignore it.
        attacker_position: Vec3Net,
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
    /// `respawn_bags` lists the dying player's placed sleeping bags so
    /// the death screen can offer them as spawn points alongside the
    /// random respawn.
    PlayerKilled {
        killer: Option<ClientId>,
        killer_name: Option<String>,
        #[serde(default)]
        respawn_bags: Vec<RespawnBagOption>,
    },
    /// Reply to a [`ClientMessage::Door`] interact from an account that
    /// isn't authorized on the door's code lock: tells the client to
    /// open the code-entry dialog for door `id`.
    DoorCodePrompt {
        id: DeployedEntityId,
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
    /// Server-side perf stats payload, sent to every client on a slow
    /// tick (~1 Hz) regardless of whether the perf HUD is open (the
    /// payload is ~30 B; an opt-in handshake isn't worth the wire
    /// shape). The client uses this for the F2 overlay panel; it
    /// doesn't affect gameplay.
    PerfStats(PerfStatsSnapshot),
    /// Echo of a client [`ClientMessage::Ping`]'s timestamp, letting the client
    /// compute its round-trip latency.
    Pong {
        client_time_ms: u32,
    },
    /// Full connected-player roster (name + measured ping), broadcast on a slow
    /// cadence. Unlike per-entity replication this deliberately includes players
    /// outside the receiver's AoI: the pause-screen player list needs everyone,
    /// not just nearby mirrors, so it rides a small periodic presence message
    /// rather than the chunk-gated replication path.
    PlayerList(Vec<PlayerListEntry>),
    Heartbeat,
    /// Reply to a [`ClientMessage::Door`] code entry: whether the code
    /// was accepted (the account is now authorized) or wrong. Drives the
    /// client's keypad feedback sounds; the toast carries the text.
    DoorCodeResult {
        accepted: bool,
    },
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
            | Self::Knockback { .. }
            | Self::PlayerKilled { .. }
            | Self::DoorCodePrompt { .. }
            | Self::DoorCodeResult { .. }
            | Self::Toast(_) => PacketDelivery::Reliable,
            // Voice rides an unordered unreliable channel so every delivered
            // frame is played even if it arrives out of order. See the
            // matching comment on `ClientMessage::delivery`.
            Self::Voice { .. } => PacketDelivery::UnreliableUnordered,
            // Impact effects (chip bursts, hit audio, floating damage numbers)
            // are pure cosmetic feedback: the authoritative damage already
            // lands via the replicated `PlayerPublic.health`, and the next
            // swing queues another effect regardless. Dropping one is far
            // cheaper than a reliable resend, so both the resource-node and
            // player variants ride the unreliable channel. The
            // gameplay-affecting `Knockback`/`PlayerKilled` stay reliable.
            Self::Correction(_)
            | Self::ResourceImpact { .. }
            | Self::PlayerImpact { .. }
            | Self::WorldTime(_)
            | Self::PerfStats(_)
            | Self::Pong { .. }
            | Self::PlayerList(_)
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

/// One row in the connected-player roster carried by
/// [`ServerMessage::PlayerList`]: who is online and their measured ping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerListEntry {
    pub client_id: ClientId,
    pub name: String,
    /// Round-trip latency in milliseconds, as last reported by that client.
    pub ping_ms: u16,
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
