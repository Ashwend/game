//! Ambient audio: looping background layers that make a world feel alive.
//!
//! Two kinds of ambient sources:
//!
//! - **Zone beds** ([`AmbientBed`]): non-spatial loops keyed off a
//!   [`AmbientZone`]. Crossfade between zones as the player enters /
//!   leaves them — e.g. moving from "Forest" to "Beach" smoothly fades
//!   one bed out and the next in. There is at most one active bed per
//!   zone slot.
//!
//! - **Spatial emitters** ([`AmbientEmitter`]): looping sounds anchored
//!   to a world position (a campfire, a river, a beehive). They fade in
//!   when the player walks into range and fade out when they leave,
//!   sharing the same spatial-falloff vocabulary as the one-shot Sfx3d
//!   impacts.
//!
//! Both kinds use [`AudioFader`] for their transitions, so the audio mix
//! never jumps — the player never hears a snap.
//!
//! Gameplay code drives this layer by:
//! - Writing to [`CurrentAmbientZone`] when the player moves into a new
//!   zone (sets the bed that should be active).
//! - Spawning entities with an [`AmbientEmitter`] component near
//!   point-of-interest props (rivers, campfires, fauna).

use bevy::{
    audio::{AudioSink, AudioSinkPlayback},
    prelude::*,
};

use crate::app::state::{ClientRuntime, ClientSettings};

use super::{
    category::{SoundCategory, category_volume},
    fader::AudioFader,
    library::{SoundLibrary, spawn_managed_loop},
    manifest::{SoundId, sound_defaults},
};

/// Non-spatial ambient zone the player is currently in. Drives which
/// [`AmbientBed`] should be live. `None` = no bed; the active bed (if
/// any) fades out and despawns.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CurrentAmbientZone(pub(crate) Option<AmbientZone>);

/// A logical ambient zone. Add a variant when introducing a new bed
/// (Forest, Beach, Cave, …) and a row in the manifest with the loop.
/// Variants are intentionally pre-declared ahead of actual assets so
/// gameplay code can write `CurrentAmbientZone(Some(ForestDay))` today
/// — the bed-management system will pick up the audio once the
/// manifest is filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub(crate) enum AmbientZone {
    /// Forest at day — birds, wind through trees, distant fauna.
    ForestDay,
    /// Cavern echo. Drips, low rumble.
    Cave,
}

impl AmbientZone {
    /// Which `SoundId` corresponds to this zone's bed loop. Returning
    /// `None` means the bed sound hasn't been authored yet — the
    /// CurrentAmbientZone update treats this as "no bed", same as
    /// `CurrentAmbientZone(None)`.
    pub(crate) fn bed_sound(self) -> Option<SoundId> {
        // Manifest doesn't ship ambient assets yet; this match returns
        // `None` for every zone today. When you drop a forest-day loop
        // under `assets/ambient/`, add the corresponding `SoundId`
        // variant + manifest row, then point this arm at it.
        match self {
            Self::ForestDay => None,
            Self::Cave => None,
        }
    }
}

/// Marker component on the currently-active bed entity. Tags it so the
/// zone-update system can find it without holding a separate handle.
/// `zone` is metadata for tooling/debugging; the `id` is what
/// [`manage_ambient_beds_system`] keys off when deciding whether to keep
/// the bed alive.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct AmbientBed {
    #[allow(dead_code)]
    pub(crate) zone: AmbientZone,
    pub(crate) id: SoundId,
}

/// Spatial looping emitter attached to a world entity. The system
/// fades it up when the player enters `audible_range`, fades it down
/// when they leave. Range is a soft boundary — the loop entity exists
/// at full mix gain whenever within range and gets faded by Bevy's
/// own spatial attenuation on top.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct AmbientEmitter {
    pub(crate) id: SoundId,
    /// Local position the loop should sound from. The system reads this
    /// rather than the entity's [`Transform`] so static emitters can be
    /// declared without a transform component (they spawn their own
    /// audio-entity child with the transform set from this anchor).
    pub(crate) anchor: Vec3,
    /// Player must come within this many world metres for the loop to
    /// be live. Outside this radius, the loop is despawned to save a
    /// rodio decoder slot.
    pub(crate) audible_range: f32,
    /// Fade-in seconds when the player crosses into range; fade-out
    /// matches.
    pub(crate) fade_secs: f32,
    /// Gain offset on top of the manifest's base gain. Negative for
    /// "quiet ambient", positive for "loud landmark" (waterfall).
    pub(crate) gain_offset_db: f32,
    /// Runtime: entity hosting the audio sink, if currently audible.
    /// `None` means out of range / not yet spawned.
    pub(crate) active_sink: Option<Entity>,
}

impl AmbientEmitter {
    /// Construct an emitter with sensible defaults — no gain offset,
    /// audible range matching the impact-cue full-volume radius, a
    /// half-second fade. Used by gameplay code spawning a campfire or
    /// river ambient at a known world point.
    #[allow(dead_code)]
    pub(crate) fn new(id: SoundId, anchor: Vec3, audible_range: f32) -> Self {
        Self {
            id,
            anchor,
            audible_range,
            fade_secs: 0.5,
            gain_offset_db: 0.0,
            active_sink: None,
        }
    }
}

