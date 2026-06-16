//! A `HashMap` that records which entries changed since the last drain, for the
//! per-component replication mirror sync.
//!
//! The mirror-sync systems in `net::host` ship only the *delta* of each
//! authoritative map each tick. That requires every mutation to flag the
//! affected id "dirty" (added / changed, so re-sync the mirror entity) or
//! "removed" (gone, so despawn the mirror entity). Getting this wrong is a
//! silent stale-replication bug: the value changes server-side but the diff
//! never ships, with no compile error, panic, or failing test.
//!
//! This newtype makes "mutation marks dirty" a property of the type rather than
//! of author discipline. There is no way to obtain `&mut` to a value without
//! the id being flagged, except through the explicit [`for_each_mut_then_mark`]
//! path, which the furnace / torch / dropped-item-physics ticks use because
//! they mutate *server-only* fields (burn countdown, smelt progress, physics
//! pose) every tick and must mark only the entries whose *replicated* field
//! actually flipped, keeping idle entities out of the delta.
//!
//! Entity-specific side effects (collider mirroring, chunk anchoring) stay on
//! the `GameServer` wrapper methods (`insert_*` / `remove_*`); this type owns
//! only the dirty/removed bookkeeping.
//!
//! [`for_each_mut_then_mark`]: DirtyTrackedMap::for_each_mut_then_mark

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ops::{Deref, Index};

#[derive(Debug)]
pub(crate) struct DirtyTrackedMap<Id, T> {
    map: HashMap<Id, T>,
    dirty: HashSet<Id>,
    removed: HashSet<Id>,
}

// Read access is open: deref to the inner map so every `HashMap` `&self` method
// (`get`, `iter`, `values`, `keys`, `len`, `contains_key`, ...) and the
// `&DirtyTrackedMap -> &HashMap` coercion (for the read-only `&HashMap`
// consumers) work transparently. There is deliberately NO `DerefMut`: mutable
// access is only available through the inherent marking methods below, so a
// caller cannot obtain `&mut` to a value without the id being flagged.
impl<Id, T> Deref for DirtyTrackedMap<Id, T> {
    type Target = HashMap<Id, T>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<Id: Eq + Hash, T> Index<&Id> for DirtyTrackedMap<Id, T> {
    type Output = T;

    fn index(&self, id: &Id) -> &T {
        &self.map[id]
    }
}

impl<Id: Copy + Eq + Hash, T> Default for DirtyTrackedMap<Id, T> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            dirty: HashSet::new(),
            removed: HashSet::new(),
        }
    }
}

impl<Id: Copy + Eq + Hash, T> DirtyTrackedMap<Id, T> {
    /// Wrap an already-built map. Nothing is flagged dirty yet; callers that
    /// need the first mirror sync to spawn every entry (world load) call
    /// [`Self::seed_all_dirty`] after.
    pub(crate) fn from_map(map: HashMap<Id, T>) -> Self {
        Self {
            map,
            dirty: HashSet::new(),
            removed: HashSet::new(),
        }
    }

    // Read access is provided by the `Deref`/`Index` impls above.

    // ---- marking mutators ----

    /// Insert (or replace) a value, flagging the id dirty for the next sync.
    pub(crate) fn insert(&mut self, id: Id, value: T) {
        self.map.insert(id, value);
        self.dirty.insert(id);
        self.removed.remove(&id);
    }

    /// Remove a value, flagging the id removed (the sync despawns the mirror
    /// entity). Returns the removed value, mirroring `HashMap::remove`.
    pub(crate) fn remove(&mut self, id: &Id) -> Option<T> {
        let removed = self.map.remove(id);
        if removed.is_some() {
            self.removed.insert(*id);
            self.dirty.remove(id);
        }
        removed
    }

    /// Mutable access, conservatively flagging the id dirty (any `&mut`
    /// hand-out may change the value; the sync's value-compare suppresses
    /// no-op diffs, so a spurious mark just costs one no-op delta entry).
    pub(crate) fn get_mut(&mut self, id: &Id) -> Option<&mut T> {
        if self.map.contains_key(id) {
            self.dirty.insert(*id);
            self.removed.remove(id);
        }
        self.map.get_mut(id)
    }

    /// Flag an existing id dirty without handing out `&mut`. Used after a
    /// caller has mutated through [`Self::for_each_mut_then_mark`] or otherwise
    /// knows a replicated field flipped. No-op for unknown ids.
    pub(crate) fn mark_dirty(&mut self, id: &Id) {
        if self.map.contains_key(id) {
            self.dirty.insert(*id);
            self.removed.remove(id);
        }
    }

    /// Iterate every value mutably WITHOUT auto-marking, flagging dirty only
    /// the entries for which `f` returns `true`. The controlled escape hatch
    /// for the per-tick ticks (furnace, torch, dropped-item physics) that
    /// mutate server-only fields every tick and must keep idle entities out of
    /// the replication delta, marking only when a replicated field flips.
    pub(crate) fn for_each_mut_then_mark(&mut self, mut f: impl FnMut(&Id, &mut T) -> bool) {
        for (id, value) in self.map.iter_mut() {
            if f(id, value) {
                self.dirty.insert(*id);
                self.removed.remove(id);
            }
        }
    }

