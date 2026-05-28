//! Crafting screen and queue HUD.
//!
//! Two distinct surfaces share this module:
//!
//! - [`crafting_ui`] — full-screen modal browser. Lists recipes from the
//!   static registry, filters them by name and category, and lets the
//!   player enqueue craft jobs. Open with `C` (or whatever the player has
//!   rebound `OpenCrafting` to).
//! - [`crafting_queue_hud`] — always-on top-right stack of progress
//!   cards. Each card shows the name of what's being crafted plus a
//!   live bar, and an `×` button that cancels the job and refunds inputs.
//!   Survives closing the crafting screen — that's the whole point.
//!
//! Authoritative state lives on the server; the UI only reads
//! `runtime.local_player().crafting` and sends [`CraftingCommand`] messages.

use bevy_egui::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Layout, Order, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, Vec2,
    text::{LayoutJob, TextFormat, TextWrapping},
    vec2,
};

use crate::{
    app::{
        state::{
            ClientRuntime, CraftingHudState, CraftingUiState, ErrorToastSink, LocalPlayerState,
            MenuState, ProgressBaseline,
        },
        systems::send_crafting_command,
    },
    crafting::{
        MAX_CRAFTING_QUEUE_LEN, RecipeCategory, RecipeDefinition, output_display_name, recipes_iter,
    },
    items::{ItemTint, item_definition},
    protocol::{
        CraftingCommand, CraftingJob, MAX_CRAFT_BATCH_SIZE, PlayerCraftingState,
        PlayerInventoryState, SERVER_TICK_RATE_HZ,
    },
};

use super::{modal::backdrop_layer, theme};

const CRAFTING_PANEL_WIDTH: f32 = 760.0;
const CRAFTING_PANEL_HEIGHT: f32 = 520.0;
const RECIPE_ROW_HEIGHT: f32 = 64.0;
/// Tint used for the input-line chunk when the player is short on that
/// material. Sourced from the warning toast palette so missing-material
/// reads consistently across the UI.
const INPUT_MISSING_COLOR: Color32 = Color32::from_rgb(228, 154, 154);

/// Render the crafting modal browser when `menu.crafting_open` is true.
/// No-op otherwise — the call is cheap and keeps the top-level ui pipeline
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

    // Scrim. Clicking outside the panel closes the screen — same gesture
    // pattern as the inventory modal.
    let backdrop = backdrop_layer(
        ctx,
        "crafting_backdrop",
        Order::Middle,
        Color32::from_rgba_unmultiplied(1, 3, 7, 190),
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

/// Apply the user's filter chips + search query, then sort the survivors
/// so the most useful recipes float to the top.
///
/// Sort order:
/// 1. Craftable recipes before missing-material ones — the player almost
///    always wants to see what they *can* make first.
/// 2. Higher [`RecipeDefinition::tier`] above lower — a stone pickaxe
///    outranks plant twine when both are craftable.
/// 3. Ties broken alphabetically by recipe name so the order is stable
///    across frames (otherwise the list could jitter as `HashMap`-backed
///    sources reorder).
fn collect_sorted_recipes<'a>(
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

struct RecipeListEntry<'a> {
    recipe: &'a RecipeDefinition,
    craftable: bool,
}

