use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
};

use crate::protocol::{DroppedWorldItem, ItemStack, Vec3Net};

pub const WOOD_ID: &str = "wood";
pub const STONE_ID: &str = "stone";
pub const COAL_ID: &str = "coal";
pub const IRON_ORE_ID: &str = "iron_ore";
pub const IRON_BAR_ID: &str = "iron_bar";
pub const SULFUR_ORE_ID: &str = "sulfur_ore";
pub const FIBER_ID: &str = "fiber";
pub const PLANT_TWINE_ID: &str = "plant_twine";
pub const BASIC_HATCHET_ID: &str = "wood_stone_hatchet";
pub const BASIC_PICKAXE_ID: &str = "wood_stone_pickaxe";
pub const WORKBENCH_T1_ID: &str = "workbench_t1";
pub const CRUDE_FURNACE_ID: &str = "crude_furnace";

/// Identifier shared between `ItemStack`, `ItemMerged`, and item definitions.
/// Backed by `Arc<str>` so clones are a refcount bump instead of a heap copy.
/// Known IDs are interned to a single allocation at startup; deserialized IDs
/// are looked up against the registry and reuse the cached `Arc` on hits.
pub type ItemId = Arc<str>;

/// Returns the interned `Arc<str>` for `id`. Compile-time constants from
/// `REGISTERED_ITEMS` resolve without allocating on hits via an O(1) hash
/// lookup; unknown ids fall through to a fresh `Arc` and are cached so
/// subsequent hits also avoid allocating. Stays open to runtime-loaded items.
pub fn intern_item_id(id: &str) -> ItemId {
    let registry = interned_registry();
    if let Some(cached) = registry.read().ok().and_then(|map| map.get(id).cloned()) {
        return cached;
    }
    // Allocate outside the write lock so a contended path doesn't hold the
    // lock through the heap allocation. The double-insert window is harmless:
    // both inserts produce the same Arc<str> contents, and the second simply
    // overwrites with an Arc that hashes/compares equal.
    let fresh: Arc<str> = Arc::from(id);
    if let Ok(mut map) = registry.write() {
        // Re-check after taking the write lock, another caller may have
        // inserted between our read miss and now. Returning the cached value
        // keeps the registry's "one Arc per id" invariant in lockstep with
        // anything that already grabbed the earlier-inserted Arc.
        if let Some(cached) = map.get(id).cloned() {
            return cached;
        }
        map.insert(Box::from(id), fresh.clone());
    }
    fresh
}

fn interned_registry() -> &'static RwLock<HashMap<Box<str>, Arc<str>>> {
    static REGISTRY: OnceLock<RwLock<HashMap<Box<str>, Arc<str>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut map = HashMap::with_capacity(REGISTERED_ITEMS.len());
        for definition in REGISTERED_ITEMS {
            let arc: Arc<str> = Arc::from(definition.id);
            map.insert(Box::from(definition.id), arc);
        }
        RwLock::new(map)
    })
}

