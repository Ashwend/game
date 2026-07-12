//! Server-authoritative fuse state for a placed (or resting thrown) explosive
//! charge, plus the `tick_fuses` countdown that detonates one on zero.
//!
//! Mirrors the torch's burn loop ([`super::torch`]) but even simpler: a fuse
//! has no inventory and no lit flag. It counts `ticks_left` down every tick,
//! and when it reaches zero the charge detonates and is destroyed. The
//! countdown is entirely server-only: NOTHING about a fuse replicates as a
//! per-tick delta (unlike the torch, which flips a replicated `active` flag),
//! because the client renders the hiss/glow from the placed entity's immutable
//! kind and its own clock, and the detonation ships as a one-shot
//! `ServerMessage::Explosion` cue plus the entity despawn. A fuse persists so a
//! reload resumes the countdown where it left off.

use crate::save::PersistedFuseState;

use super::GameServer;

/// Per-charge fuse state, stored on `DeployedEntity::fuse`. Present only on an
/// armed `DeployableKind::Explosive`; every other kind carries `None`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FuseState {
    /// Server ticks of fuse remaining. Reaches 0 -> the charge detonates and is
    /// removed. Server-only: never enters the replication delta.
    pub(crate) ticks_left: u32,
}

impl FuseState {
    /// A freshly armed fuse with `ticks_left` ticks on the clock. A placed
    /// charge arms the instant it is set; a thrown bomb arms when it rests.
    pub(crate) fn armed(ticks_left: u32) -> Self {
        Self { ticks_left }
    }

    pub(crate) fn from_persisted(p: PersistedFuseState) -> Self {
        Self {
            ticks_left: p.ticks_left,
        }
    }

    pub(crate) fn to_persisted(self) -> PersistedFuseState {
        PersistedFuseState {
            ticks_left: self.ticks_left,
        }
    }
}

impl GameServer {
    /// Advance every armed fuse one tick and detonate any that reach zero.
    ///
    /// Two passes so the mutating detonation borrow does not overlap the fuse
    /// scan: first tick every fuse down and collect the ids that hit zero, then
    /// detonate each collected charge. Nothing is marked dirty during the tick
    /// pass (the countdown is server-only and must stay out of the replication
    /// delta); `detonate_charge` handles the entity despawn (which the mirror
    /// picks up) and the blast resolution.
    pub(in crate::server) fn tick_fuses(&mut self) -> Vec<super::ServerEnvelope> {
        // Collect the charges that reach zero this tick. `for_each_mut_then_mark`
        // returns `false` for every entry so no fuse ever enters the sync delta:
        // the steady countdown mutates a server-only field only.
        let mut detonating: Vec<crate::protocol::DeployedEntityId> = Vec::new();
        self.deployed_entities.for_each_mut_then_mark(|id, entity| {
            let Some(fuse) = entity.fuse.as_mut() else {
                return false;
            };
            fuse.ticks_left = fuse.ticks_left.saturating_sub(1);
            if fuse.ticks_left == 0 {
                detonating.push(*id);
            }
            false
        });

        let mut envelopes = Vec::new();
        for id in detonating {
            envelopes.extend(self.detonate_charge(id));
        }
        envelopes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_fuse_carries_its_full_window() {
        let fuse = FuseState::armed(160);
        assert_eq!(fuse.ticks_left, 160);
    }

    #[test]
    fn fuse_persist_round_trips() {
        let fuse = FuseState::armed(42);
        let restored = FuseState::from_persisted(fuse.to_persisted());
        assert_eq!(restored.ticks_left, 42);
    }
}
