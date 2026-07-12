use crate::{
    game_balance::PICKUP_SERVER_REACH_SLACK_M,
    items::{ToolKind, normalize_stack, within_pickup_reach},
    protocol::{
        ACTIONBAR_SLOT_COUNT, ClientId, DroppedItemId, DroppedWorldItem, InventoryCommand,
        ItemStack, PlayerInventoryState, ResourceNodeId, Vec3Net,
    },
    resources::{resource_node_definition, within_gather_reach},
};

use super::{
    GameServer, ServerEnvelope,
    dropped_items::{DroppedItemBody, yaw_rotation},
    movement::{DropOrigin, drop_origin_for, player_eye_position},
    toasts::{inventory_full_toast_envelopes, item_acquired_toast_envelopes},
};

// The pure inventory math moved to `crate::inventory` so the client-side
// prediction overlay can replay the exact same operations the server runs.
// Re-exported here so existing `super::inventory::*` / `crate::server::inventory::*`
// call sites across the server keep resolving unchanged.
pub use crate::inventory::{
    accepted_inventory_quantity, add_stack_to_inventory, insert_stack_at, move_stack,
    offset_actionbar_slot, remove_stack, sort_inventory, take_items_from_inventory,
};

pub(super) fn starting_inventory() -> PlayerInventoryState {
    PlayerInventoryState::empty()
}

impl GameServer {
    pub(super) fn apply_inventory_command(
        &mut self,
        client_id: ClientId,
        command: InventoryCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            InventoryCommand::Move {
                from, to, quantity, ..
            } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    move_stack(&mut client.inventory, from, to, quantity);
                }
                // A move that touched the paperdoll (equipped or unequipped a
                // piece) can change mitigation, so recompute. The shared
                // `move_stack` validates the slot type, so a rejected equip left
                // the inventory untouched and this is a harmless no-op recompute.
                if matches!(from.container, crate::protocol::ItemContainer::Equipment)
                    || matches!(to.container, crate::protocol::ItemContainer::Equipment)
                {
                    self.recompute_protection(client_id);
                }
                Vec::new()
            }
            InventoryCommand::Drop { from, quantity, .. } => {
                let Some((stack, origin)) = self.clients.get_mut(&client_id).and_then(|client| {
                    remove_stack(&mut client.inventory, from, quantity)
                        .map(|stack| (stack, drop_origin_for(client)))
                }) else {
                    return Vec::new();
                };
                self.spawn_dropped_item_at(origin, stack);
                Vec::new()
            }
            InventoryCommand::PickUp {
                dropped_item_id, ..
            } => self.pick_up_dropped_item(client_id, dropped_item_id),
            InventoryCommand::PickUpResourceNode {
                resource_node_id, ..
            } => self.pick_up_resource_node(client_id, resource_node_id),
            InventoryCommand::RecoverProjectile { projectile_id } => {
                self.apply_recover_projectile(client_id, projectile_id)
            }
            InventoryCommand::SelectActionbarSlot { slot } => {
                if slot < ACTIONBAR_SLOT_COUNT
                    && let Some(client) = self.clients.get_mut(&client_id)
                {
                    client.inventory.active_actionbar_slot = slot;
                }
                // Swapping off a drawn bow ends the draw and restores movement;
                // swapping off a reloading crossbow lifts its reload slow too.
                self.clear_ranged_draw(client_id);
                self.clear_reload_slow(client_id);
                Vec::new()
            }
            InventoryCommand::SelectActionbarOffset { offset } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.inventory.active_actionbar_slot =
                        offset_actionbar_slot(client.inventory.active_actionbar_slot, offset);
                }
                self.clear_ranged_draw(client_id);
                self.clear_reload_slow(client_id);
                Vec::new()
            }
            InventoryCommand::Sort => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    sort_inventory(&mut client.inventory);
                }
                Vec::new()
            }
        }
    }

    /// Spawn a stack at a player's [`DropOrigin`] (feet + forward toss). The
    /// single entry point for "eject this stack to the world in front of the
    /// player" used by inventory drops, craft refunds, and furnace ejects.
    pub(super) fn spawn_dropped_item_at(&mut self, origin: DropOrigin, stack: ItemStack) {
        self.spawn_dropped_item(stack, origin.position, origin.velocity, origin.yaw);
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
        self.insert_dropped_item(
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
        // Distance-only acceptance: the client already chose this exact item
        // via the strict view ray and predicted the pickup. Re-running the
        // cone test against the player's now-moved position would reject
        // legitimate pickups and force a visible rollback, so the server only
        // confirms the player is plausibly within reach (plus slack for the
        // movement-prediction delta).
        if !within_pickup_reach(
            player_eye_position(client.controller.position),
            item.position,
            PICKUP_SERVER_REACH_SLACK_M,
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
            if let Some(body) = self.remove_dropped_item(dropped_item_id) {
                self.dropped_item_physics.remove_body(body.body_handle);
                self.chunk_manager.untrack_dropped_item(dropped_item_id);
            }
        } else if accepted > 0
            && let Some(body) = self.dropped_item_body_mut(dropped_item_id)
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
    /// `Hands`, trees and ore veins still require a tool swing.
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
        // Distance-only acceptance, same rationale as the dropped-item path:
        // the client already targeted this node via the view ray and
        // predicted the pickup, so the server just confirms the player is
        // plausibly within reach rather than re-checking the cone against a
        // moved position (which would cause false rejects + rollbacks).
        if !within_gather_reach(
            player_eye_position(client.controller.position),
            &node.definition_id,
            node.position,
            PICKUP_SERVER_REACH_SLACK_M,
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
            // Node fully picked up, remove and schedule the fresh-position
            // respawn the gather path uses on depletion. Broadcast a
            // `ResourceNodeDepleted` so clients can run the death effect:
            // a Lightyear despawn alone can't distinguish a real depletion
            // from an AoI-leave, so this reliable message is the
            // disambiguator the client's pending-depletion grace map uses.
            self.remove_resource_node(resource_node_id);
            self.chunk_manager
                .handle_node_depleted(resource_node_id, self.tick);
            envelopes.push(ServerEnvelope {
                target: super::DeliveryTarget::Broadcast,
                message: crate::protocol::ServerMessage::ResourceNodeDepleted {
                    id: resource_node_id,
                },
            });
        } else if let Some(node_mut) = self.resource_node_state_mut(resource_node_id) {
            // Partial pickup, leave the rest in the node's storage so
            // the player can come back with a bigger bag. The ECS
            // mirror picks up the new storage on the next sync and
            // Lightyear replicates the `ResourceNodeStorage` diff.
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
