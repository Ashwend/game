use super::*;
use crate::world::WorldBlock;

use super::movement::{desired_horizontal_velocity, horizontal_length};

fn test_world() -> WorldData {
    WorldData::test_world()
}

fn input(sequence: u64, direction: Vec3Net, sprint: bool, jump: bool) -> PlayerInput {
    PlayerInput {
        sequence,
        delta_seconds: 1.0 / 60.0,
        direction,
        sprint,
        jump,
        yaw: 0.0,
        pitch: 0.0,
    }
}

#[test]
fn movement_direction_matches_bevy_camera_yaw() {
    let forward = first_person_move_direction(Vec3Net::new(0.0, 0.0, 1.0), 0.0);
    assert!(forward.z < -0.99);
    assert!(forward.x.abs() < 0.001);

    let looking_right =
        first_person_move_direction(Vec3Net::new(0.0, 0.0, 1.0), -std::f32::consts::FRAC_PI_2);
    assert!(looking_right.x > 0.99);
    assert!(looking_right.z.abs() < 0.001);

    let strafe_right =
        first_person_move_direction(Vec3Net::new(1.0, 0.0, 0.0), -std::f32::consts::FRAC_PI_2);
    assert!(strafe_right.z > 0.99);
    assert!(strafe_right.x.abs() < 0.001);
}

#[test]
fn sprinting_is_forward_weighted_and_sidewalking_is_slower() {
    let forward = desired_horizontal_velocity(Vec3Net::new(0.0, 0.0, 1.0), 0.0, true);
    let side = desired_horizontal_velocity(Vec3Net::new(1.0, 0.0, 0.0), 0.0, true);
    let back = desired_horizontal_velocity(Vec3Net::new(0.0, 0.0, -1.0), 0.0, true);
    let diagonal = desired_horizontal_velocity(Vec3Net::new(1.0, 0.0, 1.0), 0.0, true);

    assert!(horizontal_length(forward) > horizontal_length(side) + 3.0);
    assert!(horizontal_length(side) > horizontal_length(back));
    assert!(horizontal_length(diagonal) <= SPRINT_SPEED);
    assert!(diagonal.x > 0.0);
    assert!(diagonal.z < 0.0);
}

#[test]
fn sprint_jump_creates_modest_forward_boost() {
    let mut controller = PlayerController::spawn();
    controller.apply_input(input(1, Vec3Net::new(0.0, 0.0, 1.0), true, true));
    controller.simulate(1.0 / 60.0, &test_world());

    assert!(controller.position.y > 0.0);
    assert!(!controller.grounded);
    assert!(horizontal_length(controller.velocity) > SPRINT_SPEED + 0.1);
    assert!(horizontal_length(controller.velocity) < SPRINT_SPEED + 0.6);
    assert!(controller.velocity.y > JUMP_SPEED - 0.4);
    assert!(controller.velocity.z < -SPRINT_SPEED - 0.1);
}

#[test]
fn controller_steps_over_low_obstacles_without_jumping() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![WorldBlock::new(
            Vec3Net::new(0.0, 0.18, -0.95),
            Vec3Net::new(0.6, 0.18, 0.25),
        )],
    };
    let mut controller = PlayerController::spawn();
    controller.apply_input(input(1, Vec3Net::new(0.0, 0.0, 1.0), false, false));

    for _ in 0..80 {
        controller.simulate(1.0 / 120.0, &world);
        if controller.position.y > 0.3 {
            break;
        }
    }

    assert!(controller.position.y > 0.3);
    assert!(controller.position.z < -0.35);
    assert!(controller.grounded);
}

#[test]
fn step_up_smooths_view_without_smoothing_physical_collision() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![WorldBlock::new(
            Vec3Net::new(0.0, 0.18, -0.95),
            Vec3Net::new(0.6, 0.18, 0.25),
        )],
    };
    let mut controller = PlayerController::spawn();
    controller.apply_input(input(1, Vec3Net::new(0.0, 0.0, 1.0), false, false));

    for _ in 0..80 {
        controller.simulate(1.0 / 120.0, &world);
        if controller.position.y > 0.3 {
            break;
        }
    }

    assert!(controller.position.y > 0.3);
    assert!(controller.view_position().y < controller.position.y - 0.05);

    controller.apply_input(input(2, Vec3Net::ZERO, false, false));
    for _ in 0..60 {
        controller.simulate(1.0 / 120.0, &world);
    }

    assert!((controller.view_position().y - controller.position.y).abs() < 0.02);
}

#[test]
fn failed_corner_step_does_not_push_player_off_current_cube() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![
            WorldBlock::new(Vec3Net::new(0.0, 0.3, 0.0), Vec3Net::new(0.9, 0.3, 0.9)),
            WorldBlock::new(
                Vec3Net::new(2.05, 1.0, 0.55),
                Vec3Net::new(0.45, 0.35, 0.45),
            ),
        ],
    };
    let mut controller = PlayerController::spawn();
    controller.position = Vec3Net::new(1.24, 0.6, 0.55);
    controller.velocity = Vec3Net::new(4.0, 0.0, 0.0);
    controller.grounded = true;
    controller.move_horizontal_with_step(&world, Axis::X, 0.2);

    assert!(controller.position.x <= 1.25);
    assert_eq!(controller.position.y, 0.6);
    assert_eq!(controller.position.z, 0.55);
    assert!(controller.grounded);
    assert_eq!(controller.velocity.x, 0.0);
}

