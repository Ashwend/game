//! Server-authoritative projectile (bow / crossbow) tests.
//!
//! Covers the fire-validation matrix (no weapon, no ammo, cooldown, non-finite
//! aim), the draw-fraction damage lerp, the ballistic step, block-stops-flight,
//! the player-hit path through `apply_player_damage`, shooter self-hit exclusion,
//! exactly-one-arrow consumption, and the stuck-arrow TTL despawn. The
//! ballistic-math, recovery-determinism, and draw-lerp unit tests live next to
//! their owners (`server::projectiles`, `items::ranged`); these are the
//! command-and-simulation integration tests.

use super::*;
use crate::{
    game_balance::{
        CROSSBOW_COOLDOWN_TICKS, CROSSBOW_RELOAD_MOVE_MULTIPLIER, IMPACT_MESSAGE_RANGE_M,
        PROJECTILE_MAX_FLIGHT_SECONDS, PROJECTILE_SELF_HIT_GRACE_TICKS,
        PROJECTILE_STUCK_TTL_SECONDS, WOODEN_BOW_DAMAGE_MAX, WOODEN_BOW_DAMAGE_MIN,
        WOODEN_BOW_DRAW_TICKS,
    },
    items::{ARROW_ID, CROSSBOW_ID, WOODEN_BOW_ID},
    protocol::{
        AccountId, ClientMessage, ItemStack, PROTOCOL_VERSION, PlayerMovement, ProjectileSurface,
        RangedCommand,
    },
    server::DeliveryTarget,
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

/// Equip a ranged weapon into the active slot with `arrows` arrows in the bag.
fn equip_ranged(server: &mut GameServer, client_id: ClientId, weapon: &str, arrows: u16) {
    let client = server.clients.get_mut(&client_id).expect("client exists");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(weapon, 1));
    client.inventory.active_actionbar_slot = 0;
    if arrows > 0 {
        client.inventory.inventory_slots[0] = Some(ItemStack::new(ARROW_ID, arrows));
    }
}

fn arrow_count(server: &GameServer, client_id: ClientId) -> u32 {
    crate::inventory::count_items_in_inventory(
        &server.clients.get(&client_id).expect("client").inventory,
        ARROW_ID,
    )
}

/// Fire a committed, full-draw shot: a bow release only fires past the minimum
/// draw gate, so the helper starts the draw and holds it a full window before
/// releasing. A crossbow ignores the draw, so the same sequence works for both.
fn fire(server: &mut GameServer, client_id: ClientId, aim: Vec3Net) -> Vec<ServerEnvelope> {
    server.apply_ranged_command(client_id, RangedCommand::DrawStart);
    server.tick += WOODEN_BOW_DRAW_TICKS;
    server.apply_ranged_command(client_id, RangedCommand::Fire { aim_dir: aim })
}

/// Straight-ahead aim (-Z, level).
const AIM_FORWARD: Vec3Net = Vec3Net::new(0.0, 0.0, -1.0);

// ---- fire validation matrix ----

#[test]
fn fire_with_no_ranged_weapon_does_nothing() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    // A bare hand (no weapon, no ammo): the fire is rejected, no projectile.
    let envelopes = fire(&mut server, shooter, AIM_FORWARD);
    assert!(envelopes.is_empty());
    assert!(server.projectiles.is_empty(), "no projectile should spawn");
}

#[test]
fn fire_with_no_ammo_is_rejected() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 0);
    fire(&mut server, shooter, AIM_FORWARD);
    assert!(
        server.projectiles.is_empty(),
        "a bow with no arrows can't fire"
    );
}

#[test]
fn fire_consumes_exactly_one_arrow_and_spawns_one_projectile() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    assert_eq!(arrow_count(&server, shooter), 5);
    fire(&mut server, shooter, AIM_FORWARD);
    assert_eq!(
        arrow_count(&server, shooter),
        4,
        "exactly one arrow is spent per shot"
    );
    assert_eq!(server.projectiles.len(), 1, "exactly one projectile spawns");
}

