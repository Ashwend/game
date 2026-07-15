use crate::{
    app::state::{PickupTargetState, PredictionState},
    inventory::accepted_inventory_quantity,
    protocol::{DroppedItemId, ItemStack, ResourceNodeId},
    resource_nodes::{next_payout_from_storage, resource_node_definition},
};

use super::swing::equipped_tool_profile;

/// Predict the inventory gain from a gather swing, returning the action
/// sequence the command should carry (`0` = not predicted, so the server's
/// `applied_action_seq` is left unchanged by this command).
///
/// Mirrors the server's `apply_gather_command` payout math exactly: the same
/// [`next_payout_from_storage`] against the node's storage (folded with any
/// unconfirmed predicted takes via [`PredictionState::effective_node_storage`])
/// and the same [`accepted_inventory_quantity`] truncation against the bag. A
/// full bag (`accepted == 0`) predicts nothing, matching the server, which
/// only emits a "full" toast and applies the cooldown.
pub(super) fn predict_gather(
    prediction: &mut PredictionState,
    local_player: &crate::app::state::LocalPlayerState,
    node_id: ResourceNodeId,
    target: &PickupTargetState,
) -> u32 {
    let Some(inventory) = local_player
        .private
        .as_ref()
        .map(|private| &private.inventory)
    else {
        return 0;
    };
    let tool = equipped_tool_profile(local_player);
    let storage = prediction.effective_node_storage(node_id, &target.resource_storage);
    // The node's per-swing cap must come from the same definition the server
    // reads, or the optimistic gain overshoots on capped nodes (meteorite).
    let per_swing_yield = target
        .resource_definition_id
        .as_deref()
        .and_then(resource_node_definition)
        .and_then(|definition| definition.per_swing_yield);
    let Some(payout) = next_payout_from_storage(&storage, tool, per_swing_yield) else {
        return 0;
    };
    let mut effective = inventory.clone();
    let accepted = accepted_inventory_quantity(&mut effective, payout.clone());
    if accepted == 0 {
        return 0;
    }
    let seq = prediction.alloc_seq();
    prediction.push_gather(seq, node_id, ItemStack::new(payout.item_id, accepted));
    seq
}

/// Predict a dropped-item pickup, returning the action sequence the command
/// should carry (`0` = not predicted). Predicts only the portion that fits
/// the bag (mirroring the server's partial pickup), and hides the world item
/// only when the *whole* stack fits, the server removes the world entity
/// from existence only on a full pickup.
pub(super) fn predict_pickup(
    prediction: &mut PredictionState,
    local_player: &crate::app::state::LocalPlayerState,
    dropped_item_id: DroppedItemId,
    target: &PickupTargetState,
) -> u32 {
    let Some(inventory) = local_player
        .private
        .as_ref()
        .map(|private| &private.inventory)
    else {
        return 0;
    };
    let Some(stack) = target.stack.clone() else {
        return 0;
    };
    let mut effective = inventory.clone();
    let accepted = accepted_inventory_quantity(&mut effective, stack.clone());
    if accepted == 0 {
        return 0;
    }
    let seq = prediction.alloc_seq();
    if accepted == stack.quantity {
        prediction.push_pickup(seq, dropped_item_id, stack);
    } else {
        prediction.push_add(seq, ItemStack::new(stack.item_id, accepted));
    }
    seq
}

/// Predict a crude (E-key) resource-node pickup, returning the action
/// sequence the command should carry (`0` = not predicted).
///
/// Mirrors the server's `pick_up_resource_node` drain exactly: walk the
/// node's storage (folded with any unconfirmed predicted takes via
/// [`PredictionState::effective_node_storage`]) adding each stack to a
/// cloned bag with the shared [`accepted_inventory_quantity`], collect what
/// fit, and treat the node as *fully drained* only when nothing is left
/// behind, matching the server, which despawns the node only on a clean
/// full drain and leaves a partial node standing. A full bag (nothing fit)
/// predicts nothing, mirroring the server's "full" toast + no-op.
pub(super) fn predict_resource_node_pickup(
    prediction: &mut PredictionState,
    local_player: &crate::app::state::LocalPlayerState,
    node_id: ResourceNodeId,
    target: &PickupTargetState,
) -> u32 {
    let Some(inventory) = local_player
        .private
        .as_ref()
        .map(|private| &private.inventory)
    else {
        return 0;
    };
    let storage = prediction.effective_node_storage(node_id, &target.resource_storage);
    let mut effective = inventory.clone();
    let mut accepted: Vec<ItemStack> = Vec::new();
    let mut fully_drained = true;
    for stack in &storage {
        if stack.quantity == 0 {
            continue;
        }
        let took = accepted_inventory_quantity(&mut effective, stack.clone());
        if took > 0 {
            accepted.push(ItemStack::new(stack.item_id.clone(), took));
        }
        if took < stack.quantity {
            fully_drained = false;
        }
    }
    if accepted.is_empty() {
        return 0;
    }
    let seq = prediction.alloc_seq();
    prediction.push_node_pickup(seq, node_id, accepted, fully_drained);
    seq
}
