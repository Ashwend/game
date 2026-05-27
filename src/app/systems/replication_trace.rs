//! Diagnostic systems for the Phase 6 re-attempt. Off unless the
//! `replication-trace` Cargo feature is enabled.
//!
//! When a Lightyear-replicated `ResourceNodeStorage` component arrives
//! on the client (or mutates on an already-known entity), this logs
//! it to `target: "replication_trace"`. Pair with the matching
//! `server: ResourceNodeStorage MUTATE …` line in `src/net/host.rs` to
//! verify whether per-component updates flow end-to-end.
//!
//! Use:
//! ```sh
//! RUST_LOG=replication_trace=info ./cli dev --features replication-trace
//! ```
//!
//! Expected output when gathering a tree (this is what we're trying to
//! prove out):
//!
//! ```text
//! server: ResourceNodeStorage MUTATE id=12 entity=… 100 -> 95
//! client: ResourceNodeStorage RECV   id=12 entity=… 100 -> 95
//! ```
//!
//! If the server line fires but the client line doesn't, Lightyear is
//! not delivering the update after the initial spawn — that's the
//! Phase 6a bug.

use bevy::{ecs::change_detection::Ref, prelude::*};

use crate::server::{ResourceNode, ResourceNodeStorage};

/// Tracks the last-seen quantity per node id so we can log a clean
/// `before -> after` diff rather than just "changed".
#[derive(Resource, Default)]
pub(crate) struct ReplicationTraceState {
    last_quantity: std::collections::HashMap<u64, u16>,
}

pub(crate) fn log_replicated_storage_changes_system(
    nodes: Query<(Entity, &ResourceNode, Ref<ResourceNodeStorage>)>,
    mut state: ResMut<ReplicationTraceState>,
) {
    for (entity, node, storage) in &nodes {
        let total: u16 = storage.0.iter().map(|s| s.quantity).sum();
        if storage.is_added() {
            info!(
                target: "replication_trace",
                "client: ResourceNodeStorage SPAWN  id={} entity={entity:?} qty={}",
                node.id, total
            );
            state.last_quantity.insert(node.id, total);
        } else if storage.is_changed() {
            let before = state.last_quantity.get(&node.id).copied().unwrap_or(0);
            info!(
                target: "replication_trace",
                "client: ResourceNodeStorage RECV   id={} entity={entity:?} {} -> {}",
                node.id, before, total
            );
            state.last_quantity.insert(node.id, total);
        }
    }
}