#[test]
fn fire_on_cooldown_is_rejected() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    // The crossbow has a long reload; a second immediate shot must be dropped.
    equip_ranged(&mut server, shooter, CROSSBOW_ID, 5);
    fire(&mut server, shooter, AIM_FORWARD);
    assert_eq!(server.projectiles.len(), 1);
    // Second shot while the reload cooldown is active: no new projectile, no
    // extra arrow spent.
    fire(&mut server, shooter, AIM_FORWARD);
    assert_eq!(
        server.projectiles.len(),
        1,
        "the crossbow can't fire again until the reload elapses"
    );
    assert_eq!(arrow_count(&server, shooter), 4, "only one arrow was spent");

    // Advance past the reload window; now a shot is accepted.
    server.clients.get_mut(&shooter).unwrap().next_ranged_tick = 0;
    server.tick = CROSSBOW_COOLDOWN_TICKS + 1;
    fire(&mut server, shooter, AIM_FORWARD);
    assert_eq!(
        server.projectiles.len(),
        2,
        "after the reload elapses the crossbow fires again"
    );
}

#[test]
fn fire_with_non_finite_aim_is_rejected() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    // A NaN aim direction is dropped before any ammo is touched.
    fire(&mut server, shooter, Vec3Net::new(f32::NAN, 0.0, -1.0));
    assert!(server.projectiles.is_empty(), "a non-finite aim can't fire");
    assert_eq!(
        arrow_count(&server, shooter),
        5,
        "a rejected shot spends no arrow"
    );
    // A zero aim vector is likewise rejected.
    fire(&mut server, shooter, Vec3Net::ZERO);
    assert!(server.projectiles.is_empty());
    assert_eq!(arrow_count(&server, shooter), 5);
}

// ---- draw-fraction damage ----

#[test]
fn a_release_below_the_minimum_draw_is_rejected() {
    // A bow release only fires past the minimum draw gate: an undrawn Fire (a
    // tap) and a barely-held one are both dropped, costing nothing.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    // No DrawStart at all: rejected, no arrow spent.
    server.apply_ranged_command(
        shooter,
        RangedCommand::Fire {
            aim_dir: AIM_FORWARD,
        },
    );
    assert!(server.projectiles.is_empty(), "a tap-fire never looses");
    assert_eq!(
        arrow_count(&server, shooter),
        5,
        "the abandoned tap is free"
    );

    // A held draw still under the minimum firing fraction: also rejected.
    server.apply_ranged_command(shooter, RangedCommand::DrawStart);
    server.tick += 2;
    server.apply_ranged_command(
        shooter,
        RangedCommand::Fire {
            aim_dir: AIM_FORWARD,
        },
    );
    assert!(
        server.projectiles.is_empty(),
        "a release below the minimum draw is a cancel, not a shot"
    );
    assert_eq!(arrow_count(&server, shooter), 5);
}

#[test]
fn damage_and_speed_scale_with_the_held_draw() {
    // A shot just past the minimum draw carries scaled-down damage AND a
    // scaled-down launch speed; a full draw carries the maximums.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    // Just past the minimum firing draw.
    let past_min = (crate::game_balance::BOW_MIN_DRAW_FRACTION_TO_FIRE
        * WOODEN_BOW_DRAW_TICKS as f32)
        .ceil() as u64;
    server.apply_ranged_command(shooter, RangedCommand::DrawStart);
    server.tick += past_min;
    server.apply_ranged_command(
        shooter,
        RangedCommand::Fire {
            aim_dir: AIM_FORWARD,
        },
    );
    let weak = *server
        .projectiles
        .values()
        .next()
        .expect("weak shot spawned");
    assert!(
        weak.damage > WOODEN_BOW_DAMAGE_MIN && weak.damage < WOODEN_BOW_DAMAGE_MAX,
        "a short-hold shot lands inside the damage band, got {}",
        weak.damage
    );

    // Reset for the full-draw shot.
    server.projectiles = Default::default();
    server.clients.get_mut(&shooter).unwrap().next_ranged_tick = 0;
    fire(&mut server, shooter, AIM_FORWARD);
    let full = *server
        .projectiles
        .values()
        .next()
        .expect("full shot spawned");
    assert_eq!(full.damage, WOODEN_BOW_DAMAGE_MAX);
    let weak_speed = weak.velocity.length_squared().sqrt();
    let full_speed = full.velocity.length_squared().sqrt();
    assert!(
        weak_speed < full_speed * 0.75,
        "a short-hold shot launches clearly slower: {weak_speed} vs {full_speed}"
    );
    assert!(
        (full_speed - crate::game_balance::WOODEN_BOW_PROJECTILE_SPEED_MPS).abs() < 1e-3,
        "a full draw launches at full speed"
    );
}

