use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::protocol::{ItemContainerSlot, ItemStack, PlayerInventoryState};

/// One audible inventory change. Returned by [`InventoryUiState::observe_inventory`]
/// so the UI layer can play the matching cue without re-diffing the
/// snapshot. The variants are mutually exclusive; ties go to whichever
/// change is most informative — gains beat losses, losses beat shuffles —
/// since a tick that did all three at once is dominated by the "new item
/// arrived" cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InventorySoundEvent {
    /// Total inventory quantity grew — an item entered the player's bag.
    Pickup,
    /// Total inventory quantity shrank — an item left the bag (drop, use).
    Drop,
    /// Per-slot contents changed but the total stayed the same — a move,
    /// swap, or partial-stack split inside the grid.
    Move,
}

/// Duration of the "you just got items in this slot" highlight. Long enough
/// to notice in peripheral vision when picking up multiple items in rapid
/// succession; short enough to not feel laggy.
pub(crate) const SLOT_FLASH_DURATION_SECS: f32 = 0.55;

#[derive(Resource, Default)]
pub(crate) struct InventoryUiState {
    pub(crate) drag: Option<InventoryDrag>,
    pub(crate) hovered_slot: Option<ItemContainerSlot>,
    pub(crate) inventory_rect: Option<egui::Rect>,
    pub(crate) actionbar_rect: Option<egui::Rect>,
    pub(crate) was_open: bool,
    /// Per-slot flash elapsed time. A slot is inserted with elapsed = 0
    /// whenever its quantity grows (or a new stack lands in an empty slot)
    /// and is removed once the elapsed time passes [`SLOT_FLASH_DURATION_SECS`].
    pub(crate) slot_flashes: HashMap<ItemContainerSlot, f32>,
    /// The most recent inventory observed from the snapshot. Used to detect
    /// when items have entered a slot so a flash can be queued. Stored as
    /// the full state because comparing slot-by-slot in a single pass is
    /// faster than maintaining a parallel slot map.
    pub(crate) last_seen_inventory: Option<PlayerInventoryState>,
}

impl InventoryUiState {
    pub(crate) fn begin_frame(&mut self) {
        self.hovered_slot = None;
        self.inventory_rect = None;
        self.actionbar_rect = None;
    }

    pub(crate) fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// Tick flash timers forward and drop any that have completed.
    pub(crate) fn tick_slot_flashes(&mut self, delta_seconds: f32) {
        let delta = delta_seconds.max(0.0);
        if delta == 0.0 || self.slot_flashes.is_empty() {
            return;
        }
        self.slot_flashes.retain(|_, elapsed| {
            *elapsed += delta;
            *elapsed < SLOT_FLASH_DURATION_SECS
        });
    }

    /// Diff `inventory` against [`Self::last_seen_inventory`] and start a
    /// flash on every slot that gained items (newly filled, item swap, or
    /// quantity increase). Drag-driven moves that just shuffle items
    /// between slots also flash the destination, which reads correctly as
    /// "items just landed here".
    ///
    /// Returns an [`InventorySoundEvent`] describing the most-informative
    /// change since the previous observation, or `None` if nothing changed
    /// or this is the seeding observation. The very first observation
    /// after (re)connecting never reports a sound, since "every slot
    /// gained the items it had before disconnect" is not a real pickup.
    pub(crate) fn observe_inventory(
        &mut self,
        inventory: &PlayerInventoryState,
    ) -> Option<InventorySoundEvent> {
        let last = self.last_seen_inventory.take();
        let mut event = None;
        if let Some(previous) = &last {
            for (index, current) in inventory.inventory_slots.iter().enumerate() {
                let previous_stack = previous.inventory_slots.get(index).and_then(Option::as_ref);
                if stack_gained_items(previous_stack, current.as_ref()) {
                    self.slot_flashes
                        .insert(ItemContainerSlot::inventory(index), 0.0);
                }
            }
            for (index, current) in inventory.actionbar_slots.iter().enumerate() {
                let previous_stack = previous.actionbar_slots.get(index).and_then(Option::as_ref);
                if stack_gained_items(previous_stack, current.as_ref()) {
                    self.slot_flashes
                        .insert(ItemContainerSlot::actionbar(index), 0.0);
                }
            }
            event = inventory_sound_event(previous, inventory);
        }
        self.last_seen_inventory = Some(inventory.clone());
        event
    }

