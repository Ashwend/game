//! Crafting recipe registry. Recipes are static, stable, server-authoritative,
//! and exposed by id, clients and server both consult this module instead of
//! shipping recipe payloads on the wire.
//!
//! The shape mirrors [`crate::items`]:
//! - String ids back every recipe, interned to `Arc<str>` for cheap clones.
//! - A `&'static [RecipeDefinition]` slice is the source of truth.
//! - An `id → definition` index gives O(1) lookups for the server's enqueue
//!   path and the client's UI.
//!
//! Scaling note: the slice + O(1) index design holds up to thousands of
//! recipes without changes. Add recipes by appending to [`REGISTERED_RECIPES`];
//! the index and category iteration follow automatically.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
};

use crate::items::{
    BASIC_HATCHET_ID, BASIC_PICKAXE_ID, BUILDING_PLAN_ID, COAL_ID, CRUDE_FURNACE_ID,
    DeployableKind, FIBER_ID, HAMMER_ID, HEWN_LOG_DOOR_ID, HEWN_LOG_ID, IRON_BAR_ID, IRON_DOOR_ID,
    IRON_HATCHET_ID, IRON_PICKAXE_ID, PLANT_TWINE_ID, SLEEPING_BAG_ID, STONE_ID,
    STORAGE_BOX_LARGE_ID, STORAGE_BOX_SMALL_ID, TOOL_CUPBOARD_ID, TORCH_ID, WOOD_ID,
    WORKBENCH_T1_ID, item_definition,
};

pub const PLANT_TWINE_RECIPE_ID: &str = "plant_twine";
pub const HEWN_LOG_RECIPE_ID: &str = "hewn_log";
pub const STONE_HATCHET_RECIPE_ID: &str = "wood_stone_hatchet";
pub const STONE_PICKAXE_RECIPE_ID: &str = "wood_stone_pickaxe";
pub const IRON_HATCHET_RECIPE_ID: &str = "iron_hatchet";
pub const IRON_PICKAXE_RECIPE_ID: &str = "iron_pickaxe";
pub const WORKBENCH_T1_RECIPE_ID: &str = "workbench_t1";
pub const CRUDE_FURNACE_RECIPE_ID: &str = "crude_furnace";
pub const BUILDING_PLAN_RECIPE_ID: &str = "building_plan";
pub const HAMMER_RECIPE_ID: &str = "hammer";
pub const HEWN_LOG_DOOR_RECIPE_ID: &str = "hewn_log_door";
pub const IRON_DOOR_RECIPE_ID: &str = "iron_door";
pub const SLEEPING_BAG_RECIPE_ID: &str = "sleeping_bag";
pub const STORAGE_BOX_SMALL_RECIPE_ID: &str = "storage_box_small";
pub const STORAGE_BOX_LARGE_RECIPE_ID: &str = "storage_box_large";
pub const TORCH_RECIPE_ID: &str = "torch";
pub const TOOL_CUPBOARD_RECIPE_ID: &str = "tool_cupboard";

/// Crafting station a recipe needs to be in range of. Cheap value type so
/// it lives next to the recipe definition without a registry lookup. The
/// server resolves "in range" by walking the local player's nearby placed
/// deployables; client UIs surface the requirement so the player knows
/// what to build first.
///
/// Furnace-style "machine inside an entity" operations are intentionally
/// not represented here, smelting happens inside the furnace's own UI,
/// not via the recipe registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeStation {
    /// Hand-craftable, no nearby structure required.
    None,
    /// A `DeployableKind::Workbench { tier }` with `tier >= min_tier`
    /// must be within its `station_radius` of the player.
    Workbench { min_tier: u8 },
}

impl RecipeStation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Hand craft",
            Self::Workbench { .. } => "Workbench",
        }
    }

    /// True when `kind` (a placed deployable) satisfies this requirement.
    /// Workbench tier 2 also satisfies a Workbench tier 1 requirement,
    /// same as how higher-tier tools satisfy lower-tier gather rules.
    pub fn satisfied_by(self, kind: DeployableKind) -> bool {
        match (self, kind) {
            (Self::None, _) => true,
            (Self::Workbench { min_tier }, DeployableKind::Workbench { tier }) => tier >= min_tier,
            _ => false,
        }
    }
}

/// Interned identifier shared between protocol messages, server state, and
/// the UI. Same `Arc<str>` story as [`crate::items::ItemId`], clones are a
/// refcount bump and deserialized ids reuse the cached `Arc` on hits.
pub type RecipeId = Arc<str>;

