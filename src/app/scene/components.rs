use bevy::prelude::*;

use crate::{
    protocol::{ClientId, DroppedItemId, ResourceNodeId},
    resources::ResourceNodeModel,
};

#[derive(Component)]
pub(crate) struct NetworkPlayer {
    // The id is also kept in `RemotePlayerEntities`; this copy is here so the
    // component carries enough context to be inspected in isolation (debug
    // overlays, future per-player queries).
    #[allow(dead_code)]
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

#[derive(Component)]
pub(crate) struct HeldItemVisual {
    pub(crate) item_id: crate::items::ItemId,
}

#[derive(Component)]
pub(crate) struct MainCamera;

#[derive(Component)]
pub(crate) struct WorldGeometry;

/// World-space upright height of a tree mesh at unit scale. Used by the
/// felling animation as the lever length for its pendulum integration.
pub(crate) fn tree_mesh_height(model: ResourceNodeModel) -> Option<f32> {
    match model {
        ResourceNodeModel::PineTree => Some(2.18),
        ResourceNodeModel::BirchTree => Some(2.04),
        ResourceNodeModel::DeadTree => Some(1.42),
        ResourceNodeModel::CoalOre | ResourceNodeModel::IronOre | ResourceNodeModel::SulfurOre => {
            None
        }
    }
}
