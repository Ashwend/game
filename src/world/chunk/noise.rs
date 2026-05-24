//! Deterministic, seedable scalar noise used by the chunk generator.
//!
//! Two channels, both pure functions of `(seed, coord)`:
//!
//! - `value_noise_2d` — single-octave value noise smoothed with a quintic
//!   fade. Cheap; used as the building block for [`fbm`].
//! - `fbm` — fractional Brownian motion (sum of value-noise octaves at
//!   doubling frequencies). Output is squashed into `[0.0, 1.0]`.
//!
//! The generator also calls `splitmix64`, which is exposed so callers can
//! derive their own per-(seed, coord, kind) RNG streams without pulling in
//! `rand`.

const PERMUTATION_TABLE_SIZE: u32 = 256;

/// SplitMix64 finalizer. Used both as a hash for the noise lattice and as a
/// stream seed for the Poisson-disk sampler. Each input maps to a different
/// 64-bit value with good avalanche behavior — exactly what we need to fold
/// `(world_seed, chunk_x, chunk_z, kind_id)` down to a deterministic stream.
pub fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Hash a 2D integer lattice point. Picks a fixed permutation offset from
/// `seed` so worlds with different seeds get different lattice values, and
/// folds in `x`/`y` so each chunk cell hashes uniquely.
fn lattice_hash(seed: u64, x: i32, y: i32) -> f32 {
    let mixed = splitmix64(
        seed ^ ((x as i64 as u64).wrapping_mul(0x9E3779B97F4A7C15))
            ^ ((y as i64 as u64).wrapping_mul(0xC6BC279692B5C323)),
    );
    // Top 24 bits → [0, 1). 24 bits is enough resolution for the f32 mantissa
    // and avoids the bias an `as f32 / u64::MAX as f32` cast would carry.
    let bits = (mixed >> 40) as u32 & ((1 << 24) - 1);
    bits as f32 / (1u32 << 24) as f32
}

fn fade(t: f32) -> f32 {
    // Quintic smoothstep: 6t^5 − 15t^4 + 10t^3. Perlin's improved fade — C2
    // continuous, so fbm derivatives stay clean.
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Single-octave value noise at `(x, y)`. Output is in `[0.0, 1.0]`.
///
/// `seed` is folded into the lattice hash so two worlds with different
/// seeds produce different fields, and the same world produces the same
/// field every load.
pub fn value_noise_2d(seed: u64, x: f32, y: f32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - xi as f32;
    let yf = y - yi as f32;

    let v00 = lattice_hash(seed, xi, yi);
    let v10 = lattice_hash(seed, xi + 1, yi);
    let v01 = lattice_hash(seed, xi, yi + 1);
    let v11 = lattice_hash(seed, xi + 1, yi + 1);

    let u = fade(xf);
    let v = fade(yf);

    let x0 = lerp(v00, v10, u);
    let x1 = lerp(v01, v11, u);
    lerp(x0, x1, v)
}

/// Fractional Brownian motion: sum `octaves` octaves of value noise at
/// doubling frequencies and halving amplitudes. Output is normalized into
/// `[0.0, 1.0]` so callers can compare it against a threshold directly.
///
/// `frequency` is the input scale: smaller values stretch the features
/// wider across the world. For 64 m grids, frequencies on the order of
/// `1/200` produce features that span 3–4 chunks — about right for "this
/// region is a forest, that one is rocky."
pub fn fbm(seed: u64, x: f32, y: f32, frequency: f32, octaves: u32) -> f32 {
    let mut amplitude = 1.0_f32;
    let mut freq = frequency;
    let mut total = 0.0_f32;
    let mut max_amplitude = 0.0_f32;
    for octave in 0..octaves {
        // Stir the seed per-octave so subsequent octaves don't sample the
        // same lattice — otherwise the fold-in just doubles the same
        // pattern at higher frequency.
        let octave_seed = splitmix64(seed ^ ((octave as u64).wrapping_mul(0xD1B54A32D192ED03)));
        let value = value_noise_2d(octave_seed, x * freq, y * freq);
        total += value * amplitude;
        max_amplitude += amplitude;
        amplitude *= 0.5;
        freq *= 2.0;
    }
    if max_amplitude <= 0.0 {
        0.0
    } else {
        (total / max_amplitude).clamp(0.0, 1.0)
    }
}

/// Tiny LCG-flavored pseudo-RNG seeded by `splitmix64`. Cheap and
/// deterministic — used by the Poisson-disk sampler to pick candidate
/// offsets within a chunk without pulling in `rand` (which would add a
/// dependency for one consumer).
#[derive(Debug, Clone, Copy)]
pub struct ChunkRng {
    state: u64,
}

impl ChunkRng {
    pub fn from_components(world_seed: u64, x: i32, z: i32, stream: u32) -> Self {
        let mixed = splitmix64(
            world_seed
                ^ ((x as i64 as u64).wrapping_mul(0xA0761D6478BD642F))
                ^ ((z as i64 as u64).wrapping_mul(0xE7037ED1A0B428DB))
                ^ ((stream as u64).wrapping_mul(0x8EBC6AF09C88C6E3)),
        );
        Self { state: mixed | 1 }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = splitmix64(self.state);
        self.state
    }

    /// Uniform `[0.0, 1.0)`.
    pub fn next_unit(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32 & ((1 << 24) - 1);
        bits as f32 / (1u32 << 24) as f32
    }

    /// Uniform `[low, high)`.
    pub fn next_range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next_unit()
    }
}

