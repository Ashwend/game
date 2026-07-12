//! Grid-based world generation.
//!
//! The world is partitioned into 64 m × 64 m grids. Each grid is
//! independently:
//!
//! 1. **Classified**, `forest`, `rocky outcrop`, `ore vein`, `plains`, or
//!    `mixed`, by sampling four seeded noise channels at its centre.
//! 2. **Populated**, Poisson-disk rejection sampling places resource
//!    nodes inside the chunk, with per-kind counts scaled from the
//!    classification's base capacity table by the local channel intensity.
//!
//! Both passes are pure functions of `(world_seed, grid_coord)`, so the
//! same world generates identically every load. The server records grid
//! state plus harvested nodes in the world save; the noise pipeline itself
//! does not need to be persisted.
//!
//! The grid pipeline replaces the hand-authored test layout, see
//! [`super::WorldData::chunk_world`].

mod classification;
mod generator;
mod noise;

pub use classification::{ChunkClassification, ClassificationChannels, base_capacity};
pub use generator::{
    ChunkSpawn, PlayableBounds, build_world_blocks, chunk_center_distance_fraction,
    chunk_kind_target, generate_chunk_spawns, generate_world_spawns, kind_target,
};
pub use noise::{ChunkRng, fbm, splitmix64, value_noise_2d};

use serde::{Deserialize, Serialize};

/// Side length of one chunk cell in metres.
///
/// Picked to give players ~9 active grids around them at the medium view
/// tier (≈192 m playable radius), big enough that a single grid sustains a
/// believable cluster of trees or ore without feeling like a postage
/// stamp.
pub const CHUNK_SIZE_M: f32 = 64.0;

/// Integer chunk coordinate. `(0, 0)` covers world-space
/// `[0, CHUNK_SIZE_M) × [0, CHUNK_SIZE_M)`. The test world is centred on the
/// origin, so its 5 × 5 chunks span coords `-2..=2` in both axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkCoord {
    pub x: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// World-space south-west corner of the cell.
    pub fn origin(self) -> (f32, f32) {
        (self.x as f32 * CHUNK_SIZE_M, self.z as f32 * CHUNK_SIZE_M)
    }

    /// World-space centre of the cell.
    pub fn centre(self) -> (f32, f32) {
        let (ox, oz) = self.origin();
        (ox + CHUNK_SIZE_M * 0.5, oz + CHUNK_SIZE_M * 0.5)
    }

    /// Convert a world-space x/z position into the chunk that contains it.
    pub fn from_world(x: f32, z: f32) -> Self {
        Self {
            x: (x / CHUNK_SIZE_M).floor() as i32,
            z: (z / CHUNK_SIZE_M).floor() as i32,
        }
    }
}

/// Square `dims × dims` grid map (always centred on the origin so the
/// playable area straddles `(0, 0)`). Use [`Self::coords`] to enumerate
/// the chunk coordinates the map covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkDims {
    pub dims: u32,
}

impl ChunkDims {
    pub const fn new(dims: u32) -> Self {
        Self { dims }
    }

    pub fn coords(self) -> impl Iterator<Item = ChunkCoord> {
        let half = self.dims as i32 / 2;
        let range = -half..=half - (1 - self.dims as i32 % 2);
        let range_z = range.clone();
        range.flat_map(move |x| range_z.clone().map(move |z| ChunkCoord::new(x, z)))
    }

    pub fn count(self) -> u32 {
        self.dims * self.dims
    }

    /// World-space side length covered by this many grids.
    pub fn world_size_m(self) -> f32 {
        self.dims as f32 * CHUNK_SIZE_M
    }
}

/// Kinds of resource node the chunk pipeline knows how to place. Maps 1:1
/// to entries in `RESOURCE_NODE_DEFINITIONS`, with the small/medium/large
/// tree variants collapsed into three kinds. Listed in a single enum (vs
/// using definition-id strings) so the capacity tables and generator
/// can pattern-match exhaustively at the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    TreeSmall,
    TreeMedium,
    TreeLarge,
    SurfaceStone,
    BranchPile,
    HayGrass,
    CoalOre,
    IronOre,
    SulfurOre,
    StoneVein,
    /// Rare crystal node. Appended LAST so the postcard variant
    /// index of every existing kind is unchanged (old `ChunkManagerSave`
    /// files stay loadable, see docs/worlds-and-saves.md).
    Meteorite,
}

