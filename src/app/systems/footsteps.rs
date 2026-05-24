use std::collections::HashMap;

use bevy::{
    audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume},
    prelude::*,
};

use super::super::state::ClientRuntime;
use crate::{
    app::{embedded_asset_path, state::ClientSettings},
    controller::{PlayerController, RUN_SPEED, WALK_SPEED, block_under_feet},
    util::variation::pick_variant_index,
    world::BlockKind,
};

/// Surface the player is walking on. Drives both clip selection and a
/// per-material gain offset, since the source recordings sit at very
/// different intrinsic levels. Add a variant + a row in
/// [`MATERIAL_CLIPS`] / [`material_gain_db`] when adding new surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FootstepMaterial {
    Dirt,
    Wood,
    Concrete,
    Sand,
}

impl FootstepMaterial {
    /// Material to fall back to when the surface under the player is
    /// unknown or has no dedicated clip set. Dirt is the most generic of
    /// the recorded surfaces, so it's the safest default.
    pub(crate) const DEFAULT: Self = Self::Dirt;
}

// Per-material clip pools. The first entry of each pair is the material,
// the second is the embedded paths for that material's footstep variants.
// Twelve variants per material is enough that the anti-repeat picker
// can't produce an audible loop at running cadence.
const MATERIAL_CLIPS: &[(FootstepMaterial, &[&str])] = &[
    (
        FootstepMaterial::Dirt,
        &[
            "movement/footstep-dirt-01.wav",
            "movement/footstep-dirt-02.wav",
            "movement/footstep-dirt-03.wav",
            "movement/footstep-dirt-04.wav",
            "movement/footstep-dirt-05.wav",
            "movement/footstep-dirt-06.wav",
            "movement/footstep-dirt-07.wav",
            "movement/footstep-dirt-08.wav",
            "movement/footstep-dirt-09.wav",
            "movement/footstep-dirt-10.wav",
            "movement/footstep-dirt-11.wav",
            "movement/footstep-dirt-12.wav",
        ],
    ),
    (
        FootstepMaterial::Wood,
        &[
            "movement/footstep-wood-01.wav",
            "movement/footstep-wood-02.wav",
            "movement/footstep-wood-03.wav",
            "movement/footstep-wood-04.wav",
            "movement/footstep-wood-05.wav",
            "movement/footstep-wood-06.wav",
            "movement/footstep-wood-07.wav",
            "movement/footstep-wood-08.wav",
            "movement/footstep-wood-09.wav",
            "movement/footstep-wood-10.wav",
            "movement/footstep-wood-11.wav",
            "movement/footstep-wood-12.wav",
        ],
    ),
    (
        FootstepMaterial::Concrete,
        &[
            "movement/footstep-concrete-01.wav",
            "movement/footstep-concrete-02.wav",
            "movement/footstep-concrete-03.wav",
            "movement/footstep-concrete-04.wav",
            "movement/footstep-concrete-05.wav",
            "movement/footstep-concrete-06.wav",
            "movement/footstep-concrete-07.wav",
            "movement/footstep-concrete-08.wav",
            "movement/footstep-concrete-09.wav",
            "movement/footstep-concrete-10.wav",
            "movement/footstep-concrete-11.wav",
            "movement/footstep-concrete-12.wav",
        ],
    ),
    (
        FootstepMaterial::Sand,
        &[
            "movement/footstep-sand-01.wav",
            "movement/footstep-sand-02.wav",
            "movement/footstep-sand-03.wav",
            "movement/footstep-sand-04.wav",
            "movement/footstep-sand-05.wav",
            "movement/footstep-sand-06.wav",
            "movement/footstep-sand-07.wav",
            "movement/footstep-sand-08.wav",
            "movement/footstep-sand-09.wav",
            "movement/footstep-sand-10.wav",
            "movement/footstep-sand-11.wav",
            "movement/footstep-sand-12.wav",
        ],
    ),
];

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

// Per-material gain bakes out the difference in intrinsic loudness across
// the source recordings (dirt and sand were captured very quietly, wood
// loud, concrete in between). Together with the base gain below this lands
// each material's *peak* footstep around -16 dB perceived at run speed.
// Tune individual materials here without re-cutting audio.
const DIRT_GAIN_DB: f32 = 13.0;
const WOOD_GAIN_DB: f32 = -7.0;
const CONCRETE_GAIN_DB: f32 = 3.0;
const SAND_GAIN_DB: f32 = 12.0;

// Material-agnostic base gain, applied after the per-material offset has
// normalized the clips to a common reference level. Footsteps fire many
// times per second once the player is moving, so they need to sit well
// below the per-hit impact cues (which play at ~-10 dB peak). -8 dB here
// puts each material's peak at ~-24 dB at run speed and ~-29 dB at
// walking — clearly subordinate to swings and impacts, but still
// present.
const FOOTSTEP_BASE_VOLUME_DECIBELS: f32 = -8.0;

