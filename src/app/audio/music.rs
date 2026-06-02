//! Menu/world music driven by [`AudioFader`].
//!
//! Music is a long-lived single-source layer, exactly one main menu
//! music entity at a time, never spatial. This file owns the "should the
//! music be playing right now?" decision and the fade-out when the player
//! leaves the menu; the audio mix math itself lives in
//! [`super::category`].

use bevy::{
    audio::{AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackSettings, Volume},
    prelude::*,
};

use crate::app::{embedded_asset_path, state::ClientSettings, state::MenuState};

use super::{
    category::{SoundCategory, category_volume},
    fader::AudioFader,
    manifest::{SoundId, sound_defaults, sound_paths},
};

/// Tagged on the menu-music audio entity so we can find it across
/// frames without holding a separate handle.
#[derive(Component)]
pub(crate) struct MainMenuMusic;

/// Fade-out length when leaving the menu backdrop. Long enough that the
/// transition into the world doesn't feel abrupt, short enough that
/// remaining at the menu briefly doesn't trap the player in an audible
/// "the music is dying" tail.
const MENU_MUSIC_FADE_SECONDS: f32 = 1.0;

type MainMenuMusicQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static mut AudioSink>,
        Option<&'static AudioFader>,
    ),
    With<MainMenuMusic>,
>;

pub(crate) fn main_menu_music_system(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    menu: Res<MenuState>,
    settings: Res<ClientSettings>,
    mut music: MainMenuMusicQuery,
) {
    let id = SoundId::MainMenuMusic;

    if menu.screen.uses_menu_backdrop() {
        if music.is_empty() {
            let paths = sound_paths(id);
            // Music has a single canonical track today; if multiple paths
            // are ever declared, pick the first deterministically.
            let path = paths
                .first()
                .copied()
                .expect("MainMenuMusic must have at least one path declared");
            commands.spawn((
                Name::new("Main Menu Music"),
                MainMenuMusic,
                AudioPlayer::new(asset_server.load(embedded_asset_path(path))),
                PlaybackSettings::LOOP.with_volume(menu_music_volume(&settings)),
            ));
        }

        // Cancel any in-flight fade-out, the player is back on the menu.
        // The sink jumps to current target volume on the next frame.
        for (entity, sink, fader) in &mut music {
            if fader.is_some() {
                commands.entity(entity).remove::<AudioFader>();
            }
            if let Some(mut sink) = sink {
                sink.set_volume(menu_music_volume(&settings));
            }
        }
        return;
    }

    // Not on the menu backdrop, attach a fade-out to any music entity
    // that doesn't already have one. The shared fader system handles the
    // rest (per-frame volume ramp + despawn on completion).
    for (entity, sink, fader) in &mut music {
        if fader.is_some() {
            continue;
        }
        let current = sink
            .as_ref()
            .map(|sink| sink.volume().to_linear())
            .unwrap_or_else(|| menu_music_volume(&settings).to_linear());
        commands
            .entity(entity)
            .insert(AudioFader::fade_out(current, MENU_MUSIC_FADE_SECONDS));
    }
}

fn menu_music_volume(settings: &ClientSettings) -> Volume {
    let defaults = sound_defaults(SoundId::MainMenuMusic);
    category_volume(SoundCategory::Music, settings, defaults.base_gain_db, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_music_volume_scales_with_music_slider() {
        let mut settings = ClientSettings::default();
        let full = menu_music_volume(&settings).to_linear();
        settings.audio.music_volume = 0.5;
        let half = menu_music_volume(&settings).to_linear();
        settings.audio.music_volume = 0.0;
        let muted = menu_music_volume(&settings).to_linear();

        assert!((half - full * 0.5).abs() < 1e-5);
        assert_eq!(muted, 0.0);
    }
}
