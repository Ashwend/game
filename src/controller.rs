mod collision;

use crate::{
    protocol::{MAX_HEALTH, PlayerInput, PlayerState, Vec3Net},
    world::WorldData,
};

use self::collision::{
    Axis, is_supported, move_with_collisions, player_overlaps_world, support_height_between,
};

pub const WALK_SPEED: f32 = 5.2;
pub const SPRINT_SPEED: f32 = 8.4;
pub const MAX_LOOK_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
const SIDE_WALK_SPEED: f32 = 4.4;
const SPRINT_STRAFE_SPEED: f32 = 5.7;
const BACKPEDAL_SPEED: f32 = 3.8;
const GROUND_ACCELERATION: f32 = 68.0;
const GROUND_DECELERATION: f32 = 76.0;
const AIR_ACCELERATION: f32 = 13.0;
const AIR_MAX_HORIZONTAL_SPEED: f32 = 12.0;
const GRAVITY: f32 = 18.0;
const MAX_FALL_SPEED: f32 = 32.0;
const JUMP_SPEED: f32 = 6.8;
const PLAYER_RADIUS: f32 = 0.35;
const PLAYER_HEIGHT: f32 = 1.8;
const STEP_HEIGHT: f32 = 0.45;
const STEP_VIEW_SMOOTH_SPEED: f32 = 5.5;
const MAX_STEP_VIEW_OFFSET: f32 = 1.0;
const LEAP_FORWARD_INPUT_THRESHOLD: f32 = 0.2;
const LEAP_TAKEOFF_SPEED: f32 = 8.65;
const LEAP_MAX_HORIZONTAL_SPEED: f32 = 8.8;
const JUMP_BUFFER_SECONDS: f32 = 0.18;
const COYOTE_TIME_SECONDS: f32 = 0.1;
const GROUND_EPSILON: f32 = 0.04;
const MAX_SIMULATION_DELTA: f32 = 0.1;
const MAX_SIMULATION_STEP: f32 = 1.0 / 120.0;

#[derive(Debug, Clone)]
pub struct PlayerController {
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub grounded: bool,
    pub last_processed_input: u64,
    last_input: PlayerInput,
    jump_buffer_timer: f32,
    coyote_timer: f32,
    step_view_offset_y: f32,
}

impl PlayerController {
    pub fn spawn() -> Self {
        Self {
            position: Vec3Net::ZERO,
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health: MAX_HEALTH,
            grounded: true,
            last_processed_input: 0,
            last_input: PlayerInput {
                sequence: 0,
                delta_seconds: 0.0,
                direction: Vec3Net::ZERO,
                sprint: false,
                jump: false,
                yaw: 0.0,
                pitch: 0.0,
            },
            jump_buffer_timer: 0.0,
            coyote_timer: COYOTE_TIME_SECONDS,
            step_view_offset_y: 0.0,
        }
    }

    pub fn from_player_state(state: &PlayerState) -> Self {
        let mut controller = Self::spawn();
        controller.position = state.position;
        controller.velocity = state.velocity;
        controller.yaw = state.yaw;
        controller.pitch = state.pitch;
        controller.health = state.health;
        controller.grounded = state.grounded;
        controller.last_processed_input = state.last_processed_input;
        controller.last_input.sequence = state.last_processed_input;
        controller.last_input.yaw = state.yaw;
        controller.last_input.pitch = state.pitch;
        controller
    }

    pub fn apply_input(&mut self, input: PlayerInput) {
        if input.sequence <= self.last_processed_input {
            return;
        }

        self.start_input(input);
        self.last_processed_input = input.sequence;
    }

    pub fn start_authoritative_input(&mut self, input: PlayerInput) {
        if input.sequence <= self.last_processed_input {
            return;
        }

        self.start_input(input);
    }

    pub fn complete_authoritative_input(&mut self, sequence: u64) {
        self.last_processed_input = self.last_processed_input.max(sequence);
    }

    fn start_input(&mut self, mut input: PlayerInput) {
        if input.jump {
            self.jump_buffer_timer = JUMP_BUFFER_SECONDS;
            input.jump = false;
        }

        self.last_input = input;
    }

