//! Audio mix categories.
//!
//! Every shipped sound is tagged with a [`SoundCategory`]. The category
//! decides which user-facing volume slider scales it, which polyphony cap
//! applies, and (later) which ducking rules apply. Centralising this here
//! means adding a new sound is "pick a category"; adding a new category
//! (e.g. an ambient slider, a voice channel) is one row + one branch.

use bevy::audio::Volume;

use crate::app::state::ClientSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SoundCategory {
    /// Long-form background music (menu, in-game soundtrack).
    Music,
    /// Non-spatial ambient layers (forest day, cave drip). Cross-faded by
    /// zone, sit a few dB under SFX.
    AmbientBed,
    /// Spatial looping ambient emitters (river, campfire, beehive). Use
    /// the same attenuation vocabulary as one-shot Sfx3d sounds.
    AmbientEmitter,
    /// Short, spatial, one-shot world sounds (impacts, tree-fall, future
    /// wildlife). Heard relative to the listener.
    Sfx3d,
    /// Short, non-spatial one-shots (swing whoosh, the local player's own
    /// footsteps, UI confirmations not in the chrome).
    Sfx2d,
    /// Chrome cues, button click/hover, slider tick, dialog open.
    Ui,
}

impl SoundCategory {
    /// Settings-slider gain (0.0 – 1.0) that should scale this category.
    /// Picked off [`ClientSettings::audio`]; future categories add a new
    /// match arm here and nothing else moves.
    pub(crate) fn slider_gain(self, settings: &ClientSettings) -> f32 {
        let raw = match self {
            Self::Music => settings.audio.music_volume,
            Self::AmbientBed | Self::AmbientEmitter | Self::Sfx3d | Self::Sfx2d => {
                settings.audio.sfx_volume
            }
            Self::Ui => settings.audio.ui_volume,
        };
        let master = settings.audio.master_volume.clamp(0.0, 1.0);
        raw.clamp(0.0, 1.0) * master
    }

    /// Maximum number of concurrent one-shot voices in this category.
    /// `None` means "no cap", used for music and ambient layers, where
    /// every active sound is intentional and we never want one to clip out.
    pub(crate) fn polyphony_cap(self) -> Option<usize> {
        match self {
            Self::Music | Self::AmbientBed | Self::AmbientEmitter => None,
            Self::Sfx3d => Some(16),
            Self::Sfx2d => Some(8),
            Self::Ui => Some(6),
        }
    }
}

/// Combine a sound's intrinsic base gain (dB) with the user's slider so the
/// final per-entity `Volume` is one helper call. Linear scaling on top of a
/// dB reference is what every individual `_volume()` helper used to do
/// inline, this is that math, deduplicated.
pub(crate) fn category_volume(
    category: SoundCategory,
    settings: &ClientSettings,
    base_gain_db: f32,
    gain_offset_db: f32,
) -> Volume {
    let base = Volume::Decibels(base_gain_db + gain_offset_db);
    Volume::Linear(base.to_linear() * category.slider_gain(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_gain_clamps_negative_or_overshoot_settings() {
        let mut settings = ClientSettings::default();
        settings.audio.sfx_volume = 2.0;
        assert_eq!(SoundCategory::Sfx2d.slider_gain(&settings), 1.0);
        settings.audio.sfx_volume = -0.5;
        assert_eq!(SoundCategory::Sfx2d.slider_gain(&settings), 0.0);
    }

    #[test]
    fn category_volume_zeroes_at_slider_zero() {
        let mut settings = ClientSettings::default();
        settings.audio.sfx_volume = 0.0;
        assert_eq!(
            category_volume(SoundCategory::Sfx3d, &settings, -10.0, 0.0).to_linear(),
            0.0
        );
    }

    #[test]
    fn category_volume_scales_linearly_with_slider() {
        let mut settings = ClientSettings::default();
        settings.audio.sfx_volume = 1.0;
        let full = category_volume(SoundCategory::Sfx3d, &settings, -10.0, 0.0).to_linear();
        settings.audio.sfx_volume = 0.5;
        let half = category_volume(SoundCategory::Sfx3d, &settings, -10.0, 0.0).to_linear();
        assert!((half - full * 0.5).abs() < 1e-5);
    }

    #[test]
    fn master_volume_scales_every_category() {
        let mut settings = ClientSettings::default();
        settings.audio.master_volume = 0.5;
        // A category at full slider should now read half-gain from master.
        assert!((SoundCategory::Music.slider_gain(&settings) - 0.5).abs() < 1e-6);
        assert!((SoundCategory::Sfx2d.slider_gain(&settings) - 0.5).abs() < 1e-6);
        assert!((SoundCategory::Ui.slider_gain(&settings) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn master_volume_compounds_with_category_slider() {
        let mut settings = ClientSettings::default();
        settings.audio.master_volume = 0.5;
        settings.audio.sfx_volume = 0.5;
        assert!((SoundCategory::Sfx3d.slider_gain(&settings) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn master_volume_clamps_out_of_range() {
        let mut settings = ClientSettings::default();
        settings.audio.master_volume = 4.0;
        assert_eq!(SoundCategory::Music.slider_gain(&settings), 1.0);
        settings.audio.master_volume = -1.0;
        assert_eq!(SoundCategory::Music.slider_gain(&settings), 0.0);
    }

    #[test]
    fn music_polyphony_is_uncapped() {
        assert_eq!(SoundCategory::Music.polyphony_cap(), None);
        assert_eq!(SoundCategory::AmbientBed.polyphony_cap(), None);
    }
}
