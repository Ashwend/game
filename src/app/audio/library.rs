//! Central audio asset library + the [`PlaySound`] event + the system
//! that drains the event queue.
//!
//! The previous architecture had each gameplay area (impact, miss,
//! tree-fall, button, footsteps) own its own asset resource, its own
//! picker state, and its own spawn helper. Adding a sound meant touching
//! all three plus the embedded-asset table.
//!
//! With this module:
//! - One [`SoundLibrary`] resource holds the handles for every sound.
//! - One [`PlaySound`] message is what gameplay code emits.
//! - One [`play_sounds_system`] drains the queue, picks a variant,
//!   applies category volume + pitch jitter + polyphony cap, and spawns
//!   the audio entity.

use std::collections::{HashMap, VecDeque};

use bevy::{
    audio::{AudioPlayer, AudioSource, PlaybackSettings, SpatialScale, Volume},
    prelude::*,
};

use crate::{app::embedded_asset_path, app::state::ClientSettings, util::hash::mix32};

use super::{
    category::{SoundCategory, category_volume},
    manifest::{
        SoundDefaults, SoundId, SpatialDefaults, all_sound_ids, sound_defaults, sound_paths,
    },
};

/// Per-variant pool state. Owns the handles and the anti-repeat picker
/// for one [`SoundId`].
#[derive(Debug)]
pub(crate) struct SoundPool {
    handles: Vec<Handle<AudioSource>>,
    fire_count: u32,
    last_index: Option<usize>,
}

impl SoundPool {
    fn new(handles: Vec<Handle<AudioSource>>) -> Self {
        Self {
            handles,
            fire_count: 0,
            last_index: None,
        }
    }

    /// Pick a variant index that doesn't repeat the previous fire. Returns
    /// `None` only when the pool is empty (a setup error).
    fn pick(&mut self) -> Option<&Handle<AudioSource>> {
        if self.handles.is_empty() {
            return None;
        }
        let index = crate::util::variation::pick_variant_index(
            &mut self.fire_count,
            &mut self.last_index,
            self.handles.len(),
        );
        Some(&self.handles[index])
    }
}

/// Pre-loaded handles for every [`SoundId`]. Built once at startup so the
/// audio decoder never spins up on the first hit. The picker state lives
/// inside the pool, so requesting a sound is one map lookup + one pick.
#[derive(Resource)]
pub(crate) struct SoundLibrary {
    pools: HashMap<SoundId, SoundPool>,
    /// Polyphony rings per category — one-shot audio entities recently
    /// spawned, oldest at the front. When a category exceeds its cap the
    /// front is despawned so the new sound replaces it instead of stacking
    /// on top. Categories without a cap (Music, Ambient*) aren't tracked
    /// here at all.
    polyphony: HashMap<SoundCategory, VecDeque<Entity>>,
}

impl SoundLibrary {
    pub(crate) fn defaults_for(&self, id: SoundId) -> SoundDefaults {
        // The lookup never fails: `setup_sound_library` enumerates
        // `all_sound_ids()`, so every variant has a pool entry.
        let _ = self.pools.get(&id);
        sound_defaults(id)
    }
}

pub(crate) fn setup_sound_library(mut commands: Commands, asset_server: Res<AssetServer>) {
    let mut pools = HashMap::with_capacity(all_sound_ids().len());
    for id in all_sound_ids() {
        let handles = sound_paths(*id)
            .iter()
            .map(|path| asset_server.load(embedded_asset_path(path)))
            .collect();
        pools.insert(*id, SoundPool::new(handles));
    }
    commands.insert_resource(SoundLibrary {
        pools,
        polyphony: HashMap::new(),
    });
}

/// Request to play a sound. Any system can write one — the central
/// [`play_sounds_system`] handles asset lookup, volume math, spatial
/// settings, and polyphony.
#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct PlaySound {
    pub(crate) id: SoundId,
    /// World position for spatial sounds. `None` plays non-spatially even
    /// if the manifest declares spatial defaults — useful for "play this
    /// hit at the listener" cases.
    pub(crate) at: Option<Vec3>,
    /// Extra gain on top of [`SoundDefaults::base_gain_db`]. Caller can
    /// emphasise a specific instance (e.g. a powerful hit) without
    /// changing the manifest.
    pub(crate) gain_offset_db: f32,
    /// Seed used for the pitch-jitter PRNG so deterministic events (a
    /// scripted scene) can produce the same speed factor between sessions.
    /// `None` uses a fresh hash of the pool's fire counter.
    pub(crate) jitter_seed: Option<u32>,
}

