//! Server-authoritative PvP combat tests.
//!
//! Each test forges a malicious or boundary `AttackPlayerCommand` and
//! verifies the server's response, rejection without HP change for
//! anti-cheat cases, the expected damage/knockback/peer broadcast for
//! the happy path.

use super::*;
use crate::{
    game_balance::{STONE_HATCHET_PVP_DAMAGE, STONE_PICKAXE_PVP_DAMAGE},
    items::{BASIC_HATCHET_ID, BASIC_PICKAXE_ID, IRON_MACE_ID, ItemModel, WOODEN_CLUB_ID},
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

/// Equip an arbitrary weapon (or any item) into the active actionbar slot, so a
/// test can put a spear / sword / mace in hand.
fn equip_item(server: &mut GameServer, client_id: ClientId, item_id: &str) {
    let client = server.clients.get_mut(&client_id).expect("client exists");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(item_id, 1));
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

/// Extract the `model` carried by the single `PlayerImpact` in an attack's
/// envelope set, so a test can assert the peer-visible impact identity.
fn player_impact_model_of(envelopes: &[ServerEnvelope]) -> ItemModel {
    envelopes
        .iter()
        .find_map(|e| match &e.message {
            ServerMessage::PlayerImpact { model, .. } => Some(*model),
            _ => None,
        })
        .expect("PlayerImpact envelope present")
}

#[test]
fn weapon_hit_broadcasts_its_own_item_model_not_hands() {
    // The wire impact identity is now `ItemModel`: a weapon hit must broadcast the
    // weapon's OWN archetype on `PlayerImpact`, so a peer's audio/VFX/camera
    // reaction reads as that weapon rather than the retired `Hands` interim.
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);

    // A club hit broadcasts the Club model.
    equip_item(&mut server, attacker, WOODEN_CLUB_ID);
    let club_impact = attack(&mut server, attacker, target);
    assert_eq!(
        player_impact_model_of(&club_impact),
        ItemModel::Club,
        "a club hit must ship the Club impact identity, not Bag/Hands"
    );

    // A mace hit broadcasts the Mace model.
    reset_attack_cooldown(&mut server, attacker);
    equip_item(&mut server, attacker, IRON_MACE_ID);
    let mace_impact = attack(&mut server, attacker, target);
    assert_eq!(
        player_impact_model_of(&mace_impact),
        ItemModel::Mace,
        "a mace hit must ship the Mace impact identity"
    );
}

#[test]
fn tool_hit_broadcasts_its_archetype_model() {
    // Anchor for the re-key: a gather-tool hit still broadcasts its archetype (a
    // hatchet reads as Hatchet, a pickaxe as Pickaxe), never a weapon model or
    // the empty-hand bag.
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);

    equip_axe(&mut server, attacker);
    assert_eq!(
        player_impact_model_of(&attack(&mut server, attacker, target)),
        ItemModel::Hatchet,
        "a hatchet hit ships the Hatchet impact identity"
    );

    reset_attack_cooldown(&mut server, attacker);
    equip_pickaxe(&mut server, attacker);
    assert_eq!(
        player_impact_model_of(&attack(&mut server, attacker, target)),
        ItemModel::Pickaxe,
        "a pickaxe hit ships the Pickaxe impact identity"
    );
}

/// Extract the single Knockback impulse from an attack's envelope set.
fn knockback_impulse_of(envelopes: &[ServerEnvelope]) -> Vec3Net {
    envelopes
        .iter()
        .find_map(|e| match &e.message {
            ServerMessage::Knockback { impulse } => Some(*impulse),
            _ => None,
        })
        .expect("Knockback envelope present")
}

/// Clear the attacker's per-swing cooldown so the next `attack()` isn't dropped
/// as rate-limited. The test server's tick never advances, so back-to-back swings
/// would otherwise all hit the `next_attack_tick` gate.
fn reset_attack_cooldown(server: &mut GameServer, attacker: ClientId) {
    if let Some(client) = server.clients.get_mut(&attacker) {
        client.next_attack_tick = 0;
    }
}

