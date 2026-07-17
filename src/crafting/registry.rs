//! The `REGISTERED_RECIPES` source-of-truth slice, the stable recipe ids, and
//! the lookups over the slice (`id -> definition` index, repair-material
//! resolution, output-name display). Recipe value types live in
//! [`super::types`].

use std::{collections::HashMap, sync::OnceLock};

use crate::items::{
    ARROW_ID, BANDAGE_ID, BASIC_HATCHET_ID, BASIC_PICKAXE_ID, BUILDING_PLAN_ID, CLOTH_ID, COAL_ID,
    CROSSBOW_ID, CRUDE_FURNACE_ID, FIBER_ID, GUNPOWDER_ID, HAMMER_ID, HEWN_LOG_DOOR_ID,
    HEWN_LOG_ID, IRON_BAR_ID, IRON_BOOTS_ID, IRON_CUIRASS_ID, IRON_DOOR_ID, IRON_GREAVES_ID,
    IRON_HATCHET_ID, IRON_HELM_ID, IRON_MACE_ID, IRON_PICKAXE_ID, IRON_SICKLE_ID, IRON_SWORD_ID,
    LAMELLAR_BOOTS_ID, LAMELLAR_GREAVES_ID, LAMELLAR_HELM_ID, LAMELLAR_VEST_ID, PADDED_HOOD_ID,
    PADDED_LEGGINGS_ID, PADDED_TUNIC_ID, PADDED_WRAPS_ID, PLANT_TWINE_ID, POWDER_BOMB_ID,
    POWDER_KEG_ID, SALVAGED_FITTINGS_ID, SATCHEL_CHARGE_ID, SLEEPING_BAG_ID, STONE_ID,
    STONE_SPEAR_ID, STORAGE_BOX_LARGE_ID, STORAGE_BOX_SMALL_ID, SULFUR_ID, TOOL_CUPBOARD_ID,
    TORCH_ID, WOOD_ID, WOOD_SHUTTER_ID, WOODEN_BOW_ID, WOODEN_CLUB_ID, WORKBENCH_T1_ID,
    item_definition,
};

use super::types::{CraftingInput, RecipeCategory, RecipeDefinition, RecipeStation};

