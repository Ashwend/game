mod scene;
mod state;
mod systems;
mod ui;

use anyhow::Result;
use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, prelude::*};
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass, input::egui_wants_any_pointer_input};

use crate::{
    save::WorldStore,
    steam::{OfflineSteamBackend, SteamBackend},
};

use self::{
    scene::{apply_world_scene_system, setup_scene},
    state::{ClientRuntime, LookState, MenuState, SaveStore, SteamUser},
    systems::{
        apply_snapshot_system, camera_follow_system, center_cursor_on_focus_system,
        chat_shortcut_system, client_input_system, interpolate_players_system, mouse_look_system,
        network_tick_system, toggle_pause_system, update_cursor_system,
    },
    ui::ui_system,
};

pub(crate) const EYE_HEIGHT: f32 = 1.62;
pub(crate) const PLAYER_VISUAL_CENTER_Y: f32 = 0.9;

pub fn run_app() -> Result<()> {
    let store = WorldStore::platform_default()?;
    store.ensure_exists()?;

    let steam = OfflineSteamBackend;
    let user = steam.current_user()?;

    App::new()
        .insert_resource(ClearColor(Color::srgb(0.015, 0.018, 0.023)))
        .insert_resource(SaveStore(store))
        .insert_resource(SteamUser(user))
        .insert_resource(MenuState::default())
        .insert_resource(ClientRuntime::default())
        .insert_resource(LookState::default())
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Game".to_owned(),
                resolution: (1280, 720).into(),
                present_mode: bevy::window::PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(EguiPlugin::default())
        .add_systems(Startup, setup_scene)
        .add_systems(EguiPrimaryContextPass, ui_system)
        .add_systems(
            Update,
            chat_shortcut_system
                .before(toggle_pause_system)
                .before(center_cursor_on_focus_system)
                .before(update_cursor_system)
                .before(mouse_look_system)
                .before(client_input_system),
        )
        .add_systems(Update, toggle_pause_system)
        .add_systems(Update, center_cursor_on_focus_system)
        .add_systems(Update, update_cursor_system)
        .add_systems(
            Update,
            mouse_look_system.run_if(not(egui_wants_any_pointer_input)),
        )
        .add_systems(Update, client_input_system.after(mouse_look_system))
        .add_systems(Update, network_tick_system.after(client_input_system))
        .add_systems(Update, apply_world_scene_system)
        .add_systems(Update, apply_snapshot_system)
        .add_systems(Update, interpolate_players_system)
        .add_systems(
            Update,
            camera_follow_system
                .run_if(not(egui_wants_any_pointer_input))
                .after(client_input_system)
                .after(mouse_look_system),
        )
        .run();

    Ok(())
}
