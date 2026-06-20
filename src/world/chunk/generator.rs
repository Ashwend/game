//! Grid-level resource node placement.
//!
//! For each chunk the pipeline:
//!
//! 1. Samples the four classification channels at the chunk centre.
//! 2. Resolves a discrete classification (forest/ore/plains/rocky/mixed).
//! 3. For each kind: scales the classification's base capacity by the
//!    channel intensity at the chunk centre, then runs Poisson-disk
//!    rejection sampling to place that many candidates, weighting accept
//!    probability by a per-kind noise mask so the points cluster
//!    naturally instead of looking uniformly scattered.
//!
//! Everything is a pure function of `(world_seed, coord)`. The same world
//! generates identically every load, and a chunk that's never been touched
//! since boot can be regenerated from seed alone.

use crate::{
    protocol::Vec3Net,
    world::{BlockKind, CHUNK_SIZE_M, WorldBlock, WorldResourceNodeSpawn},
};

use super::{
    ChunkClassification, ChunkCoord, ChunkDims, ClassificationChannels, NodeKind, base_capacity,
    kind_stream,
    noise::{ChunkRng, fbm, splitmix64},
};

/// Global multiplier on every kind's per-chunk target count. Bumped to
/// 2.0 because the world read as barren at 1.0, sparse chunks especially
/// felt empty. Spacing rules still cap how dense a single kind can get, so
/// dense chunks fill in less than 2× while sparse ones gain the most.
const DENSITY_MULTIPLIER: f32 = 2.0;
/// Octaves for the per-kind density mask used during rejection sampling.
const KIND_MASK_OCTAVES: u32 = 3;
/// Feature scale of the per-kind density mask. Smaller than the
/// classification frequency so clusters happen at the sub-grid level,
/// i.e. trees bunch up in one corner of a forest chunk, leaving the other
/// side sparse.
const KIND_MASK_FREQUENCY: f32 = 1.0 / 28.0;
/// Maximum candidate positions tried per node before giving up. Tuned so
/// the worst case (a fully populated grid trying to squeeze in one more
/// node) doesn't dominate generation time, at this point we'd rather
/// undershoot the capacity than burn cycles searching.
const MAX_CANDIDATES_PER_NODE: u32 = 18;
/// Margin from the chunk edge so neighbouring grids don't double-spawn
/// against each other when a node lands right on the boundary.
const EDGE_MARGIN_M: f32 = 0.5;
/// Minimum distance between any two nodes regardless of kind. Loose
/// enough that grass can grow right next to a stick or stone, the
/// crude clutter is small and walk-through, so visual overlap reads as
/// "ground variety" rather than as broken placement.
const CROSS_KIND_MIN_SPACING_M: f32 = 0.7;

/// Per-chunk target count for one kind: scale its classification base
/// capacity by the channel intensity, then by the global density knob.
/// Lives here so the generator and the regrow capacity table in
/// `chunk_manager` share one formula, if they drift, generation and
/// regrow ceilings disagree and the world either over- or under-fills.
pub fn kind_target(base_capacity: u16, channel: f32) -> u16 {
    (base_capacity as f32 * (0.55 + channel * 0.7) * DENSITY_MULTIPLIER).round() as u16
}

/// Ore channel a forest chunk must clear before it seeds a lone iron node:
/// high, so most of the forest stays clear and the strike feels lucky. Because
/// a chunk only stays forest when the ore channel is below the forest channel
/// (post-bias), the qualifying chunks cluster on the fringe of the barren
/// biomes, exactly where a player would expect a stray vein.
const FOREST_IRON_ORE_CHANNEL: f32 = 0.64;
/// Stone channel for a forest chunk's occasional stone vein, a touch lower
/// than iron so veins turn up "now and again" rather than rarely.
const FOREST_STONE_VEIN_CHANNEL: f32 = 0.56;

