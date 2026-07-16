//! Server-authoritative explosion resolution: the shared `resolve_explosion`
//! every blast funnels through, and `detonate_charge`, which fires it for a
//! placed (or thrown-then-rested) charge.
//!
//! ## What an explosion touches
//!
//! `resolve_explosion(center, kind)` generalises the players-only
//! `resolve_blast_on_players` (built in P5c for the meteor shower) additively.
//! For each kind of thing in the blast radius:
//!
//! - **Players** (including the placer/thrower, per the resolved design decision
//!   that own-charge self-damage counts): the existing `resolve_blast_on_players`
//!   helper, so Correction / knockback / death / the blast armor column all flow
//!   through the shared `apply_player_damage` tail unchanged.
//! - **Building pieces, doors, and deployables**: `base_damage *
//!   explosive_effectiveness_pct(kind, material) * linear_falloff`, applied
//!   through the existing deployable damage internals (`destroy_deployed_entity`
//!   at 0 HP) so a destroyed piece spills its contents, collapses what it held up
//!   (`refresh_structural_stability`), and mirrors correctly. Indestructibles
//!   (the ruin cache) stay exempt via their existing kind guard.
//! - **Resource nodes**: deliberately untouched. A charge is a raiding tool, not
//!   a mining tool; a stray blast next to an ore vein must not free-mine it. (The
//!   meteor shower DOES deplete nodes, but that is the meteor's own impact
//!   path, not this shared function.)
//!
//! Finally it emits one cosmetic `ServerMessage::Explosion { position, kind }`
//! to clients within a generous range for the VFX/SFX (consumed by the explosive
//! VFX package; the authoritative damage already landed through the mirrors).
//!
//! The meteor shower keeps calling `resolve_blast_on_players` DIRECTLY, not
//! this function: its siting guarantees no structures are ever in range, so the
//! players-only pass is the correct (and cheaper) one for it.

use crate::{
    combat::DamageKind,
    game_balance::EXPLOSION_CUE_RANGE_M,
    items::{ExplosiveKind, explosive_effectiveness_pct},
    protocol::{DeployedEntityId, ServerMessage, Vec3Net},
};

use super::{GameServer, ServerEnvelope};

impl GameServer {
    /// Detonate the charge `id`: resolve its blast at its position and remove the
    /// charge WITHOUT spilling contents (a charge has none) and without a fizzle
    /// toast (it did its job). No-op if the id is not a live explosive charge.
    /// Called by `tick_fuses` when a fuse reaches zero.
    pub(super) fn detonate_charge(&mut self, id: DeployedEntityId) -> Vec<ServerEnvelope> {
        // Read the charge's kind and centre before removing it.
        let Some(entity) = self.deployed_entities.get(&id) else {
            return Vec::new();
        };
        let crate::items::DeployableKind::Explosive { kind } = entity.kind else {
            return Vec::new();
        };
        let center = entity.position;

        // Remove the charge first so it is not caught in its own structure pass
        // (and so it can't detonate twice). No content spill (a charge stores
        // nothing) and no stability run for the charge itself; the structure
        // pass below runs stability once for everything it destroys.
        self.remove_deployed_entity_tracked(id);

        self.resolve_explosion(center, kind)
    }

    /// Resolve a spherical explosion of `kind` centred at `center`: damage
    /// players, building pieces, doors, and deployables in range (each with
    /// linear falloff), leave resource nodes untouched, and emit the cosmetic
    /// VFX/SFX cue. See the module docs for the full contract.
    pub(super) fn resolve_explosion(
        &mut self,
        center: Vec3Net,
        kind: ExplosiveKind,
    ) -> Vec<ServerEnvelope> {
        let Some(profile) = charge_profile(kind) else {
            return Vec::new();
        };
        let radius = profile.radius_m;
        let base = profile.base_damage;

        let mut envelopes = Vec::new();

        // (a) Players. The shared players-only helper handles falloff, the blast
        // armor column, Correction/knockback/death, and self-damage (the owner is
        // NOT excluded, per the resolved design decision). `base` is the ground
        // zero blast; the helper does the linear falloff to the edge.
        envelopes.extend(self.resolve_blast_on_players(
            center,
            radius,
            base as f32,
            DamageKind::Blast,
        ));

        // (b) Structures: building pieces, doors, and other deployables. Snapshot
        // the (id, damage) pairs first so the mutating destroy calls below don't
        // hold a borrow across the deployable map. A ruin cache (indestructible)
        // is skipped by the shared deployable damage guard, so it is safe to hand
        // it here; the damage helper early-returns on it.
        envelopes.extend(self.resolve_explosion_on_structures(center, radius, base, kind));

        // (c) Resource nodes: intentionally NOT touched. A charge does not mine.

        // (d) Cosmetic VFX/SFX cue to nearby clients (consumed by the explosive
        // VFX package). No `except`: everyone in range, including the placer, sees
        // and hears the blast.
        envelopes.extend(self.envelopes_within_range(
            center,
            EXPLOSION_CUE_RANGE_M,
            None,
            ServerMessage::Explosion {
                position: center,
                kind,
            },
        ));

        envelopes
    }

