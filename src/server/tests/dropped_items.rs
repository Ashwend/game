use super::*;

#[test]
fn dropped_items_spawn_near_head_and_inherit_player_velocity() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let mut running = movement(1, Vec3Net::ZERO);
    running.velocity = Vec3Net::new(0.0, 0.0, -8.0);
    server.receive(client_id, ClientMessage::Movement(running));

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::inventory(2),
            quantity: None,
        }),
    );
    let initial_item = server.snapshot().dropped_items[0].clone();

    assert!(initial_item.position.y > SERVER_EYE_HEIGHT);
    assert!(initial_item.position.z > -0.7);
    assert!(initial_item.position.z < -0.25);

    server.tick(1.0 / SERVER_TICK_RATE_HZ);
    let moving_item = server.snapshot().dropped_items[0].clone();
    assert!(moving_item.position.z < initial_item.position.z - 0.3);
}

#[test]
fn nearby_dropped_items_merge_on_server_interval() {
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 12),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 8),
        Vec3Net::new(DROPPED_ITEM_MERGE_RADIUS * 0.85, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );

    let mut envelopes = Vec::new();
    for _ in 0..DROPPED_ITEM_MERGE_INTERVAL_TICKS {
        envelopes.extend(server.tick(1.0 / SERVER_TICK_RATE_HZ));
    }

    let snapshot = server.snapshot();
    assert_eq!(snapshot.dropped_items.len(), 1);
    assert_eq!(snapshot.dropped_items[0].stack.quantity, 20);
    assert!(envelopes.iter().any(|envelope| {
        matches!(
            &envelope.message,
            ServerMessage::ItemMerged { item_id, quantity }
                if item_id.as_ref() == TEST_ORE_ID && *quantity == 8
        )
    }));
}

#[test]
fn full_stack_does_not_oscillate_with_partial_neighbour() {
    // TEST_ORE_ID has a stack limit of 20. Drop a full 20 next to a partial
    // 8 within merge range and tick well past the merge interval. The pair
    // should stay as 20 + 8 forever (no partial merge → no flip). Before
    // the partial-merge guard this oscillated 20+8 ↔ 8+20 every merge tick.
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 20),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 8),
        Vec3Net::new(DROPPED_ITEM_MERGE_RADIUS * 0.85, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );

    let mut envelopes = Vec::new();
    for _ in 0..DROPPED_ITEM_MERGE_INTERVAL_TICKS * 4 {
        envelopes.extend(server.tick(1.0 / SERVER_TICK_RATE_HZ));
    }

    let snapshot = server.snapshot();
    assert_eq!(snapshot.dropped_items.len(), 2);
    let mut quantities = snapshot
        .dropped_items
        .iter()
        .map(|item| item.stack.quantity)
        .collect::<Vec<_>>();
    quantities.sort_unstable();
    assert_eq!(quantities, vec![8, 20]);
    assert!(
        !envelopes
            .iter()
            .any(|envelope| matches!(envelope.message, ServerMessage::ItemMerged { .. })),
        "no ItemMerged envelope should be emitted when the source can't be fully absorbed"
    );
}

#[test]
fn dropped_items_outside_merge_radius_stay_separate() {
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 12),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 8),
        Vec3Net::new(DROPPED_ITEM_MERGE_RADIUS + 0.25, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );

    for _ in 0..DROPPED_ITEM_MERGE_INTERVAL_TICKS {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    assert_eq!(server.snapshot().dropped_items.len(), 2);
}

#[test]
fn dropped_items_use_rapier_gravity_and_floor_collision() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    server.receive(
        client_id,
        ClientMessage::Movement(movement(1, Vec3Net::new(0.0, 4.0, 0.0))),
    );

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::inventory(2),
            quantity: None,
        }),
    );
    let initial_item = server.snapshot().dropped_items[0].clone();

    for _ in 0..80 {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    let settled_item = server.snapshot().dropped_items[0].clone();
    assert!(settled_item.position.y < initial_item.position.y - 2.0);
    assert!(settled_item.position.y >= DROPPED_ITEM_RADIUS - 0.03);
    assert!(settled_item.position.y <= DROPPED_ITEM_RADIUS + 0.12);
    assert_ne!(settled_item.rotation, initial_item.rotation);
}

#[test]
fn dropped_item_physics_collides_with_world_blocks() {
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(TEST_ORE_ID, 1),
        Vec3Net::new(0.0, 3.0, -6.0),
        Vec3Net::ZERO,
        0.0,
    );

    for _ in 0..80 {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    let item = &server.snapshot().dropped_items[0];
    assert!(item.position.y >= 0.5 + DROPPED_ITEM_RADIUS - 0.03);
    assert!(item.position.y <= 0.5 + DROPPED_ITEM_RADIUS + 0.12);
}
