mod chat;
mod confirm;
mod hud;
mod inventory;
mod menu;
mod modal;
mod multiplayer;
mod options;
mod pause;
mod peer_overlay;
mod splash;
mod theme;
mod toast;
mod worlds;

use bevy::input::ButtonInput;
use bevy::window::{Monitor, PrimaryMonitor};
use bevy::{
    audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume},
    diagnostic::DiagnosticsStore,
};
use bevy::{ecs::system::SystemParam, prelude::*};
use bevy_egui::{EguiContexts, egui};

use self::{
    chat::chat_ui,
    confirm::{confirmation_ui, notice_ui},
    hud::hud_ui,
    inventory::inventory_ui,
    menu::main_menu_ui,
    multiplayer::multiplayer_ui,
    options::{OptionsBackTarget, options_ui},
    pause::pause_ui,
    peer_overlay::{PeerOverlay, PeerOverlayParams, collect_peer_overlay_entries, peer_overlay_ui},
    splash::loading_splash_ui,
    theme::{ButtonKind, MENU_BUTTON_WIDTH, game_button},
    toast::toast_ui,
    worlds::worlds_ui,
};
use super::state::{
    ClientErrorToast, ClientRuntime, ClientSettings, MenuBackdropVisibility, MenuState,
    OptionsUiState, SaveStore, Screen, SessionShutdownTasks, SteamUser, ToastState,
};
use super::voice::VoiceState;

#[derive(SystemParam)]
pub(crate) struct UiResources<'w, 's> {
    menu: ResMut<'w, MenuState>,
    backdrop_visibility: ResMut<'w, MenuBackdropVisibility>,
    runtime: ResMut<'w, ClientRuntime>,
    settings: ResMut<'w, ClientSettings>,
    options_ui: ResMut<'w, OptionsUiState>,
    voice: Res<'w, VoiceState>,
    physical_keys: Res<'w, ButtonInput<KeyCode>>,
    inventory_ui: ResMut<'w, super::state::InventoryUiState>,
    pickup_target: Res<'w, super::state::PickupTargetState>,
    toasts: Res<'w, ToastState>,
    shutdown_tasks: ResMut<'w, SessionShutdownTasks>,
    button_sound_requests: ResMut<'w, ButtonSoundRequests>,
    error_toasts: MessageWriter<'w, ClientErrorToast>,
    store: Res<'w, SaveStore>,
    user: Res<'w, SteamUser>,
    time: Option<Res<'w, Time>>,
    diagnostics: Res<'w, DiagnosticsStore>,
    primary_monitor: Query<'w, 's, &'static Monitor, With<PrimaryMonitor>>,
    peer_overlay: PeerOverlayParams<'w, 's>,
}

