//! Low-level placement geometry queries: which building socket, cell,
//! doorway, or wall face the player's aim resolves to, plus the occupancy
//! and box-overlap tests the ghost validates against. Client mirrors of the
//! server's snap logic (`src/server/building.rs`), so the preview pose is
//! the pose the server will accept.
//!
//! Promotion candidates for `crate::building` (near-duplicates spotted, not
//! moved; this split is client-side code motion only): [`ray_aabb`] vs
//! `server::combat::ray_aabb_entry` / `server::projectiles::ray_aabb_entry_normal`,
//! and the nearest-socket scans vs `server/building.rs` `snap_wall_socket`
//! / `snap_ceiling` / `snap_stairs` / `snap_foundation`.

use bevy::prelude::*;

use crate::{
    building::{
        BuildingPiece, cell_neighbor_sockets, platform_wall_sockets, positions_match,
        stairs_socket_on, wall_ceiling_sockets, wall_slot_blocked, wall_top_socket,
    },
    items::DeployableKind,
    protocol::{DeployedEntityId, Vec3Net},
    server::{Deployable, DeployableStability, DeployableTransform},
};

use super::super::deployable_colliders;

/// How far the aim point may sit from a building socket before the ghost
/// snaps onto it. The shared balance constant is also the server's accept
/// tolerance, so client snap and server validation cannot drift.
const SNAP_TOLERANCE_M: f32 = crate::game_balance::BUILDING_SNAP_TOLERANCE_M;
/// Latch radius for cell-sized targets (ceilings, stairs): half a cell
/// plus a touch of slack. The ghost sends the exact snapped pose, so the
/// server's tighter tolerance still passes; this only controls how
/// forgiving the aim is.
const CELL_SNAP_RANGE_M: f32 = 1.6;
/// How far the aim point may sit from a doorway before the door ghost
/// latches onto it. More generous than the socket snap, doorways are
/// big targets and there's at most a handful nearby.
const DOOR_SNAP_RANGE_M: f32 = 2.5;

/// Nearest wall-like building piece the look ray hits, as `(t, point, outward
/// normal)`. Only near-vertical faces count, a torch mounts on a wall, not a
/// floor. Player-built walls only; the distant perimeter masonry is ignored.
pub(super) fn nearest_wall_hit(
    origin: Vec3,
    forward: Vec3,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> Option<(f32, Vec3, Vec3)> {
    let mut best: Option<(f32, Vec3, Vec3)> = None;
    for (meta, transform, _) in replicated.iter() {
        let DeployableKind::Building { piece, .. } = meta.kind else {
            continue;
        };
        if !piece.is_wall_like() {
            continue;
        }
        for block in
            crate::building::building_collider_blocks(piece, transform.position, transform.yaw)
        {
            let min = Vec3::new(block.min().x, block.min().y, block.min().z);
            let max = Vec3::new(block.max().x, block.max().y, block.max().z);
            let Some((t, normal)) = ray_aabb(origin, forward, min, max) else {
                continue;
            };
            // Skip the wall's top/bottom faces: a torch mounts on the side.
            if normal.y.abs() > 0.5 || t > 50.0 {
                continue;
            }
            if best.as_ref().is_none_or(|(best_t, _, _)| t < *best_t) {
                best = Some((t, origin + forward * t, normal));
            }
        }
    }
    best
}

/// Slab-method ray vs AABB, returning the entry distance and the entry face
/// normal (pointing back toward the ray origin). `None` when the ray misses or
/// only meets the box behind the origin.
fn ray_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let inv = dir.recip();
    let mut tmin = f32::NEG_INFINITY;
    let mut tmax = f32::INFINITY;
    let mut axis = 0usize;
    let mut sign = 0.0f32;
    for i in 0..3 {
        let lo = (min[i] - origin[i]) * inv[i];
        let hi = (max[i] - origin[i]) * inv[i];
        let (near, far, face_sign) = if lo <= hi {
            (lo, hi, -1.0)
        } else {
            (hi, lo, 1.0)
        };
        if near > tmin {
            tmin = near;
            axis = i;
            sign = face_sign;
        }
        if far < tmax {
            tmax = far;
        }
        if tmax < tmin {
            return None;
        }
    }
    if tmin < 0.0 {
        return None;
    }
    let mut normal = Vec3::ZERO;
    normal[axis] = sign;
    Some((tmin, normal))
}

