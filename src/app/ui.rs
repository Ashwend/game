mod chat;
mod confirm;
mod crafting;
mod crafting_queue;
mod death_splash;
mod deployable_overlay;
pub(crate) mod floating_text;
mod furnace;
mod hud;
mod inventory;
mod login;
mod loot_bag;
mod menu;
mod modal;
mod multiplayer;
mod options;
mod pause;
mod peer_overlay;
mod splash;
mod theme;
mod toast;
mod update;
mod worlds;

use bevy::input::ButtonInput;
use bevy::window::{Monitor, PrimaryMonitor};
use bevy::{diagnostic::DiagnosticsStore, ecs::system::SystemParam, prelude::*};
use bevy_egui::{EguiContextSettings, EguiContexts, PrimaryEguiContext, egui};

use super::audio::{PlaySound, SoundId};

use self::{
    chat::chat_ui,
    confirm::{confirmation_ui, notice_ui},
    crafting::crafting_ui,
    crafting_queue::crafting_queue_hud,
    death_splash::{death_splash_ui, send_respawn},
    deployable_overlay::{
        DeployableOverlay, DeployableOverlayParams, collect_deployable_overlay_entries,
        deployable_overlay_ui,
    },
    floating_text::{FloatingDamageText, floating_damage_ui},
    furnace::furnace_ui,
    hud::hud_ui,
    inventory::{draw_drag_preview, handle_drag_release, inventory_ui},
    loot_bag::loot_bag_ui,
    menu::main_menu_ui,
    multiplayer::multiplayer_ui,
    options::{OptionsBackTarget, options_ui},
    pause::pause_ui,
    peer_overlay::{PeerOverlay, PeerOverlayParams, collect_peer_overlay_entries, peer_overlay_ui},
    splash::loading_splash_ui,
    theme::{ButtonKind, MENU_BUTTON_WIDTH, game_button},
    toast::toast_ui,
    update::{update_corner_pill, update_modal},
    worlds::worlds_ui,
};

use egui_commonmark::CommonMarkCache;

pub(crate) use death_splash::tick_death_splash_system;

use super::scene::WorldSceneState;
use super::state::{
    AuthFlow, ClientErrorToast, ClientRuntime, ClientSettings, CraftingHudState, CraftingUiState,
    CurrentUser, InventorySoundEvent, LocalPlayerState, MAX_UI_SCALE, MIN_UI_SCALE,
    MenuBackdropVisibility, MenuState, OptionsUiState, PredictionState, SaveStore, Screen,
    SessionShutdownTasks, ToastState, WorkosAuth,
};
use super::systems::PendingSessionEndReason;
use super::voice::VoiceState;
use crate::analytics::Analytics;
use crate::net::ClientNetwork;
use crate::update::UpdateState;

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
    /// Absent until the player is signed in (gated by `auth`). The menu screens
    /// only read it once `auth.is_authenticated()`.
    user: Option<Res<'w, CurrentUser>>,
    auth: ResMut<'w, AuthFlow>,
    workos: Res<'w, WorkosAuth>,
    time: Option<Res<'w, Time>>,
    diagnostics: Res<'w, DiagnosticsStore>,
    primary_monitor: Query<'w, 's, &'static Monitor, With<PrimaryMonitor>>,
    peer_overlay: PeerOverlayParams<'w, 's>,
    deployable_overlay: DeployableOverlayParams<'w, 's>,
    floating_damage: Query<'w, 's, &'static FloatingDamageText>,
    analytics: Res<'w, Analytics>,
    pending_session_end: ResMut<'w, PendingSessionEndReason>,
    client_network: Res<'w, ClientNetwork>,
    local_player: Res<'w, LocalPlayerState>,
    prediction: ResMut<'w, PredictionState>,
    scene_state: Res<'w, WorldSceneState>,
    update: ResMut<'w, UpdateState>,
}

/// Whether the just-joined world is ready for the player to interact with:
/// the `Welcome` has been applied (client id + world data present), the live
/// scene geometry for that world has been spawned, and the local player's
/// replicated entity has arrived. The loading splash holds until this is true
/// so the crossfade reveals a populated, rendered scene rather than a
/// half-streamed one.
fn world_ready_for_play(resources: &UiResources) -> bool {
    resources.runtime.client_id.is_some()
        && resources.runtime.world.is_some()
        && resources.local_player.entity.is_some()
        && resources.scene_state.applied_live_version() == Some(resources.runtime.world_version)
}

/// egui zoom factor (pixels-per-point multiplier) for the player's chosen UI
/// scale, clamped to the supported range so a malformed settings file can't
/// shrink the chrome to nothing or blow it off-screen.
fn ui_zoom_factor(settings: &ClientSettings) -> f32 {
    let scale = settings.display.ui_scale;
    if scale.is_finite() {
        scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE)
    } else {
        1.0
    }
}

