use crate::{
    items::{HANDS_TOOL, ToolProfile, item_definition},
    protocol::{ClientId, ItemStack, ResourceGatherCommand, ResourceImpactKind, ServerMessage},
    resources::{
        ResourceNodeModel, can_gather_resource_node, next_resource_payout,
        remove_resource_from_storage, resource_node_definition, resource_storage_is_empty,
    },
};

use super::{
    DeliveryTarget, GameServer, ServerEnvelope,
    inventory::add_stack_to_inventory,
    movement::player_eye_position,
    toasts::{inventory_full_toast_envelopes, item_acquired_toast_envelopes},
};

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

        // Hand-harvest fallback: if the active slot has no tool definition
        // (empty, or holding a non-tool item), use the synthesized
        // `HANDS_TOOL` profile. The node's `required_tool` decides whether
        // hands are actually accepted — crude nodes (branch piles, surface
        // stones, grass) use `ToolKind::Hands` which is satisfied by any
        // tool *or* by hands; tree/ore nodes only by the matching tool.
        let tool: ToolProfile = client
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .and_then(|definition| definition.tool)
            .unwrap_or(HANDS_TOOL);
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
            // Apply the cooldown anyway so the player can't spam a "full"
            // toast every swing impact while their bag is full.
            client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);
            return inventory_full_toast_envelopes(client_id);
        }
        client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);

        let payout_id = payout.item_id.clone();
        let mut depleted = false;
        if let Some(node) = self.resource_nodes.get_mut(&command.resource_node_id) {
            remove_resource_from_storage(node, &payout_id, accepted_quantity);
            depleted = resource_storage_is_empty(node);
        }
        let mut envelopes = item_acquired_toast_envelopes(client_id, &payout_id, accepted_quantity);
        if depleted {
            // Remove the node entirely — the chunk manager schedules a
            // fresh-position respawn 5-15 min later in the same grid.
            // Broadcast a `ResourceNodeDepleted` so clients can run the
            // death animation; without that, a Lightyear despawn alone
            // can't tell "node depleted" apart from "node left this
            // client's AoI" and would falsely animate the death of
            // every node leaving view at every chunk-boundary crossing.
            self.resource_nodes.remove(&command.resource_node_id);
            self.chunk_manager
                .handle_node_depleted(command.resource_node_id, self.tick);
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::ResourceNodeDepleted {
                    id: command.resource_node_id,
                },
            });
        }
        // Storage post-gather: the ECS mirror picks up `node.storage`
        // on the next sync and Lightyear replicates the
        // `ResourceNodeStorage` diff. No reliable side-channel needed
        // — see [Networking § Replication](../../docs/networking.md#replication).

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

pub(super) fn resource_impact_kind(model: ResourceNodeModel) -> ResourceImpactKind {
    match model {
        ResourceNodeModel::PineTreeSmall
        | ResourceNodeModel::PineTreeMedium
        | ResourceNodeModel::PineTreeLarge
        | ResourceNodeModel::BirchTreeSmall
        | ResourceNodeModel::BirchTreeMedium
        | ResourceNodeModel::BirchTreeLarge => ResourceImpactKind::Tree,
        ResourceNodeModel::CoalOre => ResourceImpactKind::CoalOre,
        ResourceNodeModel::IronOre => ResourceImpactKind::IronOre,
        ResourceNodeModel::SulfurOre => ResourceImpactKind::SulfurOre,
        ResourceNodeModel::StoneVein => ResourceImpactKind::StoneVein,
        ResourceNodeModel::BranchPile => ResourceImpactKind::Branches,
        ResourceNodeModel::SurfaceStone => ResourceImpactKind::SurfaceStone,
        ResourceNodeModel::HayGrass => ResourceImpactKind::HayGrass,
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
    };

    #[test]
    fn accepted_quantity_reports_partial_inventory_insert() {
        let mut inventory = crate::protocol::PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 199));
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
            Some(200)
        );
    }
}
