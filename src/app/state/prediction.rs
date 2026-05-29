//! Client-side optimistic prediction overlay.
//!
//! The authoritative inventory lives on the server and arrives via the
//! replicated `PlayerPrivate` component. To make gather / pickup / drop /
//! move feel instant, the client predicts the *outcome* locally the moment
//! the input fires, then reconciles against the server using a single
//! monotonically-increasing action sequence — the inventory analogue of how
//! movement reconciles via `last_processed_input`.
//!
//! ## Reconciliation invariant
//!
//! `rendered = replicated  folded-with  (ops where seq > applied_action_seq)`
//!
//! The server advances `applied_action_seq` for every predicted command it
//! processes — accepted *or* rejected (see `GameServer::note_action_seq`).
//! Each frame the client [`PredictionState::prune`]s ops whose seq the server
//! has already processed, then [`PredictionState::rebuild_effective`] replays
//! the survivors on top of the freshly-replicated inventory using the shared
//! [`crate::inventory`] helpers — the exact same math the server runs, so a
//! confirmed prediction lands identically and a rejected one simply evaporates
//! when its op is pruned.
//!
//! Only the pure state machine lives here. Per-interaction wiring (which input
//! pushes which op) and the per-frame fold system live in the input and
//! schedule layers.

use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::{
    inventory::{add_stack_to_inventory, move_stack, remove_stack},
    protocol::{DroppedItemId, ItemContainerSlot, ItemStack, PlayerInventoryState, ResourceNodeId},
};

/// A single predicted inventory mutation, awaiting server confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingKind {
    /// Gather payout or pickup: a stack added to the inventory.
    Add(ItemStack),
    /// Drop / consume: remove a quantity from a specific slot. `None`
    /// quantity means the whole stack.
    Remove {
        from: ItemContainerSlot,
        quantity: Option<u16>,
    },
    /// Player→player slot move. The dispatch layer only predicts the
    /// empty-destination case in Tier 1 (the branchy swap/merge path stays
    /// server-driven), but the replay itself just runs the shared `move_stack`.
    Move {
        from: ItemContainerSlot,
        to: ItemContainerSlot,
        quantity: Option<u16>,
    },
}

#[derive(Debug, Clone)]
struct PendingOp {
    seq: u32,
    kind: PendingKind,
}

/// A predicted gather take against a resource node, awaiting confirmation.
/// Lets back-to-back swings predict against the node's *reduced* fill rather
/// than the stale replicated storage, so the second swing on a one-item node
/// correctly predicts "nothing left".
#[derive(Debug, Clone)]
struct NodeTake {
    seq: u32,
    stack: ItemStack,
}

#[derive(Resource, Default)]
pub(crate) struct PredictionState {
    /// Next sequence number to hand out. Starts at 0; `alloc_seq` pre-increments
    /// so the first allocated seq is 1 — the server's default
    /// `applied_action_seq = 0` then never prunes a live op before it's processed.
    next_seq: u32,
    /// Pending inventory ops in seq (== push) order.
    pending: Vec<PendingOp>,
    /// Dropped item ids hidden by a predicted pickup, keyed to the seq that
    /// hid them. While present, the dropped-item render system suppresses the
    /// world visual; pruning (server rejected the pickup) un-hides it.
    hidden_dropped: HashMap<DroppedItemId, u32>,
    /// Resource node ids hidden by a predicted crude (E-key) pickup that
    /// fully drained the node, keyed to the seq that hid them. While
    /// present, the resource-node render system suppresses the world visual
    /// (and plays the depletion effect once); pruning un-hides it on reject
    /// or lets the confirmed despawn finalise it. The dropped-item analogue
    /// for the much-more-numerous resource nodes — see [`Self::is_node_hidden`].
    hidden_nodes: HashMap<ResourceNodeId, u32>,
    /// Per-node predicted storage decrements awaiting confirmation.
    node_takes: HashMap<ResourceNodeId, Vec<NodeTake>>,
}