#[test]
fn crossbow_shot_is_always_full_damage() {
    // A crossbow has a zero draw window, so a shot with no held draw is still the
    // flat crossbow damage.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, CROSSBOW_ID, 5);
    fire(&mut server, shooter, AIM_FORWARD);
    let damage = server
        .projectiles
        .values()
        .next()
        .expect("projectile spawned")
        .damage;
    assert_eq!(damage, crate::game_balance::CROSSBOW_DAMAGE);
}

// ---- simulation: player hit, self-hit exclusion, block stops flight ----

#[test]
fn projectile_hits_a_player_and_routes_through_apply_player_damage() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    let target = connect_named(&mut server, 2, "Target");
    // Shooter at origin firing -Z; target 3 m ahead in the arrow's path.
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -3.0), 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    let start_hp = server.clients.get(&target).unwrap().controller.health;
    // Aim straight at the target's chest from the shooter's eye.
    let envelopes = fire(&mut server, shooter, AIM_FORWARD);
    // No hit yet (the projectile just spawned); step the sim until it reaches the
    // target.
    let _ = envelopes;
    let mut hit_envelopes = Vec::new();
    for _ in 0..20 {
        hit_envelopes.extend(server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ));
        server.tick += 1;
        if server.clients.get(&target).unwrap().controller.health < start_hp {
            break;
        }
    }
    let new_hp = server.clients.get(&target).unwrap().controller.health;
    assert!(new_hp < start_hp, "the arrow should damage the target");
    // The player-damage tail sends the victim a Correction and a Knockback.
    assert!(
        hit_envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::Correction(_))),
        "a projectile player-hit must send the victim a Correction"
    );
    assert!(
        hit_envelopes
            .iter()
            .any(|e| matches!(&e.message, ServerMessage::Knockback { .. })),
        "a projectile player-hit must send the victim a Knockback"
    );
    // The projectile is spent after the hit.
    assert!(
        server.projectiles.is_empty(),
        "the arrow is consumed on a player hit"
    );
}

/// Count `ProjectileImpact` envelopes addressed to `client`, split into
/// `(owner_confirmations, plain)` so a test can assert the shooter gets exactly
/// one confirmation and a peer gets only the plain fan-out copy.
fn projectile_impacts_for(envelopes: &[ServerEnvelope], client: ClientId) -> (usize, usize) {
    let mut confirmations = 0;
    let mut plain = 0;
    for envelope in envelopes {
        if envelope.target != DeliveryTarget::Client(client) {
            continue;
        }
        if let ServerMessage::ProjectileImpact {
            owner_confirmation, ..
        } = &envelope.message
        {
            if *owner_confirmation {
                confirmations += 1;
            } else {
                plain += 1;
            }
        }
    }
    (confirmations, plain)
}

/// Fire straight ahead and step the sim until the arrow resolves a hit,
/// returning every envelope the hitting tick produced.
fn fire_and_run_to_hit(
    server: &mut GameServer,
    shooter: ClientId,
    target: ClientId,
) -> Vec<ServerEnvelope> {
    let start_hp = server.clients.get(&target).unwrap().controller.health;
    fire(server, shooter, AIM_FORWARD);
    let mut envelopes = Vec::new();
    for _ in 0..20 {
        envelopes.extend(server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ));
        server.tick += 1;
        if server.clients.get(&target).unwrap().controller.health < start_hp {
            break;
        }
    }
    envelopes
}

