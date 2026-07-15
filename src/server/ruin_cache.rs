//! Server-authoritative ruin-cache state: the refill scheduler and the seeded
//! loot table.
//!
//! A ruin cache is a world-spawned deployable ([`crate::items::DeployableKind::RuinCache`])
//! that anyone can loot. It stores its loot in the shared storage-box slot grid
//! (`DeployedEntity::storage`), so opening and item moves ride the exact same
//! container plumbing as a placed storage box (`ClientMessage::OpenStorageBox`
//! -> `OpenContainer::StorageBox`). This module owns only the extra behaviour a
//! cache has on top of a box: it **refills** on a timer once emptied.
//!
//! Refill flow (driven by [`GameServer::tick_ruin_caches`]):
//! 1. When a cache is fully emptied and no refill is scheduled, schedule one
//!    `RUIN_CACHE_REFILL_TICKS` in the future.
//! 2. When that tick arrives, roll the loot table into the slots, bump the
//!    refill counter, and clear the schedule.
//!
//! The loot roll is a pure, deterministic function of `(cache_id, refill_counter)`
//! via `splitmix64`, so a given cache's Nth refill is reproducible in tests and
//! identical on every host.

use crate::{
    game_balance::{
        RUIN_CACHE_CLOTH_PER_ROLL, RUIN_CACHE_FITTINGS_MAX, RUIN_CACHE_FITTINGS_MIN,
        RUIN_CACHE_GUNPOWDER_PER_ROLL, RUIN_CACHE_IRON_BAR_PER_ROLL,
        RUIN_CACHE_METEORITE_CHANCE_PCT, RUIN_CACHE_REFILL_TICKS, RUIN_CACHE_SECONDARY_ROLLS,
        RUIN_CACHE_SLOT_COUNT, RUIN_CACHE_WEIGHT_CLOTH, RUIN_CACHE_WEIGHT_GUNPOWDER,
        RUIN_CACHE_WEIGHT_IRON_BAR,
    },
    items::{CLOTH_ID, GUNPOWDER_ID, IRON_BAR_ID, METEORITE_ALLOY_ID, SALVAGED_FITTINGS_ID},
    protocol::{DeployedEntityId, ItemStack},
    save::PersistedRuinCacheState,
    server::GameServer,
    world::splitmix64,
};

/// Server-only refill bookkeeping for a ruin cache. The loot itself lives in
/// `DeployedEntity::storage` (the shared storage-box grid); this carries only
/// the refill schedule and the counter that seeds each refill's roll.
#[derive(Debug, Clone, Default)]
pub(crate) struct RuinCacheState {
    /// Tick at which a scheduled refill fires, or `None` when no refill is
    /// pending (the cache has loot, or was just filled).
    pub(crate) refill_at_tick: Option<u64>,
    /// How many times this cache has refilled. Part of the loot seed so a
    /// cache's successive refills differ (and each is reproducible).
    pub(crate) refill_counter: u64,
}

impl RuinCacheState {
    pub(crate) fn to_persisted(&self) -> PersistedRuinCacheState {
        PersistedRuinCacheState {
            refill_at_tick: self.refill_at_tick,
            refill_counter: self.refill_counter,
        }
    }

    pub(crate) fn from_persisted(persisted: PersistedRuinCacheState) -> Self {
        Self {
            refill_at_tick: persisted.refill_at_tick,
            refill_counter: persisted.refill_counter,
        }
    }
}

/// True when every slot in a cache grid is empty.
fn slots_empty(slots: &[Option<ItemStack>]) -> bool {
    slots.iter().all(Option::is_none)
}

