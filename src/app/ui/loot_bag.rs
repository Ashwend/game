//! Loot bag (death drop container) interaction modal.
//!
//! Opens when the replicated `PlayerPrivate.open_loot_bag` is set.
//! Renders two grids stacked vertically: the bag's contents on top
//! and the player's inventory on the bottom. Drag works exactly like
//! the main inventory and the furnace, same `draw_slot` widget,
//! same `UnifiedSlotRef` drag pipeline, same drop-on-ground guard.
//! Shift+click quick-transfers a stack between the two containers.
//!
//! The actionbar is intentionally not drawn inside the bag panel:
//! the on-screen hotbar at the bottom of the viewport already
//! shows it, and a player who wants to loot a kill straight onto
//! their hotbar drags down into those slots.
//!
//! The server keeps the bag alive until it's empty AND closed by
//! every looker (see `loot_bag::close_container`), so leaving the UI
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
    inventory::{INVENTORY_COLUMNS, draw_drag_preview, slot::SLOT_SIZE, slot::draw_slot},
    modal::backdrop_layer,
    theme,
};

const SLOT_GAP: f32 = 6.0;
/// Both grids (bag + your inventory) use the same column count as the main
/// inventory and furnace so all three line up edge to edge. Twelve wide keeps
/// the bag short instead of stacking it into a tall 7-wide column.
const GRID_COLS: usize = INVENTORY_COLUMNS;
/// Panel width sized to fit `GRID_COLS` exactly with the standard gaps, matching
/// `furnace.rs` and the inventory panel (`12*56 + 11*6 + 48 = 786`).
const PANEL_WIDTH: f32 = GRID_COLS as f32 * SLOT_SIZE + (GRID_COLS - 1) as f32 * SLOT_GAP + 48.0;

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

    // Shift+click quick-transfer is resolved here, exactly the same
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
    let mut idx = 0;
    while idx < view.slots.len() {
        let row_end = (idx + GRID_COLS).min(view.slots.len());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SLOT_GAP;
            for index in idx..row_end {
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
        idx = row_end;
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(12.0);

    if let Some(inventory) = inventory {
        ui.label(theme::field_label("Your inventory"));
        let mut idx = 0;
        while idx < INVENTORY_SLOT_COUNT {
            let row_end = (idx + GRID_COLS).min(INVENTORY_SLOT_COUNT);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{items::COAL_ID, protocol::ItemStack, server::PlayerPrivate};

    fn bag_view(filled: bool) -> OpenLootBagView {
        let mut slots: Vec<Option<ItemStack>> = vec![None; 14];
        if filled {
            slots[0] = Some(ItemStack::new(COAL_ID, 8));
            slots[3] = Some(ItemStack::new(COAL_ID, 2));
        }
        OpenLootBagView { id: 7, slots }
    }

    fn local_player(open_loot_bag: Option<OpenLootBagView>) -> LocalPlayerState {
        LocalPlayerState {
            entity: None,
            public: None,
            private: Some(PlayerPrivate {
                inventory: PlayerInventoryState::empty(),
                crafting: Default::default(),
                open_furnace: None,
                open_loot_bag,
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
    fn loot_bag_ui_noop_without_open_bag() {
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(None);
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            loot_bag_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        assert!(output.shapes.is_empty());
        assert!(inv_ui.loot_bag_rect.is_none());
    }

    #[test]
    fn loot_bag_ui_suppressed_while_paused() {
        let mut menu = MenuState {
            pause_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(bag_view(true)));
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            loot_bag_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn loot_bag_ui_renders_filled_bag_and_records_rect() {
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(bag_view(true)));
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            loot_bag_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        assert!(!output.shapes.is_empty());
        assert!(inv_ui.loot_bag_rect.is_some());
    }

    #[test]
    fn loot_bag_ui_renders_empty_bag() {
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(bag_view(false)));
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            loot_bag_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // Even an empty bag draws the two grids and instructional copy.
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn loot_bag_ui_resolves_pending_quick_transfer() {
        // A pending shift+click intent is consumed by the bag UI and
        // turned into a LootBag command (which fails-soft with no
        // session, recording a toast).
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(bag_view(true)));
        let mut inv_ui = InventoryUiState::default();
        inv_ui.pending_quick_transfer = Some(UnifiedSlotRef::Bag(0));
        let mut toasts: Vec<String> = Vec::new();

        run_ui(|ctx| {
            loot_bag_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // The intent was taken (cleared) and a command attempt happened.
        assert!(inv_ui.pending_quick_transfer.is_none());
        assert!(
            toasts.iter().any(|t| t.contains("not connected")),
            "quick-transfer should attempt a send (fails without session)"
        );
    }
}
