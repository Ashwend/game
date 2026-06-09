use bevy::prelude::*;

use crate::items::ItemId;

/// Client-side placement state for the deployable currently selected in
/// the actionbar. Carries the live ghost pose (world position + yaw)
/// plus whether the server is expected to accept this pose, which the
/// renderer uses to switch between green/red ghost materials.
///
/// The state is reset whenever the active actionbar slot changes, held
/// rotation does *not* persist across deployable swaps. This keeps the
/// "what am I about to place?" mental model trivial.
#[derive(Resource, Debug, Default, Clone)]
pub(crate) struct DeployablePlacementState {
    /// Item id of the deployable the player currently holds. `None`
    /// when nothing in the active actionbar slot is placeable.
    pub(crate) item_id: Option<ItemId>,
    /// World-space ground position the ghost is anchored to. `None`
    /// when the player isn't looking at a valid surface (e.g. straight
    /// up at the sky). While the player holds right-mouse to rotate, this
    /// is frozen so fine-tuning the angle can't nudge the spot.
    pub(crate) world_position: Option<Vec3>,
    /// Yaw the ghost is rotated to, in radians. Until the player takes
    /// manual control the ghost auto-faces the player (front toward them);
    /// holding right-mouse freezes position + camera and turns mouse
    /// motion into rotation. See [`manual_yaw`](Self::manual_yaw).
    pub(crate) yaw: f32,
    /// True once the player has rotated the ghost themselves (held
    /// right-mouse, or tapped `R`). While false the ghost keeps re-facing
    /// the player every frame; once true that auto-facing stops so the
    /// dialled-in angle survives repositioning. Reset when the active
    /// deployable changes or after a placement commits.
    pub(crate) manual_yaw: bool,
    /// Whether the current pose is a legal placement. Drives the ghost
    /// material and gates the place command.
    pub(crate) valid: bool,
}

impl DeployablePlacementState {
    #[expect(dead_code, reason = "reset helper kept for the placement-cancel path")]
    pub(crate) fn clear(&mut self) {
        self.item_id = None;
        self.world_position = None;
        self.valid = false;
        // Yaw is intentionally preserved across `clear` so a player
        // who re-selects the same deployable doesn't lose the spin
        // they dialled in. The select-changed path resets it explicitly.
    }
}
