//! Diagnostic systems for the Phase 6 re-attempt. Off unless the
//! `replication-trace` Cargo feature is enabled.
//!
//! Logs per-component arrivals and post-spawn diffs on the client and
//! pairs with matching `server: <Component> MUTATE …` lines emitted by
//! `src/net/host.rs` so we can verify every replicated component is
//! actually delivering after the initial spawn.
//!
//! Use:
//! ```sh
//! RUST_LOG=replication_trace=info ./cli dev --features replication-trace
//! ```
//!
//! Expected output when gathering a tree:
//!
//! ```text
//! server: ResourceNodeStorage MUTATE id=12 entity=… 100 -> 95
//! client: ResourceNodeStorage RECV   id=12 entity=… 100 -> 95
//! ```
//!
//! Coverage:
//!   - `ResourceNodeStorage`
//!   - `DeployableHealth`, `DeployableActive`, `DeployableLabel`, `DeployableStability`
//!   - `PlayerPose`, `PlayerHealth`, `PlayerInventory`, `PlayerInputAck`,
//!     `PlayerArmor`, `PlayerLifecycle`, `PlayerSleeping`, `PlayerHeldItem`,
//!     `PlayerAction`
//!   - `LootBagContents`, `LootBagTransform`
//!   - `DroppedItemTransform`, `DroppedItem` (stack-merge)
//!
//! If a server `MUTATE` line fires but the client `RECV` line doesn't,
//! Lightyear is not delivering the update after the initial spawn.

use bevy::{ecs::change_detection::Ref, prelude::*};

use crate::server::{
    Deployable, DeployableActive, DeployableAuth, DeployableHealth, DeployableLabel,
    DeployableStability, DroppedItem, DroppedItemTransform, LootBagContents, LootBagEntity,
    LootBagTransform, Player, PlayerAction, PlayerArmor, PlayerHealth, PlayerHeldItem,
    PlayerInputAck, PlayerInventory, PlayerLifecycle, PlayerPose, PlayerSleeping, ResourceNode,
    ResourceNodeStorage,
};

