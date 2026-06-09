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
    audio::{GlobalVolume, Volume},
    diagnostic::FrameTimeDiagnosticsPlugin,
    prelude::*,
    transform::TransformSystems,
    window::WindowPosition,
    winit::WinitSettings,
};
use bevy_egui::{EguiPlugin, EguiPostUpdateSet, EguiPreUpdateSet, EguiPrimaryContextPass};
use bevy_framepace::{FramepacePlugin, FramepaceSettings};

use crate::{
    analytics::AnalyticsPlugin,
    auth::{
        bypass_identity_from_env,
        workos::{self, WorkosConfig},
    },
    net::{ClientNetworkPlugin, LightyearProtocolPlugin, client_plugins},
    save::WorldStore,
    update::UpdatePlugin,
};

use self::voice::{
    IncomingVoiceMessage, VoiceDisabled, apply_voice_settings_system, manage_voice_capture_system,
    receive_voice_system, setup_voice_system, transmit_voice_system,
};

use self::{
    audio::{
        AudioPlugin, main_menu_music_system, manage_ambient_beds_system,
        manage_ambient_emitters_system, play_footsteps_system, play_impact_sounds_system,
        play_sounds_system, play_transition_stingers_system, tick_audio_faders_system,
    },
    scene::{
        GrassInstancingPlugin, GrassMaterial, GrassState, apply_world_scene_system, setup_scene,
        stream_grass_system, update_sky_system,
    },
    state::{
        AuthFlow, ClientErrorToast, ClientRuntime, ClientSettings, ClientSettingsStore,
        CombatFeedbackState, CraftingHudState, CraftingUiState, CurrentUser,
        DeployablePlacementState, GatherInputState, InventoryUiState, LocalPlayerState, LookState,
        MenuBackdropVisibility, MenuState, OptionsUiState, PickupTargetState, PredictionState,
        RemoteImpactEvent, SaveStore, SessionShutdownTasks, TestModeConfig, ToastState,
        ToolSwapState, WorkosAuth, apply_prediction_overlay_system,
        update_local_player_state_system,
    },
    systems::{
        AutoConnectRequest, CameraImpactKick, CameraMotionEffects, ClientSystemSet,
        DroppedItemEntities, LastTrackedScreen, LootBagEntities, PendingSessionEndReason,
        RemotePlayerEntities, ResourceNodeEntities, SessionTracker, animate_furnace_fire_system,
        app_quit_system, apply_deployed_entities_system, apply_display_settings_system,
        apply_dropped_items_system, apply_graphics_settings_system, apply_held_item_visual_system,
        apply_loot_bags_system, apply_resource_nodes_system, apply_snapshot_system,
        apply_test_mode_overrides_system, apply_update_system, auto_connect_poll_system,
        auto_connect_start_system, camera_follow_system, center_cursor_on_focus_system,
        chat_shortcut_system, chunk_overlay_system, client_input_system,
        close_furnace_on_escape_system, close_loot_bag_on_escape_system, drive_auth_flow_system,
        error_relay_system, flush_settings_on_exit_system, gameplay_inventory_shortcuts_system,
        maintain_world_grid_system, menu_backdrop_camera_system, mouse_look_system,
        multiplayer_test_owns_window, network_tick_system, placement_input_system,
        reposition_test_window_system, save_client_settings_system, screen_viewed_system,
        session_ended_system, session_shutdown_poll_system, session_started_system,
        spawn_impact_effects_system, surface_client_error_toasts_system,
        sync_furnace_open_flag_system, sync_loot_bag_open_flag_system, sync_view_radius_system,
        tick_combat_feedback_system, tick_felling_trees_system, tick_furnace_particles_system,
        tick_impact_chips_system, tick_resource_node_pop_in_system, toggle_crafting_system,
        toggle_inventory_system, toggle_pause_system, toggle_perf_stats_system,
        update_cursor_system, update_link_ping_system, update_pickup_target_system,
        update_placement_ghost_system, update_tool_swap_state_system,
    },
    ui::{
        ButtonSoundRequests, InventorySoundRequests, apply_ui_scale_system, button_sound_system,
        install_egui_fonts_system, inventory_sound_system, setup_item_icons, ui_system,
    },
};

