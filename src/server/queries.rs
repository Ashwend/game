use crate::protocol::{
    ClientId, DeployedEntityId, DroppedItemId, ResourceNodeId, ResourceNodeState, Vec3Net,
};

use super::{GameServer, PlayerArmor, deployable_ecs, player_ecs};

impl GameServer {
    /// Read-only view of a connected player's anchor position and AoI tier.
    /// Used by the chunk-room subscription system in `net/host` to recompute
    /// which rooms a client should be in each tick.
    pub fn client_view(
        &self,
        client_id: ClientId,
    ) -> Option<(Vec3Net, crate::protocol::ViewRadiusTier)> {
        self.clients
            .get(&client_id)
            .map(|client| (client.controller.position, client.view_tier))
    }

    /// Currently-connected client ids. Cheap; backed by the connection map.
    pub fn connected_client_ids(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.clients.keys().copied()
    }

    /// Chunks the given client's AoI ring currently covers. Returns an
    /// empty set if the client isn't connected.
    pub fn visible_chunks_for_client(
        &self,
        client_id: ClientId,
    ) -> std::collections::HashSet<crate::world::ChunkCoord> {
        let Some((position, tier)) = self.client_view(client_id) else {
            return std::collections::HashSet::new();
        };
        self.chunk_manager.visible_chunks(position, tier)
    }

    /// Chunks the given client's subscription should be *retained* for,
    /// the AoI ring widened by the spatial-hysteresis keep margin. Always a
    /// superset of `visible_chunks_for_client`. Empty if the client isn't
    /// connected.
    pub fn retained_chunks_for_client(
        &self,
        client_id: ClientId,
    ) -> std::collections::HashSet<crate::world::ChunkCoord> {
        let Some((position, tier)) = self.client_view(client_id) else {
            return std::collections::HashSet::new();
        };
        self.chunk_manager.retained_chunks(position, tier)
    }

    /// Read-only access to the live resource node map. Used by the ECS
    /// mirror system in `net/host` to keep entity state in sync with this
    /// authoritative map. Avoid mutating callers; use the existing gather/
    /// regrow paths which already update chunk_manager bookkeeping.
    pub fn resource_nodes_iter(
        &self,
    ) -> impl Iterator<Item = (&ResourceNodeId, &ResourceNodeState)> {
        self.resource_nodes.iter()
    }

    /// Quick membership check, used by the mirror sync to decide which
    /// tracked entities to despawn this tick.
    pub fn has_resource_node(&self, id: ResourceNodeId) -> bool {
        self.resource_nodes.contains_key(&id)
    }

    /// Look up the chunk an active node is anchored to. Returns `None` for
    /// a node id the chunk_manager doesn't know about (which is the normal
    /// state immediately after depletion).
    pub fn resource_node_chunk(&self, id: ResourceNodeId) -> Option<crate::world::ChunkCoord> {
        self.chunk_manager.node_chunk(id)
    }

    /// Read a single node's authoritative state. Used by the mirror sync to
    /// fetch only the nodes it needs to (re)spawn or update this tick.
    pub fn resource_node_state(&self, id: ResourceNodeId) -> Option<&ResourceNodeState> {
        self.resource_nodes.get(&id)
    }

    /// Insert (or replace) a node and record it for the next mirror sync. The
    /// single entry point for adding nodes, keeps `node_sync_dirty` accurate.
    pub(crate) fn insert_resource_node(&mut self, id: ResourceNodeId, node: ResourceNodeState) {
        self.resource_nodes.insert(id, node);
        self.node_sync_dirty.insert(id);
        self.node_sync_removed.remove(&id);
    }

    /// Remove a node and record it for the next mirror sync (which despawns the
    /// replicated entity). Returns the removed state, mirroring `HashMap`.
    pub(crate) fn remove_resource_node(&mut self, id: ResourceNodeId) -> Option<ResourceNodeState> {
        let removed = self.resource_nodes.remove(&id);
        if removed.is_some() {
            self.node_sync_removed.insert(id);
            self.node_sync_dirty.remove(&id);
        }
        removed
    }

