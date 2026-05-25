//! Server-authoritative furnace state + smelt loop + open/close
//! interaction commands.
//!
//! Furnaces are entity-bound, not recipe-bound: smelting happens inside
//! the furnace's own inventory grid rather than through the regular
//! crafting registry. Each tick the head smeltable input advances by
//! one tick; on completion the input stack shrinks by one and the
//! matching output stack grows by one. Fuel ticks down on every tick
//! the furnace is actively smelting. Auto-shutoff fires when the
//! output won't fit, when fuel runs out mid-smelt, or when there's
//! nothing left to smelt — better to flip the active flag than to
//! sit idle pretending to burn.

use crate::{
    items::{COAL_ID, IRON_BAR_ID, IRON_ORE_ID, WOOD_ID},
    protocol::{
        ClientId, DeployedEntityId, FURNACE_ITEM_SLOT_COUNT, FurnaceCommand, FurnaceSlotRef,
        ItemContainerSlot, ItemStack, OpenFurnaceView, SERVER_TICK_RATE_HZ, ServerMessage,
        ToastKind, ToastMessage,
    },
    save::PersistedFurnaceState,
};

use super::{
    DeliveryTarget, GameServer, ServerClient, ServerEnvelope,
    inventory::{add_stack_to_inventory, insert_stack_at},
};

/// How long one smelt operation takes, in ticks. 6 seconds at 20 Hz so
/// it feels like a real wait without being tedious for solo testing.
const SMELT_TICKS_PER_OUTPUT: u32 = (6.0 * SERVER_TICK_RATE_HZ) as u32;
/// Burn duration in ticks for one wood unit (4s) — short burn, lots of
/// shovelling. Coal (16s) is the upgrade path.
const WOOD_BURN_TICKS: u32 = (4.0 * SERVER_TICK_RATE_HZ) as u32;
const COAL_BURN_TICKS: u32 = (16.0 * SERVER_TICK_RATE_HZ) as u32;

/// Maximum interaction range, in metres, for `E`-to-open. Slightly larger
/// than `PLACEMENT_REACH_M` so a player who placed at max reach can
/// still interact without having to step forward.
pub(super) const FURNACE_INTERACT_RANGE_M: f32 = 5.5;

/// Per-furnace operational state. The fuel + items slots live here;
/// `active` and the timers drive the smelt loop.
#[derive(Debug, Clone, Default)]
pub(crate) struct FurnaceState {
    pub(super) fuel: Option<ItemStack>,
    pub(super) items: [Option<ItemStack>; FURNACE_ITEM_SLOT_COUNT],
    pub(super) active: bool,
    /// Remaining ticks of burn for the fuel unit currently being
    /// consumed. 0 means no fuel is mid-burn — the next tick that
    /// needs fuel will pop one off the `fuel` stack.
    pub(super) fuel_burn_ticks_left: u32,
    /// Accumulated progress against the current smelt operation.
    /// Reset when the head smelt completes or the input stack moves.
    pub(super) smelt_progress_ticks: u32,
}

impl FurnaceState {
    pub(super) fn from_persisted(p: PersistedFurnaceState) -> Self {
        let mut items: [Option<ItemStack>; FURNACE_ITEM_SLOT_COUNT] = Default::default();
        for (index, stack) in p.items.into_iter().enumerate() {
            if let Some(slot) = items.get_mut(index) {
                *slot = stack;
            }
        }
        Self {
            fuel: p.fuel,
            items,
            active: p.active,
            fuel_burn_ticks_left: p.fuel_burn_ticks_left,
            smelt_progress_ticks: p.smelt_progress_ticks,
        }
    }

    pub(super) fn to_persisted(&self) -> PersistedFurnaceState {
        PersistedFurnaceState {
            fuel: self.fuel.clone(),
            items: self.items.to_vec(),
            active: self.active,
            fuel_burn_ticks_left: self.fuel_burn_ticks_left,
            smelt_progress_ticks: self.smelt_progress_ticks,
        }
    }

    pub(super) fn to_view(&self, id: DeployedEntityId) -> OpenFurnaceView {
        OpenFurnaceView {
            id,
            fuel: self.fuel.clone(),
            items: self.items.to_vec(),
            active: self.active,
            smelt_fraction: if SMELT_TICKS_PER_OUTPUT == 0 {
                1.0
            } else {
                (self.smelt_progress_ticks as f32 / SMELT_TICKS_PER_OUTPUT as f32).clamp(0.0, 1.0)
            },
            fuel_fraction: {
                let max = max_fuel_burn_ticks_for(self.fuel.as_ref());
                if max == 0 {
                    0.0
                } else {
                    (self.fuel_burn_ticks_left as f32 / max as f32).clamp(0.0, 1.0)
                }
            },
        }
    }
}

/// Map a fuel item to how many ticks one unit burns for. The smelt loop
/// reads this when it needs to ignite the next unit.
fn fuel_burn_ticks_for(item_id: &str) -> Option<u32> {
    match item_id {
        WOOD_ID => Some(WOOD_BURN_TICKS),
        COAL_ID => Some(COAL_BURN_TICKS),
        _ => None,
    }
}

/// Upper bound for the fuel-fraction UI bar. We track the *currently
/// burning* unit, so the bar fills the moment a new unit ignites and
/// drains down to 0 before the next one kicks off.
fn max_fuel_burn_ticks_for(stack: Option<&ItemStack>) -> u32 {
    stack
        .and_then(|s| fuel_burn_ticks_for(s.item_id.as_ref()))
        .unwrap_or(WOOD_BURN_TICKS)
}

/// Smelt-result map. Right now there's only one (iron ore → iron bar);
/// extending this is how new smelt recipes ship.
fn smelt_result(item_id: &str) -> Option<ItemStack> {
    if item_id == IRON_ORE_ID {
        Some(ItemStack::new(IRON_BAR_ID, 1))
    } else {
        None
    }
}

