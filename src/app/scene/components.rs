use bevy::prelude::*;

use crate::{
    items::DeployableKind,
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
}

/// Marker for a placed structure entity (workbench, furnace, …).
/// The kind drives mesh/material lookups and the nameplate UI uses
/// `id` to match snapshot health updates.
#[derive(Component)]
pub(crate) struct NetworkDeployedEntity {
    pub(crate) id: DeployedEntityId,
    // The nameplate overlay reads `kind` to label the structure; the
    // mesh selection has already happened by spawn time. Kept on the
    // component so the overlay doesn't have to walk the snapshot to
    // recover it.
    #[expect(
        dead_code,
        reason = "nameplate overlay reads this once that path is wired"
    )]
    pub(crate) kind: DeployableKind,
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
}

#[derive(Component)]
pub(crate) struct MainCamera;

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
        | ResourceNodeModel::SurfaceStone
        | ResourceNodeModel::BranchPile
        | ResourceNodeModel::HayGrass => None,
    }
}
