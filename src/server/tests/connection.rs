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

    assert!(matches!(
        &envelopes[0],
        ServerEnvelope {
            target: DeliveryTarget::Client(target_id),
            message: ServerMessage::Kicked { reason },
        } if *target_id == client_id && reason == "Server restart"
    ));
    assert!(envelopes.iter().any(|envelope| {
        matches!(
            &envelope.message,
            ServerMessage::PlayerEvent(PlayerEvent::Left { client_id: left_id, .. })
                if *left_id == client_id
        )
    }));
    assert!(server.snapshot().players.is_empty());
}
