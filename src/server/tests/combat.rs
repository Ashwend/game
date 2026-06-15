//! Server-authoritative PvP combat tests.
//!
//! Each test forges a malicious or boundary `AttackPlayerCommand` and
//! verifies the server's response, rejection without HP change for
//! anti-cheat cases, the expected damage/knockback/peer broadcast for
//! the happy path.

use super::*;
use crate::{
    game_balance::{STONE_HATCHET_PVP_DAMAGE, STONE_PICKAXE_PVP_DAMAGE},
    items::{BASIC_HATCHET_ID, BASIC_PICKAXE_ID},
    protocol::{
        AccountId, AttackPlayerCommand, ClientMessage, ItemStack, LootBagCommand, LootBagSlotRef,
        MAX_HEALTH, PROTOCOL_VERSION, PlayerMovement,
    },
    server::loot_bag::OpenContainer,
};

fn connect_named(server: &mut GameServer, account_id: AccountId, name: &str) -> ClientId {
    server
        .connect(
            PROTOCOL_VERSION,
            Some(crate::protocol::GAME_VERSION.to_owned()),
            account_id,
            name.to_owned(),
            String::new(),
        )
        .expect("connect should succeed")
        .0
}

fn place_player(server: &mut GameServer, client_id: ClientId, position: Vec3Net, yaw: f32) {
    // Feed one movement message to relocate the controller. The
    // sequence climbs from whatever the controller saw last so the
    // accept path doesn't reject as stale.
    let next_sequence = server
        .clients
        .get(&client_id)
        .map(|c| c.controller.last_processed_input.saturating_add(1))
        .unwrap_or(1);
    server.receive(
        client_id,
        ClientMessage::Movement(PlayerMovement {
            sequence: next_sequence,
            position,
            velocity: Vec3Net::ZERO,
            yaw,
            pitch: 0.0,
            grounded: true,
        }),
    );
}

fn equip_axe(server: &mut GameServer, client_id: ClientId) {
    let client = server.clients.get_mut(&client_id).expect("client exists");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    client.inventory.active_actionbar_slot = 0;
}

fn equip_pickaxe(server: &mut GameServer, client_id: ClientId) {
    let client = server.clients.get_mut(&client_id).expect("client exists");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
    client.inventory.active_actionbar_slot = 0;
}

fn attack(server: &mut GameServer, attacker: ClientId, target: ClientId) -> Vec<ServerEnvelope> {
    server.apply_attack_player_command(
        attacker,
        AttackPlayerCommand {
            target_player_id: target,
        },
    )
}

fn target_health(server: &GameServer, target: ClientId) -> f32 {
    server
        .clients
        .get(&target)
        .map(|c| c.controller.health)
        .unwrap_or(0.0)
}

#[test]
fn attack_in_range_with_axe_applies_damage_and_emits_player_impact() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    // Attacker at origin, target 2 m in front (-Z). Attacker faces -Z (yaw=0).
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);

    let start_hp = target_health(&server, target);
    let envelopes = attack(&mut server, attacker, target);
    let new_hp = target_health(&server, target);

    assert_eq!(
        start_hp - new_hp,
        STONE_HATCHET_PVP_DAMAGE as f32,
        "stone axe should deal STONE_HATCHET_PVP_DAMAGE in HP",
    );
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::PlayerImpact { .. })),
        "PlayerImpact should be in the envelope set"
    );
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::Knockback { .. })),
        "Knockback should be in the envelope set"
    );
}

#[test]
fn point_blank_attack_lands_with_level_aim() {
    // Regression: when the target is very close, the attacker's eye (1.62 m)
    // sits well above the target's chest, so the old eye->chest view-cone test
    // judged the look direction as pointing too far down and rejected the hit,
    // even with the crosshair dead on the body. The attacker still saw their
    // predicted impact + damage text while the victim took no damage. The aim
    // test now matches the client (look ray vs body box), so a level-aim swing
    // at point-blank range must register.
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    // Half a metre apart, attacker facing -Z with level pitch (place_player
    // sets pitch = 0).
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -0.5), 0.0);
    equip_axe(&mut server, attacker);

    let start_hp = target_health(&server, target);
    let envelopes = attack(&mut server, attacker, target);
    let new_hp = target_health(&server, target);

    assert_eq!(
        start_hp - new_hp,
        STONE_HATCHET_PVP_DAMAGE as f32,
        "a point-blank level-aim swing must deal damage, not silently miss",
    );
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::PlayerImpact { .. })),
        "the victim's peers must get a PlayerImpact for a point-blank hit"
    );
}

