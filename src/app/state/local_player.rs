//! Client-side mirror of the local player's replicated components.
//!
//! UI and input systems need to read the local player's inventory,
//! crafting queue, and open-furnace state. Those live on the
//! Lightyear-replicated `PlayerPublic` / `PlayerPrivate` components on
//! the local player's entity. A single per-frame system scans the
//! `Player` query to find whichever entity matches
//! `ClientRuntime::client_id` and caches a clone of the components so
//! UI helpers (which don't own a query themselves) can read the data
//! via a plain `Res<LocalPlayerState>`.
//!
//! Clones are cheap for one entity per frame.

use bevy::prelude::*;

use crate::server::{Player, PlayerLifecycle, PlayerPrivate, PlayerPublic};

use super::{ClientRuntime, MenuState, PredictionState};

/// Refreshed every frame from the replicated Player entity whose
/// `Player.client_id == runtime.client_id`. `None` until the local
/// session connects and Lightyear has shipped the entity.
#[derive(Resource, Default, Debug)]
pub(crate) struct LocalPlayerState {
    pub(crate) entity: Option<Entity>,
    pub(crate) public: Option<PlayerPublic>,
    pub(crate) private: Option<PlayerPrivate>,
    pub(crate) lifecycle: Option<PlayerLifecycle>,
}

#[allow(clippy::type_complexity)]
pub(crate) fn update_local_player_state_system(
    runtime: Res<ClientRuntime>,
    mut state: ResMut<LocalPlayerState>,
    mut menu: ResMut<MenuState>,
    players: Query<(
        Entity,
        &Player,
        &PlayerPublic,
        Option<&PlayerPrivate>,
        Option<&PlayerLifecycle>,
    )>,
) {
    let Some(client_id) = runtime.client_id else {
        state.entity = None;
        state.public = None;
        state.private = None;
        state.lifecycle = None;
        return;
    };

    for (entity, player, public, private, lifecycle) in &players {
        if player.client_id == client_id {
            let prior = state.lifecycle;
            state.entity = Some(entity);
            state.public = Some(public.clone());
            state.private = private.cloned();
            state.lifecycle = lifecycle.copied();
            // Auto-clear the death splash when the replicated
            // lifecycle transitions Dead → Alive (i.e. server-side
            // respawn has landed). Gated on the transition rather
            // than "current lifecycle is Alive" so a `PlayerKilled`
            // message arriving on the same frame as the initial
            // component spawn (lifecycle defaults to `Alive`) can't
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
    state.public = None;
    state.private = None;
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
/// replicated inventory still lives on the `PlayerPrivate` ECS component; this
/// only overwrites the per-frame cache clone.
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