#[test]
fn knockback_scale_command_scales_the_impulse() {
    // The `/knockback-scale` admin command must multiply the knockback impulse a
    // PvP hit sends. A fresh server is neutral (1.0), so a 2.0 factor doubles the
    // impulse vector component-for-component, and resetting to 1.0 restores it.
    let mut server = server();
    // Account id 1 is the singleplayer host, so this attacker is admin.
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);

    // Neutral baseline.
    let base = knockback_impulse_of(&attack(&mut server, attacker, target));
    assert!(base.z.abs() > 0.0, "baseline knockback should be non-zero");

    // Double the knockback and swing again; each component scales by 2x.
    let out = server.apply_command(attacker, "/knockback-scale 2".to_owned());
    assert!(
        out.iter()
            .any(|e| matches!(&e.message, ServerMessage::Toast(t)
                if matches!(t.kind, crate::protocol::ToastKind::Success))),
        "a successful /knockback-scale should reply with a success toast",
    );
    reset_attack_cooldown(&mut server, attacker);
    let doubled = knockback_impulse_of(&attack(&mut server, attacker, target));
    assert!((doubled.x - base.x * 2.0).abs() < 1e-4);
    assert!((doubled.y - base.y * 2.0).abs() < 1e-4);
    assert!((doubled.z - base.z * 2.0).abs() < 1e-4);

    // Reset to neutral restores the original impulse.
    server.apply_command(attacker, "/knockback-scale 1".to_owned());
    reset_attack_cooldown(&mut server, attacker);
    let restored = knockback_impulse_of(&attack(&mut server, attacker, target));
    assert!((restored.x - base.x).abs() < 1e-4);
    assert!((restored.z - base.z).abs() < 1e-4);
}

#[test]
fn knockback_scale_zero_removes_the_impulse() {
    // A 0.0 factor zeroes the knockback, still emitting the envelope but with a
    // null impulse, so tuning can bracket the shipped feel from zero upward.
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);

    server.apply_command(attacker, "/knockback-scale 0".to_owned());
    let impulse = knockback_impulse_of(&attack(&mut server, attacker, target));
    assert_eq!(impulse.x, 0.0);
    assert_eq!(impulse.y, 0.0);
    assert_eq!(impulse.z, 0.0);
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
    // 50% melee mitigation. A hatchet is Blunt, so the melee column is what the
    // hit reads. Set it directly (rather than equipping a set) to keep the test
    // focused on the mitigation math.
    server.clients.get_mut(&target).unwrap().protection.melee = 50;

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

// ---- Equipment mitigation, durability wear, and loot-bag drain ----

use crate::items::{PADDED_HOOD_ID, PADDED_LEGGINGS_ID, PADDED_TUNIC_ID, PADDED_WRAPS_ID};
use crate::protocol::{EquipmentSlot, InventoryCommand, ItemContainerSlot};

/// Equip a padded piece via the real inventory command path so the server
/// recomputes mitigation exactly as it would in-game. Places the piece in the
/// first bag slot, then issues a Move into the matching equipment slot.
fn equip_padded(server: &mut GameServer, client_id: ClientId, item_id: &str, slot: EquipmentSlot) {
    {
        let client = server.clients.get_mut(&client_id).expect("client exists");
        client.inventory.inventory_slots[0] = Some(ItemStack::new(item_id, 1));
    }
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot::inventory(0),
            to: ItemContainerSlot::equipment(slot),
            quantity: None,
            seq: 0,
        }),
    );
}

#[test]
fn equipping_a_padded_set_reduces_melee_damage() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);

    // A full padded set = 12% melee mitigation. A stone pickaxe deals
    // STONE_PICKAXE_PVP_DAMAGE (15); 15 * (100-12)/100 = 13 (integer floor).
    equip_padded(&mut server, target, PADDED_HOOD_ID, EquipmentSlot::Head);
    equip_padded(&mut server, target, PADDED_TUNIC_ID, EquipmentSlot::Chest);
    equip_padded(&mut server, target, PADDED_LEGGINGS_ID, EquipmentSlot::Legs);
    equip_padded(&mut server, target, PADDED_WRAPS_ID, EquipmentSlot::Feet);

    // The recompute fed the replicated melee armor too.
    assert_eq!(server.clients[&target].protection.melee, 12);

    let start = target_health(&server, target);
    attack(&mut server, attacker, target);
    let damage = start - target_health(&server, target);
    let expected = (STONE_PICKAXE_PVP_DAMAGE * (100 - 12) / 100) as f32;
    assert_eq!(
        damage, expected,
        "12% melee mitigation should floor 15 to 13"
    );
}

