//! Structural stability: the server-side support graph for building
//! pieces and doors.
//!
//! Every structural piece carries a stability percentage derived from its
//! best path to the ground: foundations are 100, each vertical hop (wall
//! on a platform or on another wall, ceiling on a wall, stairs on a
//! platform) retains [`crate::game_balance::STABILITY_RETENTION_VERTICAL_PCT`],
//! and a ceiling hanging off an adjacent ceiling retains
//! [`crate::game_balance::STABILITY_RETENTION_CEILING_NEIGHBOR_PCT`] per
//! tile of cantilever. The *relations* are defined once in
//! [`crate::building::candidate_stability_pct`]; this module walks them
//! forward (supporter to supported) for the full-world recompute.
//!
//! The recompute runs only on structural change: a placement, a destroy,
//! or world load. It is a max-propagation Dijkstra over the support
//! graph, so each piece is finalised once and the cost is
//! O(pieces x sockets x log pieces). Pieces that come out at exactly 0
//! (their ground path is gone) are destroyed in the same pass, which is
//! what makes knocking out a foundation collapse everything it carried.

use std::collections::{BinaryHeap, HashMap};

use crate::{
    building::{
        BuildingPiece, StabilitySupport, candidate_stability_pct, cell_neighbor_sockets,
        platform_wall_sockets, positions_match, same_wall_plane, stairs_socket_on,
        wall_ceiling_sockets, wall_top_socket,
    },
    game_balance::{
        STABILITY_RETENTION_CEILING_NEIGHBOR_PCT as NEIGHBOR_PCT,
        STABILITY_RETENTION_VERTICAL_PCT as VERTICAL_PCT,
    },
    items::DeployableKind,
    protocol::{DeployedEntityId, Vec3Net},
};

use super::{GameServer, deployables::DeployedEntity};

impl GameServer {
    /// Stability a freshly placed piece would compute right now, from the
    /// current pieces' stored stabilities. Placement rejects below
    /// [`crate::game_balance::BUILDING_MIN_PLACEMENT_STABILITY_PCT`].
    pub(super) fn building_candidate_stability(
        &self,
        piece: BuildingPiece,
        position: Vec3Net,
        yaw: f32,
    ) -> u32 {
        let existing: Vec<StabilitySupport> = self
            .deployed_entities
            .values()
            .filter_map(|entity| {
                let DeployableKind::Building { piece, .. } = entity.kind else {
                    return None;
                };
                Some((
                    piece,
                    entity.position,
                    entity.yaw,
                    u32::from(entity.stability),
                ))
            })
            .collect();
        candidate_stability_pct(piece, position, yaw, &existing)
    }

    /// Recompute every structural piece's stability from scratch and
    /// destroy the pieces whose ground path is gone. Call after any
    /// structural change (place / destroy / load), never per tick.
    ///
    /// One pass suffices: a piece supported only through doomed pieces
    /// already computes 0 (its supporters' 0 propagates), so removing
    /// the zeros can't strand a survivor at a stale value.
    pub(super) fn refresh_structural_stability(&mut self) {
        let computed = compute_stabilities(&self.deployed_entities);
        let mut doomed = Vec::new();
        let mut changed = Vec::new();
        for (id, stability) in &computed {
            if *stability == 0 {
                doomed.push(*id);
                continue;
            }
            let stored = self
                .deployed_entities
                .get(id)
                .map(|entity| u32::from(entity.stability));
            if stored != Some(*stability) {
                changed.push((*id, *stability as u8));
            }
        }
        for (id, stability) in changed {
            // `deployed_entity_mut` flags the id dirty so the mirror
            // ships the `DeployableStability` diff.
            if let Some(entity) = self.deployed_entity_mut(id) {
                entity.stability = stability;
            }
        }
        for id in doomed {
            self.remove_deployed_entity_tracked(id);
        }

        // Free deployables (furnaces, beds, boxes) can stand on platform
        // tops; when the floor under one just collapsed it can't float.
        // Sweep elevated free-standing deployables whose surface is gone,
        // spilling container contents as loot bags.
        let orphaned: Vec<DeployedEntityId> = self
            .deployed_entities
            .values()
            .filter(|entity| {
                !matches!(
                    entity.kind,
                    DeployableKind::Building { .. } | DeployableKind::Door
                )
            })
            .filter(|entity| entity.position.y > 0.25)
            .filter(|entity| !self.valid_deployable_surface(entity.position))
            .map(|entity| entity.id)
            .collect();
        for id in orphaned {
            if let Some(removed) = self.remove_deployed_entity_tracked(id) {
                self.spill_container_contents(removed);
            }
        }
    }
}

/// Quantised spatial key for socket-position lookups. Grid positions are
/// snapped server-side, so identical points are bit-identical or within
/// float error; lookups probe the neighbouring keys and verify with
/// [`positions_match`], so the bucket size only has to be coarser than
/// the float error and finer than the 1.5 m socket spacing.
fn position_key(position: Vec3Net) -> (i64, i64, i64) {
    (
        (position.x * 10.0).round() as i64,
        (position.y * 10.0).round() as i64,
        (position.z * 10.0).round() as i64,
    )
}

