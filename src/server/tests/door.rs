//! Server-authority tests for code-locked doors: hanging in a doorway,
//! the first-open code flow, code rotation, and the doorway-destroys-door
//! cascade.

use super::*;
use crate::{
    building::BuildingPiece,
    items::{DeployableKind, HEWN_LOG_DOOR_ID, WOOD_ID},
    protocol::{DeployedEntityId, DoorCommand, PlaceBuildingCommand, ServerMessage},
};

fn connect_other(server: &mut GameServer, account_id: u64, name: &str) -> ClientId {
    let client_id = server
        .connect(
            crate::protocol::PROTOCOL_VERSION,
            Some(crate::protocol::GAME_VERSION.to_owned()),
            account_id,
            name.to_owned(),
            String::new(),
        )
        .expect("connect should succeed")
        .0;
    server
        .clients
        .get_mut(&client_id)
        .expect("connected client")
        .controller
        .position = Vec3Net::ZERO;
    client_id
}

fn give(server: &mut GameServer, client_id: ClientId, item_id: &str, quantity: u16) {
    let client = server.clients.get_mut(&client_id).expect("client");
    for slot in client.inventory.inventory_slots.iter_mut() {
        if slot.is_none() {
            *slot = Some(ItemStack::new(item_id, quantity));
            return;
        }
    }
    panic!("no free inventory slot");
}

/// Foundation + doorway + a door item in the inventory; returns the
/// doorway's entity id.
fn build_doorway(server: &mut GameServer, client_id: ClientId) -> DeployedEntityId {
    give(server, client_id, WOOD_ID, 200);
    give(server, client_id, HEWN_LOG_DOOR_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceBuilding(PlaceBuildingCommand {
            piece: BuildingPiece::Foundation,
            position: Vec3Net::new(0.0, 0.0, 2.0),
            yaw: 0.0,
        }),
    );
    server.receive(
        client_id,
        ClientMessage::PlaceBuilding(PlaceBuildingCommand {
            piece: BuildingPiece::Doorway,
            position: Vec3Net::new(0.0, 0.0, 0.6),
            yaw: 0.0,
        }),
    );
    server
        .deployed_entities
        .values()
        .find(|entity| {
            matches!(
                entity.kind,
                DeployableKind::Building {
                    piece: BuildingPiece::Doorway,
                    ..
                }
            )
        })
        .map(|entity| entity.id)
        .expect("doorway placed")
}

fn door_id(server: &GameServer) -> Option<DeployedEntityId> {
    server
        .deployed_entities
        .values()
        .find(|entity| matches!(entity.kind, DeployableKind::Door))
        .map(|entity| entity.id)
}

#[test]
fn door_hangs_in_a_doorway_and_requires_the_code_on_first_open() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);

    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("door should hang");
    let door = &server.deployed_entities[&id];
    assert_eq!(door.position, server.deployed_entities[&doorway].position);

    // First interact: not authorized (even as the placer), so the
    // server prompts for the code instead of opening.
    let envelopes = server.receive(client_id, ClientMessage::Door(DoorCommand::Interact { id }));
    assert!(
        envelopes
            .iter()
            .any(|env| matches!(env.message, ServerMessage::DoorCodePrompt { id: prompt } if prompt == id)),
        "unauthorized interact must prompt for the code"
    );
    assert!(!server.deployed_entities[&id].door.as_ref().unwrap().open);

    // Wrong code: rejected, and the keypad gets a denied signal.
    let envelopes = server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::EnterCode {
            id,
            code: "9999".to_owned(),
        }),
    );
    assert!(!server.deployed_entities[&id].door.as_ref().unwrap().open);
    assert!(
        envelopes.iter().any(|env| matches!(
            env.message,
            ServerMessage::DoorCodeResult { accepted: false }
        )),
        "wrong code must ship a denied DoorCodeResult"
    );

    // Correct code: authorizes but does NOT swing the door, unlocking
    // and opening are separate actions.
    let envelopes = server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::EnterCode {
            id,
            code: "1234".to_owned(),
        }),
    );
    let state = server.deployed_entities[&id].door.as_ref().unwrap();
    assert!(!state.open, "a correct code authorizes, it does not open");
    assert!(state.authorized.contains(&1));
    assert!(
        envelopes.iter().any(|env| matches!(
            env.message,
            ServerMessage::DoorCodeResult { accepted: true }
        )),
        "correct code must ship an accepted DoorCodeResult"
    );

    // From now on E toggles: open, then closed again.
    server.receive(client_id, ClientMessage::Door(DoorCommand::Interact { id }));
    assert!(server.deployed_entities[&id].door.as_ref().unwrap().open);
    server.receive(client_id, ClientMessage::Door(DoorCommand::Interact { id }));
    assert!(!server.deployed_entities[&id].door.as_ref().unwrap().open);
}

