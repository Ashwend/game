//! Server-side owner of the chunk system.
//!
//! Chunks are the single anchor point for AoI streaming. Every networked
//! entity that lives in the world — resource nodes, dropped items, and
//! eventually buildings — is registered with the chunk that contains its
//! position. The snapshot builder asks the manager which chunks a given
//! player should see, then collects all entities anchored to those
//! chunks; there is no parallel per-entity AoI path.
//!
//! Responsibilities, all server-authoritative:
//!
//! 1. **Seeded generation** — builds the initial node spawn list for a
//!    world from `(world_seed, dims)` by calling the pure chunk generator.
//! 2. **Membership tracking** — for every entity type the chunk system
//!    knows about, the manager remembers which chunk it belongs to so
//!    snapshots can filter by AoI in O(visible_chunks × members).
//!    Entities themselves live in their owning collections (resource
//!    nodes in `GameServer::resource_nodes`, drops in `dropped_items`,
//!    players in `clients`); the chunk manager only stores ids.
//! 3. **Regrow scheduling** — when a node is depleted, schedules a fresh
//!    spawn 5–15 min later (jittered) at a noise-valid position in the
//!    same grid, up to the chunk's capacity ceiling.
//! 4. **AoI streaming** — given a player position and view radius tier,
//!    returns the set of chunk coords (and per-coord entity ids) the
//!    player should see. Used to filter every per-player snapshot.
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

use crate::{
    protocol::{
        ClientId, DeployedEntityId, DroppedItemId, ResourceNodeId, ResourceNodeState, Vec3Net,
        ViewRadiusTier,
    },
    resources::spawn_resource_node,
    world::{
        ChunkClassification, ChunkCoord, ChunkDims, ChunkSpawn, ClassificationChannels, NodeKind,
        base_capacity, generate_world_spawns, kind_target, splitmix64,
    },
};

mod aoi;
mod membership;
mod regrow;
mod save;

pub use regrow::RegrowResult;
pub use save::*;

#[cfg(test)]
mod tests;

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

/// Spatial-hysteresis margin for room subscriptions. A chunk is *added* to a
/// client's subscription when it enters the load radius (`view + buffer`), but
/// is only *removed* once the player moves this many extra rings beyond that
/// radius. The asymmetric add/keep thresholds stop chunks thrashing
/// (load → unload → reload) when a player walks along a chunk boundary: a
/// 1-chunk wobble can never cross both thresholds, so nothing unloads. This is
/// deterministic (no timer) and costs only the extra fringe rings' replication
/// while the player lingers near an edge. See `visible_chunks` / `retained_chunks`.
const KEEP_MARGIN_RINGS: u32 = 2;

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
/// it is derived from the seed and live nodes, but the live entity sets
/// themselves need explicit tracking since players harvest nodes, drop
/// items, and move between chunks.
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
    /// Dropped items anchored to this chunk. Updated whenever an item
    /// is spawned, picked up, merged, despawned, or moves across the
    /// boundary under physics.
    dropped_items: HashSet<DroppedItemId>,
    /// Players whose current position falls inside this chunk. Updated
    /// from accepted movement messages; the per-player anchor is what
    /// the AoI ring is centred on.
    players: HashSet<ClientId>,
    /// Placed structures (workbenches, furnaces, future deployables)
    /// rooted to this chunk. Anchored once at place time — deployables
    /// do not move under physics.
    deployed_entities: HashSet<DeployedEntityId>,
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

