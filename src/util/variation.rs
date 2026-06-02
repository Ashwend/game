//! Anti-repeat variant picker used by audio systems that play one of
//! several similar clips per event (footsteps, impact sounds, etc.). Each
//! fire deterministically hashes a counter into the clip pool while
//! skipping the previously-played index, so no clip repeats consecutively
//! and every other clip stays equally likely.

use super::hash::mix32;

/// Pick a variant index in `0..count` that isn't the same as `*last_index`.
///
/// `fire_count` is advanced in place so each call gets a fresh hash;
/// `last_index` is updated to the returned pick. A `count` of `0` is a
/// caller bug (no pool to pick from), the function returns `0` and does
/// not touch `last_index`. A `count` of `1` always returns `0` since
/// there's no other clip to alternate to.
pub fn pick_variant_index(
    fire_count: &mut u32,
    last_index: &mut Option<usize>,
    count: usize,
) -> usize {
    if count == 0 {
        return 0;
    }
    *fire_count = fire_count.wrapping_add(1);
    if count == 1 {
        *last_index = Some(0);
        return 0;
    }
    let hashed = mix32(*fire_count) as usize;
    let mut pick = hashed % count;
    if let Some(last) = *last_index
        && pick == last
    {
        // Collapse the "same as last" slot onto the wrap-around end so
        // each of the other `count - 1` clips still has equal weight.
        pick = (pick + 1) % count;
    }
    *last_index = Some(pick);
    pick
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_repeats_consecutively() {
        let mut fire_count = 0;
        let mut last = None;
        let count = 7;
        let mut previous = pick_variant_index(&mut fire_count, &mut last, count);
        for _ in 0..200 {
            let pick = pick_variant_index(&mut fire_count, &mut last, count);
            assert_ne!(pick, previous, "variant repeated consecutively");
            assert!(pick < count);
            previous = pick;
        }
    }

    #[test]
    fn visits_every_variant_over_time() {
        let mut fire_count = 0;
        let mut last = None;
        let count = 9;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            seen.insert(pick_variant_index(&mut fire_count, &mut last, count));
        }
        assert_eq!(
            seen.len(),
            count,
            "should cover every variant over many fires"
        );
    }

    #[test]
    fn handles_single_variant_pool() {
        let mut fire_count = 0;
        let mut last = None;
        for _ in 0..10 {
            assert_eq!(pick_variant_index(&mut fire_count, &mut last, 1), 0);
        }
        assert_eq!(last, Some(0));
    }

    #[test]
    fn handles_empty_pool_safely() {
        let mut fire_count = 0;
        let mut last = None;
        assert_eq!(pick_variant_index(&mut fire_count, &mut last, 0), 0);
        assert_eq!(fire_count, 0, "empty-pool call must not burn a hash");
        assert_eq!(last, None, "empty-pool call must not record an index");
    }

    #[test]
    fn three_variant_pool_alternates_cleanly() {
        // Specific shape we care about for impact sounds: 3 clips, fire
        // many times, every consecutive pair must differ.
        let mut fire_count = 0;
        let mut last = None;
        let mut previous = pick_variant_index(&mut fire_count, &mut last, 3);
        for _ in 0..50 {
            let pick = pick_variant_index(&mut fire_count, &mut last, 3);
            assert_ne!(pick, previous);
            assert!(pick < 3);
            previous = pick;
        }
    }
}
