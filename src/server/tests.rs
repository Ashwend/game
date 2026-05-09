use super::*;
use crate::{
    items::{TEST_ORE_ID, TEST_RELIC_ID},
    protocol::{
        ChatMessage, ClientMessage, InventoryCommand, ItemContainerSlot, ItemStack,
        PROTOCOL_VERSION, PlayerMovement, SERVER_TICK_RATE_HZ, Vec3Net,
    },
    save::WorldSave,
    steam::offline_auth_token,
};

fn server() -> GameServer {
    GameServer::new(
        WorldSave::new("Test", Some(1)),
        ServerSettings {
            auth_mode: AuthMode::Offline,
            singleplayer_host: Some(1),
        },
    )
}

fn movement(sequence: u64, position: Vec3Net) -> PlayerMovement {
    PlayerMovement {
        sequence,
        position,
        velocity: Vec3Net::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        grounded: true,
    }
}

fn connect_host(server: &mut GameServer) -> ClientId {
    server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect")
        .0
}

#[test]
fn singleplayer_host_is_admin() {
    let mut server = server();
    let (client_id, envelopes) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

    assert_eq!(client_id, 1);
    assert!(matches!(
        &envelopes[0].message,
        ServerMessage::Welcome { is_admin: true, .. }
    ));
}

#[test]
fn rejects_invalid_auth() {
    let mut server = server();
    assert!(
        server
            .connect(PROTOCOL_VERSION, 2, "Bad".to_owned(), "wrong".to_owned())
            .is_err()
    );
}

#[test]
fn chat_is_sanitized_and_broadcast_by_server() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let envelopes = server.receive(
        client_id,
        ClientMessage::Chat {
            text: "  hello server  ".to_owned(),
        },
    );

    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].target, DeliveryTarget::Broadcast);
    assert!(matches!(
        &envelopes[0].message,
        ServerMessage::Chat(ChatMessage { from, text })
            if from == "Host" && text == "hello server"
    ));
}

#[test]
fn empty_chat_is_ignored_by_server() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let envelopes = server.receive(
        client_id,
        ClientMessage::Chat {
            text: "   ".to_owned(),
        },
    );

    assert!(envelopes.is_empty());
}

#[test]
fn movement_state_is_accepted_by_server() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    server.receive(
        client_id,
        ClientMessage::Movement(movement(1, Vec3Net::new(1.25, 0.0, 0.0))),
    );

    let snapshot = server.snapshot();
    assert_eq!(snapshot.players[0].position, Vec3Net::new(1.25, 0.0, 0.0));
    assert_eq!(snapshot.players[0].last_processed_input, 1);
}

#[test]
fn stale_movement_sequence_is_ignored_by_server() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    server.receive(
        client_id,
        ClientMessage::Movement(movement(2, Vec3Net::new(1.0, 0.0, 0.0))),
    );
    server.receive(
        client_id,
        ClientMessage::Movement(movement(1, Vec3Net::new(-1.0, 0.0, 0.0))),
    );

    let player = &server.snapshot().players[0];
    assert!(player.position.x > 0.0);
    assert_eq!(player.last_processed_input, 2);
}

#[test]
fn non_finite_movement_is_ignored_by_server() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let mut bad_movement = movement(1, Vec3Net::new(f32::NAN, 0.0, 0.0));
    bad_movement.velocity = Vec3Net::new(1.0, 0.0, 0.0);
    server.receive(client_id, ClientMessage::Movement(bad_movement));

    let player = &server.snapshot().players[0];
    assert!(player.position.x.is_finite());
    assert_eq!(player.last_processed_input, 0);
}

#[test]
fn airborne_movement_state_is_networked() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let mut jump_movement = movement(1, Vec3Net::new(0.0, 0.2, 0.0));
    jump_movement.velocity = Vec3Net::new(0.0, 4.0, 0.0);
    jump_movement.grounded = false;
    server.receive(client_id, ClientMessage::Movement(jump_movement));

    let player = &server.snapshot().players[0];
    assert!(player.position.y > 0.0);
    assert!(!player.grounded);
}

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
            to: ItemContainerSlot::actionbar(0),
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
        inventory.actionbar_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(5)
    );

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot::actionbar(0),
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
    assert!(inventory.actionbar_slots[0].is_none());
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
            to: ItemContainerSlot::actionbar(0),
            quantity: Some(5),
        }),
    );
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Drop {
            from: ItemContainerSlot::actionbar(0),
            quantity: Some(1),
        }),
    );

    let snapshot = server.snapshot();
    assert_eq!(
        snapshot.players[0].inventory.actionbar_slots[0]
            .as_ref()
            .map(|stack| stack.quantity),
        Some(4)
    );
    assert_eq!(snapshot.dropped_items[0].stack.quantity, 1);
}

#[test]
fn dropped_items_spawn_near_head_and_inherit_player_velocity() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let mut sprinting = movement(1, Vec3Net::ZERO);
    sprinting.velocity = Vec3Net::new(0.0, 0.0, -8.0);
    server.receive(client_id, ClientMessage::Movement(sprinting));

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
                if item_id == TEST_ORE_ID && *quantity == 8
        )
    }));
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
