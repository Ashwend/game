use std::collections::{HashMap, HashSet};

use crate::{
    auth::AuthMode,
    controller::{BlockGrid, PlayerController},
    protocol::{
        AccountId, CHAT_BUBBLE_DURATION_SECONDS, ClientId, CraftingJobId, DeployedEntityId,
        DroppedItemId, PlayerCraftingState, PlayerInventoryState, ResourceNodeId,
        ResourceNodeState, SERVER_TICK_RATE_HZ, ServerMessage,
    },
    save::{PersistedPlayer, WorldSave},
    world::WorldData,
    world_time::WorldTime,
};

// Seconds of silence (no Heartbeat) before a live client is swept to sleep.
// The client heartbeats once a second on the reliable channel, so three
// missed beats means the link is genuinely gone, not just lossy. Kept short so
// a disconnect (a quit that didn't send a clean `Disconnect`, or a crash)
// propagates to peers as a sleeping body within a few seconds instead of
// waiting on the much longer netcode token timeout.
const CLIENT_STALE_TIMEOUT_TICKS: u64 = 20 * 3;

/// How many ticks a chat bubble floats above the speaker before the
/// server clears it from the replicated `PlayerPublic.chat_bubble`
/// field. Derived from [`CHAT_BUBBLE_DURATION_SECONDS`] so the visible
/// lifetime is the same regardless of tick rate.
const CHAT_BUBBLE_DURATION_TICKS: u64 = (CHAT_BUBBLE_DURATION_SECONDS * SERVER_TICK_RATE_HZ) as u64;

/// Cadence of the routine [`ServerMessage::WorldTime`] broadcast. One per
/// real minute keeps clients aligned against drift without flooding the
/// wire, the client integrates locally between broadcasts using the
/// same multiplier, so the visible cycle stays smooth in between.
const WORLD_TIME_BROADCAST_INTERVAL_TICKS: u64 = (SERVER_TICK_RATE_HZ as u64) * 60;

/// Cadence of the routine [`ServerMessage::PerfStats`] broadcast, one
/// per second. The HUD never needs sub-second resolution and the
/// payload is tiny, so 1 Hz keeps bandwidth negligible.
const PERF_STATS_BROADCAST_INTERVAL_TICKS: u64 = SERVER_TICK_RATE_HZ as u64;

/// Cadence of the routine [`ServerMessage::PlayerList`] roster broadcast, one
/// per second. The pause-screen list never needs faster updates than the ping
/// values themselves change, and the payload is small (a name + ping per
/// connected player).
const PLAYER_LIST_BROADCAST_INTERVAL_TICKS: u64 = SERVER_TICK_RATE_HZ as u64;

/// Cadence of the routine world auto-save (dedicated servers only). Thirty
/// minutes bounds worst-case progress loss on a crash without thrashing the
/// disk or hitching play too often. `pub(crate)` so the host wiring can pass it
/// to [`GameServer::with_auto_save`].
pub(crate) const AUTO_SAVE_INTERVAL_TICKS: u64 = (SERVER_TICK_RATE_HZ as u64) * 60 * 30;

/// How far ahead of an auto-save the "saving soon" heads-up is announced, so
/// players can brace for the brief hitch the synchronous write causes.
const AUTO_SAVE_WARNING_TICKS: u64 = (SERVER_TICK_RATE_HZ as u64) * 30;

pub mod chunk_manager;
mod combat;
mod commands;
mod connection;
mod crafting;
pub mod deployable_ecs;
mod deployables;
mod dispatch;
pub mod dropped_item_ecs;
mod dropped_items;
mod furnace;
mod inventory;
mod lifecycle;
pub mod loot_bag;
pub mod loot_bag_ecs;
pub(crate) mod movement;
mod persistence;
pub mod player_ecs;
mod queries;
pub mod resource_node_ecs;
mod resource_nodes;
mod tick;
mod toasts;
mod voice;
mod world_time;

