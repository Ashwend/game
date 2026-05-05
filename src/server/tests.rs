use super::*;
use crate::{
    controller::JUMP_STAMINA_COST,
    protocol::{ClientMessage, MAX_STAMINA, PROTOCOL_VERSION, PlayerInput, Vec3Net},
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
fn movement_is_authoritative_on_tick() {
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
        ClientMessage::Input(PlayerInput {
            sequence: 1,
            direction: Vec3Net::new(1.0, 0.0, 0.0),
            sprint: false,
            jump: false,
            yaw: 0.0,
            pitch: 0.0,
        }),
    );
    for _ in 0..10 {
        server.tick(0.05);
    }

    let snapshot = server.snapshot();
    assert!(snapshot.players[0].position.x > 1.0);
}

#[test]
fn jump_consumes_stamina_and_is_networked() {
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
        ClientMessage::Input(PlayerInput {
            sequence: 1,
            direction: Vec3Net::ZERO,
            sprint: false,
            jump: true,
            yaw: 0.0,
            pitch: 0.0,
        }),
    );
    server.tick(0.05);

    let player = &server.snapshot().players[0];
    assert!(player.position.y > 0.0);
    assert!(player.stamina < MAX_STAMINA);
    assert!(!player.grounded);
}

#[test]
fn jump_request_survives_following_non_jump_input_before_tick() {
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
        ClientMessage::Input(PlayerInput {
            sequence: 1,
            direction: Vec3Net::ZERO,
            sprint: false,
            jump: true,
            yaw: 0.0,
            pitch: 0.0,
        }),
    );
    server.receive(
        client_id,
        ClientMessage::Input(PlayerInput {
            sequence: 2,
            direction: Vec3Net::new(0.0, 0.0, 1.0),
            sprint: true,
            jump: false,
            yaw: 0.0,
            pitch: 0.0,
        }),
    );
    server.tick(0.05);

    let player = &server.snapshot().players[0];
    assert!(player.position.y > 0.0);
    assert!(!player.grounded);
}

#[test]
fn sprint_does_not_spend_stamina_before_jump_request() {
    let mut server = server();
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect");

    let client = server.clients.get_mut(&client_id).expect("client exists");
    client.controller.stamina = JUMP_STAMINA_COST + 0.1;

    server.receive(
        client_id,
        ClientMessage::Input(PlayerInput {
            sequence: 1,
            direction: Vec3Net::new(0.0, 0.0, 1.0),
            sprint: true,
            jump: true,
            yaw: 0.0,
            pitch: 0.0,
        }),
    );
    server.tick(0.05);

    let player = &server.snapshot().players[0];
    assert!(player.position.y > 0.0);
    assert!(player.stamina <= 0.1);
}
