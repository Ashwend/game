//! Client-side crafting-station proximity.
//!
//! Mirrors the server's `GameServer::station_in_range`
//! (`src/server/deployables.rs`): a `RecipeStation::Workbench { min_tier }`
//! is satisfied when the player has a placed deployable in range whose kind
//! passes [`RecipeStation::satisfied_by`] (a tier-N bench, N >= min_tier),
//! within that deployable's `station_radius`. The two must agree, so both
//! read `satisfied_by` and the deployable profile's `station_radius`; if
//! they drift, the UI would green-light a craft the server then rejects (or
//! grey out one it would accept). The client learns nearby stations from the
//! replicated `(Deployable, DeployableTransform)` set already streamed into
//! its AoI; this module keeps the geometry as a pure, testable helper.

use crate::{
    crafting::RecipeStation,
    items::{DeployableKind, item_definition},
    protocol::Vec3Net,
};

/// A placed deployable as the client sees it, projected down to only the
/// fields station-proximity needs: its kind (drives `satisfied_by` and the
/// registry lookup for `station_radius`) and its world position. Built from
/// the replicated `(Deployable, DeployableTransform)` components.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NearbyStation {
    kind: DeployableKind,
    item_id: &'static str,
    position: Vec3Net,
}

impl NearbyStation {
    /// Build a station snapshot from a replicated deployable's kind, its
    /// registry item id (for the `station_radius` lookup), and its world
    /// position. `item_id` must be a `&'static str` from the item registry
    /// so the radius lookup resolves.
    pub(crate) fn new(kind: DeployableKind, item_id: &'static str, position: Vec3Net) -> Self {
        Self {
            kind,
            item_id,
            position,
        }
    }
}

/// Snapshot of the crafting stations the local player can currently reach,
/// resolved once per panel render from the replicated deployable set. Every
/// recipe row consults it to decide whether a `RecipeStation::Workbench`
/// requirement is met right now.
///
/// `player_pos` is `None` before the local prediction seeds (pre-Welcome);
/// with no known position we cannot verify a station, so any workbench
/// requirement reads as unmet, which is the safe default (the server would
/// reject the craft too).
#[derive(Debug, Default, Clone)]
pub(crate) struct StationContext {
    player_pos: Option<Vec3Net>,
    stations: Vec<NearbyStation>,
}

impl StationContext {
    pub(crate) fn new(player_pos: Option<Vec3Net>, stations: Vec<NearbyStation>) -> Self {
        Self {
            player_pos,
            stations,
        }
    }

    /// True when `station` is satisfied for the local player right now.
    /// `RecipeStation::None` (hand-craftable) is always satisfied.
    pub(super) fn met(&self, station: RecipeStation) -> bool {
        if matches!(station, RecipeStation::None) {
            return true;
        }
        let Some(player_pos) = self.player_pos else {
            return false;
        };
        station_satisfied(station, player_pos, self.stations.iter().copied())
    }
}

