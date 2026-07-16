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
use super::admin_items::admin_items_body;
use super::crafting::{StationContext, crafting_body};
use super::inventory::{
    PAPERDOLL_COLUMN_GAP, PAPERDOLL_COLUMN_WIDTH, draw_actionbar, draw_inventory_grid,
    draw_paperdoll_column, pickup_tooltip,
};
use super::modal::backdrop_layer;
use super::theme::{self, ButtonKind};

/// Fixed outer width. The inner content area holds the Inventory tab's
/// paperdoll column (slot stack + character preview), a gap, then the
/// 12-column bag grid, all with the standard tight gaps:
/// `PAPERDOLL_COLUMN_WIDTH (216) + PAPERDOLL_COLUMN_GAP (16) + 12*56 + 11*6 +
/// 48 (frame margins) = 1018`. The crafting tab shares this width (its detail
/// card gets the extra room). If the grid columns or the paperdoll column
/// change, recompute this; the width test below pins the arithmetic.
const PANEL_WIDTH: f32 = 1018.0;
/// Fixed inner height, sized so the inventory tab's header + 5-row bag grid +
/// hint footer and the crafting tab's master/detail split both fill the panel
/// comfortably. Both tabs reserve this so the panel never resizes when you
/// flip between them.
const PANEL_HEIGHT: f32 = 500.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Inventory,
    Crafting,
    /// Admin-only item-grant grid (see [`super::admin_items`]). A pure VIEW of
    /// the inventory-open panel: `MenuState::inventory_open` stays the open
    /// source of truth (so every overlay/control gate is untouched) and
    /// `InventoryUiState::admin_tab` selects this body instead of the grid.
    Admin,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Inventory => "Inventory",
            Tab::Crafting => "Crafting",
            Tab::Admin => "Admin",
        }
    }
}

/// Which tab the panel is showing, or `None` when it's closed. Derived from
/// the two mutually-exclusive `MenuState` bools; the admin flag picks the
/// Admin view of the inventory-open state.
fn active_tab(menu: &MenuState, inventory_ui: &InventoryUiState) -> Option<Tab> {
    if menu.inventory_open {
        Some(if inventory_ui.admin_tab {
            Tab::Admin
        } else {
            Tab::Inventory
        })
    } else if menu.crafting_open {
        Some(Tab::Crafting)
    } else {
        None
    }
}