pub(crate) fn ui_system(
    mut contexts: EguiContexts,
    mut resources: UiResources,
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
        Screen::MainMenu => {
            main_menu_ui(ctx, &mut resources.menu, &resources.store, &resources.user)
        }
        Screen::Worlds => worlds_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.store,
            &resources.user,
        ),
        Screen::Options => {
            let primary_monitor = resources.primary_monitor.single().ok();
            options_ui(
                ctx,
                &mut resources.menu,
                &mut resources.settings,
                &mut resources.options_ui,
                &resources.physical_keys,
                primary_monitor,
                OptionsBackTarget::MainMenu,
            );
        }
        Screen::Multiplayer => multiplayer_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.user,
        ),
        Screen::InGame => {
            if resources.menu.pause_options_open {
                let primary_monitor = resources.primary_monitor.single().ok();
                options_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.settings,
                    &mut resources.options_ui,
                    &resources.physical_keys,
                    primary_monitor,
                    OptionsBackTarget::PauseMenu,
                );
            } else {
                hud_ui(
                    ctx,
                    &resources.runtime,
                    &resources.diagnostics,
                    &resources.settings,
                    &resources.voice,
                );
                let snapshot_players = resources
                    .runtime
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.players.as_slice())
                    .unwrap_or(&[]);
                let peers = collect_peer_overlay_entries(
                    resources.peer_overlay.network_players.iter(),
                    snapshot_players,
                    resources.runtime.client_id,
                    &resources.voice,
                );
                let camera = resources
                    .peer_overlay
                    .camera
                    .single()
                    .ok()
                    .map(|(camera, transform)| (camera, *transform));
                peer_overlay_ui(ctx, PeerOverlay { camera, peers });

                inventory_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &mut resources.inventory_ui,
                    &resources.pickup_target,
                    &mut resources.error_toasts,
                    delta_seconds,
                );
                let inventory_open = resources.menu.inventory_open;
                chat_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &mut resources.error_toasts,
                    inventory_open,
                );
                toast_ui(ctx, &resources.toasts);
            }
            if resources.menu.pause_open && !resources.menu.pause_options_open {
                pause_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &mut resources.shutdown_tasks,
                    &resources.store,
                );
            }
        }
    }

    confirmation_ui(ctx, &mut resources.menu, &resources.store);
    notice_ui(ctx, &mut resources.menu);
    // Splash overlay sits on top of every screen and modal. It covers the
    // app-launch warmup ("Authenticating") and every menu→game transition
    // (world entry, server join).
    loading_splash_ui(
        ctx,
        &mut resources.menu,
        &resources.backdrop_visibility,
        delta_seconds,
    );
    resources
        .button_sound_requests
        .0
        .extend(theme::take_button_sounds(ctx));

    Ok(())
}

fn menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Secondary, MENU_BUTTON_WIDTH)
}

fn primary_menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Primary, MENU_BUTTON_WIDTH)
}

fn danger_menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Danger, MENU_BUTTON_WIDTH)
}

const BUTTON_CLICK_SOUND_PATH: &str = "ui/button-click.wav";
const BUTTON_HOVER_SOUND_PATH: &str = "ui/button-hover.wav";
const BUTTON_CLICK_VOLUME_DECIBELS: f32 = -12.0;
const BUTTON_HOVER_VOLUME_DECIBELS: f32 = -30.0;

#[derive(Resource, Default)]
pub(crate) struct ButtonSoundRequests(Vec<theme::ButtonSound>);

impl ButtonSoundRequests {
    pub(crate) fn push_hover(&mut self) {
        self.0.push(theme::ButtonSound::Hover);
    }
}

#[derive(Resource)]
pub(crate) struct ButtonSoundAssets {
    click: Handle<AudioSource>,
    hover: Handle<AudioSource>,
}

pub(crate) fn setup_button_sound_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ButtonSoundAssets {
        click: asset_server.load(super::embedded_asset_path(button_sound_path(
            theme::ButtonSound::Click,
        ))),
        hover: asset_server.load(super::embedded_asset_path(button_sound_path(
            theme::ButtonSound::Hover,
        ))),
    });
}

pub(crate) fn button_sound_system(
    mut commands: Commands,
    mut requests: ResMut<ButtonSoundRequests>,
    assets: Res<ButtonSoundAssets>,
    settings: Res<ClientSettings>,
) {
    for sound in std::mem::take(&mut requests.0) {
        commands.spawn((
            Name::new(format!("Button {:?} Sound", sound)),
            AudioPlayer::new(button_sound_handle(sound, &assets)),
            PlaybackSettings::DESPAWN.with_volume(button_sound_volume(sound, &settings)),
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

fn button_sound_volume(sound: theme::ButtonSound, settings: &ClientSettings) -> Volume {
    let base = match sound {
        theme::ButtonSound::Click => Volume::Decibels(BUTTON_CLICK_VOLUME_DECIBELS),
        theme::ButtonSound::Hover => Volume::Decibels(BUTTON_HOVER_VOLUME_DECIBELS),
    };
    Volume::Linear(base.to_linear() * settings.audio.ui_volume.clamp(0.0, 1.0))
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
            button_sound_volume(theme::ButtonSound::Hover, &ClientSettings::default()).to_linear()
                < button_sound_volume(theme::ButtonSound::Click, &ClientSettings::default())
                    .to_linear()
        );
    }
}
