mod audio;
mod embedded_assets;
mod scene;
mod state;
mod systems;
mod ui;
mod voice;

pub(crate) use embedded_assets::asset_path as embedded_asset_path;

use std::net::SocketAddr;

use anyhow::Result;
#[cfg(feature = "profile")]
use bevy::diagnostic::{
    EntityCountDiagnosticsPlugin, LogDiagnosticsPlugin, SystemInformationDiagnosticsPlugin,
};
use bevy::{
    diagnostic::FrameTimeDiagnosticsPlugin, prelude::*, transform::TransformSystems,
    window::WindowPosition, winit::WinitSettings,
};
use bevy_egui::{EguiPlugin, EguiPostUpdateSet, EguiPrimaryContextPass};
use bevy_framepace::{FramepacePlugin, FramepaceSettings};

use crate::{
    analytics::AnalyticsPlugin,
    net::{ClientNetworkPlugin, LightyearProtocolPlugin, client_plugins},
    save::WorldStore,
    steam::{OfflineSteamBackend, SteamBackend},
};

use self::voice::{
    IncomingVoiceMessage, apply_voice_settings_system, manage_voice_capture_system,
    receive_voice_system, setup_voice_system, transmit_voice_system,
};

use self::{
    audio::{
        AudioPlugin, main_menu_music_system, manage_ambient_beds_system,
        manage_ambient_emitters_system, play_footsteps_system, play_impact_sounds_system,
        play_sounds_system, play_transition_stingers_system, tick_audio_faders_system,
    },
    scene::{apply_world_scene_system, setup_scene, update_sky_system},
    state::{
        ClientErrorToast, ClientRuntime, ClientSettingsStore, CraftingHudState, CraftingUiState,
        DeployablePlacementState, GatherInputState, InventoryUiState, LookState,
        MenuBackdropVisibility, MenuState, OptionsUiState, PickupTargetState, RemoteImpactEvent,
        SaveStore, SessionShutdownTasks, SteamUser, TestModeConfig, ToastState, ToolSwapState,
    },
    systems::{
        AutoConnectRequest, CameraImpactKick, CameraMotionEffects, ClientSystemSet,
        DroppedItemEntities, LastTrackedScreen, PendingSessionEndReason, RemotePlayerEntities,
        ResourceNodeEntities, SessionTracker, app_quit_system, apply_deployed_entities_system,
        apply_display_settings_system, apply_dropped_items_system, apply_held_item_visual_system,
        apply_resource_nodes_system, apply_snapshot_system, apply_test_mode_overrides_system,
        auto_connect_poll_system, auto_connect_start_system, camera_follow_system,
        center_cursor_on_focus_system, chat_shortcut_system, chunk_overlay_system,
        client_input_system, close_furnace_on_escape_system, error_relay_system,
        gameplay_inventory_shortcuts_system, menu_backdrop_camera_system, mouse_look_system,
        network_tick_system, placement_input_system, reposition_test_window_system,
        save_client_settings_system, screen_viewed_system, session_ended_system,
        session_shutdown_poll_system, session_started_system, spawn_impact_effects_system,
        surface_client_error_toasts_system, sync_furnace_open_flag_system, sync_view_radius_system,
        tick_felling_trees_system, tick_impact_chips_system, tick_resource_node_pop_in_system,
        toggle_crafting_system, toggle_inventory_system, toggle_pause_system,
        toggle_perf_stats_system, update_cursor_system, update_pickup_target_system,
        update_placement_ghost_system, update_tool_swap_state_system,
    },
    ui::{
        ButtonSoundRequests, InventorySoundRequests, button_sound_system, inventory_sound_system,
        ui_system,
    },
};

pub(crate) const EYE_HEIGHT: f32 = 1.62;
pub(crate) const PLAYER_VISUAL_CENTER_Y: f32 = 0.9;

