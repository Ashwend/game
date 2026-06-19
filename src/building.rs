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

use serde::{Deserialize, Serialize};

use crate::{protocol::Vec3Net, world::WorldBlock};

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
// Geometry
// ---------------------------------------------------------------------

/// Edge length of a (square) foundation, and the width of every wall-like
/// piece, so walls exactly span a foundation edge.
pub const FOUNDATION_SIZE_M: f32 = 3.0;

/// One platform piece (foundation or ceiling) for Tool Cupboard claim
/// projection: its world position and the height of its walkable top.
pub struct ClaimPlatform {
    pub position: Vec3Net,
    pub top: f32,
}

/// XZ grid cell for a position on the 3 m building grid. The grid origin is
/// the first foundation (not the world origin), but stepping one cell
/// always changes `round(coord / 3)` by exactly one, so neighbours differ
/// by 1 on a single axis regardless of the base's fractional offset.
pub fn claim_cell_of(x: f32, z: f32) -> (i32, i32) {
    (
        (x / FOUNDATION_SIZE_M).round() as i32,
        (z / FOUNDATION_SIZE_M).round() as i32,
    )
}

/// True when `position` falls inside any of a claim's 3 m cell centres.
pub fn claim_cells_cover(cells: &[(f32, f32)], position: Vec3Net) -> bool {
    const HALF: f32 = FOUNDATION_SIZE_M / 2.0 + 0.05;
    cells
        .iter()
        .any(|(cx, cz)| (position.x - cx).abs() <= HALF && (position.z - cz).abs() <= HALF)
}

/// True when an axis-aligned XZ box overlaps any of a claim's 3 m cells.
/// Each cell is a [`FOUNDATION_SIZE_M`] square centred at its stored centre.
///
/// This is the *footprint*-aware claim test: where [`claim_cells_cover`]
/// only asks "is this point claimed?", this asks "does this piece's whole
/// model touch the claim?", so a block whose centre sits just outside a
/// claim but whose body pokes into it still counts. A tiny inset
/// (`EDGE_EPS`) makes exact edge-adjacency read as touching, not
/// overlapping, so a foundation tiled flush against the claim boundary (or
/// a hair off it from float drift) isn't falsely rejected.
pub fn claim_cells_overlap_aabb(
    cells: &[(f32, f32)],
    min_x: f32,
    min_z: f32,
    max_x: f32,
    max_z: f32,
) -> bool {
    const HALF: f32 = FOUNDATION_SIZE_M / 2.0;
    const EDGE_EPS: f32 = 0.02;
    cells.iter().any(|(cx, cz)| {
        min_x < cx + HALF - EDGE_EPS
            && max_x > cx - HALF + EDGE_EPS
            && min_z < cz + HALF - EDGE_EPS
            && max_z > cz - HALF + EDGE_EPS
    })
}

/// True when any of a piece's world-space collider boxes overlaps the
/// claim (XZ only, matching the vertical-column claim model). The single
/// source of truth for "would this placement's footprint intrude on a
/// claim", shared by the server gate and the client ghost so both agree.
pub fn claim_cells_overlap_blocks(cells: &[(f32, f32)], blocks: &[WorldBlock]) -> bool {
    blocks.iter().any(|block| {
        let min = block.min();
        let max = block.max();
        claim_cells_overlap_aabb(cells, min.x, min.z, max.x, max.z)
    })
}

/// Real XZ cell centres a Tool Cupboard at `cupboard_position` claims: the
/// connected base footprint (a flood fill over platform adjacency from the
/// platform the cupboard rests on) grown by `margin_cells`. Shared by the
/// server's authoritative gate and the client's placement ghost so both
/// agree on exactly which cells are claimed.
///
/// Adjacency: two platform cells connect if they share an XZ column (a
/// stacked upper floor) or are cardinal neighbours at the same height (a
/// contiguous floor). Walls/doorways aren't footprint cells; the floors
/// define the XZ extent.
pub fn claim_footprint_cells(
    platforms: &[ClaimPlatform],
    cupboard_position: Vec3Net,
    margin_cells: i32,
) -> Vec<(f32, f32)> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let cup_cell = claim_cell_of(cupboard_position.x, cupboard_position.z);

    // Per-platform cell + height bucket, plus spatial indices for O(1)
    // adjacency lookups during the flood fill.
    let cells: Vec<((i32, i32), i32)> = platforms
        .iter()
        .map(|p| {
            (
                claim_cell_of(p.position.x, p.position.z),
                (p.position.y * 10.0).round() as i32,
            )
        })
        .collect();
    let mut by_cell_height: HashMap<(i32, i32, i32), usize> = HashMap::new();
    let mut by_column: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (idx, ((cx, cz), hy)) in cells.iter().enumerate() {
        by_cell_height.insert((*cx, *cz, *hy), idx);
        by_column.entry((*cx, *cz)).or_default().push(idx);
    }

    // Anchor: the platform the cupboard rests on (same cell, top just under
    // the cupboard). Its real position fixes the grid offset used to map
    // cells back to world centres.
    let anchor = platforms.iter().enumerate().find(|(_, p)| {
        claim_cell_of(p.position.x, p.position.z) == cup_cell
            && (p.top - cupboard_position.y).abs() < 0.2
    });

    let (component, anchor_cell, anchor_real) = match anchor {
        Some((start, p)) => {
            let mut visited = vec![false; platforms.len()];
            let mut queue = VecDeque::new();
            let mut component: HashSet<(i32, i32)> = HashSet::new();
            visited[start] = true;
            queue.push_back(start);
            while let Some(idx) = queue.pop_front() {
                let ((cx, cz), hy) = cells[idx];
                component.insert((cx, cz));
                // Same column (any height): stacked floors.
                for &other in by_column.get(&(cx, cz)).into_iter().flatten() {
                    if !visited[other] {
                        visited[other] = true;
                        queue.push_back(other);
                    }
                }
                // Cardinal neighbour at the same height: contiguous floor.
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    if let Some(&other) = by_cell_height.get(&(cx + dx, cz + dz, hy))
                        && !visited[other]
                    {
                        visited[other] = true;
                        queue.push_back(other);
                    }
                }
            }
            (component, cup_cell, (p.position.x, p.position.z))
        }
        None => {
            // Not on a platform: claim just the cupboard's own cell.
            let mut component = HashSet::new();
            component.insert(cup_cell);
            (
                component,
                cup_cell,
                (cupboard_position.x, cupboard_position.z),
            )
        }
    };

    // Grow by the margin ring and map every cell to its real XZ centre.
    let mut expanded: HashSet<(i32, i32)> = HashSet::new();
    for (cx, cz) in &component {
        for dx in -margin_cells..=margin_cells {
            for dz in -margin_cells..=margin_cells {
                expanded.insert((cx + dx, cz + dz));
            }
        }
    }
    expanded
        .into_iter()
        .map(|(cx, cz)| {
            (
                anchor_real.0 + (cx - anchor_cell.0) as f32 * FOUNDATION_SIZE_M,
                anchor_real.1 + (cz - anchor_cell.1) as f32 * FOUNDATION_SIZE_M,
            )
        })
        .collect()
}

