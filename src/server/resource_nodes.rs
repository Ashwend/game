use std::collections::HashMap;

use crate::{
    items::item_definition,
    protocol::{
        ClientId, ItemStack, ResourceGatherCommand, ResourceImpactKind, ResourceNodeId,
        ResourceNodeState, ServerMessage,
    },
    resources::{
        ResourceNodeModel, can_gather_resource_node, definition_storage_stacks,
        next_resource_payout, remove_resource_from_storage, resource_node_definition,
        resource_storage_is_empty, spawn_resource_node,
    },
    world::WorldData,
};

use super::{
    DeliveryTarget, GameServer, ServerEnvelope,
    inventory::add_stack_to_inventory,
    movement::player_eye_position,
    toasts::{inventory_full_toast_envelopes, item_acquired_toast_envelopes},
};

/// How long a depleted resource node takes to fully regenerate. Picked so
/// the loop has rhythm without making the map feel cluttered with timers:
/// long enough that a "first-clear" of the starting zone still feels
/// earned, short enough that returning to the area later finds things
/// regrown.
pub(super) const RESPAWN_DURATION_SECONDS: f32 = 75.0;
/// Floor on a per-tick respawn step. Fixed-rate tick (~20 Hz) keeps step
/// noise low; this clamp just guards against an absurd `delta_seconds`
/// stalling progress.
const MAX_RESPAWN_STEP: f32 = 0.1;

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
        // Regenerating nodes carry no payout — they're a ghost waiting to
        // come back. Silently reject the gather so a swing hitting a
        // regrowing node doesn't dispense items or fire impact effects.
        if node.respawn_progress.is_some() {
            return Vec::new();
        }
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
            // Apply the cooldown anyway so the player can't spam a "full"
            // toast every swing impact while their bag is full.
            client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);
            return inventory_full_toast_envelopes(client_id);
        }
        client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);

        let payout_id = payout.item_id.clone();
        if let Some(node) = self.resource_nodes.get_mut(&command.resource_node_id) {
            remove_resource_from_storage(node, &payout_id, accepted_quantity);
            if resource_storage_is_empty(node) {
                // Don't delete — start the respawn timer instead. The
                // ghost remains visible to clients while it regrows.
                node.respawn_progress = Some(0.0);
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

    /// Advance every regenerating node by `delta_seconds`. When a node
    /// finishes regrowing, its storage is restocked from the definition
    /// and the `respawn_progress` flag is cleared so the next gather
    /// finds it ready. Called from `tick()` once per server step.
    pub(super) fn tick_resource_node_respawn(&mut self, delta_seconds: f32) {
        let step = delta_seconds.clamp(0.0, MAX_RESPAWN_STEP) / RESPAWN_DURATION_SECONDS;
        if step <= 0.0 {
            return;
        }
        for node in self.resource_nodes.values_mut() {
            let Some(progress) = node.respawn_progress else {
                continue;
            };
            let next = progress + step;
            if next >= 1.0 {
                if let Some(definition) = resource_node_definition(&node.definition_id) {
                    node.storage = definition_storage_stacks(definition);
                }
                node.respawn_progress = None;
            } else {
                node.respawn_progress = Some(next);
            }
        }
    }
}

fn resource_impact_kind(model: ResourceNodeModel) -> ResourceImpactKind {
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
