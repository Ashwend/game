use super::filter::matches_search;
use super::recipes::{count_in_inventory, has_all_inputs, max_craftable_batch};
use super::rows::build_inputs_galley;
use super::*;
use crate::{
    crafting::{
        PLANT_TWINE_RECIPE_ID, RecipeCategory, STONE_HATCHET_RECIPE_ID, STONE_PICKAXE_RECIPE_ID,
        recipe_definition,
    },
    items::{FIBER_ID, STONE_ID, WOOD_ID},
    protocol::{ItemStack, MAX_CRAFT_BATCH_SIZE},
    server::PlayerPrivate,
};

fn inventory_with(item: &str, qty: u16) -> PlayerInventoryState {
    let mut inv = PlayerInventoryState::empty();
    inv.inventory_slots[0] = Some(ItemStack::new(item, qty));
    inv
}

fn local_player_with_inventory(inventory: PlayerInventoryState) -> LocalPlayerState {
    LocalPlayerState {
        entity: None,
        public: None,
        private: Some(PlayerPrivate {
            inventory,
            crafting: Default::default(),
            open_furnace: None,
            open_loot_bag: None,
            last_processed_input: 0,
            applied_action_seq: 0,
        }),
        lifecycle: None,
    }
}

fn run_ui(f: impl FnMut(&egui::Context)) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 768.0),
            )),
            ..Default::default()
        },
        f,
    )
}

#[test]
fn max_craftable_batch_returns_zero_without_inventory() {
    let recipe = recipe_definition(PLANT_TWINE_RECIPE_ID).expect("recipe");
    assert_eq!(max_craftable_batch(None, recipe), 0);
}

#[test]
fn max_craftable_batch_floors_to_fewest_complete_set() {
    let recipe = recipe_definition(PLANT_TWINE_RECIPE_ID).expect("recipe");
    // Plant twine needs 3 fiber; 11 fiber → 3 twine craftable.
    let inv = inventory_with(FIBER_ID, 11);
    assert_eq!(max_craftable_batch(Some(&inv), recipe), 3);
}

#[test]
fn max_craftable_batch_clamps_at_protocol_max() {
    let recipe = recipe_definition(PLANT_TWINE_RECIPE_ID).expect("recipe");
    // 65535 fiber would naively give 21845, well above the 100 cap.
    let inv = inventory_with(FIBER_ID, u16::MAX);
    assert_eq!(
        max_craftable_batch(Some(&inv), recipe),
        MAX_CRAFT_BATCH_SIZE
    );
}

#[test]
fn has_all_inputs_requires_every_input() {
    let recipe = recipe_definition(STONE_HATCHET_RECIPE_ID).expect("recipe");
    // Stone hatchet needs wood + stone + twine. Having only wood is
    // not enough.
    let only_wood = inventory_with(WOOD_ID, 10);
    assert!(!has_all_inputs(&only_wood, recipe));

    // Plant twine just needs fiber.
    let twine = recipe_definition(PLANT_TWINE_RECIPE_ID).expect("recipe");
    assert!(has_all_inputs(&inventory_with(FIBER_ID, 3), twine));
    assert!(!has_all_inputs(&inventory_with(FIBER_ID, 2), twine));
}

#[test]
fn count_in_inventory_sums_across_inventory_and_actionbar() {
    let mut inv = PlayerInventoryState::empty();
    inv.inventory_slots[0] = Some(ItemStack::new(WOOD_ID, 5));
    inv.inventory_slots[4] = Some(ItemStack::new(WOOD_ID, 7));
    inv.actionbar_slots[1] = Some(ItemStack::new(WOOD_ID, 3));
    inv.inventory_slots[2] = Some(ItemStack::new(STONE_ID, 99));
    assert_eq!(count_in_inventory(&inv, WOOD_ID), 15);
    assert_eq!(count_in_inventory(&inv, STONE_ID), 99);
    assert_eq!(count_in_inventory(&inv, FIBER_ID), 0);
}

#[test]
fn matches_search_hits_name_and_input_material() {
    let recipe = recipe_definition(STONE_HATCHET_RECIPE_ID).expect("recipe");
    // Recipe name.
    assert!(matches_search(recipe, "hatchet"));
    // Input material name (wood) — not in the recipe name.
    assert!(matches_search(recipe, "wood"));
    // Nonsense never matches.
    assert!(!matches_search(recipe, "zzzznotathing"));
}

#[test]
fn collect_sorted_recipes_filters_by_category() {
    let ui_state = CraftingUiState {
        category_filter: Some(RecipeCategory::Tools),
        ..Default::default()
    };
    let entries = collect_sorted_recipes(&ui_state, None);
    assert!(!entries.is_empty());
    assert!(
        entries
            .iter()
            .all(|e| e.recipe.category == RecipeCategory::Tools)
    );
}

#[test]
fn collect_sorted_recipes_search_narrows_to_one() {
    // "handfuls" appears only in the plant twine description.
    let ui_state = CraftingUiState {
        search: "handfuls".to_owned(),
        ..Default::default()
    };
    let entries = collect_sorted_recipes(&ui_state, None);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].recipe.id, PLANT_TWINE_RECIPE_ID);
}

