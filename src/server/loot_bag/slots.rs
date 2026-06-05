//! Slot-manipulation helpers shared by the loot-transfer command handlers.
//!
//! These move `ItemStack`s between the looter's inventory/actionbar and the
//! "container" they have open. The container is either a world loot bag (a flat
//! slot vec) or a logged-out sleeper's *live* inventory (read/written in place,
//! so looting a sleeper is non-destructive: only what's taken leaves the body).
//! [`ContainerSlots`] hides that difference so the move/quick-transfer logic
//! doesn't care which it's shuffling stacks into.
//!
//! They're kept out of `loot_bag.rs` so the command-routing `impl GameServer`
//! block stays focused on flow rather than slot arithmetic.

use crate::{
    protocol::{
        ClientId, INVENTORY_SLOT_COUNT, ItemStack, LOOT_BAG_SLOT_COUNT, LootBagSlotRef,
        PlayerInventoryState, ServerMessage, ToastKind, ToastMessage,
    },
    server::{DeliveryTarget, ServerEnvelope},
};

/// The non-player side of an open container, abstracting a loot bag's flat slot
/// vec from a sleeping player's split inventory/actionbar.
pub(super) enum ContainerSlots<'a> {
    /// A world loot bag's slots (a death drop).
    Bag(&'a mut Vec<Option<ItemStack>>),
    /// A sleeping player's live inventory. Flat container index `0..INVENTORY`
    /// maps to the backpack, the next `ACTIONBAR` indices to the hotbar.
    Sleeper(&'a mut PlayerInventoryState),
}

impl ContainerSlots<'_> {
    fn len(&self) -> usize {
        match self {
            ContainerSlots::Bag(slots) => slots.len(),
            ContainerSlots::Sleeper(_) => LOOT_BAG_SLOT_COUNT,
        }
    }

    fn slot_mut(&mut self, index: usize) -> Option<&mut Option<ItemStack>> {
        match self {
            ContainerSlots::Bag(slots) => slots.get_mut(index),
            ContainerSlots::Sleeper(inventory) => sleeper_slot_mut(inventory, index),
        }
    }
}

/// Map a flat container index onto a sleeper's split inventory: the first
/// [`INVENTORY_SLOT_COUNT`] indices are backpack slots, the rest the hotbar.
fn sleeper_slot_mut(
    inventory: &mut PlayerInventoryState,
    index: usize,
) -> Option<&mut Option<ItemStack>> {
    if index < INVENTORY_SLOT_COUNT {
        inventory.inventory_slots.get_mut(index)
    } else {
        inventory
            .actionbar_slots
            .get_mut(index - INVENTORY_SLOT_COUNT)
    }
}

/// Resolve a player-side slot ref against the *looter's* inventory. `Bag` refs
/// belong to the container, not the looter, so they resolve to `None` here.
fn looter_slot_mut(
    looter: &mut PlayerInventoryState,
    slot: LootBagSlotRef,
) -> Option<&mut Option<ItemStack>> {
    match slot {
        LootBagSlotRef::PlayerInventory(index) => looter.inventory_slots.get_mut(index),
        LootBagSlotRef::PlayerActionbar(index) => looter.actionbar_slots.get_mut(index),
        LootBagSlotRef::Bag(_) => None,
    }
}

/// Move a stack from `from` to `to`, where each end is either the looter's
/// inventory/actionbar or the open container's slots. Atomic: if the
/// destination can't take everything, the remainder is restored at the source.
pub(super) fn move_within(
    looter: &mut PlayerInventoryState,
    container: &mut ContainerSlots,
    from: LootBagSlotRef,
    to: LootBagSlotRef,
    quantity: Option<u16>,
) {
    let Some((took, all_consumed)) = take_from_ref(looter, container, from, quantity) else {
        return;
    };
    if let Some(remainder) = insert_into_ref(looter, container, to, took) {
        restore_into_ref(looter, container, from, remainder, all_consumed);
    }
}

