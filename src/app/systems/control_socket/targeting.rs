//! Building-snap geometry for scripted placement over the control socket.
//!
//! Split out of the transport adapter (`super`) because resolving the nearest
//! socket for a building piece is gameplay geometry, not socket plumbing. These
//! are pure functions over the replicated `DeployableDump` view; the snap math
//! itself lives in `crate::building` and is shared with the live client preview
//! (`app::systems::deployables::placement`).

use anyhow::{Result, bail};

use crate::app::state::ClientRuntime;
use crate::protocol::Vec3Net;

use super::DeployableDump;

/// Kind-string needle for targeting a building block by piece, e.g.
/// `Some("wall")` → `"piece: Wall,"`. `None` matches any building block.
/// The trailing comma keeps `Wall` from matching `WindowWall`'s debug
/// representation.
pub(super) fn building_piece_needle(piece: Option<&str>) -> Result<String> {
    Ok(match piece {
        None => "Building".to_owned(),
        Some(raw) => format!("piece: {:?},", parse_building_piece(raw)?),
    })
}

pub(super) fn parse_building_piece(piece: &str) -> Result<crate::building::BuildingPiece> {
    use crate::building::BuildingPiece;
    Ok(match piece {
        "foundation" => BuildingPiece::Foundation,
        "wall" => BuildingPiece::Wall,
        "window_wall" | "window-wall" | "window" => BuildingPiece::WindowWall,
        "doorway" => BuildingPiece::Doorway,
        "ceiling" => BuildingPiece::Ceiling,
        "stairs" => BuildingPiece::Stairs,
        other => bail!(
            "unknown building piece {other:?} (foundation|wall|window_wall|doorway|ceiling|stairs)"
        ),
    })
}

/// Resolve the exact snapped pose for a scripted building placement.
/// Foundations ride the flat `y = 0` aim (the server snaps them); every
/// other piece re-derives the nearest socket from the replicated
/// building set, because the server snap is 3D and a ground-level
/// request can't reach an upper-storey socket. Falls back to the raw aim
/// when nothing is in range (the server will toast the reason).
pub(super) fn resolve_building_pose(
    piece: crate::building::BuildingPiece,
    aim: Vec3Net,
    requested_yaw: f32,
    deployables: &[DeployableDump],
) -> (Vec3Net, f32) {
    use crate::building::{
        BuildingPiece, cell_neighbor_sockets, platform_wall_sockets, positions_match,
        stairs_socket_on, wall_ceiling_sockets, wall_slot_blocked, wall_top_socket,
    };
    if matches!(piece, BuildingPiece::Foundation) {
        return (aim, requested_yaw);
    }
    let dump_position =
        |dump: &DeployableDump| Vec3Net::new(dump.position[0], dump.position[1], dump.position[2]);
    let dump_piece = |dump: &DeployableDump| -> Option<BuildingPiece> {
        // Match the longer names first: "piece: Wall" is a substring
        // hazard only against itself, but order keeps this future-proof.
        if dump.kind.contains("piece: Foundation") {
            Some(BuildingPiece::Foundation)
        } else if dump.kind.contains("piece: WindowWall") {
            Some(BuildingPiece::WindowWall)
        } else if dump.kind.contains("piece: Wall") {
            Some(BuildingPiece::Wall)
        } else if dump.kind.contains("piece: Doorway") {
            Some(BuildingPiece::Doorway)
        } else if dump.kind.contains("piece: Ceiling") {
            Some(BuildingPiece::Ceiling)
        } else if dump.kind.contains("piece: Stairs") {
            Some(BuildingPiece::Stairs)
        } else {
            None
        }
    };
    let wall_slot_taken = |position: Vec3Net, yaw: f32| {
        deployables.iter().any(|dump| {
            dump_piece(dump).is_some_and(|p| p.is_wall_like())
                && wall_slot_blocked(dump_position(dump), dump.yaw, position, yaw)
        })
    };
    let ceiling_at = |position: Vec3Net| {
        deployables.iter().any(|dump| {
            matches!(dump_piece(dump), Some(BuildingPiece::Ceiling))
                && positions_match(dump_position(dump), position)
        })
    };
    let mut best: Option<(f32, Vec3Net, f32)> = None;
    for dump in deployables {
        let Some(existing_piece) = dump_piece(dump) else {
            continue;
        };
        let position = dump_position(dump);
        let sockets: Vec<(Vec3Net, f32)> = match piece {
            BuildingPiece::Wall | BuildingPiece::WindowWall | BuildingPiece::Doorway => {
                let platform = platform_wall_sockets(existing_piece, position, dump.yaw)
                    .into_iter()
                    .flatten();
                let stacked = wall_top_socket(existing_piece, position, dump.yaw).into_iter();
                platform
                    .chain(stacked)
                    .filter(|socket| !wall_slot_taken(socket.position, socket.yaw))
                    .map(|socket| (socket.position, socket.yaw))
                    .collect()
            }
            BuildingPiece::Ceiling => {
                let carried = wall_ceiling_sockets(existing_piece, position, dump.yaw)
                    .into_iter()
                    .flatten();
                let neighbors = matches!(existing_piece, BuildingPiece::Ceiling)
                    .then(|| cell_neighbor_sockets(position, dump.yaw))
                    .into_iter()
                    .flatten();
                carried
                    .chain(neighbors)
                    .filter(|socket| !ceiling_at(socket.position))
                    .map(|socket| (socket.position, socket.yaw))
                    .collect()
            }
            BuildingPiece::Stairs => stairs_socket_on(existing_piece, position, requested_yaw)
                .into_iter()
                .map(|socket| (socket.position, socket.yaw))
                .collect(),
            BuildingPiece::Foundation => Vec::new(),
        };
        for (socket_position, socket_yaw) in sockets {
            let dx = socket_position.x - aim.x;
            let dz = socket_position.z - aim.z;
            let distance = (dx * dx + dz * dz).sqrt();
            if distance <= 1.6
                && best
                    .as_ref()
                    .is_none_or(|(current, _, _)| distance < *current)
            {
                best = Some((distance, socket_position, socket_yaw));
            }
        }
    }
    match best {
        Some((_, position, yaw)) => (position, yaw),
        None => (aim, requested_yaw),
    }
}

/// Nearest replicated deployable whose `kind` debug string matches
/// `kind_prefix` (e.g. "Doorway" matches `Building {{ piece: Doorway, … }}`
/// via the contains check below; "Door" matches the door kind, whose debug
/// string is now `Door {{ variant: … }}` for either variant).
pub(super) fn nearest_deployable_id(
    runtime: &ClientRuntime,
    deployables: &[DeployableDump],
    kind_prefix: &str,
) -> Option<u64> {
    let position = runtime.local_view().map(|view| view.position)?;
    deployables
        .iter()
        .filter(|dump| {
            if kind_prefix == "Door" {
                // Match a door of any variant ("Door { variant: ... }")
                // without also matching "Building { piece: Doorway, ... }".
                dump.kind.starts_with("Door {")
            } else {
                dump.kind.contains(kind_prefix)
            }
        })
        .min_by(|a, b| {
            let da = (a.position[0] - position.x).powi(2) + (a.position[2] - position.z).powi(2);
            let db = (b.position[0] - position.x).powi(2) + (b.position[2] - position.z).powi(2);
            da.total_cmp(&db)
        })
        .map(|dump| dump.id)
}
