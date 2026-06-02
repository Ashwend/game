//! Furnace state shape, persistence shims, and pure helpers shared
//! between the tick loop and command handlers.
//!
//! No `GameServer` impl in this module, keep it pure-data so unit tests
//! over the smelt math don't need to spin up a server.

use crate::{
    items::{COAL_ID, IRON_BAR_ID, IRON_ORE_ID, WOOD_ID},
    protocol::{DeployedEntityId, FURNACE_ITEM_SLOT_COUNT, ItemStack, OpenFurnaceView},
    save::PersistedFurnaceState,
};

use crate::server::ServerClient;

pub(crate) use crate::game_balance::{
    FURNACE_COAL_BURN_TICKS as COAL_BURN_TICKS, FURNACE_INTERACT_RANGE_M,
    FURNACE_SMELT_TICKS_PER_OUTPUT as SMELT_TICKS_PER_OUTPUT,
    FURNACE_WOOD_BURN_TICKS as WOOD_BURN_TICKS,
};

/// Per-furnace operational state. The fuel + items slots live here;
/// `active` and the timers drive the smelt loop.
#[derive(Debug, Clone, Default)]
pub(crate) struct FurnaceState {
    pub(crate) fuel: Option<ItemStack>,
    pub(crate) items: [Option<ItemStack>; FURNACE_ITEM_SLOT_COUNT],
    pub(crate) active: bool,
    /// Remaining ticks of burn for the fuel unit currently being
    /// consumed. 0 means no fuel is mid-burn, the next tick that
    /// needs fuel will pop one off the `fuel` stack.
    pub(crate) fuel_burn_ticks_left: u32,
    /// Accumulated progress against the current smelt operation.
    /// Reset when the head smelt completes or the input stack moves.
    pub(crate) smelt_progress_ticks: u32,
}

impl FurnaceState {
    pub(crate) fn from_persisted(p: PersistedFurnaceState) -> Self {
        let mut items: [Option<ItemStack>; FURNACE_ITEM_SLOT_COUNT] = Default::default();
        for (index, stack) in p.items.into_iter().enumerate() {
            if let Some(slot) = items.get_mut(index) {
                *slot = stack;
            }
        }
        Self {
            fuel: p.fuel,
            items,
            active: p.active,
            fuel_burn_ticks_left: p.fuel_burn_ticks_left,
            smelt_progress_ticks: p.smelt_progress_ticks,
        }
    }

    pub(crate) fn to_persisted(&self) -> PersistedFurnaceState {
        PersistedFurnaceState {
            fuel: self.fuel.clone(),
            items: self.items.to_vec(),
            active: self.active,
            fuel_burn_ticks_left: self.fuel_burn_ticks_left,
            smelt_progress_ticks: self.smelt_progress_ticks,
        }
    }

    pub(crate) fn to_view(&self, id: DeployedEntityId) -> OpenFurnaceView {
        OpenFurnaceView {
            id,
            fuel: self.fuel.clone(),
            items: self.items.to_vec(),
            active: self.active,
            smelt_fraction: if SMELT_TICKS_PER_OUTPUT == 0 {
                1.0
            } else {
                (self.smelt_progress_ticks as f32 / SMELT_TICKS_PER_OUTPUT as f32).clamp(0.0, 1.0)
            },
            fuel_fraction: {
                let max = max_fuel_burn_ticks_for(self.fuel.as_ref());
                if max == 0 {
                    0.0
                } else {
                    (self.fuel_burn_ticks_left as f32 / max as f32).clamp(0.0, 1.0)
                }
            },
        }
    }
}

/// Map a fuel item to how many ticks one unit burns for. The smelt loop
/// reads this when it needs to ignite the next unit.
pub(super) fn fuel_burn_ticks_for(item_id: &str) -> Option<u32> {
    match item_id {
        WOOD_ID => Some(WOOD_BURN_TICKS),
        COAL_ID => Some(COAL_BURN_TICKS),
        _ => None,
    }
}

/// Upper bound for the fuel-fraction UI bar. We track the *currently
/// burning* unit, so the bar fills the moment a new unit ignites and
/// drains down to 0 before the next one kicks off.
pub(super) fn max_fuel_burn_ticks_for(stack: Option<&ItemStack>) -> u32 {
    stack
        .and_then(|s| fuel_burn_ticks_for(s.item_id.as_ref()))
        .unwrap_or(WOOD_BURN_TICKS)
}

/// Smelt-result map. Right now there's only one (iron ore → iron bar);
/// extending this is how new smelt recipes ship.
pub(super) fn smelt_result(item_id: &str) -> Option<ItemStack> {
    if item_id == IRON_ORE_ID {
        Some(ItemStack::new(IRON_BAR_ID, 1))
    } else {
        None
    }
}

