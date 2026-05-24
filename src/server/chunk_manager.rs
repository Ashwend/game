//! Server-side owner of the chunk system.
//!
//! Responsibilities, all server-authoritative:
//!
//! 1. **Seeded generation** — builds the initial node spawn list for a
//!    world from `(world_seed, dims)` by calling the pure chunk generator.
//! 2. **Identity tracking** — remembers which grid + kind every live
//!    node belongs to so the regrow scheduler can fire fresh-position
//!    replacements after a node is depleted.
//! 3. **Regrow scheduling** — when a node is depleted, schedules a fresh
//!    spawn 5–15 min later (jittered) at a noise-valid position in the
//!    same grid, up to the chunk's capacity ceiling.
//! 4. **AoI streaming** — given a player position and view radius tier,
//!    returns the set of node IDs the player should see. The server's
//!    snapshot builder uses this to filter `WorldSnapshot.resource_nodes`
//!    per-player, so the wire only carries nodes inside the player's
//!    loaded ring.
//! 5. **Persistence** — serializes the per-chunk live counts and pending
//!    regrow events into [`ChunkManagerSave`], which the save layer
//!    embeds in `WorldStateSave`. Reload reconstitutes the manager from
//!    the seed + saved state.
//!
//! The pure generation lives in `crate::world::chunk::*`; this module
//! holds the mutation and lifecycle.

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap, HashSet},
};

use serde::{Deserialize, Serialize};

use crate::{
    protocol::{ResourceNodeId, ResourceNodeState, Vec3Net, ViewRadiusTier},
    resources::spawn_resource_node,
    world::{
        ChunkClassification, ChunkCoord, ChunkDims, ChunkSpawn, ClassificationChannels, NodeKind,
        PlayableBounds, base_capacity, generate_chunk_spawns, generate_world_spawns, splitmix64,
    },
};

/// Minimum delay before a depleted node respawns. Tied to a 20 Hz tick
/// rate — keeps the player from camping a single grid for free yield.
const MIN_REGROW_TICKS: u64 = 5 * 60 * crate::protocol::SERVER_TICK_RATE_HZ as u64;
/// Maximum delay. Together with the floor, gives the regrow window the
/// 5–15 min spec the design called for.
const MAX_REGROW_TICKS: u64 = 15 * 60 * crate::protocol::SERVER_TICK_RATE_HZ as u64;

/// AoI ring sizes, indexed by [`ViewRadiusTier`]. The server walks the
/// Chebyshev neighbourhood of the player's chunk up to this radius and
/// includes every node inside that loaded ring in the player's snapshot.
const VIEW_RADIUS_LOW: u32 = 1;
const VIEW_RADIUS_MEDIUM: u32 = 2;
const VIEW_RADIUS_HIGH: u32 = 3;

/// Extra ring loaded beyond the player's view tier, used purely as a
/// collider-stability buffer. Without it, the moment the player crosses
/// a chunk boundary the snapshot's resource-node set changes and the
/// client rebuilds its collision grid — freshly-loaded tree/ore
/// colliders can be placed as close as `EDGE_MARGIN_M` (0.5 m) from the
/// boundary, while the player's per-tick movement past the boundary is
/// only ~0.25 m at walking speed. That gap is smaller than the combined
/// player + tree hitbox width, so the next prediction step shoves the
/// player upward to resolve the overlap (visible as a vertical spasm).
///
/// Loading one extra ring keeps the newly-loaded chunks at least one
/// full cell (64 m) away from the player at the moment of crossing, so
/// any added collider is well outside any plausible collision radius.
/// Costs an extra ring of node bandwidth per snapshot in exchange for
/// jitter-free traversal.
const LOAD_BUFFER_RINGS: u32 = 1;

/// Outer-ring spawn-budget multipliers. When the chunk manager is first
/// populating the world, grids further from the centre keep a smaller
/// fraction of their generated capacity, so distant areas still read as
/// populated without paying the full per-node cost. This is the
/// "density falloff" the design called for, implemented as a fixed
/// spawn-budget instead of a moving culling window — players moving
/// around the world don't see neighbouring grids fade in/out, only
/// brand-new grids respect the budget.
const RING_BUDGET: [f32; 5] = [1.0, 0.85, 0.65, 0.45, 0.30];

