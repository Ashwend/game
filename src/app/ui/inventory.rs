use bevy_egui::egui::{
    self, Align2, Color32, FontFamily, FontId, PointerButton, Rect, Sense, Stroke, Vec2, pos2, vec2,
};

use crate::{
    app::{
        state::{
            ClientRuntime, InventoryDrag, InventoryDragButton, InventoryUiState, MenuState,
            PickupTargetState,
        },
        systems::send_inventory_command,
    },
    items::{ItemDefinition, item_definition, stack_limit},
    protocol::{
        ACTIONBAR_SLOT_COUNT, InventoryCommand, ItemContainer, ItemContainerSlot, ItemStack,
        PlayerInventoryState,
    },
};

use super::theme;

const SLOT_SIZE: f32 = 56.0;
const SLOT_GAP: f32 = 6.0;
const INVENTORY_COLUMNS: usize = 10;
const INVENTORY_ROWS: usize = 4;
const INVENTORY_PANEL_WIDTH: f32 =
    INVENTORY_COLUMNS as f32 * SLOT_SIZE + (INVENTORY_COLUMNS - 1) as f32 * SLOT_GAP + 48.0;

pub(super) fn inventory_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    inventory_ui: &mut InventoryUiState,
    pickup_target: &PickupTargetState,
) {
    inventory_ui.begin_frame();
    if inventory_ui.was_open && !menu.inventory_open {
        ctx.memory_mut(|memory| memory.stop_text_input());
        inventory_ui.cancel_drag();
    }

    if menu.inventory_open && !menu.pause_open {
        inventory_backdrop(ctx);
        draw_inventory_panel(ctx, runtime, inventory_ui);
    }

    if !menu.pause_open {
        draw_actionbar(ctx, runtime, inventory_ui, menu.inventory_open);
    }

    pickup_tooltip(ctx, menu, pickup_target);
    handle_drag_release(ctx, menu, runtime, inventory_ui);
    draw_drag_preview(ctx, inventory_ui);
    inventory_ui.was_open = menu.inventory_open;
}

fn inventory_backdrop(ctx: &egui::Context) {
    let screen_rect = ctx.content_rect();
    egui::Area::new("inventory_backdrop".into())
        .order(egui::Order::Middle)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let local_rect = Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
            ui.allocate_rect(local_rect, Sense::click());
            ui.painter().rect_filled(
                local_rect,
                0.0,
                Color32::from_rgba_unmultiplied(1, 3, 7, 190),
            );
        });
}

fn draw_inventory_panel(
    ctx: &egui::Context,
    runtime: &ClientRuntime,
    inventory_ui: &mut InventoryUiState,
) {
    let response = egui::Area::new("inventory_panel".into())
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, -26.0])
        .show(ctx, |ui| {
            ui.set_width(INVENTORY_PANEL_WIDTH);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(INVENTORY_PANEL_WIDTH - 48.0);
                ui.label(theme::section("Inventory"));
                ui.add_space(14.0);
                draw_inventory_grid(ui, runtime, inventory_ui);
            });
        });
    inventory_ui.inventory_rect = Some(response.response.rect);
}

fn draw_inventory_grid(
    ui: &mut egui::Ui,
    runtime: &ClientRuntime,
    inventory_ui: &mut InventoryUiState,
) {
    let inventory = runtime.local_player().map(|player| &player.inventory);
    for row in 0..INVENTORY_ROWS {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SLOT_GAP;
            for column in 0..INVENTORY_COLUMNS {
                let index = row * INVENTORY_COLUMNS + column;
                let slot = ItemContainerSlot::inventory(index);
                let stack = inventory.and_then(|inventory| slot_stack(inventory, slot));
                draw_slot(ui, slot, stack, None, false, true, inventory_ui);
            }
        });
        if row + 1 < INVENTORY_ROWS {
            ui.add_space(SLOT_GAP);
        }
    }
}

fn draw_actionbar(
    ctx: &egui::Context,
    runtime: &ClientRuntime,
    inventory_ui: &mut InventoryUiState,
    inventory_open: bool,
) {
    let Some(inventory) = runtime.local_player().map(|player| &player.inventory) else {
        return;
    };

    let response = egui::Area::new("actionbar".into())
        .order(egui::Order::Foreground)
        .interactable(inventory_open)
        .anchor(Align2::CENTER_BOTTOM, [0.0, -18.0])
        .show(ctx, |ui| {
            actionbar_frame(inventory_open).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SLOT_GAP;
                    for index in 0..ACTIONBAR_SLOT_COUNT {
                        let slot = ItemContainerSlot::actionbar(index);
                        let stack = slot_stack(inventory, slot);
                        draw_slot(
                            ui,
                            slot,
                            stack,
                            Some((index + 1).to_string()),
                            index == inventory.active_actionbar_slot,
                            inventory_open,
                            inventory_ui,
                        );
                    }
                });
            });
        });
    inventory_ui.actionbar_rect = Some(response.response.rect);
}

fn actionbar_frame(inventory_open: bool) -> egui::Frame {
    let alpha = if inventory_open { 236 } else { 176 };
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(5, 8, 12, alpha))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(115, 132, 151, 86),
        ))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(9, 9))
}

