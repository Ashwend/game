//! Armor taxonomy: the per-item [`ArmorProfile`] a wearable piece carries on its
//! [`ItemDefinition`], plus the pure per-kind mitigation computation the server
//! and the client prediction share.
//!
//! An armor piece is a pure defensive object: it declares which slot it fits,
//! which mesh the rig renders, its per-damage-kind protection percentages, and
//! its durability. It gathers nothing and does no damage (no `ToolProfile`, no
//! `WeaponProfile`). Every `ItemDefinition` defaults `armor: None`, so nothing
//! changes for existing items or for a player with an empty paperdoll.

use crate::combat::DamageKind;
use crate::protocol::{EquipmentSlot, ItemStack};

pub use crate::game_balance::ARMOR_TOTAL_CAP_PCT;

use super::visual::ArmorMesh;
use super::{ItemDefinition, item_definition};

/// Per-item defensive profile. Resolved from the worn stack's item definition
/// when the mitigation totals are recomputed; the numbers themselves live in
/// `game_balance.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmorProfile {
    /// Which paperdoll slot this piece may be worn in. A move into any other
    /// [`crate::protocol::ItemContainer::Equipment`] slot is rejected.
    pub slot: EquipmentSlot,
    /// The rig mesh a remote client renders for this worn piece.
    pub mesh: ArmorMesh,
    /// Percent of incoming melee (`Blunt`) damage this piece absorbs while its
    /// durability is above zero.
    pub melee_protection_pct: u8,
    /// Percent of incoming projectile damage this piece absorbs.
    pub projectile_protection_pct: u8,
    /// Percent of incoming blast damage this piece absorbs.
    pub blast_protection_pct: u8,
    /// Hits this piece survives before it stops protecting, or `None` for a
    /// piece that never wears. Mirrors [`super::ToolProfile::max_durability`].
    pub max_durability: Option<u32>,
}

impl ArmorProfile {
    /// This piece's protection against one damage `kind`, in percent. The single
    /// place damage kind maps to the profile's per-kind field, so the mitigation
    /// sum and the durability-wear check read the same value.
    pub fn protection_for(self, kind: DamageKind) -> u8 {
        match kind {
            DamageKind::Blunt => self.melee_protection_pct,
            DamageKind::Projectile => self.projectile_protection_pct,
            DamageKind::Blast => self.blast_protection_pct,
        }
    }
}

/// Per-damage-kind mitigation percentages a player currently has from worn
/// armor, each already capped at [`ARMOR_TOTAL_CAP_PCT`]. Recomputed whenever
/// equipment can change (a successful equip/unequip move, connect/restore,
/// durability wear, death) and fed to the damage path by kind.
///
/// `Default` is all-zero, exactly the mitigation of a bare player, so an empty
/// paperdoll reduces damage by nothing (identical to pre-armor behaviour).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArmorProtection {
    pub melee: u8,
    pub projectile: u8,
    pub blast: u8,
}

impl ArmorProtection {
    /// The mitigation percentage to apply for one damage `kind`.
    pub fn for_kind(self, kind: DamageKind) -> u8 {
        match kind {
            DamageKind::Blunt => self.melee,
            DamageKind::Projectile => self.projectile,
            DamageKind::Blast => self.blast,
        }
    }
}

/// Compute a player's per-kind [`ArmorProtection`] from their worn
/// `equipment_slots`. Sums each kind across every worn piece whose durability is
/// above zero (a broken piece stays equipped but contributes nothing), then
/// clamps each column to [`ARMOR_TOTAL_CAP_PCT`]. A non-armor item somehow
/// sitting in an equipment slot resolves to no [`ArmorProfile`] and is ignored,
/// so it can never mitigate.
///
/// Pure: takes the worn slots directly (no `GameServer`, no ECS), so the server
/// and any client-side mirror compute the identical value.
pub fn equipped_protection(equipment_slots: &[Option<ItemStack>]) -> ArmorProtection {
    protection_from_profiles(worn_armor_profiles(equipment_slots))
}

