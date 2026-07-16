//! The worn-armor paperdoll shown on the left of the Inventory tab: the four
//! equipment slots (Head, Chest, Legs, Feet) stacked beside a live 3D preview
//! of the character wearing them, with a compact per-kind protection summary
//! underneath.
//!
//! The preview image is rendered off-screen by the paperdoll-preview camera
//! (`app::systems::paperdoll_preview`), which dresses a dedicated copy of the
//! player rig from the same local (predicted) inventory these slots edit, so a
//! drag onto a slot dresses the figure the same frame. The slots reuse the
//! shared [`draw_slot`] widget, so a drag from the bag onto a paperdoll slot
//! rides the exact same drag pipeline as a bag-to-bag move: the destination is
//! a `UnifiedSlotRef::Player` addressing an [`ItemContainer::Equipment`](crate::protocol::ItemContainer::Equipment) slot,
//! and the shared move validation (armor-only, slot-matched, swap-never-merge)
//! silently rejects an invalid drop. The column records each slot's rect on
//! [`InventoryUiState`] so a release over a slot counts as landing on an
//! inventory surface rather than dropping on the ground.

use bevy_egui::egui::{self, Color32, RichText, StrokeKind};

use crate::{
    app::state::{InventoryUiState, LocalPlayerState, UnifiedSlotRef},
    app::systems::paperdoll_preview_texture,
    items::{ArmorProtection, equipped_protection},
    protocol::{EquipmentSlot, ItemContainerSlot, PlayerInventoryState},
};

use super::super::theme;
use super::slot::{SLOT_SIZE, draw_slot, slot_stack};

/// Painted size of the character preview. Half the off-screen target's
/// resolution, so the figure supersamples 2x.
const PREVIEW_WIDTH: f32 = 150.0;
const PREVIEW_HEIGHT: f32 = 320.0;
/// Gap between the slot stack and the preview.
const PREVIEW_GAP: f32 = 10.0;

/// Fixed width of the paperdoll column: the slot stack, a gap, then the
/// character preview. Kept as a constant so the panel-width math in
/// [`crate::app::ui::inventory_panel`] can add exactly this plus the gap.
pub(in crate::app::ui) const PAPERDOLL_COLUMN_WIDTH: f32 = SLOT_SIZE + PREVIEW_GAP + PREVIEW_WIDTH;

/// Gap between the paperdoll column and the bag grid to its right.
pub(in crate::app::ui) const PAPERDOLL_COLUMN_GAP: f32 = 16.0;

/// Vertical gap between stacked paperdoll slots.
const SLOT_GAP: f32 = 6.0;

/// Dimmed protection colour for a zero value, so an empty column reads as "no
/// protection here" without shouting.
const ZERO_PROTECTION_COLOR: Color32 = Color32::from_rgb(96, 106, 118);

