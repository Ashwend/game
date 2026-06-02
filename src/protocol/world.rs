//! Server-internal authoritative world/entity state shapes. Post-Phase-6 these
//! are no longer wire messages, the client receives the equivalent Lightyear
//! components, but the server still keys its in-memory maps and the save layer
//! by these shapes, so they live in the protocol layer. Also here: the
//! prediction-seed `PlayerState` and the movement input/result shapes.

use serde::{Deserialize, Serialize};

use super::*;

/// Server-internal authoritative shape of a dropped item. Post-Phase-6
/// this is no longer a wire type, the client receives `DroppedItem` +
/// `DroppedItemTransform` via Lightyear replication. Still used as the
/// `GameServer::dropped_items` map value and persisted on save.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DroppedWorldItem {
    pub id: DroppedItemId,
    pub stack: ItemStack,
    pub position: Vec3Net,
    pub yaw: f32,
    #[serde(default)]
    pub rotation: QuatNet,
}

/// Server-internal authoritative shape of a placed structure (workbench,
/// furnace, …). Post-Phase-6 this is no longer a wire type, the client
/// receives `Deployable` + `DeployableTransform` + `DeployableHealth` +
/// `DeployableActive` via Lightyear replication. Still used as the
/// `GameServer::deployed_entities` map value and persisted on save.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeployedEntityState {
    pub id: DeployedEntityId,
    #[serde(deserialize_with = "super::deserialize_interned_item_id")]
    pub item_id: crate::items::ItemId,
    pub kind: crate::items::DeployableKind,
    pub position: Vec3Net,
    pub yaw: f32,
    pub health: u32,
    pub max_health: u32,
    /// Public "is it doing work?" flag, for furnaces this drives the
    /// glow/smoke and tells nearby players the structure is on. Always
    /// `false` for kinds that have no active state (workbench).
    #[serde(default)]
    pub active: bool,
}

/// Server-internal authoritative shape of a live resource node. Post-Phase-6
/// this is no longer a wire type, the client receives `ResourceNode` +
/// `ResourceNodeStorage` via Lightyear replication instead. The struct
/// stays here because the server still keys its in-memory map and the
/// persisted save layer by this shape; Phase 1b would eventually fold it
/// into the ECS entities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceNodeState {
    pub id: ResourceNodeId,
    pub definition_id: String,
    pub position: Vec3Net,
    pub yaw: f32,
    pub storage: Vec<ItemStack>,
}

/// Per-frame intent emitted by the client controller. Never serialized, the
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

/// Per-client wire payload used by:
///
/// - `ServerMessage::Welcome.local_seed`, the initial prediction
///   bootstrap (server tells the connecting client where its
///   controller starts).
/// - `ServerMessage::Correction`, server-authoritative correction of
///   a divergent prediction (health rollback today, more fields if
///   prediction grows).
///
/// All other per-player state moved off the wire to Lightyear
/// replication (`PlayerPublic` / `PlayerPrivate`) during the Phase 6
/// migration; this struct is now strictly a prediction-seed shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerState {
    pub client_id: ClientId,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub grounded: bool,
    pub last_processed_input: u64,
}
