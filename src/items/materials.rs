//! Destructible-material taxonomy and the central tool-vs-material
//! effectiveness table every damage path reads through.

use super::explosives::ExplosiveKind;
use super::tools::ToolKind;

/// What a destructible thing is made of, for the tool-vs-material matchup
/// system. The taxonomy is deliberately coarse: wood vs stone is enough to
/// express "hatchet eats workbenches, pickaxe eats furnaces" today. New
/// materials (metal, concrete, …) slot in here as the world gains them, and
/// the single [`tool_effectiveness_pct`] table below is where their matchups
/// are declared, no per-entity special-casing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructibleMaterial {
    Wood,
    Stone,
    /// Sticks-tier building blocks. Deliberately fragile: any proper tool
    /// tears through in a few swings.
    Sticks,
    /// Wood-tier building blocks and doors. Raidable with tools but
    /// slowly, the soft side of a base.
    WoodBuilding,
    /// Stone-tier building blocks. Immune to every tool; raiding stone
    /// waits for future siege equipment.
    StoneBuilding,
    /// Forged metal (iron doors, and future metal building). Immune to
    /// every tool like stone, but kept a distinct material so explosives
    /// can later balance metal independently of stone.
    MetalBuilding,
    /// Sleeping bags. Tears in a couple of hits.
    Cloth,
}

/// Central tool-vs-material effectiveness table, expressed as a percentage
/// multiplier so the server stays on integer math. This is the one place
/// that answers "how well does tool X bite material Y": matched tool ≈ 1.5×,
/// mismatched proper tool ≈ 0.5×. Every destructible-entity damage path
/// (deployables today, more later) reads through here rather than branching
/// on entity type, so balancing a matchup is a single-line edit and adding a
/// material is a single new arm. Bare hands never reach this code path
/// (they're rejected upstream); the catch-all keeps the math total.
pub fn tool_effectiveness_pct(tool: ToolKind, material: DestructibleMaterial) -> u32 {
    match (tool, material) {
        (ToolKind::Axe, DestructibleMaterial::Wood) => 150,
        (ToolKind::Pickaxe, DestructibleMaterial::Stone) => 150,
        (ToolKind::Axe, DestructibleMaterial::Stone) => 50,
        (ToolKind::Pickaxe, DestructibleMaterial::Wood) => 50,
        // Building materials, the raid-balance table. Sticks shred under
        // any proper tool; wood-tier buildings take a trickle (slow but
        // real raids); stone-tier buildings are immune to tools entirely.
        (ToolKind::Axe, DestructibleMaterial::Sticks) => 300,
        (ToolKind::Pickaxe, DestructibleMaterial::Sticks) => 200,
        (ToolKind::Axe, DestructibleMaterial::WoodBuilding) => 15,
        (ToolKind::Pickaxe, DestructibleMaterial::WoodBuilding) => 5,
        (_, DestructibleMaterial::StoneBuilding) => 0,
        // Iron doors / metal: tool-proof by construction, like stone. Only
        // explosives (a separate damage path) will breach metal.
        (_, DestructibleMaterial::MetalBuilding) => 0,
        // Sleeping bags tear under anything with an edge.
        (ToolKind::Axe | ToolKind::Pickaxe, DestructibleMaterial::Cloth) => 300,
        // The hammer builds, it never breaks. Repair/upgrade/demolish all
        // ride their own commands, so zero here closes the "hammer as a
        // free raid tool" hole outright.
        (ToolKind::Hammer, _) => 0,
        // Hands shouldn't reach here, but if they do treat them as
        // worst-case mismatched so they make minimal dents.
        (ToolKind::Hands, _) => 50,
    }
}

