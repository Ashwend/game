//! Synthesise `ClientRuntime::snapshot` from Lightyear-replicated
//! components every frame.
//!
//! Phase 6 removed the per-tick `ServerMessage::Snapshot` wire broadcast.
//! Per-entity state now arrives through Lightyear's room-gated component
//! replication (Phases 4–5). To avoid rewriting every UI / pickup /
//! collision-grid consumer in one go, this system rebuilds the same
//! `WorldSnapshot` shape locally each frame from the replicated entity
//! query results — `runtime.snapshot` therefore stays a valid mirror of
//! the visible game state without anyone needing to know it's now a
//! local synthesis rather than a wire message.
//!
//! The synth runs in `ClientSystemSet::SnapshotSynth`, ordered between
//! `Network` (where Lightyear's receive pipeline has applied any
//! incoming replication diffs) and the consumer sets (`Players`,
//! `DroppedItems`, `ResourceNodes`, `DeployedEntities`) that read from
//! `runtime.snapshot`. The synthetic `tick` is a monotonic local
//! counter; this is enough for the interpolation `retarget`
//! freshness gate because the synth always increments and clients
//! never receive out-of-order frames.

use bevy::prelude::*;

use crate::{
    app::state::{ClientRuntime, runtime::resource_node_collider_set_version},
    protocol::{
        DeployedEntityState, DroppedWorldItem, PlayerState, ResourceNodeState, WorldSnapshot,
    },
    server::{
        Deployable, DeployableActive, DeployableHealth, DeployableTransform, DroppedItem,
        DroppedItemTransform, Player, PlayerPrivate, PlayerPublic, ResourceNode,
        ResourceNodeStorage,
    },
};

/// Rebuild `runtime.snapshot` from the current replicated entity queries.
/// Runs every Update. While the client is between sessions
/// (`client_id == None`) the synth is a no-op so the snapshot stays
/// `None`, which is the existing in-session sentinel used by every
/// consumer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn synthesize_runtime_snapshot_system(
    mut runtime: ResMut<ClientRuntime>,
    mut tick_counter: Local<u64>,
    nodes: Query<(&ResourceNode, &ResourceNodeStorage)>,
    drops: Query<(&DroppedItem, &DroppedItemTransform)>,
    deployables: Query<(
        &Deployable,
        &DeployableTransform,
        &DeployableHealth,
        &DeployableActive,
    )>,
    players: Query<(&Player, &PlayerPublic, Option<&PlayerPrivate>)>,
) {
    if runtime.client_id.is_none() {
        // Pre-Welcome (or post-Disconnect): leave the snapshot as `None`
        // so consumers fall back to their "no session" cleanup branches.
        return;
    }

    *tick_counter = tick_counter.wrapping_add(1);

    let players_vec: Vec<PlayerState> = players
        .iter()
        .map(|(meta, public, private)| {
            let private = private.cloned();
            PlayerState {
                client_id: meta.client_id,
                steam_id: meta.steam_id,
                name: public.name.clone(),
                position: public.position,
                velocity: public.velocity,
                yaw: public.yaw,
                pitch: public.pitch,
                health: public.health,
                grounded: public.grounded,
                last_processed_input: private
                    .as_ref()
                    .map(|p| p.last_processed_input)
                    .unwrap_or(0),
                is_admin: public.is_admin,
                chat_bubble: public.chat_bubble.clone(),
                inventory: private.as_ref().map(|p| p.inventory.clone()),
                crafting: private.as_ref().map(|p| p.crafting.clone()),
                open_furnace: private.and_then(|p| p.open_furnace),
            }
        })
        .collect();

    let dropped_items: Vec<DroppedWorldItem> = drops
        .iter()
        .map(|(drop, transform)| DroppedWorldItem {
            id: drop.id,
            stack: drop.stack.clone(),
            position: transform.position,
            yaw: transform.yaw,
            rotation: transform.rotation,
        })
        .collect();

    let resource_nodes: Vec<ResourceNodeState> = nodes
        .iter()
        .map(|(node, storage)| ResourceNodeState {
            id: node.id,
            definition_id: node.definition_id.clone(),
            position: node.position,
            yaw: node.yaw,
            storage: storage.0.clone(),
            respawn_progress: None,
        })
        .collect();

    let deployed_entities: Vec<DeployedEntityState> = deployables
        .iter()
        .map(|(meta, transform, health, active)| DeployedEntityState {
            id: meta.id,
            item_id: meta.item_id.clone(),
            kind: meta.kind,
            position: transform.position,
            yaw: transform.yaw,
            health: health.0,
            max_health: meta.max_health,
            active: active.0,
        })
        .collect();

    let snapshot = WorldSnapshot {
        tick: *tick_counter,
        players: players_vec,
        dropped_items,
        resource_nodes,
        deployed_entities,
    };

    // Rebuild the collision grid only when the live set of
    // collider-bearing entities actually changes — every frame would be
    // wasted work since the set only shifts on chunk crossings and
    // node depletions.
    let new_version = resource_node_collider_set_version(Some(&snapshot));
    if new_version != runtime.resource_node_collider_version {
        runtime.resource_node_collider_version = new_version;
        runtime.snapshot = Some(snapshot);
        runtime.rebuild_world_grid();
    } else {
        runtime.snapshot = Some(snapshot);
    }
}