pub use chunk_manager::{ChunkManager, ChunkManagerSave, view_tier_radius};
pub use connection::VersionMismatchRejection;
pub use deployable_ecs::{
    Deployable, DeployableActive, DeployableChunk, DeployableHealth, DeployableIndex,
    DeployableTransform, DeployableView, despawn_deployable_entity, spawn_deployable_entity,
};
pub use dropped_item_ecs::{
    DroppedItem, DroppedItemChunk, DroppedItemIndex, DroppedItemTransform,
    despawn_dropped_item_entity, spawn_dropped_item_entity,
};
pub use loot_bag_ecs::{
    LootBag as LootBagEntity, LootBagChunk, LootBagContents, LootBagIndex, LootBagTransform,
    LootBagView, despawn_loot_bag_entity, spawn_loot_bag_entity,
};
pub use player_ecs::{
    Player, PlayerArmor, PlayerChunk, PlayerIndex, PlayerLifecycle, PlayerPrivate, PlayerPublic,
    PlayerSleeping, PlayerView, despawn_player_entity, spawn_player_entity,
};
pub use resource_node_ecs::{
    ResourceNode, ResourceNodeChunk, ResourceNodeIndex, ResourceNodeStorage,
    despawn_resource_node_entity, spawn_resource_node_entity,
};
pub use voice::VOICE_AUDIBLE_RANGE;

use self::dropped_items::{DroppedItemBody, DroppedItemPhysics};
// Re-exported into the module namespace only for the in-tree tests, which
// reach these tick-cadence constants through `tests::*`'s `use super::*`.
// Production code references them from `self::tick`, not here.
#[cfg(test)]
use self::dropped_items::{DROPPED_ITEM_CLEANUP_INTERVAL_TICKS, DROPPED_ITEM_MERGE_INTERVAL_TICKS};

