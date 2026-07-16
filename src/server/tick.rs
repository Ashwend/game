use bevy::log::info_span;

use crate::protocol::{ClientId, ServerMessage, Vec3Net};

use super::{
    AUTO_SAVE_WARNING_TICKS, DeliveryTarget, GameServer, PERF_STATS_BROADCAST_INTERVAL_TICKS,
    PLAYER_LIST_BROADCAST_INTERVAL_TICKS, ServerEnvelope, WORLD_TIME_BROADCAST_INTERVAL_TICKS,
    dropped_items::{DROPPED_ITEM_CLEANUP_INTERVAL_TICKS, DROPPED_ITEM_MERGE_INTERVAL_TICKS},
};

impl GameServer {
    pub fn tick(&mut self, delta_seconds: f32) -> Vec<ServerEnvelope> {
        let _tick_span = info_span!("server_tick", tick = self.tick + 1).entered();
        self.tick += 1;
        self.save.state.last_authoritative_tick = self.tick;
        self.world_time.advance(delta_seconds);
        self.dropped_item_physics
            .step(delta_seconds, &mut self.dropped_items);
        // Re-anchor the items the physics step actually moved; they're exactly
        // the ones now flagged dirty (spawns and stack edits ride along as
        // cheap "already in this chunk" no-ops). At-rest items skip both the
        // dirty mark and this walk entirely. Collect first so the chunk_manager
        // mutation doesn't overlap the dropped_items borrow.
        let moved: Vec<crate::protocol::DroppedItemId> =
            self.dropped_items.dirty_ids().copied().collect();
        for id in moved {
            if let Some(body) = self.dropped_items.get(&id) {
                let position = body.item.position;
                self.chunk_manager.update_dropped_item_chunk(id, position);
            }
        }
        // Chunk manager owns regrows now, fresh-position spawns 5-15 min
        // after a node is depleted. The result is spliced into the live
        // node map; the mirror sync turns it into a replicated entity
        // on the next Update.
        let regrow = {
            let _span = info_span!("chunk_manager_tick").entered();
            self.chunk_manager.tick(self.tick, &self.resource_nodes)
        };
        for node in regrow.spawned {
            self.insert_resource_node(node.id, node);
        }
        self.tick_furnaces();
        self.tick_torches();
        self.tick_ruin_caches();
        // Tool Cupboard upkeep drain + decay (no-ops between periods).
        // Envelope-free: drained slots reach an open viewer through the
        // per-player container view, and decay HP through the deployable
        // mirror's `DeployableHealth` diff.
        self.tick_upkeep();
        self.tick_loot_bags(delta_seconds);
        self.expire_chat_bubbles();
        // Lift the crossbow reload movement slow off anyone whose reload window
        // (`next_ranged_tick`) elapsed this tick, restoring full movement.
        self.tick_reload_slows();
        // Consumables: apply any bandage whose use charge completed on OUR clock
        // this tick (the client never gets to say it finished), then pay out a
        // tick of every heal-over-time in flight. Both are envelope-free: the
        // health change reaches the local HUD and peer nameplates through the
        // replicated `PlayerHealth` diff, not a bespoke message.
        self.tick_consumable_uses();
        self.tick_heal_over_time();

        // Armed explosive charges: count each fuse down and detonate any that
        // reach zero this tick. Returns the blast consequences (player damage,
        // structure destruction, the VFX/SFX cue), so it has to feed the
        // envelope stream, unlike the (envelope-free) torch/furnace ticks above.
        let mut envelopes = self.tick_fuses();

        // meteor shower event: schedule -> announce -> impact -> cleanup. Real
        // time (this tick count), not the day/night clock, so `/time-speed` does
        // not accelerate meteors. Returns its own broadcast/consequence envelopes.
        envelopes.extend(self.tick_world_events());
        envelopes.extend(self.tick_projectiles(delta_seconds));
        envelopes.extend(self.tick_crafting());
        envelopes.extend(self.disconnect_stale_clients());
        if self.tick.is_multiple_of(DROPPED_ITEM_MERGE_INTERVAL_TICKS) {
            // The merge cue is a quiet UI blip; deliver it only to
            // clients near the merged pile instead of broadcasting
            // every merge on the map to every connected client.
            for (item_id, quantity, position) in self.merge_nearby_dropped_items() {
                envelopes.extend(self.envelopes_within_range(
                    position,
                    crate::game_balance::ITEM_MERGE_CUE_RANGE_M,
                    None,
                    ServerMessage::ItemMerged { item_id, quantity },
                ));
            }
        }
        if self
            .tick
            .is_multiple_of(DROPPED_ITEM_CLEANUP_INTERVAL_TICKS)
        {
            // Removal is silent, the next mirror sync drops the ECS
            // entity, Lightyear ships the despawn to in-AoI clients, and
            // the visual goes away. Same lifecycle as pickups and merges.
            self.despawn_aging_dropped_items();
        }

        // Auto-save schedule (dedicated hosts; loopback leaves the interval 0
        // and saves on exit instead). We only announce + flag here; the host
        // drains `auto_save_pending`, writes the world, and announces that the
        // save finished, so disk I/O stays out of this game-state module.
        if self.auto_save_interval_ticks > 0 {
            let since_save = self.tick.saturating_sub(self.last_auto_save_tick);
            if self.auto_save_announce
                && since_save
                    == self
                        .auto_save_interval_ticks
                        .saturating_sub(AUTO_SAVE_WARNING_TICKS)
            {
                envelopes.extend(self.announce(
                    "Heads up: the world auto-saves in 30 seconds, expect a brief hitch.",
                ));
            }
            if since_save >= self.auto_save_interval_ticks {
                if self.auto_save_announce {
                    envelopes.extend(self.announce("Auto-saving the world…"));
                }
                self.auto_save_pending = true;
                self.last_auto_save_tick = self.tick;
            }
        }

        if self
            .tick
            .saturating_sub(self.last_world_time_broadcast_tick)
            >= WORLD_TIME_BROADCAST_INTERVAL_TICKS
        {
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::WorldTime(self.world_time_snapshot()),
            });
            self.last_world_time_broadcast_tick = self.tick;
        }

        // Phase 6.6 deleted the per-tick `ServerMessage::Snapshot`
        // broadcast, every state consumer now reads from
        // Lightyear-replicated components. Perf stats stay on their
        // own slow broadcast tick.
        if self
            .tick
            .is_multiple_of(PERF_STATS_BROADCAST_INTERVAL_TICKS)
        {
            let client_ids = self.clients.keys().copied().collect::<Vec<_>>();
            for client_id in client_ids {
                envelopes.push(ServerEnvelope {
                    target: DeliveryTarget::Client(client_id),
                    message: ServerMessage::PerfStats(self.perf_stats_for(client_id)),
                });
            }
        }

        // Roster broadcast: every connected player's name + reported ping.
        // Sent to everyone (AoI-independent) so the pause-screen list can show
        // the whole server, not just nearby mirrors.
        if self
            .tick
            .is_multiple_of(PLAYER_LIST_BROADCAST_INTERVAL_TICKS)
        {
            let entries: Vec<crate::protocol::PlayerListEntry> = self
                .clients
                .values()
                // Sleeping (logged-out) bodies aren't "online", so they stay
                // off the roster even though their body is still in the world.
                .filter(|client| client.online)
                .map(|client| crate::protocol::PlayerListEntry {
                    client_id: client.client_id,
                    name: client.name.clone(),
                    ping_ms: client.ping_ms,
                })
                .collect();
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::PlayerList(entries),
            });
        }
        envelopes
    }

    /// Build the perf-stats payload for one client, covers the player's
    /// own AoI count plus the world-wide chunk bookkeeping. The classification
    /// is sampled at the player's feet so the HUD shows the biome under them.
    fn perf_stats_for(&self, client_id: ClientId) -> crate::protocol::PerfStatsSnapshot {
        use crate::protocol::{PerfClassificationId, PerfStatsSnapshot};
        use crate::world::ChunkCoord;
        let (position, view_tier) = self
            .clients
            .get(&client_id)
            .map(|client| (client.controller.position, client.view_tier))
            .unwrap_or((Vec3Net::ZERO, crate::protocol::ViewRadiusTier::default()));
        let coord = ChunkCoord::from_world(position.x, position.z);
        let classification = self
            .chunk_manager
            .classification_at(position)
            .map(|c| match c {
                crate::world::ChunkClassification::Forest => PerfClassificationId::Forest,
                crate::world::ChunkClassification::RockyOutcrop => {
                    PerfClassificationId::RockyOutcrop
                }
                crate::world::ChunkClassification::OreVein => PerfClassificationId::OreVein,
                crate::world::ChunkClassification::Plains => PerfClassificationId::Plains,
                crate::world::ChunkClassification::Mixed => PerfClassificationId::Mixed,
            })
            .unwrap_or(PerfClassificationId::None);
        let aoi_visible_nodes = self.chunk_manager.visible_node_count(position, view_tier) as u32;
        PerfStatsSnapshot {
            loaded_chunks: self.chunk_manager.loaded_chunk_count() as u32,
            live_nodes: self.chunk_manager.live_node_count() as u32,
            pending_regrows: self.chunk_manager.pending_regrow_count() as u32,
            aoi_visible_nodes,
            player_chunk_x: coord.x,
            player_chunk_z: coord.z,
            player_classification: classification,
        }
    }

    fn expire_chat_bubbles(&mut self) {
        let tick = self.tick;
        for client in self.clients.values_mut() {
            if let Some(bubble) = &client.chat_bubble
                && bubble.expires_tick <= tick
            {
                client.chat_bubble = None;
            }
        }
    }
}
