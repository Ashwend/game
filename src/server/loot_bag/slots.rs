//! Slot-manipulation helpers shared by the loot-bag command handlers.
//!
//! These are pure(ish) functions that move `ItemStack`s between a
//! `LootBag`'s slots and a player's inventory/actionbar. They're kept
//! out of `loot_bag.rs` so the command-routing `impl GameServer` block
//! stays focused on flow rather than slot arithmetic.

use std::collections::HashMap;

use crate::{
    protocol::{ClientId, ItemStack, LootBagSlotRef, ServerMessage, ToastKind, ToastMessage},
    server::{DeliveryTarget, ServerClient, ServerEnvelope},
};

use super::LootBag;

/// Pull a stack out of a `LootBagSlotRef`. Returns `(taken, all_consumed)`
///, `taken` is what was extracted, `all_consumed` is true if the
/// source slot is now empty (used to decide whether to restore the
/// quantity on a failed move).
pub(super) fn take_from_loot_ref(
    clients: &mut HashMap<ClientId, ServerClient>,
    client_id: ClientId,
    bag: &mut LootBag,
    slot: LootBagSlotRef,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    match slot {
        LootBagSlotRef::Bag(index) => {
            let target = bag.slots.get_mut(index)?;
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
        LootBagSlotRef::PlayerInventory(index) | LootBagSlotRef::PlayerActionbar(index) => {
            let client = clients.get_mut(&client_id)?;
            let slots = match slot {
                LootBagSlotRef::PlayerInventory(_) => &mut client.inventory.inventory_slots,
                LootBagSlotRef::PlayerActionbar(_) => &mut client.inventory.actionbar_slots,
                LootBagSlotRef::Bag(_) => unreachable!(),
            };
            let target = slots.get_mut(index)?;
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
    }
}

/// Insert a stack into a `LootBagSlotRef`. Returns the leftover stack
/// if the destination couldn't fit everything (capacity overflow or
/// mismatched item id in a non-empty slot).
pub(super) fn insert_into_loot_ref(
    clients: &mut HashMap<ClientId, ServerClient>,
    client_id: ClientId,
    bag: &mut LootBag,
    slot: LootBagSlotRef,
    stack: ItemStack,
) -> Option<ItemStack> {
    match slot {
        LootBagSlotRef::Bag(index) => {
            let target = bag.slots.get_mut(index)?;
            insert_into_slot(target, stack)
        }
        LootBagSlotRef::PlayerInventory(index) | LootBagSlotRef::PlayerActionbar(index) => {
            let client = clients.get_mut(&client_id)?;
            let slots = match slot {
                LootBagSlotRef::PlayerInventory(_) => &mut client.inventory.inventory_slots,
                LootBagSlotRef::PlayerActionbar(_) => &mut client.inventory.actionbar_slots,
                LootBagSlotRef::Bag(_) => unreachable!(),
            };
            let target = slots.get_mut(index)?;
            insert_into_slot(target, stack)
        }
    }
}

/// Slot-level insert helper. If the target is empty, the stack moves in
/// whole. If it holds the same item id, the quantities merge up to the
/// item's stack limit, returning any overflow. Mismatched ids swap
/// (returns the original contents).
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
            // Swap: caller wanted to put `incoming` here but a
            // different item is in the way. Move the existing stack
            // out and put the incoming one in.
            let displaced = existing.clone();
            *target = Some(incoming);
            Some(displaced)
        }
    }
}

/// Restore an `ItemStack` to its source slot after a failed `Move`.
/// Used when the destination rejected (full / mismatched item) so the
/// player doesn't lose items.
pub(super) fn restore_into_loot_ref(
    clients: &mut HashMap<ClientId, ServerClient>,
    client_id: ClientId,
    bag: &mut LootBag,
    slot: LootBagSlotRef,
    stack: ItemStack,
    removed_all: bool,
) {
    match slot {
        LootBagSlotRef::Bag(index) => {
            if let Some(target) = bag.slots.get_mut(index) {
                restore_slot(target, stack, removed_all);
            }
        }
        LootBagSlotRef::PlayerInventory(index) | LootBagSlotRef::PlayerActionbar(index) => {
            let Some(client) = clients.get_mut(&client_id) else {
                return;
            };
            let slots = match slot {
                LootBagSlotRef::PlayerInventory(_) => &mut client.inventory.inventory_slots,
                LootBagSlotRef::PlayerActionbar(_) => &mut client.inventory.actionbar_slots,
                LootBagSlotRef::Bag(_) => unreachable!(),
            };
            if let Some(target) = slots.get_mut(index) {
                restore_slot(target, stack, removed_all);
            }
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
