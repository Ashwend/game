//! Pure inventory math shared by the authoritative server and the
//! client-side optimistic prediction overlay.
//!
//! Everything here operates on a [`PlayerInventoryState`] plus arguments,
//! no `GameServer`, no ECS, no side effects. The server applies these to its
//! authoritative `ServerClient::inventory`; the client replays the same
//! functions on top of the replicated inventory to predict the result of an
//! action before the server confirms it. Keeping a single implementation is
//! what makes prediction match the server exactly (see
//! `src/app/state/prediction.rs`). The impure, `GameServer`-bound handlers
//! (drop spawning, pickup, resource-node pickup) stay in
//! `src/server/inventory.rs`.

use crate::{
    items::{item_definition, normalize_stack, stack_limit},
    protocol::{
        ACTIONBAR_SLOT_COUNT, INVENTORY_SLOT_COUNT, ItemContainer, ItemContainerSlot, ItemStack,
        PlayerInventoryState,
    },
};

pub fn move_stack(
    inventory: &mut PlayerInventoryState,
    from: ItemContainerSlot,
    to: ItemContainerSlot,
    quantity: Option<u16>,
) {
    if from == to || !slot_exists(inventory, from) || !slot_exists(inventory, to) {
        return;
    }

    let Some((moving, removed_all)) = remove_stack_for_move(inventory, from, quantity) else {
        return;
    };
    let remainder = insert_stack_at(inventory, to, moving, removed_all);
    if let Some(remainder) = remainder {
        restore_stack(inventory, from, remainder);
    }
}

fn remove_stack_for_move(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    let source = slot_mut(inventory, slot)?;
    let current = source.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let removed_all = amount == current.quantity;
    let item_id = current.item_id.clone();
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some((ItemStack::new(item_id, amount), removed_all))
}

pub fn remove_stack(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    quantity: Option<u16>,
) -> Option<ItemStack> {
    let source = slot_mut(inventory, slot)?;
    let current = source.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let item_id = current.item_id.clone();
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some(ItemStack::new(item_id, amount))
}

pub fn insert_stack_at(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    mut moving: ItemStack,
    allow_swap: bool,
) -> Option<ItemStack> {
    moving = normalize_stack(&moving)?;
    let target = slot_mut(inventory, slot)?;
    match target {
        None => {
            *target = Some(moving);
            None
        }
        Some(existing) if existing.item_id == moving.item_id => {
            let limit = stack_limit(&existing.item_id).unwrap_or(1);
            let room = limit.saturating_sub(existing.quantity);
            let moved = room.min(moving.quantity);
            existing.quantity += moved;
            moving.quantity -= moved;
            (moving.quantity > 0).then_some(moving)
        }
        Some(existing) if allow_swap => {
            let displaced = std::mem::replace(existing, moving);
            Some(displaced)
        }
        Some(_) => Some(moving),
    }
}

fn restore_stack(inventory: &mut PlayerInventoryState, slot: ItemContainerSlot, stack: ItemStack) {
    let Some(target) = slot_mut(inventory, slot) else {
        return;
    };
    match target {
        Some(existing) if existing.item_id == stack.item_id => {
            let limit = stack_limit(&existing.item_id).unwrap_or(1);
            existing.quantity = existing.quantity.saturating_add(stack.quantity).min(limit);
        }
        None => {
            *target = Some(stack);
        }
        Some(_) => {}
    }
}

/// Pull up to `quantity` units of `item_id` out of the inventory + actionbar.
/// Walks slots in `actionbar → inventory` order (so the toolbar drains last,
/// leaving the player's quick-access items intact when the bag has the same
/// material). Returns the actual amount removed; less than `quantity` means
/// there wasn't enough to satisfy the request.
///
/// Designed for the crafting consume path. The caller is expected to verify
/// totals up-front so the partial case shouldn't fire in practice, but the
/// function still drains what it can, since refusing to remove anything
/// would leave the inventory in a worse state if a recipe definition ever
/// goes out of sync with the take.
pub fn take_items_from_inventory(
    inventory: &mut PlayerInventoryState,
    item_id: &str,
    quantity: u16,
) -> u16 {
    let mut remaining = quantity;
    if remaining == 0 {
        return 0;
    }

    for slot in inventory
        .actionbar_slots
        .iter_mut()
        .chain(inventory.inventory_slots.iter_mut())
    {
        if remaining == 0 {
            break;
        }
        let Some(stack) = slot.as_mut() else {
            continue;
        };
        if stack.item_id.as_ref() != item_id {
            continue;
        }
        let take = remaining.min(stack.quantity);
        stack.quantity -= take;
        remaining -= take;
        if stack.quantity == 0 {
            *slot = None;
        }
    }

    quantity - remaining
}

pub fn add_stack_to_inventory(
    inventory: &mut PlayerInventoryState,
    stack: ItemStack,
) -> Option<ItemStack> {
    let mut remaining = normalize_stack(&stack)?;

    for index in 0..inventory.actionbar_slots.len() {
        let slot = ItemContainerSlot::actionbar(index);
        if inventory.actionbar_slots[index]
            .as_ref()
            .is_some_and(|existing| existing.item_id == remaining.item_id)
        {
            remaining = insert_stack_at(inventory, slot, remaining, false)?;
        }
    }

    for index in 0..inventory.inventory_slots.len() {
        let slot = ItemContainerSlot::inventory(index);
        if inventory.inventory_slots[index]
            .as_ref()
            .is_some_and(|existing| existing.item_id == remaining.item_id)
        {
            remaining = insert_stack_at(inventory, slot, remaining, false)?;
        }
    }

    // Tools and deployables are quick-access items the player reaches for
    // constantly, so a freshly crafted or picked-up one should land on the
    // actionbar when there's an open slot, before it spills into the bag.
    // Everything else keeps the original bag-first behaviour.
    if prefers_actionbar(&remaining.item_id) {
        for index in 0..inventory.actionbar_slots.len() {
            if inventory.actionbar_slots[index].is_none() {
                inventory.actionbar_slots[index] = Some(remaining);
                return None;
            }
        }
    }

    for index in 0..inventory.inventory_slots.len() {
        if inventory.inventory_slots[index].is_none() {
            inventory.inventory_slots[index] = Some(remaining);
            return None;
        }
    }

    Some(remaining)
}

