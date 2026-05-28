//! Server-authoritative loot bag state + open/close/move commands.
//!
//! A loot bag is the container spawned at a dead player's feet — it
//! holds every item the corpse was carrying so a killer can scoop up
//! the kill in one stop instead of running a vacuum over a pile of
//! individual `DroppedWorldItem`s. The bag persists until either:
//!   - all slots are empty AND the holder closes the UI, or
//!   - a future cleanup pass (lifetime-based) sweeps it.
//!
//! Command shape mirrors `FurnaceCommand` because the UI semantics
//! are identical (open one container, drag stacks between it and the
//! player). The bag has no smelt loop, no fuel slot, no active flag.

use std::collections::HashMap;

use crate::{
    items::normalize_stack,
    protocol::{
        ClientId, ItemStack, LOOT_BAG_SLOT_COUNT, LootBagCommand, LootBagId, LootBagSlotRef,
        OpenLootBagView, ToastKind, ToastMessage, Vec3Net,
    },
    server::{DeliveryTarget, GameServer, ServerEnvelope},
};

/// Max range, in metres, at which a player can open / interact with
/// a loot bag. Loosened from the swing range so a kill that knocks
/// the corpse a step or two doesn't put the loot out of reach.
pub(crate) use crate::game_balance::LOOT_BAG_INTERACT_RANGE_M;

/// Vertical offset above the dead player's feet where the bag
/// spawns. Roughly waist-height so the bag falls naturally instead of
/// materialising on the ground.
const BAG_SPAWN_HEIGHT_M: f32 = 1.0;
/// Initial downward velocity, in m/s, applied at spawn. A small
/// upward kick from gravity-only integration looks lifeless; giving
/// the bag a tiny initial drop reads as "it slumps off the corpse".
const BAG_SPAWN_VERTICAL_VELOCITY: f32 = -0.4;
/// Gravity applied to settling bags. Matches the controller's gravity
/// so the visible fall matches the player's frame of reference.
const BAG_GRAVITY: f32 = 18.0;
/// Resting Y position for a bag once it lands. Slightly above zero so
/// the visual mesh's lower face doesn't z-fight with the floor.
const BAG_RESTING_Y: f32 = 0.05;

/// Authoritative record of a loot bag in the world. Stored in
/// `GameServer::loot_bags` keyed by `LootBagId`. Fields are crate-
/// visible because the net-host mirror reads them when building the
/// per-tick replication snapshot.
#[derive(Debug, Clone)]
pub struct LootBag {
    pub(crate) id: LootBagId,
    pub(crate) position: Vec3Net,
    pub(crate) yaw: f32,
    pub(crate) slots: Vec<Option<ItemStack>>,
    /// Tick the bag was created on. The client uses death-time as a
    /// "I just killed them" handle for UI cues (loot glints, etc.)
    /// in the future; today this is just bookkeeping.
    #[allow(dead_code)]
    pub(crate) spawn_tick: u64,
    /// Vertical velocity for the spawn-time gravity settle. `0.0`
    /// once the bag is at rest; non-zero while the death drop is
    /// still falling from chest height. Horizontal velocity isn't
    /// tracked — bags fall straight down.
    pub(crate) velocity_y: f32,
    /// True once the bag has touched the ground. Skips the
    /// per-tick integration in [`GameServer::tick_loot_bags`] for
    /// resting bags so the cost stays at O(spawned-this-tick) instead
    /// of O(every-bag-ever-spawned).
    pub(crate) resting: bool,
}

impl LootBag {
    pub(super) fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    pub(super) fn to_view(&self) -> OpenLootBagView {
        OpenLootBagView {
            id: self.id,
            slots: self.slots.clone(),
        }
    }
}