#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub auth_mode: AuthMode,
    pub singleplayer_host: Option<AccountId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryTarget {
    Client(ClientId),
    Broadcast,
    /// Send to every connected client except the named one. Used for
    /// "echo to peers" payloads (e.g. impact effects) where the originating
    /// client already produced the effect locally via prediction and a
    /// second copy from the server would double-trigger it.
    BroadcastExcept(ClientId),
    /// Tear down the underlying transport session for this client. Emitted
    /// after a server-initiated `disconnect()` so the host layer can insert
    /// Lightyear's `Disconnecting` component and clear its connection map.
    /// Without this, kicked or stale clients hold their entity until the
    /// netcode timeout, and reconnects would be rejected as "already
    /// connected". The `message` field on the carrying envelope is ignored.
    Disconnect(ClientId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerEnvelope {
    pub target: DeliveryTarget,
    pub message: ServerMessage,
}

#[derive(Debug)]
pub struct GameServer {
    save: WorldSave,
    world: WorldData,
    /// Spatial index over `world.blocks`. Built once at construction. Movement
    /// Coarse block AABB index used by server-side line-of-sight checks
    /// (currently the combat LOS gate in `combat::line_of_sight_clear`).
    /// Movement is client-authoritative so the server doesn't simulate
    /// against it, but the data is here for any future server-side
    /// collision or validation check.
    world_grid: BlockGrid,
    settings: ServerSettings,
    /// WorkOS access-token verifier, present only on a dedicated server run in
    /// [`AuthMode::Workos`]. Loopback (singleplayer) and `Test` runs leave it
    /// `None`; attached via [`GameServer::with_workos`] on the dedicated path.
    workos: Option<std::sync::Arc<crate::auth::WorkosVerifier>>,
    clients: HashMap<ClientId, ServerClient>,
    account_to_client: HashMap<AccountId, ClientId>,
    /// Players who have ever been seen on this server, keyed by account ID. A
    /// disconnect or shutdown writes back into this map so a returning player
    /// picks up their inventory, position, and admin status.
    persisted_players: HashMap<AccountId, PersistedPlayer>,
    dropped_items: HashMap<DroppedItemId, DroppedItemBody>,
    dropped_item_physics: DroppedItemPhysics,
    /// Authoritative live resource node state, keyed by id. A sync system
    /// (`sync_resource_node_entities`) mirrors this map into ECS entities
    /// in the Lightyear server `World` so per-entity replication (Phase 4)
    /// can attach `Replicate` to them without flipping ownership in one
    /// big change. Future cleanup will fold this map into the entities
    /// themselves once Lightyear replication is proven.
    resource_nodes: HashMap<ResourceNodeId, ResourceNodeState>,
    /// Incremental mirror-sync bookkeeping. `sync_resource_node_entities`
    /// (in `net::host`) used to walk *every* live node each tick to reconcile
    /// the replicated ECS mirror, O(live nodes), which at tens of thousands
    /// of nodes cost ~100ms/tick. Instead, mutations to `resource_nodes` record
    /// the affected id here and the sync processes only the delta. `dirty` =
    /// added or storage-changed (re-spawn or update the mirror entity);
    /// `removed` = gone (despawn the mirror entity). Both are drained by the
    /// sync each tick. All `resource_nodes` mutations MUST go through the
    /// `insert_resource_node` / `remove_resource_node` / `resource_node_state_mut`
    /// helpers so nothing is missed (stale replication otherwise).
    node_sync_dirty: HashSet<ResourceNodeId>,
    node_sync_removed: HashSet<ResourceNodeId>,
    /// Server-authoritative chunk system. Owns per-chunk capacity, AoI
    /// visibility, and the fresh-position regrow scheduler.
    pub(crate) chunk_manager: ChunkManager,
    /// Placed structures (workbench, furnace, …) keyed by id. Anchor
    /// chunks are owned by `chunk_manager` so AoI filtering matches the
    /// same pipeline as resource nodes and dropped items.
    pub(super) deployed_entities: HashMap<DeployedEntityId, deployables::DeployedEntity>,
    /// Death-loot containers, keyed by id. Spawned by the PvP kill
    /// chain in `combat.rs`; despawned when emptied + closed by every
    /// looker. Anchor chunks tracked via `chunk_manager` so the
    /// existing AoI/replication pipeline picks them up.
    pub(super) loot_bags: HashMap<crate::protocol::LootBagId, loot_bag::LootBag>,
    next_dropped_item_id: DroppedItemId,
    next_client_id: ClientId,
    next_resource_node_id: ResourceNodeId,
    next_deployed_entity_id: DeployedEntityId,
    next_loot_bag_id: crate::protocol::LootBagId,
    tick: u64,
    /// Authoritative day/night clock. Mirrored to clients via
    /// [`ServerMessage::WorldTime`]. Persisted to the save in `world_save`.
    world_time: WorldTime,
    /// Last tick a routine `WorldTime` broadcast was sent. Lets admin
    /// commands push an out-of-band immediate snapshot and reset this
    /// counter so the next routine broadcast is a full interval later.
    last_world_time_broadcast_tick: u64,
    /// Auto-save cadence in ticks. `0` disables it (loopback singleplayer, which
    /// saves on exit instead); dedicated hosts set it via
    /// [`GameServer::with_auto_save`]. `tick` only schedules and announces; the
    /// host performs the disk write so I/O stays out of the game-state module.
    auto_save_interval_ticks: u64,
    /// Tick of the last auto-save (or host start), the schedule counts from here.
    last_auto_save_tick: u64,
    /// Raised by `tick` when an auto-save comes due; the host drains it via
    /// [`GameServer::take_auto_save_pending`], writes the world, then announces.
    auto_save_pending: bool,
}

#[derive(Debug)]
pub(super) struct ServerClient {
    pub(super) client_id: ClientId,
    pub(super) account_id: AccountId,
    pub(super) name: String,
    /// Whether a live network connection is currently driving this body.
    /// `false` means the player logged out and their body stays in the world
    /// as a "sleeping" body (Rust-style): still replicated, lootable, and
    /// killable, but frozen and excluded from the online roster / stale-timeout.
    /// A reconnect from the same account wakes the body in place.
    pub(super) online: bool,
    pub(super) controller: PlayerController,
    pub(super) inventory: PlayerInventoryState,
    /// Authoritative damage reduction (0–100, percent). Today always
    /// `0`, armor items don't exist yet, but kept on the client so
    /// the damage path doesn't have to special-case the missing field.
    /// Replicated to every peer via the [`PlayerArmor`] component
    /// attached to the mirror entity.
    pub(super) armor: u8,
    pub(super) is_admin: bool,
    pub(super) last_seen_tick: u64,
    pub(super) next_gather_tick: u64,
    /// Separate cooldown for PvP swings so a melee combo can't piggyback
    /// onto a fresh gather tick (and vice versa). Same cadence as the
    /// tool's per-swing cooldown; the cooldown is set on every accepted
    /// `AttackPlayer` after damage lands.
    pub(super) next_attack_tick: u64,
    /// Authoritative life state. `Alive` while the player is up and
    /// running, `Dead { … }` between HP-hits-zero and the respawn
    /// request. Dropped inputs and attack rejections gate on this so
    /// a corpse can't move, swing, or eat damage twice.
    pub(super) lifecycle: PlayerLifecycle,
    /// Most recent chat line + the tick it stops being broadcast. Empty
    /// outside the bubble window. Snapshots copy `text` so peer clients can
    /// render speech bubbles above the speaker's head.
    pub(super) chat_bubble: Option<ChatBubble>,
    /// AoI view radius requested by this client. Snapshot construction
    /// uses this to pick how many concentric chunk rings of resource nodes
    /// the client receives.
    pub(super) view_tier: crate::protocol::ViewRadiusTier,
    /// Active crafting queue. Inputs already debited; outputs pending.
    /// Snapshots send a clone of this to the owning client only.
    pub(super) crafting: PlayerCraftingState,
    /// Next id handed out for [`crafting::jobs`]. Wraps after 2^64 jobs,
    /// which won't happen, it's a u64 so the wrap is harmless even if
    /// the player runs a crafting macro for years.
    pub(super) next_craft_job_id: CraftingJobId,
    /// The furnace the player currently has open, if any. Only one
    /// open at a time, opening a new furnace closes the previous.
    /// Cleared on disconnect.
    pub(super) open_furnace: Option<DeployedEntityId>,
    /// The loot container (a world bag or a sleeper's live inventory) the
    /// player currently has open, if any. Same "one open container at a time"
    /// rule as furnaces, opening one closes any previously-open container.
    /// Cleared on disconnect.
    pub(super) open_container: Option<loot_bag::OpenContainer>,
    /// Highest optimistic-prediction action sequence processed for this client
    /// (advanced for accepted *and* rejected predicted commands). Mirrored into
    /// `PlayerPrivate::applied_action_seq` for the client's reconcile pass.
    pub(super) applied_action_seq: u32,
    /// Most recent round-trip latency this client reported via
    /// [`crate::protocol::ClientMessage::Ping`], in milliseconds. Surfaced to
    /// every client in the roster broadcast so the pause-screen player list can
    /// show each player's ping.
    pub(super) ping_ms: u16,
}

#[derive(Debug, Clone)]
pub(super) struct ChatBubble {
    pub(super) text: String,
    pub(super) expires_tick: u64,
}

pub(super) fn persisted_player_from(client: &ServerClient) -> PersistedPlayer {
    PersistedPlayer {
        account_id: client.account_id,
        name: client.name.clone(),
        position: client.controller.position,
        velocity: client.controller.velocity,
        yaw: client.controller.yaw,
        pitch: client.controller.pitch,
        health: client.controller.health,
        grounded: client.controller.grounded,
        last_processed_input: client.controller.last_processed_input,
        is_admin: client.is_admin,
        inventory: client.inventory.clone(),
    }
}

#[cfg(test)]
mod tests;
