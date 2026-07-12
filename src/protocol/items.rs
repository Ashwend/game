//! Item stacks, the inventory/actionbar container addressing, and the
//! per-player inventory + crafting-queue state shapes.

use serde::{Deserialize, Serialize};

use super::{ACTIONBAR_SLOT_COUNT, CraftingJobId, EQUIPMENT_SLOT_COUNT, INVENTORY_SLOT_COUNT};

/// One of the four worn-armor slots. Each armor piece declares (via its
/// [`crate::items::ArmorProfile`]) which slot it fits, and a stack can only be
/// moved into the matching [`ItemContainer::Equipment`] slot. Ordered head to
/// feet; the discriminant order is the index mapping used by
/// [`ItemContainerSlot::equipment`], so never reorder these (it is a wire and
/// save layout).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EquipmentSlot {
    Head,
    Chest,
    Legs,
    Feet,
}

impl EquipmentSlot {
    /// Every slot in index order, so the paperdoll UI and the mitigation
    /// recompute can iterate all four without hardcoding the list.
    pub const ALL: [EquipmentSlot; EQUIPMENT_SLOT_COUNT] = [
        EquipmentSlot::Head,
        EquipmentSlot::Chest,
        EquipmentSlot::Legs,
        EquipmentSlot::Feet,
    ];

    /// The `equipment_slots` vector index this slot maps to. The inverse of
    /// [`EquipmentSlot::from_index`]. Kept in lockstep with the enum order.
    pub const fn index(self) -> usize {
        match self {
            EquipmentSlot::Head => 0,
            EquipmentSlot::Chest => 1,
            EquipmentSlot::Legs => 2,
            EquipmentSlot::Feet => 3,
        }
    }