#[test]
fn shooter_gets_exactly_one_confirmation_on_a_player_hit_with_a_nearby_peer() {
    // A peer standing inside the fan-out radius gets one plain ProjectileImpact;
    // the shooter gets exactly one owner-confirmation copy (and is absent from
    // the peer fan-out, so no double-delivery).
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    let target = connect_named(&mut server, 2, "Target");
    let peer = connect_named(&mut server, 3, "Peer");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -3.0), 0.0);
    // Peer well within the 80 m fan-out radius, off to the side so it isn't in
    // the arrow's path.
    place_player(&mut server, peer, Vec3Net::new(5.0, 0.0, -3.0), 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    let envelopes = fire_and_run_to_hit(&mut server, shooter, target);

    let (shooter_confirms, shooter_plain) = projectile_impacts_for(&envelopes, shooter);
    assert_eq!(
        shooter_confirms, 1,
        "the shooter gets exactly one owner-confirmation ProjectileImpact"
    );
    assert_eq!(
        shooter_plain, 0,
        "the shooter must never receive the plain peer fan-out copy (no double-delivery)"
    );

    let (peer_confirms, peer_plain) = projectile_impacts_for(&envelopes, peer);
    assert_eq!(
        peer_plain, 1,
        "a nearby peer gets exactly one plain ProjectileImpact"
    );
    assert_eq!(
        peer_confirms, 0,
        "a peer never receives an owner-confirmation copy"
    );
}

#[test]
fn shooter_still_gets_the_confirmation_when_no_peer_is_in_range() {
    // Even with the only other player parked well beyond the fan-out radius (so
    // the proximity fan-out is empty), the shooter still receives exactly one
    // owner-confirmation copy: the owner send is independent of the fan-out.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    let target = connect_named(&mut server, 2, "Target");
    let peer = connect_named(&mut server, 3, "Peer");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -3.0), 0.0);
    // Peer parked beyond the 80 m impact-message range: excluded from the fan-out.
    let far = IMPACT_MESSAGE_RANGE_M + 50.0;
    place_player(&mut server, peer, Vec3Net::new(far, 0.0, 0.0), 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    let envelopes = fire_and_run_to_hit(&mut server, shooter, target);

    let (shooter_confirms, shooter_plain) = projectile_impacts_for(&envelopes, shooter);
    assert_eq!(
        shooter_confirms, 1,
        "the shooter's confirmation does not depend on any peer being in range"
    );
    assert_eq!(shooter_plain, 0, "still no plain copy to the shooter");

    let (_, peer_plain) = projectile_impacts_for(&envelopes, peer);
    assert_eq!(
        peer_plain, 0,
        "an out-of-range peer gets no ProjectileImpact"
    );
}

#[test]
fn world_rest_sends_the_shooter_no_projectile_impact() {
    // A pure world rest (arrow into open sky, coming down to terrain) must not
    // send the shooter any ProjectileImpact: their client cues the world thunk
    // from the arrow's moving -> stuck transition instead.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    // Lob it nearly straight up so it arcs back down to a world rest.
    fire(&mut server, shooter, Vec3Net::new(0.0, 1.0, 0.05));
    let mut envelopes = Vec::new();
    let cap = ((PROJECTILE_MAX_FLIGHT_SECONDS + PROJECTILE_STUCK_TTL_SECONDS) * SERVER_TICK_RATE_HZ)
        as u64
        + 5;
    for _ in 0..cap {
        envelopes.extend(server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ));
        server.tick += 1;
    }
    let (confirms, plain) = projectile_impacts_for(&envelopes, shooter);
    assert_eq!(
        confirms, 0,
        "a world rest never sends the shooter an owner-confirmation copy"
    );
    assert_eq!(
        plain, 0,
        "the shooter is excluded from any world-rest fan-out"
    );
    // Sanity: any ProjectileImpact that did fire this run was a World surface.
    for envelope in &envelopes {
        if let ServerMessage::ProjectileImpact { surface, .. } = &envelope.message {
            assert_eq!(
                *surface,
                ProjectileSurface::World,
                "the lobbed shot only ever resolves a world rest"
            );
        }
    }
}

