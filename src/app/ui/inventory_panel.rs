//! Unified inventory + crafting panel.
//!
//! Inventory and crafting share one fixed-size, centered panel with a tab bar.
//! `Tab` opens it on the Inventory tab, `C` on the Crafting tab; while open,
//! the other hotkey or a tab click flips the active tab in place. The panel
//! keeps the same geometry on both tabs so swapping reads as flipping a tab
//! rather than closing one window and opening another.
//!
//! The source of truth is still the two `MenuState` bools (`inventory_open` /
//! `crafting_open`), kept mutually exclusive by the toggle systems in
//! [`crate::app::systems::input`]. This module is the render-time shell that
//! hosts whichever tab is active and the per-frame inventory bookkeeping; the
//! per-tab bodies live in [`super::inventory`] (the slot grid + hotbar) and
//! [`super::crafting`] (the recipe browser).

use bevy_egui::egui::{self, Align2, Order};

use crate::app::state::{
    ClientRuntime, CraftingUiState, ErrorToastSink, InventoryUiState, LocalPlayerState, MenuState,
    PickupTargetState,
};
use crate::app::systems::send_inventory_command;
use crate::protocol::InventoryCommand;

use super::InventorySoundRequests;
use super::crafting::crafting_body;
use super::inventory::{draw_actionbar, draw_inventory_grid, pickup_tooltip};
use super::modal::backdrop_layer;
use super::theme::{self, ButtonKind};

/// Fixed outer width, sized so the 12-column inventory grid fills the inner
/// content area exactly with the standard tight gaps:
/// `12*56 + 11*6 + 48 (frame margins) = 786`. The crafting tab shares this
/// width. If the grid columns change, recompute this.
const PANEL_WIDTH: f32 = 786.0;
/// Fixed inner height, sized so the inventory's 7 displayed rows (60 real
/// slots + 2 rows of inert filler) plus the tab bar and spacing fill the panel
/// with only a hair of breathing room. The extra rows exist mostly to give the
/// crafting tab's recipe list this much vertical room. Both tabs reserve this
/// so the panel never resizes when you flip between them.
const PANEL_HEIGHT: f32 = 500.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Inventory,
    Crafting,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Inventory => "Inventory",
            Tab::Crafting => "Crafting",
        }
    }
}

/// Which tab the panel is showing, or `None` when it's closed. Derived from
/// the two mutually-exclusive `MenuState` bools.
fn active_tab(menu: &MenuState) -> Option<Tab> {
    if menu.inventory_open {
        Some(Tab::Inventory)
    } else if menu.crafting_open {
        Some(Tab::Crafting)
    } else {
        None
    }
}

/// Top-level entry for the unified panel. Replaces the old separate
/// `inventory_ui` + `crafting_ui` calls in `ui_system`.
#[allow(clippy::too_many_arguments)]
pub(super) fn inventory_panel_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    crafting_ui: &mut CraftingUiState,
    pickup_target: &PickupTargetState,
    inventory_sound_requests: &mut InventorySoundRequests,
    error_toasts: &mut dyn ErrorToastSink,
    delta_seconds: f32,
    show_hud: bool,
) {
    // Per-frame inventory bookkeeping runs regardless of which tab (or none)
    // is up: slot flashes and pickup/move/drop cues track the replicated
    // inventory whether or not the player is looking at the grid.
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

    let tab = active_tab(menu);

    // Closing the panel (or flipping off the Crafting tab) drops keyboard
    // focus so a focused recipe-search box stops eating keystrokes once the
    // player has moved on; closing additionally cancels any in-progress drag.
    let leaving_panel = inventory_ui.was_open && tab.is_none();
    let leaving_crafting = inventory_ui.was_crafting && tab != Some(Tab::Crafting);
    if leaving_panel || leaving_crafting {
        ctx.memory_mut(|memory| memory.stop_text_input());
    }
    if leaving_panel {
        inventory_ui.cancel_drag();
    }

    if let Some(tab) = tab
        && !menu.pause_open
    {
        let rect = draw_panel(
            ctx,
            menu,
            tab,
            runtime,
            local_player,
            inventory_ui,
            crafting_ui,
            error_toasts,
        );
        // Only the Inventory tab exposes draggable slots, so only there does
        // the panel double as the inventory drag surface. On the Crafting tab
        // the rect stays `None`, matching the old behavior where no grid was
        // painted.
        if tab == Tab::Inventory {
            inventory_ui.inventory_rect = Some(rect);
        }
    }

    // The hotbar and the world-pickup prompt are always-on HUD chrome, so the
    // HUD master toggle hides them (for clean screenshots) even though the
    // panel's per-frame inventory bookkeeping above keeps running regardless.
    if !menu.pause_open && show_hud {
        // The hotbar is a live drag surface only on the Inventory tab; on the
        // Crafting tab (or closed) it dims into a passive HUD strip. The
        // furnace stays the shift-transfer destination when one is open.
        let inventory_tab = tab == Some(Tab::Inventory);
        draw_actionbar(
            ctx,
            local_player,
            inventory_ui,
            inventory_tab,
            menu.furnace_open,
        );
        pickup_tooltip(ctx, menu, pickup_target);
    }

    // Drag release + preview deliberately run later in the top-level
    // `ui_system` so they see slots/rects painted by the furnace/loot-bag
    // modals this frame too.
    inventory_ui.was_open = tab.is_some();
    inventory_ui.was_crafting = tab == Some(Tab::Crafting);
}