impl PlaySound {
    pub(crate) const fn non_spatial(id: SoundId) -> Self {
        Self {
            id,
            at: None,
            gain_offset_db: 0.0,
            jitter_seed: None,
        }
    }

    pub(crate) const fn at(id: SoundId, position: Vec3) -> Self {
        Self {
            id,
            at: Some(position),
            gain_offset_db: 0.0,
            jitter_seed: None,
        }
    }
}

pub(crate) fn play_sounds_system(
    mut commands: Commands,
    mut library: ResMut<SoundLibrary>,
    settings: Res<ClientSettings>,
    mut requests: MessageReader<PlaySound>,
) {
    // Sweep any auto-despawned one-shot entities out of the polyphony
    // tracking ring so the cap reflects actual concurrent voices rather
    // than historic peak.
    let library: &mut SoundLibrary = &mut library;
    for ring in library.polyphony.values_mut() {
        ring.retain(|entity| commands.get_entity(*entity).is_ok());
    }

    for request in requests.read() {
        let defaults = sound_defaults(request.id);
        let Some(pool) = library.pools.get_mut(&request.id) else {
            // Shouldn't happen — every SoundId is loaded at startup —
            // but treat as a soft failure rather than panic.
            continue;
        };
        let Some(handle) = pool.pick().cloned() else {
            continue;
        };
        let fire_count = pool.fire_count;

        let entity = spawn_one_shot(
            &mut commands,
            &settings,
            handle,
            defaults,
            request,
            fire_count,
        );

        if let Some(cap) = defaults.category.polyphony_cap() {
            let ring = library.polyphony.entry(defaults.category).or_default();
            ring.push_back(entity);
            while ring.len() > cap {
                if let Some(old) = ring.pop_front()
                    && let Ok(mut ec) = commands.get_entity(old)
                {
                    ec.try_despawn();
                }
            }
        }
    }
}

fn spawn_one_shot(
    commands: &mut Commands,
    settings: &ClientSettings,
    handle: Handle<AudioSource>,
    defaults: SoundDefaults,
    request: &PlaySound,
    fire_count: u32,
) -> Entity {
    let volume = category_volume(
        defaults.category,
        settings,
        defaults.base_gain_db,
        request.gain_offset_db,
    );
    let speed = jittered_speed(
        defaults.pitch_jitter,
        request.jitter_seed.unwrap_or(fire_count),
    );

    let mut playback = if defaults.looped {
        PlaybackSettings::LOOP
    } else {
        PlaybackSettings::DESPAWN
    }
    .with_volume(volume)
    .with_speed(speed);

    if let (Some(anchor), Some(spatial)) = (request.at, defaults.spatial) {
        playback = playback
            .with_spatial(true)
            .with_spatial_scale(SpatialScale::new(spatial.scale));
        spawn_spatial(commands, handle, playback, anchor, spatial, request.id)
    } else {
        spawn_non_spatial(commands, handle, playback, request.id)
    }
}

fn spawn_spatial(
    commands: &mut Commands,
    handle: Handle<AudioSource>,
    playback: PlaybackSettings,
    anchor: Vec3,
    spatial: SpatialDefaults,
    id: SoundId,
) -> Entity {
    commands
        .spawn((
            Name::new(format!("Sound {id:?}")),
            AudioPlayer::new(handle),
            playback,
            Transform::from_translation(anchor + Vec3::Y * spatial.height_offset),
        ))
        .id()
}

fn spawn_non_spatial(
    commands: &mut Commands,
    handle: Handle<AudioSource>,
    playback: PlaybackSettings,
    id: SoundId,
) -> Entity {
    commands
        .spawn((
            Name::new(format!("Sound {id:?}")),
            AudioPlayer::new(handle),
            playback,
        ))
        .id()
}

/// Map a 32-bit seed to a speed factor in `[1 - jitter, 1 + jitter]`.
/// Re-uses [`mix32`] so the sequence is deterministic and well-distributed
/// without pulling in a full RNG.
fn jittered_speed(jitter: f32, seed: u32) -> f32 {
    if jitter <= 0.0 {
        return 1.0;
    }
    let hashed = mix32(seed);
    // Normalise to [-1, 1] then scale.
    let unit = (hashed as f32 / u32::MAX as f32) * 2.0 - 1.0;
    1.0 + unit * jitter
}

