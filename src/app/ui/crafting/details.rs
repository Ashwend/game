//! Right column of the crafting browser: the detail card for the selected
//! recipe. Owns everything the old per-row control cluster carried, the
//! description, per-ingredient have/need lines, the batch-quantity stepper,
//! and the Craft button with its disabled-reason tooltips. All gating here is
//! a courtesy mirror; the server re-validates every enqueue.

use bevy_egui::egui::{
    self, Align, Color32, FontFamily, FontId, Rect, RichText, Sense, Stroke, Vec2,
    text::{LayoutJob, TextFormat},
    vec2,
};

use crate::{
    app::{
        state::{ClientRuntime, CraftingUiState, ErrorToastSink},
        systems::send_crafting_command,
    },
    crafting::{CraftingInput, RecipeStation},
    items::item_definition,
    protocol::{CraftingCommand, MAX_CRAFT_BATCH_SIZE, PlayerInventoryState},
};

use super::icon::{paint_item_icon, paint_recipe_icon};
use super::recipes::{count_in_inventory, max_craftable_batch};
use super::{RecipeListEntry, theme};

/// Tint used for a shortfall count and for the station-requirement chunk when
/// the player is NOT near a satisfying station. Sourced from the warning
/// toast palette so "you can't craft this yet" reads consistently.
const BLOCKED_COLOR: Color32 = Color32::from_rgb(228, 154, 154);

const HEADER_ICON_SIZE: f32 = 48.0;
const INGREDIENT_ROW_HEIGHT: f32 = 26.0;
const INGREDIENT_ICON_SIZE: f32 = 20.0;

/// One ingredient line's numbers: what the recipe needs for the requested
/// batch vs. what the player is carrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IngredientStatus {
    pub(super) have: u16,
    pub(super) needed: u16,
}

impl IngredientStatus {
    pub(super) fn short(&self) -> bool {
        self.have < self.needed
    }
}

/// Compute an ingredient's have/need for a batch of `multiplier` crafts.
/// Saturates at `u16::MAX` so an absurd batch size doesn't wrap around and
/// silently understate the cost the card claims.
pub(super) fn ingredient_status(
    inventory: Option<&PlayerInventoryState>,
    input: &CraftingInput,
    multiplier: u16,
) -> IngredientStatus {
    let have = inventory
        .map(|inv| count_in_inventory(inv, input.item_id))
        .unwrap_or(0);
    let needed = (input.quantity as u32)
        .saturating_mul(multiplier.max(1) as u32)
        .min(u16::MAX as u32) as u16;
    IngredientStatus { have, needed }
}

