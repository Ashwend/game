use crate::protocol::{
    ClientId, DeployedEntityId, DroppedItemId, ResourceNodeId, ResourceNodeState, Vec3Net,
};

use super::{
    DeliveryTarget, GameServer, PlayerArmor, ServerEnvelope, deployable_ecs,
    deployables::DeployedEntity, dropped_items::DroppedItemBody, player_ecs,
    voice::within_range_sq,
};

impl GameServer {
    /// Per-client envelopes for a cosmetic, world-anchored event that
    /// only nearby players can perceive (impact bursts, merge cues).
    /// `DeliveryTarget::Broadcast*` would ship the message to every
    /// client on the server, including players the full map away; this
    /// filters to online clients within `range` metres of `position`,
    /// skipping the optional `except` client (typically the actor whose
    /// own client already produced the effect via prediction). Same
    /// shape as the voice fan-out in `voice.rs`.
    pub(super) fn envelopes_within_range(
        &self,
        position: Vec3Net,
        range: f32,
        except: Option<ClientId>,
        message: super::ServerMessage,
    ) -> Vec<ServerEnvelope> {
        let range_sq = range * range;
        self.clients
            .values()
            .filter(|client| client.online)
            .filter(|client| Some(client.client_id) != except)
            .filter(|client| within_range_sq(position, client.controller.position, range_sq))
            .map(|client| ServerEnvelope {
                target: DeliveryTarget::Client(client.client_id),
                message: message.clone(),
            })
            .collect()
    }

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