pub const PICKUP_RANGE: f32 = 3.4;
const PICKUP_RAY_RADIUS: f32 = 0.58;
const PICKUP_ANCHOR_HEIGHT: f32 = 0.28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemModel {
    Bag,
    Hatchet,
    Pickaxe,
    /// Deployable items render as the bag silhouette in the held-item
    /// slot, the actual structure mesh is what gets placed in the world.
    Deployable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolKind {
    /// No tool equipped. Synthesized via [`HANDS_TOOL`] when the active
    /// actionbar slot has no tool. Crude pickup nodes carry a
    /// `ToolRequirement` of `Hands` to mark themselves as
    /// E-pickup-only, no tool (including bare hands) can gather them
    /// by swinging. See [`crate::resources::ToolRequirement::allows`].
    Hands,
    Axe,
    Pickaxe,
}

impl ToolKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hands => "Bare hands",
            Self::Axe => "Hatchet",
            Self::Pickaxe => "Pickaxe",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolProfile {
    pub kind: ToolKind,
    pub tier: u8,
    pub gather_amount: u16,
    pub cooldown_ticks: u64,
}

/// Synthesized tool profile used when no actionbar item is held. The
/// server substitutes this in when the active stack carries no tool
/// definition so the gather pipeline always has a `ToolProfile` to read.
/// It's never accepted as a valid gather tool, crude nodes are E-pickup
/// only and the tool-required nodes reject Hands explicitly, but it
/// keeps the cooldown/payout math uniform across the gather path.
pub const HANDS_TOOL: ToolProfile = ToolProfile {
    kind: ToolKind::Hands,
    tier: 0,
    gather_amount: 1,
    cooldown_ticks: 10,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTint {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ItemTint {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// What kind of structure a deployable item places. The tier travels with
/// the kind so a single `RecipeStation::Workbench { min_tier }` check can
/// match any equal-or-higher workbench in range, same idea behind tool
/// tiers (`ToolProfile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeployableKind {
    Workbench { tier: u8 },
    Furnace { tier: u8 },
}

impl DeployableKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workbench { .. } => "Workbench",
            Self::Furnace { .. } => "Furnace",
        }
    }

    /// Source of truth for what the structure is built from. The damage
    /// path uses this for the tool-vs-material multiplier and the
    /// client uses it to pick the swing surface (audio/visual chip).
    /// Tier doesn't change material today, a future "reinforced" tier
    /// can introduce a new kind variant if that ever needs to differ.
    pub const fn material(self) -> DeployableMaterial {
        match self {
            Self::Workbench { .. } => DeployableMaterial::Wood,
            Self::Furnace { .. } => DeployableMaterial::Stone,
        }
    }
}

/// What the structure is made of, for the damage / tool-matchup system.
/// Kept deliberately coarse, wood vs stone is enough to express
/// "hatchet eats workbenches, pickaxe eats furnaces" without forcing
/// every future deployable to invent a new material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployableMaterial {
    Wood,
    Stone,
}

/// Tool-vs-material damage multiplier, expressed as a percentage so
/// the server stays on integer math. Matched tool ≈ 1.5× damage,
/// mismatched proper tool ≈ 0.5× damage; bare hands never reach this
/// code path (they're rejected upstream).
pub fn tool_damage_multiplier_pct(tool: ToolKind, material: DeployableMaterial) -> u32 {
    match (tool, material) {
        (ToolKind::Axe, DeployableMaterial::Wood) => 150,
        (ToolKind::Pickaxe, DeployableMaterial::Stone) => 150,
        (ToolKind::Axe, DeployableMaterial::Stone) => 50,
        (ToolKind::Pickaxe, DeployableMaterial::Wood) => 50,
        // Hands shouldn't reach here, but if they do treat them as
        // worst-case mismatched so they make minimal dents.
        (ToolKind::Hands, _) => 50,
    }
}

/// Footprint + health profile for items that drop into the world as
/// placed structures. Lives on `ItemDefinition` so item-aware UIs (action
/// bar, inventory tooltip) can show "placeable" affordances without a
/// separate registry, mirroring `ToolProfile`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeployableProfile {
    pub kind: DeployableKind,
    /// Spawn HP for the placed structure. Persisted in the world save.
    pub max_health: u32,
    /// Horizontal half-extent of the structure's AABB collider. The
    /// vertical extent is taken from `collider_half_height` and the
    /// collider is anchored on the ground.
    pub collider_half_width: f32,
    pub collider_half_height: f32,
    /// Range, in metres, within which a `RecipeStation` of this kind +
    /// tier is considered "in reach" for a player who placed it.
    /// `0.0` means the deployable does not act as a crafting station.
    pub station_radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub stack_size: u16,
    pub equipable: bool,
    pub model: ItemModel,
    pub tint: ItemTint,
    pub tool: Option<ToolProfile>,
    pub deployable: Option<DeployableProfile>,
}

impl ItemDefinition {
    pub fn effective_stack_size(self) -> u16 {
        if self.equipable {
            1
        } else {
            self.stack_size.max(1)
        }
    }
}