impl PredictionState {
    /// Allocate the next action sequence number. Monotonic across *all*
    /// predicted command kinds for this client, matching the server's single
    /// per-client `applied_action_seq`.
    pub(crate) fn alloc_seq(&mut self) -> u32 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Predict a gather payout: record the per-node take (so the next swing
    /// sees reduced fill) and add the stack to the inventory overlay.
    pub(crate) fn push_gather(&mut self, seq: u32, node: ResourceNodeId, stack: ItemStack) {
        self.node_takes.entry(node).or_default().push(NodeTake {
            seq,
            stack: stack.clone(),
        });
        self.pending.push(PendingOp {
            seq,
            kind: PendingKind::Add(stack),
        });
    }

    /// Predict a pickup: hide the world item and add the stack to the overlay.
    /// Use only when the *whole* stack fits — the server removes the world
    /// item from existence only on a full pickup. For a partial pickup (bag
    /// nearly full) use [`PredictionState::push_add`] without hiding, so the
    /// reduced ground stack stays visible and reconciles via replication.
    pub(crate) fn push_pickup(&mut self, seq: u32, dropped: DroppedItemId, stack: ItemStack) {
        self.hidden_dropped.insert(dropped, seq);
        self.pending.push(PendingOp {
            seq,
            kind: PendingKind::Add(stack),
        });
    }

    /// Predict a crude (E-key) resource-node pickup: the server drains the
    /// whole node into the bag in one shot, so record one node take + one
    /// inventory add per accepted stack (all under the same `seq`), and —
    /// when the node is fully emptied — hide the world visual. A partial
    /// pickup (near-full bag leaves storage behind) mirrors the server by
    /// adding only what fit and leaving the node visible to reconcile via
    /// replication.
    pub(crate) fn push_node_pickup(
        &mut self,
        seq: u32,
        node: ResourceNodeId,
        accepted: Vec<ItemStack>,
        fully_drained: bool,
    ) {
        for stack in accepted {
            self.node_takes.entry(node).or_default().push(NodeTake {
                seq,
                stack: stack.clone(),
            });
            self.pending.push(PendingOp {
                seq,
                kind: PendingKind::Add(stack),
            });
        }
        if fully_drained {
            self.hidden_nodes.insert(node, seq);
        }
    }

    /// Predict an inventory gain not tied to a node or a hidden world item
    /// (e.g. the portion of a pickup that fits a near-full bag).
    pub(crate) fn push_add(&mut self, seq: u32, stack: ItemStack) {
        self.pending.push(PendingOp {
            seq,
            kind: PendingKind::Add(stack),
        });
    }

    /// Predict a drop: remove the quantity from the source slot.
    pub(crate) fn push_drop(&mut self, seq: u32, from: ItemContainerSlot, quantity: Option<u16>) {
        self.pending.push(PendingOp {
            seq,
            kind: PendingKind::Remove { from, quantity },
        });
    }

    /// Predict an inventory move.
    pub(crate) fn push_move(
        &mut self,
        seq: u32,
        from: ItemContainerSlot,
        to: ItemContainerSlot,
        quantity: Option<u16>,
    ) {
        self.pending.push(PendingOp {
            seq,
            kind: PendingKind::Move { from, to, quantity },
        });
    }

    /// Is this dropped item currently hidden by an unconfirmed predicted
    /// pickup? The dropped-item render system consults this to suppress the
    /// world visual.
    pub(crate) fn is_dropped_hidden(&self, id: DroppedItemId) -> bool {
        self.hidden_dropped.contains_key(&id)
    }

    /// Is this resource node currently hidden by an unconfirmed predicted
    /// crude pickup? The resource-node render system consults this to drive
    /// the suppress / un-hide reconcile.
    pub(crate) fn is_node_hidden(&self, id: ResourceNodeId) -> bool {
        self.hidden_nodes.contains_key(&id)
    }

    /// Node ids currently hidden by an unconfirmed predicted pickup. The set
    /// is tiny (in-flight crude pickups, usually empty), so the resource-node
    /// reconcile can diff it without iterating the full replicated set.
    pub(crate) fn hidden_node_ids(&self) -> Vec<ResourceNodeId> {
        self.hidden_nodes.keys().copied().collect()
    }

    /// Drop every pending op / hidden id / node take whose seq the server has
    /// already processed (`seq <= applied_action_seq`). On the reliable,
    /// ordered command channel `applied = K` implies every `seq < K` was
    /// processed too, so this high-water mark can never over-prune.
    pub(crate) fn prune(&mut self, applied_action_seq: u32) {
        self.pending.retain(|op| op.seq > applied_action_seq);
        self.hidden_dropped
            .retain(|_, seq| *seq > applied_action_seq);
        self.hidden_nodes.retain(|_, seq| *seq > applied_action_seq);
        for takes in self.node_takes.values_mut() {
            takes.retain(|take| take.seq > applied_action_seq);
        }
        self.node_takes.retain(|_, takes| !takes.is_empty());
    }

    /// Rebuild the effective (predicted) inventory: clone the replicated base
    /// and replay every surviving pending op in seq order via the shared
    /// helpers. An op that no-ops against the current base (e.g. a `Remove`
    /// whose slot the server already emptied) simply contributes nothing —
    /// pruning by seq handles its eventual removal.
    pub(crate) fn rebuild_effective(
        &self,
        replicated: &PlayerInventoryState,
    ) -> PlayerInventoryState {
        let mut effective = replicated.clone();
        for op in &self.pending {
            match &op.kind {
                PendingKind::Add(stack) => {
                    add_stack_to_inventory(&mut effective, stack.clone());
                }
                PendingKind::Remove { from, quantity } => {
                    remove_stack(&mut effective, *from, *quantity);
                }
                PendingKind::Move { from, to, quantity } => {
                    move_stack(&mut effective, *from, *to, *quantity);
                }
            }
        }
        effective
    }

    /// Effective storage for a node: the replicated base minus every
    /// unconfirmed predicted take. Used by the gather dispatch to compute the
    /// next payout against the reduced fill.
    pub(crate) fn effective_node_storage(
        &self,
        node: ResourceNodeId,
        base: &[ItemStack],
    ) -> Vec<ItemStack> {
        let mut storage: Vec<ItemStack> = base.to_vec();
        if let Some(takes) = self.node_takes.get(&node) {
            for take in takes {
                subtract_from_storage(&mut storage, &take.stack);
            }
        }
        storage
    }

    /// Clear all prediction state. Called on disconnect and on respawn — a
    /// fresh authoritative inventory must not have stale ops replayed onto it.
    /// `next_seq` intentionally keeps climbing across reconnects so a
    /// late-arriving stale command can never collide with a fresh seq.
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.hidden_dropped.clear();
        self.hidden_nodes.clear();
        self.node_takes.clear();
    }

