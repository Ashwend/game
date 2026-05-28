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

use crate::server::{Player, PlayerPrivate, PlayerPublic};

use super::ClientRuntime;

/// Refreshed every frame from the replicated Player entity whose
/// `Player.client_id == runtime.client_id`. `None` until the local
/// session connects and Lightyear has shipped the entity.
#[derive(Resource, Default, Debug)]
pub(crate) struct LocalPlayerState {
    pub(crate) entity: Option<Entity>,
    pub(crate) public: Option<PlayerPublic>,
    pub(crate) private: Option<PlayerPrivate>,
}

pub(crate) fn update_local_player_state_system(
    runtime: Res<ClientRuntime>,
    mut state: ResMut<LocalPlayerState>,
    players: Query<(Entity, &Player, &PlayerPublic, Option<&PlayerPrivate>)>,
) {
    let Some(client_id) = runtime.client_id else {
        state.entity = None;
        state.public = None;
        state.private = None;
        return;
    };

    for (entity, player, public, private) in &players {
        if player.client_id == client_id {
            state.entity = Some(entity);
            state.public = Some(public.clone());
            state.private = private.cloned();
            return;
        }
    }

    state.entity = None;
    state.public = None;
    state.private = None;
}
