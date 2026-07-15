//! Cost, HP, and repair lookup tables: which material a tier is built
//! from and what placement, upgrades, repairs, and max health cost per
//! piece and tier. This file is the lookup/dispatch layer only; the
//! balance values themselves live in `crate::game_balance` (per the
//! CLAUDE.md rule that every tuning constant has one home).

use super::{BuildingPiece, BuildingTier};

/// `(item id, quantity)` cost pair used by placement/upgrade/repair tables.
pub type MaterialCost = (&'static str, u16);

/// The material a tier is built from, for cost lookups. The sticks-look
/// first draft is built from raw wood; the upgrade ladder then moves to
/// workbench-refined hewn logs, then stone.
pub const fn tier_material(tier: BuildingTier) -> &'static str {
    match tier {
        BuildingTier::Sticks => crate::items::WOOD_ID,
        BuildingTier::HewnWood => crate::items::HEWN_LOG_ID,
        BuildingTier::Stone => crate::items::STONE_ID,
    }
}

/// True for the pieces priced like a foundation: full-cell volumes that
/// eat more material than a single wall span (the foundation slab, the
/// solid-stepped stairs).
const fn costs_like_foundation(piece: BuildingPiece) -> bool {
    matches!(piece, BuildingPiece::Foundation | BuildingPiece::Stairs)
}

/// Cost to place a fresh piece (always at the Sticks tier, paid in raw
/// wood).
pub const fn placement_cost(piece: BuildingPiece) -> MaterialCost {
    if costs_like_foundation(piece) {
        (
            crate::items::WOOD_ID,
            crate::game_balance::BUILDING_STICKS_COST_FOUNDATION,
        )
    } else {
        (
            crate::items::WOOD_ID,
            crate::game_balance::BUILDING_STICKS_COST_WALL,
        )
    }
}

/// Cost to upgrade a piece *to* `target` tier.
pub const fn upgrade_cost(piece: BuildingPiece, target: BuildingTier) -> MaterialCost {
    let foundation = costs_like_foundation(piece);
    match target {
        // Placement covers the sticks tier; upgrading "to sticks" never
        // happens but keep the table total.
        BuildingTier::Sticks => placement_cost(piece),
        BuildingTier::HewnWood => (
            crate::items::HEWN_LOG_ID,
            if foundation {
                crate::game_balance::BUILDING_HEWN_WOOD_COST_FOUNDATION
            } else {
                crate::game_balance::BUILDING_HEWN_WOOD_COST_WALL
            },
        ),
        BuildingTier::Stone => (
            crate::items::STONE_ID,
            if foundation {
                crate::game_balance::BUILDING_STONE_COST_FOUNDATION
            } else {
                crate::game_balance::BUILDING_STONE_COST_WALL
            },
        ),
    }
}

/// Cost of one hammer repair hit on a piece of `tier`.
pub const fn repair_cost(tier: BuildingTier) -> MaterialCost {
    let quantity = match tier {
        BuildingTier::Sticks => crate::game_balance::BUILDING_REPAIR_COST_STICKS,
        BuildingTier::HewnWood => crate::game_balance::BUILDING_REPAIR_COST_HEWN_WOOD,
        BuildingTier::Stone => crate::game_balance::BUILDING_REPAIR_COST_STONE,
    };
    (tier_material(tier), quantity)
}

/// Max health of a piece at a tier. Foundations carry 1.5x the wall budget,
/// they hold the whole base up.
pub const fn building_max_health(piece: BuildingPiece, tier: BuildingTier) -> u32 {
    let wall = match tier {
        BuildingTier::Sticks => crate::game_balance::BUILDING_STICKS_WALL_HP,
        BuildingTier::HewnWood => crate::game_balance::BUILDING_HEWN_WOOD_WALL_HP,
        BuildingTier::Stone => crate::game_balance::BUILDING_STONE_WALL_HP,
    };
    if matches!(piece, BuildingPiece::Foundation) {
        wall + wall / 2
    } else {
        wall
    }
}
