use bevy_egui::egui::{self, PointerButton, Sense, Vec2, vec2};

use crate::{
    app::{
        state::{ClientRuntime, InventoryDragButton, InventoryUiState, MenuState},
        systems::send_inventory_command,
    },
    protocol::InventoryCommand,
};

use super::slot::{SLOT_SIZE, paint_slot};

pub(super) fn handle_drag_release(
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

pub(super) fn draw_drag_preview(ctx: &egui::Context, inventory_ui: &InventoryUiState) {
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
