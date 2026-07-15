//! Explosive taxonomy: the per-item [`ExplosiveProfile`] a blackpowder
//! explosive carries on its [`ItemDefinition`](crate::items::ItemDefinition),
//! and the [`ExplosiveKind`]
//! selector that keys the effectiveness matrix and the client VFX/SFX cue.
//!
//! An explosive is a pure raiding object: it declares its base damage, its
//! blast radius, its fuse window, and how it is delivered (placed on the
//! ground or thrown). It gathers nothing and does no melee
//! damage (no `ToolProfile`, no `WeaponProfile`). Every `ItemDefinition`
//! defaults `explosive: None`, so nothing changes for existing items.
//!
//! The damage the profile deals against a given structure material is NOT a
//! field here: it is the matrix `explosive_effectiveness_pct(kind, material)`
//! in [`super::materials`], so a single table row expresses "a keg does 80% of
//! its base against hewn wood but 25% against stone", exactly the raid-balance
//! lever `tool_effectiveness_pct` is for tools.

/// Which of the three blackpowder explosives an item is. The 1-byte selector
/// keys the effectiveness matrix (`explosive_effectiveness_pct`), the balance
/// constants in `game_balance`, and the client VFX/SFX (it rides
/// `ServerMessage::Explosion` and, for the thrown bomb, the projectile
/// identity). Serde-derived because it travels on the wire in both of those
/// places; a 1-byte enum, never an item-id string.
///
/// APPEND-ONLY where it is serialised (the wire `Explosion` message, the
/// thrown-projectile identity, and the persisted `DeployableKind::Explosive`):
/// new kinds go at the end and never reorder. (A retired fourth kind
/// occupied the tail slot, so dropping it kept the first three stable.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ExplosiveKind {
    /// Thrown handful of powder. The cheap starter charge: small damage and
    /// radius, lit on the throw so it blows wherever it lands and rolls.
    /// Shreds sticks bases and chips hewn wood; useless against stone or metal.
    PowderBomb,
    /// Placed barrel of powder. The workhorse breaching charge: several kegs
    /// take down a hewn-wood wall, a couple chip stone. Nothing against metal.
    PowderKeg,
    /// Placed satchel of packed charges. The tier-2 breacher: real numbers
    /// against stone, and the only charge that scratches an iron door at all.
    SatchelCharge,
}

impl ExplosiveKind {
    /// Every [`ExplosiveKind`] variant, so the effectiveness-matrix and
    /// registry completeness tests can assert each is covered. Adding a variant
    /// is a compile error in the exhaustive matches until it is listed here.
    pub const ALL: &'static [ExplosiveKind] = &[
        ExplosiveKind::PowderBomb,
        ExplosiveKind::PowderKeg,
        ExplosiveKind::SatchelCharge,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::PowderBomb => "Powder Bomb",
            Self::PowderKeg => "Powder Keg",
            Self::SatchelCharge => "Satchel Charge",
        }
    }

    /// The registry item id for this charge kind. The single mapping from a
    /// placed charge's kind back to the item that crafts and drops it, shared by
    /// the blast profile lookup and the defuse refund (which needs the recipe
    /// keyed on this id). Kept beside [`Self::label`] so a new kind lights up
    /// both from one place.
    pub const fn item_id(self) -> &'static str {
        match self {
            Self::PowderBomb => crate::items::POWDER_BOMB_ID,
            Self::PowderKeg => crate::items::POWDER_KEG_ID,
            Self::SatchelCharge => crate::items::SATCHEL_CHARGE_ID,
        }
    }
}

/// How an explosive is delivered into the world, which decides its placement
/// path (or that it has none, for the thrown bomb).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplosiveDelivery {
    /// Set on the ground under normal deployable surface rules (keg, satchel).
    Placed,
    /// Thrown along an aim vector via the projectile sim, fuse lit on the
    /// throw (the powder bomb). Never placed as a deployable.
    Thrown,
}

/// Combat + delivery stats for a blackpowder explosive, carried on
/// `ItemDefinition` beside `tool`/`weapon`/`ranged`/`armor`. Resolved by the
/// server placement / throw path; the effectiveness-per-material multiplier is
/// the separate `explosive_effectiveness_pct` matrix, not a field here. Every
/// number is authoritative and lives in `game_balance`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExplosiveProfile {
    /// Which of the three explosives this is: keys the effectiveness matrix,
    /// the wire VFX cue, and (for a thrown bomb) the projectile identity.
    pub kind: ExplosiveKind,
    /// Base blast damage at ground zero, before the per-material effectiveness
    /// multiplier and the linear distance falloff. The matrix percentages in
    /// the spec (e.g. keg 720 vs a hewn wall) are `base_damage * pct / 100`.
    pub base_damage: u32,
    /// Blast radius, in metres. Full `base_damage` at the centre, falling off
    /// linearly to zero at the edge (both against structures and players).
    pub radius_m: f32,
    /// Fuse length, in server ticks, from arming to detonation. A placed charge
    /// arms the moment it is set; a thrown bomb is lit on the throw.
    pub fuse_ticks: u32,
    /// How this explosive reaches the world (placed or thrown).
    pub delivery: ExplosiveDelivery,
    /// Charge HP: a placed charge is a fizzleable deployable. Reaching 0 through
    /// the normal deployable damage path FIZZLES it (destroyed, no detonation,
    /// no refund), the defender's counterplay. `None` would mean indestructible;
    /// every charge has a value.
    pub max_health: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explosive_kind_round_trips_through_postcard() {
        for &kind in ExplosiveKind::ALL {
            let bytes = postcard::to_allocvec(&kind).expect("serialize ExplosiveKind");
            let decoded: ExplosiveKind =
                postcard::from_bytes(&bytes).expect("deserialize ExplosiveKind");
            assert_eq!(decoded, kind, "{kind:?} did not round-trip");
        }
    }

    #[test]
    fn explosive_kind_all_lists_every_variant() {
        // The exhaustive match makes adding a variant a compile error until it
        // is slotted into `ALL`, and the count guards a dropped/duplicate entry.
        let expected = |kind: ExplosiveKind| match kind {
            ExplosiveKind::PowderBomb | ExplosiveKind::PowderKeg | ExplosiveKind::SatchelCharge => {
                true
            }
        };
        assert!(ExplosiveKind::ALL.iter().all(|&k| expected(k)));
        assert_eq!(ExplosiveKind::ALL.len(), 3);
    }
}
