mod chat;
mod confirm;
mod crafting;
mod crafting_queue;
mod death_splash;
mod deployable_overlay;
pub(crate) mod floating_text;
mod furnace;
mod hud;
mod in_game;
mod inventory;
mod inventory_panel;
mod item_icons;
mod login;
mod loot_bag;
mod menu;
mod modal;
mod multiplayer;
mod options;
mod pause;
mod peer_overlay;
mod splash;
mod text_prompt;
mod theme;
mod toast;
mod tutorial;
mod update;
mod wheel;
mod world_map;
mod worlds;

use bevy::input::ButtonInput;
use bevy::window::{Monitor, PrimaryMonitor};
use bevy::{diagnostic::DiagnosticsStore, ecs::system::SystemParam, prelude::*};
use bevy_egui::{EguiContextSettings, EguiContexts, PrimaryEguiContext, egui};

use super::audio::{PlaySound, SoundId};

use self::{
    confirm::{confirmation_ui, notice_ui},
    deployable_overlay::DeployableOverlayParams,
    floating_text::FloatingDamageText,
    in_game::in_game_ui,
    menu::main_menu_ui,
    multiplayer::multiplayer_ui,
    options::{OptionsBackTarget, VoiceTabIo, options_ui},
    peer_overlay::PeerOverlayParams,
    splash::loading_splash_ui,
    theme::{ButtonKind, MENU_BUTTON_WIDTH, game_button},
    update::{current_changelog_modal, update_corner_pill, update_modal},
    worlds::worlds_ui,
};

use egui_commonmark::CommonMarkCache;

pub(crate) use death_splash::tick_death_splash_system;
pub(crate) use item_icons::setup_item_icons;

use super::scene::WorldSceneState;
use super::state::{
    AuthFlow, ClientErrorToast, ClientRuntime, ClientSettings, CraftingHudState, CraftingUiState,
    CurrentUser, DeployablePlacementState, InventorySoundEvent, LocalPlayerState, MAX_UI_SCALE,
    MIN_UI_SCALE, MenuBackdropTime, MenuBackdropVisibility, MenuState, OptionsUiState,
    PredictionState, SaveStore, Screen, SessionShutdownTasks, ToastState, WorkosAuth,
    WorldMapState, WorldMapUiState,
};
use super::systems::PendingSessionEndReason;
use super::voice::{VoiceDeviceCache, VoiceState, VoiceUiControl};
use crate::analytics::Analytics;
use crate::net::ClientNetwork;
use crate::update::UpdateState;

