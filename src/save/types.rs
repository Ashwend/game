use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    items::DeployableKind,
    protocol::{
        AccountId, ClientId, DeployedEntityId, DroppedItemId, DroppedWorldItem, ItemStack,
        PlayerInventoryState, ResourceNodeId, ResourceNodeState, Vec3Net,
    },
    server::ChunkManagerSave,
    world::MapType,
    world_time::{DEFAULT_START_SECONDS, WorldTime},
};

use super::validate::normalize_world_name;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldSave {
    pub id: Uuid,
    pub name: String,
    pub map: MapType,
    pub created_at_unix: u64,
    pub admins: Vec<AccountId>,
    pub state: WorldStateSave,
}

impl WorldSave {
    pub fn new(name: &str, owner_account_id: Option<AccountId>) -> Self {
        Self::new_with_map(name, owner_account_id, MapType::default())
    }

    pub fn new_with_map(name: &str, owner_account_id: Option<AccountId>, map: MapType) -> Self {
        let id = Uuid::new_v4();
        let mut admins = Vec::new();
        if let Some(owner_account_id) = owner_account_id {
            admins.push(owner_account_id);
        }

        Self {
            id,
            name: normalize_world_name(name),
            map,
            created_at_unix: now_unix(),
            admins,
            state: WorldStateSave::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldStateSave {
    pub last_authoritative_tick: u64,
    pub players: Vec<PersistedPlayer>,
    pub dropped_items: Vec<DroppedWorldItem>,
    /// `None` while the world has never been hosted; once a server runs, this
    /// is always `Some` (even if empty) so harvested resources don't respawn.
    pub resource_nodes: Option<Vec<ResourceNodeState>>,
    /// Chunk manager state, per-chunk capacity tracking + pending fresh-position
    /// regrows. `None` for brand-new worlds (the server boots a fresh manager
    /// from the seed).
    #[serde(default)]
    pub chunk_manager: Option<ChunkManagerSave>,
    #[serde(default = "default_next_id")]
    pub next_dropped_item_id: DroppedItemId,
    #[serde(default = "default_next_id")]
    pub next_client_id: ClientId,
    /// Monotonic counter for admin-spawned resource nodes. World-authored
    /// nodes use their own static IDs from `WorldData::resource_nodes`; this
    /// counter starts well above them so the two ID spaces don't collide.
    #[serde(default = "default_next_resource_node_id")]
    pub next_resource_node_id: ResourceNodeId,
    /// Persisted day/night clock, wall-clock seconds within the in-game
    /// day. Reload picks up wherever the last session left off so the world
    /// doesn't jump back to morning every restart.
    #[serde(default = "default_world_time_seconds")]
    pub world_time_seconds_of_day: f32,
    /// Persisted day/night multiplier. Admins can change it via the
    /// `/speed` command; the value survives a save round-trip.
    #[serde(default = "default_world_time_multiplier")]
    pub world_time_multiplier: f32,
    /// Structures placed in the world (workbenches, furnaces, …). Each
    /// entry carries the position, kind, current health, and the item-id
    /// it was placed from so the client can pick the right mesh on load.
    #[serde(default)]
    pub deployed_entities: Vec<PersistedDeployedEntity>,
    /// Monotonic counter for placed-entity ids.
    #[serde(default = "default_next_id")]
    pub next_deployed_entity_id: DeployedEntityId,
}

impl Default for WorldStateSave {
    fn default() -> Self {
        Self {
            last_authoritative_tick: 0,
            players: Vec::new(),
            dropped_items: Vec::new(),
            resource_nodes: None,
            chunk_manager: None,
            next_dropped_item_id: default_next_id(),
            next_client_id: default_next_id(),
            next_resource_node_id: default_next_resource_node_id(),
            world_time_seconds_of_day: default_world_time_seconds(),
            world_time_multiplier: default_world_time_multiplier(),
            deployed_entities: Vec::new(),
            next_deployed_entity_id: default_next_id(),
        }
    }
}

/// On-disk shape of a placed structure. We persist the item id (so the
/// client picks the right mesh on load even if `DeployableKind` ever
/// grows new variants) plus the wire kind so legacy items still load if
/// the registry is reshuffled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedDeployedEntity {
    pub id: DeployedEntityId,
    pub item_id: String,
    pub kind: DeployableKind,
    pub position: Vec3Net,
    pub yaw: f32,
    pub health: u32,
    pub max_health: u32,
    /// account id of the player who placed this entity, or `None` for
    /// world-spawned structures. Persisted so ownership survives reloads.
    pub owner: Option<crate::protocol::AccountId>,
    /// Furnace-only sub-state. `None` for kinds that aren't furnaces
    /// (workbench). Keeps the per-kind shape out of the top-level
    /// struct so adding more deployable types later doesn't grow it.
    pub furnace: Option<PersistedFurnaceState>,
}

/// Persisted furnace state, fuel slot + item slots + active flag +
/// in-flight burn/smelt timers. Reloading restores these so a player
/// who shuts the host down mid-smelt picks up where they left off.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedFurnaceState {
    pub fuel: Option<ItemStack>,
    pub items: Vec<Option<ItemStack>>,
    pub active: bool,
    pub fuel_burn_ticks_left: u32,
    pub smelt_progress_ticks: u32,
}

impl WorldStateSave {
    pub fn world_time(&self) -> WorldTime {
        let mut time = WorldTime {
            seconds_of_day: self.world_time_seconds_of_day,
            multiplier: self.world_time_multiplier,
        };
        // Re-clamp on load, a save edited by hand or produced by a future
        // version we tolerate-via-default could carry a value outside
        // the safe range. Cheaper to fix once on load than on every tick.
        time.set_seconds(time.seconds_of_day);
        time.set_multiplier(time.multiplier);
        time
    }
}

fn default_next_id() -> u64 {
    1
}

/// Bottom of the admin-spawned resource node ID range. The test world reserves
/// the small integers (1..=72 at last count); starting the counter at 10_000
/// keeps the two ID spaces disjoint without us having to remember to bump it
/// every time a new hand-authored node is added.
const ADMIN_SPAWN_NODE_ID_BASE: ResourceNodeId = 10_000;

fn default_next_resource_node_id() -> ResourceNodeId {
    ADMIN_SPAWN_NODE_ID_BASE
}

fn default_world_time_seconds() -> f32 {
    DEFAULT_START_SECONDS
}

fn default_world_time_multiplier() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedPlayer {
    pub account_id: AccountId,
    pub name: String,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub grounded: bool,
    pub last_processed_input: u64,
    pub is_admin: bool,
    pub inventory: PlayerInventoryState,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::world_time::{MAX_MULTIPLIER, SECONDS_PER_DAY};

    use super::*;

    #[test]
    fn world_time_reclamps_out_of_range_persisted_values_on_load() {
        // A hand-edited / future-tolerated save could carry values well
        // outside the safe range; `world_time()` re-clamps once on load.
        // Negative seconds-of-day must wrap forward into the valid day range.
        let state = WorldStateSave {
            world_time_multiplier: 10_000.0,
            world_time_seconds_of_day: -100.0,
            ..Default::default()
        };

        let time = state.world_time();

        // Multiplier clamps to the hardcoded ceiling.
        assert_eq!(time.multiplier, MAX_MULTIPLIER);
        // Seconds-of-day wraps into `[0, SECONDS_PER_DAY)` via rem_euclid:
        // -100 wraps to SECONDS_PER_DAY - 100.
        assert!((0.0..SECONDS_PER_DAY).contains(&time.seconds_of_day));
        assert!((time.seconds_of_day - (SECONDS_PER_DAY - 100.0)).abs() < 0.01);
    }

    #[test]
    fn world_time_wraps_seconds_above_one_day() {
        let state = WorldStateSave {
            world_time_multiplier: 1.0,
            world_time_seconds_of_day: SECONDS_PER_DAY + 50.0,
            ..Default::default()
        };

        let time = state.world_time();

        assert!((0.0..SECONDS_PER_DAY).contains(&time.seconds_of_day));
        assert!((time.seconds_of_day - 50.0).abs() < 0.01);
    }
}
