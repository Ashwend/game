mod collision;

use crate::protocol::{MAX_HEALTH, MAX_STAMINA, PlayerInput, PlayerState, Vec3Net};

use self::collision::{Axis, is_supported, move_with_collisions};

pub const WALK_SPEED: f32 = 5.2;
pub const SPRINT_SPEED: f32 = 8.4;
const GROUND_ACCELERATION: f32 = 52.0;
const AIR_ACCELERATION: f32 = 18.0;
const GRAVITY: f32 = 18.0;
const MAX_FALL_SPEED: f32 = 32.0;
const JUMP_SPEED: f32 = 6.4;
const PLAYER_RADIUS: f32 = 0.35;
const PLAYER_HEIGHT: f32 = 1.8;
pub const JUMP_STAMINA_COST: f32 = 22.0;
const SPRINT_STAMINA_PER_SECOND: f32 = 18.0;
const STAMINA_REGEN_PER_SECOND: f32 = 30.0;
const STAMINA_REGEN_DELAY: f32 = 0.45;
const JUMP_BUFFER_SECONDS: f32 = 0.18;
const COYOTE_TIME_SECONDS: f32 = 0.1;
const GROUND_EPSILON: f32 = 0.04;
const MAX_SIMULATION_DELTA: f32 = 0.1;

#[derive(Debug, Clone)]
pub struct PlayerController {
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub stamina: f32,
    pub grounded: bool,
    pub last_processed_input: u64,
    last_input: PlayerInput,
    stamina_regen_delay: f32,
    jump_buffer_timer: f32,
    coyote_timer: f32,
}

impl PlayerController {
    pub fn spawn() -> Self {
        Self {
            position: Vec3Net::ZERO,
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health: MAX_HEALTH,
            stamina: MAX_STAMINA,
            grounded: true,
            last_processed_input: 0,
            last_input: PlayerInput {
                sequence: 0,
                direction: Vec3Net::ZERO,
                sprint: false,
                jump: false,
                yaw: 0.0,
                pitch: 0.0,
            },
            stamina_regen_delay: 0.0,
            jump_buffer_timer: 0.0,
            coyote_timer: COYOTE_TIME_SECONDS,
        }
    }

    pub fn from_player_state(state: &PlayerState) -> Self {
        let mut controller = Self::spawn();
        controller.position = state.position;
        controller.velocity = state.velocity;
        controller.yaw = state.yaw;
        controller.pitch = state.pitch;
        controller.health = state.health;
        controller.stamina = state.stamina;
        controller.grounded = state.grounded;
        controller.last_processed_input = state.last_processed_input;
        controller.last_input.sequence = state.last_processed_input;
        controller.last_input.yaw = state.yaw;
        controller.last_input.pitch = state.pitch;
        controller
    }

    pub fn apply_input(&mut self, mut input: PlayerInput) {
        if input.sequence < self.last_processed_input {
            return;
        }

        if input.jump {
            self.jump_buffer_timer = JUMP_BUFFER_SECONDS;
            input.jump = false;
        }

        self.last_processed_input = input.sequence;
        self.last_input = input;
    }

    pub fn simulate(&mut self, delta_seconds: f32) {
        let delta_seconds = delta_seconds.clamp(0.0, MAX_SIMULATION_DELTA);

        self.yaw = self.last_input.yaw;
        self.pitch = self.last_input.pitch.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
        self.health = self.health.clamp(0.0, MAX_HEALTH);

        self.grounded = is_supported(self.position);
        if self.grounded {
            self.coyote_timer = COYOTE_TIME_SECONDS;
        } else {
            self.coyote_timer = (self.coyote_timer - delta_seconds).max(0.0);
        }

        let movement_direction = first_person_move_direction(self.last_input.direction, self.yaw);
        let mut spent_stamina = false;

        if self.jump_buffer_timer > 0.0
            && self.coyote_timer > 0.0
            && self.stamina >= JUMP_STAMINA_COST
        {
            self.consume_stamina(JUMP_STAMINA_COST);
            spent_stamina = true;
            self.velocity.y = JUMP_SPEED;
            self.grounded = false;
            self.coyote_timer = 0.0;
            self.jump_buffer_timer = 0.0;
        } else {
            self.jump_buffer_timer = (self.jump_buffer_timer - delta_seconds).max(0.0);
        }

        let wants_sprint = self.last_input.sprint
            && movement_direction.length_squared() > 0.0
            && self.stamina > 0.0;
        let speed = if wants_sprint {
            SPRINT_SPEED
        } else {
            WALK_SPEED
        };

        if wants_sprint {
            self.consume_stamina(SPRINT_STAMINA_PER_SECOND * delta_seconds);
            spent_stamina = true;
        }

        if !spent_stamina {
            self.stamina_regen_delay = (self.stamina_regen_delay - delta_seconds).max(0.0);
            if self.stamina_regen_delay <= 0.0 {
                self.stamina =
                    (self.stamina + STAMINA_REGEN_PER_SECOND * delta_seconds).min(MAX_STAMINA);
            }
        }

        let target_velocity = movement_direction.scale(speed);
        let acceleration = if self.grounded {
            GROUND_ACCELERATION
        } else {
            AIR_ACCELERATION
        };
        let max_delta = acceleration * delta_seconds;
        self.velocity.x = approach(self.velocity.x, target_velocity.x, max_delta);
        self.velocity.z = approach(self.velocity.z, target_velocity.z, max_delta);

        let x_delta = self.velocity.x * delta_seconds;
        move_with_collisions(&mut self.position, &mut self.velocity, Axis::X, x_delta);
        let z_delta = self.velocity.z * delta_seconds;
        move_with_collisions(&mut self.position, &mut self.velocity, Axis::Z, z_delta);

        if self.grounded && !is_supported(self.position) {
            self.grounded = false;
        }

        if self.grounded {
            self.velocity.y = self.velocity.y.min(0.0);
        } else {
            self.velocity.y = (self.velocity.y - GRAVITY * delta_seconds).max(-MAX_FALL_SPEED);
        }

        let y_delta = self.velocity.y * delta_seconds;
        let landed = move_with_collisions(&mut self.position, &mut self.velocity, Axis::Y, y_delta);
        self.grounded = landed || is_supported(self.position);
    }

