//! Tool Cupboard claim geometry: the 3 m claim cell grid, the point and
//! footprint overlap tests, and the foundation-projected claim flood fill.
//! Shared by the server's authoritative gate and the client's placement
//! ghost so both agree on exactly which cells are claimed.

use crate::{protocol::Vec3Net, world::WorldBlock};

use super::FOUNDATION_SIZE_M;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{BuildingPiece, FOUNDATION_HEIGHT_M, building_collider_blocks};

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
}
