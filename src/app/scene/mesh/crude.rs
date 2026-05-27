//! Placeholder meshes for the crude (hand-harvestable) resource nodes —
//! surface stones, branch piles, and hay tufts.
//!
//! Each is a single primitive: one rock lump, one box stick, one
//! octahedral grass tuft. Kept deliberately low-poly so the world can
//! support a dense scatter without the tris/draw-call cost of a fuller
//! model. The yaw applied at spawn rotates each instance around Y so
//! the silhouette varies even though the mesh doesn't.

use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

const STONE_BASE: MeshColor = [0.55, 0.55, 0.52, 1.0];
const BRANCH_DARK: MeshColor = [0.34, 0.21, 0.10, 1.0];
const GRASS_BASE: MeshColor = [0.42, 0.55, 0.20, 1.0];

/// A single small rock lump sitting on the ground.
pub(crate) fn low_poly_surface_stone_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_rock_lump([0.0, 0.0, 0.0], [0.55, 0.32, 0.55], STONE_BASE);
    builder.build()
}

/// A single stick lying flat on the ground, roughly east-west before yaw.
pub(crate) fn low_poly_branch_pile_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_box([0.0, 0.05, 0.0], [0.45, 0.05, 0.07], BRANCH_DARK);
    builder.build()
}

/// A single grass tuft (octahedral cone approximation since the builder
/// has no cone primitive at this size).
pub(crate) fn low_poly_hay_grass_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_octa_rock([0.0, 0.0, 0.0], [0.22, 0.35, 0.22], GRASS_BASE);
    builder.build()
}
