use bevy_egui::egui::{
    self, Align2, Color32, FontFamily, FontId, PointerButton, Rect, Sense, Stroke, Vec2, pos2, vec2,
};

use crate::{
    app::state::{InventoryDrag, InventoryDragButton, InventoryUiState},
    items::{item_definition, stack_limit},
    protocol::{ItemContainer, ItemContainerSlot, ItemStack, PlayerInventoryState},
};

use super::super::theme;

pub(super) const SLOT_SIZE: f32 = 56.0;

pub(super) fn draw_slot(
    ui: &mut egui::Ui,
    slot: ItemContainerSlot,
    stack: Option<&ItemStack>,
    label: Option<String>,
    active: bool,
    interactive: bool,
    inventory_ui: &mut InventoryUiState,
) {
    let flash_strength = inventory_ui.slot_flash_strength(slot);

    if !interactive {
        let (_, rect) = ui.allocate_space(Vec2::splat(SLOT_SIZE));
        paint_slot(
            ui,
            rect,
            stack,
            label.as_deref(),
            active,
            false,
            false,
            flash_strength,
        );
        return;
    }

    let sense = Sense::click_and_drag();
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SLOT_SIZE), sense);
    let is_drag_source = inventory_ui
        .drag
        .as_ref()
        .is_some_and(|drag| drag.source == slot);
    let pointer_over_slot = ui
        .ctx()
        .pointer_hover_pos()
        .is_some_and(|position| rect.contains(position));
    let hovered = response.hovered() || (inventory_ui.drag.is_some() && pointer_over_slot);

    paint_slot(
        ui,
        rect,
        stack,
        label.as_deref(),
        active,
        hovered,
        is_drag_source,
        flash_strength,
    );

    if pointer_over_slot {
        inventory_ui.hovered_slot = Some(slot);
    }

    if let Some(stack) = stack
        && inventory_ui.drag.is_none()
    {
        let _ = item_tooltip(response, stack);
        if pointer_over_slot
            && ui.input(|input| input.pointer.button_pressed(PointerButton::Primary))
        {
            begin_drag(
                inventory_ui,
                slot,
                stack,
                stack.quantity,
                InventoryDragButton::Primary,
            );
        } else if pointer_over_slot
            && ui.input(|input| input.pointer.button_pressed(PointerButton::Secondary))
        {
            begin_drag(
                inventory_ui,
                slot,
                stack,
                split_quantity(stack.quantity),
                InventoryDragButton::Secondary,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_slot(
    ui: &egui::Ui,
    rect: Rect,
    stack: Option<&ItemStack>,
    label: Option<&str>,
    active: bool,
    hovered: bool,
    is_drag_source: bool,
    flash_strength: f32,
) {
    let fill = if active {
        Color32::from_rgba_unmultiplied(21, 44, 72, 236)
    } else if hovered {
        Color32::from_rgba_unmultiplied(34, 43, 54, 238)
    } else {
        Color32::from_rgba_unmultiplied(8, 12, 18, 232)
    };
    let stroke = if active {
        Stroke::new(2.0, Color32::from_rgb(96, 168, 255))
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(115, 132, 151, 92))
    };

    ui.painter()
        .rect(rect, 5, fill, stroke, egui::StrokeKind::Inside);

    if let Some(stack) = stack {
        paint_item_icon(ui, rect, stack, is_drag_source);
    }

    if let Some(label) = label {
        ui.painter().text(
            rect.left_top() + vec2(6.0, 5.0),
            Align2::LEFT_TOP,
            label,
            FontId::new(11.0, FontFamily::Monospace),
            Color32::from_rgb(195, 207, 220),
        );
    }

    if flash_strength > 0.0 {
        paint_slot_flash(ui, rect, flash_strength);
    }
}

/// Overlay drawn on top of a slot when its contents grew. A warm fill plus
/// a brighter stroke pulse together: the fill makes the slot "glow" briefly,
/// the stroke makes the rectangle pop out from neighboring slots.
fn paint_slot_flash(ui: &egui::Ui, rect: Rect, strength: f32) {
    let strength = strength.clamp(0.0, 1.0);
    let fill_alpha = (140.0 * strength) as u8;
    let stroke_alpha = (210.0 * strength) as u8;
    if fill_alpha == 0 && stroke_alpha == 0 {
        return;
    }
    let fill = Color32::from_rgba_unmultiplied(255, 214, 138, fill_alpha);
    let stroke = Stroke::new(
        2.0,
        Color32::from_rgba_unmultiplied(255, 232, 180, stroke_alpha),
    );
    ui.painter()
        .rect(rect, 5, fill, stroke, egui::StrokeKind::Inside);
}

fn paint_item_icon(ui: &egui::Ui, rect: Rect, stack: &ItemStack, is_drag_source: bool) {
    let definition = item_definition(&stack.item_id);
    let tint = definition
        .map(|definition| {
            Color32::from_rgb(definition.tint.r, definition.tint.g, definition.tint.b)
        })
        .unwrap_or(Color32::from_rgb(140, 150, 162));
    let alpha = if is_drag_source { 96 } else { 224 };
    let icon_rect = rect.shrink(9.0);
    let icon_fill = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), alpha);

    ui.painter().rect(
        icon_rect,
        6,
        icon_fill,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 34)),
        egui::StrokeKind::Inside,
    );
    ui.painter().circle_filled(
        pos2(icon_rect.center().x - 4.0, icon_rect.center().y - 3.0),
        7.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, alpha / 3),
    );

    if stack.quantity > 1 {
        ui.painter().text(
            rect.right_bottom() - vec2(6.0, 5.0),
            Align2::RIGHT_BOTTOM,
            stack.quantity.to_string(),
            FontId::new(13.0, FontFamily::Monospace),
            Color32::WHITE,
        );
    }
}

