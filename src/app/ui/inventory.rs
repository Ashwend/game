pub(super) mod drag;
mod pickup;
pub(super) mod slot;

use bevy_egui::egui::{self, Align2, Color32, Stroke};

use crate::{
    app::{
        state::{InventoryUiState, LocalPlayerState, MenuState, PickupTargetState, UnifiedSlotRef},
        ui::InventorySoundRequests,
    },
    protocol::{ACTIONBAR_SLOT_COUNT, ItemContainerSlot},
};

use self::{
    pickup::pickup_tooltip,
    slot::{SLOT_SIZE, draw_slot, slot_stack},
};

pub(super) use self::drag::{draw_drag_preview, handle_drag_release};
use super::{modal::backdrop_layer, theme};

const SLOT_GAP: f32 = 6.0;
const INVENTORY_COLUMNS: usize = 10;
const INVENTORY_ROWS: usize = 4;
const INVENTORY_PANEL_WIDTH: f32 =
    INVENTORY_COLUMNS as f32 * SLOT_SIZE + (INVENTORY_COLUMNS - 1) as f32 * SLOT_GAP + 48.0;

#[allow(clippy::too_many_arguments)]
pub(super) fn inventory_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    pickup_target: &PickupTargetState,
    inventory_sound_requests: &mut InventorySoundRequests,
    delta_seconds: f32,
) {
    inventory_ui.begin_frame();
    inventory_ui.tick_slot_flashes(delta_seconds);
    match local_player.private.as_ref().map(|p| &p.inventory) {
        Some(inventory) => {
            if let Some(event) = inventory_ui.observe_inventory(inventory) {
                inventory_sound_requests.push(event);
            }
        }
        None => inventory_ui.clear_inventory_tracking(),
    }
    if inventory_ui.was_open && !menu.inventory_open {
        ctx.memory_mut(|memory| memory.stop_text_input());
        inventory_ui.cancel_drag();
    }

    if menu.inventory_open && !menu.pause_open {
        inventory_backdrop(ctx);
        draw_inventory_panel(ctx, local_player, inventory_ui);
    }

    if !menu.pause_open {
        // Shift+click quick-transfer is only meaningful when a destination
        // container is open. The bag and the furnace are mutually
        // exclusive (opening one closes the other), so the actionbar's
        // shift-click destination is "the furnace, if it's up." Closing
        // the furnace immediately disables the gesture again.
        draw_actionbar(
            ctx,
            local_player,
            inventory_ui,
            menu.inventory_open,
            menu.furnace_open,
        );
    }

    pickup_tooltip(ctx, menu, pickup_target);
    // Drag release + preview deliberately run later in the top-level
    // `ui_system` so they see slots/rects painted by the furnace modal
    // too. Doing it here would race with the furnace UI's
    // `hovered_slot` write and turn an inventory↔inventory drag,
    // while the furnace is open, into a "drop on the ground" because
    // no rect has been recorded yet this frame.
    inventory_ui.was_open = menu.inventory_open;
}

fn inventory_backdrop(ctx: &egui::Context) {
    let _ = backdrop_layer(
        ctx,
        "inventory_backdrop",
        egui::Order::Middle,
        theme::backdrop_color(),
    );
}

fn draw_inventory_panel(
    ctx: &egui::Context,
    local_player: &LocalPlayerState,
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
                draw_inventory_grid(ui, local_player, inventory_ui);
            });
        });
    inventory_ui.inventory_rect = Some(response.response.rect);
}

fn draw_inventory_grid(
    ui: &mut egui::Ui,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
) {
    let inventory = local_player.private.as_ref().map(|p| &p.inventory);
    for row in 0..INVENTORY_ROWS {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SLOT_GAP;
            for column in 0..INVENTORY_COLUMNS {
                let index = row * INVENTORY_COLUMNS + column;
                let slot = ItemContainerSlot::inventory(index);
                let stack = inventory.and_then(|inventory| slot_stack(inventory, slot));
                draw_slot(
                    ui,
                    UnifiedSlotRef::Player(slot),
                    stack,
                    None,
                    false,
                    true,
                    // Bag and furnace are mutually exclusive surfaces;
                    // shift+click out of the bag has no destination, so
                    // the gesture falls through to the normal drag.
                    false,
                    inventory_ui,
                );
            }
        });
        if row + 1 < INVENTORY_ROWS {
            ui.add_space(SLOT_GAP);
        }
    }
}

fn draw_actionbar(
    ctx: &egui::Context,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    inventory_open: bool,
    furnace_open: bool,
) {
    let Some(inventory) = local_player.private.as_ref().map(|p| &p.inventory) else {
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
                            UnifiedSlotRef::Player(slot),
                            stack,
                            Some((index + 1).to_string()),
                            index == inventory.active_actionbar_slot,
                            inventory_open,
                            furnace_open,
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
