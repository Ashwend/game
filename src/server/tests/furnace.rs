//! Furnace tests. Split from `src/server/furnace/` because moving them
//! out kept the production code split clean (state vs. tick vs.
//! commands). The handful of internal helpers these tests exercise
//! (`tick_one_furnace`, `merge_into_optional_slot`, the burn-tick
//! constants) are exposed at `pub(crate)` visibility through the
//! furnace module's `mod.rs` so they remain test-reachable without
//! widening the production surface.

use crate::items::{
    COAL_ID, IRON_BAR_ID, IRON_ORE_ID, METEORITE_ALLOY_ID, METEORITE_INGOT_ID, SULFUR_ID,
    SULFUR_ORE_ID, WOOD_ID,
};
use crate::protocol::{FURNACE_ITEM_SLOT_COUNT, FurnaceCommand, FurnaceSlotRef, ItemStack};
use crate::server::furnace::{
    FurnaceState, SMELT_TICKS_PER_OUTPUT, WOOD_BURN_TICKS, merge_into_optional_slot,
    tick_one_furnace,
};

fn smeltable_input(item_id: &str, quantity: u16) -> Option<ItemStack> {
    Some(ItemStack::new(item_id, quantity))
}

#[test]
fn iron_ore_smelts_to_iron_bar_consuming_fuel() {
    let mut furnace = FurnaceState {
        fuel: smeltable_input(WOOD_ID, 5),
        items: Default::default(),
        active: true,
        fuel_burn_ticks_left: 0,
        smelt_progress_ticks: 0,
    };
    furnace.items[0] = smeltable_input(IRON_ORE_ID, 2);

    // Smelt one output's worth of ticks.
    for _ in 0..SMELT_TICKS_PER_OUTPUT {
        tick_one_furnace(&mut furnace);
    }
    // One ore consumed.
    assert_eq!(
        furnace.items[0].as_ref().map(|s| s.quantity),
        Some(1),
        "one iron ore should have been consumed",
    );
    // One bar produced (lands in a slot somewhere).
    let bar_count: u16 = furnace
        .items
        .iter()
        .filter_map(|slot| slot.as_ref())
        .filter(|stack| stack.item_id.as_ref() == IRON_BAR_ID)
        .map(|stack| stack.quantity)
        .sum();
    assert_eq!(bar_count, 1, "one iron bar should have been produced");
    assert!(furnace.active, "furnace should remain active");
}

#[test]
fn sulfur_ore_smelts_to_sulfur_consuming_fuel() {
    let mut furnace = FurnaceState {
        fuel: smeltable_input(COAL_ID, 5),
        items: Default::default(),
        active: true,
        fuel_burn_ticks_left: 0,
        smelt_progress_ticks: 0,
    };
    furnace.items[0] = smeltable_input(SULFUR_ORE_ID, 2);

    // Smelt one output's worth of ticks, same timing as iron ore.
    for _ in 0..SMELT_TICKS_PER_OUTPUT {
        tick_one_furnace(&mut furnace);
    }
    // One ore consumed.
    assert_eq!(
        furnace.items[0].as_ref().map(|s| s.quantity),
        Some(1),
        "one sulfur ore should have been consumed",
    );
    // One refined sulfur produced (lands in a slot somewhere).
    let sulfur_count: u16 = furnace
        .items
        .iter()
        .filter_map(|slot| slot.as_ref())
        .filter(|stack| stack.item_id.as_ref() == SULFUR_ID)
        .map(|stack| stack.quantity)
        .sum();
    assert_eq!(sulfur_count, 1, "one sulfur should have been produced");
    assert!(furnace.active, "furnace should remain active");
}

