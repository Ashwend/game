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
    /// Open/close/upgrade a workbench the player is standing next to. Like the
    /// furnace, the server tracks at most one open workbench per client. The
    /// only mutating op is the in-place tier upgrade.
    Workbench(WorkbenchCommand),
    /// Draw / cancel / fire a held ranged weapon (bow, crossbow). The server
    /// validates the weapon and ammo, tracks the draw window, scales the shot's
    /// damage by draw time, consumes one arrow, and spawns a server-simulated
    /// projectile. See [`RangedCommand`] and `docs/pvp-combat.md`.
    Ranged(RangedCommand),
    /// Throw a held explosive (the powder bomb). The server validates the held
    /// item is a thrown explosive, consumes one, and launches a heavier-
    /// ballistics projectile that arms its fuse on coming to rest. Placed
    /// charges (keg, satchel, ember) ride `PlaceDeployable`, not this. See
    /// [`ExplosiveCommand`].
    Explosive(ExplosiveCommand),
    /// Damage a placed structure (workbench, furnace, …). Server
    /// validates the active tool, the target's range/cone, and applies
    /// per-tool damage; the structure despawns when health reaches 0.
    DamageDeployable(DamageDeployableCommand),
    /// Swing the equipped tool at another player. Server re-validates
    /// tool, range, view cone, and line-of-sight against the world
    /// blocks; on success it applies armor-reduced damage and sends
    /// `PlayerImpact` + `Knockback`. See `docs/pvp-combat.md`.
    AttackPlayer(AttackPlayerCommand),
    /// Cosmetic "I started a swing" signal, sent at the moment a local
    /// swing begins (the gather/attack/damage messages above fire at the
    /// impact frame and never on whiffs, so they can't drive a remote
    /// swing animation). The server stamps the swinger's peer-visible
    /// `PlayerAction` from it; peers play the matching third-person swing.
    SwingStart(SwingStartCommand),
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
    /// Tool Cupboard authorize / deauthorize / clear-list (tap-E toggle
    /// and the hold-E wheel). See [`ClaimCommand`].
    Claim(ClaimCommand),
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
    /// Ask the server for the caller's own map markers (e.g. on map open, to
    /// load markers placed in a previous session). Answered with
    /// [`ServerMessage::WorldMapMarkers`]. The terrain image is generated
    /// client-side from the seed, so it isn't part of this. The client
    /// throttles these (cached ~1 min), so this is cheap and rare.
    RequestWorldMap,
    /// Add / rename / remove one of the caller's own map markers. The server
    /// is the authority on the id space, the per-player cap, and persistence;
    /// it answers with [`ServerMessage::WorldMapMarkers`].
    WorldMapMarker(WorldMapMarkerCommand),
    /// Start / cancel using a held consumable (the bandage). The server tracks
    /// the charge on its own clock and applies the heal itself when it completes,
    /// so there is no "apply" variant to forge. See [`ConsumableCommand`].
    Consumable(ConsumableCommand),
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
            | Self::Workbench(_)
            // Ranged draw/cancel/fire are gameplay-affecting intents (a dropped
            // fire would eat the shot the player took), so they ride the reliable
            // channel like the melee attack and swing-start.
            | Self::Ranged(_)
            // A thrown bomb is a one-shot, item-consuming intent (a dropped throw
            // would eat the bomb), so it is reliable like the ranged fire.
            | Self::Explosive(_)
            // A consumable use is item-consuming and its cancel restores the
            // movement slow, so BOTH variants must land. A dropped `UseStart`
            // silently eats the player's press; a dropped `UseCancel` would leave
            // them stuck at walking speed with a charge the server still thinks is
            // running, and it would go on to spend the bandage they let go of.
            | Self::Consumable(_)
            | Self::DamageDeployable(_)
            | Self::AttackPlayer(_)
            // Reliable: a swing-start is tiny (~5 bytes) and infrequent, and
            // both unreliable modes are wrong here. Sequenced-unreliable would
            // let a back-to-back auto-repeat swing clobber its predecessor (a
            // dropped animation), and unordered-unreliable could silently drop
            // the start outright (no swing shown). Reliable guarantees every
            // swing animates on peers.
            | Self::SwingStart(_)
            | Self::Respawn
            | Self::RespawnAtBag { .. }
            | Self::PlaceBuilding(_)
            | Self::Building(_)
            | Self::Door(_)
            | Self::SleepingBag(_)
            | Self::Claim(_)
            | Self::LootBag(_)
            | Self::LootSleeper { .. }
            | Self::OpenStorageBox { .. }
            | Self::RequestWorldMap
            | Self::WorldMapMarker(_)
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
    /// surface (see `crate::net::channels::LIGHTYEAR_PROTOCOL_ID`): keep this
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
        /// The swing's impact identity: the weapon's own archetype
        /// (Club/Spear/Sword) or a gather tool's archetype
        /// (Hatchet/Pickaxe). Drives the peer hit audio, the impact VFX, and
        /// the target's camera reaction, so the feedback matches what actually
        /// landed the hit rather than a generic stand-in.
        model: crate::items::ItemModel,
        /// Post-armor damage in HP. Used for the floating damage text.
        damage_dealt: u32,
    },
    /// A server-simulated projectile struck something (a player, a deployable, or
    /// the world). Fanned out to nearby peers so their client can play the arrow
    /// thunk / stick VFX and audio at the impact point. Purely cosmetic: the
    /// authoritative damage already landed via the replicated `PlayerHealth` /
    /// `DeployableHealth` diff (players also get a `Correction`), so this rides
    /// the unreliable channel like `PlayerImpact`. `model` is the firing weapon's
    /// archetype (Bow/Crossbow); `surface` tells the client which material cue to
    /// play.
    ///
    /// Two delivery shapes share this variant. The peer fan-out (proximity,
    /// excludes the shooter) carries `owner_confirmation = false`. A separate copy
    /// is sent straight to the shooter on a Player or Deployable hit with
    /// `owner_confirmation = true`, so the shooter's own client can raise the
    /// crosshair hit marker (the melee attacker's confirmation) in addition to the
    /// impact cue. A World rest never sends the owner copy: the shooter's client
    /// already produces that cue from the arrow's moving -> stuck transition.
    ProjectileImpact {
        /// World position the projectile came to rest / struck.
        position: Vec3Net,
        /// The firing weapon's archetype (Bow/Crossbow), for the impact audio/VFX.
        model: crate::items::ItemModel,
        /// What the projectile hit, so the client picks the right material cue.
        surface: ProjectileSurface,
        /// True only on the copy delivered to the shooter for a Player/Deployable
        /// hit; drives the crosshair hit marker. Peers (and World rests) see
        /// `false`. `serde(default)` keeps older saves/messages decoding.
        #[serde(default)]
        owner_confirmation: bool,
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
    /// A meteor shower event is live: the announce, carrying EVERY meteor of
    /// the event (4 to 5 size-varied strikes with staggered impact ticks; the
    /// admin `/meteor-here` sends a single-strike list). Broadcast reliably
    /// once at T minus the warning window, and resent (with all still-live
    /// meteors) to any client that connects while the event is alive
    /// (including the post-impact crater windows) so late joiners see the
    /// fireballs / craters immediately. There is deliberately NO global
    /// announcement UI: the client computes the sky show, per-meteor danger
    /// warning, and craters as a deterministic function of this payload plus
    /// its own authoritative-clock estimate, so nothing about a meteor is
    /// per-tick replicated. See `docs/meteor-shower.md` and
    /// `crate::world::meteor_shower`.
    MeteorShower {
        meteors: Vec<MeteorStrike>,
    },
    /// An explosive detonated at `position`. Purely a cosmetic VFX/SFX cue: the
    /// authoritative blast (player damage, structure destruction) already lands
    /// server-side and replicates through the normal player/deployable mirrors,
    /// so a dropped cue only costs one client a flash and a thump. Fanned out to
    /// clients within a generous range so a distant breach is still seen and
    /// heard (the audible-thump-plus-far-rumble feel), with the client scaling
    /// the effect by its own distance. `kind` selects the charge's effect
    /// (a bomb pop vs an ember-charge blast). Consumed by the explosive VFX
    /// package; this package only ships the cue.
    Explosion {
        position: Vec3Net,
        kind: crate::items::ExplosiveKind,
    },
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
    /// The caller's own map markers. Sent both as the reply to
    /// [`ClientMessage::RequestWorldMap`] (on map open) and as a push after a
    /// [`ClientMessage::WorldMapMarker`] mutation. The terrain image is NOT
    /// here, the client generates it locally from the seed; only the
    /// per-account markers need a round trip.
    WorldMapMarkers {
        markers: Vec<WorldMapMarker>,
    },
    /// Cinematic playback phase cue, broadcast at every phase edge while the
    /// admin-started `/cinematic` sequence runs (see `crate::cinematic` for
    /// the shared shot script and `crate::server::cinematic` for the
    /// orchestrator). The client reacts by parking or driving the detached
    /// camera and drawing the countdown slate; all timing beyond the cue edge
    /// itself is derived locally from the shared script, so the wire carries
    /// only the shot index and the phase lengths.
    Cinematic(CinematicCue),
}