/// Where the look ray crosses the horizontal plane at `plane_y`. Unlike
/// [`super::ground_under_aim`] this accepts upward rays, second-storey
/// sockets sit above the camera when the player stands at ground level.
fn aim_on_plane(camera_transform: &GlobalTransform, plane_y: f32) -> Option<Vec3> {
    let origin = camera_transform.translation();
    let forward = camera_transform.forward().as_vec3();
    if forward.y.abs() < 1e-4 {
        return None;
    }
    let t = (plane_y - origin.y) / forward.y;
    if t <= 0.0 || t > 60.0 {
        return None;
    }
    let hit = origin + forward * t;
    Some(Vec3::new(hit.x, plane_y, hit.z))
}

/// Distance from where the player is aiming (on the socket's own height
/// plane) to the socket, or `None` when the look ray can't reach that
/// plane. Aiming per-plane is what makes upper-storey sockets judged
/// where the player points, not by a ground projection far behind them.
fn aim_distance_to(camera_transform: &GlobalTransform, position: Vec3Net) -> Option<f32> {
    let aim = aim_on_plane(camera_transform, position.y)?;
    let dx = position.x - aim.x;
    let dz = position.z - aim.z;
    Some((dx * dx + dz * dz).sqrt())
}

pub(super) fn nearest_wall_socket(
    camera_transform: &GlobalTransform,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> Option<crate::building::WallSocket> {
    let mut best: Option<(f32, crate::building::WallSocket)> = None;
    let mut consider = |socket: crate::building::WallSocket| {
        let Some(distance) = aim_distance_to(camera_transform, socket.position) else {
            return;
        };
        if distance <= SNAP_TOLERANCE_M
            && best.as_ref().is_none_or(|(current, _)| distance < *current)
        {
            best = Some((distance, socket));
        }
    };
    for (meta, transform, _) in replicated.iter() {
        let DeployableKind::Building { piece, .. } = meta.kind else {
            continue;
        };
        if let Some(sockets) = platform_wall_sockets(piece, transform.position, transform.yaw) {
            for socket in sockets {
                consider(socket);
            }
        }
        // Walls also stack directly on walls (no floor needed per storey).
        if let Some(top) = wall_top_socket(piece, transform.position, transform.yaw) {
            consider(top);
        }
    }
    best.map(|(_, socket)| socket)
}

/// The ceiling pose nearest the aim: cells flanking a wall's top edge,
/// or cells adjacent to an existing ceiling (extending a ledge). Whether
/// the spot is stable enough is the stability gate's call.
pub(super) fn nearest_ceiling_cell(
    camera_transform: &GlobalTransform,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> Option<(Vec3Net, f32)> {
    let mut best: Option<(f32, Vec3Net, f32)> = None;
    let mut consider = |position: Vec3Net, yaw: f32| {
        let Some(distance) = aim_distance_to(camera_transform, position) else {
            return;
        };
        if distance <= CELL_SNAP_RANGE_M
            && best
                .as_ref()
                .is_none_or(|(current, _, _)| distance < *current)
        {
            best = Some((distance, position, yaw));
        }
    };
    for (meta, transform, _) in replicated.iter() {
        let DeployableKind::Building { piece, .. } = meta.kind else {
            continue;
        };
        if let Some(cells) = wall_ceiling_sockets(piece, transform.position, transform.yaw) {
            for cell in cells {
                consider(cell.position, cell.yaw);
            }
        }
        if matches!(piece, BuildingPiece::Ceiling) {
            for socket in cell_neighbor_sockets(transform.position, transform.yaw) {
                consider(socket.position, socket.yaw);
            }
        }
    }
    best.map(|(_, position, yaw)| (position, yaw))
}

/// The stairs base pose on the platform cell nearest the aim.
pub(super) fn nearest_stairs_cell(
    camera_transform: &GlobalTransform,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> Option<Vec3Net> {
    let mut best: Option<(f32, Vec3Net)> = None;
    for (meta, transform, _) in replicated.iter() {
        let DeployableKind::Building { piece, .. } = meta.kind else {
            continue;
        };
        let Some(socket) = stairs_socket_on(piece, transform.position, 0.0) else {
            continue;
        };
        let Some(aim) = aim_on_plane(camera_transform, socket.position.y) else {
            continue;
        };
        let dx = socket.position.x - aim.x;
        let dz = socket.position.z - aim.z;
        let distance = (dx * dx + dz * dz).sqrt();
        if distance <= CELL_SNAP_RANGE_M
            && best.as_ref().is_none_or(|(current, _)| distance < *current)
        {
            best = Some((distance, socket.position));
        }
    }
    best.map(|(_, position)| position)
}

pub(super) fn nearest_foundation_neighbor(
    aim: Vec3Net,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> Option<crate::building::WallSocket> {
    let mut best: Option<(f32, crate::building::WallSocket)> = None;
    for (meta, transform, _) in replicated.iter() {
        let DeployableKind::Building {
            piece: BuildingPiece::Foundation,
            ..
        } = meta.kind
        else {
            continue;
        };
        for socket in cell_neighbor_sockets(transform.position, transform.yaw) {
            let dx = socket.position.x - aim.x;
            let dz = socket.position.z - aim.z;
            let distance = (dx * dx + dz * dz).sqrt();
            if distance <= SNAP_TOLERANCE_M
                && best.as_ref().is_none_or(|(current, _)| distance < *current)
            {
                best = Some((distance, socket));
            }
        }
    }
    best.map(|(_, socket)| socket)
}

pub(super) fn wall_socket_occupied(
    position: Vec3Net,
    yaw: f32,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> bool {
    replicated.iter().any(|(meta, transform, _)| {
        matches!(meta.kind, DeployableKind::Building { piece, .. } if piece.is_wall_like())
            && wall_slot_blocked(transform.position, transform.yaw, position, yaw)
    })
}

pub(super) fn foundation_cell_occupied(
    position: Vec3Net,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> bool {
    replicated.iter().any(|(meta, transform, _)| {
        matches!(
            meta.kind,
            DeployableKind::Building {
                piece: BuildingPiece::Foundation,
                ..
            }
        ) && positions_match(transform.position, position)
    })
}

pub(super) fn any_replicated_overlap(
    blocks: &[crate::world::WorldBlock],
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
    skip_wall_plane: bool,
) -> bool {
    for (meta, transform, _) in replicated.iter() {
        // Stairs candidates legitimately clip walls/doors on their cell
        // edges; the caller opts out of those pairs.
        if skip_wall_plane {
            let wall_plane = matches!(meta.kind, DeployableKind::Door { .. })
                || matches!(meta.kind, DeployableKind::Building { piece, .. } if piece.is_wall_like());
            if wall_plane {
                continue;
            }
        }
        // Open/closed doesn't matter for placement previews; treat doors
        // as closed (worst case). The millimetre epsilon mirrors the
        // server's: touching faces plus f32 rounding aren't a collision.
        const EPSILON: f32 = 0.001;
        for other in deployable_colliders(meta, transform, false) {
            for candidate in blocks {
                let a_min = candidate.min();
                let a_max = candidate.max();
                let b_min = other.min();
                let b_max = other.max();
                if a_min.x + EPSILON < b_max.x
                    && a_max.x > b_min.x + EPSILON
                    && a_min.y + EPSILON < b_max.y
                    && a_max.y > b_min.y + EPSILON
                    && a_min.z + EPSILON < b_max.z
                    && a_max.z > b_min.z + EPSILON
                {
                    return true;
                }
            }
        }
    }
    false
}

/// The nearest mount opening of `mount_piece` (by horizontal distance to
/// the aim point) that doesn't already hold a panel. Panels sit at exactly
/// their opening's position, so occupancy is a position match against door
/// entities. Doors pass `Doorway`; the shutter passes `WindowWall`.
pub(super) fn nearest_free_mount(
    aim: Vec3Net,
    mount_piece: BuildingPiece,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> Option<(DeployedEntityId, Vec3Net, f32)> {
    let mut best: Option<(f32, DeployedEntityId, Vec3Net, f32)> = None;
    for (meta, transform, _) in replicated.iter() {
        let matches_mount = matches!(
            meta.kind,
            DeployableKind::Building { piece, .. } if piece == mount_piece
        );
        if !matches_mount {
            continue;
        }
        let dx = transform.position.x - aim.x;
        let dz = transform.position.z - aim.z;
        let distance = (dx * dx + dz * dz).sqrt();
        if distance > DOOR_SNAP_RANGE_M {
            continue;
        }
        let occupied = replicated.iter().any(|(other, other_transform, _)| {
            matches!(other.kind, DeployableKind::Door { .. })
                && positions_match(other_transform.position, transform.position)
        });
        if occupied {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(current, _, _, _)| distance < *current)
        {
            best = Some((distance, meta.id, transform.position, transform.yaw));
        }
    }
    best.map(|(_, id, position, yaw)| (id, position, yaw))
}
