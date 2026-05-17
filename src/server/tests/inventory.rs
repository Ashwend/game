use super::*;

#[test]
fn connect_seeds_authoritative_inventory_with_dummy_items() {
    let mut server = server();
    connect_host(&mut server);

    let snapshot = server.snapshot();
    let inventory = &snapshot.players[0].inventory;

    assert_eq!(inventory.inventory_slots.len(), 40);
    assert_eq!(inventory.actionbar_slots.len(), 9);
    assert_eq!(
        inventory.inventory_slots[0]
            .as_ref()
            .map(|stack| stack.item_id.as_str()),
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
            .map(|stack| stack.item_id.as_str()),
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

    let snapshot = server.snapshot();
    let inventory = &snapshot.players[0].inventory;
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

    let snapshot = server.snapshot();
    let inventory = &snapshot.players[0].inventory;
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

    let snapshot = server.snapshot();
    let inventory = &snapshot.players[0].inventory;
    assert_eq!(inventory.active_actionbar_slot, 3);
    assert!(inventory.actionbar_slots[3].is_none());
    assert_eq!(snapshot.dropped_items.len(), 1);
    assert_eq!(snapshot.dropped_items[0].stack.item_id, TEST_RELIC_ID);
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

    let snapshot = server.snapshot();
    assert_eq!(
        snapshot.players[0].inventory.actionbar_slots[2]
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

    let snapshot = server.snapshot();
    let inventory = &snapshot.players[0].inventory;
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

    let snapshot = server.snapshot();
    assert!(snapshot.dropped_items.is_empty());
    assert_eq!(
        snapshot.players[0].inventory.inventory_slots[2]
            .as_ref()
            .map(|stack| stack.item_id.as_str()),
        Some(TEST_RELIC_ID)
    );
}
