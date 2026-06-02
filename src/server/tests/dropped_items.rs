use super::*;

fn first_dropped_item(server: &GameServer) -> crate::protocol::DroppedWorldItem {
    server
        .dropped_items_iter()
        .next()
        .expect("at least one dropped item")
        .1
}

fn dropped_count(server: &GameServer) -> usize {
    server.dropped_items_iter().count()
}

#[test]
fn dropped_items_spawn_near_head_and_inherit_player_velocity() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    {
        let client = server
            .clients
            .get_mut(&client_id)
            .expect("connected host should exist");
        client.inventory.inventory_slots[2] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    }
    let mut running = movement(1, Vec3Net::ZERO);
    running.velocity = Vec3Net::new(0.0, 0.0, -8.0);
    server.receive(client_id, ClientMessage::Movement(running));

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::inventory(2),
            quantity: None,
            seq: 0,
        }),
    );
    let initial_item = first_dropped_item(&server);

    assert!(initial_item.position.y > SERVER_EYE_HEIGHT);
    assert!(initial_item.position.z > -0.7);
    assert!(initial_item.position.z < -0.25);

    server.tick(1.0 / SERVER_TICK_RATE_HZ);
    let moving_item = first_dropped_item(&server);
    assert!(moving_item.position.z < initial_item.position.z - 0.3);
}

#[test]
fn nearby_dropped_items_merge_on_server_interval() {
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 12),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 8),
        Vec3Net::new(DROPPED_ITEM_MERGE_RADIUS * 0.85, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );

    let mut envelopes = Vec::new();
    for _ in 0..DROPPED_ITEM_MERGE_INTERVAL_TICKS {
        envelopes.extend(server.tick(1.0 / SERVER_TICK_RATE_HZ));
    }

    let dropped: Vec<_> = server.dropped_items_iter().collect();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].1.stack.quantity, 20);
    assert!(envelopes.iter().any(|envelope| {
        matches!(
            &envelope.message,
            ServerMessage::ItemMerged { item_id, quantity }
                if item_id.as_ref() == COAL_ID && *quantity == 8
        )
    }));
}

#[test]
fn full_stack_does_not_oscillate_with_partial_neighbour() {
    // COAL_ID has a stack limit of 200. Drop a full 200 next to a partial
    // 8 within merge range and tick well past the merge interval. The pair
    // should stay as 200 + 8 forever (no partial merge → no flip). Before
    // the partial-merge guard this oscillated 200+8 ↔ 8+200 every merge tick.
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 200),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 8),
        Vec3Net::new(DROPPED_ITEM_MERGE_RADIUS * 0.85, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );

    let mut envelopes = Vec::new();
    for _ in 0..DROPPED_ITEM_MERGE_INTERVAL_TICKS * 4 {
        envelopes.extend(server.tick(1.0 / SERVER_TICK_RATE_HZ));
    }

    let dropped: Vec<_> = server.dropped_items_iter().collect();
    assert_eq!(dropped.len(), 2);
    let mut quantities = dropped
        .iter()
        .map(|(_, item)| item.stack.quantity)
        .collect::<Vec<_>>();
    quantities.sort_unstable();
    assert_eq!(quantities, vec![8, 200]);
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
        ItemStack::new(COAL_ID, 12),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 8),
        Vec3Net::new(DROPPED_ITEM_MERGE_RADIUS + 0.25, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );

    for _ in 0..DROPPED_ITEM_MERGE_INTERVAL_TICKS {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    assert_eq!(dropped_count(&server), 2);
}

#[test]
fn dropped_items_use_rapier_gravity_and_floor_collision() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    {
        let client = server
            .clients
            .get_mut(&client_id)
            .expect("connected host should exist");
        client.inventory.inventory_slots[2] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    }
    server.receive(
        client_id,
        ClientMessage::Movement(movement(1, Vec3Net::new(0.0, 4.0, 0.0))),
    );

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::inventory(2),
            quantity: None,
            seq: 0,
        }),
    );
    let initial_item = first_dropped_item(&server);

    for _ in 0..80 {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    let settled_item = first_dropped_item(&server);
    assert!(settled_item.position.y < initial_item.position.y - 2.0);
    assert!(settled_item.position.y >= DROPPED_ITEM_RADIUS - 0.03);
    assert!(settled_item.position.y <= DROPPED_ITEM_RADIUS + 0.12);
    assert_ne!(settled_item.rotation, initial_item.rotation);
}

#[test]
fn dropped_items_despawn_after_their_lifetime() {
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 4),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    assert_eq!(dropped_count(&server), 1);

    // Tick just up to one cleanup boundary short of the lifetime, the item
    // should still be present.
    let stable_ticks = DROPPED_ITEM_LIFETIME_TICKS - DROPPED_ITEM_CLEANUP_INTERVAL_TICKS;
    for _ in 0..stable_ticks {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }
    assert_eq!(
        dropped_count(&server),
        1,
        "item should still be in the world just before the lifetime expires"
    );

    // Tick through the next cleanup boundary; the item should be gone.
    for _ in 0..DROPPED_ITEM_CLEANUP_INTERVAL_TICKS * 2 {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }
    assert_eq!(
        dropped_count(&server),
        0,
        "item past its lifetime should be despawned by the cleanup sweep"
    );
}

// Deleted: `dropped_items_are_filtered_by_chunk_aoi_in_per_client_snapshots`
// was verifying the snapshot_for AoI-filter behaviour; with Phase 6.6 the
// snapshot path is gone and AoI filtering happens through Lightyear's
// room/visibility machinery, which requires the plugin set to exercise.

#[test]
fn dropped_item_physics_settles_on_the_floor() {
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 1),
        Vec3Net::new(0.0, 3.0, -6.0),
        Vec3Net::ZERO,
        0.0,
    );

    for _ in 0..80 {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    // Grid-generated worlds have no internal blocks, items settle on
    // the floor at y = DROPPED_ITEM_RADIUS plus a small jitter.
    let item = first_dropped_item(&server);
    assert!(item.position.y >= DROPPED_ITEM_RADIUS - 0.03);
    assert!(item.position.y <= DROPPED_ITEM_RADIUS + 0.12);
}
