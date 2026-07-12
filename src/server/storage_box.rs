//! Server-authoritative storage box state.
//!
//! A storage box is a placed deployable holding a fixed grid of item
//! slots. It deliberately reuses the loot-bag container machinery: once
//! opened (kind + range validated here), the box becomes the client's
//! [`super::loot_bag::OpenContainer`] and every move / quick-transfer /
//! close rides the shared `LootBagCommand` path. The only storage-box
//! specific wire piece is `ClientMessage::OpenStorageBox`.

use crate::{
    game_balance::{STORAGE_BOX_LARGE_SLOT_COUNT, STORAGE_BOX_SMALL_SLOT_COUNT},
    items::DeployableKind,
    protocol::{ClientId, DeployedEntityId, ItemStack},
    save::PersistedStorageBoxState,
    server::{GameServer, ServerEnvelope},
};

use super::loot_bag::OpenContainer;

pub(crate) use crate::game_balance::STORAGE_BOX_INTERACT_RANGE_M;

/// Slot count for a storage box tier (1 = small, anything above = large).
pub(crate) fn storage_box_slot_count(tier: u8) -> usize {
    if tier >= 2 {
        STORAGE_BOX_LARGE_SLOT_COUNT
    } else {
        STORAGE_BOX_SMALL_SLOT_COUNT
    }
}

/// Authoritative contents of a placed storage box. Lives on
/// [`super::deployables::DeployedEntity::storage`]; `None` for every
/// other deployable kind.
#[derive(Debug, Clone)]
pub(crate) struct StorageBoxState {
    pub(crate) slots: Vec<Option<ItemStack>>,
}

impl StorageBoxState {
    pub(crate) fn new(tier: u8) -> Self {
        Self {
            slots: vec![None; storage_box_slot_count(tier)],
        }
    }

    pub(crate) fn to_persisted(&self) -> PersistedStorageBoxState {
        PersistedStorageBoxState {
            slots: self.slots.clone(),
        }
    }

    pub(crate) fn from_persisted(persisted: PersistedStorageBoxState, tier: u8) -> Self {
        let mut slots = persisted.slots;
        // Defensive resize: a save written with a different slot count
        // (balance change) still loads; surplus stacks are dropped from
        // the tail rather than corrupting the grid.
        slots.resize(storage_box_slot_count(tier), None);
        Self { slots }
    }

    /// A ruin cache stores its loot in the same slot grid as a storage box, so
    /// it reuses this state, but with the ruin-cache slot count. Kept beside
    /// the box constructors so the two container kinds share one code path.
    pub(crate) fn new_ruin_cache() -> Self {
        Self {
            slots: vec![None; crate::game_balance::RUIN_CACHE_SLOT_COUNT],
        }
    }

    pub(crate) fn from_ruin_cache_persisted(persisted: PersistedStorageBoxState) -> Self {
        let mut slots = persisted.slots;
        slots.resize(crate::game_balance::RUIN_CACHE_SLOT_COUNT, None);
        Self { slots }
    }
}

impl GameServer {
    /// `ClientMessage::OpenStorageBox`: validate the target is a storage
    /// box within interact range, then open it as the client's container
    /// so the shared loot-bag command path takes over.
    pub(super) fn apply_open_storage_box(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let player_pos = client.controller.position;
        let Some(entity) = self.deployed_entities.get(&id) else {
            return Vec::new();
        };
        // Storage boxes and ruin caches share the container view: both store
        // their loot in the `storage` grid and open through this message. The
        // ruin cache uses its own (wider) interact range.
        let range = match entity.kind {
            DeployableKind::StorageBox { .. } => STORAGE_BOX_INTERACT_RANGE_M,
            DeployableKind::RuinCache => crate::game_balance::RUIN_CACHE_INTERACT_RANGE_M,
            _ => return Vec::new(),
        };
        if !player_pos.within_horizontal_range(entity.position, range) {
            return Vec::new();
        }
        if let Some(client_mut) = self.clients.get_mut(&client_id) {
            client_mut.open_container = Some(OpenContainer::StorageBox(id));
        }
        Vec::new()
    }
}
