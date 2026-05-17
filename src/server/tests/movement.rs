use super::*;

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
fn older_client_owned_movement_does_not_overwrite_newer_pose() {
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
