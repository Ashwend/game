use bevy_egui::egui::{self, PointerButton, Sense, Vec2, vec2};

use crate::{
    app::{
        state::{
            ClientRuntime, ErrorToastSink, InventoryDragButton, InventoryUiState, LocalPlayerState,
            MenuState, PredictionState, UnifiedSlotRef,
        },
        systems::{send_furnace_command, send_inventory_command, send_loot_bag_command},
    },
    protocol::{FurnaceCommand, InventoryCommand, ItemContainerSlot, LootBagCommand},
};

use super::slot::{SLOT_SIZE, paint_slot};

/// Resolve a drag release. Picks the right network message based on
/// the source + target kind:
///
/// - Player → Player: `InventoryCommand::Move` (the existing path).
/// - Anything involving a furnace slot: `FurnaceCommand::Move`,
///   `FurnaceSlotRef` already covers all four combinations
///   (player↔furnace, furnace↔furnace).
///
/// Drop-on-ground (release outside any inventory surface) only fires
/// for player-sourced drags. Furnace-sourced items that get released
/// in the void snap back to where they came from, better that than
/// silently dropping a stack of iron bars into the void.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_drag_release(
    ctx: &egui::Context,
    menu: &MenuState,
    runtime: &mut ClientRuntime,
    prediction: &mut PredictionState,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    // Drag is allowed whenever a slot surface is up: the player's own
    // inventory, the furnace modal, or the loot-bag modal. All three
    // surfaces route through this unified pipeline.
    if !menu.inventory_open && !menu.furnace_open && !menu.loot_bag_open {
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
                prediction,
                local_player,
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
        // Predict the bag removal instantly; the dropped entity still
        // appears via server replication (no local ground ghost in Tier 1).
        let seq = predict_drop(prediction, local_player, from, Some(drag.quantity));
        send_inventory_command(
            runtime,
            error_toasts,
            InventoryCommand::Drop {
                from,
                quantity: Some(drag.quantity),
                seq,
            },
        );
    }

    inventory_ui.cancel_drag();
}

