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
use crate::{app::embedded_asset_path, util::variation::pick_variant_index};

// All audio is baked into the binary by `EmbeddedAssetsPlugin`. Paths are
// passed through `embedded_asset_path(...)` so they route to the
// `embedded` asset source instead of the on-disk `assets/` folder — a
// published `Game` binary therefore needs no sibling resources.
const MAIN_MENU_MUSIC_PATH: &str = "main-screen/ambient-music.wav";
const MAIN_MENU_MUSIC_VOLUME_DECIBELS: f32 = -24.0;
const MAIN_MENU_MUSIC_FADE_SECONDS: f32 = 1.0;

// Variant pools per sound. Each fire picks one entry via
// `pick_variant_index`, with consecutive-repeat avoidance so the same
// swing never plays the exact same clip twice in a row. Variants 2 and 3
// are generated from variant 1 with ±5% rate shifts (slight pitch
// difference + slight length difference) — enough variation that a chain
// of hits feels organic without changing the sound's character.
const HATCHET_TREE_SOUND_PATHS: &[&str] = &[
    "items/hatchet-tree-1.wav",
    "items/hatchet-tree-2.wav",
    "items/hatchet-tree-3.wav",
];
const PICKAXE_ORE_SOUND_PATHS: &[&str] = &[
    "items/pickaxe-ore-node-1.wav",
    "items/pickaxe-ore-node-2.wav",
    "items/pickaxe-ore-node-3.wav",
];
const MISS_SOUND_PATHS: &[&str] = &["items/miss-1.wav", "items/miss-2.wav", "items/miss-3.wav"];

// One-shot sound for the tree-felling death animation. Plays at the moment
// the tree starts to tip — the source's audible crash arrives ~0.6s in,
// which lines up with a typical tree's pendulum-fall hitting horizontal.
const TREE_FALL_SOUND_PATH: &str = "world/tree-fall.wav";
// Source clip peaks at 0 dBFS, so anything close to 0 dB on top would clip
// and overpower the rest of the mix. -12 dB lands the crash around the
// same perceived loudness as a tool impact while still feeling like the
// most significant event happening that second.
const TREE_FALL_SOUND_VOLUME_DECIBELS: f32 = -12.0;
// Match the impact sounds' falloff so a tree crashing across the
// playspace attenuates the same way a swing impact does — keeps the
// world's spatial-audio vocabulary consistent.
const TREE_FALL_SOUND_SPATIAL_SCALE: f32 = IMPACT_SOUND_SPATIAL_SCALE;
// Anchor the sound roughly at the trunk's mid-height. The tree's `pivot`
// sits at its base; lifting the source above that puts it closer to the
// player's ear height and away from "down by your feet" perception.
const TREE_FALL_SOUND_HEIGHT_OFFSET: f32 = 1.5;

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
                AudioPlayer::new(asset_server.load(embedded_asset_path(MAIN_MENU_MUSIC_PATH))),
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

/// Pre-loaded variant pools for the per-hit impact cues and the miss
/// whoosh. Loading at startup avoids decoder spin-up on the first hit.
/// Each pool holds 3 slightly-pitched variants of the same recording so
/// repeated swings sound organic instead of identical.
#[derive(Resource, Clone)]
pub(crate) struct ImpactSoundAssets {
    pub(crate) hatchet_tree: Vec<Handle<AudioSource>>,
    pub(crate) pickaxe_ore: Vec<Handle<AudioSource>>,
    pub(crate) miss: Vec<Handle<AudioSource>>,
}

/// Anti-repeat picker state for the variant pools. One slot per sound so
/// hatchet hits, pickaxe hits, and misses each cycle independently — a
/// pickaxe hit doesn't push the hatchet picker forward and vice versa.
#[derive(Resource, Default)]
pub(crate) struct ImpactSoundPicker {
    hatchet_tree_fire_count: u32,
    hatchet_tree_last: Option<usize>,
    pickaxe_ore_fire_count: u32,
    pickaxe_ore_last: Option<usize>,
    miss_fire_count: u32,
    miss_last: Option<usize>,
}

pub(crate) fn setup_impact_sound_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    let load_pool = |paths: &[&str]| -> Vec<Handle<AudioSource>> {
        paths
            .iter()
            .map(|path| asset_server.load(embedded_asset_path(path)))
            .collect()
    };
    commands.insert_resource(ImpactSoundAssets {
        hatchet_tree: load_pool(HATCHET_TREE_SOUND_PATHS),
        pickaxe_ore: load_pool(PICKAXE_ORE_SOUND_PATHS),
        miss: load_pool(MISS_SOUND_PATHS),
    });
    commands.insert_resource(ImpactSoundPicker::default());
}