/// Top-level entry for the unified panel. Replaces the old separate
/// `inventory_ui` + `crafting_ui` calls in `ui_system`.
#[expect(clippy::too_many_arguments, reason = "egui UI plumbing")]
pub(super) fn inventory_panel_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    crafting_ui: &mut CraftingUiState,
    stations: &StationContext,
    pickup_target: &PickupTargetState,
    inventory_sound_requests: &mut InventorySoundRequests,
    error_toasts: &mut dyn ErrorToastSink,
    delta_seconds: f32,
    show_hud: bool,
) {
    // Per-frame inventory bookkeeping runs regardless of which tab (or none)
    // is up: slot flashes and pickup/move/drop cues track the replicated
    // inventory whether or not the player is looking at the grid. The
    // drop/move cues additionally gate on an item surface being open (see
    // `observe_inventory`), so ammo/charge consumption mid-combat doesn't
    // click like a UI interaction.
    inventory_ui.begin_frame();
    inventory_ui.tick_slot_flashes(delta_seconds);
    let item_ui_open =
        menu.inventory_open || menu.crafting_open || menu.furnace_open || menu.loot_bag_open;
    match local_player.private.as_ref().map(|p| &p.inventory) {
        Some(inventory) => {
            if let Some(event) = inventory_ui.observe_inventory(inventory, item_ui_open) {
                inventory_sound_requests.push(event);
            }
        }
        None => inventory_ui.clear_inventory_tracking(),
    }

    // The admin view only exists for admins; losing admin (or never having
    // it) snaps the panel back to the plain Inventory tab.
    if !runtime.is_admin {
        inventory_ui.admin_tab = false;
    }
    let tab = active_tab(menu, inventory_ui);

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
        // A reopen always lands on the plain Inventory tab, never a stale
        // admin view from last time.
        inventory_ui.admin_tab = false;
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
            stations,
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
            menu.world_map_open,
        );
        // Building-block tooltips are the hammer's readout
        // (stability/repair); they only show while the hammer is the
        // active item so every wall the player walks past stays quiet.
        let hammer_equipped = local_player
            .private
            .as_ref()
            .and_then(|private| private.inventory.active_actionbar_stack())
            .is_some_and(|stack| stack.item_id.as_ref() == crate::items::HAMMER_ID);
        pickup_tooltip(ctx, menu, pickup_target, hammer_equipped);
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
#[expect(clippy::too_many_arguments, reason = "egui UI plumbing")]
fn draw_panel(
    ctx: &egui::Context,
    menu: &mut MenuState,
    tab: Tab,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    crafting_ui: &mut CraftingUiState,
    stations: &StationContext,
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
                tab_bar(ui, menu, inventory_ui, crafting_ui, tab, runtime.is_admin);
                ui.add_space(14.0);
                match tab {
                    Tab::Inventory => {
                        // Paperdoll column on the left, then the header + bag
                        // grid to its right. Both live inside one horizontal row
                        // so a drag can cross from the bag onto a worn slot
                        // without leaving the panel.
                        ui.horizontal_top(|ui| {
                            // Zero the inter-widget spacing so the only gap
                            // between the paperdoll column and the bag grid is
                            // the explicit one below; the default spacing would
                            // widen the row past the fixed shell.
                            ui.spacing_mut().item_spacing.x = 0.0;
                            draw_paperdoll_column(ui, local_player, inventory_ui);
                            ui.add_space(PAPERDOLL_COLUMN_GAP);
                            ui.vertical(|ui| {
                                ui.set_width(
                                    PANEL_WIDTH
                                        - 48.0
                                        - PAPERDOLL_COLUMN_WIDTH
                                        - PAPERDOLL_COLUMN_GAP,
                                );
                                inventory_header(ui, local_player, runtime, error_toasts);
                                // The grid fills the remaining width and centers
                                // itself vertically in the fixed-height shell.
                                // Shift+click a bag armor piece to quick-equip
                                // it; the drag-release pass resolves the intent.
                                draw_inventory_grid(ui, local_player, inventory_ui, true);
                                // Quiet controls-legend pinned to the panel's
                                // bottom edge, in the slack the centered grid
                                // leaves below itself.
                                ui.with_layout(
                                    egui::Layout::bottom_up(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(
                                                "Drag to move  ·  Right-drag to split  ·  \
                                                 Shift-click to equip armor",
                                            )
                                            .size(11.5)
                                            .color(theme::muted_text()),
                                        );
                                    },
                                );
                            });
                        });
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
                            stations,
                            runtime,
                            error_toasts,
                        );
                    }
                    Tab::Admin => {
                        admin_items_body(ui, runtime, error_toasts);
                    }
                }
            });
        });
    response.response.rect
}

/// Thin header above the bag grid on the Inventory tab: a "Backpack" label
/// with a live used/total slot count on the left, and the right-aligned
/// "Sort" button that asks the server to auto-stack and tidy the bag. Lives
/// only on the Inventory tab so the gesture has an obvious target (the grid
/// right below it).
fn inventory_header(
    ui: &mut egui::Ui,
    local_player: &LocalPlayerState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    // Constrain the header to a fixed-height row. A bare `with_layout` would
    // claim the panel's full remaining height and center the button vertically,
    // pushing the grid to the bottom of the panel.
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 30.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(theme::field_label("Backpack"));
            if let Some(inventory) = local_player.private.as_ref().map(|p| &p.inventory) {
                let used = inventory
                    .inventory_slots
                    .iter()
                    .filter(|slot| slot.is_some())
                    .count();
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{used} / {} slots used",
                        crate::protocol::INVENTORY_SLOT_COUNT
                    ))
                    .size(12.0)
                    .color(theme::muted_text()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let sort = theme::compact_button(ui, "Sort", ButtonKind::Secondary, 72.0)
                    .on_hover_text("Auto-stack and tidy your bag");
                if sort.clicked() {
                    send_inventory_command(runtime, error_toasts, InventoryCommand::Sort);
                }
            });
        },
    );
}

