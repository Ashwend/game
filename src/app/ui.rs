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
mod inventory_panel;
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
mod tutorial;
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
    crafting_queue::crafting_queue_hud,
    death_splash::{death_splash_ui, send_respawn},
    deployable_overlay::{
        DeployableOverlay, DeployableOverlayParams, collect_deployable_overlay_entries,
        deployable_overlay_ui,
    },
    floating_text::{FloatingDamageText, floating_damage_ui},
    furnace::furnace_ui,
    hud::hud_ui,
    inventory::{draw_drag_preview, handle_drag_release},
    inventory_panel::inventory_panel_ui,
    loot_bag::loot_bag_ui,
    menu::main_menu_ui,
    multiplayer::multiplayer_ui,
    options::{OptionsBackTarget, options_ui},
    pause::pause_ui,
    peer_overlay::{PeerOverlay, PeerOverlayParams, collect_peer_overlay_entries, peer_overlay_ui},
    splash::loading_splash_ui,
    theme::{ButtonKind, MENU_BUTTON_WIDTH, game_button},
    toast::toast_ui,
    tutorial::{TutorialStep, tutorial_step, tutorial_ui},
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
    local_player: Res<'w, LocalPlayerState>,
    prediction: ResMut<'w, PredictionState>,
    scene_state: Res<'w, WorldSceneState>,
    update: ResMut<'w, UpdateState>,
    combat_feedback: Res<'w, crate::app::state::CombatFeedbackState>,
    /// Replicated resource nodes in the player's AoI, used by the tutorial to
    /// ring the nearest gatherable node during the gather step.
    resource_nodes: Query<'w, 's, &'static crate::server::ResourceNode>,
    /// One-shot sound cues (used here to play the completion sting when the
    /// tutorial finishes).
    play_sound: MessageWriter<'w, PlaySound>,
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
        // `workos` is only ever absent on the authenticated bypass path, so if
        // we're here it's present. Guard anyway rather than unwrap.
        if let Some(workos) = resources.workos.as_ref() {
            login::login_overlay_ui(ctx, &mut resources.auth, workos, &mut resources.menu);
        }
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
                // Screenshot toggles. `show_hud` is the master switch for all
                // always-on HUD chrome; `show_chat` additionally hides just the
                // chat box. Neither pauses the game: the world keeps simulating,
                // these only gate what's painted on top.
                let show_hud = resources.settings.hud.show_hud;
                let show_chat = resources.settings.hud.show_chat;
                if show_hud {
                    hud_ui(
                        ctx,
                        &resources.runtime,
                        &resources.diagnostics,
                        &resources.settings,
                        &resources.voice,
                        &resources.combat_feedback,
                    );
                }
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
                if world_overlays_visible && show_hud {
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
                if world_overlays_visible && show_hud {
                    floating_damage_ui(ctx, camera, resources.floating_damage.iter());

                    let entries = collect_deployable_overlay_entries(
                        resources.deployable_overlay.placed.iter(),
                        resources.deployable_overlay.replicated.iter(),
                    );
                    deployable_overlay_ui(ctx, DeployableOverlay { camera, entries });
                }

                // Compute the tutorial step before the panel so the crafting
                // list can pin the focused recipes to the top (keeps their
                // outlines on-screen instead of below the scroll fold). The
                // overlay itself is drawn after the panel.
                let tutorial = tutorial_step(
                    resources
                        .local_player
                        .private
                        .as_ref()
                        .map(|p| &p.inventory),
                    resources.local_player.private.as_ref().map(|p| &p.crafting),
                    resources.menu.inventory_open,
                    resources.menu.crafting_open,
                );
                let tutorial_active = !resources.settings.onboarding.completed
                    && show_hud
                    && world_ready_for_play(&resources)
                    && !resources.menu.pause_open
                    && resources.menu.death_splash.is_none();
                ctx.memory_mut(|mem| {
                    mem.data.insert_temp(
                        tutorial::pin_recipes_key(),
                        tutorial_active && tutorial == TutorialStep::CraftTools,
                    )
                });

                // Unified inventory + crafting panel: one fixed-size shell
                // with a tab bar. Replaces the two separate modals; the
                // toggle systems flip which tab is active.
                inventory_panel_ui(
                    ctx,
                    &mut resources.menu,
                    &mut resources.runtime,
                    &resources.local_player,
                    &mut resources.inventory_ui,
                    &mut resources.crafting_ui,
                    &resources.pickup_target,
                    &mut resources.inventory_sound_requests,
                    &mut resources.error_toasts,
                    delta_seconds,
                    show_hud,
                );

                // Draw the tutorial overlay (card + focus highlights). Runs after
                // the panel so the tab/recipe rects it outlines are already
                // stashed in egui memory this frame; `tutorial`/`tutorial_active`
                // were computed above (before the panel) for the recipe pinning.
                if tutorial_active && tutorial == TutorialStep::Done {
                    resources.settings.onboarding.completed = true;
                    // Celebrate: the same arrival sting as the menu reveal, plus a
                    // completion banner timed off this moment.
                    resources
                        .play_sound
                        .write(PlaySound::non_spatial(SoundId::WorldJoin));
                    let now = ctx.input(|input| input.time);
                    ctx.memory_mut(|mem| mem.data.insert_temp(tutorial::celebrate_key(), now));
                } else if tutorial_active {
                    let inventory = resources
                        .local_player
                        .private
                        .as_ref()
                        .map(|p| &p.inventory);
                    let crafting = resources.local_player.private.as_ref().map(|p| &p.crafting);
                    let player_position = resources.runtime.local_player_position();
                    // Crude (hand-pickup) nodes only, paired with what they yield,
                    // so the gather ring points at branches/stones/grass, never a
                    // tree or rock that needs a tool the player doesn't have yet.
                    let crude_nodes: Vec<(Vec3, &'static str)> = resources
                        .resource_nodes
                        .iter()
                        .filter_map(|node| {
                            let definition =
                                crate::resources::resource_node_definition(&node.definition_id)?;
                            if definition.required_tool.kind != crate::items::ToolKind::Hands {
                                return None;
                            }
                            let yield_item = definition.storage.first().map(|mat| mat.item_id)?;
                            Some((
                                Vec3::new(node.position.x, node.position.y, node.position.z),
                                yield_item,
                            ))
                        })
                        .collect();
                    tutorial_ui(
                        ctx,
                        tutorial,
                        inventory,
                        crafting,
                        camera,
                        &crude_nodes,
                        player_position,
                    );
                }

                // Completion banner, self-gated off the timestamp stamped above,
                // so it lingers for a few seconds after the tutorial finishes.
                if show_hud {
                    tutorial::completion_banner(ctx);
                }

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
                // The HUD master toggle still hides it for screenshots.
                if show_hud {
                    crafting_queue_hud(
                        ctx,
                        &mut resources.runtime,
                        &resources.local_player,
                        &mut resources.crafting_hud,
                        &mut resources.error_toasts,
                    );
                }
                let inventory_open = resources.menu.inventory_open;
                let actionbar_rect = resources.inventory_ui.actionbar_rect;
                // Chat is independent of the HUD master: hiding the HUD for a
                // clean screenshot can still leave chat up and usable if the
                // chat toggle stays on.
                if show_chat {
                    chat_ui(
                        ctx,
                        &mut resources.menu,
                        &mut resources.runtime,
                        &mut resources.error_toasts,
                        inventory_open,
                        actionbar_rect,
                    );
                }
                if show_hud {
                    toast_ui(ctx, &resources.toasts, actionbar_rect);
                }
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
