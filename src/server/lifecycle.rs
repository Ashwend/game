use std::collections::HashMap;

use crate::{
    controller::BlockGrid,
    protocol::{
        ChatMessage, ResourceNodeId, ResourceNodeState, ServerMessage, Vec3Net, sanitize_chat,
    },
    save::WorldSave,
};

use super::{
    ChunkManager, DeliveryTarget, GameServer, ServerEnvelope, ServerSettings,
    dirty_tracked_map::DirtyTrackedMap,
    dropped_items::{DroppedItemBody, DroppedItemPhysics},
};

/// Floor a persisted monotonic id counter on load so the next issued id can
/// never collide with a live entity. Returns at least 1, at least the saved
/// value, and at least one past the highest live id. The three persisted
/// entity counters (dropped items, resource nodes, deployables) all need this;
/// keeping it in one helper stops them from drifting (the dropped-item counter
/// previously only floored at 1, ignoring the live ids).
pub(super) fn next_id_floor(saved: u64, live_ids: impl IntoIterator<Item = u64>) -> u64 {
    let highest_live = live_ids.into_iter().max().unwrap_or(0);
    saved.max(highest_live.saturating_add(1)).max(1)
}

impl GameServer {
    pub fn new(mut save: WorldSave, settings: ServerSettings) -> Self {
        if let Some(host) = settings.singleplayer_host
            && !save.admins.contains(&host)
        {
            save.admins.push(host);
        }
        let world = save.map.world_data();
        let world_grid = BlockGrid::build(&world);
        let mut dropped_item_physics = DroppedItemPhysics::new(&world);

        let load_tick_for_chunk = save.state.last_authoritative_tick;
        // Resource nodes: trust the saved state once a world has ever been
        // hosted (so harvested resources don't respawn). For brand-new worlds
        // the save has `None` and we seed from the chunk generator.
        let (mut chunk_manager, resource_nodes) = match (
            save.state.resource_nodes.take(),
            save.state.chunk_manager.take(),
        ) {
            (Some(saved_nodes), Some(saved_chunk)) => {
                let nodes: HashMap<ResourceNodeId, ResourceNodeState> = saved_nodes
                    .into_iter()
                    .map(|node| (node.id, node))
                    .collect();
                let manager = ChunkManager::from_save(saved_chunk, load_tick_for_chunk);
                (manager, nodes)
            }
            _ => {
                // Brand-new world: generate from seed + dims. Any partial
                // save without grid state would also fall here, but
                // that's prevented at the save-format level (version
                // bumps are not migrated).
                let (manager, spawns) =
                    ChunkManager::new_for_world(save.map.world_seed(), save.map.chunk_dims());
                let nodes: HashMap<ResourceNodeId, ResourceNodeState> =
                    spawns.into_iter().map(|node| (node.id, node)).collect();
                (manager, nodes)
            }
        };

        let mut dropped_items = HashMap::new();
        let load_tick = save.state.last_authoritative_tick;
        for item in std::mem::take(&mut save.state.dropped_items) {
            let physics_body =
                dropped_item_physics.spawn_body(item.position, Vec3Net::ZERO, item.yaw);
            // Anchor the reloaded drop to its chunk so a returning
            // player immediately sees it via room replication, without
            // this the item exists server-side but is filtered out of
            // every AoI ring until something nudges its position.
            chunk_manager.track_dropped_item(item.id, item.position);
            dropped_items.insert(
                item.id,
                DroppedItemBody {
                    item,
                    body_handle: physics_body.body_handle,
                    // Reset the timer on load so a returning player doesn't
                    // find every dropped item already past its expiry.
                    spawn_tick: load_tick,
                },
            );
        }

        let next_dropped_item_id = next_id_floor(
            save.state.next_dropped_item_id,
            dropped_items.keys().copied(),
        );
        let mut next_client_id = save.state.next_client_id.max(1);

        // Rebuild every persisted player as a logged-out sleeping body so
        // a server restart doesn't despawn anyone: bodies come back at
        // their saved position with their saved health and inventory,
        // replicated, lootable, and killable, exactly as they were before
        // the shutdown. A reconnect from the same account routes through
        // the regular wake-sleeper path because `account_to_client` is
        // seeded here too. Sorted by account id so client-id assignment
        // is deterministic across boots.
        let mut persisted = std::mem::take(&mut save.state.players);
        persisted.sort_by_key(|player| player.account_id);
        let mut clients = HashMap::new();
        let mut account_to_client = HashMap::new();
        for player in persisted {
            let client_id = next_client_id;
            next_client_id += 1;
            let body = super::sleeping_body_from_persisted(player, client_id, load_tick);
            account_to_client.insert(body.account_id, client_id);
            chunk_manager.track_player(client_id, body.controller.position);
            clients.insert(client_id, body);
        }
        // Bodies are authoritative now; the crash-safety snapshot list
        // starts empty and refills as bodies change (`world_save` captures
        // live client state directly).
        let persisted_players = HashMap::new();
        // Floor at the chunk-generator's high-water mark so admin-spawned
        // ids can't collide with chunk-issued node ids, regardless of how
        // many nodes the world generator produced, and above every live node.
        let next_resource_node_id = next_id_floor(
            save.state
                .next_resource_node_id
                .max(chunk_manager.next_node_id()),
            resource_nodes.keys().copied(),
        );
        // Deployables: restore from save and re-anchor to their chunks
        // so the next mirror sync spawns the replicated entity and any
        // in-AoI client picks them up. The id counter floors above the
        // highest known id so a future place can't collide with a
        // persisted one.
        let persisted_deployables = std::mem::take(&mut save.state.deployed_entities);
        let deployed_entities = Self::restore_deployed_entities(persisted_deployables);
        for entity in deployed_entities.values() {
            chunk_manager.track_deployed_entity(entity.id, entity.position);
            // Mirror the structure's solid boxes into the dropped-item
            // physics world, same as a live placement does, so reloaded
            // drops keep resting on building floors.
            dropped_item_physics
                .sync_deployable_colliders(entity.id, &entity.resolved_collider_blocks());
        }
        let next_deployed_entity_id = next_id_floor(
            save.state.next_deployed_entity_id,
            deployed_entities.keys().copied(),
        );
        // Per-player map markers: rebuild the store from the save (floors the
        // id counter above the highest survivor internally).
        let world_map_markers = super::world_map::WorldMapMarkerStore::from_persisted(
            std::mem::take(&mut save.state.world_map_markers),
        );
        let world_time = save.state.world_time();
        let tick = save.state.last_authoritative_tick;

        // Wrap the authoritative maps in dirty-tracked stores and seed every
        // initial entry dirty so the first mirror sync spawns all mirror
        // entities once; after that only mutated ids are reprocessed.
        let mut dropped_items = DirtyTrackedMap::from_map(dropped_items);
        dropped_items.seed_all_dirty();
        let mut resource_nodes = DirtyTrackedMap::from_map(resource_nodes);
        resource_nodes.seed_all_dirty();
        let mut deployed_entities = DirtyTrackedMap::from_map(deployed_entities);
        deployed_entities.seed_all_dirty();

        let mut server = Self {
            tick,
            save,
            world,
            world_grid,
            settings,
            workos: None,
            clients,
            account_to_client,
            persisted_players,
            dropped_items,
            dropped_item_physics,
            resource_nodes,
            chunk_manager,
            deployed_entities,
            loot_bags: HashMap::new(),
            next_dropped_item_id,
            next_client_id,
            next_resource_node_id,
            next_deployed_entity_id,
            next_loot_bag_id: 1,
            world_time,
            last_world_time_broadcast_tick: tick,
            auto_save_interval_ticks: 0,
            last_auto_save_tick: tick,
            auto_save_pending: false,
            world_map_markers,
        };
        // Stability is not persisted: recompute it from the restored
        // pieces (which also culls anything a legacy save left without a
        // ground path).
        server.refresh_structural_stability();
        server
    }