/// Server-side resolution of [`ViewRadiusTier`] to Chebyshev grid radius.
/// Defined here rather than on the protocol enum so the wire type doesn't
/// have to know the server's ring-count tuning.
pub fn view_tier_radius(tier: ViewRadiusTier) -> u32 {
    match tier {
        ViewRadiusTier::Low => VIEW_RADIUS_LOW,
        ViewRadiusTier::Medium => VIEW_RADIUS_MEDIUM,
        ViewRadiusTier::High => VIEW_RADIUS_HIGH,
    }
}

/// Per-grid state carried in memory while the server is running. Most of
/// it is derived from the seed and live nodes, but the live node set
/// itself needs explicit tracking since players harvest it.
#[derive(Debug, Default, Clone)]
struct ActiveChunkState {
    classification: ChunkClassification,
    /// Per-kind cap derived from classification + channel intensity at
    /// generation time. Stored verbatim so regrows respect the same cap
    /// the initial pass used.
    capacity: HashMap<NodeKind, u16>,
    /// Live node ids inside this chunk, grouped by kind. Lets the
    /// scheduler check "is this kind already at cap?" in O(1).
    live_by_kind: HashMap<NodeKind, HashSet<ResourceNodeId>>,
}

impl ActiveChunkState {
    fn live_count(&self, kind: NodeKind) -> u16 {
        self.live_by_kind
            .get(&kind)
            .map(|set| set.len() as u16)
            .unwrap_or(0)
    }

    fn record_live(&mut self, kind: NodeKind, node_id: ResourceNodeId) {
        self.live_by_kind.entry(kind).or_default().insert(node_id);
    }

    fn remove_live(&mut self, kind: NodeKind, node_id: ResourceNodeId) {
        if let Some(set) = self.live_by_kind.get_mut(&kind) {
            set.remove(&node_id);
        }
    }
}

/// Pending fresh-position respawn, scheduled when a node depletes and
/// fired by [`ChunkManager::tick`] when its `fire_tick` arrives. Carries
/// the kind + chunk coord so the manager can pick a fresh placement when
/// the time comes.
#[derive(Debug, Clone, Copy)]
struct RegrowEvent {
    fire_tick: u64,
    coord: ChunkCoord,
    kind: NodeKind,
}

impl PartialEq for RegrowEvent {
    fn eq(&self, other: &Self) -> bool {
        self.fire_tick == other.fire_tick
    }
}
impl Eq for RegrowEvent {}
impl PartialOrd for RegrowEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for RegrowEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; flip the order so the earliest
        // `fire_tick` ends up at the top.
        other.fire_tick.cmp(&self.fire_tick)
    }
}

/// What [`ChunkManager::tick`] returns: every node it spawned this tick
/// so the server can splice them into the live node map and the
/// snapshot path picks them up automatically.
pub struct RegrowResult {
    pub spawned: Vec<ResourceNodeState>,
}

/// Serializable summary of chunk manager state. Embedded in
/// `WorldStateSave` so reload picks up the seed, pending regrows, and
/// per-chunk identity bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkManagerSave {
    pub world_seed: u64,
    pub dims: u32,
    pub next_node_id: ResourceNodeId,
    /// `node_id → (coord, kind)` so reload can rebuild the per-chunk
    /// live-node sets without re-running the placement RNG.
    pub node_chunks: Vec<NodeChunkEntry>,
    /// `(coord, kind, ticks_from_now)` for every scheduled regrow. The
    /// "from now" framing means a save that sits on disk for an hour
    /// doesn't dump a backlog of respawns at t+0 on load — each event
    /// re-clamps to at least [`MIN_REGROW_TICKS`].
    pub pending_regrows: Vec<PendingRegrowSave>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeChunkEntry {
    pub node_id: ResourceNodeId,
    pub coord: ChunkCoord,
    pub kind: SerializedNodeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingRegrowSave {
    pub coord: ChunkCoord,
    pub kind: SerializedNodeKind,
    pub ticks_from_now: u64,
}

/// Wire-friendly serialized form of [`NodeKind`]. The in-memory enum is
/// declared in `crate::world` and not annotated with serde for cleanliness;
/// the conversion lives here so the save module never imports `serde` for
/// gameplay enums.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SerializedNodeKind {
    TreeSmall,
    TreeMedium,
    TreeLarge,
    SurfaceStone,
    BranchPile,
    HayGrass,
    CoalOre,
    IronOre,
    SulfurOre,
    StoneVein,
}

