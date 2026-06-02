//! Crafting screen and queue HUD.
//!
//! Two distinct surfaces share this module:
//!
//! - [`crafting_ui`], full-screen modal browser. Lists recipes from the
//!   static registry, filters them by name and category, and lets the
//!   player enqueue craft jobs. Open with `C` (or whatever the player has
//!   rebound `OpenCrafting` to).
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

use bevy_egui::egui::{self, Align, Align2, Id, Layout, Order, RichText};

use crate::{
    app::state::{ClientRuntime, CraftingUiState, ErrorToastSink, LocalPlayerState, MenuState},
    crafting::{MAX_CRAFTING_QUEUE_LEN, RecipeDefinition},
    protocol::{PlayerCraftingState, PlayerInventoryState},
};

use super::{modal::backdrop_layer, theme};

use filter::{collect_sorted_recipes, draw_filter_row};
use rows::draw_recipe_row;

const CRAFTING_PANEL_WIDTH: f32 = 760.0;
const CRAFTING_PANEL_HEIGHT: f32 = 520.0;

/// Render the crafting modal browser when `menu.crafting_open` is true.
/// No-op otherwise, the call is cheap and keeps the top-level ui pipeline
/// simple.
pub(super) fn crafting_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    crafting_ui: &mut CraftingUiState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    if !menu.crafting_open || menu.pause_open {
        return;
    }

    // Scrim. Clicking outside the panel closes the screen, same gesture
    // pattern as the inventory modal.
    let backdrop = backdrop_layer(
        ctx,
        "crafting_backdrop",
        Order::Middle,
        theme::backdrop_color(),
    );
    if backdrop.clicked() {
        menu.crafting_open = false;
        return;
    }

    let inventory = local_player.private.as_ref().map(|p| p.inventory.clone());
    let crafting_state = local_player
        .private
        .as_ref()
        .map(|p| p.crafting.clone())
        .unwrap_or_default();

    egui::Area::new(Id::new("crafting_panel"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(CRAFTING_PANEL_WIDTH);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(CRAFTING_PANEL_WIDTH - 48.0);
                ui.set_min_height(CRAFTING_PANEL_HEIGHT);
                draw_panel_contents(
                    ui,
                    crafting_ui,
                    inventory.as_ref(),
                    &crafting_state,
                    runtime,
                    error_toasts,
                );
            });
        });
}

fn draw_panel_contents(
    ui: &mut egui::Ui,
    crafting_ui: &mut CraftingUiState,
    inventory: Option<&PlayerInventoryState>,
    crafting_state: &PlayerCraftingState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    ui.horizontal(|ui| {
        ui.label(theme::section("Crafting"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "Queue {}/{}",
                    crafting_state.len(),
                    MAX_CRAFTING_QUEUE_LEN
                ))
                .color(theme::muted_text()),
            );
        });
    });
    ui.add_space(8.0);
    ui.label(
        RichText::new("Browse recipes, queue what you need. Inputs are taken when you queue and refunded if you cancel.")
            .color(theme::muted_text())
            .small(),
    );
    ui.add_space(12.0);

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
    egui::ScrollArea::vertical()
        .id_salt(("crafting_recipes_scroll", scroll_id_salt))
        .max_height(CRAFTING_PANEL_HEIGHT - 110.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let visible_recipes = collect_sorted_recipes(crafting_ui, inventory);

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
}

pub(super) struct RecipeListEntry<'a> {
    pub(super) recipe: &'a RecipeDefinition,
    pub(super) craftable: bool,
}

#[cfg(test)]
mod tests;