fn draw_filter_row(ui: &mut egui::Ui, crafting_ui: &mut CraftingUiState) {
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
        // frames — egui auto-ids are stable enough here, but a named id
        // also lets future "Ctrl+F to focus search"-style shortcuts hit
        // the same widget without scraping memory.
        // Search field is *not* auto-focused on open — players use the
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

#[allow(clippy::too_many_arguments)]
fn draw_recipe_row(
    ui: &mut egui::Ui,
    recipe: &RecipeDefinition,
    inventory: Option<&PlayerInventoryState>,
    craftable: bool,
    queue_full: bool,
    crafting_ui: &mut CraftingUiState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), RECIPE_ROW_HEIGHT),
        Sense::hover(),
    );
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(4), theme::input_fill());
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );

    let inner_padding = 12.0;
    let icon_size = 36.0;
    let icon_rect = Rect::from_min_size(
        Pos2::new(
            rect.left() + inner_padding,
            rect.center().y - icon_size * 0.5,
        ),
        Vec2::splat(icon_size),
    );
    paint_recipe_icon(&painter, icon_rect, recipe);

    // The right-side controls cluster — minus, quantity input, plus, then
    // Craft. We need their total width to know where the text column
    // ends, so compute the cluster width first and lay everything out
    // from the right edge.
    let craft_button_width = 90.0;
    let qty_button_width = 28.0;
    let qty_input_width = 48.0;
    let inter_widget_gap = 4.0;
    let cluster_to_craft_gap = 8.0;
    let cluster_width = qty_button_width
        + inter_widget_gap
        + qty_input_width
        + inter_widget_gap
        + qty_button_width
        + cluster_to_craft_gap
        + craft_button_width;

    let text_left = icon_rect.right() + 12.0;
    // Stops short of the cluster so long input lines elide cleanly
    // instead of slipping under it.
    let text_right_edge = rect.right() - inner_padding - cluster_width - 14.0;

    // --- Quantity state (lives in CraftingUiState so it survives a row
    // re-render and per-recipe edits don't leak between recipes). ---
    let max_batch = max_craftable_batch(inventory, recipe);
    // `or_insert_with` so the *first* render seeds "1" without forcing
    // every recipe row to write to the map every frame.
    let buffer = crafting_ui
        .quantities
        .entry(recipe.id)
        .or_insert_with(|| "1".to_owned());
    // Strip non-digits before parsing - the input widget catches most of
    // them, but a pasted "12x" or leading whitespace would otherwise
    // make every frame's parse fail and silently disable the Craft
    // button.
    buffer.retain(|c| c.is_ascii_digit());
    if buffer.is_empty() {
        // Don't auto-substitute "1" while the player is mid-edit; an
        // empty buffer is treated as "no valid quantity" further down so
        // the Craft button disables and the +/− buttons key off the
        // last known value.
    }
    let typed_qty: Option<u16> = buffer.trim().parse::<u16>().ok().filter(|n| *n > 0);
    let display_qty = typed_qty.unwrap_or(1).min(MAX_CRAFT_BATCH_SIZE);

    // Title: includes the recipe's per-craft output multiplier (a recipe
    // that crafts ×4 per click is labelled "Plant Twine ×4"). The
    // *batch* multiplier lives in the quantity input below and is shown
    // separately on the queue card.
    let title = if recipe.output_quantity > 1 {
        format!("{} ×{}", recipe.name, recipe.output_quantity)
    } else {
        recipe.name.to_owned()
    };
    painter.text(
        Pos2::new(text_left, rect.top() + 10.0),
        Align2::LEFT_TOP,
        title,
        FontId::new(14.0, FontFamily::Proportional),
        theme::text(),
    );
    let category = recipe.category.label();
    let total_seconds = recipe.craft_seconds * display_qty as f32;
    let time_text = if display_qty > 1 {
        format!(
            "{category} • {:.0}s × {}",
            recipe.craft_seconds, display_qty
        )
    } else {
        format!("{category} • {:.0}s", recipe.craft_seconds)
    };
    painter.text(
        Pos2::new(text_left, rect.top() + 28.0),
        Align2::LEFT_TOP,
        time_text,
        FontId::new(11.5, FontFamily::Proportional),
        theme::muted_text(),
    );
    let _ = total_seconds; // kept for future "total: Xs" callouts
    let inputs_galley = build_inputs_galley(
        ui.ctx(),
        recipe,
        inventory,
        display_qty,
        text_right_edge - text_left,
    );
    let inputs_pos = Pos2::new(text_left, rect.top() + 44.0);
    painter.galley(inputs_pos, inputs_galley, theme::text());

    // --- Right-cluster layout (right-to-left). ---
    let button_height = 32.0;
    let cluster_top = rect.center().y - button_height * 0.5;
    let craft_rect = Rect::from_min_size(
        Pos2::new(
            rect.right() - inner_padding - craft_button_width,
            cluster_top,
        ),
        Vec2::new(craft_button_width, button_height),
    );
    let plus_rect = Rect::from_min_size(
        Pos2::new(
            craft_rect.left() - cluster_to_craft_gap - qty_button_width,
            cluster_top,
        ),
        Vec2::new(qty_button_width, button_height),
    );
    let input_rect = Rect::from_min_size(
        Pos2::new(
            plus_rect.left() - inter_widget_gap - qty_input_width,
            cluster_top,
        ),
        Vec2::new(qty_input_width, button_height),
    );
    let minus_rect = Rect::from_min_size(
        Pos2::new(
            input_rect.left() - inter_widget_gap - qty_button_width,
            cluster_top,
        ),
        Vec2::new(qty_button_width, button_height),
    );

    // --- Minus button. ---
    let minus_can_decrement = display_qty > 1;
    let minus_response = theme::compact_button_in_rect(
        ui,
        ("crafting_qty_minus", recipe.id),
        minus_rect,
        "−",
        theme::ButtonKind::Secondary,
    );
    if minus_response.clicked() && minus_can_decrement {
        let next = display_qty.saturating_sub(1).max(1);
        *crafting_ui
            .quantities
            .entry(recipe.id)
            .or_insert_with(|| "1".to_owned()) = next.to_string();
    }

    // --- Quantity input field. ---
    // Re-grab the buffer mutably: the minus click above may have just
    // replaced it. Using the entry API avoids a second lookup on the
    // happy path.
    let buffer_mut = crafting_ui
        .quantities
        .entry(recipe.id)
        .or_insert_with(|| "1".to_owned());
    let input_response = ui.put(
        input_rect,
        theme::text_input(buffer_mut)
            .id(egui::Id::new(("crafting_qty_input", recipe.id)))
            .desired_width(qty_input_width - 16.0)
            .horizontal_align(egui::Align::Center),
    );
    if input_response.changed() {
        // Filter again post-edit. The widget itself doesn't filter chars
        // so a paste can sneak letters in, which would silently fail the
        // parse below.
        buffer_mut.retain(|c| c.is_ascii_digit());
    }

    // Re-parse after the input edit so the +/Craft buttons see the
    // freshly typed value this frame.
    let buffer_snapshot = buffer_mut.clone();
    let typed_qty_post: Option<u16> = buffer_snapshot
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|n| *n > 0);

    // --- Plus button. ---
    // Only enabled while the typed quantity is below the max-craftable
    // ceiling. We *don't* silently clamp; the player can still type a
    // higher number to see the shortfall in the inputs row, but the +
    // button itself stops working at the limit.
    let plus_can_increment = typed_qty_post
        .map(|q| q < max_batch && q < MAX_CRAFT_BATCH_SIZE)
        .unwrap_or(false);
    let plus_response = theme::compact_button_in_rect(
        ui,
        ("crafting_qty_plus", recipe.id),
        plus_rect,
        "+",
        theme::ButtonKind::Secondary,
    );
    if plus_response.clicked() && plus_can_increment {
        let next = typed_qty_post
            .unwrap_or(1)
            .saturating_add(1)
            .min(max_batch)
            .min(MAX_CRAFT_BATCH_SIZE);
        *crafting_ui
            .quantities
            .entry(recipe.id)
            .or_insert_with(|| "1".to_owned()) = next.to_string();
    }

    // --- Craft button + disabled tooltip. ---
    // Priority order: queue-full first (a global blocker), then per-
    // recipe checks. The "Missing" label is reserved for the case where
    // the player can't even craft *one*; the "exceeds max" case keeps
    // the "Craft" label and surfaces the explanation in a tooltip so
    // the player connects the disabled button to the typed number.
    let craft_disabled: Option<String> = if queue_full {
        Some("The crafting queue is full.".to_owned())
    } else if !craftable {
        Some(format!(
            "You don't have the materials to craft {}.",
            recipe.name
        ))
    } else if let Some(qty) = typed_qty_post {
        // Order matters: the per-recipe ceiling is usually lower than
        // the global cap, so we explain that case first to point the
        // player at the *real* blocker (missing materials, not protocol
        // limits).
        if qty > max_batch {
            Some(format!(
                "You can only craft {} of {} with what you've got.",
                max_batch, recipe.name
            ))
        } else if qty > MAX_CRAFT_BATCH_SIZE {
            Some(format!(
                "Batch is capped at {MAX_CRAFT_BATCH_SIZE} per craft."
            ))
        } else {
            None
        }
    } else {
        Some(format!(
            "Enter a quantity between 1 and {} (max you can craft right now).",
            max_batch
        ))
    };

    let (craft_label, craft_kind) = if !craftable {
        ("Missing", theme::ButtonKind::Secondary)
    } else if queue_full {
        ("Queue full", theme::ButtonKind::Secondary)
    } else if craft_disabled.is_some() {
        ("Craft", theme::ButtonKind::Secondary)
    } else {
        ("Craft", theme::ButtonKind::Primary)
    };
    let craft_response = theme::compact_button_in_rect(
        ui,
        ("crafting_craft_button", recipe.id),
        craft_rect,
        craft_label,
        craft_kind,
    );
    if let Some(ref reason) = craft_disabled {
        let _ = craft_response.clone().on_hover_text(reason);
    }
    if craft_response.clicked()
        && craft_disabled.is_none()
        && let Some(qty) = typed_qty_post
    {
        theme::record_click_sound(ui, &craft_response);
        send_crafting_command(
            runtime,
            error_toasts,
            CraftingCommand::Enqueue {
                recipe_id: recipe.id.to_owned(),
                quantity: qty,
            },
        );
    }

    ui.add_space(6.0);
}

