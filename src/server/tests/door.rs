//! Server-authority tests for code-locked doors: hanging in a doorway,
//! the first-open code flow, code rotation, and the doorway-destroys-door
//! cascade.

use super::*;
use crate::{
    building::BuildingPiece,
    items::{DeployableKind, DoorVariant, HEWN_LOG_DOOR_ID, WOOD_ID},
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
        .find(|entity| matches!(entity.kind, DeployableKind::Door { .. }))
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
            variant: DoorVariant::HewnLog,
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
            variant: DoorVariant::HewnLog,
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
                variant: DoorVariant::HewnLog,
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
            variant: DoorVariant::HewnLog,
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
            variant: DoorVariant::HewnLog,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let doors = server
        .deployed_entities
        .values()
        .filter(|entity| matches!(entity.kind, DeployableKind::Door { .. }))
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
            variant: DoorVariant::HewnLog,
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
            variant: DoorVariant::HewnLog,
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
            variant: DoorVariant::HewnLog,
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

#[test]
fn iron_door_hangs_at_double_the_wood_door_hp() {
    use crate::{game_balance::IRON_DOOR_MAX_HP, items::IRON_DOOR_ID};
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);
    give(&mut server, client_id, IRON_DOOR_ID, 1);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            variant: DoorVariant::Iron,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("iron door should hang");
    let door = &server.deployed_entities[&id];
    assert!(matches!(
        door.kind,
        DeployableKind::Door {
            variant: DoorVariant::Iron
        }
    ));
    assert_eq!(door.health, IRON_DOOR_MAX_HP);
    assert_eq!(door.max_health, IRON_DOOR_MAX_HP);
}

#[test]
fn tools_do_nothing_to_an_iron_door_but_chip_a_wood_one() {
    use crate::{
        items::{IRON_DOOR_ID, IRON_HATCHET_ID},
        protocol::DamageDeployableCommand,
    };
    // Hang `variant`, swing an iron hatchet at it once, return HP lost.
    let hp_lost_to_a_hatchet = |variant: DoorVariant, door_item: &str| -> u32 {
        let mut server = server();
        let client_id = connect_host(&mut server);
        let doorway = build_doorway(&mut server, client_id);
        give(&mut server, client_id, door_item, 1);
        server.receive(
            client_id,
            ClientMessage::Door(DoorCommand::Place {
                doorway_id: doorway,
                variant,
                flip: false,
                code: "1234".to_owned(),
            }),
        );
        let id = door_id(&server).expect("door hung");
        // Put an iron hatchet in the active actionbar slot so the damage
        // handler's tool lookup finds it.
        server
            .clients
            .get_mut(&client_id)
            .unwrap()
            .inventory
            .actionbar_slots[0] = Some(ItemStack::new(IRON_HATCHET_ID, 1));
        let before = server.deployed_entities[&id].health;
        server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
        let after = server
            .deployed_entities
            .get(&id)
            .map(|entity| entity.health)
            .unwrap_or(0);
        before - after
    };

    // The iron door is metal: a hatchet does literally nothing to it.
    assert_eq!(
        hp_lost_to_a_hatchet(DoorVariant::Iron, IRON_DOOR_ID),
        0,
        "tools must not damage the iron door"
    );
    // Regression: the wood door is still the soft spot, a hatchet chips it.
    assert!(
        hp_lost_to_a_hatchet(DoorVariant::HewnLog, HEWN_LOG_DOOR_ID) > 0,
        "the wood door should still take tool damage"
    );
}

#[test]
fn iron_door_variant_survives_the_save() {
    use crate::items::IRON_DOOR_ID;
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);
    give(&mut server, client_id, IRON_DOOR_ID, 1);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            variant: DoorVariant::Iron,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("iron door");
    let save = server.world_save();
    let restored = GameServer::restore_deployed_entities(save.state.deployed_entities);
    assert!(
        matches!(
            restored[&id].kind,
            DeployableKind::Door {
                variant: DoorVariant::Iron
            }
        ),
        "the iron variant must round-trip through the save"
    );
}

/// Units of `item_id` in `client_id`'s inventory; the pickup tests check
/// the door item comes back.
fn count(server: &GameServer, client_id: ClientId, item_id: &str) -> u32 {
    crate::inventory::count_items_in_inventory(&server.clients[&client_id].inventory, item_id)
}

#[test]
fn an_unlocked_door_can_be_picked_up_and_returns_the_item() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            variant: DoorVariant::HewnLog,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("door hangs");
    // Hanging consumed the door item.
    assert_eq!(count(&server, client_id, HEWN_LOG_DOOR_ID), 0);
    // Unlock once (knowing the code is required to pick up).
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::EnterCode {
            id,
            code: "1234".to_owned(),
        }),
    );
    server.receive(client_id, ClientMessage::Door(DoorCommand::PickUp { id }));
    assert!(door_id(&server).is_none(), "the panel leaves the world");
    assert_eq!(
        count(&server, client_id, HEWN_LOG_DOOR_ID),
        1,
        "the door item returns to inventory"
    );
}

#[test]
fn picking_up_a_door_without_the_code_is_rejected() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let doorway = build_doorway(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            variant: DoorVariant::HewnLog,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("door hangs");
    // No EnterCode: the placer hasn't proven the code, so pickup is denied.
    server.receive(client_id, ClientMessage::Door(DoorCommand::PickUp { id }));
    assert!(door_id(&server).is_some(), "the door stays without the code");
    assert_eq!(count(&server, client_id, HEWN_LOG_DOOR_ID), 0);
}

#[test]
fn anyone_who_knows_the_code_can_pick_up_an_unclaimed_door() {
    // No Tool Cupboard claims the base, so the door's only protection is
    // its code: a second account that learns it may take the door.
    let mut server = server();
    let owner = connect_host(&mut server);
    let doorway = build_doorway(&mut server, owner);
    server.receive(
        owner,
        ClientMessage::Door(DoorCommand::Place {
            doorway_id: doorway,
            variant: DoorVariant::HewnLog,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    let id = door_id(&server).expect("door hangs");

    let other = connect_other(&mut server, 2, "Other");
    server
        .clients
        .get_mut(&other)
        .unwrap()
        .controller
        .position = server.deployed_entities[&id].position;
    server.receive(
        other,
        ClientMessage::Door(DoorCommand::EnterCode {
            id,
            code: "1234".to_owned(),
        }),
    );
    server.receive(other, ClientMessage::Door(DoorCommand::PickUp { id }));
    assert!(door_id(&server).is_none(), "the unclaimed door is taken");
    assert_eq!(
        count(&server, other, HEWN_LOG_DOOR_ID),
        1,
        "the picker, not the owner, gets the door"
    );
}