/// Drive [`AmbientBed`] entities so exactly one bed (matching
/// [`CurrentAmbientZone`]) is playing at full volume; any others fade
/// out and despawn.
pub(crate) fn manage_ambient_beds_system(
    mut commands: Commands,
    zone: Res<CurrentAmbientZone>,
    library: Option<Res<SoundLibrary>>,
    settings: Res<ClientSettings>,
    mut beds: Query<(
        Entity,
        &AmbientBed,
        Option<&mut AudioSink>,
        Option<&AudioFader>,
    )>,
) {
    let Some(library) = library else {
        return;
    };

    let desired = zone.0.and_then(|z| z.bed_sound());

    // Pass 1: find existing beds — fade out any whose ID no longer
    // matches the desired one.
    let mut existing_for_desired: Option<Entity> = None;
    for (entity, bed, sink, fader) in &mut beds {
        if Some(bed.id) == desired {
            existing_for_desired = Some(entity);
            continue;
        }
        if fader.is_some() {
            continue;
        }
        let current = sink
            .as_ref()
            .map(|sink| sink.volume().to_linear())
            .unwrap_or(1.0);
        commands
            .entity(entity)
            .insert(AudioFader::fade_out(current, ambient_fade_secs()));
    }

    // Pass 2: ensure the desired bed exists. Spawn it at 0 gain and
    // attach a fade-in to the manifest target.
    if let Some(id) = desired
        && existing_for_desired.is_none()
        && let Some(zone) = zone.0
    {
        let defaults = sound_defaults(id);
        if let Some(entity) = spawn_managed_loop(
            &mut commands,
            &library,
            &settings,
            id,
            None,
            0.0,
            0.0, // start silent
        ) {
            let target = category_volume(
                SoundCategory::AmbientBed,
                &settings,
                defaults.base_gain_db,
                0.0,
            )
            .to_linear();
            commands
                .entity(entity)
                .insert(AmbientBed { zone, id })
                .insert(AudioFader::new(0.0, target, ambient_fade_secs()));
        }
    }
}

/// Fade [`AmbientEmitter`]s in/out based on listener distance. Owns
/// child audio entities so the emitter component itself can survive
/// outside audible range with zero cost.
pub(crate) fn manage_ambient_emitters_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    library: Option<Res<SoundLibrary>>,
    settings: Res<ClientSettings>,
    mut emitters: Query<(Entity, &mut AmbientEmitter)>,
    sinks: Query<&AudioSink>,
) {
    let Some(library) = library else {
        return;
    };
    let Some(listener) = runtime
        .predicted_local
        .as_ref()
        .map(|player| player.position)
    else {
        return;
    };

    for (_owner, mut emitter) in &mut emitters {
        let dx = listener.x - emitter.anchor.x;
        let dy = listener.y - emitter.anchor.y;
        let dz = listener.z - emitter.anchor.z;
        let distance_sq = dx * dx + dy * dy + dz * dz;
        let in_range = distance_sq <= emitter.audible_range * emitter.audible_range;

        match (in_range, emitter.active_sink) {
            (true, None) => {
                // Just came into range — spawn the loop entity, fade in.
                let defaults = sound_defaults(emitter.id);
                if let Some(entity) = spawn_managed_loop(
                    &mut commands,
                    &library,
                    &settings,
                    emitter.id,
                    Some(emitter.anchor),
                    emitter.gain_offset_db,
                    0.0,
                ) {
                    let target = category_volume(
                        SoundCategory::AmbientEmitter,
                        &settings,
                        defaults.base_gain_db,
                        emitter.gain_offset_db,
                    )
                    .to_linear();
                    commands
                        .entity(entity)
                        .insert(AudioFader::new(0.0, target, emitter.fade_secs));
                    emitter.active_sink = Some(entity);
                }
            }
            (false, Some(entity)) => {
                // Just left range — fade out & forget.
                let current = sinks
                    .get(entity)
                    .map(|sink| sink.volume().to_linear())
                    .unwrap_or(1.0);
                commands
                    .entity(entity)
                    .insert(AudioFader::fade_out(current, emitter.fade_secs));
                emitter.active_sink = None;
            }
            _ => {}
        }
    }
}

/// Cross-fade duration for ambient bed transitions. Long enough to feel
/// like the world is breathing rather than switching channels.
fn ambient_fade_secs() -> f32 {
    2.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambient_emitter_constructs_with_inactive_sink() {
        let emitter = AmbientEmitter::new(SoundId::TreeFall, Vec3::ZERO, 10.0);
        assert!(emitter.active_sink.is_none());
        assert!(emitter.fade_secs > 0.0);
    }

    #[test]
    fn current_ambient_zone_defaults_to_none() {
        let zone = CurrentAmbientZone::default();
        assert!(zone.0.is_none());
    }

    #[test]
    fn ambient_fade_is_long_enough_to_breathe() {
        assert!(ambient_fade_secs() >= 1.0);
    }
}