#[derive(Debug)]
pub struct ChunkManager {
    world_seed: u64,
    dims: ChunkDims,
    grids: HashMap<ChunkCoord, ActiveChunkState>,
    node_chunks: HashMap<ResourceNodeId, (ChunkCoord, NodeKind)>,
    /// Reverse index from dropped item id to its current chunk. Lets the
    /// physics step's update path remove-then-reinsert when an item
    /// crosses a chunk boundary without scanning every chunk.
    dropped_item_chunks: HashMap<DroppedItemId, ChunkCoord>,
    /// Reverse index from connected client to its current chunk.
    /// Updated whenever an accepted movement message changes the chunk
    /// the player is standing in.
    player_chunks: HashMap<ClientId, ChunkCoord>,
    /// Reverse index from placed-structure id to its anchor chunk. Lets
    /// despawn/destroy paths swap the membership in O(1).
    deployed_entity_chunks: HashMap<DeployedEntityId, ChunkCoord>,
    /// Reverse index from loot-bag id to its anchor chunk. Bags don't
    /// move after spawn so the membership only changes at
    /// spawn / despawn time, but the lookup helper is still wired
    /// through here so the AoI / replication pipeline treats bags
    /// identically to other chunk-anchored entities.
    loot_bag_chunks: HashMap<crate::protocol::LootBagId, ChunkCoord>,
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

        let mut grids = build_empty_grids(world_seed, dims);
        let mut node_chunks: HashMap<ResourceNodeId, (ChunkCoord, NodeKind)> = HashMap::new();
        let mut next_node_id: u64 = 1;
        let mut live_states: Vec<ResourceNodeState> = Vec::with_capacity(spawns.len());

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
                dropped_item_chunks: HashMap::new(),
                player_chunks: HashMap::new(),
                deployed_entity_chunks: HashMap::new(),
                loot_bag_chunks: HashMap::new(),
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
        let mut grids = build_empty_grids(save.world_seed, dims);

        let mut node_chunks: HashMap<ResourceNodeId, (ChunkCoord, NodeKind)> = HashMap::new();
        for entry in save.node_chunks {
            node_chunks.insert(entry.node_id, (entry.coord, entry.kind));
            if let Some(grid) = grids.get_mut(&entry.coord) {
                grid.record_live(entry.kind, entry.node_id);
            }
        }

        let mut regrow_queue = BinaryHeap::new();
        for pending in save.pending_regrows {
            let fire_tick = now_tick.saturating_add(pending.ticks_from_now.max(MIN_REGROW_TICKS));
            regrow_queue.push(RegrowEvent {
                fire_tick,
                coord: pending.coord,
                kind: pending.kind,
            });
        }

        Self {
            world_seed: save.world_seed,
            dims,
            grids,
            node_chunks,
            dropped_item_chunks: HashMap::new(),
            player_chunks: HashMap::new(),
            deployed_entity_chunks: HashMap::new(),
            loot_bag_chunks: HashMap::new(),
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
                kind,
            })
            .collect();
        let pending_regrows = self
            .regrow_queue
            .iter()
            .map(|event| PendingRegrowSave {
                coord: event.coord,
                kind: event.kind,
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
}

/// Build the per-chunk capacity tables for every coord in `dims`, leaving
/// the live entity sets empty. Both `new_for_world` and `from_save` start
/// here, and the cap derivation defers to [`kind_target`] — the same
/// formula the generator uses at world-gen time. Keeping them on one
/// function means generation and regrow ceilings can't drift; a save
/// loaded by code that scaled differently would silently over- or
/// under-fill on the next regrow.
fn build_empty_grids(world_seed: u64, dims: ChunkDims) -> HashMap<ChunkCoord, ActiveChunkState> {
    let mut grids: HashMap<ChunkCoord, ActiveChunkState> = HashMap::new();
    for coord in dims.coords() {
        let channels = ClassificationChannels::sample(world_seed, coord);
        let classification = channels.classify();
        let mut capacity = HashMap::new();
        for kind in NodeKind::ALL {
            let channel = channels.channel_for(kind);
            let target = kind_target(base_capacity(classification, kind), channel);
            if target > 0 {
                capacity.insert(kind, target);
            }
        }
        grids.insert(
            coord,
            ActiveChunkState {
                classification,
                capacity,
                ..ActiveChunkState::default()
            },
        );
    }
    grids
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