fn tab_bar(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    inventory_ui: &mut InventoryUiState,
    crafting_ui: &mut CraftingUiState,
    active: Tab,
    is_admin: bool,
) {
    let frame = egui::Frame::NONE
        .fill(theme::input_fill())
        .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(6, 5));
    // The Admin tab only exists for admins; everyone else keeps the two-tab
    // bar (the server re-validates every grant anyway, this is pure UI).
    let tabs: &[Tab] = if is_admin {
        &[Tab::Inventory, Tab::Crafting, Tab::Admin]
    } else {
        &[Tab::Inventory, Tab::Crafting]
    };
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let count = tabs.len() as f32;
            let width = ((ui.available_width() - spacing * (count - 1.0)) / count).max(72.0);
            for &tab in tabs {
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
                            .insert_temp(super::tutorial::crafting_tab_rect_key(), rect);
                    });
                }
                if response.clicked() && tab != active {
                    select_tab(menu, inventory_ui, crafting_ui, tab);
                }
            }
        });
    });
}

/// Flip the active tab in place (never closes the panel). Switching into the
/// crafting tab resets its browser view for parity with the `C` hotkey path
/// (see [`CraftingUiState::reset_browser`]); the Admin tab rides the
/// inventory-open state with the view flag set.
fn select_tab(
    menu: &mut MenuState,
    inventory_ui: &mut InventoryUiState,
    crafting_ui: &mut CraftingUiState,
    tab: Tab,
) {
    match tab {
        Tab::Inventory => {
            menu.inventory_open = true;
            menu.crafting_open = false;
            inventory_ui.admin_tab = false;
        }
        Tab::Crafting => {
            menu.crafting_open = true;
            menu.inventory_open = false;
            inventory_ui.admin_tab = false;
            crafting_ui.reset_browser();
        }
        Tab::Admin => {
            menu.inventory_open = true;
            menu.crafting_open = false;
            inventory_ui.admin_tab = true;
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
            private: inventory.map(|inventory| PlayerPrivate {
                inventory,
                crafting: Default::default(),
                open_furnace: None,
                open_loot_bag: None,
                open_workbench: None,
                last_processed_input: 0,
                applied_action_seq: 0,
                run_speed_multiplier: 1.0,
                claim_status: Default::default(),
            }),
            lifecycle: None,
        }
    }

    fn run_ui(f: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        ctx.run_ui(
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
        let stations = StationContext::default();
        let pickup = PickupTargetState::default();
        let mut sounds = InventorySoundRequests::default();
        let mut toasts: Vec<String> = Vec::new();
        run_ui(|ui| {
            inventory_panel_ui(
                ui.ctx(),
                menu,
                &mut runtime,
                local,
                inv_ui,
                &mut crafting_ui,
                &stations,
                &pickup,
                &mut sounds,
                &mut toasts,
                0.016,
                true,
            );
        });
    }

    #[test]
    fn admin_tab_is_a_view_of_the_inventory_open_state() {
        // The admin flag only changes which body the inventory-open panel
        // shows; crafting and closed states ignore it.
        let mut inv_ui = InventoryUiState::default();
        inv_ui.admin_tab = true;
        let menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        assert_eq!(active_tab(&menu, &inv_ui), Some(Tab::Admin));
        let menu = MenuState {
            crafting_open: true,
            ..Default::default()
        };
        assert_eq!(active_tab(&menu, &inv_ui), Some(Tab::Crafting));
        assert_eq!(active_tab(&MenuState::default(), &inv_ui), None);
    }

    #[test]
    fn non_admin_never_sees_the_admin_view() {
        // A stale admin flag (e.g. admin revoked mid-session) is forced off by
        // the panel, so the body falls back to the plain inventory grid.
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        inv_ui.admin_tab = true;
        // `render` uses a default (non-admin) runtime.
        render(&mut menu, &local, &mut inv_ui);
        assert!(!inv_ui.admin_tab, "non-admin admin flag must reset");
    }

    #[test]
    fn closing_the_panel_resets_the_admin_view() {
        // Reopening the panel must land on Inventory, never a stale Admin tab.
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut inv_ui = InventoryUiState::default();
        inv_ui.admin_tab = true;
        inv_ui.was_open = true;
        let mut menu = MenuState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(!inv_ui.admin_tab);
    }

    #[test]
    fn select_tab_keeps_menu_bools_and_admin_flag_consistent() {
        let mut menu = MenuState::default();
        let mut inv_ui = InventoryUiState::default();
        let mut crafting_ui = CraftingUiState::default();

        select_tab(&mut menu, &mut inv_ui, &mut crafting_ui, Tab::Admin);
        assert!(menu.inventory_open && !menu.crafting_open && inv_ui.admin_tab);

        select_tab(&mut menu, &mut inv_ui, &mut crafting_ui, Tab::Crafting);
        assert!(!menu.inventory_open && menu.crafting_open && !inv_ui.admin_tab);

        select_tab(&mut menu, &mut inv_ui, &mut crafting_ui, Tab::Admin);
        select_tab(&mut menu, &mut inv_ui, &mut crafting_ui, Tab::Inventory);
        assert!(menu.inventory_open && !menu.crafting_open && !inv_ui.admin_tab);
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
    fn equipment_rects_registered_only_on_inventory_tab() {
        let local = local_player(Some(PlayerInventoryState::empty()));

        // Closed: the paperdoll never draws, so no equipment rects.
        let mut menu = MenuState::default();
        let mut inv_ui = InventoryUiState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.equipment_rects.iter().all(Option::is_none));

        // Inventory tab: the paperdoll column draws its four slots and each
        // registers a rect for drop-target detection.
        let mut menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.equipment_rects.iter().all(Option::is_some));

        // Crafting tab: no paperdoll, so the rects reset back to None.
        let mut menu = MenuState {
            crafting_open: true,
            ..Default::default()
        };
        let mut inv_ui = InventoryUiState::default();
        render(&mut menu, &local, &mut inv_ui);
        assert!(inv_ui.equipment_rects.iter().all(Option::is_none));
    }

    #[test]
    fn panel_width_matches_the_paperdoll_plus_grid_layout() {
        // The fixed width must equal the paperdoll column + gap + the 12-column
        // bag grid + frame margins, or the grid overflows or leaves a gap. This
        // pins the comment's arithmetic so a column change can't silently drift.
        use crate::app::ui::inventory::{PAPERDOLL_COLUMN_GAP, PAPERDOLL_COLUMN_WIDTH};
        const SLOT_SIZE: f32 = 56.0;
        const SLOT_GAP: f32 = 6.0;
        const COLUMNS: f32 = 12.0;
        const FRAME_MARGINS: f32 = 48.0;
        let grid = COLUMNS * SLOT_SIZE + (COLUMNS - 1.0) * SLOT_GAP;
        let expected = PAPERDOLL_COLUMN_WIDTH + PAPERDOLL_COLUMN_GAP + grid + FRAME_MARGINS;
        assert!((PANEL_WIDTH - expected).abs() < 0.5, "expected {expected}");
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
        let stations = StationContext::default();
        let mut inventory_rect = egui::Rect::NOTHING;
        let mut toasts: Vec<String> = Vec::new();
        run_ui(|ui| {
            inventory_rect = draw_panel(
                ui.ctx(),
                &mut inventory_menu,
                Tab::Inventory,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut crafting_ui,
                &stations,
                &mut toasts,
            );
        });

        let mut crafting_menu = MenuState {
            crafting_open: true,
            ..Default::default()
        };
        let mut crafting_rect = egui::Rect::NOTHING;
        run_ui(|ui| {
            crafting_rect = draw_panel(
                ui.ctx(),
                &mut crafting_menu,
                Tab::Crafting,
                &mut runtime,
                &local,
                &mut inv_ui,
                &mut crafting_ui,
                &stations,
                &mut toasts,
            );
        });

        assert!((inventory_rect.width() - crafting_rect.width()).abs() < 0.5);
        assert!(
            (inventory_rect.height() - crafting_rect.height()).abs() < 0.5,
            "inventory {inventory_rect:?} vs crafting {crafting_rect:?}"
        );
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
        let stations = StationContext::default();
        let mut toasts: Vec<String> = Vec::new();

        // Click near the screen corner (on the scrim, outside the centered
        // panel) while a drag is held.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(
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
            |ui| {
                draw_panel(
                    ui.ctx(),
                    &mut menu,
                    Tab::Inventory,
                    &mut runtime,
                    &local,
                    &mut inv_ui,
                    &mut crafting_ui,
                    &stations,
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