/// Cap on a player's queue length. Picked high enough that early players
/// won't bump into it and low enough that a malicious client can't flood
/// the server with unbounded refund bookkeeping.
pub const MAX_CRAFTING_QUEUE_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecipeCategory {
    /// Raw-material refinements (e.g. fiber → twine, iron ore → bar).
    Materials,
    /// Equipable tools and weapons.
    Tools,
    /// Placeable structures (workbench, furnace, …).
    Building,
    /// Catch-all so the enum doesn't need a protocol bump for one-offs.
    Misc,
}

impl RecipeCategory {
    pub const ALL: &'static [Self] = &[Self::Materials, Self::Tools, Self::Building, Self::Misc];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Materials => "Materials",
            Self::Tools => "Tools",
            Self::Building => "Building",
            Self::Misc => "Misc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CraftingInput {
    pub item_id: &'static str,
    pub quantity: u16,
}

impl CraftingInput {
    pub const fn new(item_id: &'static str, quantity: u16) -> Self {
        Self { item_id, quantity }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecipeDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: RecipeCategory,
    pub inputs: &'static [CraftingInput],
    pub output_item: &'static str,
    pub output_quantity: u16,
    /// How long one unit of the output takes to craft, in seconds. Server
    /// converts to ticks via [`crate::protocol::SERVER_TICK_RATE_HZ`].
    pub craft_seconds: f32,
    /// Progression tier the recipe belongs to. Used only for sorting in
    /// the recipe browser today (higher tier surfaces first). Keep low
    /// numbers for raw refinements and bump it as the tech tree grows:
    /// `0` = primitive material processing (e.g. plant twine), `1` =
    /// stone-age tools, `2` = iron, and so on.
    pub tier: u8,
    /// Crafting station the player must stand next to. Defaults to
    /// `RecipeStation::None` for hand-craftable recipes.
    pub station: RecipeStation,
}

/// Static recipe table. Append-only, entries must keep stable ids so saves
/// and queued jobs survive across versions.
pub const REGISTERED_RECIPES: &[RecipeDefinition] = &[
    RecipeDefinition {
        id: PLANT_TWINE_RECIPE_ID,
        name: "Plant Twine",
        description: "Twist three handfuls of plant fiber into a length of twine.",
        category: RecipeCategory::Materials,
        inputs: &[CraftingInput::new(FIBER_ID, 3)],
        output_item: PLANT_TWINE_ID,
        output_quantity: 1,
        craft_seconds: 3.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: HEWN_LOG_RECIPE_ID,
        name: "Hewn Log",
        description: "Square up raw wood into a clean structural billet. \
                      Worked at a bench, it takes a vice to hold the cut.",
        category: RecipeCategory::Materials,
        inputs: &[CraftingInput::new(WOOD_ID, 10)],
        output_item: HEWN_LOG_ID,
        output_quantity: 1,
        craft_seconds: 2.5,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: STONE_HATCHET_RECIPE_ID,
        name: "Stone Hatchet",
        description: "Lash a sharpened stone to a wooden handle with twine.",
        category: RecipeCategory::Tools,
        inputs: &[
            CraftingInput::new(WOOD_ID, 2),
            CraftingInput::new(STONE_ID, 2),
            CraftingInput::new(PLANT_TWINE_ID, 1),
        ],
        output_item: BASIC_HATCHET_ID,
        output_quantity: 1,
        craft_seconds: 8.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: STONE_PICKAXE_RECIPE_ID,
        name: "Stone Pickaxe",
        description: "Bind a heavy stone head to a sturdy handle for breaking rock.",
        category: RecipeCategory::Tools,
        inputs: &[
            CraftingInput::new(WOOD_ID, 2),
            CraftingInput::new(STONE_ID, 3),
            CraftingInput::new(PLANT_TWINE_ID, 1),
        ],
        output_item: BASIC_PICKAXE_ID,
        output_quantity: 1,
        craft_seconds: 10.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: IRON_HATCHET_RECIPE_ID,
        name: "Iron Hatchet",
        description: "Forge an iron axe head and haft it on hewn handle stock. \
                      Fells trees twice as fast as the stone hatchet.",
        category: RecipeCategory::Tools,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 2),
            CraftingInput::new(IRON_BAR_ID, 12),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: IRON_HATCHET_ID,
        output_quantity: 1,
        craft_seconds: 20.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: IRON_PICKAXE_RECIPE_ID,
        name: "Iron Pickaxe",
        description: "Forge a heavy iron head and set it on hewn handle stock. \
                      Tears ore and stone loose twice as fast as the stone pickaxe.",
        category: RecipeCategory::Tools,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 2),
            CraftingInput::new(IRON_BAR_ID, 18),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: IRON_PICKAXE_ID,
        output_quantity: 1,
        craft_seconds: 24.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: WORKBENCH_T1_RECIPE_ID,
        name: "Workbench lvl 1",
        description: "A sturdy crafting table. Stand near one to unlock its tier-1 recipes.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(WOOD_ID, 50),
            CraftingInput::new(STONE_ID, 20),
            CraftingInput::new(PLANT_TWINE_ID, 4),
        ],
        output_item: WORKBENCH_T1_ID,
        output_quantity: 1,
        craft_seconds: 14.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: CRUDE_FURNACE_RECIPE_ID,
        name: "Furnace",
        description: "A stone furnace. Place one and press E to load fuel and smelt ore.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(STONE_ID, 60),
            CraftingInput::new(WOOD_ID, 10),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: CRUDE_FURNACE_ID,
        output_quantity: 1,
        craft_seconds: 18.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: BUILDING_PLAN_RECIPE_ID,
        name: "Building Plan",
        description: "Construction sketches on rough parchment. Equip it, hold \
                      right click to pick a piece, left click to place.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(WOOD_ID, 10),
            CraftingInput::new(FIBER_ID, 5),
        ],
        output_item: BUILDING_PLAN_ID,
        output_quantity: 1,
        craft_seconds: 5.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: HAMMER_RECIPE_ID,
        name: "Hammer",
        description: "A heavy construction mallet. Swing it at your buildings \
                      to repair them; hold right click to upgrade or demolish.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(WOOD_ID, 10),
            CraftingInput::new(STONE_ID, 5),
            CraftingInput::new(PLANT_TWINE_ID, 1),
        ],
        output_item: HAMMER_ID,
        output_quantity: 1,
        craft_seconds: 8.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: HEWN_LOG_DOOR_RECIPE_ID,
        name: "Hewn Log Door",
        description: "A heavy code-locked door of squared logs. Mounts in a \
                      doorway; you set the code when you hang it.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 5),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: HEWN_LOG_DOOR_ID,
        output_quantity: 1,
        craft_seconds: 12.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: IRON_DOOR_RECIPE_ID,
        name: "Iron Door",
        description: "A forged iron door on a banded frame. Tools can't \
                      scratch it, only explosives breach it. Mounts in a \
                      doorway; you set the code when you hang it.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(IRON_BAR_ID, 30),
            CraftingInput::new(HEWN_LOG_ID, 4),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: IRON_DOOR_ID,
        output_quantity: 1,
        craft_seconds: 24.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: SLEEPING_BAG_RECIPE_ID,
        name: "Sleeping Bag",
        description: "A bedroll of woven plant fiber. Place it to anchor your \
                      respawn; hold E on it to rename it.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(FIBER_ID, 20),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: SLEEPING_BAG_ID,
        output_quantity: 1,
        craft_seconds: 8.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: TORCH_RECIPE_ID,
        name: "Torch",
        description: "Pitch-soaked wood that burns for hours. Place it on the \
                      ground or mount it on a wall to light the dark.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(WOOD_ID, 5),
            CraftingInput::new(COAL_ID, 1),
        ],
        output_item: TORCH_ID,
        output_quantity: 1,
        craft_seconds: 4.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: STORAGE_BOX_SMALL_RECIPE_ID,
        name: "Storage Box",
        description: "A small wooden chest. Place it down and press E to \
                      stash items inside.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(WOOD_ID, 60),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: STORAGE_BOX_SMALL_ID,
        output_quantity: 1,
        craft_seconds: 10.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: STORAGE_BOX_LARGE_RECIPE_ID,
        name: "Large Storage Box",
        description: "A long banded chest with more than twice the room of \
                      the small box.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(WOOD_ID, 150),
            CraftingInput::new(HEWN_LOG_ID, 2),
            CraftingInput::new(PLANT_TWINE_ID, 4),
        ],
        output_item: STORAGE_BOX_LARGE_ID,
        output_quantity: 1,
        craft_seconds: 14.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: TOOL_CUPBOARD_RECIPE_ID,
        name: "Tool Cupboard",
        description: "A locked cabinet that claims the base it sits on. \
                      While it stands, only players you authorize can \
                      build nearby. Place it on a foundation.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(WOOD_ID, 100),
            CraftingInput::new(HEWN_LOG_ID, 10),
            CraftingInput::new(STONE_ID, 50),
            CraftingInput::new(PLANT_TWINE_ID, 4),
        ],
        output_item: TOOL_CUPBOARD_ID,
        output_quantity: 1,
        craft_seconds: 20.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
];

