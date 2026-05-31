//! Distance-triggered footstep cues. Picks a surface from the block
//! under the player and emits a [`PlaySound`]; the central audio
//! pipeline picks a variant and spawns the entity.
//!
//! The old `systems/footsteps.rs` owned a footstep-specific asset
//! resource, its own picker, its own per-material gain switch, and its
//! own audio-entity spawn. All of that now lives in the manifest +
//! library. This file is just the cadence logic + the surface lookup.

use bevy::prelude::*;

use crate::{
    app::state::{ClientRuntime, LocalPlayerState},
    controller::{PlayerController, RUN_SPEED, WALK_SPEED, block_under_feet},
};

use super::{
    library::PlaySound,
    manifest::footstep_sound_for,
    surface::{SurfaceMaterial, surface_for_block_kind},
};

// Distance the player must travel along the ground between footsteps.
// Tuned against the game's movement speeds (`WALK_SPEED = 5.2 m/s`,
// `RUN_SPEED = 7.0 m/s`) so walking gives ~2.6 Hz cadence and running
// ~3.5 Hz — slow enough to read as deliberate footfalls rather than the
// frantic ~4.7 Hz a tighter interval produced at the previous run-speed
// tuning. Trigger is distance-based so the walk → run transition is
// smooth: accelerating shortens the interval frame by frame, no discrete
// state switch.
const STEP_INTERVAL_DISTANCE: f32 = 2.0;

// Don't fire footsteps while the player is barely moving — keeps a single
// stray sub-pixel of drift from playing a step.
const MIN_HORIZONTAL_SPEED: f32 = 0.5;

// Speed scaling: clips play at `MIN_VOLUME_SCALE` of the per-material
// target when at or below `WALK_SPEED`, ramping linearly to full at
// `RUN_SPEED` (the run cap). Heavier footfall at speed feels right. The
// scale converts to a dB offset by `20 * log10(scale)` so the result
// composes cleanly with the manifest's `base_gain_db`.
const MIN_VOLUME_SCALE: f32 = 0.55;

// Landing footstep: when the player touches down from a jump or fall we play
// one footstep immediately so the landing has weight. The trigger matches the
// camera landing dip (`LANDING_DIP_TRIGGER_SPEED` in camera/effects.rs) so the
// felt dip and the heard step fire together; below it, stepping off a low
// ledge stays silent. The gain ramps from `MIN`→`MAX` dB between the trigger
// speed and `MAX_FALL_SPEED` so a gentle hop lands as a soft step and a long
// drop lands as a firm thud (heavier than a mid-stride footfall).
const LANDING_MIN_FALL_SPEED: f32 = 2.0;
const LANDING_MAX_FALL_SPEED: f32 = 22.0;
const LANDING_MIN_GAIN_DB: f32 = 0.0;
const LANDING_MAX_GAIN_DB: f32 = 5.0;

/// Per-frame state for the distance-triggered footstep system. Tracks
/// the last ground position so we measure *actual* travel rather than
/// integrating velocity (which drifts when the controller resolves
/// collisions or the snapshot snaps the predicted position back). Also
/// tracks the airborne/grounded edge so we can fire a step on touchdown.
#[derive(Resource)]
pub(crate) struct FootstepState {
    last_xz: Option<(f32, f32)>,
    accumulated_distance: f32,
    /// Grounded state last frame — the airborne→grounded edge is the landing.
    was_grounded: bool,
    /// Downward speed last frame. Grounding zeroes vertical velocity in the
    /// simulator, so the landing footstep keys off the previous frame's value
    /// (same reasoning as the camera landing dip).
    prev_fall_speed: f32,
}

impl Default for FootstepState {
    fn default() -> Self {
        Self {
            last_xz: None,
            accumulated_distance: 0.0,
            // Start pinned to the grounded baseline so the first in-game frame
            // never reads a spurious airborne→grounded edge.
            was_grounded: true,
            prev_fall_speed: 0.0,
        }
    }
}

impl FootstepState {
    fn reset(&mut self) {
        // Pin to the grounded baseline (not just zeroed) so a fresh spawn or
        // a between-worlds reconnect never fires a phantom landing thud.
        *self = Self::default();
    }

