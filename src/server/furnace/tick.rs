//! Furnace smelt loop: per-tick fuel burn, head-input smelting, and the
//! auto-shutoff rules. Lives outside the `GameServer` impl so it can be
//! unit-tested without spinning up a server.

use crate::protocol::ItemStack;
use crate::server::GameServer;

use super::state::{FurnaceState, SMELT_TICKS_PER_OUTPUT, fuel_burn_ticks_for, smelt_result};

impl GameServer {
    /// Advance every furnace one tick: burn fuel, smelt the head input,
    /// auto-shutoff when output won't fit. Called once per server tick.
    ///
    /// Mirror-sync note: of the furnace fields this tick mutates, only the
    /// `active` flag is replicated through the deployable mirror
    /// (`DeployableActive`); fuel/progress reach the owning viewer through
    /// the per-player `OpenFurnaceView` instead. So we flag a deployable
    /// dirty only when `active` flips (auto-shutoff), idle furnaces and
    /// steady burns never enter the sync delta.
    pub(in crate::server) fn tick_furnaces(&mut self) {
        let mut shut_off: Vec<crate::protocol::DeployedEntityId> = Vec::new();
        for (id, entity) in self.deployed_entities.iter_mut() {
            let Some(furnace) = entity.furnace.as_mut() else {
                continue;
            };
            if !furnace.active {
                continue;
            }
            tick_one_furnace(furnace);
            if !furnace.active {
                shut_off.push(*id);
            }
        }
        for id in shut_off {
            self.mark_deployable_dirty(id);
        }
    }
}

/// One-furnace tick. Pulled out so it can be unit-tested without spinning
/// up a `GameServer`.
pub(crate) fn tick_one_furnace(furnace: &mut FurnaceState) {
    // Ignite a fresh fuel unit if we need to and one is available.
    if furnace.fuel_burn_ticks_left == 0 {
        if let Some(ignited) = consume_one_fuel(furnace) {
            furnace.fuel_burn_ticks_left = ignited;
        } else {
            // No fuel → shut down. We don't keep smelt progress when
            // the burn stalls; this matches the SetActive-off rule and
            // means an "almost done" smelt can't sit indefinitely.
            furnace.active = false;
            furnace.smelt_progress_ticks = 0;
            return;
        }
    }

    let head_index = find_smeltable_head(furnace);
    let Some(head_index) = head_index else {
        // Nothing to smelt, auto-off so the player can tell at a
        // glance the furnace isn't doing work, and the fuel doesn't
        // burn down while idle.
        furnace.active = false;
        furnace.smelt_progress_ticks = 0;
        return;
    };

    // Pre-check that the output will fit before we spend the fuel
    // tick. If it won't, auto-shutoff is the spec.
    let head_stack = furnace.items[head_index]
        .as_ref()
        .expect("smeltable head must be Some");
    let result = smelt_result(head_stack.item_id.as_ref())
        .expect("find_smeltable_head guarantees a smelt result");
    if !output_fits_somewhere(furnace, head_index, &result) {
        furnace.active = false;
        furnace.smelt_progress_ticks = 0;
        return;
    }

    // Spend one tick of fuel and advance the smelt timer.
    furnace.fuel_burn_ticks_left = furnace.fuel_burn_ticks_left.saturating_sub(1);
    furnace.smelt_progress_ticks = furnace.smelt_progress_ticks.saturating_add(1);

    if furnace.smelt_progress_ticks >= SMELT_TICKS_PER_OUTPUT {
        // Consume one input + grant one output. Output-fit was
        // verified above so `deposit_smelt_output` is guaranteed to
        // place the item somewhere.
        if let Some(stack) = furnace.items[head_index].as_mut() {
            stack.quantity = stack.quantity.saturating_sub(1);
            if stack.quantity == 0 {
                furnace.items[head_index] = None;
            }
        }
        deposit_smelt_output(furnace, result);
        furnace.smelt_progress_ticks = 0;
    }
}

fn consume_one_fuel(furnace: &mut FurnaceState) -> Option<u32> {
    let stack = furnace.fuel.as_mut()?;
    let ticks = fuel_burn_ticks_for(stack.item_id.as_ref())?;
    stack.quantity = stack.quantity.saturating_sub(1);
    if stack.quantity == 0 {
        furnace.fuel = None;
    }
    Some(ticks)
}

fn find_smeltable_head(furnace: &FurnaceState) -> Option<usize> {
    furnace.items.iter().position(|slot| {
        slot.as_ref()
            .map(|stack| smelt_result(stack.item_id.as_ref()).is_some() && stack.quantity > 0)
            .unwrap_or(false)
    })
}

/// True if the smelt result can land somewhere, either merging into
/// an existing matching stack (anywhere in the grid) or filling an
/// empty slot. The current input's slot can take its own output once
/// the input runs to 0, which is the common case.
fn output_fits_somewhere(furnace: &FurnaceState, input_index: usize, result: &ItemStack) -> bool {
    let limit = crate::items::stack_limit(result.item_id.as_ref()).unwrap_or(u16::MAX);
    let consumed_clears_slot = furnace.items[input_index]
        .as_ref()
        .map(|stack| stack.quantity == 1)
        .unwrap_or(false);
    for (index, slot) in furnace.items.iter().enumerate() {
        let is_input_slot = index == input_index;
        match slot {
            None => return true,
            Some(_) if is_input_slot && consumed_clears_slot => return true,
            Some(existing) => {
                if existing.item_id == result.item_id && existing.quantity < limit {
                    return true;
                }
            }
        }
    }
    false
}

fn deposit_smelt_output(furnace: &mut FurnaceState, output: ItemStack) {
    let limit = crate::items::stack_limit(output.item_id.as_ref()).unwrap_or(u16::MAX);
    // Try to merge with an existing matching stack first so the
    // output column packs tightly.
    for slot in furnace.items.iter_mut() {
        if let Some(existing) = slot
            && existing.item_id == output.item_id
            && existing.quantity < limit
        {
            existing.quantity = existing.quantity.saturating_add(output.quantity);
            return;
        }
    }
    // Fall back to the first empty slot.
    for slot in furnace.items.iter_mut() {
        if slot.is_none() {
            *slot = Some(output);
            return;
        }
    }
    // Output-fit was checked beforehand, this branch shouldn't trigger.
    debug_assert!(false, "deposit_smelt_output called with no room");
}
