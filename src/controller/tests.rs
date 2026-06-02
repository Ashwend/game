use super::*;
use crate::world::WorldBlock;

use super::grid::BlockGrid;
use super::movement::{desired_horizontal_velocity, horizontal_length};

fn test_world() -> WorldData {
    WorldData::test_world()
}

fn input(sequence: u64, direction: Vec3Net, run: bool, jump: bool) -> PlayerInput {
    PlayerInput {
        sequence,
        direction,
        run,
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
fn running_is_forward_weighted_and_sidewalking_is_slower() {
    let forward = desired_horizontal_velocity(Vec3Net::new(0.0, 0.0, 1.0), 0.0, true);
    let side = desired_horizontal_velocity(Vec3Net::new(1.0, 0.0, 0.0), 0.0, true);
    let back = desired_horizontal_velocity(Vec3Net::new(0.0, 0.0, -1.0), 0.0, true);
    let diagonal = desired_horizontal_velocity(Vec3Net::new(1.0, 0.0, 1.0), 0.0, true);

    // Forward run should still be clearly faster than walking strafe.
    // The exact gap shrunk with the run-speed reduction (8.4 → 7.0); 2.0
    // m/s preserves the property "forward is the dominant axis" without
    // pinning to the previous tuning.
    assert!(horizontal_length(forward) > horizontal_length(side) + 2.0);
    assert!(horizontal_length(side) > horizontal_length(back));
    assert!(horizontal_length(diagonal) <= RUN_SPEED);
    assert!(diagonal.x > 0.0);
    assert!(diagonal.z < 0.0);
}

#[test]
fn simulate_integrates_movement_using_the_target_yaw_for_the_whole_frame() {
    let mut controller = PlayerController::spawn();
    controller.apply_input(PlayerInput {
        sequence: 1,
        direction: Vec3Net::new(1.0, 0.0, 0.0),
        run: false,
        jump: false,
        yaw: -std::f32::consts::FRAC_PI_2,
        pitch: 0.0,
    });

    controller.simulate(1.0 / 60.0, &test_world());

    // Right-strafe at yaw = -PI/2 points along +Z. Position and camera yaw must
    // agree at end-of-frame so the rendered camera matches the integrated path.
    assert!(controller.position.z > 0.001);
    assert!(controller.position.x.abs() < 1.0e-4);
    assert!((controller.yaw + std::f32::consts::FRAC_PI_2).abs() < 0.0001);
}

#[test]
fn run_jump_creates_modest_forward_boost() {
    let mut controller = PlayerController::spawn();
    controller.apply_input(input(1, Vec3Net::new(0.0, 0.0, 1.0), true, true));
    controller.simulate(1.0 / 60.0, &test_world());

    assert!(controller.position.y > 0.0);
    assert!(!controller.grounded);
    assert!(horizontal_length(controller.velocity) > RUN_SPEED + 0.1);
    assert!(horizontal_length(controller.velocity) < RUN_SPEED + 0.6);
    assert!(controller.velocity.y > JUMP_SPEED - 0.4);
    assert!(controller.velocity.z < -RUN_SPEED - 0.1);
}

#[test]
fn controller_steps_over_low_obstacles_without_jumping() {
    let world = WorldData {
        floor_size: 12.0,
        blocks: vec![WorldBlock::new(
            Vec3Net::new(0.0, 0.18, -0.95),
            Vec3Net::new(0.6, 0.18, 0.25),
        )],
        resource_nodes: Vec::new(),
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
        resource_nodes: Vec::new(),
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
        resource_nodes: Vec::new(),
    };
    let mut controller = PlayerController::spawn();
    controller.position = Vec3Net::new(1.24, 0.6, 0.55);
    controller.velocity = Vec3Net::new(4.0, 0.0, 0.0);
    controller.grounded = true;
    let grid = BlockGrid::build(&world);
    controller.move_horizontal_with_step(&grid, Axis::X, 0.2);

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
        resource_nodes: Vec::new(),
    };
    let mut position = Vec3Net::new(2.35, 0.0, -6.1762643);
    let mut velocity = Vec3Net::new(0.0, 0.0, -5.0);
    let grid = BlockGrid::build(&world);

    let result = move_with_collisions(&mut position, &mut velocity, &grid, Axis::Z, -0.0417);

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
        resource_nodes: Vec::new(),
    };
    let mut position = Vec3Net::new(0.5500001, 0.0, -4.85);
    let mut velocity = Vec3Net::new(0.0, 0.0, -0.5666593);
    let grid = BlockGrid::build(&world);

    let result = move_with_collisions(&mut position, &mut velocity, &grid, Axis::Z, -0.0047);

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
        resource_nodes: Vec::new(),
    };
    let mut position = Vec3Net::new(2.35, 0.0, -6.7282076);
    let mut velocity = Vec3Net::new(0.0, 0.0, -4.5498476);
    let grid = BlockGrid::build(&world);

    let result = move_with_collisions(&mut position, &mut velocity, &grid, Axis::Z, -0.0297);

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
        resource_nodes: Vec::new(),
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
        direction: Vec3Net::ZERO,
        run: false,
        jump: true,
        yaw: 0.0,
        pitch: 0.0,
    });
    controller.apply_input(PlayerInput {
        sequence: 2,
        direction: Vec3Net::new(0.0, 0.0, 1.0),
        run: true,
        jump: false,
        yaw: 0.0,
        pitch: 0.0,
    });
    controller.simulate(0.05, &test_world());

    assert!(controller.position.y > 0.0);
    assert!(!controller.grounded);
}

