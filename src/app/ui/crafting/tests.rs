use super::details::{ingredient_status, station_met_label, station_requirement};
use super::filter::matches_search;
use super::recipes::{count_in_inventory, has_all_inputs, max_craftable_batch};
use super::stations::{NearbyStation, StationContext};
use super::*;
use crate::{
    crafting::{
        CLOTH_RECIPE_ID, GUNPOWDER_RECIPE_ID, PLANT_TWINE_RECIPE_ID, RecipeCategory, RecipeStation,
        STONE_HATCHET_RECIPE_ID, STONE_PICKAXE_RECIPE_ID, recipe_definition,
    },
    items::{COAL_ID, DeployableKind, FIBER_ID, STONE_ID, SULFUR_ID, WOOD_ID, WORKBENCH_T1_ID},
    protocol::{ItemStack, MAX_CRAFT_BATCH_SIZE, Vec3Net},
};

/// A `StationContext` with a satisfying tier-1 workbench right on top of the
/// player, so any `Workbench { min_tier: 1 }` recipe reads as station-met.
fn bench_in_range() -> StationContext {
    StationContext::new(
        Some(Vec3Net::ZERO),
        vec![NearbyStation::new(
            DeployableKind::Workbench { tier: 1 },
            WORKBENCH_T1_ID,
            Vec3Net::ZERO,
        )],
    )
}

/// A `StationContext` with no stations at all: hand recipes are still met,
/// workbench recipes are not.
fn no_stations() -> StationContext {
    StationContext::new(Some(Vec3Net::ZERO), Vec::new())
}

fn inventory_with(item: &str, qty: u16) -> PlayerInventoryState {
    let mut inv = PlayerInventoryState::empty();
    inv.inventory_slots[0] = Some(ItemStack::new(item, qty));
    inv
}

fn run_ui(f: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run_ui(
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
    // Input material name (wood), not in the recipe name.
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
    let entries = collect_sorted_recipes(&ui_state, None, &no_stations(), false);
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
    let entries = collect_sorted_recipes(&ui_state, None, &no_stations(), false);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].recipe.id, PLANT_TWINE_RECIPE_ID);
}

#[test]
fn collect_sorted_recipes_orders_craftable_first() {
    // Inventory enough for plant twine but not the stone tools.
    let inv = inventory_with(FIBER_ID, 100);
    let ui_state = CraftingUiState::default();
    let entries = collect_sorted_recipes(&ui_state, Some(&inv), &bench_in_range(), false);
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
fn collect_sorted_recipes_pins_tutorial_recipes_when_focused() {
    // Only fiber on hand: without pinning the stone tools sort low (not
    // craftable), but with the tutorial focusing them they float to the top so
    // their highlight outlines stay on-screen.
    let inv = inventory_with(FIBER_ID, 100);
    let ui_state = CraftingUiState::default();
    let entries = collect_sorted_recipes(&ui_state, Some(&inv), &bench_in_range(), true);
    assert!(entries.len() >= 3);
    for entry in entries.iter().take(3) {
        assert!(
            crate::app::ui::tutorial::is_tutorial_recipe(entry.recipe.id),
            "expected a pinned tutorial recipe at the top, got {}",
            entry.recipe.id
        );
    }
}

#[test]
fn collect_sorted_recipes_only_craftable_hides_unaffordable() {
    let inv = inventory_with(FIBER_ID, 100);
    let ui_state = CraftingUiState {
        only_craftable: true,
        ..Default::default()
    };
    let entries = collect_sorted_recipes(&ui_state, Some(&inv), &bench_in_range(), false);
    assert!(entries.iter().all(|e| e.craftable));
    // Stone pickaxe (needs wood/stone) must be hidden.
    assert!(
        !entries
            .iter()
            .any(|e| e.recipe.id == STONE_PICKAXE_RECIPE_ID)
    );
}

#[test]
fn ingredient_status_reports_shortfall_and_batch_cost() {
    let recipe = recipe_definition(PLANT_TWINE_RECIPE_ID).expect("recipe");
    let input = &recipe.inputs[0]; // 3 fiber per craft
    let inv = inventory_with(FIBER_ID, 10);

    // No inventory at all → everything reads as a shortfall.
    let none = ingredient_status(None, input, 1);
    assert_eq!((none.have, none.needed), (0, 3));
    assert!(none.short());

    // Enough fiber for one craft.
    let ok = ingredient_status(Some(&inv), input, 1);
    assert_eq!((ok.have, ok.needed), (10, 3));
    assert!(!ok.short());

    // The batch multiplier scales the needed quantity (3 fiber × 4 = 12),
    // flipping the same stock into a shortfall.
    let batch = ingredient_status(Some(&inv), input, 4);
    assert_eq!((batch.have, batch.needed), (10, 12));
    assert!(batch.short());

    // A zero multiplier is treated as a batch of one, never zero cost.
    let zero = ingredient_status(Some(&inv), input, 0);
    assert_eq!(zero.needed, 3);
}

#[test]
fn effective_selection_prefers_stored_id_and_falls_back_to_top() {
    let inv = inventory_with(FIBER_ID, 100);
    let ui_state = CraftingUiState::default();
    let entries = collect_sorted_recipes(&ui_state, Some(&inv), &bench_in_range(), false);
    assert!(entries.len() >= 2);

    // No stored selection → the top (most craftable) entry.
    assert_eq!(effective_selection_index(&entries, None), 0);

    // A stored id that's visible wins, wherever it sorts.
    let last = entries.last().expect("entries").recipe.id;
    assert_eq!(
        effective_selection_index(&entries, Some(last)),
        entries.len() - 1
    );

    // A stored id the filter hides falls back to the top entry.
    assert_eq!(
        effective_selection_index(&entries, Some("not_a_real_recipe")),
        0
    );
}

/// Drive [`crafting_body`] inside a throwaway `CentralPanel` so the tests can
/// exercise the recipe browser the same way the unified panel shell does
/// (hand it a `Ui` and let it fill the body).
fn render_body(
    crafting_ui: &mut CraftingUiState,
    inventory: Option<&PlayerInventoryState>,
) -> egui::FullOutput {
    let crafting_state = PlayerCraftingState::default();
    let stations = bench_in_range();
    let mut runtime = ClientRuntime::default();
    let mut toasts: Vec<String> = Vec::new();
    run_ui(|ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            crafting_body(
                ui,
                crafting_ui,
                inventory,
                &crafting_state,
                &stations,
                &mut runtime,
                &mut toasts,
            );
        });
    })
}