/// Compute the largest batch quantity the player can currently afford
/// for a given recipe, capped at [`MAX_CRAFT_BATCH_SIZE`].
///
/// `0` means "can't even craft one" — the same condition the existing
/// `craftable` flag tracks, but expressed as a batch-aware ceiling so
/// the recipe row can also disable the `+` button at the actual limit.
fn max_craftable_batch(inventory: Option<&PlayerInventoryState>, recipe: &RecipeDefinition) -> u16 {
    let Some(inventory) = inventory else {
        return 0;
    };
    if recipe.inputs.is_empty() {
        // No-input recipes never gate on materials, so the only ceiling
        // is the protocol's per-message cap.
        return MAX_CRAFT_BATCH_SIZE;
    }
    let mut max = MAX_CRAFT_BATCH_SIZE as u32;
    for input in recipe.inputs {
        if input.quantity == 0 {
            continue;
        }
        let have = count_in_inventory(inventory, input.item_id) as u32;
        let possible = have / input.quantity as u32;
        max = max.min(possible);
    }
    max.min(MAX_CRAFT_BATCH_SIZE as u32) as u16
}

fn paint_recipe_icon(painter: &egui::Painter, rect: Rect, recipe: &RecipeDefinition) {
    let tint = item_definition(recipe.output_item)
        .map(|definition| definition.tint)
        .unwrap_or(ItemTint::new(146, 158, 171));
    let fill = Color32::from_rgb(tint.r, tint.g, tint.b);
    painter.rect_filled(rect, CornerRadius::same(4), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );
    // Single-letter glyph: first character of the output name, clamped to
    // ASCII so non-Latin names don't render as garbled squares.
    let glyph = output_display_name(recipe)
        .chars()
        .next()
        .unwrap_or('?')
        .to_ascii_uppercase();
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph.to_string(),
        FontId::new(18.0, FontFamily::Proportional),
        Color32::from_rgb(20, 22, 24),
    );
}