impl NodeKind {
    pub const ALL: [Self; 11] = [
        Self::TreeSmall,
        Self::TreeMedium,
        Self::TreeLarge,
        Self::SurfaceStone,
        Self::BranchPile,
        Self::HayGrass,
        Self::CoalOre,
        Self::IronOre,
        Self::SulfurOre,
        Self::StoneVein,
        Self::Meteorite,
    ];

    /// `definition_id` string used by the `resources` registry.
    pub fn definition_id(self) -> &'static str {
        use crate::resources::{
            BRANCH_PILE_NODE_ID, COAL_NODE_ID, HAY_GRASS_NODE_ID, IRON_NODE_ID, METEORITE_NODE_ID,
            PINE_TREE_LARGE_NODE_ID, PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID, STONE_NODE_ID,
            SULFUR_NODE_ID, SURFACE_STONE_NODE_ID,
        };
        // Trees alternate between pine and birch deterministically inside
        // the generator, at this lookup we'd need the per-spawn pick, so
        // the generator picks the species itself and uses these constants.
        // `definition_id` is just the *default* (pine), callers that need
        // species variation use `tree_variant_id`.
        match self {
            Self::TreeSmall => PINE_TREE_SMALL_NODE_ID,
            Self::TreeMedium => PINE_TREE_NODE_ID,
            Self::TreeLarge => PINE_TREE_LARGE_NODE_ID,
            Self::SurfaceStone => SURFACE_STONE_NODE_ID,
            Self::BranchPile => BRANCH_PILE_NODE_ID,
            Self::HayGrass => HAY_GRASS_NODE_ID,
            Self::CoalOre => COAL_NODE_ID,
            Self::IronOre => IRON_NODE_ID,
            Self::SulfurOre => SULFUR_NODE_ID,
            Self::StoneVein => STONE_NODE_ID,
            Self::Meteorite => METEORITE_NODE_ID,
        }
    }

    /// Reverse of [`Self::definition_id`] / [`Self::variant_definition_id`]:
    /// map any registry `definition_id` (including the birch tree variants)
    /// back to the kind used for chunk membership bookkeeping.
    pub fn from_definition_id(definition_id: &str) -> Option<Self> {
        use crate::resources::{
            BIRCH_TREE_LARGE_NODE_ID, BIRCH_TREE_NODE_ID, BIRCH_TREE_SMALL_NODE_ID,
            BRANCH_PILE_NODE_ID, COAL_NODE_ID, HAY_GRASS_NODE_ID, IRON_NODE_ID, METEORITE_NODE_ID,
            PINE_TREE_LARGE_NODE_ID, PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID, STONE_NODE_ID,
            SULFUR_NODE_ID, SURFACE_STONE_NODE_ID,
        };
        match definition_id {
            PINE_TREE_SMALL_NODE_ID | BIRCH_TREE_SMALL_NODE_ID => Some(Self::TreeSmall),
            PINE_TREE_NODE_ID | BIRCH_TREE_NODE_ID => Some(Self::TreeMedium),
            PINE_TREE_LARGE_NODE_ID | BIRCH_TREE_LARGE_NODE_ID => Some(Self::TreeLarge),
            SURFACE_STONE_NODE_ID => Some(Self::SurfaceStone),
            BRANCH_PILE_NODE_ID => Some(Self::BranchPile),
            HAY_GRASS_NODE_ID => Some(Self::HayGrass),
            COAL_NODE_ID => Some(Self::CoalOre),
            IRON_NODE_ID => Some(Self::IronOre),
            SULFUR_NODE_ID => Some(Self::SulfurOre),
            STONE_NODE_ID => Some(Self::StoneVein),
            METEORITE_NODE_ID => Some(Self::Meteorite),
            _ => None,
        }
    }

    /// Per-spawn species pick for tree kinds, alternating pine/birch
    /// deterministically by an unsigned counter. Non-tree kinds ignore
    /// the counter and return [`Self::definition_id`].
    pub fn variant_definition_id(self, variant_counter: u64) -> &'static str {
        use crate::resources::{
            BIRCH_TREE_LARGE_NODE_ID, BIRCH_TREE_NODE_ID, BIRCH_TREE_SMALL_NODE_ID,
            PINE_TREE_LARGE_NODE_ID, PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID,
        };
        let pine = variant_counter.is_multiple_of(2);
        match (self, pine) {
            (Self::TreeSmall, true) => PINE_TREE_SMALL_NODE_ID,
            (Self::TreeSmall, false) => BIRCH_TREE_SMALL_NODE_ID,
            (Self::TreeMedium, true) => PINE_TREE_NODE_ID,
            (Self::TreeMedium, false) => BIRCH_TREE_NODE_ID,
            (Self::TreeLarge, true) => PINE_TREE_LARGE_NODE_ID,
            (Self::TreeLarge, false) => BIRCH_TREE_LARGE_NODE_ID,
            _ => self.definition_id(),
        }
    }

    /// Minimum spacing between two nodes of this kind, used by the
    /// Poisson-disk sampler. Tuned per kind to match the visible
    /// footprint of the spawned model.
    pub fn min_spacing_m(self) -> f32 {
        match self {
            Self::TreeLarge => 5.5,
            Self::TreeMedium => 4.0,
            Self::TreeSmall => 3.0,
            Self::SurfaceStone => 2.4,
            Self::CoalOre | Self::IronOre | Self::SulfurOre | Self::StoneVein => 3.0,
            // Meteorite is nearly always alone in a chunk anyway (capacity 1),
            // so the spacing only matters against the rare double; keep it wide.
            Self::Meteorite => 4.0,
            Self::BranchPile => 1.6,
            Self::HayGrass => 0.8,
        }
    }
}

