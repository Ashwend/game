//! Central audio pipeline.
//!
//! This module owns everything sound-related on the client:
//!
//! - [`manifest`] declares the catalogue of sounds (`SoundId` + per-sound
//!   mix defaults + asset path globs).
//! - [`library`] loads every clip at startup and exposes the
//!   [`PlaySound`](library::PlaySound) message + the
//!   [`play_sounds_system`](library::play_sounds_system) that drains it.
//! - [`category`] holds the SFX/UI/Music/Ambient slider routing.
//! - [`fader`] is the reusable fade-in/fade-out component.
//! - [`music`] handles long-form background music (menu / world).
//! - [`ambient`] handles zone beds + spatial looping emitters.
//! - [`footsteps`], [`impact`] are the gameplay-side cue producers; they
//!   emit `PlaySound` events instead of spawning audio entities
//!   themselves.
//! - [`surface`] is the shared surface taxonomy used by footsteps and
//!   impacts.
//!
//! Add a new sound:
//! 1. Drop the audio file under `assets/<subdir>/`.
//! 2. Add a [`SoundId`](manifest::SoundId) variant.
//! 3. Wire it up in [`manifest::sound_defaults`] and
//!    [`manifest::sound_paths`].
//! 4. Emit `PlaySound { id: SoundId::Foo, at: Some(world_position), … }`
//!    from gameplay code.

use bevy::prelude::*;

pub(crate) mod ambient;
pub(crate) mod category;
pub(crate) mod fader;
pub(crate) mod footsteps;
pub(crate) mod impact;
pub(crate) mod library;
pub(crate) mod manifest;
pub(crate) mod music;
pub(crate) mod surface;
pub(crate) mod transitions;

// Re-exports kept as the audio module's public surface. Items marked
// `#[allow(unused_imports)]` are part of the API gameplay code consumes
// today (audio bus, music, footsteps, impact) plus the future-facing
// hooks (ambient beds + emitters, fader component, library handle) that
// new sounds and gameplay systems can wire up without reaching past
// this module's boundary. Suppressing the warning is correct here — the
// items are intentionally exported even when nothing inside the binary
// references them yet.
#[allow(unused_imports)]
pub(crate) use ambient::{
    AmbientBed, AmbientEmitter, AmbientZone, CurrentAmbientZone, manage_ambient_beds_system,
    manage_ambient_emitters_system,
};
#[allow(unused_imports)]
pub(crate) use fader::{AudioFader, tick_audio_faders_system};
pub(crate) use footsteps::{FootstepState, play_footsteps_system};
#[allow(unused_imports)]
pub(crate) use impact::{emit_tree_fall_sound, play_impact_sounds_system};
#[allow(unused_imports)]
pub(crate) use library::{PlaySound, SoundLibrary, play_sounds_system, setup_sound_library};
pub(crate) use manifest::SoundId;
#[allow(unused_imports)]
pub(crate) use music::{MainMenuMusic, main_menu_music_system};
pub(crate) use transitions::{ScreenTransitionWatch, play_transition_stingers_system};

/// Bevy plugin wiring up audio resources, events, and startup loaders.
///
/// Add this to the app after `DefaultPlugins` and `EmbeddedAssetsPlugin`
/// — the asset server must exist and the embedded registry must be
/// populated before [`setup_sound_library`] runs.
pub(crate) struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FootstepState>()
            .init_resource::<CurrentAmbientZone>()
            .init_resource::<ScreenTransitionWatch>()
            .add_message::<PlaySound>()
            .add_systems(Startup, setup_sound_library);
    }
}
