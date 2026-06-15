//! Tool-durability consumption tests.
//!
//! Wear is charged only on swings that connect (gather payout, PvP hit,
//! structure hit) and the swing that empties the budget still lands its
//! effect before the tool breaks.

use super::*;
use crate::{
    game_balance::STONE_TOOL_DURABILITY,
    items::BASIC_PICKAXE_ID,
    protocol::{
        AccountId, AttackPlayerCommand, DamageDeployableCommand, PROTOCOL_VERSION,
        PlaceDeployableCommand, ResourceGatherCommand, ToastKind,
    },
};

/// Connect with an explicit account id so two test players don't collide
/// on the shared `test_support::connect_named` account (same account =
/// the second connect hard-disconnects the first).
fn connect_account(server: &mut GameServer, account_id: AccountId, name: &str) -> ClientId {
    server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            account_id,
            name.to_owned(),
            String::new(),
        )
        .expect("connect should succeed")
        .0
}

fn coal_node(id: u64, quantity: u16) -> ResourceNodeState {
    ResourceNodeState {
        id,
        definition_id: COAL_NODE_ID.to_owned(),
        position: Vec3Net::new(0.0, 0.0, -2.2),
        yaw: 0.0,
        storage: vec![ItemStack::new(COAL_ID, quantity)],
        dead: false,
    }
}

fn look_at_test_node(server: &mut GameServer, client_id: ClientId) {
    let mut movement = movement(1, Vec3Net::ZERO);
    movement.pitch = -0.42;
    server.receive(client_id, ClientMessage::Movement(movement));
}

fn equip_pickaxe(server: &mut GameServer, client_id: ClientId) {
    let client = server.clients.get_mut(&client_id).expect("client exists");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
    client.inventory.active_actionbar_slot = 0;
}

fn active_durability(server: &GameServer, client_id: ClientId) -> Option<u32> {
    server
        .clients
        .get(&client_id)
        .and_then(|client| client.inventory.active_actionbar_stack())
        .and_then(|stack| stack.durability)
}

fn set_active_durability(server: &mut GameServer, client_id: ClientId, value: u32) {
    let client = server.clients.get_mut(&client_id).expect("client exists");
    let slot = client.inventory.active_actionbar_slot;
    let stack = client.inventory.actionbar_slots[slot]
        .as_mut()
        .expect("active stack");
    stack.durability = Some(value);
}

fn gather(server: &mut GameServer, client_id: ClientId, node_id: u64) -> Vec<ServerEnvelope> {
    server.apply_gather_command(
        client_id,
        ResourceGatherCommand {
            resource_node_id: node_id,
            seq: 0,
            hit_point: Vec3Net::ZERO,
        },
    )
}

#[test]
fn gather_impact_consumes_one_durability() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_pickaxe(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 50));
    look_at_test_node(&mut server, client_id);

    assert_eq!(
        active_durability(&server, client_id),
        Some(STONE_TOOL_DURABILITY),
        "fresh tool spawns at full durability"
    );
    gather(&mut server, client_id, 99);
    assert_eq!(
        active_durability(&server, client_id),
        Some(STONE_TOOL_DURABILITY - 1),
        "a connecting gather swing costs one durability"
    );
}

#[test]
fn gather_with_full_inventory_still_wears_the_tool() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_pickaxe(&mut server, client_id);
    // Brick every other slot so the payout can't fit anywhere.
    {
        let client = server.clients.get_mut(&client_id).expect("client exists");
        for slot in client.inventory.inventory_slots.iter_mut() {
            *slot = Some(ItemStack::new(crate::items::WOOD_ID, 200));
        }
        for slot in client.inventory.actionbar_slots.iter_mut().skip(1) {
            *slot = Some(ItemStack::new(crate::items::WOOD_ID, 200));
        }
    }
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 50));
    look_at_test_node(&mut server, client_id);

    let envelopes = gather(&mut server, client_id, 99);
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.message,
            ServerMessage::Toast(toast) if matches!(toast.kind, ToastKind::Warning)
        )),
        "bag-full gather should warn"
    );
    assert_eq!(
        active_durability(&server, client_id),
        Some(STONE_TOOL_DURABILITY - 1),
        "the tool still struck the node, so it wears"
    );
}

