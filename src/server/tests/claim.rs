//! Tool Cupboard building-privilege tests: the foundation-projected claim
//! gate, the authorize/deauthorize/clear commands, the "must sit on a
//! platform" placement rule, and save/load of the authorized list.

use crate::{
    building::{BuildingPiece, platform_top_offset},
    items::{DeployableKind, TOOL_CUPBOARD_ID, intern_item_id, item_definition},
    protocol::{
        AccountId, ClaimCommand, ClientId, DeployedEntityId, GAME_VERSION, ItemStack,
        PROTOCOL_VERSION, PlaceDeployableCommand, Vec3Net,
    },
    server::GameServer,
};

use super::super::{claim::CupboardState, deployables::DeployedEntity};
use crate::server::test_support::{connect_named, place_foundation, server};

/// Connect a client on a specific account (the shared `connect_named`
/// pins everyone to account 1, which can't model owner vs. raider).
fn connect_account(server: &mut GameServer, account: AccountId, name: &str) -> ClientId {
    let client_id = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            account,
            name.to_owned(),
            String::new(),
        )
        .expect("client should connect")
        .0;
    server
        .clients
        .get_mut(&client_id)
        .expect("connected client")
        .controller
        .position = Vec3Net::ZERO;
    client_id
}

/// Drop a Tool Cupboard onto the top of the foundation at `foundation_pos`
/// and recompute the claim footprint, returning the cupboard id.
fn place_cupboard(
    server: &mut GameServer,
    foundation_pos: Vec3Net,
    owner: AccountId,
) -> DeployedEntityId {
    let top = platform_top_offset(BuildingPiece::Foundation).expect("foundation has a top");
    let pos = Vec3Net::new(foundation_pos.x, foundation_pos.y + top, foundation_pos.z);
    let profile = item_definition(TOOL_CUPBOARD_ID)
        .and_then(|def| def.deployable)
        .expect("cupboard profile");
    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = DeployedEntity {
        id,
        item_id: intern_item_id(TOOL_CUPBOARD_ID),
        kind: DeployableKind::ToolCupboard,
        position: pos,
        yaw: 0.0,
        health: profile.max_health,
        max_health: profile.max_health,
        owner: Some(owner),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: Some(CupboardState {
            authorized: vec![owner],
        }),
        ruin_cache: None,
        fuse: None,
    };
    server.insert_deployed_entity(id, entity);
    server.chunk_manager.track_deployed_entity(id, pos);
    server.recompute_claim_footprints();
    id
}

#[test]
fn claim_gate_blocks_non_owner_but_not_owner_or_admin() {
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);
    place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));

    let inside = Vec3Net::ZERO; // the claimed cell itself
    let margin = Vec3Net::new(6.0, 0.0, 0.0); // two cells out, well inside the margin ring

    // A non-owner is blocked on the footprint and inside the margin ring,
    // with no carve-out: even a first-tier piece is refused.
    assert!(server.claim_blocks_placement(inside, crate::protocol::AccountId(2)));
    assert!(server.claim_blocks_placement(margin, crate::protocol::AccountId(2)));
    // An authorized account (the placer, auto-added) is never blocked.
    assert!(!server.claim_blocks_placement(inside, crate::protocol::AccountId(1)));
}

#[test]
fn claim_does_not_reach_past_the_margin_ring() {
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);
    place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));

    // Two cells past the last claimed cell (footprint cell 0 + the margin
    // ring), well clear of the point test's reach regardless of the exact
    // margin value.
    let margin = crate::game_balance::BUILDING_PRIVILEGE_MARGIN_CELLS;
    let far = Vec3Net::new((margin + 2) as f32 * 3.0, 0.0, 0.0);
    assert!(!server.claim_blocks_placement(far, crate::protocol::AccountId(2)));
}

#[test]
fn footprint_gate_blocks_a_slab_reaching_into_the_claim() {
    use crate::building::{BuildingPiece, building_collider_blocks};

    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);
    place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));

    // The first cell beyond the margin ring: a foundation centred there is
    // flush-adjacent to the last claimed cell, so its body stays out and a
    // non-owner may build it. Derived from the margin so it survives tuning.
    let margin = crate::game_balance::BUILDING_PRIVILEGE_MARGIN_CELLS;
    let edge = (margin + 1) as f32 * 3.0;
    let flush =
        building_collider_blocks(BuildingPiece::Foundation, Vec3Net::new(edge, 0.0, 0.0), 0.0);
    assert!(!server.claim_blocks_footprint(&flush, crate::protocol::AccountId(2)));

    // Slide that same slab a metre back toward the base and its body now
    // overlaps the claim, the footprint gate refuses it even though its
    // centre is still outside the last claimed cell.
    let intruding = building_collider_blocks(
        BuildingPiece::Foundation,
        Vec3Net::new(edge - 1.0, 0.0, 0.0),
        0.0,
    );
    assert!(server.claim_blocks_footprint(&intruding, crate::protocol::AccountId(2)));
    // The owner (authorized) is never blocked, footprint or not.
    assert!(!server.claim_blocks_footprint(&intruding, crate::protocol::AccountId(1)));
}