impl GameServer {
    pub(super) fn apply_furnace_command(
        &mut self,
        client_id: ClientId,
        command: FurnaceCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            FurnaceCommand::Open { id } => self.open_furnace(client_id, id),
            FurnaceCommand::Close => {
                self.close_furnace(client_id);
                Vec::new()
            }
            FurnaceCommand::SetActive { active } => {
                self.set_open_furnace_active(client_id, active);
                Vec::new()
            }
            FurnaceCommand::Move { from, to, quantity } => {
                self.move_in_furnace(client_id, from, to, quantity)
            }
            FurnaceCommand::QuickTransfer { from } => self.quick_transfer(client_id, from),
        }
    }

    /// Resolve a shift+click "send this somewhere useful" intent.
    ///
    /// The destination is computed from the source location + item kind:
    ///
    /// - **Player → furnace.** Fuel items land in the fuel slot (merging
    ///   with the same fuel, swapping with a different fuel if the
    ///   displaced stack can be re-housed in the player's bag). Anything
    ///   else flows into the items grid, merging into matching stacks
    ///   first before taking an empty slot.
    /// - **Furnace → player.** The stack goes back into the player's
    ///   inventory via [`add_stack_to_inventory`] so the existing
    ///   "matching → empty" priority is reused (no second implementation
    ///   of the same rule).
    ///
    /// All operations are clamped to the receiving slot's stack limit; a
    /// remainder that doesn't fit is put back into the source slot. No
    /// silent drops on the ground — the player asked for a transfer, not
    /// a discard.
    fn quick_transfer(&mut self, client_id: ClientId, from: FurnaceSlotRef) -> Vec<ServerEnvelope> {
        let Some(furnace_id) = self.client_open_furnace(client_id) else {
            return Vec::new();
        };
        // Peek the source stack so we know the item kind (fuel vs not)
        // and how much we're moving before we commit to taking it out.
        let Some(source) = self.peek_furnace_slot(client_id, furnace_id, from) else {
            return Vec::new();
        };

        match from {
            FurnaceSlotRef::PlayerInventory(_) | FurnaceSlotRef::PlayerActionbar(_) => {
                self.quick_transfer_player_to_furnace(client_id, furnace_id, from, source);
            }
            FurnaceSlotRef::Fuel | FurnaceSlotRef::Item(_) => {
                self.quick_transfer_furnace_to_player(client_id, furnace_id, from);
            }
        }
        Vec::new()
    }

    /// Read-only inspect of the slot's current stack. Returns a clone so
    /// the borrow doesn't tangle with subsequent mutating calls — the
    /// stack is small and the call only happens on shift+click.
    fn peek_furnace_slot(
        &self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        slot: FurnaceSlotRef,
    ) -> Option<ItemStack> {
        match slot {
            FurnaceSlotRef::PlayerInventory(index) => self
                .clients
                .get(&client_id)?
                .inventory
                .inventory_slots
                .get(index)?
                .clone(),
            FurnaceSlotRef::PlayerActionbar(index) => self
                .clients
                .get(&client_id)?
                .inventory
                .actionbar_slots
                .get(index)?
                .clone(),
            FurnaceSlotRef::Fuel => self
                .deployed_entities
                .get(&furnace_id)?
                .furnace
                .as_ref()?
                .fuel
                .clone(),
            FurnaceSlotRef::Item(index) => self
                .deployed_entities
                .get(&furnace_id)?
                .furnace
                .as_ref()?
                .items
                .get(index)?
                .clone(),
        }
    }

    fn quick_transfer_player_to_furnace(
        &mut self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        from: FurnaceSlotRef,
        source: ItemStack,
    ) {
        let is_fuel = fuel_burn_ticks_for(source.item_id.as_ref()).is_some();

        // Take the whole source stack up front. We restore any remainder
        // at the end so a partial fit doesn't silently delete items.
        let Some((mut taken, _)) =
            self.take_from_furnace_slot(client_id, furnace_id, from, None)
        else {
            return;
        };

        if is_fuel {
            // Fuel path: merge → place → swap (if the displaced stack
            // can be re-housed in the player's bag).
            taken = match self.place_in_fuel_slot_with_swap(client_id, furnace_id, taken) {
                FuelPlaceOutcome::Placed => return,
                FuelPlaceOutcome::Rejected(stack) => stack,
            };
        } else {
            // Items path: merge into matching item slots, then fill
            // first empty item slot.
            let Some(entity) = self.deployed_entities.get_mut(&furnace_id) else {
                self.restore_to_source(client_id, furnace_id, from, taken);
                return;
            };
            let Some(furnace) = entity.furnace.as_mut() else {
                self.restore_to_source(client_id, furnace_id, from, taken);
                return;
            };
            taken = match add_stack_to_furnace_items(furnace, taken) {
                None => return,
                Some(remainder) => remainder,
            };
        }

        // Anything we couldn't place goes back where it came from. A
        // partial-fit path already drained the source via
        // `take_from_furnace_slot`, so the deposit can land on an empty
        // slot cleanly.
        self.restore_to_source(client_id, furnace_id, from, taken);
    }

    fn quick_transfer_furnace_to_player(
        &mut self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        from: FurnaceSlotRef,
    ) {
        // Furnace → player: drain the source, run it through the
        // existing inventory adder which handles matching-stack + first-
        // empty-slot in one shot.
        let Some((taken, _)) =
            self.take_from_furnace_slot(client_id, furnace_id, from, None)
        else {
            return;
        };
        let leftover = match self.clients.get_mut(&client_id) {
            Some(client) => add_stack_to_inventory(&mut client.inventory, taken),
            None => return,
        };
        if let Some(remainder) = leftover {
            // No room in the player's inventory — put back into the
            // furnace slot we drained.
            self.restore_to_source(client_id, furnace_id, from, remainder);
        }
    }

    /// Place `stack` into the fuel slot, handling all three sub-cases
    /// in one place:
    /// - target empty → fill
    /// - target has same fuel → merge (clamped to stack limit)
    /// - target has different fuel → swap, but only if the player can
    ///   actually receive the displaced fuel; otherwise the operation
    ///   is rejected so the player doesn't silently lose the existing
    ///   contents to a drop-on-ground recovery.
    fn place_in_fuel_slot_with_swap(
        &mut self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        stack: ItemStack,
    ) -> FuelPlaceOutcome {
        let Some(entity) = self.deployed_entities.get_mut(&furnace_id) else {
            return FuelPlaceOutcome::Rejected(stack);
        };
        let Some(furnace) = entity.furnace.as_mut() else {
            return FuelPlaceOutcome::Rejected(stack);
        };

        match furnace.fuel.as_ref().map(|s| s.item_id.clone()) {
            Some(existing_id) if existing_id == stack.item_id => {
                // Same fuel: merge into the existing stack.
                match merge_into_optional_slot(&mut furnace.fuel, stack) {
                    None => FuelPlaceOutcome::Placed,
                    Some(remainder) => FuelPlaceOutcome::Rejected(remainder),
                }
            }
            Some(_) => {
                // Different fuel: dry-run a swap by checking whether
                // the displaced stack would fit in the player's bag.
                // We don't actually take it out until we know it does.
                let displaced = furnace
                    .fuel
                    .clone()
                    .expect("fuel slot non-empty by match arm");
                if !inventory_has_room_for(self.clients.get(&client_id), &displaced) {
                    return FuelPlaceOutcome::Rejected(stack);
                }
                // Pulling fuel out cancels the in-flight burn timer for
                // the same reason `move_in_furnace` does it: the bar
                // would otherwise read against the new fuel's
                // denominator, which is misleading.
                furnace.fuel_burn_ticks_left = 0;
                furnace.fuel = Some(stack);
                // Now the swap is committed; route the displaced fuel
                // into the player. The pre-check guarantees this fits.
                if let Some(client) = self.clients.get_mut(&client_id) {
                    let leftover = add_stack_to_inventory(&mut client.inventory, displaced);
                    debug_assert!(
                        leftover.is_none(),
                        "inventory_has_room_for should guarantee the fit"
                    );
                    let _ = leftover;
                }
                FuelPlaceOutcome::Placed
            }
            None => {
                furnace.fuel = Some(stack);
                FuelPlaceOutcome::Placed
            }
        }
    }

    /// Drop `stack` back into the source slot a quick-transfer just
    /// drained. The slot was emptied before this call, so a clean
    /// `deposit_into_furnace_slot` lands without contention; if it
    /// somehow can't (e.g. the player's slot index is out of range
    /// after a save migration), the items spawn at the player's feet
    /// rather than vanish.
    fn restore_to_source(
        &mut self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        from: FurnaceSlotRef,
        stack: ItemStack,
    ) {
        if stack.quantity == 0 {
            return;
        }
        if let Some(leftover) =
            self.deposit_into_furnace_slot(client_id, furnace_id, from, stack, false)
        {
            let drop_origin = self.clients.get(&client_id).map(|client| {
                (
                    crate::server::movement::drop_position(&client.controller),
                    crate::server::movement::drop_velocity(&client.controller),
                    client.controller.yaw,
                )
            });
            if let Some((position, velocity, yaw)) = drop_origin {
                self.spawn_dropped_item(leftover, position, velocity, yaw);
            }
        }
    }

    fn open_furnace(&mut self, client_id: ClientId, id: DeployedEntityId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let player_pos = client.controller.position;
        let Some(entity) = self.deployed_entities.get(&id) else {
            return furnace_toast(
                client_id,
                ToastKind::Warning,
                "Furnace not found".to_owned(),
            );
        };
        if entity.furnace.is_none() {
            return furnace_toast(client_id, ToastKind::Warning, "Not a furnace".to_owned());
        }
        let dx = entity.position.x - player_pos.x;
        let dz = entity.position.z - player_pos.z;
        if (dx * dx + dz * dz).sqrt() > FURNACE_INTERACT_RANGE_M {
            return furnace_toast(
                client_id,
                ToastKind::Warning,
                "Too far from the furnace".to_owned(),
            );
        }
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.open_furnace = Some(id);
        }
        Vec::new()
    }

    pub(super) fn close_furnace(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.open_furnace = None;
        }
    }

    fn set_open_furnace_active(&mut self, client_id: ClientId, active: bool) {
        let Some(furnace_id) = self.client_open_furnace(client_id) else {
            return;
        };
        let Some(entity) = self.deployed_entities.get_mut(&furnace_id) else {
            return;
        };
        let Some(furnace) = entity.furnace.as_mut() else {
            return;
        };
        furnace.active = active;
        if !active {
            // Pausing snaps the smelt progress back to zero so the
            // player can't "save" a 90%-smelted timer by flipping the
            // switch off and on — feels gamey and also keeps the
            // server free of carryover state across long off-periods.
            furnace.smelt_progress_ticks = 0;
        }
    }

    fn move_in_furnace(
        &mut self,
        client_id: ClientId,
        from: FurnaceSlotRef,
        to: FurnaceSlotRef,
        quantity: Option<u16>,
    ) -> Vec<ServerEnvelope> {
        if from == to {
            return Vec::new();
        }
        let Some(furnace_id) = self.client_open_furnace(client_id) else {
            return Vec::new();
        };
        // Take from the source. `source_drained` tells the deposit path
        // whether the source slot is now empty: when it is, swap is
        // allowed (matches the player↔player Move semantics); when it
        // isn't, swap would strand the displaced item, so deposit
        // returns the incoming as leftover and the bounce-back
        // re-merges it with the partial source stack.
        let Some((taken, source_drained)) =
            self.take_from_furnace_slot(client_id, furnace_id, from, quantity)
        else {
            return Vec::new();
        };

        // Removing the fuel stack cancels the in-flight burn timer.
        // The burn bar reads `fuel_burn_ticks_left / max_burn_ticks`,
        // so without this reset the indicator stays high (up to 100%
        // when the denominator falls back to wood) while the previously
        // ignited unit ticks down invisibly. Pulling fuel out is the
        // player saying "I'm swapping this" - feedback should match.
        if matches!(from, FurnaceSlotRef::Fuel)
            && source_drained
            && let Some(furnace) = self
                .deployed_entities
                .get_mut(&furnace_id)
                .and_then(|entity| entity.furnace.as_mut())
        {
            furnace.fuel_burn_ticks_left = 0;
        }

        let leftover =
            self.deposit_into_furnace_slot(client_id, furnace_id, to, taken, source_drained);
        if let Some(remainder) = leftover {
            // Put the remainder back. Swap is irrelevant here — the
            // source slot is either empty (drained) or holds the same
            // item type we took out of it, so insert_stack_at will
            // place/merge cleanly. If anything still doesn't fit
            // (rare edge: e.g. fuel slot rejecting a swapped iron
            // bar), it drops at the player's feet — same pattern as
            // crafting refunds.
            let leftover2 =
                self.deposit_into_furnace_slot(client_id, furnace_id, from, remainder, false);
            if let Some(stack) = leftover2 {
                let drop_origin = self.clients.get(&client_id).map(|client| {
                    (
                        crate::server::movement::drop_position(&client.controller),
                        crate::server::movement::drop_velocity(&client.controller),
                        client.controller.yaw,
                    )
                });
                if let Some((position, velocity, yaw)) = drop_origin {
                    self.spawn_dropped_item(stack, position, velocity, yaw);
                }
            }
        }
        Vec::new()
    }

    fn client_open_furnace(&self, client_id: ClientId) -> Option<DeployedEntityId> {
        self.clients.get(&client_id).and_then(|c| c.open_furnace)
    }

    fn take_from_furnace_slot(
        &mut self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        slot: FurnaceSlotRef,
        quantity: Option<u16>,
    ) -> Option<(ItemStack, bool)> {
        match slot {
            FurnaceSlotRef::PlayerInventory(slot_index)
            | FurnaceSlotRef::PlayerActionbar(slot_index) => {
                let client = self.clients.get_mut(&client_id)?;
                let slot_ref = match slot {
                    FurnaceSlotRef::PlayerInventory(_) => {
                        client.inventory.inventory_slots.get_mut(slot_index)?
                    }
                    FurnaceSlotRef::PlayerActionbar(_) => {
                        client.inventory.actionbar_slots.get_mut(slot_index)?
                    }
                    _ => unreachable!(),
                };
                take_partial(slot_ref, quantity)
            }
            FurnaceSlotRef::Fuel => {
                let entity = self.deployed_entities.get_mut(&furnace_id)?;
                let furnace = entity.furnace.as_mut()?;
                take_partial(&mut furnace.fuel, quantity)
            }
            FurnaceSlotRef::Item(slot_index) => {
                let entity = self.deployed_entities.get_mut(&furnace_id)?;
                let furnace = entity.furnace.as_mut()?;
                let slot_ref = furnace.items.get_mut(slot_index)?;
                take_partial(slot_ref, quantity)
            }
        }
    }

    /// Place `stack` into `slot`. For player slots, the move respects
    /// the targeted slot index so a drag from furnace→inventory lands
    /// where the player aimed instead of falling through to a
    /// first-empty-slot scan. `allow_swap` mirrors the player↔player
    /// move rule: swap only when the source slot was fully drained,
    /// since a partial drag plus swap would strand the displaced item.
    fn deposit_into_furnace_slot(
        &mut self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        slot: FurnaceSlotRef,
        stack: ItemStack,
        allow_swap: bool,
    ) -> Option<ItemStack> {
        match slot {
            FurnaceSlotRef::PlayerInventory(index) => {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return Some(stack);
                };
                insert_stack_at(
                    &mut client.inventory,
                    ItemContainerSlot::inventory(index),
                    stack,
                    allow_swap,
                )
            }
            FurnaceSlotRef::PlayerActionbar(index) => {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return Some(stack);
                };
                insert_stack_at(
                    &mut client.inventory,
                    ItemContainerSlot::actionbar(index),
                    stack,
                    allow_swap,
                )
            }
            FurnaceSlotRef::Fuel => {
                let Some(entity) = self.deployed_entities.get_mut(&furnace_id) else {
                    return Some(stack);
                };
                let Some(furnace) = entity.furnace.as_mut() else {
                    return Some(stack);
                };
                // Reject non-fuel items so the player can't park a stack
                // of iron ore in the fuel slot. Better UX feedback than
                // letting it sit there doing nothing.
                if fuel_burn_ticks_for(stack.item_id.as_ref()).is_none() {
                    return Some(stack);
                }
                merge_into_optional_slot(&mut furnace.fuel, stack)
            }
            FurnaceSlotRef::Item(slot_index) => {
                let Some(entity) = self.deployed_entities.get_mut(&furnace_id) else {
                    return Some(stack);
                };
                let Some(furnace) = entity.furnace.as_mut() else {
                    return Some(stack);
                };
                let Some(target) = furnace.items.get_mut(slot_index) else {
                    return Some(stack);
                };
                merge_into_optional_slot(target, stack)
            }
        }
    }

    /// Advance every furnace one tick: burn fuel, smelt the head input,
    /// auto-shutoff when output won't fit. Called once per server tick.
    pub(super) fn tick_furnaces(&mut self) {
        for entity in self.deployed_entities.values_mut() {
            let Some(furnace) = entity.furnace.as_mut() else {
                continue;
            };
            if !furnace.active {
                continue;
            }
            tick_one_furnace(furnace);
        }
    }

    /// Build the per-client `open_furnace` view, if any, for the
    /// snapshot path.
    pub(super) fn open_furnace_view_for(&self, client_id: ClientId) -> Option<OpenFurnaceView> {
        let furnace_id = self.client_open_furnace(client_id)?;
        let entity = self.deployed_entities.get(&furnace_id)?;
        let furnace = entity.furnace.as_ref()?;
        Some(furnace.to_view(furnace_id))
    }
}