/// Stable RNG-stream offset per node kind. Used to seed the Poisson-disk
/// sampler so two kinds in the same chunk sample independent point sets.
pub fn kind_stream(kind: NodeKind) -> u32 {
    match kind {
        NodeKind::TreeSmall => 1,
        NodeKind::TreeMedium => 2,
        NodeKind::TreeLarge => 3,
        NodeKind::SurfaceStone => 4,
        NodeKind::BranchPile => 5,
        NodeKind::HayGrass => 6,
        NodeKind::CoalOre => 7,
        NodeKind::IronOre => 8,
        NodeKind::SulfurOre => 9,
        NodeKind::StoneVein => 10,
        NodeKind::Meteorite => 11,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_dims_iterates_count_coords() {
        let dims = ChunkDims::new(5);
        let coords: Vec<_> = dims.coords().collect();
        assert_eq!(coords.len(), dims.count() as usize);
        // Centre coord is present.
        assert!(coords.contains(&ChunkCoord::new(0, 0)));
        // Extreme coords are present.
        assert!(coords.contains(&ChunkCoord::new(-2, -2)));
        assert!(coords.contains(&ChunkCoord::new(2, 2)));
    }

    #[test]
    fn chunk_coord_world_round_trip() {
        let coord = ChunkCoord::new(-1, 2);
        let (ox, oz) = coord.origin();
        // Cell at (-1, 2) spans x in [-64, 0).
        assert_eq!(ox, -64.0);
        assert_eq!(oz, 128.0);
        assert_eq!(ChunkCoord::from_world(ox, oz), coord);
        assert_eq!(
            ChunkCoord::from_world(ox + CHUNK_SIZE_M - 0.001, oz + 0.5),
            coord
        );
    }

    #[test]
    fn from_world_handles_negative_correctly() {
        // -0.5 m sits in cell x = -1, not 0, make sure we floor, not trunc.
        assert_eq!(ChunkCoord::from_world(-0.5, 0.5), ChunkCoord::new(-1, 0));
    }
}