#[test]
fn meteorite_alloy_smelts_to_ingot_consuming_fuel() {
    let mut furnace = FurnaceState {
        fuel: smeltable_input(COAL_ID, 5),
        items: Default::default(),
        active: true,
        fuel_burn_ticks_left: 0,
        smelt_progress_ticks: 0,
    };
    furnace.items[0] = smeltable_input(METEORITE_ALLOY_ID, 2);

    // Same timing as the other smeltables: one output per window.
    for _ in 0..SMELT_TICKS_PER_OUTPUT {
        tick_one_furnace(&mut furnace);
    }
    assert_eq!(
        furnace.items[0].as_ref().map(|s| s.quantity),
        Some(1),
        "one meteorite alloy should have been consumed",
    );
    let ingot_count: u16 = furnace
        .items
        .iter()
        .filter_map(|slot| slot.as_ref())
        .filter(|stack| stack.item_id.as_ref() == METEORITE_INGOT_ID)
        .map(|stack| stack.quantity)
        .sum();
    assert_eq!(
        ingot_count, 1,
        "one meteorite ingot should have been produced"
    );
    assert!(furnace.active, "furnace should remain active");
}

#[test]
fn auto_shutoff_when_output_cannot_fit() {
    let mut furnace = FurnaceState {
        fuel: smeltable_input(COAL_ID, 5),
        items: Default::default(),
        active: true,
        fuel_burn_ticks_left: 0,
        smelt_progress_ticks: 0,
    };
    furnace.items[0] = smeltable_input(IRON_ORE_ID, 5);
    for index in 1..FURNACE_ITEM_SLOT_COUNT {
        furnace.items[index] = smeltable_input("stone", 1);
    }

    tick_one_furnace(&mut furnace);
    assert!(
        !furnace.active,
        "furnace must auto-shutoff when output won't fit"
    );
}

#[test]
fn auto_shutoff_when_no_fuel_and_smelt_pending() {
    let mut furnace = FurnaceState {
        fuel: None,
        items: Default::default(),
        active: true,
        fuel_burn_ticks_left: 0,
        smelt_progress_ticks: 0,
    };
    furnace.items[0] = smeltable_input(IRON_ORE_ID, 1);

    tick_one_furnace(&mut furnace);
    assert!(!furnace.active, "no fuel → auto-off");
}

#[test]
fn auto_shutoff_when_nothing_to_smelt() {
    let mut furnace = FurnaceState {
        fuel: smeltable_input(WOOD_ID, 5),
        items: Default::default(),
        active: true,
        fuel_burn_ticks_left: 0,
        smelt_progress_ticks: 0,
    };
    tick_one_furnace(&mut furnace);
    assert!(!furnace.active, "no input → auto-off");
}

#[test]
fn furnace_auto_shutoff_marks_the_deployable_dirty_for_the_mirror() {
    let mut server = crate::server::test_support::server();
    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id,
        item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
        kind: crate::items::DeployableKind::Furnace { tier: 1 },
        position: crate::protocol::Vec3Net::ZERO,
        yaw: 0.0,
        health: 800,
        max_health: 800,
        owner: Some(crate::protocol::AccountId(1)),
        // Active but with no fuel, so the very next tick auto-shuts it off.
        furnace: Some(FurnaceState {
            active: true,
            ..Default::default()
        }),
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
    let _ = server.drain_deployable_sync();

    // The `active` flip is replicated as `DeployableActive`, so the tick
    // must flag the deployable for the mirror sync.
    server.tick_furnaces();
    assert!(
        !server.deployed_entities[&id]
            .furnace
            .as_ref()
            .unwrap()
            .active,
        "no fuel → auto-off"
    );
    let (dirty, removed) = server.drain_deployable_sync();
    assert_eq!(dirty, vec![id]);
    assert!(removed.is_empty());

    // Once idle, further ticks produce no delta at all.
    server.tick_furnaces();
    let (dirty, removed) = server.drain_deployable_sync();
    assert!(
        dirty.is_empty() && removed.is_empty(),
        "an idle furnace must not re-enter the dirty set"
    );
}

#[test]
fn non_fuel_rejected_in_fuel_slot_via_merge_helper() {
    let mut slot: Option<ItemStack> = None;
    let leftover = merge_into_optional_slot(&mut slot, ItemStack::new(IRON_ORE_ID, 4));
    assert_eq!(slot.as_ref().map(|s| s.quantity), Some(4));
    assert!(leftover.is_none());
}

