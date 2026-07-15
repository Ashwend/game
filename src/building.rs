//! Shared domain rules for the base-building system: piece/tier taxonomy,
//! geometry (dimensions, edge sockets, collider boxes), placement snapping,
//! and the cost tables. Both the client (ghost preview + snapping UX) and the
//! server (placement validation, damage, repair/upgrade costs) read this
//! module so the two can never disagree about what a legal placement is.
//!
//! Geometry conventions:
//! - A piece's `position` is the centre of its *base* (same convention as
//!   other deployables). Foundations sit on the ground (`y = 0`); wall-like
//!   pieces sit on a foundation edge, so their base is the foundation top.
//! - Building yaw is always snapped to 90° increments. That keeps every
//!   collider an exact axis-aligned box, which the AABB-only collision
//!   pipeline (`WorldBlock` + `BlockGrid`) represents losslessly.
//! - Wall-like pieces span their local X axis (width 3 m) with thickness on
//!   local Z; `yaw` rotates local +Z like every other deployable.
//!
//! Split by concern into submodules and re-exported flat so
//! `crate::building::X` call sites stay stable regardless of which submodule
//! owns `X`. The taxonomy, the piece dimensions, and the shared quarter-turn
//! rotation helper stay here in the root: every submodule reads them.

use serde::{Deserialize, Serialize};

mod claims;
mod collision;
mod costs;
mod sockets;
mod stability;

pub use claims::{
    ClaimPlatform, claim_cell_of, claim_cells_cover, claim_cells_overlap_aabb,
    claim_cells_overlap_blocks, claim_footprint_cells,
};
pub use collision::{LocalBox, building_collider_blocks, door_collider_blocks, piece_local_boxes};
pub use costs::{
    MaterialCost, building_max_health, placement_cost, repair_cost, tier_material, upgrade_cost,
};
pub use sockets::{
    SOCKET_EPSILON_M, WallSocket, ceiling_socket_above, cell_neighbor_sockets,
    foundation_wall_sockets, platform_top_offset, platform_wall_sockets, positions_match,
    same_wall_plane, snap_yaw_quarter_turn, stairs_socket_on, wall_ceiling_sockets,
    wall_face_inset_offset, wall_slot_blocked, wall_top_socket,
};
pub use stability::{StabilitySupport, candidate_stability_pct};

/// Which structural piece a building block is. The set mirrors the classic
/// survival-game starter kit: floor, solid wall, window wall, doorway,
/// ceiling, and stairs. New variants append at the end: the save layer and
/// wire protocol encode the variant index, so reordering would silently
/// reinterpret old data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuildingPiece {
    Foundation,
    Wall,
    WindowWall,
    Doorway,
    Ceiling,
    Stairs,
}

impl BuildingPiece {
    pub const ALL: [Self; 6] = [
        Self::Foundation,
        Self::Wall,
        Self::WindowWall,
        Self::Doorway,
        Self::Ceiling,
        Self::Stairs,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Foundation => "Foundation",
            Self::Wall => "Wall",
            Self::WindowWall => "Window Wall",
            Self::Doorway => "Doorway",
            Self::Ceiling => "Ceiling",
            Self::Stairs => "Stairs",
        }
    }

    /// True for the pieces that mount on a platform edge socket (solid
    /// wall, window wall, doorway).
    pub const fn is_wall_like(self) -> bool {
        matches!(self, Self::Wall | Self::WindowWall | Self::Doorway)
    }

    /// True for the horizontal pieces that define a 3 m grid cell and
    /// carry walls on their edges: foundations on the ground, ceilings as
    /// each storey's floor/roof.
    pub const fn is_platform(self) -> bool {
        matches!(self, Self::Foundation | Self::Ceiling)
    }
}

/// Material tier of a placed building block. Pieces are always placed at
/// `Sticks` (the twig-lattice first draft, built from raw wood) and
/// upgraded in place with the hammer. Variant order is load-bearing:
/// postcard encodes the variant index into saves and the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BuildingTier {
    Sticks,
    HewnWood,
    Stone,
}

impl BuildingTier {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sticks => "Sticks",
            Self::HewnWood => "Hewn Wood",
            Self::Stone => "Stone",
        }
    }

    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Sticks => Some(Self::HewnWood),
            Self::HewnWood => Some(Self::Stone),
            Self::Stone => None,
        }
    }
}

// ---------------------------------------------------------------------
// Piece dimensions
// ---------------------------------------------------------------------

/// Edge length of a (square) foundation, and the width of every wall-like
/// piece, so walls exactly span a foundation edge.
pub const FOUNDATION_SIZE_M: f32 = 3.0;

