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
