//! The structural-stability support relations: what a candidate piece may
//! stand on and how much stability each support path retains. The server's
//! full-world recompute (`src/server/stability.rs`) walks the same rules in
//! reverse, and the client uses this to predict ghost validity from
//! replicated stabilities.

use crate::protocol::Vec3Net;

use super::{
    BuildingPiece, cell_neighbor_sockets, platform_wall_sockets, positions_match, same_wall_plane,
    stairs_socket_on, wall_ceiling_sockets, wall_top_socket,
};

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
