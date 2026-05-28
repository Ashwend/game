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

use crate::server::{
    Deployable, DeployableActive, DeployableHealth, Player, PlayerArmor, ResourceNode,
    ResourceNodeStorage,
};

/// Tracks the last-seen value per id so we can log a clean
/// `before -> after` diff rather than just "changed".
#[derive(Resource, Default)]
pub(crate) struct ReplicationTraceState {
    node_qty: std::collections::HashMap<u64, u16>,
    deployable_health: std::collections::HashMap<u64, u32>,
    deployable_active: std::collections::HashMap<u64, bool>,
    player_armor: std::collections::HashMap<u64, u8>,
}

pub(crate) fn log_replicated_storage_changes_system(
    nodes: Query<(Entity, &ResourceNode, Ref<ResourceNodeStorage>)>,
    deployables: Query<(
        Entity,
        &Deployable,
        Ref<DeployableHealth>,
        Ref<DeployableActive>,
    )>,
    players: Query<(Entity, &Player, Ref<PlayerArmor>)>,
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
            state.node_qty.insert(node.id, total);
        } else if storage.is_changed() {
            let before = state.node_qty.get(&node.id).copied().unwrap_or(0);
            info!(
                target: "replication_trace",
                "client: ResourceNodeStorage RECV   id={} entity={entity:?} {before} -> {total}",
                node.id
            );
            state.node_qty.insert(node.id, total);
        }
    }

    for (entity, meta, health, active) in &deployables {
        if health.is_added() {
            info!(
                target: "replication_trace",
                "client: DeployableHealth   SPAWN  id={} entity={entity:?} hp={}",
                meta.id, health.0
            );
            state.deployable_health.insert(meta.id, health.0);
        } else if health.is_changed() {
            let before = state.deployable_health.get(&meta.id).copied().unwrap_or(0);
            info!(
                target: "replication_trace",
                "client: DeployableHealth   RECV   id={} entity={entity:?} {before} -> {}",
                meta.id, health.0
            );
            state.deployable_health.insert(meta.id, health.0);
        }

        if active.is_added() {
            info!(
                target: "replication_trace",
                "client: DeployableActive   SPAWN  id={} entity={entity:?} active={}",
                meta.id, active.0
            );
            state.deployable_active.insert(meta.id, active.0);
        } else if active.is_changed() {
            let before = state
                .deployable_active
                .get(&meta.id)
                .copied()
                .unwrap_or(false);
            info!(
                target: "replication_trace",
                "client: DeployableActive   RECV   id={} entity={entity:?} {before} -> {}",
                meta.id, active.0
            );
            state.deployable_active.insert(meta.id, active.0);
        }
    }

    for (entity, player, armor) in &players {
        if armor.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerArmor        SPAWN  client={} entity={entity:?} armor={}",
                player.client_id, armor.0
            );
            state.player_armor.insert(player.client_id, armor.0);
        } else if armor.is_changed() {
            let before = state
                .player_armor
                .get(&player.client_id)
                .copied()
                .unwrap_or(0);
            info!(
                target: "replication_trace",
                "client: PlayerArmor        RECV   client={} entity={entity:?} {before} -> {}",
                player.client_id, armor.0
            );
            state.player_armor.insert(player.client_id, armor.0);
        }
    }
}
