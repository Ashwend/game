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
    app::state::ClientRuntime,
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

/// Per-frame state for the distance-triggered footstep system. Tracks
/// the last ground position so we measure *actual* travel rather than
/// integrating velocity (which drifts when the controller resolves
/// collisions or the snapshot snaps the predicted position back).
#[derive(Resource, Default)]
pub(crate) struct FootstepState {
    last_xz: Option<(f32, f32)>,
    accumulated_distance: f32,
}

impl FootstepState {
    fn reset(&mut self) {
        self.last_xz = None;
        self.accumulated_distance = 0.0;
    }
}

pub(crate) fn play_footsteps_system(
    runtime: Res<ClientRuntime>,
    mut state: ResMut<FootstepState>,
    mut play: MessageWriter<PlaySound>,
) {
    let Some(predicted) = runtime.predicted_local.as_ref() else {
        // No local player yet (pre-connect, between worlds). Drop any
        // accumulated travel so the next swing of motion starts cleanly.
        state.reset();
        return;
    };

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

    let velocity = predicted.velocity;
    let horizontal_speed = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt();

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
    fn footstep_state_reset_clears_accumulator() {
        let mut state = FootstepState {
            last_xz: Some((1.0, 2.0)),
            accumulated_distance: 1.2,
        };
        state.reset();
        assert!(state.last_xz.is_none());
        assert_eq!(state.accumulated_distance, 0.0);
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