// Agent automation surface (control socket + off-screen capture) is dev-only:
// gated on `debug_assertions` so it compiles out of shipped release builds and
// can't be driven by a bot in the final game.
#[cfg(debug_assertions)]
use self::systems::{HeadlessCapture, insert_capture_target, redirect_camera_to_capture};

#[cfg(all(unix, debug_assertions))]
use self::systems::{ClientControlSocket, drain_control_socket};

#[cfg(all(debug_assertions, target_os = "macos"))]
use self::systems::relinquish_macos_focus_system;

pub(crate) const EYE_HEIGHT: f32 = 1.62;
pub(crate) const PLAYER_VISUAL_CENTER_Y: f32 = 0.9;

/// Authoritative Update-phase order for client systems.
///
/// One ordered list, one source of truth: every consecutive pair becomes an
/// `after(prev)` edge in the schedule. Add new sets here in the slot that
/// matches their data dependency, not in a side chain. The phases below are
/// purely for human navigation, the runtime only sees the flat list.
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
    ClientSystemSet::LocalPlayerSync,
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
    ClientSystemSet::WorldGridRebuild,
    ClientSystemSet::SessionShutdown,
    ClientSystemSet::Quit,
    ClientSystemSet::Display,
    ClientSystemSet::SettingsSave,
    ClientSystemSet::WorldScene,
    ClientSystemSet::Players,
    ClientSystemSet::DroppedItems,
    ClientSystemSet::ResourceNodes,
    ClientSystemSet::Grass,
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
    ClientSystemSet::FurnaceFireAnimate,
    ClientSystemSet::FurnaceParticleTick,
    ClientSystemSet::NodeDeathTick,
    // Transition stingers (e.g. world-join) ride on `MenuState`
    // edge-detection, not on the gameplay event stream, slotted just
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

/// Menu-only systems form their own short chain, independent of the main
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

    let settings_store = ClientSettingsStore::platform_default()?;
    let settings = settings_store.load().unwrap_or_else(|error| {
        eprintln!("could not load client settings: {error:#}");
        Default::default()
    });
    let test_mode = TestModeConfig::from_env();
    // Dev-only off-screen capture: when set, the primary camera renders into an
    // image instead of the window and the window comes up hidden so frames keep
    // advancing without an on-screen surface. See `systems::headless_capture`.
    // Compiled out of release builds; always `None` there so the window stays
    // visible and the capture wiring below is dropped entirely.
    #[cfg(debug_assertions)]
    let headless_capture = HeadlessCapture::resolution_from_env();
    #[cfg(not(debug_assertions))]
    let headless_capture: Option<(u32, u32)> = None;
    // Agent-driven sessions (off-screen capture and/or the control socket)
    // should come up in the background: the primary window is created unfocused
    // (winit `with_active(false)`), so launching the agent doesn't steal focus
    // or jump in front of whatever the user is doing. Normal `./cli client`
    // play is untouched. Always `false` in release (agent paths compile out).
    #[cfg(all(unix, debug_assertions))]
    let agent_driven =
        headless_capture.is_some() || std::env::var_os(ClientControlSocket::ENV).is_some();
    #[cfg(all(not(unix), debug_assertions))]
    let agent_driven = headless_capture.is_some();
    #[cfg(not(debug_assertions))]
    let agent_driven = false;
    // A plain `client` launch must sign in through WorkOS before the title
    // screen appears; only the test / `--connect` path bypasses the gate with
    // an identity injected from the environment.
    let bypass_auth = auto_connect.is_some();

    let mut app = App::new();
    if let Some(addr) = auto_connect {
        app.insert_resource(AutoConnectRequest { addr });
    }
    if bypass_auth {
        // Test / `--connect`: skip the WorkOS gate with the identity injected
        // via `GAME_ACCOUNT_ID` / `GAME_PLAYER_NAME` so spawned windows land
        // in-world without a browser.
        app.insert_resource(CurrentUser(bypass_identity_from_env()));
        app.insert_resource(AuthFlow::Authenticated);
    } else {
        let workos = WorkosConfig::load();
        app.insert_resource(if workos::has_stored_session() {
            // A stored session: verify (silently refresh) behind the spinner.
            AuthFlow::Verifying(workos::begin_restore(&workos))
        } else {
            AuthFlow::LoggedOut { error: None }
        });
        app.insert_resource(WorkosAuth(workos));
    }
    insert_client_resources(&mut app, store, settings_store, &settings, &test_mode);
    add_window_and_default_plugins(
        &mut app,
        &test_mode,
        &settings,
        headless_capture,
        agent_driven,
    );
    add_third_party_plugins(&mut app, &settings);

    configure_client_schedule(&mut app);

    install_dev_agent_wiring(&mut app, headless_capture, agent_driven);

    add_client_systems(&mut app);

    app.run();

    Ok(())
}