/// True when `station` is satisfied for a player standing at `player_pos`,
/// given the placed deployables they can see. `RecipeStation::None`
/// (hand-craftable) is always satisfied. Otherwise scans for any placed
/// deployable whose kind satisfies the requirement and sits within its own
/// `station_radius` of the player, exactly the server's `station_in_range`
/// loop, so client gating never disagrees with server authority.
pub(super) fn station_satisfied<I>(station: RecipeStation, player_pos: Vec3Net, stations: I) -> bool
where
    I: IntoIterator<Item = NearbyStation>,
{
    if matches!(station, RecipeStation::None) {
        return true;
    }
    stations.into_iter().any(|entry| {
        if !station.satisfied_by(entry.kind) {
            return false;
        }
        // Radius comes from the deployable's own profile, same source the
        // server reads, so a tuning change to a bench's reach moves both.
        let Some(profile) = item_definition(entry.item_id).and_then(|def| def.deployable) else {
            return false;
        };
        player_pos.within_horizontal_range(entry.position, profile.station_radius)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::WORKBENCH_T1_ID;

    fn bench(tier: u8, x: f32, z: f32) -> NearbyStation {
        NearbyStation {
            kind: DeployableKind::Workbench { tier },
            item_id: WORKBENCH_T1_ID,
            position: Vec3Net::new(x, 0.0, z),
        }
    }

    #[test]
    fn hand_recipes_are_always_satisfied() {
        // No stations at all, hand-craftable is still fine.
        assert!(station_satisfied(
            RecipeStation::None,
            Vec3Net::ZERO,
            std::iter::empty(),
        ));
    }

    #[test]
    fn workbench_in_radius_satisfies_but_out_of_radius_does_not() {
        // The tier-1 bench's station_radius is 5m (registry). A bench 2m
        // away satisfies; the same bench 10m away does not.
        let close = bench(1, 2.0, 0.0);
        let far = bench(1, 10.0, 0.0);
        let req = RecipeStation::Workbench { min_tier: 1 };
        assert!(station_satisfied(req, Vec3Net::ZERO, [close]));
        assert!(!station_satisfied(req, Vec3Net::ZERO, [far]));
    }

    #[test]
    fn vertical_offset_never_changes_range() {
        // Range is horizontal only (mirrors within_horizontal_range): a
        // bench directly below the player at the same XZ still counts.
        let req = RecipeStation::Workbench { min_tier: 1 };
        let under = NearbyStation {
            kind: DeployableKind::Workbench { tier: 1 },
            item_id: WORKBENCH_T1_ID,
            position: Vec3Net::new(0.0, -20.0, 0.0),
        };
        assert!(station_satisfied(req, Vec3Net::ZERO, [under]));
    }

    #[test]
    fn tier_one_bench_does_not_satisfy_a_min_tier_two_recipe() {
        // A nearby tier-1 bench is not enough for a min_tier-2 recipe, even
        // in range: satisfied_by gates on tier >= min_tier.
        let close = bench(1, 1.0, 0.0);
        let req = RecipeStation::Workbench { min_tier: 2 };
        assert!(!station_satisfied(req, Vec3Net::ZERO, [close]));
    }

    #[test]
    fn tier_two_bench_satisfies_a_min_tier_one_recipe() {
        // Higher tier satisfies a lower requirement, same as tool tiers.
        let close = bench(2, 1.0, 0.0);
        let req = RecipeStation::Workbench { min_tier: 1 };
        assert!(station_satisfied(req, Vec3Net::ZERO, [close]));
    }

    #[test]
    fn context_met_treats_hand_recipes_as_always_satisfied() {
        // Even with no known position and no stations, a hand recipe is met.
        let ctx = StationContext::new(None, Vec::new());
        assert!(ctx.met(RecipeStation::None));
    }

    #[test]
    fn context_met_is_false_for_workbench_recipe_without_a_known_position() {
        // Pre-Welcome the player position is unknown; a workbench recipe
        // reads as unmet so the UI never green-lights a craft the server
        // would reject.
        let ctx = StationContext::new(None, vec![bench(1, 0.0, 0.0)]);
        assert!(!ctx.met(RecipeStation::Workbench { min_tier: 1 }));
    }

    #[test]
    fn context_met_resolves_a_workbench_recipe_against_nearby_benches() {
        let ctx = StationContext::new(Some(Vec3Net::ZERO), vec![bench(1, 2.0, 0.0)]);
        assert!(ctx.met(RecipeStation::Workbench { min_tier: 1 }));
        assert!(!ctx.met(RecipeStation::Workbench { min_tier: 2 }));
    }

    #[test]
    fn non_workbench_deployables_never_satisfy_a_workbench_requirement() {
        // A furnace in range must not satisfy a workbench recipe.
        let furnace = NearbyStation {
            kind: DeployableKind::Furnace { tier: 1 },
            item_id: crate::items::CRUDE_FURNACE_ID,
            position: Vec3Net::new(1.0, 0.0, 0.0),
        };
        let req = RecipeStation::Workbench { min_tier: 1 };
        assert!(!station_satisfied(req, Vec3Net::ZERO, [furnace]));
    }
}
