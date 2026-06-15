//! Chunk classification, pure functions that decide what a chunk "is"
//! (forest, ore vein, plains, rocky outcrop, mixed) from a small set of
//! seeded noise channels, and how many of each resource node kind a chunk
//! of that classification should hold.
//!
//! The classification is the *dominant* density channel sampled at the
//! grid's centre. Sub-dominant channels still contribute to the capacity
//! table at a reduced weight, so a "Forest" grid can still scatter a
//! handful of stones and an ore vein, but the dominant channel sets the
//! visual identity.

use serde::{Deserialize, Serialize};

use super::{
    CHUNK_SIZE_M, ChunkCoord, NodeKind,
    noise::{fbm, splitmix64},
};

/// Number of noise octaves for the classification fields. Kept low (2) on
/// purpose: extra octaves pile sub-chunk detail onto the channels, and since
/// the classification is an argmax of four channels, that detail makes the
/// winner flip on a tiny scale, which reads as confetti-speckle rather than
/// coherent biomes. Two octaves keeps an organic edge without the speckle.
const CLASSIFICATION_FBM_OCTAVES: u32 = 2;

/// Base feature scale for the classification channels. Smaller frequency =
/// larger features. At `1/600` the channels span ~9-10 chunks (~600 m), so a
/// biome reads as a sizable region you walk *through* rather than a cluster of
/// tiny single-chunk patches. (Was `1/220`, which fragmented the map into
/// confetti once you saw more than a few chunks of it.)
const CLASSIFICATION_BASE_FREQUENCY: f32 = 1.0 / 600.0;

/// Floor a channel must clear to count toward the classification.
/// Channels below this contribute very little, they may still seed a few
/// scatter nodes but won't push the classification toward their kind.
const CLASSIFICATION_THRESHOLD: f32 = 0.42;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkClassification {
    /// Tree-dense biome. High tree capacity, some branches and grass.
    Forest,
    /// Stone outcrop. High surface-stone capacity, occasional ore lump.
    RockyOutcrop,
    /// Concentrated ore deposit. Coal/iron/sulfur in clusters, sparse trees.
    OreVein,
    /// Grass and open ground. High hay/grass capacity, scattered branches.
    Plains,
    /// Roughly balanced, no single channel dominates. A transition cell.
    #[default]
    Mixed,
}

impl ChunkClassification {
    pub const ALL: [Self; 5] = [
        Self::Forest,
        Self::RockyOutcrop,
        Self::OreVein,
        Self::Plains,
        Self::Mixed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Forest => "Forest",
            Self::RockyOutcrop => "Rocky outcrop",
            Self::OreVein => "Ore vein",
            Self::Plains => "Plains",
            Self::Mixed => "Mixed",
        }
    }
}

/// Per-classification channel samples, the raw blended-noise values that
/// drove the classification. Kept around so the generator can scale
/// sub-dominant kinds' capacity by `(channel × weight)` instead of using
/// fixed numbers, which would make the boundaries between classifications
/// feel artificial.
#[derive(Debug, Clone, Copy)]
pub struct ClassificationChannels {
    pub forest: f32,
    pub stone: f32,
    pub ore: f32,
    pub hay: f32,
}

impl ClassificationChannels {
    /// Sample all four channels at the chunk centre. Each channel has its
    /// own offset folded into the seed so they don't share a noise
    /// pattern, otherwise the dominant channel would always win in the
    /// same way across the map.
    pub fn sample(world_seed: u64, coord: ChunkCoord) -> Self {
        let centre_x = coord.x as f32 * CHUNK_SIZE_M + CHUNK_SIZE_M * 0.5;
        let centre_z = coord.z as f32 * CHUNK_SIZE_M + CHUNK_SIZE_M * 0.5;
        Self::sample_at(world_seed, centre_x, centre_z)
    }

