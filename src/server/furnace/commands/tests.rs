use super::*;
use crate::server::furnace::FurnaceState;
use crate::{
    items::{COAL_ID, IRON_ORE_ID, WOOD_ID},
    protocol::Vec3Net,
    server::test_support::{connect_named, server},
};

/// Build a server with one connected admin client and a furnace
/// placed at the origin. Returns the server, client id, and furnace id.
fn fixture() -> (GameServer, ClientId, DeployedEntityId) {
    // `connect_named` pins the client to the origin so the furnace placed at
    // origin below sits in interact range despite the random initial spawn.
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");

    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id,
        item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
        kind: crate::items::DeployableKind::Furnace { tier: 1 },
        position: Vec3Net::ZERO,
        yaw: 0.0,
        health: 800,
        max_health: 800,
        owner: Some(1),
        furnace: Some(FurnaceState::default()),
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: None,
    };
    server.deployed_entities.insert(id, entity);
    (server, client_id, id)
}

fn furnace_of(server: &GameServer, id: DeployedEntityId) -> &FurnaceState {
    server.deployed_entities[&id].furnace.as_ref().unwrap()
}

#[test]
fn open_within_range_sets_open_pointer() {
    let (mut server, client, furnace) = fixture();
    let out = server.apply_furnace_command(client, FurnaceCommand::Open { id: furnace });
    assert!(out.is_empty(), "successful open emits no toast");
    assert_eq!(server.clients[&client].open_furnace, Some(furnace));
}

#[test]
fn open_too_far_warns_and_leaves_closed() {
    let (mut server, client, furnace) = fixture();
    // Walk the player well past the interact range.
    server.clients.get_mut(&client).unwrap().controller.position =
        Vec3Net::new(FURNACE_INTERACT_RANGE_M * 5.0, 0.0, 0.0);

    let out = server.apply_furnace_command(client, FurnaceCommand::Open { id: furnace });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
    assert!(server.clients[&client].open_furnace.is_none());
}

#[test]
fn open_missing_furnace_warns() {
    let (mut server, client, _furnace) = fixture();
    let out = server.apply_furnace_command(client, FurnaceCommand::Open { id: 9999 });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
}

#[test]
fn quick_transfer_fuel_from_player_lands_in_fuel_slot() {
    let (mut server, client, furnace) = fixture();
    server
        .clients
        .get_mut(&client)
        .unwrap()
        .inventory
        .inventory_slots[0] = Some(ItemStack::new(WOOD_ID, 10));
    server.clients.get_mut(&client).unwrap().open_furnace = Some(furnace);

    server.apply_furnace_command(
        client,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::PlayerInventory(0),
        },
    );

    let fuel = furnace_of(&server, furnace)
        .fuel
        .as_ref()
        .expect("fuel slot filled");
    assert_eq!(fuel.item_id.as_ref(), WOOD_ID);
    assert_eq!(fuel.quantity, 10);
    // Source slot is now empty.
    assert!(server.clients[&client].inventory.inventory_slots[0].is_none());
}

#[test]
fn quick_transfer_ore_from_player_lands_in_item_grid() {
    let (mut server, client, furnace) = fixture();
    server
        .clients
        .get_mut(&client)
        .unwrap()
        .inventory
        .inventory_slots[1] = Some(ItemStack::new(IRON_ORE_ID, 4));
    server.clients.get_mut(&client).unwrap().open_furnace = Some(furnace);

    server.apply_furnace_command(
        client,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::PlayerInventory(1),
        },
    );

    let in_grid = furnace_of(&server, furnace)
        .items
        .iter()
        .flatten()
        .any(|s| s.item_id.as_ref() == IRON_ORE_ID && s.quantity == 4);
    assert!(
        in_grid,
        "non-fuel quick-transfer should land in the item grid"
    );
    // Fuel slot must remain empty, ore is not fuel.
    assert!(furnace_of(&server, furnace).fuel.is_none());
}

#[test]
fn quick_transfer_from_furnace_returns_stack_to_player() {
    let (mut server, client, furnace) = fixture();
    server
        .deployed_entities
        .get_mut(&furnace)
        .unwrap()
        .furnace
        .as_mut()
        .unwrap()
        .items[0] = Some(ItemStack::new(COAL_ID, 6));
    server.clients.get_mut(&client).unwrap().open_furnace = Some(furnace);

    server.apply_furnace_command(
        client,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::Item(0),
        },
    );

    assert!(furnace_of(&server, furnace).items[0].is_none());
    let landed = server.clients[&client]
        .inventory
        .actionbar_slots
        .iter()
        .chain(server.clients[&client].inventory.inventory_slots.iter())
        .flatten()
        .any(|s| s.item_id.as_ref() == COAL_ID && s.quantity == 6);
    assert!(landed, "the stack should flow back into the player's grid");
}

#[test]
fn set_active_off_resets_smelt_progress() {
    let (mut server, client, furnace) = fixture();
    {
        let f = server
            .deployed_entities
            .get_mut(&furnace)
            .unwrap()
            .furnace
            .as_mut()
            .unwrap();
        f.active = true;
        f.smelt_progress_ticks = 50;
    }
    server.clients.get_mut(&client).unwrap().open_furnace = Some(furnace);

    server.apply_furnace_command(client, FurnaceCommand::SetActive { active: false });

    let f = furnace_of(&server, furnace);
    assert!(!f.active);
    assert_eq!(
        f.smelt_progress_ticks, 0,
        "pausing must snap smelt progress to zero so it can't be banked"
    );
}

#[test]
fn command_out_of_range_closes_and_drops_the_action() {
    let (mut server, client, furnace) = fixture();
    server.clients.get_mut(&client).unwrap().open_furnace = Some(furnace);
    // Walk away beyond range, then issue a SetActive, the gate should
    // close the furnace instead of applying it.
    server.clients.get_mut(&client).unwrap().controller.position =
        Vec3Net::new(FURNACE_INTERACT_RANGE_M * 5.0, 0.0, 0.0);

    server.apply_furnace_command(client, FurnaceCommand::SetActive { active: true });

    assert!(
        server.clients[&client].open_furnace.is_none(),
        "an out-of-range command must close the open furnace"
    );
    assert!(
        !furnace_of(&server, furnace).active,
        "the active flag must not be flipped on while out of range"
    );
}

#[test]
fn close_clears_open_pointer() {
    let (mut server, client, furnace) = fixture();
    server.clients.get_mut(&client).unwrap().open_furnace = Some(furnace);
    server.apply_furnace_command(client, FurnaceCommand::Close);
    assert!(server.clients[&client].open_furnace.is_none());
}

#[test]
fn open_furnace_view_mirrors_state() {
    let (mut server, client, furnace) = fixture();
    server
        .deployed_entities
        .get_mut(&furnace)
        .unwrap()
        .furnace
        .as_mut()
        .unwrap()
        .fuel = Some(ItemStack::new(WOOD_ID, 2));
    server.clients.get_mut(&client).unwrap().open_furnace = Some(furnace);

    let view = server.open_furnace_view_for(client).expect("view present");
    assert_eq!(view.fuel.as_ref().unwrap().quantity, 2);
}
