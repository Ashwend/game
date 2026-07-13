use super::*;
use crate::{
    inventory::count_items_in_inventory,
    items::{IRON_BAR_ID, METEORITE_INGOT_ID, SALVAGED_FITTINGS_ID, WORKBENCH_T1_ID},
    protocol::{ItemStack, ToastKind, Vec3Net},
    server::test_support::{connect_named, server},
};

/// Build a server with one connected client and a tier-1 workbench at the
/// origin. Returns the server, client id, and workbench id. The client is
/// pinned to the origin (see `connect_named`), so the bench sits in interact
/// range.
fn fixture() -> (GameServer, ClientId, DeployedEntityId) {
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");

    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id,
        item_id: crate::items::intern_item_id(WORKBENCH_T1_ID),
        kind: DeployableKind::Workbench { tier: 1 },
        position: Vec3Net::ZERO,
        yaw: 0.0,
        health: 500,
        max_health: 500,
        owner: Some(1),
        furnace: None,
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
    server.insert_deployed_entity(id, entity);
    (server, client_id, id)
}

/// Seed the full tier-2 upgrade cost into the player's inventory.
fn give_upgrade_materials(server: &mut GameServer, client_id: ClientId) {
    let client = server.clients.get_mut(&client_id).unwrap();
    client.inventory.inventory_slots[0] = Some(ItemStack::new(IRON_BAR_ID, 30));
    client.inventory.inventory_slots[1] = Some(ItemStack::new(SALVAGED_FITTINGS_ID, 6));
    client.inventory.inventory_slots[2] = Some(ItemStack::new(METEORITE_INGOT_ID, 4));
}

fn kind_of(server: &GameServer, id: DeployedEntityId) -> DeployableKind {
    server.deployed_entities[&id].kind
}

#[test]
fn open_within_range_sets_open_pointer() {
    let (mut server, client, workbench) = fixture();
    let out = server.apply_workbench_command(client, WorkbenchCommand::Open { id: workbench });
    assert!(out.is_empty(), "successful open emits no toast");
    assert_eq!(server.clients[&client].open_workbench, Some(workbench));
}

#[test]
fn open_missing_entity_warns() {
    let (mut server, client, _workbench) = fixture();
    let out = server.apply_workbench_command(client, WorkbenchCommand::Open { id: 9999 });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
    assert!(server.clients[&client].open_workbench.is_none());
}

#[test]
fn open_too_far_warns_and_leaves_closed() {
    let (mut server, client, workbench) = fixture();
    server.clients.get_mut(&client).unwrap().controller.position =
        Vec3Net::new(WORKBENCH_INTERACT_RANGE_M * 5.0, 0.0, 0.0);

    let out = server.apply_workbench_command(client, WorkbenchCommand::Open { id: workbench });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
    assert!(server.clients[&client].open_workbench.is_none());
}

#[test]
fn upgrade_unknown_entity_warns() {
    let (mut server, client, _workbench) = fixture();
    let out = server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: 9999 });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
}

#[test]
fn upgrade_entity_without_table_row_warns() {
    // A furnace has no upgrade row today; the kind-agnostic handler must
    // reject it rather than mutating a structure without a declared path.
    let (mut server, client, _workbench) = fixture();
    let furnace_id = server.next_deployed_entity_id;
    server.next_deployed_entity_id += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id: furnace_id,
        item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
        kind: DeployableKind::Furnace { tier: 1 },
        position: Vec3Net::ZERO,
        yaw: 0.0,
        health: 800,
        max_health: 800,
        owner: Some(1),
        furnace: Some(crate::server::furnace::FurnaceState::default()),
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
    server.insert_deployed_entity(furnace_id, entity);

    let out = server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: furnace_id });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
    // Kind unchanged.
    assert_eq!(
        kind_of(&server, furnace_id),
        DeployableKind::Furnace { tier: 1 }
    );
}

#[test]
fn upgrade_out_of_range_warns_and_does_not_consume() {
    let (mut server, client, workbench) = fixture();
    give_upgrade_materials(&mut server, client);
    server.clients.get_mut(&client).unwrap().controller.position =
        Vec3Net::new(WORKBENCH_INTERACT_RANGE_M * 5.0, 0.0, 0.0);

    let out = server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: workbench });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
    // Neither the tier nor the materials changed.
    assert_eq!(
        kind_of(&server, workbench),
        DeployableKind::Workbench { tier: 1 }
    );
    let inv = &server.clients[&client].inventory;
    assert_eq!(count_items_in_inventory(inv, IRON_BAR_ID), 30);
    assert_eq!(count_items_in_inventory(inv, METEORITE_INGOT_ID), 4);
}

