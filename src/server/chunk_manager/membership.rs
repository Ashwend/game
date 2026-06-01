//! Entity-membership tracking: which chunk each networked entity is
//! anchored to, plus the reverse lookups and per-coord queries the AoI
//! path reads. Entities live in their owning collections elsewhere; the
//! chunk manager only stores ids.

use super::ChunkManager;
use crate::{
    protocol::{ClientId, DeployedEntityId, DroppedItemId, ResourceNodeId, Vec3Net},
    world::{ChunkCoord, NodeKind},
};

impl ChunkManager {
    /// Convert a world position to the chunk coord that contains it,
    /// clamping into the loaded map. Used by every "where does this
    /// entity anchor?" call so the rule is in one place — anything that
    /// drifts outside the world's playable area still gets a legal home
    /// chunk to live in until it's despawned.
    fn anchor_chunk_for(&self, position: Vec3Net) -> ChunkCoord {
        let raw = ChunkCoord::from_world(position.x, position.z);
        if self.grids.contains_key(&raw) {
            return raw;
        }
        // Out-of-bounds: clamp to the nearest loaded ring so the entity
        // is still findable. We don't expect this in normal play but a
        // dropped item physics-launched past the perimeter shouldn't
        // disappear from snapshots silently.
        let half = self.dims.dims as i32 / 2;
        let max = half - (1 - self.dims.dims as i32 % 2);
        ChunkCoord::new(raw.x.clamp(-half, max), raw.z.clamp(-half, max))
    }

    /// Register a dropped item with the chunk containing its current
    /// position. Idempotent — calling twice with the same id is a no-op
    /// on the second call.
    pub fn track_dropped_item(&mut self, id: DroppedItemId, position: Vec3Net) {
        let coord = self.anchor_chunk_for(position);
        // If the item was previously tracked at a different chunk, move
        // it. This keeps the membership index consistent with whatever
        // anchor `update_dropped_item` last recorded.
        if let Some(&previous) = self.dropped_item_chunks.get(&id) {
            if previous == coord {
                return;
            }
            if let Some(grid) = self.grids.get_mut(&previous) {
                grid.dropped_items.remove(&id);
            }
        }
        if let Some(grid) = self.grids.get_mut(&coord) {
            grid.dropped_items.insert(id);
        }
        self.dropped_item_chunks.insert(id, coord);
    }

    /// Stop tracking a dropped item. Called on pickup, merge, despawn,
    /// or save shutdown. Safe to call for an unknown id.
    pub fn untrack_dropped_item(&mut self, id: DroppedItemId) {
        if let Some(coord) = self.dropped_item_chunks.remove(&id)
            && let Some(grid) = self.grids.get_mut(&coord)
        {
            grid.dropped_items.remove(&id);
        }
    }

    /// Re-anchor a dropped item that may have drifted across a chunk
    /// boundary under physics. Cheap when the chunk hasn't changed
    /// (one HashMap lookup + comparison) so it's fine to call once per
    /// item per physics step.
    pub fn update_dropped_item_chunk(&mut self, id: DroppedItemId, position: Vec3Net) {
        let new_coord = self.anchor_chunk_for(position);
        match self.dropped_item_chunks.get(&id).copied() {
            Some(old_coord) if old_coord == new_coord => {}
            Some(old_coord) => {
                if let Some(grid) = self.grids.get_mut(&old_coord) {
                    grid.dropped_items.remove(&id);
                }
                if let Some(grid) = self.grids.get_mut(&new_coord) {
                    grid.dropped_items.insert(id);
                }
                self.dropped_item_chunks.insert(id, new_coord);
            }
            None => {
                // Not previously tracked — fall through to a fresh
                // registration so a missed `track_dropped_item` call
                // can't permanently orphan the item.
                self.track_dropped_item(id, position);
            }
        }
    }

    /// Register a connected client at the chunk containing their current
    /// position. Called from connect.
    pub fn track_player(&mut self, id: ClientId, position: Vec3Net) {
        let coord = self.anchor_chunk_for(position);
        if let Some(&previous) = self.player_chunks.get(&id) {
            if previous == coord {
                return;
            }
            if let Some(grid) = self.grids.get_mut(&previous) {
                grid.players.remove(&id);
            }
        }
        if let Some(grid) = self.grids.get_mut(&coord) {
            grid.players.insert(id);
        }
        self.player_chunks.insert(id, coord);
    }

    /// Stop tracking a client (disconnect / kick).
    pub fn untrack_player(&mut self, id: ClientId) {
        if let Some(coord) = self.player_chunks.remove(&id)
            && let Some(grid) = self.grids.get_mut(&coord)
        {
            grid.players.remove(&id);
        }
    }

    /// Update a player's chunk anchor from an accepted movement update.
    /// Called once per accepted `PlayerMovement`, which is the same path
    /// the snapshot reads `controller.position` from.
    pub fn update_player_chunk(&mut self, id: ClientId, position: Vec3Net) {
        let new_coord = self.anchor_chunk_for(position);
        match self.player_chunks.get(&id).copied() {
            Some(old_coord) if old_coord == new_coord => {}
            Some(old_coord) => {
                if let Some(grid) = self.grids.get_mut(&old_coord) {
                    grid.players.remove(&id);
                }
                if let Some(grid) = self.grids.get_mut(&new_coord) {
                    grid.players.insert(id);
                }
                self.player_chunks.insert(id, new_coord);
            }
            None => {
                self.track_player(id, position);
            }
        }
    }