/// One phase edge of cinematic playback, the payload of
/// [`ServerMessage::Cinematic`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CinematicCue {
    /// The init phase started: the server is cleaning the world and spawning
    /// the stage (props, dummy actors). The client blanks the HUD and holds.
    Initializing,
    /// The on-screen countdown to `shot_index` started; the camera parks on
    /// the shot's opening frame for the whole slate.
    Countdown { shot_index: u8, seconds: f32 },
    /// Shot `shot_index` is playing; the client drives the camera along the
    /// shot's authored path.
    ShotStarted { shot_index: u8 },
    /// Idle gap after a shot (the camera holds the last frame so post has a
    /// clean cut). `next_shot_index` is `None` after the final shot.
    Intermission {
        next_shot_index: Option<u8>,
        seconds: f32,
    },
    /// Playback finished or was aborted; the client restores the normal
    /// first-person camera, controls, and HUD.
    Stopped,
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
            // Marker edits are rare and must not be dropped (a lost delete
            // would resurrect a pin), so they ride the reliable channel.
            | Self::WorldMapMarkers { .. }
            // The meteor shower announce is a one-shot event that seeds the whole
            // client-side sky show; a dropped announce would leave a player
            // blind to an incoming meteor, so it rides the reliable channel
            // (and is resent on connect for late joiners regardless).
            | Self::MeteorShower { .. }
            // Cinematic cues are rare one-shot phase edges; a dropped cue
            // would desync the local director from the server timeline for
            // the rest of the phase, so they ride the reliable channel.
            | Self::Cinematic(_)
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
            | Self::ProjectileImpact { .. }
            // The explosion cue is pure cosmetic feedback (flash + thump); the
            // authoritative blast already landed via the replicated mirrors, so
            // it rides the unreliable channel like the other impact cues.
            | Self::Explosion { .. }
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