#[test]
fn upgrade_unaffordable_warns_and_does_not_consume() {
    let (mut server, client, workbench) = fixture();
    // Only part of the cost: iron bars but no fittings or meteorite.
    server
        .clients
        .get_mut(&client)
        .unwrap()
        .inventory
        .inventory_slots[0] = Some(ItemStack::new(IRON_BAR_ID, 30));

    let out = server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: workbench });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
    // No partial drain: the affordability check runs before any take.
    assert_eq!(
        kind_of(&server, workbench),
        DeployableKind::Workbench { tier: 1 }
    );
    assert_eq!(
        count_items_in_inventory(&server.clients[&client].inventory, IRON_BAR_ID),
        30
    );
}

#[test]
fn upgrade_success_consumes_inputs_mutates_tier_and_keeps_same_id() {
    let (mut server, client, workbench) = fixture();
    give_upgrade_materials(&mut server, client);

    let out = server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: workbench });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Success)
    ));

    // Same id retained; tier bumped to 2.
    assert!(server.deployed_entities.contains_key(&workbench));
    assert_eq!(
        kind_of(&server, workbench),
        DeployableKind::Workbench { tier: 2 }
    );

    // Every cost input fully consumed.
    let inv = &server.clients[&client].inventory;
    assert_eq!(count_items_in_inventory(inv, IRON_BAR_ID), 0);
    assert_eq!(count_items_in_inventory(inv, SALVAGED_FITTINGS_ID), 0);
    assert_eq!(count_items_in_inventory(inv, METEORITE_INGOT_ID), 0);

    // The upgraded bench keeps its original item id (definition still resolves).
    assert_eq!(
        server.deployed_entities[&workbench].item_id.as_ref(),
        WORKBENCH_T1_ID
    );
}

#[test]
fn upgrade_then_no_further_upgrade_available() {
    // After the one available upgrade the bench is at its ceiling: a second
    // Upgrade finds no table row and warns without mutating.
    let (mut server, client, workbench) = fixture();
    give_upgrade_materials(&mut server, client);
    server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: workbench });
    assert_eq!(
        kind_of(&server, workbench),
        DeployableKind::Workbench { tier: 2 }
    );

    let out = server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: workbench });
    assert!(matches!(
        out.first().map(|e| &e.message),
        Some(ServerMessage::Toast(t)) if matches!(t.kind, ToastKind::Warning)
    ));
    assert_eq!(
        kind_of(&server, workbench),
        DeployableKind::Workbench { tier: 2 }
    );
}

#[test]
fn close_clears_open_pointer() {
    let (mut server, client, workbench) = fixture();
    server.apply_workbench_command(client, WorkbenchCommand::Open { id: workbench });
    assert_eq!(server.clients[&client].open_workbench, Some(workbench));

    server.apply_workbench_command(client, WorkbenchCommand::Close);
    assert!(server.clients[&client].open_workbench.is_none());
}

/// Station-gating: a `RecipeStation::Workbench { min_tier: 2 }` recipe is out
/// of reach next to a tier-1 bench and satisfied once the same bench is
/// upgraded to tier 2. Exercises the server-side gate through `station_in_range`
/// so the tier upgrade actually unlocks tier-2 crafting.
#[test]
fn tier_two_station_gate_unlocks_after_upgrade() {
    use crate::crafting::RecipeStation;

    let (mut server, client, workbench) = fixture();
    let tier2 = RecipeStation::Workbench { min_tier: 2 };

    // Near a tier-1 bench, a tier-2 station requirement is not satisfied.
    assert!(!server.station_in_range(client, tier2));

    // Upgrade the bench in place.
    give_upgrade_materials(&mut server, client);
    server.apply_workbench_command(client, WorkbenchCommand::Upgrade { id: workbench });
    assert_eq!(
        kind_of(&server, workbench),
        DeployableKind::Workbench { tier: 2 }
    );

    // Now the same bench satisfies the tier-2 requirement.
    assert!(server.station_in_range(client, tier2));
    // And it still satisfies a tier-1 requirement (higher tier covers lower).
    assert!(server.station_in_range(client, RecipeStation::Workbench { min_tier: 1 }));
}
