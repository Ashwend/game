use bevy::prelude::*;

use crate::{
    controller::{RUN_SPEED, WALK_SPEED},
    items::ToolKind,
};

const AXE_KICK_PITCH: f32 = 0.010;
const AXE_KICK_DOWN: f32 = 0.005;
const AXE_KICK_DURATION: f32 = 0.08;
const PICKAXE_KICK_PITCH: f32 = 0.038;
const PICKAXE_KICK_DOWN: f32 = 0.024;
const PICKAXE_KICK_DURATION: f32 = 0.18;
// Bare-hand kick: lighter than an axe; you feel the swing but the camera
// barely shifts. Keeps hand harvesting from feeling weightier than tool
// harvesting.
const HANDS_KICK_PITCH: f32 = 0.005;
const HANDS_KICK_DOWN: f32 = 0.002;
const HANDS_KICK_DURATION: f32 = 0.06;

// "I just got hit by a player" reaction. Larger and more downward-biased
// than any swing-side kick — the camera jolts down rather than up, so the
// recipient can tell at a glance whether the wobble was their own swing or
// an incoming hit. Hatchet and pickaxe variants scale with the swinger's
// tool so a pickaxe blow rocks the camera harder than a hatchet jab.
const HIT_RECEIVED_AXE_PITCH: f32 = 0.015;
const HIT_RECEIVED_AXE_DOWN: f32 = 0.045;
const HIT_RECEIVED_AXE_DURATION: f32 = 0.16;
const HIT_RECEIVED_PICKAXE_PITCH: f32 = 0.025;
const HIT_RECEIVED_PICKAXE_DOWN: f32 = 0.075;
const HIT_RECEIVED_PICKAXE_DURATION: f32 = 0.22;

// Head bob: walk-speed cadence is ~2 footsteps/sec, which is one full sine
// cycle per second (a step is half a cycle). BOB_FREQ_CYCLES_PER_METER *
// walk_speed ≈ 1.0 cycle/sec keeps the bob in step with the player's gait.
const BOB_FREQ_CYCLES_PER_METER: f32 = 0.192;
// Peak bob displacement at walk speed. Running scales up linearly until
// `BOB_AMP_SPEED_CAP_FRACTION` of walk speed, then plateaus so very fast
// motion doesn't shake the camera apart.
const BOB_BASE_AMP_METERS: f32 = 0.012;
const BOB_AMP_SPEED_CAP_FRACTION: f32 = 1.5;
const BOB_AMP_LERP_RATE: f32 = 12.0;

// Run FOV: full +RUN_FOV_BOOST_DEG when horizontal speed reaches
// RUN_SPEED, linear ramp from WALK_SPEED upward. The boost is small on
// purpose — enough to register peripherally without warping the geometry.
pub(super) const BASE_FOV_DEG: f32 = 65.0;
pub(super) const RUN_FOV_BOOST_DEG: f32 = 5.0;
const FOV_LERP_RATE: f32 = 8.0;

// Landing dip: half-sine pulse on touchdown. Triggered when the player goes
// from airborne to grounded with a downward velocity below the minimum
// trigger, scaled toward the max amplitude at terminal fall speed.
const LANDING_DIP_TRIGGER_SPEED: f32 = 2.0;
const LANDING_DIP_MAX_FALL_SPEED: f32 = 22.0;
const LANDING_DIP_MAX_METERS: f32 = 0.085;
const LANDING_DIP_DURATION: f32 = 0.22;

#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct CameraMotionEffects {
    bob_phase: f32,
    bob_amp_smooth: f32,
    fov_offset_deg: f32,
    was_grounded: bool,
    prev_fall_speed: f32,
    dip_elapsed: f32,
    dip_amplitude: f32,
}

