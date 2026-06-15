//! World-map terrain raster: a low-resolution top-down biome image built
//! straight from the seed's chunk-classification noise.
//!
//! The raster is a **pure function of `(world_seed, dims)`**, both of which the
//! client already receives in `Welcome` (`MapType`), so the *client* generates
//! it locally on demand, there's no need to render it on the headless server
//! and ship it over the wire. The "cartoonish, smushed colours" look comes from
//! the flat biome palette plus the client upscaling this small raster with
//! linear filtering, no GPU shader required.

use super::{CHUNK_SIZE_M, ChunkClassification, ChunkDims, ClassificationChannels};

/// Pixels per side of the rastered terrain image. 256 keeps the buffer at
/// 256 KB while still resolving biome regions (which span 3-4 chunks) into
/// recognizable blobs. Markers ride as vector pins, so spotting a point of
/// interest doesn't depend on this.
pub const WORLD_MAP_TEXELS: u32 = 256;

/// World-space AABB the map image covers, derived from the chunk grid the same
/// way [`ChunkDims::coords`] enumerates it (centred on the origin, square):
/// `(min_x, min_z, max_x, max_z)`.
pub fn world_map_bounds(dims: ChunkDims) -> (f32, f32, f32, f32) {
    let n = dims.dims as i32;
    let half = n / 2;
    let min_chunk = -half;
    // Mirror `coords()`: odd grids run -half..=half, even grids -half..=half-1.
    let max_chunk = half - (1 - n % 2);
    let min = min_chunk as f32 * CHUNK_SIZE_M;
    let max = (max_chunk + 1) as f32 * CHUNK_SIZE_M;
    (min, min, max, max)
}

/// Render the biome terrain raster for a world as `WORLD_MAP_TEXELS²` RGBA8
/// pixels (row 0 = the north edge / `min_z`, increasing rows run south). Pure
/// function of the seed + dims, so client and server would produce byte-
/// identical output; today only the client calls it.
pub fn render_world_map_rgba(world_seed: u64, dims: ChunkDims) -> Vec<u8> {
    let texels = WORLD_MAP_TEXELS;
    let (min_x, min_z, max_x, max_z) = world_map_bounds(dims);
    let span_x = max_x - min_x;
    let span_z = max_z - min_z;

    let mut rgba = Vec::with_capacity((texels * texels * 4) as usize);
    for py in 0..texels {
        let wz = min_z + (py as f32 + 0.5) / texels as f32 * span_z;
        for px in 0..texels {
            let wx = min_x + (px as f32 + 0.5) / texels as f32 * span_x;
            let channels = ClassificationChannels::sample_at(world_seed, wx, wz);
            let classification = channels.classify();
            let intensity = dominant_intensity(channels, classification);
            let [r, g, b] = biome_rgb(classification, intensity);
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

/// Flat cartoon biome palette, nudged brighter where the dominant channel reads
/// strongly so regions aren't dead flat. The flatness is intentional: it reads
/// as a stylized map once the client softens it on upscale.
fn biome_rgb(classification: ChunkClassification, intensity: f32) -> [u8; 3] {
    let base = match classification {
        ChunkClassification::Forest => [60, 108, 56],
        ChunkClassification::RockyOutcrop => [126, 124, 118],
        ChunkClassification::OreVein => [122, 96, 72],
        ChunkClassification::Plains => [150, 172, 96],
        ChunkClassification::Mixed => [104, 132, 84],
    };
    // Dominant channels sit roughly in 0.42..0.9; map that to a gentle
    // 0.86..1.08 brightness so stronger biomes read a touch deeper/brighter.
    let t = ((intensity - 0.42) / 0.48).clamp(0.0, 1.0);
    let scale = 0.86 + 0.22 * t;
    base.map(|channel| (channel as f32 * scale).clamp(0.0, 255.0) as u8)
}

/// Strength of the channel that won the classification, used only to modulate
/// brightness. `Mixed` has no single winner, so average the four.
fn dominant_intensity(
    channels: ClassificationChannels,
    classification: ChunkClassification,
) -> f32 {
    match classification {
        ChunkClassification::Forest => channels.forest,
        ChunkClassification::RockyOutcrop => channels.stone,
        ChunkClassification::OreVein => channels.ore,
        ChunkClassification::Plains => channels.hay,
        ChunkClassification::Mixed => {
            (channels.forest + channels.stone + channels.ore + channels.hay) * 0.25
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_render_is_deterministic_and_well_formed() {
        let dims = ChunkDims::new(31);
        let a = render_world_map_rgba(0xABCD, dims);
        let b = render_world_map_rgba(0xABCD, dims);
        assert_eq!(a, b, "same seed + dims must render byte-identical terrain");
        assert_eq!(
            a.len(),
            (WORLD_MAP_TEXELS as usize) * (WORLD_MAP_TEXELS as usize) * 4,
            "buffer must be width*height*4 RGBA bytes"
        );
        // Every alpha byte is opaque.
        assert!(a.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn world_bounds_are_square_and_centred() {
        // Medium (31) spans 31*64 = 1984 m, straddling the origin.
        let (min_x, min_z, max_x, max_z) = world_map_bounds(ChunkDims::new(31));
        assert_eq!(min_x, min_z);
        assert_eq!(max_x, max_z);
        assert!((max_x - min_x - 1984.0).abs() < 0.001);
        assert!(min_x < 0.0 && max_x > 0.0, "origin must be inside the map");
    }

    #[test]
    fn distinct_seeds_produce_distinct_maps() {
        let dims = ChunkDims::new(15);
        let a = render_world_map_rgba(1, dims);
        let b = render_world_map_rgba(2, dims);
        assert_ne!(a, b, "different seeds must differ");
    }
}