/// Per-chunk node target for one kind, the single source of truth shared by
/// world generation and the regrow capacity grid (they MUST agree or the world
/// over/under-fills). Wraps `base_capacity` + channel scaling, plus the
/// forest-fringe rule: a forest's interior has no veins (the rich deposits stay
/// in the high-risk rocky/ore biomes), but where the ore/stone channel runs
/// high, the edge of a nearby barren biome, a forest can hold a lone iron node
/// and a little more often a stone vein, so a forest newcomer can still strike
/// it lucky without diluting the barren yields.
pub fn chunk_kind_target(
    classification: ChunkClassification,
    channels: ClassificationChannels,
    kind: NodeKind,
) -> u16 {
    if classification == ChunkClassification::Forest {
        match kind {
            // A single lucky iron node where the ore channel is high.
            NodeKind::IronOre => return u16::from(channels.ore >= FOREST_IRON_ORE_CHANNEL),
            // An occasional small stone vein, channel-scaled like everywhere
            // else so it's a vein, not a lone rock.
            NodeKind::StoneVein => {
                return if channels.stone >= FOREST_STONE_VEIN_CHANNEL {
                    kind_target(1, channels.stone)
                } else {
                    0
                };
            }
            // Forest never holds coal or sulfur, those stay barren-only.
            NodeKind::CoalOre | NodeKind::SulfurOre => return 0,
            _ => {}
        }
    }
    kind_target(base_capacity(classification, kind), channels.channel_for(kind))
}

/// One node placement decided by the generator. Carries the kind so the
/// caller can keep grid-level bookkeeping (capacity counts, regrow
/// timers) without rederiving the kind from the definition id.
#[derive(Debug, Clone)]
pub struct ChunkSpawn {
    pub coord: ChunkCoord,
    pub kind: NodeKind,
    pub spawn: WorldResourceNodeSpawn,
}

/// Axis-aligned rectangle the generator (and regrow scheduler) is
/// allowed to place nodes inside. Matches the playable interior carved
/// out by [`build_world_blocks`] minus a small clearance so node visuals
/// (the widest tree trunk is ≈0.46 m half-width) don't clip into the
/// stone perimeter. Without this, chunks at the world edge, which
/// extend past the centre-aligned walls, drop trees and ore *outside*
/// the wall, where the player can never reach them.
#[derive(Debug, Clone, Copy)]
pub struct PlayableBounds {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
}

impl PlayableBounds {
    /// Visual clearance kept between a placed node's centre and the
    /// inside face of the perimeter wall. Sized for the widest tree
    /// trunk plus a little breathing room.
    const WALL_CLEARANCE_M: f32 = 1.0;

    pub fn from_dims(dims: ChunkDims) -> Self {
        // Mirror `build_world_blocks`: walls of thickness 0.5 sit inset
        // by their own thickness from the outer edge, so the inner face
        // lands at `half - wall_thickness * 2`. Walls are centred on
        // the world origin even though the chunk grid is not, the
        // clamp here is what keeps spawns on the player side of the
        // wall on both axes.
        let half = dims.world_size_m() * 0.5;
        let wall_thickness = 0.5;
        let inner = half - wall_thickness * 2.0 - Self::WALL_CLEARANCE_M;
        Self {
            min_x: -inner,
            max_x: inner,
            min_z: -inner,
            max_z: inner,
        }
    }

    pub fn contains(&self, x: f32, z: f32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }
}

/// Generate all node spawns for every grid covered by `dims`. Node IDs
/// are dense and contiguous starting from `1`, the server adopts these
/// for its `ResourceNodeId` counter so admin-spawned nodes pick up safely
/// above the world-authored range.
pub fn generate_world_spawns(world_seed: u64, dims: ChunkDims) -> Vec<ChunkSpawn> {
    let bounds = PlayableBounds::from_dims(dims);
    let mut out = Vec::new();
    let mut next_id: u64 = 1;
    for coord in dims.coords() {
        let mut chunk_spawns = generate_chunk_spawns(world_seed, coord, &mut next_id, bounds);
        out.append(&mut chunk_spawns);
    }
    out
}