impl Default for CameraMotionEffects {
    fn default() -> Self {
        Self {
            bob_phase: 0.0,
            bob_amp_smooth: 0.0,
            fov_offset_deg: 0.0,
            // Default to "grounded" so the first frame after a session
            // start doesn't trigger a phantom landing dip.
            was_grounded: true,
            prev_fall_speed: 0.0,
            dip_elapsed: 0.0,
            dip_amplitude: 0.0,
        }
    }
}

impl CameraMotionEffects {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn advance(
        &mut self,
        dt: f32,
        horizontal_speed: f32,
        grounded: bool,
        fall_speed: f32,
    ) {
        let dt = dt.max(0.0);

        let bob_amp_target = if grounded {
            let fraction = (horizontal_speed / WALK_SPEED).min(BOB_AMP_SPEED_CAP_FRACTION);
            fraction * BOB_BASE_AMP_METERS
        } else {
            0.0
        };
        let bob_lerp = (BOB_AMP_LERP_RATE * dt).clamp(0.0, 1.0);
        self.bob_amp_smooth += (bob_amp_target - self.bob_amp_smooth) * bob_lerp;
        if grounded {
            self.bob_phase +=
                horizontal_speed * BOB_FREQ_CYCLES_PER_METER * std::f32::consts::TAU * dt;
            // Keep phase bounded so very long sessions don't lose precision.
            if self.bob_phase > std::f32::consts::TAU * 64.0 {
                self.bob_phase -= std::f32::consts::TAU * 64.0;
            }
        }

        let speed_above_walk = (horizontal_speed - WALK_SPEED).max(0.0);
        let speed_fraction =
            (speed_above_walk / (RUN_SPEED - WALK_SPEED).max(f32::EPSILON)).clamp(0.0, 1.0);
        let fov_target = RUN_FOV_BOOST_DEG * speed_fraction;
        let fov_lerp = (FOV_LERP_RATE * dt).clamp(0.0, 1.0);
        self.fov_offset_deg += (fov_target - self.fov_offset_deg) * fov_lerp;

        // Landing detection: airborne → grounded transition with enough
        // downward speed to be felt. Use the *previous* frame's fall speed
        // because grounding zeroes vy in the simulator.
        if !self.was_grounded && grounded && self.prev_fall_speed >= LANDING_DIP_TRIGGER_SPEED {
            let intensity = ((self.prev_fall_speed - LANDING_DIP_TRIGGER_SPEED)
                / (LANDING_DIP_MAX_FALL_SPEED - LANDING_DIP_TRIGGER_SPEED))
                .clamp(0.0, 1.0);
            self.dip_amplitude = LANDING_DIP_MAX_METERS * (0.35 + 0.65 * intensity); // small but felt even on light landings
            self.dip_elapsed = 0.0;
        }
        if self.dip_amplitude > 0.0 {
            self.dip_elapsed += dt;
            if self.dip_elapsed >= LANDING_DIP_DURATION {
                self.dip_amplitude = 0.0;
                self.dip_elapsed = 0.0;
            }
        }

        // Cache for the next tick's landing detection.
        self.prev_fall_speed = if grounded { 0.0 } else { fall_speed };
        self.was_grounded = grounded;
    }

    pub(super) fn bob_offset_y(&self) -> f32 {
        self.bob_phase.sin() * self.bob_amp_smooth
    }

    pub(super) fn landing_dip_y(&self) -> f32 {
        if self.dip_amplitude <= 0.0 {
            return 0.0;
        }
        let t = (self.dip_elapsed / LANDING_DIP_DURATION).clamp(0.0, 1.0);
        let pulse = (t * std::f32::consts::PI).sin();
        self.dip_amplitude * pulse
    }