    /// Read-only view of the currently-dirty ids (not yet drained). Used by the
    /// dropped-item tick to re-anchor exactly the items the physics step moved,
    /// without exposing the ability to mutate the dirty set.
    pub(crate) fn dirty_ids(&self) -> impl Iterator<Item = &Id> {
        self.dirty.iter()
    }

    /// Drain the accumulated deltas: `(dirty ids, removed ids)`. Called once
    /// per tick by the matching mirror-sync system.
    pub(crate) fn drain_sync(&mut self) -> (Vec<Id>, Vec<Id>) {
        (self.dirty.drain().collect(), self.removed.drain().collect())
    }

    /// Re-flag ids dirty so the next sync reprocesses them (the resource-node
    /// spawn-budget requeue). Re-inserting is idempotent; ids whose entry has
    /// since been removed are dropped harmlessly by the sync's value filter.
    pub(crate) fn requeue_dirty(&mut self, ids: impl IntoIterator<Item = Id>) {
        self.dirty.extend(ids);
    }

    /// Seed every live id dirty so the first sync pass spawns all mirror
    /// entities once (world load on connect). After that only mutated ids are
    /// reprocessed.
    pub(crate) fn seed_all_dirty(&mut self) {
        let ids: Vec<Id> = self.map.keys().copied().collect();
        self.dirty.extend(ids);
    }

    /// Test-only: remove every entry, flagging each removed for the next sync.
    /// Used by tests that wipe the world-generated nodes to isolate one fixture
    /// node. `cfg(test)` so it is absent (not dead code) in release builds.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        for id in self.map.keys().copied().collect::<Vec<_>>() {
            self.removed.insert(id);
            self.dirty.remove(&id);
        }
        self.map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drained(map: &mut DirtyTrackedMap<u64, i32>) -> (Vec<u64>, Vec<u64>) {
        let (mut dirty, mut removed) = map.drain_sync();
        dirty.sort_unstable();
        removed.sort_unstable();
        (dirty, removed)
    }

    #[test]
    fn insert_flags_dirty() {
        let mut map = DirtyTrackedMap::default();
        map.insert(1, 10);
        map.insert(2, 20);
        assert_eq!(drained(&mut map), (vec![1, 2], vec![]));
        // Drained: nothing dirty until the next mutation.
        assert_eq!(drained(&mut map), (vec![], vec![]));
    }

    #[test]
    fn get_mut_flags_dirty_but_plain_get_does_not() {
        let mut map = DirtyTrackedMap::default();
        map.insert(1, 10);
        let _ = drained(&mut map);
        assert_eq!(map.get(&1), Some(&10));
        assert_eq!(drained(&mut map), (vec![], vec![]), "read must not dirty");
        *map.get_mut(&1).unwrap() = 11;
        assert_eq!(drained(&mut map), (vec![1], vec![]), "get_mut must dirty");
        // get_mut on a missing id flags nothing.
        assert!(map.get_mut(&99).is_none());
        assert_eq!(drained(&mut map), (vec![], vec![]));
    }

    #[test]
    fn remove_flags_removed_and_clears_dirty() {
        let mut map = DirtyTrackedMap::default();
        map.insert(1, 10);
        let _ = drained(&mut map);
        *map.get_mut(&1).unwrap() = 11; // dirty
        assert_eq!(map.remove(&1), Some(11));
        // Removed wins, the stale dirty mark is cleared so the sync despawns
        // rather than trying to refresh a gone entity.
        assert_eq!(drained(&mut map), (vec![], vec![1]));
        // Removing a missing id is a no-op delta.
        assert_eq!(map.remove(&1), None);
        assert_eq!(drained(&mut map), (vec![], vec![]));
    }

    #[test]
    fn reinsert_after_remove_clears_removed() {
        let mut map = DirtyTrackedMap::default();
        map.insert(1, 10);
        let _ = drained(&mut map);
        map.remove(&1);
        map.insert(1, 99);
        // Net effect: present and dirty, not removed.
        assert_eq!(drained(&mut map), (vec![1], vec![]));
    }

    #[test]
    fn for_each_mut_then_mark_marks_only_flipped_entries() {
        let mut map = DirtyTrackedMap::default();
        map.insert(1, 10);
        map.insert(2, 20);
        map.insert(3, 30);
        let _ = drained(&mut map);
        // Mutate every value (a "server-only" change), but only report 2 and 3
        // as having flipped a replicated field.
        map.for_each_mut_then_mark(|id, value| {
            *value += 1;
            *id != 1
        });
        assert_eq!(
            drained(&mut map),
            (vec![2, 3], vec![]),
            "only entries whose closure returned true should dirty"
        );
        // The values were all mutated regardless of the mark.
        assert_eq!(map.get(&1), Some(&11));
    }

    #[test]
    fn seed_all_dirty_flags_every_live_id() {
        let mut map = DirtyTrackedMap::from_map([(1, 10), (2, 20)].into_iter().collect());
        // from_map starts clean.
        assert_eq!(drained(&mut map), (vec![], vec![]));
        map.seed_all_dirty();
        assert_eq!(drained(&mut map), (vec![1, 2], vec![]));
    }

    #[test]
    fn requeue_dirty_reflags_for_the_next_pass() {
        let mut map = DirtyTrackedMap::default();
        map.insert(1, 10);
        let _ = drained(&mut map);
        map.requeue_dirty([1, 2]);
        assert_eq!(drained(&mut map), (vec![1, 2], vec![]));
    }
}