fn matches_search(recipe: &RecipeDefinition, needle: &str) -> bool {
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

fn has_all_inputs(inventory: &PlayerInventoryState, recipe: &RecipeDefinition) -> bool {
    recipe
        .inputs
        .iter()
        .all(|input| count_in_inventory(inventory, input.item_id) >= input.quantity)
}

fn count_in_inventory(inventory: &PlayerInventoryState, item_id: &str) -> u16 {
    let mut total: u32 = 0;
    for slot in inventory
        .inventory_slots
        .iter()
        .chain(inventory.actionbar_slots.iter())
    {
        if let Some(stack) = slot
            && stack.item_id.as_ref() == item_id
        {
            total = total.saturating_add(stack.quantity as u32);
        }
    }
    total.min(u16::MAX as u32) as u16
}

/// Build the per-recipe input line as a multi-color `Galley`. Each input
/// reads as `"×needed Name  (need N more)"` when the player is short,
/// or `"×needed Name  (have/needed)"` when they're not — the recipe cost
/// is always visible, but the shortfall is what the player actually
/// needs to act on, so it gets the red "(need N more)" callout. Built
/// via `LayoutJob` because `painter.text` only supports one color per
/// call.
///
/// `multiplier` scales the per-input quantities by the requested batch
/// size: a `multiplier = 3` on a recipe that needs 2 wood per craft
/// renders the input as `×6 Wood` so the player sees the *batch* cost
/// matching what the server will deduct.
fn build_inputs_galley(
    ctx: &egui::Context,
    recipe: &RecipeDefinition,
    inventory: Option<&PlayerInventoryState>,
    multiplier: u16,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let font = FontId::new(11.5, FontFamily::Proportional);
    let separator_format = TextFormat {
        font_id: font.clone(),
        color: theme::muted_text(),
        ..Default::default()
    };

    let multiplier = multiplier.max(1);
    let mut job = LayoutJob::default();
    for (index, input) in recipe.inputs.iter().enumerate() {
        if index > 0 {
            job.append("  ·  ", 0.0, separator_format.clone());
        }
        let name = item_definition(input.item_id)
            .map(|def| def.name)
            .unwrap_or(input.item_id);
        let have = inventory
            .map(|inv| count_in_inventory(inv, input.item_id))
            .unwrap_or(0);
        // Saturate at `u16::MAX` so an absurd batch size doesn't wrap
        // around and silently understate the cost the row claims.
        let needed = (input.quantity as u32)
            .saturating_mul(multiplier as u32)
            .min(u16::MAX as u32) as u16;
        let shortfall = needed.saturating_sub(have);

        // The base "×needed Name" chunk stays in the primary text colour
        // even when short — the player can always read the actual cost.
        let base_chunk = format!("×{needed} {name}");
        job.append(
            &base_chunk,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: theme::text(),
                ..Default::default()
            },
        );

        if shortfall > 0 {
            // Red shortfall: the actionable bit of information.
            job.append(
                &format!("  (need {shortfall} more)"),
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: INPUT_MISSING_COLOR,
                    ..Default::default()
                },
            );
        } else {
            // Quiet "have/need" so the player can still see their margin
            // without it competing with the cost.
            job.append(
                &format!("  ({have}/{needed})"),
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: theme::muted_text(),
                    ..Default::default()
                },
            );
        }
    }
    job.wrap = TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ctx.fonts_mut(|fonts| fonts.layout_job(job))
}

