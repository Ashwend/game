mod chat;
mod confirm;
mod hud;
mod menu;
mod modal;
mod multiplayer;
mod pause;
mod theme;
mod worlds;

use bevy::{app::AppExit, ecs::system::SystemParam, prelude::*};
use bevy::{
    audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume},
    diagnostic::DiagnosticsStore,
};
use bevy_egui::{EguiContexts, egui};

use self::{
    chat::chat_ui,
    confirm::confirmation_ui,
    hud::hud_ui,
    menu::main_menu_ui,
    multiplayer::multiplayer_ui,
    pause::pause_ui,
    theme::{ButtonKind, game_button},
    worlds::worlds_ui,
};
use super::state::{
    ClientRuntime, MenuBackdropVisibility, MenuState, SaveStore, Screen, SteamUser,
};

#[derive(SystemParam)]
pub(crate) struct UiResources<'w> {
    menu: ResMut<'w, MenuState>,
    backdrop_visibility: ResMut<'w, MenuBackdropVisibility>,
    runtime: ResMut<'w, ClientRuntime>,
    button_sound_requests: ResMut<'w, ButtonSoundRequests>,
    store: Res<'w, SaveStore>,
    user: Res<'w, SteamUser>,
    time: Option<Res<'w, Time>>,
    diagnostics: Res<'w, DiagnosticsStore>,
}

pub(crate) fn ui_system(
    mut contexts: EguiContexts,
    mut resources: UiResources,
    mut app_exit: MessageWriter<AppExit>,
) -> bevy::prelude::Result {
    let ctx = contexts.ctx_mut()?;
    theme::apply_game_style(ctx);
    let delta_seconds = resources
        .time
        .as_ref()
        .map(|time| time.delta_secs())
        .unwrap_or(1.0 / 60.0);
    let cover_alpha = resources
        .backdrop_visibility
        .cover_alpha(resources.menu.screen, delta_seconds);
    theme::backdrop_cover(ctx, cover_alpha);

    match resources.menu.screen {
        Screen::MainMenu => main_menu_ui(
            ctx,
            &mut resources.menu,
            &resources.store,
            &resources.user,
            &mut app_exit,
        ),
        Screen::Worlds => worlds_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.store,
            &resources.user,
        ),
        Screen::Multiplayer => multiplayer_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.user,
        ),
        Screen::InGame => {
            hud_ui(ctx, &resources.runtime, &resources.diagnostics);
            chat_ui(ctx, &mut resources.menu, &mut resources.runtime);
            if resources.menu.pause_open {
                pause_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &resources.store,
                );
            }
        }
    }

    confirmation_ui(ctx, &mut resources.menu, &resources.store);
    resources
        .button_sound_requests
        .0
        .extend(theme::take_button_sounds(ctx));

    Ok(())
}

fn menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Secondary, 260.0)
}

fn primary_menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Primary, 260.0)
}

fn danger_menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Danger, 260.0)
}

const BUTTON_CLICK_SOUND_PATH: &str = "ui/button-click.wav";
const BUTTON_HOVER_SOUND_PATH: &str = "ui/button-hover.wav";
const BUTTON_CLICK_VOLUME_DECIBELS: f32 = -12.0;
const BUTTON_HOVER_VOLUME_DECIBELS: f32 = -30.0;

#[derive(Resource, Default)]
pub(crate) struct ButtonSoundRequests(Vec<theme::ButtonSound>);

#[derive(Resource)]
pub(crate) struct ButtonSoundAssets {
    click: Handle<AudioSource>,
    hover: Handle<AudioSource>,
}

pub(crate) fn setup_button_sound_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ButtonSoundAssets {
        click: asset_server.load(button_sound_path(theme::ButtonSound::Click)),
        hover: asset_server.load(button_sound_path(theme::ButtonSound::Hover)),
    });
}

pub(crate) fn button_sound_system(
    mut commands: Commands,
    mut requests: ResMut<ButtonSoundRequests>,
    assets: Res<ButtonSoundAssets>,
) {
    for sound in std::mem::take(&mut requests.0) {
        commands.spawn((
            Name::new(format!("Button {:?} Sound", sound)),
            AudioPlayer::new(button_sound_handle(sound, &assets)),
            PlaybackSettings::DESPAWN.with_volume(button_sound_volume(sound)),
        ));
    }
}

fn button_sound_handle(
    sound: theme::ButtonSound,
    assets: &ButtonSoundAssets,
) -> Handle<AudioSource> {
    match sound {
        theme::ButtonSound::Click => assets.click.clone(),
        theme::ButtonSound::Hover => assets.hover.clone(),
    }
}

fn button_sound_path(sound: theme::ButtonSound) -> &'static str {
    match sound {
        theme::ButtonSound::Click => BUTTON_CLICK_SOUND_PATH,
        theme::ButtonSound::Hover => BUTTON_HOVER_SOUND_PATH,
    }
}

fn button_sound_volume(sound: theme::ButtonSound) -> Volume {
    match sound {
        theme::ButtonSound::Click => Volume::Decibels(BUTTON_CLICK_VOLUME_DECIBELS),
        theme::ButtonSound::Hover => Volume::Decibels(BUTTON_HOVER_VOLUME_DECIBELS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_hover_sound_is_subtler_than_click() {
        assert_eq!(
            button_sound_path(theme::ButtonSound::Click),
            BUTTON_CLICK_SOUND_PATH
        );
        assert_eq!(
            button_sound_path(theme::ButtonSound::Hover),
            BUTTON_HOVER_SOUND_PATH
        );
        assert!(
            button_sound_volume(theme::ButtonSound::Hover).to_linear()
                < button_sound_volume(theme::ButtonSound::Click).to_linear()
        );
    }
}