#[test]
fn authorize_self_then_deauthorize_self() {
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);
    let cupboard = place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));
    let raider = connect_account(&mut server, crate::protocol::AccountId(2), "Raider");

    // Not authorized -> blocked.
    assert!(server.claim_blocks_placement(Vec3Net::ZERO, crate::protocol::AccountId(2)));

    server.apply_claim_command(raider, ClaimCommand::AuthorizeSelf { id: cupboard });
    assert!(server.cupboard_authorizes(cupboard, crate::protocol::AccountId(2)));
    assert!(!server.claim_blocks_placement(Vec3Net::ZERO, crate::protocol::AccountId(2)));

    server.apply_claim_command(raider, ClaimCommand::DeauthorizeSelf { id: cupboard });
    assert!(!server.cupboard_authorizes(cupboard, crate::protocol::AccountId(2)));
    assert!(server.claim_blocks_placement(Vec3Net::ZERO, crate::protocol::AccountId(2)));
}

#[test]
fn placer_starts_authorized_and_can_toggle_off() {
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);
    let cupboard = place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));
    let owner = connect_named(&mut server, "Owner"); // account 1

    // The placer is authorized by default (auto-added on placement).
    assert!(server.cupboard_authorizes(cupboard, crate::protocol::AccountId(1)));
    assert!(!server.claim_blocks_placement(Vec3Net::ZERO, crate::protocol::AccountId(1)));

    // ...but can toggle their own access off with tap-E, like anyone.
    server.apply_claim_command(owner, ClaimCommand::DeauthorizeSelf { id: cupboard });
    assert!(!server.cupboard_authorizes(cupboard, crate::protocol::AccountId(1)));
    assert!(server.claim_blocks_placement(Vec3Net::ZERO, crate::protocol::AccountId(1)));

    // ...and back on.
    server.apply_claim_command(owner, ClaimCommand::AuthorizeSelf { id: cupboard });
    assert!(server.cupboard_authorizes(cupboard, crate::protocol::AccountId(1)));
}

#[test]
fn building_modify_follows_cupboard_authorization() {
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO); // builder is account 1
    let pos = Vec3Net::ZERO;

    // Unclaimed base: only the original builder may upgrade/demolish.
    assert!(server.building_modify_allowed(pos, crate::protocol::AccountId(1), true));
    assert!(!server.building_modify_allowed(pos, crate::protocol::AccountId(2), false));

    // Once claimed, authorization at the cupboard governs modify rights.
    let cupboard = place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));
    let teammate = connect_account(&mut server, crate::protocol::AccountId(2), "Teammate");
    assert!(!server.building_modify_allowed(pos, crate::protocol::AccountId(2), false));
    server.apply_claim_command(teammate, ClaimCommand::AuthorizeSelf { id: cupboard });
    assert!(server.building_modify_allowed(pos, crate::protocol::AccountId(2), false));
}

#[test]
fn clear_list_revokes_authorized_but_keeps_owner() {
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);
    let cupboard = place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));
    let owner = connect_named(&mut server, "Owner"); // account 1
    let friend = connect_account(&mut server, crate::protocol::AccountId(2), "Friend");

    server.apply_claim_command(friend, ClaimCommand::AuthorizeSelf { id: cupboard });
    assert!(server.cupboard_authorizes(cupboard, crate::protocol::AccountId(2)));

    server.apply_claim_command(owner, ClaimCommand::ClearList { id: cupboard });
    assert!(!server.cupboard_authorizes(cupboard, crate::protocol::AccountId(2)));
    // The owner can never be cleared out of their own claim.
    assert!(server.cupboard_authorizes(cupboard, crate::protocol::AccountId(1)));
}

#[test]
fn cupboard_must_be_placed_on_a_platform() {
    let mut server = server();
    let owner = connect_named(&mut server, "Owner"); // account 1
    server
        .clients
        .get_mut(&owner)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(ItemStack::new(TOOL_CUPBOARD_ID, 2));

    // Bare ground: rejected, and nothing is placed.
    server.apply_place_deployable_command(
        owner,
        PlaceDeployableCommand {
            item_id: intern_item_id(TOOL_CUPBOARD_ID),
            position: Vec3Net::ZERO,
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(
        !server
            .deployed_entities
            .values()
            .any(|entity| matches!(entity.kind, DeployableKind::ToolCupboard)),
        "a cupboard must not place on bare ground"
    );

    // On a foundation top: accepted.
    place_foundation(&mut server, Vec3Net::ZERO);
    let top = platform_top_offset(BuildingPiece::Foundation).unwrap();
    server.apply_place_deployable_command(
        owner,
        PlaceDeployableCommand {
            item_id: intern_item_id(TOOL_CUPBOARD_ID),
            position: Vec3Net::new(0.0, top, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(
        server
            .deployed_entities
            .values()
            .any(|entity| matches!(entity.kind, DeployableKind::ToolCupboard)),
        "a cupboard should place on a foundation top"
    );
}

#[test]
fn authorized_list_survives_save_round_trip() {
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);
    // place_cupboard auto-adds the placer (1); authorize a second account.
    let cupboard = place_cupboard(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(1));
    server
        .deployed_entity_mut(cupboard)
        .unwrap()
        .cupboard
        .as_mut()
        .unwrap()
        .authorized
        .push(crate::protocol::AccountId(2));

    let persisted = server.persisted_deployed_entities();
    let restored = GameServer::restore_deployed_entities(persisted);
    let restored_cupboard = restored.get(&cupboard).expect("cupboard restored");
    assert_eq!(
        restored_cupboard
            .cupboard
            .as_ref()
            .expect("cupboard state restored")
            .authorized,
        [1, 2].map(crate::protocol::AccountId).to_vec()
    );
}
