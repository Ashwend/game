use bevy::prelude::*;

use super::builder::{
    BARK_DARK, BARK_MID, BIRCH_BARK, BIRCH_BARK_BAND, DEAD_WOOD, DEAD_WOOD_DARK, LEAF_BIRCH,
    LEAF_BIRCH_DARK, LEAF_BIRCH_LIGHT, LEAF_PINE, LEAF_PINE_DARK, LEAF_PINE_LIGHT,
    LowPolyMeshBuilder,
};

pub(crate) fn low_poly_pine_tree_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Root flare and tapered trunk.
    builder.add_box([0.0, 0.05, 0.0], [0.17, 0.06, 0.17], BARK_DARK);
    builder.add_box([0.0, 0.22, 0.0], [0.13, 0.13, 0.13], BARK_MID);
    builder.add_box([0.0, 0.44, 0.0], [0.115, 0.10, 0.115], BARK_DARK);
    builder.add_box([0.0, 0.60, 0.0], [0.10, 0.08, 0.10], BARK_MID);
    // Layered foliage cones — alternating dark/medium shades for depth, a
    // brighter outermost layer near the top for a sun-catching highlight.
    builder.add_cone(0.54, 0.50, 0.84, 8, LEAF_PINE_DARK);
    builder.add_cone(0.84, 0.50, 0.70, 8, LEAF_PINE);
    builder.add_cone(1.14, 0.50, 0.56, 8, LEAF_PINE_DARK);
    builder.add_cone(1.42, 0.45, 0.42, 8, LEAF_PINE);
    builder.add_cone(1.68, 0.40, 0.28, 7, LEAF_PINE_LIGHT);
    // Top spike.
    builder.add_cone(1.92, 0.26, 0.14, 6, LEAF_PINE_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_birch_tree_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk built from alternating light bark and dark horizontal bands —
    // the classic birch look. Bands are very thin so they read as papery
    // markings rather than separate sections.
    builder.add_box([0.0, 0.06, 0.0], [0.115, 0.06, 0.115], BIRCH_BARK);
    builder.add_box([0.0, 0.17, 0.0], [0.118, 0.030, 0.118], BIRCH_BARK_BAND);
    builder.add_box([0.0, 0.32, 0.0], [0.108, 0.12, 0.108], BIRCH_BARK);
    builder.add_box([0.0, 0.48, 0.0], [0.112, 0.030, 0.112], BIRCH_BARK_BAND);
    builder.add_box([0.0, 0.64, 0.0], [0.105, 0.12, 0.105], BIRCH_BARK);
    builder.add_box([0.0, 0.80, 0.0], [0.108, 0.026, 0.108], BIRCH_BARK_BAND);
    builder.add_box([0.0, 0.96, 0.0], [0.10, 0.12, 0.10], BIRCH_BARK);
    builder.add_box([0.0, 1.11, 0.0], [0.103, 0.022, 0.103], BIRCH_BARK_BAND);
    builder.add_box([0.0, 1.22, 0.0], [0.092, 0.08, 0.092], BIRCH_BARK);
    // Dense canopy of overlapping octa-rocks with three shades for depth.
    builder.add_octa_rock([0.0, 1.54, 0.0], [0.70, 0.46, 0.66], LEAF_BIRCH);
    builder.add_octa_rock([-0.32, 1.36, 0.10], [0.42, 0.32, 0.38], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.34, 1.38, -0.06], [0.40, 0.32, 0.38], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.10, 1.68, 0.18], [0.38, 0.30, 0.34], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([-0.22, 1.62, -0.20], [0.34, 0.26, 0.30], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([0.04, 1.84, -0.02], [0.22, 0.20, 0.22], LEAF_BIRCH_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_dead_tree_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Twisted, gnarled trunk built from offset segments. The slight X/Z
    // jitter on each section gives the silhouette a weathered, organic feel.
    builder.add_box([0.0, 0.06, 0.0], [0.16, 0.06, 0.16], DEAD_WOOD_DARK);
    builder.add_box([0.02, 0.22, -0.01], [0.13, 0.12, 0.13], DEAD_WOOD);
    builder.add_box([-0.01, 0.46, 0.02], [0.12, 0.13, 0.12], DEAD_WOOD_DARK);
    builder.add_box([0.03, 0.70, -0.02], [0.11, 0.12, 0.11], DEAD_WOOD);
    builder.add_box([-0.02, 0.92, 0.01], [0.10, 0.10, 0.10], DEAD_WOOD_DARK);
    builder.add_box([0.0, 1.10, 0.0], [0.09, 0.08, 0.09], DEAD_WOOD);
    // Splintered, jagged top.
    builder.add_tri_prism([[-0.07, 1.18], [0.07, 1.42], [0.08, 1.18]], 0.08, DEAD_WOOD);
    builder.add_tri_prism(
        [[-0.06, 1.18], [-0.04, 1.34], [0.04, 1.18]],
        0.05,
        DEAD_WOOD_DARK,
    );
    // Broken branches sticking out in different directions, with knotty
    // tip stubs to suggest they've been snapped off.
    builder.add_box([0.28, 0.82, 0.04], [0.20, 0.044, 0.044], DEAD_WOOD_DARK);
    builder.add_box([0.44, 0.86, 0.04], [0.05, 0.030, 0.030], DEAD_WOOD);
    builder.add_box([-0.24, 1.00, -0.06], [0.18, 0.040, 0.040], DEAD_WOOD);
    builder.add_box([-0.38, 0.94, -0.06], [0.05, 0.028, 0.028], DEAD_WOOD_DARK);
    builder.add_box([0.06, 0.56, 0.26], [0.040, 0.034, 0.18], DEAD_WOOD_DARK);
    builder.add_box([-0.10, 0.38, -0.22], [0.040, 0.034, 0.18], DEAD_WOOD);
    // Knots — small nubs on the trunk for character.
    builder.add_box([0.10, 0.36, 0.10], [0.026, 0.026, 0.026], DEAD_WOOD_DARK);
    builder.add_box([-0.09, 0.62, -0.10], [0.026, 0.026, 0.026], DEAD_WOOD_DARK);
    builder.add_box([0.07, 0.86, -0.09], [0.024, 0.024, 0.024], DEAD_WOOD_DARK);
    builder.build()
}