/// One-furnace tick. Pulled out so it can be unit-tested without spinning
/// up a `GameServer`.
fn tick_one_furnace(furnace: &mut FurnaceState) {
    // Ignite a fresh fuel unit if we need to and one is available.
    if furnace.fuel_burn_ticks_left == 0 {
        if let Some(ignited) = consume_one_fuel(furnace) {
            furnace.fuel_burn_ticks_left = ignited;
        } else {
            // No fuel → shut down. We don't keep smelt progress when
            // the burn stalls; this matches the SetActive-off rule and
            // means an "almost done" smelt can't sit indefinitely.
            furnace.active = false;
            furnace.smelt_progress_ticks = 0;
            return;
        }
    }

    let head_index = find_smeltable_head(furnace);
    let Some(head_index) = head_index else {
        // Nothing to smelt — auto-off so the player can tell at a
        // glance the furnace isn't doing work, and the fuel doesn't
        // burn down while idle.
        furnace.active = false;
        furnace.smelt_progress_ticks = 0;
        return;
    };

    // Pre-check that the output will fit before we spend the fuel
    // tick. If it won't, auto-shutoff is the spec.
    let head_stack = furnace.items[head_index]
        .as_ref()
        .expect("smeltable head must be Some");
    let result = smelt_result(head_stack.item_id.as_ref())
        .expect("find_smeltable_head guarantees a smelt result");
    if !output_fits_somewhere(furnace, head_index, &result) {
        furnace.active = false;
        furnace.smelt_progress_ticks = 0;
        return;
    }

    // Spend one tick of fuel and advance the smelt timer.
    furnace.fuel_burn_ticks_left = furnace.fuel_burn_ticks_left.saturating_sub(1);
    furnace.smelt_progress_ticks = furnace.smelt_progress_ticks.saturating_add(1);

    if furnace.smelt_progress_ticks >= SMELT_TICKS_PER_OUTPUT {
        // Consume one input + grant one output. Output-fit was
        // verified above so `deposit_smelt_output` is guaranteed to
        // place the item somewhere.
        let consumed_id = furnace.items[head_index]
            .as_ref()
            .map(|stack| stack.item_id.clone());
        if let Some(stack) = furnace.items[head_index].as_mut() {
            stack.quantity = stack.quantity.saturating_sub(1);
            if stack.quantity == 0 {
                furnace.items[head_index] = None;
            }
        }
        let _ = consumed_id;
        deposit_smelt_output(furnace, result);
        furnace.smelt_progress_ticks = 0;
    }
}