// Speed scaling: clips play at `MIN_VOLUME_SCALE` of the per-material
// target when at or below `WALK_SPEED`, ramping linearly to full at
// `RUN_SPEED` (the run cap). Heavier footfall at speed feels right.
const MIN_VOLUME_SCALE: f32 = 0.55;

/// Pre-loaded handles for the per-material footstep pools. Loading at
/// startup avoids decoder spin-up on the first step after spawning.
#[derive(Resource, Clone)]
pub(crate) struct FootstepAssets {
    by_material: HashMap<FootstepMaterial, Vec<Handle<AudioSource>>>,
}

impl FootstepAssets {
    /// Look up the clip pool for `material`, falling back to
    /// [`FootstepMaterial::DEFAULT`] if the requested material has no
    /// dedicated set. Returns `None` only if even the default material is
    /// missing — which would be a setup error, not a runtime case.
    fn clips_for(&self, material: FootstepMaterial) -> Option<&[Handle<AudioSource>]> {
        self.by_material
            .get(&material)
            .or_else(|| self.by_material.get(&FootstepMaterial::DEFAULT))
            .map(|v| v.as_slice())
    }
}

pub(crate) fn setup_footstep_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    let mut by_material = HashMap::with_capacity(MATERIAL_CLIPS.len());
    for (material, paths) in MATERIAL_CLIPS {
        let handles = paths
            .iter()
            .map(|path| asset_server.load(embedded_asset_path(path)))
            .collect();
        by_material.insert(*material, handles);
    }
    commands.insert_resource(FootstepAssets { by_material });
}

/// Per-frame state for the distance-triggered footstep system. Tracks the
/// last ground position so we measure *actual* travel rather than
/// integrating velocity (which drifts when the controller resolves
/// collisions or the snapshot snaps the predicted position back).
#[derive(Resource, Default)]
pub(crate) struct FootstepState {
    last_xz: Option<(f32, f32)>,
    accumulated_distance: f32,
    last_clip_index: Option<usize>,
    fire_count: u32,
}

impl FootstepState {
    fn reset(&mut self) {
        self.last_xz = None;
        self.accumulated_distance = 0.0;
    }
}

pub(crate) fn play_footsteps_system(
    mut commands: Commands,
    assets: Res<FootstepAssets>,
    settings: Res<ClientSettings>,
    runtime: Res<ClientRuntime>,
    mut state: ResMut<FootstepState>,
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
        let material = surface_material_under(predicted, runtime.world_grid.as_ref());
        let Some(clips) = assets.clips_for(material) else {
            // Setup error — no clips even for the default material.
            return;
        };
        // ResMut's DerefMut blocks the borrow checker from seeing
        // `fire_count` and `last_clip_index` as disjoint fields when both
        // are passed to one call. Rebinding to a plain `&mut` re-enables
        // the split-borrow rule.
        let state: &mut FootstepState = &mut state;
        let clip = pick_variant_index(
            &mut state.fire_count,
            &mut state.last_clip_index,
            clips.len(),
        );
        let handle = clips[clip].clone();
        let volume = footstep_volume(material, horizontal_speed, &settings);
        commands.spawn((
            Name::new("Footstep"),
            AudioPlayer::new(handle),
            PlaybackSettings::DESPAWN.with_volume(volume),
        ));
    }
}

/// Resolve the material under the player. Looks up the topmost block whose
/// top surface is right under the player's feet — if there is one, its
/// kind picks the material; otherwise the player is on the world floor
/// and we fall back to dirt.
fn surface_material_under(
    predicted: &PlayerController,
    grid: Option<&crate::controller::BlockGrid>,
) -> FootstepMaterial {
    let Some(grid) = grid else {
        return FootstepMaterial::DEFAULT;
    };
    block_under_feet(predicted.position, grid)
        .map(|block| material_for_block_kind(block.kind))
        .unwrap_or(FootstepMaterial::DEFAULT)
}

/// Map a world block's kind to the footstep material we play when the
/// player is standing on top of one. Both `Standard` and `Stone` blocks
/// in the test/training-ground feel like masonry surfaces, so they share
/// the concrete clip set.
fn material_for_block_kind(kind: BlockKind) -> FootstepMaterial {
    match kind {
        BlockKind::Standard | BlockKind::Stone => FootstepMaterial::Concrete,
    }
}