/// The single material a hammer repair hit consumes for a crafted
/// deployable: the recipe's *primary* (first) input, stone for the
/// furnace, wood for the workbench and boxes, fiber for the bag, at a
/// quarter of the recipe amount per hit. One repair hit restores a
/// quarter of max HP, so a full repair from near-dead costs about the
/// primary input of crafting it fresh, without the secondary materials.
/// `None` when nothing crafts into `item_id` (world-spawned kinds).
pub fn repair_material_for(item_id: &str) -> Option<(&'static str, u16)> {
    let recipe = REGISTERED_RECIPES
        .iter()
        .find(|recipe| recipe.output_item == item_id)?;
    let input = recipe.inputs.first()?;
    Some((input.item_id, (input.quantity / 4).max(1)))
}

fn recipes_by_id() -> &'static HashMap<&'static str, &'static RecipeDefinition> {
    static INDEX: OnceLock<HashMap<&'static str, &'static RecipeDefinition>> = OnceLock::new();
    INDEX.get_or_init(|| {
        REGISTERED_RECIPES
            .iter()
            .map(|recipe| (recipe.id, recipe))
            .collect()
    })
}

pub fn recipe_definition(id: &str) -> Option<&'static RecipeDefinition> {
    recipes_by_id().get(id).copied()
}

