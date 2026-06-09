use super::slots::{insert_into_slot, restore_slot};
use super::*;
use crate::items::{COAL_ID, WOOD_ID, stack_limit};
use crate::server::test_support::server;

#[test]
fn insert_into_empty_slot_fills_it() {
    let mut slot: Option<ItemStack> = None;
    let leftover = insert_into_slot(&mut slot, ItemStack::new(WOOD_ID, 5));
    assert!(leftover.is_none());
    assert_eq!(slot.as_ref().unwrap().quantity, 5);
}

#[test]
fn insert_into_matching_slot_merges_up_to_limit_and_returns_overflow() {
    let limit = stack_limit(WOOD_ID).unwrap();
    let mut slot = Some(ItemStack::new(WOOD_ID, limit - 2));
    // Incoming 5 → only 2 fit, 3 overflow back.
    let leftover = insert_into_slot(&mut slot, ItemStack::new(WOOD_ID, 5));
    assert_eq!(slot.as_ref().unwrap().quantity, limit);
    let overflow = leftover.expect("overflow returned when limit is hit");
    assert_eq!(overflow.quantity, 3);
    assert_eq!(overflow.item_id.as_ref(), WOOD_ID);
}

#[test]
fn insert_into_full_matching_slot_rejects_entire_stack() {
    let limit = stack_limit(WOOD_ID).unwrap();
    let mut slot = Some(ItemStack::new(WOOD_ID, limit));
    let leftover = insert_into_slot(&mut slot, ItemStack::new(WOOD_ID, 4));
    assert_eq!(
        slot.as_ref().unwrap().quantity,
        limit,
        "full slot is unchanged"
    );
    assert_eq!(leftover.expect("rejected").quantity, 4);
}

#[test]
fn insert_into_mismatched_slot_swaps_contents() {
    let mut slot = Some(ItemStack::new(COAL_ID, 3));
    let displaced = insert_into_slot(&mut slot, ItemStack::new(WOOD_ID, 1));
    // The incoming item now occupies the slot; the old stack is returned.
    assert_eq!(slot.as_ref().unwrap().item_id.as_ref(), WOOD_ID);
    let out = displaced.expect("mismatch swaps the old stack out");
    assert_eq!(out.item_id.as_ref(), COAL_ID);
    assert_eq!(out.quantity, 3);
}

#[test]
fn restore_slot_merges_partial_back_into_same_item() {
    // removed_all = false + matching item → quantities add.
    let mut slot = Some(ItemStack::new(WOOD_ID, 6));
    restore_slot(&mut slot, ItemStack::new(WOOD_ID, 4), false);
    assert_eq!(slot.as_ref().unwrap().quantity, 10);
}

#[test]
fn restore_slot_overwrites_when_source_was_fully_drained() {
    // removed_all = true → the stack is placed straight back (slot was
    // emptied during the take, so there's nothing to merge with).
    let mut slot: Option<ItemStack> = None;
    restore_slot(&mut slot, ItemStack::new(COAL_ID, 7), true);
    assert_eq!(slot.as_ref().unwrap().item_id.as_ref(), COAL_ID);
    assert_eq!(slot.as_ref().unwrap().quantity, 7);
}

#[test]
fn loot_bag_is_empty_reflects_slot_contents() {
    let mut bag = LootBag {
        id: 1,
        position: Vec3Net::ZERO,
        yaw: 0.0,
        slots: vec![None; LOOT_BAG_SLOT_COUNT],
        spawn_tick: 0,
        velocity_y: 0.0,
        resting: true,
    };
    assert!(bag.is_empty());
    bag.slots[2] = Some(ItemStack::new(WOOD_ID, 1));
    assert!(!bag.is_empty());

    // to_view mirrors the slot layout.
    let view = bag.to_view();
    assert_eq!(view.id, 1);
    assert_eq!(view.slots[2].as_ref().unwrap().quantity, 1);
}

#[test]
fn spawn_loot_bag_truncates_overflowing_item_list() {
    let mut server = server();
    // More stacks than the bag has slots, the extras are dropped.
    let items: Vec<ItemStack> = (0..(LOOT_BAG_SLOT_COUNT + 3))
        .map(|_| ItemStack::new(WOOD_ID, 1))
        .collect();
    let id = server.spawn_loot_bag(Vec3Net::ZERO, 0.0, items);
    let bag = &server.loot_bags[&id];
    assert_eq!(bag.slots.len(), LOOT_BAG_SLOT_COUNT);
    assert!(bag.slots.iter().all(Option::is_some));
}