// =====================================================================
// Crafting queue HUD: top-right stack of progress cards.
// =====================================================================

const QUEUE_CARD_WIDTH: f32 = 280.0;
const QUEUE_CARD_HEIGHT: f32 = 56.0;
const QUEUE_CARD_GAP: f32 = 8.0;
const QUEUE_TOP_MARGIN: f32 = 24.0;
const QUEUE_RIGHT_MARGIN: f32 = 24.0;
const QUEUE_CANCEL_BUTTON_SIZE: f32 = 22.0;
/// Number of queue cards rendered (with a graduated fade) before the
/// HUD switches to a compact "+N more" overflow bar. Beyond this we
/// don't paint per-job cards at all — the always-open recipe browser
/// shows the full queue if the player wants the rest.
const QUEUE_VISIBLE_CARDS: usize = 3;
/// Height of the compact "+N more" overflow indicator drawn below the
/// last visible queue card. Shorter than a full card on purpose so it
/// reads as a secondary "there's more" hint, not another job entry.
const QUEUE_OVERFLOW_BAR_HEIGHT: f32 = 22.0;

pub(super) fn crafting_queue_hud(
    ctx: &egui::Context,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    hud_state: &mut CraftingHudState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    let Some(jobs) = local_player
        .private
        .as_ref()
        .map(|p| p.crafting.jobs.clone())
    else {
        // Clear any stale baselines so a future job_id collision (the
        // server's id allocator wraps eventually) can't inherit a wrong
        // observation timestamp.
        hud_state.progress.clear();
        return;
    };
    if jobs.is_empty() {
        hud_state.progress.clear();
        return;
    }

    let now_secs = ctx.input(|input| input.time);
    // Forget baselines for jobs that left the queue (completed or
    // cancelled). Saves us from accumulating entries forever as a
    // long-running player chains hundreds of crafts. We rebuild the
    // retained set from the current snapshot in a single pass.
    {
        let live: std::collections::HashSet<_> = jobs.iter().map(|job| job.job_id).collect();
        hud_state.progress.retain(|job_id, _| live.contains(job_id));
    }

    let screen_rect = ctx.content_rect();
    let card_x_right = screen_rect.right() - QUEUE_RIGHT_MARGIN;
    let card_x_left = card_x_right - QUEUE_CARD_WIDTH;

    let mut cancel_target: Option<crate::protocol::CraftingJobId> = None;
    // Egui repaints on input and animation. The bar interpolation is
    // continuous, so we need to ask for the next frame regardless of
    // whether anything else moved.
    ctx.request_repaint();

    let visible_count = jobs.len().min(QUEUE_VISIBLE_CARDS);
    let hidden_count = jobs.len().saturating_sub(QUEUE_VISIBLE_CARDS);
    for (index, job) in jobs.iter().enumerate() {
        let alpha = queue_card_alpha(index);
        if alpha <= 0.0 {
            // Past the visible window — don't draw an interactable card
            // the player can't see. The "+N more" overflow bar drawn
            // after this loop covers those.
            continue;
        }
        let is_head = index == 0;
        let fraction = smoothed_fraction(hud_state, job, now_secs, is_head);
        let y_top = screen_rect.top()
            + QUEUE_TOP_MARGIN
            + index as f32 * (QUEUE_CARD_HEIGHT + QUEUE_CARD_GAP);
        let rect = Rect::from_min_size(
            Pos2::new(card_x_left, y_top),
            Vec2::new(QUEUE_CARD_WIDTH, QUEUE_CARD_HEIGHT),
        );
        let area_response = egui::Area::new(Id::new(("crafting_queue_card", job.job_id)))
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                // egui Ui-level opacity multiplier — applies to every
                // paint + widget inside the area. One knob, everything
                // (background, stroke, text, progress bar, cancel
                // button) fades together.
                ui.multiply_opacity(alpha);
                draw_queue_card(ui, rect, job, is_head, fraction)
            });
        if area_response.inner.cancel_clicked {
            cancel_target = Some(job.job_id);
        }
    }

    // Compact "+N more" overflow indicator under the last visible card.
    // Anchored to the bottom of the last *rendered* card so a queue of
    // exactly QUEUE_VISIBLE_CARDS doesn't show one, while a queue with
    // any hidden jobs does. Sits at the same x extents as the cards so
    // the column reads as a single stack.
    if hidden_count > 0 && visible_count > 0 {
        let last_visible_bottom = screen_rect.top()
            + QUEUE_TOP_MARGIN
            + (visible_count - 1) as f32 * (QUEUE_CARD_HEIGHT + QUEUE_CARD_GAP)
            + QUEUE_CARD_HEIGHT;
        let bar_rect = Rect::from_min_size(
            Pos2::new(card_x_left, last_visible_bottom + QUEUE_CARD_GAP * 0.5),
            Vec2::new(QUEUE_CARD_WIDTH, QUEUE_OVERFLOW_BAR_HEIGHT),
        );
        egui::Area::new(Id::new("crafting_queue_overflow"))
            .order(Order::Foreground)
            .fixed_pos(bar_rect.min)
            .show(ctx, |ui| {
                draw_queue_overflow(ui, bar_rect, hidden_count);
            });
    }

    if let Some(job_id) = cancel_target {
        send_crafting_command(runtime, error_toasts, CraftingCommand::Cancel { job_id });
    }
}