#[test]
fn armored_target_takes_projectile_column_mitigated_damage() {
    // A target wearing projectile armor takes less than the raw shot damage,
    // proving the projectile column of the armor table is applied.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -3.0), 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    // Equip a full padded set on the target and recompute mitigation.
    {
        let client = server.clients.get_mut(&target).unwrap();
        client.inventory.equipment_slots[0] = Some(ItemStack::new(crate::items::PADDED_HOOD_ID, 1));
        client.inventory.equipment_slots[1] =
            Some(ItemStack::new(crate::items::PADDED_TUNIC_ID, 1));
        client.inventory.equipment_slots[2] =
            Some(ItemStack::new(crate::items::PADDED_LEGGINGS_ID, 1));
        client.inventory.equipment_slots[3] =
            Some(ItemStack::new(crate::items::PADDED_WRAPS_ID, 1));
    }
    server.recompute_protection(target);
    let projectile_armor = server
        .clients
        .get(&target)
        .unwrap()
        .protection
        .for_kind(crate::combat::DamageKind::Projectile);
    assert!(projectile_armor > 0, "the padded set stops some projectile");

    let start_hp = server.clients.get(&target).unwrap().controller.health;
    fire(&mut server, shooter, AIM_FORWARD);
    for _ in 0..20 {
        server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ);
        server.tick += 1;
        if server.clients.get(&target).unwrap().controller.health < start_hp {
            break;
        }
    }
    let taken = start_hp - server.clients.get(&target).unwrap().controller.health;
    // The helper's full-draw bow shot is WOODEN_BOW_DAMAGE_MAX raw; armor must
    // reduce it.
    assert!(
        taken < WOODEN_BOW_DAMAGE_MAX as f32,
        "projectile armor should mitigate the shot: took {taken}, raw {WOODEN_BOW_DAMAGE_MAX}"
    );
    assert!(taken > 0.0, "armor is capped, so some damage still lands");
}

#[test]
fn shooter_is_not_hit_by_its_own_arrow_at_spawn() {
    // The projectile spawns at the shooter's eye, inside their body box. The
    // self-hit grace window must keep it from resolving against the shooter on
    // the first frames.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    let start_hp = server.clients.get(&shooter).unwrap().controller.health;
    // Fire straight up so the arrow lingers near the shooter for several ticks.
    fire(&mut server, shooter, Vec3Net::new(0.0, 1.0, 0.0));
    for _ in 0..(PROJECTILE_SELF_HIT_GRACE_TICKS as usize) {
        server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ);
        server.tick += 1;
    }
    assert_eq!(
        server.clients.get(&shooter).unwrap().controller.health,
        start_hp,
        "the shooter must not be hit by their own arrow during the grace window"
    );
}

#[test]
fn a_wall_between_shooter_and_target_stops_the_arrow() {
    // A stone foundation wall between the shooter and target blocks the shot, so
    // the target takes no damage and the arrow rests instead.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    let target = connect_named(&mut server, 2, "Target");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    place_player(&mut server, target, Vec3Net::new(0.0, 0.0, -6.0), 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    // A vertical wall 3 m ahead, between shooter and target. A wall at yaw 0
    // spans local X (3 m wide) and is thin in Z, so its collider stands squarely
    // across the arrow's -Z path at eye height.
    crate::server::test_support::place_building(
        &mut server,
        crate::building::BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.0, -3.0),
        0.0,
    );

    let start_hp = server.clients.get(&target).unwrap().controller.health;
    fire(&mut server, shooter, AIM_FORWARD);
    for _ in 0..30 {
        server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ);
        server.tick += 1;
    }
    assert_eq!(
        server.clients.get(&target).unwrap().controller.health,
        start_hp,
        "a wall between shooter and target must stop the arrow"
    );
}

