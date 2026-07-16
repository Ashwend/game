//! Server authority for the Tool Cupboard base-claim object.
//!
//! A Tool Cupboard is a free-standing deployable that must sit on a
//! building platform. While it stands it projects *building privilege*
//! over its base: the connected platform footprint plus a margin ring of
//! [`crate::game_balance::BUILDING_PRIVILEGE_MARGIN_CELLS`] cells. Inside
//! that region only the owner and the accounts on the cupboard's
//! authorized list (and admins) may place construction, with a raid
//! carve-out so a base can never fully trap a raider out (handled at the
//! building-placement call site, not here).
//!
//! Auth model (mirrors the door code-lock list, deliberately): the owner
//! is implicit and permanent (never in the list, never removable);
//! anyone within reach may authorize *themselves* by tapping E, the Rust
//! Tool Cupboard model where the real protection is keeping the cupboard
//! behind locked doors. The hold-E wheel additionally offers a clear.
//!
//! The privilege geometry is *foundation-projected*, not a sphere: the
//! claim grows from the building's grid footprint (reusing the same 3 m
//! cell grid the stability graph walks), so a raid base can't be wedged
//! against someone's wall the way a fixed radius would allow. The
//! footprint cache is rebuilt on every structural change (placement,
//! destroy, load) and on cupboard placement; see
//! [`GameServer::recompute_claim_footprints`].

use crate::{
    building::{
        ClaimPlatform, FOUNDATION_SIZE_M, claim_cells_cover, claim_footprint_cells,
        platform_top_offset,
    },
    game_balance::{
        BUILDING_PRIVILEGE_MARGIN_CELLS, DEPLOYABLE_DAMAGE_RANGE_M as INTERACT_RANGE_M,
    },
    items::DeployableKind,
    protocol::{AccountId, ClaimCommand, ClientId, DeployedEntityId, ToastKind},
};

use super::{GameServer, ServerEnvelope};

/// Authorized-account list for one Tool Cupboard, plus the transient upkeep
/// bookkeeping. The owner is *not* in the list (it lives on the entity);
/// clearing the list therefore can never lock the owner out.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CupboardState {
    pub(crate) authorized: Vec<AccountId>,
    /// Fractional upkeep owed carried between drain periods, one bucket per
    /// building tier (`[sticks-wood, hewn-logs, stone]`). Transient: never
    /// persisted, a restart forgives the sub-integer remainder.
    pub(crate) upkeep_carry: [f32; 3],
    /// Which tier buckets went unpaid on the last drain (their pieces are
    /// decaying). Drives the container panel's decay warning. Transient.
    pub(crate) upkeep_unpaid: [bool; 3],
}

impl CupboardState {
    pub(crate) fn from_persisted(p: crate::save::PersistedCupboardState) -> Self {
        Self {
            authorized: p.authorized,
            ..Default::default()
        }
    }

    pub(crate) fn to_persisted(&self) -> crate::save::PersistedCupboardState {
        crate::save::PersistedCupboardState {
            authorized: self.authorized.clone(),
        }
    }
}

