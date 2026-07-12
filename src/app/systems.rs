// Dev-only: corrects macOS focus-stealing for agent-driven launches. Gated on
// `debug_assertions` + macOS so it compiles out of shipped builds.
#[cfg(all(debug_assertions, target_os = "macos"))]
mod agent_window;
pub(crate) mod analytics;
mod auth;
mod auto_connect;
mod camera;
mod chunk_overlay;
mod combat_feedback;
mod crafting_feedback;
// Dev-only agent automation: the control socket and off-screen capture are
// gated on `debug_assertions` so they compile out of shipped release builds.
#[cfg(all(unix, debug_assertions))]
mod control_socket;
mod deployables;
#[cfg(debug_assertions)]
pub(crate) mod dev_render;
mod display;
pub(crate) mod effects;
pub(crate) mod explosion_vfx;
mod furnace_fire;
mod graphics;
#[cfg(debug_assertions)]
mod headless_capture;
pub(crate) mod input;
mod items;
mod network;
pub(crate) mod node_death;
mod paperdoll_preview;
mod players;
mod quit;
#[cfg(feature = "replication-trace")]
pub(crate) mod replication_trace;
mod settings;
mod test_mode;
pub(crate) mod torch_fire;
mod update;
mod world_map;

use bevy::prelude::SystemSet;

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) use agent_window::relinquish_macos_focus_system;
pub(crate) use analytics::{
    EquipmentWatch, LastTrackedScreen, MeteorShowerImpactWatch, PendingSessionEndReason,
    SessionTracker, WorkbenchWatch, equipment_change_system, error_relay_system,
    meteor_shower_impact_system, screen_viewed_system, session_ended_system,
    session_started_system, workbench_upgrade_system,
};
pub(crate) use auth::drive_auth_flow_system;
pub(crate) use auto_connect::{
    AutoConnectRequest, auto_connect_poll_system, auto_connect_start_system,
};
pub(crate) use camera::{
    CameraImpactKick, CameraMotionEffects, VIEWMODEL_BASE_FOV_DEG, camera_follow_system,
    menu_backdrop_camera_system, sync_viewmodel_fov_system,
};
pub(crate) use chunk_overlay::chunk_overlay_system;
pub(crate) use combat_feedback::tick_combat_feedback_system;
#[cfg(all(unix, debug_assertions))]
pub(crate) use control_socket::{ClientControlSocket, drain_control_socket};
pub(crate) use crafting_feedback::{CraftCompletionWatch, craft_complete_cue_system};
pub(crate) use deployables::charge_fuse::{
    ChargeFuseAssets, animate_charge_fuse_system, tick_charge_spark_particles_system,
};
pub(crate) use deployables::{
    ChargeGhostMeshes, DeployedEntityVisuals, animate_door_panels_system,
    apply_deployed_entities_system, maintain_wall_visual_insets_system, maintain_world_grid_system,
    placement_input_system, prepare_charge_ghost_meshes_system, update_claim_boundary_system,
    update_placement_ghost_system,
};
pub(crate) use display::apply_display_settings_system;
pub(crate) use effects::{spawn_impact_effects_system, tick_impact_chips_system};
pub(crate) use explosion_vfx::{
    ExplosionEffectAssets, ExplosionEvent, spawn_explosion_effects_system,
    tick_explosion_flash_system, tick_explosion_smoke_system,
};
pub(crate) use furnace_fire::{
    FurnaceParticle, animate_furnace_fire_system, furnace_flicker, tick_furnace_particles_system,
};
pub(crate) use graphics::apply_graphics_settings_system;
#[cfg(debug_assertions)]
pub(crate) use headless_capture::{
    HeadlessCapture, insert_capture_target, redirect_camera_to_capture,
};
pub(crate) use input::{
    PredictedArrowEvent, RangedFireSampler, center_cursor_on_focus_system, chat_shortcut_system,
    client_input_system, close_furnace_on_escape_system, close_loot_bag_on_escape_system,
    close_workbench_on_escape_system, gameplay_inventory_shortcuts_system, mouse_look_system,
    send_crafting_command, send_furnace_command, send_inventory_command, send_loot_bag_command,
    send_workbench_command, sync_furnace_open_flag_system, sync_loot_bag_open_flag_system,
    sync_workbench_open_flag_system, toggle_crafting_system, toggle_inventory_system,
    toggle_pause_system, toggle_perf_stats_system, update_cursor_system, wheel_menu_system,
};
pub(crate) use items::{
    ArmorMaterials, DroppedItemEntities, LootBagEntities, ProjectileVisuals, ResourceNodeEntities,
    apply_dropped_items_system, apply_held_item_visual_system, apply_loot_bags_system,
    apply_projectiles_system, apply_resource_node_stage_system, apply_resource_nodes_system,
    build_armor_visuals, build_held_item_visuals, insert_resource_node_material,
    resource_node_transform_at, resource_node_visual, spawn_predicted_arrows_system,
    sway_hay_grass_system, sword_slash_trail_system, tick_resource_node_pop_in_system,
    tree_foliage_visual, update_pickup_target_system, update_tool_swap_state_system,
};
pub(crate) use network::{
    network_tick_system, session_shutdown_poll_system, surface_client_error_toasts_system,
    update_link_ping_system,
};
pub(crate) use node_death::tick_felling_trees_system;
pub(crate) use paperdoll_preview::{
    paperdoll_preview_texture, setup_paperdoll_preview, sync_paperdoll_preview_system,
};
pub(crate) use players::{
    RemotePlayerEntities, animate_remote_players_system, apply_remote_player_appearance_system,
    apply_snapshot_system, reconcile_player_rigs_system, tick_dying_players_system,
};
pub(crate) use quit::app_quit_system;
pub(crate) use settings::{
    flush_settings_on_exit_system, save_client_settings_system, sync_view_radius_system,
};
pub(crate) use test_mode::{
    apply_test_mode_overrides_system, multiplayer_test_owns_window, reposition_test_window_system,
};
pub(crate) use torch_fire::{animate_torch_fire_system, tick_torch_particles_system};
pub(crate) use update::apply_update_system;
pub(crate) use world_map::{generate_world_map_texture_system, world_map_input_system};

/// Declaration order here is NOT schedule order. The runtime order is the flat
/// `CLIENT_UPDATE_ORDER` / `CLIENT_MENU_ORDER` lists in `app.rs`; this enum is
/// just the shared vocabulary. A test in `app.rs`
/// (`every_system_set_is_ordered_exactly_once`) asserts every variant below is
/// slotted into exactly one of those lists, so adding a variant here without
/// ordering it fails the suite.
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
    /// Reconcile arrow visuals against the replicated projectile set and advance
    /// predicted own-arrows. After `ResourceNodes` so it shares the post-snapshot
    /// world view; purely cosmetic and client-only (the server owns the shot).
    Projectiles,
    /// Stream procedural detail-grass tiles around the camera. After
    /// `ResourceNodes` so it shares the post-snapshot world view; purely
    /// cosmetic and client-only.
    Grass,
    DeployedEntities,
    PlacementGhost,
    PlacementInput,
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