/// Applies the player's UI-scale preference to the primary egui context.
///
/// bevy_egui 0.39 bakes the display scale factor into egui's zoom every frame,
/// so driving zoom directly fights it and breaks layout (see the note in
/// [`ui_system`]). The supported knob is [`EguiContextSettings::scale_factor`],
/// which bevy_egui multiplies into the context's pixels-per-point on top of the
/// display scale. Written only when it changes so a stable value never
/// re-triggers egui's per-frame zoom path.
pub(crate) fn apply_ui_scale_system(
    settings: Res<ClientSettings>,
    mut contexts: Query<&mut EguiContextSettings, With<PrimaryEguiContext>>,
) {
    let target = ui_zoom_factor(&settings);
    for mut ctx_settings in &mut contexts {
        if ctx_settings.scale_factor != target {
            ctx_settings.scale_factor = target;
        }
    }
}

/// Registers the custom title typeface on the primary egui context once.
///
/// `ctx.set_fonts` rebuilds the font atlas, so this must not run per frame,
/// the `Local` flag latches after the first successful install. Runs ahead of
/// [`ui_system`] in the context pass so the very first frame already has the
/// font available.
pub(crate) fn install_egui_fonts_system(
    mut contexts: EguiContexts,
    mut installed: Local<bool>,
) -> bevy::prelude::Result {
    if *installed {
        return Ok(());
    }
    theme::install_title_font(contexts.ctx_mut()?);
    *installed = true;
    Ok(())
}