/// Draw the shared scrim + fixed-size panel with its tab bar and the active
/// tab's body. Returns the panel's outer rect so the caller can record the
/// inventory drag surface.
#[allow(clippy::too_many_arguments)]
fn draw_panel(
    ctx: &egui::Context,
    menu: &mut MenuState,
    tab: Tab,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    crafting_ui: &mut CraftingUiState,
    error_toasts: &mut dyn ErrorToastSink,
) -> egui::Rect {
    // One shared scrim behind the panel. Clicking it closes the panel, but
    // only when no drag is in flight: a slot drag that releases over the
    // scrim must end the drag (drop-on-ground / snap-back), never also dismiss
    // the panel.
    let backdrop = backdrop_layer(
        ctx,
        "inventory_panel_backdrop",
        Order::Middle,
        theme::backdrop_color(),
    );
    if backdrop.clicked() && inventory_ui.drag.is_none() {
        menu.inventory_open = false;
        menu.crafting_open = false;
    }

    let response = egui::Area::new("inventory_panel".into())
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(PANEL_WIDTH);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(PANEL_WIDTH - 48.0);
                // Fix the height on both tabs so the shell never resizes when
                // the tab flips; the crafting body reads the leftover height
                // for its scroll area.
                ui.set_min_height(PANEL_HEIGHT);
                ui.set_max_height(PANEL_HEIGHT);
                tab_bar(ui, menu, crafting_ui, tab);
                ui.add_space(14.0);
                match tab {
                    Tab::Inventory => {
                        inventory_toolbar(ui, runtime, error_toasts);
                        // The grid fills the panel width and centers itself
                        // vertically in the fixed-height shell.
                        draw_inventory_grid(ui, local_player, inventory_ui);
                    }
                    Tab::Crafting => {
                        let inventory = local_player.private.as_ref().map(|p| p.inventory.clone());
                        let crafting_state = local_player
                            .private
                            .as_ref()
                            .map(|p| p.crafting.clone())
                            .unwrap_or_default();
                        crafting_body(
                            ui,
                            crafting_ui,
                            inventory.as_ref(),
                            &crafting_state,
                            runtime,
                            error_toasts,
                        );
                    }
                }
            });
        });
    response.response.rect
}

/// Thin header above the bag grid on the Inventory tab. Right-aligned "Sort"
/// button that asks the server to auto-stack and tidy the bag; the rest of the
/// row is left for future per-tab controls. Lives only on the Inventory tab so
/// the gesture has an obvious target (the grid right below it).
fn inventory_toolbar(
    ui: &mut egui::Ui,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    // Constrain the header to a fixed-height row. A bare `with_layout` would
    // claim the panel's full remaining height and center the button vertically,
    // pushing the grid to the bottom of the panel.
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 30.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let sort = theme::compact_button(ui, "Sort", ButtonKind::Secondary, 72.0)
                .on_hover_text("Auto-stack and tidy your bag");
            if sort.clicked() {
                theme::record_click_sound(ui, &sort);
                send_inventory_command(runtime, error_toasts, InventoryCommand::Sort);
            }
        },
    );
}

fn tab_bar(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    crafting_ui: &mut CraftingUiState,
    active: Tab,
) {
    let frame = egui::Frame::NONE
        .fill(theme::input_fill())
        .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(6, 5));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let width = ((ui.available_width() - spacing) / 2.0).max(72.0);
            for tab in [Tab::Inventory, Tab::Crafting] {
                let kind = if tab == active {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                };
                let response = theme::compact_button(ui, tab.label(), kind, width);
                if tab == Tab::Crafting {
                    // Stash the rect so the tutorial overlay can outline this tab
                    // without the step being threaded through the panel.
                    let rect = response.rect;
                    ui.ctx().memory_mut(|mem| {
                        mem.data
                            .insert_temp(super::tutorial::crafting_tab_rect_key(), rect)
                    });
                }
                if response.clicked() && tab != active {
                    select_tab(menu, crafting_ui, tab);
                }
            }
        });
    });
}

