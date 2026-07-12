//! Item/recipe icon painting for the crafting browser: the shipped icon
//! texture when one is registered, else the tinted-square + letter-glyph
//! placeholder (headless/test contexts and icon-less items keep working).

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Stroke, StrokeKind, pos2,
};

use crate::{
    crafting::{RecipeDefinition, output_display_name},
    items::{ItemTint, item_definition},
};

use super::super::item_icons;
use super::theme;

/// Paint a recipe's output icon into `rect`.
pub(super) fn paint_recipe_icon(ui: &egui::Ui, rect: Rect, recipe: &RecipeDefinition) {
    paint_item_icon(ui, rect, recipe.output_item, output_display_name(recipe));
}

/// Paint an item's icon into `rect`: the real transparent PNG when it was
/// registered at startup, otherwise a square tinted from the item registry
/// with the first letter of `display_name` as a glyph.
pub(super) fn paint_item_icon(ui: &egui::Ui, rect: Rect, item_id: &str, display_name: &str) {
    if let Some(texture_id) = item_icons::texture_for(item_id) {
        ui.painter().image(
            texture_id,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        return;
    }

    let tint = item_definition(item_id)
        .map(|definition| definition.tint)
        .unwrap_or(ItemTint::new(146, 158, 171));
    let fill = Color32::from_rgb(tint.r, tint.g, tint.b);
    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );
    // Single-letter glyph: first character of the display name, clamped to
    // ASCII so non-Latin names don't render as garbled squares.
    let glyph = display_name
        .chars()
        .next()
        .unwrap_or('?')
        .to_ascii_uppercase();
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph.to_string(),
        FontId::new(rect.height() * 0.5, FontFamily::Proportional),
        Color32::from_rgb(20, 22, 24),
    );
}
