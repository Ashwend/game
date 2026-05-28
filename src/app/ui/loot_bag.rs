//! Loot bag (death drop container) interaction modal.
//!
//! Opens when the replicated `PlayerPrivate.open_loot_bag` is set.
//! Renders two grids stacked vertically: the bag's contents on top
//! and the player's inventory on the bottom. Drag works exactly like
//! the main inventory and the furnace — same `draw_slot` widget,
//! same `UnifiedSlotRef` drag pipeline, same drop-on-ground guard.
//! Shift+click quick-transfers a stack between the two containers.
//!
//! The actionbar is intentionally not drawn inside the bag panel:
//! the on-screen hotbar at the bottom of the viewport already
//! shows it, and a player who wants to loot a kill straight onto
//! their hotbar drags down into those slots.
//!
//! The server keeps the bag alive until it's empty AND closed by
//! every looker (see `loot_bag::close_loot_bag`), so leaving the UI
//! is the signal that says "I'm done with this pile".

use bevy_egui::egui::{self, Align, Align2, Layout, Order, RichText};

use crate::{
    app::{
        state::{
            ClientRuntime, ErrorToastSink, InventoryUiState, LocalPlayerState, MenuState,
            UnifiedSlotRef,
        },
        systems::send_loot_bag_command,
    },
    protocol::{
        INVENTORY_SLOT_COUNT, ItemContainerSlot, LootBagCommand, LootBagSlotRef, OpenLootBagView,
        PlayerInventoryState,
    },
};

use super::{
    inventory::{draw_drag_preview, slot::draw_slot},
    modal::backdrop_layer,
    theme,
};

const PANEL_WIDTH: f32 = 720.0;
const SLOT_GAP: f32 = 6.0;
const BAG_COLS: usize = 7;
const INVENTORY_COLS: usize = 10;

pub(super) fn loot_bag_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    if menu.pause_open {
        return;
    }
    let view: OpenLootBagView = match local_player
        .private
        .as_ref()
        .and_then(|private| private.open_loot_bag.clone())
    {
        Some(v) => v,
        None => return,
    };
    let inventory = local_player.private.as_ref().map(|p| p.inventory.clone());

    // Scrim. Click outside the panel sends Close to the server.
    let backdrop = backdrop_layer(
        ctx,
        "loot_bag_backdrop",
        Order::Middle,
        theme::backdrop_color(),
    );
    if backdrop.clicked() {
        send_loot_bag_command(runtime, error_toasts, LootBagCommand::Close);
        return;
    }

    let mut close_requested = false;
    let response = egui::Area::new(egui::Id::new("loot_bag_panel"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(PANEL_WIDTH);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(PANEL_WIDTH - 48.0);
                draw_panel(
                    ui,
                    &view,
                    inventory.as_ref(),
                    inventory_ui,
                    &mut close_requested,
                );
            });
        });
    // Record the panel rect so a player-sourced drag released over the
    // bag doesn't fall through to the drop-on-ground path. Mirrors
    // `furnace_rect`.
    inventory_ui.loot_bag_rect = Some(response.response.rect);

    // Drag preview rides on the same tooltip layer as the main
    // inventory's preview. Drawing it after the panel lets it float
    // above the slots while the player drags.
    draw_drag_preview(ctx, inventory_ui);

    if close_requested {
        send_loot_bag_command(runtime, error_toasts, LootBagCommand::Close);
    }

    // Shift+click quick-transfer is resolved here — exactly the same
    // shape as the furnace UI. A non-bag slot Shift+click while the
    // bag is open quick-transfers into the bag; a bag slot
    // Shift+click empties out into the player's inventory.
    if let Some(source) = inventory_ui.pending_quick_transfer.take() {
        let from = match source {
            UnifiedSlotRef::Bag(_) | UnifiedSlotRef::Player(_) => source.as_loot_bag_ref(),
            // Furnace + bag can't both be open today, so this branch
            // exists only as a fallback. Default to a no-op bag
            // ref; the server rejects unknown slots.
            UnifiedSlotRef::Furnace(_) => LootBagSlotRef::Bag(0),
        };
        send_loot_bag_command(
            runtime,
            error_toasts,
            LootBagCommand::QuickTransfer { from },
        );
    }
}

fn draw_panel(
    ui: &mut egui::Ui,
    view: &OpenLootBagView,
    inventory: Option<&PlayerInventoryState>,
    inventory_ui: &mut InventoryUiState,
    close_requested: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label(theme::section("Loot bag"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close_response =
                theme::compact_button(ui, "Close", theme::ButtonKind::Secondary, 84.0);
            theme::record_click_sound(ui, &close_response);
            if close_response.clicked() {
                *close_requested = true;
            }
        });
    });
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Drag items between the bag and your inventory. Right-click to split a stack, \
             Shift+click to quick-transfer to the other container.",
        )
        .color(theme::muted_text())
        .small(),
    );
    ui.add_space(12.0);

    ui.label(theme::field_label("Bag contents"));
    let bag_rows = view.slots.len().div_ceil(BAG_COLS);
    for row in 0..bag_rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SLOT_GAP;
            for col in 0..BAG_COLS {
                let index = row * BAG_COLS + col;
                let stack = view.slots.get(index).and_then(|slot| slot.as_ref());
                draw_slot(
                    ui,
                    UnifiedSlotRef::Bag(index),
                    stack,
                    None,
                    false,
                    true,
                    // Shift-transfer is enabled because we have a
                    // counterpart container open (the player's
                    // inventory below).
                    true,
                    inventory_ui,
                );
            }
        });
        ui.add_space(SLOT_GAP);
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(12.0);

    if let Some(inventory) = inventory {
        ui.label(theme::field_label("Your inventory"));
        let mut idx = 0;
        while idx < INVENTORY_SLOT_COUNT {
            let row_end = (idx + INVENTORY_COLS).min(INVENTORY_SLOT_COUNT);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = SLOT_GAP;
                for slot_index in idx..row_end {
                    let stack = inventory
                        .inventory_slots
                        .get(slot_index)
                        .and_then(|s| s.as_ref());
                    draw_slot(
                        ui,
                        UnifiedSlotRef::Player(ItemContainerSlot::inventory(slot_index)),
                        stack,
                        None,
                        false,
                        true,
                        true,
                        inventory_ui,
                    );
                }
            });
            ui.add_space(SLOT_GAP);
            idx = row_end;
        }
    } else {
        ui.label(RichText::new("Inventory unavailable").color(theme::muted_text()));
    }
}
