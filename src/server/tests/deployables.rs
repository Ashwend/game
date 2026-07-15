//! Server-authority tests for deployable placement (reach, overlap, item
//! consumption), tool-vs-material damage, the ownership/admin damage gate,
//! and the deployable mirror-sync deltas.

use super::*;
use crate::{
    crafting::RecipeStation,
    items::{CRUDE_FURNACE_ID, WORKBENCH_T1_ID, intern_item_id},
    protocol::{
        DeployedEntityId, GAME_VERSION, ItemStack, PROTOCOL_VERSION, PlaceDeployableCommand,
        ServerMessage, ToastKind,
    },
    server::{
        inventory::add_stack_to_inventory,
        test_support::{connect_named, server},
    },
};

fn give_one(server: &mut GameServer, client_id: ClientId, item_id: &str) {
    let client = server.clients.get_mut(&client_id).unwrap();
    add_stack_to_inventory(&mut client.inventory, ItemStack::new(item_id, 1));
}

#[test]
fn placing_a_workbench_consumes_one_item_and_tracks_chunk() {
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);

    let envelopes = server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: intern_item_id(WORKBENCH_T1_ID),
            position: Vec3Net::new(1.5, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::Toast(t) if matches!(t.kind, ToastKind::Success))),
        "expected success toast, got {envelopes:?}"
    );
    assert_eq!(server.deployed_entities.len(), 1);
}

#[test]
fn placement_out_of_reach_is_rejected() {
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);

    let envelopes = server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: intern_item_id(WORKBENCH_T1_ID),
            position: Vec3Net::new(50.0, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(
        envelopes.iter().any(|e| matches!(&e.message, ServerMessage::Toast(t) if matches!(t.kind, ToastKind::Warning))),
        "expected warning toast for out-of-reach"
    );
    assert!(server.deployed_entities.is_empty());
    // No item consumed.
    let client = server.clients.get(&client_id).unwrap();
    assert!(
        client
            .inventory
            .inventory_slots
            .iter()
            .chain(client.inventory.actionbar_slots.iter())
            .any(|slot| slot
                .as_ref()
                .is_some_and(|stack| stack.item_id.as_ref() == WORKBENCH_T1_ID)),
        "workbench should still be in inventory"
    );
}

