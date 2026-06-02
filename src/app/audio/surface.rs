//! World-surface taxonomy used for both footstep clip selection and tool
//! impact selection. One enum, two consumers, the same "Stone" you walk
//! on is the same "Stone" your pickaxe hits.
//!
//! Add a variant here when introducing a surface that needs its own
//! per-step pool or its own tool-impact pool. The mapping helpers below
//! convert from world-side concepts ([`crate::world::BlockKind`],
//! [`crate::resources::ResourceNodeModel`]) so that game code never
//! has to spell out "iron ore plays stone-impact" by hand.

use crate::{resources::ResourceNodeModel, world::BlockKind};

// Some variants are unconstructed today, they exist so the audio
// manifest's footstep + impact tables can declare pools for surfaces
// that the world doesn't yet expose (Sand, Stone, …). Annotated rather
// than removed because removing a variant rolls back the manifest's
// readiness for those surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub(crate) enum SurfaceMaterial {
    Dirt,
    Wood,
    Concrete,
    Sand,
    Stone,
    Iron,
    Coal,
    Sulfur,
}

impl SurfaceMaterial {
    /// Material to fall back to when a lookup yields nothing (footsteps
    /// over the world floor, an unknown resource node). Dirt is the most
    /// generic of the recorded surfaces.
    pub(crate) const DEFAULT: Self = Self::Dirt;
}

/// Map a structural block's kind to the surface a player walks on /
/// the tool would strike. Standard and stone blocks both read as masonry,
/// so they share the concrete surface today, when a dedicated stone-floor
/// footstep pool exists, route `BlockKind::Stone` to `Stone` here.
pub(crate) fn surface_for_block_kind(kind: BlockKind) -> SurfaceMaterial {
    match kind {
        BlockKind::Standard | BlockKind::Stone => SurfaceMaterial::Concrete,
    }
}

/// Map a resource node's visual model to the surface a tool hits when
/// striking it. Tree variants all read as wood; ore variants get their
/// own surface so each ore type can have its own distinct impact pool
/// without changing call sites.
pub(crate) fn surface_for_resource_model(model: ResourceNodeModel) -> SurfaceMaterial {
    match model {
        ResourceNodeModel::PineTreeSmall
        | ResourceNodeModel::PineTreeMedium
        | ResourceNodeModel::PineTreeLarge
        | ResourceNodeModel::BirchTreeSmall
        | ResourceNodeModel::BirchTreeMedium
        | ResourceNodeModel::BirchTreeLarge => SurfaceMaterial::Wood,
        ResourceNodeModel::CoalOre => SurfaceMaterial::Coal,
        ResourceNodeModel::IronOre => SurfaceMaterial::Iron,
        ResourceNodeModel::SulfurOre => SurfaceMaterial::Sulfur,
        // Plain rock vein, same impact pool as the small surface
        // stones, just at the heavier per-swing strike.
        ResourceNodeModel::StoneVein => SurfaceMaterial::Stone,
        // Crude materials: branches read as wood, surface stone as stone,
        // hay tufts as dirt (closest "soft scrape" in the surface taxonomy).
        ResourceNodeModel::BranchPile => SurfaceMaterial::Wood,
        ResourceNodeModel::SurfaceStone => SurfaceMaterial::Stone,
        ResourceNodeModel::HayGrass => SurfaceMaterial::Dirt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tree_model_maps_to_wood() {
        for model in [
            ResourceNodeModel::PineTreeSmall,
            ResourceNodeModel::PineTreeMedium,
            ResourceNodeModel::PineTreeLarge,
            ResourceNodeModel::BirchTreeSmall,
            ResourceNodeModel::BirchTreeMedium,
            ResourceNodeModel::BirchTreeLarge,
        ] {
            assert_eq!(surface_for_resource_model(model), SurfaceMaterial::Wood);
        }
    }

    #[test]
    fn each_ore_gets_its_own_surface() {
        assert_eq!(
            surface_for_resource_model(ResourceNodeModel::CoalOre),
            SurfaceMaterial::Coal
        );
        assert_eq!(
            surface_for_resource_model(ResourceNodeModel::IronOre),
            SurfaceMaterial::Iron
        );
        assert_eq!(
            surface_for_resource_model(ResourceNodeModel::SulfurOre),
            SurfaceMaterial::Sulfur
        );
    }
}