/// Height of the foundation platform. Tall enough to read as a real floor,
/// low enough that a jump clears it (the controller has no auto-step).
pub const FOUNDATION_HEIGHT_M: f32 = 0.5;
/// Height of wall-like pieces, measured from the foundation top.
pub const WALL_HEIGHT_M: f32 = 3.0;
/// Thickness of wall-like pieces.
pub const WALL_THICKNESS_M: f32 = 0.2;
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

/// Snap a building yaw to the nearest 90° increment, wrapped to [-π, π].
/// Buildings only exist on the quarter-turn grid (see module docs).
pub fn snap_yaw_quarter_turn(yaw: f32) -> f32 {
    use std::f32::consts::FRAC_PI_2;
    if !yaw.is_finite() {
        return 0.0;
    }
    let steps = (yaw / FRAC_PI_2).round();
    let snapped = steps * FRAC_PI_2;
    // Wrap to [-π, π] so 270° and -90° compare equal downstream.
    let mut wrapped = snapped % std::f32::consts::TAU;
    if wrapped > std::f32::consts::PI {
        wrapped -= std::f32::consts::TAU;
    } else if wrapped < -std::f32::consts::PI {
        wrapped += std::f32::consts::TAU;
    }
    wrapped
}

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

/// One mountable wall socket on a foundation: the edge midpoint (at
/// foundation-top height) plus the yaw a wall must take to span that edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallSocket {
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Height of a platform's walkable top surface above its base `position.y`
/// (where walls and stairs mount). `None` for non-platform pieces.
pub const fn platform_top_offset(piece: BuildingPiece) -> Option<f32> {
    match piece {
        BuildingPiece::Foundation => Some(FOUNDATION_HEIGHT_M),
        BuildingPiece::Ceiling => Some(CEILING_THICKNESS_M),
        _ => None,
    }
}

/// The four wall sockets of a foundation at `position`/`yaw`. Wall-like
/// pieces mount exactly here and nowhere else.
pub fn foundation_wall_sockets(position: Vec3Net, yaw: f32) -> [WallSocket; 4] {
    edge_wall_sockets(position, FOUNDATION_HEIGHT_M, yaw)
}

/// The four wall sockets of any platform piece (foundation edges at
/// ground level, ceiling edges one storey up), or `None` for pieces that
/// don't host walls.
pub fn platform_wall_sockets(
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
) -> Option<[WallSocket; 4]> {
    platform_top_offset(piece).map(|top| edge_wall_sockets(position, top, yaw))
}

fn edge_wall_sockets(position: Vec3Net, top_offset: f32, yaw: f32) -> [WallSocket; 4] {
    let yaw = snap_yaw_quarter_turn(yaw);
    let half = FOUNDATION_SIZE_M / 2.0;
    let top = position.y + top_offset;
    // Edge midpoints in platform-local space, with the wall running
    // along the edge: ±Z edges host walls whose local X spans world X
    // (wall yaw = platform yaw), ±X edges host quarter-turned walls.
    let locals: [(f32, f32, f32); 4] = [
        (0.0, half, 0.0),
        (0.0, -half, 0.0),
        (half, 0.0, std::f32::consts::FRAC_PI_2),
        (-half, 0.0, std::f32::consts::FRAC_PI_2),
    ];
    locals.map(|(lx, lz, extra_yaw)| {
        let (dx, dz) = rotate_offset(yaw, lx, lz);
        WallSocket {
            position: Vec3Net::new(position.x + dx, top, position.z + dz),
            yaw: snap_yaw_quarter_turn(yaw + extra_yaw),
        }
    })
}

