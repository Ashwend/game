use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::protocol::{
    EQUIPMENT_SLOT_COUNT, FurnaceSlotRef, ItemContainerSlot, ItemStack, LootBagSlotRef,
    PlayerInventoryState,
};

/// Either-or addressable slot used by the unified drag pipeline. The
/// main inventory, furnace, and loot-bag UIs all speak this type so a
/// drag originating in any container can be dropped on a slot in any
/// container; the dispatch in `handle_drag_release` translates it
/// back into the matching wire command:
///   - `InventoryCommand::Move` for player↔player,
///   - `FurnaceCommand::Move` for anything touching a furnace slot,
///   - `LootBagCommand::Move` for anything touching a bag slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum UnifiedSlotRef {
    /// A slot in the player's own inventory or actionbar.
    Player(ItemContainerSlot),
    /// A slot inside the currently-open furnace (fuel slot or one of
    /// the smelt-grid slots).
    Furnace(FurnaceSlotRef),
    /// A slot inside the currently-open loot bag.
    Bag(usize),
}

impl UnifiedSlotRef {
    pub(crate) fn is_player(self) -> bool {
        matches!(self, Self::Player(_))
    }

    /// True when this ref points at a bag slot. Used by the drag
    /// release dispatcher to pick the right wire command.
    pub(crate) fn is_bag(self) -> bool {
        matches!(self, Self::Bag(_))
    }

    /// Map this unified ref to its `FurnaceSlotRef` form. Player slots
    /// pass through via the matching `FurnaceSlotRef::PlayerInventory`
    /// / `FurnaceSlotRef::PlayerActionbar` variants so a cross-
    /// container move can be expressed as one `FurnaceCommand::Move`.
    pub(crate) fn as_furnace_ref(self) -> FurnaceSlotRef {
        match self {
            Self::Furnace(slot) => slot,
            Self::Player(slot) => match slot.container {
                crate::protocol::ItemContainer::Inventory => {
                    FurnaceSlotRef::PlayerInventory(slot.slot)
                }
                crate::protocol::ItemContainer::Actionbar => {
                    FurnaceSlotRef::PlayerActionbar(slot.slot)
                }
                // Equipment (paperdoll) slots don't map onto a furnace command;
                // the furnace UI never surfaces a paperdoll slot as a drag
                // target, so this is unreachable in practice. Fail closed to the
                // fuel sentinel so a misroute is rejected server-side rather than
                // silently smelting a worn piece.
                crate::protocol::ItemContainer::Equipment => FurnaceSlotRef::Fuel,
            },
            // Bag → furnace is not a valid move; the caller should
            // never reach here. Return a sentinel so a misroute fails
            // closed (the server rejects unknown slots) rather than
            // open.
            Self::Bag(_) => FurnaceSlotRef::Fuel,
        }
    }

    /// Same idea for [`LootBagSlotRef`]. Player slots map through the
    /// matching variants so a player→bag drag is one
    /// `LootBagCommand::Move`. Furnace slots can't reach a bag
    /// command, the bag UI is mutually exclusive with the furnace
    /// modal, so the unreachable branch is OK as a fallback.
    pub(crate) fn as_loot_bag_ref(self) -> LootBagSlotRef {
        match self {
            Self::Bag(index) => LootBagSlotRef::Bag(index),
            Self::Player(slot) => match slot.container {
                crate::protocol::ItemContainer::Inventory => {
                    LootBagSlotRef::PlayerInventory(slot.slot)
                }
                crate::protocol::ItemContainer::Actionbar => {
                    LootBagSlotRef::PlayerActionbar(slot.slot)
                }
                // Paperdoll slots don't map onto a loot-bag command (the bag UI
                // never exposes a paperdoll drag target). Fail closed to a bag
                // sentinel so a misroute is rejected rather than moving a worn
                // piece into a bag through the wrong command family.
                crate::protocol::ItemContainer::Equipment => LootBagSlotRef::Bag(0),
            },
            Self::Furnace(_) => LootBagSlotRef::Bag(0),
        }
    }
}

