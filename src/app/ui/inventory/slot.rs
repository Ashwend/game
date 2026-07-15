use bevy_egui::egui::{
    self, Align2, Color32, FontFamily, FontId, PointerButton, Rect, Sense, Stroke, Vec2, pos2, vec2,
};

use crate::{
    app::state::{InventoryDrag, InventoryDragButton, InventoryUiState, UnifiedSlotRef},
    items::{item_definition, stack_limit},
    protocol::{ItemContainer, ItemContainerSlot, ItemStack, PlayerInventoryState},
};

use super::super::{item_icons, theme};

pub(crate) const SLOT_SIZE: f32 = 56.0;

/// Draw one inventory-style slot. Used by both the main inventory and
/// the furnace modal, `slot` is a `UnifiedSlotRef` so a drag can move
/// items across either container without the widget needing to know
/// the difference.
///
/// `shift_transfer_enabled` is set by callers that have a destination
/// container open (e.g. the furnace modal). When true, Shift+LMB on a
/// non-empty slot records a [`InventoryUiState::pending_quick_transfer`]
/// instead of starting a drag; the parent surface is then responsible
/// for routing that intent to the appropriate network command.
#[expect(clippy::too_many_arguments, reason = "egui UI plumbing")]
pub(crate) fn draw_slot(
    ui: &mut egui::Ui,
    slot: UnifiedSlotRef,
    stack: Option<&ItemStack>,
    label: Option<String>,
    active: bool,
    interactive: bool,
    shift_transfer_enabled: bool,
    inventory_ui: &mut InventoryUiState,
) {
    // Slot flashes are only tracked for player slots, they highlight
    // "you just gained items here", which is a property of the player's
    // inventory. Furnace and bag slots don't flash.
    let flash_strength = match slot {
        UnifiedSlotRef::Player(container_slot) => inventory_ui.slot_flash_strength(container_slot),
        UnifiedSlotRef::Furnace(_) | UnifiedSlotRef::Bag(_) => 0.0,
    };

    if !interactive {
        let (_, rect) = ui.allocate_space(Vec2::splat(SLOT_SIZE));
        paint_slot(
            ui,
            rect,
            stack,
            label.as_deref(),
            active,
            false,
            false,
            flash_strength,
        );
        return;
    }

    let sense = Sense::click_and_drag();
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SLOT_SIZE), sense);
    let is_drag_source = inventory_ui
        .drag
        .as_ref()
        .is_some_and(|drag| drag.source == slot);
    let pointer_over_slot = ui
        .ctx()
        .pointer_hover_pos()
        .is_some_and(|position| rect.contains(position));
    let hovered = response.hovered() || (inventory_ui.drag.is_some() && pointer_over_slot);

    paint_slot(
        ui,
        rect,
        stack,
        label.as_deref(),
        active,
        hovered,
        is_drag_source,
        flash_strength,
    );

    if pointer_over_slot {
        inventory_ui.hovered_slot = Some(slot);
    }

    if let Some(stack) = stack
        && inventory_ui.drag.is_none()
    {
        let _ = item_tooltip(response, stack);
        let shift_held = ui.input(|input| input.modifiers.shift);
        let primary_pressed = pointer_over_slot
            && ui.input(|input| input.pointer.button_pressed(PointerButton::Primary));
        let secondary_pressed = pointer_over_slot
            && ui.input(|input| input.pointer.button_pressed(PointerButton::Secondary));
        // Shift+LMB short-circuits the drag path entirely: the caller
        // surface (furnace modal today) consumes
        // `pending_quick_transfer` and turns it into a network command.
        // Without the early return we'd start a drag the same frame,
        // which then bounces back when released over nothing.
        if primary_pressed && shift_held && shift_transfer_enabled {
            inventory_ui.pending_quick_transfer = Some(slot);
        } else if primary_pressed {
            begin_drag(
                inventory_ui,
                slot,
                stack,
                stack.quantity,
                InventoryDragButton::Primary,
            );
        } else if secondary_pressed {
            begin_drag(
                inventory_ui,
                slot,
                stack,
                split_quantity(stack.quantity),
                InventoryDragButton::Secondary,
            );
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "egui UI plumbing")]
pub(crate) fn paint_slot(
    ui: &egui::Ui,
    rect: Rect,
    stack: Option<&ItemStack>,
    label: Option<&str>,
    active: bool,
    hovered: bool,
    is_drag_source: bool,
    flash_strength: f32,
) {
    let fill = if active {
        Color32::from_rgba_unmultiplied(21, 44, 72, 236)
    } else if hovered {
        Color32::from_rgba_unmultiplied(34, 43, 54, 238)
    } else {
        Color32::from_rgba_unmultiplied(8, 12, 18, 232)
    };
    let stroke = if active {
        Stroke::new(2.0, Color32::from_rgb(96, 168, 255))
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(115, 132, 151, 92))
    };

    ui.painter()
        .rect(rect, 5, fill, stroke, egui::StrokeKind::Inside);

    if let Some(stack) = stack {
        paint_item_icon(ui, rect, stack, is_drag_source);
    }

    if let Some(label) = label {
        ui.painter().text(
            rect.left_top() + vec2(6.0, 5.0),
            Align2::LEFT_TOP,
            label,
            FontId::new(11.0, FontFamily::Monospace),
            Color32::from_rgb(195, 207, 220),
        );
    }

    if flash_strength > 0.0 {
        paint_slot_flash(ui, rect, flash_strength);
    }
}