#[allow(clippy::too_many_arguments)]
fn send_move_command(
    runtime: &mut ClientRuntime,
    prediction: &mut PredictionState,
    local_player: &LocalPlayerState,
    error_toasts: &mut dyn ErrorToastSink,
    source: UnifiedSlotRef,
    target: UnifiedSlotRef,
    quantity: Option<u16>,
) {
    // Bag moves take priority, `as_loot_bag_ref` covers every
    // combination the bag command shape accepts (player↔bag,
    // bag↔bag). Furnace moves come next; only player↔player falls
    // through to the inventory-only command.
    if source.is_bag() || target.is_bag() {
        send_loot_bag_command(
            runtime,
            error_toasts,
            LootBagCommand::Move {
                from: source.as_loot_bag_ref(),
                to: target.as_loot_bag_ref(),
                quantity,
            },
        );
        return;
    }
    match (source, target) {
        (UnifiedSlotRef::Player(from), UnifiedSlotRef::Player(to)) => {
            let seq = predict_move(prediction, local_player, from, to, quantity);
            send_inventory_command(
                runtime,
                error_toasts,
                InventoryCommand::Move {
                    from,
                    to,
                    quantity,
                    seq,
                },
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

/// Predict a player-inventory drop, returning the action sequence the
/// command should carry (`0` = not predicted). Predicts unconditionally when
/// the local inventory is known, `remove_stack` no-ops harmlessly on replay
/// if the source slot turns out empty.
fn predict_drop(
    prediction: &mut PredictionState,
    local_player: &LocalPlayerState,
    from: ItemContainerSlot,
    quantity: Option<u16>,
) -> u32 {
    let Some(inventory) = local_player
        .private
        .as_ref()
        .map(|private| &private.inventory)
    else {
        return 0;
    };
    if inventory.slot(from).is_none() {
        return 0;
    }
    let seq = prediction.alloc_seq();
    prediction.push_drop(seq, from, quantity);
    seq
}

/// Predict a player→player inventory move, returning the action sequence the
/// command should carry (`0` = not predicted). Tier 1 predicts only the
/// empty-destination case, swap/merge onto an occupied slot stays
/// server-driven, since a mispredicted displacement is more jarring than a
/// brief replication delay. The shared `move_stack` replay handles the actual
/// relocation deterministically.
fn predict_move(
    prediction: &mut PredictionState,
    local_player: &LocalPlayerState,
    from: ItemContainerSlot,
    to: ItemContainerSlot,
    quantity: Option<u16>,
) -> u32 {
    let Some(inventory) = local_player
        .private
        .as_ref()
        .map(|private| &private.inventory)
    else {
        return 0;
    };
    // Empty destination + non-empty source, or the move no-ops / displaces.
    if inventory.slot(to).is_some() || inventory.slot(from).is_none() {
        return 0;
    }
    let seq = prediction.alloc_seq();
    prediction.push_move(seq, from, to, quantity);
    seq
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
        && !inventory_ui
            .loot_bag_rect
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::state::{InventoryDrag, MenuState},
        items::COAL_ID,
        protocol::{ItemContainerSlot, ItemStack},
    };

    fn drag_state(source: UnifiedSlotRef) -> InventoryUiState {
        let mut state = InventoryUiState::default();
        state.drag = Some(InventoryDrag {
            source,
            stack: ItemStack::new(COAL_ID, 6),
            quantity: 6,
            button: InventoryDragButton::Primary,
        });
        state
    }

    fn run_input(events: Vec<egui::Event>, mut f: impl FnMut(&egui::Context)) {
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 768.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| f(ctx),
        );
    }

    #[test]
    fn release_with_no_surface_open_cancels_drag() {
        let menu = MenuState {
            inventory_open: false,
            furnace_open: false,
            loot_bag_open: false,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut prediction = PredictionState::default();
        let local_player = LocalPlayerState::default();
        let mut inv_ui = drag_state(UnifiedSlotRef::Player(ItemContainerSlot::inventory(0)));
        let mut toasts: Vec<String> = Vec::new();

        run_input(Vec::new(), |ctx| {
            handle_drag_release(
                ctx,
                &menu,
                &mut runtime,
                &mut prediction,
                &local_player,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // No container surface is up, so the drag is dropped on the floor
        // (state-wise) and never becomes a network command.
        assert!(inv_ui.drag.is_none());
        assert!(toasts.is_empty());
    }

    #[test]
    fn no_release_event_keeps_drag_alive() {
        let menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut prediction = PredictionState::default();
        let local_player = LocalPlayerState::default();
        let mut inv_ui = drag_state(UnifiedSlotRef::Player(ItemContainerSlot::inventory(0)));
        let mut toasts: Vec<String> = Vec::new();

        // No pointer-release event this frame.
        run_input(Vec::new(), |ctx| {
            handle_drag_release(
                ctx,
                &menu,
                &mut runtime,
                &mut prediction,
                &local_player,
                &mut inv_ui,
                &mut toasts,
            );
        });
        assert!(
            inv_ui.drag.is_some(),
            "drag persists until the button is released"
        );
    }

    #[test]
    fn release_over_target_sends_move_and_clears_drag() {
        let menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut prediction = PredictionState::default();
        let local_player = LocalPlayerState::default();
        let mut inv_ui = drag_state(UnifiedSlotRef::Player(ItemContainerSlot::inventory(0)));
        // Hovering a different slot makes the release a Move.
        inv_ui.hovered_slot = Some(UnifiedSlotRef::Player(ItemContainerSlot::inventory(5)));
        let mut toasts: Vec<String> = Vec::new();

        // Simulate pressing then releasing the primary button so egui's
        // pointer state reports `button_released(Primary)`.
        let events = vec![
            egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        run_input(events, |ctx| {
            handle_drag_release(
                ctx,
                &menu,
                &mut runtime,
                &mut prediction,
                &local_player,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // Player→player move with no session fails-soft into a toast and
        // the drag is cleared either way.
        assert!(inv_ui.drag.is_none());
        assert!(toasts.iter().any(|t| t.contains("not connected")));
    }

    #[test]
    fn release_on_same_slot_is_a_noop_move() {
        let menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut prediction = PredictionState::default();
        let local_player = LocalPlayerState::default();
        let source = UnifiedSlotRef::Player(ItemContainerSlot::inventory(2));
        let mut inv_ui = drag_state(source);
        // Released back over the originating slot → no command.
        inv_ui.hovered_slot = Some(source);
        let mut toasts: Vec<String> = Vec::new();

        let events = vec![
            egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        run_input(events, |ctx| {
            handle_drag_release(
                ctx,
                &menu,
                &mut runtime,
                &mut prediction,
                &local_player,
                &mut inv_ui,
                &mut toasts,
            );
        });
        assert!(inv_ui.drag.is_none());
        assert!(
            toasts.is_empty(),
            "dropping onto the source slot sends nothing"
        );
    }

    #[test]
    fn release_outside_surfaces_drops_player_item() {
        let menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut prediction = PredictionState::default();
        let local_player = LocalPlayerState::default();
        let mut inv_ui = drag_state(UnifiedSlotRef::Player(ItemContainerSlot::inventory(0)));
        // No hovered slot and no recorded surface rects → the pointer is
        // "outside" everything, so a player item drops on the ground.
        inv_ui.hovered_slot = None;
        let mut toasts: Vec<String> = Vec::new();

        let events = vec![
            egui::Event::PointerButton {
                pos: egui::pos2(640.0, 384.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: egui::pos2(640.0, 384.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        run_input(events, |ctx| {
            handle_drag_release(
                ctx,
                &menu,
                &mut runtime,
                &mut prediction,
                &local_player,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // Drop command attempted (fails-soft, no session) and drag cleared.
        assert!(inv_ui.drag.is_none());
        assert!(toasts.iter().any(|t| t.contains("not connected")));
    }

    #[test]
    fn bag_source_release_over_player_routes_loot_bag_command() {
        let menu = MenuState {
            loot_bag_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut prediction = PredictionState::default();
        let local_player = LocalPlayerState::default();
        let mut inv_ui = drag_state(UnifiedSlotRef::Bag(0));
        inv_ui.hovered_slot = Some(UnifiedSlotRef::Player(ItemContainerSlot::inventory(1)));
        let mut toasts: Vec<String> = Vec::new();

        let events = vec![
            egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        run_input(events, |ctx| {
            handle_drag_release(
                ctx,
                &menu,
                &mut runtime,
                &mut prediction,
                &local_player,
                &mut inv_ui,
                &mut toasts,
            );
        });
        // The bag→player move attempts a LootBag command (fails-soft).
        assert!(inv_ui.drag.is_none());
        assert!(toasts.iter().any(|t| t.contains("not connected")));
    }

    fn run_preview(inv_ui: &InventoryUiState) -> egui::FullOutput {
        let ctx = egui::Context::default();
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 768.0),
                )),
                events: vec![egui::Event::PointerMoved(egui::pos2(100.0, 100.0))],
                ..Default::default()
            },
            |ctx| draw_drag_preview(ctx, inv_ui),
        )
    }

    #[test]
    fn draw_drag_preview_paints_only_while_dragging() {
        // No drag → the early return means no preview shapes.
        let idle = InventoryUiState::default();
        let idle_out = run_preview(&idle);

        // Active drag with a pointer position → the floating preview paints.
        let dragging = drag_state(UnifiedSlotRef::Player(ItemContainerSlot::inventory(0)));
        let drag_out = run_preview(&dragging);

        assert!(drag_out.shapes.len() > idle_out.shapes.len());
    }
}
