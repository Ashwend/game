//! Terrain ground-texturing data: a small per-world raster of soft biome blend
//! weights, baked from the same chunk-classification noise the in-game world map
//! uses. The GPU terrain material (`assets/shaders/terrain.wgsl`) samples it to
//! splat-blend the four per-biome ground textures, so the textured floor and the
//! map agree on where each biome sits.
//!
//! Like [`crate::world::map_texture`], this is a **pure function of
//! `(world_seed, floor_size)`** and is computed on the client. Where the map
//! raster stores a flat argmax biome colour, this stores *soft* per-biome weights
//! so the ground cross-fades between biomes instead of hard-cutting.

use super::ClassificationChannels;

/// Side length (texels) of the square biome-weight raster. 512 keeps biome
/// borders crisp even on the largest map (4032 m -> ~7.9 m/texel, so the GPU's
/// bilinear tap smears a border over only ~8 m rather than ~16 m at 256). That,
/// plus the narrow blend band below, is what lets a small biome surrounded by
/// others still show its own ground texture instead of being all transition.
pub const TERRAIN_WEIGHT_TEXELS: u32 = 512;

/// Blend width in classification channel-space. A biome interior, whose winning
/// channel leads the runners-up by more than this, renders as a single pure biome
/// (matching the map's flat regions); within this band of a rival channel the two
/// ground textures cross-fade. ~0.03 works out to roughly a 12-18 m border at the
/// current feature scale, narrow enough that small biomes read as themselves, not
/// a permanent cross-fade.
const TERRAIN_BIOME_BLEND_BAND: f32 = 0.03;

/// Soft biome blend weights `[forest, rocky, ore, plains]` at a point. The four
/// values sum to one and come from the four classification channels, so the
/// ground tracks the same biome layout the world map shows. Channels trailing the
/// leader by more than [`TERRAIN_BIOME_BLEND_BAND`] drop out entirely (crisp
/// interiors); the rest are squared to sharpen the cross-fade, then normalised.
/// Returns an even blend for a degenerate all-zero sample so the result is always
/// convex.
pub fn biome_blend_weights(channels: ClassificationChannels) -> [f32; 4] {
    // Bias to match the biome label (`ClassificationChannels::classify`), so
    // the green-leaning biome distribution and the ground splat agree.
    let channels = channels.biased();
    let c = [channels.forest, channels.stone, channels.ore, channels.hay];
    let cmax = c.iter().copied().fold(0.0_f32, f32::max);
    let floor = cmax - TERRAIN_BIOME_BLEND_BAND;

    let mut w = [0.0_f32; 4];
    let mut sum = 0.0_f32;
    for (i, &channel) in c.iter().enumerate() {
        let above = (channel - floor).max(0.0);
        let sharpened = above * above;
        w[i] = sharpened;
        sum += sharpened;
    }

    if sum > 1e-6 {
        for weight in &mut w {
            *weight /= sum;
        }
    } else {
        w = [0.25; 4];
    }
    w
}

/// Render the `TERRAIN_WEIGHT_TEXELS²` RGBA8 biome-weight raster for a world.
///
/// The square covers the ground plane exactly: centred on the origin with side
/// `floor_size` (`[-floor_size/2, floor_size/2]` on both axes), matching the
/// origin-centred ground mesh, so the shader maps world XZ to UV with a plain
/// `xz / floor_size + 0.5`. Channels are `R = forest, G = rocky, B = ore,
/// A = plains` blend weights. Row 0 is the north edge (`-floor_size/2` in Z),
/// increasing rows run south, the same convention as [`super::map_texture`].
pub fn render_terrain_weight_rgba(world_seed: u64, floor_size: f32) -> Vec<u8> {
    let texels = TERRAIN_WEIGHT_TEXELS;
    let mut rgba = vec![0u8; (texels * texels * 4) as usize];
    fill_terrain_weight_rows(world_seed, floor_size, 0, texels, &mut rgba);
    rgba
}

