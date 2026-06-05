use super::*;

use super::targets::{ATTACK_RANGE_M, deployable_aim_point, ray_aabb_entry_distance};

use crate::items::item_definition;
use crate::protocol::{MAX_HEALTH, Vec3Net};

fn make_player(
    id: crate::protocol::ClientId,
    x: f32,
    z: f32,
    alive: bool,
) -> (Player, PlayerPublic) {
    let player = Player {
        client_id: id,
        account_id: 0,
    };
    let public = PlayerPublic {
        name: "tester".into(),
        position: Vec3Net::new(x, 0.0, z),
        velocity: Vec3Net::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        health: if alive { MAX_HEALTH } else { 0.0 },
        grounded: true,
        is_admin: false,
        chat_bubble: None,
    };
    (player, public)
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
        [(&target.0, &target.1, false)].into_iter(),
    )
    .expect("player should be in front and in range");
    assert_eq!(hit.0.client_id, 7);
    assert!(hit.3 < ATTACK_RANGE_M);
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
        [(&target.0, &target.1, false)].into_iter(),
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
        [(&target.0, &target.1, false)].into_iter(),
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
        [(&target.0, &target.1, false)].into_iter(),
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
        [(&target.0, &target.1, true)].into_iter(),
    )
    .expect("a sleeper looked down at should resolve");
    assert_eq!(hit.0.client_id, 7);
    assert!(hit.2, "the resolved target is flagged sleeping");
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
        [(&target.0, &target.1, false)].into_iter(),
    );
    assert!(hit.is_none(), "behind-camera targets must not register");
}

#[test]
fn ray_aabb_entry_distance_hits_a_box_in_front() {
    // Ray from origin pointing -Z; box centred 5 units ahead.
    let origin = Vec3Net::new(0.0, 0.0, 0.0);
    let direction = Vec3Net::new(0.0, 0.0, -1.0);
    let centre = Vec3Net::new(0.0, 0.0, -5.0);
    let distance = ray_aabb_entry_distance(origin, direction, centre, 0.4, 0.95)
        .expect("ray should enter the box");
    // Enters at the near face: 5 - half_width(0.4) = 4.6.
    assert!((distance - 4.6).abs() < 1e-3);
}

#[test]
fn ray_aabb_entry_distance_misses_offset_box() {
    // Box well to the side of a forward ray -> no hit.
    let origin = Vec3Net::new(0.0, 0.0, 0.0);
    let direction = Vec3Net::new(0.0, 0.0, -1.0);
    let centre = Vec3Net::new(10.0, 0.0, -5.0);
    assert!(ray_aabb_entry_distance(origin, direction, centre, 0.4, 0.95).is_none());
}

#[test]
fn ray_aabb_entry_distance_rejects_box_behind_origin() {
    // Box behind the eye (positive Z while forward is -Z).
    let origin = Vec3Net::new(0.0, 0.0, 0.0);
    let direction = Vec3Net::new(0.0, 0.0, -1.0);
    let centre = Vec3Net::new(0.0, 0.0, 5.0);
    assert!(ray_aabb_entry_distance(origin, direction, centre, 0.4, 0.95).is_none());
}

#[test]
fn ray_aabb_entry_distance_inside_box_returns_zero() {
    // Origin inside the box -> point-blank, entry distance 0.
    let origin = Vec3Net::new(0.0, 0.0, 0.0);
    let direction = Vec3Net::new(0.0, 0.0, -1.0);
    let centre = Vec3Net::new(0.0, 0.0, 0.0);
    let distance = ray_aabb_entry_distance(origin, direction, centre, 1.0, 1.0)
        .expect("inside the box still counts as a hit");
    assert_eq!(distance, 0.0);
}

fn workbench(
    id: crate::protocol::DeployedEntityId,
    x: f32,
    z: f32,
) -> (Deployable, DeployableTransform) {
    (
        Deployable {
            id,
            item_id: crate::items::intern_item_id(crate::items::WORKBENCH_T1_ID),
            kind: crate::items::DeployableKind::Workbench { tier: 1 },
            max_health: 500,
        },
        DeployableTransform {
            position: Vec3Net::new(x, 0.0, z),
            yaw: 0.0,
        },
    )
}

#[test]
fn best_deployable_target_picks_the_closest_in_cone() {
    // Aim slightly downward (pitch < 0) so the look ray lines up with
    // the aim points, which sit at the structure's collider half-height
    // below eye level. Both structures are in front; the nearer one wins.
    let eye = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    let near = workbench(1, 0.0, -2.0);
    let far = workbench(2, 0.0, -3.0);
    let pitch = -0.5;
    let hit = best_deployable_target(
        eye,
        0.0,
        pitch,
        [(&near.0, &near.1), (&far.0, &far.1)].into_iter(),
    )
    .expect("a deployable in front should be targeted");
    assert_eq!(hit.0.id, 1);
    // The winning score is the smaller eye→centre distance.
    assert!(hit.2 < 3.0);
}

#[test]
fn best_deployable_target_skips_out_of_range_and_off_cone() {
    let eye = Vec3Net::new(0.0, EYE_HEIGHT, 0.0);
    // Far beyond the 5.5m interact range.
    let far = workbench(1, 0.0, -50.0);
    assert!(best_deployable_target(eye, 0.0, 0.0, [(&far.0, &far.1)].into_iter()).is_none());

    // In range but off to the side, outside the look cone.
    let side = workbench(2, 5.0, 0.0);
    assert!(best_deployable_target(eye, 0.0, 0.0, [(&side.0, &side.1)].into_iter()).is_none());
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

#[test]
fn deployable_aim_point_lifts_to_the_collider_half_height() {
    let (meta, transform) = workbench(1, 0.0, 0.0);
    let profile = item_definition(&meta.item_id).unwrap().deployable.unwrap();
    let aim = deployable_aim_point(&meta, &transform);
    assert!((aim.y - profile.collider_half_height).abs() < 1e-4);
}
