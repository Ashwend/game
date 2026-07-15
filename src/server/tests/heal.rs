//! Server-authoritative consumable (bandage) tests.
//!
//! The security-critical property under test is that **the client cannot make a
//! heal happen**. It can only say "I started" and "I let go"; the server's own
//! tick clock decides whether the charge completed. So the tests here lean hard
//! on: an under-charged use never applies and never spends the item, and the item
//! is spent exactly once when it does.
//!
//! Also covered: the movement slow and its restore on every exit path, the
//! instant/over-time heal split, the `MAX_HEALTH` clamp, refresh-not-stack, and
//! the corpse rules.

use super::*;
use crate::{
    game_balance::{
        BANDAGE_HEAL_DURATION_TICKS, BANDAGE_HEAL_OVER_TIME, BANDAGE_INSTANT_HEAL,
        BANDAGE_USE_MOVE_MULTIPLIER, BANDAGE_USE_TICKS,
    },
    items::{BANDAGE_ID, WOOD_ID},
    protocol::{ConsumableCommand, MAX_HEALTH},
};

/// Connect a player with `count` bandages in the active actionbar slot, hurt down
/// to `health`.
fn setup(count: u16, health: f32) -> (GameServer, ClientId) {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let client = server.clients.get_mut(&client_id).expect("client exists");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BANDAGE_ID, count));
    client.inventory.active_actionbar_slot = 0;
    client.controller.health = health;
    (server, client_id)
}

fn health(server: &GameServer, client_id: ClientId) -> f32 {
    server.clients[&client_id].controller.health
}

fn bandages(server: &GameServer, client_id: ClientId) -> u32 {
    crate::inventory::count_items_in_inventory(&server.clients[&client_id].inventory, BANDAGE_ID)
}

fn speed(server: &GameServer, client_id: ClientId) -> f32 {
    server.clients[&client_id].run_speed_multiplier
}

/// Advance the server `ticks` fixed steps, heartbeating as a live client does.
///
/// The heartbeat is not incidental: `disconnect_stale_clients` sweeps a silent
/// session to a sleeping body after `CLIENT_STALE_TIMEOUT_TICKS` (3 s), and a
/// sleeping body's heal-over-time is deliberately dropped. Several of these tests
/// run well past 3 s of ticks, so without it they would be testing the offline
/// path by accident.
fn advance(server: &mut GameServer, client_id: ClientId, ticks: u64) {
    for _ in 0..ticks {
        server.receive(client_id, ClientMessage::Heartbeat);
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }
}

#[test]
fn a_full_charge_spends_one_bandage_and_lands_the_instant_heal() {
    let (mut server, client_id) = setup(3, 40.0);

    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    // One tick short of full: nothing has happened yet.
    advance(&mut server, client_id, BANDAGE_USE_TICKS - 1);
    assert_eq!(health(&server, client_id), 40.0, "must not heal early");
    assert_eq!(bandages(&server, client_id), 3, "must not spend early");

    // The tick the charge completes on: the item is spent and the instant chunk
    // lands. The over-time half has not started paying out yet.
    advance(&mut server, client_id, 1);
    assert_eq!(bandages(&server, client_id), 2);
    assert_eq!(health(&server, client_id), 40.0 + BANDAGE_INSTANT_HEAL);
    // And the charge is cleared, so it cannot re-complete next tick.
    assert!(server.clients[&client_id].use_started_tick.is_none());
    assert_eq!(speed(&server, client_id), 1.0, "movement restored");
}

#[test]
fn releasing_early_costs_nothing_at_all() {
    let (mut server, client_id) = setup(3, 40.0);

    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    // Get most of the way there, then let go.
    advance(&mut server, client_id, BANDAGE_USE_TICKS - 5);
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseCancel),
    );

    // Run well past when it *would* have completed. It must never apply.
    advance(&mut server, client_id, BANDAGE_USE_TICKS * 2);
    assert_eq!(
        health(&server, client_id),
        40.0,
        "an abandoned use never heals"
    );
    assert_eq!(bandages(&server, client_id), 3, "an abandoned use is free");
    assert_eq!(
        speed(&server, client_id),
        1.0,
        "movement restored on cancel"
    );
}

#[test]
fn the_client_cannot_forge_a_completed_charge() {
    // The protocol has no "apply" message, so the strongest thing a hostile client
    // can do is spam UseStart. Each one only ever RESTARTS the server's clock, so
    // spamming it can never complete a charge, it can only prevent one.
    let (mut server, client_id) = setup(1, 10.0);

    for _ in 0..(BANDAGE_USE_TICKS * 3) {
        server.receive(
            client_id,
            ClientMessage::Consumable(ConsumableCommand::UseStart),
        );
        advance(&mut server, client_id, 1);
    }

    assert_eq!(
        health(&server, client_id),
        10.0,
        "restarting the charge every tick must never complete it"
    );
    assert_eq!(bandages(&server, client_id), 1);
}