fn consume_one_fuel(furnace: &mut FurnaceState) -> Option<u32> {
    let stack = furnace.fuel.as_mut()?;
    let ticks = fuel_burn_ticks_for(stack.item_id.as_ref())?;
    stack.quantity = stack.quantity.saturating_sub(1);
    if stack.quantity == 0 {
        furnace.fuel = None;
    }
    Some(ticks)
}

fn find_smeltable_head(furnace: &FurnaceState) -> Option<usize> {
    furnace.items.iter().position(|slot| {
        slot.as_ref()
            .map(|stack| smelt_result(stack.item_id.as_ref()).is_some() && stack.quantity > 0)
            .unwrap_or(false)
    })
}

/// True if the smelt result can land somewhere — either merging into
/// an existing matching stack (anywhere in the grid) or filling an
/// empty slot. The current input's slot can take its own output once
/// the input runs to 0, which is the common case.
fn output_fits_somewhere(furnace: &FurnaceState, input_index: usize, result: &ItemStack) -> bool {
    let limit = crate::items::stack_limit(result.item_id.as_ref()).unwrap_or(u16::MAX);
    let consumed_clears_slot = furnace.items[input_index]
        .as_ref()
        .map(|stack| stack.quantity == 1)
        .unwrap_or(false);
    for (index, slot) in furnace.items.iter().enumerate() {
        let is_input_slot = index == input_index;
        match slot {
            None => return true,
            Some(existing) if is_input_slot && consumed_clears_slot => return true,
            Some(existing) => {
                if existing.item_id == result.item_id && existing.quantity < limit {
                    return true;
                }
            }
        }
    }
    false
}