    /// The cheap key that decides a client's AoI subscription set: their anchor
    /// chunk plus view tier. The loaded-chunk grid is fixed after world
    /// construction, so when this key is unchanged the add/keep chunk sets are
    /// identical to last tick and the room-subscription system can skip the
    /// per-chunk grid scan entirely. `None` if the client isn't connected.
    pub fn client_aoi_key(
        &self,
        client_id: ClientId,
    ) -> Option<(crate::world::ChunkCoord, crate::protocol::ViewRadiusTier)> {
        self.client_view(client_id)
            .map(|(pos, tier)| (crate::world::ChunkCoord::from_world(pos.x, pos.z), tier))
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

    /// Read-only iteration over live dropped items (id + wire-shape view).
    /// The mirror sync reads per-id deltas via [`Self::dropped_item_state`];
    /// this stays for tests and diagnostics.
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

    /// Read a single dropped item's wire-shape state. Used by the mirror
    /// sync to fetch only the items it needs to (re)spawn or update.
    pub fn dropped_item_state(
        &self,
        id: DroppedItemId,
    ) -> Option<&crate::protocol::DroppedWorldItem> {
        self.dropped_items.get(&id).map(|body| &body.item)
    }

    /// Insert (or replace) a dropped item and record it for the next mirror
    /// sync. The single entry point for adding drops, keeps
    /// `dropped_item_sync_dirty` accurate.
    pub(super) fn insert_dropped_item(&mut self, id: DroppedItemId, body: DroppedItemBody) {
        self.dropped_items.insert(id, body);
        self.dropped_item_sync_dirty.insert(id);
        self.dropped_item_sync_removed.remove(&id);
    }

    /// Remove a dropped item and record it for the next mirror sync (which
    /// despawns the replicated entity). Returns the removed body, mirroring
    /// `HashMap`.
    pub(super) fn remove_dropped_item(&mut self, id: DroppedItemId) -> Option<DroppedItemBody> {
        let removed = self.dropped_items.remove(&id);
        if removed.is_some() {
            self.dropped_item_sync_removed.insert(id);
            self.dropped_item_sync_dirty.remove(&id);
        }
        removed
    }

    /// Mutable access to a dropped item's body, conservatively flagging it
    /// dirty for the next mirror sync (any hand-out of `&mut` may change the
    /// item). The single entry point for in-place drop edits. The per-tick
    /// physics step does NOT go through here, it marks only the bodies whose
    /// transform actually changed so at-rest items stay out of the delta.
    pub(super) fn dropped_item_body_mut(
        &mut self,
        id: DroppedItemId,
    ) -> Option<&mut DroppedItemBody> {
        if self.dropped_items.contains_key(&id) {
            self.dropped_item_sync_dirty.insert(id);
            self.dropped_item_sync_removed.remove(&id);
        }
        self.dropped_items.get_mut(&id)
    }

    /// Drain the accumulated mirror-sync deltas: `(dirty ids, removed ids)`.
    /// Called once per tick by `sync_dropped_item_entities`.
    pub fn drain_dropped_item_sync(&mut self) -> (Vec<DroppedItemId>, Vec<DroppedItemId>) {
        (
            self.dropped_item_sync_dirty.drain().collect(),
            self.dropped_item_sync_removed.drain().collect(),
        )
    }

    /// Chunk a deployable is anchored to.
    pub fn deployable_chunk(&self, id: DeployedEntityId) -> Option<crate::world::ChunkCoord> {
        self.chunk_manager.deployed_entity_chunk(id)
    }

    /// Read a single deployable as the wire-shape view the mirror needs.
    pub fn deployable_view(&self, id: DeployedEntityId) -> Option<deployable_ecs::DeployableView> {
        self.deployed_entities.get(&id).map(|entity| {
            // `active` doubles as the door's open flag, the furnace's burn
            // state, and the torch's lit flag. A kind has at most one of the
            // three, so OR-ing them is unambiguous.
            let active = entity.furnace.as_ref().map(|f| f.active).unwrap_or(false)
                || entity.door.as_ref().map(|door| door.open).unwrap_or(false)
                || entity
                    .torch
                    .as_ref()
                    .map(|torch| torch.active)
                    .unwrap_or(false);
            deployable_ecs::DeployableView {
                id: entity.id,
                item_id: entity.item_id.clone(),
                kind: entity.kind,
                position: entity.position,
                yaw: entity.yaw,
                health: entity.health,
                max_health: entity.max_health,
                active,
                owner: entity.owner,
                label: entity.label.clone(),
                stability: entity.stability,
            }
        })
    }

    /// Insert (or replace) a deployable and record it for the next mirror
    /// sync. The single entry point for adding placed structures, keeps
    /// `deployable_sync_dirty` accurate and mirrors the structure's solid
    /// boxes into the dropped-item physics world so items land on it.
    pub(super) fn insert_deployed_entity(&mut self, id: DeployedEntityId, entity: DeployedEntity) {
        self.dropped_item_physics
            .sync_deployable_colliders(id, &entity.resolved_collider_blocks());
        self.deployed_entities.insert(id, entity);
        self.deployable_sync_dirty.insert(id);
        self.deployable_sync_removed.remove(&id);
    }

    /// Remove a deployable and record it for the next mirror sync (which
    /// despawns the replicated entity). Returns the removed state, mirroring
    /// `HashMap`. Also drops the structure's dropped-item physics colliders,
    /// waking items that rested on them.
    pub(super) fn remove_deployed_entity(
        &mut self,
        id: DeployedEntityId,
    ) -> Option<DeployedEntity> {
        let removed = self.deployed_entities.remove(&id);
        if removed.is_some() {
            self.dropped_item_physics.remove_deployable_colliders(id);
            self.deployable_sync_removed.insert(id);
            self.deployable_sync_dirty.remove(&id);
        }
        removed
    }

    /// Re-mirror one deployable's solid boxes into the dropped-item
    /// physics world after an in-place mutation that changed them (today:
    /// the door open/close toggle, which moves the panel's box between
    /// the closed plane and the swung pose).
    pub(super) fn refresh_deployable_physics_colliders(&mut self, id: DeployedEntityId) {
        let Some(blocks) = self
            .deployed_entities
            .get(&id)
            .map(DeployedEntity::resolved_collider_blocks)
        else {
            return;
        };
        self.dropped_item_physics
            .sync_deployable_colliders(id, &blocks);
    }

    /// Mutable access to a deployable, conservatively flagging it dirty for
    /// the next mirror sync (any hand-out of `&mut` may change the entity).
    /// The single entry point for in-place deployable edits.
    pub(super) fn deployed_entity_mut(
        &mut self,
        id: DeployedEntityId,
    ) -> Option<&mut DeployedEntity> {
        self.mark_deployable_dirty(id);
        self.deployed_entities.get_mut(&id)
    }

    /// Record a deployable as needing a mirror re-sync without handing out a
    /// `&mut`. Used by the furnace tick, which iterates the map directly and
    /// only flags the entities whose replicated `active` flag actually
    /// flipped, so idle furnaces never enter the delta.
    pub(super) fn mark_deployable_dirty(&mut self, id: DeployedEntityId) {
        if self.deployed_entities.contains_key(&id) {
            self.deployable_sync_dirty.insert(id);
            self.deployable_sync_removed.remove(&id);
        }
    }

    /// Drain the accumulated mirror-sync deltas: `(dirty ids, removed ids)`.
    /// Called once per tick by `sync_deployable_entities`.
    pub fn drain_deployable_sync(&mut self) -> (Vec<DeployedEntityId>, Vec<DeployedEntityId>) {
        (
            self.deployable_sync_dirty.drain().collect(),
            self.deployable_sync_removed.drain().collect(),
        )
    }

    /// Iterate connected players as wire-shape views (one field per
    /// replicated component) for the player mirror.
    pub fn players_iter(&self) -> impl Iterator<Item = player_ecs::PlayerView> + '_ {
        self.clients.values().map(|client| player_ecs::PlayerView {
            client_id: client.client_id,
            account_id: client.account_id,
            profile: player_ecs::PlayerProfile {
                name: client.name.clone(),
                is_admin: client.is_admin,
            },
            pose: player_ecs::PlayerPose {
                position: client.controller.position,
                velocity: client.controller.velocity,
                yaw: client.controller.yaw,
                pitch: client.controller.pitch,
                grounded: client.controller.grounded,
            },
            health: player_ecs::PlayerHealth(client.controller.health),
            chat_bubble: player_ecs::PlayerChatBubble(
                client
                    .chat_bubble
                    .as_ref()
                    .map(|bubble| bubble.text.clone()),
            ),
            inventory: player_ecs::PlayerInventory(client.inventory.clone()),
            crafting: player_ecs::PlayerCrafting(client.crafting.clone()),
            containers: player_ecs::PlayerOpenContainers {
                open_furnace: self.open_furnace_view_for(client.client_id),
                open_loot_bag: self.open_loot_bag_view_for(client.client_id),
            },
            input_ack: player_ecs::PlayerInputAck {
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
