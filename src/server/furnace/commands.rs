//! Furnace interaction commands: open/close, set-active, slot moves,
//! and shift-click quick transfer. All entry points re-validate the
//! player's distance to the open furnace so a client whose UI persists
//! after they walked away can't move items out of line-of-sight.

use crate::protocol::{
    ClientId, DeployedEntityId, FurnaceCommand, FurnaceSlotRef, ItemContainerSlot, ItemStack,
    OpenFurnaceView, ServerMessage, ToastKind, ToastMessage,
};

use crate::server::{
    DeliveryTarget, GameServer, ServerEnvelope,
    container_slots::Container,
    inventory::{add_stack_to_inventory, insert_stack_at},
};

use super::state::{
    FUEL_SLOT_INDEX, FURNACE_INTERACT_RANGE_M, FuelPlaceOutcome, FurnaceContainer,
    add_stack_to_furnace_items, fuel_burn_ticks_for, inventory_has_room_for,
    merge_into_optional_slot, take_partial,
};

impl GameServer {
    pub(in crate::server) fn apply_furnace_command(
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
                if !self.open_furnace_in_range(client_id) {
                    self.close_furnace(client_id);
                    return Vec::new();
                }
                self.set_open_furnace_active(client_id, active);
                Vec::new()
            }
            FurnaceCommand::Move { from, to, quantity } => {
                if !self.open_furnace_in_range(client_id) {
                    self.close_furnace(client_id);
                    return Vec::new();
                }
                self.move_in_furnace(client_id, from, to, quantity)
            }
            FurnaceCommand::QuickTransfer { from } => {
                if !self.open_furnace_in_range(client_id) {
                    self.close_furnace(client_id);
                    return Vec::new();
                }
                self.quick_transfer(client_id, from)
            }
        }
    }

    /// Re-validate that the client's currently-open furnace is still
    /// within interact range. Returns `true` if there is no open furnace
    /// (no constraint to enforce) or if the player is in range. Returns
    /// `false` if the open furnace exists in the world but the player has
    /// walked away, caller should `close_furnace` and drop the command.
    fn open_furnace_in_range(&self, client_id: ClientId) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let Some(furnace_id) = client.open_furnace else {
            return true;
        };
        let Some(entity) = self.deployed_entities.get(&furnace_id) else {
            return false;
        };
        client
            .controller
            .position
            .within_horizontal_range(entity.position, FURNACE_INTERACT_RANGE_M)
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
    /// silent drops on the ground, the player asked for a transfer, not
    /// a discard.
    fn quick_transfer(&mut self, client_id: ClientId, from: FurnaceSlotRef) -> Vec<ServerEnvelope> {
        let Some(furnace_id) = self.client_open_furnace(client_id) else {
            return Vec::new();
        };
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
    /// the borrow doesn't tangle with subsequent mutating calls.
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

        let Some((mut taken, _)) = self.take_from_furnace_slot(client_id, furnace_id, from, None)
        else {
            return;
        };

        if is_fuel {
            taken = match self.place_in_fuel_slot_with_swap(client_id, furnace_id, taken) {
                FuelPlaceOutcome::Placed => return,
                FuelPlaceOutcome::Rejected(stack) => stack,
            };
        } else {
            let Some(entity) = self.deployed_entity_mut(furnace_id) else {
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

        self.restore_to_source(client_id, furnace_id, from, taken);
    }

    fn quick_transfer_furnace_to_player(
        &mut self,
        client_id: ClientId,
        furnace_id: DeployedEntityId,
        from: FurnaceSlotRef,
    ) {
        let Some((taken, _)) = self.take_from_furnace_slot(client_id, furnace_id, from, None)
        else {
            return;
        };
        let leftover = match self.clients.get_mut(&client_id) {
            Some(client) => add_stack_to_inventory(&mut client.inventory, taken),
            None => return,
        };
        if let Some(remainder) = leftover {
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
        // Direct field borrow (not `deployed_entity_mut`) because the swap
        // branch below also reads `self.clients` while `furnace` is live;
        // the explicit mark keeps the mirror-sync contract intact.
        self.mark_deployable_dirty(furnace_id);
        let Some(entity) = self.deployed_entities.get_mut(&furnace_id) else {
            return FuelPlaceOutcome::Rejected(stack);
        };
        let Some(furnace) = entity.furnace.as_mut() else {
            return FuelPlaceOutcome::Rejected(stack);
        };

        match furnace.fuel.as_ref().map(|s| s.item_id.clone()) {
            Some(existing_id) if existing_id == stack.item_id => {
                match merge_into_optional_slot(&mut furnace.fuel, stack) {
                    None => FuelPlaceOutcome::Placed,
                    Some(remainder) => FuelPlaceOutcome::Rejected(remainder),
                }
            }
            Some(_) => {
                let displaced = furnace
                    .fuel
                    .clone()
                    .expect("fuel slot non-empty by match arm");
                if !inventory_has_room_for(self.clients.get(&client_id), &displaced) {
                    return FuelPlaceOutcome::Rejected(stack);
                }
                furnace.fuel_burn_ticks_left = 0;
                furnace.fuel = Some(stack);
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
    /// drained. If the slot can't take it (rare edge after a save
    /// migration), the items spawn at the player's feet rather than
    /// vanish.
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
            let drop_origin = self
                .clients
                .get(&client_id)
                .map(crate::server::movement::drop_origin_for);
            if let Some(origin) = drop_origin {
                self.spawn_dropped_item_at(origin, leftover);
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
        if !player_pos.within_horizontal_range(entity.position, FURNACE_INTERACT_RANGE_M) {
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

    pub(in crate::server) fn close_furnace(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.open_furnace = None;
        }
    }

    fn set_open_furnace_active(&mut self, client_id: ClientId, active: bool) {
        let Some(furnace_id) = self.client_open_furnace(client_id) else {
            return;
        };
        // `deployed_entity_mut` flags the entity dirty so the mirror
        // re-syncs `DeployableActive` next pass.
        let Some(entity) = self.deployed_entity_mut(furnace_id) else {
            return;
        };
        let Some(furnace) = entity.furnace.as_mut() else {
            return;
        };
        furnace.active = active;
        if !active {
            // Pausing snaps the smelt progress back to zero so the
            // player can't "save" a 90%-smelted timer by flipping the
            // switch off and on.
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
        let Some((taken, source_drained)) =
            self.take_from_furnace_slot(client_id, furnace_id, from, quantity)
        else {
            return Vec::new();
        };

        // Fire the container's post-take hook so the furnace can cancel the
        // in-flight burn timer when the fuel slot empties (see
        // `FurnaceContainer::after_take`). Only container-side takes have a
        // hook; player-slot takes don't.
        if let Some(index) = container_index(from)
            && let Some(furnace) = self
                .deployed_entity_mut(furnace_id)
                .and_then(|entity| entity.furnace.as_mut())
        {
            FurnaceContainer(furnace).after_take(index, source_drained);
        }

        let leftover =
            self.deposit_into_furnace_slot(client_id, furnace_id, to, taken, source_drained);
        if let Some(remainder) = leftover {
            let leftover2 =
                self.deposit_into_furnace_slot(client_id, furnace_id, from, remainder, false);
            if let Some(stack) = leftover2 {
                let drop_origin = self
                    .clients
                    .get(&client_id)
                    .map(crate::server::movement::drop_origin_for);
                if let Some(origin) = drop_origin {
                    self.spawn_dropped_item_at(origin, stack);
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
            FurnaceSlotRef::Fuel | FurnaceSlotRef::Item(_) => {
                let index = container_index(slot)?;
                let entity = self.deployed_entity_mut(furnace_id)?;
                let furnace = entity.furnace.as_mut()?;
                let mut container = FurnaceContainer(furnace);
                let slot_ref = container.slot_mut(index)?;
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
            FurnaceSlotRef::Fuel | FurnaceSlotRef::Item(_) => {
                let Some(index) = container_index(slot) else {
                    return Some(stack);
                };
                let Some(entity) = self.deployed_entity_mut(furnace_id) else {
                    return Some(stack);
                };
                let Some(furnace) = entity.furnace.as_mut() else {
                    return Some(stack);
                };
                let mut container = FurnaceContainer(furnace);
                // An out-of-range item index keeps the stack instead of dropping
                // it; `Container::insert` can't tell us OOB from "placed", so the
                // range check stays explicit here.
                if index >= container.slot_count() {
                    return Some(stack);
                }
                container.insert(index, stack)
            }
        }
    }

    /// Build the per-client `open_furnace` view, if any, for the
    /// per-component replication path.
    pub(in crate::server) fn open_furnace_view_for(
        &self,
        client_id: ClientId,
    ) -> Option<OpenFurnaceView> {
        let furnace_id = self.client_open_furnace(client_id)?;
        let entity = self.deployed_entities.get(&furnace_id)?;
        let furnace = entity.furnace.as_ref()?;
        Some(furnace.to_view(furnace_id))
    }
}

/// Flat [`Container`] index for a furnace-side slot ref, or `None` for a player
/// slot. `Fuel` is index `0`; `Item(i)` is `i + 1`, matching
/// [`FurnaceContainer`]'s index space.
fn container_index(slot: FurnaceSlotRef) -> Option<usize> {
    match slot {
        FurnaceSlotRef::Fuel => Some(FUEL_SLOT_INDEX),
        FurnaceSlotRef::Item(index) => Some(index + 1),
        FurnaceSlotRef::PlayerInventory(_) | FurnaceSlotRef::PlayerActionbar(_) => None,
    }
}

fn furnace_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(kind, text)),
    }]
}