    /// Update the airborne/grounded tracking and report the fall speed of a
    /// landing footfall if this frame is the moment the player touched down
    /// hard enough to be heard. While dead the state is pinned to the grounded
    /// baseline: the predicted controller keeps falling under gravity during
    /// the death splash, then respawn snaps it to the ground — without this
    /// guard that snap would fire a phantom thud from the pre-death fall.
    fn detect_landing(&mut self, grounded: bool, fall_speed: f32, alive: bool) -> Option<f32> {
        if !alive {
            self.was_grounded = true;
            self.prev_fall_speed = 0.0;
            return None;
        }
        let landed =
            !self.was_grounded && grounded && self.prev_fall_speed >= LANDING_MIN_FALL_SPEED;
        let landing_fall_speed = landed.then_some(self.prev_fall_speed);
        self.was_grounded = grounded;
        self.prev_fall_speed = if grounded { 0.0 } else { fall_speed };
        landing_fall_speed
    }
}

pub(crate) fn play_footsteps_system(
    runtime: Res<ClientRuntime>,
    local_player: Res<LocalPlayerState>,
    mut state: ResMut<FootstepState>,
    mut play: MessageWriter<PlaySound>,
) {
    let Some(predicted) = runtime.predicted_local.as_ref() else {
        // No local player yet (pre-connect, between worlds). Drop any
        // accumulated travel so the next swing of motion starts cleanly.
        state.reset();
        return;
    };

    let velocity = predicted.velocity;
    let horizontal_speed = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt();
    // Positive = falling; clamped to 0 on the way up.
    let fall_speed = (-velocity.y).max(0.0);
    let alive = !matches!(
        local_player.lifecycle,
        Some(crate::server::PlayerLifecycle::Dead { .. })
    );

    // Landing footstep — fired the instant the player touches down, *before*
    // the standing-still / airborne guard below so a straight-up jump still
    // lands audibly even though the player isn't moving horizontally.
    if let Some(landing_fall_speed) = state.detect_landing(predicted.grounded, fall_speed, alive) {
        let surface = surface_material_under(predicted, runtime.world_grid.as_ref());
        let id = footstep_sound_for(surface);
        play.write(PlaySound {
            id,
            at: None,
            gain_offset_db: landing_gain_offset_db(landing_fall_speed),
            jitter_seed: None,
        });
        // Start the next stride clean so the landing thud and the first
        // running step after it don't double up.
        state.accumulated_distance = 0.0;
    }

    let position = predicted.position;
    let current_xz = (position.x, position.z);

    let Some((last_x, last_z)) = state.last_xz else {
        // First tick after (re)spawn — seed last position and skip; we
        // have no previous frame to measure travel against.
        state.last_xz = Some(current_xz);
        return;
    };

    // Always update last_xz before any early return — otherwise a frame
    // spent airborne would make the following ground frame see a huge
    // teleport-sized displacement and fire a burst of steps.
    state.last_xz = Some(current_xz);

    if !predicted.grounded || horizontal_speed < MIN_HORIZONTAL_SPEED {
        // In the air or effectively standing still: hold the accumulator
        // where it is so a brief stop (or a small jump) doesn't reset
        // mid-stride and produce a doubled-up step when motion resumes.
        return;
    }

    let dx = current_xz.0 - last_x;
    let dz = current_xz.1 - last_z;
    state.accumulated_distance += (dx * dx + dz * dz).sqrt();

    while state.accumulated_distance >= STEP_INTERVAL_DISTANCE {
        state.accumulated_distance -= STEP_INTERVAL_DISTANCE;
        let surface = surface_material_under(predicted, runtime.world_grid.as_ref());
        let id = footstep_sound_for(surface);
        play.write(PlaySound {
            id,
            at: None,
            gain_offset_db: speed_gain_offset_db(horizontal_speed),
            jitter_seed: None,
        });
    }
}