#[test]
fn a_melee_hit_wears_only_the_pieces_that_stopped_it() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_axe(&mut server, attacker);

    // Wraps protect against melee (1%), so they wear on a Blunt hit.
    equip_padded(&mut server, target, PADDED_WRAPS_ID, EquipmentSlot::Feet);
    let full = crate::game_balance::PADDED_ARMOR_DURABILITY;

    attack(&mut server, attacker, target);

    let worn = server.clients[&target].inventory.equipment_slots[EquipmentSlot::Feet.index()]
        .as_ref()
        .and_then(|stack| stack.durability);
    assert_eq!(
        worn,
        Some(full - 1),
        "the melee-stopping piece should wear by 1"
    );
}

#[test]
fn a_broken_piece_stays_worn_but_stops_protecting() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);

    // A full padded set, then break the tunic (the biggest melee contributor).
    equip_padded(&mut server, target, PADDED_HOOD_ID, EquipmentSlot::Head);
    equip_padded(&mut server, target, PADDED_TUNIC_ID, EquipmentSlot::Chest);
    equip_padded(&mut server, target, PADDED_LEGGINGS_ID, EquipmentSlot::Legs);
    equip_padded(&mut server, target, PADDED_WRAPS_ID, EquipmentSlot::Feet);
    {
        let stack = server
            .clients
            .get_mut(&target)
            .unwrap()
            .inventory
            .equipment_slots[EquipmentSlot::Chest.index()]
        .as_mut()
        .unwrap();
        stack.durability = Some(0);
    }
    // Recompute mitigation now that the chest is broken: it must drop the tunic's
    // 5% melee, leaving 7% (hood 3 + legs 3 + wraps 1). The broken chest stays in
    // the slot.
    server.recompute_protection(target);
    assert_eq!(server.clients[&target].protection.melee, 12 - 5);
    assert!(
        server.clients[&target].inventory.equipment_slots[EquipmentSlot::Chest.index()].is_some(),
        "a broken piece stays equipped"
    );
}

#[test]
fn death_drains_worn_armor_into_the_loot_bag_and_zeroes_mitigation() {
    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Victim");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_pickaxe(&mut server, attacker);
    equip_padded(&mut server, target, PADDED_HOOD_ID, EquipmentSlot::Head);
    server.clients.get_mut(&target).unwrap().controller.health = 1.0; // one swing kills.

    attack(&mut server, attacker, target);

    // The worn hood dropped into the single death bag alongside anything else.
    let bags: Vec<_> = server.loot_bags_iter().collect();
    assert_eq!(bags.len(), 1, "death should spawn exactly one loot bag");
    let (_, bag) = bags[0];
    assert!(
        bag.slots
            .iter()
            .any(|slot| matches!(slot, Some(s) if s.item_id.as_ref() == PADDED_HOOD_ID)),
        "loot bag should hold the victim's worn hood"
    );
    // The paperdoll is empty and mitigation is back to zero.
    let client = server.clients.get(&target).unwrap();
    assert!(client.inventory.equipment_slots.iter().all(Option::is_none));
    assert_eq!(client.protection, crate::items::ArmorProtection::default());
}

#[test]
fn pierce_interaction_is_unchanged_by_the_equipment_rework() {
    // P1c's pierce-then-mitigation math still holds against a worn set: a swing
    // whose attack profile pierces some armor shaves the effective melee value
    // first. Assert the end-to-end shape via the pure combat helpers against the
    // protection the equipment path produces.
    use crate::combat::{damage_after_armor, effective_armor_after_pierce};

    let mut server = server();
    let target = connect_named(&mut server, 2, "Target");
    equip_padded(&mut server, target, PADDED_HOOD_ID, EquipmentSlot::Head);
    equip_padded(&mut server, target, PADDED_TUNIC_ID, EquipmentSlot::Chest);
    equip_padded(&mut server, target, PADDED_LEGGINGS_ID, EquipmentSlot::Legs);
    equip_padded(&mut server, target, PADDED_WRAPS_ID, EquipmentSlot::Feet);
    let melee = server.clients[&target].protection.melee; // 12

    // No pierce: 100 raw through 12 armor = 88.
    assert_eq!(
        damage_after_armor(100, effective_armor_after_pierce(melee, 0)),
        88
    );
    // 50% pierce halves the effective armor to 6, letting 94 through.
    assert_eq!(
        damage_after_armor(100, effective_armor_after_pierce(melee, 50)),
        94
    );
}