#[test]
fn non_fatal_hit_sends_the_victim_a_health_correction() {
    // The victim renders their HP bar from local prediction, which only moves
    // when the server sends a `Correction`. A landed hit must therefore push
    // one to the target carrying the reduced health.
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);

    let start_hp = target_health(&server, target);
    let envelopes = attack(&mut server, attacker, target);
    let new_hp = target_health(&server, target);
    assert!(new_hp < start_hp, "the hit must reduce server-side HP");
    assert!(new_hp > 0.0, "this fixture is a non-fatal hit");

    let correction = envelopes
        .iter()
        .find_map(|e| match (&e.target, &e.message) {
            (DeliveryTarget::Client(id), ServerMessage::Correction(state)) if *id == target => {
                Some(state)
            }
            _ => None,
        })
        .expect("a landed hit must send the victim a health Correction");
    assert_eq!(correction.client_id, target);
    assert_eq!(
        correction.health, new_hp,
        "the correction must carry the victim's reduced health"
    );
}

#[test]
fn a_sleeping_body_can_be_attacked_and_damaged() {
    // A logged-out player's body stays in the world as a sleeper; it is a
    // living body, so it can be swung on and killed (its gear then drops as a
    // loot bag, and the owner respawns fresh on their next login).
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let victim = connect_named(&mut server, 2, "Victim");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, victim, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);

    // Victim logs out: their body sleeps where it stood.
    server.disconnect(victim);
    assert!(
        server
            .clients
            .get(&victim)
            .is_some_and(|client| !client.online),
        "the victim's body should be asleep"
    );

    // A nearby online witness; the impact cue is range-gated to online
    // clients, so the sleeping victim itself receives nothing.
    let witness = connect_named(&mut server, 3, "Witness");
    place_player(&mut server, witness, Vec3Net::new(2.0, 0.0, 0.0), 0.0);

    let start_hp = target_health(&server, victim);
    let envelopes = attack(&mut server, attacker, victim);
    assert!(
        target_health(&server, victim) < start_hp,
        "a sleeping body still takes damage"
    );
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::PlayerImpact { .. })
                && matches!(e.target, DeliveryTarget::Client(id) if id == witness)),
        "the hit on a sleeper reaches nearby online peers like any other"
    );
}

#[test]
fn a_sleeper_is_hittable_without_facing_it() {
    // The aim cone is waived for a helpless laid-out sleeper (range + LOS still
    // apply), so a standing player can strike the body without centring it. A
    // standing target facing away the same way would be out of the cone.
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let victim = connect_named(&mut server, 2, "Victim");
    // Attacker faces +Z (yaw = PI), away from the victim sitting at -Z.
    place_player(
        &mut server,
        attacker,
        Vec3Net::new(0.0, 0.0, 0.0),
        std::f32::consts::PI,
    );
    place_player(&mut server, victim, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);

    // While the victim is online, facing away misses the aim cone.
    let start_hp = target_health(&server, victim);
    let _ = attack(&mut server, attacker, victim);
    assert_eq!(
        target_health(&server, victim),
        start_hp,
        "a standing target outside the aim cone is not hit"
    );

    // Once asleep, the cone is waived and the same swing lands.
    server.disconnect(victim);
    let _ = attack(&mut server, attacker, victim);
    assert!(
        target_health(&server, victim) < start_hp,
        "a sleeper is hittable even when not centred in the aim cone"
    );
}