    /// Return the chunk currently anchoring a given player. Used by the
    /// snapshot path so it doesn't have to recompute the chunk from
    /// world position when the manager already knows.
    pub fn player_chunk(&self, id: ClientId) -> Option<ChunkCoord> {
        self.player_chunks.get(&id).copied()
    }

    /// Snapshot of the chunk coordinates anchoring at least one entity
    /// or player. Useful for perf overlays; production callers should
    /// prefer `visible_chunks` so the player's AoI ring is the gate.
    pub fn loaded_coords(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.grids.keys().copied()
    }

    /// Reverse lookup: which chunk does this node id live in? Returns
    /// `None` if the node id is unknown (already depleted, or never
    /// existed). Used by the ECS mirror system to attach the right
    /// chunk component when a fresh entity is spawned.
    pub fn node_chunk(&self, id: ResourceNodeId) -> Option<ChunkCoord> {
        self.node_chunks.get(&id).map(|(coord, _)| *coord)
    }

    /// Reverse lookup for dropped items. Returns `None` if the item id is
    /// not currently tracked (e.g. just picked up, just despawned).
    pub fn dropped_item_chunk(&self, id: DroppedItemId) -> Option<ChunkCoord> {
        self.dropped_item_chunks.get(&id).copied()
    }

    /// Reverse lookup for placed structures.
    pub fn deployed_entity_chunk(&self, id: DeployedEntityId) -> Option<ChunkCoord> {
        self.deployed_entity_chunks.get(&id).copied()
    }

    /// Live resource node ids anchored to `coord`. Empty iterator for
    /// unloaded chunks. Cheap — backed by the per-chunk live set.
    pub fn nodes_in(&self, coord: ChunkCoord) -> impl Iterator<Item = ResourceNodeId> + '_ {
        self.grids.get(&coord).into_iter().flat_map(|grid| {
            grid.live_by_kind
                .values()
                .flat_map(|set| set.iter().copied())
        })
    }

    /// Dropped item ids anchored to `coord`.
    pub fn dropped_items_in(&self, coord: ChunkCoord) -> impl Iterator<Item = DroppedItemId> + '_ {
        self.grids
            .get(&coord)
            .into_iter()
            .flat_map(|grid| grid.dropped_items.iter().copied())
    }

    /// Connected client ids anchored to `coord`.
    pub fn players_in(&self, coord: ChunkCoord) -> impl Iterator<Item = ClientId> + '_ {
        self.grids
            .get(&coord)
            .into_iter()
            .flat_map(|grid| grid.players.iter().copied())
    }

    /// Placed-structure ids anchored to `coord`.
    pub fn deployed_entities_in(
        &self,
        coord: ChunkCoord,
    ) -> impl Iterator<Item = DeployedEntityId> + '_ {
        self.grids
            .get(&coord)
            .into_iter()
            .flat_map(|grid| grid.deployed_entities.iter().copied())
    }

    /// Register a freshly-placed structure at the chunk containing its
    /// world position. Idempotent — repeat calls with the same id move
    /// the membership rather than duplicating it.
    pub fn track_deployed_entity(&mut self, id: DeployedEntityId, position: Vec3Net) {
        let coord = self.anchor_chunk_for(position);
        if let Some(&previous) = self.deployed_entity_chunks.get(&id) {
            if previous == coord {
                return;
            }
            if let Some(grid) = self.grids.get_mut(&previous) {
                grid.deployed_entities.remove(&id);
            }
        }
        if let Some(grid) = self.grids.get_mut(&coord) {
            grid.deployed_entities.insert(id);
        }
        self.deployed_entity_chunks.insert(id, coord);
    }

    /// Stop tracking a placed structure (destroyed, save shutdown).
    pub fn untrack_deployed_entity(&mut self, id: DeployedEntityId) {
        if let Some(coord) = self.deployed_entity_chunks.remove(&id)
            && let Some(grid) = self.grids.get_mut(&coord)
        {
            grid.deployed_entities.remove(&id);
        }
    }

    /// Register a freshly-spawned loot bag at its anchor chunk. Bags
    /// don't move, so this is called once at spawn time. Lightyear
    /// room replication handles visibility from there.
    pub fn track_loot_bag(&mut self, id: crate::protocol::LootBagId, position: Vec3Net) {
        let coord = self.anchor_chunk_for(position);
        self.loot_bag_chunks.insert(id, coord);
    }

    /// Stop tracking a loot bag (despawn).
    pub fn untrack_loot_bag(&mut self, id: crate::protocol::LootBagId) {
        self.loot_bag_chunks.remove(&id);
    }

    /// Look up the chunk a loot bag is anchored to.
    pub fn loot_bag_chunk(&self, id: crate::protocol::LootBagId) -> Option<ChunkCoord> {
        self.loot_bag_chunks.get(&id).copied()
    }

    /// Register an externally-spawned resource node (admin command, etc.)
    /// so it appears in the AoI snapshot for clients in range. Without
    /// this, a `/spawn-ore` node would exist in `resource_nodes` but be
    /// invisible in every snapshot because the per-chunk membership set
    /// is the AoI source of truth.
    pub fn track_resource_node(
        &mut self,
        node_id: ResourceNodeId,
        kind: NodeKind,
        position: Vec3Net,
    ) {
        let coord = self.anchor_chunk_for(position);
        if let Some(grid) = self.grids.get_mut(&coord) {
            grid.record_live(kind, node_id);
        }
        self.node_chunks.insert(node_id, (coord, kind));
    }
}