/// Compute the progress fraction the card should render this frame.
///
/// For the head job: anchor a baseline the first time we see a given
/// `progress_ticks` value, then advance the fraction off the wall clock
/// at `SERVER_TICK_RATE_HZ` until the next snapshot rebases it. The
/// final clamp at 1.0 keeps a stale or slow-arriving "completed"
/// snapshot from painting past the bar's right edge.
///
/// Queued (non-head) jobs always render at 0 — the server doesn't
/// advance them, so neither should we.
fn smoothed_fraction(
    hud_state: &mut CraftingHudState,
    job: &CraftingJob,
    now_secs: f64,
    is_head: bool,
) -> f32 {
    if !is_head {
        // Keep a baseline so the moment a queued job becomes head, it
        // starts from where the server says it is rather than fading in
        // from whatever stale value the interpolator made up.
        hud_state
            .progress
            .insert(job.job_id, baseline_from(job, now_secs));
        return 0.0;
    }

    let entry = hud_state.progress.entry(job.job_id);
    let baseline = match entry {
        std::collections::hash_map::Entry::Occupied(mut slot) => {
            let current = slot.get_mut();
            if current.observed_ticks != job.progress_ticks
                || current.total_ticks != job.total_ticks
            {
                *current = baseline_from(job, now_secs);
            }
            *current
        }
        std::collections::hash_map::Entry::Vacant(slot) => {
            *slot.insert(baseline_from(job, now_secs))
        }
    };

    if baseline.total_ticks == 0 {
        return 1.0;
    }
    let elapsed_ticks =
        (now_secs - baseline.observed_at_secs).max(0.0) as f32 * SERVER_TICK_RATE_HZ;
    let projected = baseline.observed_ticks as f32 + elapsed_ticks;
    (projected / baseline.total_ticks as f32).clamp(0.0, 1.0)
}