    /// Mutable access to a node, conservatively flagging it dirty for the next
    /// mirror sync (any hand-out of `&mut` may change the node). The single
    /// entry point for in-place node edits.
    pub(crate) fn resource_node_state_mut(
        &mut self,
        id: ResourceNodeId,
    ) -> Option<&mut ResourceNodeState> {
        // Mark before borrowing the map mutably; the change-detection compare
        // still happens on the actual value in the sync, so a spurious mark
        // just costs one no-op delta entry.
        if self.resource_nodes.contains_key(&id) {
            self.node_sync_dirty.insert(id);
            self.node_sync_removed.remove(&id);
        }
        self.resource_nodes.get_mut(&id)
    }

    /// Drain the accumulated mirror-sync deltas: `(dirty ids, removed ids)`.
    /// Called once per tick by `sync_resource_node_entities`.
    pub fn drain_resource_node_sync(&mut self) -> (Vec<ResourceNodeId>, Vec<ResourceNodeId>) {
        (
            self.node_sync_dirty.drain().collect(),
            self.node_sync_removed.drain().collect(),
        )
    }

    /// Iterate live dropped items (id + wire-shape view) for the
    /// `sync_dropped_item_entities` mirror.
    pub fn dropped_items_iter(
        &self,
    ) -> impl Iterator<Item = (DroppedItemId, crate::protocol::DroppedWorldItem)> + '_ {
        self.dropped_items
            .iter()
            .map(|(id, body)| (*id, body.item.clone()))
    }

    /// Chunk a dropped item is anchored to (per chunk_manager bookkeeping).
    pub fn dropped_item_chunk(&self, id: DroppedItemId) -> Option<crate::world::ChunkCoord> {
        self.chunk_manager.dropped_item_chunk(id)
    }

    /// Iterate live deployables as the wire-shape view the mirror needs.
    pub fn deployables_iter(&self) -> impl Iterator<Item = deployable_ecs::DeployableView> + '_ {
        self.deployed_entities.values().map(|entity| {
            let active = entity.furnace.as_ref().map(|f| f.active).unwrap_or(false);
            deployable_ecs::DeployableView {
                id: entity.id,
                item_id: entity.item_id.clone(),
                kind: entity.kind,
                position: entity.position,
                yaw: entity.yaw,
                health: entity.health,
                max_health: entity.max_health,
                active,
            }
        })
    }

    /// Chunk a deployable is anchored to.
    pub fn deployable_chunk(&self, id: DeployedEntityId) -> Option<crate::world::ChunkCoord> {
        self.chunk_manager.deployed_entity_chunk(id)
    }

    /// Iterate connected players as wire-shape views (public + private
    /// split) for the player mirror.
    pub fn players_iter(&self) -> impl Iterator<Item = player_ecs::PlayerView> + '_ {
        self.clients.values().map(|client| player_ecs::PlayerView {
            client_id: client.client_id,
            account_id: client.account_id,
            public: player_ecs::PlayerPublic {
                name: client.name.clone(),
                position: client.controller.position,
                velocity: client.controller.velocity,
                yaw: client.controller.yaw,
                pitch: client.controller.pitch,
                health: client.controller.health,
                grounded: client.controller.grounded,
                is_admin: client.is_admin,
                chat_bubble: client
                    .chat_bubble
                    .as_ref()
                    .map(|bubble| bubble.text.clone()),
            },
            private: player_ecs::PlayerPrivate {
                inventory: client.inventory.clone(),
                crafting: client.crafting.clone(),
                open_furnace: self.open_furnace_view_for(client.client_id),
                open_loot_bag: self.open_loot_bag_view_for(client.client_id),
                last_processed_input: client.controller.last_processed_input,
                applied_action_seq: client.applied_action_seq,
            },
            armor: PlayerArmor(client.armor),
            lifecycle: client.lifecycle,
            sleeping: player_ecs::PlayerSleeping(!client.online),
        })
    }

    /// Chunk a connected player is anchored to.
    pub fn player_chunk(&self, id: ClientId) -> Option<crate::world::ChunkCoord> {
        self.chunk_manager.player_chunk(id)
    }
}
