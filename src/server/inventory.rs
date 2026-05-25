use crate::{
    items::{ToolKind, can_pick_up, normalize_stack, stack_limit},
    protocol::{
        ACTIONBAR_SLOT_COUNT, ClientId, DroppedItemId, DroppedWorldItem, INVENTORY_SLOT_COUNT,
        InventoryCommand, ItemContainer, ItemContainerSlot, ItemStack, PlayerInventoryState,
        ResourceNodeId, Vec3Net,
    },
    resources::{can_gather_resource_node, resource_node_definition},
};

use super::{
    GameServer, ServerEnvelope,
    dropped_items::{DroppedItemBody, yaw_rotation},
    movement::{drop_position, drop_velocity, player_eye_position},
    toasts::{inventory_full_toast_envelopes, item_acquired_toast_envelopes},
};

pub(super) fn starting_inventory() -> PlayerInventoryState {
    PlayerInventoryState::empty()
}

pub(super) fn move_stack(
    inventory: &mut PlayerInventoryState,
    from: ItemContainerSlot,
    to: ItemContainerSlot,
    quantity: Option<u16>,
) {
    if from == to || !slot_exists(inventory, from) || !slot_exists(inventory, to) {
        return;
    }

    let Some((moving, removed_all)) = remove_stack_for_move(inventory, from, quantity) else {
        return;
    };
    let remainder = insert_stack_at(inventory, to, moving, removed_all);
    if let Some(remainder) = remainder {
        restore_stack(inventory, from, remainder);
    }
}

fn remove_stack_for_move(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    let source = slot_mut(inventory, slot)?;
    let current = source.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let removed_all = amount == current.quantity;
    let item_id = current.item_id.clone();
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some((ItemStack::new(item_id, amount), removed_all))
}

pub(super) fn remove_stack(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    quantity: Option<u16>,
) -> Option<ItemStack> {
    let source = slot_mut(inventory, slot)?;
    let current = source.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let item_id = current.item_id.clone();
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some(ItemStack::new(item_id, amount))
}

pub(super) fn insert_stack_at(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    mut moving: ItemStack,
    allow_swap: bool,
) -> Option<ItemStack> {
    moving = normalize_stack(&moving)?;
    let target = slot_mut(inventory, slot)?;
    match target {
        None => {
            *target = Some(moving);
            None
        }
        Some(existing) if existing.item_id == moving.item_id => {
            let limit = stack_limit(&existing.item_id).unwrap_or(1);
            let room = limit.saturating_sub(existing.quantity);
            let moved = room.min(moving.quantity);
            existing.quantity += moved;
            moving.quantity -= moved;
            (moving.quantity > 0).then_some(moving)
        }
        Some(existing) if allow_swap => {
            let displaced = std::mem::replace(existing, moving);
            Some(displaced)
        }
        Some(_) => Some(moving),
    }
}

fn restore_stack(inventory: &mut PlayerInventoryState, slot: ItemContainerSlot, stack: ItemStack) {
    let Some(target) = slot_mut(inventory, slot) else {
        return;
    };
    match target {
        Some(existing) if existing.item_id == stack.item_id => {
            let limit = stack_limit(&existing.item_id).unwrap_or(1);
            existing.quantity = existing.quantity.saturating_add(stack.quantity).min(limit);
        }
        None => {
            *target = Some(stack);
        }
        Some(_) => {}
    }
}

/// Pull up to `quantity` units of `item_id` out of the inventory + actionbar.
/// Walks slots in `actionbar → inventory` order (so the toolbar drains last,
/// leaving the player's quick-access items intact when the bag has the same
/// material). Returns the actual amount removed; less than `quantity` means
/// there wasn't enough to satisfy the request.
///
/// Designed for the crafting consume path. The caller is expected to verify
/// totals up-front so the partial case shouldn't fire in practice — but the
/// function still drains what it can, since refusing to remove anything
/// would leave the inventory in a worse state if a recipe definition ever
/// goes out of sync with the take.
pub(super) fn take_items_from_inventory(
    inventory: &mut PlayerInventoryState,
    item_id: &str,
    quantity: u16,
) -> u16 {
    let mut remaining = quantity;
    if remaining == 0 {
        return 0;
    }

    for slot in inventory
        .actionbar_slots
        .iter_mut()
        .chain(inventory.inventory_slots.iter_mut())
    {
        if remaining == 0 {
            break;
        }
        let Some(stack) = slot.as_mut() else {
            continue;
        };
        if stack.item_id.as_ref() != item_id {
            continue;
        }
        let take = remaining.min(stack.quantity);
        stack.quantity -= take;
        remaining -= take;
        if stack.quantity == 0 {
            *slot = None;
        }
    }

    quantity - remaining
}