#[test]
fn removing_fuel_resets_the_burn_timer() {
    use crate::{
        auth::AuthMode,
        protocol::{GAME_VERSION, PROTOCOL_VERSION},
        save::WorldSave,
        server::ServerSettings,
    };

    let mut server = crate::server::GameServer::new(
        WorldSave::new("Test", Some(crate::protocol::AccountId(1))),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: Some(crate::protocol::AccountId(1)),
        },
    );
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            crate::protocol::AccountId(1),
            "Tester".to_owned(),
            String::new(),
        )
        .expect("connect ok");
    // Pin to origin: the furnace below is placed at origin and these tests
    // assume the player stands within interact range of it. The random initial
    // spawn would otherwise drop the player elsewhere.
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .controller
        .position = crate::protocol::Vec3Net::ZERO;

    let entity_id = {
        let id = server.next_deployed_entity_id;
        server.next_deployed_entity_id.0 += 1;
        let furnace = FurnaceState {
            fuel: Some(ItemStack::new(WOOD_ID, 5)),
            fuel_burn_ticks_left: WOOD_BURN_TICKS / 2,
            active: true,
            ..Default::default()
        };
        let entity = crate::server::deployables::DeployedEntity {
            id,
            item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
            kind: crate::items::DeployableKind::Furnace { tier: 1 },
            position: crate::protocol::Vec3Net::ZERO,
            yaw: 0.0,
            health: 800,
            max_health: 800,
            owner: Some(crate::protocol::AccountId(1)),
            furnace: Some(furnace),
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
        id
    };
    server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::Move {
            from: FurnaceSlotRef::Fuel,
            to: FurnaceSlotRef::PlayerInventory(0),
            quantity: None,
        },
    );

    let furnace = server
        .deployed_entities
        .get(&entity_id)
        .unwrap()
        .furnace
        .as_ref()
        .unwrap();
    assert!(furnace.fuel.is_none(), "fuel slot should be empty");
    assert_eq!(
        furnace.fuel_burn_ticks_left, 0,
        "removing fuel must cancel the in-flight burn timer so the UI bar reads 0%",
    );
}

#[test]
fn partial_fuel_drag_keeps_burn_timer_running() {
    use crate::{
        auth::AuthMode,
        protocol::{GAME_VERSION, PROTOCOL_VERSION},
        save::WorldSave,
        server::ServerSettings,
    };

    let mut server = crate::server::GameServer::new(
        WorldSave::new("Test", Some(crate::protocol::AccountId(1))),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: Some(crate::protocol::AccountId(1)),
        },
    );
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            crate::protocol::AccountId(1),
            "Tester".to_owned(),
            String::new(),
        )
        .expect("connect ok");
    // Pin to origin: the furnace below is placed at origin and these tests
    // assume the player stands within interact range of it. The random initial
    // spawn would otherwise drop the player elsewhere.
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .controller
        .position = crate::protocol::Vec3Net::ZERO;

    let entity_id = {
        let id = server.next_deployed_entity_id;
        server.next_deployed_entity_id.0 += 1;
        let furnace = FurnaceState {
            fuel: Some(ItemStack::new(WOOD_ID, 5)),
            fuel_burn_ticks_left: WOOD_BURN_TICKS / 2,
            active: true,
            ..Default::default()
        };
        let entity = crate::server::deployables::DeployedEntity {
            id,
            item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
            kind: crate::items::DeployableKind::Furnace { tier: 1 },
            position: crate::protocol::Vec3Net::ZERO,
            yaw: 0.0,
            health: 800,
            max_health: 800,
            owner: Some(crate::protocol::AccountId(1)),
            furnace: Some(furnace),
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
        id
    };
    server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::Move {
            from: FurnaceSlotRef::Fuel,
            to: FurnaceSlotRef::PlayerInventory(0),
            quantity: Some(1),
        },
    );

    let furnace = server
        .deployed_entities
        .get(&entity_id)
        .unwrap()
        .furnace
        .as_ref()
        .unwrap();
    assert_eq!(
        furnace.fuel.as_ref().map(|s| s.quantity),
        Some(4),
        "partial drag should leave 4 wood",
    );
    assert_eq!(
        furnace.fuel_burn_ticks_left,
        WOOD_BURN_TICKS / 2,
        "partial drag should not cancel the in-flight burn timer",
    );
}

