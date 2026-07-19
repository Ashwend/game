//! Snapshot-application and held-item systems for the client. Split into
//! sub-modules by concern so swing-pose math, snapshot diffing, and visual
//! application stay independently auditable:
//!
//! - `dropped`, `apply_dropped_items_system`, `DroppedItemEntities`,
//!   `DroppedItemInterpolation`.
//! - `resource_nodes`, `apply_resource_nodes_system`, `ResourceNodeEntities`.
//! - `pickup`, `update_pickup_target_system` (throttled).
//! - `tool_swap`, `update_tool_swap_state_system`.
//! - `held`, `apply_held_item_visual_system` and held-item local transform.
//! - `swing_poses`, `bag_idle_pose`, `hatchet_swing_pose`,
//!   `pickaxe_swing_pose`, and the `smoothstep`/`lerp` primitives.
//! - `remote_swing_pose`, the third-person body-rig swing curves (arm
//!   rotations) shared with `app::systems::players`.

mod armor;
mod dropped;
mod held;
mod loot_bag;
mod pickup;
mod projectiles;
mod remote_swing_pose;
mod resource_nodes;
mod slash_trail;
mod swing_poses;
mod tool_swap;

pub(crate) use armor::{ArmorMaterials, ArmorVisuals, armor_layers, build_armor_visuals};
pub(crate) use dropped::{DroppedItemEntities, apply_dropped_items_system};
pub(crate) use held::{
    HeldGripSockets, HeldItemVisuals, RangedPoseInputs, apply_held_item_visual_system,
    build_held_item_visuals, carry_forearm_rotation, carry_upper_arm_rotation,
    held_item_hand_transform, held_item_layers, held_piece_local_transform,
    insert_held_layer_material, load_grip_sockets, resolve_grip_sockets_system,
};
pub(crate) use loot_bag::{LootBagEntities, apply_loot_bags_system};
pub(crate) use pickup::update_pickup_target_system;
pub(crate) use projectiles::{
    ProjectileVisuals, apply_projectiles_system, spawn_predicted_arrows_system,
};
pub(crate) use remote_swing_pose::remote_swing_arm_pose;
pub(crate) use resource_nodes::{
    ResourceNodeEntities, apply_resource_node_stage_system, apply_resource_nodes_system,
    insert_resource_node_material, resource_node_transform_at, resource_node_visual,
    sway_hay_grass_system, tick_resource_node_pop_in_system, tree_foliage_visual,
};
pub(crate) use slash_trail::sword_slash_trail_system;
pub(crate) use tool_swap::update_tool_swap_state_system;