/// One audible inventory change. Returned by [`InventoryUiState::observe_inventory`]
/// so the UI layer can play the matching cue without re-diffing the
/// snapshot. The variants are mutually exclusive; ties go to whichever
/// change is most informative, gains beat losses, losses beat shuffles,
/// since a tick that did all three at once is dominated by the "new item
/// arrived" cue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InventorySoundEvent {
    /// Total inventory quantity grew, an item entered the player's bag.
    /// Carries the id of the item that gained the most units this diff so
    /// the cue can match the material (stick clatter, stone clack) instead
    /// of one generic rustle; `None` when the gaining slot is ambiguous.
    Pickup {
        item_id: Option<crate::items::ItemId>,
    },
    /// Total inventory quantity shrank, an item left the bag (drop, use).
    Drop,
    /// Per-slot contents changed but the total stayed the same, a move,
    /// swap, or partial-stack split inside the grid.
    Move,
}

/// Duration of the "you just got items in this slot" highlight. Long enough
/// to notice in peripheral vision when picking up multiple items in rapid
/// succession; short enough to not feel laggy.
pub(crate) const SLOT_FLASH_DURATION_SECS: f32 = 0.55;

/// How long after the client sends a pickup command an inventory gain
/// still counts as "the player picked something up." Anything beyond this
/// window is treated as a harvest payout or a server-side grant and stays
/// silent, pressing E is the only thing that should trigger the pickup
/// cue. Covers a typical loopback round-trip plus a snapshot interval
/// with margin to spare.
const PICKUP_INTENT_WINDOW_SECS: f32 = 0.6;