/// Advance one cache's refill state by a tick. Pure so it can be unit-tested
/// without a `GameServer`: given the cache's server-only state, its loot slots,
/// the cache id, and the current tick, it schedules or fires a refill, mutating
/// both in place. Returns `true` when the slots changed (a refill fired), so
/// the caller can flag the deployable dirty.
pub(crate) fn tick_one_ruin_cache(
    state: &mut RuinCacheState,
    slots: &mut Vec<Option<ItemStack>>,
    cache_id: DeployedEntityId,
    now_tick: u64,
) -> bool {
    let empty = slots_empty(slots);
    match state.refill_at_tick {
        // Loot present and no refill pending: nothing to do. If a refill was
        // scheduled but the cache is no longer empty (a refill fired last tick
        // or an item was returned), cancel the schedule.
        None => {
            if empty {
                // Newly emptied: schedule a refill.
                state.refill_at_tick = Some(now_tick.saturating_add(RUIN_CACHE_REFILL_TICKS));
            }
            false
        }
        Some(fire_tick) => {
            if !empty {
                // Someone put loot back (or a refill already filled it): drop
                // the pending schedule so we don't overwrite a stocked cache.
                state.refill_at_tick = None;
                return false;
            }
            if now_tick < fire_tick {
                return false;
            }
            // Fire: roll the loot table into the (empty) slots.
            let loot = roll_loot(cache_id, state.refill_counter);
            fill_slots(slots, loot);
            state.refill_counter = state.refill_counter.wrapping_add(1);
            state.refill_at_tick = None;
            true
        }
    }
}

/// Deposit the rolled stacks into the cache's slot grid, one stack per slot,
/// truncating at the grid size (the table never rolls more stacks than slots).
fn fill_slots(slots: &mut Vec<Option<ItemStack>>, loot: Vec<ItemStack>) {
    // Normalise the grid to the cache size first (defensive against a resized
    // persisted grid), then drop each stack into the next free slot.
    slots.clear();
    slots.resize(RUIN_CACHE_SLOT_COUNT, None);
    for (slot, stack) in slots.iter_mut().zip(loot) {
        *slot = Some(stack);
    }
}

/// A tiny deterministic RNG walk seeded from `(cache_id, refill_counter)`. Not
/// `ChunkRng` (that is chunk-keyed); a bare `splitmix64` walk is the right
/// primitive for a per-cache, per-refill stream, mirroring the ruin scatter.
struct LootRng {
    state: u64,
}