/// Whether a queue card at `index` is rendered. Cards within the
/// visible window paint at full opacity; anything past the window is
/// represented by the compact "+N more" overflow bar instead. Kept as a
/// helper (rather than inline) so the fade rule has one place to live
/// if we ever want to bring graduated opacity back.
fn queue_card_alpha(index: usize) -> f32 {
    if index < QUEUE_VISIBLE_CARDS {
        1.0
    } else {
        0.0
    }
}

fn baseline_from(job: &CraftingJob, now_secs: f64) -> ProgressBaseline {
    ProgressBaseline {
        observed_ticks: job.progress_ticks,
        total_ticks: job.total_ticks,
        observed_at_secs: now_secs,
    }
}

struct QueueCardResponse {
    cancel_clicked: bool,
}

/// Slim "+N more" pill drawn under the last visible queue card when the
/// queue runs deeper than [`QUEUE_VISIBLE_CARDS`]. Non-interactive on
/// purpose — clicking it doesn't open or expand anything, since the
/// crafting modal already exposes the full queue count. The goal is a
/// silent visual hint, not another button.
fn draw_queue_overflow(ui: &mut egui::Ui, rect: Rect, hidden_count: usize) {
    let _ = ui.allocate_rect(rect, Sense::hover());
    let painter = ui.painter().clone();

    let corner = CornerRadius::same(4);
    // Same panel fill the cards use but at a reduced alpha — the bar is
    // a secondary cue, not the primary HUD element. egui's
    // `multiply_opacity` would dim text and stroke too; we want the
    // text legible so we modulate just the background here.
    let fill = theme::panel_fill().gamma_multiply(0.65);
    painter.rect_filled(rect, corner, fill);
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );

    let label = if hidden_count == 1 {
        "+1 more queued".to_owned()
    } else {
        format!("+{hidden_count} more queued")
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(11.5, FontFamily::Proportional),
        theme::muted_text(),
    );
}