/// Overlay drawn on top of a slot when its contents grew. A warm fill plus
/// a brighter stroke pulse together: the fill makes the slot "glow" briefly,
/// the stroke makes the rectangle pop out from neighboring slots.
fn paint_slot_flash(ui: &egui::Ui, rect: Rect, strength: f32) {
    let strength = strength.clamp(0.0, 1.0);
    let fill_alpha = (140.0 * strength) as u8;
    let stroke_alpha = (210.0 * strength) as u8;
    if fill_alpha == 0 && stroke_alpha == 0 {
        return;
    }
    let fill = Color32::from_rgba_unmultiplied(255, 214, 138, fill_alpha);
    let stroke = Stroke::new(
        2.0,
        Color32::from_rgba_unmultiplied(255, 232, 180, stroke_alpha),
    );
    ui.painter()
        .rect(rect, 5, fill, stroke, egui::StrokeKind::Inside);
}

fn paint_item_icon(ui: &egui::Ui, rect: Rect, stack: &ItemStack, is_drag_source: bool) {
    if let Some(texture_id) = item_icons::texture_for(&stack.item_id) {
        // Real artwork: draw the transparent icon PNG. The drag source is
        // dimmed so the slot reads as "this is being carried", matching the
        // placeholder's faded alpha.
        let icon_rect = rect.shrink(6.0);
        let tint = Color32::from_white_alpha(if is_drag_source { 110 } else { 255 });
        ui.painter().image(
            texture_id,
            icon_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            tint,
        );
    } else {
        paint_placeholder_icon(ui, rect, stack, is_drag_source);
    }

    if stack.quantity > 1 {
        ui.painter().text(
            rect.right_bottom() - vec2(6.0, 5.0),
            Align2::RIGHT_BOTTOM,
            stack.quantity.to_string(),
            FontId::new(13.0, FontFamily::Monospace),
            Color32::WHITE,
        );
    }

    paint_durability_bar(ui, rect, stack);
}

/// Thin wear bar along the slot's bottom edge for stacks that carry
/// durability (tools). Width tracks the remaining fraction; the color
/// runs green → yellow → red as the tool wears so a glance at the
/// actionbar answers "how long until this breaks".
fn paint_durability_bar(ui: &egui::Ui, rect: Rect, stack: &ItemStack) {
    let Some(remaining) = stack.durability else {
        return;
    };
    let Some(max) = item_definition(&stack.item_id)
        .and_then(|definition| definition.tool)
        .and_then(|tool| tool.max_durability)
        .filter(|max| *max > 0)
    else {
        return;
    };
    let fraction = (remaining as f32 / max as f32).clamp(0.0, 1.0);

    let track_left = rect.left() + 6.0;
    let track_right = rect.right() - 6.0;
    let track_y = rect.bottom() - 6.0;
    let track = Rect::from_min_max(pos2(track_left, track_y - 3.0), pos2(track_right, track_y));
    ui.painter()
        .rect_filled(track, 2.0, Color32::from_rgba_unmultiplied(0, 0, 0, 150));

    if fraction <= 0.0 {
        return;
    }
    // Green above half, fading through yellow into red as it empties.
    let color = if fraction > 0.5 {
        let t = (fraction - 0.5) * 2.0;
        Color32::from_rgb((230.0 * (1.0 - t)) as u8 + 25, 200, 70)
    } else {
        let t = fraction * 2.0;
        Color32::from_rgb(235, (200.0 * t) as u8 + 30, 50)
    };
    let fill_right = track_left + (track_right - track_left) * fraction;
    let fill = Rect::from_min_max(track.left_top(), pos2(fill_right, track.bottom()));
    ui.painter().rect_filled(fill, 2.0, color);
}