    pub fn reconcile(&mut self, server: &PlayerState) -> Reconciliation {
        const SNAP_DISTANCE_SQ: f32 = 1.0;
        const SOFT_DISTANCE_SQ: f32 = 0.04;

        let server_delta = Vec3Net::new(
            server.position.x - self.position.x,
            server.position.y - self.position.y,
            server.position.z - self.position.z,
        );
        let distance_sq = server_delta.length_squared();

        self.health = server.health;
        self.stamina = if distance_sq > SNAP_DISTANCE_SQ {
            server.stamina
        } else {
            self.stamina * 0.85 + server.stamina * 0.15
        }
        .clamp(0.0, MAX_STAMINA);

        if distance_sq > SNAP_DISTANCE_SQ {
            self.position = server.position;
            self.velocity = server.velocity;
            self.grounded = server.grounded;
            self.last_processed_input = self.last_processed_input.max(server.last_processed_input);
            Reconciliation::Snap
        } else if distance_sq > SOFT_DISTANCE_SQ {
            self.position = self.position.plus(server_delta.scale(0.15));
            self.velocity = self.velocity.scale(0.9).plus(server.velocity.scale(0.1));
            self.grounded = self.grounded || server.grounded;
            Reconciliation::SoftCorrect
        } else {
            Reconciliation::Accepted
        }
    }

    fn consume_stamina(&mut self, amount: f32) {
        self.stamina = (self.stamina - amount).max(0.0);
        self.stamina_regen_delay = STAMINA_REGEN_DELAY;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciliation {
    Accepted,
    SoftCorrect,
    Snap,
}

pub fn first_person_move_direction(input: Vec3Net, yaw: f32) -> Vec3Net {
    let input = Vec3Net::new(input.x, 0.0, input.z).normalize_or_zero();
    if input.length_squared() == 0.0 {
        return Vec3Net::ZERO;
    }

    let forward = Vec3Net::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3Net::new(yaw.cos(), 0.0, -yaw.sin());
    right
        .scale(input.x)
        .plus(forward.scale(input.z))
        .normalize_or_zero()
}

fn approach(current: f32, target: f32, max_delta: f32) -> f32 {
    let difference = target - current;
    if difference.abs() <= max_delta {
        target
    } else {
        current + difference.signum() * max_delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn jump_request_survives_following_non_jump_input_before_tick() {
        let mut controller = PlayerController::spawn();
        controller.apply_input(PlayerInput {
            sequence: 1,
            direction: Vec3Net::ZERO,
            sprint: false,
            jump: true,
            yaw: 0.0,
            pitch: 0.0,
        });
        controller.apply_input(PlayerInput {
            sequence: 2,
            direction: Vec3Net::new(0.0, 0.0, 1.0),
            sprint: true,
            jump: false,
            yaw: 0.0,
            pitch: 0.0,
        });
        controller.simulate(0.05);

        assert!(controller.position.y > 0.0);
        assert!(!controller.grounded);
    }

    #[test]
    fn sprint_does_not_spend_stamina_before_jump_request() {
        let mut controller = PlayerController::spawn();
        controller.stamina = JUMP_STAMINA_COST + 0.1;
        controller.apply_input(PlayerInput {
            sequence: 1,
            direction: Vec3Net::new(0.0, 0.0, 1.0),
            sprint: true,
            jump: true,
            yaw: 0.0,
            pitch: 0.0,
        });
        controller.simulate(0.05);

        assert!(controller.position.y > 0.0);
        assert!(controller.stamina <= 0.1);
    }
}