/// Generate the node spawns for a single grid, advancing `next_id` so
/// nodes across the whole world get unique IDs. Candidate positions
/// outside `bounds` are silently rejected, chunks at the world edge
/// extend past the centre-aligned perimeter walls, so without this
/// filter the outermost rings would drop nodes into the void beyond
/// the player's reach.
pub fn generate_chunk_spawns(
    world_seed: u64,
    coord: ChunkCoord,
    next_id: &mut u64,
    bounds: PlayableBounds,
) -> Vec<ChunkSpawn> {
    let channels = ClassificationChannels::sample(world_seed, coord);
    let classification = channels.classify();

    // Track placed points across all kinds in this chunk for the
    // cross-kind spacing rule, index is `(world_x, world_z)`. Each
    // kind also enforces its own (looser) spacing in its inner loop.
    let mut placed_global: Vec<(f32, f32)> = Vec::new();
    let mut spawns = Vec::new();
    let mut tree_variant_counter: u64 = splitmix64(world_seed ^ 0xBADCAFE);

    for kind in NodeKind::ALL {
        // Scale the classification's base capacity by the channel value
        // (a channel just above the ~0.42 threshold still delivers ~0.7×
        // capacity, a saturated channel ~1.05×), with the forest-fringe ore
        // rule folded in. Shared verbatim with the regrow capacity grid.
        let target = chunk_kind_target(classification, channels, kind);
        if target == 0 {
            continue;
        }

        let mut placed_for_kind: Vec<(f32, f32)> = Vec::new();
        let mut rng = ChunkRng::from_components(world_seed, coord.x, coord.z, kind_stream(kind));
        let mut placed = 0u16;
        let mut attempt = 0u32;
        let max_attempts = (target as u32) * MAX_CANDIDATES_PER_NODE;
        let kind_spacing = kind.min_spacing_m();
        let kind_spacing_sq = kind_spacing * kind_spacing;
        let cross_spacing_sq = CROSS_KIND_MIN_SPACING_M * CROSS_KIND_MIN_SPACING_M;

        while placed < target && attempt < max_attempts {
            attempt += 1;

            let (origin_x, origin_z) = coord.origin();
            let x = origin_x + rng.next_range(EDGE_MARGIN_M, CHUNK_SIZE_M - EDGE_MARGIN_M);
            let z = origin_z + rng.next_range(EDGE_MARGIN_M, CHUNK_SIZE_M - EDGE_MARGIN_M);

            // Drop candidates that would land outside the playable
            // interior. The chunk grid extends past the perimeter wall
            // on the world's positive axes, so without this gate the
            // outer ring would scatter trees and ore where the player
            // can't reach them.
            if !bounds.contains(x, z) {
                continue;
            }

            // Per-kind density mask, accept probability tracks the local
            // noise value. This is what turns a uniformly-sampled point
            // set into clusters: low-mask regions stay sparse, high-mask
            // regions accumulate nodes.
            let mask_seed = splitmix64(world_seed ^ (kind_stream(kind) as u64).wrapping_mul(0x57));
            let mask = fbm(mask_seed, x, z, KIND_MASK_FREQUENCY, KIND_MASK_OCTAVES);
            // Floor the threshold so even a "weak" cell still seeds a
            // few placements, otherwise low-channel kinds (e.g. ore in
            // a forest chunk) struggle to land even one.
            let accept_floor = 0.25;
            if rng.next_unit() > mask.max(accept_floor) {
                continue;
            }

            if placed_for_kind
                .iter()
                .any(|&(px, pz)| sq_dist(x, z, px, pz) < kind_spacing_sq)
            {
                continue;
            }
            if placed_global
                .iter()
                .any(|&(px, pz)| sq_dist(x, z, px, pz) < cross_spacing_sq)
            {
                continue;
            }

            let yaw = rng.next_range(-std::f32::consts::PI, std::f32::consts::PI);
            let definition_id = if matches!(
                kind,
                NodeKind::TreeSmall | NodeKind::TreeMedium | NodeKind::TreeLarge
            ) {
                tree_variant_counter = splitmix64(tree_variant_counter);
                kind.variant_definition_id(tree_variant_counter)
            } else {
                kind.definition_id()
            };
            spawns.push(ChunkSpawn {
                coord,
                kind,
                spawn: WorldResourceNodeSpawn::new(
                    *next_id,
                    definition_id,
                    Vec3Net::new(x, 0.0, z),
                    yaw,
                ),
            });
            *next_id += 1;
            placed_for_kind.push((x, z));
            placed_global.push((x, z));
            placed += 1;
        }
    }

    spawns
}