// ---- stuck-arrow TTL ----

#[test]
fn a_projectile_that_rests_despawns_after_the_stuck_ttl() {
    // Fire into open sky with a short-lived shot: after it comes to rest (max
    // flight or a world hit) and the stuck TTL elapses, the projectile is gone.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    // Fire nearly straight up so the arc is long; the max-flight cap or a ground
    // rest eventually removes it, but we drive it deterministically by ticking
    // well past the flight cap plus the stuck TTL.
    fire(&mut server, shooter, Vec3Net::new(0.0, 1.0, 0.05));
    let total_ticks = ((PROJECTILE_MAX_FLIGHT_SECONDS + PROJECTILE_STUCK_TTL_SECONDS)
        * SERVER_TICK_RATE_HZ) as u64
        + 5;
    for _ in 0..total_ticks {
        server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ);
        server.tick += 1;
    }
    assert!(
        server.projectiles.is_empty(),
        "the projectile must despawn once its flight cap and stuck TTL elapse"
    );
}

// ---- stuck-arrow recovery (E pickup) ----

/// Fire one arrow and rest the shooter's single live projectile at `rest`,
/// returning its id. Every world rest sticks (E-recoverable until the TTL).
fn fire_and_rest(server: &mut GameServer, shooter: ClientId, rest: Vec3Net) -> u64 {
    server.clients.get_mut(&shooter).unwrap().next_ranged_tick = 0;
    fire(server, shooter, AIM_FORWARD);
    let (id, projectile) = server
        .projectiles
        .iter()
        .map(|(id, p)| (*id, *p))
        .next()
        .expect("one live projectile");
    server.rest_projectile_in_world(projectile, rest);
    id
}

#[test]
fn every_world_rest_sticks_with_the_flight_direction_and_no_dropped_item() {
    // A world rest always parks the arrow as a STUCK projectile: near-zero
    // speed with the final flight DIRECTION kept as an epsilon velocity (so
    // every client orients the shaft into the impact), and no separate
    // dropped-item entity (the old design's cosmetic-stuck-plus-hidden-drop
    // read as arrows vanishing or being un-pickupable).
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    let rest = Vec3Net::new(1.0, 0.0, -2.0);

    let id = fire_and_rest(&mut server, shooter, rest);

    assert!(
        server.stuck_projectiles.contains_key(&id),
        "every world rest sticks"
    );
    let parked = server.projectiles.get(&id).expect("stuck arrow kept");
    assert_eq!(
        parked.position, rest,
        "stuck arrow snapped to the rest point"
    );
    let speed = parked.velocity.length_squared().sqrt();
    assert!(
        speed > 0.0 && speed < 0.1,
        "rest keeps a tiny direction-only velocity, got {speed}"
    );
    assert!(
        parked.velocity.z < 0.0,
        "the epsilon velocity points along the shot's flight direction (-Z aim)"
    );
    assert!(
        server.dropped_items.is_empty(),
        "a world rest must not spawn a dropped item"
    );
}

#[test]
fn an_arrow_arcing_into_open_ground_lodges_at_the_surface() {
    // The flat world floor (y = 0) is a world solid for the sweep: an arrow
    // fired down at open ground must lodge at the surface exactly like a tree
    // hit (it used to sail straight through the floor and despawn unrecovered).
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    // Angled clearly downward at the open meadow a few metres ahead.
    fire(&mut server, shooter, Vec3Net::new(0.0, -0.5, -1.0));
    let id = *server
        .projectiles
        .keys()
        .next()
        .expect("projectile spawned");
    for _ in 0..40 {
        server.tick_projectiles(1.0 / SERVER_TICK_RATE_HZ);
        server.tick += 1;
        if server.stuck_projectiles.contains_key(&id) {
            break;
        }
    }

    assert!(
        server.stuck_projectiles.contains_key(&id),
        "the ground shot lodges instead of sailing through the floor"
    );
    let parked = server.projectiles.get(&id).expect("stuck arrow kept");
    assert!(
        parked.position.y.abs() < 0.05,
        "lodged right at the surface, got y {}",
        parked.position.y
    );
    assert!(
        parked.velocity.y < 0.0,
        "the epsilon rest direction points down into the ground"
    );
}