/// The pose a ceiling takes when roofing the storey that stands on this
/// platform cell: same XZ, nested into the top of the wall band so the
/// slab's top lands flush with the wall tops, same yaw. `None` for
/// non-platform pieces.
pub fn ceiling_socket_above(
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
) -> Option<WallSocket> {
    let top = platform_top_offset(piece)?;
    Some(WallSocket {
        position: Vec3Net::new(
            position.x,
            position.y + top + WALL_HEIGHT_M - CEILING_THICKNESS_M,
            position.z,
        ),
        yaw: snap_yaw_quarter_turn(yaw),
    })
}

/// The base pose stairs take standing on this platform cell: same XZ,
/// sitting on the platform's top surface. The flight's rise direction is
/// the caller's quarter-snapped yaw, not the platform's. `None` for
/// non-platform pieces.
pub fn stairs_socket_on(
    piece: BuildingPiece,
    position: Vec3Net,
    stairs_yaw: f32,
) -> Option<WallSocket> {
    let top = platform_top_offset(piece)?;
    Some(WallSocket {
        position: Vec3Net::new(position.x, position.y + top, position.z),
        yaw: snap_yaw_quarter_turn(stairs_yaw),
    })
}

/// The four adjacent-cell sockets of a cell-sized piece: the neighbouring
/// grid cells at the same height, sharing this one's yaw. Used by
/// foundations (extending the ground grid) and ceilings (extending a
/// ledge outward).
pub fn cell_neighbor_sockets(position: Vec3Net, yaw: f32) -> [WallSocket; 4] {
    let yaw = snap_yaw_quarter_turn(yaw);
    let step = FOUNDATION_SIZE_M;
    let locals: [(f32, f32); 4] = [(step, 0.0), (-step, 0.0), (0.0, step), (0.0, -step)];
    locals.map(|(lx, lz)| {
        let (dx, dz) = rotate_offset(yaw, lx, lz);
        WallSocket {
            position: Vec3Net::new(position.x + dx, position.y, position.z + dz),
            yaw,
        }
    })
}

/// The socket on top of a wall-like piece where another wall-like piece
/// stacks (building upward without a full floor each storey). `None` for
/// non-wall pieces.
pub fn wall_top_socket(piece: BuildingPiece, position: Vec3Net, yaw: f32) -> Option<WallSocket> {
    piece.is_wall_like().then(|| WallSocket {
        position: Vec3Net::new(position.x, position.y + WALL_HEIGHT_M, position.z),
        yaw: snap_yaw_quarter_turn(yaw),
    })
}

/// The two ceiling cells a wall-like piece can carry: the cells on either
/// side of the wall's top edge. `None` for non-wall pieces.
pub fn wall_ceiling_sockets(
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
) -> Option<[WallSocket; 2]> {
    if !piece.is_wall_like() {
        return None;
    }
    let yaw = snap_yaw_quarter_turn(yaw);
    // Nested slab: the carried ceiling's base sits one slab below the
    // wall top so its walkable surface is flush with the wall's top edge.
    let top = position.y + WALL_HEIGHT_M - CEILING_THICKNESS_M;
    let half = FOUNDATION_SIZE_M / 2.0;
    // The wall spans local X; the carried cells sit half a cell away
    // along local Z on both sides.
    let (dx, dz) = rotate_offset(yaw, 0.0, half);
    Some([
        WallSocket {
            position: Vec3Net::new(position.x + dx, top, position.z + dz),
            yaw,
        },
        WallSocket {
            position: Vec3Net::new(position.x - dx, top, position.z - dz),
            yaw,
        },
    ])
}

/// Positions closer than this count as "the same socket" for occupancy and
/// server-side snap validation. Sockets are 3 m apart so 5 cm is plenty.
pub const SOCKET_EPSILON_M: f32 = 0.05;

/// True when two yaws describe the same wall plane (equal modulo π, a wall
/// spanning an edge from either side is the same wall).
pub fn same_wall_plane(a: f32, b: f32) -> bool {
    let diff = (a - b).rem_euclid(std::f32::consts::PI);
    !(0.05..=std::f32::consts::PI - 0.05).contains(&diff)
}

pub fn positions_match(a: Vec3Net, b: Vec3Net) -> bool {
    (a.x - b.x).abs() < SOCKET_EPSILON_M
        && (a.y - b.y).abs() < SOCKET_EPSILON_M
        && (a.z - b.z).abs() < SOCKET_EPSILON_M
}

/// True when a wall-like piece standing at `existing` blocks a wall-like
/// placement at `candidate`: same plane, same edge XZ, and vertical
/// spans that overlap. The vertical slack matters because a storey can
/// offer a wall socket from several sources (a wall's own top and the
/// adjacent ceiling's edge, which the nested-slab geometry makes
/// coincide); anything closer than a full wall height on the same edge
/// would interpenetrate, so the whole band counts as one slot.
pub fn wall_slot_blocked(
    existing_position: Vec3Net,
    existing_yaw: f32,
    candidate_position: Vec3Net,
    candidate_yaw: f32,
) -> bool {
    (existing_position.x - candidate_position.x).abs() < SOCKET_EPSILON_M
        && (existing_position.z - candidate_position.z).abs() < SOCKET_EPSILON_M
        && (existing_position.y - candidate_position.y).abs() < WALL_HEIGHT_M - 0.1
        && same_wall_plane(existing_yaw, candidate_yaw)
}

