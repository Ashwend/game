//! Procedural low-poly mesh for the base-building system: the door placement
//! ghost (panel + swing-arc indicator). (The construction hammer and the
//! building-plan scroll are now authored glbs, see `art/items/*` +
//! `ItemVisualAssets`.)
//!
//! The rendered building pieces and door panels are authored Blender glbs
//! (`art/building/build_pieces.py` + `build_door.py`), built from the same
//! box layout as [`crate::building::piece_local_boxes`] so the silhouette
//! matches what blocks movement; only the ghost stays procedural here. All
//! colours are linear albedos in the prop range documented in [`super::builder`].

use bevy::prelude::*;

use crate::building::{
    DOOR_OPEN_ANGLE_RAD, DOOR_PANEL_HEIGHT_M, DOOR_PANEL_THICKNESS_M, DOOR_PANEL_WIDTH_M,
};

use super::builder::{LowPolyMeshBuilder, MeshColor};

// Palettes (linear albedo, see builder.rs for the calibration notes).
const DOOR_LOG: MeshColor = [0.170, 0.082, 0.030, 1.0];
const DOOR_BRACE: MeshColor = [0.080, 0.040, 0.016, 1.0];

/// Placement ghost for the door: the closed panel centred in the opening
/// plus a flat swing-arc fan on the side the door will open toward. The
/// arc is the "which way does it swing?" indicator; flipping the door
/// rotates the whole ghost half a turn, which mirrors hinge and arc
/// together exactly like the placed door behaves.
pub(crate) fn door_ghost_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    let hw = DOOR_PANEL_WIDTH_M / 2.0;
    let hh = DOOR_PANEL_HEIGHT_M / 2.0;
    let ht = DOOR_PANEL_THICKNESS_M / 2.0;
    builder.add_box([0.0, hh, 0.0], [hw, hh, ht], DOOR_LOG);

    // Swing arc: a fan of triangles at ankle height sweeping from the
    // closed pose toward +Z (the open direction), hinged at -X.
    let hinge_x = -hw;
    let radius = DOOR_PANEL_WIDTH_M;
    let segments = 8;
    let y = 0.06;
    for i in 0..segments {
        let a0 = DOOR_OPEN_ANGLE_RAD * (i as f32 / segments as f32);
        let a1 = DOOR_OPEN_ANGLE_RAD * ((i + 1) as f32 / segments as f32);
        // Closed pose points along +X from the hinge; opening sweeps
        // toward +Z.
        let p0 = [hinge_x + radius * a0.cos(), y, radius * a0.sin()];
        let p1 = [hinge_x + radius * a1.cos(), y, radius * a1.sin()];
        let hinge = [hinge_x, y, 0.0];
        // Double-sided so the indicator reads from both sides of the wall.
        builder.push_triangle(hinge, p0, p1, DOOR_BRACE);
        builder.push_triangle(hinge, p1, p0, DOOR_BRACE);
    }
    builder.build()
}
