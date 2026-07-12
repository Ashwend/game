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
    items::{armor_profile, item_definition, normalize_stack, stack_limit},
    protocol::{
        ACTIONBAR_SLOT_COUNT, EQUIPMENT_SLOT_COUNT, EquipmentSlot, INVENTORY_SLOT_COUNT,
        ItemContainer, ItemContainerSlot, ItemStack, PlayerInventoryState,
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

    // Moves into a paperdoll slot take a dedicated equip path: slot-type
    // validation plus always-swap semantics (armor never merges, so equipping
    // over an occupied slot swaps the incumbent out, even for two pieces sharing
    // an item id). Runs in the shared path so the server and the client's
    // optimistic prediction reach the same decision by construction. A rejected
    // equip mutates nothing (validation happens before any state change).
    if to.container == ItemContainer::Equipment {
        equip_move(inventory, from, to, quantity);
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

/// Perform a validated move whose destination is an [`ItemContainer::Equipment`]
/// slot. Rejects (leaving the inventory untouched) unless the source is a single
/// armor piece whose profile slot matches the destination. On success it swaps:
/// the source piece is worn and any incumbent piece is placed back in the source
/// slot. Swap (never merge) is the right semantics for the paperdoll, two pieces
/// of the same slot type, even the same item id, exchange rather than stacking
/// (armor's stack limit is one).
fn equip_move(
    inventory: &mut PlayerInventoryState,
    from: ItemContainerSlot,
    to: ItemContainerSlot,
    quantity: Option<u16>,
) {
    if !equip_move_is_valid(inventory, from, to, quantity) {
        return;
    }
    // Take the whole source piece (armor is always a stack of one), then swap it
    // with whatever currently occupies the destination equipment slot.
    let Some((moving, _)) = remove_stack_for_move(inventory, from, None) else {
        return;
    };
    let Some(target) = slot_mut(inventory, to) else {
        // Should not happen (validated above), but restore the source rather
        // than dropping the piece if it somehow does.
        restore_stack(inventory, from, moving);
        return;
    };
    let displaced = target.replace(moving);
    // The incumbent (if any) goes back to the now-empty source slot. It is armor
    // of the same slot type, so it lands in the bag/actionbar cleanly.
    if let Some(displaced) = displaced {
        restore_stack(inventory, from, displaced);
    }
}

/// Whether a move whose destination is an [`ItemContainer::Equipment`] slot is
/// allowed. The source stack must be a single armor piece whose
/// [`crate::items::ArmorProfile`] slot matches the target paperdoll slot, and
/// the caller must be moving the whole stack (armor never stacks, and a partial
/// equip makes no sense). Reads only, so a `false` result leaves the inventory
/// untouched.
fn equip_move_is_valid(
    inventory: &PlayerInventoryState,
    from: ItemContainerSlot,
    to: ItemContainerSlot,
    quantity: Option<u16>,
) -> bool {
    let Some(target_slot) = EquipmentSlot::from_index(to.slot) else {
        return false;
    };
    let Some(source) = inventory.slot(from) else {
        return false;
    };
    // Whole-stack only: an armor stack is always quantity 1, so any explicit
    // partial quantity is a malformed equip request. `None` means "move all",
    // which is fine.
    if quantity.is_some_and(|q| q != source.quantity) {
        return false;
    }
    // Non-armor items (and unknown ids) carry no armor profile, so they can
    // never enter a paperdoll slot. The profile's declared slot must match the
    // destination (a helmet only in the head slot).
    armor_profile(&source.item_id).is_some_and(|profile| profile.slot == target_slot)
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
    // Carry durability along rather than rebuilding via `ItemStack::new`,
    // which would hand back a factory-fresh tool. Tools never stack (limit
    // 1) so a move is always whole; splitting a stackable item (no
    // durability) keeps the source's `None` either way.
    let durability = current.durability;
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some((
        ItemStack {
            item_id,
            quantity: amount,
            durability,
        },
        removed_all,
    ))
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
    // Preserve durability so a dropped or relocated worn tool keeps its
    // wear instead of coming back factory-fresh (see `remove_stack_for_move`).
    let durability = current.durability;
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some(ItemStack {
        item_id,
        quantity: amount,
        durability,
    })
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
/// Total units of `item_id` across the actionbar + inventory. Used by
/// cost checks ("can the player afford this?") before a
/// [`take_items_from_inventory`] so a failed purchase never drains a
/// partial amount.
pub fn count_items_in_inventory(inventory: &PlayerInventoryState, item_id: &str) -> u32 {
    inventory
        .actionbar_slots
        .iter()
        .chain(inventory.inventory_slots.iter())
        .filter_map(|slot| slot.as_ref())
        .filter(|stack| stack.item_id.as_ref() == item_id)
        .map(|stack| u32::from(stack.quantity))
        .sum()
}

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
/// actionbar slot over the bag. True for the items the player equips and uses
/// directly: tools, deployables, and the combat weapons (melee + ranged). A
/// freshly crafted or picked-up sword, bow, or crossbow is a quick-access item
/// exactly like a hatchet, so it lands on the toolbar before spilling into the
/// bag. False for everything else, so gathered resources still flow into the
/// main inventory as before.
fn prefers_actionbar(item_id: &str) -> bool {
    item_definition(item_id).is_some_and(|def| {
        def.tool.is_some()
            || def.deployable.is_some()
            || def.weapon.is_some()
            || def.ranged.is_some()
    })
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
        ItemContainer::Equipment => inventory.equipment_slots.get_mut(slot.slot),
    }
}

fn slot_exists(inventory: &PlayerInventoryState, slot: ItemContainerSlot) -> bool {
    (match slot.container {
        ItemContainer::Inventory => slot.slot < INVENTORY_SLOT_COUNT,
        ItemContainer::Actionbar => slot.slot < ACTIONBAR_SLOT_COUNT,
        ItemContainer::Equipment => slot.slot < EQUIPMENT_SLOT_COUNT,
    }) && (match slot.container {
        ItemContainer::Inventory => slot.slot < inventory.inventory_slots.len(),
        ItemContainer::Actionbar => slot.slot < inventory.actionbar_slots.len(),
        ItemContainer::Equipment => slot.slot < inventory.equipment_slots.len(),
    })
}

pub fn offset_actionbar_slot(current: usize, offset: i8) -> usize {
    (current as isize + offset as isize).rem_euclid(ACTIONBAR_SLOT_COUNT as isize) as usize
}

/// Auto-stack and tidy the main inventory bag in place. Partial stacks of the
/// same item merge up to that item's stack limit, the distinct items are
/// ordered by display name, and the freed slots collect at the end.
///
/// The actionbar is intentionally left untouched: it's the player's curated
/// quick-access row, reordering it would shuffle tools out from under the
/// number keys. Pure (no `GameServer`, no ECS), so it lives here with the rest
/// of the shared inventory math and is unit-tested directly.
pub fn sort_inventory(inventory: &mut PlayerInventoryState) {
    // Sum every distinct item's total quantity across the bag. Stack-limit
    // boundaries are re-applied on the way out, so the merge can ignore them.
    let mut totals: Vec<(crate::items::ItemId, u32)> = Vec::new();
    for stack in inventory.inventory_slots.iter().flatten() {
        if stack.quantity == 0 {
            continue;
        }
        if let Some(entry) = totals.iter_mut().find(|entry| entry.0 == stack.item_id) {
            entry.1 += stack.quantity as u32;
        } else {
            totals.push((stack.item_id.clone(), stack.quantity as u32));
        }
    }

    // Order by display name, then id, so the layout is deterministic and reads
    // alphabetically rather than by whatever pickup order filled the bag.
    totals.sort_by(|a, b| {
        let name_a = item_definition(a.0.as_ref())
            .map(|def| def.name)
            .unwrap_or(a.0.as_ref());
        let name_b = item_definition(b.0.as_ref())
            .map(|def| def.name)
            .unwrap_or(b.0.as_ref());
        name_a
            .to_ascii_lowercase()
            .cmp(&name_b.to_ascii_lowercase())
            .then_with(|| a.0.as_ref().cmp(b.0.as_ref()))
    });

    // Repack from slot 0 as full-then-remainder stacks. Merging can only ever
    // free slots, never need more, so the result always fits.
    for slot in inventory.inventory_slots.iter_mut() {
        *slot = None;
    }
    let mut index = 0;
    for (item_id, mut remaining) in totals {
        let limit = stack_limit(&item_id).unwrap_or(1).max(1) as u32;
        while remaining > 0 && index < inventory.inventory_slots.len() {
            let take = remaining.min(limit) as u16;
            inventory.inventory_slots[index] = Some(ItemStack::new(item_id.clone(), take));
            remaining -= take as u32;
            index += 1;
        }
    }
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

    #[test]
    fn moving_a_worn_tool_keeps_its_durability() {
        // A 50%-worn pickaxe dragged from the actionbar into the bag must
        // arrive with the same wear, not reset to factory-fresh.
        let mut inventory = PlayerInventoryState::empty();
        let mut pickaxe = ItemStack::new(BASIC_PICKAXE_ID, 1);
        let full = pickaxe.durability.expect("pickaxe wears");
        let worn = full / 2;
        pickaxe.durability = Some(worn);
        inventory.actionbar_slots[0] = Some(pickaxe);

        move_stack(
            &mut inventory,
            ItemContainerSlot::actionbar(0),
            ItemContainerSlot::inventory(3),
            None,
        );

        assert!(inventory.actionbar_slots[0].is_none());
        assert_eq!(
            inventory.inventory_slots[3]
                .as_ref()
                .and_then(|stack| stack.durability),
            Some(worn)
        );
    }

    #[test]
    fn dropping_a_worn_tool_keeps_its_durability() {
        // `remove_stack` is the drop path; the ejected stack must carry the
        // tool's remaining wear so the world drop isn't factory-fresh.
        let mut inventory = PlayerInventoryState::empty();
        let mut pickaxe = ItemStack::new(BASIC_PICKAXE_ID, 1);
        pickaxe.durability = Some(7);
        inventory.inventory_slots[0] = Some(pickaxe);

        let dropped = remove_stack(&mut inventory, ItemContainerSlot::inventory(0), None)
            .expect("stack removed");
        assert_eq!(dropped.durability, Some(7));
    }

    #[test]
    fn sort_merges_partial_stacks_and_packs_to_front() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[3] = Some(ItemStack::new(COAL_ID, 5));
        inventory.inventory_slots[7] = Some(ItemStack::new(COAL_ID, 8));
        inventory.inventory_slots[1] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));

        sort_inventory(&mut inventory);

        // Two distinct items survive, both packed to the front; the coal's two
        // partial stacks merged into one of 13.
        let occupied: Vec<_> = inventory
            .inventory_slots
            .iter()
            .filter_map(|slot| slot.as_ref())
            .collect();
        assert_eq!(occupied.len(), 2);
        let coal_qty: u16 = inventory
            .inventory_slots
            .iter()
            .flatten()
            .filter(|stack| stack.item_id.as_ref() == COAL_ID)
            .map(|stack| stack.quantity)
            .sum();
        assert_eq!(coal_qty, 13);
        assert!(inventory.inventory_slots[0].is_some());
        assert!(inventory.inventory_slots[1].is_some());
        assert!(inventory.inventory_slots[2..].iter().all(Option::is_none));
    }

    #[test]
    fn sort_splits_when_total_exceeds_stack_limit() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 150));
        inventory.inventory_slots[5] = Some(ItemStack::new(COAL_ID, 100));

        sort_inventory(&mut inventory);

        // 250 coal at a 200 cap repacks into a full 200 stack plus a 50
        // remainder, contiguous from slot 0.
        assert_eq!(
            inventory.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(200)
        );
        assert_eq!(
            inventory.inventory_slots[1].as_ref().map(|s| s.quantity),
            Some(50)
        );
        assert!(inventory.inventory_slots[2..].iter().all(Option::is_none));
    }

    #[test]
    fn sort_leaves_the_actionbar_untouched() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.actionbar_slots[2] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
        inventory.inventory_slots[4] = Some(ItemStack::new(COAL_ID, 3));

        sort_inventory(&mut inventory);

        // The pickaxe stays in its actionbar slot; only the bag is repacked.
        assert_eq!(
            inventory.actionbar_slots[2]
                .as_ref()
                .map(|s| s.item_id.as_ref()),
            Some(BASIC_PICKAXE_ID)
        );
        assert_eq!(
            inventory.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(3)
        );
    }

    // ---- Equipment slot-type validation (shared move path) ----

    use crate::items::{PADDED_HOOD_ID, PADDED_TUNIC_ID};
    use crate::protocol::EquipmentSlot;

    #[test]
    fn move_armor_into_its_matching_slot_equips_it() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(PADDED_HOOD_ID, 1));

        move_stack(
            &mut inventory,
            ItemContainerSlot::inventory(0),
            ItemContainerSlot::equipment(EquipmentSlot::Head),
            None,
        );

        // The hood moved into the head slot and left the bag.
        assert!(inventory.inventory_slots[0].is_none());
        assert_eq!(
            inventory
                .equipment(EquipmentSlot::Head)
                .map(|stack| stack.item_id.as_ref()),
            Some(PADDED_HOOD_ID)
        );
    }

    #[test]
    fn move_armor_into_the_wrong_slot_is_rejected_cleanly() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(PADDED_HOOD_ID, 1));

        // A hood is a head piece; the chest slot must refuse it.
        move_stack(
            &mut inventory,
            ItemContainerSlot::inventory(0),
            ItemContainerSlot::equipment(EquipmentSlot::Chest),
            None,
        );

        // Nothing moved: the hood is still in the bag and the chest slot is empty
        // (no partial state).
        assert_eq!(
            inventory.inventory_slots[0]
                .as_ref()
                .map(|stack| stack.item_id.as_ref()),
            Some(PADDED_HOOD_ID)
        );
        assert!(inventory.equipment(EquipmentSlot::Chest).is_none());
    }

    #[test]
    fn move_non_armor_into_an_equipment_slot_is_rejected() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 5));

        move_stack(
            &mut inventory,
            ItemContainerSlot::inventory(0),
            ItemContainerSlot::equipment(EquipmentSlot::Head),
            None,
        );

        // The coal never enters the paperdoll and stays put in the bag.
        assert_eq!(
            inventory.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(5)
        );
        assert!(inventory.equipment(EquipmentSlot::Head).is_none());
    }

    #[test]
    fn swapping_two_same_slot_armor_pieces_via_equip_move() {
        // A hood is worn; a second hood in the bag is dragged onto the same slot.
        // The worn piece swaps out to the bag, the new one equips. Both are head
        // pieces, so the move is valid.
        let mut inventory = PlayerInventoryState::empty();
        let mut worn = ItemStack::new(PADDED_HOOD_ID, 1);
        worn.durability = Some(10); // a distinctly-worn incumbent
        inventory.equipment_slots[EquipmentSlot::Head.index()] = Some(worn);
        inventory.inventory_slots[0] = Some(ItemStack::new(PADDED_HOOD_ID, 1));

        move_stack(
            &mut inventory,
            ItemContainerSlot::inventory(0),
            ItemContainerSlot::equipment(EquipmentSlot::Head),
            None,
        );

        // The freshly-equipped hood is at full durability; the swapped-out
        // incumbent (durability 10) landed back in the source bag slot.
        assert_eq!(
            inventory
                .equipment(EquipmentSlot::Head)
                .and_then(|stack| stack.durability),
            Some(crate::game_balance::PADDED_ARMOR_DURABILITY)
        );
        assert_eq!(
            inventory.inventory_slots[0]
                .as_ref()
                .and_then(|stack| stack.durability),
            Some(10)
        );
    }

    #[test]
    fn moving_armor_out_of_an_equipment_slot_follows_existing_rules() {
        // Unequip: a worn piece moves back to the bag with no slot-type gate
        // (only moves INTO equipment are validated). Durability is preserved.
        let mut inventory = PlayerInventoryState::empty();
        let mut worn = ItemStack::new(PADDED_TUNIC_ID, 1);
        worn.durability = Some(42);
        inventory.equipment_slots[EquipmentSlot::Chest.index()] = Some(worn);

        move_stack(
            &mut inventory,
            ItemContainerSlot::equipment(EquipmentSlot::Chest),
            ItemContainerSlot::inventory(3),
            None,
        );

        assert!(inventory.equipment(EquipmentSlot::Chest).is_none());
        assert_eq!(
            inventory.inventory_slots[3]
                .as_ref()
                .and_then(|stack| stack.durability),
            Some(42)
        );
    }

    #[test]
    fn sort_leaves_equipment_untouched() {
        // sort_inventory only repacks the bag; a worn piece and the actionbar
        // stay exactly where they are.
        let mut inventory = PlayerInventoryState::empty();
        inventory.equipment_slots[EquipmentSlot::Head.index()] =
            Some(ItemStack::new(PADDED_HOOD_ID, 1));
        inventory.inventory_slots[4] = Some(ItemStack::new(COAL_ID, 3));

        sort_inventory(&mut inventory);

        assert_eq!(
            inventory
                .equipment(EquipmentSlot::Head)
                .map(|stack| stack.item_id.as_ref()),
            Some(PADDED_HOOD_ID)
        );
        // The bag was still repacked to the front.
        assert_eq!(
            inventory.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(3)
        );
    }
}