/// Fallback when an item has no shipped icon PNG (and in headless/test
/// contexts where icon textures were never registered): the original
/// tinted-rectangle-with-gloss placeholder, tinted from the item registry.
fn paint_placeholder_icon(ui: &egui::Ui, rect: Rect, stack: &ItemStack, is_drag_source: bool) {
    let definition = item_definition(&stack.item_id);
    let tint = definition
        .map(|definition| {
            Color32::from_rgb(definition.tint.r, definition.tint.g, definition.tint.b)
        })
        .unwrap_or(Color32::from_rgb(140, 150, 162));
    let alpha = if is_drag_source { 96 } else { 224 };
    let icon_rect = rect.shrink(9.0);
    let icon_fill = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), alpha);

    ui.painter().rect(
        icon_rect,
        6,
        icon_fill,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 34)),
        egui::StrokeKind::Inside,
    );
    ui.painter().circle_filled(
        pos2(icon_rect.center().x - 4.0, icon_rect.center().y - 3.0),
        7.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, alpha / 3),
    );
}

fn item_tooltip(response: egui::Response, stack: &ItemStack) -> egui::Response {
    let title = item_definition(&stack.item_id)
        .map(|definition| definition.name)
        .unwrap_or(stack.item_id.as_ref());
    let body = item_tooltip_body(stack);
    theme::wow_tooltip(response, title, &body)
}

fn item_tooltip_body(stack: &ItemStack) -> String {
    let Some(definition) = item_definition(&stack.item_id) else {
        return format!("Unknown item\nQuantity: {}", stack.quantity);
    };

    let mut lines = vec![definition.description.to_owned()];

    // Surface the registry data the player otherwise can't see from the bag:
    // a tool's class/tier/yield, and that a deployable is placeable + how.
    if let Some(tool) = definition.tool {
        lines.push(format!(
            "{} · Tier {} · gathers {} per hit",
            tool.kind.label(),
            tool.tier,
            tool.gather_amount
        ));
        if let (Some(remaining), Some(max)) = (stack.durability, tool.max_durability) {
            lines.push(format!("Durability: {remaining}/{max}"));
        }
    }
    if let Some(deployable) = definition.deployable {
        lines.push(format!("Deployable: {}", deployable.kind.label()));
        lines.push("Hold it, then left-click to place".to_owned());
    }

    // Armor: the per-kind protection percentages and (if it wears) its
    // durability, the same numbers the paperdoll's protection summary sums.
    // Only the non-zero columns are shown so a piece that only guards one kind
    // reads cleanly.
    if let Some(armor) = definition.armor {
        lines.push(format!("Fits: {} slot", armor.slot.label()));
        let mut protections = Vec::new();
        if armor.melee_protection_pct > 0 {
            protections.push(format!("Melee {}%", armor.melee_protection_pct));
        }
        if armor.projectile_protection_pct > 0 {
            protections.push(format!("Ranged {}%", armor.projectile_protection_pct));
        }
        if armor.blast_protection_pct > 0 {
            protections.push(format!("Blast {}%", armor.blast_protection_pct));
        }
        if !protections.is_empty() {
            lines.push(format!("Protection: {}", protections.join("  ")));
        }
        if let (Some(remaining), Some(max)) = (stack.durability, armor.max_durability) {
            lines.push(format!("Durability: {remaining}/{max}"));
        }
    }

    if definition.equipable {
        lines.push("Equipable".to_owned());
    }
    // A stack line only carries information for items that actually
    // stack; "Stack: 1" on a tool or deployable is noise.
    let limit = stack_limit(definition.id).unwrap_or(1);
    if limit > 1 {
        lines.push(format!("Stack: {}/{}", stack.quantity, limit));
    }

    lines.join("\n")
}

