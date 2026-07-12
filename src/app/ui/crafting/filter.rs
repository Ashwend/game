//! Filter row (search + category chips) and the recipe collection/sort
//! pipeline that feeds the crafting browser list.

use bevy_egui::egui::{self, Align, Layout};

use crate::{
    app::state::CraftingUiState,
    crafting::{RecipeCategory, RecipeDefinition, output_display_name, recipes_iter},
    items::item_definition,
    protocol::PlayerInventoryState,
};

use super::recipes::has_all_inputs;
use super::stations::StationContext;
use super::{RecipeListEntry, theme};

/// Apply the user's filter chips + search query, then sort the survivors
/// so the most useful recipes float to the top.
///
/// Sort order:
/// 1. Craftable recipes before missing-material ones, the player almost
///    always wants to see what they *can* make first.
/// 2. Higher [`RecipeDefinition::tier`] above lower, a stone pickaxe
///    outranks plant twine when both are craftable.
/// 3. Ties broken alphabetically by recipe name so the order is stable
///    across frames (otherwise the list could jitter as `HashMap`-backed
///    sources reorder).
pub(super) fn collect_sorted_recipes<'a>(
    crafting_ui: &CraftingUiState,
    inventory: Option<&PlayerInventoryState>,
    stations: &StationContext,
    pin_tutorial: bool,
) -> Vec<RecipeListEntry<'a>> {
    let needle = crafting_ui.search.trim().to_lowercase();
    let mut entries: Vec<RecipeListEntry<'a>> = recipes_iter()
        .filter(|recipe| {
            if let Some(category) = crafting_ui.category_filter
                && recipe.category != category
            {
                return false;
            }
            if !needle.is_empty() && !matches_search(recipe, &needle) {
                return false;
            }
            true
        })
        .map(|recipe| {
            let affordable = inventory
                .map(|inv| has_all_inputs(inv, recipe))
                .unwrap_or(false);
            let station_met = stations.met(recipe.station);
            // A recipe you can afford but have no station for is not
            // craftable right now: it keeps the "Only craftable" filter and
            // the sort honest (and the Craft button disabled), matching the
            // server's own station gate.
            RecipeListEntry {
                recipe,
                craftable: affordable && station_met,
                station_met,
            }
        })
        .filter(|entry| !crafting_ui.only_craftable || entry.craftable)
        .collect();
    entries.sort_by(|a, b| {
        // While the tutorial is focusing recipes, float them to the very top so
        // their highlight outlines stay on-screen instead of below the fold.
        let a_pin = pin_tutorial && crate::app::ui::tutorial::is_tutorial_recipe(a.recipe.id);
        let b_pin = pin_tutorial && crate::app::ui::tutorial::is_tutorial_recipe(b.recipe.id);
        b_pin
            .cmp(&a_pin)
            .then(b.craftable.cmp(&a.craftable))
            .then(b.recipe.tier.cmp(&a.recipe.tier))
            .then(a.recipe.name.cmp(b.recipe.name))
    });
    entries
}

pub(super) fn draw_filter_row(ui: &mut egui::Ui, crafting_ui: &mut CraftingUiState) {
    //  Row 1: search field with the "Only craftable" toggle pinned to its
    //  right (the toggle used to share the chip row, where a growing category
    //  list eventually overlapped it).
    //  Row 2: category chips, sized to their labels and wrapped if the enum
    //  ever outgrows the panel width.
    ui.horizontal(|ui| {
        ui.set_min_height(theme::COMPACT_ROW_HEIGHT);
        let toggle_width = 130.0;
        let search_width =
            (ui.available_width() - toggle_width - ui.spacing().item_spacing.x).max(120.0);
        // Pin the TextEdit id so `request_focus` / the C-hotkey focus guard
        // can target it across frames. The field is *not* auto-focused on
        // open: players mostly browse via the chips, and clicking still
        // focuses it.
        let _ = ui.add_sized(
            [search_width, theme::COMPACT_ROW_HEIGHT],
            theme::text_input(&mut crafting_ui.search)
                .id(egui::Id::new("crafting_search_input"))
                .hint_text("Search recipes…"),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.checkbox(&mut crafting_ui.only_craftable, "Only craftable");
        });
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        // Category chips behave like a radio group: clicking any chip
        // makes it the active filter, even if it was already selected.
        // "All" is the explicit way to clear the filter.
        let all_selected = crafting_ui.category_filter.is_none();
        if category_chip(ui, "All", all_selected) {
            crafting_ui.category_filter = None;
        }
        for &category in RecipeCategory::ALL {
            let selected = crafting_ui.category_filter == Some(category);
            if category_chip(ui, category.label(), selected) {
                crafting_ui.category_filter = Some(category);
            }
        }
    });
}

fn category_chip(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let kind = if selected {
        theme::ButtonKind::Primary
    } else {
        theme::ButtonKind::Secondary
    };
    // A small minimum keeps short labels ("All", "Misc") from shrinking to
    // stamps while longer labels size to their text, so the whole set fits on
    // one row (the old fixed 90px chips overflowed the panel).
    let response = theme::compact_button(ui, label, kind, 52.0);
    response.clicked()
}

pub(super) fn matches_search(recipe: &RecipeDefinition, needle: &str) -> bool {
    let lower_name = recipe.name.to_lowercase();
    if lower_name.contains(needle) {
        return true;
    }
    let lower_description = recipe.description.to_lowercase();
    if lower_description.contains(needle) {
        return true;
    }
    let output_name = output_display_name(recipe).to_lowercase();
    if output_name.contains(needle) {
        return true;
    }
    recipe.inputs.iter().any(|input| {
        item_definition(input.item_id)
            .map(|def| def.name.to_lowercase().contains(needle))
            .unwrap_or(false)
    })
}