#[test]
fn looting_a_sleeper_opens_their_live_inventory_non_destructively() {
    let mut server = server();
    let looter = connect_named(&mut server, 1, "Looter");
    let target = connect_named(&mut server, 2, "Sleeper");
    place_player(&mut server, looter, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -1.0), 0.0);
    server
        .clients
        .get_mut(&target)
        .unwrap()
        .inventory
        .inventory_slots[0] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));

    // An awake player can't be looted.
    assert!(server.apply_loot_sleeper(looter, target).is_empty());
    assert!(
        server.clients[&looter].open_container.is_none(),
        "an online player can't be opened for looting"
    );

    // Once they sleep, opening loots their *live* inventory: nothing is moved,
    // the view mirrors the body's slots, and the body's pack is untouched.
    server.disconnect(target);
    let _ = server.apply_loot_sleeper(looter, target);
    assert_eq!(
        server.clients[&looter].open_container,
        Some(OpenContainer::Sleeper(target)),
        "the looter has the sleeper's live inventory open"
    );
    assert!(
        server.clients[&target].inventory.inventory_slots[0].is_some(),
        "opening alone moves nothing off the body"
    );
    let view = server
        .open_loot_bag_view_for(looter)
        .expect("the open sleeper has a view");
    assert_eq!(
        view.slots.iter().flatten().count(),
        1,
        "the view mirrors the sleeper's one item"
    );

    // Re-opening and closing without taking leaves the body exactly as it was
    // (the bug this fixes: the old spill-to-bag emptied the body on first open).
    server.close_container(looter);
    let _ = server.apply_loot_sleeper(looter, target);
    assert!(
        server.clients[&target].inventory.inventory_slots[0].is_some(),
        "a closed-then-reopened body still holds its items"
    );

    // Taking the item via a Move removes only that stack from the body. Clear
    // the looter's destination slot first so the starting kit can't force a swap
    // that would push an item back onto the body.
    server
        .clients
        .get_mut(&looter)
        .unwrap()
        .inventory
        .inventory_slots[0] = None;
    let _ = server.apply_loot_bag_command(
        looter,
        LootBagCommand::Move {
            from: LootBagSlotRef::Bag(0),
            to: LootBagSlotRef::PlayerInventory(0),
            quantity: None,
        },
    );
    assert!(
        server.clients[&target].inventory.inventory_slots[0].is_none(),
        "the looted slot is emptied on the body"
    );
    assert_eq!(
        server.clients[&looter].inventory.inventory_slots[0]
            .as_ref()
            .map(|s| s.item_id.as_ref()),
        Some(BASIC_HATCHET_ID),
        "the looter received the item"
    );
}

#[test]
fn an_empty_sleeper_still_opens_showing_nothing() {
    let mut server = server();
    let looter = connect_named(&mut server, 1, "Looter");
    let target = connect_named(&mut server, 2, "Sleeper");
    place_player(&mut server, looter, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -1.0), 0.0);

    server.disconnect(target);
    let envelopes = server.apply_loot_sleeper(looter, target);
    assert!(
        envelopes.is_empty(),
        "opening an empty body is not a rejected warning, it just opens"
    );
    assert_eq!(
        server.clients[&looter].open_container,
        Some(OpenContainer::Sleeper(target)),
        "an empty body still opens"
    );
    let view = server
        .open_loot_bag_view_for(looter)
        .expect("the empty sleeper still has a view");
    assert_eq!(
        view.slots.iter().flatten().count(),
        0,
        "the view shows an empty body"
    );
}

#[test]
fn waking_a_sleeper_closes_anyone_looting_it() {
    let mut server = server();
    let looter = connect_named(&mut server, 1, "Looter");
    let target = connect_named(&mut server, 2, "Sleeper");
    place_player(&mut server, looter, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -1.0), 0.0);

    server.disconnect(target);
    let _ = server.apply_loot_sleeper(looter, target);
    assert!(server.clients[&looter].open_container.is_some());

    // The body wakes (reconnects): the looter's view must close.
    let _ = server.connect(
        PROTOCOL_VERSION,
        Some(crate::protocol::GAME_VERSION.to_owned()),
        2,
        "Sleeper".to_owned(),
        String::new(),
    );
    assert!(
        server.clients[&looter].open_container.is_none(),
        "waking the body closes any looter viewing it"
    );
}

#[test]
fn attack_outside_range_is_rejected_without_damage() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -20.0), 0.0);
    equip_axe(&mut server, attacker);

    let start_hp = target_health(&server, target);
    let envelopes = attack(&mut server, attacker, target);
    let new_hp = target_health(&server, target);

    assert_eq!(start_hp, new_hp, "out-of-range attack must not damage");
    assert!(envelopes.is_empty(), "rejected attacks emit no envelopes");
}

#[test]
fn attack_without_tool_is_rejected() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    // No tool equipped (bare hands).

    let start_hp = target_health(&server, target);
    attack(&mut server, attacker, target);
    let new_hp = target_health(&server, target);

    assert_eq!(start_hp, new_hp, "bare hands can't damage players");
}

#[test]
fn attack_self_is_rejected() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    equip_axe(&mut server, attacker);

    let start_hp = target_health(&server, attacker);
    attack(&mut server, attacker, attacker);
    let new_hp = target_health(&server, attacker);

    assert_eq!(start_hp, new_hp, "self-damage must be rejected");
}