/// Height of the foundation platform. Tall enough to read as a real floor,
/// low enough to sit under the controller's auto-step (`STEP_HEIGHT`, 0.45 m)
/// so a player walks straight up onto a ground-level foundation instead of
/// having to hop the lip every time.
pub const FOUNDATION_HEIGHT_M: f32 = 0.4;
/// Height of wall-like pieces, measured from the foundation top.
pub const WALL_HEIGHT_M: f32 = 3.0;
/// Thickness of wall-like pieces.
pub const WALL_THICKNESS_M: f32 = 0.2;
/// How far a perimeter wall's rendered model is nudged inward (toward the
/// supporting platform) so its outer face sits flush with the foundation
/// edge instead of overhanging it by half the wall thickness. See
/// [`wall_face_inset_offset`].
pub const WALL_FACE_INSET_M: f32 = WALL_THICKNESS_M / 2.0;
/// A few extra millimetres of inset past flush. At an outer corner two
/// perpendicular perimeter walls both reach the shared corner; landing
/// their outer faces *exactly* on the foundation edge makes the two corner
/// columns coincident and they z-fight ("mesh congestion / flicker").
/// Recessing each long face this far behind the edge lets the neighbouring
/// wall's end face (which still meets the edge) cleanly occlude it, so the
/// corner reads flush but nothing fights. Far too small to see (a 3 mm
/// recess on a 3 m wall).
pub const WALL_FACE_INSET_BIAS_M: f32 = 0.003;
/// Slab thickness of a ceiling. The slab nests into the top of the wall
/// band (base at `WALL_HEIGHT_M - CEILING_THICKNESS_M` above the floor),
/// so its walkable upper surface sits exactly flush with the wall tops.
/// That makes every storey exactly `WALL_HEIGHT_M` tall regardless of
/// whether the next wall stacks on a wall or stands on a ceiling edge.
pub const CEILING_THICKNESS_M: f32 = 0.2;
/// Steps in a stairs piece. The flight spans a full cell and rises one
/// storey, landing flush with the ceiling top above; the per-step rise
/// (0.375 m) stays under the controller's 0.45 m auto-step.
pub const STAIR_STEP_COUNT: usize = 8;
/// Total rise of a stairs piece: one storey. Ceilings nest into the wall
/// band, so the floor-to-floor distance is exactly the wall height.
pub const STAIR_RISE_M: f32 = WALL_HEIGHT_M;

/// Doorway opening: wide and tall enough for the player capsule with a
/// little slack, framed on both sides and capped by a header beam.
pub const DOORWAY_OPENING_WIDTH_M: f32 = 1.1;
pub const DOORWAY_OPENING_HEIGHT_M: f32 = 2.2;

/// Window opening: a head-height hole you can see (and later shoot)
/// through. Sized so the player can clamber through with a jump, like the
/// genre expects.
pub const WINDOW_OPENING_WIDTH_M: f32 = 1.0;
pub const WINDOW_SILL_HEIGHT_M: f32 = 1.1;
pub const WINDOW_OPENING_HEIGHT_M: f32 = 1.1;

/// Door panel dimensions: slightly smaller than the doorway opening so the
/// closed panel reads as seated inside the frame.
pub const DOOR_PANEL_WIDTH_M: f32 = 1.04;
pub const DOOR_PANEL_HEIGHT_M: f32 = 2.14;
pub const DOOR_PANEL_THICKNESS_M: f32 = 0.08;
/// How far the door swings when opened, in radians (~100°).
pub const DOOR_OPEN_ANGLE_RAD: f32 = 1.745;

/// Rotate a local-space (x, z) offset by a quarter-turn-snapped yaw.
/// Exact for the four cardinal yaws, no trig drift in socket positions.
fn rotate_offset(yaw: f32, x: f32, z: f32) -> (f32, f32) {
    // Quantize to a quarter-turn index: 0 = +Z forward, 1 = 90° …
    let steps = ((yaw / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4);
    match steps {
        0 => (x, z),
        1 => (z, -x),
        2 => (-x, -z),
        _ => (-z, x),
    }
}

// ---------------------------------------------------------------------
// Hidden item-registry ids for placed pieces
// ---------------------------------------------------------------------
//
// Building blocks are not inventory items (the building plan places them
// directly), but every `DeployedEntity` carries an `item_id` that the save
// layer and registry lookups key off. Each piece therefore has a hidden,
// non-craftable item definition.

pub const BUILDING_FOUNDATION_ITEM_ID: &str = "building_foundation";
pub const BUILDING_WALL_ITEM_ID: &str = "building_wall";
pub const BUILDING_WINDOW_WALL_ITEM_ID: &str = "building_window_wall";
pub const BUILDING_DOORWAY_ITEM_ID: &str = "building_doorway";
pub const BUILDING_CEILING_ITEM_ID: &str = "building_ceiling";
pub const BUILDING_STAIRS_ITEM_ID: &str = "building_stairs";

pub const fn building_item_id(piece: BuildingPiece) -> &'static str {
    match piece {
        BuildingPiece::Foundation => BUILDING_FOUNDATION_ITEM_ID,
        BuildingPiece::Wall => BUILDING_WALL_ITEM_ID,
        BuildingPiece::WindowWall => BUILDING_WINDOW_WALL_ITEM_ID,
        BuildingPiece::Doorway => BUILDING_DOORWAY_ITEM_ID,
        BuildingPiece::Ceiling => BUILDING_CEILING_ITEM_ID,
        BuildingPiece::Stairs => BUILDING_STAIRS_ITEM_ID,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_path_walks_sticks_hewn_wood_stone() {
        assert_eq!(BuildingTier::Sticks.next(), Some(BuildingTier::HewnWood));
        assert_eq!(BuildingTier::HewnWood.next(), Some(BuildingTier::Stone));
        assert_eq!(BuildingTier::Stone.next(), None);
    }
}