/// Draw the paperdoll: the four equipment slots stacked top to bottom, the
/// character preview beside them, and the protection summary underneath.
/// Registers each slot's rect on `inventory_ui.equipment_rects` so the
/// drag-release resolver can tell a drop over a paperdoll slot apart from a
/// drop on the ground.
pub(in crate::app::ui) fn draw_paperdoll_column(
    ui: &mut egui::Ui,
    local_player: &LocalPlayerState,
    inventory_ui: &mut InventoryUiState,
) {
    let inventory = local_player.private.as_ref().map(|p| &p.inventory);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        // Slot stack, vertically centered against the preview so the pair
        // reads as one unit.
        ui.vertical(|ui| {
            ui.set_width(SLOT_SIZE);
            ui.spacing_mut().item_spacing.y = 0.0;
            let stack_height = EquipmentSlot::ALL.len() as f32 * (SLOT_SIZE + SLOT_GAP) - SLOT_GAP;
            ui.add_space(((PREVIEW_HEIGHT - stack_height) / 2.0).max(0.0));
            for (index, equipment_slot) in EquipmentSlot::ALL.into_iter().enumerate() {
                let slot = ItemContainerSlot::equipment(equipment_slot);
                let stack = inventory.and_then(|inventory| slot_stack(inventory, slot));
                // An empty slot shows a subdued slot-name label so the player
                // reads what belongs there; a worn slot shows the piece and
                // hides the label (the icon speaks for itself).
                let label = stack.is_none().then(|| equipment_slot.label().to_owned());

                let before = ui.cursor().min;
                draw_slot(
                    ui,
                    UnifiedSlotRef::Player(slot),
                    stack,
                    label,
                    false,
                    true,
                    // No shift-transfer destination for a paperdoll slot; the
                    // gesture falls through to a normal drag.
                    false,
                    inventory_ui,
                );
                // Record the rect this slot occupied so a drag released over
                // it is treated as an inventory surface, not a drop on the
                // ground.
                let rect = egui::Rect::from_min_size(before, egui::vec2(SLOT_SIZE, SLOT_SIZE));
                inventory_ui.equipment_rects[index] = Some(rect);

                if index + 1 < EquipmentSlot::ALL.len() {
                    ui.add_space(SLOT_GAP);
                }
            }
        });

        ui.add_space(PREVIEW_GAP);

        ui.vertical(|ui| {
            ui.set_width(PREVIEW_WIDTH);
            draw_character_preview(ui);
            ui.add_space(10.0);
            draw_protection_summary(ui, inventory);
        });
    });
}

