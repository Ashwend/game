use super::*;

use super::targets::ATTACK_RANGE_M;

use crate::protocol::{MAX_HEALTH, Vec3Net};

fn make_player(
    id: crate::protocol::ClientId,
    x: f32,
    z: f32,
    alive: bool,
) -> (Player, Vec3Net, f32) {
    let player = Player {
        client_id: id,
        account_id: 0,
    };
    let position = Vec3Net::new(x, 0.0, z);
    let health = if alive { MAX_HEALTH } else { 0.0 };
    (player, position, health)
}

fn candidate(target: &(Player, Vec3Net, f32), sleeping: bool) -> PlayerTargetCandidate<'_> {
    PlayerTargetCandidate {
        player: &target.0,
        name: "tester",
        position: target.1,
        health: target.2,
        sleeping,
    }
}

#[test]
fn player_in_view_within_range_resolves_as_target() {
    let attacker = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let target = make_player(7, 0.0, -2.0, true);

    // yaw=0, pitch=0 → forward is -Z, target sits at z=-2.
    let hit = best_player_target(
        attacker,
        0.0,
        0.0,
        Some(1),
        [candidate(&target, false)].into_iter(),
    )
    .expect("player should be in front and in range");
    assert_eq!(hit.0.player.client_id, 7);
    assert!(hit.1 < ATTACK_RANGE_M);
}

#[test]
fn local_player_is_skipped() {
    let attacker = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let target = make_player(1, 0.0, -2.0, true);

    let hit = best_player_target(
        attacker,
        0.0,
        0.0,
        Some(1),
        [candidate(&target, false)].into_iter(),
    );
    assert!(hit.is_none(), "local client must not target itself");
}

#[test]
fn player_out_of_range_is_skipped() {
    let attacker = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    // Beyond ATTACK_RANGE_M.
    let target = make_player(7, 0.0, -(ATTACK_RANGE_M + 1.0), true);

    let hit = best_player_target(
        attacker,
        0.0,
        0.0,
        Some(1),
        [candidate(&target, false)].into_iter(),
    );
    assert!(hit.is_none(), "player past attack range must not target");
}

#[test]
fn dead_player_is_skipped() {
    let attacker = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let target = make_player(7, 0.0, -2.0, false);

    let hit = best_player_target(
        attacker,
        0.0,
        0.0,
        Some(1),
        [candidate(&target, false)].into_iter(),
    );
    assert!(hit.is_none(), "dead targets are not attackable");
}

#[test]
fn sleeping_body_resolves_as_a_low_target() {
    // A logged-out body lies on the ground just ahead; looking down at it
    // resolves via the low, wide sleeper hit box and flags it sleeping so the
    // tooltip can identify it.
    let attacker = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let target = make_player(7, 0.0, -1.5, true);
    let hit = best_player_target(
        attacker,
        0.0,
        -0.8,
        Some(1),
        [candidate(&target, true)].into_iter(),
    )
    .expect("a sleeper looked down at should resolve");
    assert_eq!(hit.0.player.client_id, 7);
    assert!(hit.0.sleeping, "the resolved target is flagged sleeping");
}

#[test]
fn player_behind_is_skipped() {
    let attacker = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    // Target behind the camera (positive Z). Forward is -Z.
    let target = make_player(7, 0.0, 2.0, true);

    let hit = best_player_target(
        attacker,
        0.0,
        0.0,
        Some(1),
        [candidate(&target, false)].into_iter(),
    );
    assert!(hit.is_none(), "behind-camera targets must not register");
}

type DeployableFixture = (
    Deployable,
    DeployableTransform,
    crate::server::DeployableStability,
    crate::server::DeployableActive,
);

fn workbench(id: crate::protocol::DeployedEntityId, x: f32, z: f32) -> DeployableFixture {
    (
        Deployable {
            id,
            item_id: crate::items::intern_item_id(crate::items::WORKBENCH_T1_ID),
            kind: crate::items::DeployableKind::Workbench { tier: 1 },
            max_health: 500,
            owner: None,
            placed_at_tick: 0,
        },
        DeployableTransform {
            position: Vec3Net::new(x, 0.0, z),
            yaw: 0.0,
        },
        crate::server::DeployableStability(100),
        crate::server::DeployableActive(false),
    )
}