/// Sum each damage kind across `profiles` and clamp every column to
/// [`ARMOR_TOTAL_CAP_PCT`]. The core of [`equipped_protection`], split out so a
/// test can feed synthetic over-cap profiles and assert the clamp without
/// needing registry items at contrived percentages.
pub fn protection_from_profiles(profiles: impl Iterator<Item = ArmorProfile>) -> ArmorProtection {
    let mut melee = 0u32;
    let mut projectile = 0u32;
    let mut blast = 0u32;

    for profile in profiles {
        melee += u32::from(profile.melee_protection_pct);
        projectile += u32::from(profile.projectile_protection_pct);
        blast += u32::from(profile.blast_protection_pct);
    }

    let cap = u32::from(ARMOR_TOTAL_CAP_PCT);
    ArmorProtection {
        melee: melee.min(cap) as u8,
        projectile: projectile.min(cap) as u8,
        blast: blast.min(cap) as u8,
    }
}

/// Iterate the [`ArmorProfile`]s of the pieces that currently *contribute*
/// mitigation: worn, resolvable to an armor definition, and durability above
/// zero. Shared by [`equipped_protection`] and the durability-wear path so both
/// agree on which pieces count.
pub fn worn_armor_profiles(
    equipment_slots: &[Option<ItemStack>],
) -> impl Iterator<Item = ArmorProfile> + '_ {
    equipment_slots
        .iter()
        .filter_map(|slot| slot.as_ref())
        .filter(|stack| stack.durability.map(|d| d > 0).unwrap_or(true))
        .filter_map(|stack| armor_profile(&stack.item_id))
}