impl LootRng {
    fn new(cache_id: DeployedEntityId, refill_counter: u64) -> Self {
        let seed = splitmix64(
            cache_id.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ refill_counter.wrapping_mul(0xC6BC_2796_92B5_C323)
                ^ 0x2011_C0DE_CACE_2011,
        );
        Self { state: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = splitmix64(self.state);
        self.state
    }

    /// Uniform integer in `[low, high]` inclusive.
    fn range_inclusive(&mut self, low: u32, high: u32) -> u32 {
        if high <= low {
            return low;
        }
        let span = (high - low + 1) as u64;
        low + (self.next_u64() % span) as u32
    }

    /// Uniform in `[0, 100)`.
    fn percent(&mut self) -> u32 {
        (self.next_u64() % 100) as u32
    }
}

/// Roll the ruin-cache loot table for `(cache_id, refill_counter)`. Guarantees
/// `salvaged_fittings` (the cache's exclusive source) plus weighted secondary
/// rolls and a rare meteorite. Pure and deterministic so tests can pin it.
pub(crate) fn roll_loot(cache_id: DeployedEntityId, refill_counter: u64) -> Vec<ItemStack> {
    let mut rng = LootRng::new(cache_id, refill_counter);
    let mut loot: Vec<ItemStack> = Vec::new();

    // Salvaged fittings ALWAYS, between the configured bounds.
    let fittings = rng.range_inclusive(RUIN_CACHE_FITTINGS_MIN, RUIN_CACHE_FITTINGS_MAX);
    loot.push(ItemStack::new(SALVAGED_FITTINGS_ID, fittings as u16));

    // Weighted secondary rolls: gunpowder / iron_bar / cloth.
    let total_weight =
        RUIN_CACHE_WEIGHT_GUNPOWDER + RUIN_CACHE_WEIGHT_IRON_BAR + RUIN_CACHE_WEIGHT_CLOTH;
    for _ in 0..RUIN_CACHE_SECONDARY_ROLLS {
        if total_weight == 0 {
            break;
        }
        let pick = (rng.next_u64() % total_weight as u64) as u32;
        let stack = if pick < RUIN_CACHE_WEIGHT_GUNPOWDER {
            ItemStack::new(GUNPOWDER_ID, RUIN_CACHE_GUNPOWDER_PER_ROLL)
        } else if pick < RUIN_CACHE_WEIGHT_GUNPOWDER + RUIN_CACHE_WEIGHT_IRON_BAR {
            ItemStack::new(IRON_BAR_ID, RUIN_CACHE_IRON_BAR_PER_ROLL)
        } else {
            ItemStack::new(CLOTH_ID, RUIN_CACHE_CLOTH_PER_ROLL)
        };
        loot.push(stack);
    }

    // Rare meteorite bonus.
    if rng.percent() < RUIN_CACHE_METEORITE_CHANCE_PCT {
        loot.push(ItemStack::new(METEORITE_ALLOY_ID, 1));
    }

    loot
}

/// A freshly spawned cache is stocked immediately (rolled with refill_counter 0)
/// so the first visit to a ruin finds loot. Used by the world-gen spawn path.
pub(crate) fn initial_cache_slots(cache_id: DeployedEntityId) -> Vec<Option<ItemStack>> {
    let mut slots = vec![None; RUIN_CACHE_SLOT_COUNT];
    fill_slots(&mut slots, roll_loot(cache_id, 0));
    slots
}

impl GameServer {
    /// Advance every ruin cache one tick: schedule a refill when a cache is
    /// emptied, and fire the refill when its timer arrives. Called once per
    /// server tick, next to `tick_furnaces` / `tick_torches`.
    ///
    /// Mirror-sync note: cache loot lives in the storage slot grid, which
    /// reaches viewers through the per-player `open_loot_bag` view (rebuilt each
    /// tick from `entity.storage`), not through a replicated deployable
    /// component. So a refill does not strictly need a dirty mark to reach an
    /// open viewer, but flagging it dirty on a refill is harmless and keeps the
    /// change observable to the deployable mirror, so we report it.
    pub(in crate::server) fn tick_ruin_caches(&mut self) {
        let now = self.tick;
        self.deployed_entities.for_each_mut_then_mark(|id, entity| {
            let Some(cache) = entity.ruin_cache.as_mut() else {
                return false;
            };
            let Some(storage) = entity.storage.as_mut() else {
                return false;
            };
            tick_one_ruin_cache(cache, &mut storage.slots, *id, now)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roll_always_yields_fittings_within_bounds() {
        for cache_id in 1..50u64 {
            for counter in 0..5u64 {
                let loot = roll_loot(cache_id, counter);
                let fittings: u16 = loot
                    .iter()
                    .filter(|s| s.item_id.as_ref() == SALVAGED_FITTINGS_ID)
                    .map(|s| s.quantity)
                    .sum();
                assert!(
                    (RUIN_CACHE_FITTINGS_MIN as u16..=RUIN_CACHE_FITTINGS_MAX as u16)
                        .contains(&fittings),
                    "cache {cache_id} refill {counter}: fittings {fittings} out of bounds"
                );
            }
        }
    }

    #[test]
    fn roll_is_deterministic_per_cache_and_counter() {
        let a = roll_loot(7, 3);
        let b = roll_loot(7, 3);
        assert_eq!(a.len(), b.len());
        for (sa, sb) in a.iter().zip(b.iter()) {
            assert_eq!(sa.item_id, sb.item_id);
            assert_eq!(sa.quantity, sb.quantity);
        }
        // A different counter should (very likely) differ somewhere.
        let c = roll_loot(7, 4);
        assert!(a != c, "successive refills of a cache should differ");
    }

    #[test]
    fn secondary_weights_are_nonzero_and_sum_positive() {
        let total =
            RUIN_CACHE_WEIGHT_GUNPOWDER + RUIN_CACHE_WEIGHT_IRON_BAR + RUIN_CACHE_WEIGHT_CLOTH;
        assert!(total > 0, "secondary loot weights must sum positive");
    }

    #[test]
    fn roll_never_exceeds_the_slot_count() {
        // fittings (1 stack) + secondary rolls + at most one meteorite must fit
        // the grid, or `fill_slots` would silently drop loot.
        let max_stacks = 1 + RUIN_CACHE_SECONDARY_ROLLS as usize + 1;
        assert!(
            max_stacks <= RUIN_CACHE_SLOT_COUNT,
            "loot table can produce {max_stacks} stacks but the cache has {RUIN_CACHE_SLOT_COUNT} slots"
        );
        for cache_id in 1..20u64 {
            let loot = roll_loot(cache_id, 0);
            assert!(loot.len() <= RUIN_CACHE_SLOT_COUNT);
        }
    }

    #[test]
    fn meteorite_is_rare_but_present_across_many_rolls() {
        let mut ember = 0usize;
        let trials = 2000usize;
        for i in 0..trials {
            let loot = roll_loot(i as u64, (i as u64) % 7);
            if loot
                .iter()
                .any(|s| s.item_id.as_ref() == METEORITE_ALLOY_ID)
            {
                ember += 1;
            }
        }
        let pct = 100.0 * ember as f32 / trials as f32;
        // Should land in the neighbourhood of the configured chance, not 0 and
        // not everywhere.
        assert!(
            (2.0..20.0).contains(&pct),
            "meteorite appeared in {pct:.1}% of rolls (expected ~{RUIN_CACHE_METEORITE_CHANCE_PCT}%)"
        );
    }

    #[test]
    fn empty_cache_schedules_then_fires_a_refill() {
        let mut state = RuinCacheState::default();
        let mut slots: Vec<Option<ItemStack>> = vec![None; RUIN_CACHE_SLOT_COUNT];

        // Tick once while empty: schedules a refill, no loot yet.
        let changed = tick_one_ruin_cache(&mut state, &mut slots, 1, 100);
        assert!(!changed);
        let fire = state.refill_at_tick.expect("a refill should be scheduled");
        assert_eq!(fire, 100 + RUIN_CACHE_REFILL_TICKS);
        assert!(
            slots.iter().all(Option::is_none),
            "no loot before the timer"
        );

        // Tick just before the fire tick: still nothing.
        assert!(!tick_one_ruin_cache(&mut state, &mut slots, 1, fire - 1));
        assert!(slots.iter().all(Option::is_none));

        // Tick at the fire tick: refill fires.
        let fired = tick_one_ruin_cache(&mut state, &mut slots, 1, fire);
        assert!(fired, "refill should fire at the scheduled tick");
        assert!(
            state.refill_at_tick.is_none(),
            "schedule cleared after fire"
        );
        assert_eq!(state.refill_counter, 1);
        assert!(
            slots.iter().any(Option::is_some),
            "the cache should hold loot after a refill"
        );
        assert!(
            slots
                .iter()
                .flatten()
                .any(|s| s.item_id.as_ref() == SALVAGED_FITTINGS_ID),
            "a refill always includes salvaged fittings"
        );
    }

    #[test]
    fn a_stocked_cache_does_not_schedule_a_refill() {
        let mut state = RuinCacheState::default();
        let mut slots = initial_cache_slots(5);
        // Cache has loot: ticking must not schedule anything.
        let changed = tick_one_ruin_cache(&mut state, &mut slots, 5, 500);
        assert!(!changed);
        assert!(
            state.refill_at_tick.is_none(),
            "a stocked cache never schedules a refill"
        );
    }

    #[test]
    fn returning_loot_cancels_a_pending_refill() {
        let mut state = RuinCacheState::default();
        let mut slots: Vec<Option<ItemStack>> = vec![None; RUIN_CACHE_SLOT_COUNT];
        // Emptied -> schedule.
        tick_one_ruin_cache(&mut state, &mut slots, 9, 10);
        assert!(state.refill_at_tick.is_some());
        // Someone drops a stack back in before the timer.
        slots[0] = Some(ItemStack::new(GUNPOWDER_ID, 1));
        let changed = tick_one_ruin_cache(&mut state, &mut slots, 9, 20);
        assert!(!changed);
        assert!(
            state.refill_at_tick.is_none(),
            "returning loot should cancel the pending refill"
        );
    }

    #[test]
    fn initial_slots_are_stocked_with_fittings() {
        let slots = initial_cache_slots(3);
        assert!(
            slots
                .iter()
                .flatten()
                .any(|s| s.item_id.as_ref() == SALVAGED_FITTINGS_ID),
            "a freshly spawned cache should already hold fittings"
        );
        assert_eq!(slots.len(), RUIN_CACHE_SLOT_COUNT);
    }
}
