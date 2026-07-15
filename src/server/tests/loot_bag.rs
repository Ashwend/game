//! Server-authoritative loot bag tests.
//!
//! The combat tests already cover the death → spawn chain end-to-end;
//! this module focuses on the bag commands themselves (Open/Close/
//! Move/QuickTransfer) so the per-command branches are exercised
//! without going through a full kill.

use super::*;
use crate::{
    items::{COAL_ID, WOOD_ID},
    protocol::{
        AccountId, ClientMessage, GAME_VERSION, ItemStack, LOOT_BAG_SLOT_COUNT, LootBagCommand,
        LootBagSlotRef, PROTOCOL_VERSION, PlayerMovement, Vec3Net,
    },
    server::loot_bag::{LOOT_BAG_INTERACT_RANGE_M, OpenContainer},
};

fn connect_named(server: &mut GameServer, account_id: AccountId, name: &str) -> ClientId {
    server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            account_id,
            name.to_owned(),
            String::new(),
        )
        .expect("connect should succeed")
        .0
}

fn place_player(server: &mut GameServer, client_id: ClientId, position: Vec3Net) {
    let next_sequence = server
        .clients
        .get(&client_id)
        .map(|c| c.controller.last_processed_input.saturating_add(1))
        .unwrap_or(1);
    server.receive(
        client_id,
        ClientMessage::Movement(PlayerMovement {
            sequence: next_sequence,
            position,
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            grounded: true,
        }),
    );
}

fn apply(server: &mut GameServer, client: ClientId, cmd: LootBagCommand) {
    let _ = server.apply_loot_bag_command(client, cmd);
}

#[test]
fn spawn_loot_bag_lays_out_items_and_pads_with_empty_slots() {
    let mut server = server();
    let id = server.spawn_loot_bag(
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 5), ItemStack::new(COAL_ID, 2)],
    );
    let bag = server.loot_bags.get(&id).expect("bag exists after spawn");
    assert_eq!(bag.slots.len(), LOOT_BAG_SLOT_COUNT);
    assert_eq!(bag.slots[0].as_ref().unwrap().item_id.as_ref(), WOOD_ID);
    assert_eq!(bag.slots[0].as_ref().unwrap().quantity, 5);
    assert_eq!(bag.slots[1].as_ref().unwrap().item_id.as_ref(), COAL_ID);
    // Slots past the seeded items must be empty so the bag UI shows
    // free space.
    assert!(bag.slots[2..].iter().all(Option::is_none));
}

#[test]
fn open_loot_bag_within_range_records_open_state() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 3)],
    );

    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    assert_eq!(
        server.clients[&client].open_container,
        Some(OpenContainer::LootBag(bag_id)),
        "open should mark the client's open-container pointer"
    );
    // The replicated view helper should mirror the bag contents.
    let view = server
        .open_loot_bag_view_for(client)
        .expect("view present after open");
    assert_eq!(view.id, bag_id);
    assert_eq!(view.slots[0].as_ref().unwrap().quantity, 3);
}

#[test]
fn open_loot_bag_out_of_range_is_rejected() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "FarAway");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    // Bag well outside the interact radius.
    let far = LOOT_BAG_INTERACT_RANGE_M * 4.0;
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(far, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 1)],
    );

    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    assert!(
        server.clients[&client].open_container.is_none(),
        "out-of-range open must not set the open pointer"
    );
}

#[test]
fn close_loot_bag_empty_destroys_the_entity() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(Vec3Net::new(1.0, 0.0, 0.0), 0.0, Vec::new());

    apply(&mut server, client, LootBagCommand::Open { id: bag_id });
    assert!(server.loot_bags.contains_key(&bag_id));

    // Bag is empty (spawned with no items), closing should destroy it.
    apply(&mut server, client, LootBagCommand::Close);
    assert!(
        !server.loot_bags.contains_key(&bag_id),
        "empty bag should be GC'd when the last viewer closes it"
    );
    assert!(server.clients[&client].open_container.is_none());
}

#[test]
fn close_loot_bag_keeps_nonempty_entity_alive() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 10)],
    );
    apply(&mut server, client, LootBagCommand::Open { id: bag_id });
    apply(&mut server, client, LootBagCommand::Close);

    assert!(
        server.loot_bags.contains_key(&bag_id),
        "non-empty bag must persist so a follow-up looter can scoop it"
    );
    assert!(server.clients[&client].open_container.is_none());
}

#[test]
fn move_from_bag_into_empty_player_slot_transfers_full_stack() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 12)],
    );
    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    apply(
        &mut server,
        client,
        LootBagCommand::Move {
            from: LootBagSlotRef::Bag(0),
            to: LootBagSlotRef::PlayerInventory(0),
            quantity: None,
        },
    );

    let bag = &server.loot_bags[&bag_id];
    assert!(
        bag.slots[0].is_none(),
        "bag slot 0 should be empty after a full move"
    );
    let inv_slot = server.clients[&client].inventory.inventory_slots[0]
        .as_ref()
        .expect("inventory slot 0 holds the moved stack");
    assert_eq!(inv_slot.quantity, 12);
}

