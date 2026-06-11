//! Server-authoritative storage box tests: placement initialises the
//! grid, opening is kind+range gated, moves ride the shared container
//! path, contents persist through the save, and destruction spills a
//! loot bag.

use super::*;
use crate::{
    items::{STORAGE_BOX_SMALL_ID, WOOD_ID, intern_item_id},
    protocol::{
        ClientMessage, ContainerViewKind, ItemStack, LootBagCommand, LootBagSlotRef,
        PlaceDeployableCommand, Vec3Net,
    },
    server::loot_bag::OpenContainer,
};

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

fn place_box(server: &mut GameServer, client_id: ClientId, position: Vec3Net) -> DeployedEntityId {
    give(server, client_id, STORAGE_BOX_SMALL_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id(STORAGE_BOX_SMALL_ID),
            position,
            yaw: 0.0,
        }),
    );
    server
        .deployed_entities
        .values()
        .find(|entity| matches!(entity.kind, crate::items::DeployableKind::StorageBox { .. }))
        .map(|entity| entity.id)
        .expect("storage box placed")
}

#[test]
fn placed_box_opens_in_range_and_takes_items() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_box(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));

    let entity = &server.deployed_entities[&id];
    let storage = entity.storage.as_ref().expect("box has a slot grid");
    assert_eq!(
        storage.slots.len(),
        crate::game_balance::STORAGE_BOX_SMALL_SLOT_COUNT
    );

    server.receive(client_id, ClientMessage::OpenStorageBox { id });
    assert_eq!(
        server.clients[&client_id].open_container,
        Some(OpenContainer::StorageBox(id))
    );
    let view = server
        .open_loot_bag_view_for(client_id)
        .expect("open box resolves a container view");
    assert_eq!(view.kind, ContainerViewKind::StorageBox);

    // Stash wood into slot 0 through the shared container move.
    give(&mut server, client_id, WOOD_ID, 40);
    let wood_slot = server.clients[&client_id]
        .inventory
        .inventory_slots
        .iter()
        .position(|slot| {
            slot.as_ref()
                .is_some_and(|stack| stack.item_id.as_ref() == WOOD_ID)
        })
        .expect("wood in inventory");
    server.receive(
        client_id,
        ClientMessage::LootBag(LootBagCommand::Move {
            from: LootBagSlotRef::PlayerInventory(wood_slot),
            to: LootBagSlotRef::Bag(0),
            quantity: None,
        }),
    );
    let stored = server.deployed_entities[&id]
        .storage
        .as_ref()
        .unwrap()
        .slots[0]
        .as_ref()
        .expect("wood landed in the box");
    assert_eq!(stored.item_id.as_ref(), WOOD_ID);
    assert_eq!(stored.quantity, 40);
}

#[test]
fn opening_a_box_out_of_range_is_rejected() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_box(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .controller
        .position = Vec3Net::new(50.0, 0.0, 0.0);
    server.receive(client_id, ClientMessage::OpenStorageBox { id });
    assert_eq!(server.clients[&client_id].open_container, None);
}

#[test]
fn box_contents_round_trip_through_the_save() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_box(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));
    server
        .deployed_entities
        .get_mut(&id)
        .unwrap()
        .storage
        .as_mut()
        .unwrap()
        .slots[2] = Some(ItemStack::new(WOOD_ID, 17));

    let save = server.world_save();
    let restored = GameServer::restore_deployed_entities(save.state.deployed_entities);
    let storage = restored[&id].storage.as_ref().expect("storage persists");
    assert_eq!(storage.slots[2].as_ref().unwrap().quantity, 17);
    assert_eq!(
        storage.slots.len(),
        crate::game_balance::STORAGE_BOX_SMALL_SLOT_COUNT
    );
}

#[test]
fn destroying_a_box_spills_its_contents_as_a_loot_bag() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let id = place_box(&mut server, client_id, Vec3Net::new(2.0, 0.0, 0.0));
    server
        .deployed_entities
        .get_mut(&id)
        .unwrap()
        .storage
        .as_mut()
        .unwrap()
        .slots[0] = Some(ItemStack::new(WOOD_ID, 25));
    server.receive(client_id, ClientMessage::OpenStorageBox { id });

    assert!(server.loot_bags.is_empty());
    server.destroy_deployed_entity(id);
    assert!(!server.deployed_entities.contains_key(&id));
    // The open pointer is cleared so a stale Move can't write into a
    // destroyed box.
    assert_eq!(server.clients[&client_id].open_container, None);
    let bag = server.loot_bags.values().next().expect("contents spilled");
    assert_eq!(bag.slots[0].as_ref().unwrap().item_id.as_ref(), WOOD_ID);
    assert_eq!(bag.slots[0].as_ref().unwrap().quantity, 25);
}