impl From<NodeKind> for SerializedNodeKind {
    fn from(kind: NodeKind) -> Self {
        match kind {
            NodeKind::TreeSmall => Self::TreeSmall,
            NodeKind::TreeMedium => Self::TreeMedium,
            NodeKind::TreeLarge => Self::TreeLarge,
            NodeKind::SurfaceStone => Self::SurfaceStone,
            NodeKind::BranchPile => Self::BranchPile,
            NodeKind::HayGrass => Self::HayGrass,
            NodeKind::CoalOre => Self::CoalOre,
            NodeKind::IronOre => Self::IronOre,
            NodeKind::SulfurOre => Self::SulfurOre,
            NodeKind::StoneVein => Self::StoneVein,
        }
    }
}

impl From<SerializedNodeKind> for NodeKind {
    fn from(kind: SerializedNodeKind) -> Self {
        match kind {
            SerializedNodeKind::TreeSmall => Self::TreeSmall,
            SerializedNodeKind::TreeMedium => Self::TreeMedium,
            SerializedNodeKind::TreeLarge => Self::TreeLarge,
            SerializedNodeKind::SurfaceStone => Self::SurfaceStone,
            SerializedNodeKind::BranchPile => Self::BranchPile,
            SerializedNodeKind::HayGrass => Self::HayGrass,
            SerializedNodeKind::CoalOre => Self::CoalOre,
            SerializedNodeKind::IronOre => Self::IronOre,
            SerializedNodeKind::SulfurOre => Self::SulfurOre,
            SerializedNodeKind::StoneVein => Self::StoneVein,
        }
    }
}

#[derive(Debug)]
pub struct ChunkManager {
    world_seed: u64,
    dims: ChunkDims,
    grids: HashMap<ChunkCoord, ActiveChunkState>,
    node_chunks: HashMap<ResourceNodeId, (ChunkCoord, NodeKind)>,
    regrow_queue: BinaryHeap<RegrowEvent>,
    next_node_id: u64,
    /// Stir-in counter for regrow RNG so identical re-scheduling on the
    /// same tick doesn't pick identical fresh positions.
    placement_counter: u64,
}

impl ChunkManager {
    /// Build a fresh chunk manager for a brand-new world. Returns the
    /// manager along with the initial node spawn list; the server inserts
    /// those into its `resource_nodes` map as usual.
    pub fn new_for_world(world_seed: u64, dims: ChunkDims) -> (Self, Vec<ResourceNodeState>) {
        let mut spawns = generate_world_spawns(world_seed, dims);
        // Trim outer rings to the spawn-budget table — strips out a
        // deterministic suffix of spawns per (coord, kind) so the world's
        // outer rings sit at the budgeted density without us having to
        // re-run the Poisson sampler with a target multiplier.
        apply_ring_budget(&mut spawns);

        let mut grids: HashMap<ChunkCoord, ActiveChunkState> = HashMap::new();
        let mut node_chunks: HashMap<ResourceNodeId, (ChunkCoord, NodeKind)> = HashMap::new();
        let mut next_node_id: u64 = 1;
        let mut live_states: Vec<ResourceNodeState> = Vec::with_capacity(spawns.len());

        // Pre-seed the per-chunk capacity table from the classification
        // sampler so regrows respect the same cap the initial pass used.
        for coord in dims.coords() {
            let channels = ClassificationChannels::sample(world_seed, coord);
            let classification = channels.classify();
            let mut capacity = HashMap::new();
            for kind in NodeKind::ALL {
                // Match the generator's scaling so the cap is the same
                // count the world was generated at: round(base × (0.55 + ch × 0.7)).
                let base = base_capacity(classification, kind) as f32;
                let channel = channels.channel_for(kind);
                let target = (base * (0.55 + channel * 0.7)).round() as u16;
                if target > 0 {
                    capacity.insert(kind, target);
                }
            }
            grids.insert(
                coord,
                ActiveChunkState {
                    classification,
                    capacity,
                    live_by_kind: HashMap::new(),
                },
            );
        }

        for chunk_spawn in spawns {
            let Some(node) = spawn_resource_node(&chunk_spawn.spawn) else {
                continue;
            };
            next_node_id = next_node_id.max(chunk_spawn.spawn.id + 1);
            if let Some(grid) = grids.get_mut(&chunk_spawn.coord) {
                grid.record_live(chunk_spawn.kind, node.id);
            }
            node_chunks.insert(node.id, (chunk_spawn.coord, chunk_spawn.kind));
            live_states.push(node);
        }

        (
            Self {
                world_seed,
                dims,
                grids,
                node_chunks,
                regrow_queue: BinaryHeap::new(),
                next_node_id,
                placement_counter: splitmix64(world_seed ^ 0x00C0_FFEE_BABE),
            },
            live_states,
        )
    }