#[derive(Resource, Default)]
pub(crate) struct InventoryUiState {
    pub(crate) drag: Option<InventoryDrag>,
    pub(crate) hovered_slot: Option<UnifiedSlotRef>,
    pub(crate) inventory_rect: Option<egui::Rect>,
    pub(crate) actionbar_rect: Option<egui::Rect>,
    /// Tracks the rect of any open furnace surface so a player-side
    /// drag released over the furnace doesn't fall through to the
    /// "drop on the ground" path. `None` when the furnace UI isn't
    /// up.
    pub(crate) furnace_rect: Option<egui::Rect>,
    /// Same purpose as `furnace_rect` but for the loot bag UI. A
    /// drag released inside the bag panel routes through the bag
    /// command path; released outside, it falls back to the
    /// standard inventory drop-on-ground.
    pub(crate) loot_bag_rect: Option<egui::Rect>,
    /// Rect of each worn-armor (paperdoll) slot, indexed by
    /// [`crate::protocol::EquipmentSlot::index`]. Registered per frame while the
    /// Inventory tab is up so a drag released over a paperdoll slot counts as
    /// landing on an inventory surface (never falls through to drop-on-ground).
    /// Every entry is `None` on the Crafting tab or with the panel closed.
    pub(crate) equipment_rects: [Option<egui::Rect>; EQUIPMENT_SLOT_COUNT],
    /// Single-frame inbox for shift+click "quick transfer" intents.
    /// The slot widget sets this when the player Shift+LMBs a slot
    /// while a container surface (furnace today) is up; the container's
    /// UI consumes it, sends the network command, and clears the
    /// field. Cleared at frame start by [`begin_frame`] so a stale
    /// click from a previous frame can never fire twice.
    pub(crate) pending_quick_transfer: Option<UnifiedSlotRef>,
    /// Whether the unified inventory + crafting panel was open last frame
    /// (either tab). The open->closed transition drops keyboard focus and
    /// cancels any in-progress drag.
    pub(crate) was_open: bool,
    /// Whether the panel was specifically on the Crafting tab last frame.
    /// Lets the panel drop a focused recipe-search text input when the
    /// player flips Crafting->Inventory, not just when the panel closes.
    pub(crate) was_crafting: bool,
    /// Whether the panel is showing the admin item-grant tab. Client-local
    /// VIEW state only: the panel-open source of truth stays the `MenuState`
    /// bools (`inventory_open` carries the admin tab), so every overlay /
    /// control gate keeps working unchanged. Reset when the panel closes (a
    /// reopen always lands on Inventory) and forced off for non-admins; the
    /// server independently rejects `/give` from non-admins regardless.
    pub(crate) admin_tab: bool,
    /// Per-slot flash elapsed time. A slot is inserted with elapsed = 0
    /// whenever its quantity grows (or a new stack lands in an empty slot)
    /// and is removed once the elapsed time passes [`SLOT_FLASH_DURATION_SECS`].
    pub(crate) slot_flashes: HashMap<ItemContainerSlot, f32>,
    /// The most recent inventory observed from the snapshot. Used to detect
    /// when items have entered a slot so a flash can be queued. Stored as
    /// the full state because comparing slot-by-slot in a single pass is
    /// faster than maintaining a parallel slot map.
    pub(crate) last_seen_inventory: Option<PlayerInventoryState>,
    /// Seconds remaining in the "the player just asked to pick something
    /// up" window. Set by [`Self::note_pickup_intent`] when a
    /// `PickUp`/`PickUpResourceNode` command goes out and counted down
    /// each frame. While positive, an inventory total increase is
    /// attributed to that pickup and consumes the timer; once it expires,
    /// inventory gains are silent (tool harvesting, server grants).
    pickup_intent_secs_remaining: f32,
    /// Same shape for losses: set by [`Self::note_drop_intent`] when the
    /// player explicitly throws something away with the drop shortcut (the
    /// one loss that happens with every item UI closed). While positive, a
    /// total decrease plays the drop cue even without an item UI up. All
    /// other audible losses happen inside an open item surface (drag-drop,
    /// container transfers, crafting), which
    /// [`Self::observe_inventory`] gates on directly, so server-side
    /// consumption (firing an arrow, a thrown bomb burning its stack)
    /// stays silent instead of clicking like a UI interaction.
    drop_intent_secs_remaining: f32,
}

impl InventoryUiState {
    pub(crate) fn begin_frame(&mut self) {
        self.hovered_slot = None;
        self.inventory_rect = None;
        self.actionbar_rect = None;
        self.furnace_rect = None;
        self.loot_bag_rect = None;
        self.equipment_rects = [None; EQUIPMENT_SLOT_COUNT];
        // Last frame's shift+click should have been consumed by now.
        // Clearing here makes the field strictly single-frame so a
        // surface that opens after the click was recorded can't pick up
        // a phantom intent.
        self.pending_quick_transfer = None;
    }

    pub(crate) fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// Tick flash timers forward and drop any that have completed.
    pub(crate) fn tick_slot_flashes(&mut self, delta_seconds: f32) {
        let delta = delta_seconds.max(0.0);
        if delta == 0.0 {
            return;
        }
        if !self.slot_flashes.is_empty() {
            self.slot_flashes.retain(|_, elapsed| {
                *elapsed += delta;
                *elapsed < SLOT_FLASH_DURATION_SECS
            });
        }
        if self.pickup_intent_secs_remaining > 0.0 {
            self.pickup_intent_secs_remaining =
                (self.pickup_intent_secs_remaining - delta).max(0.0);
        }
        if self.drop_intent_secs_remaining > 0.0 {
            self.drop_intent_secs_remaining = (self.drop_intent_secs_remaining - delta).max(0.0);
        }
    }

    /// Mark that the player just sent a pickup command. The next
    /// inventory total increase observed within
    /// [`PICKUP_INTENT_WINDOW_SECS`] is treated as the matching pickup and
    /// fires the cue; later increases (harvest payouts, server grants)
    /// stay silent.
    pub(crate) fn note_pickup_intent(&mut self) {
        self.pickup_intent_secs_remaining = PICKUP_INTENT_WINDOW_SECS;
    }