/// Central explosive-vs-material effectiveness matrix, the raid-balance lever
/// for blackpowder charges, expressed as a percentage multiplier on the
/// explosive's `base_damage` so the server stays on integer math. This is the
/// explosive analogue of [`tool_effectiveness_pct`]: the one place that answers
/// "how well does charge X breach material Y", read by [`resolve_explosion`]
/// for every structure in the blast radius rather than branching on entity
/// type. The spec matrix is exact:
///
/// | Charge | Sticks | Wood | Stone | Metal |
/// | --- | --- | --- | --- | --- |
/// | PowderBomb | 100 | 40 | 8 | 0 |
/// | PowderKeg | 100 | 80 | 25 | 0 |
/// | SatchelCharge | 100 | 85 | 45 | 8 |
///
/// The four building/door raid materials map to the four columns directly.
/// Utility-deployable materials (a furnace, workbench, box, or torch caught in
/// the blast) fold onto the wood/stone columns: `Wood`/`Cloth` read as the wood
/// column, `Stone` as the stone column, so a keg near a furnace still damages it
/// sensibly without a separate matrix. Tuning a matchup is a single-line edit,
/// per CLAUDE.md (the actual numbers live in [`crate::game_balance`]).
///
/// [`resolve_explosion`]: crate::server::GameServer::resolve_explosion
pub fn explosive_effectiveness_pct(kind: ExplosiveKind, material: DestructibleMaterial) -> u32 {
    use crate::game_balance::{
        POWDER_BOMB_EFFECTIVENESS_METAL_PCT, POWDER_BOMB_EFFECTIVENESS_STICKS_PCT,
        POWDER_BOMB_EFFECTIVENESS_STONE_PCT, POWDER_BOMB_EFFECTIVENESS_WOOD_PCT,
        POWDER_KEG_EFFECTIVENESS_METAL_PCT, POWDER_KEG_EFFECTIVENESS_STICKS_PCT,
        POWDER_KEG_EFFECTIVENESS_STONE_PCT, POWDER_KEG_EFFECTIVENESS_WOOD_PCT,
        SATCHEL_CHARGE_EFFECTIVENESS_METAL_PCT, SATCHEL_CHARGE_EFFECTIVENESS_STICKS_PCT,
        SATCHEL_CHARGE_EFFECTIVENESS_STONE_PCT, SATCHEL_CHARGE_EFFECTIVENESS_WOOD_PCT,
    };

    // Collapse every destructible material onto one of the four spec columns
    // (sticks / wood / stone / metal). Building blocks and doors already carry a
    // distinct material per tier; free-standing utility deployables fold onto
    // wood or stone so a charge that lands next to one still bites.
    #[derive(Clone, Copy)]
    enum Column {
        Sticks,
        Wood,
        Stone,
        Metal,
    }
    let column = match material {
        DestructibleMaterial::Sticks => Column::Sticks,
        // Hewn-wood buildings, wood doors, the tool cupboard, and every plain
        // wood/cloth utility deployable share the wood column.
        DestructibleMaterial::WoodBuilding
        | DestructibleMaterial::Wood
        | DestructibleMaterial::Cloth => Column::Wood,
        DestructibleMaterial::StoneBuilding | DestructibleMaterial::Stone => Column::Stone,
        DestructibleMaterial::MetalBuilding => Column::Metal,
    };

    match (kind, column) {
        (ExplosiveKind::PowderBomb, Column::Sticks) => POWDER_BOMB_EFFECTIVENESS_STICKS_PCT,
        (ExplosiveKind::PowderBomb, Column::Wood) => POWDER_BOMB_EFFECTIVENESS_WOOD_PCT,
        (ExplosiveKind::PowderBomb, Column::Stone) => POWDER_BOMB_EFFECTIVENESS_STONE_PCT,
        (ExplosiveKind::PowderBomb, Column::Metal) => POWDER_BOMB_EFFECTIVENESS_METAL_PCT,
        (ExplosiveKind::PowderKeg, Column::Sticks) => POWDER_KEG_EFFECTIVENESS_STICKS_PCT,
        (ExplosiveKind::PowderKeg, Column::Wood) => POWDER_KEG_EFFECTIVENESS_WOOD_PCT,
        (ExplosiveKind::PowderKeg, Column::Stone) => POWDER_KEG_EFFECTIVENESS_STONE_PCT,
        (ExplosiveKind::PowderKeg, Column::Metal) => POWDER_KEG_EFFECTIVENESS_METAL_PCT,
        (ExplosiveKind::SatchelCharge, Column::Sticks) => SATCHEL_CHARGE_EFFECTIVENESS_STICKS_PCT,
        (ExplosiveKind::SatchelCharge, Column::Wood) => SATCHEL_CHARGE_EFFECTIVENESS_WOOD_PCT,
        (ExplosiveKind::SatchelCharge, Column::Stone) => SATCHEL_CHARGE_EFFECTIVENESS_STONE_PCT,
        (ExplosiveKind::SatchelCharge, Column::Metal) => SATCHEL_CHARGE_EFFECTIVENESS_METAL_PCT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_material_multiplier_favours_matched_pairings() {
        // Matched: hatchet→wood and pickaxe→stone hit hardest.
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Axe, DestructibleMaterial::Wood),
            150
        );
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Pickaxe, DestructibleMaterial::Stone),
            150
        );
        // Mismatched proper tools still chip but at a third of the
        // matched rate (50 / 150 = 1/3).
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Axe, DestructibleMaterial::Stone),
            50
        );
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Pickaxe, DestructibleMaterial::Wood),
            50
        );
    }

    #[test]
    fn iron_door_is_immune_to_every_tool() {
        // The whole point of the iron door: no tool can scratch metal, so a
        // stone base with iron doors is tool-proof and only explosives (a
        // future, separate damage path) breach it.
        for tool in [
            ToolKind::Axe,
            ToolKind::Pickaxe,
            ToolKind::Hammer,
            ToolKind::Hands,
        ] {
            assert_eq!(
                tool_effectiveness_pct(tool, DestructibleMaterial::MetalBuilding),
                0,
                "{tool:?} should do nothing to metal"
            );
        }
        // Regression: the wood door still takes a (slow) trickle from an axe.
        assert!(tool_effectiveness_pct(ToolKind::Axe, DestructibleMaterial::WoodBuilding) > 0);
    }

    /// The explosive matrix matches the spec percentages exactly, across the
    /// four raid materials that map one-to-one to the spec's four columns.
    #[test]
    fn explosive_matrix_matches_the_spec_percentages() {
        use DestructibleMaterial::{MetalBuilding, Sticks, StoneBuilding, WoodBuilding};
        use ExplosiveKind::{PowderBomb, PowderKeg, SatchelCharge};

        // (kind, sticks, wood, stone, metal) rows straight from the spec matrix.
        let rows = [
            (PowderBomb, 100, 40, 8, 0),
            (PowderKeg, 100, 80, 25, 0),
            (SatchelCharge, 100, 85, 45, 8),
        ];
        for (kind, sticks, wood, stone, metal) in rows {
            assert_eq!(
                explosive_effectiveness_pct(kind, Sticks),
                sticks,
                "{kind:?} sticks"
            );
            assert_eq!(
                explosive_effectiveness_pct(kind, WoodBuilding),
                wood,
                "{kind:?} wood"
            );
            assert_eq!(
                explosive_effectiveness_pct(kind, StoneBuilding),
                stone,
                "{kind:?} stone"
            );
            assert_eq!(
                explosive_effectiveness_pct(kind, MetalBuilding),
                metal,
                "{kind:?} metal"
            );
        }
    }

    /// A bomb does 0 to metal (an iron door is bomb-proof) and satchel does
    /// exactly its spec 8% against metal, the two edges the raid math leans on.
    #[test]
    fn explosive_metal_edges_hold() {
        assert_eq!(
            explosive_effectiveness_pct(
                ExplosiveKind::PowderBomb,
                DestructibleMaterial::MetalBuilding
            ),
            0,
            "a powder bomb cannot scratch an iron door"
        );
        assert_eq!(
            explosive_effectiveness_pct(
                ExplosiveKind::SatchelCharge,
                DestructibleMaterial::MetalBuilding
            ),
            8,
            "a satchel does exactly 8% of base vs metal"
        );
    }

    /// Free-standing utility deployables (furnace = Stone, workbench/box =
    /// Wood, sleeping bag = Cloth) fold onto the spec columns so a charge that
    /// lands beside one still bites through the same matrix.
    #[test]
    fn utility_deployable_materials_fold_onto_wood_and_stone() {
        // A keg reads Wood/Cloth as the wood column (80%) and Stone as the
        // stone column (25%).
        assert_eq!(
            explosive_effectiveness_pct(ExplosiveKind::PowderKeg, DestructibleMaterial::Wood),
            80
        );
        assert_eq!(
            explosive_effectiveness_pct(ExplosiveKind::PowderKeg, DestructibleMaterial::Cloth),
            80
        );
        assert_eq!(
            explosive_effectiveness_pct(ExplosiveKind::PowderKeg, DestructibleMaterial::Stone),
            25
        );
    }
}