#[test]
fn a_stuck_arrow_is_recovered_with_e_within_reach() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 10);
    let rest = Vec3Net::new(1.0, 0.0, -2.0);
    let stuck_id = fire_and_rest(&mut server, shooter, rest);
    let before = arrow_count(&server, shooter);

    let envelopes = server.apply_recover_projectile(shooter, stuck_id);

    assert_eq!(
        arrow_count(&server, shooter),
        before + 1,
        "the recovered arrow lands back in the bag"
    );
    assert!(
        !server.projectiles.contains_key(&stuck_id),
        "the recovered projectile despawns"
    );
    assert!(
        !server.stuck_projectiles.contains_key(&stuck_id),
        "the stuck bookkeeping is cleared"
    );
    assert!(!envelopes.is_empty(), "the pickup toast is sent");
}

#[test]
fn recovery_is_rejected_out_of_reach_and_for_flying_arrows() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 10);

    // A stuck arrow far out of pickup reach: the recover is a silent no-op.
    let far_rest = Vec3Net::new(0.0, 0.0, -50.0);
    let far_id = fire_and_rest(&mut server, shooter, far_rest);
    let before = arrow_count(&server, shooter);
    server.apply_recover_projectile(shooter, far_id);
    assert_eq!(
        arrow_count(&server, shooter),
        before,
        "an out-of-reach stuck arrow is not granted"
    );
    assert!(
        server.projectiles.contains_key(&far_id),
        "the out-of-reach arrow stays stuck"
    );

    // An in-flight projectile (not in stuck_projectiles) can't be snatched.
    server.clients.get_mut(&shooter).unwrap().next_ranged_tick = 0;
    fire(&mut server, shooter, AIM_FORWARD);
    let flying_id = server
        .projectiles
        .keys()
        .copied()
        .find(|id| !server.stuck_projectiles.contains_key(id))
        .expect("a flying projectile");
    let before = arrow_count(&server, shooter);
    server.apply_recover_projectile(shooter, flying_id);
    assert_eq!(
        arrow_count(&server, shooter),
        before,
        "a flying arrow can't be recovered"
    );
}

// ---- crossbow reload movement slow ----

/// The crossbow's current move-speed multiplier + whether its reload slow is armed.
fn reload_move_state(server: &GameServer, client_id: ClientId) -> (f32, bool) {
    let client = server.clients.get(&client_id).expect("client exists");
    (client.run_speed_multiplier, client.reload_slow_active)
}

#[test]
fn crossbow_fire_slows_movement_and_the_tick_restores_it_when_the_reload_elapses() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, CROSSBOW_ID, 5);

    // Before firing: full speed, no reload slow.
    assert_eq!(reload_move_state(&server, shooter), (1.0, false));

    // Firing the crossbow arms the reload movement slow.
    fire(&mut server, shooter, AIM_FORWARD);
    let (mult, armed) = reload_move_state(&server, shooter);
    assert!(armed, "the crossbow reload slow is armed on fire");
    assert!(
        (mult - CROSSBOW_RELOAD_MOVE_MULTIPLIER).abs() < 1e-6,
        "movement is impaired to the reload multiplier while reloading, got {mult}"
    );

    // Partway through the reload the slow is still active: ticking short of the
    // reload window does not restore movement.
    let reload_tick = server.clients.get(&shooter).unwrap().next_ranged_tick;
    while server.tick + 1 < reload_tick {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
        let (mult, armed) = reload_move_state(&server, shooter);
        assert!(armed, "still reloading before the window elapses");
        assert!((mult - CROSSBOW_RELOAD_MOVE_MULTIPLIER).abs() < 1e-6);
    }

    // The tick that reaches the reload window restores full movement.
    server.tick(1.0 / SERVER_TICK_RATE_HZ);
    assert!(
        server.tick >= reload_tick,
        "advanced to the reload completion tick"
    );
    assert_eq!(
        reload_move_state(&server, shooter),
        (1.0, false),
        "movement is restored the moment the reload window elapses"
    );
}