impl GameServer {
    /// Authoritative bag-spawn entry point. Used by the death chain
    /// in `combat.rs` to drop the corpse's items as a single bag.
    /// Empty input is allowed but pointless — the caller is expected
    /// to filter before calling. Returns the new bag id.
    pub(crate) fn spawn_loot_bag(
        &mut self,
        position: Vec3Net,
        yaw: f32,
        items: Vec<ItemStack>,
    ) -> LootBagId {
        let id = self.allocate_loot_bag_id();
        let mut slots: Vec<Option<ItemStack>> = vec![None; LOOT_BAG_SLOT_COUNT];
        for (index, stack) in items.into_iter().enumerate() {
            if index >= slots.len() {
                break;
            }
            slots[index] = normalize_stack(&stack);
        }
        // Spawn at chest height so the bag visibly falls off the
        // corpse instead of materialising on the ground. Gravity
        // ticks below pull it down to `BAG_RESTING_Y`.
        let spawn_position = Vec3Net::new(position.x, position.y + BAG_SPAWN_HEIGHT_M, position.z);
        let bag = LootBag {
            id,
            position: spawn_position,
            yaw,
            slots,
            spawn_tick: self.tick,
            velocity_y: BAG_SPAWN_VERTICAL_VELOCITY,
            resting: false,
        };
        self.loot_bags.insert(id, bag);
        self.chunk_manager.track_loot_bag(id, spawn_position);
        id
    }

    /// Integrate every in-flight loot bag's gravity drop by one tick.
    /// At-rest bags are skipped so the per-tick cost stays at
    /// O(spawned-this-tick) instead of O(every-bag-ever-spawned).
    pub(crate) fn tick_loot_bags(&mut self, delta_seconds: f32) {
        let dt = delta_seconds.clamp(0.0, 0.1);
        if dt <= 0.0 {
            return;
        }
        for bag in self.loot_bags.values_mut() {
            if bag.resting {
                continue;
            }
            bag.velocity_y -= BAG_GRAVITY * dt;
            bag.position.y += bag.velocity_y * dt;
            if bag.position.y <= BAG_RESTING_Y {
                bag.position.y = BAG_RESTING_Y;
                bag.velocity_y = 0.0;
                bag.resting = true;
            }
        }
    }

