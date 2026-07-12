//! Left column of the crafting browser: the scrollable recipe list. Each row
//! is icon + name + a status dot; clicking a row selects it for the detail
//! card. All crafting controls (quantity, Craft) live in the detail card
//! ([`super::details`]), so rows stay quiet and scannable.

use bevy_egui::egui::{
    self, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Pos2, Rect, Sense, Stroke,
    StrokeKind, Vec2, pos2,
    text::{LayoutJob, TextFormat, TextWrapping},
    vec2,
};

use crate::app::state::CraftingUiState;

use super::icon::paint_recipe_icon;
use super::{RecipeListEntry, theme};

const ROW_HEIGHT: f32 = 44.0;
const ROW_GAP: f32 = 4.0;
const ICON_SIZE: f32 = 30.0;

/// Status-dot palette: green = craftable right now, amber = blocked only by a
/// station (reach a bench), dim = missing materials.
const DOT_CRAFTABLE: Color32 = Color32::from_rgb(126, 205, 132);
const DOT_STATION: Color32 = Color32::from_rgb(226, 180, 112);
const DOT_MISSING: Color32 = Color32::from_rgba_premultiplied(90, 100, 112, 160);

/// Draw the scrollable recipe list. `selected_index` is the effective
/// selection resolved by the caller (stored id when visible, else the top
/// entry); a row click writes the clicked id back into
/// [`CraftingUiState::selected_recipe`].
pub(super) fn draw_recipe_list(
    ui: &mut egui::Ui,
    entries: &[RecipeListEntry],
    selected_index: usize,
    max_height: f32,
    crafting_ui: &mut CraftingUiState,
) {
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

    let scroll_output = egui::ScrollArea::vertical()
        .id_salt(("crafting_recipes_scroll", scroll_id_salt))
        .max_height(max_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            for (index, entry) in entries.iter().enumerate() {
                draw_recipe_row(ui, entry, index == selected_index, crafting_ui);
            }
        });

    // Keep the wheel working when a tooltip is showing over the list. egui's
    // ScrollArea only consumes the mouse wheel when the pointer's top-most
    // interactable layer IS the scroll area's own layer (its
    // `is_hovering_outer_rect` guard fails otherwise). A hover tooltip is
    // rendered as an interactable `Order::Tooltip` area, so once it pops up
    // under the cursor it becomes that top layer and the list silently stops
    // reacting to the wheel (dragging the bar still works because that is a
    // direct press, not a hover-routed wheel). When a tooltip is stealing the
    // layer over the list, apply the wheel to the offset ourselves, mirroring
    // egui's own `offset -= smooth_scroll_delta` convention. This fires only
    // in the exact case egui skips, so the wheel is never applied twice.
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
    // outlines to it (a row scrolled out of view must not paint below the
    // panel).
    ui.ctx().memory_mut(|mem| {
        mem.data.insert_temp(
            crate::app::ui::tutorial::craft_viewport_key(),
            scroll_output.inner_rect,
        );
    });
}

fn draw_recipe_row(
    ui: &mut egui::Ui,
    entry: &RecipeListEntry,
    selected: bool,
    crafting_ui: &mut CraftingUiState,
) {
    let recipe = entry.recipe;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);

    let (fill, stroke) = if selected {
        (
            Color32::from_rgba_unmultiplied(21, 44, 72, 236),
            Stroke::new(1.0, theme::accent()),
        )
    } else if response.hovered() {
        (
            theme::button_hover_fill(),
            Stroke::new(1.0, theme::panel_stroke()),
        )
    } else {
        (theme::input_fill(), Stroke::new(1.0, theme::panel_stroke()))
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(4),
        fill,
        stroke,
        StrokeKind::Inside,
    );

    let icon_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 8.0, rect.center().y - ICON_SIZE * 0.5),
        Vec2::splat(ICON_SIZE),
    );
    paint_recipe_icon(ui, icon_rect, recipe);

    // Status dot on the right edge; the name elides before reaching it.
    let dot_center = pos2(rect.right() - 13.0, rect.center().y);
    let dot_color = if entry.craftable {
        DOT_CRAFTABLE
    } else if !entry.station_met {
        DOT_STATION
    } else {
        DOT_MISSING
    };
    ui.painter().circle_filled(dot_center, 3.5, dot_color);

    // Title, including the per-craft output multiplier ("Plant Twine ×4"),
    // dimmed when the recipe can't be crafted right now.
    let title = if recipe.output_quantity > 1 {
        format!("{} ×{}", recipe.name, recipe.output_quantity)
    } else {
        recipe.name.to_owned()
    };
    let title_color = if entry.craftable {
        theme::text()
    } else {
        theme::muted_text()
    };
    let text_left = icon_rect.right() + 10.0;
    let text_max_width = (dot_center.x - 12.0 - text_left).max(0.0);
    let mut job = LayoutJob::default();
    job.append(
        &title,
        0.0,
        TextFormat {
            font_id: FontId::new(13.5, FontFamily::Proportional),
            color: title_color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: text_max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let text_pos = Pos2::new(text_left, rect.center().y - galley.size().y * 0.5);
    ui.painter().galley(text_pos, galley, title_color);

    // Stash the row rect so the tutorial overlay can ring the recipe the
    // player is meant to click next. When this recipe is the one on the
    // detail card, `details` overwrites the key with the Craft button rect
    // (it draws after the list), walking the outline through the two-click
    // flow: pick the row, then press Craft.
    if crate::app::ui::tutorial::is_tutorial_recipe(recipe.id) {
        let key = crate::app::ui::tutorial::recipe_rect_key(recipe.id);
        ui.ctx().memory_mut(|mem| mem.data.insert_temp(key, rect));
    }

    if response.clicked() {
        theme::record_click_sound(ui, &response);
        crafting_ui.selected_recipe = Some(recipe.id);
    }

    // The dot legend rides on a hover tooltip only when the row is blocked,
    // so a craftable list stays clean.
    if !entry.craftable {
        let reason = if !entry.station_met {
            "Needs a crafting station in range."
        } else {
            "Missing materials."
        };
        response.on_hover_text(reason);
    }
}