/// Shift-click "send this somewhere useful". From a container slot the stack
/// flows back into the looter's inventory (merging into matching stacks); from a
/// looter slot it lands in the first empty container slot. Returns a warning
/// message when the destination is full (the stack is left where it started).
pub(super) fn quick_transfer_within(
    looter: &mut PlayerInventoryState,
    container: &mut ContainerSlots,
    from: LootBagSlotRef,
) -> Option<&'static str> {
    match from {
        LootBagSlotRef::Bag(index) => {
            let stack = container.slot_mut(index).and_then(Option::take)?;
            if let Some(leftover) = crate::server::inventory::add_stack_to_inventory(looter, stack)
            {
                // Couldn't fit it all, put what didn't fit back.
                if let Some(target) = container.slot_mut(index) {
                    *target = Some(leftover);
                }
                return Some("Inventory full");
            }
            None
        }
        LootBagSlotRef::PlayerInventory(_) | LootBagSlotRef::PlayerActionbar(_) => {
            let stack = looter_slot_mut(looter, from).and_then(Option::take)?;
            // First empty container slot.
            for index in 0..container.len() {
                if let Some(target) = container.slot_mut(index)
                    && target.is_none()
                {
                    *target = Some(stack);
                    return None;
                }
            }
            // Container full, restore the stack to its origin so the player
            // doesn't drop it on the floor by accident.
            if let Some(target) = looter_slot_mut(looter, from) {
                *target = Some(stack);
            }
            Some("No room left")
        }
    }
}

/// Pull a stack out of a slot ref. Returns `(taken, all_consumed)`, `taken` is
/// what was extracted, `all_consumed` is true if the source slot is now empty
/// (used to decide whether to merge on a failed-move restore).
fn take_from_ref(
    looter: &mut PlayerInventoryState,
    container: &mut ContainerSlots,
    slot: LootBagSlotRef,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    let target = match slot {
        LootBagSlotRef::Bag(index) => container.slot_mut(index)?,
        _ => looter_slot_mut(looter, slot)?,
    };
    let current = target.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let all = amount == current.quantity;
    let taken = ItemStack::new(current.item_id.as_ref(), amount);
    current.quantity -= amount;
    if current.quantity == 0 {
        *target = None;
    }
    Some((taken, all))
}

/// Insert a stack into a slot ref. Returns the leftover if the destination
/// couldn't fit everything (capacity overflow or mismatched item id).
fn insert_into_ref(
    looter: &mut PlayerInventoryState,
    container: &mut ContainerSlots,
    slot: LootBagSlotRef,
    stack: ItemStack,
) -> Option<ItemStack> {
    let target = match slot {
        LootBagSlotRef::Bag(index) => container.slot_mut(index)?,
        _ => looter_slot_mut(looter, slot)?,
    };
    insert_into_slot(target, stack)
}

/// Restore an `ItemStack` to its source slot after a failed `Move`, so the
/// player doesn't lose items.
fn restore_into_ref(
    looter: &mut PlayerInventoryState,
    container: &mut ContainerSlots,
    slot: LootBagSlotRef,
    stack: ItemStack,
    removed_all: bool,
) {
    let target = match slot {
        LootBagSlotRef::Bag(index) => container.slot_mut(index),
        _ => looter_slot_mut(looter, slot),
    };
    if let Some(target) = target {
        restore_slot(target, stack, removed_all);
    }
}

/// Slot-level insert helper. If the target is empty, the stack moves in whole.
/// If it holds the same item id, the quantities merge up to the item's stack
/// limit, returning any overflow. Mismatched ids swap (returns the original
/// contents).
pub(super) fn insert_into_slot(
    target: &mut Option<ItemStack>,
    incoming: ItemStack,
) -> Option<ItemStack> {
    match target {
        None => {
            *target = Some(incoming);
            None
        }
        Some(existing) if existing.item_id == incoming.item_id => {
            let limit = crate::items::stack_limit(&existing.item_id).unwrap_or(u16::MAX);
            let space = limit.saturating_sub(existing.quantity);
            if space == 0 {
                return Some(incoming);
            }
            let take = incoming.quantity.min(space);
            existing.quantity += take;
            let remainder = incoming.quantity - take;
            if remainder == 0 {
                None
            } else {
                Some(ItemStack::new(incoming.item_id.as_ref(), remainder))
            }
        }
        Some(existing) => {
            // Swap: caller wanted to put `incoming` here but a different item is
            // in the way. Move the existing stack out and put the incoming one in.
            let displaced = existing.clone();
            *target = Some(incoming);
            Some(displaced)
        }
    }
}

pub(super) fn restore_slot(target: &mut Option<ItemStack>, stack: ItemStack, removed_all: bool) {
    match (target.as_mut(), removed_all) {
        (Some(existing), false) if existing.item_id == stack.item_id => {
            existing.quantity = existing.quantity.saturating_add(stack.quantity);
        }
        _ => {
            *target = Some(stack);
        }
    }
}

pub(super) fn reply_warning(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(ToastKind::Warning, text)),
    }]
}