#[test]
fn partial_move_leaves_remainder_in_source_slot() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 12)],
    );
    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    apply(
        &mut server,
        client,
        LootBagCommand::Move {
            from: LootBagSlotRef::Bag(0),
            to: LootBagSlotRef::PlayerInventory(0),
            quantity: Some(4),
        },
    );

    let bag = &server.loot_bags[&bag_id];
    let remainder = bag.slots[0]
        .as_ref()
        .expect("bag retains its remainder after a partial move");
    assert_eq!(remainder.quantity, 8);
    let inv = server.clients[&client].inventory.inventory_slots[0]
        .as_ref()
        .expect("inventory holds the partial transfer");
    assert_eq!(inv.quantity, 4);
}

#[test]
fn move_into_matching_stack_merges_quantities() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    {
        let c = server.clients.get_mut(&client).unwrap();
        c.inventory.inventory_slots[0] = Some(ItemStack::new(WOOD_ID, 10));
    }
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 5)],
    );
    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    apply(
        &mut server,
        client,
        LootBagCommand::Move {
            from: LootBagSlotRef::Bag(0),
            to: LootBagSlotRef::PlayerInventory(0),
            quantity: None,
        },
    );

    let inv = server.clients[&client].inventory.inventory_slots[0]
        .as_ref()
        .expect("inventory holds merged stack");
    assert_eq!(inv.quantity, 15);
    assert!(server.loot_bags[&bag_id].slots[0].is_none());
}

#[test]
fn move_with_closed_bag_is_a_noop() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 3)],
    );
    // Note: no Open command, the move must be rejected on the open-
    // bag gate, not by silently mutating the bag.
    apply(
        &mut server,
        client,
        LootBagCommand::Move {
            from: LootBagSlotRef::Bag(0),
            to: LootBagSlotRef::PlayerInventory(0),
            quantity: None,
        },
    );

    let bag = &server.loot_bags[&bag_id];
    assert_eq!(
        bag.slots[0].as_ref().unwrap().quantity,
        3,
        "no open → no move"
    );
    assert!(server.clients[&client].inventory.inventory_slots[0].is_none());
}

#[test]
fn quick_transfer_from_bag_lands_in_first_empty_inventory_slot() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(COAL_ID, 7)],
    );
    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    apply(
        &mut server,
        client,
        LootBagCommand::QuickTransfer {
            from: LootBagSlotRef::Bag(0),
        },
    );

    assert!(server.loot_bags[&bag_id].slots[0].is_none());
    // Quick-transfer routes through `add_stack_to_inventory`, which
    // prefers the actionbar; either slot 0 of actionbar or inventory
    // should hold the coal.
    let client_ref = &server.clients[&client];
    let landed = client_ref
        .inventory
        .actionbar_slots
        .iter()
        .chain(client_ref.inventory.inventory_slots.iter())
        .flatten()
        .any(|s| s.item_id.as_ref() == COAL_ID && s.quantity == 7);
    assert!(
        landed,
        "quick-transfer should deposit somewhere in the player's grid"
    );
}

#[test]
fn quick_transfer_from_player_lands_in_first_empty_bag_slot() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    {
        let c = server.clients.get_mut(&client).unwrap();
        c.inventory.inventory_slots[2] = Some(ItemStack::new(WOOD_ID, 4));
    }
    let bag_id = server.spawn_loot_bag(Vec3Net::new(1.0, 0.0, 0.0), 0.0, Vec::new());
    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    apply(
        &mut server,
        client,
        LootBagCommand::QuickTransfer {
            from: LootBagSlotRef::PlayerInventory(2),
        },
    );

    assert!(server.clients[&client].inventory.inventory_slots[2].is_none());
    let landed = server.loot_bags[&bag_id]
        .slots
        .iter()
        .flatten()
        .any(|s| s.item_id.as_ref() == WOOD_ID && s.quantity == 4);
    assert!(landed, "stack should land in the first free bag slot");
}

#[test]
fn destroy_loot_bag_clears_open_pointer() {
    let mut server = server();
    let client = connect_named(&mut server, crate::protocol::AccountId(1), "Looter");
    place_player(&mut server, client, Vec3Net::new(0.0, 0.0, 0.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(1.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 1)],
    );
    apply(&mut server, client, LootBagCommand::Open { id: bag_id });

    server.destroy_loot_bag(bag_id);

    assert!(!server.loot_bags.contains_key(&bag_id));
    assert!(
        server.clients[&client].open_container.is_none(),
        "destroying a bag must clear every client's open pointer so a stale Move can't reach in"
    );
}

#[test]
fn tick_loot_bags_drops_freshly_spawned_bag_until_resting() {
    let mut server = server();
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 1)],
    );
    // Spawn places the bag at y = position.y + BAG_SPAWN_HEIGHT_M (1.0).
    let start_y = server.loot_bags[&bag_id].position.y;
    assert!(start_y > 0.5, "bag spawns above the ground");

    // Step gravity in slices small enough to obey the dt clamp until
    // the bag reports `resting`.
    for _ in 0..200 {
        server.tick_loot_bags(0.05);
        if server.loot_bags[&bag_id].resting {
            break;
        }
    }

    let bag = &server.loot_bags[&bag_id];
    assert!(bag.resting, "bag should reach the ground in finite steps");
    assert!(
        bag.position.y >= 0.0 && bag.position.y < 0.1,
        "resting bag y should snap to the floor offset, got {}",
        bag.position.y
    );
    assert_eq!(bag.velocity_y, 0.0);
}