/// Authoritative Update-phase order for client systems.
///
/// One ordered list, one source of truth: every consecutive pair becomes an
/// `after(prev)` edge in the schedule. Add new sets here in the slot that
/// matches their data dependency, not in a side chain. The phases below are
/// purely for human navigation — the runtime only sees the flat list.
///
/// Phases:
/// - Input/UI shortcut intake (Focus → InventoryShortcuts).
/// - Network tick and the tool-swap animation that reads its snapshot
///   (Network → ToolSwap). ToolSwap must run after Network because the
///   active actionbar slot lives on the snapshot, and before HeldItem so
///   the entry-animation fraction is fresh when the held-item visual is
///   rebuilt the same frame a new tool first appears.
/// - Session lifecycle and settings (SessionShutdown → SettingsSave).
/// - Scene application from the freshest snapshot (WorldScene → HeldItem).
/// - Look-target scan + impact effect pipeline (PickupTarget → NodeDeathTick).
///   ImpactSounds peeks the pending impact before ImpactEffectsSpawn takes
///   (and clears) it, so the cue plays even when the visual system runs in
///   the same frame.
const CLIENT_UPDATE_ORDER: &[ClientSystemSet] = &[
    ClientSystemSet::Focus,
    ClientSystemSet::ChatShortcut,
    ClientSystemSet::PauseToggle,
    ClientSystemSet::InventoryToggle,
    ClientSystemSet::CraftingToggle,
    ClientSystemSet::Cursor,
    ClientSystemSet::Look,
    ClientSystemSet::Input,
    ClientSystemSet::InventoryShortcuts,
    ClientSystemSet::Network,
    ClientSystemSet::ToolSwap,
    ClientSystemSet::SessionShutdown,
    ClientSystemSet::Quit,
    ClientSystemSet::Display,
    ClientSystemSet::SettingsSave,
    ClientSystemSet::WorldScene,
    ClientSystemSet::Players,
    ClientSystemSet::DroppedItems,
    ClientSystemSet::ResourceNodes,
    ClientSystemSet::DeployedEntities,
    // Placement preview / input rides after the snapshot has been
    // applied (so the local-player position used for reach checks is
    // current) but before the camera follow runs (so a fresh place
    // command doesn't double-process the same click frame).
    ClientSystemSet::PlacementInput,
    ClientSystemSet::PlacementGhost,
    ClientSystemSet::Camera,
    ClientSystemSet::HeldItem,
    ClientSystemSet::Sky,
    ClientSystemSet::PickupTarget,
    ClientSystemSet::ImpactSounds,
    ClientSystemSet::Footsteps,
    ClientSystemSet::ImpactEffectsSpawn,
    ClientSystemSet::ImpactEffectsTick,
    ClientSystemSet::NodeDeathTick,
    // Transition stingers (e.g. world-join) ride on `MenuState`
    // edge-detection, not on the gameplay event stream — slotted just
    // before the drain so the cue arrives in the same frame as the
    // screen change.
    ClientSystemSet::TransitionStingers,
    // Audio drain phase: every system that emits PlaySound has run by
    // now (impact, footsteps, node death, UI button, transitions), so
    // a single play_sounds_system pass empties the queue and spawns
    // the entities. The fader/ambient systems follow so any sink they
    // touch is the sink play_sounds_system just spawned.
    ClientSystemSet::PlaySounds,
    ClientSystemSet::AudioFaderTick,
    ClientSystemSet::AmbientBeds,
    ClientSystemSet::AmbientEmitters,
    ClientSystemSet::VoiceCaptureManage,
    ClientSystemSet::VoiceTransmit,
    ClientSystemSet::VoiceReceive,
    ClientSystemSet::VoiceSettings,
    ClientSystemSet::TestModeApply,
    ClientSystemSet::TestWindowReposition,
    // Analytics observer systems run last so the screen/session/error
    // edges they detect reflect every other system's writes from this
    // frame.
    ClientSystemSet::Analytics,
];

/// Menu-only systems form their own short chain — independent of the main
/// gameplay flow because they read menu state, not snapshots.
const CLIENT_MENU_ORDER: &[ClientSystemSet] = &[
    ClientSystemSet::MainMenuMusic,
    ClientSystemSet::MenuBackdropCamera,
];

fn configure_client_schedule(app: &mut App) {
    for window in CLIENT_UPDATE_ORDER.windows(2) {
        app.configure_sets(Update, window[1].after(window[0]));
    }
    for window in CLIENT_MENU_ORDER.windows(2) {
        app.configure_sets(Update, window[1].after(window[0]));
    }
}

