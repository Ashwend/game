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
    /// Door placement: hinge/swing mirror toggled by right-click while
    /// the door ghost is up. Mirroring is a half-turn of the ghost (and
    /// the placed door), so this only feeds the yaw computation.
    pub(crate) door_flip: bool,
    /// Door placement: the doorway building block the ghost is snapped
    /// to. `None` while no free doorway is near the aim point.
    pub(crate) door_target: Option<crate::protocol::DeployedEntityId>,
    /// Material cost readout for a building-piece ghost, already projected
    /// to the screen so the in-game overlay can pin it under the ghost.
    /// `None` for deployables and doors (they consume the held item itself,
    /// not raw materials) and whenever no building ghost is shown. Filled
    /// each frame by `update_placement_ghost_system`.
    pub(crate) building_cost: Option<BuildingCostReadout>,
    /// Torch placement: `true` when the aim is on the side of a wall (the
    /// torch mounts and tilts out), `false` for an upright floor/ground
    /// mount. Shipped in `PlaceDeployableCommand.wall_mounted` and folded
    /// into the ghost's kind so the preview tilts. Ignored for other kinds.
    pub(crate) wall_mounted: bool,
    /// Ruin footprint circles for the current world, mirroring the server's
    /// placement gate so a ghost inside a footprint (plus the placement
    /// margin) turns red instead of being rejected on the wire. Cached per
    /// `(seed, dims)` (the key below) and refreshed lazily by
    /// `update_placement_ghost_system` when the world changes.
    pub(crate) ruin_footprints: Vec<crate::world::RuinFootprint>,
    /// The `(seed, dims)` the cached footprints were computed for.
    pub(crate) ruin_footprints_key: Option<(u64, crate::world::ChunkDims)>,
}

/// What the building ghost's cost label shows: the material, how much the
/// piece needs, how much the player currently has, and where on screen to
/// anchor the label (the projected base of the ghost). The UI colours it by
/// [`Self::affordable`] so the player can see at a glance whether they can pay.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BuildingCostReadout {
    pub(crate) material: &'static str,
    pub(crate) required: u16,
    pub(crate) have: u32,
    pub(crate) anchor: Vec2,
}

impl BuildingCostReadout {
    /// Whether the player holds enough of the material to place the piece.
    pub(crate) fn affordable(&self) -> bool {
        self.have >= u32::from(self.required)
    }
}