pub(super) fn add_stack_to_inventory(
    inventory: &mut PlayerInventoryState,
    stack: ItemStack,
) -> Option<ItemStack> {
    let mut remaining = normalize_stack(&stack)?;

    for index in 0..inventory.actionbar_slots.len() {
        let slot = ItemContainerSlot::actionbar(index);
        if inventory.actionbar_slots[index]
            .as_ref()
            .is_some_and(|existing| existing.item_id == remaining.item_id)
        {
            remaining = insert_stack_at(inventory, slot, remaining, false)?;
        }
    }

    for index in 0..inventory.inventory_slots.len() {
        let slot = ItemContainerSlot::inventory(index);
        if inventory.inventory_slots[index]
            .as_ref()
            .is_some_and(|existing| existing.item_id == remaining.item_id)
        {
            remaining = insert_stack_at(inventory, slot, remaining, false)?;
        }
    }

    for index in 0..inventory.inventory_slots.len() {
        if inventory.inventory_slots[index].is_none() {
            inventory.inventory_slots[index] = Some(remaining);
            return None;
        }
    }

    Some(remaining)
}

fn slot_mut(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
) -> Option<&mut Option<ItemStack>> {
    match slot.container {
        ItemContainer::Inventory => inventory.inventory_slots.get_mut(slot.slot),
        ItemContainer::Actionbar => inventory.actionbar_slots.get_mut(slot.slot),
    }
}

fn slot_exists(inventory: &PlayerInventoryState, slot: ItemContainerSlot) -> bool {
    (match slot.container {
        ItemContainer::Inventory => slot.slot < INVENTORY_SLOT_COUNT,
        ItemContainer::Actionbar => slot.slot < ACTIONBAR_SLOT_COUNT,
    }) && (match slot.container {
        ItemContainer::Inventory => slot.slot < inventory.inventory_slots.len(),
        ItemContainer::Actionbar => slot.slot < inventory.actionbar_slots.len(),
    })
}

pub(super) fn offset_actionbar_slot(current: usize, offset: i8) -> usize {
    (current as isize + offset as isize).rem_euclid(ACTIONBAR_SLOT_COUNT as isize) as usize
}