    pub(super) fn apply_loot_bag_command(
        &mut self,
        client_id: ClientId,
        command: LootBagCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            LootBagCommand::Open { id } => self.open_loot_bag(client_id, id),
            LootBagCommand::Close => {
                self.close_loot_bag(client_id);
                Vec::new()
            }
            LootBagCommand::Move { from, to, quantity } => {
                if !self.open_loot_bag_in_range(client_id) {
                    self.close_loot_bag(client_id);
                    return Vec::new();
                }
                self.move_loot_bag_stack(client_id, from, to, quantity)
            }
            LootBagCommand::QuickTransfer { from } => {
                if !self.open_loot_bag_in_range(client_id) {
                    self.close_loot_bag(client_id);
                    return Vec::new();
                }
                self.quick_transfer_loot_bag(client_id, from)
            }
        }
    }

    /// Re-validate that the client's currently-open loot bag is still
    /// within interact range. The bag UI can persist client-side while
    /// the player walks away; without this check, stacks could be moved
    /// out of line-of-sight.
    fn open_loot_bag_in_range(&self, client_id: ClientId) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let Some(bag_id) = client.open_loot_bag else {
            return true;
        };
        let Some(bag) = self.loot_bags.get(&bag_id) else {
            return false;
        };
        let dx = bag.position.x - client.controller.position.x;
        let dz = bag.position.z - client.controller.position.z;
        (dx * dx + dz * dz).sqrt() <= LOOT_BAG_INTERACT_RANGE_M
    }

    /// Quick membership / view helper for the player-private replication
    /// path.
    pub(crate) fn open_loot_bag_view_for(&self, client_id: ClientId) -> Option<OpenLootBagView> {
        let bag_id = self.clients.get(&client_id)?.open_loot_bag?;
        self.loot_bags.get(&bag_id).map(LootBag::to_view)
    }

    pub(crate) fn loot_bags_iter(&self) -> impl Iterator<Item = (LootBagId, &LootBag)> + '_ {
        self.loot_bags.iter().map(|(id, bag)| (*id, bag))
    }

    pub(crate) fn loot_bag_chunk(&self, id: LootBagId) -> Option<crate::world::ChunkCoord> {
        self.chunk_manager.loot_bag_chunk(id)
    }

    fn open_loot_bag(&mut self, client_id: ClientId, id: LootBagId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let player_pos = client.controller.position;
        let Some(bag) = self.loot_bags.get(&id) else {
            return Vec::new();
        };
        let dx = bag.position.x - player_pos.x;
        let dz = bag.position.z - player_pos.z;
        if (dx * dx + dz * dz).sqrt() > LOOT_BAG_INTERACT_RANGE_M {
            return Vec::new();
        }
        if let Some(client_mut) = self.clients.get_mut(&client_id) {
            client_mut.open_loot_bag = Some(id);
        }
        Vec::new()
    }

    pub(crate) fn close_loot_bag(&mut self, client_id: ClientId) {
        // Take the open id out of the client record first so the
        // "is the bag empty after this player walked away from it?"
        // check below sees the up-to-date open state.
        let opened = self
            .clients
            .get_mut(&client_id)
            .and_then(|c| c.open_loot_bag.take());
        let Some(bag_id) = opened else {
            return;
        };
        // If no other client has this bag open and it's empty, the
        // entity is just litter — clean it up.
        let still_open_elsewhere = self
            .clients
            .values()
            .any(|c| c.open_loot_bag == Some(bag_id));
        if still_open_elsewhere {
            return;
        }
        if self
            .loot_bags
            .get(&bag_id)
            .map(LootBag::is_empty)
            .unwrap_or(false)
        {
            self.destroy_loot_bag(bag_id);
        }
    }

    pub(crate) fn destroy_loot_bag(&mut self, id: LootBagId) {
        if self.loot_bags.remove(&id).is_none() {
            return;
        }
        self.chunk_manager.untrack_loot_bag(id);
        // Clear any client's pointer at this id so a stale Move
        // doesn't reach into a removed bag.
        for client in self.clients.values_mut() {
            if client.open_loot_bag == Some(id) {
                client.open_loot_bag = None;
            }
        }
    }

    fn move_loot_bag_stack(
        &mut self,
        client_id: ClientId,
        from: LootBagSlotRef,
        to: LootBagSlotRef,
        quantity: Option<u16>,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let Some(bag_id) = client.open_loot_bag else {
            return Vec::new();
        };
        let Some(bag) = self.loot_bags.get_mut(&bag_id) else {
            return Vec::new();
        };

        // Pull the source stack out into a temporary.
        let Some(removed) = take_from_loot_ref(&mut self.clients, client_id, bag, from, quantity)
        else {
            return Vec::new();
        };

        // Try to insert into the destination. If the dest write fails,
        // restore the removed amount at the source so the move is
        // atomic.
        let (took, all_consumed) = removed;
        let restore = insert_into_loot_ref(&mut self.clients, client_id, bag, to, took.clone());
        if let Some(remainder) = restore {
            restore_into_loot_ref(
                &mut self.clients,
                client_id,
                bag,
                from,
                remainder,
                all_consumed,
            );
        }
        Vec::new()
    }

    fn quick_transfer_loot_bag(
        &mut self,
        client_id: ClientId,
        from: LootBagSlotRef,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let Some(bag_id) = client.open_loot_bag else {
            return Vec::new();
        };
        let bag_present = self.loot_bags.contains_key(&bag_id);
        if !bag_present {
            return Vec::new();
        }

        match from {
            LootBagSlotRef::Bag(slot) => {
                // From bag → first empty inventory slot, merging into
                // a matching stack first.
                let Some(bag) = self.loot_bags.get_mut(&bag_id) else {
                    return Vec::new();
                };
                let Some(slot_ref) = bag.slots.get_mut(slot) else {
                    return Vec::new();
                };
                let Some(stack) = slot_ref.take() else {
                    return Vec::new();
                };
                if let Some(client_mut) = self.clients.get_mut(&client_id) {
                    let leftover = crate::server::inventory::add_stack_to_inventory(
                        &mut client_mut.inventory,
                        stack,
                    );
                    if let Some(leftover) = leftover {
                        // Couldn't fit it all — put what didn't fit back.
                        if let Some(bag) = self.loot_bags.get_mut(&bag_id) {
                            bag.slots[slot] = Some(leftover);
                        }
                        return reply_warning(client_id, "Inventory full");
                    }
                }
                Vec::new()
            }
            LootBagSlotRef::PlayerInventory(slot) => {
                let Some(stack) = self.clients.get_mut(&client_id).and_then(|c| {
                    c.inventory
                        .inventory_slots
                        .get_mut(slot)
                        .and_then(Option::take)
                }) else {
                    return Vec::new();
                };
                self.deposit_to_first_empty_bag_slot(bag_id, client_id, stack, from)
            }
            LootBagSlotRef::PlayerActionbar(slot) => {
                let Some(stack) = self.clients.get_mut(&client_id).and_then(|c| {
                    c.inventory
                        .actionbar_slots
                        .get_mut(slot)
                        .and_then(Option::take)
                }) else {
                    return Vec::new();
                };
                self.deposit_to_first_empty_bag_slot(bag_id, client_id, stack, from)
            }
        }
    }

    fn deposit_to_first_empty_bag_slot(
        &mut self,
        bag_id: LootBagId,
        client_id: ClientId,
        stack: ItemStack,
        origin: LootBagSlotRef,
    ) -> Vec<ServerEnvelope> {
        let Some(bag) = self.loot_bags.get_mut(&bag_id) else {
            // Bag vanished mid-flight; restore the stack so the
            // player doesn't lose items to a race.
            self.restore_to_player_slot(client_id, origin, stack);
            return Vec::new();
        };
        for slot in bag.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(stack);
                return Vec::new();
            }
        }
        // Bag full — restore the stack to its origin slot so the
        // player doesn't drop it on the floor by accident.
        self.restore_to_player_slot(client_id, origin, stack);
        reply_warning(client_id, "Bag is full")
    }

    fn restore_to_player_slot(
        &mut self,
        client_id: ClientId,
        origin: LootBagSlotRef,
        stack: ItemStack,
    ) {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return;
        };
        match origin {
            LootBagSlotRef::PlayerInventory(slot) => {
                if let Some(target) = client.inventory.inventory_slots.get_mut(slot)
                    && target.is_none()
                {
                    *target = Some(stack);
                }
            }
            LootBagSlotRef::PlayerActionbar(slot) => {
                if let Some(target) = client.inventory.actionbar_slots.get_mut(slot)
                    && target.is_none()
                {
                    *target = Some(stack);
                }
            }
            // Bag origin can't be restored via this path — caller
            // routes through `deposit_to_first_empty_bag_slot` only
            // for player-origin transfers.
            LootBagSlotRef::Bag(_) => {}
        }
    }

    fn allocate_loot_bag_id(&mut self) -> LootBagId {
        let id = self.next_loot_bag_id;
        self.next_loot_bag_id = self.next_loot_bag_id.saturating_add(1);
        id
    }
}