#[test]
fn bow_fire_does_not_arm_the_reload_slow() {
    // A bow's tiny post-fire floor is not a reload: firing it leaves movement at
    // full speed (the draw slow, tested elsewhere, is the bow's only penalty).
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);
    fire(&mut server, shooter, AIM_FORWARD);
    assert_eq!(
        reload_move_state(&server, shooter),
        (1.0, false),
        "a bow shot never arms the reload movement slow"
    );
}

#[test]
fn swapping_off_a_reloading_crossbow_restores_movement() {
    // Switching to another actionbar slot mid-reload lifts the reload slow, so a
    // player is never stuck slow after putting the crossbow away.
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    place_player(&mut server, shooter, Vec3Net::ZERO, 0.0);
    equip_ranged(&mut server, shooter, CROSSBOW_ID, 5);
    // Give slot 1 a hatchet to swap to.
    server
        .clients
        .get_mut(&shooter)
        .unwrap()
        .inventory
        .actionbar_slots[1] = Some(ItemStack::new(crate::items::BASIC_HATCHET_ID, 1));

    fire(&mut server, shooter, AIM_FORWARD);
    assert!(reload_move_state(&server, shooter).1, "reload slow armed");

    server.apply_inventory_command(
        shooter,
        crate::protocol::InventoryCommand::SelectActionbarSlot { slot: 1 },
    );
    assert_eq!(
        reload_move_state(&server, shooter),
        (1.0, false),
        "swapping off the crossbow restores movement immediately"
    );
}

// ---- peer-visible bow draw (PlayerChargeFraction) ----

/// The peer-replicated draw fraction ramps 0 -> 1 while a bow is held drawn and
/// snaps back to 0 when the draw clears, so peers can animate the drawn bow.
#[test]
fn held_bow_draw_replicates_a_rising_fraction() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, WOODEN_BOW_ID, 5);

    let draw_of = |server: &GameServer| {
        server
            .players_iter()
            .find(|view| view.client_id == shooter)
            .expect("shooter view")
            .charge_fraction
            .0
    };

    // Not drawing: zero.
    assert_eq!(draw_of(&server), 0.0);

    // Half the draw window in: about half drawn.
    server.apply_ranged_command(shooter, RangedCommand::DrawStart);
    server.tick += WOODEN_BOW_DRAW_TICKS / 2;
    let half = draw_of(&server);
    assert!(
        (0.4..=0.6).contains(&half),
        "half-drawn fraction was {half}"
    );

    // Held past the full window: clamped to full draw.
    server.tick += WOODEN_BOW_DRAW_TICKS;
    assert_eq!(draw_of(&server), 1.0);

    // Cleared (cancel / release): back to rest.
    server.clear_ranged_draw(shooter);
    assert_eq!(draw_of(&server), 0.0);
}

/// A crossbow is instant-fire (no draw window), so its replicated draw fraction
/// stays 0 even mid-"draw": peers never see a crossbow hold a draw pose.
#[test]
fn crossbow_never_reports_a_draw_fraction() {
    let mut server = server();
    let shooter = connect_named(&mut server, 1, "Shooter");
    equip_ranged(&mut server, shooter, CROSSBOW_ID, 5);
    server.apply_ranged_command(shooter, RangedCommand::DrawStart);
    server.tick += 100;
    let draw = server
        .players_iter()
        .find(|view| view.client_id == shooter)
        .expect("shooter view")
        .charge_fraction
        .0;
    assert_eq!(
        draw, 0.0,
        "a crossbow has no draw window, so no peer draw pose"
    );
}