    pub fn simulate(&mut self, delta_seconds: f32, world: &WorldData) {
        let mut remaining = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, MAX_SIMULATION_DELTA)
        } else {
            0.0
        };

        while remaining > 0.0 {
            let step = remaining.min(MAX_SIMULATION_STEP);
            self.simulate_step(step, world);
            remaining -= step;
        }
    }

    pub fn view_position(&self) -> Vec3Net {
        Vec3Net::new(
            self.position.x,
            self.position.y + self.step_view_offset_y,
            self.position.z,
        )
    }

    fn simulate_step(&mut self, delta_seconds: f32, world: &WorldData) {
        self.yaw = self.last_input.yaw;
        self.pitch = self.last_input.pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
        self.health = self.health.clamp(0.0, MAX_HEALTH);

        self.grounded = is_supported(self.position, world);
        if self.grounded {
            self.coyote_timer = COYOTE_TIME_SECONDS;
        } else {
            self.coyote_timer = (self.coyote_timer - delta_seconds).max(0.0);
        }

        let local_input = clamped_local_move_input(self.last_input.direction);
        let target_velocity = desired_horizontal_velocity(
            self.last_input.direction,
            self.yaw,
            self.last_input.sprint,
        );

        if self.jump_buffer_timer > 0.0 && self.coyote_timer > 0.0 {
            self.velocity.y = JUMP_SPEED;
            self.step_view_offset_y = 0.0;
            self.apply_leap_takeoff(local_input, target_velocity);
            self.grounded = false;
            self.coyote_timer = 0.0;
            self.jump_buffer_timer = 0.0;
        } else {
            self.jump_buffer_timer = (self.jump_buffer_timer - delta_seconds).max(0.0);
        }

        if self.grounded {
            let acceleration = if target_velocity.length_squared() <= f32::EPSILON {
                GROUND_DECELERATION
            } else {
                GROUND_ACCELERATION
            };
            self.velocity =
                approach_horizontal(self.velocity, target_velocity, acceleration * delta_seconds);
        } else {
            self.velocity = accelerate_air(
                self.velocity,
                target_velocity,
                AIR_ACCELERATION * delta_seconds,
            );
        }

        let x_delta = self.velocity.x * delta_seconds;
        self.move_horizontal_with_step(world, Axis::X, x_delta);
        let z_delta = self.velocity.z * delta_seconds;
        self.move_horizontal_with_step(world, Axis::Z, z_delta);

        if self.grounded && !is_supported(self.position, world) {
            self.grounded = false;
        }

        if self.grounded {
            self.velocity.y = self.velocity.y.min(0.0);
        } else {
            self.velocity.y = (self.velocity.y - GRAVITY * delta_seconds).max(-MAX_FALL_SPEED);
        }

        let y_delta = self.velocity.y * delta_seconds;
        let movement = move_with_collisions(
            &mut self.position,
            &mut self.velocity,
            world,
            Axis::Y,
            y_delta,
        );
        self.grounded = movement.landed || is_supported(self.position, world);
        self.step_view_offset_y = approach(
            self.step_view_offset_y,
            0.0,
            STEP_VIEW_SMOOTH_SPEED * delta_seconds,
        );
    }

    fn move_horizontal_with_step(&mut self, world: &WorldData, axis: Axis, delta: f32) {
        if delta == 0.0 {
            return;
        }

        let start_position = self.position;
        let start_velocity = self.velocity;
        let movement =
            move_with_collisions(&mut self.position, &mut self.velocity, world, axis, delta);
        if !movement.collided || !self.grounded || start_velocity.y > 0.0 {
            return;
        }

        if !self.try_step_up(start_position, start_velocity, world, axis, delta)
            && start_position.y > GROUND_EPSILON
            && is_supported(start_position, world)
            && !is_supported(self.position, world)
        {
            self.position = start_position;
            self.velocity = start_velocity;
            match axis {
                Axis::X => self.velocity.x = 0.0,
                Axis::Y => self.velocity.y = 0.0,
                Axis::Z => self.velocity.z = 0.0,
            }
        }
    }

    fn try_step_up(
        &mut self,
        start_position: Vec3Net,
        start_velocity: Vec3Net,
        world: &WorldData,
        axis: Axis,
        delta: f32,
    ) -> bool {
        let mut stepped_position = start_position;
        stepped_position.y += STEP_HEIGHT;
        if player_overlaps_world(stepped_position, world) {
            return false;
        }

        let mut stepped_velocity = start_velocity;
        let horizontal = move_with_collisions(
            &mut stepped_position,
            &mut stepped_velocity,
            world,
            axis,
            delta,
        );
        if horizontal.collided {
            return false;
        }

        let Some(support_y) = support_height_between(
            stepped_position,
            world,
            start_position.y - GROUND_EPSILON,
            start_position.y + STEP_HEIGHT + GROUND_EPSILON,
        ) else {
            return false;
        };
        if support_y + GROUND_EPSILON < start_position.y
            || support_y - start_position.y > STEP_HEIGHT + GROUND_EPSILON
        {
            return false;
        }

        stepped_position.y = support_y;
        if player_overlaps_world(stepped_position, world) {
            return false;
        }

        let step_delta = stepped_position.y - start_position.y;
        self.position = stepped_position;
        self.velocity.x = stepped_velocity.x;
        self.velocity.y = 0.0;
        self.velocity.z = stepped_velocity.z;
        self.grounded = true;
        if step_delta > GROUND_EPSILON {
            self.step_view_offset_y =
                (self.step_view_offset_y - step_delta).max(-MAX_STEP_VIEW_OFFSET);
        }
        true
    }

    fn apply_leap_takeoff(&mut self, local_input: Vec3Net, target_velocity: Vec3Net) {
        if !self.last_input.sprint || local_input.z < LEAP_FORWARD_INPUT_THRESHOLD {
            return;
        }

        let target_speed = horizontal_length(target_velocity);
        if target_speed <= f32::EPSILON {
            return;
        }

        let target_direction = target_velocity.scale(target_speed.recip());
        let current_speed = horizontal_dot(self.velocity, target_direction);
        let takeoff_speed = LEAP_TAKEOFF_SPEED
            .max(target_speed)
            .min(LEAP_MAX_HORIZONTAL_SPEED);
        if current_speed < takeoff_speed {
            let impulse = takeoff_speed - current_speed;
            self.velocity.x += target_direction.x * impulse;
            self.velocity.z += target_direction.z * impulse;
        }
        self.velocity = clamp_horizontal_speed(self.velocity, LEAP_MAX_HORIZONTAL_SPEED);
    }

    pub fn reconcile(&mut self, server: &PlayerState) -> Reconciliation {
        const SNAP_DISTANCE_SQ: f32 = 1.0;

        let server_delta = Vec3Net::new(
            server.position.x - self.position.x,
            server.position.y - self.position.y,
            server.position.z - self.position.z,
        );
        let distance_sq = server_delta.length_squared();

        self.health = server.health;

        if distance_sq > SNAP_DISTANCE_SQ {
            self.position = server.position;
            self.velocity = server.velocity;
            self.grounded = server.grounded;
            self.last_processed_input = self.last_processed_input.max(server.last_processed_input);
            self.step_view_offset_y = 0.0;
            Reconciliation::Snap
        } else {
            Reconciliation::Accepted
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciliation {
    Accepted,
    Snap,
}

pub fn first_person_move_direction(input: Vec3Net, yaw: f32) -> Vec3Net {
    let input = clamped_local_move_input(input).normalize_or_zero();
    if input.length_squared() == 0.0 {
        return Vec3Net::ZERO;
    }

    rotate_local_horizontal(input, yaw).normalize_or_zero()
}

fn desired_horizontal_velocity(input: Vec3Net, yaw: f32, sprint: bool) -> Vec3Net {
    let input = clamped_local_move_input(input);
    if input.length_squared() == 0.0 {
        return Vec3Net::ZERO;
    }

    let forward_speed = if input.z < 0.0 {
        BACKPEDAL_SPEED
    } else if sprint && input.z > 0.0 {
        SPRINT_SPEED
    } else {
        WALK_SPEED
    };
    let side_speed = if sprint && input.z > 0.0 {
        SPRINT_STRAFE_SPEED
    } else {
        SIDE_WALK_SPEED
    };
    let local_velocity = Vec3Net::new(input.x * side_speed, 0.0, input.z * forward_speed);
    rotate_local_horizontal(local_velocity, yaw)
}

fn clamped_local_move_input(input: Vec3Net) -> Vec3Net {
    let input = Vec3Net::new(input.x.clamp(-1.0, 1.0), 0.0, input.z.clamp(-1.0, 1.0));
    if input.length_squared() > 1.0 {
        input.normalize_or_zero()
    } else {
        input
    }
}

fn rotate_local_horizontal(input: Vec3Net, yaw: f32) -> Vec3Net {
    let forward = Vec3Net::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3Net::new(yaw.cos(), 0.0, -yaw.sin());
    right.scale(input.x).plus(forward.scale(input.z))
}

fn approach_horizontal(mut current: Vec3Net, target: Vec3Net, max_delta: f32) -> Vec3Net {
    let difference = Vec3Net::new(target.x - current.x, 0.0, target.z - current.z);
    let distance = horizontal_length(difference);
    if distance <= max_delta || distance <= f32::EPSILON {
        current.x = target.x;
        current.z = target.z;
    } else {
        let scale = max_delta / distance;
        current.x += difference.x * scale;
        current.z += difference.z * scale;
    }
    current
}

fn approach(current: f32, target: f32, max_delta: f32) -> f32 {
    let difference = target - current;
    if difference.abs() <= max_delta {
        target
    } else {
        current + difference.signum() * max_delta
    }
}

fn accelerate_air(mut velocity: Vec3Net, target_velocity: Vec3Net, max_delta: f32) -> Vec3Net {
    let target_speed = horizontal_length(target_velocity);
    if target_speed <= f32::EPSILON {
        return velocity;
    }

    let target_direction = target_velocity.scale(target_speed.recip());
    let current_speed = horizontal_dot(velocity, target_direction);
    let added_speed = target_speed - current_speed;
    if added_speed <= 0.0 {
        return velocity;
    }

    let acceleration = max_delta.min(added_speed);
    velocity.x += target_direction.x * acceleration;
    velocity.z += target_direction.z * acceleration;
    clamp_horizontal_speed(velocity, AIR_MAX_HORIZONTAL_SPEED)
}

fn clamp_horizontal_speed(mut velocity: Vec3Net, max_speed: f32) -> Vec3Net {
    let speed = horizontal_length(velocity);
    if speed > max_speed {
        let scale = max_speed / speed;
        velocity.x *= scale;
        velocity.z *= scale;
    }
    velocity
}

fn horizontal_length(value: Vec3Net) -> f32 {
    horizontal_length_squared(value).sqrt()
}

fn horizontal_length_squared(value: Vec3Net) -> f32 {
    value.x.mul_add(value.x, value.z * value.z)
}

fn horizontal_dot(a: Vec3Net, b: Vec3Net) -> f32 {
    a.x.mul_add(b.x, a.z * b.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WorldBlock;

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
}