pub const REGISTERED_ITEMS: &[ItemDefinition] = &[
    ItemDefinition {
        id: WOOD_ID,
        name: "Wood",
        description: "A common building material gathered from trees.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(139, 95, 56),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: STONE_ID,
        name: "Stone",
        description: "A rough stone material used for primitive tools.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(122, 128, 126),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: COAL_ID,
        name: "Coal",
        description: "A fuel-rich mineral gathered from coal nodes.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(42, 45, 48),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: IRON_ORE_ID,
        name: "Iron Ore",
        description: "Raw iron-bearing rock ready for later smelting systems.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(155, 120, 94),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: SULFUR_ORE_ID,
        name: "Sulfur Ore",
        description: "A yellow mineral gathered from sulfur nodes.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(218, 189, 73),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: FIBER_ID,
        name: "Plant Fiber",
        description: "Coarse fibers pulled from grass tufts. Used for crude bindings.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(168, 184, 96),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: PLANT_TWINE_ID,
        name: "Plant Twine",
        description: "Twisted plant fibers. The binding that holds primitive tools together.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(196, 176, 110),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: IRON_BAR_ID,
        name: "Iron Bar",
        description: "Refined iron, smelted from raw ore in a furnace.",
        stack_size: 100,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(196, 198, 204),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: BASIC_HATCHET_ID,
        name: "Stone Hatchet",
        description: "A basic wood-and-stone axe for gathering trees.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Hatchet,
        tint: ItemTint::new(148, 122, 82),
        tool: Some(ToolProfile {
            kind: ToolKind::Axe,
            tier: 1,
            gather_amount: 6,
            cooldown_ticks: 6,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: BASIC_PICKAXE_ID,
        name: "Stone Pickaxe",
        description: "A basic wood-and-stone pickaxe for gathering ore nodes.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Pickaxe,
        tint: ItemTint::new(134, 128, 112),
        tool: Some(ToolProfile {
            kind: ToolKind::Pickaxe,
            tier: 1,
            gather_amount: 6,
            cooldown_ticks: 6,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: WORKBENCH_T1_ID,
        name: "Workbench lvl 1",
        description: "A sturdy table for assembling tier-1 crafted goods. \
                      Place it in the world and craft within ~5m of it.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        tint: ItemTint::new(136, 96, 56),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Workbench { tier: 1 },
            max_health: 500,
            collider_half_width: 0.55,
            collider_half_height: 0.45,
            station_radius: 5.0,
        }),
    },
    ItemDefinition {
        id: CRUDE_FURNACE_ID,
        name: "Furnace",
        description: "A stone furnace for smelting ore into bars. \
                      Requires a workbench nearby to build.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        tint: ItemTint::new(102, 92, 84),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Furnace { tier: 1 },
            max_health: 800,
            collider_half_width: 0.50,
            collider_half_height: 0.60,
            station_radius: 5.0,
        }),
    },
];

/// Build-once `id → definition` lookup over [`REGISTERED_ITEMS`]. The slice
/// itself is the source of truth; this index just gives `item_definition` an
/// O(1) hit instead of a linear scan, which matters once gather/pickup hits
/// run every swing.
fn item_definitions_by_id() -> &'static HashMap<&'static str, &'static ItemDefinition> {
    static INDEX: OnceLock<HashMap<&'static str, &'static ItemDefinition>> = OnceLock::new();
    INDEX.get_or_init(|| {
        REGISTERED_ITEMS
            .iter()
            .map(|definition| (definition.id, definition))
            .collect()
    })
}

pub fn item_definition(item_id: &str) -> Option<&'static ItemDefinition> {
    item_definitions_by_id().get(item_id).copied()
}

pub fn stack_limit(item_id: &str) -> Option<u16> {
    item_definition(item_id).map(|definition| definition.effective_stack_size())
}

pub fn normalize_stack(stack: &ItemStack) -> Option<ItemStack> {
    let limit = stack_limit(&stack.item_id)?;
    let quantity = stack.quantity.clamp(1, limit);
    Some(ItemStack::new(stack.item_id.clone(), quantity))
}

pub fn look_forward(yaw: f32, pitch: f32) -> Vec3Net {
    let pitch_cos = pitch.cos();
    Vec3Net::new(-yaw.sin() * pitch_cos, pitch.sin(), -yaw.cos() * pitch_cos).normalize_or_zero()
}

pub fn pickup_anchor(item: &DroppedWorldItem) -> Vec3Net {
    pickup_anchor_from_position(item.position)
}

pub fn pickup_anchor_from_position(position: Vec3Net) -> Vec3Net {
    position.plus(Vec3Net::new(0.0, PICKUP_ANCHOR_HEIGHT, 0.0))
}

