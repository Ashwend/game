use super::*;

#[test]
fn connect_seeds_authoritative_inventory_with_dummy_items() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present in its own snapshot");

    assert_eq!(inventory.inventory_slots.len(), 40);
    assert_eq!(inventory.actionbar_slots.len(), 9);
    assert_eq!(
        inventory.inventory_slots[0]
            .as_ref()
            .map(|stack| stack.item_id.as_ref()),
        Some(TEST_ORE_ID)
    );
    assert_eq!(
        inventory.inventory_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(12)
    );
    assert!(inventory.inventory_slots[1].is_some());
    assert_eq!(
        inventory.inventory_slots[2]
            .as_ref()
            .map(|stack| stack.item_id.as_ref()),
        Some(TEST_RELIC_ID)
    );
}

#[test]
fn inventory_move_splits_merges_and_populates_actionbar() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot::inventory(0),
            to: ItemContainerSlot::actionbar(2),
            quantity: Some(5),
        }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    assert_eq!(
        inventory.inventory_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(7)
    );
    assert_eq!(
        inventory.actionbar_slots[2]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(5)
    );

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot::actionbar(2),
            to: ItemContainerSlot::inventory(0),
            quantity: None,
        }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    assert_eq!(
        inventory.inventory_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(12)
    );
    assert!(inventory.actionbar_slots[2].is_none());
}

#[test]
fn actionbar_selection_and_drop_are_server_authoritative() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot::inventory(2),
            to: ItemContainerSlot::actionbar(3),
            quantity: None,
        }),
    );
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 3 }),
    );
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::actionbar(3),
            quantity: None,
        }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    assert_eq!(inventory.active_actionbar_slot, 3);
    assert!(inventory.actionbar_slots[3].is_none());
    assert_eq!(snapshot.dropped_items.len(), 1);
    assert_eq!(
        snapshot.dropped_items[0].stack.item_id.as_ref(),
        TEST_RELIC_ID
    );
}

#[test]
fn actionbar_q_style_drop_removes_one_item_from_stack() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot::inventory(0),
            to: ItemContainerSlot::actionbar(2),
            quantity: Some(5),
        }),
    );
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::actionbar(2),
            quantity: Some(1),
        }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    assert_eq!(
        inventory.actionbar_slots[2]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(4)
    );
    assert_eq!(snapshot.dropped_items[0].stack.quantity, 1);
}

#[test]
fn pickup_merges_actionbar_stacks_before_inventory() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected host should exist");
    client.inventory.inventory_slots[0] = None;
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(TEST_ORE_ID, 18));

    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 8),
        Vec3Net::new(0.0, SERVER_EYE_HEIGHT - 0.28, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    let dropped_item_id = server.snapshot().dropped_items[0].id;

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    assert!(snapshot.dropped_items.is_empty());
    assert_eq!(
        inventory.actionbar_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(20)
    );
    assert_eq!(
        inventory.inventory_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(6)
    );
}

#[test]
fn pickup_requires_looking_at_dropped_item_and_restores_inventory() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::inventory(2),
            quantity: None,
        }),
    );
    let dropped_item_id = server.snapshot().dropped_items[0].id;

    let mut look_away = movement(1, Vec3Net::ZERO);
    look_away.yaw = std::f32::consts::PI;
    server.receive(client_id, ClientMessage::Movement(look_away));
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );
    assert_eq!(server.snapshot().dropped_items.len(), 1);

    let look_at_drop = movement(2, Vec3Net::ZERO);
    server.receive(client_id, ClientMessage::Movement(look_at_drop));
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    assert!(snapshot.dropped_items.is_empty());
    assert_eq!(
        inventory.inventory_slots[2]
            .as_ref()
            .map(|stack| stack.item_id.as_ref()),
        Some(TEST_RELIC_ID)
    );
}