#[test]
fn attack_dead_target_is_rejected() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);
    server.clients.get_mut(&target).unwrap().controller.health = 0.0;

    let envelopes = attack(&mut server, attacker, target);

    assert_eq!(
        target_health(&server, target),
        0.0,
        "dead targets stay dead"
    );
    assert!(
        envelopes.is_empty(),
        "no PlayerImpact emitted for an already-dead target"
    );
}

#[test]
fn attack_in_cooldown_is_rejected() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);

    let first_envelopes = attack(&mut server, attacker, target);
    assert!(!first_envelopes.is_empty(), "first attack should land",);
    let after_first = target_health(&server, target);

    // Same tick → cooldown blocks the next swing.
    let second_envelopes = attack(&mut server, attacker, target);
    let after_second = target_health(&server, target);
    assert_eq!(
        after_first, after_second,
        "in-cooldown attack must not reduce HP"
    );
    assert!(
        second_envelopes.is_empty(),
        "in-cooldown attack emits nothing"
    );
}

#[test]
fn pickaxe_deals_more_damage_than_axe_in_one_swing() {
    let mut axe_server = server();
    let axe_attacker = connect_named(&mut axe_server, 1, "Axer");
    let axe_target = connect_named(&mut axe_server, 2, "Target");
    place_player(
        &mut axe_server,
        axe_attacker,
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
    );
    place_player(
        &mut axe_server,
        axe_target,
        Vec3Net::new(0.0, 0.0, -2.0),
        0.0,
    );
    equip_axe(&mut axe_server, axe_attacker);
    let axe_start = target_health(&axe_server, axe_target);
    attack(&mut axe_server, axe_attacker, axe_target);
    let axe_damage = axe_start - target_health(&axe_server, axe_target);

    let mut pick_server = server();
    let pick_attacker = connect_named(&mut pick_server, 1, "Picker");
    let pick_target = connect_named(&mut pick_server, 2, "Target");
    place_player(
        &mut pick_server,
        pick_attacker,
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
    );
    place_player(
        &mut pick_server,
        pick_target,
        Vec3Net::new(0.0, 0.0, -2.0),
        0.0,
    );
    equip_pickaxe(&mut pick_server, pick_attacker);
    let pick_start = target_health(&pick_server, pick_target);
    attack(&mut pick_server, pick_attacker, pick_target);
    let pick_damage = pick_start - target_health(&pick_server, pick_target);

    assert_eq!(axe_damage, STONE_HATCHET_PVP_DAMAGE as f32);
    assert_eq!(pick_damage, STONE_PICKAXE_PVP_DAMAGE as f32);
    assert!(
        pick_damage > axe_damage,
        "pickaxe (slow burst) should deal more per-swing than axe (fast DPS)"
    );
}

#[test]
fn attack_applies_armor_reduction() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);
    server.clients.get_mut(&target).unwrap().armor = 50; // 50% reduction

    let start = target_health(&server, target);
    attack(&mut server, attacker, target);
    let damage = start - target_health(&server, target);
    // 8 raw × (100-50)/100 = 4
    assert_eq!(damage, (STONE_HATCHET_PVP_DAMAGE as f32) / 2.0);
}

#[test]
fn attack_behind_attacker_is_rejected_by_view_cone() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    // Attacker faces -Z (yaw 0); target sits behind at +Z.
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, 2.0), 0.0);
    equip_axe(&mut server, attacker);

    let start = target_health(&server, target);
    attack(&mut server, attacker, target);
    assert_eq!(start, target_health(&server, target));
}

#[test]
fn player_impact_reaches_nearby_peers_but_not_attacker_or_distant_clients() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    // A spectator far outside IMPACT_MESSAGE_RANGE_M: the cosmetic cue
    // must not be shipped across the map to clients who can neither
    // hear nor see it.
    let distant = connect_named(&mut server, 3, "Distant");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    place_player(
        &mut server,
        distant,
        Vec3Net::new(crate::game_balance::IMPACT_MESSAGE_RANGE_M * 2.0, 0.0, 0.0),
        0.0,
    );
    equip_axe(&mut server, attacker);

    let envelopes = attack(&mut server, attacker, target);
    let impact_targets: Vec<_> = envelopes
        .iter()
        .filter(|e| matches!(&e.message, ServerMessage::PlayerImpact { .. }))
        .map(|e| e.target.clone())
        .collect();
    assert_eq!(
        impact_targets,
        vec![DeliveryTarget::Client(target)],
        "PlayerImpact goes to nearby peers only: not the attacker (their          client predicted it) and not out-of-range clients"
    );

    let knockback = envelopes
        .iter()
        .find(|e| matches!(&e.message, ServerMessage::Knockback { .. }))
        .expect("Knockback envelope present");
    assert!(
        matches!(knockback.target, DeliveryTarget::Client(id) if id == target),
        "Knockback should target the victim only, got {:?}",
        knockback.target
    );
}

