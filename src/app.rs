mod scene;
mod state;
mod systems;
mod ui;

use anyhow::Result;
use bevy::{
    diagnostic::FrameTimeDiagnosticsPlugin, prelude::*, transform::TransformSystems,
    winit::WinitSettings,
};
use bevy_egui::{EguiPlugin, EguiPostUpdateSet, EguiPrimaryContextPass};

use crate::{
    save::WorldStore,
    steam::{OfflineSteamBackend, SteamBackend},
};

use self::{
    scene::{apply_world_scene_system, setup_scene},
    state::{
        ClientRuntime, ClientSettingsStore, GatherInputState, InventoryUiState, LookState,
        MenuBackdropVisibility, MenuState, PickupTargetState, SaveStore, SessionShutdownTasks,
        SteamUser,
    },
    systems::{
        CameraImpactKick, ClientSystemSet, app_quit_system, apply_display_settings_system,
        apply_dropped_items_system, apply_held_item_visual_system, apply_resource_nodes_system,
        apply_snapshot_system, camera_follow_system, center_cursor_on_focus_system,
        chat_shortcut_system, client_input_system, gameplay_inventory_shortcuts_system,
        main_menu_music_system, menu_backdrop_camera_system, mouse_look_system,
        network_tick_system, save_client_settings_system, session_shutdown_poll_system,
        spawn_impact_effects_system, tick_impact_chips_system, toggle_inventory_system,
        toggle_pause_system, update_cursor_system, update_pickup_target_system,
    },
    ui::{ButtonSoundRequests, button_sound_system, setup_button_sound_assets, ui_system},
};

pub(crate) const EYE_HEIGHT: f32 = 1.62;
pub(crate) const PLAYER_VISUAL_CENTER_Y: f32 = 0.9;

pub fn run_app() -> Result<()> {
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

    App::new()
        .insert_resource(ClearColor(Color::srgb(0.015, 0.018, 0.023)))
        .insert_resource(SaveStore(store))
        .insert_resource(SteamUser(user))
        .insert_resource(settings_store)
        .insert_resource(settings)
        .insert_resource(MenuState::default())
        .insert_resource(MenuBackdropVisibility::default())
        .insert_resource(ClientRuntime::default())
        .insert_resource(SessionShutdownTasks::default())
        .insert_resource(InventoryUiState::default())
        .insert_resource(PickupTargetState::default())
        .insert_resource(GatherInputState::default())
        .insert_resource(CameraImpactKick::default())
        .insert_resource(LookState::default())
        .insert_resource(WinitSettings::continuous())
        .init_resource::<ButtonSoundRequests>()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Game".to_owned(),
                    resolution: (
                        window_settings.resolution.width,
                        window_settings.resolution.height,
                    )
                        .into(),
                    present_mode: window_settings.present_mode(),
                    mode: window_settings.window_mode(None),
                    resizable: false,
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(EguiPlugin::default())
        .configure_sets(
            PostUpdate,
            EguiPostUpdateSet::EndPass.before(TransformSystems::Propagate),
        )
        .configure_sets(
            Update,
            (
                ClientSystemSet::Focus,
                ClientSystemSet::ChatShortcut,
                ClientSystemSet::PauseToggle,
                ClientSystemSet::InventoryToggle,
                ClientSystemSet::Cursor,
                ClientSystemSet::Look,
                ClientSystemSet::Input,
                ClientSystemSet::InventoryShortcuts,
                ClientSystemSet::Network,
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
                ClientSystemSet::PickupTarget,
            )
                .chain(),
        )
        .configure_sets(
            Update,
            (
                ClientSystemSet::PickupTarget,
                ClientSystemSet::ImpactEffectsSpawn,
                ClientSystemSet::ImpactEffectsTick,
            )
                .chain(),
        )
        .configure_sets(
            Update,
            (
                ClientSystemSet::MainMenuMusic,
                ClientSystemSet::MenuBackdropCamera,
            )
                .chain(),
        )
        .add_systems(Startup, setup_scene)
        .add_systems(Startup, setup_button_sound_assets)
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
            gameplay_inventory_shortcuts_system.in_set(ClientSystemSet::InventoryShortcuts),
        )
        .add_systems(Update, network_tick_system.in_set(ClientSystemSet::Network))
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
        .add_systems(Update, camera_follow_system.in_set(ClientSystemSet::Camera))
        .add_systems(
            PostUpdate,
            camera_follow_system.before(TransformSystems::Propagate),
        )
        .add_systems(
            Update,
            apply_held_item_visual_system.in_set(ClientSystemSet::HeldItem),
        )
        .add_systems(
            Update,
            update_pickup_target_system.in_set(ClientSystemSet::PickupTarget),
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
            main_menu_music_system.in_set(ClientSystemSet::MainMenuMusic),
        )
        .add_systems(
            Update,
            menu_backdrop_camera_system.in_set(ClientSystemSet::MenuBackdropCamera),
        )
        .run();

    Ok(())
}