const _: () = {
    // The permutation table size is referenced from a few places — keep
    // it exported as a compile-time constant rather than a magic literal.
    assert!(PERMUTATION_TABLE_SIZE.is_power_of_two());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_noise_is_deterministic_per_seed_and_position() {
        let a = value_noise_2d(123, 1.5, -7.25);
        let b = value_noise_2d(123, 1.5, -7.25);
        assert_eq!(a, b);
    }

    #[test]
    fn value_noise_differs_across_seeds() {
        let a = value_noise_2d(123, 1.5, -7.25);
        let b = value_noise_2d(124, 1.5, -7.25);
        assert!((a - b).abs() > f32::EPSILON);
    }

    #[test]
    fn value_noise_output_is_in_unit_range() {
        for x in -20..=20 {
            for y in -20..=20 {
                let value = value_noise_2d(42, x as f32 * 0.3, y as f32 * 0.3);
                assert!(
                    (0.0..=1.0).contains(&value),
                    "value_noise out of range at ({x}, {y}): {value}"
                );
            }
        }
    }

    #[test]
    fn fbm_output_is_in_unit_range_and_varies() {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for x in -10..=10 {
            for y in -10..=10 {
                let value = fbm(7, x as f32, y as f32, 0.1, 4);
                assert!((0.0..=1.0).contains(&value));
                min = min.min(value);
                max = max.max(value);
            }
        }
        // Should actually span a reasonable range, not collapse to a constant.
        assert!(max - min > 0.2, "fbm range too small: [{min}, {max}]");
    }

    #[test]
    fn chunk_rng_is_deterministic_per_components() {
        let mut a = ChunkRng::from_components(99, 3, -1, 5);
        let mut b = ChunkRng::from_components(99, 3, -1, 5);
        for _ in 0..8 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn chunk_rng_streams_are_independent_per_stream() {
        let mut a = ChunkRng::from_components(7, 0, 0, 0);
        let mut b = ChunkRng::from_components(7, 0, 0, 1);
        // First sample should differ — same seed/coord, different stream.
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn splitmix_avalanches() {
        let a = splitmix64(0x1234);
        let b = splitmix64(0x1235);
        // Single-bit input difference should diffuse — at least ~half the
        // output bits flip on average. Just assert it's not the identity.
        assert!((a ^ b).count_ones() > 10);
    }
}
