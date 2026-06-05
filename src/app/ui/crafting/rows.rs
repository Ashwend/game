//! Single recipe row rendering: icon, title/time/inputs text column, the
//! quantity stepper, and the Craft button (with disabled tooltip logic).

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, StrokeKind,
    Vec2,
    text::{LayoutJob, TextFormat, TextWrapping},
    vec2,
};

use crate::{
    app::{
        state::{ClientRuntime, CraftingUiState, ErrorToastSink},
        systems::send_crafting_command,
    },
    crafting::{RecipeDefinition, output_display_name},
    items::{ItemTint, item_definition},
    protocol::{CraftingCommand, MAX_CRAFT_BATCH_SIZE, PlayerInventoryState},
};

use super::recipes::{count_in_inventory, max_craftable_batch};
use super::theme;

const RECIPE_ROW_HEIGHT: f32 = 64.0;
/// Tint used for the input-line chunk when the player is short on that
/// material. Sourced from the warning toast palette so missing-material
/// reads consistently across the UI.
const INPUT_MISSING_COLOR: Color32 = Color32::from_rgb(228, 154, 154);

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_recipe_row(
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

    // The right-side controls cluster, minus, quantity input, plus, then
    // Craft. We need their total width to know where the text column
    // ends, so compute the cluster width first and lay everything out
    // from the right edge.
    let craft_button_width = 90.0;
    let qty_button_width = 28.0;
    let qty_input_width = 48.0;
    let max_button_width = 44.0;
    let inter_widget_gap = 4.0;
    let cluster_to_craft_gap = 8.0;
    let cluster_width = max_button_width
        + inter_widget_gap
        + qty_button_width
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
    // Stash the Craft button rect (not the whole row) for the tutorial overlay
    // to outline the starter-tool recipes, so the highlight rings just the
    // button the player is meant to click, without threading the tutorial step
    // through the crafting panel.
    if crate::app::ui::tutorial::is_tutorial_recipe(recipe.id) {
        let key = crate::app::ui::tutorial::recipe_rect_key(recipe.id);
        ui.ctx()
            .memory_mut(|mem| mem.data.insert_temp(key, craft_rect));
    }
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
    let max_rect = Rect::from_min_size(
        Pos2::new(
            minus_rect.left() - inter_widget_gap - max_button_width,
            cluster_top,
        ),
        Vec2::new(max_button_width, button_height),
    );

    // --- Max button: one click enqueues the largest batch the player can
    // currently afford. `max_craftable_batch` already clamps to
    // `MAX_CRAFT_BATCH_SIZE`, so this never exceeds the protocol cap. Disabled
    // (with a hover reason) when nothing can be crafted or the queue is full,
    // mirroring the Craft button's gating. ---
    let max_disabled: Option<String> = if queue_full {
        Some("The crafting queue is full.".to_owned())
    } else if max_batch == 0 {
        Some(format!(
            "You don't have the materials to craft {}.",
            recipe.name
        ))
    } else {
        None
    };
    let max_response = theme::compact_button_in_rect(
        ui,
        ("crafting_qty_max", recipe.id),
        max_rect,
        "Max",
        theme::ButtonKind::Secondary,
    );
    let max_response = match max_disabled {
        Some(ref reason) => max_response.on_hover_text(reason),
        None => {
            max_response.on_hover_text(format!("Craft {max_batch} now (the most you can make)"))
        }
    };
    if max_response.clicked() && max_disabled.is_none() {
        theme::record_click_sound(ui, &max_response);
        send_crafting_command(
            runtime,
            error_toasts,
            CraftingCommand::Enqueue {
                recipe_id: recipe.id.to_owned(),
                quantity: max_batch,
            },
        );
    }

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

/// Build the per-recipe input line as a multi-color `Galley`. Each input
/// reads as `"×needed Name  (need N more)"` when the player is short,
/// or `"×needed Name  (have/needed)"` when they're not, the recipe cost
/// is always visible, but the shortfall is what the player actually
/// needs to act on, so it gets the red "(need N more)" callout. Built
/// via `LayoutJob` because `painter.text` only supports one color per
/// call.
///
/// `multiplier` scales the per-input quantities by the requested batch
/// size: a `multiplier = 3` on a recipe that needs 2 wood per craft
/// renders the input as `×6 Wood` so the player sees the *batch* cost
/// matching what the server will deduct.
pub(super) fn build_inputs_galley(
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
        // even when short, the player can always read the actual cost.
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