/// Convert the linear speed ramp the old footsteps used into a dB offset
/// so the manifest's `base_gain_db` can be the *peak* footfall level and
/// walking cadence sits 5–6 dB below it. This keeps the speed shape
/// continuous with the previous tuning.
fn speed_gain_offset_db(horizontal_speed: f32) -> f32 {
    let t = ((horizontal_speed - WALK_SPEED) / (RUN_SPEED - WALK_SPEED)).clamp(0.0, 1.0);
    let scale = MIN_VOLUME_SCALE + (1.0 - MIN_VOLUME_SCALE) * t;
    // 20 * log10(scale) in dB. Range: 20*log10(0.55) ≈ -5.2 dB to 0 dB.
    20.0 * scale.log10()
}

/// Gain offset for a landing footstep, ramped by how hard the player hit the
/// ground. A landing at the trigger speed plays at the run-footfall level
/// (`LANDING_MIN_GAIN_DB`); a terminal-speed drop adds `LANDING_MAX_GAIN_DB`
/// on top so it reads as a heavy thud rather than a casual step.
fn landing_gain_offset_db(fall_speed: f32) -> f32 {
    let t = ((fall_speed - LANDING_MIN_FALL_SPEED)
        / (LANDING_MAX_FALL_SPEED - LANDING_MIN_FALL_SPEED))
        .clamp(0.0, 1.0);
    LANDING_MIN_GAIN_DB + (LANDING_MAX_GAIN_DB - LANDING_MIN_GAIN_DB) * t
}

