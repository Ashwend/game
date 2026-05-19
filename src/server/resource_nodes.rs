use std::collections::HashMap;

use crate::{
    items::item_definition,
    protocol::{
        ClientId, ItemStack, ResourceGatherCommand, ResourceImpactKind, ResourceNodeId,
        ResourceNodeState, ServerMessage,
    },
    resources::{
        ResourceNodeModel, can_gather_resource_node, next_resource_payout,
        remove_resource_from_storage, resource_node_definition, resource_storage_is_empty,
        spawn_resource_node,
    },
    world::WorldData,
};

use super::{
    DeliveryTarget, GameServer, ServerEnvelope, inventory::add_stack_to_inventory,
    item_acquired_toast_envelopes, movement::player_eye_position,
};

pub(super) fn initial_resource_nodes(
    world: &WorldData,
) -> HashMap<ResourceNodeId, ResourceNodeState> {
    world
        .resource_nodes
        .iter()
        .filter_map(spawn_resource_node)
        .map(|node| (node.id, node))
        .collect()
}

impl GameServer {
    pub(super) fn apply_gather_command(
        &mut self,
        client_id: ClientId,
        command: ResourceGatherCommand,
    ) -> Vec<ServerEnvelope> {
        let Some(node) = self.resource_nodes.get(&command.resource_node_id).cloned() else {
            return Vec::new();
        };
        let Some(node_definition) = resource_node_definition(&node.definition_id) else {
            return Vec::new();
        };
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if self.tick < client.next_gather_tick {
            return Vec::new();
        }

        let Some(active_stack) = client.inventory.active_actionbar_stack() else {
            return Vec::new();
        };
        let Some(tool) =
            item_definition(&active_stack.item_id).and_then(|definition| definition.tool)
        else {
            return Vec::new();
        };
        if !node_definition.required_tool.allows(tool) {
            return Vec::new();
        }
        if !can_gather_resource_node(
            player_eye_position(client.controller.position),
            client.controller.yaw,
            client.controller.pitch,
            &node,
        ) {
            return Vec::new();
        }

        let Some(payout) = next_resource_payout(&node, tool) else {
            return Vec::new();
        };
        if item_definition(&payout.item_id).is_none() {
            return Vec::new();
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let accepted_quantity = accepted_inventory_quantity(&mut client.inventory, payout.clone());
        if accepted_quantity == 0 {
            return Vec::new();
        }
        client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);

        let payout_id = payout.item_id.clone();
        if let Some(node) = self.resource_nodes.get_mut(&command.resource_node_id) {
            remove_resource_from_storage(node, &payout_id, accepted_quantity);
            if resource_storage_is_empty(node) {
                self.resource_nodes.remove(&command.resource_node_id);
            }
        }

        let mut envelopes = item_acquired_toast_envelopes(client_id, &payout_id, accepted_quantity);
        envelopes.push(ServerEnvelope {
            // Skip the swinger — their client already played the impact via
            // local prediction. Sending a second copy from the server would
            // double-trigger both the sound and the chip burst.
            target: DeliveryTarget::BroadcastExcept(client_id),
            message: ServerMessage::ResourceImpact {
                position: node.position,
                kind: resource_impact_kind(node_definition.model),
            },
        });
        envelopes
    }
}

fn resource_impact_kind(model: ResourceNodeModel) -> ResourceImpactKind {
    if model.is_tree() {
        ResourceImpactKind::Tree
    } else {
        ResourceImpactKind::OreNode
    }
}

fn accepted_inventory_quantity(
    inventory: &mut crate::protocol::PlayerInventoryState,
    stack: ItemStack,
) -> u16 {
    let requested = stack.quantity;
    match add_stack_to_inventory(inventory, stack) {
        Some(remainder) => requested.saturating_sub(remainder.quantity),
        None => requested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::{BASIC_PICKAXE_ID, COAL_ID},
        protocol::ItemStack,
        resources::COAL_NODE_ID,
    };

    #[test]
    fn accepted_quantity_reports_partial_inventory_insert() {
        let mut inventory = crate::protocol::PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 99));
        for slot in inventory.inventory_slots.iter_mut().skip(1) {
            *slot = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
        }

        assert_eq!(
            accepted_inventory_quantity(&mut inventory, ItemStack::new(COAL_ID, 3)),
            1
        );
        assert_eq!(
            inventory.inventory_slots[0]
                .as_ref()
                .map(|stack| stack.quantity),
            Some(100)
        );
    }

    #[test]
    fn initial_nodes_are_spawned_from_world_data() {
        let world = WorldData {
            floor_size: 16.0,
            blocks: Vec::new(),
            resource_nodes: vec![crate::world::WorldResourceNodeSpawn::new(
                7,
                COAL_NODE_ID,
                crate::protocol::Vec3Net::ZERO,
                0.0,
            )],
        };

        let nodes = initial_resource_nodes(&world);

        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes.get(&7).map(|node| node.definition_id.as_str()),
            Some(COAL_NODE_ID)
        );
    }
}