/// Outcome of a fuel-slot deposit during quick transfer. `Placed` means
/// the source was consumed in full; `Rejected` returns the un-placed
/// stack so the caller can route it back to where it came from.
pub(super) enum FuelPlaceOutcome {
    Placed,
    Rejected(ItemStack),
}

/// Pull up to `quantity` (or the whole stack if `quantity` is `None`)
/// out of `slot`. Returns the taken stack and whether the slot is now
/// empty, the caller uses the `drained` flag to decide whether a
/// downstream deposit is allowed to swap with an occupied target.
pub(super) fn take_partial(
    slot: &mut Option<ItemStack>,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    let stack = slot.as_mut()?;
    let want = quantity.unwrap_or(stack.quantity).min(stack.quantity);
    if want == 0 {
        return None;
    }
    let taken = ItemStack {
        item_id: stack.item_id.clone(),
        quantity: want,
    };
    stack.quantity -= want;
    let drained = stack.quantity == 0;
    if drained {
        *slot = None;
    }
    Some((taken, drained))
}

/// Spread `stack` across the furnace's items grid using the same rules
/// the player-inventory adder uses: existing matching stacks fill first
/// (capped by their item stack limit), then the first empty slot. Any
/// leftover that didn't fit comes back so the caller can decide what to
/// do with it (quick transfer routes it back to the source slot).
pub(super) fn add_stack_to_furnace_items(
    furnace: &mut FurnaceState,
    mut remaining: ItemStack,
) -> Option<ItemStack> {
    let limit = crate::items::stack_limit(remaining.item_id.as_ref()).unwrap_or(u16::MAX);

    for slot in furnace.items.iter_mut() {
        if remaining.quantity == 0 {
            return None;
        }
        let Some(existing) = slot.as_mut() else {
            continue;
        };
        if existing.item_id != remaining.item_id {
            continue;
        }
        let space = limit.saturating_sub(existing.quantity);
        if space == 0 {
            continue;
        }
        let take = remaining.quantity.min(space);
        existing.quantity = existing.quantity.saturating_add(take);
        remaining.quantity -= take;
    }

    if remaining.quantity == 0 {
        return None;
    }

    for slot in furnace.items.iter_mut() {
        if slot.is_some() {
            continue;
        }
        let take = remaining.quantity.min(limit);
        *slot = Some(ItemStack {
            item_id: remaining.item_id.clone(),
            quantity: take,
        });
        remaining.quantity -= take;
        if remaining.quantity == 0 {
            return None;
        }
    }

    Some(remaining)
}

/// Would `stack` fit into the player's inventory + actionbar following
/// the standard "merge into matching stacks first, then take first
/// empty inventory slot" rule? Used as a dry-run before committing to a
/// fuel-slot swap so the displaced fuel doesn't get orphaned at the
/// player's feet.
pub(super) fn inventory_has_room_for(client: Option<&ServerClient>, stack: &ItemStack) -> bool {
    let Some(client) = client else { return false };
    let limit = crate::items::stack_limit(stack.item_id.as_ref()).unwrap_or(u16::MAX);
    let mut remaining = stack.quantity;

    // Matching stacks (any container) absorb up to their stack limit.
    let matching_slots = client
        .inventory
        .actionbar_slots
        .iter()
        .chain(client.inventory.inventory_slots.iter());
    for slot in matching_slots {
        if remaining == 0 {
            return true;
        }
        let Some(existing) = slot.as_ref() else {
            continue;
        };
        if existing.item_id != stack.item_id {
            continue;
        }
        let space = limit.saturating_sub(existing.quantity);
        remaining = remaining.saturating_sub(space);
    }
    if remaining == 0 {
        return true;
    }
    // First-empty-slot fallback mirrors `add_stack_to_inventory`: only
    // the bag's empty slots count, not the actionbar's.
    client.inventory.inventory_slots.iter().any(Option::is_none)
}

pub(crate) fn merge_into_optional_slot(
    target: &mut Option<ItemStack>,
    mut incoming: ItemStack,
) -> Option<ItemStack> {
    let limit = crate::items::stack_limit(incoming.item_id.as_ref()).unwrap_or(u16::MAX);
    match target {
        Some(existing) if existing.item_id == incoming.item_id => {
            let space = limit.saturating_sub(existing.quantity);
            if space == 0 {
                return Some(incoming);
            }
            let take = incoming.quantity.min(space);
            existing.quantity = existing.quantity.saturating_add(take);
            incoming.quantity -= take;
            if incoming.quantity == 0 {
                None
            } else {
                Some(incoming)
            }
        }
        Some(_) => Some(incoming),
        None => {
            // Honour stack limit even when placing into an empty slot.
            let take = incoming.quantity.min(limit);
            let placed = ItemStack {
                item_id: incoming.item_id.clone(),
                quantity: take,
            };
            *target = Some(placed);
            incoming.quantity -= take;
            if incoming.quantity == 0 {
                None
            } else {
                Some(incoming)
            }
        }
    }
}