    /// Rebuild a chunk manager from a saved snapshot. The capacity table
    /// is recomputed from the seed (deterministic), and the saved node
    /// → grid map is replayed so live state matches whatever survived
    /// to the save.
    pub fn from_save(save: ChunkManagerSave, now_tick: u64) -> Self {
        let dims = ChunkDims::new(save.dims);
        let mut grids: HashMap<ChunkCoord, ActiveChunkState> = HashMap::new();
        for coord in dims.coords() {
            let channels = ClassificationChannels::sample(save.world_seed, coord);
            let classification = channels.classify();
            let mut capacity = HashMap::new();
            for kind in NodeKind::ALL {
                let base = base_capacity(classification, kind) as f32;
                let channel = channels.channel_for(kind);
                let target = (base * (0.55 + channel * 0.7)).round() as u16;
                if target > 0 {
                    capacity.insert(kind, target);
                }
            }
            grids.insert(
                coord,
                ActiveChunkState {
                    classification,
                    capacity,
                    live_by_kind: HashMap::new(),
                },
            );
        }

        let mut node_chunks: HashMap<ResourceNodeId, (ChunkCoord, NodeKind)> = HashMap::new();
        for entry in save.node_chunks {
            let kind: NodeKind = entry.kind.into();
            node_chunks.insert(entry.node_id, (entry.coord, kind));
            if let Some(grid) = grids.get_mut(&entry.coord) {
                grid.record_live(kind, entry.node_id);
            }
        }

        let mut regrow_queue = BinaryHeap::new();
        for pending in save.pending_regrows {
            let fire_tick = now_tick.saturating_add(pending.ticks_from_now.max(MIN_REGROW_TICKS));
            regrow_queue.push(RegrowEvent {
                fire_tick,
                coord: pending.coord,
                kind: pending.kind.into(),
            });
        }

        Self {
            world_seed: save.world_seed,
            dims,
            grids,
            node_chunks,
            regrow_queue,
            next_node_id: save.next_node_id.max(1),
            placement_counter: splitmix64(
                save.world_seed ^ now_tick.wrapping_mul(0x00C0_FFEE_BABE),
            ),
        }
    }

    /// Wire-friendly snapshot of chunk manager state for the save file.
    pub fn to_save(&self, now_tick: u64) -> ChunkManagerSave {
        let node_chunks = self
            .node_chunks
            .iter()
            .map(|(&node_id, &(coord, kind))| NodeChunkEntry {
                node_id,
                coord,
                kind: kind.into(),
            })
            .collect();
        let pending_regrows = self
            .regrow_queue
            .iter()
            .map(|event| PendingRegrowSave {
                coord: event.coord,
                kind: event.kind.into(),
                // Save as "ticks from now" so the load path doesn't have
                // to know the original schedule's wall-clock time.
                ticks_from_now: event.fire_tick.saturating_sub(now_tick),
            })
            .collect();
        ChunkManagerSave {
            world_seed: self.world_seed,
            dims: self.dims.dims,
            next_node_id: self.next_node_id,
            node_chunks,
            pending_regrows,
        }
    }

    pub fn world_seed(&self) -> u64 {
        self.world_seed
    }

    pub fn dims(&self) -> ChunkDims {
        self.dims
    }

    pub fn next_node_id(&self) -> u64 {
        self.next_node_id
    }

    pub fn pending_regrow_count(&self) -> usize {
        self.regrow_queue.len()
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.grids.len()
    }

