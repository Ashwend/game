mod collision;
mod grid;
mod movement;
#[cfg(test)]
mod tests;

use crate::{
    protocol::{MAX_HEALTH, PlayerInput, PlayerState, Vec3Net},
    world::WorldData,
};

use self::collision::{Axis, is_supported, move_with_collisions, support_height_between};
use self::movement::{
    accelerate_air, approach, approach_horizontal, clamp_horizontal_speed,
    clamped_local_move_input, desired_horizontal_velocity, horizontal_dot, horizontal_length,
};

pub use self::collision::block_under_feet;
pub use self::grid::BlockGrid;
pub use self::movement::{RUN_SPEED, WALK_SPEED, first_person_move_direction};

/// Whether a player capsule standing at `position` overlaps any solid block in
/// `grid`. Movement itself is client-authoritative, so the server doesn't
/// simulate against this; it's exposed for the server's spawn-placement check
/// (keep random spawns out of walls, trees, ore, and placed structures). Build
/// `grid` with [`BlockGrid::build_with_extras`] to include node/deployable
/// colliders, matching what the client actually collides with.
pub fn player_overlaps_world(position: Vec3Net, grid: &BlockGrid) -> bool {
    self::collision::player_overlaps_world(position, grid)
}

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
// Leap caps are deliberately just above `RUN_SPEED` so a running jump
// gives a small, satisfying forward boost without becoming a movement
// exploit. Scaled together with the run-speed reduction (was 8.65 / 8.8
// when `RUN_SPEED` was 8.4); the same ~0.25 / ~0.4 m/s deltas above the
// run speed preserve the leap's "modest boost" feel.
const LEAP_TAKEOFF_SPEED: f32 = 7.25;
const LEAP_MAX_HORIZONTAL_SPEED: f32 = 7.4;
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
                direction: Vec3Net::ZERO,
                run: false,
                jump: false,
                yaw: 0.0,
                pitch: 0.0,
            },
            jump_buffer_timer: 0.0,
            coyote_timer: COYOTE_TIME_SECONDS,
            step_view_offset_y: 0.0,
        }
    }

    /// Seed a `PlayerController` from a `PlayerState`. Used at
    /// `ServerMessage::Welcome` time (prediction bootstrap from
    /// `Welcome.local_seed`) and when applying a
    /// `ServerMessage::Correction`. Post-Phase-6 `PlayerState` is
    /// trimmed to exactly the fields prediction needs.
    pub fn from_player_state(state: &PlayerState) -> Self {
        Self::from_fields(
            state.position,
            state.velocity,
            state.yaw,
            state.pitch,
            state.health,
            state.grounded,
            state.last_processed_input,
        )
    }

    pub fn from_persisted(
        position: Vec3Net,
        velocity: Vec3Net,
        yaw: f32,
        pitch: f32,
        health: f32,
        grounded: bool,
        last_processed_input: u64,
    ) -> Self {
        Self::from_fields(
            position,
            velocity,
            yaw,
            pitch,
            health,
            grounded,
            last_processed_input,
        )
    }

    fn from_fields(
        position: Vec3Net,
        velocity: Vec3Net,
        yaw: f32,
        pitch: f32,
        health: f32,
        grounded: bool,
        last_processed_input: u64,
    ) -> Self {
        let mut controller = Self::spawn();
        controller.position = position;
        controller.velocity = velocity;
        controller.yaw = yaw;
        controller.pitch = pitch;
        controller.health = health;
        controller.grounded = grounded;
        controller.last_processed_input = last_processed_input;
        controller.last_input.sequence = last_processed_input;
        controller.last_input.yaw = yaw;
        controller.last_input.pitch = pitch;
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
        // Build the spatial index once per simulate() call. For procedural
        // worlds the substep loop hits the chunk many times, so amortising
        // a single build across all of them is a big win.
        let grid = BlockGrid::build(world);
        self.simulate_with_grid(delta_seconds, &grid);
    }

    /// Same as [`simulate`] but takes a pre-built [`BlockGrid`]. Use this from
    /// hot loops (server tick, client prediction) that already keep a chunk
    /// alongside their world data, avoids rebuilding the chunk every call.
    pub fn simulate_with_grid(&mut self, delta_seconds: f32, grid: &BlockGrid) {
        let mut remaining = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, MAX_SIMULATION_DELTA)
        } else {
            0.0
        };

        self.yaw = normalize_angle(self.last_input.yaw);
        self.pitch = self.last_input.pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);

        while remaining > 0.0 {
            let step = remaining.min(MAX_SIMULATION_STEP);
            self.simulate_step(step, grid);
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

    fn simulate_step(&mut self, delta_seconds: f32, grid: &BlockGrid) {
        self.health = self.health.clamp(0.0, MAX_HEALTH);

        self.grounded = is_supported(self.position, grid);
        if self.grounded {
            self.coyote_timer = COYOTE_TIME_SECONDS;
        } else {
            self.coyote_timer = (self.coyote_timer - delta_seconds).max(0.0);
        }

        let local_input = clamped_local_move_input(self.last_input.direction);
        let target_velocity =
            desired_horizontal_velocity(self.last_input.direction, self.yaw, self.last_input.run);

        if self.jump_buffer_timer > 0.0 && self.coyote_timer > 0.0 {
            self.velocity.y = JUMP_SPEED;
            self.step_view_offset_y = 0.0;
            self.apply_leap_takeoff(local_input, target_velocity);
            self.grounded = false;
            self.coyote_timer = 0.0;
            self.jump_buffer_timer = 0.0;
        } else if self.grounded {
            // Only decay the buffer while the player is on the ground.
            // Freezing it in mid-air means a press anywhere during the jump
            // arc (early or late) persists until landing and fires on the
            // very first substep we touch down, what bunny-hopping needs.
            // The buffer can't accumulate "ghost" jumps across long
            // airtime because it's always reset to 0 the instant a jump
            // fires, and `start_input` only sets it back to the cap when
            // a fresh `just_pressed` Space arrives.
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
        self.move_horizontal_with_step(grid, Axis::X, x_delta);
        let z_delta = self.velocity.z * delta_seconds;
        self.move_horizontal_with_step(grid, Axis::Z, z_delta);

        if self.grounded && !is_supported(self.position, grid) {
            self.grounded = false;
        }

        if self.grounded {
            // Clamp downward velocity to zero (no sinking into the floor) but
            // leave upward velocity alone. At high frame rates a fresh jump
            // can still register `grounded = true` here because the substep
            // only moves the player a few centimetres up, still inside
            // `GROUND_EPSILON`. Replacing the upward `JUMP_SPEED` with 0
            // would silently eat the jump.
            self.velocity.y = self.velocity.y.max(0.0);
        } else {
            self.velocity.y = (self.velocity.y - GRAVITY * delta_seconds).max(-MAX_FALL_SPEED);
        }

        let y_delta = self.velocity.y * delta_seconds;
        let movement = move_with_collisions(
            &mut self.position,
            &mut self.velocity,
            grid,
            Axis::Y,
            y_delta,
        );
        self.grounded = movement.landed || is_supported(self.position, grid);
        self.step_view_offset_y = approach(
            self.step_view_offset_y,
            0.0,
            STEP_VIEW_SMOOTH_SPEED * delta_seconds,
        );
    }

    fn move_horizontal_with_step(&mut self, grid: &BlockGrid, axis: Axis, delta: f32) {
        if delta == 0.0 {
            return;
        }

        let start_position = self.position;
        let start_velocity = self.velocity;
        let movement =
            move_with_collisions(&mut self.position, &mut self.velocity, grid, axis, delta);
        if !movement.collided || !self.grounded || start_velocity.y > 0.0 {
            return;
        }

        if !self.try_step_up(start_position, start_velocity, grid, axis, delta)
            && start_position.y > GROUND_EPSILON
            && is_supported(start_position, grid)
            && !is_supported(self.position, grid)
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
        grid: &BlockGrid,
        axis: Axis,
        delta: f32,
    ) -> bool {
        let mut stepped_position = start_position;
        stepped_position.y += STEP_HEIGHT;
        if player_overlaps_world(stepped_position, grid) {
            return false;
        }

        let mut stepped_velocity = start_velocity;
        let horizontal = move_with_collisions(
            &mut stepped_position,
            &mut stepped_velocity,
            grid,
            axis,
            delta,
        );
        if horizontal.collided {
            return false;
        }

        let Some(support_y) = support_height_between(
            stepped_position,
            grid,
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
        if player_overlaps_world(stepped_position, grid) {
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
        if !self.last_input.run || local_input.z < LEAP_FORWARD_INPUT_THRESHOLD {
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
}

fn normalize_angle(value: f32) -> f32 {
    use std::f32::consts::{PI, TAU};

    (value + PI).rem_euclid(TAU) - PI
}