    /// Returns the flash strength for `slot`, with 1.0 right after the
    /// trigger and 0.0 at the end of the fade window. Uses an ease-out
    /// curve so the bright instant is short and the fade lingers a little
    /// — natural attention-grabbing without being garish.
    pub(crate) fn slot_flash_strength(&self, slot: ItemContainerSlot) -> f32 {
        let Some(elapsed) = self.slot_flashes.get(&slot) else {
            return 0.0;
        };
        let progress = (*elapsed / SLOT_FLASH_DURATION_SECS).clamp(0.0, 1.0);
        (1.0 - progress).powi(2)
    }

    /// Drop any tracked state — call this when the player disconnects so
    /// stale slots from the previous session don't bleed into the next one.
    pub(crate) fn clear_inventory_tracking(&mut self) {
        self.slot_flashes.clear();
        self.last_seen_inventory = None;
    }
}

fn inventory_sound_event(
    previous: &PlayerInventoryState,
    current: &PlayerInventoryState,
) -> Option<InventorySoundEvent> {
    let previous_total = total_quantity(previous);
    let current_total = total_quantity(current);
    if current_total > previous_total {
        Some(InventorySoundEvent::Pickup)
    } else if current_total < previous_total {
        Some(InventorySoundEvent::Drop)
    } else if slots_rearranged(previous, current) {
        Some(InventorySoundEvent::Move)
    } else {
        None
    }
}

fn total_quantity(inventory: &PlayerInventoryState) -> u64 {
    let sum_slots = |slots: &[Option<ItemStack>]| -> u64 {
        slots
            .iter()
            .flatten()
            .map(|stack| u64::from(stack.quantity))
            .sum()
    };
    sum_slots(&inventory.inventory_slots) + sum_slots(&inventory.actionbar_slots)
}

fn slots_rearranged(previous: &PlayerInventoryState, current: &PlayerInventoryState) -> bool {
    fn diff(previous: &[Option<ItemStack>], current: &[Option<ItemStack>]) -> bool {
        previous
            .iter()
            .zip(current.iter())
            .any(|(p, c)| !stacks_equal(p.as_ref(), c.as_ref()))
    }
    diff(&previous.inventory_slots, &current.inventory_slots)
        || diff(&previous.actionbar_slots, &current.actionbar_slots)
}

fn stacks_equal(previous: Option<&ItemStack>, current: Option<&ItemStack>) -> bool {
    match (previous, current) {
        (None, None) => true,
        (Some(a), Some(b)) => a.item_id == b.item_id && a.quantity == b.quantity,
        _ => false,
    }
}