#[test]
fn early_air_press_still_fires_jump_on_landing() {
    // A tap on the very first frame of a jump shouldn't get lost just
    // because the jump arc lasts longer than `JUMP_BUFFER_SECONDS`. The
    // buffer freezes while airborne so the press persists until the
    // player touches down and the jump fires immediately.
    let mut controller = PlayerController::spawn();
    let world = test_world();
    let substep = 1.0 / 120.0; // step substep-by-substep for direct observation

    // First press: takes off the ground.
    controller.apply_input(PlayerInput {
        sequence: 1,
        direction: Vec3Net::ZERO,
        run: false,
        jump: true,
        yaw: 0.0,
        pitch: 0.0,
    });
    controller.simulate(substep, &world);
    assert!(!controller.grounded, "first press should leave the ground");
    // The buffer was consumed by the jump that just fired.
    assert_eq!(controller.jump_buffer_timer, 0.0);

    // Second press, while still going up, well before any landing.
    controller.apply_input(PlayerInput {
        sequence: 2,
        direction: Vec3Net::ZERO,
        run: false,
        jump: true,
        yaw: 0.0,
        pitch: 0.0,
    });
    let buffer_at_air_press = controller.jump_buffer_timer;
    assert!(buffer_at_air_press > 0.0, "press should refill the buffer");

    // Step the rest of the arc, well past `JUMP_BUFFER_SECONDS` of airtime.
    // The buffer must NOT decay while airborne; the OLD behaviour would have
    // chewed it down to zero long before landing.
    let mut saw_rejump = false;
    // Sequence numbers continue from 3 (set during the jump arc above), one per
    // simulated input over the 250-substep airborne stretch.
    for sequence in 4..=253u64 {
        controller.apply_input(PlayerInput {
            sequence,
            direction: Vec3Net::ZERO,
            run: false,
            jump: false,
            yaw: 0.0,
            pitch: 0.0,
        });

        let pre_velocity_y = controller.velocity.y;
        let pre_buffer = controller.jump_buffer_timer;
        controller.simulate(substep, &world);

        // The auto-rejump signal: velocity.y was non-positive (falling or
        // grounded) before the substep, the buffer was full (we still had
        // the stored press), and afterwards velocity.y is sharply positive
        //, the only path to that state is the jump branch firing on a
        // landing-substep, which also zeros the buffer.
        if pre_velocity_y <= 0.0
            && pre_buffer > 0.0
            && controller.velocity.y > JUMP_SPEED * 0.9
            && controller.jump_buffer_timer == 0.0
        {
            saw_rejump = true;
            break;
        }

        // While airborne with no press, the buffer must stay frozen.
        if !controller.grounded {
            assert!(
                (controller.jump_buffer_timer - buffer_at_air_press).abs() < 1e-4,
                "buffer should freeze in air, got {} (expected {})",
                controller.jump_buffer_timer,
                buffer_at_air_press,
            );
        }
    }

    assert!(saw_rejump, "buffered mid-air press should fire on landing");
}

#[test]
fn rapid_tap_bunny_hops_on_every_landing() {
    // Tap-driven bunny-hopping: one fresh `just_pressed` per jump cycle.
    // We simulate that by sending `jump: true` once at the start, releasing
    // for one frame (so the next press is a fresh transition), and
    // tapping again. After 4 s of this rhythm the player should have
    // jumped multiple times without holding Space.
    let mut controller = PlayerController::spawn();
    let world = test_world();
    let dt = 1.0 / 60.0;
    let mut jumps_observed = 0u32;
    let mut was_grounded = controller.grounded;
    let mut tap_phase = true;

    for sequence in 1u64..=240 {
        // Tap-release-tap-release. Each `jump: true` here represents a
        // genuine new keypress from the player's perspective.
        let jump = tap_phase;
        tap_phase = !tap_phase;
        controller.apply_input(PlayerInput {
            sequence,
            direction: Vec3Net::ZERO,
            run: false,
            jump,
            yaw: 0.0,
            pitch: 0.0,
        });
        controller.simulate(dt, &world);

        if was_grounded && !controller.grounded {
            jumps_observed += 1;
        }
        was_grounded = controller.grounded;
    }

    assert!(
        jumps_observed >= 3,
        "expected at least 3 rapid-tap hops in 4 s, got {jumps_observed}",
    );
}