#[test]
fn breaking_swing_still_pays_out_and_clears_the_slot() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_pickaxe(&mut server, client_id);
    set_active_durability(&mut server, client_id, 1);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 50));
    look_at_test_node(&mut server, client_id);

    let envelopes = gather(&mut server, client_id, 99);

    let client = server.clients.get(&client_id).expect("client exists");
    assert!(
        client.inventory.actionbar_slots[0].is_none(),
        "tool at zero durability is removed from the actionbar"
    );
    assert!(
        client.inventory.inventory_slots.iter().any(|slot| {
            slot.as_ref()
                .is_some_and(|stack| stack.item_id.as_ref() == COAL_ID)
        }),
        "the breaking swing still granted its payout"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.message,
            ServerMessage::Toast(toast)
                if matches!(toast.kind, ToastKind::Warning) && toast.text.contains("broke")
        )),
        "the owner is told their tool broke: {envelopes:?}"
    );
}

#[test]
fn pvp_hit_consumes_attacker_durability() {
    let mut server = server();
    let attacker = connect_account(&mut server, 1, "Attacker");
    let target = connect_account(&mut server, 2, "Target");
    equip_pickaxe(&mut server, attacker);

    // Attacker at origin facing -Z, target 2 m in front.
    let mut attacker_move = movement(10, Vec3Net::ZERO);
    attacker_move.yaw = 0.0;
    server.receive(attacker, ClientMessage::Movement(attacker_move));
    server.receive(
        target,
        ClientMessage::Movement(movement(10, Vec3Net::new(0.0, 0.0, -2.0))),
    );

    server.apply_attack_player_command(
        attacker,
        AttackPlayerCommand {
            target_player_id: target,
        },
    );
    assert_eq!(
        active_durability(&server, attacker),
        Some(STONE_TOOL_DURABILITY - 1),
        "a landed PvP swing costs one durability"
    );
}

#[test]
fn rejected_pvp_swing_costs_no_durability() {
    let mut server = server();
    let attacker = connect_account(&mut server, 1, "Attacker");
    let target = connect_account(&mut server, 2, "Target");
    equip_pickaxe(&mut server, attacker);

    // Target far out of melee range: the swing is rejected.
    server.receive(
        target,
        ClientMessage::Movement(movement(10, Vec3Net::new(0.0, 0.0, -50.0))),
    );
    server.apply_attack_player_command(
        attacker,
        AttackPlayerCommand {
            target_player_id: target,
        },
    );
    assert_eq!(
        active_durability(&server, attacker),
        Some(STONE_TOOL_DURABILITY),
        "a rejected swing must not wear the tool"
    );
}

#[test]
fn deployable_hit_consumes_durability() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    // Give the player a workbench and place it in reach.
    {
        let client = server.clients.get_mut(&client_id).expect("client exists");
        client.inventory.inventory_slots[0] =
            Some(ItemStack::new(crate::items::WORKBENCH_T1_ID, 1));
    }
    server.receive(
        client_id,
        ClientMessage::Movement(movement(5, Vec3Net::ZERO)),
    );
    server.apply_place_deployable_command(
        client_id,
        PlaceDeployableCommand {
            item_id: crate::items::intern_item_id(crate::items::WORKBENCH_T1_ID),
            position: Vec3Net::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    let deployed_id = *server
        .deployed_entities
        .keys()
        .next()
        .expect("workbench placed");

    equip_pickaxe(&mut server, client_id);
    server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id: deployed_id });
    assert_eq!(
        active_durability(&server, client_id),
        Some(STONE_TOOL_DURABILITY - 1),
        "a structure hit costs one durability"
    );
}

#[test]
fn non_tool_stacks_never_wear() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    {
        let client = server.clients.get_mut(&client_id).expect("client exists");
        client.inventory.actionbar_slots[0] = Some(ItemStack::new(COAL_ID, 5));
        client.inventory.active_actionbar_slot = 0;
    }
    let envelopes = server.consume_active_tool_durability(client_id);
    assert!(envelopes.is_empty());
    let client = server.clients.get(&client_id).expect("client exists");
    let stack = client.inventory.actionbar_slots[0]
        .as_ref()
        .expect("stack untouched");
    assert_eq!(stack.durability, None);
    assert_eq!(stack.quantity, 5);
}