// ---- melee weapons ----

use crate::items::{IRON_SWORD_ID, STONE_SPEAR_ID};

#[test]
fn a_mace_hit_pierces_armor_against_an_armored_target() {
    // The mace is the anti-armor weapon: its 50% pierce shaves the target's
    // effective armor before mitigation, so an armored victim takes more from a
    // mace than the same armor would leave through a non-piercing weapon.
    use crate::combat::{damage_after_armor, effective_armor_after_pierce};
    use crate::game_balance::{IRON_MACE_ARMOR_PIERCE_PCT, IRON_MACE_PVP_DAMAGE};

    let mut server = server();
    let attacker = connect_named(&mut server, 1, "Attacker");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, attacker, Vec3Net::new(0.0, 0.0, 0.0), 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -2.0), 0.0);
    equip_item(&mut server, attacker, IRON_MACE_ID);
    // 40% melee mitigation. A mace is Blunt, so the melee column is what the hit
    // reads. Set it directly to keep the test on the pierce math.
    server.clients.get_mut(&target).unwrap().protection.melee = 40;

    let start = target_health(&server, target);
    attack(&mut server, attacker, target);
    let dealt = start - target_health(&server, target);

    // Expected: pierce halves the effective armor (40 -> 20), then mitigation.
    let effective = effective_armor_after_pierce(40, IRON_MACE_ARMOR_PIERCE_PCT);
    assert_eq!(effective, 20, "50% pierce halves 40 armor to 20");
    let expected = damage_after_armor(IRON_MACE_PVP_DAMAGE, effective) as f32;
    assert_eq!(dealt, expected, "the mace pierces before mitigation");

    // And it beats what the same armor would leave through with no pierce, the
    // whole point of the anti-armor weapon.
    let without_pierce = damage_after_armor(IRON_MACE_PVP_DAMAGE, 40) as f32;
    assert!(
        dealt > without_pierce,
        "pierce lets more damage through than the raw armor would"
    );
}

#[test]
fn a_spear_connects_where_a_sword_does_not_at_extended_range() {
    // The spear's whole identity is reach: it validates a hit at a distance the
    // sword (standard 3.5 m reach) rejects. Server range is feet-to-feet, so a
    // target at 4.2 m is inside the spear's 4.5 m reach but past the sword's.
    let target_z = -4.2;

    // Sword at 4.2 m: rejected (out of its 3.5 m reach), no damage.
    let mut sword_server = server();
    let sword_attacker = connect_named(&mut sword_server, 1, "Swordsman");
    let sword_target = connect_named(&mut sword_server, 2, "Target");
    place_player(
        &mut sword_server,
        sword_attacker,
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
    );
    place_player(
        &mut sword_server,
        sword_target,
        Vec3Net::new(0.0, 0.0, target_z),
        0.0,
    );
    equip_item(&mut sword_server, sword_attacker, IRON_SWORD_ID);
    let sword_start = target_health(&sword_server, sword_target);
    let sword_envelopes = attack(&mut sword_server, sword_attacker, sword_target);
    assert_eq!(
        sword_start,
        target_health(&sword_server, sword_target),
        "the sword cannot reach a target at 4.2 m"
    );
    assert!(
        sword_envelopes.is_empty(),
        "an out-of-reach sword swing emits nothing"
    );

    // Spear at the same 4.2 m: connects (inside its 4.5 m reach).
    let mut spear_server = server();
    let spear_attacker = connect_named(&mut spear_server, 1, "Spearman");
    let spear_target = connect_named(&mut spear_server, 2, "Target");
    place_player(
        &mut spear_server,
        spear_attacker,
        Vec3Net::new(0.0, 0.0, 0.0),
        0.0,
    );
    place_player(
        &mut spear_server,
        spear_target,
        Vec3Net::new(0.0, 0.0, target_z),
        0.0,
    );
    equip_item(&mut spear_server, spear_attacker, STONE_SPEAR_ID);
    let spear_start = target_health(&spear_server, spear_target);
    let spear_envelopes = attack(&mut spear_server, spear_attacker, spear_target);
    assert!(
        target_health(&spear_server, spear_target) < spear_start,
        "the spear reaches a target at 4.2 m where the sword cannot"
    );
    assert!(
        spear_envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::PlayerImpact { .. })),
        "the spear hit reaches nearby peers"
    );
}
