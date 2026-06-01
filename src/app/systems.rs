pub(crate) mod analytics;
mod auth;
mod auto_connect;
mod camera;
mod chunk_overlay;
mod deployables;
mod display;
pub(crate) mod effects;
mod furnace_fire;
mod graphics;
pub(crate) mod input;
mod items;
mod network;
pub(crate) mod node_death;
mod players;
mod quit;
#[cfg(feature = "replication-trace")]
pub(crate) mod replication_trace;
mod settings;
mod test_mode;

use bevy::prelude::SystemSet;

pub(crate) use analytics::{
    LastTrackedScreen, PendingSessionEndReason, SessionTracker, error_relay_system,
    screen_viewed_system, session_ended_system, session_started_system,
};
pub(crate) use auth::drive_auth_flow_system;
pub(crate) use auto_connect::{
    AutoConnectRequest, auto_connect_poll_system, auto_connect_start_system,
};
pub(crate) use camera::{
    CameraImpactKick, CameraMotionEffects, camera_follow_system, menu_backdrop_camera_system,
};
pub(crate) use chunk_overlay::chunk_overlay_system;
pub(crate) use deployables::{
    apply_deployed_entities_system, maintain_world_grid_system, placement_input_system,
    update_placement_ghost_system,
};
pub(crate) use display::apply_display_settings_system;
pub(crate) use effects::{spawn_impact_effects_system, tick_impact_chips_system};
pub(crate) use furnace_fire::{animate_furnace_fire_system, tick_furnace_particles_system};
pub(crate) use graphics::apply_graphics_settings_system;
pub(crate) use input::{
    center_cursor_on_focus_system, chat_shortcut_system, client_input_system,
    close_furnace_on_escape_system, close_loot_bag_on_escape_system,
    gameplay_inventory_shortcuts_system, mouse_look_system, send_crafting_command,
    send_furnace_command, send_inventory_command, send_loot_bag_command,
    sync_furnace_open_flag_system, sync_loot_bag_open_flag_system, toggle_crafting_system,
    toggle_inventory_system, toggle_pause_system, toggle_perf_stats_system, update_cursor_system,
};
pub(crate) use items::{
    DroppedItemEntities, LootBagEntities, ResourceNodeEntities, apply_dropped_items_system,
    apply_held_item_visual_system, apply_loot_bags_system, apply_resource_nodes_system,
    resource_node_transform_at, resource_node_visual, tick_resource_node_pop_in_system,
    update_pickup_target_system, update_tool_swap_state_system,
};
pub(crate) use network::{
    network_tick_system, session_shutdown_poll_system, surface_client_error_toasts_system,
};
pub(crate) use node_death::tick_felling_trees_system;
pub(crate) use players::{RemotePlayerEntities, apply_snapshot_system, tick_dying_players_system};
pub(crate) use quit::app_quit_system;
pub(crate) use settings::{save_client_settings_system, sync_view_radius_system};
pub(crate) use test_mode::{
    apply_test_mode_overrides_system, multiplayer_test_owns_window, reposition_test_window_system,
};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ClientSystemSet {
    /// Refresh `LocalPlayerState` from the replicated `Player` /
    /// `PlayerPublic` / `PlayerPrivate` components. Runs at the very
    /// start of `Update` so every later set sees the current values
    /// without dipping into the snapshot.
    LocalPlayerSync,
    Focus,
    ChatShortcut,
    PauseToggle,
    InventoryToggle,
    CraftingToggle,
    Cursor,
    Look,
    Input,
    ToolSwap,
    InventoryShortcuts,
    Network,
    /// Rebuild `ClientRuntime::world_grid` whenever the world version,
    /// the snapshot's resource-node collider set, or the replicated
    /// `Deployable` set changes. Runs after `Network` so freshly
    /// processed snapshots are reflected this frame.
    WorldGridRebuild,
    SessionShutdown,
    Quit,
    Display,
    SettingsSave,
    WorldScene,
    Players,
    DroppedItems,
    ResourceNodes,
    /// Stream procedural detail-grass tiles around the camera. After
    /// `ResourceNodes` so it shares the post-snapshot world view; purely
    /// cosmetic and client-only.
    Grass,
    DeployedEntities,
    PlacementGhost,
    PlacementInput,
    Camera,
    HeldItem,
    Sky,
    PickupTarget,
    Footsteps,
    ImpactEffectsSpawn,
    ImpactEffectsTick,
    /// Flicker lit furnaces and emit their flame + ember particles. Purely
    /// cosmetic and client-only; rides after the impact-effect tick so all
    /// particle work shares the same post-snapshot window.
    FurnaceFireAnimate,
    FurnaceParticleTick,
    ImpactSounds,
    TransitionStingers,
    PlaySounds,
    AudioFaderTick,
    AmbientBeds,
    AmbientEmitters,
    NodeDeathTick,
    MainMenuMusic,
    MenuBackdropCamera,
    AutoConnect,
    Analytics,
    VoiceCaptureManage,
    VoiceTransmit,
    VoiceReceive,
    VoiceSettings,
    TestModeApply,
    TestWindowReposition,
}