fn deposit_smelt_output(furnace: &mut FurnaceState, output: ItemStack) {
    let limit = crate::items::stack_limit(output.item_id.as_ref()).unwrap_or(u16::MAX);
    // Try to merge with an existing matching stack first so the
    // output column packs tightly.
    for slot in furnace.items.iter_mut() {
        if let Some(existing) = slot
            && existing.item_id == output.item_id
            && existing.quantity < limit
        {
            existing.quantity = existing.quantity.saturating_add(output.quantity);
            return;
        }
    }
    // Fall back to the first empty slot.
    for slot in furnace.items.iter_mut() {
        if slot.is_none() {
            *slot = Some(output);
            return;
        }
    }
    // Output-fit was checked beforehand — this branch shouldn't trigger.
    debug_assert!(false, "deposit_smelt_output called with no room");
}

/// Pull up to `quantity` (or the whole stack if `quantity` is `None`)
/// out of `slot`. Returns the taken stack and whether the slot is now
/// empty — the caller uses the `drained` flag to decide whether a
/// downstream deposit is allowed to swap with an occupied target.
fn take_partial(slot: &mut Option<ItemStack>, quantity: Option<u16>) -> Option<(ItemStack, bool)> {
    let stack = slot.as_mut()?;
    let want = quantity.unwrap_or(stack.quantity).min(stack.quantity);
    if want == 0 {
        return None;
    }
    let taken = ItemStack {
        item_id: stack.item_id.clone(),
        quantity: want,
    };
    stack.quantity -= want;
    let drained = stack.quantity == 0;
    if drained {
        *slot = None;
    }
    Some((taken, drained))
}

/// Outcome of a fuel-slot deposit during quick transfer. `Placed` means
/// the source was consumed in full; `Rejected` returns the un-placed
/// stack so the caller can route it back to where it came from.
enum FuelPlaceOutcome {
    Placed,
    Rejected(ItemStack),
}

/// Spread `stack` across the furnace's items grid using the same rules
/// the player-inventory adder uses: existing matching stacks fill first
/// (capped by their item stack limit), then the first empty slot. Any
/// leftover that didn't fit comes back so the caller can decide what to
/// do with it (quick transfer routes it back to the source slot).
fn add_stack_to_furnace_items(
    furnace: &mut FurnaceState,
    mut remaining: ItemStack,
) -> Option<ItemStack> {
    let limit = crate::items::stack_limit(remaining.item_id.as_ref()).unwrap_or(u16::MAX);

    for slot in furnace.items.iter_mut() {
        if remaining.quantity == 0 {
            return None;
        }
        let Some(existing) = slot.as_mut() else {
            continue;
        };
        if existing.item_id != remaining.item_id {
            continue;
        }
        let space = limit.saturating_sub(existing.quantity);
        if space == 0 {
            continue;
        }
        let take = remaining.quantity.min(space);
        existing.quantity = existing.quantity.saturating_add(take);
        remaining.quantity -= take;
    }

    if remaining.quantity == 0 {
        return None;
    }

    for slot in furnace.items.iter_mut() {
        if slot.is_some() {
            continue;
        }
        let take = remaining.quantity.min(limit);
        *slot = Some(ItemStack {
            item_id: remaining.item_id.clone(),
            quantity: take,
        });
        remaining.quantity -= take;
        if remaining.quantity == 0 {
            return None;
        }
    }

    Some(remaining)
}

/// Would `stack` fit into the player's inventory + actionbar following
/// the standard "merge into matching stacks first, then take first
/// empty inventory slot" rule? Used as a dry-run before committing to a
/// fuel-slot swap so the displaced fuel doesn't get orphaned at the
/// player's feet.
fn inventory_has_room_for(client: Option<&ServerClient>, stack: &ItemStack) -> bool {
    let Some(client) = client else { return false };
    let limit = crate::items::stack_limit(stack.item_id.as_ref()).unwrap_or(u16::MAX);
    let mut remaining = stack.quantity;

    // Matching stacks (any container) absorb up to their stack limit.
    let matching_slots = client
        .inventory
        .actionbar_slots
        .iter()
        .chain(client.inventory.inventory_slots.iter());
    for slot in matching_slots {
        if remaining == 0 {
            return true;
        }
        let Some(existing) = slot.as_ref() else {
            continue;
        };
        if existing.item_id != stack.item_id {
            continue;
        }
        let space = limit.saturating_sub(existing.quantity);
        remaining = remaining.saturating_sub(space);
    }
    if remaining == 0 {
        return true;
    }
    // First-empty-slot fallback mirrors `add_stack_to_inventory`: only
    // the bag's empty slots count, not the actionbar's.
    client
        .inventory
        .inventory_slots
        .iter()
        .any(Option::is_none)
}

fn merge_into_optional_slot(
    target: &mut Option<ItemStack>,
    mut incoming: ItemStack,
) -> Option<ItemStack> {
    let limit = crate::items::stack_limit(incoming.item_id.as_ref()).unwrap_or(u16::MAX);
    match target {
        Some(existing) if existing.item_id == incoming.item_id => {
            let space = limit.saturating_sub(existing.quantity);
            if space == 0 {
                return Some(incoming);
            }
            let take = incoming.quantity.min(space);
            existing.quantity = existing.quantity.saturating_add(take);
            incoming.quantity -= take;
            if incoming.quantity == 0 {
                None
            } else {
                Some(incoming)
            }
        }
        Some(_) => Some(incoming),
        None => {
            // Honour stack limit even when placing into an empty slot.
            let take = incoming.quantity.min(limit);
            let placed = ItemStack {
                item_id: incoming.item_id.clone(),
                quantity: take,
            };
            *target = Some(placed);
            incoming.quantity -= take;
            if incoming.quantity == 0 {
                None
            } else {
                Some(incoming)
            }
        }
    }
}

