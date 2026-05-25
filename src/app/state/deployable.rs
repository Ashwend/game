use bevy::prelude::*;

use crate::items::ItemId;

/// Client-side placement state for the deployable currently selected in
/// the actionbar. Carries the live ghost pose (world position + yaw)
/// plus whether the server is expected to accept this pose, which the
/// renderer uses to switch between green/red ghost materials.
///
/// The state is reset whenever the active actionbar slot changes — held
/// rotation does *not* persist across deployable swaps. This keeps the
/// "what am I about to place?" mental model trivial.
#[derive(Resource, Debug, Default, Clone)]
pub(crate) struct DeployablePlacementState {
    /// Item id of the deployable the player currently holds. `None`
    /// when nothing in the active actionbar slot is placeable.
    pub(crate) item_id: Option<ItemId>,
    /// World-space ground position the ghost is anchored to. `None`
    /// when the player isn't looking at a valid surface (e.g. straight
    /// up at the sky).
    pub(crate) world_position: Option<Vec3>,
    /// Yaw the ghost is rotated to, in radians. Held right-mouse rotates
    /// this value without moving `world_position` — the structure spins
    /// in place under the player's aim until they let go.
    pub(crate) yaw: f32,
    /// Whether the current pose is a legal placement. Drives the ghost
    /// material and gates the place command.
    pub(crate) valid: bool,
}

impl DeployablePlacementState {
    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        self.item_id = None;
        self.world_position = None;
        self.valid = false;
        // Yaw is intentionally preserved across `clear` so a player
        // who re-selects the same deployable doesn't lose the spin
        // they dialled in. The select-changed path resets it explicitly.
    }
}
