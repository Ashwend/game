use std::collections::HashMap;

use crate::{
    protocol::Vec3Net,
    world::{WorldBlock, WorldData},
};

use super::{PLAYER_HEIGHT, PLAYER_RADIUS};

/// Edge length of one chunk cell, in world units. Chosen to be a few times
/// larger than the player radius so a typical player query touches at most
/// four cells. Increase if blocks tend to be much larger than this; decrease
/// for very dense fields of small blocks.
const CELL_SIZE: f32 = 4.0;

/// Uniform spatial hash over collidable AABBs. The grid owns its own block
/// list so it can mix static `WorldData::blocks` with dynamic colliders
/// (e.g. live tree trunks from the current snapshot). Cells are keyed by
/// `(cell_x, cell_z)` in the horizontal plane, vertical extent is unbounded
/// per cell because the world is mostly "flat with stuff on top." Built or
/// rebuilt by the runtime; consulted by the collision routines.
#[derive(Debug, Clone, Default)]
pub struct BlockGrid {
    blocks: Vec<WorldBlock>,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl BlockGrid {
    pub fn build(world: &WorldData) -> Self {
        Self::build_with_extras(world, &[])
    }

    /// Same as [`build`] but mixes additional collider blocks into the chunk.
    /// Use this on the client to compose static world geometry with
    /// per-frame dynamic colliders such as live tree trunks.
    pub fn build_with_extras(world: &WorldData, extras: &[WorldBlock]) -> Self {
        let mut blocks = Vec::with_capacity(world.blocks.len() + extras.len());
        blocks.extend_from_slice(&world.blocks);
        blocks.extend_from_slice(extras);

        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (index, block) in blocks.iter().enumerate() {
            let min = block.min();
            let max = block.max();
            let (cell_min_x, cell_min_z) = cell_for(min.x, min.z);
            let (cell_max_x, cell_max_z) = cell_for(max.x, max.z);
            for cell_x in cell_min_x..=cell_max_x {
                for cell_z in cell_min_z..=cell_max_z {
                    cells
                        .entry((cell_x, cell_z))
                        .or_default()
                        .push(index as u32);
                }
            }
        }
        Self { blocks, cells }
    }

    /// Returns the block stored at `index`. Collision routines read through
    /// this rather than indexing into `WorldData::blocks` directly so they
    /// transparently see both static blocks and dynamic extras.
    pub fn block(&self, index: usize) -> WorldBlock {
        self.blocks[index]
    }

    /// Yields candidate block indices that could overlap a player AABB
    /// centered at `position`. May include some false positives, callers
    /// still perform the precise per-block test.
    pub fn candidates_for_player(&self, position: Vec3Net) -> Candidates<'_> {
        let min_x = position.x - PLAYER_RADIUS;
        let max_x = position.x + PLAYER_RADIUS;
        let min_z = position.z - PLAYER_RADIUS;
        let max_z = position.z + PLAYER_RADIUS;
        Candidates::new(self, min_x, min_z, max_x, max_z)
    }

    /// Yields candidates for a swept query that grows the player AABB along
    /// the given horizontal axis by `delta`. Sweep distances larger than a
    /// cell still resolve correctly because the iterator visits every cell
    /// in the swept range.
    pub fn candidates_for_swept(
        &self,
        position: Vec3Net,
        delta_x: f32,
        delta_z: f32,
    ) -> Candidates<'_> {
        let min_x = (position.x - PLAYER_RADIUS).min(position.x + delta_x - PLAYER_RADIUS);
        let max_x = (position.x + PLAYER_RADIUS).max(position.x + delta_x + PLAYER_RADIUS);
        let min_z = (position.z - PLAYER_RADIUS).min(position.z + delta_z - PLAYER_RADIUS);
        let max_z = (position.z + PLAYER_RADIUS).max(position.z + delta_z + PLAYER_RADIUS);
        Candidates::new(self, min_x, min_z, max_x, max_z)
    }

    /// Yields candidates for a vertical query, the horizontal footprint
    /// stays at the player AABB, the caller checks the Y axis itself.
    pub fn candidates_for_vertical(&self, position: Vec3Net) -> Candidates<'_> {
        let _ = PLAYER_HEIGHT; // documented dependency; vertical filtering happens in the per-block test.
        self.candidates_for_player(position)
    }
}

pub struct Candidates<'a> {
    grid: &'a BlockGrid,
    cell_min_z: i32,
    cell_max_x: i32,
    cell_max_z: i32,
    cursor_x: i32,
    cursor_z: i32,
    current: Option<std::slice::Iter<'a, u32>>,
    seen: smallvec_seen::SeenSet,
}

