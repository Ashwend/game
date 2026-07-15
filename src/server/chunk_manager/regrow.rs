//! Regrow scheduling and placement: when a node depletes, schedule a
//! fresh spawn 5–15 min later (jittered) at a noise-valid position in the
//! same grid, up to the chunk's capacity ceiling, then fire it on tick.

use std::collections::HashMap;

use super::{ChunkManager, MAX_REGROW_TICKS, MIN_REGROW_TICKS, RegrowEvent};
use crate::{
    protocol::{ResourceNodeId, ResourceNodeState},
    resource_nodes::spawn_resource_node,
    world::{ChunkCoord, ChunkSpawn, NodeKind, PlayableBounds, generate_chunk_spawns, splitmix64},
};

/// What [`ChunkManager::tick`] returns: every node it spawned this tick
/// so the server can splice them into the live node map and the
/// snapshot path picks them up automatically.
pub struct RegrowResult {
    pub spawned: Vec<ResourceNodeState>,
}

impl ChunkManager {
    /// Called from the gather path when a node has been depleted and
    /// removed from the server's live map. Schedules a fresh respawn
    /// 5–15 min later (jittered), unless the chunk is already at cap for
    /// this kind (e.g. an admin-placed extra node was harvested).
    pub fn handle_node_depleted(&mut self, node_id: ResourceNodeId, now_tick: u64) {
        let Some((coord, kind)) = self.node_chunks.remove(&node_id) else {
            return;
        };
        if let Some(grid) = self.grids.get_mut(&coord) {
            grid.remove_live(kind, node_id);
        }
        // Pick a per-event delay deterministically, same coord+kind+tick
        // round-trips identically on save+load.
        self.placement_counter = splitmix64(self.placement_counter ^ now_tick ^ node_id.0);
        let span = MAX_REGROW_TICKS.saturating_sub(MIN_REGROW_TICKS).max(1);
        let jitter = self.placement_counter % span;
        let fire_tick = now_tick
            .saturating_add(MIN_REGROW_TICKS)
            .saturating_add(jitter);
        self.regrow_queue.push(RegrowEvent {
            fire_tick,
            coord,
            kind,
        });
    }

    /// Drain any regrow events whose `fire_tick` has arrived, place a
    /// fresh node for each, and return the new node states so the server
    /// can splice them into its live map.
    ///
    /// `existing_positions` is consulted so newly-placed nodes don't land
    /// on top of unrelated nodes that survived in the same grid.
    pub fn tick(
        &mut self,
        now_tick: u64,
        existing_positions: &HashMap<ResourceNodeId, ResourceNodeState>,
    ) -> RegrowResult {
        let mut spawned = Vec::new();
        while let Some(top) = self.regrow_queue.peek().copied() {
            if top.fire_tick > now_tick {
                break;
            }
            self.regrow_queue.pop();
            if let Some(state) = self.place_fresh_node(top.coord, top.kind, existing_positions) {
                spawned.push(state);
            }
        }
        RegrowResult { spawned }
    }

    /// Find an open position inside `coord` for `kind`, build the
    /// resource state, and bookkeep it in the per-chunk live set. Returns
    /// `None` if the chunk is full for this kind or no candidate fit
    /// inside the candidate budget, better to drop a respawn than to
    /// jam a node into an occupied square.
    fn place_fresh_node(
        &mut self,
        coord: ChunkCoord,
        kind: NodeKind,
        existing_positions: &HashMap<ResourceNodeId, ResourceNodeState>,
    ) -> Option<ResourceNodeState> {
        let grid = self.grids.get(&coord)?;
        let cap = grid.capacity.get(&kind).copied().unwrap_or(0);
        if cap == 0 || grid.live_count(kind) >= cap {
            return None;
        }

        // Use a one-grid regenerator with a salted next_id slot so the
        // placement is deterministic for the same `(seed, coord, count)`
        // tuple. We reuse the generator's placement code (Poisson-disk
        // rejection + per-kind noise mask) to make new nodes feel
        // organically placed, then pick the first candidate that doesn't
        // collide with the surviving live nodes.
        let bounds = PlayableBounds::from_dims(self.dims);
        let candidates = candidate_positions(
            self.world_seed,
            coord,
            kind,
            self.placement_counter,
            bounds,
            &self.ruin_footprints,
        );
        self.placement_counter = splitmix64(self.placement_counter ^ 0xA5A5_5A5A);

        for spawn in candidates {
            if collides_with_existing(&spawn, existing_positions) {
                continue;
            }
            let id = self.next_node_id;
            self.next_node_id = self.next_node_id.saturating_add(1);
            let world_spawn = crate::world::WorldResourceNodeSpawn::new(
                crate::protocol::ResourceNodeId(id),
                spawn.spawn.definition_id.clone(),
                spawn.spawn.position,
                spawn.spawn.yaw,
            );
            let Some(state) = spawn_resource_node(&world_spawn, Some(self.world_seed)) else {
                continue;
            };
            if let Some(grid) = self.grids.get_mut(&coord) {
                grid.record_live(kind, crate::protocol::ResourceNodeId(id));
            }
            self.node_chunks
                .insert(crate::protocol::ResourceNodeId(id), (coord, kind));
            return Some(state);
        }
        None
    }
}

/// Reusable helper: produce a sequence of candidate spawn positions for
/// `(coord, kind)` using the same Poisson-disk noise mask the generator
/// uses, salted by a counter so repeated calls get fresh candidates.
fn candidate_positions(
    world_seed: u64,
    coord: ChunkCoord,
    kind: NodeKind,
    salt: u64,
    bounds: PlayableBounds,
    ruin_footprints: &[crate::world::RuinFootprint],
) -> Vec<ChunkSpawn> {
    // We just want a few candidates, not the full target capacity, the
    // caller filters by collision and picks the first survivor. Re-run
    // the per-chunk generator with a salted seed so we're not handing
    // out the same point set every time. The bounds get forwarded so a
    // regrow can't land past the perimeter wall any more than the
    // initial pass can, and the ruin footprints so a regrow can't drop a
    // node inside a ruin the initial pass kept clear.
    let salted_seed = splitmix64(world_seed ^ salt);
    let mut next_id: u64 = 1;
    generate_chunk_spawns(salted_seed, coord, &mut next_id, bounds, ruin_footprints)
        .into_iter()
        .filter(|spawn| spawn.kind == kind)
        .collect()
}

fn collides_with_existing(
    candidate: &ChunkSpawn,
    existing: &HashMap<ResourceNodeId, ResourceNodeState>,
) -> bool {
    const MIN_DISTANCE_M: f32 = 1.2;
    let min_sq = MIN_DISTANCE_M * MIN_DISTANCE_M;
    let pos = candidate.spawn.position;
    existing.values().any(|state| {
        let dx = state.position.x - pos.x;
        let dz = state.position.z - pos.z;
        dx * dx + dz * dz < min_sq
    })
}