/// Client-state resource and message registrations.
///
/// Roughly 45 `insert_resource`/`init_resource`/`add_message` calls plus the
/// `ClearColor`/`WinitSettings` setup. Order is preserved verbatim: these must
/// run before `add_window_and_default_plugins` so the chain matches the original
/// top-to-bottom sequence.
fn insert_client_resources(
    app: &mut App,
    store: WorldStore,
    settings_store: ClientSettingsStore,
    settings: &ClientSettings,
    test_mode: &TestModeConfig,
) {
    app.insert_resource(ClearColor(Color::srgb(0.015, 0.018, 0.023)))
        .insert_resource(SaveStore(store))
        .insert_resource(settings_store)
        .insert_resource(settings.clone())
        .insert_resource(MenuState::default())
        .insert_resource(OptionsUiState::default())
        .insert_resource(test_mode.clone())
        .insert_resource(MenuBackdropVisibility::default())
        .insert_resource(ClientRuntime::default())
        .insert_resource(LocalPlayerState::default())
        .init_resource::<PredictionState>()
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
        .insert_resource(CombatFeedbackState::default())
        .insert_resource(DroppedItemEntities::default())
        .insert_resource(LootBagEntities::default())
        .insert_resource(ResourceNodeEntities::default())
        .insert_resource(GrassState::default())
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
        .add_message::<IncomingVoiceMessage>();
}

/// `DefaultPlugins` plus the `WindowPlugin` configuration, including the
/// test-window / headless-capture / agent-driven branches that drive resolution,
/// window mode, visibility, and focus at startup.
fn add_window_and_default_plugins(
    app: &mut App,
    test_mode: &TestModeConfig,
    settings: &ClientSettings,
    headless_capture: Option<(u32, u32)>,
    agent_driven: bool,
) {
    let window_settings = settings.display;
    app.add_plugins(
        DefaultPlugins
            // Mirror every log line into <data_dir>/logs/ashwend.log so a
            // packaged release (no attached terminal) still leaves something
            // to inspect. See `crate::logging`.
            .set(bevy::log::LogPlugin {
                custom_layer: crate::logging::install_file_layer,
                ..default()
            })
            .set(WindowPlugin {
                // `multiplayer-test` overrides the window resolution via
                // env vars and the actual position is set after the
                // primary monitor has been queried, see
                // `reposition_test_window_system`. Trying to centre at
                // startup would need a screen-size guess and that's exactly
                // what we'd get wrong on the dev's actual monitor.
                primary_window: Some(Window {
                    title: "Ashwend".to_owned(),
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
                        // applies, fullscreen would ignore it.
                        bevy::window::WindowMode::Windowed
                    } else {
                        window_settings.window_mode(None)
                    },
                    resizable: false,
                    // Headless capture renders to an off-screen image, so the
                    // window is created hidden. winit then runs the schedule
                    // each cycle (its `all_invisible` path) so the capture image
                    // stays fresh without an on-screen surface to throttle.
                    visible: headless_capture.is_none(),
                    // Agent-driven sessions come up unfocused so the window
                    // doesn't steal focus or jump in front of other windows.
                    focused: !agent_driven,
                    ..default()
                }),
                ..default()
            }),
    );
}