#[test]
fn the_use_slows_movement_and_every_exit_restores_it() {
    // Start: slowed.
    let (mut server, client_id) = setup(2, 50.0);
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    assert_eq!(speed(&server, client_id), BANDAGE_USE_MOVE_MULTIPLIER);

    // Exit 1: cancel.
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseCancel),
    );
    assert_eq!(speed(&server, client_id), 1.0);

    // Exit 2: swapping the actionbar slot away mid-charge.
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    assert_eq!(speed(&server, client_id), BANDAGE_USE_MOVE_MULTIPLIER);
    server.receive(
        client_id,
        ClientMessage::Inventory(crate::protocol::InventoryCommand::SelectActionbarSlot {
            slot: 1,
        }),
    );
    assert_eq!(speed(&server, client_id), 1.0);
    assert!(server.clients[&client_id].use_started_tick.is_none());

    // Exit 3: completion (back on the bandage slot).
    server.receive(
        client_id,
        ClientMessage::Inventory(crate::protocol::InventoryCommand::SelectActionbarSlot {
            slot: 0,
        }),
    );
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(&mut server, client_id, BANDAGE_USE_TICKS);
    assert_eq!(speed(&server, client_id), 1.0);
}

#[test]
fn the_heal_over_time_pays_out_the_full_remainder_and_then_stops() {
    let (mut server, client_id) = setup(1, 20.0);
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(&mut server, client_id, BANDAGE_USE_TICKS);

    let after_instant = health(&server, client_id);
    assert_eq!(after_instant, 20.0 + BANDAGE_INSTANT_HEAL);

    // Mid-window: some, but not all, of the trickle has landed.
    advance(&mut server, client_id, BANDAGE_HEAL_DURATION_TICKS / 2);
    let midway = health(&server, client_id);
    assert!(
        midway > after_instant && midway < after_instant + BANDAGE_HEAL_OVER_TIME,
        "trickle should be partway: {midway}"
    );

    // End of the window: the whole remainder has landed, to within the sub-point
    // batching tolerance.
    advance(&mut server, client_id, BANDAGE_HEAL_DURATION_TICKS / 2 + 2);
    let total = 20.0 + BANDAGE_INSTANT_HEAL + BANDAGE_HEAL_OVER_TIME;
    assert!(
        (health(&server, client_id) - total).abs() < 0.01,
        "expected ~{total}, got {}",
        health(&server, client_id)
    );
    assert!(
        server.clients[&client_id].heal_over_time.is_none(),
        "HoT cleared"
    );

    // And it stays put: no runaway trickle.
    advance(&mut server, client_id, BANDAGE_HEAL_DURATION_TICKS);
    assert!((health(&server, client_id) - total).abs() < 0.01);
}

#[test]
fn healing_never_exceeds_max_health() {
    // The server never calls `simulate_step`, so its clamp does not apply here.
    // `apply_player_heal` has to do it, or we replicate >100 HP to every nameplate.
    let (mut server, client_id) = setup(1, MAX_HEALTH - 2.0);
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(
        &mut server,
        client_id,
        BANDAGE_USE_TICKS + BANDAGE_HEAL_DURATION_TICKS + 5,
    );
    assert_eq!(health(&server, client_id), MAX_HEALTH);
}

#[test]
fn a_second_bandage_refreshes_the_trickle_rather_than_stacking_it() {
    let (mut server, client_id) = setup(2, 10.0);

    // First bandage, then let a chunk of its trickle run.
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(&mut server, client_id, BANDAGE_USE_TICKS);
    advance(&mut server, client_id, BANDAGE_HEAL_DURATION_TICKS / 2);

    // Second bandage on top.
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(&mut server, client_id, BANDAGE_USE_TICKS);

    let hot = server.clients[&client_id]
        .heal_over_time
        .expect("a fresh trickle is armed");
    // The completing tick also pays out one tick of the fresh trickle, so this
    // reads a hair under the full budget rather than exactly at it. What matters
    // is the ceiling: had the two bandages STACKED, the leftover half of the first
    // (~10 HP) would still be owed on top of the second's 20, and `remaining`
    // would be ~30.
    assert!(
        hot.remaining <= BANDAGE_HEAL_OVER_TIME && hot.remaining > BANDAGE_HEAL_OVER_TIME - 1.0,
        "the remainder is REPLACED, not added to the leftover: {}",
        hot.remaining
    );
    assert_eq!(bandages(&server, client_id), 0, "both were spent");
}