#[expect(clippy::too_many_arguments, reason = "egui UI plumbing")]
pub(super) fn draw_recipe_details(
    ui: &mut egui::Ui,
    entry: &RecipeListEntry,
    inventory: Option<&PlayerInventoryState>,
    queue_full: bool,
    height: f32,
    crafting_ui: &mut CraftingUiState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    let recipe = entry.recipe;
    egui::Frame::NONE
        .fill(theme::input_fill())
        .stroke(Stroke::new(1.0, theme::panel_stroke()))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Fill the column height so the card and the list read as one
            // fixed shell. 30 = the frame's inner margins (2 × 14) plus its
            // stroke (egui counts 2 × stroke.width into the frame's total
            // size); anything less overflows the fixed-height panel.
            ui.set_min_height(height - 30.0);

            // --- Quantity state (lives in CraftingUiState so per-recipe
            // edits survive switching selection and don't leak between
            // recipes). ---
            let max_batch = max_craftable_batch(inventory, recipe);
            let buffer = crafting_ui
                .quantities
                .entry(recipe.id)
                .or_insert_with(|| "1".to_owned());
            // Strip non-digits before parsing - the input widget catches most
            // of them, but a pasted "12x" or leading whitespace would
            // otherwise make every frame's parse fail and silently disable
            // the Craft button. An empty buffer is left alone while the
            // player is mid-edit; it reads as "no valid quantity" below.
            buffer.retain(|c| c.is_ascii_digit());
            let typed_qty: Option<u16> = buffer.trim().parse::<u16>().ok().filter(|n| *n > 0);
            let display_qty = typed_qty.unwrap_or(1).min(MAX_CRAFT_BATCH_SIZE);

            // The header/description/ingredients draw into an EXACT-height,
            // clipped region so the card never resizes with its content: a
            // description that wraps to two lines (or a longer ingredient
            // list) eats into this region's slack instead of pushing the
            // controls down and growing the card. The controls block below
            // then always sits at the same y.
            let controls_block_reserve = 66.0;
            let content_height = (height - 30.0 - controls_block_reserve).max(120.0);
            let (content_rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), content_height), Sense::hover());
            let mut content_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            content_ui.set_clip_rect(content_rect.intersect(ui.clip_rect()));
            {
                let ui = &mut content_ui;
                draw_header(ui, entry, display_qty);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(recipe.description)
                        .size(12.0)
                        .color(theme::muted_text()),
                );
                ui.add_space(10.0);

                ui.label(theme::field_label("Requires"));
                ui.add_space(4.0);
                for input in recipe.inputs {
                    draw_ingredient_row(
                        ui,
                        input,
                        ingredient_status(inventory, input, display_qty),
                    );
                }
                if recipe.inputs.is_empty() {
                    ui.label(
                        RichText::new("No materials needed.")
                            .size(12.0)
                            .color(theme::muted_text()),
                    );
                }
            }

            let total_seconds = recipe.craft_seconds * display_qty as f32;
            let time_line = if display_qty > 1 {
                format!(
                    "Time: {:.0}s each · {total_seconds:.0}s total",
                    recipe.craft_seconds
                )
            } else {
                format!("Time: {total_seconds:.0}s")
            };
            ui.label(
                RichText::new(time_line)
                    .size(11.5)
                    .color(theme::muted_text()),
            );
            ui.add_space(6.0);

            draw_controls_row(
                ui,
                entry,
                queue_full,
                max_batch,
                display_qty,
                crafting_ui,
                runtime,
                error_toasts,
            );
        });
}

/// Big icon + name row, with the category/time/station meta line underneath.
fn draw_header(ui: &mut egui::Ui, entry: &RecipeListEntry, display_qty: u16) {
    let recipe = entry.recipe;
    ui.horizontal(|ui| {
        let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(HEADER_ICON_SIZE), Sense::hover());
        paint_recipe_icon(ui, icon_rect, recipe);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let title = if recipe.output_quantity > 1 {
                format!("{} ×{}", recipe.name, recipe.output_quantity)
            } else {
                recipe.name.to_owned()
            };
            ui.label(
                RichText::new(title)
                    .size(16.0)
                    .strong()
                    .color(theme::text()),
            );
            let category = recipe.category.label();
            let time_text = if display_qty > 1 {
                format!("{category} • {:.0}s × {display_qty}", recipe.craft_seconds)
            } else {
                format!("{category} • {:.0}s", recipe.craft_seconds)
            };
            ui.label(meta_layout_job(
                &time_text,
                recipe.station,
                entry.station_met,
            ));
        });
    });
}

