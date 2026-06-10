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
fn mirror_sync_deltas_track_spawn_mutate_and_remove() {
    let mut server = server();
    let (dirty, removed) = server.drain_dropped_item_sync();
    assert!(
        dirty.is_empty() && removed.is_empty(),
        "a fresh world has no dropped-item deltas"
    );

    // Spawn is recorded as dirty (→ sync spawns a mirror entity).
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 4),
        Vec3Net::new(0.0, DROPPED_ITEM_RADIUS, -2.0),
        Vec3Net::ZERO,
        0.0,
    );
    let id = server.dropped_items_iter().next().expect("item spawned").0;
    let (dirty, removed) = server.drain_dropped_item_sync();
    assert_eq!(dirty, vec![id]);
    assert!(removed.is_empty());

    // Mutating via the guarded accessor re-flags it dirty (→ stack diff).
    server
        .dropped_item_body_mut(id)
        .expect("item present")
        .item
        .stack
        .quantity = 2;
    let (dirty, removed) = server.drain_dropped_item_sync();
    assert_eq!(dirty, vec![id]);
    assert!(removed.is_empty());

    // Removal is recorded as removed, not dirty (→ sync despawns it).
    assert!(server.remove_dropped_item(id).is_some());
    let (dirty, removed) = server.drain_dropped_item_sync();
    assert!(dirty.is_empty(), "removed item must not stay dirty");
    assert_eq!(removed, vec![id]);

    // A mutate-attempt on an absent item records nothing.
    assert!(server.dropped_item_body_mut(id).is_none());
    let (dirty, removed) = server.drain_dropped_item_sync();
    assert!(dirty.is_empty() && removed.is_empty());
}

#[test]
fn settling_item_marks_dirty_and_at_rest_item_does_not() {
    let mut server = server();
    server.spawn_dropped_item(
        ItemStack::new(COAL_ID, 1),
        Vec3Net::new(0.0, 3.0, -6.0),
        Vec3Net::ZERO,
        0.0,
    );
    let id = server.dropped_items_iter().next().expect("item spawned").0;
    let _ = server.drain_dropped_item_sync();

    // While the body is falling, the physics step re-flags it every tick.
    server.tick(1.0 / SERVER_TICK_RATE_HZ);
    let (dirty, removed) = server.drain_dropped_item_sync();
    assert_eq!(dirty, vec![id], "a moving item must re-enter the dirty set");
    assert!(removed.is_empty());

    // Let it settle fully (same budget the settle test above uses, plus
    // headroom for the rigid body to fall asleep), then drain whatever the
    // settling produced.
    for _ in 0..200 {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }
    let _ = server.drain_dropped_item_sync();

    // At rest the physics step writes identical poses, so the item must
    // produce no dirty entries at all.
    for _ in 0..20 {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }
    let (dirty, removed) = server.drain_dropped_item_sync();
    assert!(
        dirty.is_empty(),
        "an at-rest item must not be marked dirty, got {dirty:?}"
    );
    assert!(removed.is_empty());
}

#[test]
fn merge_marks_the_target_dirty_and_the_drained_source_removed() {
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
    let spawned: Vec<_> = server.dropped_items_iter().map(|(id, _)| id).collect();
    let _ = server.drain_dropped_item_sync();

    for _ in 0..DROPPED_ITEM_MERGE_INTERVAL_TICKS {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    let survivor = {
        let dropped: Vec<_> = server.dropped_items_iter().collect();
        assert_eq!(dropped.len(), 1, "pair should have merged");
        assert_eq!(dropped[0].1.stack.quantity, 20);
        dropped[0].0
    };
    let drained_source = spawned
        .into_iter()
        .find(|id| *id != survivor)
        .expect("two items were spawned");

    let (dirty, removed) = server.drain_dropped_item_sync();
    assert!(
        dirty.contains(&survivor),
        "merge target must be flagged dirty so the stack diff ships"
    );
    assert!(
        !dirty.contains(&drained_source),
        "fully drained source must not stay dirty"
    );
    assert_eq!(removed, vec![drained_source]);
}

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
