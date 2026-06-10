//! Shared player <-> container stack-move primitives.
//!
//! Both the loot bag and the furnace let a player drag stacks between their own
//! inventory/actionbar and a "container" they have open. The move skeleton is
//! the same in both cases: pull a (partial) stack out of the source, push it
//! into the destination, and if the destination can't take everything, deal
//! with the remainder. What differs is the leaf policy at each end, and those
//! differences are expressed through the [`Container`] trait so the skeleton
//! itself stays single-sourced:
//!
//! - A loot bag is a flat slot vec; a sleeper is a split inventory; a furnace is
//!   a fuel slot plus an items grid. The trait hides which one a move shuffles
//!   stacks into.
//! - Loot bags swap mismatched items unconditionally and ignore stack limits
//!   when filling an empty slot; the furnace's container slots never swap, honour
//!   the stack limit even into an empty slot, and the fuel slot only accepts fuel
//!   items. Each impl supplies its own slot-insert policy.
//! - The furnace resets its in-flight burn timer when fuel leaves the fuel slot
//!   ([`Container::after_take`]), routes shift-clicks by item kind, and drops
//!   un-restorable leftovers at the player's feet; loot bags merge leftovers back
//!   into the source slot and shift-click into the first empty slot. Those are
//!   trait hooks layered on top of the shared skeleton, not baked into it.
//!
//! Keeping the skeleton here means neither `loot_bag.rs` nor `furnace/commands.rs`
//! re-implements the take/insert/restore arithmetic.

use crate::{
    items::stack_limit,
    protocol::{ClientId, ItemStack, PlayerInventoryState, ServerMessage, ToastKind, ToastMessage},
    server::{DeliveryTarget, ServerEnvelope},
};

/// A slot in the operating player's own inventory or actionbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayerSlot {
    Inventory(usize),
    Actionbar(usize),
}

impl PlayerSlot {
    pub(crate) fn resolve(
        self,
        player: &mut PlayerInventoryState,
    ) -> Option<&mut Option<ItemStack>> {
        match self {
            PlayerSlot::Inventory(index) => player.inventory_slots.get_mut(index),
            PlayerSlot::Actionbar(index) => player.actionbar_slots.get_mut(index),
        }
    }
}

/// Which side of an open container a slot ref points at, normalised away from
/// the consumer-specific `LootBagSlotRef` / `FurnaceSlotRef` enums so the shared
/// skeleton can route a move without knowing the wire shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotSide {
    /// A slot in the operating player's own inventory or actionbar.
    Player(PlayerSlot),
    /// A slot belonging to the open container. The `usize` is the container's
    /// own index space, interpreted by the [`Container`] impl.
    Container(usize),
}

/// The non-player side of an open container. Each impl owns its own index space
/// and its own per-slot placement policy; the shared skeleton only ever talks to
/// it through this trait.
pub(crate) trait Container {
    /// Number of addressable container slots, used by the loot-bag style
    /// "first empty slot" quick-transfer scan.
    fn slot_count(&self) -> usize;

    /// Direct mutable access to a container slot, used to take stacks out and to
    /// scan for an empty slot. Returns `None` for an out-of-range index.
    fn slot_mut(&mut self, index: usize) -> Option<&mut Option<ItemStack>>;

    /// Insert `stack` into container slot `index` using this container's own
    /// placement policy (merge / swap / reject / stack-limit). Returns the
    /// leftover that didn't fit, if any.
    fn insert(&mut self, index: usize, stack: ItemStack) -> Option<ItemStack>;

    /// Hook fired after a take from container slot `index`. `drained` is true if
    /// the slot is now empty. Default is a no-op; the furnace overrides it to
    /// cancel the in-flight burn timer when the fuel slot empties.
    fn after_take(&mut self, index: usize, drained: bool) {
        let _ = (index, drained);
    }
}

/// Pull up to `quantity` (or the whole stack) out of a slot. Returns the taken
/// stack and whether the slot is now empty.
pub(crate) fn take_from_slot(
    slot: &mut Option<ItemStack>,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    let current = slot.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let drained = amount == current.quantity;
    let taken = ItemStack::new(current.item_id.as_ref(), amount);
    current.quantity -= amount;
    if current.quantity == 0 {
        *slot = None;
    }
    Some((taken, drained))
}

/// Slot-level insert with unconditional swap on a mismatched id. Used by the
/// loot bag (both ends) where dragging a stack onto a different item swaps them.
/// An empty slot takes the whole incoming stack (no stack-limit clamp). A
/// matching slot merges up to the item's stack limit, returning any overflow.
pub(crate) fn insert_into_slot(
    target: &mut Option<ItemStack>,
    incoming: ItemStack,
) -> Option<ItemStack> {
    match target {
        None => {
            *target = Some(incoming);
            None
        }
        Some(existing) if existing.item_id == incoming.item_id => {
            let limit = stack_limit(&existing.item_id).unwrap_or(u16::MAX);
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

/// Merge `incoming` into `target` honouring the item's stack limit even into an
/// empty slot. A mismatched id is rejected (no swap). Used by the furnace's
/// container slots, where a drag onto a different item must not swap.
pub(crate) fn merge_into_optional_slot(
    target: &mut Option<ItemStack>,
    mut incoming: ItemStack,
) -> Option<ItemStack> {
    let limit = stack_limit(incoming.item_id.as_ref()).unwrap_or(u16::MAX);
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
            // Carry durability along: a tool (stack limit 1) always moves
            // whole, and rebuilding the stack without the field would hand
            // back a factory-fresh tool.
            let placed = ItemStack {
                item_id: incoming.item_id.clone(),
                quantity: take,
                durability: incoming.durability,
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

/// Restore an `ItemStack` to its source slot after a failed move. If the slot
/// still holds the same item and the source wasn't fully drained, quantities
/// merge; otherwise the stack is placed straight back.
pub(crate) fn restore_slot(target: &mut Option<ItemStack>, stack: ItemStack, removed_all: bool) {
    match (target.as_mut(), removed_all) {
        (Some(existing), false) if existing.item_id == stack.item_id => {
            existing.quantity = existing.quantity.saturating_add(stack.quantity);
        }
        _ => {
            *target = Some(stack);
        }
    }
}

pub(crate) fn reply_warning(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(ToastKind::Warning, text)),
    }]
}