/// One meteor of a shower event, as announced in [`ServerMessage::MeteorShower`].
/// Everything a client needs to derive that meteor's whole presentation
/// (fireball arc, danger warning, crater) plus the shared clock estimate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct MeteorStrike {
    /// Ground-zero world position (y is floor level).
    pub impact_position: Vec3Net,
    /// Server tick this meteor strikes. Staggered per meteor so the event
    /// reads as a shower, not a volley.
    pub impact_tick: u64,
    /// Seeds this meteor's approach azimuth/arc (see
    /// `crate::world::meteor_shower::meteor_world_state`).
    pub trajectory_seed: u64,
    /// Size multiplier in `(0, 1]`: exactly one meteor per event is the 1.0
    /// headliner, the rest roll the secondary band. Scales the blast radius,
    /// ground-zero damage, danger radius, crater geometry, loot count, and
    /// the fireball's visual/audio scale.
    pub size: f32,
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

/// What a projectile struck, carried on [`ServerMessage::ProjectileImpact`] so
/// the client picks the right impact material cue (a flesh thunk vs a wood/stone
/// thock). Deliberately coarse: the three outcomes the projectile sim resolves.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ProjectileSurface {
    /// Hit a player body (the flesh-hit cue).
    Player,
    /// Hit a placed deployable or building piece (a wood/stone structure thock).
    Deployable,
    /// Came to rest against the world (terrain / perimeter wall): the arrow-stick
    /// thock.
    World,
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