pub const PLANT_TWINE_RECIPE_ID: &str = "plant_twine";
pub const CLOTH_RECIPE_ID: &str = "cloth";
pub const BANDAGE_RECIPE_ID: &str = "bandage";
pub const GUNPOWDER_RECIPE_ID: &str = "gunpowder";
pub const HEWN_LOG_RECIPE_ID: &str = "hewn_log";
pub const STONE_HATCHET_RECIPE_ID: &str = "wood_stone_hatchet";
pub const STONE_PICKAXE_RECIPE_ID: &str = "wood_stone_pickaxe";
pub const IRON_HATCHET_RECIPE_ID: &str = "iron_hatchet";
pub const IRON_PICKAXE_RECIPE_ID: &str = "iron_pickaxe";
pub const WOODEN_CLUB_RECIPE_ID: &str = "wooden_club";
pub const STONE_SPEAR_RECIPE_ID: &str = "stone_spear";
pub const IRON_SWORD_RECIPE_ID: &str = "iron_sword";
pub const IRON_MACE_RECIPE_ID: &str = "iron_mace";
/// Ranged weapons and their ammunition. The bow and arrows are hand/tier-1
/// entry-level; the crossbow is a tier-2 forge job that sinks looted salvaged
/// fittings.
pub const WOODEN_BOW_RECIPE_ID: &str = "wooden_bow";
pub const CROSSBOW_RECIPE_ID: &str = "crossbow";
pub const ARROW_RECIPE_ID: &str = "arrow";
/// Padded (cloth) armor set recipes, hand-craftable. Total across the set is
/// ~14 cloth + 8 plant_twine, split chest-heaviest.
pub const PADDED_HOOD_RECIPE_ID: &str = "padded_hood";
pub const PADDED_TUNIC_RECIPE_ID: &str = "padded_tunic";
pub const PADDED_LEGGINGS_RECIPE_ID: &str = "padded_leggings";
pub const PADDED_WRAPS_RECIPE_ID: &str = "padded_wraps";
/// Lamellar (wood slat) armor set recipes, workbench tier 1. Total across the
/// set is ~10 hewn_log + 8 cloth + 10 plant_twine.
pub const LAMELLAR_HELM_RECIPE_ID: &str = "lamellar_helm";
pub const LAMELLAR_VEST_RECIPE_ID: &str = "lamellar_vest";
pub const LAMELLAR_GREAVES_RECIPE_ID: &str = "lamellar_greaves";
pub const LAMELLAR_BOOTS_RECIPE_ID: &str = "lamellar_boots";
/// Iron (plate) armor set recipes, workbench tier 2. Total across the set is
/// ~40 iron_bar + 10 cloth + 6 salvaged_fittings.
pub const IRON_HELM_RECIPE_ID: &str = "iron_helm";
pub const IRON_CUIRASS_RECIPE_ID: &str = "iron_cuirass";
pub const IRON_GREAVES_RECIPE_ID: &str = "iron_greaves";
pub const IRON_BOOTS_RECIPE_ID: &str = "iron_boots";
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
/// Blackpowder explosive recipes. The bomb and keg are workbench tier 1; the
/// satchel is tier 2 (the salvaged-fittings sink).
pub const POWDER_BOMB_RECIPE_ID: &str = "powder_bomb";
pub const POWDER_KEG_RECIPE_ID: &str = "powder_keg";
pub const SATCHEL_CHARGE_RECIPE_ID: &str = "satchel_charge";
pub const WOOD_SHUTTER_RECIPE_ID: &str = "wood_shutter";
pub const IRON_SICKLE_RECIPE_ID: &str = "iron_sickle";

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
        id: CLOTH_RECIPE_ID,
        name: "Cloth",
        description: "Weave four bundles of plant fiber into a square of coarse cloth.",
        category: RecipeCategory::Materials,
        inputs: &[CraftingInput::new(FIBER_ID, 4)],
        output_item: CLOTH_ID,
        output_quantity: 1,
        craft_seconds: 4.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: BANDAGE_RECIPE_ID,
        name: "Bandage",
        description: "Tear cloth into a strip and roll it. Binds a wound: some of \
                      the mending is immediate, the rest seeps back over the \
                      following seconds. Cheap enough to always carry a few.",
        category: RecipeCategory::Consumables,
        // Deliberately cheap and hand-craftable. Cloth is 4 fiber, so a bandage is
        // 6 hand-gathers of tall grass end to end, with no station, no smelting,
        // and no tech gate. Healing should never be the thing you cannot afford:
        // the item's cost is the 3 seconds and the movement slow you pay to USE it
        // (see game_balance::BANDAGE_*), not the materials.
        inputs: &[
            CraftingInput::new(CLOTH_ID, 1),
            CraftingInput::new(FIBER_ID, 2),
        ],
        output_item: BANDAGE_ID,
        output_quantity: 1,
        craft_seconds: 3.0,
        tier: 0,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: GUNPOWDER_RECIPE_ID,
        name: "Gunpowder",
        description: "Grind coal and sulfur together into coarse blasting powder. \
                      Needs a workbench to mill the charge evenly.",
        category: RecipeCategory::Materials,
        // The master raid-economy lever. Every charge is priced in gunpowder,
        // so this recipe is what makes raiding an investment: 2 coal + 2
        // sulfur PER unit (sulfur is the strategic ore, mined from the
        // leanest node and smelted 1:1 at furnace pace). A satchel run on an
        // iron door (5 charges, 300 powder) is a real farming expedition,
        // not an afternoon errand. Tune raid cost here and in the charge
        // recipes below, never by inflating charge damage.
        inputs: &[
            CraftingInput::new(COAL_ID, 2),
            CraftingInput::new(SULFUR_ID, 2),
        ],
        output_item: GUNPOWDER_ID,
        output_quantity: 1,
        craft_seconds: 4.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
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
            CraftingInput::new(IRON_BAR_ID, 18),
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
            CraftingInput::new(IRON_BAR_ID, 25),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: IRON_PICKAXE_ID,
        output_quantity: 1,
        craft_seconds: 24.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: WOODEN_CLUB_RECIPE_ID,
        name: "Wooden Club",
        description: "Shape a heavy knot of hardwood and wrap the grip. The \
                      first real weapon, hand-crafted from what a starter has.",
        category: RecipeCategory::Weapons,
        inputs: &[
            CraftingInput::new(WOOD_ID, 6),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: WOODEN_CLUB_ID,
        output_quantity: 1,
        craft_seconds: 6.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: STONE_SPEAR_RECIPE_ID,
        name: "Stone Spear",
        description: "Lash a knapped stone point to a long haft. Reaches a metre \
                      past any other melee weapon; slow, but it keeps enemies off.",
        category: RecipeCategory::Weapons,
        inputs: &[
            CraftingInput::new(WOOD_ID, 8),
            CraftingInput::new(STONE_ID, 4),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: STONE_SPEAR_ID,
        output_quantity: 1,
        craft_seconds: 8.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: IRON_SWORD_RECIPE_ID,
        name: "Iron Sword",
        description: "Forge an iron blade and set it on a wrapped hewn grip. The \
                      balanced workhorse, good in any fight.",
        category: RecipeCategory::Weapons,
        inputs: &[
            CraftingInput::new(IRON_BAR_ID, 15),
            CraftingInput::new(HEWN_LOG_ID, 1),
            CraftingInput::new(CLOTH_ID, 1),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: IRON_SWORD_ID,
        output_quantity: 1,
        craft_seconds: 20.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: IRON_MACE_RECIPE_ID,
        name: "Iron Mace",
        description: "Forge a brutal iron head and haul it onto a heavy haft. The \
                      slowest, hardest swing in the game, and the answer to plate.",
        category: RecipeCategory::Weapons,
        inputs: &[
            CraftingInput::new(IRON_BAR_ID, 20),
            CraftingInput::new(HEWN_LOG_ID, 2),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: IRON_MACE_ID,
        output_quantity: 1,
        craft_seconds: 24.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 2 },
    },
    RecipeDefinition {
        id: WOODEN_BOW_RECIPE_ID,
        name: "Wooden Bow",
        description: "Bend a green stave and string it with waxed twine. Hold to \
                      draw; the longer you pull, the harder it hits.",
        category: RecipeCategory::Weapons,
        inputs: &[
            CraftingInput::new(WOOD_ID, 12),
            CraftingInput::new(PLANT_TWINE_ID, 6),
            CraftingInput::new(CLOTH_ID, 2),
        ],
        output_item: WOODEN_BOW_ID,
        output_quantity: 1,
        craft_seconds: 16.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: CROSSBOW_RECIPE_ID,
        name: "Crossbow",
        description: "Forge an iron prod onto a hewn stock and geared latch. Slow \
                      to reload, but a single bolt hits like nothing else at range.",
        category: RecipeCategory::Weapons,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 6),
            CraftingInput::new(IRON_BAR_ID, 8),
            CraftingInput::new(SALVAGED_FITTINGS_ID, 4),
            CraftingInput::new(PLANT_TWINE_ID, 4),
        ],
        output_item: CROSSBOW_ID,
        output_quantity: 1,
        craft_seconds: 28.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 2 },
    },
    RecipeDefinition {
        id: ARROW_RECIPE_ID,
        name: "Arrow",
        description: "Fletch a stone-tipped shaft. Crafts four at a time; missed \
                      shots stick where they land and can be picked back up \
                      before they're lost.",
        category: RecipeCategory::Weapons,
        inputs: &[
            CraftingInput::new(WOOD_ID, 2),
            CraftingInput::new(STONE_ID, 1),
            CraftingInput::new(FIBER_ID, 1),
        ],
        output_item: ARROW_ID,
        output_quantity: 4,
        craft_seconds: 4.0,
        tier: 1,
        station: RecipeStation::None,
    },
    // Padded (cloth) armor set. Hand-craftable. The set totals ~14 cloth + 8
    // plant_twine, split chest-heaviest: chest 6/3, head 3/2, legs 3/2, feet
    // 2/1.
    RecipeDefinition {
        id: PADDED_HOOD_RECIPE_ID,
        name: "Padded Helmet",
        description: "Quilt a cloth hood and bind it with twine. Turns a \
                      glancing blow.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(CLOTH_ID, 3),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: PADDED_HOOD_ID,
        output_quantity: 1,
        craft_seconds: 6.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: PADDED_TUNIC_RECIPE_ID,
        name: "Padded Chestplate",
        description: "Layer and quilt cloth into a thick tunic. The most \
                      protective padded piece.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(CLOTH_ID, 6),
            CraftingInput::new(PLANT_TWINE_ID, 3),
        ],
        output_item: PADDED_TUNIC_ID,
        output_quantity: 1,
        craft_seconds: 10.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: PADDED_LEGGINGS_RECIPE_ID,
        name: "Padded Leggings",
        description: "Quilt cloth leggings and bind them with twine.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(CLOTH_ID, 3),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: PADDED_LEGGINGS_ID,
        output_quantity: 1,
        craft_seconds: 7.0,
        tier: 1,
        station: RecipeStation::None,
    },
    RecipeDefinition {
        id: PADDED_WRAPS_RECIPE_ID,
        name: "Padded Boots",
        description: "Bind cloth wraps around the feet. The lightest padded \
                      piece.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(CLOTH_ID, 2),
            CraftingInput::new(PLANT_TWINE_ID, 1),
        ],
        output_item: PADDED_WRAPS_ID,
        output_quantity: 1,
        craft_seconds: 4.0,
        tier: 1,
        station: RecipeStation::None,
    },
    // Lamellar (wood slat) armor set. Workbench tier 1. The set totals ~10
    // hewn_log + 8 cloth + 10 plant_twine, split chest-heaviest: chest 4/3/4,
    // head 3/2/2, legs 2/2/3, feet 1/1/1.
    RecipeDefinition {
        id: LAMELLAR_HELM_RECIPE_ID,
        name: "Lamellar Helmet",
        description: "Lace hewn-wood slats over a padded cap. Twice the cover \
                      of the padded helmet.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 3),
            CraftingInput::new(CLOTH_ID, 2),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: LAMELLAR_HELM_ID,
        output_quantity: 1,
        craft_seconds: 12.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: LAMELLAR_VEST_RECIPE_ID,
        name: "Lamellar Chestplate",
        description: "Lash rows of wood slats over a cloth backing, with a \
                      slatted shoulder cap. The most protective lamellar piece.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 4),
            CraftingInput::new(CLOTH_ID, 3),
            CraftingInput::new(PLANT_TWINE_ID, 4),
        ],
        output_item: LAMELLAR_VEST_ID,
        output_quantity: 1,
        craft_seconds: 18.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: LAMELLAR_GREAVES_RECIPE_ID,
        name: "Lamellar Leggings",
        description: "Slat wood greaves over padded leggings.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 2),
            CraftingInput::new(CLOTH_ID, 2),
            CraftingInput::new(PLANT_TWINE_ID, 3),
        ],
        output_item: LAMELLAR_GREAVES_ID,
        output_quantity: 1,
        craft_seconds: 13.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: LAMELLAR_BOOTS_RECIPE_ID,
        name: "Lamellar Boots",
        description: "Slat wood boots over bound cloth. The lightest lamellar \
                      piece.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 1),
            CraftingInput::new(CLOTH_ID, 1),
            CraftingInput::new(PLANT_TWINE_ID, 1),
        ],
        output_item: LAMELLAR_BOOTS_ID,
        output_quantity: 1,
        craft_seconds: 9.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    // Iron (plate) armor set. Workbench tier 2. The set totals ~40 iron_bar +
    // 10 cloth + 6 salvaged_fittings, split chest-heaviest: chest 16/4/3, head
    // 10/2/1, legs 9/2/1, feet 5/2/1.
    RecipeDefinition {
        id: IRON_HELM_RECIPE_ID,
        name: "Iron Helmet",
        description: "Forge an iron helm over a padded cap. Turns aside all but \
                      the heaviest blows.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(IRON_BAR_ID, 10),
            CraftingInput::new(CLOTH_ID, 2),
            CraftingInput::new(SALVAGED_FITTINGS_ID, 1),
        ],
        output_item: IRON_HELM_ID,
        output_quantity: 1,
        craft_seconds: 22.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 2 },
    },
    RecipeDefinition {
        id: IRON_CUIRASS_RECIPE_ID,
        name: "Iron Chestplate",
        description: "Forge a breastplate over padding, with a plate pauldron. \
                      The most protective piece in the game.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(IRON_BAR_ID, 16),
            CraftingInput::new(CLOTH_ID, 4),
            CraftingInput::new(SALVAGED_FITTINGS_ID, 3),
        ],
        output_item: IRON_CUIRASS_ID,
        output_quantity: 1,
        craft_seconds: 30.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 2 },
    },
    RecipeDefinition {
        id: IRON_GREAVES_RECIPE_ID,
        name: "Iron Leggings",
        description: "Forge plate greaves over padded leggings.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(IRON_BAR_ID, 9),
            CraftingInput::new(CLOTH_ID, 2),
            CraftingInput::new(SALVAGED_FITTINGS_ID, 1),
        ],
        output_item: IRON_GREAVES_ID,
        output_quantity: 1,
        craft_seconds: 24.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 2 },
    },
    RecipeDefinition {
        id: IRON_BOOTS_RECIPE_ID,
        name: "Iron Boots",
        description: "Forge plate boots over padding. The lightest iron piece, \
                      and still plate.",
        category: RecipeCategory::Armor,
        inputs: &[
            CraftingInput::new(IRON_BAR_ID, 5),
            CraftingInput::new(CLOTH_ID, 2),
            CraftingInput::new(SALVAGED_FITTINGS_ID, 1),
        ],
        output_item: IRON_BOOTS_ID,
        output_quantity: 1,
        craft_seconds: 16.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 2 },
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
            CraftingInput::new(IRON_BAR_ID, 40),
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
    // Blackpowder explosives. Costs are spec-exact. The bomb and keg are the
    // workbench-tier-1 raiding starters; the satchel is tier-2 and sinks the
    // looted salvaged fittings.
    RecipeDefinition {
        id: POWDER_BOMB_RECIPE_ID,
        name: "Powder Bomb",
        description: "Wrap a handful of powder in cloth and lash on a fuse. Throw \
                      it: shreds sticks huts, chips hewn wood.",
        category: RecipeCategory::Explosives,
        inputs: &[
            CraftingInput::new(GUNPOWDER_ID, 10),
            CraftingInput::new(CLOTH_ID, 2),
            CraftingInput::new(PLANT_TWINE_ID, 1),
        ],
        output_item: POWDER_BOMB_ID,
        output_quantity: 1,
        craft_seconds: 8.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: POWDER_KEG_RECIPE_ID,
        name: "Powder Keg",
        description: "Pack a staved barrel with powder and hoop it in iron. The \
                      workhorse breaching charge. Place it and stand clear.",
        category: RecipeCategory::Explosives,
        inputs: &[
            CraftingInput::new(GUNPOWDER_ID, 30),
            CraftingInput::new(WOOD_ID, 15),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: POWDER_KEG_ID,
        output_quantity: 1,
        craft_seconds: 12.0,
        tier: 2,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: SATCHEL_CHARGE_RECIPE_ID,
        name: "Satchel Charge",
        description: "Bind several charges into a strapped satchel. Real numbers \
                      against stone, and the first charge that touches iron.",
        category: RecipeCategory::Explosives,
        inputs: &[
            CraftingInput::new(GUNPOWDER_ID, 60),
            CraftingInput::new(CLOTH_ID, 4),
            CraftingInput::new(SALVAGED_FITTINGS_ID, 2),
        ],
        output_item: SATCHEL_CHARGE_ID,
        output_quantity: 1,
        craft_seconds: 20.0,
        tier: 3,
        station: RecipeStation::Workbench { min_tier: 2 },
    },
    RecipeDefinition {
        id: WOOD_SHUTTER_RECIPE_ID,
        name: "Window Shutter",
        description: "Batten hewn boards into a window panel. Mounts in a \
                      window opening; no lock, base authorization swings it.",
        category: RecipeCategory::Building,
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 3),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: WOOD_SHUTTER_ID,
        output_quantity: 1,
        craft_seconds: 10.0,
        tier: 1,
        station: RecipeStation::Workbench { min_tier: 1 },
    },
    RecipeDefinition {
        id: IRON_SICKLE_RECIPE_ID,
        name: "Iron Sickle",
        description: "Forge a curved iron blade onto a short haft. Reaps a \
                      whole tuft of tall grass in one sweep.",
        category: RecipeCategory::Tools,
        // Bench-tier beside the iron hatchet/pickaxe, a touch cheaper (a
        // light harvest blade, not a work head). Everything it accelerates
        // (the cloth/twine sinks: armor, bow, bags) is bench-tier anyway;
        // before the forge, fiber comes from bare-hand tuft plucks.
        inputs: &[
            CraftingInput::new(HEWN_LOG_ID, 1),
            CraftingInput::new(IRON_BAR_ID, 8),
            CraftingInput::new(PLANT_TWINE_ID, 2),
        ],
        output_item: IRON_SICKLE_ID,
        output_quantity: 1,
        craft_seconds: 16.0,
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

/// The recipe that crafts `item_id`, looked up by its `output_item`. Used by the
/// defuse refund (recover half a charge's recipe materials) the same way
/// [`repair_material_for`] scans by output. `None` for world-spawned kinds that
/// nothing crafts into.
pub fn recipe_for_output(item_id: &str) -> Option<&'static RecipeDefinition> {
    REGISTERED_RECIPES
        .iter()
        .find(|recipe| recipe.output_item == item_id)
}

