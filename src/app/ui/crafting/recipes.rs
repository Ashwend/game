//! Pure inventory/recipe math: how many of a recipe the player can
//! afford, whether they have every input, and per-item inventory counts.

use crate::{
    crafting::RecipeDefinition,
    protocol::{MAX_CRAFT_BATCH_SIZE, PlayerInventoryState},
};

/// Compute the largest batch quantity the player can currently afford
/// for a given recipe, capped at [`MAX_CRAFT_BATCH_SIZE`].
///
/// `0` means "can't even craft one" — the same condition the existing
/// `craftable` flag tracks, but expressed as a batch-aware ceiling so
/// the recipe row can also disable the `+` button at the actual limit.
pub(super) fn max_craftable_batch(
    inventory: Option<&PlayerInventoryState>,
    recipe: &RecipeDefinition,
) -> u16 {
    let Some(inventory) = inventory else {
        return 0;
    };
    if recipe.inputs.is_empty() {
        // No-input recipes never gate on materials, so the only ceiling
        // is the protocol's per-message cap.
        return MAX_CRAFT_BATCH_SIZE;
    }
    let mut max = MAX_CRAFT_BATCH_SIZE as u32;
    for input in recipe.inputs {
        if input.quantity == 0 {
            continue;
        }
        let have = count_in_inventory(inventory, input.item_id) as u32;
        let possible = have / input.quantity as u32;
        max = max.min(possible);
    }
    max.min(MAX_CRAFT_BATCH_SIZE as u32) as u16
}

pub(super) fn has_all_inputs(inventory: &PlayerInventoryState, recipe: &RecipeDefinition) -> bool {
    recipe
        .inputs
        .iter()
        .all(|input| count_in_inventory(inventory, input.item_id) >= input.quantity)
}

pub(super) fn count_in_inventory(inventory: &PlayerInventoryState, item_id: &str) -> u16 {
    let mut total: u32 = 0;
    for slot in inventory
        .inventory_slots
        .iter()
        .chain(inventory.actionbar_slots.iter())
    {
        if let Some(stack) = slot
            && stack.item_id.as_ref() == item_id
        {
            total = total.saturating_add(stack.quantity as u32);
        }
    }
    total.min(u16::MAX as u32) as u16
}