#[test]
fn moving_from_furnace_to_a_specific_player_inventory_slot_respects_the_target() {
    use crate::{
        auth::AuthMode,
        protocol::{GAME_VERSION, PROTOCOL_VERSION},
        save::WorldSave,
        server::ServerSettings,
    };

    let mut server = crate::server::GameServer::new(
        WorldSave::new("Test", Some(crate::protocol::AccountId(1))),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: Some(crate::protocol::AccountId(1)),
        },
    );
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            crate::protocol::AccountId(1),
            "Tester".to_owned(),
            String::new(),
        )
        .expect("connect ok");
    // Pin to origin: the furnace below is placed at origin and these tests
    // assume the player stands within interact range of it. The random initial
    // spawn would otherwise drop the player elsewhere.
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .controller
        .position = crate::protocol::Vec3Net::ZERO;

    let entity_id = {
        let id = server.next_deployed_entity_id;
        server.next_deployed_entity_id.0 += 1;
        let entity = crate::server::deployables::DeployedEntity {
            id,
            item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
            kind: crate::items::DeployableKind::Furnace { tier: 1 },
            position: crate::protocol::Vec3Net::ZERO,
            yaw: 0.0,
            health: 800,
            max_health: 800,
            owner: Some(crate::protocol::AccountId(1)),
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
        id
    };
    {
        let furnace = server
            .deployed_entities
            .get_mut(&entity_id)
            .unwrap()
            .furnace
            .as_mut()
            .unwrap();
        furnace.items[0] = Some(ItemStack::new(IRON_BAR_ID, 7));
    }
    server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);

    const TARGET: usize = 5;
    server.apply_furnace_command(
        client_id,
        FurnaceCommand::Move {
            from: FurnaceSlotRef::Item(0),
            to: FurnaceSlotRef::PlayerInventory(TARGET),
            quantity: None,
        },
    );

    let client = server.clients.get(&client_id).unwrap();
    let landed = client.inventory.inventory_slots[TARGET]
        .as_ref()
        .expect("target slot should be filled");
    assert_eq!(landed.item_id.as_ref(), IRON_BAR_ID);
    assert_eq!(landed.quantity, 7);
    for (index, slot) in client.inventory.inventory_slots.iter().enumerate() {
        if index == TARGET {
            continue;
        }
        assert!(
            slot.as_ref()
                .map(|s| s.item_id.as_ref() != IRON_BAR_ID)
                .unwrap_or(true),
            "iron bar should not appear in slot {index}; bug would have put it here",
        );
    }
    let furnace = server
        .deployed_entities
        .get(&entity_id)
        .unwrap()
        .furnace
        .as_ref()
        .unwrap();
    assert!(furnace.items[0].is_none());
}

/// Boilerplate-free fixture for the QuickTransfer tests. Spins up a
/// server, connects one client, spawns a furnace, sets it as their
/// open furnace, and returns both ids so the test body can mutate
/// the relevant slots before issuing the shift-click command.
fn furnace_test_fixture() -> (
    crate::server::GameServer,
    crate::protocol::ClientId,
    crate::protocol::DeployedEntityId,
) {
    use crate::{
        auth::AuthMode,
        protocol::{GAME_VERSION, PROTOCOL_VERSION},
        save::WorldSave,
        server::ServerSettings,
    };

    let mut server = crate::server::GameServer::new(
        WorldSave::new("Test", Some(crate::protocol::AccountId(1))),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: Some(crate::protocol::AccountId(1)),
        },
    );
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            crate::protocol::AccountId(1),
            "Tester".to_owned(),
            String::new(),
        )
        .expect("connect ok");
    // Pin to origin: the furnace below is placed at origin and these tests
    // assume the player stands within interact range of it. The random initial
    // spawn would otherwise drop the player elsewhere.
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .controller
        .position = crate::protocol::Vec3Net::ZERO;

    let entity_id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id: entity_id,
        item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
        kind: crate::items::DeployableKind::Furnace { tier: 1 },
        position: crate::protocol::Vec3Net::ZERO,
        yaw: 0.0,
        health: 800,
        max_health: 800,
        owner: Some(crate::protocol::AccountId(1)),
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
    server.deployed_entities.insert(entity_id, entity);
    server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);
    (server, client_id, entity_id)
}