#[test]
fn collision_resolution_does_not_cascade_across_nearby_blocks() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![
            WorldBlock::new(Vec3Net::new(0.0, 0.25, -6.0), Vec3Net::new(2.0, 0.25, 0.8)),
            WorldBlock::new(Vec3Net::new(1.7, 0.38, -4.1), Vec3Net::new(0.8, 0.38, 0.5)),
        ],
    };
    let mut position = Vec3Net::new(2.35, 0.0, -6.1762643);
    let mut velocity = Vec3Net::new(0.0, 0.0, -5.0);

    let result = move_with_collisions(&mut position, &mut velocity, &world, Axis::Z, -0.0417);

    assert!(!result.collided);
    assert!((position.z - -6.217964).abs() < 0.001);
    assert_eq!(velocity.z, -5.0);
}

#[test]
fn collision_resolution_ignores_adjacent_face_not_crossed_by_axis_move() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![
            WorldBlock::new(Vec3Net::new(0.0, 0.25, -6.0), Vec3Net::new(2.0, 0.25, 0.8)),
            WorldBlock::new(Vec3Net::new(1.7, 0.38, -4.1), Vec3Net::new(0.8, 0.38, 0.5)),
        ],
    };
    let mut position = Vec3Net::new(0.5500001, 0.0, -4.85);
    let mut velocity = Vec3Net::new(0.0, 0.0, -0.5666593);

    let result = move_with_collisions(&mut position, &mut velocity, &world, Axis::Z, -0.0047);

    assert!(result.collided);
    assert!((position.z - -4.85).abs() < 0.001);
    assert_eq!(velocity.z, 0.0);
}

#[test]
fn collision_resolution_allows_sliding_out_of_current_axis_overlap() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![
            WorldBlock::new(Vec3Net::new(0.0, 0.25, -6.0), Vec3Net::new(2.0, 0.25, 0.8)),
            WorldBlock::new(Vec3Net::new(1.7, 0.38, -4.1), Vec3Net::new(0.8, 0.38, 0.5)),
        ],
    };
    let mut position = Vec3Net::new(2.35, 0.0, -6.7282076);
    let mut velocity = Vec3Net::new(0.0, 0.0, -4.5498476);

    let result = move_with_collisions(&mut position, &mut velocity, &world, Axis::Z, -0.0297);

    assert!(!result.collided);
    assert!((position.z - -6.757908).abs() < 0.001);
    assert_eq!(velocity.z, -4.5498476);
}

#[test]
fn controller_cannot_step_up_tall_walls() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![WorldBlock::new(
            Vec3Net::new(0.0, 0.7, -0.95),
            Vec3Net::new(0.6, 0.7, 0.25),
        )],
    };
    let mut controller = PlayerController::spawn();
    controller.apply_input(input(1, Vec3Net::new(0.0, 0.0, 1.0), false, false));

    for _ in 0..80 {
        controller.simulate(1.0 / 120.0, &world);
    }

    assert!(controller.position.y < 0.05);
    assert!(controller.position.z > -0.5);
    assert!(controller.grounded);
}

#[test]
fn jump_request_survives_following_non_jump_input_before_tick() {
    let mut controller = PlayerController::spawn();
    controller.apply_input(PlayerInput {
        sequence: 1,
        delta_seconds: 0.05,
        direction: Vec3Net::ZERO,
        sprint: false,
        jump: true,
        yaw: 0.0,
        pitch: 0.0,
    });
    controller.apply_input(PlayerInput {
        sequence: 2,
        delta_seconds: 0.05,
        direction: Vec3Net::new(0.0, 0.0, 1.0),
        sprint: true,
        jump: false,
        yaw: 0.0,
        pitch: 0.0,
    });
    controller.simulate(0.05, &test_world());

    assert!(controller.position.y > 0.0);
    assert!(!controller.grounded);
}

#[test]
fn reconciliation_keeps_local_prediction_until_snap_threshold() {
    let mut controller = PlayerController::spawn();
    controller.position = Vec3Net::new(0.6, 0.0, 0.0);
    controller.velocity = Vec3Net::new(5.0, 0.0, 0.0);

    let mut server = PlayerState {
        client_id: 1,
        steam_id: 1,
        name: "Player".to_owned(),
        position: Vec3Net::ZERO,
        velocity: Vec3Net::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        health: MAX_HEALTH,
        grounded: true,
        last_processed_input: 1,
        is_admin: false,
    };

    assert_eq!(controller.reconcile(&server), Reconciliation::Accepted);
    assert_eq!(controller.position, Vec3Net::new(0.6, 0.0, 0.0));
    assert_eq!(controller.velocity, Vec3Net::new(5.0, 0.0, 0.0));

    server.position = Vec3Net::new(2.0, 0.0, 0.0);
    assert_eq!(controller.reconcile(&server), Reconciliation::Snap);
    assert_eq!(controller.position, server.position);
}
