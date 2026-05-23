use crate::{
    items::{
        BASIC_HATCHET_ID, BASIC_PICKAXE_ID, TEST_BANDAGE_ID, TEST_ORE_ID, TEST_RELIC_ID,
        can_pick_up, normalize_stack, stack_limit,
    },
    protocol::{
        ACTIONBAR_SLOT_COUNT, ClientId, DroppedItemId, DroppedWorldItem, INVENTORY_SLOT_COUNT,
        InventoryCommand, ItemContainer, ItemContainerSlot, ItemStack, PlayerInventoryState,
        Vec3Net,
    },
};

use super::{
    GameServer, ServerEnvelope,
    dropped_items::{DroppedItemBody, yaw_rotation},
    movement::{drop_position, drop_velocity, player_eye_position},
    toasts::{inventory_full_toast_envelopes, item_acquired_toast_envelopes},
};

pub(super) fn starting_inventory() -> PlayerInventoryState {
    let mut inventory = PlayerInventoryState::empty();
    inventory.inventory_slots[0] = Some(ItemStack::new(TEST_ORE_ID, 12));
    inventory.inventory_slots[1] = Some(ItemStack::new(TEST_BANDAGE_ID, 5));
    inventory.inventory_slots[2] = Some(ItemStack::new(TEST_RELIC_ID, 1));
    inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    inventory.actionbar_slots[1] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
    inventory
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

fn insert_stack_at(
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
            },
        );
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
}
