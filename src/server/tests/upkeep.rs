//! Server-authority tests for Tool Cupboard upkeep and decay: the periodic
//! drain by claimed-piece tier counts, the fractional carry, the unpaid
//! decay, and the persistence of the cupboard's storage grid.

use super::*;
use crate::{
    game_balance::{UPKEEP_DECAY_PCT_PER_PERIOD, UPKEEP_PERIOD_TICKS},
    items::{DeployableKind, TOOL_CUPBOARD_ID, WOOD_ID, intern_item_id},
    protocol::{DeployedEntityId, PlaceDeployableCommand},
    server::test_support::place_foundation,
};

/// Foundation at the origin + a cupboard placed on it through the real
/// placement command (so the claim footprint is live). Returns the
/// cupboard's id.
fn claimed_base(server: &mut GameServer, owner: ClientId) -> DeployedEntityId {
    place_foundation(server, Vec3Net::ZERO);
    let top = crate::building::platform_top_offset(crate::building::BuildingPiece::Foundation)
        .expect("foundation is a platform");
    server
        .clients
        .get_mut(&owner)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(ItemStack::new(TOOL_CUPBOARD_ID, 1));
    server.apply_place_deployable_command(
        owner,
        PlaceDeployableCommand {
            item_id: intern_item_id(TOOL_CUPBOARD_ID),
            position: Vec3Net::new(0.0, top, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    server
        .deployed_entities
        .values()
        .find(|entity| matches!(entity.kind, DeployableKind::ToolCupboard))
        .map(|entity| entity.id)
        .expect("cupboard placed")
}

/// Stock the cupboard's grid with `quantity` of `item_id`.
fn stock(server: &mut GameServer, cupboard: DeployedEntityId, item_id: &str, quantity: u16) {
    let entity = server.deployed_entity_mut(cupboard).expect("cupboard");
    let storage = entity.storage.as_mut().expect("cupboard has a grid");
    let slot = storage
        .slots
        .iter_mut()
        .find(|slot| slot.is_none())
        .expect("free slot");
    *slot = Some(ItemStack::new(item_id, quantity));
}

fn wood_stocked(server: &GameServer, cupboard: DeployedEntityId) -> u32 {
    server.deployed_entities[&cupboard]
        .storage
        .as_ref()
        .expect("grid")
        .slots
        .iter()
        .flatten()
        .filter(|stack| stack.item_id.as_ref() == WOOD_ID)
        .map(|stack| u32::from(stack.quantity))
        .sum()
}

/// Run exactly one upkeep period boundary.
fn run_one_period(server: &mut GameServer, period: u64) {
    server.tick = period * UPKEEP_PERIOD_TICKS;
    server.tick_upkeep();
}

#[test]
fn upkeep_drains_wood_with_fractional_carry_and_a_stocked_base_never_decays() {
    let mut server = server();
    let owner = connect_host(&mut server);
    let cupboard = claimed_base(&mut server, owner);
    stock(&mut server, cupboard, WOOD_ID, 20);

    // One sticks foundation at 3 wood per in-game day, drained every 1/6 of
    // a day, owes 0.5 wood per period: the first period banks the fraction
    // (nothing taken), the second period takes exactly 1.
    run_one_period(&mut server, 1);
    assert_eq!(
        wood_stocked(&server, cupboard),
        20,
        "a sub-integer bill banks as carry, taking nothing"
    );
    run_one_period(&mut server, 2);
    assert_eq!(
        wood_stocked(&server, cupboard),
        19,
        "the banked fraction pays out as a whole unit on the next period"
    );

    // Stocked: the foundation never decays.
    let foundation = server
        .deployed_entities
        .values()
        .find(|entity| matches!(entity.kind, DeployableKind::Building { .. }))
        .map(|entity| (entity.id, entity.health, entity.max_health))
        .expect("foundation");
    assert_eq!(foundation.1, foundation.2, "a paid base keeps full health");
}

#[test]
fn unpaid_upkeep_decays_the_claimed_pieces_until_destroyed() {
    let mut server = server();
    let owner = connect_host(&mut server);
    let cupboard = claimed_base(&mut server, owner);
    // Nothing stocked: the first full-unit bill goes unpaid and decay begins.

    let foundation_id = server
        .deployed_entities
        .values()
        .find(|entity| matches!(entity.kind, DeployableKind::Building { .. }))
        .map(|entity| entity.id)
        .expect("foundation");
    let max = server.deployed_entities[&foundation_id].max_health;

    // Period 1 banks the 0.5-wood fraction; period 2 is the first with a
    // whole-unit bill (0.5 carry -> 1.0 owed), which goes unpaid.
    run_one_period(&mut server, 1);
    run_one_period(&mut server, 2);
    let after = server.deployed_entities[&foundation_id].health;
    let expected_loss = (max * UPKEEP_DECAY_PCT_PER_PERIOD / 100).max(1);
    assert_eq!(
        after,
        max - expected_loss,
        "an unpaid period costs the piece its decay fraction"
    );
    // The cupboard flags the decay for the container readout.
    let info = server
        .upkeep_info_for(cupboard)
        .expect("cupboard has an upkeep readout");
    assert!(info.decaying, "the readout flags the unpaid decay");

    // Keep the bill unpaid long enough and the piece rots away entirely
    // (destroyed through the normal spill + stability path).
    for period in 3..60 {
        if !server.deployed_entities.contains_key(&foundation_id) {
            break;
        }
        run_one_period(&mut server, period);
    }
    assert!(
        !server.deployed_entities.contains_key(&foundation_id),
        "a dry base decays to nothing"
    );
}

#[test]
fn restocking_stops_further_decay() {
    let mut server = server();
    let owner = connect_host(&mut server);
    let cupboard = claimed_base(&mut server, owner);

    let foundation_id = server
        .deployed_entities
        .values()
        .find(|entity| matches!(entity.kind, DeployableKind::Building { .. }))
        .map(|entity| entity.id)
        .expect("foundation");

    // One unpaid whole-unit period: decay lands (period 1 banks the
    // fraction, period 2 bills a whole unit that the empty grid can't pay).
    run_one_period(&mut server, 1);
    run_one_period(&mut server, 2);
    let decayed = server.deployed_entities[&foundation_id].health;
    assert!(decayed < server.deployed_entities[&foundation_id].max_health);

    // Restock, run more periods: no further decay (lost HP stays lost).
    stock(&mut server, cupboard, WOOD_ID, 50);
    for period in 3..8 {
        run_one_period(&mut server, period);
    }
    assert_eq!(
        server.deployed_entities[&foundation_id].health, decayed,
        "a restocked base stops decaying"
    );
    let info = server.upkeep_info_for(cupboard).expect("readout");
    assert!(!info.decaying, "the decay flag clears once paid");
}

#[test]
fn cupboard_storage_survives_a_save_round_trip() {
    let mut server = server();
    let owner = connect_host(&mut server);
    let cupboard = claimed_base(&mut server, owner);
    stock(&mut server, cupboard, WOOD_ID, 37);

    let persisted = server.persisted_deployed_entities();
    let restored = GameServer::restore_deployed_entities(persisted);
    let restored_cupboard = restored
        .values()
        .find(|entity| matches!(entity.kind, DeployableKind::ToolCupboard))
        .expect("cupboard restored");
    let stocked: u32 = restored_cupboard
        .storage
        .as_ref()
        .expect("grid restored")
        .slots
        .iter()
        .flatten()
        .filter(|stack| stack.item_id.as_ref() == WOOD_ID)
        .map(|stack| u32::from(stack.quantity))
        .sum();
    assert_eq!(stocked, 37, "the upkeep stock persists in the save");
}
