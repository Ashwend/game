//! Furnace interaction modal.
//!
//! Opens when the server's snapshot includes `local_player.open_furnace`.
//! Shares the slot widget + drag state with the main inventory so the
//! player gets the same stack/split/drag behaviour across both
//! surfaces. Differences:
//!   - Adds the on/off toggle + smelt/burn progress bars.
//!   - Tracks its own panel rect on [`InventoryUiState::furnace_rect`]
//!     so a drag released over the furnace doesn't fall through to the
//!     "drop on the ground" path.
//!
//! The server rejects illegal moves (non-fuel into the fuel slot,
//! out-of-range, etc.); the drag UX just shows the gesture and the
//! server snaps the failing item back via the next snapshot.

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, Id, Layout, Order, Pos2, Rect, RichText, Sense, vec2,
};

use crate::{
    app::{
        state::{ClientRuntime, ErrorToastSink, InventoryUiState, MenuState, UnifiedSlotRef},
        systems::send_furnace_command,
    },
    protocol::{
        FURNACE_ITEM_SLOT_COUNT, FurnaceCommand, FurnaceSlotRef, INVENTORY_SLOT_COUNT,
        ItemContainerSlot, OpenFurnaceView, PlayerInventoryState, PlayerState,
    },
};

use super::{
    inventory::{drag::draw_drag_preview, slot::draw_slot},
    modal::backdrop_layer,
    theme,
};

const PANEL_WIDTH: f32 = 720.0;
const PANEL_HEIGHT: f32 = 540.0;
const SLOT_GAP: f32 = 6.0;
// Match the main inventory's column count so a player who's used to the
// bag's layout sees the same shape here. Actionbar is intentionally
// omitted - the on-screen hotbar at the bottom of the viewport already
// shows it, and the player can drag stacks straight to those slots.
const INVENTORY_COLS: usize = 10;

pub(super) fn furnace_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    inventory_ui: &mut InventoryUiState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    if menu.pause_open {
        return;
    }
    // Source of truth is the server: a snapshot with `open_furnace`
    // means the modal is open. Closing the modal is "send Close" from
    // the client which clears the field on the next snapshot.
    let view: OpenFurnaceView = match runtime
        .local_player()
        .and_then(PlayerState::open_furnace)
        .cloned()
    {
        Some(view) => view,
        None => return,
    };

    let inventory = runtime
        .local_player()
        .and_then(PlayerState::inventory)
        .cloned();

    // Scrim. Click outside the panel sends Close to the server.
    let backdrop = backdrop_layer(
        ctx,
        "furnace_backdrop",
        Order::Middle,
        Color32::from_rgba_unmultiplied(1, 3, 7, 190),
    );
    if backdrop.clicked() {
        send_furnace_command(runtime, error_toasts, FurnaceCommand::Close);
        return;
    }

    let mut close_requested = false;
    let mut toggle_to: Option<bool> = None;
    let response = egui::Area::new(Id::new("furnace_panel"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(PANEL_WIDTH);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(PANEL_WIDTH - 48.0);
                ui.set_min_height(PANEL_HEIGHT);
                draw_panel(
                    ui,
                    &view,
                    inventory.as_ref(),
                    inventory_ui,
                    &mut close_requested,
                    &mut toggle_to,
                );
            });
        });
    // Record the panel rect so a player-sourced drag released *over*
    // the furnace doesn't trigger the drop-on-ground path. Drops still
    // happen for actual outside-the-panel releases.
    inventory_ui.furnace_rect = Some(response.response.rect);

    // The unified drag preview rides on the same pointer-following
    // tooltip layer as the main inventory's preview. We draw it after
    // the panel so it floats above the slots while the player is
    // dragging.
    draw_drag_preview(ctx, inventory_ui);

    if let Some(active) = toggle_to {
        send_furnace_command(runtime, error_toasts, FurnaceCommand::SetActive { active });
    }
    if close_requested {
        send_furnace_command(runtime, error_toasts, FurnaceCommand::Close);
    }

    // Shift+click intents recorded by any of the slots above (player
    // inventory grid, actionbar, fuel slot, or furnace items grid) are
    // resolved here so the network command is sent exactly once per
    // gesture. The `take()` clears the pending field so a stale value
    // from a previous frame can't fire twice.
    if let Some(source) = inventory_ui.pending_quick_transfer.take() {
        send_furnace_command(
            runtime,
            error_toasts,
            FurnaceCommand::QuickTransfer {
                from: source.as_furnace_ref(),
            },
        );
    }
}

fn draw_panel(
    ui: &mut egui::Ui,
    view: &OpenFurnaceView,
    inventory: Option<&PlayerInventoryState>,
    inventory_ui: &mut InventoryUiState,
    close_requested: &mut bool,
    toggle_to: &mut Option<bool>,
) {
    ui.horizontal(|ui| {
        ui.label(theme::section("Furnace"));
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
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
            "Load fuel into the leftmost slot. Drop smeltable items (e.g. iron ore) into the \
             furnace grid. Smelted output appears in the same grid. Drag the same way as your \
             inventory: left-click to grab, right-click to split, drag anywhere to move.",
        )
        .color(theme::muted_text())
        .small(),
    );
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(theme::field_label("Fuel"));
            draw_slot(
                ui,
                UnifiedSlotRef::Furnace(FurnaceSlotRef::Fuel),
                view.fuel.as_ref(),
                None,
                false,
                true,
                true,
                inventory_ui,
            );
            ui.add_space(8.0);
            ui.label(theme::field_label("Burn"));
            draw_progress_bar(ui, view.fuel_fraction, theme::accent());
            ui.add_space(8.0);
            ui.label(theme::field_label("Smelt"));
            draw_progress_bar(ui, view.smelt_fraction, Color32::from_rgb(230, 152, 64));
            ui.add_space(12.0);
            let (label, kind) = if view.active {
                ("Turn off", theme::ButtonKind::Danger)
            } else {
                ("Turn on", theme::ButtonKind::Primary)
            };
            let toggle = theme::compact_button(ui, label, kind, 132.0);
            theme::record_click_sound(ui, &toggle);
            if toggle.clicked() {
                *toggle_to = Some(!view.active);
            }
        });

        ui.add_space(16.0);

        ui.vertical(|ui| {
            ui.label(theme::field_label("Furnace contents"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = SLOT_GAP;
                for index in 0..FURNACE_ITEM_SLOT_COUNT {
                    let stack = view.items.get(index).and_then(|s| s.as_ref());
                    draw_slot(
                        ui,
                        UnifiedSlotRef::Furnace(FurnaceSlotRef::Item(index)),
                        stack,
                        None,
                        false,
                        true,
                        true,
                        inventory_ui,
                    );
                }
            });
        });
    });

    ui.add_space(16.0);
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

fn draw_progress_bar(ui: &mut egui::Ui, fraction: f32, fill_color: Color32) {
    let height = 8.0;
    let width = 132.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(3), theme::input_fill());
    let clamped = fraction.clamp(0.0, 1.0);
    if clamped > 0.0 {
        let fill_rect = Rect::from_min_max(
            rect.min,
            Pos2::new(rect.left() + rect.width() * clamped, rect.bottom()),
        );
        painter.rect_filled(fill_rect, CornerRadius::same(3), fill_color);
    }
}