    /// True when there is nothing to fold — lets the fold system skip the
    /// clone+replay entirely on the common idle frame.
    pub(crate) fn is_idle(&self) -> bool {
        self.pending.is_empty()
            && self.hidden_dropped.is_empty()
            && self.hidden_nodes.is_empty()
            && self.node_takes.is_empty()
    }
}

/// Subtract `taken` (by item id) from a storage vec, dropping emptied stacks.
/// Mirrors the server's `remove_resource_from_storage` drain so a predicted
/// take reduces the node fill the same way the authoritative path will.
fn subtract_from_storage(storage: &mut Vec<ItemStack>, taken: &ItemStack) {
    let mut remaining = taken.quantity;
    for stack in storage.iter_mut() {
        if remaining == 0 {
            break;
        }
        if stack.item_id != taken.item_id {
            continue;
        }
        let removed = stack.quantity.min(remaining);
        stack.quantity -= removed;
        remaining -= removed;
    }
    storage.retain(|stack| stack.quantity > 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::COAL_ID;

    fn inv_with(slot0: Option<ItemStack>) -> PlayerInventoryState {
        let mut inv = PlayerInventoryState::empty();
        inv.inventory_slots[0] = slot0;
        inv
    }

    const NODE: ResourceNodeId = 42;

    #[test]
    fn seq_starts_at_one_and_is_monotonic() {
        let mut state = PredictionState::default();
        assert_eq!(state.alloc_seq(), 1);
        assert_eq!(state.alloc_seq(), 2);
        assert_eq!(state.alloc_seq(), 3);
    }

    #[test]
    fn add_op_shows_in_effective_then_no_double_count_on_confirm() {
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_gather(seq, NODE, ItemStack::new(COAL_ID, 5));

        // Before the server confirms, the gain shows on top of an empty bag.
        let effective = state.rebuild_effective(&PlayerInventoryState::empty());
        assert_eq!(
            effective.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(5)
        );

        // Server confirms: replicated inventory now contains the 5 coal AND
        // applied_action_seq == seq. Pruning must leave exactly 5, not 10.
        let confirmed = inv_with(Some(ItemStack::new(COAL_ID, 5)));
        state.prune(seq);
        assert!(state.is_idle());
        let effective = state.rebuild_effective(&confirmed);
        assert_eq!(
            effective.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(5),
            "no double-count after confirm"
        );
    }

    #[test]
    fn rejected_add_reverts_when_seq_advances_without_state_change() {
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_gather(seq, NODE, ItemStack::new(COAL_ID, 5));

        // Server rejected (e.g. cooldown): replicated inventory unchanged, but
        // applied_action_seq still advanced past the op.
        state.prune(seq);
        let effective = state.rebuild_effective(&PlayerInventoryState::empty());
        assert!(
            effective.inventory_slots[0].is_none(),
            "rejected prediction must evaporate"
        );
    }

    #[test]
    fn drop_op_removes_from_effective() {
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_drop(seq, ItemContainerSlot::inventory(0), Some(1));

        let replicated = inv_with(Some(ItemStack::new(COAL_ID, 3)));
        let effective = state.rebuild_effective(&replicated);
        assert_eq!(
            effective.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(2)
        );
    }

    #[test]
    fn move_into_empty_slot_predicts_relocation() {
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_move(
            seq,
            ItemContainerSlot::inventory(0),
            ItemContainerSlot::inventory(1),
            None,
        );

        let replicated = inv_with(Some(ItemStack::new(COAL_ID, 7)));
        let effective = state.rebuild_effective(&replicated);
        assert!(effective.inventory_slots[0].is_none());
        assert_eq!(
            effective.inventory_slots[1].as_ref().map(|s| s.quantity),
            Some(7)
        );
    }

    #[test]
    fn move_replayed_over_server_mutated_inventory_no_ops_gracefully() {
        // Pending move from slot 0, but the server already emptied slot 0. The
        // replayed move finds nothing and contributes nothing — no corruption.
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_move(
            seq,
            ItemContainerSlot::inventory(0),
            ItemContainerSlot::inventory(1),
            None,
        );
        let effective = state.rebuild_effective(&PlayerInventoryState::empty());
        assert!(effective.inventory_slots[0].is_none());
        assert!(effective.inventory_slots[1].is_none());
    }

    #[test]
    fn gather_multi_swing_predicts_against_reduced_node_fill() {
        let mut state = PredictionState::default();
        let base = vec![ItemStack::new(COAL_ID, 1)];

        // First swing predicts taking the single unit.
        let storage = state.effective_node_storage(NODE, &base);
        assert_eq!(storage.first().map(|s| s.quantity), Some(1));
        let seq1 = state.alloc_seq();
        state.push_gather(seq1, NODE, ItemStack::new(COAL_ID, 1));

        // Second swing must now see an empty node.
        let storage = state.effective_node_storage(NODE, &base);
        assert!(
            storage.is_empty(),
            "second swing should predict the node is empty"
        );

        // Server confirms the first take: node ledger prunes, and since the
        // replicated storage will also be empty, the prediction is consistent.
        state.prune(seq1);
        assert!(state.is_idle());
    }

    #[test]
    fn pickup_hides_then_unhides_on_reject() {
        let mut state = PredictionState::default();
        let id: DroppedItemId = 7;
        let seq = state.alloc_seq();
        state.push_pickup(seq, id, ItemStack::new(COAL_ID, 2));
        assert!(state.is_dropped_hidden(id));

        // Server rejected the pickup (someone else grabbed it / out of range):
        // applied_action_seq advances, the item is still in the world.
        state.prune(seq);
        assert!(
            !state.is_dropped_hidden(id),
            "rejected pickup un-hides item"
        );
    }

    #[test]
    fn node_pickup_full_drain_hides_and_adds_then_confirms() {
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_node_pickup(
            seq,
            NODE,
            vec![ItemStack::new(COAL_ID, 3)],
            true, // fully drained
        );
        assert!(state.is_node_hidden(NODE));
        assert_eq!(state.hidden_node_ids(), vec![NODE]);

        // Gain shows immediately on top of an empty bag.
        let effective = state.rebuild_effective(&PlayerInventoryState::empty());
        assert_eq!(
            effective.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(3)
        );

        // Server confirms: inventory now holds the 3, applied_action_seq == seq.
        let confirmed = inv_with(Some(ItemStack::new(COAL_ID, 3)));
        state.prune(seq);
        assert!(state.is_idle());
        assert!(!state.is_node_hidden(NODE));
        let effective = state.rebuild_effective(&confirmed);
        assert_eq!(
            effective.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(3),
            "no double-count after confirm"
        );
    }

    #[test]
    fn node_pickup_unhides_and_reverts_on_reject() {
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_node_pickup(seq, NODE, vec![ItemStack::new(COAL_ID, 2)], true);
        assert!(state.is_node_hidden(NODE));

        // Server rejected (out of range / already gone): applied_action_seq
        // advances with the replicated inventory unchanged.
        state.prune(seq);
        assert!(!state.is_node_hidden(NODE), "rejected pickup un-hides node");
        let effective = state.rebuild_effective(&PlayerInventoryState::empty());
        assert!(
            effective.inventory_slots[0].is_none(),
            "rejected gain must evaporate"
        );
    }

    #[test]
    fn node_pickup_partial_adds_without_hiding() {
        // Near-full bag: only part of the node fit, so the node stays in the
        // world (server leaves the remainder) and must not be hidden.
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_node_pickup(seq, NODE, vec![ItemStack::new(COAL_ID, 1)], false);
        assert!(
            !state.is_node_hidden(NODE),
            "partial pickup leaves the node visible"
        );
        let effective = state.rebuild_effective(&PlayerInventoryState::empty());
        assert_eq!(
            effective.inventory_slots[0].as_ref().map(|s| s.quantity),
            Some(1)
        );
    }

    #[test]
    fn clear_drops_everything() {
        let mut state = PredictionState::default();
        let seq = state.alloc_seq();
        state.push_pickup(seq, 1, ItemStack::new(COAL_ID, 1));
        let seq2 = state.alloc_seq();
        state.push_gather(seq2, NODE, ItemStack::new(COAL_ID, 1));
        let seq3 = state.alloc_seq();
        state.push_node_pickup(seq3, NODE, vec![ItemStack::new(COAL_ID, 1)], true);
        state.clear();
        assert!(state.is_idle());
        assert!(!state.is_dropped_hidden(1));
        assert!(!state.is_node_hidden(NODE));
    }
}