    /// Sample all four channels at an arbitrary world point (not just a
    /// chunk centre). The world-map raster uses this to read smooth biome
    /// gradients at texel resolution; [`Self::sample`] is the per-chunk
    /// special case. Keeping the per-channel seed offsets here means the
    /// map and the live generation always agree on where each biome sits.
    pub fn sample_at(world_seed: u64, x: f32, z: f32) -> Self {
        Self {
            forest: fbm(
                splitmix64(world_seed ^ 0xF01A_BE57_u64),
                x,
                z,
                CLASSIFICATION_BASE_FREQUENCY,
                CLASSIFICATION_FBM_OCTAVES,
            ),
            stone: fbm(
                splitmix64(world_seed ^ 0x5703_A6E5_u64),
                x,
                z,
                CLASSIFICATION_BASE_FREQUENCY,
                CLASSIFICATION_FBM_OCTAVES,
            ),
            ore: fbm(
                splitmix64(world_seed ^ 0x0A5E_4D11_u64),
                x,
                z,
                CLASSIFICATION_BASE_FREQUENCY,
                CLASSIFICATION_FBM_OCTAVES,
            ),
            hay: fbm(
                splitmix64(world_seed ^ 0xA44C_7321_u64),
                x,
                z,
                CLASSIFICATION_BASE_FREQUENCY,
                CLASSIFICATION_FBM_OCTAVES,
            ),
        }
    }

    /// Pick the classification whose channel is largest. If no channel
    /// clears the threshold, the chunk is `Mixed`.
    pub fn classify(self) -> ChunkClassification {
        let candidates = [
            (self.forest, ChunkClassification::Forest),
            (self.stone, ChunkClassification::RockyOutcrop),
            (self.ore, ChunkClassification::OreVein),
            (self.hay, ChunkClassification::Plains),
        ];
        let (peak, choice) = candidates.iter().copied().fold(
            (0.0_f32, ChunkClassification::Mixed),
            |(best, kind), (v, k)| {
                if v > best { (v, k) } else { (best, kind) }
            },
        );
        if peak < CLASSIFICATION_THRESHOLD {
            ChunkClassification::Mixed
        } else {
            choice
        }
    }

    /// Channel value for a given node kind. Used by the generator's
    /// capacity scaling so each kind's count tracks its channel intensity,
    /// not just the chunk's discrete label.
    pub fn channel_for(&self, kind: NodeKind) -> f32 {
        match kind {
            NodeKind::TreeSmall | NodeKind::TreeMedium | NodeKind::TreeLarge => self.forest,
            // Stone vein follows the same rocky channel as the small
            // surface lumps, wherever the ground is stony, both spawn.
            NodeKind::SurfaceStone | NodeKind::StoneVein => self.stone,
            NodeKind::CoalOre | NodeKind::IronOre | NodeKind::SulfurOre => self.ore,
            NodeKind::HayGrass => self.hay,
            // Branches are a fallout of trees + plains, they show up where
            // forests and meadows are present. Take the max so a forest
            // edge still has plenty of branches.
            NodeKind::BranchPile => self.forest.max(self.hay),
        }
    }
}

