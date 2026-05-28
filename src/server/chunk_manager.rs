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

use serde::{Deserialize, Serialize};

use crate::{
    protocol::{
        ClientId, DeployedEntityId, DroppedItemId, ResourceNodeId, ResourceNodeState, Vec3Net,
        ViewRadiusTier,
    },
    resources::spawn_resource_node,
    world::{
        ChunkClassification, ChunkCoord, ChunkDims, ChunkSpawn, ClassificationChannels, NodeKind,
        PlayableBounds, base_capacity, generate_chunk_spawns, generate_world_spawns, kind_target,
        splitmix64,
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
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingRegrowSave {
    pub coord: ChunkCoord,
    pub kind: NodeKind,
    pub ticks_from_now: u64,
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

    /// The chunk coords a player at `player_pos` should receive
    /// snapshot data for under the given view tier. Centralizes the AoI
    /// ring math so every networked entity flows through the same
    /// chunk-visibility decision.
    pub fn visible_chunks(&self, player_pos: Vec3Net, tier: ViewRadiusTier) -> HashSet<ChunkCoord> {
        // Include the load-buffer ring so the client's collider grid is
        // already populated when the player crosses a boundary — see
        // `LOAD_BUFFER_RINGS` for the why.
        let radius = (view_tier_radius(tier) + LOAD_BUFFER_RINGS) as i32;
        let player_grid = ChunkCoord::from_world(player_pos.x, player_pos.z);
        let mut visible = HashSet::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let coord = ChunkCoord::new(player_grid.x + dx, player_grid.z + dz);
                if self.grids.contains_key(&coord) {
                    visible.insert(coord);
                }
            }
        }
        visible
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

    #[test]
    fn dropped_item_anchor_moves_when_position_crosses_chunk_boundary() {
        // Use a wider world so we can move across a chunk boundary
        // without falling off the playable map.
        let (mut manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
        let id: DroppedItemId = 42;

        manager.track_dropped_item(id, Vec3Net::new(8.0, 0.0, 0.0));
        let initial: Vec<_> = manager.dropped_items_in(ChunkCoord::new(0, 0)).collect();
        assert!(
            initial.contains(&id),
            "item should anchor in its origin chunk"
        );

        // 70m crosses into chunk x=1 (chunks are 64m wide).
        manager.update_dropped_item_chunk(id, Vec3Net::new(70.0, 0.0, 0.0));
        let after_origin: Vec<_> = manager.dropped_items_in(ChunkCoord::new(0, 0)).collect();
        let after_dest: Vec<_> = manager.dropped_items_in(ChunkCoord::new(1, 0)).collect();
        assert!(
            after_origin.is_empty(),
            "item must be removed from its old chunk after crossing the boundary"
        );
        assert!(
            after_dest.contains(&id),
            "item must appear in the new chunk after crossing the boundary"
        );

        manager.untrack_dropped_item(id);
        let after_untrack: Vec<_> = manager.dropped_items_in(ChunkCoord::new(1, 0)).collect();
        assert!(
            after_untrack.is_empty(),
            "untracking must drop the item from the chunk membership index"
        );
    }

    #[test]
    fn player_anchor_follows_position_updates() {
        let (mut manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
        let client_id: ClientId = 7;

        manager.track_player(client_id, Vec3Net::ZERO);
        assert_eq!(manager.player_chunk(client_id), Some(ChunkCoord::new(0, 0)));

        // 200m in +x and +z lands in chunk (3, 3).
        manager.update_player_chunk(client_id, Vec3Net::new(200.0, 0.0, 200.0));
        assert_eq!(
            manager.player_chunk(client_id),
            // 200/64 = 3.125 → floor → 3, but the test world is 5x5
            // (chunks -2..=2) so the out-of-bounds clamp pins it to 2.
            Some(ChunkCoord::new(2, 2))
        );

        manager.untrack_player(client_id);
        assert_eq!(manager.player_chunk(client_id), None);
    }

    #[test]
    fn visible_chunks_centers_on_player_and_excludes_unloaded_coords() {
        let (manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
        let visible_at_origin = manager.visible_chunks(Vec3Net::ZERO, ViewRadiusTier::Low);
        // Low tier + load buffer = radius 2; in a 5x5 world that's the
        // entire grid (chunks -2..=2).
        assert_eq!(visible_at_origin.len(), 25);

        // A player parked at the corner can only see chunks that exist.
        let corner = manager.visible_chunks(Vec3Net::new(128.0, 0.0, 128.0), ViewRadiusTier::Low);
        for coord in &corner {
            assert!(
                coord.x.abs() <= 2 && coord.z.abs() <= 2,
                "visible chunk {coord:?} is outside the loaded grid"
            );
        }
    }
}
