//! AoI / visibility math: given a player position and view tier, which
//! chunks (and per-coord entity ids) the player should see. The ring math
//! is centralized here so every networked entity flows through the same
//! chunk-visibility decision.

use std::collections::HashSet;

use super::{ChunkManager, KEEP_MARGIN_RINGS, LOAD_BUFFER_RINGS, view_tier_radius};
use crate::{
    protocol::{
        ClientId, DeployedEntityId, DroppedItemId, ResourceNodeId, Vec3Net, ViewRadiusTier,
    },
    world::{ChunkClassification, ChunkCoord},
};

impl ChunkManager {
    /// The chunk coords a player at `player_pos` should receive
    /// snapshot data for under the given view tier, the **add** radius for
    /// room subscriptions. Centralizes the AoI ring math so every networked
    /// entity flows through the same chunk-visibility decision. Includes the
    /// load-buffer ring so the client's collider grid is already populated
    /// when the player crosses a boundary, see `LOAD_BUFFER_RINGS`.
    pub fn visible_chunks(&self, player_pos: Vec3Net, tier: ViewRadiusTier) -> HashSet<ChunkCoord> {
        self.chunks_within(player_pos, view_tier_radius(tier) + LOAD_BUFFER_RINGS)
    }

    /// The **keep** radius for room subscriptions: the load radius plus
    /// `KEEP_MARGIN_RINGS`. A subscribed chunk is retained until it falls
    /// outside this larger ring, which is the spatial hysteresis that stops
    /// boundary thrash. Always a superset of `visible_chunks`.
    pub fn retained_chunks(
        &self,
        player_pos: Vec3Net,
        tier: ViewRadiusTier,
    ) -> HashSet<ChunkCoord> {
        self.chunks_within(
            player_pos,
            view_tier_radius(tier) + LOAD_BUFFER_RINGS + KEEP_MARGIN_RINGS,
        )
    }

    /// Loaded chunks within a Chebyshev `radius` (in chunks) of the player.
    fn chunks_within(&self, player_pos: Vec3Net, radius: u32) -> HashSet<ChunkCoord> {
        let radius = radius as i32;
        let player_grid = ChunkCoord::from_world(player_pos.x, player_pos.z);
        let mut out = HashSet::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let coord = ChunkCoord::new(player_grid.x + dx, player_grid.z + dz);
                if self.grids.contains_key(&coord) {
                    out.insert(coord);
                }
            }
        }
        out
    }

    /// Node ids visible to a player at `player_pos` under the given view
    /// tier. Thin wrapper around `visible_chunks` + `nodes_in` so the
    /// AoI ring math stays in one place.
    pub fn nodes_visible_to(
        &self,
        player_pos: Vec3Net,
        tier: ViewRadiusTier,
    ) -> HashSet<ResourceNodeId> {
        self.visible_chunks(player_pos, tier)
            .into_iter()
            .flat_map(|coord| self.nodes_in(coord))
            .collect()
    }

    /// Count of nodes visible to a player without materialising the id
    /// set. `nodes_visible_to` collects ~1800 ids into a fresh `HashSet`
    /// at AoI scale; the per-client perf-stats broadcast only needs the
    /// number. Each node is anchored to exactly one chunk, so summing
    /// per-chunk counts equals the deduplicated set size.
    pub fn visible_node_count(&self, player_pos: Vec3Net, tier: ViewRadiusTier) -> usize {
        self.visible_chunks(player_pos, tier)
            .into_iter()
            .map(|coord| self.nodes_in(coord).count())
            .sum()
    }

    /// Dropped item ids visible to a player at `player_pos` under the
    /// given view tier.
    pub fn dropped_items_visible_to(
        &self,
        player_pos: Vec3Net,
        tier: ViewRadiusTier,
    ) -> HashSet<DroppedItemId> {
        self.visible_chunks(player_pos, tier)
            .into_iter()
            .flat_map(|coord| self.dropped_items_in(coord))
            .collect()
    }

    /// Client ids visible to a player at `player_pos` under the given
    /// view tier. The caller decides whether to additionally include
    /// the player themselves.
    pub fn players_visible_to(
        &self,
        player_pos: Vec3Net,
        tier: ViewRadiusTier,
    ) -> HashSet<ClientId> {
        self.visible_chunks(player_pos, tier)
            .into_iter()
            .flat_map(|coord| self.players_in(coord))
            .collect()
    }

    /// Placed-structure ids visible to a player at `player_pos`.
    pub fn deployed_entities_visible_to(
        &self,
        player_pos: Vec3Net,
        tier: ViewRadiusTier,
    ) -> HashSet<DeployedEntityId> {
        self.visible_chunks(player_pos, tier)
            .into_iter()
            .flat_map(|coord| self.deployed_entities_in(coord))
            .collect()
    }

    /// World-space classification under the player's feet. Used by the
    /// perf HUD so the player can see which biome they're standing in.
    pub fn classification_at(&self, position: Vec3Net) -> Option<ChunkClassification> {
        let coord = ChunkCoord::from_world(position.x, position.z);
        self.grids.get(&coord).map(|g| g.classification)
    }
}