/// Axis-aligned solid boxes for a piece at `position`/`yaw`, in world
/// space. This is the single source of truth for building collision: the
/// server's spawn-safety grid, the placement overlap test, and the client
/// movement grid all build from these. Pieces with openings (window,
/// doorway) return one box per solid segment so the hole is genuinely
/// passable.
pub fn building_collider_blocks(
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
) -> Vec<WorldBlock> {
    piece_local_boxes(piece)
        .iter()
        .map(|(center, half)| {
            let (cx, cz) = rotate_offset(yaw, center.0, center.2);
            let (hx, hz) = rotate_half_extents(yaw, half.0, half.2);
            let mut bottom = position.y + center.1 - half.1;
            let top = position.y + center.1 + half.1;
            // A raised foundation's collider reaches down to the ground:
            // the skirt under the slab is solid, so nothing can be placed
            // (or walked) underneath it and an XZ-overlapping foundation
            // at a different height still collides.
            if matches!(piece, BuildingPiece::Foundation) {
                bottom = bottom.min(0.0);
            }
            WorldBlock::new(
                Vec3Net::new(position.x + cx, (bottom + top) / 2.0, position.z + cz),
                Vec3Net::new(hx, (top - bottom) / 2.0, hz),
            )
        })
        .collect()
}

/// Collider for a door panel seated in a doorway at `position`/`yaw`.
/// The box follows the hinge state so collision, interact targeting, and
/// hit detection all happen where the panel visibly is: closed, it fills
/// the opening plane; open, it is the AABB around the panel swung
/// [`DOOR_OPEN_ANGLE_RAD`] about the hinge (the local -X edge of the
/// opening, the same pivot and angle the client's panel mesh animates
/// with). The swung panel isn't axis-aligned, so its AABB is slightly
/// larger than the panel itself, close enough for the AABB-only
/// collision pipeline while leaving the opening genuinely passable.
pub fn door_collider_blocks(position: Vec3Net, yaw: f32, open: bool) -> Vec<WorldBlock> {
    let half_h = DOOR_PANEL_HEIGHT_M / 2.0;
    let ((cx_local, cz_local), (hx_local, hz_local)) = if open {
        open_door_panel_local_box()
    } else {
        (
            (0.0, 0.0),
            (DOORWAY_OPENING_WIDTH_M / 2.0, DOOR_PANEL_THICKNESS_M / 2.0),
        )
    };
    let (cx, cz) = rotate_offset(yaw, cx_local, cz_local);
    let (hx, hz) = rotate_half_extents(yaw, hx_local, hz_local);
    vec![WorldBlock::new(
        Vec3Net::new(position.x + cx, position.y + half_h, position.z + cz),
        Vec3Net::new(hx, half_h, hz),
    )]
}

/// XZ centre and half-extents (door-local space) of the AABB around the
/// open panel: the `DOOR_PANEL_WIDTH_M` x `DOOR_PANEL_THICKNESS_M`
/// rectangle rotated `-DOOR_OPEN_ANGLE_RAD` about the hinge at local
/// `(-DOOR_PANEL_WIDTH_M / 2, 0)`, matching `animate_door_panels_system`.
fn open_door_panel_local_box() -> ((f32, f32), (f32, f32)) {
    let hinge_x = -DOOR_PANEL_WIDTH_M / 2.0;
    let angle = -DOOR_OPEN_ANGLE_RAD;
    // Local +X (the panel span) and +Z (the panel thickness) after the
    // hinge rotation about Y.
    let span = (angle.cos(), -angle.sin());
    let thickness = (angle.sin(), angle.cos());
    let half_t = DOOR_PANEL_THICKNESS_M / 2.0;
    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for reach in [0.0, DOOR_PANEL_WIDTH_M] {
        for side in [-half_t, half_t] {
            let x = hinge_x + span.0 * reach + thickness.0 * side;
            let z = span.1 * reach + thickness.1 * side;
            min = (min.0.min(x), min.1.min(z));
            max = (max.0.max(x), max.1.max(z));
        }
    }
    (
        ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0),
        ((max.0 - min.0) / 2.0, (max.1 - min.1) / 2.0),
    )
}

