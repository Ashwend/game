//! Serializable summaries of chunk manager state embedded in the save file.

use serde::{Deserialize, Serialize};

use crate::{
    protocol::ResourceNodeId,
    world::{ChunkCoord, NodeKind},
};

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
    /// re-clamps to at least [`super::MIN_REGROW_TICKS`].
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
