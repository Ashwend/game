//! Admin item-grant tab: the third tab of the unified inventory/crafting
//! panel, visible only while the local player is an admin.
//!
//! A scrollable grid of every obtainable item (icon + tooltip, reusing the
//! inventory slot painter) where a click grants the item through the existing
//! `/give` command path: the click sends `ClientMessage::Command { "give
//! <id> <n>" }`, so the server's `command_give` stays the single authority for
//! admin checks, stack splitting, and overflow toasts. This tab is pure UI
//! sugar over `/give`; a non-admin never sees it, and a spoofed click would
//! just earn the server's "admin only" reply.
//!
//! Grant sizes scale with how the item stacks: bulk resources (stack 100+)
//! give 100 on left click / 250 on right click, ordinary stackables give
//! 1 / 10, and unstackables give a single one on either button.

use bevy_egui::egui::{self, Sense, Vec2};

use crate::{
    app::state::{ClientRuntime, ErrorToastSink},
    items::{DeployableKind, ItemDefinition, REGISTERED_ITEMS, RUIN_CACHE_ID},
    protocol::{ClientMessage, ItemStack},
};

use super::inventory::slot::{SLOT_SIZE, paint_slot};
use super::theme;

/// Slot gap matching the inventory grid, so the two tabs read as one family.
const GRID_GAP: f32 = 6.0;
/// Grid columns. Wider than the 12-column bag grid because the panel grew for
/// the paperdoll's character preview; 14 tiles of 56 + gaps still fit the
/// shared shell with room to spare.
const GRID_COLUMNS: usize = 14;

/// The `(left_click, right_click)` grant quantities for an item, scaled to how
/// it stacks: bulk resources land in piles, gear lands one at a time.
pub(super) fn grant_amounts(definition: &ItemDefinition) -> (u32, u32) {
    match definition.stack_size {
        0..=1 => (1, 1),
        2..=99 => (1, 10),
        _ => (100, 250),
    }
}

/// Every item the admin tab offers: the full registry minus the rows that are
/// not real inventory items (hidden building-piece entries placed via the
/// building plan, and the world-spawned ruin cache, which is not placeable).
fn grantable_items() -> impl Iterator<Item = &'static ItemDefinition> {
    REGISTERED_ITEMS.iter().filter(|definition| {
        let hidden_building = matches!(
            definition.deployable.map(|profile| profile.kind),
            Some(DeployableKind::Building { .. })
        );
        !hidden_building && definition.id != RUIN_CACHE_ID
    })
}

/// The Admin tab body: a hint line and the scrollable item grid.
pub(super) fn admin_items_body(
    ui: &mut egui::Ui,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    ui.label(
        egui::RichText::new(
            "Click an item to grant it to yourself (left / right click for small / large \
             amounts). Grants go through /give, so the server still validates everything.",
        )
        .size(12.0)
        .color(theme::muted_text()),
    );
    ui.add_space(10.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(GRID_GAP);
            let mut items = grantable_items().peekable();
            while items.peek().is_some() {
                ui.horizontal(|ui| {
                    for definition in items.by_ref().take(GRID_COLUMNS) {
                        admin_item_slot(ui, definition, runtime, error_toasts);
                    }
                });
            }
        });
}

/// One clickable item tile: the shared slot chrome + icon, a hover tooltip
/// with the item's name / description / grant amounts, and the click-to-give
/// behaviour.
fn admin_item_slot(
    ui: &mut egui::Ui,
    definition: &'static ItemDefinition,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SLOT_SIZE), Sense::click());
    // The painter wants a stack; quantity 1 keeps the count badge off so the
    // tile reads as a catalogue entry, not an owned pile.
    let stack = ItemStack::new(definition.id, 1);
    paint_slot(
        ui,
        rect,
        Some(&stack),
        None,
        false,
        response.hovered(),
        false,
        0.0,
    );

    let (left_amount, right_amount) = grant_amounts(definition);
    let response = response.on_hover_ui(|ui| {
        ui.set_max_width(260.0);
        ui.label(egui::RichText::new(definition.name).strong());
        ui.label(
            egui::RichText::new(definition.description)
                .size(12.0)
                .color(theme::muted_text()),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(if left_amount == right_amount {
                format!("Click: +{left_amount}")
            } else {
                format!("Left click: +{left_amount}    Right click: +{right_amount}")
            })
            .size(12.0)
            .color(theme::muted_text()),
        );
    });
    if response.clicked() {
        grant(runtime, error_toasts, definition.id, left_amount);
    }
    if response.secondary_clicked() {
        grant(runtime, error_toasts, definition.id, right_amount);
    }
}

/// Send one `/give` through the session, surfacing a send failure as an error
/// toast exactly like the chat command path.
fn grant(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    item_id: &str,
    amount: u32,
) {
    let message = ClientMessage::Command {
        text: format!("give {item_id} {amount}"),
    };
    let failure: Option<String> = if let Some(session) = runtime.session.as_mut() {
        session
            .send(message)
            .err()
            .map(|error| format!("give failed: {error}"))
    } else {
        Some("give failed: not connected".to_owned())
    };
    if let Some(text) = failure {
        runtime.push_error_message(text.clone());
        error_toasts.push_error(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{IRON_HATCHET_ID, POWDER_BOMB_ID, WOOD_ID, item_definition};

    #[test]
    fn grant_amounts_scale_with_stack_size() {
        // Bulk resource (wood stacks 100+): big piles.
        let wood = item_definition(WOOD_ID).unwrap();
        assert!(wood.stack_size >= 100, "wood is the bulk archetype");
        assert_eq!(grant_amounts(wood), (100, 250));
        // Ordinary stackable (bombs stack to 20): single / handful.
        let bomb = item_definition(POWDER_BOMB_ID).unwrap();
        assert!(bomb.stack_size > 1 && bomb.stack_size < 100);
        assert_eq!(grant_amounts(bomb), (1, 10));
        // Unstackable gear: one at a time on either button.
        let hatchet = item_definition(IRON_HATCHET_ID).unwrap();
        assert_eq!(hatchet.stack_size, 1);
        assert_eq!(grant_amounts(hatchet), (1, 1));
    }

    #[test]
    fn grantable_items_hide_building_rows_and_the_ruin_cache() {
        let ids: Vec<&str> = grantable_items().map(|d| d.id).collect();
        assert!(ids.contains(&WOOD_ID));
        assert!(ids.contains(&POWDER_BOMB_ID));
        assert!(!ids.contains(&RUIN_CACHE_ID), "world-spawned, not givable");
        for definition in REGISTERED_ITEMS {
            let hidden_building = matches!(
                definition.deployable.map(|profile| profile.kind),
                Some(DeployableKind::Building { .. })
            );
            assert_eq!(
                ids.contains(&definition.id),
                !hidden_building && definition.id != RUIN_CACHE_ID,
                "{} filtered wrong",
                definition.id
            );
        }
    }
}