/// Third-party plugin registration: frame-time diagnostics, the Lightyear
/// client protocol/network stack, the optional replication-trace and profile
/// diagnostics, embedded assets, the grass material, audio, egui, frame pacing,
/// analytics, the update checker, and the egui end-pass ordering.
fn add_third_party_plugins(app: &mut App, settings: &ClientSettings) {
    let window_settings = settings.display;
    app
        // 480 sample history: ~1 second at 500 FPS, ~4 seconds at 120 FPS.
        // The perf HUD pulls p99/max from this window so the player sees
        // hitches that the smoothed FPS number hides, 120 samples (default)
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
    #[cfg(feature = "replication-trace")]
    {
        use self::systems::replication_trace::{
            ReplicationTraceState, log_replicated_storage_changes_system,
        };
        app.init_resource::<ReplicationTraceState>()
            .add_systems(Update, log_replicated_storage_changes_system);
    }
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
        // Grass wind material: the custom wind + distance-fade shader, an
        // `ExtendedMaterial<StandardMaterial, GrassWindExtension>`. Now used only
        // by the hay-grass resource node; the cosmetic detail grass moved to the
        // GPU-instanced pipeline below. Client-only (the dedicated server has no
        // render app); after EmbeddedAssetsPlugin so `shaders/grass.wgsl`
        // resolves when hay grass first spawns.
        .add_plugins(MaterialPlugin::<GrassMaterial>::default())
        // GPU-instanced detail grass: the project's one custom render pipeline.
        // Draws one shared blade mesh thousands of times per tile from a per-blade
        // instance buffer, so the field can be far denser than baking every blade
        // into a tile mesh. Client-only; after EmbeddedAssetsPlugin so
        // `shaders/grass_instanced.wgsl` resolves.
        .add_plugins(GrassInstancingPlugin)
        // Audio: registers PlaySound event, SoundLibrary, FootstepState,
        // and the global ambient-zone resource. Must come after
        // EmbeddedAssetsPlugin so the asset paths it loads at startup
        // resolve through the embedded source.
        .add_plugins(AudioPlugin)
        .add_plugins(EguiPlugin::default())
        // Software frame pacing. With this plugin running we can leave
        // `PresentMode` at `Immediate` everywhere and rely on a CPU-side
        // sleep to cap the frame rate, `Fifo`/`AutoVsync` are not
        // reliable on macOS Metal (flicker, no cap respectively). The
        // limiter starts in whatever state the saved settings ask for;
        // `apply_display_settings_system` keeps it in sync when the user
        // toggles vsync at runtime.
        .add_plugins(FramepacePlugin)
        // Analytics. Disabled by default; reads `analytics.local.toml` /
        // `POSTHOG_*` env vars at startup. Client-only, dedicated server
        // and admin CLI never load this plugin.
        .add_plugins(AnalyticsPlugin)
        // Update checker + self-updater. Client-only; spawns a background
        // thread that asks GitHub for the latest release on boot. Disabled
        // implicitly when offline (the check just reports "up to date").
        .add_plugins(UpdatePlugin)
        .insert_resource(FramepaceSettings {
            limiter: window_settings.frame_limiter(),
        })
        .configure_sets(
            PostUpdate,
            EguiPostUpdateSet::EndPass.before(TransformSystems::Propagate),
        );
}

/// Dev-only agent-automation wiring: voice/volume muting for agent-driven runs,
/// off-screen headless capture, the Unix control socket, and macOS focus
/// relinquish. The agent-mute block runs in every build; the capture / socket /
/// focus blocks keep their original `cfg` gates so they compile out of release.
fn install_dev_agent_wiring(
    app: &mut App,
    // Only consumed by the `debug_assertions`-gated capture block below; in
    // release every reader compiles out and the value is always `None`.
    #[cfg_attr(not(debug_assertions), allow(unused_variables))] headless_capture: Option<(
        u32,
        u32,
    )>,
    agent_driven: bool,
) {
    // Agent-driven sessions don't exercise voice chat, so disable it entirely.
    // This keeps automated runs from opening the microphone, which on macOS
    // forces a Bluetooth headset out of A2DP into low-quality HFP mode. We also
    // mute the global volume so a headless/automated run is silent, there's no
    // one listening. Muting (rather than disabling the audio plugin) keeps the
    // pipeline intact, so audio sinks still despawn normally.
    if agent_driven {
        app.insert_resource(VoiceDisabled);
        app.insert_resource(GlobalVolume::new(Volume::Linear(0.0)));
    }

    // Dev-only off-screen capture: allocate the render-target image, insert the
    // resource the screenshot path keys off of, and point `MainCamera` at it
    // once the scene spawns. After `DefaultPlugins`, so `Assets<Image>` exists.
    // The whole block compiles out of release builds.
    #[cfg(debug_assertions)]
    if let Some((width, height)) = headless_capture {
        insert_capture_target(app, width, height);
        app.add_systems(Startup, redirect_camera_to_capture.after(setup_scene));
        eprintln!("headless capture enabled: rendering to {width}x{height} off-screen image");
    }

    // Dev-only client control socket: bound only when GAME_CONTROL_SOCKET names
    // a path, and only in dev builds (`debug_assertions`), so shipped release
    // builds don't even contain the code, a bot can't drive the final game by
    // setting the env var. Lets an agent drive the client for automated testing
    // (screenshot / command / state dump).
    #[cfg(all(unix, debug_assertions))]
    if let Some(path) = std::env::var_os(ClientControlSocket::ENV) {
        match ClientControlSocket::bind(std::path::PathBuf::from(path)) {
            Ok(socket) => {
                app.insert_resource(socket);
                app.add_systems(Update, drain_control_socket);
                eprintln!("client control socket listening (GAME_CONTROL_SOCKET)");
            }
            Err(error) => eprintln!("could not bind client control socket: {error:#}"),
        }
    }

    // Dev-only (macOS): an agent-driven launch should not steal focus. winit
    // activates the app on launch regardless of the window's `focused` flag, so
    // on the first frame we drop to a background accessory app and hand focus
    // back. Other platforms get unfocused spawn from `focused: false` above.
    #[cfg(all(debug_assertions, target_os = "macos"))]
    if agent_driven {
        app.add_systems(Startup, relinquish_macos_focus_system);
    }
}