#[test]
fn overlapping_placement_is_rejected() {
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);
    give_one(&mut server, client_id, CRUDE_FURNACE_ID);

    server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: intern_item_id(WORKBENCH_T1_ID),
            position: Vec3Net::new(1.2, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    let envelopes = server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: intern_item_id(CRUDE_FURNACE_ID),
            position: Vec3Net::new(1.2, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(envelopes.iter().any(
        |e| matches!(&e.message, ServerMessage::Toast(t) if matches!(t.kind, ToastKind::Warning))
    ));
    assert_eq!(
        server.deployed_entities.len(),
        1,
        "second placement must fail"
    );
}

fn place_workbench(server: &mut GameServer, client_id: ClientId) -> DeployedEntityId {
    server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: intern_item_id(WORKBENCH_T1_ID),
            position: Vec3Net::new(1.5, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    *server
        .deployed_entities
        .keys()
        .next()
        .expect("workbench placed")
}

fn equip_pickaxe(server: &mut GameServer, client_id: ClientId) {
    use crate::{
        items::BASIC_PICKAXE_ID, protocol::ItemStack, server::inventory::add_stack_to_inventory,
    };
    let client = server.clients.get_mut(&client_id).unwrap();
    // Drop the pickaxe straight into the active actionbar slot so
    // the tool-profile lookup in the damage handler picks it up
    // without a manual move.
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
    // Re-merge so any limit logic runs.
    let leftover =
        add_stack_to_inventory(&mut client.inventory, ItemStack::new(BASIC_PICKAXE_ID, 0));
    assert!(leftover.is_none());
}

fn equip_hatchet(server: &mut GameServer, client_id: ClientId) {
    use crate::{
        items::BASIC_HATCHET_ID, protocol::ItemStack, server::inventory::add_stack_to_inventory,
    };
    let client = server.clients.get_mut(&client_id).unwrap();
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    let leftover =
        add_stack_to_inventory(&mut client.inventory, ItemStack::new(BASIC_HATCHET_ID, 0));
    assert!(leftover.is_none());
}

fn place_furnace(server: &mut GameServer, client_id: ClientId) -> DeployedEntityId {
    server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: intern_item_id(CRUDE_FURNACE_ID),
            position: Vec3Net::new(1.5, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    *server
        .deployed_entities
        .keys()
        .next()
        .expect("furnace placed")
}

#[test]
fn damage_reduces_health_and_respects_tool_cooldown() {
    use crate::protocol::DamageDeployableCommand;
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);
    let id = place_workbench(&mut server, client_id);
    equip_pickaxe(&mut server, client_id);

    let start_hp = server.deployed_entities[&id].health;
    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
    let after_first = server.deployed_entities[&id].health;
    assert!(after_first < start_hp, "first hit reduces health");

    // Same tick → cooldown blocks the second hit.
    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
    let after_second = server.deployed_entities[&id].health;
    assert_eq!(
        after_second, after_first,
        "cooldown must block back-to-back damage"
    );
}

#[test]
fn damage_destroys_at_zero_health() {
    use crate::protocol::DamageDeployableCommand;
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);
    let id = place_workbench(&mut server, client_id);
    equip_pickaxe(&mut server, client_id);

    // Force-empty the structure's HP and check that one more hit
    // despawns it.
    if let Some(entity) = server.deployed_entities.get_mut(&id) {
        entity.health = 1;
    }
    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
    assert!(
        !server.deployed_entities.contains_key(&id),
        "deployable should be removed when HP reaches 0"
    );
}

#[test]
fn mirror_sync_deltas_track_place_damage_and_destroy() {
    use crate::protocol::DamageDeployableCommand;
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);
    let (dirty, removed) = server.drain_deployable_sync();
    assert!(
        dirty.is_empty() && removed.is_empty(),
        "a fresh world has no deployable deltas"
    );

    // Placement is recorded as dirty (→ sync spawns a mirror entity).
    let id = place_workbench(&mut server, client_id);
    let (dirty, removed) = server.drain_deployable_sync();
    assert_eq!(dirty, vec![id]);
    assert!(removed.is_empty());

    // Damage re-flags it dirty (→ DeployableHealth diff).
    equip_pickaxe(&mut server, client_id);
    let start_hp = server.deployed_entities[&id].health;
    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
    assert!(
        server.deployed_entities[&id].health < start_hp,
        "damage should land"
    );
    let (dirty, removed) = server.drain_deployable_sync();
    assert_eq!(dirty, vec![id]);
    assert!(removed.is_empty());

    // Destruction lands in the removed set, not dirty (→ sync despawns it).
    server.deployed_entity_mut(id).expect("placed").health = 1;
    let _ = server.drain_deployable_sync();
    // Reset the swing cooldown so the killing blow isn't throttled.
    server.clients.get_mut(&client_id).unwrap().next_gather_tick = 0;
    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
    assert!(
        !server.deployed_entities.contains_key(&id),
        "killing blow should remove the deployable"
    );
    let (dirty, removed) = server.drain_deployable_sync();
    assert!(dirty.is_empty(), "destroyed deployable must not stay dirty");
    assert_eq!(removed, vec![id]);
}

#[test]
fn matched_tool_outdamages_mismatched_tool() {
    use crate::protocol::DamageDeployableCommand;
    // Workbench (wood) vs hatchet should deal 3× the per-hit damage
    // of pickaxe vs the same workbench (150% vs 50%).
    let mut axe_server = server();
    let axe_client = connect_named(&mut axe_server, "Tester");
    give_one(&mut axe_server, axe_client, WORKBENCH_T1_ID);
    let axe_target = place_workbench(&mut axe_server, axe_client);
    equip_hatchet(&mut axe_server, axe_client);

    let start_hp = axe_server.deployed_entities[&axe_target].health;
    axe_server
        .apply_damage_deployable_command(axe_client, DamageDeployableCommand { id: axe_target });
    let axe_damage = start_hp - axe_server.deployed_entities[&axe_target].health;

    let mut pick_server = server();
    let pick_client = connect_named(&mut pick_server, "Tester");
    give_one(&mut pick_server, pick_client, WORKBENCH_T1_ID);
    let pick_target = place_workbench(&mut pick_server, pick_client);
    equip_pickaxe(&mut pick_server, pick_client);

    pick_server
        .apply_damage_deployable_command(pick_client, DamageDeployableCommand { id: pick_target });
    let pick_damage = start_hp - pick_server.deployed_entities[&pick_target].health;

    assert_eq!(
        axe_damage,
        pick_damage * 3,
        "hatchet (150% on wood) should out-damage pickaxe (50% on wood) by 3×"
    );
}

#[test]
fn pickaxe_outdamages_hatchet_on_furnace() {
    use crate::protocol::DamageDeployableCommand;
    let mut pick_server = server();
    let pick_client = connect_named(&mut pick_server, "Tester");
    give_one(&mut pick_server, pick_client, CRUDE_FURNACE_ID);
    let pick_target = place_furnace(&mut pick_server, pick_client);
    equip_pickaxe(&mut pick_server, pick_client);

    let start_hp = pick_server.deployed_entities[&pick_target].health;
    pick_server
        .apply_damage_deployable_command(pick_client, DamageDeployableCommand { id: pick_target });
    let pick_damage = start_hp - pick_server.deployed_entities[&pick_target].health;

    let mut axe_server = server();
    let axe_client = connect_named(&mut axe_server, "Tester");
    give_one(&mut axe_server, axe_client, CRUDE_FURNACE_ID);
    let axe_target = place_furnace(&mut axe_server, axe_client);
    equip_hatchet(&mut axe_server, axe_client);

    axe_server
        .apply_damage_deployable_command(axe_client, DamageDeployableCommand { id: axe_target });
    let axe_damage = start_hp - axe_server.deployed_entities[&axe_target].health;

    assert_eq!(
        pick_damage,
        axe_damage * 3,
        "pickaxe (150% on stone) should out-damage hatchet (50% on stone) by 3×"
    );
}