#[test]
fn a_corpse_neither_bandages_nor_regenerates() {
    // Two bandages: one is spent arming the trickle below, leaving one for the
    // corpse to fail to use.
    let (mut server, client_id) = setup(2, 30.0);

    // Arm a trickle, then die mid-way through it.
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(&mut server, client_id, BANDAGE_USE_TICKS);
    assert!(server.clients[&client_id].heal_over_time.is_some());

    server.kill_player(client_id, None, "");
    let dead_health = health(&server, client_id);
    assert_eq!(dead_health, 0.0);

    // The trickle is dropped, not paused: dying mid-heal forfeits the rest. A HoT
    // that ticked a corpse back above zero would resurrect them without going
    // through the respawn path.
    advance(&mut server, client_id, BANDAGE_HEAL_DURATION_TICKS + 5);
    assert_eq!(
        health(&server, client_id),
        0.0,
        "a corpse must not regenerate"
    );
    assert!(server.clients[&client_id].heal_over_time.is_none());

    // And a corpse cannot start a new bandage. Death already emptied the corpse's
    // inventory into a loot bag, so hand one straight back to isolate the rule
    // under test: even WITH a bandage in hand, a dead player cannot use it.
    {
        let client = server.clients.get_mut(&client_id).expect("client exists");
        client.inventory.actionbar_slots[0] = Some(ItemStack::new(BANDAGE_ID, 1));
        client.inventory.active_actionbar_slot = 0;
    }
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    assert!(server.clients[&client_id].use_started_tick.is_none());
    advance(&mut server, client_id, BANDAGE_USE_TICKS + 5);
    assert_eq!(health(&server, client_id), 0.0, "a corpse stays at zero");
    assert_eq!(bandages(&server, client_id), 1, "and does not spend one");
}

#[test]
fn a_non_consumable_item_cannot_start_a_use() {
    // Otherwise a client could slow-walk (or worse, arm a heal) holding a rock.
    let mut server = server();
    let client_id = connect_host(&mut server);
    {
        let client = server.clients.get_mut(&client_id).expect("client exists");
        client.inventory.actionbar_slots[0] = Some(ItemStack::new(WOOD_ID, 10));
        client.inventory.active_actionbar_slot = 0;
        client.controller.health = 40.0;
    }

    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    assert!(server.clients[&client_id].use_started_tick.is_none());
    assert_eq!(
        speed(&server, client_id),
        1.0,
        "no free movement slow either"
    );

    advance(&mut server, client_id, BANDAGE_USE_TICKS + 5);
    assert_eq!(health(&server, client_id), 40.0);
}

#[test]
fn losing_the_bandage_mid_charge_cancels_the_heal() {
    // The stack can vanish under a live charge (dropped, stashed in a box, looted).
    // The completion path re-checks, so no item means no heal.
    let (mut server, client_id) = setup(1, 30.0);
    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(&mut server, client_id, BANDAGE_USE_TICKS - 2);

    server
        .clients
        .get_mut(&client_id)
        .expect("client exists")
        .inventory
        .actionbar_slots[0] = None;

    advance(&mut server, client_id, 10);
    assert_eq!(health(&server, client_id), 30.0, "no item, no heal");
}

#[test]
fn a_cancel_with_no_use_running_is_a_harmless_no_op() {
    // Idempotence matters: the client fires UseCancel from four different guards
    // (overlay, wheel, swap, death) and they can overlap.
    let (mut server, client_id) = setup(1, 50.0);

    // Notably it must NOT stomp a movement multiplier it did not set.
    server
        .clients
        .get_mut(&client_id)
        .expect("client exists")
        .run_speed_multiplier = 2.0;

    for _ in 0..3 {
        server.receive(
            client_id,
            ClientMessage::Consumable(ConsumableCommand::UseCancel),
        );
    }
    assert_eq!(
        speed(&server, client_id),
        2.0,
        "must not stomp an admin /speed"
    );
    assert_eq!(health(&server, client_id), 50.0);
}

#[test]
fn the_peer_visible_charge_fraction_ramps_while_wrapping_and_zeroes_after() {
    // Peers see this: someone mid-bandage is slowed and committed, and worth
    // rushing. It rides the same replicated component as the bow draw.
    let (mut server, client_id) = setup(1, 30.0);
    let fraction = |server: &GameServer| {
        server
            .players_iter()
            .find(|view| view.client_id == client_id)
            .expect("player view")
            .charge_fraction
            .0
    };

    assert_eq!(fraction(&server), 0.0, "idle: nothing to show");

    server.receive(
        client_id,
        ClientMessage::Consumable(ConsumableCommand::UseStart),
    );
    advance(&mut server, client_id, BANDAGE_USE_TICKS / 2);
    let midway = fraction(&server);
    assert!(
        (midway - 0.5).abs() < 0.05,
        "halfway through the wrap, got {midway}"
    );

    // Once it completes the charge is cleared, so peers stop seeing it.
    advance(&mut server, client_id, BANDAGE_USE_TICKS / 2 + 1);
    assert_eq!(fraction(&server), 0.0);
}
