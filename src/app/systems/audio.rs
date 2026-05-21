use bevy::{
    audio::{
        AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings, SpatialScale,
        Volume,
    },
    prelude::*,
};

use super::super::state::{
    ClientSettings, GatherInputState, ImpactEffectKind, MenuState, RemoteImpactEvent,
};

const MAIN_MENU_MUSIC_PATH: &str = "main-screen/ambient-music.wav";
const MAIN_MENU_MUSIC_VOLUME_DECIBELS: f32 = -24.0;
const MAIN_MENU_MUSIC_FADE_SECONDS: f32 = 1.0;

const HATCHET_TREE_SOUND_PATH: &str = "items/hatchet-tree.mp3";
const PICKAXE_ORE_SOUND_PATH: &str = "items/pickaxe-ore-node.mp3";

// Per-hit impact sounds are short, sharp transients — playing them anywhere
// near the recorded peak would clip and drown out everything else. -10 dB
// keeps them present without dominating the mix; the SFX slider scales from
// there.
const IMPACT_SOUND_VOLUME_DECIBELS: f32 = -10.0;

// Rodio attenuates spatial sources as `gain = (1 / scaled_distance²).min(1.0)`,
// which means anything beyond `1 / SPATIAL_SCALE` world units starts dropping
// off and the falloff is steep (1/d²). With the default scale of 1.0 the
// listener (at eye height) is already past the threshold for an impact
// anchored at the ore/tree base, so even a melee-range hit sounds quiet and
// volume snaps up dramatically the moment you step inside 1 m. Scaling
// positions down extends the full-volume zone — at 0.06 the cap holds out
// to ~16 m and impacts at the far edge of a 30 m playspace still come
// through at ~30% gain, which gives a natural "fair range" falloff without
// the on/off cliff.
const IMPACT_SOUND_SPATIAL_SCALE: f32 = 0.06;
// Eye is ~1.6 m above the tree/ore anchor. Lifting the sound source about a
// meter off the ground puts it closer to where the player is actually
// looking when swinging, which makes the local hit feel "at the point of
// contact" instead of "down by your feet".
const IMPACT_SOUND_HEIGHT_OFFSET: f32 = 1.0;

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
    settings: Res<ClientSettings>,
    time: Option<Res<Time>>,
    mut music: MainMenuMusicQuery,
) {
    if menu.screen.uses_menu_backdrop() {
        if music.is_empty() {
            commands.spawn((
                Name::new("Main Menu Music"),
                MainMenuMusic,
                AudioPlayer::new(asset_server.load(MAIN_MENU_MUSIC_PATH)),
                PlaybackSettings::LOOP.with_volume(main_menu_music_volume(&settings)),
            ));
        }

        for (entity, sink, fade_out) in &mut music {
            if fade_out.is_some() {
                commands.entity(entity).remove::<MainMenuMusicFadeOut>();
            }
            if let Some(mut sink) = sink {
                sink.set_volume(main_menu_music_volume(&settings));
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
            sink.set_volume(faded_main_menu_music_volume(fade_progress, &settings));
        }

        if fade_progress >= 1.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn main_menu_music_volume(settings: &ClientSettings) -> Volume {
    let base = Volume::Decibels(MAIN_MENU_MUSIC_VOLUME_DECIBELS);
    Volume::Linear(base.to_linear() * settings.audio.music_volume.clamp(0.0, 1.0))
}

/// Pre-loaded mp3 handles for the per-hit impact cues. Loading once at
/// startup avoids the first-impact stutter that an on-demand
/// `asset_server.load` would cause inside the gameplay loop.
#[derive(Resource, Clone)]
pub(crate) struct ImpactSoundAssets {
    pub(crate) hatchet_tree: Handle<AudioSource>,
    pub(crate) pickaxe_ore: Handle<AudioSource>,
}

pub(crate) fn setup_impact_sound_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ImpactSoundAssets {
        hatchet_tree: asset_server.load(HATCHET_TREE_SOUND_PATH),
        pickaxe_ore: asset_server.load(PICKAXE_ORE_SOUND_PATH),
    });
}

/// Spawn spatial audio for tree/ore impacts — both the local player's own
/// hits (drained from the audio cue slot, which is queued a few frames ahead
/// of the visual impact so the MP3 attack lines up with the moment the tool
/// lands) and remote players' hits (delivered via `RemoteImpactEvent`).
/// Spatial playback gives natural distance falloff and L/R panning so a tree
/// being chopped to your west sounds west.
pub(crate) fn play_impact_sounds_system(
    mut commands: Commands,
    assets: Res<ImpactSoundAssets>,
    settings: Res<ClientSettings>,
    mut gather_input: ResMut<GatherInputState>,
    mut remote_impacts: MessageReader<RemoteImpactEvent>,
) {
    if let Some(cue) = gather_input.take_pending_audio_cue() {
        spawn_impact_sound(&mut commands, &assets, &settings, cue.anchor, cue.kind);
    }
    for event in remote_impacts.read() {
        spawn_impact_sound(&mut commands, &assets, &settings, event.anchor, event.kind);
    }
}

fn spawn_impact_sound(
    commands: &mut Commands,
    assets: &ImpactSoundAssets,
    settings: &ClientSettings,
    anchor: Vec3,
    kind: ImpactEffectKind,
) {
    let handle = match kind {
        ImpactEffectKind::WoodChips => assets.hatchet_tree.clone(),
        ImpactEffectKind::StoneShards => assets.pickaxe_ore.clone(),
    };
    commands.spawn((
        Name::new("Impact Sound"),
        AudioPlayer::new(handle),
        PlaybackSettings::DESPAWN
            .with_spatial(true)
            .with_spatial_scale(SpatialScale::new(IMPACT_SOUND_SPATIAL_SCALE))
            .with_volume(impact_sound_volume(settings)),
        Transform::from_translation(anchor + Vec3::Y * IMPACT_SOUND_HEIGHT_OFFSET),
    ));
}

fn impact_sound_volume(settings: &ClientSettings) -> Volume {
    let base = Volume::Decibels(IMPACT_SOUND_VOLUME_DECIBELS);
    Volume::Linear(base.to_linear() * settings.audio.sfx_volume.clamp(0.0, 1.0))
}

fn faded_main_menu_music_volume(fade_progress: f32, settings: &ClientSettings) -> Volume {
    main_menu_music_volume(settings).fade_towards(Volume::SILENT, fade_progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_menu_music_volume_is_half_the_previous_linear_level() {
        let settings = ClientSettings::default();
        let linear_volume = main_menu_music_volume(&settings).to_linear();

        assert!(linear_volume > 0.062);
        assert!(linear_volume < 0.064);
    }

    #[test]
    fn faded_main_menu_music_volume_reaches_silence() {
        let settings = ClientSettings::default();
        let start = main_menu_music_volume(&settings).to_linear();
        let halfway = faded_main_menu_music_volume(0.5, &settings).to_linear();
        let end = faded_main_menu_music_volume(1.0, &settings).to_linear();

        assert!(halfway < start);
        assert!(halfway > end);
        assert_eq!(end, 0.0);
    }

    #[test]
    fn impact_sound_volume_scales_with_sfx_setting() {
        let mut settings = ClientSettings::default();
        let full = impact_sound_volume(&settings).to_linear();

        settings.audio.sfx_volume = 0.5;
        let half = impact_sound_volume(&settings).to_linear();

        settings.audio.sfx_volume = 0.0;
        let muted = impact_sound_volume(&settings).to_linear();

        assert!(full > 0.0);
        assert!((half - full * 0.5).abs() < 1e-5);
        assert_eq!(muted, 0.0);
    }
}