/// Half-extents swap between X and Z on quarter turns.
fn rotate_half_extents(yaw: f32, hx: f32, hz: f32) -> (f32, f32) {
    let steps = ((yaw / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4);
    if steps % 2 == 0 { (hx, hz) } else { (hz, hx) }
}

/// One solid segment of a piece in local space: `(center (x, y, z),
/// half-extents (x, y, z))` with the piece's base at the local origin and
/// width along local X.
pub type LocalBox = ((f32, f32, f32), (f32, f32, f32));

/// Local-space solid boxes per piece. Shared by the collider builder
/// above and the client's composite-mesh builder so the ghost/visual
/// exactly matches what blocks movement.
pub fn piece_local_boxes(piece: BuildingPiece) -> &'static [LocalBox] {
    const F_HALF: f32 = FOUNDATION_SIZE_M / 2.0;
    const F_HH: f32 = FOUNDATION_HEIGHT_M / 2.0;
    const W_HALF: f32 = FOUNDATION_SIZE_M / 2.0;
    const W_HH: f32 = WALL_HEIGHT_M / 2.0;
    const T_H: f32 = WALL_THICKNESS_M / 2.0;

    const FOUNDATION: &[LocalBox] = &[((0.0, F_HH, 0.0), (F_HALF, F_HH, F_HALF))];

    const WALL: &[LocalBox] = &[((0.0, W_HH, 0.0), (W_HALF, W_HH, T_H))];

    // Window wall: left jamb, right jamb, sill band, header band.
    const WIN_HW: f32 = WINDOW_OPENING_WIDTH_M / 2.0;
    const WIN_JAMB_HW: f32 = (W_HALF - WIN_HW) / 2.0;
    const WIN_JAMB_CX: f32 = WIN_HW + WIN_JAMB_HW;
    const WIN_SILL_HH: f32 = WINDOW_SILL_HEIGHT_M / 2.0;
    const WIN_TOP: f32 = WINDOW_SILL_HEIGHT_M + WINDOW_OPENING_HEIGHT_M;
    const WIN_HEADER_HH: f32 = (WALL_HEIGHT_M - WIN_TOP) / 2.0;
    const WINDOW_WALL: &[LocalBox] = &[
        ((-WIN_JAMB_CX, W_HH, 0.0), (WIN_JAMB_HW, W_HH, T_H)),
        ((WIN_JAMB_CX, W_HH, 0.0), (WIN_JAMB_HW, W_HH, T_H)),
        ((0.0, WIN_SILL_HH, 0.0), (WIN_HW, WIN_SILL_HH, T_H)),
        (
            (0.0, WIN_TOP + WIN_HEADER_HH, 0.0),
            (WIN_HW, WIN_HEADER_HH, T_H),
        ),
    ];

    // Doorway: left jamb, right jamb, header beam over the opening.
    const DOOR_HW: f32 = DOORWAY_OPENING_WIDTH_M / 2.0;
    const DOOR_JAMB_HW: f32 = (W_HALF - DOOR_HW) / 2.0;
    const DOOR_JAMB_CX: f32 = DOOR_HW + DOOR_JAMB_HW;
    const DOOR_HEADER_HH: f32 = (WALL_HEIGHT_M - DOORWAY_OPENING_HEIGHT_M) / 2.0;
    const DOORWAY: &[LocalBox] = &[
        ((-DOOR_JAMB_CX, W_HH, 0.0), (DOOR_JAMB_HW, W_HH, T_H)),
        ((DOOR_JAMB_CX, W_HH, 0.0), (DOOR_JAMB_HW, W_HH, T_H)),
        (
            (0.0, DOORWAY_OPENING_HEIGHT_M + DOOR_HEADER_HH, 0.0),
            (DOOR_HW, DOOR_HEADER_HH, T_H),
        ),
    ];

    // Ceiling: one slab spanning the cell, base on the wall tops.
    const C_HH: f32 = CEILING_THICKNESS_M / 2.0;
    const CEILING: &[LocalBox] = &[((0.0, C_HH, 0.0), (F_HALF, C_HH, F_HALF))];

    const STAIRS: &[LocalBox] = &stair_step_boxes();

    match piece {
        BuildingPiece::Foundation => FOUNDATION,
        BuildingPiece::Wall => WALL,
        BuildingPiece::WindowWall => WINDOW_WALL,
        BuildingPiece::Doorway => DOORWAY,
        BuildingPiece::Ceiling => CEILING,
        BuildingPiece::Stairs => STAIRS,
    }
}

/// The stairs flight as solid filled steps, ascending along local +Z
/// from the cell's -Z edge to the +Z edge. Each box runs from the base
/// to that step's tread height, so the underside is solid (no crawling
/// through a staircase) and every tread is a real walkable AABB.
const fn stair_step_boxes() -> [LocalBox; STAIR_STEP_COUNT] {
    const RISE: f32 = STAIR_RISE_M / STAIR_STEP_COUNT as f32;
    const DEPTH: f32 = FOUNDATION_SIZE_M / STAIR_STEP_COUNT as f32;
    let mut boxes = [((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)); STAIR_STEP_COUNT];
    let mut index = 0;
    while index < STAIR_STEP_COUNT {
        let tread_top = RISE * (index as f32 + 1.0);
        let center_z = -FOUNDATION_SIZE_M / 2.0 + DEPTH * (index as f32 + 0.5);
        boxes[index] = (
            (0.0, tread_top / 2.0, center_z),
            (FOUNDATION_SIZE_M / 2.0, tread_top / 2.0, DEPTH / 2.0),
        );
        index += 1;
    }
    boxes
}

// ---------------------------------------------------------------------
// Structural stability
// ---------------------------------------------------------------------

/// One existing structural piece, as the stability maths sees it:
/// `(piece, position, yaw, stability_pct)`.
pub type StabilitySupport = (BuildingPiece, Vec3Net, f32, u32);