pub fn recipes_iter() -> impl Iterator<Item = &'static RecipeDefinition> {
    REGISTERED_RECIPES.iter()
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
    fn the_bandage_is_cheap_hand_craftable_and_needs_no_station() {
        // Healing must never be the thing you cannot afford. The bandage's cost is
        // the 3 seconds and the movement slow you pay to USE it (see
        // game_balance::BANDAGE_*), not the materials, so this pins the recipe as
        // station-free and buildable out of the two most abundant materials in the
        // game. If a future pass gates it behind a workbench or an iron ingot, this
        // fails loudly rather than quietly making the game harsher.
        let recipe = recipe_definition(BANDAGE_RECIPE_ID).expect("bandage recipe is registered");
        assert_eq!(recipe.output_item, BANDAGE_ID);
        assert_eq!(
            recipe.station,
            RecipeStation::None,
            "must be hand-craftable"
        );
        assert_eq!(recipe.category, RecipeCategory::Consumables);

        // Every input must itself be hand-gatherable or trivially hand-crafted from
        // something that is: cloth is 4 fiber, and fiber comes off tall grass with
        // bare hands.
        for input in recipe.inputs {
            assert!(
                input.item_id == CLOTH_ID || input.item_id == FIBER_ID,
                "bandage input {} is not a basic hand-gathered material",
                input.item_id
            );
        }
        assert!(recipe.craft_seconds <= 5.0, "should be a quick craft");
    }
}