/// Pull a stack out of a `LootBagSlotRef`. Returns `(taken, all_consumed)`
/// — `taken` is what was extracted, `all_consumed` is true if the
/// source slot is now empty (used to decide whether to restore the
/// quantity on a failed move).
fn take_from_loot_ref(
    clients: &mut HashMap<ClientId, crate::server::ServerClient>,
    client_id: ClientId,
    bag: &mut LootBag,
    slot: LootBagSlotRef,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    match slot {
        LootBagSlotRef::Bag(index) => {
            let target = bag.slots.get_mut(index)?;
            let current = target.as_mut()?;
            let amount = quantity
                .unwrap_or(current.quantity)
                .clamp(1, current.quantity);
            let all = amount == current.quantity;
            let taken = ItemStack::new(current.item_id.as_ref(), amount);
            current.quantity -= amount;
            if current.quantity == 0 {
                *target = None;
            }
            Some((taken, all))
        }
        LootBagSlotRef::PlayerInventory(index) | LootBagSlotRef::PlayerActionbar(index) => {
            let client = clients.get_mut(&client_id)?;
            let slots = match slot {
                LootBagSlotRef::PlayerInventory(_) => &mut client.inventory.inventory_slots,
                LootBagSlotRef::PlayerActionbar(_) => &mut client.inventory.actionbar_slots,
                LootBagSlotRef::Bag(_) => unreachable!(),
            };
            let target = slots.get_mut(index)?;
            let current = target.as_mut()?;
            let amount = quantity
                .unwrap_or(current.quantity)
                .clamp(1, current.quantity);
            let all = amount == current.quantity;
            let taken = ItemStack::new(current.item_id.as_ref(), amount);
            current.quantity -= amount;
            if current.quantity == 0 {
                *target = None;
            }
            Some((taken, all))
        }
    }
}