#[test]
fn loot_bag_rests_on_a_building_floor() {
    let mut server = server();
    let foundation_top = crate::building::FOUNDATION_HEIGHT_M;
    crate::server::test_support::place_foundation(&mut server, Vec3Net::new(0.0, 0.0, -6.0));

    // Death on the foundation: the bag spawns at chest height above the
    // slab and must come to rest on its walkable top, not the ground.
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(0.0, foundation_top, -6.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 1)],
    );
    for _ in 0..200 {
        server.tick_loot_bags(0.05);
        if server.loot_bags[&bag_id].resting {
            break;
        }
    }

    let bag = &server.loot_bags[&bag_id];
    assert!(bag.resting, "bag should settle in finite steps");
    assert!(
        bag.position.y >= foundation_top && bag.position.y < foundation_top + 0.1,
        "bag must rest on the foundation top, got y {}",
        bag.position.y
    );
}

#[test]
fn destroying_the_floor_under_a_resting_bag_drops_it() {
    let mut server = server();
    let foundation_top = crate::building::FOUNDATION_HEIGHT_M;
    let foundation_id =
        crate::server::test_support::place_foundation(&mut server, Vec3Net::new(0.0, 0.0, -6.0));
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(0.0, foundation_top, -6.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 1)],
    );
    for _ in 0..200 {
        server.tick_loot_bags(0.05);
        if server.loot_bags[&bag_id].resting {
            break;
        }
    }
    assert!(server.loot_bags[&bag_id].position.y >= foundation_top);

    server.destroy_deployed_entity(foundation_id);
    for _ in 0..200 {
        server.tick_loot_bags(0.05);
        if server.loot_bags[&bag_id].resting {
            break;
        }
    }

    let bag = &server.loot_bags[&bag_id];
    assert!(bag.resting, "bag should settle again after the collapse");
    assert!(
        bag.position.y < 0.1,
        "bag must fall to the ground once its floor is destroyed, got y {}",
        bag.position.y
    );
}

#[test]
fn tick_loot_bags_skips_at_rest_bags() {
    let mut server = server();
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 1)],
    );
    // Force resting state and snap to ground.
    {
        let bag = server.loot_bags.get_mut(&bag_id).unwrap();
        bag.resting = true;
        bag.position.y = 0.05;
        bag.velocity_y = 0.0;
    }
    let before = server.loot_bags[&bag_id].position.y;

    server.tick_loot_bags(0.1);

    let after = server.loot_bags[&bag_id].position.y;
    assert_eq!(before, after, "an at-rest bag must not be integrated again");
}

#[test]
fn tick_loot_bags_with_zero_dt_is_noop() {
    let mut server = server();
    let bag_id = server.spawn_loot_bag(
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
        vec![ItemStack::new(WOOD_ID, 1)],
    );
    let before = server.loot_bags[&bag_id].position.y;

    server.tick_loot_bags(0.0);

    assert_eq!(
        server.loot_bags[&bag_id].position.y, before,
        "dt = 0 must short-circuit the gravity loop"
    );
}

#[test]
fn loot_bags_iter_yields_every_spawned_bag() {
    let mut server = server();
    let a = server.spawn_loot_bag(Vec3Net::new(0.0, 0.0, 0.0), 0.0, Vec::new());
    let b = server.spawn_loot_bag(Vec3Net::new(2.0, 0.0, 0.0), 0.0, Vec::new());
    let ids: Vec<_> = server.loot_bags_iter().map(|(id, _)| id).collect();
    assert!(ids.contains(&a));
    assert!(ids.contains(&b));
    assert_eq!(ids.len(), 2);
}

// Moved from `src/server/loot_bag/tests.rs` so every server test lives
// under `src/server/tests/`. Kept as a nested module: these exercise the
// slot-level primitives and the bag struct directly, not the command flow
// covered above.
mod slot_and_bag_tests {
    use crate::items::{COAL_ID, WOOD_ID, stack_limit};
    use crate::protocol::{ItemStack, LOOT_BAG_SLOT_COUNT, Vec3Net};
    use crate::server::{
        container_slots::{insert_into_slot, restore_slot},
        loot_bag::LootBag,
        test_support::server,
    };

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
            id: crate::protocol::LootBagId(1),
            position: Vec3Net::ZERO,
            yaw: 0.0,
            slots: vec![None; LOOT_BAG_SLOT_COUNT],
            spawn_tick: 0,
            velocity_y: 0.0,
            resting: true,
            rest_y: 0.05,
        };
        assert!(bag.is_empty());
        bag.slots[2] = Some(ItemStack::new(WOOD_ID, 1));
        assert!(!bag.is_empty());

        // to_view mirrors the slot layout.
        let view = bag.to_view();
        assert_eq!(view.id, crate::protocol::LootBagId(1));
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
}