struct StructureNode {
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
}

/// Max-propagation over the support graph. Returns the stability of
/// every structural entity (building pieces and doors); free-standing
/// deployables are not in the map.
fn compute_stabilities(
    entities: &HashMap<DeployedEntityId, DeployedEntity>,
) -> HashMap<DeployedEntityId, u32> {
    let mut stability: HashMap<DeployedEntityId, u32> = HashMap::new();
    let mut nodes: HashMap<DeployedEntityId, StructureNode> = HashMap::new();
    let mut by_position: HashMap<(i64, i64, i64), Vec<DeployedEntityId>> = HashMap::new();
    let mut doors_by_parent: HashMap<DeployedEntityId, Vec<DeployedEntityId>> = HashMap::new();
    let mut heap: BinaryHeap<(u32, DeployedEntityId)> = BinaryHeap::new();

    for entity in entities.values() {
        match entity.kind {
            DeployableKind::Building { piece, .. } => {
                let start = if matches!(piece, BuildingPiece::Foundation) {
                    100
                } else {
                    0
                };
                stability.insert(entity.id, start);
                if start > 0 {
                    heap.push((start, entity.id));
                }
                by_position
                    .entry(position_key(entity.position))
                    .or_default()
                    .push(entity.id);
                nodes.insert(
                    entity.id,
                    StructureNode {
                        piece,
                        position: entity.position,
                        yaw: entity.yaw,
                    },
                );
            }
            DeployableKind::Door => {
                stability.insert(entity.id, 0);
                if let Some(door) = &entity.door {
                    doors_by_parent
                        .entry(door.parent)
                        .or_default()
                        .push(entity.id);
                }
            }
            _ => {}
        }
    }

    // Pieces whose base sits at `position`, probing the neighbouring
    // quantisation buckets so float error can't split a socket match.
    let pieces_at = |position: Vec3Net| -> Vec<DeployedEntityId> {
        let (kx, ky, kz) = position_key(position);
        let mut found = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(ids) = by_position.get(&(kx + dx, ky + dy, kz + dz)) else {
                        continue;
                    };
                    for id in ids {
                        if nodes
                            .get(id)
                            .is_some_and(|node| positions_match(node.position, position))
                        {
                            found.push(*id);
                        }
                    }
                }
            }
        }
        found
    };

    while let Some((value, id)) = heap.pop() {
        // Stale heap entry: a better path already finalised this node.
        if stability.get(&id).copied().unwrap_or(0) > value {
            continue;
        }
        let Some(node) = nodes.get(&id) else {
            // Doors support nothing further.
            continue;
        };
        let mut relaxations: Vec<(DeployedEntityId, u32)> = Vec::new();

        // Platform: walls on the edge sockets, stairs on the cell.
        if let Some(sockets) = platform_wall_sockets(node.piece, node.position, node.yaw) {
            for socket in sockets {
                for target in pieces_at(socket.position) {
                    if nodes[&target].piece.is_wall_like() {
                        relaxations.push((target, VERTICAL_PCT));
                    }
                }
            }
        }
        if let Some(socket) = stairs_socket_on(node.piece, node.position, 0.0) {
            for target in pieces_at(socket.position) {
                if matches!(nodes[&target].piece, BuildingPiece::Stairs) {
                    relaxations.push((target, VERTICAL_PCT));
                }
            }
        }
        // Wall-like: the wall stacked directly above (same plane) and
        // the ceilings carried on either side of the top edge.
        if let Some(top) = wall_top_socket(node.piece, node.position, node.yaw) {
            for target in pieces_at(top.position) {
                if nodes[&target].piece.is_wall_like()
                    && same_wall_plane(nodes[&target].yaw, node.yaw)
                {
                    relaxations.push((target, VERTICAL_PCT));
                }
            }
        }
        if let Some(cells) = wall_ceiling_sockets(node.piece, node.position, node.yaw) {
            for cell in cells {
                for target in pieces_at(cell.position) {
                    if matches!(nodes[&target].piece, BuildingPiece::Ceiling) {
                        relaxations.push((target, VERTICAL_PCT));
                    }
                }
            }
        }
        // Ceiling: cantilevered neighbours.
        if matches!(node.piece, BuildingPiece::Ceiling) {
            for socket in cell_neighbor_sockets(node.position, node.yaw) {
                for target in pieces_at(socket.position) {
                    if matches!(nodes[&target].piece, BuildingPiece::Ceiling) {
                        relaxations.push((target, NEIGHBOR_PCT));
                    }
                }
            }
        }
        // Doorway: the mounted door rides at full retention.
        if matches!(node.piece, BuildingPiece::Doorway)
            && let Some(doors) = doors_by_parent.get(&id)
        {
            for door in doors {
                relaxations.push((*door, 100));
            }
        }

        for (target, retention) in relaxations {
            let propagated = value * retention / 100;
            let entry = stability.entry(target).or_insert(0);
            if propagated > *entry {
                *entry = propagated;
                heap.push((propagated, target));
            }
        }
    }

    stability
}
