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
//!
//! Split by concern into submodules and re-exported flat so `crate::crafting::X`
//! call sites stay stable regardless of which submodule owns `X`.

mod registry;
mod types;

pub use registry::{
    BUILDING_PLAN_RECIPE_ID, CLOTH_RECIPE_ID, CRUDE_FURNACE_RECIPE_ID, GUNPOWDER_RECIPE_ID,
    HAMMER_RECIPE_ID, HEWN_LOG_DOOR_RECIPE_ID, HEWN_LOG_RECIPE_ID, IRON_DOOR_RECIPE_ID,
    IRON_HATCHET_RECIPE_ID, IRON_PICKAXE_RECIPE_ID, PLANT_TWINE_RECIPE_ID, REGISTERED_RECIPES,
    SLEEPING_BAG_RECIPE_ID, STONE_HATCHET_RECIPE_ID, STONE_PICKAXE_RECIPE_ID,
    STORAGE_BOX_LARGE_RECIPE_ID, STORAGE_BOX_SMALL_RECIPE_ID, TOOL_CUPBOARD_RECIPE_ID,
    TORCH_RECIPE_ID, WORKBENCH_T1_RECIPE_ID, output_display_name, recipe_definition,
    recipe_for_output, recipes_iter, repair_material_for,
};
pub use types::{
    CraftingInput, MAX_CRAFTING_QUEUE_LEN, RecipeCategory, RecipeDefinition, RecipeId,
    RecipeStation, intern_recipe_id,
};