/// Maximum number of nodes of a given kind a "pure" example of a
/// classification should sustain. Multiplied by the channel intensity at
/// generation time, so a strong forest chunk sits near these numbers while
/// a weak one comes in well under.
///
/// Picked to feel reasonable on a 64 m × 64 m grid (≈4096 m²): a dense
/// forest chunk carrying 14 trees averages one tree per ~290 m², roughly
/// matches the hand-placed test world's tree density.
pub fn base_capacity(classification: ChunkClassification, kind: NodeKind) -> u16 {
    use ChunkClassification::*;
    use NodeKind::*;
    match (classification, kind) {
        (Forest, TreeSmall) => 4,
        (Forest, TreeMedium) => 8,
        (Forest, TreeLarge) => 3,
        (Forest, BranchPile) => 10,
        (Forest, SurfaceStone) => 2,
        (Forest, HayGrass) => 12,
        (Forest, CoalOre | IronOre | SulfurOre) => 0,
        (Forest, StoneVein) => 0,

        (RockyOutcrop, TreeSmall) => 1,
        (RockyOutcrop, TreeMedium) => 1,
        (RockyOutcrop, TreeLarge) => 0,
        (RockyOutcrop, BranchPile) => 3,
        (RockyOutcrop, SurfaceStone) => 14,
        (RockyOutcrop, HayGrass) => 2,
        (RockyOutcrop, CoalOre) => 1,
        (RockyOutcrop, IronOre) => 1,
        (RockyOutcrop, SulfurOre) => 0,
        // The headline rock vein for rocky chunks, the player should be
        // able to walk into one of these and gather stone in earnest.
        (RockyOutcrop, StoneVein) => 4,

        (OreVein, TreeSmall) => 0,
        (OreVein, TreeMedium) => 1,
        (OreVein, TreeLarge) => 0,
        (OreVein, BranchPile) => 2,
        (OreVein, SurfaceStone) => 6,
        (OreVein, HayGrass) => 1,
        (OreVein, CoalOre) => 3,
        (OreVein, IronOre) => 3,
        (OreVein, SulfurOre) => 2,
        // Plain rock alongside the ore, visually grounds the ore-vein
        // chunk as a bigger rocky region.
        (OreVein, StoneVein) => 2,

        (Plains, TreeSmall) => 2,
        (Plains, TreeMedium) => 1,
        (Plains, TreeLarge) => 0,
        (Plains, BranchPile) => 6,
        (Plains, SurfaceStone) => 3,
        (Plains, HayGrass) => 28,
        (Plains, CoalOre | IronOre | SulfurOre) => 0,
        (Plains, StoneVein) => 1,

        (Mixed, TreeSmall) => 2,
        (Mixed, TreeMedium) => 3,
        (Mixed, TreeLarge) => 1,
        (Mixed, BranchPile) => 5,
        (Mixed, SurfaceStone) => 4,
        (Mixed, HayGrass) => 8,
        (Mixed, CoalOre) => 1,
        (Mixed, IronOre) => 1,
        (Mixed, SulfurOre) => 0,
        (Mixed, StoneVein) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_is_deterministic_per_seed() {
        let a = ClassificationChannels::sample(123, ChunkCoord::new(2, -1)).classify();
        let b = ClassificationChannels::sample(123, ChunkCoord::new(2, -1)).classify();
        assert_eq!(a, b);
    }

    #[test]
    fn classification_varies_across_chunk() {
        let mut seen = std::collections::HashSet::new();
        // Sample a wide window (~1.3 km). Biome features now span ~600 m, so a
        // handful of chunks can sit inside a single region; a wider sweep is
        // what verifies the noise produces variety rather than collapsing.
        for x in -10..=10 {
            for z in -10..=10 {
                let c = ClassificationChannels::sample(42, ChunkCoord::new(x, z)).classify();
                seen.insert(c);
            }
        }
        assert!(
            seen.len() >= 3,
            "expected >=3 classifications across the window, saw: {seen:?}"
        );
    }

    #[test]
    fn channel_for_kind_routes_to_correct_field() {
        let channels = ClassificationChannels {
            forest: 0.1,
            stone: 0.2,
            ore: 0.3,
            hay: 0.4,
        };
        assert_eq!(channels.channel_for(NodeKind::TreeMedium), 0.1);
        assert_eq!(channels.channel_for(NodeKind::SurfaceStone), 0.2);
        assert_eq!(channels.channel_for(NodeKind::CoalOre), 0.3);
        assert_eq!(channels.channel_for(NodeKind::HayGrass), 0.4);
        // Branches use max(forest, hay).
        assert_eq!(channels.channel_for(NodeKind::BranchPile), 0.4);
    }

    #[test]
    fn ore_capacity_is_zero_in_pure_forest() {
        for ore in [NodeKind::CoalOre, NodeKind::IronOre, NodeKind::SulfurOre] {
            assert_eq!(base_capacity(ChunkClassification::Forest, ore), 0);
        }
    }
}