/// Resolve the surface under the player. Looks up the topmost block
/// whose top surface is right under the player's feet — if there is
/// one, its kind picks the surface; otherwise the player is on the
/// world floor and we fall back to dirt.
fn surface_material_under(
    predicted: &PlayerController,
    grid: Option<&crate::controller::BlockGrid>,
) -> SurfaceMaterial {
    let Some(grid) = grid else {
        return SurfaceMaterial::DEFAULT;
    };
    block_under_feet(predicted.position, grid)
        .map(|block| surface_for_block_kind(block.kind))
        .unwrap_or(SurfaceMaterial::DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::ClientSettings;
    use bevy::audio::Volume;

    use super::super::category::{SoundCategory, category_volume};
    use super::super::manifest::{SoundId, sound_defaults};

    #[test]
    fn speed_gain_clamps_to_walk_below_walk_speed() {
        let at_walk = speed_gain_offset_db(WALK_SPEED);
        let at_idle = speed_gain_offset_db(0.0);
        let at_jog = speed_gain_offset_db((WALK_SPEED + RUN_SPEED) * 0.5);
        let at_run = speed_gain_offset_db(RUN_SPEED);
        let above_run = speed_gain_offset_db(RUN_SPEED * 2.0);

        // Below walk floors at the walk-speed gain (the minimum scale).
        assert_eq!(at_walk, at_idle);
        // Monotonic ramp up to run speed.
        assert!(at_walk < at_jog);
        assert!(at_jog < at_run);
        // Run-speed peak at 0 dB.
        assert!((at_run - 0.0).abs() < 1e-5);
        // Above run clamps to run.
        assert_eq!(at_run, above_run);
    }

    #[test]
    fn category_volume_for_footsteps_obeys_sfx_slider() {
        // Sanity-check the wiring: the manifest's footstep sounds route
        // through Sfx2d, so the SFX slider must scale them.
        let defaults = sound_defaults(SoundId::FootstepDirt);
        assert_eq!(defaults.category, SoundCategory::Sfx2d);
        let mut settings = ClientSettings::default();
        let full =
            category_volume(defaults.category, &settings, defaults.base_gain_db, 0.0).to_linear();
        settings.audio.sfx_volume = 0.0;
        let muted =
            category_volume(defaults.category, &settings, defaults.base_gain_db, 0.0).to_linear();
        assert!(full > 0.0);
        assert_eq!(muted, 0.0);
    }

    #[test]
    fn footstep_state_reset_clears_accumulator_and_pins_grounded() {
        let mut state = FootstepState {
            last_xz: Some((1.0, 2.0)),
            accumulated_distance: 1.2,
            was_grounded: false,
            prev_fall_speed: 9.0,
        };
        state.reset();
        assert!(state.last_xz.is_none());
        assert_eq!(state.accumulated_distance, 0.0);
        // Reset pins to the grounded baseline so a fresh (re)spawn never reads
        // a spurious airborne→grounded edge.
        assert!(state.was_grounded);
        assert_eq!(state.prev_fall_speed, 0.0);
    }

    #[test]
    fn landing_fires_once_on_touchdown_with_previous_fall_speed() {
        let mut state = FootstepState::default();
        // Standing on the ground: no landing.
        assert!(state.detect_landing(true, 0.0, true).is_none());
        // Jump: airborne, rising then falling — no landing yet.
        assert!(state.detect_landing(false, 0.0, true).is_none());
        assert!(state.detect_landing(false, 8.0, true).is_none());
        // Touch down: fires with the *previous* frame's fall speed (8.0),
        // because grounding has already zeroed vertical velocity this frame.
        let speed = state
            .detect_landing(true, 0.0, true)
            .expect("landing fires on touchdown");
        assert!((speed - 8.0).abs() < 1e-6);
        // Still grounded next frame: no repeat.
        assert!(state.detect_landing(true, 0.0, true).is_none());
    }

    #[test]
    fn gentle_step_off_a_ledge_does_not_thud() {
        let mut state = FootstepState::default();
        state.detect_landing(true, 0.0, true); // grounded
        state.detect_landing(false, 0.0, true); // airborne
        state.detect_landing(false, 1.0, true); // drifting down below trigger
        // Lands under the trigger speed → no footstep.
        assert!(state.detect_landing(true, 0.0, true).is_none());
    }

    #[test]
    fn dead_player_respawn_does_not_fire_a_phantom_landing() {
        let mut state = FootstepState::default();
        state.detect_landing(true, 0.0, true); // grounded, alive
        state.detect_landing(false, 20.0, true); // airborne, falling hard
        // Dies mid-fall: state pins to the grounded baseline, no thud.
        assert!(state.detect_landing(false, 25.0, false).is_none());
        // Respawn snaps to the ground on the first alive frame — must NOT
        // fire a landing from the pre-death fall.
        assert!(state.detect_landing(true, 0.0, true).is_none());
    }

    #[test]
    fn landing_gain_ramps_with_fall_speed_and_clamps() {
        let soft = landing_gain_offset_db(LANDING_MIN_FALL_SPEED);
        let mid = landing_gain_offset_db((LANDING_MIN_FALL_SPEED + LANDING_MAX_FALL_SPEED) * 0.5);
        let hard = landing_gain_offset_db(LANDING_MAX_FALL_SPEED);
        let terminal = landing_gain_offset_db(LANDING_MAX_FALL_SPEED * 2.0);

        assert!(soft < mid);
        assert!(mid < hard);
        // Above the max fall speed the gain clamps rather than running away.
        assert_eq!(hard, terminal);
        assert!((soft - LANDING_MIN_GAIN_DB).abs() < 1e-6);
        assert!((hard - LANDING_MAX_GAIN_DB).abs() < 1e-6);
    }

    #[test]
    fn footsteps_use_volume_helpers_for_master_compat() {
        // Volume helper composition smoke check: footsteps at slider=1.0
        // produce a positive linear gain that reacts to the slider. The
        // per-material `base_gain_db` deliberately sits above 0 dB for the
        // quieter source recordings (dirt is captured ~13 dB below the
        // others); the audio engine clips on its own so we just verify
        // the math composes and scales linearly with the SFX slider.
        let defaults = sound_defaults(SoundId::FootstepDirt);
        let mut settings = ClientSettings::default();
        let peak =
            category_volume(defaults.category, &settings, defaults.base_gain_db, 0.0).to_linear();
        assert!(peak > 0.0);
        settings.audio.sfx_volume = 0.5;
        let half =
            category_volume(defaults.category, &settings, defaults.base_gain_db, 0.0).to_linear();
        assert!((half - peak * 0.5).abs() < 1e-5);
        // Final Volume type smoke check.
        let _ = Volume::Linear(peak);
    }
}