#[test]
fn player_dies_spawns_loot_bag_and_emits_player_killed() {
    use crate::items::WOOD_ID;
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Victim");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);
    // Seed loot so the death has something to scoop into the bag.
    {
        let client = server.clients.get_mut(&target).unwrap();
        client.inventory.inventory_slots[0] = Some(ItemStack::new(WOOD_ID, 50));
        client.controller.health = 1.0; // one swing kills.
    }

    let envelopes = attack(&mut server, attacker, target);

    let dead = matches!(
        server.clients.get(&target).map(|c| c.lifecycle),
        Some(crate::server::PlayerLifecycle::Dead { .. })
    );
    assert!(dead, "target should be flagged Dead after fatal hit");
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::PlayerKilled { .. })),
        "PlayerKilled envelope should be emitted"
    );
    // Death should produce one loot bag containing the victim's
    // wood pile, not a scattered set of `DroppedWorldItem`s.
    let bags: Vec<_> = server.loot_bags_iter().collect();
    assert_eq!(bags.len(), 1, "death should spawn exactly one loot bag");
    let (_, bag) = bags[0];
    assert!(
        bag.slots.iter().any(
            |slot| matches!(slot, Some(s) if s.item_id.as_ref() == WOOD_ID && s.quantity == 50)
        ),
        "loot bag should hold the victim's wood pile"
    );
    // Victim's inventory + actionbar must be empty post-death.
    let client = server.clients.get(&target).unwrap();
    assert!(client.inventory.inventory_slots.iter().all(Option::is_none));
    assert!(client.inventory.actionbar_slots.iter().all(Option::is_none));
}

#[test]
fn dead_player_cannot_be_attacked_again() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Victim");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);
    server.clients.get_mut(&target).unwrap().controller.health = 1.0;

    // First swing kills.
    attack(&mut server, attacker, target);
    let envelopes_after_death = attack(&mut server, attacker, target);
    assert!(
        envelopes_after_death.is_empty(),
        "second swing on a corpse must be silently dropped"
    );
}

#[test]
fn respawn_command_resets_health_and_moves_player() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Victim");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);
    server.clients.get_mut(&target).unwrap().controller.health = 1.0;

    attack(&mut server, attacker, target);
    let death_pos = server.clients[&target].controller.position;

    let respawn_envelopes = server.apply_respawn_command(target);

    let client = server.clients.get(&target).unwrap();
    assert!(client.lifecycle.is_alive(), "respawn should flip to Alive");
    assert_eq!(client.controller.health, MAX_HEALTH);
    assert!(
        client.controller.position.x != death_pos.x || client.controller.position.z != death_pos.z,
        "respawn should move the player off the death spot"
    );
    assert!(
        respawn_envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::Correction(_))),
        "Correction message should be sent so client predictor snaps"
    );
}

#[test]
fn respawn_command_rejected_when_alive() {
    let mut server = server();
    let client = connect_named(&mut server, 1, "Alpha");
    let envelopes = server.apply_respawn_command(client);
    assert!(
        envelopes.is_empty(),
        "respawn on a live player must be a no-op"
    );
}

#[test]
fn dead_player_movement_updates_are_ignored() {
    use crate::protocol::ClientMessage;
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Victim");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);
    server.clients.get_mut(&target).unwrap().controller.health = 1.0;
    attack(&mut server, attacker, target);
    let death_pos = server.clients[&target].controller.position;

    server.receive(
        target,
        ClientMessage::Movement(PlayerMovement {
            sequence: 999,
            position: Vec3Net::new(100.0, 0.0, 100.0),
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            grounded: true,
        }),
    );

    let after = server.clients[&target].controller.position;
    assert_eq!(
        after, death_pos,
        "dead player must not be able to move via Movement messages"
    );
}

#[test]
fn attack_damage_clamps_at_zero_health() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);
    // 5 HP left, pickaxe deals 15 → must clamp at 0, no negative HP.
    server.clients.get_mut(&target).unwrap().controller.health = 5.0;

    attack(&mut server, attacker, target);
    assert_eq!(target_health(&server, target), 0.0);
    let _ = MAX_HEALTH;
}