/// Build the static block geometry for a chunk world: a perimeter stone
/// wall sized to the dims so players can't wander off the playable area.
/// Internal blocks (the old hand-placed obstacle course) are gone,
/// gameplay terrain is the resource nodes themselves now.
pub fn build_world_blocks(dims: ChunkDims) -> Vec<WorldBlock> {
    let world_size = dims.world_size_m();
    let half = world_size * 0.5;
    let wall_thickness = 0.5;
    let wall_height = 2.0;
    let wall_half_height = wall_height; // y centre at `wall_half_height`.
    let wall_y = wall_half_height;
    // Walls sit just inside the playable edge so players can't see past
    // the wall texture into raw void.
    let inset = wall_thickness;
    vec![
        // North wall.
        WorldBlock {
            center: Vec3Net::new(0.0, wall_y, half - inset),
            half_extents: Vec3Net::new(half, wall_half_height, wall_thickness),
            kind: BlockKind::Stone,
        },
        // South wall.
        WorldBlock {
            center: Vec3Net::new(0.0, wall_y, -(half - inset)),
            half_extents: Vec3Net::new(half, wall_half_height, wall_thickness),
            kind: BlockKind::Stone,
        },
        // East wall.
        WorldBlock {
            center: Vec3Net::new(half - inset, wall_y, 0.0),
            half_extents: Vec3Net::new(wall_thickness, wall_half_height, half),
            kind: BlockKind::Stone,
        },
        // West wall.
        WorldBlock {
            center: Vec3Net::new(-(half - inset), wall_y, 0.0),
            half_extents: Vec3Net::new(wall_thickness, wall_half_height, half),
            kind: BlockKind::Stone,
        },
    ]
}