fn item_tooltip(response: egui::Response, stack: &ItemStack) -> egui::Response {
    let title = item_definition(&stack.item_id)
        .map(|definition| definition.name)
        .unwrap_or(stack.item_id.as_ref());
    let body = item_tooltip_body(stack);
    theme::wow_tooltip(response, title, &body)
}

fn item_tooltip_body(stack: &ItemStack) -> String {
    let Some(definition) = item_definition(&stack.item_id) else {
        return format!("Unknown item\nQuantity: {}", stack.quantity);
    };
    let stack_line = if definition.equipable {
        "Equipable\nStack: 1".to_owned()
    } else {
        format!(
            "Stack: {}/{}",
            stack.quantity,
            stack_limit(definition.id).unwrap_or(1)
        )
    };
    format!("{}\n{}", definition.description, stack_line)
}

fn begin_drag(
    inventory_ui: &mut InventoryUiState,
    source: ItemContainerSlot,
    stack: &ItemStack,
    quantity: u16,
    button: InventoryDragButton,
) {
    inventory_ui.drag = Some(InventoryDrag {
        source,
        stack: stack.clone(),
        quantity,
        button,
    });
}

pub(super) fn slot_stack(
    inventory: &PlayerInventoryState,
    slot: ItemContainerSlot,
) -> Option<&ItemStack> {
    match slot.container {
        ItemContainer::Inventory => inventory.inventory_slots.get(slot.slot),
        ItemContainer::Actionbar => inventory.actionbar_slots.get(slot.slot),
    }
    .and_then(Option::as_ref)
}

fn split_quantity(quantity: u16) -> u16 {
    quantity.div_ceil(2).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{items::COAL_ID, protocol::INVENTORY_SLOT_COUNT};

    #[test]
    fn right_drag_takes_half_rounded_up() {
        assert_eq!(split_quantity(1), 1);
        assert_eq!(split_quantity(2), 1);
        assert_eq!(split_quantity(3), 2);
    }

    #[test]
    fn tooltip_body_uses_registry_stack_limits() {
        let body = item_tooltip_body(&ItemStack::new(COAL_ID, 3));
        assert!(body.contains("3/200"));
    }

    #[test]
    fn empty_inventory_slot_lookup_is_safe() {
        let inventory = PlayerInventoryState::empty();
        assert!(
            slot_stack(
                &inventory,
                ItemContainerSlot::inventory(INVENTORY_SLOT_COUNT)
            )
            .is_none()
        );
    }
}
