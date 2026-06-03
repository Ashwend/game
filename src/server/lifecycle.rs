use std::collections::{HashMap, HashSet};

use crate::{
    controller::BlockGrid,
    protocol::{
        ChatMessage, ResourceNodeId, ResourceNodeState, ServerMessage, Vec3Net, sanitize_chat,
    },
    save::WorldSave,
};

use super::{
    ChunkManager, DeliveryTarget, GameServer, ServerEnvelope, ServerSettings,
    dropped_items::{DroppedItemBody, DroppedItemPhysics},
};

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

        let persisted_players = std::mem::take(&mut save.state.players)
            .into_iter()
            .map(|player| (player.account_id, player))
            .collect();

        let next_dropped_item_id = save.state.next_dropped_item_id.max(1);
        let next_client_id = save.state.next_client_id.max(1);
        // Floor at the chunk-generator's high-water mark so admin-spawned
        // ids can't collide with chunk-issued node ids, regardless of how
        // many nodes the world generator produced.
        let next_resource_node_id = save.state.next_resource_node_id.max(
            chunk_manager
                .next_node_id()
                .max(resource_nodes.keys().copied().max().unwrap_or(0) + 1),
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
        }
        let next_deployed_entity_id = save.state.next_deployed_entity_id.max(
            deployed_entities
                .keys()
                .copied()
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        let world_time = save.state.world_time();
        let tick = save.state.last_authoritative_tick;

        // Seed the mirror-sync dirty set with every initial node so the first
        // `sync_resource_node_entities` pass spawns all mirror entities once;
        // after that only mutated nodes are reprocessed.
        let node_sync_dirty: HashSet<ResourceNodeId> = resource_nodes.keys().copied().collect();

        Self {
            tick,
            save,
            world,
            world_grid,
            settings,
            workos: None,
            clients: HashMap::new(),
            account_to_client: HashMap::new(),
            persisted_players,
            dropped_items,
            dropped_item_physics,
            resource_nodes,
            node_sync_dirty,
            node_sync_removed: HashSet::new(),
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
        }
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