fn material_gain_db(material: FootstepMaterial) -> f32 {
    match material {
        FootstepMaterial::Dirt => DIRT_GAIN_DB,
        FootstepMaterial::Wood => WOOD_GAIN_DB,
        FootstepMaterial::Concrete => CONCRETE_GAIN_DB,
        FootstepMaterial::Sand => SAND_GAIN_DB,
    }
}

fn footstep_volume(
    material: FootstepMaterial,
    horizontal_speed: f32,
    settings: &ClientSettings,
) -> Volume {
    let base = Volume::Decibels(FOOTSTEP_BASE_VOLUME_DECIBELS + material_gain_db(material));
    let speed_t = ((horizontal_speed - WALK_SPEED) / (RUN_SPEED - WALK_SPEED)).clamp(0.0, 1.0);
    let scale = MIN_VOLUME_SCALE + (1.0 - MIN_VOLUME_SCALE) * speed_t;
    let sfx = settings.audio.sfx_volume.clamp(0.0, 1.0);
    Volume::Linear(base.to_linear() * scale * sfx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footstep_volume_scales_between_walk_and_run() {
        let settings = ClientSettings::default();
        let at_walk = footstep_volume(FootstepMaterial::Dirt, WALK_SPEED, &settings).to_linear();
        let at_run = footstep_volume(FootstepMaterial::Dirt, RUN_SPEED, &settings).to_linear();
        let below_walk =
            footstep_volume(FootstepMaterial::Dirt, WALK_SPEED * 0.5, &settings).to_linear();
        let above_run =
            footstep_volume(FootstepMaterial::Dirt, RUN_SPEED * 2.0, &settings).to_linear();

        assert!(at_walk < at_run);
        assert_eq!(below_walk, at_walk, "below walk speed should clamp to walk");
        assert_eq!(above_run, at_run, "above run speed should clamp to run");
    }

    #[test]
    fn footstep_volume_respects_sfx_setting() {
        let mut settings = ClientSettings::default();
        let full = footstep_volume(FootstepMaterial::Dirt, RUN_SPEED, &settings).to_linear();
        settings.audio.sfx_volume = 0.5;
        let half = footstep_volume(FootstepMaterial::Dirt, RUN_SPEED, &settings).to_linear();
        settings.audio.sfx_volume = 0.0;
        let muted = footstep_volume(FootstepMaterial::Dirt, RUN_SPEED, &settings).to_linear();
        assert!((half - full * 0.5).abs() < 1e-5);
        assert_eq!(muted, 0.0);
    }

    #[test]
    fn footstep_volume_differs_per_material() {
        let settings = ClientSettings::default();
        let dirt = footstep_volume(FootstepMaterial::Dirt, RUN_SPEED, &settings).to_linear();
        let wood = footstep_volume(FootstepMaterial::Wood, RUN_SPEED, &settings).to_linear();
        let concrete =
            footstep_volume(FootstepMaterial::Concrete, RUN_SPEED, &settings).to_linear();
        let sand = footstep_volume(FootstepMaterial::Sand, RUN_SPEED, &settings).to_linear();

        // Each material gets its own offset; the gains are tuned so they
        // can never collapse to identical levels.
        assert!(dirt > wood);
        assert!(sand > wood);
        assert!(concrete > wood);
        assert!((dirt - sand).abs() > 0.0);
    }

    #[test]
    fn footstep_state_reset_clears_accumulator() {
        let mut state = FootstepState {
            last_xz: Some((1.0, 2.0)),
            accumulated_distance: 1.2,
            last_clip_index: Some(3),
            fire_count: 7,
        };
        state.reset();
        assert!(state.last_xz.is_none());
        assert_eq!(state.accumulated_distance, 0.0);
        // last_clip_index and fire_count survive on purpose — they keep
        // the variation cadence going across brief pauses (e.g. when the
        // player stops, turns, and starts walking again).
        assert_eq!(state.last_clip_index, Some(3));
        assert_eq!(state.fire_count, 7);
    }

    #[test]
    fn material_default_is_dirt() {
        // The default surface plays the dirt clip set, and `clips_for`
        // falls through to it when a material is missing.
        assert_eq!(FootstepMaterial::DEFAULT, FootstepMaterial::Dirt);
    }

    #[test]
    fn every_material_has_a_clip_pool_declared() {
        let declared: std::collections::HashSet<_> =
            MATERIAL_CLIPS.iter().map(|(m, _)| *m).collect();
        for material in [
            FootstepMaterial::Dirt,
            FootstepMaterial::Wood,
            FootstepMaterial::Concrete,
            FootstepMaterial::Sand,
        ] {
            assert!(
                declared.contains(&material),
                "{material:?} has no clip pool"
            );
        }
        // Inverse direction is enforced by the match in `material_gain_db`
        // — every declared material is one a variant covers.
    }
}
