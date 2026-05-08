use bevy::{
    audio::{AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackSettings, Volume},
    prelude::*,
};

use super::super::state::MenuState;

const MAIN_MENU_MUSIC_PATH: &str = "main-screen/ambient-music.wav";
const MAIN_MENU_MUSIC_VOLUME_DECIBELS: f32 = -24.0;
const MAIN_MENU_MUSIC_FADE_SECONDS: f32 = 1.0;

#[derive(Component)]
pub(crate) struct MainMenuMusic;

#[derive(Component, Default)]
pub(crate) struct MainMenuMusicFadeOut {
    elapsed_seconds: f32,
}

type MainMenuMusicQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static mut AudioSink>,
        Option<&'static mut MainMenuMusicFadeOut>,
    ),
    With<MainMenuMusic>,
>;

pub(crate) fn main_menu_music_system(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    menu: Res<MenuState>,
    time: Option<Res<Time>>,
    mut music: MainMenuMusicQuery,
) {
    if menu.screen.uses_menu_backdrop() {
        if music.is_empty() {
            commands.spawn((
                Name::new("Main Menu Music"),
                MainMenuMusic,
                AudioPlayer::new(asset_server.load(MAIN_MENU_MUSIC_PATH)),
                PlaybackSettings::LOOP.with_volume(main_menu_music_volume()),
            ));
        }

        for (entity, sink, fade_out) in &mut music {
            if fade_out.is_some() {
                commands.entity(entity).remove::<MainMenuMusicFadeOut>();
            }
            if let Some(mut sink) = sink {
                sink.set_volume(main_menu_music_volume());
            }
        }
        return;
    }

    let delta_seconds = time
        .as_ref()
        .map(|time| time.delta_secs())
        .unwrap_or(1.0 / 60.0)
        .max(0.0);

    for (entity, sink, fade_out) in &mut music {
        let elapsed_seconds = if let Some(mut fade_out) = fade_out {
            fade_out.elapsed_seconds += delta_seconds;
            fade_out.elapsed_seconds
        } else {
            commands.entity(entity).insert(MainMenuMusicFadeOut {
                elapsed_seconds: delta_seconds,
            });
            delta_seconds
        };

        let fade_progress = (elapsed_seconds / MAIN_MENU_MUSIC_FADE_SECONDS).clamp(0.0, 1.0);
        if let Some(mut sink) = sink {
            sink.set_volume(faded_main_menu_music_volume(fade_progress));
        }

        if fade_progress >= 1.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn main_menu_music_volume() -> Volume {
    Volume::Decibels(MAIN_MENU_MUSIC_VOLUME_DECIBELS)
}

fn faded_main_menu_music_volume(fade_progress: f32) -> Volume {
    main_menu_music_volume().fade_towards(Volume::SILENT, fade_progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_menu_music_volume_is_half_the_previous_linear_level() {
        let linear_volume = main_menu_music_volume().to_linear();

        assert!(linear_volume > 0.062);
        assert!(linear_volume < 0.064);
    }

    #[test]
    fn faded_main_menu_music_volume_reaches_silence() {
        let start = main_menu_music_volume().to_linear();
        let halfway = faded_main_menu_music_volume(0.5).to_linear();
        let end = faded_main_menu_music_volume(1.0).to_linear();

        assert!(halfway < start);
        assert!(halfway > end);
        assert_eq!(end, 0.0);
    }
}