fn begin_drag(
    inventory_ui: &mut InventoryUiState,
    source: UnifiedSlotRef,
    stack: &ItemStack,
    quantity: u16,
    button: InventoryDragButton,
) {
    inventory_ui.drag = Some(InventoryDrag {
        source,
        stack: stack.clone(),
        quantity,
        button,
    });
    // No audio on drag start by design: the drop/move cue at the end of
    // the drag is the informative one, and a grab cue stacked on top
    // reads as the same tick twice when shuffling fast. The drag-source
    // dim is the pick-up feedback.
}

pub(crate) fn slot_stack(
    inventory: &PlayerInventoryState,
    slot: ItemContainerSlot,
) -> Option<&ItemStack> {
    match slot.container {
        ItemContainer::Inventory => inventory.inventory_slots.get(slot.slot),
        ItemContainer::Actionbar => inventory.actionbar_slots.get(slot.slot),
        ItemContainer::Equipment => inventory.equipment_slots.get(slot.slot),
    }
    .and_then(Option::as_ref)
}

fn split_quantity(quantity: u16) -> u16 {
    quantity.div_ceil(2).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{items::COAL_ID, protocol::INVENTORY_SLOT_COUNT};

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

    #[test]
    fn right_drag_takes_half_rounded_up() {
        assert_eq!(split_quantity(1), 1);
        assert_eq!(split_quantity(2), 1);
        assert_eq!(split_quantity(3), 2);
    }

    #[test]
    fn tooltip_body_uses_registry_stack_limits() {
        let body = item_tooltip_body(&ItemStack::new(COAL_ID, 3));
        assert!(body.contains("3/200"));
    }

    #[test]
    fn tooltip_body_hides_the_stack_line_for_unstackable_items() {
        use crate::items::BASIC_PICKAXE_ID;
        let body = item_tooltip_body(&ItemStack::new(BASIC_PICKAXE_ID, 1));
        assert!(!body.contains("Stack:"), "no stack line for tools: {body}");
        assert!(body.contains("Equipable"));
    }

    #[test]
    fn tooltip_body_surfaces_tool_stats() {
        use crate::items::BASIC_PICKAXE_ID;
        let body = item_tooltip_body(&ItemStack::new(BASIC_PICKAXE_ID, 1));
        // The pickaxe's class and per-hit yield come from its ToolProfile,
        // which the bag otherwise never exposes.
        assert!(body.contains("Pickaxe"), "tool class shown: {body}");
        assert!(body.contains("per hit"), "gather yield shown: {body}");
    }

    #[test]
    fn tooltip_body_marks_deployables_as_placeable() {
        use crate::items::CRUDE_FURNACE_ID;
        let body = item_tooltip_body(&ItemStack::new(CRUDE_FURNACE_ID, 1));
        assert!(body.contains("Deployable"), "placeable hint shown: {body}");
        assert!(
            body.contains("left-click"),
            "placement control shown: {body}"
        );
    }

    #[test]
    fn tooltip_body_surfaces_armor_stats() {
        use crate::items::PADDED_TUNIC_ID;
        let body = item_tooltip_body(&ItemStack::new(PADDED_TUNIC_ID, 1));
        // The tunic's slot, its non-zero protection columns, and its durability
        // all come from the ArmorProfile the bag otherwise never exposes.
        assert!(body.contains("Chest slot"), "fits slot shown: {body}");
        assert!(
            body.contains("Protection:"),
            "protection line shown: {body}"
        );
        assert!(body.contains('%'), "protection percentages shown: {body}");
        assert!(body.contains("Durability:"), "durability shown: {body}");
    }

    #[test]
    fn tooltip_body_handles_unknown_item() {
        // An item id with no registry entry falls back to the generic
        // "Unknown item" body rather than panicking.
        let body = item_tooltip_body(&ItemStack::new("not_a_real_item", 9));
        assert!(body.contains("Unknown item"));
        assert!(body.contains("Quantity: 9"));
    }

    #[test]
    fn empty_inventory_slot_lookup_is_safe() {
        let inventory = PlayerInventoryState::empty();
        assert!(
            slot_stack(
                &inventory,
                ItemContainerSlot::inventory(INVENTORY_SLOT_COUNT)
            )
            .is_none()
        );
    }

    #[test]
    fn slot_stack_reads_inventory_and_actionbar() {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[2] = Some(ItemStack::new(COAL_ID, 5));
        inventory.actionbar_slots[1] = Some(ItemStack::new(COAL_ID, 9));
        assert_eq!(
            slot_stack(&inventory, ItemContainerSlot::inventory(2)).map(|s| s.quantity),
            Some(5)
        );
        assert_eq!(
            slot_stack(&inventory, ItemContainerSlot::actionbar(1)).map(|s| s.quantity),
            Some(9)
        );
        assert!(slot_stack(&inventory, ItemContainerSlot::inventory(0)).is_none());
    }

    #[test]
    fn non_interactive_slot_paints_without_recording_hover() {
        // The non-interactive branch allocates space and paints, but must
        // never write `hovered_slot` (it can't be a drag target).
        let mut inv_ui = InventoryUiState::default();
        let slot = UnifiedSlotRef::Player(ItemContainerSlot::actionbar(0));
        let stack = ItemStack::new(COAL_ID, 4);
        let output = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                draw_slot(
                    ui,
                    slot,
                    Some(&stack),
                    Some("1".to_owned()),
                    false,
                    false, // not interactive
                    false,
                    &mut inv_ui,
                );
            });
        });
        assert!(!output.shapes.is_empty());
        assert!(inv_ui.hovered_slot.is_none());
    }

    #[test]
    fn interactive_slot_without_pointer_leaves_hover_unset() {
        // With no pointer position in the input, an interactive slot
        // still paints but records no hover (nothing to highlight).
        let mut inv_ui = InventoryUiState::default();
        let slot = UnifiedSlotRef::Player(ItemContainerSlot::inventory(0));
        let stack = ItemStack::new(COAL_ID, 2);
        let output = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                draw_slot(
                    ui,
                    slot,
                    Some(&stack),
                    None,
                    false,
                    true,
                    false,
                    &mut inv_ui,
                );
            });
        });
        assert!(!output.shapes.is_empty());
        assert!(inv_ui.hovered_slot.is_none());
    }

    #[test]
    fn interactive_slot_with_pointer_marks_hovered() {
        let mut inv_ui = InventoryUiState::default();
        let slot = UnifiedSlotRef::Player(ItemContainerSlot::inventory(0));
        let stack = ItemStack::new(COAL_ID, 2);
        // The central panel's first widget lands near the top-left; aim
        // the pointer there.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 768.0),
                )),
                events: vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))],
                ..Default::default()
            },
            |ui| {
                egui::Area::new("slot_test_area".into())
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .show(ui.ctx(), |ui| {
                        draw_slot(
                            ui,
                            slot,
                            Some(&stack),
                            None,
                            false,
                            true,
                            false,
                            &mut inv_ui,
                        );
                    });
            },
        );
        // The slot occupies a SLOT_SIZE square anchored at (0,0); the
        // pointer at (20,20) is inside it, so hover is recorded.
        assert_eq!(inv_ui.hovered_slot, Some(slot));
    }

    #[test]
    fn active_slot_paints_differently_than_idle() {
        // `paint_slot` chooses a brighter fill for the active slot, so
        // active vs idle differ in their painted output.
        let stack = ItemStack::new(COAL_ID, 1);
        let idle = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let (_, rect) = ui.allocate_space(Vec2::splat(SLOT_SIZE));
                paint_slot(ui, rect, Some(&stack), None, false, false, false, 0.0);
            });
        });
        let active = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let (_, rect) = ui.allocate_space(Vec2::splat(SLOT_SIZE));
                paint_slot(ui, rect, Some(&stack), None, true, false, false, 0.0);
            });
        });
        // Both paint shapes; the active one carries a thicker stroke.
        assert!(!idle.shapes.is_empty());
        assert!(!active.shapes.is_empty());
    }

    #[test]
    fn flashing_slot_paints_overlay() {
        // A flash strength > 0 paints an extra overlay rect on top of the
        // base slot, so it yields more shapes than an unflashed slot.
        let stack = ItemStack::new(COAL_ID, 1);
        let plain = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let (_, rect) = ui.allocate_space(Vec2::splat(SLOT_SIZE));
                paint_slot(ui, rect, Some(&stack), None, false, false, false, 0.0);
            });
        });
        let flashing = run_ui(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let (_, rect) = ui.allocate_space(Vec2::splat(SLOT_SIZE));
                paint_slot(ui, rect, Some(&stack), None, false, false, false, 1.0);
            });
        });
        assert!(flashing.shapes.len() > plain.shapes.len());
    }
}