/// All client system registrations across the Startup, PreUpdate, Update,
/// PostUpdate, and egui passes. Split by phase into the helpers below; the order
/// of those calls reproduces the original top-to-bottom registration sequence.
fn add_client_systems(app: &mut App) {
    add_startup_and_egui_systems(app);
    add_input_systems(app);
    add_network_systems(app);
    add_display_systems(app);
    add_scene_systems(app);
    add_audio_systems(app);
    add_menu_and_auth_systems(app);
    add_voice_systems(app);
    add_test_and_analytics_systems(app);
}

/// Startup spawns plus the egui passes (the primary-context UI chain and the
/// font installer that must run before bevy_egui's `begin_pass`).
fn add_startup_and_egui_systems(app: &mut App) {
    app.add_systems(Startup, setup_scene)
        .add_systems(Startup, setup_voice_system)
        .add_systems(Startup, setup_item_icons)
        .add_systems(
            EguiPrimaryContextPass,
            (ui_system, button_sound_system, inventory_sound_system).chain(),
        )
        // Install the title typeface before bevy_egui's `begin_pass` so the
        // named font family is bound when `ui_system` lays out the menu title
        // on the very first frame. `set_fonts` is applied lazily at the next
        // `begin_pass`, so running this inside the context pass would leave the
        // family unbound for one frame and panic the layout.
        .add_systems(
            PreUpdate,
            install_egui_fonts_system
                .after(EguiPreUpdateSet::ProcessInput)
                .before(EguiPreUpdateSet::BeginPass),
        );
}

/// Local-player sync, input/UI shortcut intake, and the tool-swap animation.
fn add_input_systems(app: &mut App) {
    app.add_systems(
        Update,
        // Sync the local player's replicated components, then fold the
        // optimistic prediction overlay onto the fresh inventory. Chained
        // so the overlay always reads the just-synced replicated base.
        (
            update_local_player_state_system,
            apply_prediction_overlay_system,
        )
            .chain()
            .in_set(ClientSystemSet::LocalPlayerSync),
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
            sync_loot_bag_open_flag_system,
            close_loot_bag_on_escape_system,
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
    );
}

/// Network tick, link ping, error-toast surfacing, session shutdown, quit, and
/// the staged self-update applier.
fn add_network_systems(app: &mut App) {
    app.add_systems(Update, network_tick_system.in_set(ClientSystemSet::Network))
        .add_systems(
            Update,
            update_link_ping_system.in_set(ClientSystemSet::Network),
        )
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
        // Applies a staged self-update: saves any open world, then launches the
        // updater and quits. Reacts to `UpdateState::Applying` set by the modal.
        .add_systems(Update, apply_update_system);
}