/// Insert a stack into a `LootBagSlotRef`. Returns the leftover stack
/// if the destination couldn't fit everything (capacity overflow or
/// mismatched item id in a non-empty slot).
fn insert_into_loot_ref(
    clients: &mut HashMap<ClientId, crate::server::ServerClient>,
    client_id: ClientId,
    bag: &mut LootBag,
    slot: LootBagSlotRef,
    stack: ItemStack,
) -> Option<ItemStack> {
    match slot {
        LootBagSlotRef::Bag(index) => {
            let target = bag.slots.get_mut(index)?;
            insert_into_slot(target, stack)
        }
        LootBagSlotRef::PlayerInventory(index) | LootBagSlotRef::PlayerActionbar(index) => {
            let client = clients.get_mut(&client_id)?;
            let slots = match slot {
                LootBagSlotRef::PlayerInventory(_) => &mut client.inventory.inventory_slots,
                LootBagSlotRef::PlayerActionbar(_) => &mut client.inventory.actionbar_slots,
                LootBagSlotRef::Bag(_) => unreachable!(),
            };
            let target = slots.get_mut(index)?;
            insert_into_slot(target, stack)
        }
    }
}

/// Slot-level insert helper. If the target is empty, the stack moves in
/// whole. If it holds the same item id, the quantities merge up to the
/// item's stack limit, returning any overflow. Mismatched ids swap
/// (returns the original contents).
fn insert_into_slot(target: &mut Option<ItemStack>, incoming: ItemStack) -> Option<ItemStack> {
    match target {
        None => {
            *target = Some(incoming);
            None
        }
        Some(existing) if existing.item_id == incoming.item_id => {
            let limit = crate::items::stack_limit(&existing.item_id).unwrap_or(u16::MAX);
            let space = limit.saturating_sub(existing.quantity);
            if space == 0 {
                return Some(incoming);
            }
            let take = incoming.quantity.min(space);
            existing.quantity += take;
            let remainder = incoming.quantity - take;
            if remainder == 0 {
                None
            } else {
                Some(ItemStack::new(incoming.item_id.as_ref(), remainder))
            }
        }
        Some(existing) => {
            // Swap: caller wanted to put `incoming` here but a
            // different item is in the way. Move the existing stack
            // out and put the incoming one in.
            let displaced = existing.clone();
            *target = Some(incoming);
            Some(displaced)
        }
    }
}

/// Restore an `ItemStack` to its source slot after a failed `Move`.
/// Used when the destination rejected (full / mismatched item) so the
/// player doesn't lose items.
fn restore_into_loot_ref(
    clients: &mut HashMap<ClientId, crate::server::ServerClient>,
    client_id: ClientId,
    bag: &mut LootBag,
    slot: LootBagSlotRef,
    stack: ItemStack,
    removed_all: bool,
) {
    match slot {
        LootBagSlotRef::Bag(index) => {
            if let Some(target) = bag.slots.get_mut(index) {
                restore_slot(target, stack, removed_all);
            }
        }
        LootBagSlotRef::PlayerInventory(index) | LootBagSlotRef::PlayerActionbar(index) => {
            let Some(client) = clients.get_mut(&client_id) else {
                return;
            };
            let slots = match slot {
                LootBagSlotRef::PlayerInventory(_) => &mut client.inventory.inventory_slots,
                LootBagSlotRef::PlayerActionbar(_) => &mut client.inventory.actionbar_slots,
                LootBagSlotRef::Bag(_) => unreachable!(),
            };
            if let Some(target) = slots.get_mut(index) {
                restore_slot(target, stack, removed_all);
            }
        }
    }
}

fn restore_slot(target: &mut Option<ItemStack>, stack: ItemStack, removed_all: bool) {
    match (target.as_mut(), removed_all) {
        (Some(existing), false) if existing.item_id == stack.item_id => {
            existing.quantity = existing.quantity.saturating_add(stack.quantity);
        }
        _ => {
            *target = Some(stack);
        }
    }
}

fn reply_warning(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: crate::protocol::ServerMessage::Toast(ToastMessage::new(ToastKind::Warning, text)),
    }]
}