impl GameServer {
    /// Route a [`ClaimCommand`] (tap-E toggle and hold-E wheel).
    pub(super) fn apply_claim_command(
        &mut self,
        client_id: ClientId,
        command: ClaimCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            ClaimCommand::AuthorizeSelf { id } => self.cupboard_set_self_auth(client_id, id, true),
            ClaimCommand::DeauthorizeSelf { id } => {
                self.cupboard_set_self_auth(client_id, id, false)
            }
            ClaimCommand::ClearList { id } => self.cupboard_clear_list(client_id, id),
        }
    }

    /// Add or remove the caller's own account from the cupboard's
    /// authorized list (tap E, or the wheel's self-toggle). Everyone is
    /// equal here, including the placer: the placer is auto-added to the
    /// list on placement but may toggle themselves off and back on like
    /// anyone else.
    fn cupboard_set_self_auth(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
        authorize: bool,
    ) -> Vec<ServerEnvelope> {
        let Some(account) = self.cupboard_actor_in_range(client_id, id) else {
            return Vec::new();
        };
        // `deployed_entity_mut` flags the entity dirty so the mirror ships
        // the `DeployableAuth` diff to everyone in the room.
        let Some(entity) = self.deployed_entity_mut(id) else {
            return Vec::new();
        };
        let Some(cupboard) = entity.cupboard.as_mut() else {
            return Vec::new();
        };
        if authorize {
            if cupboard.authorized.contains(&account) {
                return claim_toast(
                    client_id,
                    ToastKind::Info,
                    "Already authorized here".to_owned(),
                );
            }
            cupboard.authorized.push(account);
            claim_toast(
                client_id,
                ToastKind::Success,
                "Authorized on this cupboard".to_owned(),
            )
        } else {
            if !cupboard.authorized.contains(&account) {
                return claim_toast(
                    client_id,
                    ToastKind::Info,
                    "You weren't authorized here".to_owned(),
                );
            }
            cupboard.authorized.retain(|a| *a != account);
            claim_toast(
                client_id,
                ToastKind::Success,
                "Removed yourself from this cupboard".to_owned(),
            )
        }
    }

    /// Clear everyone *else* from the cupboard's authorized list (hold-E
    /// wheel). Any authorized account may do this; the caller stays
    /// authorized so they never lock themselves out.
    fn cupboard_clear_list(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        let Some(account) = self.cupboard_actor_in_range(client_id, id) else {
            return Vec::new();
        };
        if !self.cupboard_authorizes(id, account) {
            return claim_toast(
                client_id,
                ToastKind::Warning,
                "Only authorized players can clear the list".to_owned(),
            );
        }
        let Some(entity) = self.deployed_entity_mut(id) else {
            return Vec::new();
        };
        let Some(cupboard) = entity.cupboard.as_mut() else {
            return Vec::new();
        };
        // "Clear" means "deauthorize everyone else", so the caller keeps
        // their own access.
        if cupboard.authorized.iter().all(|authed| *authed == account) {
            return claim_toast(
                client_id,
                ToastKind::Info,
                "Nobody else is authorized".to_owned(),
            );
        }
        cupboard.authorized = vec![account];
        claim_toast(
            client_id,
            ToastKind::Success,
            "Cleared everyone else from the list".to_owned(),
        )
    }

    /// Range + existence gate shared by every cupboard interaction.
    /// Measured to the cupboard's collider surface, like doors. Returns
    /// the actor's account id.
    fn cupboard_actor_in_range(
        &self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Option<AccountId> {
        let client = self.clients.get(&client_id)?;
        let entity = self.deployed_entities.get(&id)?;
        if !matches!(entity.kind, DeployableKind::ToolCupboard) {
            return None;
        }
        super::deployables::within_horizontal_range_of_blocks(
            client.controller.position,
            &entity.resolved_collider_blocks(),
            INTERACT_RANGE_M,
        )
        .then_some(client.account_id)
    }

    /// True when `account` is on cupboard `id`'s authorized list. The
    /// placer is auto-added at placement, but is otherwise an ordinary
    /// member (they can remove themselves), so access is purely the list.
    pub(super) fn cupboard_authorizes(&self, id: DeployedEntityId, account: AccountId) -> bool {
        self.deployed_entities
            .get(&id)
            .and_then(|entity| entity.cupboard.as_ref())
            .is_some_and(|cupboard| cupboard.authorized.contains(&account))
    }

    /// True when a Tool Cupboard's claim covers `position` and `placer`
    /// is not allowed to build there. Allowed when authorized by *any*
    /// covering claim, so a cooperating ally with their own cupboard on
    /// the same base isn't blocked by a neighbour's. No admin bypass:
    /// claim authorization binds everyone; walk up and tap E to authorize.
    pub(super) fn claim_blocks_placement(
        &self,
        position: crate::protocol::Vec3Net,
        placer: AccountId,
    ) -> bool {
        let mut covered = false;
        for (cupboard_id, cells) in &self.claim_footprints {
            if !claim_cells_cover(cells, position) {
                continue;
            }
            covered = true;
            if self.cupboard_authorizes(*cupboard_id, placer) {
                return false;
            }
        }
        covered
    }

    /// Footprint-aware variant of [`Self::claim_blocks_placement`]: true
    /// when any of a placement's world-space collider `blocks` overlaps a
    /// claim `placer` isn't authorized for. Unlike the point test, this
    /// catches a piece whose *snap centre* sits just outside a claim but
    /// whose body (a foundation slab, a wall span) reaches into it, so a
    /// base can't be fenced in by butting models up against the boundary.
    pub(super) fn claim_blocks_footprint(
        &self,
        blocks: &[crate::world::WorldBlock],
        placer: AccountId,
    ) -> bool {
        let mut covered = false;
        for (cupboard_id, cells) in &self.claim_footprints {
            if !crate::building::claim_cells_overlap_blocks(cells, blocks) {
                continue;
            }
            covered = true;
            if self.cupboard_authorizes(*cupboard_id, placer) {
                return false;
            }
        }
        covered
    }

    /// Whether `account` may upgrade/demolish the building piece at
    /// `position`. Inside a cupboard claim, modify rights follow the
    /// claim's authorized list (shared base management), so an authorized
    /// teammate can demolish a piece they didn't place. Outside any claim
    /// it falls back to the original builder (`is_owner`). No admin bypass.
    pub(super) fn building_modify_allowed(
        &self,
        position: crate::protocol::Vec3Net,
        account: AccountId,
        is_owner: bool,
    ) -> bool {
        let mut covered = false;
        for (cupboard_id, cells) in &self.claim_footprints {
            if !claim_cells_cover(cells, position) {
                continue;
            }
            covered = true;
            if self.cupboard_authorizes(*cupboard_id, account) {
                return true;
            }
        }
        if covered { false } else { is_owner }
    }

    /// The player's Tool Cupboard standing at `position`, for the HUD's
    /// at-a-glance indicator: inside any claimed footprint at all, and if
    /// so, authorized on at least one covering cupboard. Same walk as
    /// [`Self::building_modify_allowed`], minus the outside-claim owner
    /// fallback (outside every claim the HUD shows nothing).
    pub(super) fn claim_status_at(
        &self,
        position: crate::protocol::Vec3Net,
        account: AccountId,
    ) -> crate::server::PlayerClaimStatus {
        let mut inside_claim = false;
        let mut authorized = false;
        for (cupboard_id, cells) in &self.claim_footprints {
            if !claim_cells_cover(cells, position) {
                continue;
            }
            inside_claim = true;
            if self.cupboard_authorizes(*cupboard_id, account) {
                authorized = true;
                break;
            }
        }
        crate::server::PlayerClaimStatus {
            inside_claim,
            authorized,
        }
    }

    /// Rebuild the per-cupboard claim footprint cache. Called on every
    /// structural change (via [`Self::refresh_structural_stability`]) and
    /// after a cupboard is placed. The footprint is the connected
    /// platform cells of the base the cupboard sits on, grown by the
    /// margin ring, stored as real XZ cell centres so the gate can do a
    /// cheap point-in-cell test.
    pub(super) fn recompute_claim_footprints(&mut self) {
        self.claim_footprints.clear();

        // Every foundation/ceiling top is a platform the claim can project
        // from; gather them once for the shared geometry helper.
        let platforms: Vec<ClaimPlatform> = self
            .deployed_entities
            .values()
            .filter_map(|entity| {
                let DeployableKind::Building { piece, .. } = entity.kind else {
                    return None;
                };
                let top = platform_top_offset(piece)?;
                Some(ClaimPlatform {
                    position: entity.position,
                    top: entity.position.y + top,
                })
            })
            .collect();

        let cupboards: Vec<(DeployedEntityId, crate::protocol::Vec3Net)> = self
            .deployed_entities
            .values()
            .filter(|entity| matches!(entity.kind, DeployableKind::ToolCupboard))
            .map(|entity| (entity.id, entity.position))
            .collect();
        for (id, position) in cupboards {
            let cells =
                claim_footprint_cells(&platforms, position, BUILDING_PRIVILEGE_MARGIN_CELLS);
            self.claim_footprints.insert(id, cells);
        }
    }

    /// True when `position` sits on a building platform (foundation or
    /// ceiling top), as opposed to bare ground. Tool Cupboards require
    /// this so their claim has a base to project from.
    pub(super) fn on_building_platform(&self, position: crate::protocol::Vec3Net) -> bool {
        let half = FOUNDATION_SIZE_M / 2.0;
        self.deployed_entities.values().any(|entity| {
            let DeployableKind::Building { piece, .. } = entity.kind else {
                return false;
            };
            let Some(top) = platform_top_offset(piece) else {
                return false;
            };
            (entity.position.y + top - position.y).abs() <= 0.05
                && (entity.position.x - position.x).abs() <= half
                && (entity.position.z - position.z).abs() <= half
        })
    }
}

fn claim_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    super::toasts::toast(client_id, kind, text)
}
