use bevy::prelude::*;

use super::builder::{
    BARK_DARK, BIRCH_BARK, LEAF_BIRCH, LEAF_BIRCH_LIGHT, LEAF_PINE, LEAF_PINE_DARK,
    LEAF_PINE_LIGHT, LowPolyMeshBuilder,
};

// The full-detail live trees (pine + birch, three sizes each) AND the bare dead
// snags are now authored Blender glbs (`art/trees/*`, loaded in `scene::assets`):
// a textured bark trunk + (for live trees) an alpha-masked needle/leaf canopy.
// Only the cheap distance LODs stay procedural here, because a flat vertex-coloured
// stand-in is all that survives the 80 m `VisibilityRange` switch. See
// [Icon to 3D model](../../../../docs/icon-to-model.md) for the tree glb pipeline
// and [Materials](../../../../docs/materials.md) for the material conventions.

// ---------------------------------------------------------------------------
// Distance LOD meshes
//
// Low-poly stand-ins swapped in past ~80 m via `VisibilityRange` hard switch (see
// the resource-node spawn path). Each preserves its full-detail glb counterpart's
// height, canopy extent, and (retuned to the texture midtones) colour palette so
// the hard switch reads as the same tree, but with ~1/3 the triangles (single
// trunk box, few low-segment cones / canopy blobs) and no texture fetch. At 80 m+
// on screen the facet count is imperceptible; the win is the per-frame vertex
// throughput across a forest of distant trees.
// ---------------------------------------------------------------------------

// Connection rule: a cone's base disc sits at its `base_y`, and
// `add_octa_rock`'s lowest vertex sits at `cy - 0.82 * sy`. For the canopy to
// read as attached, that low point must dip *below* the trunk's top so they
// overlap rather than float. Trunk top = box center_y + half_y.

pub(crate) fn low_poly_pine_tree_small_lod_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk top at y = 1.1; first cone base at 0.95 overlaps it.
    builder.add_box([0.0, 0.55, 0.0], [0.16, 0.55, 0.16], BARK_DARK);
    builder.add_cone(0.95, 1.65, 1.25, 5, LEAF_PINE_DARK);
    builder.add_cone(2.40, 1.50, 0.85, 5, LEAF_PINE);
    builder.add_cone(3.60, 1.00, 0.40, 5, LEAF_PINE_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_pine_tree_medium_lod_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk top at y = 1.8; first cone base at 1.55 overlaps it.
    builder.add_box([0.0, 0.90, 0.0], [0.20, 0.90, 0.20], BARK_DARK);
    builder.add_cone(1.55, 2.45, 1.85, 6, LEAF_PINE_DARK);
    builder.add_cone(3.60, 2.00, 1.25, 6, LEAF_PINE);
    builder.add_cone(5.20, 1.40, 0.60, 5, LEAF_PINE_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_pine_tree_large_lod_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk top at y = 2.8; first cone base at 2.4 overlaps it.
    builder.add_box([0.0, 1.40, 0.0], [0.26, 1.40, 0.26], BARK_DARK);
    builder.add_cone(2.40, 3.00, 2.40, 6, LEAF_PINE_DARK);
    builder.add_cone(5.00, 2.40, 1.60, 6, LEAF_PINE);
    builder.add_cone(7.00, 2.00, 0.80, 5, LEAF_PINE_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_birch_tree_small_lod_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk top at y = 2.0; octa1 low point = 2.5 - 0.82*0.85 = 1.80 < 2.0.
    builder.add_box([0.0, 1.00, 0.0], [0.15, 1.00, 0.15], BIRCH_BARK);
    builder.add_octa_rock([0.0, 2.50, 0.0], [1.05, 0.85, 1.00], LEAF_BIRCH);
    builder.add_octa_rock([0.0, 3.05, 0.0], [0.55, 0.50, 0.55], LEAF_BIRCH_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_birch_tree_medium_lod_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk top at y = 3.0; octa1 low point = 3.65 - 0.82*1.0 = 2.83 < 3.0.
    builder.add_box([0.0, 1.50, 0.0], [0.19, 1.50, 0.19], BIRCH_BARK);
    builder.add_octa_rock([0.0, 3.65, 0.0], [1.50, 1.00, 1.40], LEAF_BIRCH);
    builder.add_octa_rock([0.0, 4.40, 0.0], [0.70, 0.60, 0.70], LEAF_BIRCH_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_birch_tree_large_lod_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk top at y = 4.0; octa1 low point = 4.95 - 0.82*1.35 = 3.84 < 4.0.
    builder.add_box([0.0, 2.00, 0.0], [0.24, 2.00, 0.24], BIRCH_BARK);
    builder.add_octa_rock([0.0, 4.95, 0.0], [2.05, 1.35, 1.90], LEAF_BIRCH);
    builder.add_octa_rock([0.0, 5.95, 0.0], [0.90, 0.75, 0.85], LEAF_BIRCH_LIGHT);
    builder.build()
}
