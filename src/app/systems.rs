mod audio;
mod camera;
mod display;
mod effects;
mod input;
mod items;
mod network;
mod players;
mod quit;
mod settings;

use bevy::prelude::SystemSet;

pub(crate) use audio::main_menu_music_system;
pub(crate) use camera::{CameraImpactKick, camera_follow_system, menu_backdrop_camera_system};
pub(crate) use display::apply_display_settings_system;
pub(crate) use effects::{spawn_impact_effects_system, tick_impact_chips_system};
pub(crate) use input::{
    center_cursor_on_focus_system, chat_shortcut_system, client_input_system,
    gameplay_inventory_shortcuts_system, mouse_look_system, send_inventory_command,
    toggle_inventory_system, toggle_pause_system, update_cursor_system,
};
pub(crate) use items::{
    apply_dropped_items_system, apply_held_item_visual_system, apply_resource_nodes_system,
    update_pickup_target_system,
};
pub(crate) use network::{network_tick_system, session_shutdown_poll_system};
pub(crate) use players::apply_snapshot_system;
pub(crate) use quit::app_quit_system;
pub(crate) use settings::save_client_settings_system;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ClientSystemSet {
    Focus,
    ChatShortcut,
    PauseToggle,
    InventoryToggle,
    Cursor,
    Look,
    Input,
    InventoryShortcuts,
    Network,
    SessionShutdown,
    Quit,
    Display,
    SettingsSave,
    WorldScene,
    Players,
    DroppedItems,
    ResourceNodes,
    Camera,
    HeldItem,
    PickupTarget,
    ImpactEffectsSpawn,
    ImpactEffectsTick,
    MainMenuMusic,
    MenuBackdropCamera,
}
