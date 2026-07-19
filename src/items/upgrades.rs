//! Generic deployable upgrade table.
//!
//! Declares, as data, which placed structures can be upgraded in place to a
//! higher-tier variant and what that costs. The server's upgrade handler is
//! kind-agnostic: it looks up the row for an entity's current kind, so a
//! future furnace tier (or any other station) is one new row here, not a new
//! code path. The upgrade mutates the deployable's `DeployableKind` in place
//! (same entity id), which is why the "from" and "to" are full kinds and the
//! tier lives inside them.
//!
//! Costs mirror the crafting-recipe precedent ([`CraftingInput`] lists with
//! inline quantities) and never travel on the wire: the client reads this same
//! compile-time table to render costs and affordability, exactly as recipes
//! stay off the wire.

use crate::crafting::CraftingInput;

use super::deployables::DeployableKind;
use super::ids::{IRON_BAR_ID, METEORITE_INGOT_ID, SALVAGED_FITTINGS_ID};

/// One upgrade path: mutate a placed `from` structure into `to` in place,
/// consuming `cost`. `from` and `to` share the entity id; only the kind (and
/// the tier it carries) changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeployableUpgrade {
    pub from: DeployableKind,
    pub to: DeployableKind,
    pub cost: &'static [CraftingInput],
}

/// Source-of-truth upgrade table. One row today: the workbench tier-1 to
/// tier-2 upgrade. Add a row (and its cost) to make any other station
/// upgradable; the server and client both drive off this list.
pub const DEPLOYABLE_UPGRADES: &[DeployableUpgrade] = &[DeployableUpgrade {
    from: DeployableKind::Workbench { tier: 1 },
    to: DeployableKind::Workbench { tier: 2 },
    // The gate in front of every tier-2 craft (satchels, iron armor,
    // crossbow), so it is priced as a mid-game project, not a checkpoint:
    // 6 ingots is most of one meteorite find (8 alloy) or a slice of a
    // crater cluster, and 8 fittings is two to three salvage-chest cycles.
    cost: &[
        CraftingInput::new(IRON_BAR_ID, 50),
        CraftingInput::new(SALVAGED_FITTINGS_ID, 8),
        CraftingInput::new(METEORITE_INGOT_ID, 6),
    ],
}];

/// The upgrade available from a placed structure's current kind, if any.
/// `None` means the structure is already at its top tier (or was never
/// upgradable). Kind equality is exact, so a tier-2 workbench returns `None`
/// while a tier-1 one returns the tier-2 row.
pub fn upgrade_for(kind: DeployableKind) -> Option<&'static DeployableUpgrade> {
    DEPLOYABLE_UPGRADES
        .iter()
        .find(|upgrade| upgrade.from == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbench_tier_one_upgrades_to_tier_two() {
        let upgrade =
            upgrade_for(DeployableKind::Workbench { tier: 1 }).expect("t1 workbench upgrades");
        assert_eq!(upgrade.to, DeployableKind::Workbench { tier: 2 });
        // The cost list carries inline quantities, mirroring the recipe registry.
        assert_eq!(upgrade.cost.len(), 3);
        assert!(
            upgrade
                .cost
                .iter()
                .any(|input| input.item_id == METEORITE_INGOT_ID && input.quantity == 6)
        );
    }

    #[test]
    fn top_tier_workbench_has_no_upgrade() {
        // A tier-2 workbench is the ceiling for now, so no row matches it.
        assert!(upgrade_for(DeployableKind::Workbench { tier: 2 }).is_none());
    }

    #[test]
    fn unlisted_kinds_have_no_upgrade() {
        // Only rows in the table are upgradable; everything else returns None
        // so the handler never mutates a structure without a declared path.
        assert!(upgrade_for(DeployableKind::Furnace { tier: 1 }).is_none());
        assert!(upgrade_for(DeployableKind::ToolCupboard).is_none());
    }
}