/// Flip the active tab in place (never closes the panel). Switching into the
/// crafting tab resets its browser view for parity with the `C` hotkey path
/// (see [`CraftingUiState::reset_browser`]).
fn select_tab(menu: &mut MenuState, crafting_ui: &mut CraftingUiState, tab: Tab) {
    match tab {
        Tab::Inventory => {
            menu.inventory_open = true;
            menu.crafting_open = false;
        }
        Tab::Crafting => {
            menu.crafting_open = true;
            menu.inventory_open = false;
            crafting_ui.reset_browser();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::state::{InventoryDrag, InventoryDragButton, UnifiedSlotRef},
        items::COAL_ID,
        protocol::{ItemContainerSlot, ItemStack, PlayerInventoryState},
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

    fn render(menu: &mut MenuState, local: &LocalPlayerState, inv_ui: &mut InventoryUiState) {
        let mut runtime = ClientRuntime::default();
        let mut crafting_ui = CraftingUiState::default();
        let pickup = PickupTargetState::default();
        let mut sounds = InventorySoundRequests::default();
        let mut toasts: Vec<String> = Vec::new();
        run_ui(|ctx| {
            inventory_panel_ui(
                ctx,
                menu,
                &mut runtime,
                local,
                inv_ui,
                &mut crafting_ui,
                &pickup,
                &mut sounds,
                &mut toasts,
                0.016,
                true,
            );
        });
    }

    #[test]
    fn inventory_tab_records_drag_surface_rect() {
        let local = local_player(Some(PlayerInventoryState::empty()));

        // Closed: no panel, so the inventory drag-surface rect stays None.
        let mut menu = MenuState::default();
        let mut inv_ui = InventoryUiState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.inventory_rect.is_none());

        // Inventory tab open: the panel rect is recorded as the drag surface.
        let mut menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.inventory_rect.is_some());
    }

    #[test]
    fn crafting_tab_leaves_inventory_rect_none() {
        // The crafting tab has no draggable slots, so it must not register the
        // panel as the inventory drag surface (else a drag released over the
        // recipe list would think it landed in the bag).
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut menu = MenuState {
            crafting_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.inventory_rect.is_none());
    }

    #[test]
    fn pause_hides_panel_and_actionbar() {
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut menu = MenuState {
            inventory_open: true,
            pause_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.inventory_rect.is_none());
        assert!(inv_ui.actionbar_rect.is_none());
    }

    #[test]
    fn closing_panel_cancels_in_progress_drag() {
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut inv_ui = InventoryUiState::default();
        // Was open last frame with a live drag; now closed.
        inv_ui.was_open = true;
        inv_ui.drag = Some(InventoryDrag {
            source: UnifiedSlotRef::Player(ItemContainerSlot::inventory(0)),
            stack: ItemStack::new(COAL_ID, 3),
            quantity: 3,
            button: InventoryDragButton::Primary,
        });
        let mut menu = MenuState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.drag.is_none());
        assert!(!inv_ui.was_open);
    }

    #[test]
    fn panel_is_same_fixed_size_on_both_tabs() {
        // Flipping tabs must not resize the panel: both tabs build the Area
        // from the same fixed width/height, so the outer rects match.
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut runtime = ClientRuntime::default();
        let mut crafting_ui = CraftingUiState::default();

        let mut inventory_menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        let mut inventory_rect = egui::Rect::NOTHING;
        let mut toasts: Vec<String> = Vec::new();
        run_ui(|ctx| {
            inventory_rect = draw_panel(
                ctx,
                &mut inventory_menu,
                Tab::Inventory,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut crafting_ui,
                &mut toasts,
            );
        });

        let mut crafting_menu = MenuState {
            crafting_open: true,
            ..Default::default()
        };
        let mut crafting_rect = egui::Rect::NOTHING;
        run_ui(|ctx| {
            crafting_rect = draw_panel(
                ctx,
                &mut crafting_menu,
                Tab::Crafting,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut crafting_ui,
                &mut toasts,
            );
        });

        assert!((inventory_rect.width() - crafting_rect.width()).abs() < 0.5);
        assert!((inventory_rect.height() - crafting_rect.height()).abs() < 0.5);
    }

    #[test]
    fn backdrop_click_does_not_close_panel_mid_drag() {
        // A drag in progress must survive a release over the scrim; the
        // release ends the drag elsewhere, the panel stays open.
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        inv_ui.drag = Some(InventoryDrag {
            source: UnifiedSlotRef::Player(ItemContainerSlot::inventory(0)),
            stack: ItemStack::new(COAL_ID, 1),
            quantity: 1,
            button: InventoryDragButton::Primary,
        });
        let mut runtime = ClientRuntime::default();
        let mut crafting_ui = CraftingUiState::default();
        let mut toasts: Vec<String> = Vec::new();

        // Click near the screen corner (on the scrim, outside the centered
        // panel) while a drag is held.
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 768.0),
                )),
                events: vec![
                    egui::Event::PointerButton {
                        pos: egui::pos2(8.0, 8.0),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::PointerButton {
                        pos: egui::pos2(8.0, 8.0),
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                draw_panel(
                    ctx,
                    &mut menu,
                    Tab::Inventory,
                    &mut runtime,
                    &local,
                    &mut inv_ui,
                    &mut crafting_ui,
                    &mut toasts,
                );
            },
        );
        assert!(
            menu.inventory_open,
            "drag in flight must keep the panel open"
        );
    }
}