#[derive(SystemParam)]
pub(crate) struct UiResources<'w, 's> {
    menu: ResMut<'w, MenuState>,
    backdrop_visibility: ResMut<'w, MenuBackdropVisibility>,
    /// Pinned time of day for the menu backdrop sky. The debug-only title-screen
    /// slider mutates it; the sky system reads it (`scene::sky`).
    menu_backdrop_time: ResMut<'w, MenuBackdropTime>,
    runtime: ResMut<'w, ClientRuntime>,
    settings: ResMut<'w, ClientSettings>,
    options_ui: ResMut<'w, OptionsUiState>,
    voice: Res<'w, VoiceState>,
    /// Cached input/output device names for the Voice tab device pickers.
    voice_devices: Res<'w, VoiceDeviceCache>,
    /// UI -> systems channel for the Voice tab mic test + device refresh.
    voice_control: ResMut<'w, VoiceUiControl>,
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
    /// Absent on the test / `--connect` bypass path, which injects an
    /// authenticated identity and never inserts the WorkOS config. Only read in
    /// the unauthenticated login branch, which the bypass path never reaches.
    workos: Option<Res<'w, WorkosAuth>>,
    time: Option<Res<'w, Time>>,
    diagnostics: Res<'w, DiagnosticsStore>,
    primary_monitor: Query<'w, 's, &'static Monitor, With<PrimaryMonitor>>,
    peer_overlay: PeerOverlayParams<'w, 's>,
    deployable_overlay: DeployableOverlayParams<'w, 's>,
    floating_damage: Query<'w, 's, &'static FloatingDamageText>,
    analytics: Res<'w, Analytics>,
    pending_session_end: ResMut<'w, PendingSessionEndReason>,
    client_network: Res<'w, ClientNetwork>,
    /// Building-placement ghost state, read by the in-game cost overlay to
    /// draw the material cost + affordability under the ghost.
    placement: Res<'w, DeployablePlacementState>,
    /// World-map texture + markers, drawn by the toggle-to-view overlay.
    world_map: Res<'w, WorldMapState>,
    /// Transient world-map interaction state (which marker popup is open).
    world_map_ui: ResMut<'w, WorldMapUiState>,
    local_player: Res<'w, LocalPlayerState>,
    prediction: ResMut<'w, PredictionState>,
    scene_state: Res<'w, WorldSceneState>,
    update: ResMut<'w, UpdateState>,
    combat_feedback: Res<'w, crate::app::state::CombatFeedbackState>,
    /// Radial wheel overlay state (building plan / hammer / door / bag).
    /// Input lives in `systems::input::wheel`; the UI only paints it.
    wheel: Res<'w, crate::app::state::WheelMenuState>,
    /// Replicated resource nodes in the player's AoI, used by the tutorial to
    /// ring the nearest gatherable node during the gather step.
    resource_nodes: Query<'w, 's, &'static crate::server::ResourceNode>,
    /// One-shot sound cues (used here to play the completion sting when the
    /// tutorial finishes).
    play_sound: MessageWriter<'w, PlaySound>,
    /// Delayed one-shots: the audio tab's test sequences ride through here
    /// so they start after the test button's own click cue.
    scheduled_sounds: ResMut<'w, crate::app::audio::ScheduledSounds>,
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
    mut splash_diag_throttle: Local<f32>,
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
        // `workos` is only ever absent on the authenticated bypass path, so if
        // we're here it's present. Guard anyway rather than unwrap.
        if let Some(workos) = resources.workos.as_ref() {
            login::login_overlay_ui(
                ctx,
                &mut resources.auth,
                workos,
                &mut resources.menu,
                &resources.analytics,
            );
        }
        return Ok(());
    }
    let user = resources
        .user
        .as_deref()
        .expect("authenticated state implies CurrentUser is present");

    match resources.menu.screen {
        Screen::MainMenu => main_menu_ui(
            ctx,
            &mut resources.menu,
            &resources.store,
            user,
            &mut resources.update,
            &mut resources.menu_backdrop_time,
            resources.settings.dev.backdrop_time_slider,
        ),
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
            let mut voice_io = VoiceTabIo {
                devices: &resources.voice_devices,
                control: &mut resources.voice_control,
                input_level: resources.voice.mic_level(),
                playback_available: resources.voice.playback_available,
            };
            options_ui(
                ctx,
                &mut resources.menu,
                &mut resources.settings,
                &mut resources.options_ui,
                &resources.physical_keys,
                primary_monitor,
                OptionsBackTarget::MainMenu,
                &mut voice_io,
            );
        }
        Screen::Multiplayer => multiplayer_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            user,
            &resources.client_network,
            resources.workos.as_deref(),
            &resources.analytics,
        ),
        Screen::InGame => in_game_ui(ctx, &mut resources, delta_seconds),
    }

    // Update affordances. The corner pill rides every menu screen (the in-game
    // HUD uses a pause-menu row instead); the changelog modal is a global
    // overlay so it works from any screen.
    if !matches!(resources.menu.screen, Screen::InGame) {
        update_corner_pill(ctx, &mut resources.update);
    }
    update_modal(ctx, &mut resources.update, &mut commonmark_cache);
    // The "what's new in this version" modal, opened from the title-screen
    // version label. Global overlay so it animates closed cleanly from anywhere.
    current_changelog_modal(ctx, &mut resources.update, &mut commonmark_cache);

    confirmation_ui(
        ctx,
        &mut resources.menu,
        &mut resources.settings,
        &resources.store,
        &resources.analytics,
    );
    notice_ui(ctx, &mut resources.menu);
    // Splash overlay sits on top of every screen and modal. It covers the
    // app-launch warmup ("Authenticating") and every menu→game transition
    // (world entry, server join). World-entry splashes hold until the joined
    // world is actually ready to play (see `world_ready_for_play`).
    let world_ready = world_ready_for_play(&resources);
    // Diagnostic breadcrumb for the "I'm in-game but can only see the loader"
    // report: when a world-entry splash is up and the readiness gate hasn't
    // cleared, log which of the four conditions is still missing, throttled to
    // ~once a second. Otherwise the 20s fallback silently hides the culprit.
    // Reproduce, then read <data_dir>/logs/ashwend.log to see what's stuck.
    if let Some(splash) = resources.menu.loading_splash.as_ref() {
        let world_entry = matches!(
            splash.kind,
            crate::app::state::LoadingSplashKind::JoiningServer
                | crate::app::state::LoadingSplashKind::EnteringWorld
        );
        if world_entry && !world_ready {
            *splash_diag_throttle += delta_seconds;
            if *splash_diag_throttle >= 1.0 {
                *splash_diag_throttle = 0.0;
                let scene_applied = resources.scene_state.applied_live_version()
                    == Some(resources.runtime.world_version);
                bevy::log::warn!(
                    "loading splash waiting on world-ready ({:.0}s): client_id={} world_data={} \
                     local_player_entity={} scene_applied={} [world_version={}, scene_version={:?}, screen={:?}]",
                    splash.elapsed_seconds,
                    resources.runtime.client_id.is_some(),
                    resources.runtime.world.is_some(),
                    resources.local_player.entity.is_some(),
                    scene_applied,
                    resources.runtime.world_version,
                    resources.scene_state.applied_live_version(),
                    resources.menu.screen,
                );
            }
        } else {
            *splash_diag_throttle = 0.0;
        }
    }
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
    // Audio-tab test buttons queue (delay, sound) pairs in egui memory
    // the same way button clicks do; hand them to the scheduler so the
    // sequence starts after the button's own click cue has cleared and
    // each sample plays at whatever the sliders say when it fires.
    for (delay_secs, sound_id) in options::take_test_sounds(ctx) {
        resources
            .scheduled_sounds
            .push(delay_secs, PlaySound::non_spatial(sound_id));
    }

    Ok(())
}

