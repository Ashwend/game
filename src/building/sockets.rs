//! Socket-snap geometry: quarter-turn yaw snapping, the wall/ceiling/stairs
//! socket maps every placement snaps to, the socket-occupancy predicates,
//! and the perimeter wall face inset. The client preview and the server's
//! snap validation both walk these functions, so a socket the ghost shows
//! is exactly a socket the server accepts.

use crate::protocol::Vec3Net;

use super::{
    BuildingPiece, CEILING_THICKNESS_M, ClaimPlatform, FOUNDATION_HEIGHT_M, FOUNDATION_SIZE_M,
    WALL_FACE_INSET_BIAS_M, WALL_FACE_INSET_M, WALL_HEIGHT_M, rotate_offset,
};

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

/// Inward XZ offset to nudge a wall-like piece's *rendered model* so its
/// outer face is flush with the supporting platform edge rather than
/// overhanging it by [`WALL_FACE_INSET_M`]. The wall's stored `position`
/// stays on the edge midpoint (the canonical socket every other system
/// snaps, stacks, and supports against, and the collider the server
/// validates), so this is a visual-only nudge applied where the mesh is
/// placed.
///
/// A wall sits on the edge between two cells. The offset points toward the
/// cell that carries a platform at the wall's base height, and is `ZERO`
/// for an *interior* wall (a platform on both sides, so neither face
/// overhangs) or a wall with no platform to align to (a bare wall stack):
/// those stay centred on the edge. `platforms` is every foundation/ceiling
/// in range, each with its walkable `top`.
pub fn wall_face_inset_offset(
    wall_position: Vec3Net,
    wall_yaw: f32,
    platforms: &[ClaimPlatform],
) -> Vec3Net {
    let yaw = snap_yaw_quarter_turn(wall_yaw);
    // The wall spans local X; its two faces look along local ±Z. The cells
    // it sits between are half a foundation away along that world normal.
    let (nx, nz) = rotate_offset(yaw, 0.0, 1.0);
    let half = FOUNDATION_SIZE_M / 2.0;
    let supported = |sign: f32| {
        let cx = wall_position.x + sign * nx * half;
        let cz = wall_position.z + sign * nz * half;
        platforms.iter().any(|platform| {
            (platform.top - wall_position.y).abs() < SOCKET_EPSILON_M
                && (platform.position.x - cx).abs() < SOCKET_EPSILON_M
                && (platform.position.z - cz).abs() < SOCKET_EPSILON_M
        })
    };
    let plus = supported(1.0);
    let minus = supported(-1.0);
    // Inset a hair past flush so the outer face tucks just *behind* the
    // foundation edge; this is what keeps perpendicular corner walls from
    // z-fighting (see [`WALL_FACE_INSET_BIAS_M`]).
    let inset = WALL_FACE_INSET_M + WALL_FACE_INSET_BIAS_M;
    match (plus, minus) {
        // Platform on the +normal side only: outer face is on -normal, so
        // nudge toward +normal until the outer face meets the edge.
        (true, false) => Vec3Net::new(nx * inset, 0.0, nz * inset),
        (false, true) => Vec3Net::new(-nx * inset, 0.0, -nz * inset),
        // Interior wall, or an unsupported stack: leave it centred.
        _ => Vec3Net::ZERO,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{STAIR_RISE_M, WALL_THICKNESS_M};

    #[test]
    fn perimeter_wall_insets_so_its_outer_face_meets_the_edge() {
        // A lone foundation at the origin; a wall on its +Z edge has open
        // air on the outside, so the model nudges in until the outer face
        // sits flush with (just inside) the foundation edge instead of
        // overhanging it.
        let foundation_top = FOUNDATION_HEIGHT_M;
        let half = FOUNDATION_SIZE_M / 2.0;
        let platforms = [ClaimPlatform {
            position: Vec3Net::ZERO,
            top: foundation_top,
        }];
        let wall_pos = Vec3Net::new(0.0, foundation_top, half);

        let offset = wall_face_inset_offset(wall_pos, 0.0, &platforms);

        assert!(offset.x.abs() < 1e-6);
        assert!((offset.z + (WALL_FACE_INSET_M + WALL_FACE_INSET_BIAS_M)).abs() < 1e-6);
        // The outer face lands the corner bias just *inside* the edge: never
        // past it (the user's "don't exceed the foundation surface"), and no
        // more than the bias short of it.
        let outer_face = wall_pos.z + offset.z + WALL_THICKNESS_M / 2.0;
        assert!(outer_face <= half + 1e-6, "outer face overhangs the edge");
        assert!(
            (half - outer_face - WALL_FACE_INSET_BIAS_M).abs() < 1e-6,
            "outer face should sit one bias inside the edge"
        );
    }

    #[test]
    fn interior_wall_between_two_foundations_stays_centered() {
        // A wall on the shared edge of two foundations overhangs neither
        // (floor on both sides), so it must not move.
        let foundation_top = FOUNDATION_HEIGHT_M;
        let half = FOUNDATION_SIZE_M / 2.0;
        let platforms = [
            ClaimPlatform {
                position: Vec3Net::ZERO,
                top: foundation_top,
            },
            ClaimPlatform {
                position: Vec3Net::new(0.0, 0.0, FOUNDATION_SIZE_M),
                top: foundation_top,
            },
        ];
        let wall_pos = Vec3Net::new(0.0, foundation_top, half);

        assert_eq!(
            wall_face_inset_offset(wall_pos, 0.0, &platforms),
            Vec3Net::ZERO
        );
    }

    #[test]
    fn quarter_turned_perimeter_wall_insets_along_its_normal() {
        // The +X edge wall (yaw 90°) nudges along X, not Z.
        let foundation_top = FOUNDATION_HEIGHT_M;
        let half = FOUNDATION_SIZE_M / 2.0;
        let platforms = [ClaimPlatform {
            position: Vec3Net::ZERO,
            top: foundation_top,
        }];
        let wall_pos = Vec3Net::new(half, foundation_top, 0.0);

        let offset = wall_face_inset_offset(wall_pos, std::f32::consts::FRAC_PI_2, &platforms);

        assert!(offset.z.abs() < 1e-6);
        let outer_face = wall_pos.x + offset.x + WALL_THICKNESS_M / 2.0;
        assert!(outer_face <= half + 1e-6, "outer face overhangs the edge");
        assert!(
            (half - outer_face - WALL_FACE_INSET_BIAS_M).abs() < 1e-6,
            "outer face should sit one bias inside the edge"
        );
    }

    #[test]
    fn wall_with_no_platform_at_its_base_stays_centered() {
        // A wall stacked above the floor band (no foundation/ceiling at its
        // base height on either side) has nothing to align to.
        let platforms = [ClaimPlatform {
            position: Vec3Net::ZERO,
            top: FOUNDATION_HEIGHT_M,
        }];
        let wall_pos = Vec3Net::new(
            0.0,
            FOUNDATION_HEIGHT_M + WALL_HEIGHT_M,
            FOUNDATION_SIZE_M / 2.0,
        );

        assert_eq!(
            wall_face_inset_offset(wall_pos, 0.0, &platforms),
            Vec3Net::ZERO
        );
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
}