/// Tracks the last-seen value per id so we can log a clean
/// `before -> after` diff rather than just "changed".
#[derive(Resource, Default)]
pub(crate) struct ReplicationTraceState {
    node_qty: std::collections::HashMap<u64, u16>,
    deployable_health: std::collections::HashMap<u64, u32>,
    deployable_active: std::collections::HashMap<u64, bool>,
    deployable_label: std::collections::HashMap<u64, Option<String>>,
    deployable_stability: std::collections::HashMap<u64, u8>,
    deployable_auth: std::collections::HashMap<u64, Vec<crate::protocol::AccountId>>,
    player_armor: std::collections::HashMap<u64, u8>,
    player_lifecycle: std::collections::HashMap<u64, PlayerLifecycle>,
    player_sleeping: std::collections::HashMap<u64, bool>,
    player_held: std::collections::HashMap<u64, Option<crate::items::HeldMesh>>,
    player_action_seq: std::collections::HashMap<u64, u32>,
    loot_bag_occupied: std::collections::HashMap<u64, usize>,
    dropped_item_qty: std::collections::HashMap<u64, u16>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn log_replicated_storage_changes_system(
    nodes: Query<(Entity, &ResourceNode, Ref<ResourceNodeStorage>)>,
    deployables: Query<(
        Entity,
        &Deployable,
        Ref<DeployableHealth>,
        Ref<DeployableActive>,
        Ref<DeployableLabel>,
        Ref<DeployableStability>,
        Ref<DeployableAuth>,
    )>,
    players_pose: Query<(Entity, &Player, Ref<PlayerPose>)>,
    players_health: Query<(Entity, &Player, Ref<PlayerHealth>)>,
    players_inventory: Query<(Entity, &Player, Ref<PlayerInventory>)>,
    players_input_ack: Query<(Entity, &Player, Ref<PlayerInputAck>)>,
    players_armor: Query<(Entity, &Player, Ref<PlayerArmor>)>,
    players_lifecycle: Query<(Entity, &Player, Ref<PlayerLifecycle>)>,
    players_sleeping: Query<(Entity, &Player, Ref<PlayerSleeping>)>,
    players_held: Query<(Entity, &Player, Ref<PlayerHeldItem>)>,
    players_action: Query<(Entity, &Player, Ref<PlayerAction>)>,
    loot_bags: Query<(Entity, &LootBagEntity, Ref<LootBagContents>)>,
    loot_bag_transforms: Query<(Entity, &LootBagEntity, Ref<LootBagTransform>)>,
    dropped_items: Query<(Entity, Ref<DroppedItem>, Ref<DroppedItemTransform>)>,
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

    for (entity, meta, health, active, label, stability, auth) in &deployables {
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

        // Sleeping-bag renames. `Ref::is_changed()` fires every replication
        // tick for Lightyear-touched components, so gate on a real value
        // delta like the dropped-item stack log below.
        if label.is_added() {
            info!(
                target: "replication_trace",
                "client: DeployableLabel    SPAWN  id={} entity={entity:?} label={:?}",
                meta.id, label.0
            );
            state.deployable_label.insert(meta.id, label.0.clone());
        } else if label.is_changed() {
            let before = state.deployable_label.get(&meta.id).cloned().flatten();
            if before.as_deref() != label.0.as_deref() {
                info!(
                    target: "replication_trace",
                    "client: DeployableLabel    RECV   id={} entity={entity:?} {:?} -> {:?}",
                    meta.id, before, label.0
                );
                state.deployable_label.insert(meta.id, label.0.clone());
            }
        }

        // Structural stability updates. Value-delta gated like the label
        // log: Lightyear bumps the change tick on every replication tick.
        if stability.is_added() {
            info!(
                target: "replication_trace",
                "client: DeployableStability SPAWN  id={} entity={entity:?} pct={}",
                meta.id, stability.0
            );
            state.deployable_stability.insert(meta.id, stability.0);
        } else if stability.is_changed() {
            let before = state
                .deployable_stability
                .get(&meta.id)
                .copied()
                .unwrap_or(0);
            if before != stability.0 {
                info!(
                    target: "replication_trace",
                    "client: DeployableStability RECV   id={} entity={entity:?} {before} -> {}",
                    meta.id, stability.0
                );
                state.deployable_stability.insert(meta.id, stability.0);
            }
        }

        // Tool Cupboard authorize-list updates. Value-delta gated like the
        // label/stability logs (Lightyear bumps the change tick on every
        // replication tick regardless of the value).
        if auth.is_added() {
            info!(
                target: "replication_trace",
                "client: DeployableAuth      SPAWN  id={} entity={entity:?} n={}",
                meta.id, auth.0.len()
            );
            state.deployable_auth.insert(meta.id, auth.0.clone());
        } else if auth.is_changed() {
            let before = state
                .deployable_auth
                .get(&meta.id)
                .cloned()
                .unwrap_or_default();
            if before != auth.0 {
                info!(
                    target: "replication_trace",
                    "client: DeployableAuth      RECV   id={} entity={entity:?} {:?} -> {:?}",
                    meta.id, before, auth.0
                );
                state.deployable_auth.insert(meta.id, auth.0.clone());
            }
        }
    }

    for (entity, player, pose) in &players_pose {
        if pose.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerPose         SPAWN  client={} entity={entity:?} pos={:?}",
                player.client_id, pose.position
            );
        } else if pose.is_changed() {
            info!(
                target: "replication_trace",
                "client: PlayerPose         RECV   client={} entity={entity:?} pos={:?}",
                player.client_id, pose.position
            );
        }
    }

    for (entity, player, health) in &players_health {
        if health.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerHealth       SPAWN  client={} entity={entity:?} hp={}",
                player.client_id, health.0
            );
        } else if health.is_changed() {
            info!(
                target: "replication_trace",
                "client: PlayerHealth       RECV   client={} entity={entity:?} hp={}",
                player.client_id, health.0
            );
        }
    }

    for (entity, player, inventory) in &players_inventory {
        let occupied = inventory
            .0
            .inventory_slots
            .iter()
            .chain(inventory.0.actionbar_slots.iter())
            .filter(|slot| slot.is_some())
            .count();
        if inventory.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerInventory    SPAWN  client={} entity={entity:?} occupied={occupied}",
                player.client_id
            );
        } else if inventory.is_changed() {
            info!(
                target: "replication_trace",
                "client: PlayerInventory    RECV   client={} entity={entity:?} occupied={occupied}",
                player.client_id
            );
        }
    }

    for (entity, player, input_ack) in &players_input_ack {
        if input_ack.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerInputAck     SPAWN  client={} entity={entity:?} last_input={}",
                player.client_id, input_ack.last_processed_input
            );
        } else if input_ack.is_changed() {
            info!(
                target: "replication_trace",
                "client: PlayerInputAck     RECV   client={} entity={entity:?} last_input={}",
                player.client_id, input_ack.last_processed_input
            );
        }
    }

    for (entity, player, armor) in &players_armor {
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

    for (entity, player, lifecycle) in &players_lifecycle {
        if lifecycle.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerLifecycle    SPAWN  client={} entity={entity:?} state={:?}",
                player.client_id, *lifecycle
            );
            state.player_lifecycle.insert(player.client_id, *lifecycle);
        } else if lifecycle.is_changed() {
            let before = state
                .player_lifecycle
                .get(&player.client_id)
                .copied()
                .unwrap_or(PlayerLifecycle::Alive);
            info!(
                target: "replication_trace",
                "client: PlayerLifecycle    RECV   client={} entity={entity:?} {before:?} -> {:?}",
                player.client_id, *lifecycle
            );
            state.player_lifecycle.insert(player.client_id, *lifecycle);
        }
    }

    for (entity, player, sleeping) in &players_sleeping {
        if sleeping.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerSleeping     SPAWN  client={} entity={entity:?} sleeping={}",
                player.client_id, sleeping.0
            );
            state.player_sleeping.insert(player.client_id, sleeping.0);
        } else if sleeping.is_changed() {
            let before = state
                .player_sleeping
                .get(&player.client_id)
                .copied()
                .unwrap_or(false);
            info!(
                target: "replication_trace",
                "client: PlayerSleeping     RECV   client={} entity={entity:?} {before} -> {}",
                player.client_id, sleeping.0
            );
            state.player_sleeping.insert(player.client_id, sleeping.0);
        }
    }

    // Held mesh ships on a tool swap. `Ref::is_changed()` fires every
    // replication tick for Lightyear-touched components, so gate on a real
    // value delta (see the CLAUDE.md replication notes).
    for (entity, player, held) in &players_held {
        if held.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerHeldItem     SPAWN  client={} entity={entity:?} held={:?}",
                player.client_id, held.0
            );
            state.player_held.insert(player.client_id, held.0);
        } else if held.is_changed() {
            let before = state.player_held.get(&player.client_id).copied().flatten();
            if before != held.0 {
                info!(
                    target: "replication_trace",
                    "client: PlayerHeldItem     RECV   client={} entity={entity:?} {before:?} -> {:?}",
                    player.client_id, held.0
                );
                state.player_held.insert(player.client_id, held.0);
            }
        }
    }

    // Swing action ships once per swing (seq increments). Value-delta gated on
    // the seq for the same is_changed() reason.
    for (entity, player, action) in &players_action {
        if action.is_added() {
            info!(
                target: "replication_trace",
                "client: PlayerAction       SPAWN  client={} entity={entity:?} seq={} tool={:?}",
                player.client_id, action.seq, action.tool
            );
            state.player_action_seq.insert(player.client_id, action.seq);
        } else if action.is_changed() {
            let before = state
                .player_action_seq
                .get(&player.client_id)
                .copied()
                .unwrap_or(0);
            if before != action.seq {
                info!(
                    target: "replication_trace",
                    "client: PlayerAction       RECV   client={} entity={entity:?} seq {before} -> {} tool={:?}",
                    player.client_id, action.seq, action.tool
                );
                state.player_action_seq.insert(player.client_id, action.seq);
            }
        }
    }

    for (entity, bag, contents) in &loot_bags {
        let occupied = contents.0.iter().filter(|s| s.is_some()).count();
        if contents.is_added() {
            info!(
                target: "replication_trace",
                "client: LootBagContents    SPAWN  id={} entity={entity:?} occupied={occupied}",
                bag.id
            );
            state.loot_bag_occupied.insert(bag.id, occupied);
        } else if contents.is_changed() {
            let before = state.loot_bag_occupied.get(&bag.id).copied().unwrap_or(0);
            info!(
                target: "replication_trace",
                "client: LootBagContents    RECV   id={} entity={entity:?} {before} -> {occupied}",
                bag.id
            );
            state.loot_bag_occupied.insert(bag.id, occupied);
        }
    }

    for (entity, bag, transform) in &loot_bag_transforms {
        if transform.is_added() {
            info!(
                target: "replication_trace",
                "client: LootBagTransform     SPAWN  id={} entity={entity:?} pos={:?}",
                bag.id, transform.position
            );
        } else if transform.is_changed() {
            info!(
                target: "replication_trace",
                "client: LootBagTransform     RECV   id={} entity={entity:?} pos={:?}",
                bag.id, transform.position
            );
        }
    }

    for (entity, drop, transform) in &dropped_items {
        if transform.is_added() {
            info!(
                target: "replication_trace",
                "client: DroppedItemTransform SPAWN id={} entity={entity:?} pos={:?}",
                drop.id, transform.position
            );
            state.dropped_item_qty.insert(drop.id, drop.stack.quantity);
        } else if transform.is_changed() {
            info!(
                target: "replication_trace",
                "client: DroppedItemTransform RECV  id={} entity={entity:?} pos={:?}",
                drop.id, transform.position
            );
        }
        // Stack quantity changes ship on the `DroppedItem` component when two
        // nearby drops merge. `Ref::is_changed()` fires every replication tick
        // for Lightyear-touched components, so gate the log on a real
        // before -> after delta (see the CLAUDE.md replication notes).
        if !drop.is_added() && drop.is_changed() {
            let before = state.dropped_item_qty.get(&drop.id).copied().unwrap_or(0);
            let after = drop.stack.quantity;
            if before != after {
                info!(
                    target: "replication_trace",
                    "client: DroppedItem          RECV   id={} entity={entity:?} qty {before} -> {after}",
                    drop.id
                );
                state.dropped_item_qty.insert(drop.id, after);
            }
        }
    }
}