    /// The structure pass of `resolve_explosion`: damage every building piece,
    /// door, and deployable whose collider surface is inside `radius`, each by
    /// `base * effectiveness_pct(kind, material) * linear_falloff`, through the
    /// deployable damage internals (destroy + spill + stability at 0 HP).
    fn resolve_explosion_on_structures(
        &mut self,
        center: Vec3Net,
        radius: f32,
        base: u32,
        kind: ExplosiveKind,
    ) -> Vec<ServerEnvelope> {
        if radius <= 0.0 || base == 0 {
            return Vec::new();
        }
        // Snapshot (id, damage) so the destroy loop doesn't borrow the map.
        let mut hits: Vec<(DeployedEntityId, u32)> = Vec::new();
        for entity in self.deployed_entities.values() {
            // Indestructibles (the ruin cache) never take blast damage; the
            // deployable damage helper guards this too, but skipping here avoids
            // a wasted call and keeps the falloff math off an immune target.
            if matches!(entity.kind, crate::items::DeployableKind::RuinCache) {
                continue;
            }
            let distance = nearest_surface_distance(center, entity);
            if distance > radius {
                continue;
            }
            let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
            let pct = explosive_effectiveness_pct(kind, entity.kind.material());
            // base * pct/100 * falloff, in that order, on integer-friendly math.
            let scaled = (base as u64).saturating_mul(pct as u64) / 100;
            let damage = (scaled as f32 * falloff).round() as u32;
            if damage == 0 {
                continue;
            }
            hits.push((entity.id, damage));
        }

        let mut envelopes = Vec::new();
        for (id, damage) in hits {
            envelopes.extend(self.damage_deployable_by(id, damage));
        }
        envelopes
    }

    /// Apply `damage` to the deployable `id` through the shared destroy internals:
    /// subtract HP, and on 0 destroy it (spill contents, collapse dependents via
    /// `refresh_structural_stability`). Skips the ruin cache (indestructible) and
    /// a charge's own fizzle toast (a charge caught in a blast just detonates its
    /// own fuse normally, or is removed by the primary detonation). Returns no
    /// envelopes today (the health/removal replicate through the mirror), but the
    /// signature returns a `Vec` so a future consequence can ride along.
    fn damage_deployable_by(&mut self, id: DeployedEntityId, damage: u32) -> Vec<ServerEnvelope> {
        let Some(entity) = self.deployed_entities.get(&id) else {
            return Vec::new();
        };
        if matches!(entity.kind, crate::items::DeployableKind::RuinCache) {
            return Vec::new();
        }
        let Some(entity) = self.deployed_entity_mut(id) else {
            return Vec::new();
        };
        entity.health = entity.health.saturating_sub(damage);
        let dead = entity.health == 0;
        // Stamp the combat-damage tick for the repair lockout (`damage` is
        // always nonzero here; the structure pass filters zero hits).
        self.recently_damaged.insert(id, self.tick);
        if dead {
            // Same destroy path a swing/projectile takes: spill + stability.
            self.destroy_deployed_entity(id);
        }
        Vec::new()
    }
}

/// The [`crate::items::ExplosiveProfile`] behind a charge kind, looked up from
/// the registry via the item id. `None` only if the registry lost the row.
fn charge_profile(kind: ExplosiveKind) -> Option<crate::items::ExplosiveProfile> {
    crate::items::item_definition(kind.item_id()).and_then(|def| def.explosive)
}

/// The horizontal distance from `center` to the nearest point on the
/// deployable's collider surface, matching how the melee/projectile damage gates
/// measure to a structure (the entity centre can sit out of range while its
/// 3 m foundation edge is right at the blast). Falls back to the entity centre
/// when the collider can't be resolved.
fn nearest_surface_distance(center: Vec3Net, entity: &super::deployables::DeployedEntity) -> f32 {
    let blocks = entity.resolved_collider_blocks();
    if blocks.is_empty() {
        return center.horizontal_distance_squared(entity.position).sqrt();
    }
    let mut nearest_sq = f32::MAX;
    for block in blocks {
        let min = block.min();
        let max = block.max();
        // Closest point on the AABB footprint to the blast centre (XZ only, so a
        // charge on the ground and a wall it is set against read the same).
        let cx = center.x.clamp(min.x, max.x);
        let cz = center.z.clamp(min.z, max.z);
        let dx = center.x - cx;
        let dz = center.z - cz;
        let d = dx * dx + dz * dz;
        if d < nearest_sq {
            nearest_sq = d;
        }
    }
    nearest_sq.sqrt()
}
