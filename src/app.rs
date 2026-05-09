mod scene;
mod state;
mod systems;
mod ui;

use anyhow::Result;
use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, prelude::*, transform::TransformSystems};
use bevy_egui::{
    EguiPlugin, EguiPostUpdateSet, EguiPrimaryContextPass, input::egui_wants_any_pointer_input,
};

use crate::{
    save::WorldStore,
    steam::{OfflineSteamBackend, SteamBackend},
};

use self::{
    scene::{apply_world_scene_system, setup_scene},
    state::{
        ClientRuntime, ClientSettingsStore, InventoryUiState, LookState, MenuBackdropVisibility,
        MenuState, PickupTargetState, SaveStore, SteamUser,
    },
    systems::{
        app_quit_system, apply_display_settings_system, apply_dropped_items_system,
        apply_held_item_visual_system, apply_snapshot_system, camera_follow_system,
        center_cursor_on_focus_system, chat_shortcut_system, client_input_system,
        gameplay_inventory_shortcuts_system, main_menu_music_system, menu_backdrop_camera_system,
        mouse_look_system, network_tick_system, save_client_settings_system,
        toggle_inventory_system, toggle_pause_system, update_cursor_system,
        update_pickup_target_system,
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
        .insert_resource(InventoryUiState::default())
        .insert_resource(PickupTargetState::default())
        .insert_resource(LookState::default())
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
        .add_systems(Startup, setup_scene)
        .add_systems(Startup, setup_button_sound_assets)
        .add_systems(
            EguiPrimaryContextPass,
            (ui_system, button_sound_system).chain(),
        )
        .add_systems(
            Update,
            chat_shortcut_system
                .before(toggle_pause_system)
                .before(toggle_inventory_system)
                .before(center_cursor_on_focus_system)
                .before(update_cursor_system)
                .before(mouse_look_system)
                .before(client_input_system),
        )
        .add_systems(
            Update,
            toggle_pause_system
                .before(toggle_inventory_system)
                .before(update_cursor_system)
                .before(mouse_look_system)
                .before(client_input_system),
        )
        .add_systems(
            Update,
            toggle_inventory_system
                .after(toggle_pause_system)
                .before(update_cursor_system)
                .before(mouse_look_system)
                .before(client_input_system)
                .before(gameplay_inventory_shortcuts_system),
        )
        .add_systems(Update, center_cursor_on_focus_system)
        .add_systems(Update, update_cursor_system)
        .add_systems(
            Update,
            mouse_look_system.run_if(not(egui_wants_any_pointer_input)),
        )
        .add_systems(Update, client_input_system.after(mouse_look_system))
        .add_systems(Update, gameplay_inventory_shortcuts_system)
        .add_systems(Update, network_tick_system.after(client_input_system))
        .add_systems(Update, app_quit_system)
        .add_systems(Update, apply_display_settings_system)
        .add_systems(
            Update,
            save_client_settings_system.after(apply_display_settings_system),
        )
        .add_systems(Update, apply_world_scene_system)
        .add_systems(Update, apply_snapshot_system)
        .add_systems(
            Update,
            apply_dropped_items_system.after(network_tick_system),
        )
        .add_systems(
            Update,
            apply_held_item_visual_system.after(camera_follow_system),
        )
        .add_systems(Update, main_menu_music_system)
        .add_systems(Update, menu_backdrop_camera_system)
        .add_systems(
            Update,
            camera_follow_system
                .run_if(not(egui_wants_any_pointer_input))
                .after(client_input_system)
                .after(mouse_look_system),
        )
        .add_systems(
            Update,
            update_pickup_target_system
                .after(camera_follow_system)
                .after(apply_dropped_items_system),
        )
        .run();

    Ok(())
}