/// Stability a piece at `position`/`yaw` would have, given the existing
/// pieces (with their current stabilities). This is the single source of
/// truth for the support relations; the server's full recompute walks the
/// same rules in reverse, and the client uses it to predict ghost
/// validity from replicated stabilities:
///
/// - Foundations stand on the ground: always 100.
/// - Wall-like pieces stand on a platform's edge socket or stack on a
///   wall-like piece directly below (same plane), keeping
///   [`crate::game_balance::STABILITY_RETENTION_VERTICAL_PCT`].
/// - Ceilings hang from a wall-like piece under one of their edges
///   (vertical retention) or from an adjacent ceiling
///   ([`crate::game_balance::STABILITY_RETENTION_CEILING_NEIGHBOR_PCT`],
///   the cantilever decay).
/// - Stairs stand on a platform cell (vertical retention).
pub fn candidate_stability_pct(
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
    existing: &[StabilitySupport],
) -> u32 {
    use crate::game_balance::{
        STABILITY_RETENTION_CEILING_NEIGHBOR_PCT as NEIGHBOR_PCT,
        STABILITY_RETENTION_VERTICAL_PCT as VERTICAL_PCT,
    };
    let mut best = 0u32;
    let mut consider = |stability: u32, retention: u32| {
        best = best.max(stability * retention / 100);
    };
    match piece {
        BuildingPiece::Foundation => return 100,
        BuildingPiece::Wall | BuildingPiece::WindowWall | BuildingPiece::Doorway => {
            for (other_piece, other_position, other_yaw, stability) in existing {
                // Standing on a platform's edge socket.
                if let Some(sockets) =
                    platform_wall_sockets(*other_piece, *other_position, *other_yaw)
                    && sockets
                        .iter()
                        .any(|socket| positions_match(socket.position, position))
                {
                    consider(*stability, VERTICAL_PCT);
                }
                // Stacked on a wall-like piece in the same plane.
                if let Some(top) = wall_top_socket(*other_piece, *other_position, *other_yaw)
                    && positions_match(top.position, position)
                    && same_wall_plane(*other_yaw, yaw)
                {
                    consider(*stability, VERTICAL_PCT);
                }
            }
        }
        BuildingPiece::Ceiling => {
            for (other_piece, other_position, other_yaw, stability) in existing {
                // Carried by a wall-like piece under one of the edges.
                if let Some(sockets) =
                    wall_ceiling_sockets(*other_piece, *other_position, *other_yaw)
                    && sockets
                        .iter()
                        .any(|socket| positions_match(socket.position, position))
                {
                    consider(*stability, VERTICAL_PCT);
                }
                // Hanging off an adjacent ceiling (cantilever).
                if matches!(other_piece, BuildingPiece::Ceiling)
                    && cell_neighbor_sockets(*other_position, *other_yaw)
                        .iter()
                        .any(|socket| positions_match(socket.position, position))
                {
                    consider(*stability, NEIGHBOR_PCT);
                }
            }
        }
        BuildingPiece::Stairs => {
            for (other_piece, other_position, _, stability) in existing {
                if let Some(socket) = stairs_socket_on(*other_piece, *other_position, 0.0)
                    && positions_match(socket.position, position)
                {
                    consider(*stability, VERTICAL_PCT);
                }
            }
        }
    }
    best
}

// ---------------------------------------------------------------------
// Costs and health
// ---------------------------------------------------------------------

/// `(item id, quantity)` cost pair used by placement/upgrade/repair tables.
pub type MaterialCost = (&'static str, u16);

/// The material a tier is built from, for cost lookups. The sticks-look
/// first draft is built from raw wood; the upgrade ladder then moves to
/// workbench-refined hewn logs, then stone.
pub const fn tier_material(tier: BuildingTier) -> &'static str {
    match tier {
        BuildingTier::Sticks => crate::items::WOOD_ID,
        BuildingTier::HewnWood => crate::items::HEWN_LOG_ID,
        BuildingTier::Stone => crate::items::STONE_ID,
    }
}

/// True for the pieces priced like a foundation: full-cell volumes that
/// eat more material than a single wall span (the foundation slab, the
/// solid-stepped stairs).
const fn costs_like_foundation(piece: BuildingPiece) -> bool {
    matches!(piece, BuildingPiece::Foundation | BuildingPiece::Stairs)
}

/// Cost to place a fresh piece (always at the Sticks tier, paid in raw
/// wood).
pub const fn placement_cost(piece: BuildingPiece) -> MaterialCost {
    if costs_like_foundation(piece) {
        (
            crate::items::WOOD_ID,
            crate::game_balance::BUILDING_STICKS_COST_FOUNDATION,
        )
    } else {
        (
            crate::items::WOOD_ID,
            crate::game_balance::BUILDING_STICKS_COST_WALL,
        )
    }
}

/// Cost to upgrade a piece *to* `target` tier.
pub const fn upgrade_cost(piece: BuildingPiece, target: BuildingTier) -> MaterialCost {
    let foundation = costs_like_foundation(piece);
    match target {
        // Placement covers the sticks tier; upgrading "to sticks" never
        // happens but keep the table total.
        BuildingTier::Sticks => placement_cost(piece),
        BuildingTier::HewnWood => (
            crate::items::HEWN_LOG_ID,
            if foundation {
                crate::game_balance::BUILDING_HEWN_WOOD_COST_FOUNDATION
            } else {
                crate::game_balance::BUILDING_HEWN_WOOD_COST_WALL
            },
        ),
        BuildingTier::Stone => (
            crate::items::STONE_ID,
            if foundation {
                crate::game_balance::BUILDING_STONE_COST_FOUNDATION
            } else {
                crate::game_balance::BUILDING_STONE_COST_WALL
            },
        ),
    }
}

/// Cost of one hammer repair hit on a piece of `tier`.
pub const fn repair_cost(tier: BuildingTier) -> MaterialCost {
    let quantity = match tier {
        BuildingTier::Sticks => crate::game_balance::BUILDING_REPAIR_COST_STICKS,
        BuildingTier::HewnWood => crate::game_balance::BUILDING_REPAIR_COST_HEWN_WOOD,
        BuildingTier::Stone => crate::game_balance::BUILDING_REPAIR_COST_STONE,
    };
    (tier_material(tier), quantity)
}