/// Entry point used by the `client` CLI subcommand.
///
/// Pass `auto_connect = Some(addr)` to skip the menu and immediately attempt
/// a network connection to `addr` once the app is up. The multiplayer-test
/// helper relies on this so the two spawned client windows land directly in
/// the shared test world.
pub fn run_app(auto_connect: Option<SocketAddr>) -> Result<()> {
    let store = WorldStore::platform_default()?;
    store.ensure_exists()?;

    let steam = OfflineSteamBackend;
    let user = steam.current_user()?;
    let settings_store = ClientSettingsStore::platform_default()?;
    let settings = settings_store.load().unwrap_or_else(|error| {
        eprintln!("could not load client settings: {error:#}");
        Default::default()
    });
    let window_settings = settings.display;
    let test_mode = TestModeConfig::from_env();

    let mut app = App::new();
    if let Some(addr) = auto_connect {
        app.insert_resource(AutoConnectRequest { addr });
    }
    app.insert_resource(ClearColor(Color::srgb(0.015, 0.018, 0.023)))
        .insert_resource(SaveStore(store))
        .insert_resource(SteamUser(user))
        .insert_resource(settings_store)
        .insert_resource(settings)
        .insert_resource(MenuState::default())
        .insert_resource(OptionsUiState::default())
        .insert_resource(test_mode.clone())
        .insert_resource(MenuBackdropVisibility::default())
        .insert_resource(ClientRuntime::default())
        .insert_resource(SessionShutdownTasks::default())
        .insert_resource(InventoryUiState::default())
        .insert_resource(CraftingUiState::default())
        .insert_resource(CraftingHudState::default())
        .insert_resource(DeployablePlacementState::default())
        .insert_resource(PickupTargetState::default())
        .insert_resource(GatherInputState::default())
        .insert_resource(ToolSwapState::default())
        .insert_resource(CameraImpactKick::default())
        .insert_resource(CameraMotionEffects::default())
        .insert_resource(DroppedItemEntities::default())
        .insert_resource(ResourceNodeEntities::default())
        .insert_resource(RemotePlayerEntities::default())
        .insert_resource(LookState::default())
        .insert_resource(ToastState::default())
        .insert_resource(LastTrackedScreen::default())
        .insert_resource(SessionTracker::default())
        .insert_resource(PendingSessionEndReason::default())
        // `continuous()` rather than `desktop_app()`: the menu backdrop
        // camera pans continuously (see `menu_backdrop_camera_system`) and
        // needs steady frames to look smooth. Switching to reactive update
        // would chop the animation. If the backdrop is later gated behind
        // `MenuBackdropVisibility::is_active(...)` we can revisit and use
        // `desktop_app()` (or a reactive-low-power variant) when no panning
        // animation is on-screen.
        .insert_resource(WinitSettings::continuous())
        .init_resource::<ButtonSoundRequests>()
        .init_resource::<InventorySoundRequests>()
        .add_message::<RemoteImpactEvent>()
        .add_message::<ClientErrorToast>()
        .add_message::<IncomingVoiceMessage>()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                // `multiplayer-test` overrides the window resolution via
                // env vars and the actual position is set after the
                // primary monitor has been queried — see
                // `reposition_test_window_system`. Trying to centre at
                // startup would need a screen-size guess and that's exactly
                // what we'd get wrong on the dev's actual monitor.
                primary_window: Some(Window {
                    title: "Game".to_owned(),
                    resolution: test_mode
                        .window
                        .map(|w| (w.width, w.height).into())
                        .unwrap_or_else(|| {
                            (
                                window_settings.resolution.width,
                                window_settings.resolution.height,
                            )
                                .into()
                        }),
                    position: WindowPosition::default(),
                    present_mode: window_settings.present_mode(),
                    mode: if test_mode.window.is_some() {
                        // Test windows always come up in plain windowed
                        // mode so the post-monitor reposition actually
                        // applies — fullscreen would ignore it.
                        bevy::window::WindowMode::Windowed
                    } else {
                        window_settings.window_mode(None)
                    },
                    resizable: false,
                    ..default()
                }),
                ..default()
            }),
        )
        // 480 sample history: ~1 second at 500 FPS, ~4 seconds at 120 FPS.
        // The perf HUD pulls p99/max from this window so the player sees
        // hitches that the smoothed FPS number hides — 120 samples (default)
        // at 500 FPS is only 0.24 s, too short to catch periodic stalls.
        .add_plugins(FrameTimeDiagnosticsPlugin::new(480))
        // Lightyear client lives in the main Bevy app from Phase 3 of the
        // replication migration onward. The two plugins together register
        // the protocol channels and message types, and wire up the
        // connection lifecycle systems against the shared `ClientNetwork`
        // resource that gameplay code (and `ClientSession`) read/write.
        .add_plugins(client_plugins())
        .add_plugins(LightyearProtocolPlugin)
        .add_plugins(ClientNetworkPlugin);
    // `./cli profile` (Cargo feature `profile`): pair the Chrome trace
    // emitted by `bevy/trace_chrome` with text diagnostics so the log shows
    // FPS, frame time, entity count, and CPU/RAM alongside the spans. Gated
    // because `SystemInformationDiagnosticsPlugin` samples `sysinfo` on a
    // background thread and we don't want that cost in shipped builds.
    #[cfg(feature = "profile")]
    {
        app.add_plugins(LogDiagnosticsPlugin::default())
            .add_plugins(EntityCountDiagnosticsPlugin {
                max_history_length: 480,
            })
            .add_plugins(SystemInformationDiagnosticsPlugin);
    }
    app
        // Self-contained binary: every shipped sound is registered into
        // Bevy's `embedded` asset source so we don't have to ship a
        // sibling `assets/` folder. Must come after DefaultPlugins so
        // AssetPlugin (and therefore `EmbeddedAssetRegistry`) exists.
        .add_plugins(embedded_assets::EmbeddedAssetsPlugin)
        // Audio: registers PlaySound event, SoundLibrary, FootstepState,
        // and the global ambient-zone resource. Must come after
        // EmbeddedAssetsPlugin so the asset paths it loads at startup
        // resolve through the embedded source.
        .add_plugins(AudioPlugin)
        .add_plugins(EguiPlugin::default())
        // Software frame pacing. With this plugin running we can leave
        // `PresentMode` at `Immediate` everywhere and rely on a CPU-side
        // sleep to cap the frame rate — `Fifo`/`AutoVsync` are not
        // reliable on macOS Metal (flicker, no cap respectively). The
        // limiter starts in whatever state the saved settings ask for;
        // `apply_display_settings_system` keeps it in sync when the user
        // toggles vsync at runtime.
        .add_plugins(FramepacePlugin)
        // Analytics. Disabled by default; reads `analytics.local.toml` /
        // `POSTHOG_*` env vars at startup. Client-only — dedicated server
        // and admin CLI never load this plugin.
        .add_plugins(AnalyticsPlugin)
        .insert_resource(FramepaceSettings {
            limiter: window_settings.frame_limiter(),
        })
        .configure_sets(
            PostUpdate,
            EguiPostUpdateSet::EndPass.before(TransformSystems::Propagate),
        );

    configure_client_schedule(&mut app);

    app.add_systems(Startup, setup_scene)
        .add_systems(Startup, setup_voice_system)
        .add_systems(
            EguiPrimaryContextPass,
            (ui_system, button_sound_system, inventory_sound_system).chain(),
        )
        .add_systems(
            Update,
            chat_shortcut_system.in_set(ClientSystemSet::ChatShortcut),
        )
        .add_systems(
            Update,
            toggle_pause_system.in_set(ClientSystemSet::PauseToggle),
        )
        // Both run alongside the pause toggle so they share its place
        // in the schedule (input intake before gameplay simulation).
        .add_systems(
            Update,
            (
                sync_furnace_open_flag_system,
                close_furnace_on_escape_system,
            )
                .in_set(ClientSystemSet::PauseToggle),
        )
        .add_systems(
            Update,
            toggle_inventory_system.in_set(ClientSystemSet::InventoryToggle),
        )
        .add_systems(
            Update,
            toggle_crafting_system.in_set(ClientSystemSet::CraftingToggle),
        )
        .add_systems(Update, toggle_perf_stats_system)
        .add_systems(
            Update,
            center_cursor_on_focus_system.in_set(ClientSystemSet::Focus),
        )
        .add_systems(Update, update_cursor_system.in_set(ClientSystemSet::Cursor))
        .add_systems(Update, mouse_look_system.in_set(ClientSystemSet::Look))
        .add_systems(Update, client_input_system.in_set(ClientSystemSet::Input))
        .add_systems(
            Update,
            update_tool_swap_state_system.in_set(ClientSystemSet::ToolSwap),
        )
        .add_systems(
            Update,
            gameplay_inventory_shortcuts_system.in_set(ClientSystemSet::InventoryShortcuts),
        )
        .add_systems(Update, network_tick_system.in_set(ClientSystemSet::Network))
        .add_systems(
            Update,
            // Surfaces queued error toasts after the network tick has had
            // its chance to enqueue any. Sharing the Network set keeps
            // toast latency to one frame for UI/input writers and zero
            // frames for writers in network_tick_system itself.
            surface_client_error_toasts_system
                .in_set(ClientSystemSet::Network)
                .after(network_tick_system),
        )
        .add_systems(
            Update,
            session_shutdown_poll_system.in_set(ClientSystemSet::SessionShutdown),
        )
        .add_systems(Update, app_quit_system.in_set(ClientSystemSet::Quit))
        .add_systems(
            Update,
            apply_display_settings_system.in_set(ClientSystemSet::Display),
        )
        .add_systems(
            Update,
            save_client_settings_system.in_set(ClientSystemSet::SettingsSave),
        )
        .add_systems(Update, sync_view_radius_system)
        .add_systems(Update, chunk_overlay_system)
        .add_systems(
            Update,
            apply_world_scene_system.in_set(ClientSystemSet::WorldScene),
        )
        .add_systems(
            Update,
            apply_snapshot_system.in_set(ClientSystemSet::Players),
        )
        .add_systems(
            Update,
            apply_dropped_items_system.in_set(ClientSystemSet::DroppedItems),
        )
        .add_systems(
            Update,
            apply_resource_nodes_system.in_set(ClientSystemSet::ResourceNodes),
        )
        .add_systems(
            Update,
            apply_deployed_entities_system.in_set(ClientSystemSet::DeployedEntities),
        )
        .add_systems(
            Update,
            placement_input_system.in_set(ClientSystemSet::PlacementInput),
        )
        .add_systems(
            Update,
            update_placement_ghost_system.in_set(ClientSystemSet::PlacementGhost),
        )
        // Camera follow runs only in PostUpdate, before transform propagation.
        // Running in both Update and PostUpdate would advance the impact-kick
        // timer twice per frame (halving its visible duration) and write a
        // stale camera transform that other Update-phase systems would read.
        .add_systems(
            PostUpdate,
            camera_follow_system.before(TransformSystems::Propagate),
        )
        .add_systems(
            Update,
            apply_held_item_visual_system.in_set(ClientSystemSet::HeldItem),
        )
        .add_systems(Update, update_sky_system.in_set(ClientSystemSet::Sky))
        .add_systems(
            Update,
            update_pickup_target_system.in_set(ClientSystemSet::PickupTarget),
        )
        .add_systems(
            Update,
            play_impact_sounds_system.in_set(ClientSystemSet::ImpactSounds),
        )
        .add_systems(
            Update,
            play_transition_stingers_system.in_set(ClientSystemSet::TransitionStingers),
        )
        .add_systems(
            Update,
            play_footsteps_system.in_set(ClientSystemSet::Footsteps),
        )
        // Central audio bus: drains PlaySound events and spawns the
        // audio entities. Must run after every system that writes
        // PlaySound (impact, footsteps, node death, button) so a single
        // frame's worth of events is one round-trip.
        .add_systems(
            Update,
            play_sounds_system.in_set(ClientSystemSet::PlaySounds),
        )
        .add_systems(
            Update,
            tick_audio_faders_system.in_set(ClientSystemSet::AudioFaderTick),
        )
        .add_systems(
            Update,
            manage_ambient_beds_system.in_set(ClientSystemSet::AmbientBeds),
        )
        .add_systems(
            Update,
            manage_ambient_emitters_system.in_set(ClientSystemSet::AmbientEmitters),
        )
        .add_systems(
            Update,
            spawn_impact_effects_system.in_set(ClientSystemSet::ImpactEffectsSpawn),
        )
        .add_systems(
            Update,
            tick_impact_chips_system.in_set(ClientSystemSet::ImpactEffectsTick),
        )
        .add_systems(
            Update,
            tick_felling_trees_system.in_set(ClientSystemSet::NodeDeathTick),
        )
        .add_systems(
            Update,
            // Same phase as the falling-tree tick — both ride the
            // post-snapshot scene update window and write to local
            // transforms that no other system reads after them.
            tick_resource_node_pop_in_system.in_set(ClientSystemSet::NodeDeathTick),
        )
        .add_systems(
            Update,
            main_menu_music_system.in_set(ClientSystemSet::MainMenuMusic),
        )
        .add_systems(
            Update,
            menu_backdrop_camera_system.in_set(ClientSystemSet::MenuBackdropCamera),
        )
        .add_systems(
            Update,
            (auto_connect_start_system, auto_connect_poll_system)
                .chain()
                .in_set(ClientSystemSet::AutoConnect),
        )
        .add_systems(
            Update,
            manage_voice_capture_system.in_set(ClientSystemSet::VoiceCaptureManage),
        )
        .add_systems(
            Update,
            transmit_voice_system.in_set(ClientSystemSet::VoiceTransmit),
        )
        .add_systems(
            Update,
            receive_voice_system.in_set(ClientSystemSet::VoiceReceive),
        )
        .add_systems(
            Update,
            apply_voice_settings_system.in_set(ClientSystemSet::VoiceSettings),
        )
        .add_systems(
            Update,
            apply_test_mode_overrides_system.in_set(ClientSystemSet::TestModeApply),
        )
        .add_systems(
            Update,
            reposition_test_window_system.in_set(ClientSystemSet::TestWindowReposition),
        )
        .add_systems(
            Update,
            (
                screen_viewed_system,
                session_started_system,
                session_ended_system,
                error_relay_system,
            )
                .chain()
                .in_set(ClientSystemSet::Analytics),
        )
        .run();

    Ok(())
}
