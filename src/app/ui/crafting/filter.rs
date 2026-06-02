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
            let craftable = inventory
                .map(|inv| has_all_inputs(inv, recipe))
                .unwrap_or(false);
            RecipeListEntry { recipe, craftable }
        })
        .filter(|entry| !crafting_ui.only_craftable || entry.craftable)
        .collect();
    entries.sort_by(|a, b| {
        b.craftable
            .cmp(&a.craftable)
            .then(b.recipe.tier.cmp(&a.recipe.tier))
            .then(a.recipe.name.cmp(b.recipe.name))
    });
    entries
}

pub(super) fn draw_filter_row(ui: &mut egui::Ui, crafting_ui: &mut CraftingUiState) {
    // Two rows so the right-anchored checkbox can't collide with the
    // rightmost category chip when the panel is at its minimum width.
    //  Row 1: search field on the left, "Only craftable" toggle on the right.
    //  Row 2: category chips.
    ui.horizontal(|ui| {
        // Same `add_sized` trick the worlds-dialog forms use: pinning
        // both label and input to `COMPACT_ROW_HEIGHT` lines their
        // text baselines up. Without the sized label the bare `ui.label`
        // is shorter than the padded input and rides at the top of the
        // row, which reads as misalignment.
        ui.add_sized(
            [56.0, theme::COMPACT_ROW_HEIGHT],
            egui::Label::new(theme::field_label("Search")),
        );
        // Pin the TextEdit id so `request_focus` can target it across
        // frames, egui auto-ids are stable enough here, but a named id
        // also lets future "Ctrl+F to focus search"-style shortcuts hit
        // the same widget without scraping memory.
        // Search field is *not* auto-focused on open, players use the
        // crafting screen mostly via category chips and clicking, not
        // typing. Clicking the field still focuses it normally. See the
        // toggle system for the rationale.
        let _ = ui.add_sized(
            [260.0, theme::COMPACT_ROW_HEIGHT],
            theme::text_input(&mut crafting_ui.search)
                .id(egui::Id::new("crafting_search_input"))
                .hint_text("Recipe or material…"),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.checkbox(&mut crafting_ui.only_craftable, "Only craftable");
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
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
    let response = theme::compact_button(ui, label, kind, 90.0);
    theme::record_click_sound(ui, &response);
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
