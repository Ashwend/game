//! World-map wire shapes: just the per-player markers the player drops on the
//! map themselves.
//!
//! The biome *terrain* image is NOT on the wire: it's a pure function of
//! `(world_seed, dims)`, both of which the client already receives in
//! `Welcome`, so the client generates it locally (see
//! [`crate::world::map_texture`]). Only the markers, which are per-account,
//! server-owned, and persisted, need a round trip. Markers are points the
//! player placed by hand (right-click on the map), shipped as plain positions
//! plus an optional label so the client can draw crisp pins at any map zoom,
//! and only ever sent to their owner.

use serde::{Deserialize, Serialize};

/// One player-placed pin on the map. The `id` is server-assigned and unique
/// per account; `name` is empty until the player labels it. Persisted in the
/// world save and only ever sent to its owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldMapMarker {
    pub id: u32,
    pub x: f32,
    pub z: f32,
    /// Player-given label; empty while the marker is still unnamed.
    pub name: String,
}

/// Client -> server mutation of the caller's own map markers. The server
/// owns the id space and the persisted store; it replies to every variant
/// with the caller's full updated marker list ([`super::ServerMessage::WorldMapMarkers`])
/// so the overlay refreshes without a full map refetch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorldMapMarkerCommand {
    /// Drop a new (unnamed) marker at a world position. The server assigns
    /// the id and rejects the add once the per-player cap is reached.
    Add { x: f32, z: f32 },
    /// Set (or clear, when empty) a marker's label.
    Rename { id: u32, name: String },
    /// Delete a marker.
    Remove { id: u32 },
}
