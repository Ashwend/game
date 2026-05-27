//! ECS mirror for authoritative players.
//!
//! Companion to [`crate::server::resource_node_ecs`]. Player state lives
//! in `GameServer::clients: HashMap<ClientId, ServerClient>`; the
//! `sync_player_entities` system in `net/host.rs` reconciles that map
//! into ECS entities so Phase 4/5 chunk-room replication can attach
//! `Replicate` per entity.
//!
//! Split into [`PlayerPublic`] (replicated to every client in the same
//! room) and [`PlayerPrivate`] (replicated only to the owning client).
//! This is the shape we want for Phase 5 — putting it on the components
//! now means the replication wiring later just needs `Replicate` markers
//! with the right `NetworkTarget`, no further refactor.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    protocol::{
        ClientId, OpenFurnaceView, PlayerCraftingState, PlayerInventoryState, SteamId, Vec3Net,
    },
    world::ChunkCoord,
};

/// Identity. Immutable after spawn. The wire-stable `client_id` is the
/// link back to the Lightyear `ClientOf` connection entity, and is what
/// every gameplay message refers to.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub client_id: ClientId,
    pub steam_id: SteamId,
}

/// Player state that every peer in the same chunk room can see. Phase 5
/// marks this with `Replicate::to_clients(NetworkTarget::All)` and lets
/// the room machinery + `NetworkVisibility` gate per-client.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerPublic {
    pub name: String,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub grounded: bool,
    pub is_admin: bool,
    /// Most recent chat bubble text, or `None` if the bubble window has
    /// expired. Only the text is public — the expiry tick is server-only
    /// bookkeeping.
    pub chat_bubble: Option<String>,
}

/// Player state that only the owning client should ever see. Phase 5
/// pairs the entity-level `Replicate` (broadcast to all clients in the
/// chunk room) with a per-component `ComponentReplicationOverrides<PlayerPrivate>`
/// that disables this component for every sender except the owning
/// client's link entity. Peers therefore never receive the wire bytes.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerPrivate {
    pub inventory: PlayerInventoryState,
    pub crafting: PlayerCraftingState,
    /// Full open-furnace view for the smelt UI. `None` when the player
    /// hasn't opened any furnace. This carries the whole `OpenFurnaceView`
    /// rather than just the deployable id so the client can render the
    /// inputs/outputs/progress bars without a separate message —
    /// `PlayerPrivate` only replicates to the owner (Phase 5 override),
    /// so the contents stay private without any extra wire-side gating.
    pub open_furnace: Option<OpenFurnaceView>,
    pub last_processed_input: u64,
}

/// Anchor chunk for room subscription. Updated when the player crosses
/// a chunk boundary (mirror reads `ChunkManager::player_chunk`).
#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerChunk(pub ChunkCoord);

/// `ClientId → Entity` lookup so gather/chat/inventory paths can resolve
/// a player in O(1).
#[derive(Resource, Default, Debug)]
pub struct PlayerIndex {
    by_id: HashMap<ClientId, Entity>,
}

impl PlayerIndex {
    pub fn get(&self, id: ClientId) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }

    pub fn insert(&mut self, id: ClientId, entity: Entity) {
        self.by_id.insert(id, entity);
    }

    pub fn remove(&mut self, id: ClientId) -> Option<Entity> {
        self.by_id.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ClientId, Entity)> + '_ {
        self.by_id.iter().map(|(id, entity)| (*id, *entity))
    }
}

/// Wire-shape view used by the mirror to spawn or refresh a player
/// entity. Mirrors `ServerClient` without taking a copy of its internal
/// shape.
pub struct PlayerView {
    pub client_id: ClientId,
    pub steam_id: SteamId,
    pub public: PlayerPublic,
    pub private: PlayerPrivate,
}

pub fn spawn_player_entity(world: &mut World, view: PlayerView, chunk: ChunkCoord) -> Entity {
    let id = view.client_id;
    let entity = world
        .spawn((
            Player {
                client_id: view.client_id,
                steam_id: view.steam_id,
            },
            view.public,
            view.private,
            PlayerChunk(chunk),
        ))
        .id();
    world.resource_mut::<PlayerIndex>().insert(id, entity);
    entity
}

pub fn despawn_player_entity(world: &mut World, id: ClientId) -> Option<Entity> {
    let entity = world.resource_mut::<PlayerIndex>().remove(id)?;
    if let Ok(entity_world) = world.get_entity_mut(entity) {
        entity_world.despawn();
    }
    Some(entity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_world() -> World {
        let mut world = World::new();
        world.init_resource::<PlayerIndex>();
        world
    }

    fn sample_view(client_id: ClientId) -> PlayerView {
        PlayerView {
            client_id,
            steam_id: 42,
            public: PlayerPublic {
                name: "Alice".to_owned(),
                position: Vec3Net::ZERO,
                velocity: Vec3Net::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                health: 100.0,
                grounded: true,
                is_admin: false,
                chat_bubble: None,
            },
            private: PlayerPrivate {
                inventory: PlayerInventoryState::empty(),
                crafting: PlayerCraftingState::default(),
                open_furnace: None,
                last_processed_input: 0,
            },
        }
    }

    #[test]
    fn spawn_and_despawn_round_trip_index() {
        let mut world = fresh_world();
        let entity = spawn_player_entity(&mut world, sample_view(1), ChunkCoord::new(0, 0));
        assert_eq!(world.resource::<PlayerIndex>().get(1), Some(entity));

        let public = world.get::<PlayerPublic>(entity).expect("public");
        assert_eq!(public.name, "Alice");
        let private = world.get::<PlayerPrivate>(entity).expect("private");
        assert_eq!(private.last_processed_input, 0);

        let despawned = despawn_player_entity(&mut world, 1);
        assert_eq!(despawned, Some(entity));
        assert!(world.resource::<PlayerIndex>().get(1).is_none());
    }
}