fn draw_queue_card(
    ui: &mut egui::Ui,
    rect: Rect,
    job: &CraftingJob,
    is_head: bool,
    fraction: f32,
) -> QueueCardResponse {
    // Allocate the card rect so egui's layout doesn't paint a phantom
    // hover row underneath.
    let _ = ui.allocate_rect(rect, Sense::hover());
    let painter = ui.painter().clone();

    let corner = CornerRadius::same(5);
    painter.rect_filled(rect, corner, theme::panel_fill());
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );

    let recipe = crate::crafting::recipe_definition(&job.recipe_id);
    let recipe_name = recipe.map(|r| r.name).unwrap_or("Unknown recipe");
    // Batch jobs get a "×N" suffix so the queue HUD makes it obvious why
    // a single card has a long progress bar.
    let display_name = if job.quantity > 1 {
        format!("{recipe_name} ×{}", job.quantity)
    } else {
        recipe_name.to_owned()
    };
    let tint = recipe
        .and_then(|r| item_definition(r.output_item))
        .map(|definition| definition.tint)
        .unwrap_or(ItemTint::new(146, 158, 171));

    // Everything in the top row — dot, name, status, and cancel button
    // — pivots around this single vertical center. The dot is placed by
    // its center, but the text was previously anchored TOP, which drifts
    // a couple of pixels off the dot's optical center because the font
    // baseline isn't where the glyph extents start. `_CENTER` alignment
    // on the text keeps them on the same line.
    let row_center_y = rect.top() + 18.0;

    // Color dot mirroring the inventory tint of the output item — gives
    // the player a fast visual signal of what's brewing.
    let dot_radius = 6.0;
    painter.circle_filled(
        Pos2::new(rect.left() + 16.0, row_center_y),
        dot_radius,
        Color32::from_rgb(tint.r, tint.g, tint.b),
    );

    painter.text(
        Pos2::new(rect.left() + 32.0, row_center_y),
        Align2::LEFT_CENTER,
        &display_name,
        FontId::new(13.5, FontFamily::Proportional),
        theme::text(),
    );

    let status_text = if is_head {
        format!("Crafting… {:>3.0}%", fraction * 100.0)
    } else {
        "Queued".to_owned()
    };
    painter.text(
        Pos2::new(
            rect.right() - 12.0 - QUEUE_CANCEL_BUTTON_SIZE - 8.0,
            row_center_y,
        ),
        Align2::RIGHT_CENTER,
        status_text,
        FontId::new(11.5, FontFamily::Proportional),
        theme::muted_text(),
    );

    // Progress bar along the bottom.
    let bar_height = 6.0;
    let bar_left = rect.left() + 12.0;
    let bar_right = rect.right() - 12.0;
    let bar_top = rect.bottom() - 14.0;
    let bar_bg = Rect::from_min_max(
        Pos2::new(bar_left, bar_top),
        Pos2::new(bar_right, bar_top + bar_height),
    );
    painter.rect_filled(bar_bg, CornerRadius::same(3), theme::input_fill());
    // The caller passed the interpolated fraction for the head job and
    // 0.0 for queued jobs — the card itself doesn't need to know which
    // is which beyond the status-text label above.
    let _ = is_head;
    let fill_right = bar_left + (bar_right - bar_left) * fraction;
    if fill_right > bar_left {
        let bar_fill = Rect::from_min_max(
            Pos2::new(bar_left, bar_top),
            Pos2::new(fill_right, bar_top + bar_height),
        );
        painter.rect_filled(bar_fill, CornerRadius::same(3), theme::accent());
    }

    // Cancel × button — right-aligned, vertically centered on the same
    // row as the dot/name/status so the whole header sits on one line.
    let cancel_rect = Rect::from_center_size(
        Pos2::new(
            rect.right() - 12.0 - QUEUE_CANCEL_BUTTON_SIZE * 0.5,
            row_center_y,
        ),
        Vec2::splat(QUEUE_CANCEL_BUTTON_SIZE),
    );
    let cancel_response = ui.interact(
        cancel_rect,
        ui.id().with(("crafting_cancel", job.job_id)),
        Sense::click(),
    );
    let hovered = cancel_response.hovered();
    painter.rect_filled(
        cancel_rect,
        CornerRadius::same(4),
        if hovered {
            theme::button_hover_fill()
        } else {
            theme::button_fill()
        },
    );
    painter.rect_stroke(
        cancel_rect,
        CornerRadius::same(4),
        Stroke::new(1.0, theme::button_stroke()),
        StrokeKind::Inside,
    );
    painter.text(
        cancel_rect.center(),
        Align2::CENTER_CENTER,
        "×",
        FontId::new(15.0, FontFamily::Proportional),
        theme::text(),
    );
    theme::record_click_sound(ui, &cancel_response);

    QueueCardResponse {
        cancel_clicked: cancel_response.clicked(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crafting::{PLANT_TWINE_RECIPE_ID, recipe_definition},
        items::FIBER_ID,
        protocol::{ItemStack, PlayerInventoryState},
    };

    fn inventory_with(item: &str, qty: u16) -> PlayerInventoryState {
        let mut inv = PlayerInventoryState::empty();
        inv.inventory_slots[0] = Some(ItemStack::new(item, qty));
        inv
    }

    #[test]
    fn queue_card_alpha_shows_first_three_only() {
        // Top three render at full opacity, anything deeper is handled
        // by the "+N more" overflow bar drawn beneath them.
        assert!((queue_card_alpha(0) - 1.0).abs() < f32::EPSILON);
        assert!((queue_card_alpha(1) - 1.0).abs() < f32::EPSILON);
        assert!((queue_card_alpha(2) - 1.0).abs() < f32::EPSILON);
        assert!(queue_card_alpha(3) <= 0.0);
        assert!(queue_card_alpha(15) <= 0.0);
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
}