    /// Attach a WorkOS access-token verifier (dedicated [`AuthMode::Workos`]
    /// only). A builder so the loopback/test construction paths stay untouched.
    pub fn with_workos(
        mut self,
        verifier: Option<std::sync::Arc<crate::auth::WorkosVerifier>>,
    ) -> Self {
        self.workos = verifier;
        self
    }

    /// Enable periodic auto-save (dedicated hosts). `interval_ticks == 0` leaves
    /// it disabled, which is the loopback/singleplayer default (those save on
    /// exit). The schedule counts from the current tick, so the first auto-save
    /// lands one interval after the host starts or a world is loaded.
    pub fn with_auto_save(mut self, interval_ticks: u64) -> Self {
        self.auto_save_interval_ticks = interval_ticks;
        self.last_auto_save_tick = self.tick;
        self
    }

    /// Drain the "auto-save is due" flag. The host calls this after `tick`,
    /// snapshots [`GameServer::world_save`], writes it, and announces the
    /// result, keeping disk I/O out of this authoritative game-state module.
    pub fn take_auto_save_pending(&mut self) -> bool {
        std::mem::take(&mut self.auto_save_pending)
    }

    pub fn announce(&self, text: impl AsRef<str>) -> Vec<ServerEnvelope> {
        sanitize_chat(text.as_ref())
            .map(|text| ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::Chat(ChatMessage {
                    from: "Server".to_owned(),
                    text,
                }),
            })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::next_id_floor;

    #[test]
    fn next_id_floor_never_collides_with_a_live_id() {
        // Empty world: at least 1.
        assert_eq!(next_id_floor(0, std::iter::empty()), 1);
        assert_eq!(next_id_floor(5, std::iter::empty()), 5);
        // Saved counter ahead of the live ids wins.
        assert_eq!(next_id_floor(20, [3, 7, 11]), 20);
        // Under-floored saved counter (the historical dropped-item bug): the
        // helper lifts it above the highest live id so a fresh id cannot reuse
        // a live one.
        assert_eq!(next_id_floor(4, [3, 7, 11]), 12);
        assert_eq!(next_id_floor(0, [9]), 10);
    }
}