    pub(super) fn fov_radians(&self) -> f32 {
        (BASE_FOV_DEG + self.fov_offset_deg).to_radians()
    }
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub(crate) struct CameraImpactKick {
    pitch_magnitude: f32,
    down_magnitude: f32,
    duration: f32,
    elapsed: f32,
}

impl CameraImpactKick {
    pub(crate) fn trigger(&mut self, tool: ToolKind) {
        let (pitch, down, duration) = match tool {
            ToolKind::Hands => (HANDS_KICK_PITCH, HANDS_KICK_DOWN, HANDS_KICK_DURATION),
            ToolKind::Axe => (AXE_KICK_PITCH, AXE_KICK_DOWN, AXE_KICK_DURATION),
            ToolKind::Pickaxe => (PICKAXE_KICK_PITCH, PICKAXE_KICK_DOWN, PICKAXE_KICK_DURATION),
        };
        // If a previous kick is still decaying, take the stronger of the two so
        // rapid hits accumulate rather than stomp on each other.
        self.pitch_magnitude = self.pitch_magnitude.max(pitch);
        self.down_magnitude = self.down_magnitude.max(down);
        self.duration = duration;
        self.elapsed = 0.0;
    }

    /// Trigger the "I just got hit by a player" kick. Distinct profile
    /// from the swing-side kick — sharper, more downward-biased — so the
    /// recipient can tell at a glance whether the wobble was their own
    /// swing or an incoming hit. `attacker_tool` scales the response:
    /// pickaxe blows rock the camera harder than hatchet jabs.
    pub(crate) fn trigger_from_hit(&mut self, attacker_tool: ToolKind) {
        let (pitch, down, duration) = match attacker_tool {
            ToolKind::Pickaxe => (
                HIT_RECEIVED_PICKAXE_PITCH,
                HIT_RECEIVED_PICKAXE_DOWN,
                HIT_RECEIVED_PICKAXE_DURATION,
            ),
            // Axe and any future light-melee tool share the lighter
            // profile. Bare hands shouldn't be reaching here — the
            // server rejects bare-handed PvP — but using the lighter
            // profile keeps the kick proportionate if the path ever
            // surfaces.
            ToolKind::Axe | ToolKind::Hands => (
                HIT_RECEIVED_AXE_PITCH,
                HIT_RECEIVED_AXE_DOWN,
                HIT_RECEIVED_AXE_DURATION,
            ),
        };
        self.pitch_magnitude = self.pitch_magnitude.max(pitch);
        self.down_magnitude = self.down_magnitude.max(down);
        self.duration = duration;
        self.elapsed = 0.0;
    }

