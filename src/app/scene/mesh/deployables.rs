//! Low-poly meshes for placed structures. The visuals are intentionally
//! blocky and quick to read so the player can recognise a workbench or
//! furnace at a glance — detail can come later without changing the
//! footprint, since the AABB collider is driven by the item's
//! `DeployableProfile` half-extents and not by mesh bounds.
//!
//! Every box in these meshes is butted edge-to-edge against its
//! neighbour with no overlap. Overlapping faces produce z-fighting
//! flicker at distance — particularly visible on the furnace, which
//! the player walks around. If you tweak a dimension here, sanity-check
//! it against the others on the same axis.

use bevy::prelude::Mesh;

use super::builder::{
    IRON_BAND, LEATHER_WRAP, LowPolyMeshBuilder, MeshColor, STONE_DARK, STONE_EDGE, STONE_LIGHT,
    WOOD_DARK, WOOD_LIGHT, WOOD_MID,
};

/// Workbench tier 1: a four-legged plank table with stretchers
/// connecting the front and back leg pairs at the bottom, a stacked
/// plank tabletop, and a mounted iron vice on the right edge.
///
/// Stretchers sit at the leg's bottom and butt cleanly against the
/// inner face of each leg — earlier revisions had a single cross-brace
/// floating in the middle (no leg contact), which read as "loose
/// hovering bar" instead of a connected frame.
pub(crate) fn low_poly_workbench_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();

    // Four legs.
    let leg_half = [0.05, 0.45, 0.05];
    let leg_offsets = [
        [-0.46, 0.45, -0.26],
        [0.46, 0.45, -0.26],
        [-0.46, 0.45, 0.26],
        [0.46, 0.45, 0.26],
    ];
    for offset in leg_offsets {
        builder.add_box(offset, leg_half, WOOD_DARK);
    }

    // Front + back stretchers spanning the leg pairs along x. Half-width
    // = 0.41 lands the ends exactly at the inner face of each leg
    // (legs occupy x=±0.41..±0.51), so they look connected without
    // crossing into the leg geometry.
    let stretcher_half = [0.41, 0.03, 0.04];
    builder.add_box([0.0, 0.18, 0.26], stretcher_half, WOOD_DARK);
    builder.add_box([0.0, 0.18, -0.26], stretcher_half, WOOD_DARK);

    // Side stretchers along z, tucked between front + back legs. End
    // half-extent of 0.18 means the bar runs z=±0.21 → adjacent to
    // each leg's inner face (legs at z=±0.21..±0.31).
    let side_stretcher_half = [0.04, 0.03, 0.18];
    builder.add_box([-0.46, 0.18, 0.0], side_stretcher_half, WOOD_DARK);
    builder.add_box([0.46, 0.18, 0.0], side_stretcher_half, WOOD_DARK);

    // Tabletop plank stack — two slabs to read as planks. Stacked
    // edge-to-edge (lower slab top y=0.96, upper slab bottom y=0.96)
    // so they don't z-fight.
    builder.add_box([0.0, 0.92, 0.0], [0.55, 0.04, 0.34], WOOD_LIGHT);
    builder.add_box([0.0, 0.99, 0.0], [0.55, 0.03, 0.34], WOOD_MID);

    // Vice block mounted on the right edge.
    builder.add_box([0.36, 1.07, 0.18], [0.10, 0.05, 0.10], IRON_BAND);
    builder.add_box([0.36, 1.15, 0.18], [0.04, 0.03, 0.10], IRON_BAND);

    // Hide-strap detail on the leg corners — leans into the "lashed
    // together" early-game aesthetic that the existing tool meshes share.
    builder.add_box([-0.46, 0.86, -0.26], [0.07, 0.02, 0.07], LEATHER_WRAP);
    builder.add_box([0.46, 0.86, -0.26], [0.07, 0.02, 0.07], LEATHER_WRAP);

    builder.build()
}

/// Furnace tier 1: a stone hearth built as four walls around a real
/// cavity, with a chimney rising off the back. Composing the body as
/// walls (instead of two overlapping halves) means the mouth is an
/// actual hole the player can see into, and there's no z-fighting at
/// the seam where two same-coloured slabs used to overlap.
pub(crate) fn low_poly_crude_furnace_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();

    // Base slab. The walls above start exactly at the base's top
    // (y=0.20) so there's no overlap.
    builder.add_box([0.0, 0.10, 0.0], [0.50, 0.10, 0.45], STONE_DARK);

    // Four walls forming the hearth shell. y=0.20..0.92 for all walls.
    // Side walls cover the full footprint depth; the back wall fills
    // the gap between them along z. The front side is left open — the
    // mouth.
    let wall_height_half = 0.36;
    let wall_y = 0.20 + wall_height_half;
    // Side walls (left + right): x=±0.40, half-width 0.10 → occupy
    // x=±0.30..±0.50. Full depth z=±0.45.
    builder.add_box(
        [-0.40, wall_y, 0.0],
        [0.10, wall_height_half, 0.45],
        STONE_LIGHT,
    );
    builder.add_box(
        [0.40, wall_y, 0.0],
        [0.10, wall_height_half, 0.45],
        STONE_LIGHT,
    );
    // Back wall: fills x=-0.30..0.30 (between side walls), z=-0.45..-0.30.
    builder.add_box(
        [0.0, wall_y, -0.375],
        [0.30, wall_height_half, 0.075],
        STONE_DARK,
    );

    // Inner ceiling: closes the top of the cavity from inside. Sits
    // between the side walls in x and between the back wall + mouth in
    // z, at y=0.78..0.92. Tinted darker so the cavity reads as a
    // shadowed interior.
    builder.add_box([0.0, 0.85, 0.0], [0.30, 0.07, 0.375], HEARTH_INTERIOR);

    // Capstone lid sitting on top of the walls. y=0.92..1.00 — adjacent
    // to wall tops without overlap.
    builder.add_box([0.0, 0.96, 0.0], [0.52, 0.04, 0.47], STONE_EDGE);

    // Chimney: stack of two boxes rising off the back of the capstone.
    // Bottom at y=1.00 (capstone top) → no overlap.
    builder.add_box([0.0, 1.28, -0.20], [0.16, 0.28, 0.14], STONE_DARK);
    builder.add_box([0.0, 1.60, -0.20], [0.13, 0.04, 0.12], STONE_LIGHT);

    // Loading lip in front of the mouth, sitting on top of the base.
    // Pushed forward to z=0.45..0.53 so it doesn't overlap the base
    // (which ends at z=0.45).
    builder.add_box([0.0, 0.22, 0.49], [0.20, 0.02, 0.04], IRON_BAND);

    builder.build()
}

/// Used for the inside of the furnace cavity — darker than the outer
/// stones so the interior reads as a shadowed mouth even before the
/// lit-furnace point light kicks in.
const HEARTH_INTERIOR: MeshColor = [0.18, 0.16, 0.14, 1.0];