fn client_inventory_slot(
    server: &crate::server::GameServer,
    client_id: crate::protocol::ClientId,
    index: usize,
) -> Option<&ItemStack> {
    server.clients[&client_id].inventory.inventory_slots[index].as_ref()
}

fn furnace_item_slot(
    server: &crate::server::GameServer,
    entity_id: crate::protocol::DeployedEntityId,
    index: usize,
) -> Option<&ItemStack> {
    server.deployed_entities[&entity_id]
        .furnace
        .as_ref()
        .unwrap()
        .items[index]
        .as_ref()
}

fn furnace_fuel_slot(
    server: &crate::server::GameServer,
    entity_id: crate::protocol::DeployedEntityId,
) -> Option<&ItemStack> {
    server.deployed_entities[&entity_id]
        .furnace
        .as_ref()
        .unwrap()
        .fuel
        .as_ref()
}

#[test]
fn quick_transfer_routes_fuel_from_player_to_fuel_slot() {
    let (mut server, client_id, entity_id) = furnace_test_fixture();
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .inventory
        .inventory_slots[2] = Some(ItemStack::new(WOOD_ID, 12));

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::PlayerInventory(2),
        },
    );

    assert!(client_inventory_slot(&server, client_id, 2).is_none());
    let fuel = furnace_fuel_slot(&server, entity_id).expect("fuel placed");
    assert_eq!(fuel.item_id.as_ref(), WOOD_ID);
    assert_eq!(fuel.quantity, 12);
}

#[test]
fn quick_transfer_routes_smeltable_from_player_to_first_empty_item_slot() {
    let (mut server, client_id, entity_id) = furnace_test_fixture();
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .inventory
        .inventory_slots[5] = Some(ItemStack::new(IRON_ORE_ID, 8));

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::PlayerInventory(5),
        },
    );

    assert!(client_inventory_slot(&server, client_id, 5).is_none());
    let ore = furnace_item_slot(&server, entity_id, 0).expect("ore landed");
    assert_eq!(ore.item_id.as_ref(), IRON_ORE_ID);
    assert_eq!(ore.quantity, 8);
}

#[test]
fn quick_transfer_merges_into_existing_furnace_stack_before_taking_empty_slot() {
    let (mut server, client_id, entity_id) = furnace_test_fixture();
    {
        let furnace = server
            .deployed_entities
            .get_mut(&entity_id)
            .unwrap()
            .furnace
            .as_mut()
            .unwrap();
        furnace.items[1] = Some(ItemStack::new(IRON_ORE_ID, 50));
    }
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .inventory
        .inventory_slots[0] = Some(ItemStack::new(IRON_ORE_ID, 30));

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::PlayerInventory(0),
        },
    );

    assert!(furnace_item_slot(&server, entity_id, 0).is_none());
    assert_eq!(
        furnace_item_slot(&server, entity_id, 1).unwrap().quantity,
        80,
        "matching stack should fill before an empty slot is consumed",
    );
    assert!(client_inventory_slot(&server, client_id, 0).is_none());
}

#[test]
fn quick_transfer_swaps_fuel_when_a_different_fuel_is_present() {
    let (mut server, client_id, entity_id) = furnace_test_fixture();
    {
        let furnace = server
            .deployed_entities
            .get_mut(&entity_id)
            .unwrap()
            .furnace
            .as_mut()
            .unwrap();
        furnace.fuel = Some(ItemStack::new(COAL_ID, 4));
        furnace.fuel_burn_ticks_left = 200;
    }
    server
        .clients
        .get_mut(&client_id)
        .unwrap()
        .inventory
        .inventory_slots[0] = Some(ItemStack::new(WOOD_ID, 5));

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::PlayerInventory(0),
        },
    );

    assert_eq!(
        furnace_fuel_slot(&server, entity_id)
            .unwrap()
            .item_id
            .as_ref(),
        WOOD_ID,
    );
    let coal_total: u16 = server.clients[&client_id]
        .inventory
        .inventory_slots
        .iter()
        .chain(server.clients[&client_id].inventory.actionbar_slots.iter())
        .filter_map(|s| s.as_ref())
        .filter(|s| s.item_id.as_ref() == COAL_ID)
        .map(|s| s.quantity)
        .sum();
    assert_eq!(coal_total, 4);
    assert_eq!(
        server.deployed_entities[&entity_id]
            .furnace
            .as_ref()
            .unwrap()
            .fuel_burn_ticks_left,
        0,
        "swap should reset the in-flight burn timer",
    );
}