impl GameServer {
    pub(super) fn apply_inventory_command(
        &mut self,
        client_id: ClientId,
        command: InventoryCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            InventoryCommand::Move { from, to, quantity } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    move_stack(&mut client.inventory, from, to, quantity);
                }
                Vec::new()
            }
            InventoryCommand::Drop { from, quantity } => {
                let Some((stack, position, velocity, yaw)) =
                    self.clients.get_mut(&client_id).and_then(|client| {
                        remove_stack(&mut client.inventory, from, quantity).map(|stack| {
                            (
                                stack,
                                drop_position(&client.controller),
                                drop_velocity(&client.controller),
                                client.controller.yaw,
                            )
                        })
                    })
                else {
                    return Vec::new();
                };
                self.spawn_dropped_item(stack, position, velocity, yaw);
                Vec::new()
            }
            InventoryCommand::PickUp { dropped_item_id } => {
                self.pick_up_dropped_item(client_id, dropped_item_id)
            }
            InventoryCommand::PickUpResourceNode { resource_node_id } => {
                self.pick_up_resource_node(client_id, resource_node_id)
            }
            InventoryCommand::SelectActionbarSlot { slot } => {
                if slot < ACTIONBAR_SLOT_COUNT
                    && let Some(client) = self.clients.get_mut(&client_id)
                {
                    client.inventory.active_actionbar_slot = slot;
                }
                Vec::new()
            }
            InventoryCommand::SelectActionbarOffset { offset } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.inventory.active_actionbar_slot =
                        offset_actionbar_slot(client.inventory.active_actionbar_slot, offset);
                }
                Vec::new()
            }
        }
    }

    pub(super) fn spawn_dropped_item(
        &mut self,
        stack: ItemStack,
        position: Vec3Net,
        velocity: Vec3Net,
        yaw: f32,
    ) {
        let Some(stack) = normalize_stack(&stack) else {
            return;
        };
        let id = self.next_dropped_item_id;
        self.next_dropped_item_id += 1;
        let physics_body = self
            .dropped_item_physics
            .spawn_body(position, velocity, yaw);
        self.dropped_items.insert(
            id,
            DroppedItemBody {
                item: DroppedWorldItem {
                    id,
                    stack,
                    position,
                    yaw,
                    rotation: yaw_rotation(yaw),
                },
                body_handle: physics_body.body_handle,
                spawn_tick: self.tick,
            },
        );
        // Anchor the drop to its chunk so the AoI snapshot path picks
        // it up. Future physics steps will re-anchor if it drifts.
        self.chunk_manager.track_dropped_item(id, position);
    }

    fn pick_up_dropped_item(
        &mut self,
        client_id: ClientId,
        dropped_item_id: DroppedItemId,
    ) -> Vec<ServerEnvelope> {
        let Some(item) = self
            .dropped_items
            .get(&dropped_item_id)
            .map(|body| body.item.clone())
        else {
            return Vec::new();
        };
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !can_pick_up(
            player_eye_position(client.controller.position),
            client.controller.yaw,
            client.controller.pitch,
            &item,
        ) {
            return Vec::new();
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let requested = item.stack.quantity;
        let remainder = add_stack_to_inventory(&mut client.inventory, item.stack.clone());
        let accepted = match &remainder {
            Some(rem) => requested.saturating_sub(rem.quantity),
            None => requested,
        };
        if remainder.is_none() {
            if let Some(body) = self.dropped_items.remove(&dropped_item_id) {
                self.dropped_item_physics.remove_body(body.body_handle);
                self.chunk_manager.untrack_dropped_item(dropped_item_id);
            }
        } else if accepted > 0
            && let Some(body) = self.dropped_items.get_mut(&dropped_item_id)
        {
            body.item.stack.quantity = body.item.stack.quantity.saturating_sub(accepted);
        }
        if accepted == 0 {
            return inventory_full_toast_envelopes(client_id);
        }
        item_acquired_toast_envelopes(client_id, &item.stack.item_id, accepted)
    }

    /// Quick-pickup path for crude resource nodes: drains storage straight
    /// into the player's inventory, removes the node if fully emptied
    /// (and schedules a fresh-position respawn via the chunk manager), and
    /// returns toasts mirroring the per-item gather path. Server-side
    /// gate: rejects nodes whose `required_tool` is anything other than
    /// `Hands` — trees and ore veins still require a tool swing.
    fn pick_up_resource_node(
        &mut self,
        client_id: ClientId,
        resource_node_id: ResourceNodeId,
    ) -> Vec<ServerEnvelope> {
        let Some(node) = self.resource_nodes.get(&resource_node_id).cloned() else {
            return Vec::new();
        };
        let Some(definition) = resource_node_definition(&node.definition_id) else {
            return Vec::new();
        };
        if definition.required_tool.kind != ToolKind::Hands {
            return Vec::new();
        }
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        // Same view-ray gate as the gather path: the player must be
        // looking at the node and within range.
        if !can_gather_resource_node(
            player_eye_position(client.controller.position),
            client.controller.yaw,
            client.controller.pitch,
            &node,
        ) {
            return Vec::new();
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let mut accepted_per_item: Vec<(crate::items::ItemId, u16)> = Vec::new();
        let mut new_storage: Vec<ItemStack> = Vec::new();
        let mut any_leftover = false;
        for stack in &node.storage {
            if stack.quantity == 0 {
                continue;
            }
            let requested = stack.quantity;
            let remainder = add_stack_to_inventory(&mut client.inventory, stack.clone());
            let accepted = match &remainder {
                Some(rem) => requested.saturating_sub(rem.quantity),
                None => requested,
            };
            if accepted > 0 {
                accepted_per_item.push((stack.item_id.clone(), accepted));
            }
            if let Some(rem) = remainder
                && rem.quantity > 0
            {
                new_storage.push(rem);
                any_leftover = true;
            }
        }

        let mut envelopes = Vec::new();
        if !any_leftover {
            // Node fully picked up — remove and schedule the fresh-position
            // respawn the gather path uses on depletion. Broadcast a
            // `ResourceNodeDepleted` so clients can run the death effect
            // (the snapshot diff can't otherwise distinguish a real
            // depletion from an AoI-leave).
            self.resource_nodes.remove(&resource_node_id);
            self.chunk_manager
                .handle_node_depleted(resource_node_id, self.tick);
            envelopes.push(ServerEnvelope {
                target: super::DeliveryTarget::Broadcast,
                message: crate::protocol::ServerMessage::ResourceNodeDepleted {
                    id: resource_node_id,
                },
            });
        } else if let Some(node_mut) = self.resource_nodes.get_mut(&resource_node_id) {
            // Partial pickup — leave the rest in the node's storage so
            // the player can come back with a bigger bag.
            node_mut.storage = new_storage;
        }

        if accepted_per_item.is_empty() {
            envelopes.extend(inventory_full_toast_envelopes(client_id));
            return envelopes;
        }
        for (item_id, quantity) in accepted_per_item {
            envelopes.extend(item_acquired_toast_envelopes(client_id, &item_id, quantity));
        }
        envelopes
    }
}