    pub fn classification(&self, coord: ChunkCoord) -> Option<ChunkClassification> {
        self.grids.get(&coord).map(|g| g.classification)
    }

    /// Total live node count across the world. Used by the perf HUD.
    pub fn live_node_count(&self) -> usize {
        self.node_chunks.len()
    }

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
        // Pick a per-event delay deterministically — same coord+kind+tick
        // round-trips identically on save+load.
        self.placement_counter = splitmix64(self.placement_counter ^ now_tick ^ node_id);
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
    /// inside the candidate budget — better to drop a respawn than to
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
        let candidates =
            candidate_positions(self.world_seed, coord, kind, self.placement_counter, bounds);
        self.placement_counter = splitmix64(self.placement_counter ^ 0xA5A5_5A5A);

        for spawn in candidates {
            if collides_with_existing(&spawn, existing_positions) {
                continue;
            }
            let id = self.next_node_id;
            self.next_node_id = self.next_node_id.saturating_add(1);
            let world_spawn = crate::world::WorldResourceNodeSpawn::new(
                id,
                spawn.spawn.definition_id.clone(),
                spawn.spawn.position,
                spawn.spawn.yaw,
            );
            let Some(state) = spawn_resource_node(&world_spawn) else {
                continue;
            };
            if let Some(grid) = self.grids.get_mut(&coord) {
                grid.record_live(kind, id);
            }
            self.node_chunks.insert(id, (coord, kind));
            return Some(state);
        }
        None
    }

    /// Returns the node IDs visible to a player at `player_pos` under
    /// the given view tier. Anything outside the loaded ring is omitted
    /// from the per-player snapshot.
    pub fn nodes_visible_to(
        &self,
        player_pos: Vec3Net,
        tier: ViewRadiusTier,
    ) -> HashSet<ResourceNodeId> {
        // Include the load-buffer ring so the client's collider grid is
        // already populated when the player crosses a boundary — see
        // `LOAD_BUFFER_RINGS` for the why.
        let radius = (view_tier_radius(tier) + LOAD_BUFFER_RINGS) as i32;
        let player_grid = ChunkCoord::from_world(player_pos.x, player_pos.z);
        let mut visible = HashSet::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let coord = ChunkCoord::new(player_grid.x + dx, player_grid.z + dz);
                let Some(grid) = self.grids.get(&coord) else {
                    continue;
                };
                for set in grid.live_by_kind.values() {
                    visible.extend(set.iter().copied());
                }
            }
        }
        visible
    }

    /// World-space classification under the player's feet. Used by the
    /// perf HUD so the player can see which biome they're standing in.
    pub fn classification_at(&self, position: Vec3Net) -> Option<ChunkClassification> {
        let coord = ChunkCoord::from_world(position.x, position.z);
        self.grids.get(&coord).map(|g| g.classification)
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
) -> Vec<ChunkSpawn> {
    // We just want a few candidates, not the full target capacity — the
    // caller filters by collision and picks the first survivor. Re-run
    // the per-chunk generator with a salted seed so we're not handing
    // out the same point set every time. The bounds get forwarded so a
    // regrow can't land past the perimeter wall any more than the
    // initial pass can.
    let salted_seed = splitmix64(world_seed ^ salt);
    let mut next_id: u64 = 1;
    generate_chunk_spawns(salted_seed, coord, &mut next_id, bounds)
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

/// Strip outer-ring spawns down to the budget multiplier in
/// [`RING_BUDGET`]. Deterministic: sorts by node id then keeps the first
/// `keep_n` entries per `(coord, kind)`. Saving the spawns themselves
/// would also work but adds save complexity for no gameplay win.
fn apply_ring_budget(spawns: &mut Vec<ChunkSpawn>) {
    // Group by (coord, kind) → list of indices into spawns. Walk groups,
    // compute keep_n from the ring distance, mark survivors.
    let mut groups: HashMap<(ChunkCoord, NodeKind), Vec<usize>> = HashMap::new();
    for (idx, spawn) in spawns.iter().enumerate() {
        groups
            .entry((spawn.coord, spawn.kind))
            .or_default()
            .push(idx);
    }
    let mut keep: HashSet<usize> = HashSet::new();
    for ((coord, _kind), indices) in groups {
        let ring = coord.x.abs().max(coord.z.abs()) as usize;
        let multiplier = RING_BUDGET
            .get(ring)
            .copied()
            .unwrap_or(*RING_BUDGET.last().unwrap());
        let keep_n = (indices.len() as f32 * multiplier).round() as usize;
        for idx in indices.into_iter().take(keep_n) {
            keep.insert(idx);
        }
    }
    let mut survivors = Vec::with_capacity(keep.len());
    for (idx, spawn) in spawns.drain(..).enumerate() {
        if keep.contains(&idx) {
            survivors.push(spawn);
        }
    }
    *spawns = survivors;
}

/// Convert a world-space position into the chunk the player is standing in,
/// for HUD / debug. Just delegates to [`ChunkCoord::from_world`].
pub fn chunk_for_position(pos: Vec3Net) -> ChunkCoord {
    ChunkCoord::from_world(pos.x, pos.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::ChunkDims;

    #[test]
    fn new_for_world_yields_consistent_node_state() {
        let (manager, nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
        assert_eq!(manager.live_node_count(), nodes.len());
        // Every live node should be tracked in node_chunks.
        for state in &nodes {
            assert!(manager.node_chunks.contains_key(&state.id));
        }
    }

    #[test]
    fn save_round_trips_state() {
        let (mut manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
        manager.regrow_queue.push(RegrowEvent {
            fire_tick: 10_000,
            coord: ChunkCoord::new(0, 0),
            kind: NodeKind::TreeMedium,
        });
        let save = manager.to_save(5_000);
        let restored = ChunkManager::from_save(save, 0);
        assert_eq!(restored.live_node_count(), manager.live_node_count());
        assert_eq!(restored.pending_regrow_count(), 1);
    }

    #[test]
    fn nodes_visible_to_returns_within_radius_only() {
        let (manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
        let near = manager.nodes_visible_to(Vec3Net::new(0.0, 0.0, 0.0), ViewRadiusTier::Low);
        let far = manager.nodes_visible_to(Vec3Net::new(0.0, 0.0, 0.0), ViewRadiusTier::High);
        // High view should never see fewer than low view.
        assert!(far.len() >= near.len());
        // Both should be within the total live count.
        assert!(far.len() <= manager.live_node_count());
    }

    #[test]
    fn handle_depleted_schedules_regrow_within_window() {
        let (mut manager, nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(3));
        let some_node = nodes.first().expect("world should have at least one node");
        let now_tick = 1_000;
        manager.handle_node_depleted(some_node.id, now_tick);
        let event = manager
            .regrow_queue
            .peek()
            .copied()
            .expect("regrow event should have been scheduled");
        let delay = event.fire_tick - now_tick;
        assert!(
            (MIN_REGROW_TICKS..=MAX_REGROW_TICKS).contains(&delay),
            "delay {delay} not in [{MIN_REGROW_TICKS}, {MAX_REGROW_TICKS}]"
        );
    }

    #[test]
    fn tick_spawns_pending_regrows() {
        let (mut manager, mut nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(3));
        let initial_count = manager.live_node_count();
        let some_node = nodes.remove(0);
        let depleted_id = some_node.id;
        // Mirror the server's depletion: remove from the live map +
        // notify the manager.
        let mut existing: HashMap<ResourceNodeId, ResourceNodeState> =
            nodes.into_iter().map(|n| (n.id, n)).collect();
        manager.handle_node_depleted(depleted_id, 0);
        // Fast-forward past the maximum regrow window.
        let RegrowResult { spawned } = manager.tick(MAX_REGROW_TICKS + 1, &existing);
        // We should get back exactly one fresh spawn — same kind, fresh
        // position, new id.
        assert_eq!(spawned.len(), 1, "expected exactly one regrow");
        let fresh = &spawned[0];
        assert_ne!(fresh.id, depleted_id);
        existing.insert(fresh.id, fresh.clone());
        // Net live count: manager dropped one and replaced it.
        assert_eq!(manager.live_node_count(), initial_count);
    }

    #[test]
    fn view_tier_radius_is_monotonic() {
        assert!(view_tier_radius(ViewRadiusTier::Low) < view_tier_radius(ViewRadiusTier::Medium));
        assert!(view_tier_radius(ViewRadiusTier::Medium) < view_tier_radius(ViewRadiusTier::High));
    }
}
