use super::*;
use crate::{
    items::{TEST_ORE_ID, TEST_RELIC_ID},
    protocol::{
        ChatMessage, ClientMessage, InventoryCommand, ItemContainerSlot, PROTOCOL_VERSION,
        PlayerMovement, Vec3Net,
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

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::PickUp { dropped_item_id }),
    );
    assert_eq!(server.snapshot().dropped_items.len(), 1);

    let mut look_down = movement(1, Vec3Net::ZERO);
    look_down.pitch = -0.7;
    server.receive(client_id, ClientMessage::Movement(look_down));
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