#[test]
fn pickup_emits_success_toast_to_requesting_client() {
    use crate::protocol::{ServerMessage, ToastKind};

    let mut server = server();
    let client_id = connect_host(&mut server);
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected host should exist");
    client.inventory.inventory_slots[0] = None;
    client.inventory.actionbar_slots[0] = None;

    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 5),
        Vec3Net::new(0.0, SERVER_EYE_HEIGHT - 0.28, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    let dropped_item_id = server.snapshot().dropped_items[0].id;

    let envelopes = server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );

    let toast = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) => Some((envelope.target.clone(), payload.clone())),
            _ => None,
        })
        .expect("server should emit a Toast envelope on successful pickup");

    assert_eq!(toast.0, super::DeliveryTarget::Client(client_id));
    assert_eq!(toast.1.kind, ToastKind::Success);
    assert!(
        toast.1.text.starts_with("+5 "),
        "toast text should report accepted quantity, got {}",
        toast.1.text
    );
}

#[test]
fn partial_pickup_decrements_dropped_stack_and_keeps_it_in_world() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected host should exist");
    // Pre-fill every slot with non-stackable relics so no merge target exists,
    // then leave one ore stack with exactly 5 units of headroom (stack limit
    // is 20). A pickup of 8 should accept 5 and leave 3 in the world.
    for slot in client.inventory.inventory_slots.iter_mut() {
        *slot = Some(ItemStack::new(TEST_RELIC_ID, 1));
    }
    for slot in client.inventory.actionbar_slots.iter_mut() {
        *slot = Some(ItemStack::new(TEST_RELIC_ID, 1));
    }
    client.inventory.inventory_slots[0] = Some(ItemStack::new(TEST_ORE_ID, 15));

    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 8),
        Vec3Net::new(0.0, SERVER_EYE_HEIGHT - 0.28, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    let dropped_item_id = server.snapshot().dropped_items[0].id;

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    assert_eq!(
        inventory.inventory_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(20)
    );
    assert_eq!(snapshot.dropped_items.len(), 1);
    assert_eq!(
        snapshot.dropped_items[0].stack.quantity, 3,
        "remaining dropped quantity should equal original minus accepted (8 - 5 = 3)"
    );

    // A second pickup with no further headroom must not refund the previously
    // accepted quantity into the dropped item.
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );
    let snapshot = server.snapshot_for(client_id);
    assert_eq!(snapshot.dropped_items.len(), 1);
    assert_eq!(snapshot.dropped_items[0].stack.quantity, 3);
}

#[test]
fn pickup_into_full_inventory_emits_warning_toast() {
    use crate::protocol::{ServerMessage, ToastKind};

    let mut server = server();
    let client_id = connect_host(&mut server);
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected host should exist");
    // Saturate every slot with a non-stackable item so the incoming ore can
    // never find room.
    for slot in client.inventory.inventory_slots.iter_mut() {
        *slot = Some(ItemStack::new(TEST_RELIC_ID, 1));
    }
    for slot in client.inventory.actionbar_slots.iter_mut() {
        *slot = Some(ItemStack::new(TEST_RELIC_ID, 1));
    }

    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 3),
        Vec3Net::new(0.0, SERVER_EYE_HEIGHT - 0.28, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    let dropped_item_id = server.snapshot().dropped_items[0].id;

    let envelopes = server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );

    let toast = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) => Some((envelope.target.clone(), payload.clone())),
            _ => None,
        })
        .expect("inventory-full pickup should still produce a warning toast");
    assert_eq!(toast.0, super::DeliveryTarget::Client(client_id));
    assert_eq!(toast.1.kind, ToastKind::Warning);
    assert!(
        toast.1.text.to_ascii_lowercase().contains("full"),
        "toast should mention inventory being full, got {}",
        toast.1.text
    );
}

#[test]
fn failed_pickup_emits_no_toast() {
    use crate::protocol::ServerMessage;

    let mut server = server();
    let client_id = connect_host(&mut server);

    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 3),
        Vec3Net::new(0.0, SERVER_EYE_HEIGHT - 0.28, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    let dropped_item_id = server.snapshot().dropped_items[0].id;

    // Turn the player around so the dropped item is behind them; the pickup
    // line-of-sight check rejects the request and no toast should fire.
    let mut look_away = movement(1, Vec3Net::ZERO);
    look_away.yaw = std::f32::consts::PI;
    server.receive(client_id, ClientMessage::Movement(look_away));

    let envelopes = server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );

    assert!(
        !envelopes
            .iter()
            .any(|envelope| matches!(envelope.message, ServerMessage::Toast(_))),
        "rejected pickup should not push a toast"
    );
}