impl<'a> Candidates<'a> {
    fn new(grid: &'a BlockGrid, min_x: f32, min_z: f32, max_x: f32, max_z: f32) -> Self {
        let (cell_min_x, cell_min_z) = cell_for(min_x, min_z);
        let (cell_max_x, cell_max_z) = cell_for(max_x, max_z);
        Self {
            grid,
            cell_min_z,
            cell_max_x,
            cell_max_z,
            cursor_x: cell_min_x,
            cursor_z: cell_min_z,
            current: None,
            seen: smallvec_seen::SeenSet::default(),
        }
    }
}

impl Iterator for Candidates<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(iter) = self.current.as_mut() {
                for &index in iter.by_ref() {
                    if self.seen.insert(index) {
                        return Some(index as usize);
                    }
                }
                self.current = None;
            }

            if self.cursor_x > self.cell_max_x {
                return None;
            }

            let cell = self.grid.cells.get(&(self.cursor_x, self.cursor_z));

            self.cursor_z += 1;
            if self.cursor_z > self.cell_max_z {
                self.cursor_z = self.cell_min_z;
                self.cursor_x += 1;
            }

            if let Some(indices) = cell {
                self.current = Some(indices.iter());
            }
        }
    }
}

fn cell_for(x: f32, z: f32) -> (i32, i32) {
    (
        (x / CELL_SIZE).floor() as i32,
        (z / CELL_SIZE).floor() as i32,
    )
}

/// Tiny dedup helper. A query touches at most a handful of cells; the same
/// block can appear in several of them when it straddles cell boundaries. We
/// want to yield each candidate once. A heap-free linear-scan set is faster
/// than `HashSet` at this size and avoids per-query allocation.
mod smallvec_seen {
    const STACK_CAP: usize = 16;

    #[derive(Default)]
    pub(super) struct SeenSet {
        stack: [u32; STACK_CAP],
        stack_len: usize,
        heap: Vec<u32>,
    }

    impl SeenSet {
        pub(super) fn insert(&mut self, value: u32) -> bool {
            if self.stack[..self.stack_len].contains(&value) || self.heap.contains(&value) {
                return false;
            }
            if self.stack_len < STACK_CAP {
                self.stack[self.stack_len] = value;
                self.stack_len += 1;
            } else {
                self.heap.push(value);
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WorldBlock;

    fn block(center: Vec3Net, half: Vec3Net) -> WorldBlock {
        WorldBlock::new(center, half)
    }

    fn world_with(blocks: Vec<WorldBlock>) -> WorldData {
        WorldData {
            floor_size: 64.0,
            blocks,
            resource_nodes: Vec::new(),
        }
    }

    #[test]
    fn grid_returns_block_for_overlapping_position() {
        let world = world_with(vec![
            block(Vec3Net::new(10.0, 0.5, 10.0), Vec3Net::new(0.5, 0.5, 0.5)),
            block(Vec3Net::new(-20.0, 0.5, -20.0), Vec3Net::new(0.5, 0.5, 0.5)),
        ]);
        let grid = BlockGrid::build(&world);

        let candidates = grid
            .candidates_for_player(Vec3Net::new(10.0, 0.0, 10.0))
            .collect::<Vec<_>>();
        assert_eq!(candidates, vec![0]);

        let far_candidates = grid
            .candidates_for_player(Vec3Net::new(50.0, 0.0, 50.0))
            .collect::<Vec<_>>();
        assert!(far_candidates.is_empty());
    }

    #[test]
    fn grid_visits_blocks_along_a_sweep() {
        let world = world_with(vec![
            block(Vec3Net::new(2.0, 0.5, 0.0), Vec3Net::new(0.5, 0.5, 0.5)),
            block(Vec3Net::new(6.0, 0.5, 0.0), Vec3Net::new(0.5, 0.5, 0.5)),
            block(Vec3Net::new(10.0, 0.5, 0.0), Vec3Net::new(0.5, 0.5, 0.5)),
        ]);
        let grid = BlockGrid::build(&world);

        let mut seen = grid
            .candidates_for_swept(Vec3Net::new(0.0, 0.0, 0.0), 12.0, 0.0)
            .collect::<Vec<_>>();
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2]);
    }

    #[test]
    fn grid_deduplicates_a_block_that_straddles_cell_boundaries() {
        // A wide block centered on a cell boundary will be registered in
        // multiple cells but should only surface once per query.
        let world = world_with(vec![block(
            Vec3Net::new(0.0, 0.5, 0.0),
            Vec3Net::new(3.0, 0.5, 3.0),
        )]);
        let grid = BlockGrid::build(&world);

        let seen = grid
            .candidates_for_player(Vec3Net::new(0.0, 0.0, 0.0))
            .collect::<Vec<_>>();
        assert_eq!(seen, vec![0]);
    }
}
