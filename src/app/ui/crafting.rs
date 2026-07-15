//! Crafting recipe browser and queue HUD.
//!
//! Two distinct surfaces share this module:
//!
//! - [`crafting_body`], the recipe browser: a master/detail split. The left
//!   column ([`list`]) is the searchable, filterable recipe list; the right
//!   column ([`details`]) is a detail card for the selected recipe with the
//!   description, per-ingredient have/need lines, batch quantity, and the
//!   Craft button. It renders into a `Ui` supplied by the unified inventory +
//!   crafting panel (see [`crate::app::ui::inventory_panel`]); the panel owns
//!   the window chrome and tab bar, this just fills the body when the
//!   Crafting tab is active.
//! - [`crafting_queue_hud`](super::crafting_queue::crafting_queue_hud),
//!   always-on top-right stack of progress
//!   cards. Each card shows the name of what's being crafted plus a
//!   live bar, and an `×` button that cancels the job and refunds inputs.
//!   Survives closing the crafting screen, that's the whole point.
//!
//! Authoritative state lives on the server; the UI only reads
//! `runtime.local_player().crafting` and sends [`CraftingCommand`](crate::protocol::CraftingCommand) messages.

mod details;
mod filter;
mod icon;
mod list;
mod recipes;
mod stations;

use bevy_egui::egui::{self, RichText};

use crate::{
    app::state::{ClientRuntime, CraftingUiState, ErrorToastSink},
    crafting::{MAX_CRAFTING_QUEUE_LEN, RecipeDefinition},
    protocol::{PlayerCraftingState, PlayerInventoryState},
};

use super::theme;

use details::draw_recipe_details;
use filter::{collect_sorted_recipes, draw_filter_row};
use list::draw_recipe_list;
pub(crate) use stations::{NearbyStation, StationContext};

/// Width of the recipe list column; the detail card takes the rest of the
/// panel's fixed width.
const LIST_COLUMN_WIDTH: f32 = 340.0;
/// Gap between the list column and the detail card.
const LIST_DETAILS_GAP: f32 = 14.0;

/// Fill the body of the unified panel with the crafting browser: the filter
/// rows (search + craftable toggle + category chips), then the master/detail
/// split. The caller (the inventory panel shell) owns the surrounding `Area`,
/// frame, fixed size, and tab bar, so this only lays out content into the `Ui`
/// it's handed and bounds the two columns to whatever height is left.
pub(super) fn crafting_body(
    ui: &mut egui::Ui,
    crafting_ui: &mut CraftingUiState,
    inventory: Option<&PlayerInventoryState>,
    crafting_state: &PlayerCraftingState,
    stations: &StationContext,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    draw_filter_row(ui, crafting_ui);
    ui.add_space(10.0);

    // While the tutorial is focusing recipes, pin them to the top of the list.
    let pin_tutorial = ui.ctx().memory(|mem| {
        mem.data
            .get_temp::<bool>(crate::app::ui::tutorial::pin_recipes_key())
            .unwrap_or(false)
    });
    let visible_recipes = collect_sorted_recipes(crafting_ui, inventory, stations, pin_tutorial);

    if visible_recipes.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No recipes match your filter.").color(theme::muted_text()));
        });
        return;
    }

    let selected_index = effective_selection_index(&visible_recipes, crafting_ui.selected_recipe);
    let queue_full = crafting_state.len() >= MAX_CRAFTING_QUEUE_LEN;

    // Bound both columns to the height the panel left us so the fixed-size
    // shell doesn't grow when the registry overflows; the list scrolls, the
    // card pins its controls to the bottom.
    let body_height = ui.available_height();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.vertical(|ui| {
            ui.set_width(LIST_COLUMN_WIDTH);
            draw_recipe_list(
                ui,
                &visible_recipes,
                selected_index,
                body_height,
                crafting_ui,
            );
        });
        ui.add_space(LIST_DETAILS_GAP);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            draw_recipe_details(
                ui,
                &visible_recipes[selected_index],
                inventory,
                queue_full,
                body_height,
                crafting_ui,
                runtime,
                error_toasts,
            );
        });
    });
}

/// Resolve which entry the detail card shows: the stored selection when the
/// current filter still lists it, otherwise the top entry (the sort puts the
/// most craftable recipe first). The stored id is deliberately NOT overwritten
/// by the fallback, so clearing a filter restores the player's own selection.
fn effective_selection_index(
    entries: &[RecipeListEntry],
    selected_recipe: Option<&'static str>,
) -> usize {
    selected_recipe
        .and_then(|id| entries.iter().position(|entry| entry.recipe.id == id))
        .unwrap_or(0)
}

pub(super) struct RecipeListEntry<'a> {
    pub(super) recipe: &'a RecipeDefinition,
    /// Affordable AND station-met: the "can I craft this right now" flag
    /// the sort/filter and Craft button key off. A recipe the player can
    /// afford but has no station for counts as not craftable here, so the
    /// "Only craftable" filter hides it and the button stays disabled.
    pub(super) craftable: bool,
    /// Whether the recipe's station requirement is satisfied on its own
    /// (independent of materials). The list dot and the card's meta line use
    /// this to show a subdued vs. red requirement; hand recipes are always
    /// `true`.
    pub(super) station_met: bool,
}

#[cfg(test)]
mod tests;