    /// Mark that the player just sent an explicit drop command (the drop
    /// shortcut). The next inventory total decrease observed within the
    /// intent window plays the drop cue even with every item UI closed.
    pub(crate) fn note_drop_intent(&mut self) {
        self.drop_intent_secs_remaining = PICKUP_INTENT_WINDOW_SECS;
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
    ///
    /// `item_ui_open` is whether any item surface (inventory/crafting
    /// panel, furnace, loot bag / container) is up this frame. The
    /// drop/move cues are UI-handling feedback, so they only play while the
    /// player is actually handling items there, or (for drops) within the
    /// [`Self::note_drop_intent`] window of the drop shortcut. Without the
    /// gate, every server-side consumption, a fired arrow, a thrown bomb, a
    /// crafted batch's inputs, clicked like a UI interaction mid-combat.
    pub(crate) fn observe_inventory(
        &mut self,
        inventory: &PlayerInventoryState,
        item_ui_open: bool,
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
            match event {
                // Inventory gains the player didn't ask for (harvest
                // payouts, admin grants) should stay silent. A real pickup
                // has a matching command sent within the last few frames;
                // consume the intent flag so a delayed harvest delta that
                // arrives after a successful pickup doesn't replay the cue.
                Some(InventorySoundEvent::Pickup { .. }) => {
                    if self.pickup_intent_secs_remaining > 0.0 {
                        self.pickup_intent_secs_remaining = 0.0;
                    } else {
                        event = None;
                    }
                }
                // Losses are audible only when the player is handling items
                // (an item UI is up) or just pressed the drop shortcut;
                // otherwise it's ammo/charge consumption, not a UI action.
                Some(InventorySoundEvent::Drop) => {
                    if self.drop_intent_secs_remaining > 0.0 {
                        self.drop_intent_secs_remaining = 0.0;
                    } else if !item_ui_open {
                        event = None;
                    }
                }
                // Shuffles only happen from an open item surface; a
                // server-side rearrangement with everything closed is not a
                // player action.
                Some(InventorySoundEvent::Move) => {
                    if !item_ui_open {
                        event = None;
                    }
                }
                None => {}
            }
        }
        self.last_seen_inventory = Some(inventory.clone());
        event
    }

    /// Returns the flash strength for `slot`, with 1.0 right after the
    /// trigger and 0.0 at the end of the fade window. Uses an ease-out
    /// curve so the bright instant is short and the fade lingers a little
    ///, natural attention-grabbing without being garish.
    pub(crate) fn slot_flash_strength(&self, slot: ItemContainerSlot) -> f32 {
        let Some(elapsed) = self.slot_flashes.get(&slot) else {
            return 0.0;
        };
        let progress = (*elapsed / SLOT_FLASH_DURATION_SECS).clamp(0.0, 1.0);
        (1.0 - progress).powi(2)
    }

    /// Drop any tracked state, call this when the player disconnects so
    /// stale slots from the previous session don't bleed into the next one.
    pub(crate) fn clear_inventory_tracking(&mut self) {
        self.slot_flashes.clear();
        self.last_seen_inventory = None;
        self.pickup_intent_secs_remaining = 0.0;
    }
}

fn inventory_sound_event(
    previous: &PlayerInventoryState,
    current: &PlayerInventoryState,
) -> Option<InventorySoundEvent> {
    let previous_total = total_quantity(previous);
    let current_total = total_quantity(current);
    if current_total > previous_total {
        Some(InventorySoundEvent::Pickup {
            item_id: dominant_gained_item(previous, current),
        })
    } else if current_total < previous_total {
        Some(InventorySoundEvent::Drop)
    } else if slots_rearranged(previous, current) {
        Some(InventorySoundEvent::Move)
    } else {
        None
    }
}