/// Pre-loaded handle for the tree-fall sound used by the felling-tree
/// death animation. Loading at startup avoids the decoder spinning up
/// the first time a tree falls.
#[derive(Resource, Clone)]
pub(crate) struct TreeFallSoundAsset {
    pub(crate) handle: Handle<AudioSource>,
}

pub(crate) fn setup_tree_fall_sound_asset(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(TreeFallSoundAsset {
        handle: asset_server.load(embedded_asset_path(TREE_FALL_SOUND_PATH)),
    });
}

/// Spawn the tree-fall sound at `anchor` (the tree's base position). The
/// audio entity is independent of the felling tree so playback survives
/// the trunk fading out and despawning when the animation finishes.
pub(crate) fn spawn_tree_fall_sound(
    commands: &mut Commands,
    asset: &TreeFallSoundAsset,
    settings: &ClientSettings,
    anchor: Vec3,
) {
    commands.spawn((
        Name::new("Tree Fall Sound"),
        AudioPlayer::new(asset.handle.clone()),
        PlaybackSettings::DESPAWN
            .with_spatial(true)
            .with_spatial_scale(SpatialScale::new(TREE_FALL_SOUND_SPATIAL_SCALE))
            .with_volume(tree_fall_volume(settings)),
        Transform::from_translation(anchor + Vec3::Y * TREE_FALL_SOUND_HEIGHT_OFFSET),
    ));
}

fn tree_fall_volume(settings: &ClientSettings) -> Volume {
    let base = Volume::Decibels(TREE_FALL_SOUND_VOLUME_DECIBELS);
    Volume::Linear(base.to_linear() * settings.audio.sfx_volume.clamp(0.0, 1.0))
}

/// Spawn spatial audio for tree/ore impacts — both the local player's own
/// hits (drained from the audio cue slot) and remote players' hits
/// (delivered via `RemoteImpactEvent`). Spatial playback gives natural
/// distance falloff and L/R panning so a tree being chopped to your west
/// sounds west.
pub(crate) fn play_impact_sounds_system(
    mut commands: Commands,
    assets: Res<ImpactSoundAssets>,
    settings: Res<ClientSettings>,
    mut picker: ResMut<ImpactSoundPicker>,
    mut gather_input: ResMut<GatherInputState>,
    mut remote_impacts: MessageReader<RemoteImpactEvent>,
) {
    let picker: &mut ImpactSoundPicker = &mut picker;
    if let Some(cue) = gather_input.take_pending_audio_cue() {
        spawn_impact_sound(
            &mut commands,
            &assets,
            picker,
            &settings,
            cue.anchor,
            cue.kind,
        );
    }
    if gather_input.take_pending_miss_audio() {
        spawn_miss_sound(&mut commands, &assets, picker, &settings);
    }
    for event in remote_impacts.read() {
        spawn_impact_sound(
            &mut commands,
            &assets,
            picker,
            &settings,
            event.anchor,
            event.kind,
        );
    }
}

fn spawn_impact_sound(
    commands: &mut Commands,
    assets: &ImpactSoundAssets,
    picker: &mut ImpactSoundPicker,
    settings: &ClientSettings,
    anchor: Vec3,
    kind: ImpactEffectKind,
) {
    let (pool, fire_count, last_index) = match kind {
        ImpactEffectKind::WoodChips => (
            &assets.hatchet_tree,
            &mut picker.hatchet_tree_fire_count,
            &mut picker.hatchet_tree_last,
        ),
        ImpactEffectKind::StoneShards => (
            &assets.pickaxe_ore,
            &mut picker.pickaxe_ore_fire_count,
            &mut picker.pickaxe_ore_last,
        ),
    };
    if pool.is_empty() {
        return;
    }
    let index = pick_variant_index(fire_count, last_index, pool.len());
    commands.spawn((
        Name::new("Impact Sound"),
        AudioPlayer::new(pool[index].clone()),
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

// Miss whooshes belong to the local swinger — there's no world anchor the
// sound originates from, so play it non-spatially. That also avoids any
// distance falloff making the player's own swing feel quiet.
fn spawn_miss_sound(
    commands: &mut Commands,
    assets: &ImpactSoundAssets,
    picker: &mut ImpactSoundPicker,
    settings: &ClientSettings,
) {
    if assets.miss.is_empty() {
        return;
    }
    let index = pick_variant_index(
        &mut picker.miss_fire_count,
        &mut picker.miss_last,
        assets.miss.len(),
    );
    commands.spawn((
        Name::new("Miss Sound"),
        AudioPlayer::new(assets.miss[index].clone()),
        PlaybackSettings::DESPAWN.with_volume(impact_sound_volume(settings)),
    ));
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
