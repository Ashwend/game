//! Item registry and the shared item taxonomy: string ids, id interning,
//! the visual/tool/deployable/material enums, and the `REGISTERED_ITEMS`
//! source-of-truth slice with its `id -> definition` lookup.
//!
//! Split by concern into submodules and re-exported flat so `crate::items::X`
//! call sites stay stable regardless of which submodule owns `X`.

mod armor;
mod deployables;
mod explosives;
mod ids;
mod materials;
mod pickup;
mod ranged;
mod registry;
mod tools;
mod upgrades;
mod visual;
mod weapons;

pub use armor::{
    ARMOR_TOTAL_CAP_PCT, ArmorProfile, ArmorProtection, armor_profile, equipped_protection,
    protection_from_profiles, worn_armor_profiles,
};
pub use deployables::{DeployableKind, DeployableProfile, DoorVariant};
pub use explosives::{ExplosiveDelivery, ExplosiveKind, ExplosiveProfile};
pub use ids::*;
pub use materials::{DestructibleMaterial, explosive_effectiveness_pct, tool_effectiveness_pct};
pub use pickup::{
    PICKUP_RANGE, best_pickup_target, can_pick_up, look_forward, pickup_anchor,
    pickup_anchor_from_position, pickup_score, pickup_score_at_position, within_pickup_reach,
};
pub use ranged::RangedProfile;
pub use registry::{
    ItemDefinition, ItemId, REGISTERED_ITEMS, intern_item_id, item_definition, normalize_stack,
    stack_limit,
};
pub use tools::{HANDS_TOOL, ToolKind, ToolProfile};
pub use upgrades::{DEPLOYABLE_UPGRADES, DeployableUpgrade, upgrade_for};
pub use visual::{
    ArmorJoint, ArmorLayerSpec, ArmorMaterial, ArmorMesh, ArmorMeshVisual, HeldGrip,
    HeldLayerMeshSource, HeldLayerSpec, HeldMesh, HeldMeshMaterial, HeldMeshVisual, HeldPieceSlot,
    ItemModel, ItemTint,
};
pub use weapons::WeaponProfile;
