//! Placeholder meshes for the crude (hand-harvestable) resource nodes —
//! surface stones, branch piles, and hay tufts.
//!
//! These are intentionally simple: a single rock lump for the surface
//! stone, a couple of stacked wedges for the branch pile, and a tiny
//! grass cone for the hay tuft. Each one is small enough to read as
//! "interactable ground clutter" at a glance and visually distinct from
//! its larger cousin (ore vein, full tree, …) so the world doesn't look
//! like every node is identical.

use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

const STONE_BASE: MeshColor = [0.55, 0.55, 0.52, 1.0];
const STONE_TOP: MeshColor = [0.68, 0.68, 0.65, 1.0];
const BRANCH_DARK: MeshColor = [0.34, 0.21, 0.10, 1.0];
const BRANCH_LIGHT: MeshColor = [0.55, 0.36, 0.18, 1.0];
const GRASS_BASE: MeshColor = [0.42, 0.55, 0.20, 1.0];
const GRASS_TIP: MeshColor = [0.62, 0.74, 0.32, 1.0];

/// A small rock lump sitting on the ground. Hand-harvestable; yields one
/// `stone` per swing.
pub(crate) fn low_poly_surface_stone_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_rock_lump([0.0, 0.0, 0.0], [0.55, 0.32, 0.55], STONE_BASE);
    builder.add_rock_lump([0.18, 0.0, -0.05], [0.30, 0.20, 0.32], STONE_TOP);
    builder.add_rock_lump([-0.20, 0.0, 0.12], [0.26, 0.18, 0.28], STONE_TOP);
    builder.build()
}

/// A small pile of fallen sticks. Two short wedges crossing slightly so
/// it doesn't read as a single block. The lowest stick rests flush with
/// the terrain so the cast shadow doesn't reveal a gap between the pile
/// and the ground.
pub(crate) fn low_poly_branch_pile_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Long stick lying roughly east-west, base flush with y=0.
    builder.add_box([0.0, 0.05, 0.0], [0.45, 0.05, 0.07], BRANCH_DARK);
    // Shorter stick crossing on a slight angle (approximated by an
    // offset+rotate-equivalent: the builder doesn't rotate, so we just
    // place it nearby with different proportions).
    builder.add_box([0.05, 0.10, 0.08], [0.30, 0.05, 0.06], BRANCH_LIGHT);
    builder.add_box([-0.05, 0.07, -0.12], [0.22, 0.04, 0.05], BRANCH_DARK);
    builder.build()
}

/// A tuft of grass. Two stacked cones (approximated as octa rocks since
/// the builder doesn't have a cone primitive) so it has a bit of
/// silhouette variety.
pub(crate) fn low_poly_hay_grass_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_octa_rock([0.0, 0.0, 0.0], [0.22, 0.35, 0.22], GRASS_BASE);
    builder.add_octa_rock([0.05, 0.18, -0.05], [0.16, 0.30, 0.16], GRASS_TIP);
    builder.add_octa_rock([-0.06, 0.10, 0.06], [0.14, 0.26, 0.14], GRASS_BASE);
    builder.build()
}
