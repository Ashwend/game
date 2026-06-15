pub mod chunk;
pub mod map_texture;
pub mod terrain_texture;

pub use chunk::{
    CHUNK_SIZE_M, ChunkClassification, ChunkCoord, ChunkDims, ChunkRng, ChunkSpawn,
    ClassificationChannels, NodeKind, PlayableBounds, base_capacity, build_world_blocks, fbm,
    generate_chunk_spawns, generate_world_spawns, kind_target, splitmix64, value_noise_2d,
};
pub use map_texture::{WORLD_MAP_TEXELS, render_world_map_rgba, world_map_bounds};
pub use terrain_texture::{
    TERRAIN_WEIGHT_TEXELS, biome_blend_weights, fill_terrain_weight_rows,
    render_terrain_weight_rgba,
};

use serde::{Deserialize, Serialize};

use crate::protocol::Vec3Net;

/// Fixed seed used by [`WorldData::test_world`] and the default map so the
/// generated world is the same every load, handy for tests and for the
/// loopback menu backdrop.
pub const TEST_WORLD_SEED: u64 = 0x7E57_5EED_5EED_5EED;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MapType {
    Procedural {
        seed: u64,
        #[serde(default)]
        size: ProceduralMapSize,
    },
}

impl Default for MapType {
    fn default() -> Self {
        Self::Procedural {
            seed: TEST_WORLD_SEED,
            size: ProceduralMapSize::default(),
        }
    }
}