/// Display/graphics/settings application plus the always-on view-radius and
/// chunk-overlay helpers.
fn add_display_systems(app: &mut App) {
    app.add_systems(
        Update,
        (
            // Gated off for multiplayer-test windows: there the test
            // harness owns the window (it may sit borderless-fullscreen on
            // a non-primary monitor), so letting the saved display
            // settings re-assert themselves would fight the placement.
            apply_display_settings_system.run_if(not(multiplayer_test_owns_window)),
            apply_ui_scale_system,
        )
            .in_set(ClientSystemSet::Display),
    )
    .add_systems(
        Update,
        apply_graphics_settings_system.in_set(ClientSystemSet::Display),
    )
    .add_systems(
        Update,
        save_client_settings_system.in_set(ClientSystemSet::SettingsSave),
    )
    .add_systems(Update, sync_view_radius_system)
    // In `Last`, not `Update`: the debounced save never fires while the options
    // panel is open (egui marks settings changed every frame), so quitting from
    // the settings screen would drop the change. `Last` is also the only place
    // that observes the window-close `AppExit`, see the analytics drain.
    .add_systems(Last, flush_settings_on_exit_system)
    .add_systems(Update, chunk_overlay_system);
}

/// Combat-feedback HUD, scene application from the freshest snapshot, world-grid
/// rebuild, placement preview/input, camera follow, the held-item visual, and
/// the sky update.
fn add_scene_systems(app: &mut App) {
    app.add_systems(Update, tick_combat_feedback_system)
        .add_systems(
            Update,
            crate::app::ui::floating_text::tick_floating_damage_system,
        )
        .add_systems(Update, crate::app::ui::tick_death_splash_system)
        // Runs after the replicated-player reconcile so the
        // DyingPlayer component a kill stamps onto the visual
        // entity is in place before the tick advances its
        // animation. The reconciler lives in `ClientSystemSet::Players`;
        // ordering against the set (not the function) avoids the
        // transitive-cycle panic you'd otherwise get from naming the
        // function directly. No `before` constraint, the death
        // animation only mutates the visual entity's transform +
        // material alpha, neither of which the later sets read.
        .add_systems(
            Update,
            crate::app::systems::tick_dying_players_system.after(ClientSystemSet::Players),
        )
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
            apply_loot_bags_system.in_set(ClientSystemSet::DroppedItems),
        )
        .add_systems(
            Update,
            apply_resource_nodes_system.in_set(ClientSystemSet::ResourceNodes),
        )
        .add_systems(Update, stream_grass_system.in_set(ClientSystemSet::Grass))
        .add_systems(
            Update,
            apply_deployed_entities_system.in_set(ClientSystemSet::DeployedEntities),
        )
        .add_systems(
            Update,
            maintain_world_grid_system.in_set(ClientSystemSet::WorldGridRebuild),
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
        .add_systems(Update, update_sky_system.in_set(ClientSystemSet::Sky));
}

/// Look-target scan, the impact-effect pipeline, furnace/node ticks, transition
/// stingers, and the central audio drain plus faders and ambient management.
fn add_audio_systems(app: &mut App) {
    app.add_systems(
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
        animate_furnace_fire_system.in_set(ClientSystemSet::FurnaceFireAnimate),
    )
    .add_systems(
        Update,
        tick_furnace_particles_system.in_set(ClientSystemSet::FurnaceParticleTick),
    )
    .add_systems(
        Update,
        tick_felling_trees_system.in_set(ClientSystemSet::NodeDeathTick),
    )
    .add_systems(
        Update,
        // Same phase as the falling-tree tick, both ride the
        // post-snapshot scene update window and write to local
        // transforms that no other system reads after them.
        tick_resource_node_pop_in_system.in_set(ClientSystemSet::NodeDeathTick),
    );
}

/// Main-menu music, menu backdrop camera, auto-connect, and the WorkOS auth gate.
fn add_menu_and_auth_systems(app: &mut App) {
    app.add_systems(
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
    // Polls the in-flight WorkOS login/refresh and advances the auth gate.
    .add_systems(Update, drive_auth_flow_system);
}

/// Voice capture, transmit, receive, and settings systems.
fn add_voice_systems(app: &mut App) {
    app.add_systems(
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
    );
}

/// Test-mode overrides, the test-window reposition, and the analytics observer
/// chain that runs last so it reflects every other system's writes this frame.
fn add_test_and_analytics_systems(app: &mut App) {
    app.add_systems(
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
    );
}