#[test]
fn crafting_body_renders_recipe_rows() {
    let inventory = inventory_with(FIBER_ID, 9);
    let mut ui_state = CraftingUiState {
        // Simulate a fresh open: the body should consume the pending reset.
        scroll_reset_pending: true,
        // Pick a known recipe so the detail card (the only place that seeds a
        // quantity buffer now) shows plant twine.
        selected_recipe: Some(PLANT_TWINE_RECIPE_ID),
        ..Default::default()
    };

    let output = render_body(&mut ui_state, Some(&inventory));

    assert!(!output.shapes.is_empty());
    // The scroll-reset flag is consumed by the draw.
    assert!(!ui_state.scroll_reset_pending);
    // The detail card seeds the selected recipe's quantity buffer.
    assert!(ui_state.quantities.contains_key(PLANT_TWINE_RECIPE_ID));
    // Only the selected recipe's buffer is seeded; rows no longer carry
    // steppers, so nothing else writes to the map.
    assert_eq!(ui_state.quantities.len(), 1);
}

#[test]
fn crafting_body_filtered_to_nothing_still_renders_empty_state() {
    let mut ui_state = CraftingUiState {
        search: "zzzznomatch".to_owned(),
        ..Default::default()
    };

    let output = render_body(&mut ui_state, None);

    // Still draws the "No recipes match your filter." copy.
    assert!(!output.shapes.is_empty());
}

#[test]
fn station_labels_read_met_vs_unmet_and_skip_hand_recipes() {
    // Hand recipes have no station line at all.
    assert_eq!(station_met_label(RecipeStation::None), None);
    assert_eq!(station_requirement(RecipeStation::None), None);
    // Workbench recipes read subdued when met, red "Requires ..." when not.
    assert_eq!(
        station_met_label(RecipeStation::Workbench { min_tier: 1 }).as_deref(),
        Some("Workbench Tier 1"),
    );
    assert_eq!(
        station_requirement(RecipeStation::Workbench { min_tier: 2 }).as_deref(),
        Some("Requires Workbench Tier 2"),
    );
}

#[test]
fn row_state_folds_station_gate_into_craftable_for_gunpowder() {
    // Gunpowder needs a workbench (min_tier 1) plus 2 coal + 1 sulfur. With
    // the materials but NO bench, the row is station-unmet and therefore not
    // craftable; with a bench in range it becomes craftable.
    let mut inv = PlayerInventoryState::empty();
    inv.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 10));
    inv.inventory_slots[1] = Some(ItemStack::new(SULFUR_ID, 10));
    let ui_state = CraftingUiState::default();

    let no_bench = collect_sorted_recipes(&ui_state, Some(&inv), &no_stations(), false);
    let gp_no_bench = no_bench
        .iter()
        .find(|e| e.recipe.id == GUNPOWDER_RECIPE_ID)
        .expect("gunpowder present");
    assert!(!gp_no_bench.station_met, "no bench => station unmet");
    assert!(
        !gp_no_bench.craftable,
        "affordable but station-unmet must not count as craftable",
    );

    let with_bench = collect_sorted_recipes(&ui_state, Some(&inv), &bench_in_range(), false);
    let gp_bench = with_bench
        .iter()
        .find(|e| e.recipe.id == GUNPOWDER_RECIPE_ID)
        .expect("gunpowder present");
    assert!(gp_bench.station_met, "bench in range => station met");
    assert!(
        gp_bench.craftable,
        "affordable and station-met must be craftable",
    );
}

#[test]
fn hand_recipe_is_station_met_without_any_station() {
    // Cloth is a hand recipe: 4 fiber, no station. It reads station-met and
    // craftable even with no benches at all.
    let inv = inventory_with(FIBER_ID, 10);
    let ui_state = CraftingUiState::default();
    let entries = collect_sorted_recipes(&ui_state, Some(&inv), &no_stations(), false);
    let cloth = entries
        .iter()
        .find(|e| e.recipe.id == CLOTH_RECIPE_ID)
        .expect("cloth present");
    assert!(cloth.station_met, "hand recipe is always station-met");
    assert!(cloth.craftable, "affordable hand recipe is craftable");
}

#[test]
fn only_craftable_filter_hides_station_unmet_gunpowder() {
    // With materials but no bench, "Only craftable" must hide gunpowder: it is
    // input-affordable but station-unmet, which counts as not craftable.
    let mut inv = PlayerInventoryState::empty();
    inv.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 10));
    inv.inventory_slots[1] = Some(ItemStack::new(SULFUR_ID, 10));
    let ui_state = CraftingUiState {
        only_craftable: true,
        ..Default::default()
    };
    let entries = collect_sorted_recipes(&ui_state, Some(&inv), &no_stations(), false);
    assert!(
        !entries.iter().any(|e| e.recipe.id == GUNPOWDER_RECIPE_ID),
        "station-unmet gunpowder must be hidden by the craftable-only filter",
    );
}
