mod collision;
mod movement;
#[cfg(test)]
mod tests;

use crate::{
    protocol::{MAX_HEALTH, PlayerInput, PlayerState, Vec3Net},
    world::WorldData,
};

use self::collision::{
    Axis, is_supported, move_with_collisions, player_overlaps_world, support_height_between,
};
use self::movement::{
    accelerate_air, approach, approach_horizontal, clamp_horizontal_speed,
    clamped_local_move_input, desired_horizontal_velocity, horizontal_dot, horizontal_length,
};

pub use self::movement::{SPRINT_SPEED, WALK_SPEED, first_person_move_direction};

pub const MAX_LOOK_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
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
            let accelerating = target_velocity.length_squared() > f32::EPSILON;
            self.velocity =
                approach_horizontal(self.velocity, target_velocity, delta_seconds, accelerating);
        } else {
            self.velocity = accelerate_air(self.velocity, target_velocity, delta_seconds);
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
