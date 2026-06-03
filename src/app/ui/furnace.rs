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
        state::{
            ClientRuntime, ErrorToastSink, InventoryUiState, LocalPlayerState, MenuState,
            UnifiedSlotRef,
        },
        systems::send_furnace_command,
    },
    protocol::{
        FURNACE_ITEM_SLOT_COUNT, FurnaceCommand, FurnaceSlotRef, INVENTORY_SLOT_COUNT,
        ItemContainerSlot, OpenFurnaceView, PlayerInventoryState,
    },
};

use super::{
    inventory::{INVENTORY_COLUMNS, drag::draw_drag_preview, slot::SLOT_SIZE, slot::draw_slot},
    modal::backdrop_layer,
    theme,
};

const SLOT_GAP: f32 = 6.0;
// Match the main inventory's column count so a player who's used to the
// bag's layout sees the same shape here. Shared with the inventory panel so
// the two can't drift. Actionbar is intentionally omitted - the on-screen
// hotbar at the bottom of the viewport already shows it, and the player can
// drag stacks straight to those slots.
const INVENTORY_COLS: usize = INVENTORY_COLUMNS;
// Sized so the player-inventory grid (the widest element) fills the inner
// content area exactly: `cols*slot + (cols-1)*gap + 48 (frame margins)`. The
// fuel/contents cluster up top is naturally narrower and sits left-aligned.
const PANEL_WIDTH: f32 =
    INVENTORY_COLS as f32 * SLOT_SIZE + (INVENTORY_COLS - 1) as f32 * SLOT_GAP + 48.0;
const PANEL_HEIGHT: f32 = 540.0;

pub(super) fn furnace_ui(
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
    // Source of truth is the replicated `PlayerPrivate.open_furnace`:
    // the server populates it whenever the player opens a furnace and
    // clears it on Close. The modal mirrors that, present when set,
    // absent otherwise.
    let view: OpenFurnaceView = match local_player
        .private
        .as_ref()
        .and_then(|private| private.open_furnace.clone())
    {
        Some(view) => view,
        None => return,
    };

    let inventory = local_player.private.as_ref().map(|p| p.inventory.clone());

    // Scrim. Click outside the panel sends Close to the server.
    let backdrop = backdrop_layer(
        ctx,
        "furnace_backdrop",
        Order::Middle,
        theme::backdrop_color(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::state::ClientRuntime,
        items::{COAL_ID, IRON_ORE_ID},
        protocol::ItemStack,
        server::PlayerPrivate,
    };

    fn furnace_view(active: bool) -> OpenFurnaceView {
        OpenFurnaceView {
            id: 1,
            fuel: Some(ItemStack::new(COAL_ID, 5)),
            items: vec![Some(ItemStack::new(IRON_ORE_ID, 3)), None, None],
            active,
            smelt_fraction: 0.5,
            fuel_fraction: 0.25,
        }
    }

    fn local_player(open_furnace: Option<OpenFurnaceView>) -> LocalPlayerState {
        LocalPlayerState {
            entity: None,
            public: None,
            private: Some(PlayerPrivate {
                inventory: PlayerInventoryState::empty(),
                crafting: Default::default(),
                open_furnace,
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
    fn furnace_ui_noop_without_open_furnace() {
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(None);
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            furnace_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // No furnace open → nothing renders and no panel rect is recorded.
        assert!(output.shapes.is_empty());
        assert!(inv_ui.furnace_rect.is_none());
    }

    #[test]
    fn furnace_ui_suppressed_while_paused() {
        let mut menu = MenuState {
            pause_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(furnace_view(true)));
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            furnace_ui(
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
    fn furnace_ui_renders_panel_and_records_rect() {
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(furnace_view(true)));
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            furnace_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // Open furnace paints the panel and records the rect so a drag
        // released over it doesn't fall through to drop-on-ground.
        assert!(!output.shapes.is_empty());
        assert!(inv_ui.furnace_rect.is_some());
    }

    #[test]
    fn furnace_ui_renders_with_inactive_furnace() {
        // Inactive furnace shows the "Turn on" primary button instead of
        // the danger "Turn off", both branches must paint cleanly.
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(furnace_view(false)));
        let mut inv_ui = InventoryUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            furnace_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut toasts,
            );
        });
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn draw_progress_bar_fills_and_empties() {
        // A full bar paints more shapes than an empty one (the fill rect
        // is only drawn when the fraction is > 0).
        let full_out = run_ui(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_progress_bar(ui, 1.0, Color32::WHITE);
            });
        });
        let empty_out = run_ui(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_progress_bar(ui, 0.0, Color32::WHITE);
            });
        });
        assert!(
            full_out.shapes.len() > empty_out.shapes.len(),
            "filled bar should paint more than empty"
        );
    }
}
