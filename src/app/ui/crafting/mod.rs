//! Crafting recipe browser and queue HUD.
//!
//! Two distinct surfaces share this module:
//!
//! - [`crafting_body`], the recipe browser. Lists recipes from the static
//!   registry, filters them by name and category, and lets the player enqueue
//!   craft jobs. It renders into a `Ui` supplied by the unified inventory +
//!   crafting panel (see [`crate::app::ui::inventory_panel`]); the panel owns
//!   the window chrome and tab bar, this just fills the body when the Crafting
//!   tab is active.
//! - [`crafting_queue_hud`], always-on top-right stack of progress
//!   cards. Each card shows the name of what's being crafted plus a
//!   live bar, and an `×` button that cancels the job and refunds inputs.
//!   Survives closing the crafting screen, that's the whole point.
//!
//! Authoritative state lives on the server; the UI only reads
//! `runtime.local_player().crafting` and sends [`CraftingCommand`] messages.

mod filter;
mod recipes;
mod rows;

use bevy_egui::egui::{self, RichText};

use crate::{
    app::state::{ClientRuntime, CraftingUiState, ErrorToastSink},
    crafting::{MAX_CRAFTING_QUEUE_LEN, RecipeDefinition},
    protocol::{PlayerCraftingState, PlayerInventoryState},
};

use super::theme;

use filter::{collect_sorted_recipes, draw_filter_row};
use rows::draw_recipe_row;

/// Fill the body of the unified panel with the crafting browser: the filter
/// row (search + category chips + craftable toggle) and the scrollable recipe
/// list. The caller (the inventory panel shell) owns the surrounding `Area`,
/// frame, fixed size, and tab bar, so this only lays out content into the `Ui`
/// it's handed and bounds its scroll area to whatever height is left.
pub(super) fn crafting_body(
    ui: &mut egui::Ui,
    crafting_ui: &mut CraftingUiState,
    inventory: Option<&PlayerInventoryState>,
    crafting_state: &PlayerCraftingState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    draw_filter_row(ui, crafting_ui);
    ui.add_space(10.0);

    // Scroll-reset trick: egui keeps the scroll offset per `Id`, so a
    // pending reset is implemented by swapping to a fresh id for one
    // frame. The next frame returns to the stable id so the player's
    // mid-session scrolling survives until they reopen.
    let scroll_id_salt: u64 = if crafting_ui.scroll_reset_pending {
        crafting_ui.scroll_reset_pending = false;
        1
    } else {
        0
    };
    // While the tutorial is focusing recipes, pin them to the top of the list.
    let pin_tutorial = ui.ctx().memory(|mem| {
        mem.data
            .get_temp::<bool>(crate::app::ui::tutorial::pin_recipes_key())
            .unwrap_or(false)
    });

    // Bound the list to the height the panel left us so the fixed-size shell
    // doesn't grow when the registry overflows; the remainder scrolls.
    let body_height = ui.available_height();
    let scroll_output = egui::ScrollArea::vertical()
        .id_salt(("crafting_recipes_scroll", scroll_id_salt))
        .max_height(body_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let visible_recipes = collect_sorted_recipes(crafting_ui, inventory, pin_tutorial);

            if visible_recipes.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("No recipes match your filter.").color(theme::muted_text()),
                    );
                });
                return;
            }

            let queue_full = crafting_state.len() >= MAX_CRAFTING_QUEUE_LEN;
            for entry in visible_recipes {
                draw_recipe_row(
                    ui,
                    entry.recipe,
                    inventory,
                    entry.craftable,
                    queue_full,
                    crafting_ui,
                    runtime,
                    error_toasts,
                );
            }
        });

    // Keep the wheel working when a recipe-row tooltip is showing. egui's
    // ScrollArea only consumes the mouse wheel when the pointer's top-most
    // interactable layer IS the scroll area's own layer (its
    // `is_hovering_outer_rect` guard fails otherwise). A row's `.on_hover_text`
    // tooltip is rendered as an interactable `Order::Tooltip` area, so once it
    // pops up (after the ~0.5s hover delay) under the cursor it becomes that top
    // layer and the list silently stops reacting to the wheel, the intermittent
    // "scroll sometimes does nothing" (dragging the bar still works because that
    // is a direct press, not a hover-routed wheel). When a tooltip is stealing
    // the layer over the list, apply the wheel to the offset ourselves, mirroring
    // egui's own `offset -= smooth_scroll_delta` convention. This fires only in
    // the exact case egui skips, so the wheel is never applied twice.
    let panel_layer = ui.layer_id();
    if let Some(pointer) = ui.ctx().pointer_hover_pos()
        && scroll_output.inner_rect.contains(pointer)
        && ui.ctx().layer_id_at(pointer) != Some(panel_layer)
    {
        let wheel_y = ui.ctx().input(|input| input.smooth_scroll_delta.y);
        if wheel_y != 0.0
            && let Some(mut state) =
                egui::containers::scroll_area::State::load(ui.ctx(), scroll_output.id)
        {
            let max_offset =
                (scroll_output.content_size.y - scroll_output.inner_rect.height()).max(0.0);
            state.offset.y = (state.offset.y - wheel_y).clamp(0.0, max_offset);
            state.store(ui.ctx(), scroll_output.id);
            // Consume it so no parent scroll area also uses the same delta.
            ui.ctx()
                .input_mut(|input| input.smooth_scroll_delta.y = 0.0);
        }
    }

    // Stash the scroll viewport so the tutorial overlay can clip its recipe
    // outlines to it (a row scrolled out of view must not paint below the panel).
    ui.ctx().memory_mut(|mem| {
        mem.data.insert_temp(
            crate::app::ui::tutorial::craft_viewport_key(),
            scroll_output.inner_rect,
        );
    });
}

pub(super) struct RecipeListEntry<'a> {
    pub(super) recipe: &'a RecipeDefinition,
    pub(super) craftable: bool,
}

#[cfg(test)]
mod tests;
