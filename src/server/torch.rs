//! Server-authoritative torch state: the lit flag plus a burn-down timer.
//!
//! Mirrors the furnace's burn loop ([`super::furnace`]) but far simpler: a
//! torch has no inventory, it just counts `burn_ticks_left` down while lit and
//! goes dark when it reaches zero. Only the `active` (lit) flag is replicated
//! to clients, through `DeployableActive`; the remaining burn time is
//! server-only (and persisted so a reload resumes the countdown).

use crate::game_balance::TORCH_BURN_TICKS;
use crate::save::PersistedTorchState;

use super::GameServer;

/// Per-torch operational state, stored on `DeployedEntity::torch`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TorchState {
    /// Lit / unlit. Replicated as `DeployableActive`; drives the client's
    /// flame + light rig.
    pub(crate) active: bool,
    /// Server ticks of burn remaining. Reaches 0 → the torch goes dark.
    pub(crate) burn_ticks_left: u32,
}

impl TorchState {
    /// A freshly placed, lit torch with a full burn budget.
    pub(crate) fn new() -> Self {
        Self {
            active: true,
            burn_ticks_left: TORCH_BURN_TICKS,
        }
    }

    pub(crate) fn from_persisted(p: PersistedTorchState) -> Self {
        Self {
            active: p.active,
            burn_ticks_left: p.burn_ticks_left,
        }
    }

    pub(crate) fn to_persisted(self) -> PersistedTorchState {
        PersistedTorchState {
            active: self.active,
            burn_ticks_left: self.burn_ticks_left,
        }
    }
}

impl GameServer {
    /// Advance every lit torch one tick; extinguish any that burn out and
    /// flag them dirty so the `active = false` flip replicates.
    ///
    /// Mirror-sync note: like the furnace, only the `active` flip ships
    /// (`DeployableActive`), so a torch that is simply still burning never
    /// enters the sync delta, the steady countdown is server-only.
    pub(in crate::server) fn tick_torches(&mut self) {
        // Mark dirty only torches whose `active` flag flips off this tick; the
        // steady countdown mutates a server-only field and must stay out of the
        // replication delta.
        self.deployed_entities
            .for_each_mut_then_mark(|_id, entity| {
                let Some(torch) = entity.torch.as_mut() else {
                    return false;
                };
                if !torch.active {
                    return false;
                }
                torch.burn_ticks_left = torch.burn_ticks_left.saturating_sub(1);
                if torch.burn_ticks_left == 0 {
                    torch.active = false;
                    true
                } else {
                    false
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::{DeployableKind, TORCH_ID, intern_item_id},
        protocol::{DeployedEntityId, Vec3Net},
        server::deployables::DeployedEntity,
    };

    fn place_lit_torch(server: &mut GameServer, burn_ticks_left: u32) -> DeployedEntityId {
        let id = server.next_deployed_entity_id;
        server.next_deployed_entity_id.0 += 1;
        server.insert_deployed_entity(
            id,
            DeployedEntity {
                id,
                item_id: intern_item_id(TORCH_ID),
                kind: DeployableKind::Torch { wall: false },
                position: Vec3Net::ZERO,
                yaw: 0.0,
                health: 60,
                max_health: 60,
                owner: Some(crate::protocol::AccountId(1)),
                furnace: None,
                placed_at_tick: 0,
                door: None,
                label: None,
                stability: 100,
                storage: None,
                torch: Some(TorchState {
                    active: true,
                    burn_ticks_left,
                }),
                cupboard: None,
                ruin_cache: None,
                fuse: None,
            },
        );
        id
    }

    #[test]
    fn a_fresh_torch_is_lit_with_a_full_burn() {
        let torch = TorchState::new();
        assert!(torch.active);
        assert_eq!(torch.burn_ticks_left, TORCH_BURN_TICKS);
    }

    #[test]
    fn a_lit_torch_counts_down_without_re_replicating() {
        let mut server = crate::server::test_support::server();
        let id = place_lit_torch(&mut server, 10);
        let _ = server.drain_deployable_sync();

        server.tick_torches();

        let torch = server.deployed_entities[&id].torch.as_ref().unwrap();
        assert!(torch.active, "still lit");
        assert_eq!(torch.burn_ticks_left, 9, "one tick of burn consumed");
        let (dirty, removed) = server.drain_deployable_sync();
        assert!(
            dirty.is_empty() && removed.is_empty(),
            "a steady burn must not re-enter the sync delta"
        );
    }

    #[test]
    fn a_torch_burns_out_and_marks_itself_for_replication() {
        let mut server = crate::server::test_support::server();
        let id = place_lit_torch(&mut server, 1);
        let _ = server.drain_deployable_sync();

        server.tick_torches();

        let torch = server.deployed_entities[&id].torch.as_ref().unwrap();
        assert!(!torch.active, "burned out → dark");
        assert_eq!(torch.burn_ticks_left, 0);
        let (dirty, _removed) = server.drain_deployable_sync();
        assert_eq!(dirty, vec![id], "the extinguish flip must replicate once");
    }
}
