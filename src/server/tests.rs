use super::*;
use crate::{
    protocol::{ChatMessage, ClientMessage, PROTOCOL_VERSION, PlayerMovement, Vec3Net},
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
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

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
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

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
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

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
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

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
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

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
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

    let mut jump_movement = movement(1, Vec3Net::new(0.0, 0.2, 0.0));
    jump_movement.velocity = Vec3Net::new(0.0, 4.0, 0.0);
    jump_movement.grounded = false;
    server.receive(client_id, ClientMessage::Movement(jump_movement));

    let player = &server.snapshot().players[0];
    assert!(player.position.y > 0.0);
    assert!(!player.grounded);
}
