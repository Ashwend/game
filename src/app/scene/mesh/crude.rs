//! Placeholder meshes for the crude (hand-harvestable) resource nodes: surface
//! stones and branch piles.
//!
//! Stones are a single low-poly rock lump; the branch pile is a handful of thin
//! crossed boxes. Kept deliberately low-poly so the world can support a dense
//! scatter without the tris/draw-call cost of a fuller model. The yaw applied at
//! spawn rotates each instance around Y so the silhouette varies even though the
//! mesh doesn't. (Hay grass is no longer built here, it uses the shared grass-card
//! texture via [`super::builder::build_hay_tuft_mesh`] + a `StandardMaterial`.)

use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

// Linear albedos; see the palette note in `builder.rs` for the calibration
// anchor (the ground sits at linear ~(0.027, 0.095, 0.040)).
const STONE_BASE: MeshColor = [0.170, 0.165, 0.148, 1.0];
const BRANCH_DARK: MeshColor = [0.095, 0.045, 0.016, 1.0];
const BRANCH_MID: MeshColor = [0.140, 0.075, 0.030, 1.0];
/// Weathered, bark-stripped gray stick.
const BRANCH_GRAY: MeshColor = [0.105, 0.088, 0.062, 1.0];

/// A single small rock lump sitting on the ground.
pub(crate) fn low_poly_surface_stone_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_rock_lump([0.0, 0.0, 0.0], [0.55, 0.32, 0.55], STONE_BASE);
    builder.build()
}

/// A loose pile of sticks: a few thin boxes crossed at varied yaws and
/// slight tilts, in mixed bark tones, so it reads as fallen branches
/// rather than a milled plank. Footprint stays under ~0.9m before yaw.
pub(crate) fn low_poly_branch_pile_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Bottom layer: two long sticks nearly flat on the ground.
    builder.add_box_oriented(
        [0.00, 0.035, 0.02],
        [0.42, 0.035, 0.040],
        0.15,
        0.00,
        BRANCH_DARK,
    );
    builder.add_box_oriented(
        [0.05, 0.060, -0.08],
        [0.36, 0.030, 0.036],
        -0.60,
        0.05,
        BRANCH_MID,
    );
    // Crossing layer: a weathered stick resting on the first two.
    builder.add_box_oriented(
        [-0.06, 0.095, 0.06],
        [0.31, 0.028, 0.032],
        1.00,
        -0.06,
        BRANCH_GRAY,
    );
    // Short offcuts tucked against the pile.
    builder.add_box_oriented(
        [0.16, 0.045, 0.12],
        [0.18, 0.025, 0.028],
        -1.25,
        0.00,
        BRANCH_DARK,
    );
    builder.add_box_oriented(
        [-0.18, 0.030, -0.11],
        [0.14, 0.020, 0.024],
        0.65,
        0.00,
        BRANCH_MID,
    );
    builder.build()
}
