//! Item stacks, the inventory/actionbar container addressing, and the
//! per-player inventory + crafting-queue state shapes.

use serde::{Deserialize, Serialize};

use super::{ACTIONBAR_SLOT_COUNT, CraftingJobId, INVENTORY_SLOT_COUNT};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemStack {
    #[serde(deserialize_with = "super::deserialize_interned_item_id")]
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

/// One in-progress crafting job. `progress_ticks` advances toward
/// `total_ticks`; when they meet the server grants the recipe's output
/// (multiplied by `quantity`) and pops the job. Inputs are not echoed back
///, they were taken at enqueue time and the recipe id lets the client
/// reconstruct everything else from the static registry.
///
/// `quantity` is the batch size. A job with `quantity = 3` ran with
/// 3× the inputs at enqueue time, has `total_ticks = ticks_per_unit × 3`,
/// and on completion grants `output_quantity × 3` of the output item in a
/// single grant. The UI uses `quantity > 1` to render `×N` next to the
/// job's name in the queue HUD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CraftingJob {
    pub job_id: CraftingJobId,
    #[serde(deserialize_with = "super::deserialize_interned_recipe_id")]
    pub recipe_id: crate::crafting::RecipeId,
    pub progress_ticks: u32,
    pub total_ticks: u32,
    pub quantity: u16,
}

impl CraftingJob {
    pub fn new(
        job_id: CraftingJobId,
        recipe_id: impl AsRef<str>,
        total_ticks: u32,
        quantity: u16,
    ) -> Self {
        Self {
            job_id,
            recipe_id: crate::crafting::intern_recipe_id(recipe_id.as_ref()),
            progress_ticks: 0,
            total_ticks,
            quantity,
        }
    }

    /// Fraction of the head job's craft time that has elapsed, in `[0.0, 1.0]`.
    /// Returns `1.0` for zero-duration recipes so the UI doesn't divide by
    /// zero or stall on a permanent empty bar.
    pub fn progress_fraction(&self) -> f32 {
        if self.total_ticks == 0 {
            return 1.0;
        }
        (self.progress_ticks as f32 / self.total_ticks as f32).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerCraftingState {
    pub jobs: Vec<CraftingJob>,
}

impl PlayerCraftingState {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }
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

    /// Pad (or trim) the slot vectors to the current canonical capacities.
    /// A persisted inventory written before a capacity change keeps its old
    /// length on load; normalizing on restore exposes any newly-added empty
    /// slots and keeps the on-wire shape consistent with fresh inventories.
    /// Bounds checks already use the live vec length, so this is about making
    /// the new slots usable, not about safety.
    pub fn normalize_capacity(&mut self) {
        self.inventory_slots.resize(INVENTORY_SLOT_COUNT, None);
        self.actionbar_slots.resize(ACTIONBAR_SLOT_COUNT, None);
        if self.active_actionbar_slot >= ACTIONBAR_SLOT_COUNT {
            self.active_actionbar_slot = 0;
        }
    }

    pub fn active_actionbar_stack(&self) -> Option<&ItemStack> {
        self.actionbar_slots
            .get(self.active_actionbar_slot)
            .and_then(Option::as_ref)
    }

    /// Read-only access to the stack in a specific slot. Returns `None` for
    /// an empty *or* out-of-range slot. Used by the client-side move
    /// prediction to gate on an empty destination.
    pub fn slot(&self, slot: ItemContainerSlot) -> Option<&ItemStack> {
        match slot.container {
            ItemContainer::Inventory => self.inventory_slots.get(slot.slot),
            ItemContainer::Actionbar => self.actionbar_slots.get(slot.slot),
        }
        .and_then(Option::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::items::{COAL_ID, IRON_ORE_ID, intern_item_id};

    use super::*;

    #[test]
    fn normalize_capacity_grows_short_inventory_and_preserves_stacks() {
        let mut state = PlayerInventoryState::empty();
        // Simulate a save written before a capacity bump: a short slot vec
        // with stacks at known indices.
        state.inventory_slots = vec![
            Some(ItemStack::new(IRON_ORE_ID, 4)),
            None,
            Some(ItemStack::new(COAL_ID, 7)),
        ];

        state.normalize_capacity();

        assert_eq!(state.inventory_slots.len(), INVENTORY_SLOT_COUNT);
        // Original stacks stay put at their indices.
        assert_eq!(
            state.inventory_slots[0],
            Some(ItemStack::new(IRON_ORE_ID, 4))
        );
        assert_eq!(state.inventory_slots[1], None);
        assert_eq!(state.inventory_slots[2], Some(ItemStack::new(COAL_ID, 7)));
        // Every newly-added trailing slot is empty.
        assert!(state.inventory_slots[3..].iter().all(Option::is_none));
    }

    #[test]
    fn normalize_capacity_trims_over_long_inventory() {
        let mut state = PlayerInventoryState::empty();
        state.inventory_slots = vec![None; INVENTORY_SLOT_COUNT + 10];
        state.actionbar_slots = vec![None; ACTIONBAR_SLOT_COUNT + 5];

        state.normalize_capacity();

        assert_eq!(state.inventory_slots.len(), INVENTORY_SLOT_COUNT);
        assert_eq!(state.actionbar_slots.len(), ACTIONBAR_SLOT_COUNT);
    }

    #[test]
    fn normalize_capacity_resets_out_of_range_active_slot() {
        let mut state = PlayerInventoryState::empty();
        state.active_actionbar_slot = ACTIONBAR_SLOT_COUNT + 3;
        state.normalize_capacity();
        assert_eq!(state.active_actionbar_slot, 0);

        // An in-range value is left untouched.
        let mut in_range = PlayerInventoryState::empty();
        in_range.active_actionbar_slot = ACTIONBAR_SLOT_COUNT - 1;
        in_range.normalize_capacity();
        assert_eq!(in_range.active_actionbar_slot, ACTIONBAR_SLOT_COUNT - 1);
    }

    #[test]
    fn item_stack_round_trips_and_reuses_the_interned_arc() {
        let stack = ItemStack::new(IRON_ORE_ID, 4);
        let encoded = postcard::to_allocvec(&stack).expect("encode item stack");
        let decoded: ItemStack = postcard::from_bytes(&encoded).expect("decode item stack");

        assert_eq!(decoded, stack);
        // The `deserialize_interned_item_id` hook routes the decoded id
        // through the global intern table, so the decoded `Arc<str>` is the
        // same allocation the registry already holds (refcount bump, not a
        // fresh heap copy).
        assert!(Arc::ptr_eq(&decoded.item_id, &intern_item_id(IRON_ORE_ID)));
    }

    #[test]
    fn crafting_job_recipe_id_round_trips_and_reuses_the_interned_arc() {
        use crate::crafting::{STONE_HATCHET_RECIPE_ID, intern_recipe_id};

        let job = CraftingJob::new(7, STONE_HATCHET_RECIPE_ID, 120, 2);
        let encoded = postcard::to_allocvec(&job).expect("encode crafting job");
        let decoded: CraftingJob = postcard::from_bytes(&encoded).expect("decode crafting job");

        assert_eq!(decoded, job);
        // Same interning guarantee as the item id, via
        // `deserialize_interned_recipe_id`.
        assert!(Arc::ptr_eq(
            &decoded.recipe_id,
            &intern_recipe_id(STONE_HATCHET_RECIPE_ID)
        ));
    }
}