/// The item whose slot gained the most units between the two snapshots.
/// A pickup that splits across several slots reports the largest single
/// gain, good enough to pick the material cue. Slots whose item changed
/// entirely count their full new quantity as the gain.
fn dominant_gained_item(
    previous: &PlayerInventoryState,
    current: &PlayerInventoryState,
) -> Option<crate::items::ItemId> {
    let mut best: Option<(u16, crate::items::ItemId)> = None;
    let pairs = previous
        .inventory_slots
        .iter()
        .zip(current.inventory_slots.iter())
        .chain(
            previous
                .actionbar_slots
                .iter()
                .zip(current.actionbar_slots.iter()),
        );
    for (previous_stack, current_stack) in pairs {
        let Some(current_stack) = current_stack else {
            continue;
        };
        let previous_quantity = match previous_stack {
            Some(stack) if stack.item_id == current_stack.item_id => stack.quantity,
            _ => 0,
        };
        if current_stack.quantity > previous_quantity {
            let gain = current_stack.quantity - previous_quantity;
            if best.as_ref().is_none_or(|(top, _)| gain > *top) {
                best = Some((gain, current_stack.item_id.clone()));
            }
        }
    }
    best.map(|(_, item_id)| item_id)
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
    pub(crate) source: UnifiedSlotRef,
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
                source: UnifiedSlotRef::Player(ItemContainerSlot::inventory(2)),
                stack: ItemStack::new("ore", 4),
                quantity: 2,
                button: InventoryDragButton::Secondary,
            }),
            hovered_slot: Some(UnifiedSlotRef::Player(ItemContainerSlot::actionbar(1))),
            inventory_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(10.0, 10.0),
            )),
            actionbar_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(5.0, 5.0),
            )),
            furnace_rect: None,
            loot_bag_rect: None,
            equipment_rects: [None; EQUIPMENT_SLOT_COUNT],
            pending_quick_transfer: Some(UnifiedSlotRef::Player(ItemContainerSlot::inventory(0))),
            was_open: true,
            was_crafting: false,
            admin_tab: false,
            slot_flashes: HashMap::new(),
            last_seen_inventory: None,
            pickup_intent_secs_remaining: 0.0,
            drop_intent_secs_remaining: 0.0,
        };

        state.begin_frame();

        assert!(state.hovered_slot.is_none());
        assert!(state.inventory_rect.is_none());
        assert!(state.actionbar_rect.is_none());
        // Single-frame intents reset at begin_frame so a stale shift-
        // click can't fire twice.
        assert!(state.pending_quick_transfer.is_none());
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
        state.observe_inventory(&first, false);
        // The first observation seeds the baseline, nothing should flash.
        assert!(state.slot_flashes.is_empty());

        let mut second = first.clone();
        second.inventory_slots[3] = Some(ItemStack::new("coal", 4));
        second.actionbar_slots[0] = Some(ItemStack::new("hatchet", 1));
        state.observe_inventory(&second, false);
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
        state.observe_inventory(&third, false);
        assert!(
            state
                .slot_flashes
                .contains_key(&ItemContainerSlot::inventory(3))
        );
    }

    #[test]
    fn observe_inventory_classifies_total_quantity_changes() {
        let mut state = InventoryUiState::default();

        // Seed baseline, no event on the first observation.
        let baseline = PlayerInventoryState::empty();
        assert_eq!(state.observe_inventory(&baseline, true), None);

        // A noted pickup intent followed by a quantity gain reads as
        // Pickup carrying the gained item's id (the material cue picks
        // its sound off this). Without the intent, the same gain would
        // be silent.
        state.note_pickup_intent();
        let mut after_pickup = baseline.clone();
        after_pickup.inventory_slots[0] = Some(ItemStack::new("coal", 3));
        assert_eq!(
            state.observe_inventory(&after_pickup, true),
            Some(InventorySoundEvent::Pickup {
                item_id: Some("coal".into())
            })
        );

        // Stack consolidated into the same total → Move (same total,
        // different per-slot contents).
        let mut after_move = after_pickup.clone();
        after_move.inventory_slots[0] = None;
        after_move.actionbar_slots[0] = Some(ItemStack::new("coal", 3));
        assert_eq!(
            state.observe_inventory(&after_move, true),
            Some(InventorySoundEvent::Move)
        );

        // Same snapshot again → no event.
        assert_eq!(state.observe_inventory(&after_move, true), None);

        // Quantity shrank → Drop.
        let mut after_drop = after_move.clone();
        after_drop.actionbar_slots[0] = Some(ItemStack::new("coal", 1));
        assert_eq!(
            state.observe_inventory(&after_drop, true),
            Some(InventorySoundEvent::Drop)
        );
    }

    #[test]
    fn observe_inventory_returns_none_when_nothing_changed() {
        let mut state = InventoryUiState::default();
        let snapshot = PlayerInventoryState::empty();
        assert_eq!(state.observe_inventory(&snapshot, false), None);
        assert_eq!(state.observe_inventory(&snapshot, false), None);
    }

    #[test]
    fn observe_inventory_suppresses_pickup_without_intent() {
        let mut state = InventoryUiState::default();
        let baseline = PlayerInventoryState::empty();
        state.observe_inventory(&baseline, false);

        // No `note_pickup_intent`, a quantity gain here is a harvest
        // payout, not a pickup, and must stay silent.
        let mut grew = baseline.clone();
        grew.inventory_slots[0] = Some(ItemStack::new("wood", 1));
        assert_eq!(state.observe_inventory(&grew, false), None);
    }

    #[test]
    fn pickup_intent_expires_after_window() {
        let mut state = InventoryUiState::default();
        state.note_pickup_intent();
        // Tick past the intent window so the next gain reads as harvest.
        state.tick_slot_flashes(PICKUP_INTENT_WINDOW_SECS + 0.05);

        let baseline = PlayerInventoryState::empty();
        state.observe_inventory(&baseline, false);
        let mut grew = baseline.clone();
        grew.inventory_slots[0] = Some(ItemStack::new("stone", 1));
        assert_eq!(state.observe_inventory(&grew, false), None);
    }

    #[test]
    fn pickup_intent_is_consumed_so_later_gain_stays_silent() {
        let mut state = InventoryUiState::default();
        let mut snapshot = PlayerInventoryState::empty();
        state.observe_inventory(&snapshot, false);

        state.note_pickup_intent();
        snapshot.inventory_slots[0] = Some(ItemStack::new("ore", 2));
        assert_eq!(
            state.observe_inventory(&snapshot, false),
            Some(InventorySoundEvent::Pickup {
                item_id: Some("ore".into())
            })
        );

        // A second gain in the same window (e.g. a harvest tick that
        // landed right after) must not piggyback on the spent intent.
        snapshot.inventory_slots[0] = Some(ItemStack::new("ore", 5));
        assert_eq!(state.observe_inventory(&snapshot, false), None);
    }

    #[test]
    fn ammo_consumption_stays_silent_with_every_item_ui_closed() {
        // Firing the bow / throwing a bomb shrinks a stack server-side. With
        // no item UI open and no drop shortcut pressed, that loss must NOT
        // click like a UI interaction (the drop cue mid-combat bug).
        let mut state = InventoryUiState::default();
        let mut snapshot = PlayerInventoryState::empty();
        snapshot.actionbar_slots[0] = Some(ItemStack::new("arrow", 12));
        state.observe_inventory(&snapshot, false);

        snapshot.actionbar_slots[0] = Some(ItemStack::new("arrow", 11));
        assert_eq!(state.observe_inventory(&snapshot, false), None);

        // The whole stack burning away is still consumption, still silent.
        snapshot.actionbar_slots[0] = None;
        assert_eq!(state.observe_inventory(&snapshot, false), None);
    }

    #[test]
    fn drop_shortcut_intent_lets_the_drop_cue_through_once() {
        let mut state = InventoryUiState::default();
        let mut snapshot = PlayerInventoryState::empty();
        snapshot.actionbar_slots[0] = Some(ItemStack::new("stone", 10));
        state.observe_inventory(&snapshot, false);

        // The drop shortcut notes intent; the matching loss clicks even
        // with every panel closed.
        state.note_drop_intent();
        snapshot.actionbar_slots[0] = Some(ItemStack::new("stone", 9));
        assert_eq!(
            state.observe_inventory(&snapshot, false),
            Some(InventorySoundEvent::Drop)
        );

        // The intent is consumed: a later unrelated loss stays silent.
        snapshot.actionbar_slots[0] = Some(ItemStack::new("stone", 8));
        assert_eq!(state.observe_inventory(&snapshot, false), None);
    }

    #[test]
    fn moves_without_an_open_item_ui_stay_silent() {
        // A server-side rearrangement with everything closed is not a
        // player action; the shuffle cue only backs open-UI handling.
        let mut state = InventoryUiState::default();
        let mut snapshot = PlayerInventoryState::empty();
        snapshot.inventory_slots[0] = Some(ItemStack::new("coal", 3));
        state.observe_inventory(&snapshot, false);

        snapshot.inventory_slots[0] = None;
        snapshot.actionbar_slots[2] = Some(ItemStack::new("coal", 3));
        assert_eq!(state.observe_inventory(&snapshot, false), None);
    }

    #[test]
    fn unified_slot_ref_classifies_bag_and_player_variants() {
        let bag = UnifiedSlotRef::Bag(3);
        let player = UnifiedSlotRef::Player(ItemContainerSlot::inventory(0));
        let furnace = UnifiedSlotRef::Furnace(crate::protocol::FurnaceSlotRef::Fuel);

        assert!(bag.is_bag());
        assert!(!bag.is_player());
        assert!(!player.is_bag());
        assert!(player.is_player());
        assert!(!furnace.is_bag());
        assert!(!furnace.is_player());
    }

    #[test]
    fn unified_slot_ref_maps_to_loot_bag_ref() {
        // A bag slot stays a bag slot end-to-end.
        let bag = UnifiedSlotRef::Bag(5);
        assert_eq!(
            bag.as_loot_bag_ref(),
            LootBagSlotRef::Bag(5),
            "Bag → Bag should round-trip the index untouched"
        );

        // Player inventory routes through `PlayerInventory(slot)` so a
        // player→bag drag is one `LootBagCommand::Move`.
        let inv = UnifiedSlotRef::Player(ItemContainerSlot::inventory(2));
        assert_eq!(inv.as_loot_bag_ref(), LootBagSlotRef::PlayerInventory(2));

        // Same idea for the actionbar.
        let bar = UnifiedSlotRef::Player(ItemContainerSlot::actionbar(4));
        assert_eq!(bar.as_loot_bag_ref(), LootBagSlotRef::PlayerActionbar(4));
    }

    #[test]
    fn unified_slot_ref_maps_to_furnace_ref() {
        // A furnace slot stays a furnace slot.
        let fuel = UnifiedSlotRef::Furnace(crate::protocol::FurnaceSlotRef::Fuel);
        assert_eq!(fuel.as_furnace_ref(), crate::protocol::FurnaceSlotRef::Fuel);

        let inv = UnifiedSlotRef::Player(ItemContainerSlot::inventory(7));
        assert_eq!(
            inv.as_furnace_ref(),
            crate::protocol::FurnaceSlotRef::PlayerInventory(7)
        );
        let bar = UnifiedSlotRef::Player(ItemContainerSlot::actionbar(1));
        assert_eq!(
            bar.as_furnace_ref(),
            crate::protocol::FurnaceSlotRef::PlayerActionbar(1)
        );
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