/// Max health of a piece at a tier. Foundations carry 1.5x the wall budget,
/// they hold the whole base up.
pub const fn building_max_health(piece: BuildingPiece, tier: BuildingTier) -> u32 {
    let wall = match tier {
        BuildingTier::Sticks => crate::game_balance::BUILDING_STICKS_WALL_HP,
        BuildingTier::HewnWood => crate::game_balance::BUILDING_HEWN_WOOD_WALL_HP,
        BuildingTier::Stone => crate::game_balance::BUILDING_STONE_WALL_HP,
    };
    if matches!(piece, BuildingPiece::Foundation) {
        wall + wall / 2
    } else {
        wall
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
    fn footprint_overlap_catches_intruding_models_but_not_flush_tiling() {
        // A single claimed cell centred at the origin (3 m square).
        let cells = [(0.0, 0.0)];

        // A foundation slab dead-centre on the claimed cell overlaps.
        let inside = building_collider_blocks(BuildingPiece::Foundation, Vec3Net::ZERO, 0.0);
        assert!(claim_cells_overlap_blocks(&cells, &inside));

        // A foundation tiled flush into the next cell (3 m away on the same
        // grid) only shares the boundary line, so it must NOT count.
        let adjacent = building_collider_blocks(
            BuildingPiece::Foundation,
            Vec3Net::new(FOUNDATION_SIZE_M, 0.0, 0.0),
            0.0,
        );
        assert!(!claim_cells_overlap_blocks(&cells, &adjacent));

        // An off-grid foundation whose body reaches a metre into the claim
        // is blocked even though its centre sits outside the cell.
        let intruding = building_collider_blocks(
            BuildingPiece::Foundation,
            Vec3Net::new(FOUNDATION_SIZE_M - 1.0, 0.0, 0.0),
            0.0,
        );
        assert!(claim_cells_overlap_blocks(&cells, &intruding));
    }

    #[test]
    fn footprint_overlap_catches_a_wall_poking_over_the_boundary() {
        // Claimed cell at the origin; an outsider's wall span sits on the
        // shared edge of the next cell over, so its 0.2 m thickness pokes
        // into the claim. The point test at the wall centre could land
        // either side of the boundary, but the footprint test sees the body.
        let cells = [(0.0, 0.0)];
        let half = FOUNDATION_SIZE_M / 2.0;
        let wall = building_collider_blocks(
            BuildingPiece::Wall,
            Vec3Net::new(0.0, FOUNDATION_HEIGHT_M, half),
            0.0,
        );
        assert!(claim_cells_overlap_blocks(&cells, &wall));
    }

    #[test]
    fn yaw_snapping_lands_on_quarter_turns() {
        assert_eq!(snap_yaw_quarter_turn(0.1), 0.0);
        assert!((snap_yaw_quarter_turn(1.5) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((snap_yaw_quarter_turn(3.0).abs() - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(snap_yaw_quarter_turn(f32::NAN), 0.0);
    }

    #[test]
    fn wall_sockets_sit_on_foundation_edges_at_top_height() {
        let sockets = foundation_wall_sockets(Vec3Net::new(10.0, 0.0, -4.0), 0.0);
        for socket in sockets {
            assert!((socket.position.y - FOUNDATION_HEIGHT_M).abs() < 1e-6);
            let dx = (socket.position.x - 10.0).abs();
            let dz = (socket.position.z + 4.0).abs();
            // Exactly one axis offset by half the foundation size.
            assert!(
                (dx - 1.5).abs() < 1e-6 && dz < 1e-6 || dx < 1e-6 && (dz - 1.5).abs() < 1e-6,
                "socket off-grid: {dx} {dz}"
            );
        }
    }

    #[test]
    fn rotated_foundation_sockets_follow_the_quarter_turn() {
        let sockets = cell_neighbor_sockets(Vec3Net::ZERO, std::f32::consts::FRAC_PI_2);
        // A quarter-turned foundation still produces neighbours on the
        // cardinal grid (rotation permutes them, never skews them).
        for socket in sockets {
            let dx = socket.position.x.abs();
            let dz = socket.position.z.abs();
            assert!((dx - 3.0).abs() < 1e-6 && dz < 1e-6 || dx < 1e-6 && (dz - 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn doorway_boxes_leave_the_opening_clear() {
        let blocks = building_collider_blocks(BuildingPiece::Doorway, Vec3Net::ZERO, 0.0);
        assert_eq!(blocks.len(), 3);
        // Nothing solid in the middle of the opening at chest height.
        for block in &blocks {
            let min = block.min();
            let max = block.max();
            let inside_x = min.x < 0.0 && max.x > 0.0;
            let inside_y = min.y < 1.0 && max.y > 1.0;
            assert!(!(inside_x && inside_y), "opening obstructed by {block:?}");
        }
    }

    #[test]
    fn window_wall_boxes_leave_the_window_clear() {
        let blocks = building_collider_blocks(BuildingPiece::WindowWall, Vec3Net::ZERO, 0.0);
        assert_eq!(blocks.len(), 4);
        let probe_y = WINDOW_SILL_HEIGHT_M + WINDOW_OPENING_HEIGHT_M / 2.0;
        for block in &blocks {
            let min = block.min();
            let max = block.max();
            let inside_x = min.x < 0.0 && max.x > 0.0;
            let inside_y = min.y < probe_y && max.y > probe_y;
            assert!(!(inside_x && inside_y), "window obstructed by {block:?}");
        }
    }

    #[test]
    fn quarter_turned_walls_swap_their_half_extents() {
        let blocks = building_collider_blocks(
            BuildingPiece::Wall,
            Vec3Net::ZERO,
            std::f32::consts::FRAC_PI_2,
        );
        let block = blocks[0];
        let half = block.half_extents;
        assert!((half.x - WALL_THICKNESS_M / 2.0).abs() < 1e-6);
        assert!((half.z - FOUNDATION_SIZE_M / 2.0).abs() < 1e-6);
    }

    #[test]
    fn upgrade_path_walks_sticks_hewn_wood_stone() {
        assert_eq!(BuildingTier::Sticks.next(), Some(BuildingTier::HewnWood));
        assert_eq!(BuildingTier::HewnWood.next(), Some(BuildingTier::Stone));
        assert_eq!(BuildingTier::Stone.next(), None);
    }

    #[test]
    fn stair_steps_stay_under_the_controller_auto_step() {
        let boxes = piece_local_boxes(BuildingPiece::Stairs);
        assert_eq!(boxes.len(), STAIR_STEP_COUNT);
        let mut previous_top = 0.0;
        for (center, half) in boxes {
            let top = center.1 + half.1;
            let rise = top - previous_top;
            // Controller STEP_HEIGHT is 0.45; every tread must be
            // climbable without jumping.
            assert!(rise > 0.0 && rise <= 0.45, "unwalkable step rise {rise}");
            previous_top = top;
        }
        assert!((previous_top - STAIR_RISE_M).abs() < 1e-4);
    }

    #[test]
    fn ceiling_socket_stacks_storeys_exactly() {
        // Ceilings nest into the wall band: base one slab below the wall
        // top, walkable top flush with it. Every storey is then exactly
        // one wall height tall.
        let first = ceiling_socket_above(BuildingPiece::Foundation, Vec3Net::ZERO, 0.0)
            .expect("foundations host ceilings");
        assert!(
            (first.position.y - (FOUNDATION_HEIGHT_M + WALL_HEIGHT_M - CEILING_THICKNESS_M)).abs()
                < 1e-6
        );
        let second = ceiling_socket_above(BuildingPiece::Ceiling, first.position, 0.0)
            .expect("ceilings host the next storey");
        assert!((second.position.y - first.position.y - WALL_HEIGHT_M).abs() < 1e-6);
        assert!(ceiling_socket_above(BuildingPiece::Wall, Vec3Net::ZERO, 0.0).is_none());
    }

    #[test]
    fn stacked_walls_and_ceiling_edge_walls_share_one_height() {
        // The fix for uneven second storeys: a wall stacked on a wall and
        // a wall standing on the adjacent ceiling's edge must start at
        // exactly the same height, or storeys drift 0.2 m apart per
        // floor depending on build order.
        let foundation = Vec3Net::ZERO;
        let wall = foundation_wall_sockets(foundation, 0.0)[0];
        let stacked = wall_top_socket(BuildingPiece::Wall, wall.position, wall.yaw)
            .expect("walls stack on walls");
        let ceiling = ceiling_socket_above(BuildingPiece::Foundation, foundation, 0.0).unwrap();
        let edge_wall = platform_wall_sockets(BuildingPiece::Ceiling, ceiling.position, 0.0)
            .expect("ceilings host walls")[0];
        assert!(
            (stacked.position.y - edge_wall.position.y).abs() < 1e-5,
            "stacked {} vs ceiling-edge {}",
            stacked.position.y,
            edge_wall.position.y
        );
    }

    #[test]
    fn stairs_top_lands_flush_with_the_next_floor() {
        // Stairs on a foundation top must end exactly at the upper
        // surface of a ceiling roofing that storey.
        let stairs = stairs_socket_on(BuildingPiece::Foundation, Vec3Net::ZERO, 0.0)
            .expect("platforms host stairs");
        let stairs_top = stairs.position.y + STAIR_RISE_M;
        let ceiling = ceiling_socket_above(BuildingPiece::Foundation, Vec3Net::ZERO, 0.0).unwrap();
        let floor_above = ceiling.position.y + CEILING_THICKNESS_M;
        assert!((stairs_top - floor_above).abs() < 1e-5);
    }

    #[test]
    fn open_door_collider_follows_the_swung_panel() {
        let closed = door_collider_blocks(Vec3Net::ZERO, 0.0, false);
        assert_eq!(closed.len(), 1);
        assert!(
            closed[0].center.z.abs() < 1e-6,
            "closed panel sits in the opening plane"
        );

        let open = door_collider_blocks(Vec3Net::ZERO, 0.0, true);
        assert_eq!(open.len(), 1);
        let panel = open[0];
        // The panel swings toward local +Z about the hinge on the -X
        // side: its box must clear the opening and sit on the swing side.
        assert!(
            panel.max().x < -0.4,
            "open panel must leave the opening passable, max x {}",
            panel.max().x
        );
        assert!(panel.center.z > 0.3, "open panel sits on the swing side");
        assert!((panel.max().y - DOOR_PANEL_HEIGHT_M).abs() < 1e-5);
        assert!(panel.min().y.abs() < 1e-5);

        // A quarter-turned door swings its panel along the rotated axes:
        // rotate_offset maps local (x, z) to (z, -x) at 90 degrees.
        let turned = door_collider_blocks(Vec3Net::ZERO, std::f32::consts::FRAC_PI_2, true);
        assert!(turned[0].center.x > 0.3);
        assert!(turned[0].center.z > 0.3);
    }
}