#[test]
fn collect_sorted_recipes_orders_craftable_first() {
    // Inventory enough for plant twine but not the stone tools.
    let inv = inventory_with(FIBER_ID, 100);
    let ui_state = CraftingUiState::default();
    let entries = collect_sorted_recipes(&ui_state, Some(&inv));
    // The first craftable entry must precede any non-craftable one.
    let first_non_craftable = entries.iter().position(|e| !e.craftable);
    let last_craftable = entries.iter().rposition(|e| e.craftable);
    if let (Some(non), Some(craft)) = (first_non_craftable, last_craftable) {
        assert!(
            craft < non,
            "craftable recipes must sort before missing ones"
        );
    }
    // Plant twine is craftable here.
    let twine = entries
        .iter()
        .find(|e| e.recipe.id == PLANT_TWINE_RECIPE_ID)
        .expect("twine present");
    assert!(twine.craftable);
}

#[test]
fn collect_sorted_recipes_only_craftable_hides_unaffordable() {
    let inv = inventory_with(FIBER_ID, 100);
    let ui_state = CraftingUiState {
        only_craftable: true,
        ..Default::default()
    };
    let entries = collect_sorted_recipes(&ui_state, Some(&inv));
    assert!(entries.iter().all(|e| e.craftable));
    // Stone pickaxe (needs wood/stone) must be hidden.
    assert!(
        !entries
            .iter()
            .any(|e| e.recipe.id == STONE_PICKAXE_RECIPE_ID)
    );
}

#[test]
fn build_inputs_galley_reports_shortfall_and_surplus() {
    // Galley layout needs fonts, which only exist inside a `run`.
    let recipe = recipe_definition(PLANT_TWINE_RECIPE_ID).expect("recipe");
    let inv = inventory_with(FIBER_ID, 10);
    let mut short = String::new();
    let mut ok = String::new();
    let mut batch = String::new();
    run_ui(|ctx| {
        // No inventory at all → every input shows "(need N more)".
        short = build_inputs_galley(ctx, recipe, None, 1, 400.0)
            .text()
            .to_owned();
        // Enough fiber → "(have/needed)" form.
        ok = build_inputs_galley(ctx, recipe, Some(&inv), 1, 400.0)
            .text()
            .to_owned();
        // Batch multiplier scales the needed quantity (3 fiber × 4 = 12).
        batch = build_inputs_galley(ctx, recipe, Some(&inv), 4, 400.0)
            .text()
            .to_owned();
    });
    assert!(short.contains("need 3 more"));
    assert!(ok.contains("(10/3)"));
    assert!(batch.contains("×12"));
}

#[test]
fn crafting_ui_noop_when_closed() {
    let mut menu = MenuState {
        crafting_open: false,
        ..Default::default()
    };
    let mut runtime = ClientRuntime::default();
    let local = local_player_with_inventory(PlayerInventoryState::empty());
    let mut ui_state = CraftingUiState::default();
    let mut toasts: Vec<String> = Vec::new();

    let output = run_ui(|ctx| {
        crafting_ui(
            ctx,
            &mut menu,
            &mut runtime,
            &local,
            &mut ui_state,
            &mut toasts,
        );
    });
    // Closed screen paints nothing meaningful.
    assert!(output.shapes.is_empty());
}

#[test]
fn crafting_ui_renders_recipe_rows_when_open() {
    let mut menu = MenuState {
        crafting_open: true,
        ..Default::default()
    };
    let mut runtime = ClientRuntime::default();
    let local = local_player_with_inventory(inventory_with(FIBER_ID, 9));
    let mut ui_state = CraftingUiState::default();
    let mut toasts: Vec<String> = Vec::new();

    let output = run_ui(|ctx| {
        crafting_ui(
            ctx,
            &mut menu,
            &mut runtime,
            &local,
            &mut ui_state,
            &mut toasts,
        );
    });
    assert!(!output.shapes.is_empty());
    // The scroll-reset flag is consumed by the draw.
    assert!(!ui_state.scroll_reset_pending);
    // Rendering at least one recipe seeds its quantity buffer.
    assert!(ui_state.quantities.contains_key(PLANT_TWINE_RECIPE_ID));
}

#[test]
fn crafting_ui_filtered_to_nothing_still_renders_empty_state() {
    let mut menu = MenuState {
        crafting_open: true,
        ..Default::default()
    };
    let mut runtime = ClientRuntime::default();
    let local = local_player_with_inventory(PlayerInventoryState::empty());
    let mut ui_state = CraftingUiState {
        search: "zzzznomatch".to_owned(),
        ..Default::default()
    };
    let mut toasts: Vec<String> = Vec::new();

    let output = run_ui(|ctx| {
        crafting_ui(
            ctx,
            &mut menu,
            &mut runtime,
            &local,
            &mut ui_state,
            &mut toasts,
        );
    });
    // Still draws the panel + "No recipes match your filter." copy.
    assert!(!output.shapes.is_empty());
}