pub(crate) fn ui_system(
    mut contexts: EguiContexts,
    mut resources: UiResources,
    mut commonmark_cache: Local<CommonMarkCache>,
) -> bevy::prelude::Result {
    let ctx = contexts.ctx_mut()?;
    theme::apply_game_style(ctx);
    // NOTE: do NOT call `ctx.set_zoom_factor()` here. bevy_egui 0.39 bakes the
    // display scale factor into egui's zoom every frame via
    // `set_pixels_per_point`; setting a different zoom here makes the two
    // ping-pong, and egui's `begin_pass` jitter-avoidance hack discards the
    // real `screen_rect` on every zoom change, so the whole UI is laid out in
    // egui's ~5000x5000 default and centred menus render off-screen on HiDPI.
    // User UI scale is applied via `EguiContextSettings::scale_factor` in
    // `apply_ui_scale_system` instead.
    let delta_seconds = resources
        .time
        .as_ref()
        .map(|time| time.delta_secs())
        .unwrap_or(1.0 / 60.0);
    let cover_alpha = resources
        .backdrop_visibility
        .cover_alpha(resources.menu.screen, delta_seconds);
    theme::backdrop_cover(ctx, cover_alpha);

    // Gate the title screen behind WorkOS sign-in: until authenticated, render
    // the login splash (or the verifying/authenticating spinner) in place of
    // the menu. `drive_auth_flow_system` advances the spinner states.
    if !resources.auth.is_authenticated() {
        login::login_overlay_ui(
            ctx,
            &mut resources.auth,
            &resources.workos,
            &mut resources.menu,
        );
        return Ok(());
    }
    let user = resources
        .user
        .as_deref()
        .expect("authenticated state implies CurrentUser is present");

    match resources.menu.screen {
        Screen::MainMenu => main_menu_ui(ctx, &mut resources.menu, &resources.store, user),
        Screen::Worlds => worlds_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.store,
            user,
            &resources.client_network,
            &resources.analytics,
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
            user,
            &resources.client_network,
            &resources.analytics,
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
                // Suppress the peer overlay (nameplates, chat bubbles)
                // whenever a full-screen modal is up. Nameplates
                // render at Order::Foreground; without this gate
                // they'd poke through the bag / furnace / inventory /
                // crafting panels.
                let world_overlays_visible = !resources.menu.inventory_open
                    && !resources.menu.crafting_open
                    && !resources.menu.furnace_open
                    && !resources.menu.loot_bag_open;
                let camera = resources
                    .peer_overlay
                    .camera
                    .single()
                    .ok()
                    .map(|(camera, transform)| (camera, *transform));
                if world_overlays_visible {
                    let peers = collect_peer_overlay_entries(
                        resources.peer_overlay.network_players.iter(),
                        resources.peer_overlay.replicated_players.iter(),
                        resources.runtime.client_id,
                        &resources.voice,
                    );
                    peer_overlay_ui(ctx, PeerOverlay { camera, peers });
                }

                // Floating damage + deployable nametags are also
                // world-overlay layers; suppress them under the same
                // gate so a full-screen modal isn't pocked with
                // floating numbers and structure labels.
                if world_overlays_visible {
                    floating_damage_ui(ctx, camera, resources.floating_damage.iter());

                    let entries = collect_deployable_overlay_entries(
                        resources.deployable_overlay.placed.iter(),
                        resources.deployable_overlay.replicated.iter(),
                    );
                    deployable_overlay_ui(ctx, DeployableOverlay { camera, entries });
                }

                inventory_ui(
                    ctx,
                    &mut resources.menu,
                    &resources.local_player,
                    &mut resources.inventory_ui,
                    &resources.pickup_target,
                    &mut resources.inventory_sound_requests,
                    delta_seconds,
                );
                crafting_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &resources.local_player,
                    &mut resources.crafting_ui,
                    &mut resources.error_toasts,
                );
                furnace_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &resources.local_player,
                    &mut resources.inventory_ui,
                    &mut resources.error_toasts,
                );
                loot_bag_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &resources.local_player,
                    &mut resources.inventory_ui,
                    &mut resources.error_toasts,
                );
                // Drag release + preview run after every slot-drawing
                // surface (inventory, furnace) so the release decision
                // sees `hovered_slot` and the panel rects populated by
                // *this* frame. Without this ordering, a drag inside
                // the inventory while the furnace is open releases on
                // a `None` hovered_slot and falls through to the
                // drop-on-ground branch.
                handle_drag_release(
                    ctx,
                    &resources.menu,
                    &mut resources.runtime,
                    &mut resources.prediction,
                    &resources.local_player,
                    &mut resources.inventory_ui,
                    &mut resources.error_toasts,
                );
                draw_drag_preview(ctx, &resources.inventory_ui);
                // The queue HUD is always visible while jobs exist,
                // closing the crafting browser must not hide it, that
                // would defeat the point of the queue being persistent.
                crafting_queue_hud(
                    ctx,
                    &mut resources.runtime,
                    &resources.local_player,
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
                // Death splash sits above every other in-game UI but
                // below modal dialogs / loading splash. Renders only
                // while `menu.death_splash` is set (server flipped the
                // local player to Dead and the runtime stored the
                // killer name).
                if let Some(splash) = resources.menu.death_splash.clone() {
                    let respawn_clicked = death_splash_ui(ctx, &splash);
                    if respawn_clicked {
                        send_respawn(&mut resources.runtime);
                    }
                }
            }
            if resources.menu.pause_open && !resources.menu.pause_options_open {
                pause_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &mut resources.shutdown_tasks,
                    &resources.store,
                    &mut resources.pending_session_end,
                    &mut resources.update,
                );
            }
        }
    }

    // Update affordances. The corner pill rides every menu screen (the in-game
    // HUD uses a pause-menu row instead); the changelog modal is a global
    // overlay so it works from any screen.
    if !matches!(resources.menu.screen, Screen::InGame) {
        update_corner_pill(ctx, &mut resources.update);
    }
    update_modal(ctx, &mut resources.update, &mut commonmark_cache);

    confirmation_ui(
        ctx,
        &mut resources.menu,
        &resources.store,
        &resources.analytics,
    );
    notice_ui(ctx, &mut resources.menu);
    // Splash overlay sits on top of every screen and modal. It covers the
    // app-launch warmup ("Authenticating") and every menu→game transition
    // (world entry, server join). World-entry splashes hold until the joined
    // world is actually ready to play (see `world_ready_for_play`).
    let world_ready = world_ready_for_play(&resources);
    loading_splash_ui(
        ctx,
        &mut resources.menu,
        &resources.backdrop_visibility,
        world_ready,
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

    #[test]
    fn ui_zoom_factor_passes_through_in_range_scale() {
        let mut settings = ClientSettings::default();
        settings.display.ui_scale = 1.25;
        assert!((ui_zoom_factor(&settings) - 1.25).abs() < 1e-6);
    }

    #[test]
    fn ui_zoom_factor_clamps_extremes() {
        let mut settings = ClientSettings::default();
        settings.display.ui_scale = 10.0;
        assert_eq!(ui_zoom_factor(&settings), MAX_UI_SCALE);
        settings.display.ui_scale = 0.0;
        assert_eq!(ui_zoom_factor(&settings), MIN_UI_SCALE);
    }

    #[test]
    fn ui_zoom_factor_falls_back_on_non_finite() {
        let mut settings = ClientSettings::default();
        settings.display.ui_scale = f32::NAN;
        assert_eq!(ui_zoom_factor(&settings), 1.0);
        // Negative infinity is also non-finite and should fall back.
        settings.display.ui_scale = f32::NEG_INFINITY;
        assert_eq!(ui_zoom_factor(&settings), 1.0);
    }

    #[test]
    fn ui_zoom_factor_clamps_to_exact_bounds_at_the_edges() {
        let mut settings = ClientSettings::default();
        settings.display.ui_scale = MIN_UI_SCALE;
        assert_eq!(ui_zoom_factor(&settings), MIN_UI_SCALE);
        settings.display.ui_scale = MAX_UI_SCALE;
        assert_eq!(ui_zoom_factor(&settings), MAX_UI_SCALE);
    }

    #[test]
    fn inventory_sounds_map_to_distinct_pool_ids() {
        assert_eq!(
            inventory_sound_id(InventorySoundEvent::Pickup),
            SoundId::InventoryPickup
        );
        assert_eq!(
            inventory_sound_id(InventorySoundEvent::Drop),
            SoundId::InventoryDrop
        );
        assert_eq!(
            inventory_sound_id(InventorySoundEvent::Move),
            SoundId::InventoryMove
        );
    }

    #[test]
    fn button_sound_requests_queue_hover_events() {
        let mut requests = ButtonSoundRequests::default();
        requests.push_hover();
        requests.push_hover();
        assert_eq!(requests.0.len(), 2);
    }
}