/// Public helper for fixed-volume spawns that need to stand outside the
/// normal `PlaySound` pipeline — currently the ambient-emitter system,
/// which owns the loop entity itself so it can fade and despawn it
/// independently. Returns the entity it spawned.
pub(crate) fn spawn_managed_loop(
    commands: &mut Commands,
    library: &SoundLibrary,
    settings: &ClientSettings,
    id: SoundId,
    anchor: Option<Vec3>,
    gain_offset_db: f32,
    starting_volume_scale: f32,
) -> Option<Entity> {
    let defaults = library.defaults_for(id);
    let pool = library.pools.get(&id)?;
    let handle = pool.handles.first()?.clone();
    let volume = {
        let v = category_volume(
            defaults.category,
            settings,
            defaults.base_gain_db,
            gain_offset_db,
        );
        Volume::Linear(v.to_linear() * starting_volume_scale.clamp(0.0, 1.0))
    };
    let mut playback = PlaybackSettings::LOOP.with_volume(volume);
    let entity = if let (Some(anchor), Some(spatial)) = (anchor, defaults.spatial) {
        playback = playback
            .with_spatial(true)
            .with_spatial_scale(SpatialScale::new(spatial.scale));
        commands
            .spawn((
                Name::new(format!("Loop {id:?}")),
                AudioPlayer::new(handle),
                playback,
                Transform::from_translation(anchor + Vec3::Y * spatial.height_offset),
            ))
            .id()
    } else {
        commands
            .spawn((
                Name::new(format!("Loop {id:?}")),
                AudioPlayer::new(handle),
                playback,
            ))
            .id()
    };
    Some(entity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jittered_speed_is_one_when_jitter_is_zero() {
        for seed in [0u32, 1, 42, u32::MAX] {
            assert_eq!(jittered_speed(0.0, seed), 1.0);
        }
    }

    #[test]
    fn jittered_speed_stays_inside_window() {
        let jitter = 0.05;
        for seed in 0..2048 {
            let s = jittered_speed(jitter, seed);
            assert!(s >= 1.0 - jitter - 1e-5, "seed {seed}: speed {s} too low");
            assert!(s <= 1.0 + jitter + 1e-5, "seed {seed}: speed {s} too high");
        }
    }

    #[test]
    fn jittered_speed_is_deterministic_for_a_seed() {
        // Same seed must yield the same speed so scripted/deterministic
        // events reproduce between sessions.
        assert_eq!(jittered_speed(0.1, 12345), jittered_speed(0.1, 12345));
        // Different seeds generally differ.
        assert_ne!(jittered_speed(0.1, 1), jittered_speed(0.1, 2));
    }

    #[test]
    fn empty_pool_picks_nothing() {
        let mut pool = SoundPool::new(Vec::new());
        assert!(pool.pick().is_none(), "empty pool has no variant to pick");
    }

    #[test]
    fn single_variant_pool_always_returns_the_same_handle() {
        let handle = Handle::<AudioSource>::default();
        let mut pool = SoundPool::new(vec![handle.clone()]);
        // A one-element pool can't alternate; it always returns slot 0.
        let first = pool.pick().cloned().expect("pick from non-empty pool");
        let second = pool.pick().cloned().expect("pick again");
        assert_eq!(first, second);
        assert_eq!(first, handle);
        // Each fire advances the pool's fire counter.
        assert_eq!(pool.fire_count, 2);
    }

    #[test]
    fn multi_variant_pool_never_repeats_consecutively() {
        // Five pool slots — identity doesn't matter here; we assert the
        // picker's `last_index` never repeats consecutively.
        let handles: Vec<Handle<AudioSource>> =
            (0..5).map(|_| Handle::<AudioSource>::default()).collect();
        let mut pool = SoundPool::new(handles);
        let mut prev = pool.last_index;
        for _ in 0..50 {
            assert!(pool.pick().is_some());
            let picked = pool.last_index;
            if let (Some(p), Some(prev_idx)) = (picked, prev) {
                assert_ne!(p, prev_idx, "consecutive variant repeat in pool pick");
            }
            prev = picked;
        }
    }

    #[test]
    fn defaults_for_returns_manifest_defaults() {
        // `defaults_for` should mirror `sound_defaults` regardless of the
        // pool map contents. Build a library with no pools and confirm it
        // still resolves the manifest row for a known id.
        let library = SoundLibrary {
            pools: HashMap::new(),
            polyphony: HashMap::new(),
        };
        let id = SoundId::SwingMiss;
        let defaults = library.defaults_for(id);
        assert_eq!(defaults.category, sound_defaults(id).category);
        assert_eq!(defaults.base_gain_db, sound_defaults(id).base_gain_db);
    }
}
