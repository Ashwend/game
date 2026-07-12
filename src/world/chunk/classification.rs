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

/// Per-channel weights applied when deciding a chunk's **biome label** and
/// its **ground texture** (see [`ClassificationChannels::biased`]), so a
/// world leans toward forest and meadow with rock + ore as the minority.
/// Without this the four channels are i.i.d. noise, an even ~25%-each split
/// that on an unlucky seed leaves large stretches classified rocky/ore, i.e.
/// barren. The forest/plains channels are weighted up and the rocky/ore
/// channels down so the green biomes claim more of the map.
///
/// Crucially this biases only the *label* (which `base_capacity` row a chunk
/// uses) and the ground splat, NOT the per-kind density: `channel_for` still
/// reads the raw, unweighted channels, so a forest chunk keeps its tuned tree
/// count and an ore vein its tuned ore count, there are simply more forest /
/// plains chunks and fewer rocky / ore ones.
///
/// Classification is recomputed from the seed on every load (it isn't saved),
/// so this re-labels *existing* worlds too. New worlds are fully consistent
/// (generation and load both classify the same way). For a world generated
/// before this bias existed, a chunk that flips away from ore/rocky loses that
/// kind's capacity, so already-placed ore there stops respawning once mined,
/// start a fresh world for the clean result. (Persisting the per-chunk
/// classification in the save would make any future tuning forward-safe.)
const BIOME_BIAS: ClassificationChannels = ClassificationChannels {
    forest: 1.19,
    stone: 0.92,
    ore: 0.89,
    hay: 1.08,
};

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

    /// Channels scaled by [`BIOME_BIAS`]. Used for the biome-label
    /// ([`Self::classify`]) and ground-texture decisions so the map leans
    /// green; capacity scaling deliberately keeps using the raw channels.
    pub fn biased(self) -> Self {
        Self {
            forest: self.forest * BIOME_BIAS.forest,
            stone: self.stone * BIOME_BIAS.stone,
            ore: self.ore * BIOME_BIAS.ore,
            hay: self.hay * BIOME_BIAS.hay,
        }
    }

    /// Pick the classification whose (biased) channel is largest. If no
    /// channel clears the threshold, the chunk is `Mixed`.
    pub fn classify(self) -> ChunkClassification {
        let b = self.biased();
        let candidates = [
            (b.forest, ChunkClassification::Forest),
            (b.stone, ChunkClassification::RockyOutcrop),
            (b.ore, ChunkClassification::OreVein),
            (b.hay, ChunkClassification::Plains),
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
            // Meteorite rides the ore channel: it only ever spawns in the rich
            // ore/rocky biomes (gated further by classification + distance in
            // `chunk_kind_target`), so tracking the ore intensity keeps it in the
            // same veins as the other minerals.
            NodeKind::CoalOre | NodeKind::IronOre | NodeKind::SulfurOre | NodeKind::Meteorite => {
                self.ore
            }
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
        (Forest, Meteorite) => 0,

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
        // Meteorite: base 1, so at most one per eligible chunk. The strict
        // distance-ring + noise-mask gate in `chunk_kind_target` makes most
        // eligible chunks hold none (roughly an order of magnitude rarer than
        // iron), so this is a ceiling, not a per-chunk guarantee.
        (RockyOutcrop, Meteorite) => 1,

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
        // Meteorite: base 1 (the ore-vein biome is the other eligible one);
        // same strict distance + noise gate as rocky, so most ore-vein chunks
        // still hold none.
        (OreVein, Meteorite) => 1,

        (Plains, TreeSmall) => 2,
        (Plains, TreeMedium) => 1,
        (Plains, TreeLarge) => 0,
        (Plains, BranchPile) => 6,
        (Plains, SurfaceStone) => 3,
        (Plains, HayGrass) => 28,
        (Plains, CoalOre | IronOre | SulfurOre) => 0,
        (Plains, StoneVein) => 1,
        (Plains, Meteorite) => 0,

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
        // Meteorite is deliberately barren-biome-only (rocky/ore); Mixed
        // transition cells never seed it, keeping it a reward for pushing into
        // the committed rocky/ore regions.
        (Mixed, Meteorite) => 0,
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

    /// Sample a big multi-seed grid and report the biome split. Forest +
    /// Plains + Mixed (the green/lively biomes) should be the clear majority,
    /// while rock and ore stay present (a base still needs stone and ore).
    #[test]
    fn biome_bias_favours_green_but_keeps_rock_and_ore() {
        use std::collections::HashMap;
        let is_green = |c: ChunkClassification| {
            matches!(
                c,
                ChunkClassification::Forest
                    | ChunkClassification::Plains
                    | ChunkClassification::Mixed
            )
        };
        let mut counts: HashMap<ChunkClassification, u32> = HashMap::new();
        let mut total = 0u32;
        let mut worst_green = 100.0_f32;
        for seed in 0..60u64 {
            let (mut seed_green, mut seed_total) = (0u32, 0u32);
            for x in -25..25 {
                for z in -25..25 {
                    let c = ClassificationChannels::sample(seed, ChunkCoord::new(x, z)).classify();
                    *counts.entry(c).or_default() += 1;
                    total += 1;
                    seed_total += 1;
                    if is_green(c) {
                        seed_green += 1;
                    }
                }
            }
            worst_green = worst_green.min(100.0 * seed_green as f32 / seed_total as f32);
        }
        let pct =
            |c: ChunkClassification| 100.0 * *counts.get(&c).unwrap_or(&0) as f32 / total as f32;
        let (forest, plains, mixed, rocky, ore) = (
            pct(ChunkClassification::Forest),
            pct(ChunkClassification::Plains),
            pct(ChunkClassification::Mixed),
            pct(ChunkClassification::RockyOutcrop),
            pct(ChunkClassification::OreVein),
        );
        println!(
            "biome split (avg): forest {forest:.1}%  plains {plains:.1}%  mixed {mixed:.1}%  rocky {rocky:.1}%  ore {ore:.1}%  | worst-seed green {worst_green:.1}%"
        );
        let green = forest + plains + mixed;
        // Green leads clearly on average...
        assert!(
            green > 60.0,
            "green biomes should dominate, got {green:.1}%"
        );
        // ...and even the unluckiest seed isn't a barren wasteland.
        assert!(
            worst_green > 50.0,
            "worst seed should still be majority green, got {worst_green:.1}%"
        );
        // But rock and ore stay reachable for progression.
        assert!(ore > 8.0, "ore must stay reachable, got {ore:.1}%");
        assert!(rocky > 8.0, "rock must stay reachable, got {rocky:.1}%");
    }

    #[test]
    fn ore_capacity_is_zero_in_pure_forest() {
        for ore in [NodeKind::CoalOre, NodeKind::IronOre, NodeKind::SulfurOre] {
            assert_eq!(base_capacity(ChunkClassification::Forest, ore), 0);
        }
    }
}
