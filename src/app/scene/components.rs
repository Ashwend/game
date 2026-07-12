use bevy::prelude::*;

use crate::{
    protocol::{ClientId, DeployedEntityId, DroppedItemId, LootBagId, ResourceNodeId},
    resources::ResourceNodeModel,
};

#[derive(Component)]
pub(crate) struct NetworkPlayer {
    // The id is also kept in `RemotePlayerEntities`; this copy is here so the
    // component carries enough context to be inspected in isolation (debug
    // overlays, future per-player queries).
    pub(crate) client_id: ClientId,
}

#[derive(Component)]
pub(crate) struct NetworkDroppedItem {
    pub(crate) id: DroppedItemId,
}

#[derive(Component)]
pub(crate) struct NetworkResourceNode {
    pub(crate) id: ResourceNodeId,
    pub(crate) model: ResourceNodeModel,
    /// A tree standing where growth is poor renders as a bare dead snag (no
    /// canopy). Tracked here so the felling effect drops a bare trunk for a dead
    /// snag instead of sprouting the live canopy the model alone would imply.
    pub(crate) dead: bool,
}

/// Marker for a placed structure entity (workbench, furnace, …). The
/// nameplate UI uses `id` to match the replicated state. What mesh the
/// visual carries (and when an upgrade has to swap it) is tracked in
/// the reconciler's `DeployedEntityVisuals` resource, not here.
#[derive(Component)]
pub(crate) struct NetworkDeployedEntity {
    pub(crate) id: DeployedEntityId,
}

/// Marker for the client-only ghost preview rendered while the player
/// has a deployable selected. Single instance, the placement system
/// despawns and respawns when the kind changes.
#[derive(Component)]
pub(crate) struct DeployablePlacementGhost;

/// Visual marker for a death loot bag in the world. One per
/// replicated `LootBag` entity; lets the pickup-target ray test
/// distinguish bags from regular dropped items.
#[derive(Component)]
pub(crate) struct NetworkLootBag {
    #[expect(
        dead_code,
        reason = "carried so the bag marker can be inspected in isolation"
    )]
    pub(crate) id: LootBagId,
}

#[derive(Component)]
pub(crate) struct HeldItemVisual {
    pub(crate) item_id: crate::items::ItemId,
    /// Which animatable rig piece this layer is, so the per-frame update can give
    /// it its own local transform (bow limbs / string, crossbow string) on top of
    /// the shared whole-item swing transform. [`HeldPieceSlot::Static`] for every
    /// melee / tool layer (identity local transform), so those are unchanged.
    pub(crate) slot: crate::items::HeldPieceSlot,
}

#[derive(Component)]
pub(crate) struct MainCamera;

/// Dedicated camera for the first-person held item. It renders only the
/// viewmodel layer ([`VIEWMODEL_RENDER_LAYER`]) in its own pass with a fresh,
/// cleared depth buffer, so the in-hand tool never depth-tests against the world
/// and stops clipping into nearby walls / ore / peers. Spawned as a child of the
/// [`MainCamera`] so it shares the eye transform automatically.
#[derive(Component)]
pub(crate) struct ViewmodelCamera;

/// Render layer the first-person held item lives on. Only [`ViewmodelCamera`]
/// draws it; the world [`MainCamera`] (layer 0) never does, and the custom grass
/// queue skips layer-mismatched views so the field stays out of the viewmodel
/// pass. The third-person tool on remote players stays on the world layer.
pub(crate) const VIEWMODEL_RENDER_LAYER: usize = 1;

#[derive(Component)]
pub(crate) struct WorldGeometry;

/// World-space upright height of a tree mesh. Used by the felling animation
/// as the lever length for its pendulum integration. Heights are baked into
/// the mesh itself, so this returns the canonical top-Y value.
pub(crate) fn tree_mesh_height(model: ResourceNodeModel) -> Option<f32> {
    match model {
        ResourceNodeModel::PineTreeSmall => Some(4.50),
        ResourceNodeModel::PineTreeMedium => Some(6.60),
        ResourceNodeModel::PineTreeLarge => Some(9.10),
        ResourceNodeModel::BirchTreeSmall => Some(3.60),
        ResourceNodeModel::BirchTreeMedium => Some(5.30),
        ResourceNodeModel::BirchTreeLarge => Some(7.15),
        // Non-tree models don't fall, so they don't carry a mesh height.
        ResourceNodeModel::CoalOre
        | ResourceNodeModel::IronOre
        | ResourceNodeModel::SulfurOre
        | ResourceNodeModel::StoneVein
        | ResourceNodeModel::Meteorite
        | ResourceNodeModel::SurfaceStone
        | ResourceNodeModel::BranchPile
        | ResourceNodeModel::HayGrass => None,
    }
}