/// Animated fade-in factor (0..1) for always-on HUD chrome (the hotbar and
/// chat) that should fade *out* while the world-map overlay is open, so it
/// doesn't collide with the map's footer text, and back *in* when it closes.
/// Returns 1.0 when the map is closed, easing toward 0.0 while it's open.
/// Shared by both so they fade in lockstep off one animation id.
pub(super) fn world_map_overlay_fade(ctx: &egui::Context, world_map_open: bool) -> f32 {
    const HUD_MAP_FADE_SECS: f32 = 0.18;
    1.0 - ctx.animate_bool_with_time(
        egui::Id::new("hud_world_map_fade"),
        world_map_open,
        HUD_MAP_FADE_SECS,
    )
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
        InventorySoundEvent::Pickup { item_id } => {
            pickup_sound_for(item_id.as_ref().map(|id| id.as_ref()))
        }
        InventorySoundEvent::Drop => SoundId::InventoryDrop,
        InventorySoundEvent::Move => SoundId::InventoryMove,
    }
}

/// Material-matched pickup cue: grabbing sticks rattles wood, grabbing a
/// stone (or any ore chunk) clacks rock, everything else keeps the
/// generic brushed-from-the-grass rustle (which is exactly right for the
/// fiber/hay pickup it was originally cut for).
fn pickup_sound_for(item_id: Option<&str>) -> SoundId {
    use crate::items::{COAL_ID, IRON_ORE_ID, STONE_ID, SULFUR_ORE_ID, WOOD_ID};
    match item_id {
        Some(WOOD_ID) => SoundId::PickupSticks,
        Some(STONE_ID | COAL_ID | IRON_ORE_ID | SULFUR_ORE_ID) => SoundId::PickupStones,
        _ => SoundId::InventoryPickup,
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
            inventory_sound_id(InventorySoundEvent::Pickup { item_id: None }),
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
    fn pickup_cue_matches_the_material() {
        // Sticks rattle wood, stone and every ore chunk clack rock, and
        // anything else (fiber, tools, unknown ids) keeps the grass
        // rustle the generic cue was recorded for.
        assert_eq!(
            pickup_sound_for(Some(crate::items::WOOD_ID)),
            SoundId::PickupSticks
        );
        assert_eq!(
            pickup_sound_for(Some(crate::items::STONE_ID)),
            SoundId::PickupStones
        );
        assert_eq!(
            pickup_sound_for(Some(crate::items::IRON_ORE_ID)),
            SoundId::PickupStones
        );
        assert_eq!(
            pickup_sound_for(Some(crate::items::FIBER_ID)),
            SoundId::InventoryPickup
        );
        assert_eq!(pickup_sound_for(None), SoundId::InventoryPickup);
    }

    #[test]
    fn button_sound_requests_queue_hover_events() {
        let mut requests = ButtonSoundRequests::default();
        requests.push_hover();
        requests.push_hover();
        assert_eq!(requests.0.len(), 2);
    }
}