/// The live character render inside a quiet slot-style frame. Headless/test
/// contexts (and the first frame before the render lands) just show the frame;
/// the camera only renders while this tab is up, so the image is always
/// current when it exists.
fn draw_character_preview(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(PREVIEW_WIDTH, PREVIEW_HEIGHT),
        egui::Sense::hover(),
    );
    ui.painter().rect(
        rect,
        6,
        Color32::from_rgba_unmultiplied(6, 9, 13, 180),
        egui::Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );
    if let Some(texture) = paperdoll_preview_texture() {
        ui.painter().image(
            texture,
            rect.shrink(1.0),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

/// The per-kind protection the summary shows: the shared [`equipped_protection`]
/// over the worn slots, so the number the player sees is exactly the mitigation
/// the server applies. An absent inventory reads zero (a bare player). Split out
/// so a test can assert the summary math without scraping rendered text.
fn summary_protection(inventory: Option<&PlayerInventoryState>) -> ArmorProtection {
    inventory
        .map(|inventory| equipped_protection(&inventory.equipment_slots))
        .unwrap_or_default()
}

/// A compact three-line protection readout. Zero values are dimmed.
fn draw_protection_summary(ui: &mut egui::Ui, inventory: Option<&PlayerInventoryState>) {
    let protection = summary_protection(inventory);

    ui.label(theme::field_label("Protection"));
    ui.add_space(4.0);
    protection_row(ui, "Melee", protection.melee);
    protection_row(ui, "Ranged", protection.projectile);
    protection_row(ui, "Blast", protection.blast);
}

/// One `Label  N%` protection row. A zero reads dimmed so the eye skips it.
fn protection_row(ui: &mut egui::Ui, label: &str, value: u8) {
    let color = if value == 0 {
        ZERO_PROTECTION_COLOR
    } else {
        theme::text()
    };
    ui.label(
        RichText::new(format!("{label} {value}%"))
            .size(12.0)
            .color(color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::{PADDED_HOOD_ID, PADDED_LEGGINGS_ID, PADDED_TUNIC_ID, PADDED_WRAPS_ID},
        protocol::{ItemStack, PlayerInventoryState},
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

    /// Every worn slot gets a rect registered, so a drop over any paperdoll slot
    /// resolves as an inventory surface rather than the ground.
    #[test]
    fn paperdoll_registers_all_four_slot_rects() {
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut inv_ui = InventoryUiState::default();
        run_ui(|ui| {
            egui::Area::new("paperdoll_test".into())
                .fixed_pos(egui::pos2(0.0, 0.0))
                .show(ui.ctx(), |ui| {
                    draw_paperdoll_column(ui, &local, &mut inv_ui);
                });
        });
        assert!(
            inv_ui.equipment_rects.iter().all(Option::is_some),
            "all four paperdoll slots register a rect"
        );
    }

    /// The four slots stack top to bottom in `EquipmentSlot::ALL` order (Head at
    /// the top, Feet at the bottom), so the rects are vertically ordered.
    #[test]
    fn paperdoll_slots_stack_top_to_bottom() {
        let local = local_player(Some(PlayerInventoryState::empty()));
        let mut inv_ui = InventoryUiState::default();
        run_ui(|ui| {
            egui::Area::new("paperdoll_order".into())
                .fixed_pos(egui::pos2(0.0, 0.0))
                .show(ui.ctx(), |ui| {
                    draw_paperdoll_column(ui, &local, &mut inv_ui);
                });
        });
        let rects: Vec<_> = inv_ui
            .equipment_rects
            .iter()
            .map(|rect| rect.expect("slot rect present"))
            .collect();
        for pair in rects.windows(2) {
            assert!(
                pair[0].top() < pair[1].top(),
                "each slot sits below the previous"
            );
        }
    }

    /// The protection summary reports exactly what `equipped_protection` sums
    /// over the worn slots (the padded set's spec totals), so the number the
    /// player reads is the mitigation the server applies.
    #[test]
    fn protection_summary_matches_equipped_protection() {
        let mut worn = PlayerInventoryState::empty();
        worn.equipment_slots[EquipmentSlot::Head.index()] = Some(ItemStack::new(PADDED_HOOD_ID, 1));
        worn.equipment_slots[EquipmentSlot::Chest.index()] =
            Some(ItemStack::new(PADDED_TUNIC_ID, 1));
        worn.equipment_slots[EquipmentSlot::Legs.index()] =
            Some(ItemStack::new(PADDED_LEGGINGS_ID, 1));
        worn.equipment_slots[EquipmentSlot::Feet.index()] =
            Some(ItemStack::new(PADDED_WRAPS_ID, 1));

        let summary = summary_protection(Some(&worn));
        assert_eq!(summary, equipped_protection(&worn.equipment_slots));
        // Sanity-check against the padded set's spec totals.
        assert_eq!(summary.melee, 12);
        assert_eq!(summary.projectile, 10);
        assert_eq!(summary.blast, 4);
    }

    /// A missing inventory (not yet replicated) reads as a bare player: zero
    /// mitigation everywhere, never a panic.
    #[test]
    fn protection_summary_is_zero_without_inventory() {
        assert_eq!(summary_protection(None), ArmorProtection::default());
    }

    /// A full padded set renders more painted shapes than an empty paperdoll:
    /// four item icons plus the non-zero protection rows land on top.
    #[test]
    fn worn_set_paints_more_than_empty() {
        let empty = local_player(Some(PlayerInventoryState::empty()));
        let mut worn_inventory = PlayerInventoryState::empty();
        worn_inventory.equipment_slots[EquipmentSlot::Head.index()] =
            Some(ItemStack::new(PADDED_HOOD_ID, 1));
        worn_inventory.equipment_slots[EquipmentSlot::Chest.index()] =
            Some(ItemStack::new(PADDED_TUNIC_ID, 1));
        worn_inventory.equipment_slots[EquipmentSlot::Legs.index()] =
            Some(ItemStack::new(PADDED_LEGGINGS_ID, 1));
        worn_inventory.equipment_slots[EquipmentSlot::Feet.index()] =
            Some(ItemStack::new(PADDED_WRAPS_ID, 1));
        let worn = local_player(Some(worn_inventory));

        let mut inv_ui_a = InventoryUiState::default();
        let empty_out = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                draw_paperdoll_column(ui, &empty, &mut inv_ui_a);
            });
        });
        let mut inv_ui_b = InventoryUiState::default();
        let worn_out = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                draw_paperdoll_column(ui, &worn, &mut inv_ui_b);
            });
        });
        assert!(worn_out.shapes.len() > empty_out.shapes.len());
    }
}
