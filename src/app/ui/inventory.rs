//! Inventory tab rendering for the unified panel: the bag grid and the
//! always-on hotbar (actionbar). The panel shell in
//! [`crate::app::ui::inventory_panel`] owns the window chrome, backdrop, tab
//! bar, and per-frame bookkeeping; this module only draws the slot grid, the
//! hotbar, and (re-exported) the drag pipeline and pickup tooltip.

pub(super) mod drag;
mod pickup;
pub(super) mod slot;

use bevy_egui::egui::{self, Align2, Color32, Stroke};

use crate::{
    app::state::{InventoryUiState, LocalPlayerState, UnifiedSlotRef},
    protocol::{ACTIONBAR_SLOT_COUNT, INVENTORY_SLOT_COUNT, ItemContainerSlot},
};

use self::slot::{SLOT_SIZE, draw_disabled_slot, draw_slot, slot_stack};

pub(super) use self::drag::{draw_drag_preview, handle_drag_release};
pub(in crate::app::ui) use self::pickup::pickup_tooltip;

const SLOT_GAP: f32 = 6.0;
/// Columns in the bag grid. Shared so the furnace's "Your inventory" mirror
/// lays the bag out with the exact same shape (and sizes its panel to fit).
pub(in crate::app::ui) const INVENTORY_COLUMNS: usize = 12;
/// Rows actually drawn in the grid. The first `INVENTORY_SLOT_COUNT` cells
/// (12x5 = 60) are real, usable slots; any cells past that are inert filler
/// tiles. We draw more rows than we have slots purely to give the shared panel
/// more vertical height (which the crafting tab's recipe list benefits from)
/// without leaving the inventory tab looking half-empty.
const INVENTORY_DISPLAY_ROWS: usize = 7;

/// Draw the bag grid with the standard tight gaps. The panel width is sized to
/// fit [`INVENTORY_COLUMNS`] exactly, so the rows fill the width edge-to-edge;
/// here we just center the grid vertically in whatever height the fixed-height
/// shell gave us. The caller records the resulting rect as the drag surface.
pub(super) fn draw_inventory_grid(
    ui: &mut egui::Ui,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
) {
    let inventory = local_player.private.as_ref().map(|p| &p.inventory);

    let rows = INVENTORY_DISPLAY_ROWS as f32;
    let grid_height = rows * SLOT_SIZE + (rows - 1.0) * SLOT_GAP;

    // Drive the vertical spacing ourselves (the theme's default item spacing
    // is larger) so the row-gap and centering math are exact.
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.add_space(((ui.available_height() - grid_height) / 2.0).max(0.0));

    for row in 0..INVENTORY_DISPLAY_ROWS {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SLOT_GAP;
            for column in 0..INVENTORY_COLUMNS {
                let index = row * INVENTORY_COLUMNS + column;
                if index < INVENTORY_SLOT_COUNT {
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
                } else {
                    // Inert filler past the real slot count: present for layout,
                    // never interactive.
                    draw_disabled_slot(ui);
                }
            }
        });
        if row + 1 < INVENTORY_DISPLAY_ROWS {
            ui.add_space(SLOT_GAP);
        }
    }
}

/// Draw the always-on hotbar. `inventory_open` here means "the panel is on
/// the Inventory tab", which is what makes the hotbar a live drag surface;
/// on the Crafting tab (or with the panel closed) it recedes into a dim,
/// non-interactive HUD strip.
pub(super) fn draw_actionbar(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::COAL_ID,
        protocol::{ItemStack, PlayerInventoryState},
        server::PlayerPrivate,
    };

    fn local_player(inventory: Option<PlayerInventoryState>) -> LocalPlayerState {
        LocalPlayerState {
            entity: None,
            public: None,
            private: inventory.map(|inventory| PlayerPrivate {
                inventory,
                crafting: Default::default(),
                open_furnace: None,
                open_loot_bag: None,
                last_processed_input: 0,
                applied_action_seq: 0,
            }),
            lifecycle: None,
        }
    }

    fn run_ui(f: impl FnMut(&egui::Context)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 768.0),
                )),
                ..Default::default()
            },
            f,
        )
    }

    #[test]
    fn actionbar_frame_dims_when_inventory_closed() {
        // Closed inventory uses a lower alpha than open so the hotbar
        // recedes into the HUD when it isn't the focus.
        let open = actionbar_frame(true);
        let closed = actionbar_frame(false);
        assert_ne!(open.fill, closed.fill);
    }

    #[test]
    fn actionbar_records_rect_when_inventory_present() {
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut inv_ui = InventoryUiState::default();
        run_ui(|ctx| draw_actionbar(ctx, &local, &mut inv_ui, true, false));
        assert!(inv_ui.actionbar_rect.is_some());
    }

    #[test]
    fn actionbar_absent_without_private_state() {
        // No private state → no inventory to draw, so the actionbar
        // early-returns and records no rect.
        let local = local_player(None);
        let mut inv_ui = InventoryUiState::default();
        run_ui(|ctx| draw_actionbar(ctx, &local, &mut inv_ui, true, false));
        assert!(inv_ui.actionbar_rect.is_none());
    }

    #[test]
    fn populated_grid_renders_more_than_empty() {
        // A populated grid paints item icons and count text on top of
        // the empty slot frames, so it produces strictly more shapes.
        let empty = local_player(Some(PlayerInventoryState::empty()));
        let mut full_inv = PlayerInventoryState::empty();
        full_inv.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 42));
        let full = local_player(Some(full_inv));

        let mut inv_ui_a = InventoryUiState::default();
        let empty_out = run_ui(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_inventory_grid(ui, &empty, &mut inv_ui_a);
            });
        });

        let mut inv_ui_b = InventoryUiState::default();
        let full_out = run_ui(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_inventory_grid(ui, &full, &mut inv_ui_b);
            });
        });

        assert!(full_out.shapes.len() > empty_out.shapes.len());
    }
}