#[test]
fn fresh_press_after_full_landing_jumps_immediately() {
    // Reproduces the user-reported scenario: jump once, wait for the
    // player to fully land and settle on the ground, *then* press Space.
    // The fresh press must register on the first substep of that frame.
    let mut controller = PlayerController::spawn();
    let world = test_world();
    let dt = 1.0 / 60.0;
    let mut sequence: u64 = 0;

    // One jump to get airborne.
    sequence += 1;
    controller.apply_input(PlayerInput {
        sequence,
        direction: Vec3Net::ZERO,
        run: false,
        jump: true,
        yaw: 0.0,
        pitch: 0.0,
    });
    controller.simulate(dt, &world);
    assert!(!controller.grounded, "first press should leave the ground");

    // Wait for the player to land and settle. The arc takes ~0.75 s; 90
    // frames is well past that.
    for _ in 0..90 {
        sequence += 1;
        controller.apply_input(PlayerInput {
            sequence,
            direction: Vec3Net::ZERO,
            run: false,
            jump: false,
            yaw: 0.0,
            pitch: 0.0,
        });
        controller.simulate(dt, &world);
    }

    assert!(
        controller.grounded,
        "player should be settled on the ground"
    );
    assert!(
        controller.position.y.abs() < 0.05,
        "settled player should be near y=0, got {}",
        controller.position.y,
    );

    // Now a fresh press, just like the user described.
    sequence += 1;
    controller.apply_input(PlayerInput {
        sequence,
        direction: Vec3Net::ZERO,
        run: false,
        jump: true,
        yaw: 0.0,
        pitch: 0.0,
    });
    let buffer_after_press = controller.jump_buffer_timer;
    assert!(
        buffer_after_press > 0.0,
        "press should fill the buffer, got {buffer_after_press}",
    );

    let pre_velocity_y = controller.velocity.y;
    controller.simulate(dt, &world);

    assert!(
        !controller.grounded,
        "should be airborne after the press; grounded={}",
        controller.grounded,
    );
    assert!(
        pre_velocity_y <= 0.0 && controller.velocity.y > JUMP_SPEED * 0.9,
        "velocity.y should be ~JUMP_SPEED after jumping, was {pre_velocity_y} → {}",
        controller.velocity.y,
    );
}

#[test]
fn high_framerate_jump_is_not_smothered_by_grounded_clamp() {
    // Repro for the "press Space and nothing happens" bug at high FPS.
    // At ~250 FPS each substep advances the player ~3 cm up after a jump
    //, still inside `GROUND_EPSILON`. The end-of-substep `is_supported`
    // check therefore latches `grounded = true`, and on the *next*
    // substep the grounded velocity clamp must NOT wipe the upward
    // velocity. Pre-fix this happened reliably and the jump silently
    // vanished.
    let mut controller = PlayerController::spawn();
    let world = test_world();
    let dt = 1.0 / 250.0; // simulate a 250 FPS frame

    controller.apply_input(PlayerInput {
        sequence: 1,
        direction: Vec3Net::ZERO,
        run: false,
        jump: true,
        yaw: 0.0,
        pitch: 0.0,
    });
    controller.simulate(dt, &world);

    // After one 4-ms substep, the player should still have most of the
    // upward jump velocity. Even if `grounded` reads true (because we're
    // within GROUND_EPSILON), `velocity.y` must remain positive, the
    // jump survived the grounded clamp.
    assert!(
        controller.velocity.y > JUMP_SPEED * 0.9,
        "high-fps jump should not be wiped, got vy={}",
        controller.velocity.y,
    );

    // Step many more frames; the player should fully clear the ground
    // even though they keep reading `grounded = true` for the first frame
    // or two.
    for sequence in 2u64..=30 {
        controller.apply_input(PlayerInput {
            sequence,
            direction: Vec3Net::ZERO,
            run: false,
            jump: false,
            yaw: 0.0,
            pitch: 0.0,
        });
        controller.simulate(dt, &world);
    }

    assert!(
        controller.position.y > 0.3,
        "player should have climbed well above GROUND_EPSILON, got y={}",
        controller.position.y,
    );
}

#[test]
fn buffer_does_not_auto_fire_without_a_press() {
    // Sanity-check the negative: no Space press at all means no jump,
    // even after the player has been on the ground for a long time.
    let mut controller = PlayerController::spawn();
    let world = test_world();
    let dt = 1.0 / 60.0;

    let mut was_grounded = controller.grounded;
    let mut transitions = 0u32;
    for _ in 0..120 {
        controller.apply_input(PlayerInput {
            sequence: 1,
            direction: Vec3Net::ZERO,
            run: false,
            jump: false,
            yaw: 0.0,
            pitch: 0.0,
        });
        controller.simulate(dt, &world);
        if was_grounded && !controller.grounded {
            transitions += 1;
        }
        was_grounded = controller.grounded;
    }

    assert_eq!(transitions, 0, "no press, no jump");
    assert!(controller.grounded);
}