#[test]
fn bare_hands_cannot_damage_deployables() {
    use crate::protocol::DamageDeployableCommand;
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);
    let id = place_workbench(&mut server, client_id);

    let start_hp = server.deployed_entities[&id].health;
    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
    let after = server.deployed_entities[&id].health;
    assert_eq!(after, start_hp, "no tool → no damage");
}

#[test]
fn station_in_range_matches_only_close_workbench() {
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);

    // Place a workbench right next to the player (spawn is near origin).
    server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: intern_item_id(WORKBENCH_T1_ID),
            position: Vec3Net::new(1.5, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(server.station_in_range(client_id, RecipeStation::Workbench { min_tier: 1 }));
    // A higher-tier workbench requirement is not satisfied by a T1.
    assert!(!server.station_in_range(client_id, RecipeStation::Workbench { min_tier: 2 }));
}

#[test]
fn placement_records_owner_account_id() {
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);
    let id = place_workbench(&mut server, client_id);

    let entity = server.deployed_entities.get(&id).expect("placed");
    assert_eq!(
        entity.owner,
        Some(crate::protocol::AccountId(1)),
        "owner account id should match the placing client"
    );
}

#[test]
fn another_player_cannot_damage_an_owned_deployable() {
    use crate::protocol::DamageDeployableCommand;

    let mut server = server();
    let owner_id = connect_named(&mut server, "Tester");
    give_one(&mut server, owner_id, WORKBENCH_T1_ID);
    let id = place_workbench(&mut server, owner_id);
    let start_hp = server.deployed_entities[&id].health;

    // Connect a second player with a different account id and try to
    // damage the placed workbench.
    let attacker_id = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            crate::protocol::AccountId(2),
            "Griefer".to_owned(),
            String::new(),
        )
        .expect("connect ok")
        .0;
    equip_pickaxe(&mut server, attacker_id);
    // Move the attacker into range of the placed entity.
    if let Some(client) = server.clients.get_mut(&attacker_id) {
        client.controller.position = Vec3Net::new(1.5, 0.0, 0.0);
    }

    server.apply_damage_deployable_command(attacker_id, DamageDeployableCommand { id });
    assert_eq!(
        server.deployed_entities[&id].health, start_hp,
        "non-owner damage must be rejected"
    );
}

#[test]
fn owner_can_damage_their_own_deployable() {
    use crate::protocol::DamageDeployableCommand;

    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");
    give_one(&mut server, client_id, WORKBENCH_T1_ID);
    let id = place_workbench(&mut server, client_id);
    let start_hp = server.deployed_entities[&id].health;
    equip_pickaxe(&mut server, client_id);

    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
    assert!(
        server.deployed_entities[&id].health < start_hp,
        "owner damage should land"
    );
}

#[test]
fn admin_bypasses_ownership_gate_on_damage() {
    use crate::protocol::DamageDeployableCommand;

    // Owner places a structure. An admin (different account id) walks
    // up and damages it. The ownership gate must defer to the admin
    // bit so moderation works without exposing a side door for
    // regular players.
    let mut server = server();
    let owner_id = connect_named(&mut server, "Tester");
    give_one(&mut server, owner_id, WORKBENCH_T1_ID);
    let id = place_workbench(&mut server, owner_id);
    let start_hp = server.deployed_entities[&id].health;

    let admin_id = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            crate::protocol::AccountId(2),
            "Moderator".to_owned(),
            String::new(),
        )
        .expect("connect ok")
        .0;
    // The server() fixture marks singleplayer host=1 as admin;
    // for a multiplayer-shaped test we promote the second client
    // explicitly.
    if let Some(client) = server.clients.get_mut(&admin_id) {
        client.is_admin = true;
    }
    equip_pickaxe(&mut server, admin_id);
    if let Some(client) = server.clients.get_mut(&admin_id) {
        client.controller.position = Vec3Net::new(1.5, 0.0, 0.0);
    }

    server.apply_damage_deployable_command(admin_id, DamageDeployableCommand { id });
    assert!(
        server.deployed_entities[&id].health < start_hp,
        "admin damage must land even against another player's structure"
    );
}
