use crate::{
    items::{HANDS_TOOL, ToolProfile, item_definition},
    protocol::{ClientId, ResourceGatherCommand, ResourceImpactKind, ServerMessage, Vec3Net},
    resources::{
        ResourceNodeModel, can_gather_resource_node, next_resource_payout,
        remove_resource_from_storage, resource_node_definition, resource_storage_is_empty,
    },
};

use super::{
    DeliveryTarget, GameServer, ServerEnvelope,
    inventory::accepted_inventory_quantity,
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
        // hands are actually accepted, crude nodes (branch piles, surface
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
            // toast every swing impact while their bag is full. The tool
            // still struck the node, so wear applies too.
            client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);
            let mut envelopes = inventory_full_toast_envelopes(client_id);
            envelopes.extend(self.consume_active_tool_durability(client_id));
            return envelopes;
        }
        client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);

        let payout_id = payout.item_id.clone();
        // Wear lands after the payout: the swing that breaks the tool
        // still pays out its gather.
        let wear_envelopes = self.consume_active_tool_durability(client_id);
        let mut depleted = false;
        if let Some(node) = self.resource_node_state_mut(command.resource_node_id) {
            remove_resource_from_storage(node, &payout_id, accepted_quantity);
            depleted = resource_storage_is_empty(node);
        }
        let mut envelopes = item_acquired_toast_envelopes(client_id, &payout_id, accepted_quantity);
        envelopes.extend(wear_envelopes);
        if depleted {
            // Remove the node entirely, the chunk manager schedules a
            // fresh-position respawn 5-15 min later in the same grid.
            // Broadcast a `ResourceNodeDepleted` so clients can run the
            // death animation; without that, a Lightyear despawn alone
            // can't tell "node depleted" apart from "node left this
            // client's AoI" and would falsely animate the death of
            // every node leaving view at every chunk-boundary crossing.
            self.remove_resource_node(command.resource_node_id);
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
        //, see [Networking § Replication](../../docs/networking.md#replication).

        // Skip the swinger, their client already played the impact via
        // local prediction (a second copy would double-trigger the sound
        // and chip burst), and only deliver to clients close enough to
        // perceive the effect at all.
        // Spawn the peers' burst where the swinger's look ray actually hit the
        // node (e.g. partway up a tree), not at its base. The range gate still
        // keys off the node centre; only the broadcast position carries the
        // hit point (clamped near the node).
        let impact_position = sanitize_impact_point(command.hit_point, node.position);
        envelopes.extend(self.envelopes_within_range(
            node.position,
            crate::game_balance::IMPACT_MESSAGE_RANGE_M,
            Some(client_id),
            ServerMessage::ResourceImpact {
                position: impact_position,
                kind: resource_impact_kind(node_definition.model),
            },
        ));
        envelopes
    }
}

/// Clamp a client-supplied impact point to something sane near the node: finite
/// and within a generous radius (tall trees reach ~9 m). Cosmetic anti-abuse so
/// a forged gather can't spray particles across the map; falls back to the node
/// centre on anything out of bounds.
fn sanitize_impact_point(hit: Vec3Net, node: Vec3Net) -> Vec3Net {
    const MAX_OFFSET_M: f32 = 12.0;
    if hit.x.is_finite()
        && hit.y.is_finite()
        && hit.z.is_finite()
        && hit.minus(node).length_squared() <= MAX_OFFSET_M * MAX_OFFSET_M
    {
        hit
    } else {
        node
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
        // Meteorite reuses the stone-vein wire impact kind (rocky strike, iron
        // pickaxe): no new wire variant, so no protocol bump for the node.
        ResourceNodeModel::Meteorite => ResourceImpactKind::StoneVein,
        ResourceNodeModel::BranchPile => ResourceImpactKind::Branches,
        ResourceNodeModel::SurfaceStone => ResourceImpactKind::SurfaceStone,
        ResourceNodeModel::HayGrass => ResourceImpactKind::HayGrass,
    }
}