fn stack_gained_items(previous: Option<&ItemStack>, current: Option<&ItemStack>) -> bool {
    match (previous, current) {
        (_, None) => false,
        (None, Some(current)) => current.quantity > 0,
        (Some(previous), Some(current)) => {
            previous.item_id != current.item_id || current.quantity > previous.quantity
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InventoryDrag {
    pub(crate) source: ItemContainerSlot,
    pub(crate) stack: ItemStack,
    pub(crate) quantity: u16,
    pub(crate) button: InventoryDragButton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InventoryDragButton {
    Primary,
    Secondary,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ItemContainerSlot, ItemStack};

    #[test]
    fn inventory_ui_state_resets_frame_and_drag_state() {
        let mut state = InventoryUiState {
            drag: Some(InventoryDrag {
                source: ItemContainerSlot::inventory(2),
                stack: ItemStack::new("ore", 4),
                quantity: 2,
                button: InventoryDragButton::Secondary,
            }),
            hovered_slot: Some(ItemContainerSlot::actionbar(1)),
            inventory_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(10.0, 10.0),
            )),
            actionbar_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(5.0, 5.0),
            )),
            was_open: true,
            slot_flashes: HashMap::new(),
            last_seen_inventory: None,
        };

        state.begin_frame();

        assert!(state.hovered_slot.is_none());
        assert!(state.inventory_rect.is_none());
        assert!(state.actionbar_rect.is_none());
        assert!(state.drag.is_some());
        assert!(state.was_open);

        state.cancel_drag();
        assert!(state.drag.is_none());
    }

    #[test]
    fn observe_inventory_flashes_slots_that_just_gained_items() {
        let mut state = InventoryUiState::default();

        let mut first = PlayerInventoryState::empty();
        first.actionbar_slots[0] = Some(ItemStack::new("hatchet", 1));
        state.observe_inventory(&first);
        // The first observation seeds the baseline — nothing should flash.
        assert!(state.slot_flashes.is_empty());

        let mut second = first.clone();
        second.inventory_slots[3] = Some(ItemStack::new("coal", 4));
        second.actionbar_slots[0] = Some(ItemStack::new("hatchet", 1));
        state.observe_inventory(&second);
        assert!(
            state
                .slot_flashes
                .contains_key(&ItemContainerSlot::inventory(3)),
            "newly filled inventory slot should flash"
        );
        assert!(
            !state
                .slot_flashes
                .contains_key(&ItemContainerSlot::actionbar(0)),
            "unchanged actionbar slot should not flash"
        );

        let mut third = second.clone();
        third.inventory_slots[3] = Some(ItemStack::new("coal", 9));
        state.observe_inventory(&third);
        assert!(
            state
                .slot_flashes
                .contains_key(&ItemContainerSlot::inventory(3))
        );
    }

    #[test]
    fn observe_inventory_classifies_total_quantity_changes() {
        let mut state = InventoryUiState::default();

        // Seed baseline — no event on the first observation.
        let baseline = PlayerInventoryState::empty();
        assert_eq!(state.observe_inventory(&baseline), None);

        // Quantity grew → Pickup.
        let mut after_pickup = baseline.clone();
        after_pickup.inventory_slots[0] = Some(ItemStack::new("coal", 3));
        assert_eq!(
            state.observe_inventory(&after_pickup),
            Some(InventorySoundEvent::Pickup)
        );

        // Stack consolidated into the same total → Move (same total,
        // different per-slot contents).
        let mut after_move = after_pickup.clone();
        after_move.inventory_slots[0] = None;
        after_move.actionbar_slots[0] = Some(ItemStack::new("coal", 3));
        assert_eq!(
            state.observe_inventory(&after_move),
            Some(InventorySoundEvent::Move)
        );

        // Same snapshot again → no event.
        assert_eq!(state.observe_inventory(&after_move), None);

        // Quantity shrank → Drop.
        let mut after_drop = after_move.clone();
        after_drop.actionbar_slots[0] = Some(ItemStack::new("coal", 1));
        assert_eq!(
            state.observe_inventory(&after_drop),
            Some(InventorySoundEvent::Drop)
        );
    }

    #[test]
    fn observe_inventory_returns_none_when_nothing_changed() {
        let mut state = InventoryUiState::default();
        let snapshot = PlayerInventoryState::empty();
        assert_eq!(state.observe_inventory(&snapshot), None);
        assert_eq!(state.observe_inventory(&snapshot), None);
    }

    #[test]
    fn slot_flash_strength_eases_out_over_duration() {
        let mut state = InventoryUiState::default();
        state
            .slot_flashes
            .insert(ItemContainerSlot::inventory(0), 0.0);

        let start = state.slot_flash_strength(ItemContainerSlot::inventory(0));
        state.tick_slot_flashes(SLOT_FLASH_DURATION_SECS * 0.5);
        let mid = state.slot_flash_strength(ItemContainerSlot::inventory(0));
        state.tick_slot_flashes(SLOT_FLASH_DURATION_SECS);
        let after = state.slot_flash_strength(ItemContainerSlot::inventory(0));

        assert!(start > mid);
        assert!(mid > 0.0);
        assert_eq!(after, 0.0);
    }
}
