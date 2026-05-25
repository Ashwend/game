mod chat;
mod confirm;
mod crafting;
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
use bevy::{diagnostic::DiagnosticsStore, ecs::system::SystemParam, prelude::*};
use bevy_egui::{EguiContexts, egui};

use super::audio::{PlaySound, SoundId};

use self::{
    chat::chat_ui,
    confirm::{confirmation_ui, notice_ui},
    crafting::{crafting_queue_hud, crafting_ui},
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
    ClientErrorToast, ClientRuntime, ClientSettings, CraftingHudState, CraftingUiState,
    InventorySoundEvent, MenuBackdropVisibility, MenuState, OptionsUiState, SaveStore, Screen,
    SessionShutdownTasks, SteamUser, ToastState,
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
    crafting_ui: ResMut<'w, CraftingUiState>,
    crafting_hud: ResMut<'w, CraftingHudState>,
    pickup_target: Res<'w, super::state::PickupTargetState>,
    toasts: Res<'w, ToastState>,
    shutdown_tasks: ResMut<'w, SessionShutdownTasks>,
    button_sound_requests: ResMut<'w, ButtonSoundRequests>,
    inventory_sound_requests: ResMut<'w, InventorySoundRequests>,
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
                    &mut resources.inventory_sound_requests,
                    delta_seconds,
                );
                crafting_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &mut resources.crafting_ui,
                    &mut resources.error_toasts,
                );
                // The queue HUD is always visible while jobs exist —
                // closing the crafting browser must not hide it, that
                // would defeat the point of the queue being persistent.
                crafting_queue_hud(
                    ctx,
                    &mut resources.runtime,
                    &mut resources.crafting_hud,
                    &mut resources.error_toasts,
                );
                let inventory_open = resources.menu.inventory_open;
                let actionbar_rect = resources.inventory_ui.actionbar_rect;
                chat_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &mut resources.error_toasts,
                    inventory_open,
                    actionbar_rect,
                );
                toast_ui(ctx, &resources.toasts, actionbar_rect);
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

/// Queue of [`theme::ButtonSound`] events the egui middleware recorded
/// during the frame's draw pass. `ui_system` flushes them via
/// [`button_sound_system`], which translates each into a [`PlaySound`]
/// event for the central audio bus.
#[derive(Resource, Default)]
pub(crate) struct ButtonSoundRequests(Vec<theme::ButtonSound>);

impl ButtonSoundRequests {
    pub(crate) fn push_hover(&mut self) {
        self.0.push(theme::ButtonSound::Hover);
    }
}

pub(crate) fn button_sound_system(
    mut requests: ResMut<ButtonSoundRequests>,
    mut play: MessageWriter<PlaySound>,
) {
    for sound in std::mem::take(&mut requests.0) {
        play.write(PlaySound::non_spatial(button_sound_id(sound)));
    }
}

fn button_sound_id(sound: theme::ButtonSound) -> SoundId {
    match sound {
        theme::ButtonSound::Click => SoundId::UiButtonClick,
        theme::ButtonSound::Hover => SoundId::UiButtonHover,
    }
}

/// Queue of inventory change cues recorded while the UI observed the
/// player's inventory snapshot. Drained by [`inventory_sound_system`] in
/// the same pass as button sounds so all UI-driven cues flow through the
/// central [`PlaySound`] bus.
#[derive(Resource, Default)]
pub(crate) struct InventorySoundRequests(Vec<InventorySoundEvent>);

impl InventorySoundRequests {
    pub(crate) fn push(&mut self, event: InventorySoundEvent) {
        self.0.push(event);
    }
}

pub(crate) fn inventory_sound_system(
    mut requests: ResMut<InventorySoundRequests>,
    mut play: MessageWriter<PlaySound>,
) {
    for event in std::mem::take(&mut requests.0) {
        play.write(PlaySound::non_spatial(inventory_sound_id(event)));
    }
}

fn inventory_sound_id(event: InventorySoundEvent) -> SoundId {
    match event {
        InventorySoundEvent::Pickup => SoundId::InventoryPickup,
        InventorySoundEvent::Drop => SoundId::InventoryDrop,
        InventorySoundEvent::Move => SoundId::InventoryMove,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_sounds_map_to_distinct_ui_pool_ids() {
        assert_eq!(
            button_sound_id(theme::ButtonSound::Click),
            SoundId::UiButtonClick
        );
        assert_eq!(
            button_sound_id(theme::ButtonSound::Hover),
            SoundId::UiButtonHover
        );
    }
}
