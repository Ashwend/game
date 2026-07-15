//! Server-authority tests for sleeping bags: rename, pick-up, the
//! death-screen bag list, and respawning at a bag.

use super::*;
use crate::{
    items::{DeployableKind, SLEEPING_BAG_ID},
    protocol::{DeployedEntityId, PlaceDeployableCommand, SleepingBagCommand, Vec3Net},
    server::PlayerLifecycle,
};

fn connect_other(server: &mut GameServer, account_id: u64, name: &str) -> ClientId {
    let client_id = server
        .connect(
            crate::protocol::PROTOCOL_VERSION,
            Some(crate::protocol::GAME_VERSION.to_owned()),
            crate::protocol::AccountId(account_id),
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

fn place_bag(server: &mut GameServer, client_id: ClientId, position: Vec3Net) -> DeployedEntityId {
    give(server, client_id, SLEEPING_BAG_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: crate::items::intern_item_id(SLEEPING_BAG_ID),
            position,
            yaw: 0.0,
            wall_mounted: false,
        }),
    );
    server
        .deployed_entities
        .values()
        .filter(|entity| matches!(entity.kind, DeployableKind::SleepingBag))
        .map(|entity| entity.id)
        .max()
        .expect("bag placed")
}

fn kill(server: &mut GameServer, client_id: ClientId) {
    let tick = server.tick;
    let client = server.clients.get_mut(&client_id).expect("client");
    client.lifecycle = PlayerLifecycle::Dead {
        since_tick: tick,
        killer: None,
    };
    client.controller.health = 0.0;
}

#[test]
fn bags_place_through_the_normal_deployable_path() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_bag(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));
    assert!(matches!(
        server.deployed_entities[&id].kind,
        DeployableKind::SleepingBag
    ));
}

#[test]
fn rename_sets_the_label_and_sanitizes_input() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_bag(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));

    server.receive(
        client_id,
        ClientMessage::SleepingBag(SleepingBagCommand::Rename {
            id,
            name: "  home base \u{7}".to_owned(),
        }),
    );
    assert_eq!(
        server.deployed_entities[&id].label.as_deref(),
        Some("home base")
    );

    // Another player can't rename someone else's bag.
    let intruder = connect_other(&mut server, 2, "Intruder");
    server.receive(
        intruder,
        ClientMessage::SleepingBag(SleepingBagCommand::Rename {
            id,
            name: "mine now".to_owned(),
        }),
    );
    assert_eq!(
        server.deployed_entities[&id].label.as_deref(),
        Some("home base")
    );
}

#[test]
fn pickup_returns_the_item_and_despawns_the_bag() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_bag(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));
    assert_eq!(
        crate::inventory::count_items_in_inventory(
            &server.clients[&client_id].inventory,
            SLEEPING_BAG_ID
        ),
        0
    );

    server.receive(
        client_id,
        ClientMessage::SleepingBag(SleepingBagCommand::PickUp { id }),
    );

    assert!(!server.deployed_entities.contains_key(&id));
    assert_eq!(
        crate::inventory::count_items_in_inventory(
            &server.clients[&client_id].inventory,
            SLEEPING_BAG_ID
        ),
        1
    );
}

#[test]
fn death_lists_owned_bags_with_their_names() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_bag(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));
    server.receive(
        client_id,
        ClientMessage::SleepingBag(SleepingBagCommand::Rename {
            id,
            name: "north camp".to_owned(),
        }),
    );

    let bags = server.respawn_bag_options(crate::protocol::AccountId(1));
    assert_eq!(bags.len(), 1);
    assert_eq!(bags[0].id, id);
    assert_eq!(bags[0].name, "north camp");

    // Other accounts see nothing.
    assert!(
        server
            .respawn_bag_options(crate::protocol::AccountId(999))
            .is_empty()
    );
}

#[test]
fn respawn_at_bag_revives_beside_it_and_rejects_strangers() {
    let mut server = server();
    let owner = connect_host(&mut server);
    // Within placement reach of the origin-pinned owner.
    let bag_position = Vec3Net::new(3.0, 0.0, 3.0);
    let id = place_bag(&mut server, owner, bag_position);

    kill(&mut server, owner);
    server.receive(owner, ClientMessage::RespawnAtBag { id });

    let client = &server.clients[&owner];
    assert!(client.lifecycle.is_alive());
    assert!(
        client
            .controller
            .position
            .within_horizontal_range(bag_position, 2.5),
        "respawn should land beside the bag, got {:?}",
        client.controller.position
    );

    // A stranger dying can't respawn at someone else's bag.
    let stranger = connect_other(&mut server, 2, "Stranger");
    kill(&mut server, stranger);
    server.receive(stranger, ClientMessage::RespawnAtBag { id });
    assert!(
        server.clients[&stranger].lifecycle.is_dead(),
        "stranger must stay dead after targeting a foreign bag"
    );
}

#[test]
fn respawn_at_bag_requires_being_dead() {
    let mut server = server();
    let owner = connect_host(&mut server);
    let id = place_bag(&mut server, owner, Vec3Net::new(4.0, 0.0, 0.0));
    let before = server.clients[&owner].controller.position;

    server.receive(owner, ClientMessage::RespawnAtBag { id });

    assert_eq!(
        server.clients[&owner].controller.position, before,
        "alive players can't teleport via the respawn path"
    );
}