/// One ingredient line: small icon, name, right-aligned `have / needed`
/// (warning-red when short, so the shortfall is what the eye lands on).
fn draw_ingredient_row(ui: &mut egui::Ui, input: &CraftingInput, status: IngredientStatus) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), INGREDIENT_ROW_HEIGHT),
        Sense::hover(),
    );
    let icon_rect = Rect::from_min_size(
        egui::pos2(
            rect.left() + 2.0,
            rect.center().y - INGREDIENT_ICON_SIZE * 0.5,
        ),
        Vec2::splat(INGREDIENT_ICON_SIZE),
    );
    let name = item_definition(input.item_id)
        .map(|def| def.name)
        .unwrap_or(input.item_id);
    paint_item_icon(ui, icon_rect, input.item_id, name);

    ui.painter().text(
        egui::pos2(icon_rect.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        FontId::new(12.5, FontFamily::Proportional),
        theme::text(),
    );

    let count_color = if status.short() {
        BLOCKED_COLOR
    } else {
        theme::muted_text()
    };
    ui.painter().text(
        egui::pos2(rect.right() - 2.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{} / {}", status.have, status.needed),
        FontId::new(12.5, FontFamily::Proportional),
        count_color,
    );
}

/// The quantity stepper (− / input / + / Max) and the Craft button.
#[expect(clippy::too_many_arguments, reason = "egui UI plumbing")]
fn draw_controls_row(
    ui: &mut egui::Ui,
    entry: &RecipeListEntry,
    queue_full: bool,
    max_batch: u16,
    display_qty: u16,
    crafting_ui: &mut CraftingUiState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    let recipe = entry.recipe;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        // --- Minus. ---
        let minus = theme::compact_button(ui, "−", theme::ButtonKind::Secondary, 30.0);
        if minus.clicked() && display_qty > 1 {
            let next = display_qty.saturating_sub(1).max(1);
            *crafting_ui
                .quantities
                .entry(recipe.id)
                .or_insert_with(|| "1".to_owned()) = next.to_string();
        }

        // --- Quantity input field. ---
        let buffer_mut = crafting_ui
            .quantities
            .entry(recipe.id)
            .or_insert_with(|| "1".to_owned());
        let input_response = ui.add_sized(
            [48.0, theme::COMPACT_ROW_HEIGHT],
            theme::text_input(buffer_mut)
                .id(egui::Id::new(("crafting_qty_input", recipe.id)))
                .horizontal_align(Align::Center),
        );
        if input_response.changed() {
            // Filter again post-edit. The widget itself doesn't filter chars
            // so a paste can sneak letters in, which would silently fail the
            // parse below.
            buffer_mut.retain(|c| c.is_ascii_digit());
        }
        // Re-parse after the input edit so the +/Max/Craft buttons see the
        // freshly typed value this frame.
        let typed_qty: Option<u16> = crafting_ui
            .quantities
            .get(recipe.id)
            .and_then(|buffer| buffer.trim().parse::<u16>().ok())
            .filter(|n| *n > 0);

        // --- Plus: enabled only below the max-craftable ceiling. We *don't*
        // silently clamp; the player can still type a higher number to see
        // the shortfall in the ingredient lines, but the + button itself
        // stops working at the limit. ---
        let plus = theme::compact_button(ui, "+", theme::ButtonKind::Secondary, 30.0);
        let plus_can_increment = typed_qty
            .map(|q| q < max_batch && q < MAX_CRAFT_BATCH_SIZE)
            .unwrap_or(false);
        if plus.clicked() && plus_can_increment {
            let next = typed_qty
                .unwrap_or(1)
                .saturating_add(1)
                .min(max_batch)
                .min(MAX_CRAFT_BATCH_SIZE);
            *crafting_ui
                .quantities
                .entry(recipe.id)
                .or_insert_with(|| "1".to_owned()) = next.to_string();
        }

        // --- Max: fill the quantity with the largest affordable batch (the
        // old Max button enqueued instantly; filling the field instead lets
        // the player read the batch cost before committing). ---
        let max_response = theme::compact_button(ui, "Max", theme::ButtonKind::Secondary, 48.0);
        let max_response = if max_batch == 0 {
            max_response.on_hover_text(format!(
                "You don't have the materials to craft {}.",
                recipe.name
            ))
        } else {
            max_response.on_hover_text(format!(
                "Set the batch to {max_batch}, the most you can make"
            ))
        };
        if max_response.clicked() && max_batch > 0 {
            *crafting_ui
                .quantities
                .entry(recipe.id)
                .or_insert_with(|| "1".to_owned()) = max_batch.to_string();
        }

        // --- Craft button + disabled tooltip. Priority order: queue-full
        // first (a global blocker), then the station gate, then per-recipe
        // material checks. The station reason is called out before materials
        // so an unmet workbench gate reads as "reach a bench", not "gather
        // more" (the server rejects it either way; this makes the gate
        // legible). ---
        let typed_for_craft = typed_qty;
        let craft_disabled: Option<String> = if queue_full {
            Some("The crafting queue is full.".to_owned())
        } else if !entry.station_met {
            station_requirement(recipe.station)
                .map(|label| format!("{label} to craft {}.", recipe.name))
        } else if !entry.craftable {
            Some(format!(
                "You don't have the materials to craft {}.",
                recipe.name
            ))
        } else if let Some(qty) = typed_for_craft {
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
                "Enter a quantity between 1 and {max_batch} (max you can craft right now)."
            ))
        };

        let (craft_label, craft_kind) = if queue_full {
            ("Queue full".to_owned(), theme::ButtonKind::Secondary)
        } else if !entry.station_met {
            ("No bench".to_owned(), theme::ButtonKind::Secondary)
        } else if !entry.craftable {
            ("Missing materials".to_owned(), theme::ButtonKind::Secondary)
        } else if craft_disabled.is_some() {
            ("Craft".to_owned(), theme::ButtonKind::Secondary)
        } else if display_qty > 1 {
            (format!("Craft ×{display_qty}"), theme::ButtonKind::Primary)
        } else {
            ("Craft".to_owned(), theme::ButtonKind::Primary)
        };
        ui.add_space(4.0);
        let craft_width = ui.available_width().max(90.0);
        let craft_response = theme::compact_button(ui, &craft_label, craft_kind, craft_width);
        // Stash the Craft button rect for the tutorial overlay: when the
        // focused recipe is on the card, the pulsing ring moves from its list
        // row to this button (this draws after the list, so it wins the key).
        if crate::app::ui::tutorial::is_tutorial_recipe(recipe.id) {
            let key = crate::app::ui::tutorial::recipe_rect_key(recipe.id);
            ui.ctx()
                .memory_mut(|mem| mem.data.insert_temp(key, craft_response.rect));
        }
        if let Some(ref reason) = craft_disabled {
            let _ = craft_response.clone().on_hover_text(reason);
        }
        if craft_response.clicked()
            && craft_disabled.is_none()
            && let Some(qty) = typed_for_craft
        {
            send_crafting_command(
                runtime,
                error_toasts,
                CraftingCommand::Enqueue {
                    recipe_id: recipe.id.to_owned(),
                    quantity: qty,
                },
            );
        }
    });
}

