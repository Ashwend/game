//! Client-side mirror of the local player's replicated components.
//!
//! UI and input systems need to read the local player's inventory,
//! crafting queue, and open-furnace state. Those arrive as separate
//! Lightyear-replicated components (`PlayerInventory`, `PlayerCrafting`,
//! `PlayerOpenContainers`, `PlayerInputAck`, split per cadence so the
//! wire diffs stay small). A single per-frame system scans the `Player`
//! query to find whichever entity matches `ClientRuntime::client_id`
//! and reassembles the pieces into one [`PlayerPrivate`] view so UI
//! helpers (which don't own a query themselves) can read the data via a
//! plain `Res<LocalPlayerState>`.
//!
//! Clones are cheap for one entity per frame.

use bevy::prelude::*;

use crate::server::{
    Player, PlayerClaimStatus, PlayerCrafting, PlayerHealth, PlayerInputAck, PlayerInventory,
    PlayerLifecycle, PlayerOpenContainers, PlayerPrivate, PlayerPrivateState,
};

use super::{ClientRuntime, MenuState, PredictionState};

/// Refreshed every frame from the replicated Player entity whose
/// `Player.client_id == runtime.client_id`. `None` until the local
/// session connects and Lightyear has shipped the entity.
#[derive(Resource, Default, Debug)]
pub(crate) struct LocalPlayerState {
    pub(crate) entity: Option<Entity>,
    pub(crate) private: Option<PlayerPrivate>,
    pub(crate) lifecycle: Option<PlayerLifecycle>,
}

pub(crate) fn update_local_player_state_system(
    mut runtime: ResMut<ClientRuntime>,
    mut state: ResMut<LocalPlayerState>,
    mut menu: ResMut<MenuState>,
    // `PlayerHealth` rides here so the local HUD tracks the authoritative
    // replicated health. The bar reads the *predicted* health, which only the
    // server's per-hit `Correction` message writes; a single dropped message
    // would otherwise strand it at full while the server records every hit.
    players: Query<(
        Entity,
        &Player,
        Option<&PlayerLifecycle>,
        Option<&PlayerHealth>,
    )>,
    // The five owner-only components live on a separate private mirror entity
    // that the server replicates to this client alone, so this query yields
    // exactly one row (our own). Requiring all five keeps the assembled view
    // atomic (they ship together in the private entity's initial replication).
    private: Query<
        (
            &PlayerInventory,
            &PlayerCrafting,
            &PlayerOpenContainers,
            &PlayerInputAck,
            &PlayerClaimStatus,
        ),
        With<PlayerPrivateState>,
    >,
) {
    let Some(client_id) = runtime.client_id else {
        state.entity = None;
        state.private = None;
        state.lifecycle = None;
        return;
    };

    state.private = private.single().ok().map(
        |(inventory, crafting, containers, input_ack, claim_status)| PlayerPrivate {
            inventory: inventory.0.clone(),
            crafting: crafting.0.clone(),
            open_furnace: containers.open_furnace.clone(),
            open_loot_bag: containers.open_loot_bag.clone(),
            open_workbench: containers.open_workbench,
            last_processed_input: input_ack.last_processed_input,
            applied_action_seq: input_ack.applied_action_seq,
            run_speed_multiplier: input_ack.run_speed_multiplier,
            claim_status: *claim_status,
        },
    );

    for (entity, player, lifecycle, health) in &players {
        if player.client_id == client_id {
            // Authoritative health -> the predicted value the HUD renders.
            if let Some(health) = health {
                runtime.sync_local_health(health.0);
            }
            let prior = state.lifecycle;
            state.entity = Some(entity);
            state.lifecycle = lifecycle.copied();
            // Auto-clear the death splash when the replicated lifecycle
            // transitions Dead → Alive (i.e. server-side respawn has landed).
            // Gated on the transition rather than "current lifecycle is Alive"
            // so a `PlayerKilled` message arriving on the same frame as the
            // initial component spawn (lifecycle defaults to `Alive`) can't
            // silently dismiss the splash before the Dead diff lands.
            if menu.death_splash.is_some()
                && matches!(prior, Some(PlayerLifecycle::Dead { .. }))
                && matches!(lifecycle, Some(PlayerLifecycle::Alive))
            {
                menu.death_splash = None;
            }
            return;
        }
    }

    state.entity = None;
    state.lifecycle = None;
}

/// Fold the client-side optimistic prediction overlay onto the just-synced
/// local player inventory. Runs immediately after
/// [`update_local_player_state_system`] (same `LocalPlayerSync` set), so it
/// reads the freshly-replicated inventory and overwrites
/// `LocalPlayerState.private.inventory` *in place* with the predicted result.
/// Every downstream consumer (inventory UI, actionbar, gather eligibility,
/// `observe_inventory` sound/flash) reads through that field, so none of them
/// need to know prediction exists, they just see the optimistic state, which
/// reconciles to the authoritative one as the server confirms or rejects each
/// action.
///
/// See [`PredictionState`] for the reconciliation invariant. The raw
/// replicated inventory still lives on the `PlayerInventory` ECS component;
/// this only overwrites the per-frame cache clone.
pub(crate) fn apply_prediction_overlay_system(
    runtime: Res<ClientRuntime>,
    mut prediction: ResMut<PredictionState>,
    mut state: ResMut<LocalPlayerState>,
) {
    // No local session, or the player is down: a corpse has no pending
    // inventory actions, and respawn hands back a fresh authoritative
    // inventory that stale ops must not be replayed onto. Clear and bail.
    let disconnected = runtime.client_id.is_none();
    let dead = matches!(state.lifecycle, Some(PlayerLifecycle::Dead { .. }));
    if disconnected || dead {
        prediction.clear();
        return;
    }

    let Some(private) = state.private.as_mut() else {
        return;
    };

    // Drop ops the server has already processed (confirmed or rejected),
    // then replay the survivors onto the replicated base for display.
    prediction.prune(private.applied_action_seq);
    if prediction.is_idle() {
        return;
    }
    private.inventory = prediction.rebuild_effective(&private.inventory);
}