fn draw_slot(
    ui: &mut egui::Ui,
    slot: ItemContainerSlot,
    stack: Option<&ItemStack>,
    label: Option<String>,
    active: bool,
    interactive: bool,
    inventory_ui: &mut InventoryUiState,
) {
    if !interactive {
        let (_, rect) = ui.allocate_space(Vec2::splat(SLOT_SIZE));
        paint_slot(ui, rect, stack, label.as_deref(), active, false, false);
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
    );

    if pointer_over_slot {
        inventory_ui.hovered_slot = Some(slot);
    }

    if let Some(stack) = stack {
        if inventory_ui.drag.is_none() {
            let _ = item_tooltip(response, stack);
        }
        if interactive && inventory_ui.drag.is_none() {
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
}

fn paint_slot(
    ui: &egui::Ui,
    rect: Rect,
    stack: Option<&ItemStack>,
    label: Option<&str>,
    active: bool,
    hovered: bool,
    is_drag_source: bool,
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
        .unwrap_or(stack.item_id.as_str());
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

fn handle_drag_release(
    ctx: &egui::Context,
    menu: &MenuState,
    runtime: &mut ClientRuntime,
    inventory_ui: &mut InventoryUiState,
) {
    if !menu.inventory_open {
        inventory_ui.cancel_drag();
        return;
    }

    let Some(drag) = inventory_ui.drag.clone() else {
        return;
    };
    let released = ctx.input(|input| match drag.button {
        InventoryDragButton::Primary => input.pointer.button_released(PointerButton::Primary),
        InventoryDragButton::Secondary => input.pointer.button_released(PointerButton::Secondary),
    });
    if !released {
        return;
    }

    if let Some(target) = inventory_ui.hovered_slot {
        if target != drag.source {
            send_inventory_command(
                runtime,
                InventoryCommand::Move {
                    from: drag.source,
                    to: target,
                    quantity: Some(drag.quantity),
                },
            );
        }
    } else if pointer_is_outside_inventory_surfaces(ctx, inventory_ui) {
        send_inventory_command(
            runtime,
            InventoryCommand::Drop {
                from: drag.source,
                quantity: Some(drag.quantity),
            },
        );
    }

    inventory_ui.cancel_drag();
}

fn pointer_is_outside_inventory_surfaces(
    ctx: &egui::Context,
    inventory_ui: &InventoryUiState,
) -> bool {
    let Some(pointer) = ctx.pointer_hover_pos() else {
        return true;
    };
    !inventory_ui
        .inventory_rect
        .is_some_and(|rect| rect.contains(pointer))
        && !inventory_ui
            .actionbar_rect
            .is_some_and(|rect| rect.contains(pointer))
}

fn draw_drag_preview(ctx: &egui::Context, inventory_ui: &InventoryUiState) {
    let Some(drag) = &inventory_ui.drag else {
        return;
    };
    let Some(pointer) = ctx.pointer_hover_pos() else {
        return;
    };

    egui::Area::new("inventory_drag_preview".into())
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(pointer - vec2(SLOT_SIZE * 0.5, SLOT_SIZE * 0.5))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(SLOT_SIZE), Sense::hover());
            let mut stack = drag.stack.clone();
            stack.quantity = drag.quantity;
            paint_slot(ui, rect, Some(&stack), None, false, false, false);
        });
}

fn pickup_tooltip(ctx: &egui::Context, menu: &MenuState, pickup_target: &PickupTargetState) {
    if menu.pause_open || menu.inventory_open || menu.chat_open {
        return;
    }

    let (Some(screen_position), Some(stack)) =
        (pickup_target.screen_position, pickup_target.stack.as_ref())
    else {
        return;
    };
    let title = item_definition(&stack.item_id)
        .map(|definition: &ItemDefinition| definition.name)
        .unwrap_or(stack.item_id.as_str());
    let body = if stack.quantity > 1 {
        format!("Press E to pick up\nQuantity: {}", stack.quantity)
    } else {
        "Press E to pick up".to_owned()
    };

    theme::anchored_wow_tooltip(
        ctx,
        "pickup_target_tooltip",
        pos2(screen_position.x, screen_position.y),
        title,
        &body,
    );
}

fn slot_stack(inventory: &PlayerInventoryState, slot: ItemContainerSlot) -> Option<&ItemStack> {
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
    use crate::items::TEST_ORE_ID;

    #[test]
    fn right_drag_takes_half_rounded_up() {
        assert_eq!(split_quantity(1), 1);
        assert_eq!(split_quantity(2), 1);
        assert_eq!(split_quantity(3), 2);
    }

    #[test]
    fn tooltip_body_uses_registry_stack_limits() {
        let body = item_tooltip_body(&ItemStack::new(TEST_ORE_ID, 3));
        assert!(body.contains("3/20"));
    }

    #[test]
    fn empty_inventory_slot_lookup_is_safe() {
        let inventory = PlayerInventoryState::empty();
        assert!(
            slot_stack(
                &inventory,
                ItemContainerSlot::inventory(crate::protocol::INVENTORY_SLOT_COUNT)
            )
            .is_none()
        );
    }
}
