//! Slot-manipulation helpers shared by the loot-transfer command handlers.
//!
//! These move `ItemStack`s between the looter's inventory/actionbar and the
//! "container" they have open. The container is either a world loot bag (a flat
//! slot vec) or a logged-out sleeper's *live* inventory (read/written in place,
//! so looting a sleeper is non-destructive: only what's taken leaves the body).
//! [`ContainerSlots`] hides that difference so the move/quick-transfer logic
//! doesn't care which it's shuffling stacks into.
//!
//! The take/insert/restore arithmetic itself lives in
//! [`crate::server::container_slots`], shared with the furnace; this module only
//! supplies the loot-bag/sleeper [`Container`] impl and the player-side slot
//! resolution.
//!
//! They're kept out of `loot_bag.rs` so the command-routing `impl GameServer`
//! block stays focused on flow rather than slot arithmetic.

use crate::{
    protocol::{
        INVENTORY_SLOT_COUNT, ItemStack, LOOT_BAG_SLOT_COUNT, LootBagSlotRef, PlayerInventoryState,
    },
    server::container_slots::{Container, PlayerSlot, SlotSide, take_from_slot},
};

// `insert_into_slot` and `restore_slot` are re-exported (not just `use`d) so the
// loot-bag unit tests can exercise these slot-level primitives directly through
// `super::slots::*`; the implementations live in the shared module.
pub(super) use crate::server::container_slots::{insert_into_slot, reply_warning, restore_slot};

/// The non-player side of an open container, abstracting a loot bag's flat slot
/// vec from a sleeping player's split inventory/actionbar.
pub(super) enum ContainerSlots<'a> {
    /// A world loot bag's slots (a death drop).
    Bag(&'a mut Vec<Option<ItemStack>>),
    /// A sleeping player's live inventory. Flat container index `0..INVENTORY`
    /// maps to the backpack, the next `ACTIONBAR` indices to the hotbar.
    Sleeper(&'a mut PlayerInventoryState),
}

impl Container for ContainerSlots<'_> {
    fn slot_count(&self) -> usize {
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

    fn insert(&mut self, index: usize, stack: ItemStack) -> Option<ItemStack> {
        let target = self.slot_mut(index)?;
        insert_into_slot(target, stack)
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

/// Normalise a loot-bag slot ref into the shared player/container distinction.
fn side_of(slot: LootBagSlotRef) -> SlotSide {
    match slot {
        LootBagSlotRef::PlayerInventory(index) => SlotSide::Player(PlayerSlot::Inventory(index)),
        LootBagSlotRef::PlayerActionbar(index) => SlotSide::Player(PlayerSlot::Actionbar(index)),
        LootBagSlotRef::Bag(index) => SlotSide::Container(index),
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
    let from = side_of(from);
    let to = side_of(to);
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
    match side_of(from) {
        SlotSide::Container(index) => {
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
        SlotSide::Player(player_slot) => {
            let stack = player_slot.resolve(looter).and_then(Option::take)?;
            // First empty container slot.
            for index in 0..container.slot_count() {
                if let Some(target) = container.slot_mut(index)
                    && target.is_none()
                {
                    *target = Some(stack);
                    return None;
                }
            }
            // Container full, restore the stack to its origin so the player
            // doesn't drop it on the floor by accident.
            if let Some(target) = player_slot.resolve(looter) {
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
    slot: SlotSide,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    let target = match slot {
        SlotSide::Container(index) => container.slot_mut(index)?,
        SlotSide::Player(player_slot) => player_slot.resolve(looter)?,
    };
    take_from_slot(target, quantity)
}

/// Insert a stack into a slot ref. Returns the leftover if the destination
/// couldn't fit everything (capacity overflow or mismatched item id).
fn insert_into_ref(
    looter: &mut PlayerInventoryState,
    container: &mut ContainerSlots,
    slot: SlotSide,
    stack: ItemStack,
) -> Option<ItemStack> {
    match slot {
        SlotSide::Container(index) => container.insert(index, stack),
        SlotSide::Player(player_slot) => {
            let target = player_slot.resolve(looter)?;
            insert_into_slot(target, stack)
        }
    }
}

/// Restore an `ItemStack` to its source slot after a failed `Move`, so the
/// player doesn't lose items.
fn restore_into_ref(
    looter: &mut PlayerInventoryState,
    container: &mut ContainerSlots,
    slot: SlotSide,
    stack: ItemStack,
    removed_all: bool,
) {
    let target = match slot {
        SlotSide::Container(index) => container.slot_mut(index),
        SlotSide::Player(player_slot) => player_slot.resolve(looter),
    };
    if let Some(target) = target {
        restore_slot(target, stack, removed_all);
    }
}