fn building_wall(id: crate::protocol::DeployedEntityId, x: f32, z: f32) -> DeployableFixture {
    (
        Deployable {
            id,
            item_id: crate::items::intern_item_id(crate::building::BUILDING_WALL_ITEM_ID),
            kind: crate::items::DeployableKind::Building {
                piece: crate::building::BuildingPiece::Wall,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: 250,
            owner: None,
            placed_at_tick: 0,
        },
        DeployableTransform {
            // Walls sit on a foundation top (base at y = 0.5).
            position: Vec3Net::new(x, 0.5, z),
            yaw: 0.0,
        },
        crate::server::DeployableStability(100),
        crate::server::DeployableActive(false),
    )
}

fn door(id: crate::protocol::DeployedEntityId, z: f32, open: bool) -> DeployableFixture {
    (
        Deployable {
            id,
            item_id: crate::items::intern_item_id(crate::items::HEWN_LOG_DOOR_ID),
            kind: crate::items::DeployableKind::Door,
            max_health: 400,
            owner: None,
            placed_at_tick: 0,
        },
        DeployableTransform {
            position: Vec3Net::new(0.0, 0.5, z),
            yaw: 0.0,
        },
        crate::server::DeployableStability(100),
        crate::server::DeployableActive(open),
    )
}

fn fixture_refs(
    fixture: &DeployableFixture,
) -> (
    &Deployable,
    &DeployableTransform,
    &crate::server::DeployableStability,
    &crate::server::DeployableActive,
) {
    (&fixture.0, &fixture.1, &fixture.2, &fixture.3)
}

#[test]
fn best_deployable_target_picks_the_closest_ray_hit() {
    // Aim slightly downward so the look ray passes through the boxes.
    // Both structures are in front; the nearer one wins.
    let eye = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let near = workbench(1, 0.0, -2.0);
    let far = workbench(2, 0.0, -3.0);
    let pitch = -0.5;
    let hit = best_deployable_target(
        eye,
        0.0,
        pitch,
        [fixture_refs(&near), fixture_refs(&far)].into_iter(),
    )
    .expect("a deployable in front should be targeted");
    assert_eq!(hit.0.id, 1);
    assert!(hit.3 < 3.0);
}

#[test]
fn best_deployable_target_skips_out_of_range_and_missed() {
    let eye = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    // Far beyond the interact range.
    let far = workbench(1, 0.0, -50.0);
    assert!(best_deployable_target(eye, 0.0, -0.05, [fixture_refs(&far)].into_iter()).is_none());

    // In range but off to the side, the ray never enters its box.
    let side = workbench(2, 5.0, 0.0);
    assert!(best_deployable_target(eye, 0.0, 0.0, [fixture_refs(&side)].into_iter()).is_none());
}

#[test]
fn point_blank_wall_wins_over_the_wall_behind_it() {
    // Regression: the old cone-toward-centre test skipped a 3 m wall the
    // player stood right in front of (its centre sat far outside the
    // cone at point-blank range) and latched onto a wall further away
    // whose centre happened to line up with the ray. The ray-vs-boxes
    // test must pick the nearer wall.
    let eye = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let near = building_wall(1, 1.0, -0.5);
    let behind = building_wall(2, 0.0, -2.8);
    let hit = best_deployable_target(
        eye,
        0.0,
        0.0,
        [fixture_refs(&near), fixture_refs(&behind)].into_iter(),
    )
    .expect("the near wall must be targetable at point-blank range");
    assert_eq!(
        hit.0.id, 1,
        "must hit the wall in front, not the one behind"
    );
    assert!(
        hit.3 < 1.0,
        "hit distance is the entry point, got {}",
        hit.3
    );
}

#[test]
fn ray_through_a_doorway_opening_hits_the_piece_behind() {
    // The doorway's collider has a genuine hole; aiming through it must
    // resolve to the wall behind, not the doorway frame.
    let doorway = (
        Deployable {
            id: 1,
            item_id: crate::items::intern_item_id(crate::building::BUILDING_DOORWAY_ITEM_ID),
            kind: crate::items::DeployableKind::Building {
                piece: crate::building::BuildingPiece::Doorway,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: 250,
            owner: None,
            placed_at_tick: 0,
        },
        DeployableTransform {
            position: Vec3Net::new(0.0, 0.0, -1.0),
            yaw: 0.0,
        },
        crate::server::DeployableStability(100),
        crate::server::DeployableActive(false),
    );
    let wall_behind = building_wall(2, 0.0, -2.5);
    // Eye at standing height aiming straight through the opening centre.
    let eye = Vec3Net::new(0.0, 1.2, 0.0);
    let hit = best_deployable_target(
        eye,
        0.0,
        0.0,
        [fixture_refs(&doorway), fixture_refs(&wall_behind)].into_iter(),
    )
    .expect("the wall behind the opening should be hit");
    assert_eq!(hit.0.id, 2);
}

#[test]
fn door_targeting_follows_the_swung_panel() {
    // A closed door (base at y = 0.5, yaw 0) is targeted through the
    // opening plane; once open, the panel swings toward +Z around the
    // hinge on the -X side and the target volume moves with it.
    let closed = door(1, -2.0, false);
    let centre_eye = Vec3Net::new(0.0, 1.7, 0.0);
    assert!(
        best_deployable_target(centre_eye, 0.0, 0.0, [fixture_refs(&closed)].into_iter()).is_some(),
        "a closed panel fills the opening plane"
    );

    let open = door(1, -2.0, true);
    assert!(
        best_deployable_target(centre_eye, 0.0, 0.0, [fixture_refs(&open)].into_iter()).is_none(),
        "an open door leaves the opening clear, the ray must pass through"
    );
    // Aim down the swung panel's resting position: hinge side (-X),
    // sticking out toward +Z from the doorway at z = -2.
    let panel_eye = Vec3Net::new(-0.6, 1.7, 0.0);
    let hit = best_deployable_target(panel_eye, 0.0, 0.0, [fixture_refs(&open)].into_iter())
        .expect("the swung panel must be targetable where the mesh is");
    assert_eq!(hit.0.id, 1);
}

#[test]
fn best_loot_bag_target_finds_bag_in_front() {
    let eye = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let bag = LootBagEntity { id: 9 };
    let transform = LootBagTransform {
        position: Vec3Net::new(0.0, EYE_HEIGHT - 0.4, -2.0),
        yaw: 0.0,
    };
    let hit = best_loot_bag_target(eye, 0.0, 0.0, [(&bag, &transform)].into_iter())
        .expect("a bag in front and in range should be found");
    assert_eq!(hit.0.id, 9);

    // A bag far past the 4.5m range is rejected.
    let far = LootBagTransform {
        position: Vec3Net::new(0.0, EYE_HEIGHT - 0.4, -50.0),
        yaw: 0.0,
    };
    assert!(best_loot_bag_target(eye, 0.0, 0.0, [(&bag, &far)].into_iter()).is_none());
}