/// The [`ArmorProfile`] for an item id, or `None` if the id is unknown or the
/// item carries no armor profile.
pub fn armor_profile(item_id: &str) -> Option<ArmorProfile> {
    item_definition(item_id).and_then(|definition: &ItemDefinition| definition.armor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{
        IRON_BOOTS_ID, IRON_CUIRASS_ID, IRON_GREAVES_ID, IRON_HELM_ID, LAMELLAR_BOOTS_ID,
        LAMELLAR_GREAVES_ID, LAMELLAR_HELM_ID, LAMELLAR_VEST_ID, PADDED_HOOD_ID,
        PADDED_LEGGINGS_ID, PADDED_TUNIC_ID, PADDED_WRAPS_ID,
    };
    use crate::protocol::{EQUIPMENT_SLOT_COUNT, ItemStack};

    /// Wear a full set (one id per slot) and read back the per-kind mitigation.
    /// Shared by the three set-sum tests so each only lists its four ids and the
    /// expected totals.
    fn full_set_protection(head: &str, chest: &str, legs: &str, feet: &str) -> ArmorProtection {
        let mut slots = vec![None; EQUIPMENT_SLOT_COUNT];
        slots[EquipmentSlot::Head.index()] = Some(ItemStack::new(head, 1));
        slots[EquipmentSlot::Chest.index()] = Some(ItemStack::new(chest, 1));
        slots[EquipmentSlot::Legs.index()] = Some(ItemStack::new(legs, 1));
        slots[EquipmentSlot::Feet.index()] = Some(ItemStack::new(feet, 1));
        equipped_protection(&slots)
    }

    /// A full padded set worn in every slot. The columns must sum to exactly the
    /// spec's set totals (melee 12 / projectile 10 / blast 4), well under the
    /// 60% cap.
    #[test]
    fn full_padded_set_sums_to_the_spec_totals() {
        let protection = full_set_protection(
            PADDED_HOOD_ID,
            PADDED_TUNIC_ID,
            PADDED_LEGGINGS_ID,
            PADDED_WRAPS_ID,
        );
        assert_eq!(protection.melee, 12);
        assert_eq!(protection.projectile, 10);
        assert_eq!(protection.blast, 4);
    }

    /// A full lamellar set worn in every slot. Columns must sum to exactly the
    /// spec's set totals (melee 24 / projectile 20 / blast 10), under the cap.
    #[test]
    fn full_lamellar_set_sums_to_the_spec_totals() {
        let protection = full_set_protection(
            LAMELLAR_HELM_ID,
            LAMELLAR_VEST_ID,
            LAMELLAR_GREAVES_ID,
            LAMELLAR_BOOTS_ID,
        );
        assert_eq!(protection.melee, 24);
        assert_eq!(protection.projectile, 20);
        assert_eq!(protection.blast, 10);
    }

    /// A full iron set worn in every slot. Columns must sum to exactly the
    /// spec's set totals (melee 40 / projectile 36 / blast 20). The melee total
    /// (40) stays under the 60% cap, so the strongest set still leaves a real
    /// chunk of every hit landing.
    #[test]
    fn full_iron_set_sums_to_the_spec_totals() {
        let protection = full_set_protection(
            IRON_HELM_ID,
            IRON_CUIRASS_ID,
            IRON_GREAVES_ID,
            IRON_BOOTS_ID,
        );
        assert_eq!(protection.melee, 40);
        assert_eq!(protection.projectile, 36);
        assert_eq!(protection.blast, 20);
    }

    /// A broken piece (durability 0) stays worn but adds nothing. Wearing the
    /// tunic at 0 durability drops its whole contribution from every column.
    #[test]
    fn zero_durability_piece_contributes_nothing() {
        let mut slots = vec![None; EQUIPMENT_SLOT_COUNT];
        let mut tunic = ItemStack::new(PADDED_TUNIC_ID, 1);
        tunic.durability = Some(0);
        slots[EquipmentSlot::Chest.index()] = Some(tunic);

        let protection = equipped_protection(&slots);
        assert_eq!(protection, ArmorProtection::default());
    }

    /// The per-kind total is clamped to the cap. Two synthetic 40% pieces sum to
    /// 80 raw and must clamp to exactly 60 in every column.
    #[test]
    fn protection_clamps_to_the_cap() {
        let heavy = ArmorProfile {
            slot: EquipmentSlot::Chest,
            mesh: ArmorMesh::PaddedTunic,
            melee_protection_pct: 40,
            projectile_protection_pct: 40,
            blast_protection_pct: 40,
            max_durability: Some(100),
        };
        let protection = protection_from_profiles([heavy, heavy].into_iter());
        assert_eq!(protection.melee, ARMOR_TOTAL_CAP_PCT);
        assert_eq!(protection.projectile, ARMOR_TOTAL_CAP_PCT);
        assert_eq!(protection.blast, ARMOR_TOTAL_CAP_PCT);
    }

    /// An empty paperdoll is zero mitigation everywhere, identical to a bare
    /// player. This is the zero-behavior-change guarantee.
    #[test]
    fn empty_paperdoll_is_zero_protection() {
        let slots = vec![None; EQUIPMENT_SLOT_COUNT];
        assert_eq!(equipped_protection(&slots), ArmorProtection::default());
    }

    /// `for_kind` selects the right column for each damage kind, so the damage
    /// path can never read the wrong mitigation for a hit.
    #[test]
    fn protection_selects_the_column_by_damage_kind() {
        let protection = ArmorProtection {
            melee: 11,
            projectile: 22,
            blast: 33,
        };
        assert_eq!(protection.for_kind(DamageKind::Blunt), 11);
        assert_eq!(protection.for_kind(DamageKind::Projectile), 22);
        assert_eq!(protection.for_kind(DamageKind::Blast), 33);
    }

    /// The armor profile exposes the same per-kind mapping, so the wear path
    /// (which asks "did this piece protect against this kind?") reads the same
    /// field the mitigation sum does.
    #[test]
    fn profile_protection_for_maps_each_kind() {
        let profile = ArmorProfile {
            slot: EquipmentSlot::Feet,
            mesh: ArmorMesh::PaddedWraps,
            melee_protection_pct: 1,
            projectile_protection_pct: 2,
            blast_protection_pct: 0,
            max_durability: Some(100),
        };
        assert_eq!(profile.protection_for(DamageKind::Blunt), 1);
        assert_eq!(profile.protection_for(DamageKind::Projectile), 2);
        assert_eq!(profile.protection_for(DamageKind::Blast), 0);
    }
}