pub fn recipes_iter() -> impl Iterator<Item = &'static RecipeDefinition> {
    REGISTERED_RECIPES.iter()
}

/// Intern a recipe id so identical strings produced by the deserializer share
/// a single `Arc<str>`. Mirrors [`crate::items::intern_item_id`].
pub fn intern_recipe_id(id: &str) -> RecipeId {
    let registry = interned_registry();
    if let Some(cached) = registry.read().ok().and_then(|map| map.get(id).cloned()) {
        return cached;
    }
    let fresh: Arc<str> = Arc::from(id);
    if let Ok(mut map) = registry.write() {
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
        let mut map = HashMap::with_capacity(REGISTERED_RECIPES.len());
        for recipe in REGISTERED_RECIPES {
            let arc: Arc<str> = Arc::from(recipe.id);
            map.insert(Box::from(recipe.id), arc);
        }
        RwLock::new(map)
    })
}

/// Resolve the display name of a recipe's output. Falls back to the raw
/// item id when the item registry doesn't know it, which is a programmer
/// error but shouldn't crash the UI.
pub fn output_display_name(recipe: &RecipeDefinition) -> &'static str {
    item_definition(recipe.output_item)
        .map(|definition| definition.name)
        .unwrap_or(recipe.output_item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_has_no_duplicate_ids() {
        let mut seen = HashSet::new();
        for recipe in REGISTERED_RECIPES {
            assert!(
                seen.insert(recipe.id),
                "duplicate recipe id in registry: {}",
                recipe.id
            );
        }
    }

    #[test]
    fn every_recipe_resolves_back_through_the_index() {
        for recipe in REGISTERED_RECIPES {
            assert!(
                recipe_definition(recipe.id).is_some(),
                "recipe {} missing from index",
                recipe.id
            );
        }
    }

    #[test]
    fn every_recipe_output_is_a_known_item() {
        for recipe in REGISTERED_RECIPES {
            assert!(
                item_definition(recipe.output_item).is_some(),
                "recipe {} produces unknown item {}",
                recipe.id,
                recipe.output_item
            );
        }
    }

    #[test]
    fn every_recipe_input_is_a_known_item() {
        for recipe in REGISTERED_RECIPES {
            for input in recipe.inputs {
                assert!(
                    item_definition(input.item_id).is_some(),
                    "recipe {} consumes unknown item {}",
                    recipe.id,
                    input.item_id
                );
            }
        }
    }

    #[test]
    fn intern_returns_same_arc_for_same_id() {
        let a = intern_recipe_id(PLANT_TWINE_RECIPE_ID);
        let b = intern_recipe_id(PLANT_TWINE_RECIPE_ID);
        assert!(Arc::ptr_eq(&a, &b));
    }
}