#[test]
fn changing_the_code_revokes_other_accounts() {
    let mut server = server();
    let owner = connect_host(&mut server);
    let doorway = build_doorway(&mut server, owner);
    server.receive(
        owner,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("door");

    // Both the owner and a guest learn the code.
    let guest = connect_other(&mut server, 2, "Guest");
    for client in [owner, guest] {
        server.receive(
            client,
            ClientMessage::Door(DoorCommand::EnterCode {
                id,
                code: "1234".to_owned(),
            }),
        );
    }
    assert_eq!(
        server.deployed_entities[&id]
            .door
            .as_ref()
            .unwrap()
            .authorized
            .len(),
        2
    );

    // Owner rotates the code: only the changer stays authorized.
    server.receive(
        owner,
        ClientMessage::Door(DoorCommand::ChangeCode {
            id,
            code: "555555".to_owned(),
        }),
    );
    let state = server.deployed_entities[&id].door.as_ref().unwrap();
    assert_eq!(state.code, "555555");
    assert_eq!(state.authorized, vec![1]);

    // The guest can't rotate a code they don't know.
    server.receive(
        guest,
        ClientMessage::Door(DoorCommand::ChangeCode {
            id,
            code: "4321".to_owned(),
        }),
    );
    assert_eq!(
        server.deployed_entities[&id].door.as_ref().unwrap().code,
        "555555"
    );
}

#[test]
fn invalid_codes_and_occupied_doorways_are_rejected() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);

    // Non-digit and wrong-length codes never hang a door.
    for bad in ["12a4", "123", "1234567"] {
        server.receive(
            client_id,
            ClientMessage::Door(DoorCommand::Place {
                doorway_id: doorway,
                flip: false,
                code: bad.to_owned(),
            }),
        );
        assert!(door_id(&server).is_none(), "code {bad:?} must be rejected");
    }

    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    assert!(door_id(&server).is_some());

    // A second door in the same doorway is refused (and the item kept).
    give(&mut server, client_id, HEWN_LOG_DOOR_ID, 1);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let doors = server
        .deployed_entities
        .values()
        .filter(|entity| matches!(entity.kind, DeployableKind::Door))
        .count();
    assert_eq!(doors, 1);
    assert_eq!(
        crate::inventory::count_items_in_inventory(
            &server.clients[&client_id].inventory,
            HEWN_LOG_DOOR_ID
        ),
        1,
        "rejected placement must not eat the door item"
    );
}

#[test]
fn flipping_rotates_the_door_half_a_turn() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            flip: true,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("door");
    let door = &server.deployed_entities[&id];
    let doorway_yaw = server.deployed_entities[&doorway].yaw;
    let diff = (door.yaw - doorway_yaw).abs();
    assert!(
        (diff - std::f32::consts::PI).abs() < 1e-3,
        "flip = half-turn off the doorway yaw, got diff {diff}"
    );
}

#[test]
fn destroying_the_doorway_takes_the_door_with_it() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    assert!(door_id(&server).is_some());

    server.destroy_deployed_entity(doorway);

    assert!(door_id(&server).is_none(), "door must cascade with doorway");
}

#[test]
fn door_state_round_trips_through_the_save() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            flip: false,
            code: "8080".to_owned(),
        }),
    );
    let id = door_id(&server).expect("door");
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::EnterCode {
            id,
            code: "8080".to_owned(),
        }),
    );
    // Entering the code authorizes; opening is its own interact.
    server.receive(client_id, ClientMessage::Door(DoorCommand::Interact { id }));

    let save = server.world_save();
    let restored = GameServer::restore_deployed_entities(save.state.deployed_entities);
    let door = restored[&id].door.as_ref().expect("door state persists");
    assert_eq!(door.code, "8080");
    assert_eq!(door.authorized, vec![1]);
    assert!(door.open);
    assert_eq!(door.parent, doorway);
}