impl MapType {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Procedural { .. } => "Procedural",
        }
    }

    pub fn world_data(&self) -> WorldData {
        match self {
            Self::Procedural { seed, size } => {
                WorldData::chunk_world(*seed, ChunkDims::new(size.dims()))
            }
        }
    }

    /// World seed used by the chunk generator.
    pub fn world_seed(&self) -> u64 {
        match self {
            Self::Procedural { seed, .. } => *seed,
        }
    }

    /// Grid dimensions the world is generated against.
    pub fn chunk_dims(&self) -> ChunkDims {
        match self {
            Self::Procedural { size, .. } => ChunkDims::new(size.dims()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProceduralMapSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl ProceduralMapSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    /// Chunk cells per side of the largest map. 63 chunks at the 64 m grid
    /// size is 4032 m, an approximate 4 km square playable area. Medium and
    /// small are derived from this maximum so all three sizes scale together
    /// if it ever changes.
    const LARGE_DIMS: u32 = 63;

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
        }
    }

    /// Number of chunk cells per side. Large approximates a 4 km square
    /// (`LARGE_DIMS` = 63 cells = 4032 m); medium and small are 1/2 and 1/4 of
    /// that extent. Each is forced to an odd count (`| 1`) so the grid keeps a
    /// single center chunk over the origin where the player spawns. This lands
    /// small/medium/large at 15/31/63 cells = 960/1984/4032 m.
    pub fn dims(self) -> u32 {
        match self {
            Self::Small => (Self::LARGE_DIMS / 4) | 1,
            Self::Medium => (Self::LARGE_DIMS / 2) | 1,
            Self::Large => Self::LARGE_DIMS,
        }
    }

    pub fn floor_size(self) -> f32 {
        self.dims() as f32 * CHUNK_SIZE_M
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldData {
    pub floor_size: f32,
    pub blocks: Vec<WorldBlock>,
    /// Initial resource node spawn list. **Always empty for grid-generated
    /// worlds**, the server's [`crate::server::ChunkManager`] owns initial
    /// generation and serves nodes per-player via AoI streaming. This
    /// field remains in the struct as the historical/test surface and as
    /// a hook for hand-authored levels.
    #[serde(default)]
    pub resource_nodes: Vec<WorldResourceNodeSpawn>,
}

impl Default for WorldData {
    fn default() -> Self {
        MapType::default().world_data()
    }
}

impl WorldData {
    /// Build a chunk-driven world: perimeter walls sized to `dims`,
    /// empty `resource_nodes` (the server's chunk manager populates the
    /// live node map from the seed). `seed` is forwarded purely so this
    /// function is consistent with `MapType::world_seed()`, the actual
    /// node generation happens server-side.
    pub fn chunk_world(seed: u64, dims: ChunkDims) -> Self {
        // Touch `seed` so callers can be confident this signature
        // does not silently drop input. Pure block geometry doesn't need
        // it (perimeter walls are dims-only), but keeping the parameter
        // signals intent and leaves room for seed-influenced blocks
        // (e.g. landmark rocks) later.
        let _ = seed;
        Self {
            floor_size: dims.world_size_m(),
            blocks: build_world_blocks(dims),
            resource_nodes: Vec::new(),
        }
    }

    /// Convenience helper used by tests and the menu backdrop fallback,
    /// returns a deterministic small procedural world.
    pub fn test_world() -> Self {
        MapType::default().world_data()
    }

    /// Hand-crafted scene used as the **main menu backdrop**. No
    /// perimeter walls (so the player doesn't see masonry in the splash
    /// view), just a densely-populated slice of forest with trees, ore
    /// lumps, branches, surface stones, and grass tufts spread across
    /// foreground / midground / background bands.
    ///
    /// The camera sits at `(-5.8, eye, 7.2)` looking towards
    /// `(0.4, 0.85, -3.6)`, see
    /// `crate::app::systems::camera::menu_backdrop`. Positions below are
    /// hand-tuned to that view so the scene reads as a layered woodland
    /// regardless of which world the player ends up generating.
    pub fn menu_backdrop_world() -> Self {
        use crate::resources::{
            BIRCH_TREE_LARGE_NODE_ID, BIRCH_TREE_NODE_ID, BIRCH_TREE_SMALL_NODE_ID,
            BRANCH_PILE_NODE_ID, COAL_NODE_ID, HAY_GRASS_NODE_ID, IRON_NODE_ID,
            PINE_TREE_LARGE_NODE_ID, PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID, SULFUR_NODE_ID,
            SURFACE_STONE_NODE_ID,
        };
        // `(definition_id, x, z, yaw)`, id is assigned sequentially below
        // so reordering or adding entries doesn't break IDs. Yaw values are
        // hand-picked per node so each tree/ore reads as a distinct
        // silhouette instead of a cloned row.
        let placements: &[(&str, f32, f32, f32)] = &[
            // --- BACKGROUND TREELINE -----------------------------------
            // A wall of tall trees frames the deep horizon, with a couple
            // of large pines and birches as silhouette anchors.
            (PINE_TREE_LARGE_NODE_ID, 5.2, -16.0, 0.6),
            (PINE_TREE_LARGE_NODE_ID, -3.8, -17.5, -0.4),
            (BIRCH_TREE_LARGE_NODE_ID, 9.6, -15.2, 1.2),
            (BIRCH_TREE_LARGE_NODE_ID, -7.4, -15.8, -0.9),
            (PINE_TREE_NODE_ID, 1.4, -18.5, 0.1),
            (PINE_TREE_NODE_ID, -1.6, -16.8, 1.7),
            (BIRCH_TREE_NODE_ID, 12.0, -17.0, -0.6),
            (BIRCH_TREE_NODE_ID, -10.2, -17.8, 0.4),
            (PINE_TREE_SMALL_NODE_ID, 3.8, -19.5, -1.1),
            (BIRCH_TREE_SMALL_NODE_ID, -4.6, -19.2, 0.8),
            // --- MID-BACKGROUND ----------------------------------------
            // Larger anchor trees that the camera focuses on past the
            // depth-of-field sweet spot.
            (PINE_TREE_LARGE_NODE_ID, 3.6, -10.4, 0.4),
            (BIRCH_TREE_LARGE_NODE_ID, -4.8, -10.8, -1.3),
            (PINE_TREE_NODE_ID, 7.2, -11.2, 0.7),
            (PINE_TREE_NODE_ID, -1.0, -12.3, -0.2),
            (BIRCH_TREE_NODE_ID, -8.0, -11.8, 1.0),
            (BIRCH_TREE_NODE_ID, 10.4, -10.0, -0.5),
            (PINE_TREE_SMALL_NODE_ID, 0.6, -13.2, 1.4),
            (BIRCH_TREE_SMALL_NODE_ID, -2.6, -13.6, 0.3),
            // --- MIDGROUND TREES ---------------------------------------
            // These sit roughly at the look-at depth (~z=-7), the visual
            // anchor of the composition.
            (PINE_TREE_NODE_ID, 4.4, -7.4, -0.6),
            (BIRCH_TREE_NODE_ID, -3.2, -7.8, 0.9),
            (BIRCH_TREE_NODE_ID, 8.0, -8.2, -0.2),
            (PINE_TREE_SMALL_NODE_ID, -5.8, -7.4, 1.2),
            (BIRCH_TREE_SMALL_NODE_ID, 6.4, -6.6, 0.5),
            (PINE_TREE_SMALL_NODE_ID, 1.4, -8.0, -1.4),
            // --- MIDGROUND ORE -----------------------------------------
            // Ore lumps scattered between the midground trees so each
            // type is visible.
            (COAL_NODE_ID, 5.2, -9.0, 0.6),
            (IRON_NODE_ID, -0.4, -9.8, -0.3),
            (SULFUR_NODE_ID, 2.6, -11.6, 1.0),
            (COAL_NODE_ID, -2.0, -10.6, -1.2),
            (IRON_NODE_ID, 9.0, -8.0, 0.2),
            (SULFUR_NODE_ID, -7.2, -9.2, -0.8),
            // --- FOREGROUND CLUSTER ------------------------------------
            // Close-camera band, clear of full-size trees so the eye
            // reads detail (crude nodes + a couple of saplings).
            (PINE_TREE_SMALL_NODE_ID, -4.6, -3.6, 0.2),
            (BIRCH_TREE_SMALL_NODE_ID, 5.6, -3.2, -0.6),
            (PINE_TREE_SMALL_NODE_ID, 7.4, -4.6, 1.1),
            (BIRCH_TREE_SMALL_NODE_ID, -2.0, -2.6, -1.4),
            // Foreground ore, close enough to show the chunk detail.
            (COAL_NODE_ID, 4.2, -4.1, 0.6),
            (IRON_NODE_ID, -0.6, -4.7, -0.4),
            (SULFUR_NODE_ID, 2.0, -2.8, 1.3),
            // --- SURFACE STONES (spread across all bands) --------------
            (SURFACE_STONE_NODE_ID, -1.6, -3.0, 1.1),
            (SURFACE_STONE_NODE_ID, 1.0, -1.6, -0.3),
            (SURFACE_STONE_NODE_ID, 6.0, -2.2, 0.8),
            (SURFACE_STONE_NODE_ID, -3.4, -5.8, -1.0),
            (SURFACE_STONE_NODE_ID, 3.0, -6.2, 0.5),
            (SURFACE_STONE_NODE_ID, -6.4, -6.8, 1.4),
            (SURFACE_STONE_NODE_ID, 7.8, -7.6, -0.7),
            (SURFACE_STONE_NODE_ID, 0.4, -11.4, 0.2),
            // --- BRANCH PILES ------------------------------------------
            (BRANCH_PILE_NODE_ID, 1.7, -2.4, -0.7),
            (BRANCH_PILE_NODE_ID, -3.6, -4.0, 0.6),
            (BRANCH_PILE_NODE_ID, 5.8, -5.4, 1.0),
            (BRANCH_PILE_NODE_ID, -5.0, -2.8, -1.2),
            (BRANCH_PILE_NODE_ID, 3.4, -3.6, 0.1),
            (BRANCH_PILE_NODE_ID, -1.2, -6.0, -0.5),
            (BRANCH_PILE_NODE_ID, 8.4, -3.8, 1.4),
            (BRANCH_PILE_NODE_ID, -4.2, -8.4, 0.8),
            // --- HAY/GRASS TUFTS ---------------------------------------
            // Sprinkled liberally across the open ground.
            (HAY_GRASS_NODE_ID, 2.4, -5.6, 0.0),
            (HAY_GRASS_NODE_ID, -3.0, -4.4, 0.3),
            (HAY_GRASS_NODE_ID, 0.0, -2.0, 1.2),
            (HAY_GRASS_NODE_ID, -1.4, -5.0, -0.9),
            (HAY_GRASS_NODE_ID, 4.6, -2.0, 0.7),
            (HAY_GRASS_NODE_ID, -5.4, -5.6, -0.2),
            (HAY_GRASS_NODE_ID, 6.8, -5.8, 1.1),
            (HAY_GRASS_NODE_ID, -7.0, -3.6, 0.5),
            (HAY_GRASS_NODE_ID, 2.2, -7.2, -0.6),
            (HAY_GRASS_NODE_ID, -2.6, -8.6, 1.3),
            (HAY_GRASS_NODE_ID, 5.4, -7.0, -1.1),
            (HAY_GRASS_NODE_ID, -0.8, -3.8, 0.4),
            // --- RIGHT-SIDE FILL ---------------------------------------
            // The menu camera looks forward-and-right (forward ≈ +x, -z),
            // so the right half of the frame opens up to a much larger
            // visible x. These placements push out to x≈22 so the right
            // edge isn't a flat background colour.
            //
            // Right-side background treeline (deep z, far x).
            (PINE_TREE_LARGE_NODE_ID, 16.4, -18.0, 0.7),
            (BIRCH_TREE_LARGE_NODE_ID, 19.4, -16.4, -0.4),
            (PINE_TREE_NODE_ID, 14.8, -19.5, 1.1),
            (BIRCH_TREE_NODE_ID, 22.0, -17.6, 0.3),
            (PINE_TREE_SMALL_NODE_ID, 17.6, -20.2, -1.0),
            // Right-side mid-background.
            (PINE_TREE_LARGE_NODE_ID, 13.6, -12.0, -0.5),
            (BIRCH_TREE_NODE_ID, 16.0, -13.6, 0.8),
            (PINE_TREE_NODE_ID, 18.8, -11.0, 1.4),
            (PINE_TREE_SMALL_NODE_ID, 15.2, -14.6, -0.9),
            (BIRCH_TREE_SMALL_NODE_ID, 20.0, -13.0, 0.5),
            // Right-side midground.
            (BIRCH_TREE_NODE_ID, 11.4, -8.0, 0.6),
            (PINE_TREE_NODE_ID, 13.0, -9.4, -1.0),
            (BIRCH_TREE_SMALL_NODE_ID, 14.2, -6.6, 1.2),
            (PINE_TREE_SMALL_NODE_ID, 10.6, -7.0, 0.3),
            (BIRCH_TREE_NODE_ID, 16.4, -8.4, -0.7),
            // Right-side foreground saplings, keep the close band
            // populated as the camera pans right.
            (BIRCH_TREE_SMALL_NODE_ID, 9.6, -5.0, -0.7),
            (PINE_TREE_SMALL_NODE_ID, 11.0, -3.8, 1.0),
            (BIRCH_TREE_SMALL_NODE_ID, 12.8, -2.6, 0.4),
            // Right-side ore.
            (COAL_NODE_ID, 12.6, -10.5, 0.4),
            (IRON_NODE_ID, 15.6, -9.2, -0.6),
            (SULFUR_NODE_ID, 11.0, -12.6, 1.3),
            (IRON_NODE_ID, 18.0, -10.2, 0.1),
            (COAL_NODE_ID, 14.4, -11.4, -1.2),
            // Right-side crude detail.
            (SURFACE_STONE_NODE_ID, 10.0, -4.5, 0.5),
            (SURFACE_STONE_NODE_ID, 13.0, -7.4, -0.8),
            (SURFACE_STONE_NODE_ID, 16.8, -6.4, 1.0),
            (SURFACE_STONE_NODE_ID, 12.2, -3.0, -0.3),
            (BRANCH_PILE_NODE_ID, 11.4, -5.6, 1.1),
            (BRANCH_PILE_NODE_ID, 14.6, -8.6, -0.3),
            (BRANCH_PILE_NODE_ID, 17.4, -7.6, 0.8),
            (BRANCH_PILE_NODE_ID, 9.8, -2.2, -1.1),
            (HAY_GRASS_NODE_ID, 9.6, -3.0, 0.0),
            (HAY_GRASS_NODE_ID, 12.0, -6.0, 0.7),
            (HAY_GRASS_NODE_ID, 15.0, -4.6, -1.2),
            (HAY_GRASS_NODE_ID, 17.6, -5.4, 0.6),
            (HAY_GRASS_NODE_ID, 13.8, -5.0, -0.4),
            (HAY_GRASS_NODE_ID, 19.0, -7.2, 1.1),
            // --- FAR-LEFT FILL -----------------------------------------
            // The left edge of the screen looks past the camera's offset
            // into a deep-z, moderate-negative-x band. Pack trees + ore
            // out to x ≈ -18 at z ≈ -16…-28.
            (PINE_TREE_LARGE_NODE_ID, -14.0, -22.0, 0.5),
            (BIRCH_TREE_LARGE_NODE_ID, -16.5, -20.0, -0.8),
            (PINE_TREE_LARGE_NODE_ID, -18.0, -25.0, -0.3),
            (PINE_TREE_NODE_ID, -12.0, -24.5, 1.2),
            (BIRCH_TREE_NODE_ID, -13.5, -19.0, 0.9),
            (PINE_TREE_NODE_ID, -11.0, -16.0, -0.6),
            (BIRCH_TREE_LARGE_NODE_ID, -15.0, -16.5, 1.4),
            (PINE_TREE_NODE_ID, -12.5, -13.5, 0.2),
            (BIRCH_TREE_NODE_ID, -10.0, -14.5, -1.1),
            (PINE_TREE_SMALL_NODE_ID, -13.0, -21.0, 0.7),
            (BIRCH_TREE_SMALL_NODE_ID, -16.0, -23.0, -0.2),
            (PINE_TREE_SMALL_NODE_ID, -9.0, -11.0, 1.3),
            (BIRCH_TREE_SMALL_NODE_ID, -11.5, -10.5, -0.9),
            (COAL_NODE_ID, -10.0, -13.0, 0.5),
            (IRON_NODE_ID, -12.0, -17.5, -0.6),
            (SULFUR_NODE_ID, -14.0, -19.5, 0.8),
            (SURFACE_STONE_NODE_ID, -9.0, -10.0, -0.5),
            (SURFACE_STONE_NODE_ID, -12.6, -15.4, 1.0),
            (BRANCH_PILE_NODE_ID, -10.5, -12.5, 1.0),
            (BRANCH_PILE_NODE_ID, -13.0, -17.0, -0.4),
            (HAY_GRASS_NODE_ID, -9.5, -7.5, 0.4),
            (HAY_GRASS_NODE_ID, -11.5, -9.5, -0.7),
            (HAY_GRASS_NODE_ID, -13.6, -12.0, 0.2),
            // --- FAR-RIGHT FILL ----------------------------------------
            // The right edge can reach world x ≈ 35-40 at depth because
            // the camera's forward axis tilts toward +x. These belts cover
            // near, mid, and deep right.
            (PINE_TREE_LARGE_NODE_ID, 26.0, -12.0, 0.4),
            (BIRCH_TREE_LARGE_NODE_ID, 30.0, -14.0, -0.6),
            (PINE_TREE_LARGE_NODE_ID, 34.0, -10.0, 1.1),
            (BIRCH_TREE_LARGE_NODE_ID, 28.0, -8.0, -0.3),
            (PINE_TREE_NODE_ID, 24.0, -10.5, 0.8),
            (BIRCH_TREE_NODE_ID, 26.0, -6.0, -1.0),
            (PINE_TREE_NODE_ID, 32.0, -7.0, 0.5),
            (BIRCH_TREE_NODE_ID, 36.0, -8.0, -0.4),
            (PINE_TREE_NODE_ID, 22.0, -4.5, 1.2),
            (BIRCH_TREE_NODE_ID, 24.0, -2.5, 0.0),
            (PINE_TREE_SMALL_NODE_ID, 20.0, -3.0, -0.8),
            (BIRCH_TREE_SMALL_NODE_ID, 28.0, -4.5, 0.6),
            (PINE_TREE_SMALL_NODE_ID, 23.0, -7.0, -1.2),
            (BIRCH_TREE_SMALL_NODE_ID, 30.0, -5.5, 0.9),
            (PINE_TREE_NODE_ID, 38.0, -11.5, -0.2),
            (BIRCH_TREE_SMALL_NODE_ID, 34.0, -6.0, 1.3),
            (COAL_NODE_ID, 22.0, -8.5, 0.3),
            (IRON_NODE_ID, 26.0, -7.0, -0.7),
            (SULFUR_NODE_ID, 30.0, -9.5, 1.0),
            (COAL_NODE_ID, 24.0, -5.0, -0.4),
            (IRON_NODE_ID, 32.0, -6.5, 0.5),
            (SULFUR_NODE_ID, 36.0, -10.0, -0.9),
            (SURFACE_STONE_NODE_ID, 20.0, -5.5, -0.3),
            (SURFACE_STONE_NODE_ID, 25.0, -4.0, 1.1),
            (SURFACE_STONE_NODE_ID, 28.0, -7.5, -0.9),
            (SURFACE_STONE_NODE_ID, 33.0, -8.5, 0.6),
            (BRANCH_PILE_NODE_ID, 22.0, -3.5, 0.4),
            (BRANCH_PILE_NODE_ID, 26.0, -5.5, -1.0),
            (BRANCH_PILE_NODE_ID, 30.0, -7.0, 0.8),
            (BRANCH_PILE_NODE_ID, 35.0, -9.0, -0.5),
            (HAY_GRASS_NODE_ID, 20.0, -4.0, 0.0),
            (HAY_GRASS_NODE_ID, 24.0, -6.0, -0.5),
            (HAY_GRASS_NODE_ID, 28.0, -5.0, 1.2),
            (HAY_GRASS_NODE_ID, 32.0, -7.5, -0.3),
            (HAY_GRASS_NODE_ID, 22.5, -6.5, 0.9),
            (HAY_GRASS_NODE_ID, 27.0, -10.0, -1.0),
            (HAY_GRASS_NODE_ID, 31.0, -4.5, 0.4),
            // --- NEAR-CAMERA RIGHT (very close, just inside the FOV) ---
            // Things at z ≈ 0 to +1 in world space project to the
            // far-right of the screen because the camera looks past the
            // right shoulder.
            (PINE_TREE_SMALL_NODE_ID, 7.4, 0.6, 0.4),
            (BIRCH_TREE_SMALL_NODE_ID, 9.0, 1.6, -0.7),
            (BIRCH_TREE_SMALL_NODE_ID, 10.6, 0.2, -1.1),
            (SURFACE_STONE_NODE_ID, 7.6, 0.0, 0.2),
            (SURFACE_STONE_NODE_ID, 6.0, 1.4, -0.3),
            (BRANCH_PILE_NODE_ID, 5.8, -0.4, 1.0),
            (BRANCH_PILE_NODE_ID, 8.4, 1.0, -0.6),
            (COAL_NODE_ID, 8.8, -0.4, 0.5),
            (HAY_GRASS_NODE_ID, 7.0, 1.0, -0.4),
            (HAY_GRASS_NODE_ID, 8.4, -1.0, 0.6),
            (HAY_GRASS_NODE_ID, 9.6, 1.4, 0.8),
        ];

        let resource_nodes = placements
            .iter()
            .enumerate()
            .map(|(index, (definition_id, x, z, yaw))| {
                WorldResourceNodeSpawn::new(
                    (index as u64) + 1,
                    *definition_id,
                    Vec3Net::new(*x, 0.0, *z),
                    *yaw,
                )
            })
            .collect();

        Self {
            // Floor has to span the wide x range, the far-right fill
            // reaches x ≈ 38 and the deep-left fill reaches x ≈ -18 and
            // z ≈ -25. A 90 m plane covers both with margin against the
            // panning camera.
            floor_size: 90.0,
            blocks: Vec::new(),
            resource_nodes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldResourceNodeSpawn {
    pub id: u64,
    pub definition_id: String,
    pub position: Vec3Net,
    pub yaw: f32,
}

impl WorldResourceNodeSpawn {
    pub fn new(id: u64, definition_id: impl Into<String>, position: Vec3Net, yaw: f32) -> Self {
        Self {
            id,
            definition_id: definition_id.into(),
            position,
            yaw,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// Default obstacle, gets the rotating block palette in the renderer.
    #[default]
    Standard,
    /// Grayish stone block, used for perimeter walls and similar structural
    /// pieces that should read as masonry rather than test geometry.
    Stone,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WorldBlock {
    pub center: Vec3Net,
    pub half_extents: Vec3Net,
    #[serde(default)]
    pub kind: BlockKind,
}

impl WorldBlock {
    pub const fn new(center: Vec3Net, half_extents: Vec3Net) -> Self {
        Self {
            center,
            half_extents,
            kind: BlockKind::Standard,
        }
    }

    pub const fn stone(center: Vec3Net, half_extents: Vec3Net) -> Self {
        Self {
            center,
            half_extents,
            kind: BlockKind::Stone,
        }
    }

    pub fn min(self) -> Vec3Net {
        Vec3Net::new(
            self.center.x - self.half_extents.x,
            self.center.y - self.half_extents.y,
            self.center.z - self.half_extents.z,
        )
    }

    pub fn max(self) -> Vec3Net {
        Vec3Net::new(
            self.center.x + self.half_extents.x,
            self.center.y + self.half_extents.y,
            self.center.z + self.half_extents.z,
        )
    }

    pub fn size(self) -> Vec3Net {
        self.half_extents.scale(2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_world_has_perimeter_walls_and_no_spawns() {
        let world = WorldData::test_world();
        assert!(world.floor_size > 0.0);
        assert_eq!(world.blocks.len(), 4, "expected four perimeter walls");
        assert!(world.resource_nodes.is_empty());
        for block in world.blocks {
            assert!(block.min().y >= 0.0);
            assert_eq!(block.kind, BlockKind::Stone);
        }
    }

    #[test]
    fn map_type_default_and_labels_are_stable() {
        assert_eq!(
            MapType::default(),
            MapType::Procedural {
                seed: TEST_WORLD_SEED,
                size: ProceduralMapSize::default(),
            }
        );
        assert_eq!(
            MapType::Procedural {
                seed: 42,
                size: ProceduralMapSize::Medium,
            }
            .label(),
            "Procedural"
        );
    }

    #[test]
    fn map_type_exposes_seed_and_dims() {
        let procedural = MapType::Procedural {
            seed: 99,
            size: ProceduralMapSize::Large,
        };
        assert_eq!(procedural.world_seed(), 99);
        assert_eq!(
            procedural.chunk_dims().dims,
            ProceduralMapSize::Large.dims()
        );
    }

    #[test]
    fn procedural_world_floor_size_matches_dims() {
        let world = MapType::Procedural {
            seed: 42,
            size: ProceduralMapSize::Large,
        }
        .world_data();
        assert_eq!(world.floor_size, ProceduralMapSize::Large.floor_size());
        assert!(world.resource_nodes.is_empty());
    }
}