#[test]
fn quick_transfer_rejects_fuel_swap_when_player_has_no_room() {
    let (mut server, client_id, entity_id) = furnace_test_fixture();
    {
        let inv = &mut server.clients.get_mut(&client_id).unwrap().inventory;
        for slot in inv.inventory_slots.iter_mut() {
            *slot = Some(ItemStack::new(crate::items::STONE_ID, 200));
        }
        for slot in inv.actionbar_slots.iter_mut() {
            *slot = Some(ItemStack::new(crate::items::STONE_ID, 200));
        }
        inv.actionbar_slots[3] = Some(ItemStack::new(WOOD_ID, 5));
    }
    {
        let furnace = server
            .deployed_entities
            .get_mut(&entity_id)
            .unwrap()
            .furnace
            .as_mut()
            .unwrap();
        furnace.fuel = Some(ItemStack::new(COAL_ID, 4));
    }

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::PlayerActionbar(3),
        },
    );

    assert_eq!(
        furnace_fuel_slot(&server, entity_id)
            .unwrap()
            .item_id
            .as_ref(),
        COAL_ID,
    );
    assert_eq!(
        server.clients[&client_id].inventory.actionbar_slots[3]
            .as_ref()
            .unwrap()
            .item_id
            .as_ref(),
        WOOD_ID,
    );
}

#[test]
fn quick_transfer_routes_furnace_item_back_into_player_inventory() {
    let (mut server, client_id, entity_id) = furnace_test_fixture();
    {
        let furnace = server
            .deployed_entities
            .get_mut(&entity_id)
            .unwrap()
            .furnace
            .as_mut()
            .unwrap();
        furnace.items[2] = Some(ItemStack::new(IRON_BAR_ID, 7));
    }

    server.apply_furnace_command(
        client_id,
        FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::Item(2),
        },
    );

    assert!(furnace_item_slot(&server, entity_id, 2).is_none());
    let bar = client_inventory_slot(&server, client_id, 0).expect("bar landed");
    assert_eq!(bar.item_id.as_ref(), IRON_BAR_ID);
    assert_eq!(bar.quantity, 7);
}

// Moved from `src/server/furnace/commands/tests.rs` so every server test
// lives under `src/server/tests/`. Kept as a nested module so the command
// fixtures stay self-contained next to the smelt-loop tests above.
mod commands_tests {
    use crate::{
        game_balance::FURNACE_INTERACT_RANGE_M,
        items::{COAL_ID, IRON_ORE_ID, WOOD_ID},
        protocol::{
            ClientId, DeployedEntityId, FurnaceCommand, FurnaceSlotRef, ItemStack, ServerMessage,
            ToastKind, Vec3Net,
        },
        server::{
            GameServer,
            furnace::FurnaceState,
            test_support::{connect_named, server},
        },
    };

    /// Build a server with one connected admin client and a furnace
    /// placed at the origin. Returns the server, client id, and furnace id.
    fn fixture() -> (GameServer, ClientId, DeployedEntityId) {
        // `connect_named` pins the client to the origin so the furnace placed at
        // origin below sits in interact range despite the random initial spawn.
        let mut server = server();
        let client_id = connect_named(&mut server, "Tester");

        let id = server.next_deployed_entity_id;
        server.next_deployed_entity_id.0 += 1;
        let entity = crate::server::deployables::DeployedEntity {
            id,
            item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
            kind: crate::items::DeployableKind::Furnace { tier: 1 },
            position: Vec3Net::ZERO,
            yaw: 0.0,
            health: 800,
            max_health: 800,
            owner: Some(crate::protocol::AccountId(1)),
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
        let out = server.apply_furnace_command(
            client,
            FurnaceCommand::Open {
                id: crate::protocol::DeployedEntityId(9999),
            },
        );
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
}