fn furnace_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(kind, text)),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smeltable_input(item_id: &str, quantity: u16) -> Option<ItemStack> {
        Some(ItemStack::new(item_id, quantity))
    }

    #[test]
    fn iron_ore_smelts_to_iron_bar_consuming_fuel() {
        let mut furnace = FurnaceState {
            fuel: smeltable_input(WOOD_ID, 5),
            items: Default::default(),
            active: true,
            fuel_burn_ticks_left: 0,
            smelt_progress_ticks: 0,
        };
        furnace.items[0] = smeltable_input(IRON_ORE_ID, 2);

        // Smelt one output's worth of ticks.
        for _ in 0..SMELT_TICKS_PER_OUTPUT {
            tick_one_furnace(&mut furnace);
        }
        // One ore consumed.
        assert_eq!(
            furnace.items[0].as_ref().map(|s| s.quantity),
            Some(1),
            "one iron ore should have been consumed",
        );
        // One bar produced (lands in a slot somewhere).
        let bar_count: u16 = furnace
            .items
            .iter()
            .filter_map(|slot| slot.as_ref())
            .filter(|stack| stack.item_id.as_ref() == IRON_BAR_ID)
            .map(|stack| stack.quantity)
            .sum();
        assert_eq!(bar_count, 1, "one iron bar should have been produced");
        assert!(furnace.active, "furnace should remain active");
    }

    #[test]
    fn auto_shutoff_when_output_cannot_fit() {
        // Output slots filled with a non-matching item, no empty slots.
        let mut furnace = FurnaceState {
            fuel: smeltable_input(COAL_ID, 5),
            items: Default::default(),
            active: true,
            fuel_burn_ticks_left: 0,
            smelt_progress_ticks: 0,
        };
        furnace.items[0] = smeltable_input(IRON_ORE_ID, 5);
        // Fill the rest with stone (non-matching, not smeltable).
        for index in 1..FURNACE_ITEM_SLOT_COUNT {
            furnace.items[index] = smeltable_input("stone", 1);
        }

        tick_one_furnace(&mut furnace);
        assert!(
            !furnace.active,
            "furnace must auto-shutoff when output won't fit"
        );
    }

    #[test]
    fn auto_shutoff_when_no_fuel_and_smelt_pending() {
        let mut furnace = FurnaceState {
            fuel: None,
            items: Default::default(),
            active: true,
            fuel_burn_ticks_left: 0,
            smelt_progress_ticks: 0,
        };
        furnace.items[0] = smeltable_input(IRON_ORE_ID, 1);

        tick_one_furnace(&mut furnace);
        assert!(!furnace.active, "no fuel → auto-off");
    }

    #[test]
    fn auto_shutoff_when_nothing_to_smelt() {
        let mut furnace = FurnaceState {
            fuel: smeltable_input(WOOD_ID, 5),
            items: Default::default(),
            active: true,
            fuel_burn_ticks_left: 0,
            smelt_progress_ticks: 0,
        };
        tick_one_furnace(&mut furnace);
        assert!(!furnace.active, "no input → auto-off");
    }

    #[test]
    fn non_fuel_rejected_in_fuel_slot_via_merge_helper() {
        let mut slot: Option<ItemStack> = None;
        let leftover = merge_into_optional_slot(&mut slot, ItemStack::new(IRON_ORE_ID, 4));
        // The merge helper itself doesn't care about fuel - the gate
        // is in `deposit_into_furnace_slot`. This test guards that the
        // generic helper still works for non-fuel items so we can use
        // it everywhere.
        assert_eq!(slot.as_ref().map(|s| s.quantity), Some(4));
        assert!(leftover.is_none());
    }

    #[test]
    fn removing_fuel_resets_the_burn_timer() {
        use crate::{
            protocol::{GAME_VERSION, PROTOCOL_VERSION},
            save::WorldSave,
            server::ServerSettings,
            steam::{AuthMode, offline_auth_token},
        };

        let mut server = crate::server::GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        );
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Tester".to_owned(),
                offline_auth_token(1),
            )
            .expect("connect ok");

        // Seed a furnace with one wood unit already burning. The
        // visible fuel stack is what's left after ignition (so we
        // simulate "ignition already happened" by leaving the slot
        // empty but the burn timer hot).
        let entity_id = {
            let id = server.next_deployed_entity_id;
            server.next_deployed_entity_id += 1;
            // Simulate mid-burn: a unit was ignited some ticks back.
            let furnace = FurnaceState {
                fuel: Some(ItemStack::new(WOOD_ID, 5)),
                fuel_burn_ticks_left: WOOD_BURN_TICKS / 2,
                active: true,
                ..Default::default()
            };
            let entity = super::super::deployables::DeployedEntity {
                id,
                item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
                kind: crate::items::DeployableKind::Furnace { tier: 1 },
                position: crate::protocol::Vec3Net::ZERO,
                yaw: 0.0,
                health: 800,
                max_health: 800,
                furnace: Some(furnace),
            };
            server.deployed_entities.insert(id, entity);
            id
        };
        server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);

        // Player drags the entire fuel stack out into an empty
        // inventory slot.
        server.apply_furnace_command(
            client_id,
            FurnaceCommand::Move {
                from: FurnaceSlotRef::Fuel,
                to: FurnaceSlotRef::PlayerInventory(0),
                quantity: None,
            },
        );

        let furnace = server
            .deployed_entities
            .get(&entity_id)
            .unwrap()
            .furnace
            .as_ref()
            .unwrap();
        assert!(furnace.fuel.is_none(), "fuel slot should be empty");
        assert_eq!(
            furnace.fuel_burn_ticks_left, 0,
            "removing fuel must cancel the in-flight burn timer so the UI bar reads 0%",
        );
    }

    #[test]
    fn partial_fuel_drag_keeps_burn_timer_running() {
        // Pulling 1 unit out of a 5-stack is "adjusting", not
        // "removing" - the in-flight burn should keep going so the
        // player doesn't lose work for trimming the pile.
        use crate::{
            protocol::{GAME_VERSION, PROTOCOL_VERSION},
            save::WorldSave,
            server::ServerSettings,
            steam::{AuthMode, offline_auth_token},
        };

        let mut server = crate::server::GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        );
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Tester".to_owned(),
                offline_auth_token(1),
            )
            .expect("connect ok");

        let entity_id = {
            let id = server.next_deployed_entity_id;
            server.next_deployed_entity_id += 1;
            let furnace = FurnaceState {
                fuel: Some(ItemStack::new(WOOD_ID, 5)),
                fuel_burn_ticks_left: WOOD_BURN_TICKS / 2,
                active: true,
                ..Default::default()
            };
            let entity = super::super::deployables::DeployedEntity {
                id,
                item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
                kind: crate::items::DeployableKind::Furnace { tier: 1 },
                position: crate::protocol::Vec3Net::ZERO,
                yaw: 0.0,
                health: 800,
                max_health: 800,
                furnace: Some(furnace),
            };
            server.deployed_entities.insert(id, entity);
            id
        };
        server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);

        server.apply_furnace_command(
            client_id,
            FurnaceCommand::Move {
                from: FurnaceSlotRef::Fuel,
                to: FurnaceSlotRef::PlayerInventory(0),
                quantity: Some(1),
            },
        );

        let furnace = server
            .deployed_entities
            .get(&entity_id)
            .unwrap()
            .furnace
            .as_ref()
            .unwrap();
        assert_eq!(
            furnace.fuel.as_ref().map(|s| s.quantity),
            Some(4),
            "partial drag should leave 4 wood",
        );
        assert_eq!(
            furnace.fuel_burn_ticks_left,
            WOOD_BURN_TICKS / 2,
            "partial drag should not cancel the in-flight burn timer",
        );
    }

    #[test]
    fn moving_from_furnace_to_a_specific_player_inventory_slot_respects_the_target() {
        use crate::{
            protocol::{GAME_VERSION, PROTOCOL_VERSION},
            save::WorldSave,
            server::ServerSettings,
            steam::{AuthMode, offline_auth_token},
        };

        let mut server = crate::server::GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        );
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Tester".to_owned(),
                offline_auth_token(1),
            )
            .expect("connect ok");

        // Spawn a placed furnace directly in the server's tables (skip
        // the place command machinery — this test is about the move
        // path).
        let entity_id = {
            let id = server.next_deployed_entity_id;
            server.next_deployed_entity_id += 1;
            let entity = super::super::deployables::DeployedEntity {
                id,
                item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
                kind: crate::items::DeployableKind::Furnace { tier: 1 },
                position: crate::protocol::Vec3Net::ZERO,
                yaw: 0.0,
                health: 800,
                max_health: 800,
                furnace: Some(FurnaceState::default()),
            };
            server.deployed_entities.insert(id, entity);
            id
        };
        // Seed an iron bar into the furnace's first item slot.
        {
            let furnace = server
                .deployed_entities
                .get_mut(&entity_id)
                .unwrap()
                .furnace
                .as_mut()
                .unwrap();
            furnace.items[0] = Some(ItemStack::new(IRON_BAR_ID, 7));
        }
        // Mark the client as having this furnace open so the move
        // command is accepted.
        server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);

        // Pick a specific empty inventory slot (anything past 0 — the
        // bug was that the first empty slot got the item regardless
        // of the target). Slot index 5 is well into the grid.
        const TARGET: usize = 5;
        server.apply_furnace_command(
            client_id,
            FurnaceCommand::Move {
                from: FurnaceSlotRef::Item(0),
                to: FurnaceSlotRef::PlayerInventory(TARGET),
                quantity: None,
            },
        );

        let client = server.clients.get(&client_id).unwrap();
        // Iron bars landed in the targeted slot...
        let landed = client.inventory.inventory_slots[TARGET]
            .as_ref()
            .expect("target slot should be filled");
        assert_eq!(landed.item_id.as_ref(), IRON_BAR_ID);
        assert_eq!(landed.quantity, 7);
        // ...and not in slot 0 where add_stack_to_inventory would
        // have shoved them.
        for (index, slot) in client.inventory.inventory_slots.iter().enumerate() {
            if index == TARGET {
                continue;
            }
            assert!(
                slot.as_ref()
                    .map(|s| s.item_id.as_ref() != IRON_BAR_ID)
                    .unwrap_or(true),
                "iron bar should not appear in slot {index}; bug would have put it here",
            );
        }
        // Furnace slot 0 is now empty.
        let furnace = server
            .deployed_entities
            .get(&entity_id)
            .unwrap()
            .furnace
            .as_ref()
            .unwrap();
        assert!(furnace.items[0].is_none());
    }

    /// Boilerplate-free fixture for the QuickTransfer tests. Spins up a
    /// server, connects one client, spawns a furnace, sets it as their
    /// open furnace, and returns both ids so the test body can mutate
    /// the relevant slots before issuing the shift-click command.
    fn furnace_test_fixture() -> (
        crate::server::GameServer,
        crate::protocol::ClientId,
        crate::protocol::DeployedEntityId,
    ) {
        use crate::{
            protocol::{GAME_VERSION, PROTOCOL_VERSION},
            save::WorldSave,
            server::ServerSettings,
            steam::{AuthMode, offline_auth_token},
        };

        let mut server = crate::server::GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        );
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Tester".to_owned(),
                offline_auth_token(1),
            )
            .expect("connect ok");

        let entity_id = server.next_deployed_entity_id;
        server.next_deployed_entity_id += 1;
        let entity = super::super::deployables::DeployedEntity {
            id: entity_id,
            item_id: crate::items::intern_item_id(crate::items::CRUDE_FURNACE_ID),
            kind: crate::items::DeployableKind::Furnace { tier: 1 },
            position: crate::protocol::Vec3Net::ZERO,
            yaw: 0.0,
            health: 800,
            max_health: 800,
            furnace: Some(FurnaceState::default()),
        };
        server.deployed_entities.insert(entity_id, entity);
        server.clients.get_mut(&client_id).unwrap().open_furnace = Some(entity_id);
        (server, client_id, entity_id)
    }

    fn client_inventory_slot(
        server: &crate::server::GameServer,
        client_id: crate::protocol::ClientId,
        index: usize,
    ) -> Option<&ItemStack> {
        server.clients[&client_id].inventory.inventory_slots[index].as_ref()
    }

    fn furnace_item_slot(
        server: &crate::server::GameServer,
        entity_id: crate::protocol::DeployedEntityId,
        index: usize,
    ) -> Option<&ItemStack> {
        server.deployed_entities[&entity_id]
            .furnace
            .as_ref()
            .unwrap()
            .items[index]
            .as_ref()
    }

    fn furnace_fuel_slot(
        server: &crate::server::GameServer,
        entity_id: crate::protocol::DeployedEntityId,
    ) -> Option<&ItemStack> {
        server.deployed_entities[&entity_id]
            .furnace
            .as_ref()
            .unwrap()
            .fuel
            .as_ref()
    }

    #[test]
    fn quick_transfer_routes_fuel_from_player_to_fuel_slot() {
        let (mut server, client_id, entity_id) = furnace_test_fixture();
        server.clients.get_mut(&client_id).unwrap().inventory.inventory_slots[2] =
            Some(ItemStack::new(WOOD_ID, 12));

        server.apply_furnace_command(
            client_id,
            FurnaceCommand::QuickTransfer {
                from: FurnaceSlotRef::PlayerInventory(2),
            },
        );

        assert!(client_inventory_slot(&server, client_id, 2).is_none());
        let fuel = furnace_fuel_slot(&server, entity_id).expect("fuel placed");
        assert_eq!(fuel.item_id.as_ref(), WOOD_ID);
        assert_eq!(fuel.quantity, 12);
    }

    #[test]
    fn quick_transfer_routes_smeltable_from_player_to_first_empty_item_slot() {
        let (mut server, client_id, entity_id) = furnace_test_fixture();
        server.clients.get_mut(&client_id).unwrap().inventory.inventory_slots[5] =
            Some(ItemStack::new(IRON_ORE_ID, 8));

        server.apply_furnace_command(
            client_id,
            FurnaceCommand::QuickTransfer {
                from: FurnaceSlotRef::PlayerInventory(5),
            },
        );

        assert!(client_inventory_slot(&server, client_id, 5).is_none());
        let ore = furnace_item_slot(&server, entity_id, 0).expect("ore landed");
        assert_eq!(ore.item_id.as_ref(), IRON_ORE_ID);
        assert_eq!(ore.quantity, 8);
    }

    #[test]
    fn quick_transfer_merges_into_existing_furnace_stack_before_taking_empty_slot() {
        let (mut server, client_id, entity_id) = furnace_test_fixture();
        // Furnace already has 50 ore in slot 1; player has 30 more.
        {
            let furnace = server.deployed_entities.get_mut(&entity_id).unwrap()
                .furnace.as_mut().unwrap();
            furnace.items[1] = Some(ItemStack::new(IRON_ORE_ID, 50));
        }
        server.clients.get_mut(&client_id).unwrap().inventory.inventory_slots[0] =
            Some(ItemStack::new(IRON_ORE_ID, 30));

        server.apply_furnace_command(
            client_id,
            FurnaceCommand::QuickTransfer {
                from: FurnaceSlotRef::PlayerInventory(0),
            },
        );

        // Slot 1 absorbed everything; slot 0 stays empty.
        assert!(furnace_item_slot(&server, entity_id, 0).is_none());
        assert_eq!(
            furnace_item_slot(&server, entity_id, 1).unwrap().quantity,
            80,
            "matching stack should fill before an empty slot is consumed",
        );
        assert!(client_inventory_slot(&server, client_id, 0).is_none());
    }

    #[test]
    fn quick_transfer_swaps_fuel_when_a_different_fuel_is_present() {
        let (mut server, client_id, entity_id) = furnace_test_fixture();
        // Furnace has coal; player has wood.
        {
            let furnace = server.deployed_entities.get_mut(&entity_id).unwrap()
                .furnace.as_mut().unwrap();
            furnace.fuel = Some(ItemStack::new(COAL_ID, 4));
            furnace.fuel_burn_ticks_left = 200;
        }
        server.clients.get_mut(&client_id).unwrap().inventory.inventory_slots[0] =
            Some(ItemStack::new(WOOD_ID, 5));

        server.apply_furnace_command(
            client_id,
            FurnaceCommand::QuickTransfer {
                from: FurnaceSlotRef::PlayerInventory(0),
            },
        );

        // Fuel slot now holds wood, the player has the displaced coal,
        // and the in-flight burn timer was reset (a different fuel
        // changes the denominator of the burn bar).
        assert_eq!(
            furnace_fuel_slot(&server, entity_id).unwrap().item_id.as_ref(),
            WOOD_ID,
        );
        // Coal landed somewhere in the player's bag.
        let coal_total: u16 = server.clients[&client_id]
            .inventory
            .inventory_slots
            .iter()
            .chain(server.clients[&client_id].inventory.actionbar_slots.iter())
            .filter_map(|s| s.as_ref())
            .filter(|s| s.item_id.as_ref() == COAL_ID)
            .map(|s| s.quantity)
            .sum();
        assert_eq!(coal_total, 4);
        assert_eq!(
            server.deployed_entities[&entity_id].furnace.as_ref().unwrap().fuel_burn_ticks_left,
            0,
            "swap should reset the in-flight burn timer",
        );
    }

    #[test]
    fn quick_transfer_rejects_fuel_swap_when_player_has_no_room() {
        let (mut server, client_id, entity_id) = furnace_test_fixture();
        // To force a rejection we need the displaced fuel (coal) to
        // have *no* valid landing spot in the player's bag. The trick:
        // park the source wood in the actionbar — when we drain it the
        // freed slot is in the actionbar, and `add_stack_to_inventory`
        // only falls back to empty *inventory* slots (intentional, so
        // pickup doesn't randomly stuff items into the toolbar). With
        // every inventory slot also full of non-matching stone, the
        // displaced coal has nowhere to go.
        {
            let inv = &mut server.clients.get_mut(&client_id).unwrap().inventory;
            for slot in inv.inventory_slots.iter_mut() {
                *slot = Some(ItemStack::new(crate::items::STONE_ID, 200));
            }
            for slot in inv.actionbar_slots.iter_mut() {
                *slot = Some(ItemStack::new(crate::items::STONE_ID, 200));
            }
            inv.actionbar_slots[3] = Some(ItemStack::new(WOOD_ID, 5));
        }
        {
            let furnace = server.deployed_entities.get_mut(&entity_id).unwrap()
                .furnace.as_mut().unwrap();
            furnace.fuel = Some(ItemStack::new(COAL_ID, 4));
        }

        server.apply_furnace_command(
            client_id,
            FurnaceCommand::QuickTransfer {
                from: FurnaceSlotRef::PlayerActionbar(3),
            },
        );

        // Swap aborted: fuel slot still has coal, source slot still has
        // wood. No silent drop, no orphaned items.
        assert_eq!(
            furnace_fuel_slot(&server, entity_id).unwrap().item_id.as_ref(),
            COAL_ID,
        );
        assert_eq!(
            server.clients[&client_id].inventory.actionbar_slots[3]
                .as_ref()
                .unwrap()
                .item_id
                .as_ref(),
            WOOD_ID,
        );
    }

    #[test]
    fn quick_transfer_routes_furnace_item_back_into_player_inventory() {
        let (mut server, client_id, entity_id) = furnace_test_fixture();
        {
            let furnace = server.deployed_entities.get_mut(&entity_id).unwrap()
                .furnace.as_mut().unwrap();
            furnace.items[2] = Some(ItemStack::new(IRON_BAR_ID, 7));
        }

        server.apply_furnace_command(
            client_id,
            FurnaceCommand::QuickTransfer {
                from: FurnaceSlotRef::Item(2),
            },
        );

        assert!(furnace_item_slot(&server, entity_id, 2).is_none());
        // Iron bar ends up in the first empty inventory slot.
        let bar = client_inventory_slot(&server, client_id, 0).expect("bar landed");
        assert_eq!(bar.item_id.as_ref(), IRON_BAR_ID);
        assert_eq!(bar.quantity, 7);
    }
}