pub fn pickup_score(eye: Vec3Net, yaw: f32, pitch: f32, item: &DroppedWorldItem) -> Option<f32> {
    pickup_score_at_position(eye, yaw, pitch, item.position)
}

/// Projection-along-ray distance from the eye to the pickup anchor at
/// `position`. `None` when the point is outside the swept pickup
/// cylinder. Same math as [`pickup_score`] but reads the position
/// directly so callers iterating replicated `DroppedItemTransform`
/// don't need to materialise a `DroppedWorldItem`.
pub fn pickup_score_at_position(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    position: Vec3Net,
) -> Option<f32> {
    let anchor = pickup_anchor_from_position(position);
    let to_item = anchor.minus(eye);
    // Cheap distance cull before the trig in `look_forward`. Anything outside
    // the swept cylinder is unreachable; the bound stays conservative so it
    // never rejects a candidate the ray test would have accepted.
    let max_reach_sq = (PICKUP_RANGE + PICKUP_RAY_RADIUS).powi(2);
    if to_item.length_squared() > max_reach_sq {
        return None;
    }

    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let projection = to_item.dot(forward);
    if !(0.0..=PICKUP_RANGE).contains(&projection) {
        return None;
    }

    let closest = eye.plus(forward.scale(projection));
    let lateral = anchor.minus(closest);
    if lateral.length_squared() > PICKUP_RAY_RADIUS * PICKUP_RAY_RADIUS {
        return None;
    }

    Some(projection)
}

pub fn can_pick_up(eye: Vec3Net, yaw: f32, pitch: f32, item: &DroppedWorldItem) -> bool {
    pickup_score(eye, yaw, pitch, item).is_some()
}

pub fn best_pickup_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    items: impl Iterator<Item = &'a DroppedWorldItem>,
) -> Option<&'a DroppedWorldItem> {
    items
        .filter_map(|item| pickup_score(eye, yaw, pitch, item).map(|score| (score, item)))
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, item)| item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DroppedWorldItem, ItemStack, QuatNet};

    #[test]
    fn equipable_items_force_stack_size_one() {
        assert_eq!(stack_limit(BASIC_HATCHET_ID), Some(1));
        assert_eq!(stack_limit(COAL_ID), Some(200));
        assert_eq!(
            normalize_stack(&ItemStack::new(BASIC_HATCHET_ID, 40)),
            Some(ItemStack::new(BASIC_HATCHET_ID, 1))
        );
    }

    #[test]
    fn tool_material_multiplier_favours_matched_pairings() {
        // Matched: hatchet→wood and pickaxe→stone hit hardest.
        assert_eq!(
            tool_damage_multiplier_pct(ToolKind::Axe, DeployableMaterial::Wood),
            150
        );
        assert_eq!(
            tool_damage_multiplier_pct(ToolKind::Pickaxe, DeployableMaterial::Stone),
            150
        );
        // Mismatched proper tools still chip but at a third of the
        // matched rate (50 / 150 = 1/3).
        assert_eq!(
            tool_damage_multiplier_pct(ToolKind::Axe, DeployableMaterial::Stone),
            50
        );
        assert_eq!(
            tool_damage_multiplier_pct(ToolKind::Pickaxe, DeployableMaterial::Wood),
            50
        );
    }

    #[test]
    fn deployable_kind_material_matches_visual_intent() {
        assert_eq!(
            DeployableKind::Workbench { tier: 1 }.material(),
            DeployableMaterial::Wood
        );
        assert_eq!(
            DeployableKind::Furnace { tier: 1 }.material(),
            DeployableMaterial::Stone
        );
    }

    #[test]
    fn pickup_target_uses_view_ray_and_range() {
        let item = DroppedWorldItem {
            id: 1,
            stack: ItemStack::new(COAL_ID, 1),
            position: Vec3Net::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            rotation: QuatNet::IDENTITY,
        };
        let eye = Vec3Net::new(0.0, 0.6, 0.0);

        assert!(can_pick_up(eye, 0.0, -0.16, &item));
        assert!(!can_pick_up(eye, std::f32::consts::PI, -0.16, &item));
    }
}
