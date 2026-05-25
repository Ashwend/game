use super::*;

#[test]
fn singleplayer_host_is_admin() {
    let mut server = server();
    let (client_id, envelopes) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
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
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                2,
                "Bad".to_owned(),
                "wrong".to_owned()
            )
            .is_err()
    );
}

#[test]
fn rejects_mismatched_client_versions() {
    let mut server = server();
    let error = server
        .connect(
            PROTOCOL_VERSION,
            Some("0.1.0".to_owned()),
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect_err("version mismatch should reject auth");

    assert!(error.to_string().contains("version mismatch"));
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
fn chat_populates_speaker_bubble_for_snapshot_window() {
    use crate::protocol::CHAT_BUBBLE_DURATION_SECONDS;

    let mut server = server();
    let client_id = connect_host(&mut server);

    let _ = server.receive(
        client_id,
        ClientMessage::Chat {
            text: "hi there".to_owned(),
        },
    );

    let snapshot = server.snapshot_for(client_id);
    let speaker = snapshot
        .players
        .iter()
        .find(|player| player.client_id == client_id)
        .expect("speaker should be in snapshot");
    assert_eq!(speaker.chat_bubble.as_deref(), Some("hi there"));

    let dt = 1.0 / SERVER_TICK_RATE_HZ;
    let ticks_to_expire = (CHAT_BUBBLE_DURATION_SECONDS * SERVER_TICK_RATE_HZ) as u64 + 1;
    for _ in 0..ticks_to_expire {
        server.tick(dt);
    }

    let snapshot = server.snapshot_for(client_id);
    let speaker = snapshot
        .players
        .iter()
        .find(|player| player.client_id == client_id)
        .expect("speaker should still be in snapshot");
    assert!(
        speaker.chat_bubble.is_none(),
        "bubble should auto-clear after the broadcast window"
    );
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
fn server_announcements_are_broadcast_as_chat() {
    let server = server();

    let envelopes = server.announce("  restart soon  ");

    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].target, DeliveryTarget::Broadcast);
    assert!(matches!(
        &envelopes[0].message,
        ServerMessage::Chat(ChatMessage { from, text })
            if from == "Server" && text == "restart soon"
    ));
}

#[test]
fn silent_clients_are_disconnected_after_timeout() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let mut envelopes = Vec::new();

    for _ in 0..=CLIENT_STALE_TIMEOUT_TICKS {
        envelopes = server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    assert!(matches!(
        envelopes.iter().find_map(|envelope| match &envelope.message {
            ServerMessage::PlayerEvent(event) => Some(event),
            _ => None,
        }),
        Some(PlayerEvent::Left { client_id: left_id, name })
            if *left_id == client_id && name == "Host"
    ));
    assert!(
        envelopes.iter().any(|envelope| matches!(
            envelope.target,
            DeliveryTarget::Disconnect(target_id) if target_id == client_id
        )),
        "stale-client eviction must emit a transport-level Disconnect so Lightyear tears the session down"
    );
    assert!(server.snapshot().players.is_empty());
    assert!(
        server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Host".to_owned(),
                offline_auth_token(1),
            )
            .is_ok()
    );
}

#[test]
fn heartbeat_keeps_client_connected_until_it_stops() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    for _ in 0..CLIENT_STALE_TIMEOUT_TICKS {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }
    server.receive(client_id, ClientMessage::Heartbeat);
    for _ in 0..CLIENT_STALE_TIMEOUT_TICKS {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    assert_eq!(server.snapshot().players.len(), 1);
    server.tick(1.0 / SERVER_TICK_RATE_HZ);
    assert!(server.snapshot().players.is_empty());
}

#[test]
fn kick_all_sends_reason_before_disconnects() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let envelopes = server.kick_all("Server restart");

    let kicked_index = envelopes
        .iter()
        .position(|envelope| {
            matches!(
                envelope,
                ServerEnvelope {
                    target: DeliveryTarget::Client(target_id),
                    message: ServerMessage::Kicked { reason },
                } if *target_id == client_id && reason == "Server restart"
            )
        })
        .expect("Kicked envelope should be emitted for the kicked client");
    let disconnect_index = envelopes
        .iter()
        .position(|envelope| {
            matches!(
                envelope.target,
                DeliveryTarget::Disconnect(target_id) if target_id == client_id
            )
        })
        .expect("DeliveryTarget::Disconnect envelope should be emitted so the host layer can tear down the transport session");
    assert!(
        kicked_index < disconnect_index,
        "Kicked envelope must precede the transport-level Disconnect so the client sees the reason before the connection drops"
    );
    assert!(envelopes.iter().any(|envelope| {
        matches!(
            &envelope.message,
            ServerMessage::PlayerEvent(PlayerEvent::Left { client_id: left_id, .. })
                if *left_id == client_id
        )
    }));
    assert!(server.snapshot().players.is_empty());
}

#[test]
fn world_save_round_trips_player_inventory_and_position() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);

    let pose = PlayerMovement {
        sequence: 1,
        position: Vec3Net::new(12.0, 4.5, -7.0),
        velocity: Vec3Net::ZERO,
        yaw: 0.75,
        pitch: -0.25,
        grounded: true,
    };
    server.receive(client_id, ClientMessage::Movement(pose));

    // Move an item onto the actionbar so we can verify inventory state
    // survives a save/load cycle.
    let envelopes = server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot {
                container: crate::protocol::ItemContainer::Actionbar,
                slot: 0,
            },
            to: ItemContainerSlot {
                container: crate::protocol::ItemContainer::Actionbar,
                slot: 4,
            },
            quantity: None,
        }),
    );
    drop(envelopes);

    let save = server.world_save();
    assert_eq!(save.state.players.len(), 1);

    let mut restored = GameServer::new(
        save,
        ServerSettings {
            auth_mode: AuthMode::Offline,
            singleplayer_host: Some(1),
        },
    );
    let (restored_client_id, restored_envelopes) = restored
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("returning host should reconnect");

    let snapshot = restored.snapshot_for(restored_client_id);
    let player = snapshot
        .players
        .iter()
        .find(|player| player.client_id == restored_client_id)
        .expect("restored client should appear in snapshot");
    assert!((player.position.x - 12.0).abs() < f32::EPSILON);
    assert!((player.position.y - 4.5).abs() < f32::EPSILON);
    assert!((player.position.z + 7.0).abs() < f32::EPSILON);
    assert!((player.yaw - 0.75).abs() < f32::EPSILON);

    let inventory = player
        .inventory
        .as_ref()
        .expect("snapshot should carry inventory for the receiving client");
    assert!(inventory.actionbar_slots[0].is_none());
    assert!(inventory.actionbar_slots[4].is_some());

    drop(restored_envelopes);
}
