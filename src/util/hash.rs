//! Cheap deterministic integer mixers shared across modules.
//!
//! `mix32` is the classic Murmur3 finalizer. It produces a well-distributed
//! `u32` from any 32-bit seed and is fast enough to call inline. We use it for
//! impact-chip spread, admin-spawned ore placement, and node-set fingerprints
//! — anywhere we want "feels different per input" without dragging in the
//! `rand` crate.

/// Mix a 32-bit seed into a well-distributed `u32`. Identical inputs yield
/// identical outputs (deterministic), so callers can reproduce sequences by
/// reusing the same seed.
#[inline]
pub fn mix32(seed: u32) -> u32 {
    let mut x = seed.wrapping_add(0x9E3779B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EBCA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2AE35);
    x ^= x >> 16;
    x
}

/// `mix32` result mapped to `[0, 1)`. 24 bits of mantissa precision is
/// plenty for picking world positions, spread angles, and similar.
#[inline]
pub fn hashed_unit(seed: u32) -> f32 {
    (mix32(seed) & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix32_is_deterministic() {
        assert_eq!(mix32(7), mix32(7));
        assert_ne!(mix32(7), mix32(8));
    }

    #[test]
    fn hashed_unit_stays_in_unit_interval_and_varies() {
        for seed in 0..200u32 {
            let value = hashed_unit(seed);
            assert!((0.0..1.0).contains(&value));
        }
        assert_ne!(hashed_unit(1), hashed_unit(2));
        assert_ne!(hashed_unit(100), hashed_unit(101));
    }
}