/// Bake the biome-weight texels for rows `row_start..row_end` into `out`.
///
/// `out` must be exactly `(row_end - row_start) * TERRAIN_WEIGHT_TEXELS * 4`
/// bytes, the contiguous RGBA slice for that horizontal band (row-major, row 0
/// north). Because rows are independent and depend only on `(world_seed,
/// floor_size)`, a caller can split the raster into disjoint bands and bake them
/// in parallel; the concatenated result is byte-identical to a single serial
/// [`render_terrain_weight_rgba`] pass. The whole 512² bake is ~2.1M value-noise
/// evaluations and runs once on world load, so the bevy-side scene builder fans
/// it across the compute task pool rather than stalling one frame on it. Stays a
/// pure function (no bevy) to keep this domain module engine-free.
pub fn fill_terrain_weight_rows(
    world_seed: u64,
    floor_size: f32,
    row_start: u32,
    row_end: u32,
    out: &mut [u8],
) {
    let texels = TERRAIN_WEIGHT_TEXELS;
    let half = floor_size * 0.5;
    debug_assert_eq!(
        out.len(),
        ((row_end - row_start) * texels * 4) as usize,
        "out slice must cover exactly rows {row_start}..{row_end}"
    );

    let mut i = 0usize;
    for py in row_start..row_end {
        let wz = -half + (py as f32 + 0.5) / texels as f32 * floor_size;
        for px in 0..texels {
            let wx = -half + (px as f32 + 0.5) / texels as f32 * floor_size;
            let w = biome_blend_weights(ClassificationChannels::sample_at(world_seed, wx, wz));
            out[i] = (w[0] * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 1] = (w[1] * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 2] = (w[2] * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 3] = (w[3] * 255.0).round().clamp(0.0, 255.0) as u8;
            i += 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_weights_sum_to_one() {
        let channels = ClassificationChannels {
            forest: 0.7,
            stone: 0.4,
            ore: 0.55,
            hay: 0.3,
        };
        let w = biome_blend_weights(channels);
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "weights must sum to 1, got {sum}");
    }

    #[test]
    fn clear_leader_renders_as_pure_biome() {
        // Forest leads the runner-up by well over the blend band, so the point is
        // 100% forest (the interior crispness that mirrors the map's flat region).
        let channels = ClassificationChannels {
            forest: 0.9,
            stone: 0.5,
            ore: 0.4,
            hay: 0.3,
        };
        let w = biome_blend_weights(channels);
        assert_eq!(w, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn near_tie_cross_fades_between_the_two() {
        // Forest and plains within the blend band *after the green bias*: both
        // contribute, the others (far below) drop out. Raw hay leads slightly
        // because forest's larger bias weight (`BIOME_BIAS`) flips the order, so
        // post-bias forest is the marginal leader and keeps the larger share.
        let channels = ClassificationChannels {
            forest: 0.80,
            stone: 0.50,
            ore: 0.40,
            hay: 0.86,
        };
        let w = biome_blend_weights(channels);
        assert!(w[0] > 0.0 && w[3] > 0.0, "both leaders contribute: {w:?}");
        assert_eq!(w[1], 0.0, "rocky trails the band and drops out");
        assert_eq!(w[2], 0.0, "ore trails the band and drops out");
        assert!(
            w[0] > w[3],
            "the marginal leader keeps the larger share: {w:?}"
        );
    }

    #[test]
    fn degenerate_sample_falls_back_to_even_blend() {
        let channels = ClassificationChannels {
            forest: 0.0,
            stone: 0.0,
            ore: 0.0,
            hay: 0.0,
        };
        assert_eq!(biome_blend_weights(channels), [0.25; 4]);
    }

    #[test]
    fn raster_is_deterministic_and_well_formed() {
        let a = render_terrain_weight_rgba(0xC0FFEE, 1984.0);
        let b = render_terrain_weight_rgba(0xC0FFEE, 1984.0);
        assert_eq!(a, b, "same seed + floor must render byte-identical weights");
        assert_eq!(
            a.len(),
            (TERRAIN_WEIGHT_TEXELS as usize) * (TERRAIN_WEIGHT_TEXELS as usize) * 4,
            "buffer must be texels*texels*4 RGBA bytes"
        );
    }

    #[test]
    fn distinct_seeds_produce_distinct_weights() {
        let a = render_terrain_weight_rgba(1, 960.0);
        let b = render_terrain_weight_rgba(2, 960.0);
        assert_ne!(a, b, "different seeds must differ");
    }

    #[test]
    fn banded_fill_matches_serial_render() {
        // The parallel bake splits the raster into row bands and fills each into
        // a disjoint slice. Concatenating bands (in row order) must reproduce the
        // single-pass serial output exactly, otherwise the parallel path would
        // produce a different ground than the deterministic reference.
        let seed = 0xABCDEF;
        let floor = 1536.0;
        let serial = render_terrain_weight_rgba(seed, floor);

        let texels = TERRAIN_WEIGHT_TEXELS;
        let row_bytes = (texels * 4) as usize;
        let mut banded = vec![0u8; serial.len()];
        // Deliberately uneven, non-divisor band size to exercise the tail band.
        let band_rows = 37u32;
        let mut row_start = 0u32;
        while row_start < texels {
            let row_end = (row_start + band_rows).min(texels);
            let off = row_start as usize * row_bytes;
            let end = row_end as usize * row_bytes;
            fill_terrain_weight_rows(seed, floor, row_start, row_end, &mut banded[off..end]);
            row_start = row_end;
        }
        assert_eq!(banded, serial, "banded fill must equal the serial raster");
    }
}
