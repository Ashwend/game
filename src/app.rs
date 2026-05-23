mod embedded_assets;
mod scene;
mod state;
mod systems;
mod ui;
mod voice;

pub(crate) use embedded_assets::asset_path as embedded_asset_path;

use std::net::SocketAddr;

use anyhow::Result;
use bevy::{
    diagnostic::FrameTimeDiagnosticsPlugin, prelude::*, transform::TransformSystems,
    window::WindowPosition, winit::WinitSettings,
};
use bevy_egui::{EguiPlugin, EguiPostUpdateSet, EguiPrimaryContextPass};

use crate::{
    save::WorldStore,
    steam::{OfflineSteamBackend, SteamBackend},
};

use self::voice::{
    IncomingVoiceMessage, apply_voice_settings_system, receive_voice_system, setup_voice_system,
    transmit_voice_system,
};

use self::{
    scene::{apply_world_scene_system, setup_scene, update_sky_system},
    state::{
        ClientErrorToast, ClientRuntime, ClientSettingsStore, GatherInputState, InventoryUiState,
        LookState, MenuBackdropVisibility, MenuState, OptionsUiState, PickupTargetState,
        RemoteImpactEvent, SaveStore, SessionShutdownTasks, SteamUser, TestModeConfig, ToastState,
        ToolSwapState,
    },
    systems::{
        AutoConnectRequest, CameraImpactKick, CameraMotionEffects, ClientSystemSet,
        DroppedItemEntities, RemotePlayerEntities, ResourceNodeEntities, app_quit_system,
        apply_display_settings_system, apply_dropped_items_system, apply_held_item_visual_system,
        apply_resource_nodes_system, apply_snapshot_system, apply_test_mode_overrides_system,
        auto_connect_poll_system, auto_connect_start_system, camera_follow_system,
        center_cursor_on_focus_system, chat_shortcut_system, client_input_system,
        gameplay_inventory_shortcuts_system, main_menu_music_system, menu_backdrop_camera_system,
        mouse_look_system, network_tick_system, play_impact_sounds_system,
        reposition_test_window_system, save_client_settings_system, session_shutdown_poll_system,
        setup_impact_sound_assets, spawn_impact_effects_system, surface_client_error_toasts_system,
        tick_felling_trees_system, tick_impact_chips_system, tick_resource_node_pop_in_system,
        toggle_inventory_system, toggle_pause_system, update_cursor_system,
        update_pickup_target_system, update_tool_swap_state_system,
    },
    ui::{ButtonSoundRequests, button_sound_system, setup_button_sound_assets, ui_system},
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
    ClientSystemSet::Camera,
    ClientSystemSet::HeldItem,
    ClientSystemSet::Sky,
    ClientSystemSet::PickupTarget,
    ClientSystemSet::ImpactSounds,
    ClientSystemSet::ImpactEffectsSpawn,
    ClientSystemSet::ImpactEffectsTick,
    ClientSystemSet::NodeDeathTick,
    ClientSystemSet::VoiceTransmit,
    ClientSystemSet::VoiceReceive,
    ClientSystemSet::VoiceSettings,
    ClientSystemSet::TestModeApply,
    ClientSystemSet::TestWindowReposition,
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
        // `continuous()` rather than `desktop_app()`: the menu backdrop
        // camera pans continuously (see `menu_backdrop_camera_system`) and
        // needs steady frames to look smooth. Switching to reactive update
        // would chop the animation. If the backdrop is later gated behind
        // `MenuBackdropVisibility::is_active(...)` we can revisit and use
        // `desktop_app()` (or a reactive-low-power variant) when no panning
        // animation is on-screen.
        .insert_resource(WinitSettings::continuous())
        .init_resource::<ButtonSoundRequests>()
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
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        // Self-contained binary: every shipped sound is registered into
        // Bevy's `embedded` asset source so we don't have to ship a
        // sibling `assets/` folder. Must come after DefaultPlugins so
        // AssetPlugin (and therefore `EmbeddedAssetRegistry`) exists.
        .add_plugins(embedded_assets::EmbeddedAssetsPlugin)
        .add_plugins(EguiPlugin::default())
        .configure_sets(
            PostUpdate,
            EguiPostUpdateSet::EndPass.before(TransformSystems::Propagate),
        );

    configure_client_schedule(&mut app);

    app.add_systems(Startup, setup_scene)
        .add_systems(Startup, setup_button_sound_assets)
        .add_systems(Startup, setup_impact_sound_assets)
        .add_systems(Startup, setup_voice_system)
        .add_systems(
            EguiPrimaryContextPass,
            (ui_system, button_sound_system).chain(),
        )
        .add_systems(
            Update,
            chat_shortcut_system.in_set(ClientSystemSet::ChatShortcut),
        )
        .add_systems(
            Update,
            toggle_pause_system.in_set(ClientSystemSet::PauseToggle),
        )
        .add_systems(
            Update,
            toggle_inventory_system.in_set(ClientSystemSet::InventoryToggle),
        )
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
        .run();

    Ok(())
}