/// Subdued label for a recipe's station requirement once it is satisfied
/// (the player is near a bench that qualifies). `None` for hand recipes,
/// which have no station line at all.
pub(super) fn station_met_label(station: RecipeStation) -> Option<String> {
    match station {
        RecipeStation::None => None,
        RecipeStation::Workbench { min_tier } => Some(format!("Workbench Tier {min_tier}")),
    }
}

/// Red "Requires ..." label for a recipe's station requirement when it is
/// not satisfied, used both on the card's meta line and in the disabled
/// Craft button's tooltip. `None` for hand recipes.
pub(super) fn station_requirement(station: RecipeStation) -> Option<String> {
    match station {
        RecipeStation::None => None,
        RecipeStation::Workbench { min_tier } => {
            Some(format!("Requires Workbench Tier {min_tier}"))
        }
    }
}

/// Build the category/time meta line, optionally appending the recipe's
/// station requirement as a trailing chunk in its own colour: subdued
/// [`theme::muted_text`] when a satisfying station is in range, red
/// [`BLOCKED_COLOR`] when it is not. Hand recipes append nothing.
fn meta_layout_job(time_text: &str, station: RecipeStation, station_met: bool) -> LayoutJob {
    let font = FontId::new(11.5, FontFamily::Proportional);
    let mut job = LayoutJob::default();
    job.append(
        time_text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: theme::muted_text(),
            ..Default::default()
        },
    );
    // Only workbench recipes carry a requirement chunk; hand recipes get
    // just the time text.
    let (label, color) = if station_met {
        (station_met_label(station), theme::muted_text())
    } else {
        (station_requirement(station), BLOCKED_COLOR)
    };
    if let Some(label) = label {
        job.append(
            &format!("  •  {label}"),
            0.0,
            TextFormat {
                font_id: font,
                color,
                ..Default::default()
            },
        );
    }
    job
}