    pub(super) fn advance(&mut self, dt: f32) -> (f32, f32) {
        if self.duration <= 0.0 {
            return (0.0, 0.0);
        }
        self.elapsed += dt.max(0.0);
        if self.elapsed >= self.duration {
            self.pitch_magnitude = 0.0;
            self.down_magnitude = 0.0;
            self.duration = 0.0;
            self.elapsed = 0.0;
            return (0.0, 0.0);
        }
        // Half-sine pulse: ramps in fast, settles smoothly.
        let t = self.elapsed / self.duration;
        let pulse = (t * std::f32::consts::PI).sin();
        (self.pitch_magnitude * pulse, self.down_magnitude * pulse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_kick_pulses_and_decays_after_trigger() {
        let mut kick = CameraImpactKick::default();
        assert_eq!(kick.advance(0.1), (0.0, 0.0));

        kick.trigger(ToolKind::Pickaxe);
        let (mid_pitch, mid_drop) = kick.advance(PICKAXE_KICK_DURATION * 0.5);
        assert!(mid_pitch > 0.0);
        assert!(mid_drop > 0.0);

        let (after_pitch, after_drop) = kick.advance(PICKAXE_KICK_DURATION);
        assert_eq!((after_pitch, after_drop), (0.0, 0.0));
    }

    #[test]
    fn motion_effects_reset_to_neutral_state() {
        let mut motion = CameraMotionEffects {
            bob_phase: 1.0,
            bob_amp_smooth: 0.05,
            fov_offset_deg: 3.0,
            was_grounded: false,
            prev_fall_speed: 10.0,
            dip_elapsed: 0.1,
            dip_amplitude: 0.04,
        };

        motion.reset();

        assert_eq!(motion.bob_amp_smooth, 0.0);
        assert_eq!(motion.fov_offset_deg, 0.0);
        assert_eq!(motion.dip_amplitude, 0.0);
        assert!(
            motion.was_grounded,
            "reset should default to grounded so it cannot trigger a phantom dip"
        );
        assert_eq!(motion.bob_offset_y(), 0.0);
        assert_eq!(motion.landing_dip_y(), 0.0);
    }

    #[test]
    fn head_bob_amplitude_scales_with_horizontal_speed_while_grounded() {
        let mut motion = CameraMotionEffects::default();
        // Several short steps so the smoothed amplitude has time to ramp up.
        for _ in 0..40 {
            motion.advance(1.0 / 60.0, WALK_SPEED, true, 0.0);
        }
        let walking_amp = motion.bob_amp_smooth;
        assert!(walking_amp > 0.0);

        let mut running = CameraMotionEffects::default();
        for _ in 0..40 {
            running.advance(1.0 / 60.0, RUN_SPEED, true, 0.0);
        }
        assert!(running.bob_amp_smooth > walking_amp);
    }

    #[test]
    fn head_bob_disengages_in_the_air() {
        let mut motion = CameraMotionEffects::default();
        for _ in 0..40 {
            motion.advance(1.0 / 60.0, WALK_SPEED, true, 0.0);
        }
        let grounded_amp = motion.bob_amp_smooth;
        assert!(grounded_amp > 0.0);

        for _ in 0..40 {
            motion.advance(1.0 / 60.0, WALK_SPEED, false, 0.0);
        }
        assert!(motion.bob_amp_smooth < grounded_amp * 0.1);
    }

    #[test]
    fn run_fov_offset_ramps_up_with_speed() {
        let mut motion = CameraMotionEffects::default();
        for _ in 0..120 {
            motion.advance(1.0 / 60.0, RUN_SPEED, true, 0.0);
        }
        assert!(motion.fov_offset_deg > RUN_FOV_BOOST_DEG * 0.85);

        for _ in 0..120 {
            motion.advance(1.0 / 60.0, WALK_SPEED, true, 0.0);
        }
        assert!(motion.fov_offset_deg < 0.05);
    }

    #[test]
    fn landing_dip_triggers_on_fast_touchdown_and_decays() {
        let mut motion = CameraMotionEffects::default();
        // Airborne with a hard downward velocity.
        motion.advance(1.0 / 60.0, 0.0, false, 12.0);
        // Touchdown.
        motion.advance(1.0 / 60.0, 0.0, true, 0.0);
        let initial_dip = motion.landing_dip_y();
        assert!(initial_dip > 0.0);

        for _ in 0..30 {
            motion.advance(1.0 / 60.0, 0.0, true, 0.0);
        }
        assert_eq!(motion.landing_dip_y(), 0.0);
    }

    #[test]
    fn landing_dip_ignores_gentle_touchdowns() {
        let mut motion = CameraMotionEffects::default();
        motion.advance(1.0 / 60.0, 0.0, false, 0.5);
        motion.advance(1.0 / 60.0, 0.0, true, 0.0);
        assert_eq!(motion.landing_dip_y(), 0.0);
    }

    #[test]
    fn pickaxe_kick_is_heavier_than_axe_kick() {
        let mut axe_kick = CameraImpactKick::default();
        axe_kick.trigger(ToolKind::Axe);
        let (axe_peak, _) = axe_kick.advance(AXE_KICK_DURATION * 0.5);

        let mut pickaxe_kick = CameraImpactKick::default();
        pickaxe_kick.trigger(ToolKind::Pickaxe);
        let (pickaxe_peak, _) = pickaxe_kick.advance(PICKAXE_KICK_DURATION * 0.5);

        assert!(pickaxe_peak > axe_peak);
    }
}
