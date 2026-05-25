use bevy_egui::egui::{self, PointerButton, Sense, Vec2, vec2};

use crate::{
    app::{
        state::{
            ClientRuntime, ErrorToastSink, InventoryDragButton, InventoryUiState, MenuState,
            UnifiedSlotRef,
        },
        systems::{send_furnace_command, send_inventory_command},
    },
    protocol::{FurnaceCommand, InventoryCommand},
};

use super::slot::{SLOT_SIZE, paint_slot};

/// Resolve a drag release. Picks the right network message based on
/// the source + target kind:
///
/// - Player → Player: `InventoryCommand::Move` (the existing path).
/// - Anything involving a furnace slot: `FurnaceCommand::Move` —
///   `FurnaceSlotRef` already covers all four combinations
///   (player↔furnace, furnace↔furnace).
///
/// Drop-on-ground (release outside any inventory surface) only fires
/// for player-sourced drags. Furnace-sourced items that get released
/// in the void snap back to where they came from — better that than
/// silently dropping a stack of iron bars into the void.
pub(crate) fn handle_drag_release(
    ctx: &egui::Context,
    menu: &MenuState,
    runtime: &mut ClientRuntime,
    inventory_ui: &mut InventoryUiState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    // Drag is allowed while either the inventory or the furnace is up
    // — both surfaces draw slots and route through the same drag state.
    if !menu.inventory_open && !menu.furnace_open {
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
            send_move_command(
                runtime,
                error_toasts,
                drag.source,
                target,
                Some(drag.quantity),
            );
        }
    } else if drag.source.is_player()
        && pointer_is_outside_inventory_surfaces(ctx, inventory_ui)
        && let UnifiedSlotRef::Player(from) = drag.source
    {
        send_inventory_command(
            runtime,
            error_toasts,
            InventoryCommand::Drop {
                from,
                quantity: Some(drag.quantity),
            },
        );
    }

    inventory_ui.cancel_drag();
}

fn send_move_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    source: UnifiedSlotRef,
    target: UnifiedSlotRef,
    quantity: Option<u16>,
) {
    match (source, target) {
        (UnifiedSlotRef::Player(from), UnifiedSlotRef::Player(to)) => {
            send_inventory_command(
                runtime,
                error_toasts,
                InventoryCommand::Move { from, to, quantity },
            );
        }
        _ => {
            send_furnace_command(
                runtime,
                error_toasts,
                FurnaceCommand::Move {
                    from: source.as_furnace_ref(),
                    to: target.as_furnace_ref(),
                    quantity,
                },
            );
        }
    }
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
        && !inventory_ui
            .furnace_rect
            .is_some_and(|rect| rect.contains(pointer))
}

pub(crate) fn draw_drag_preview(ctx: &egui::Context, inventory_ui: &InventoryUiState) {
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
            paint_slot(ui, rect, Some(&stack), None, false, false, false, 0.0);
        });
}
