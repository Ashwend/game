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
fn version_mismatch_is_a_typed_rejection_carrying_both_sides() {
    let mut server = server();
    // A bumped protocol is a version mismatch just like a different build.
    let error = server
        .connect(
            PROTOCOL_VERSION + 1,
            Some("0.0.1".to_owned()),
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect_err("a mismatched protocol must be rejected");

    let mismatch = error
        .downcast_ref::<crate::server::VersionMismatchRejection>()
        .expect("rejection must be typed so routing answers with ServerMessage::VersionMismatch");
    assert_eq!(mismatch.server_version, GAME_VERSION);
    assert_eq!(mismatch.server_protocol, PROTOCOL_VERSION);
    assert_eq!(mismatch.client_version.as_deref(), Some("0.0.1"));
    assert_eq!(mismatch.client_protocol, PROTOCOL_VERSION + 1);
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
fn chat_populates_speaker_bubble_for_broadcast_window() {
    use crate::protocol::CHAT_BUBBLE_DURATION_SECONDS;

    let mut server = server();
    let client_id = connect_host(&mut server);

    let _ = server.receive(
        client_id,
        ClientMessage::Chat {
            text: "hi there".to_owned(),
        },
    );

    let speaker = server
        .players_iter()
        .find(|player| player.client_id == client_id)
        .expect("speaker should be in players_iter");
    assert_eq!(speaker.public.chat_bubble.as_deref(), Some("hi there"));

    let dt = 1.0 / SERVER_TICK_RATE_HZ;
    let ticks_to_expire = (CHAT_BUBBLE_DURATION_SECONDS * SERVER_TICK_RATE_HZ) as u64 + 1;
    for _ in 0..ticks_to_expire {
        server.tick(dt);
    }

    let speaker = server
        .players_iter()
        .find(|player| player.client_id == client_id)
        .expect("speaker should still be present");
    assert!(
        speaker.public.chat_bubble.is_none(),
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
    assert!(server.players_iter().next().is_none());
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

    assert_eq!(server.players_iter().count(), 1);
    server.tick(1.0 / SERVER_TICK_RATE_HZ);
    assert!(server.players_iter().next().is_none());
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
    assert!(server.players_iter().next().is_none());
}

#[test]
fn reconnect_with_same_identity_takes_over_and_preserves_state() {
    let mut server = server();
    let first = connect_host(&mut server);
    equip_basic_tools(&mut server, first);

    // Move an item so we can prove the live inventory carries across the
    // takeover instead of resetting to starting state.
    server.receive(
        first,
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
            seq: 0,
        }),
    );

    // Reconnect with the same identity before the old session has timed out.
    // This is the quick-reconnect path that used to be rejected for ~10s.
    let (second, envelopes) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("reconnect with the same identity should take over, not be rejected");

    assert_ne!(first, second, "takeover should issue a fresh client id");

    // The old session is torn down — a Left for the old id and a transport
    // Disconnect targeting it — and that teardown precedes the new Welcome so
    // peers and the host layer see the old session leave first.
    assert!(envelopes.iter().any(|envelope| matches!(
        &envelope.message,
        ServerMessage::PlayerEvent(PlayerEvent::Left { client_id, .. }) if *client_id == first
    )));
    let disconnect_index = envelopes
        .iter()
        .position(|envelope| {
            matches!(
                envelope.target,
                DeliveryTarget::Disconnect(id) if id == first
            )
        })
        .expect("old session must emit a transport-level Disconnect");
    let welcome_index = envelopes
        .iter()
        .position(|envelope| matches!(&envelope.message, ServerMessage::Welcome { .. }))
        .expect("new session must receive a Welcome");
    assert!(
        disconnect_index < welcome_index,
        "old session teardown must precede the new Welcome"
    );

    // Exactly one live player, and it inherited the moved inventory.
    assert_eq!(server.players_iter().count(), 1);
    let client = server.clients.get(&second).expect("new session exists");
    assert!(client.inventory.actionbar_slots[0].is_none());
    assert!(client.inventory.actionbar_slots[4].is_some());
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
            seq: 0,
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

    let player = restored
        .players_iter()
        .find(|player| player.client_id == restored_client_id)
        .expect("restored client should appear in the live state");
    assert!((player.public.position.x - 12.0).abs() < f32::EPSILON);
    assert!((player.public.position.y - 4.5).abs() < f32::EPSILON);
    assert!((player.public.position.z + 7.0).abs() < f32::EPSILON);
    assert!((player.public.yaw - 0.75).abs() < f32::EPSILON);

    let client = restored
        .clients
        .get(&restored_client_id)
        .expect("restored client exists");
    let inventory = &client.inventory;
    assert!(inventory.actionbar_slots[0].is_none());
    assert!(inventory.actionbar_slots[4].is_some());

    drop(restored_envelopes);
}