/// Whether a freshly added stack of `item_id` should prefer an empty
/// actionbar slot over the bag. True for tools and deployables, the items
/// the player equips and uses directly, and false for everything else, so
/// gathered resources still flow into the main inventory as before.
fn prefers_actionbar(item_id: &str) -> bool {
    item_definition(item_id).is_some_and(|def| def.tool.is_some() || def.deployable.is_some())
}

/// How many units of `stack` would actually fit if added to `inventory`,
/// mutating `inventory` to reflect the insert. Mirrors the server's gather
/// payout accounting (`requested − overflow`); the prediction overlay calls
/// this so a near-full bag predicts the same truncated gain the server will.
pub fn accepted_inventory_quantity(inventory: &mut PlayerInventoryState, stack: ItemStack) -> u16 {
    let requested = stack.quantity;
    match add_stack_to_inventory(inventory, stack) {
        Some(remainder) => requested.saturating_sub(remainder.quantity),
        None => requested,
    }
}

fn slot_mut(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
) -> Option<&mut Option<ItemStack>> {
    match slot.container {
        ItemContainer::Inventory => inventory.inventory_slots.get_mut(slot.slot),
        ItemContainer::Actionbar => inventory.actionbar_slots.get_mut(slot.slot),
    }
}

fn slot_exists(inventory: &PlayerInventoryState, slot: ItemContainerSlot) -> bool {
    (match slot.container {
        ItemContainer::Inventory => slot.slot < INVENTORY_SLOT_COUNT,
        ItemContainer::Actionbar => slot.slot < ACTIONBAR_SLOT_COUNT,
    }) && (match slot.container {
        ItemContainer::Inventory => slot.slot < inventory.inventory_slots.len(),
        ItemContainer::Actionbar => slot.slot < inventory.actionbar_slots.len(),
    })
}

pub fn offset_actionbar_slot(current: usize, offset: i8) -> usize {
    (current as isize + offset as isize).rem_euclid(ACTIONBAR_SLOT_COUNT as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::{BASIC_PICKAXE_ID, COAL_ID},
        protocol::ItemStack,
    };

    #[test]
    fn accepted_quantity_reports_partial_inventory_insert() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 199));
        for slot in inventory.inventory_slots.iter_mut().skip(1) {
            *slot = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
        }

        assert_eq!(
            accepted_inventory_quantity(&mut inventory, ItemStack::new(COAL_ID, 3)),
            1
        );
        assert_eq!(
            inventory.inventory_slots[0]
                .as_ref()
                .map(|stack| stack.quantity),
            Some(200)
        );
    }

    #[test]
    fn add_stack_merges_into_existing_then_fills_empty_slot() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 5));

        assert!(add_stack_to_inventory(&mut inventory, ItemStack::new(COAL_ID, 10)).is_none());
        assert_eq!(
            inventory.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(15)
        );
    }

    #[test]
    fn tools_land_on_the_actionbar_before_the_bag() {
        let mut inventory = PlayerInventoryState::empty();

        assert!(
            add_stack_to_inventory(&mut inventory, ItemStack::new(BASIC_PICKAXE_ID, 1)).is_none()
        );

        // The pickaxe went to the first actionbar slot, leaving the bag empty.
        assert_eq!(
            inventory.actionbar_slots[0]
                .as_ref()
                .map(|stack| stack.item_id.as_ref()),
            Some(BASIC_PICKAXE_ID)
        );
        assert!(inventory.inventory_slots.iter().all(Option::is_none));
    }

    #[test]
    fn tools_fall_back_to_the_bag_when_the_actionbar_is_full() {
        let mut inventory = PlayerInventoryState::empty();
        for slot in inventory.actionbar_slots.iter_mut() {
            *slot = Some(ItemStack::new(COAL_ID, 1));
        }

        assert!(
            add_stack_to_inventory(&mut inventory, ItemStack::new(BASIC_PICKAXE_ID, 1)).is_none()
        );

        assert_eq!(
            inventory.inventory_slots[0]
                .as_ref()
                .map(|stack| stack.item_id.as_ref()),
            Some(BASIC_PICKAXE_ID)
        );
    }

    #[test]
    fn non_tools_ignore_empty_actionbar_slots() {
        let mut inventory = PlayerInventoryState::empty();

        assert!(add_stack_to_inventory(&mut inventory, ItemStack::new(COAL_ID, 5)).is_none());

        // Resources still flow into the bag even with the actionbar wide open.
        assert!(inventory.actionbar_slots.iter().all(Option::is_none));
        assert_eq!(
            inventory.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(5)
        );
    }

    #[test]
    fn move_into_empty_slot_relocates_stack() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 7));

        move_stack(
            &mut inventory,
            ItemContainerSlot::inventory(0),
            ItemContainerSlot::inventory(1),
            None,
        );

        assert!(inventory.inventory_slots[0].is_none());
        assert_eq!(
            inventory.inventory_slots[1].as_ref().map(|s| s.quantity),
            Some(7)
        );
    }

    #[test]
    fn remove_stack_on_empty_slot_is_none() {
        let mut inventory = PlayerInventoryState::empty();
        assert!(remove_stack(&mut inventory, ItemContainerSlot::inventory(0), None).is_none());
    }
}