    /// Short display name for the paperdoll UI: shown on an empty slot so the
    /// player reads what goes there, and in the tooltip title fallback.
    pub const fn label(self) -> &'static str {
        match self {
            EquipmentSlot::Head => "Head",
            EquipmentSlot::Chest => "Chest",
            EquipmentSlot::Legs => "Legs",
            EquipmentSlot::Feet => "Feet",
        }
    }

    /// Resolve a slot from its `equipment_slots` index, or `None` if out of
    /// range. Used to interpret an [`ItemContainerSlot`] whose container is
    /// [`ItemContainer::Equipment`].
    pub const fn from_index(index: usize) -> Option<EquipmentSlot> {
        match index {
            0 => Some(EquipmentSlot::Head),
            1 => Some(EquipmentSlot::Chest),
            2 => Some(EquipmentSlot::Legs),
            3 => Some(EquipmentSlot::Feet),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemStack {
    #[serde(deserialize_with = "super::deserialize_interned_item_id")]
    pub item_id: crate::items::ItemId,
    pub quantity: u16,
    /// Remaining impact budget for tools, counted down by the server on
    /// swings that connect. `None` for items without wear (materials,
    /// deployables). Initialised to the registry's `max_durability` at
    /// creation; code that splits or moves stacks must carry the value
    /// along rather than rebuilding the stack, or a worn tool comes out
    /// of the move factory-fresh.
    pub durability: Option<u32>,
}

impl ItemStack {
    /// Build a stack with full durability for tools (looked up from the
    /// item registry) and `None` for everything else. Every "the world
    /// just produced this item" site (crafting grants, admin gives, test
    /// kits) routes through here so new tools always spawn pristine.
    pub fn new(item_id: impl AsRef<str>, quantity: u16) -> Self {
        let item_id = crate::items::intern_item_id(item_id.as_ref());
        // A durable item's wear budget comes from whichever profile it carries:
        // a gather tool's `max_durability`, a dedicated weapon's, or a worn
        // armor piece's. An item carries at most one of these profiles today, so
        // the fallback order never double-counts.
        let durability = crate::items::item_definition(&item_id).and_then(|definition| {
            definition
                .tool
                .and_then(|tool| tool.max_durability)
                .or_else(|| definition.weapon.and_then(|weapon| weapon.max_durability))
                .or_else(|| definition.armor.and_then(|armor| armor.max_durability))
        });
        Self {
            item_id,
            quantity,
            durability,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ItemContainer {
    Inventory,
    Actionbar,
    /// The worn-armor paperdoll. The `slot` index of an
    /// [`ItemContainerSlot`] with this container is an
    /// [`EquipmentSlot::index`]; moving a stack in requires the item's
    /// [`crate::items::ArmorProfile`] slot to match.
    Equipment,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ItemContainerSlot {
    pub container: ItemContainer,
    pub slot: usize,
}

impl ItemContainerSlot {
    pub const fn inventory(slot: usize) -> Self {
        Self {
            container: ItemContainer::Inventory,
            slot,
        }
    }

    pub const fn actionbar(slot: usize) -> Self {
        Self {
            container: ItemContainer::Actionbar,
            slot,
        }
    }

    /// Address a worn-armor slot. The index comes from
    /// [`EquipmentSlot::index`], so `equipment(EquipmentSlot::Head)` targets
    /// `equipment_slots[0]`.
    pub const fn equipment(slot: EquipmentSlot) -> Self {
        Self {
            container: ItemContainer::Equipment,
            slot: slot.index(),
        }
    }
}

/// One in-progress crafting job. `progress_ticks` advances toward
/// `total_ticks`; when they meet the server grants the recipe's output
/// (multiplied by `quantity`) and pops the job. Inputs are not echoed back
///, they were taken at enqueue time and the recipe id lets the client
/// reconstruct everything else from the static registry.
///
/// `quantity` is the batch size. A job with `quantity = 3` ran with
/// 3× the inputs at enqueue time, has `total_ticks = ticks_per_unit × 3`,
/// and on completion grants `output_quantity × 3` of the output item in a
/// single grant. The UI uses `quantity > 1` to render `×N` next to the
/// job's name in the queue HUD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CraftingJob {
    pub job_id: CraftingJobId,
    #[serde(deserialize_with = "super::deserialize_interned_recipe_id")]
    pub recipe_id: crate::crafting::RecipeId,
    pub progress_ticks: u32,
    pub total_ticks: u32,
    pub quantity: u16,
}

impl CraftingJob {
    pub fn new(
        job_id: CraftingJobId,
        recipe_id: impl AsRef<str>,
        total_ticks: u32,
        quantity: u16,
    ) -> Self {
        Self {
            job_id,
            recipe_id: crate::crafting::intern_recipe_id(recipe_id.as_ref()),
            progress_ticks: 0,
            total_ticks,
            quantity,
        }
    }

    /// Fraction of the head job's craft time that has elapsed, in `[0.0, 1.0]`.
    /// Returns `1.0` for zero-duration recipes so the UI doesn't divide by
    /// zero or stall on a permanent empty bar.
    pub fn progress_fraction(&self) -> f32 {
        if self.total_ticks == 0 {
            return 1.0;
        }
        (self.progress_ticks as f32 / self.total_ticks as f32).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerCraftingState {
    pub jobs: Vec<CraftingJob>,
}

impl PlayerCraftingState {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerInventoryState {
    pub inventory_slots: Vec<Option<ItemStack>>,
    pub actionbar_slots: Vec<Option<ItemStack>>,
    /// The four worn-armor slots, indexed by [`EquipmentSlot::index`]. Persists
    /// via `PersistedPlayer.inventory` (one save-format bump). `#[serde(default)]`
    /// so an in-memory `PlayerInventoryState` deserialized from a pre-bump shape
    /// (or built via a struct literal that omits it) comes back with an empty
    /// paperdoll rather than failing to decode; `normalize_capacity` then pads
    /// it to the canonical length. The wire path always carries the field once
    /// both sides run this build.
    #[serde(default)]
    pub equipment_slots: Vec<Option<ItemStack>>,
    pub active_actionbar_slot: usize,
}

impl Default for PlayerInventoryState {
    fn default() -> Self {
        Self::empty()
    }
}

impl PlayerInventoryState {
    pub fn empty() -> Self {
        Self {
            inventory_slots: vec![None; INVENTORY_SLOT_COUNT],
            actionbar_slots: vec![None; ACTIONBAR_SLOT_COUNT],
            equipment_slots: vec![None; EQUIPMENT_SLOT_COUNT],
            active_actionbar_slot: 0,
        }
    }

    /// Pad (or trim) the slot vectors to the current canonical capacities.
    /// A persisted inventory written before a capacity change keeps its old
    /// length on load; normalizing on restore exposes any newly-added empty
    /// slots and keeps the on-wire shape consistent with fresh inventories.
    /// Bounds checks already use the live vec length, so this is about making
    /// the new slots usable, not about safety.
    pub fn normalize_capacity(&mut self) {
        self.inventory_slots.resize(INVENTORY_SLOT_COUNT, None);
        self.actionbar_slots.resize(ACTIONBAR_SLOT_COUNT, None);
        // A save (or a serde-defaulted state) written before the paperdoll
        // landed has zero equipment slots; pad it up so the four worn slots
        // exist and stay in the on-wire shape. Trimming an over-long vec keeps
        // the length canonical the same way the bag/actionbar do.
        self.equipment_slots.resize(EQUIPMENT_SLOT_COUNT, None);
        if self.active_actionbar_slot >= ACTIONBAR_SLOT_COUNT {
            self.active_actionbar_slot = 0;
        }
    }

    pub fn active_actionbar_stack(&self) -> Option<&ItemStack> {
        self.actionbar_slots
            .get(self.active_actionbar_slot)
            .and_then(Option::as_ref)
    }

    /// Read-only access to the stack in a specific slot. Returns `None` for
    /// an empty *or* out-of-range slot. Used by the client-side move
    /// prediction to gate on an empty destination.
    pub fn slot(&self, slot: ItemContainerSlot) -> Option<&ItemStack> {
        match slot.container {
            ItemContainer::Inventory => self.inventory_slots.get(slot.slot),
            ItemContainer::Actionbar => self.actionbar_slots.get(slot.slot),
            ItemContainer::Equipment => self.equipment_slots.get(slot.slot),
        }
        .and_then(Option::as_ref)
    }

    /// Read the currently-worn piece in `slot`, if any. Thin typed wrapper over
    /// [`PlayerInventoryState::slot`] for the mitigation recompute and UI.
    pub fn equipment(&self, slot: EquipmentSlot) -> Option<&ItemStack> {
        self.slot(ItemContainerSlot::equipment(slot))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::items::{COAL_ID, IRON_ORE_ID, intern_item_id};

    use super::*;

    #[test]
    fn normalize_capacity_grows_short_inventory_and_preserves_stacks() {
        let mut state = PlayerInventoryState::empty();
        // Simulate a save written before a capacity bump: a short slot vec
        // with stacks at known indices.
        state.inventory_slots = vec![
            Some(ItemStack::new(IRON_ORE_ID, 4)),
            None,
            Some(ItemStack::new(COAL_ID, 7)),
        ];

        state.normalize_capacity();

        assert_eq!(state.inventory_slots.len(), INVENTORY_SLOT_COUNT);
        // Original stacks stay put at their indices.
        assert_eq!(
            state.inventory_slots[0],
            Some(ItemStack::new(IRON_ORE_ID, 4))
        );
        assert_eq!(state.inventory_slots[1], None);
        assert_eq!(state.inventory_slots[2], Some(ItemStack::new(COAL_ID, 7)));
        // Every newly-added trailing slot is empty.
        assert!(state.inventory_slots[3..].iter().all(Option::is_none));
    }

    #[test]
    fn normalize_capacity_trims_over_long_inventory() {
        let mut state = PlayerInventoryState::empty();
        state.inventory_slots = vec![None; INVENTORY_SLOT_COUNT + 10];
        state.actionbar_slots = vec![None; ACTIONBAR_SLOT_COUNT + 5];

        state.normalize_capacity();

        assert_eq!(state.inventory_slots.len(), INVENTORY_SLOT_COUNT);
        assert_eq!(state.actionbar_slots.len(), ACTIONBAR_SLOT_COUNT);
    }

    #[test]
    fn normalize_capacity_resets_out_of_range_active_slot() {
        let mut state = PlayerInventoryState::empty();
        state.active_actionbar_slot = ACTIONBAR_SLOT_COUNT + 3;
        state.normalize_capacity();
        assert_eq!(state.active_actionbar_slot, 0);

        // An in-range value is left untouched.
        let mut in_range = PlayerInventoryState::empty();
        in_range.active_actionbar_slot = ACTIONBAR_SLOT_COUNT - 1;
        in_range.normalize_capacity();
        assert_eq!(in_range.active_actionbar_slot, ACTIONBAR_SLOT_COUNT - 1);
    }

    #[test]
    fn item_stack_round_trips_and_reuses_the_interned_arc() {
        let stack = ItemStack::new(IRON_ORE_ID, 4);
        let encoded = postcard::to_allocvec(&stack).expect("encode item stack");
        let decoded: ItemStack = postcard::from_bytes(&encoded).expect("decode item stack");

        assert_eq!(decoded, stack);
        // The `deserialize_interned_item_id` hook routes the decoded id
        // through the global intern table, so the decoded `Arc<str>` is the
        // same allocation the registry already holds (refcount bump, not a
        // fresh heap copy).
        assert!(Arc::ptr_eq(&decoded.item_id, &intern_item_id(IRON_ORE_ID)));
    }

    #[test]
    fn legacy_sticks_stacks_deserialize_as_wood() {
        // `sticks` was folded into `wood` (2026-06). Saves use postcard
        // like the wire, so a stack persisted before the fold must come
        // back as wood instead of an unknown item id.
        let legacy = ItemStack {
            item_id: intern_item_id("sticks"),
            quantity: 25,
            durability: None,
        };
        let encoded = postcard::to_allocvec(&legacy).expect("encode legacy stack");
        let decoded: ItemStack = postcard::from_bytes(&encoded).expect("decode legacy stack");

        assert_eq!(decoded.item_id.as_ref(), crate::items::WOOD_ID);
        assert_eq!(decoded.quantity, 25);
    }

    #[test]
    fn crafting_job_recipe_id_round_trips_and_reuses_the_interned_arc() {
        use crate::crafting::{STONE_HATCHET_RECIPE_ID, intern_recipe_id};

        let job = CraftingJob::new(7, STONE_HATCHET_RECIPE_ID, 120, 2);
        let encoded = postcard::to_allocvec(&job).expect("encode crafting job");
        let decoded: CraftingJob = postcard::from_bytes(&encoded).expect("decode crafting job");

        assert_eq!(decoded, job);
        // Same interning guarantee as the item id, via
        // `deserialize_interned_recipe_id`.
        assert!(Arc::ptr_eq(
            &decoded.recipe_id,
            &intern_recipe_id(STONE_HATCHET_RECIPE_ID)
        ));
    }
}