fn sq_dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 {
    let dx = ax - bx;
    let dz = az - bz;
    dx * dx + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forest_holds_lucky_iron_and_occasional_veins_while_barren_stays_rich() {
        let (mut forest, mut forest_iron, mut forest_vein) = (0u32, 0u32, 0u32);
        let (mut barren_chunks, mut barren_iron) = (0u32, 0u32);
        for seed in 0..50u64 {
            for x in -25..25 {
                for z in -25..25 {
                    let ch = ClassificationChannels::sample(seed, ChunkCoord::new(x, z));
                    let c = ch.classify();
                    match c {
                        ChunkClassification::Forest => {
                            forest += 1;
                            forest_iron += u32::from(chunk_kind_target(c, ch, NodeKind::IronOre) > 0);
                            forest_vein +=
                                u32::from(chunk_kind_target(c, ch, NodeKind::StoneVein) > 0);
                        }
                        ChunkClassification::OreVein | ChunkClassification::RockyOutcrop => {
                            barren_chunks += 1;
                            barren_iron += u32::from(chunk_kind_target(c, ch, NodeKind::IronOre));
                        }
                        _ => {}
                    }
                }
            }
        }
        let iron_pct = 100.0 * forest_iron as f32 / forest as f32;
        let vein_pct = 100.0 * forest_vein as f32 / forest as f32;
        let barren_iron_avg = barren_iron as f32 / barren_chunks.max(1) as f32;
        println!(
            "forest chunks {forest}: iron {iron_pct:.1}%  stone-vein {vein_pct:.1}%  | barren iron/chunk {barren_iron_avg:.2}"
        );
        // Iron in the forest is a lucky strike, not a blanket.
        assert!(
            (5.0..30.0).contains(&iron_pct),
            "forest iron should be a lucky minority, got {iron_pct:.1}%"
        );
        // Stone veins turn up "now and again", a bit more often than iron.
        assert!(vein_pct > iron_pct, "veins should beat iron: {vein_pct:.1} vs {iron_pct:.1}");
        assert!(vein_pct < 55.0, "veins still a minority, got {vein_pct:.1}%");
        // The high-risk barren biomes keep the rich iron (much more than forest).
        assert!(barren_iron_avg > 2.0, "barren must out-yield forest iron, got {barren_iron_avg:.2}");
    }

    #[test]
    fn generate_world_spawns_is_deterministic() {
        let dims = ChunkDims::new(5);
        let a = generate_world_spawns(0xABCDEF, dims);
        let b = generate_world_spawns(0xABCDEF, dims);
        assert_eq!(a.len(), b.len());
        for (sa, sb) in a.iter().zip(b.iter()) {
            assert_eq!(sa.coord, sb.coord);
            assert_eq!(sa.kind, sb.kind);
            assert_eq!(sa.spawn.id, sb.spawn.id);
            assert_eq!(sa.spawn.definition_id, sb.spawn.definition_id);
            assert_eq!(sa.spawn.position, sb.spawn.position);
        }
    }

    #[test]
    fn generate_world_spawns_produces_unique_ids() {
        let dims = ChunkDims::new(5);
        let spawns = generate_world_spawns(7, dims);
        let mut ids: Vec<u64> = spawns.iter().map(|s| s.spawn.id).collect();
        ids.sort();
        let original_len = ids.len();
        ids.dedup();
        assert_eq!(
            ids.len(),
            original_len,
            "node IDs should be unique across the world"
        );
    }

    #[test]
    fn generate_world_spawns_populates_5x5_with_variety() {
        let spawns = generate_world_spawns(42, ChunkDims::new(5));
        assert!(
            spawns.len() >= 80,
            "expected at least 80 nodes in a 5x5 world, got {}",
            spawns.len()
        );
        let mut kinds: std::collections::HashSet<NodeKind> = std::collections::HashSet::new();
        for spawn in &spawns {
            kinds.insert(spawn.kind);
        }
        // We should see several kinds across the map, exact set
        // depends on the seed, but a healthy mix is expected.
        assert!(
            kinds.len() >= 5,
            "expected at least 5 node kinds in spawns, saw: {kinds:?}"
        );
    }

    /// Bounds wide enough that the chunk-local margin test isn't
    /// secondarily clipped by the playable-area gate.
    fn unbounded() -> PlayableBounds {
        PlayableBounds {
            min_x: f32::MIN,
            max_x: f32::MAX,
            min_z: f32::MIN,
            max_z: f32::MAX,
        }
    }

    #[test]
    fn placed_nodes_respect_min_spacing_inside_grid() {
        let mut next_id = 1;
        let spawns = generate_chunk_spawns(7, ChunkCoord::new(0, 0), &mut next_id, unbounded());
        for i in 0..spawns.len() {
            for j in (i + 1)..spawns.len() {
                let a = &spawns[i].spawn;
                let b = &spawns[j].spawn;
                let dx = a.position.x - b.position.x;
                let dz = a.position.z - b.position.z;
                let dist = (dx * dx + dz * dz).sqrt();
                let min = if spawns[i].kind == spawns[j].kind {
                    spawns[i].kind.min_spacing_m()
                } else {
                    CROSS_KIND_MIN_SPACING_M
                };
                assert!(
                    dist + 1e-3 >= min,
                    "nodes #{} and #{} too close: {dist} < {min}",
                    a.id,
                    b.id
                );
            }
        }
    }

    #[test]
    fn placed_nodes_stay_inside_grid_with_margin() {
        let coord = ChunkCoord::new(1, -1);
        let mut next_id = 1;
        let spawns = generate_chunk_spawns(7, coord, &mut next_id, unbounded());
        let (ox, oz) = coord.origin();
        for spawn in &spawns {
            assert!(spawn.spawn.position.x >= ox + EDGE_MARGIN_M - 1e-3);
            assert!(spawn.spawn.position.x <= ox + CHUNK_SIZE_M - EDGE_MARGIN_M + 1e-3);
            assert!(spawn.spawn.position.z >= oz + EDGE_MARGIN_M - 1e-3);
            assert!(spawn.spawn.position.z <= oz + CHUNK_SIZE_M - EDGE_MARGIN_M + 1e-3);
        }
    }

    #[test]
    fn world_spawns_stay_inside_playable_bounds() {
        // Chunks for dims=5 span world x in [-128, 192] while the
        // walls sit at ±160. Without bounds clipping the eastmost
        // ring would drop nodes past the wall, this test pins the
        // generator-side gate.
        let dims = ChunkDims::new(5);
        let bounds = PlayableBounds::from_dims(dims);
        let spawns = generate_world_spawns(42, dims);
        for spawn in &spawns {
            assert!(
                bounds.contains(spawn.spawn.position.x, spawn.spawn.position.z),
                "node {} at ({:.2}, {:.2}) escaped playable bounds {:?}",
                spawn.spawn.id,
                spawn.spawn.position.x,
                spawn.spawn.position.z,
                bounds,
            );
        }
    }

    #[test]
    fn perimeter_walls_form_closed_box() {
        let dims = ChunkDims::new(5);
        let blocks = build_world_blocks(dims);
        assert_eq!(blocks.len(), 4);
        for block in blocks {
            assert_eq!(block.kind, BlockKind::Stone);
            assert!(block.size().y >= 2.0);
        }
    }
}
