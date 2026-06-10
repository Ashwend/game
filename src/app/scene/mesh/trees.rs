use bevy::prelude::*;

use super::builder::{
    BARK_DARK, BARK_MID, BIRCH_BARK, BIRCH_BARK_BAND, LEAF_BIRCH, LEAF_BIRCH_DARK,
    LEAF_BIRCH_LIGHT, LEAF_PINE, LEAF_PINE_DARK, LEAF_PINE_LIGHT, LowPolyMeshBuilder, MeshColor,
};

// Each pine variant has a clear bare trunk before foliage starts; this lets
// the player read "real tree" rather than "shrub" at a distance. Larger
// variants get more foliage layers so the silhouette stays full at height.

pub(crate) fn low_poly_pine_tree_small_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk: ~1.4m visible before foliage. Flared base, tapering up.
    builder.add_box([0.0, 0.10, 0.0], [0.20, 0.10, 0.20], BARK_DARK);
    builder.add_box([0.0, 0.40, 0.0], [0.16, 0.20, 0.16], BARK_MID);
    builder.add_box([0.0, 0.85, 0.0], [0.14, 0.25, 0.14], BARK_DARK);
    builder.add_box([0.0, 1.25, 0.0], [0.12, 0.15, 0.12], BARK_MID);
    // Foliage cones overlap the upper trunk and stack to ~4.5m. Mid layers
    // sit slightly off the trunk axis; a perfectly lathe-symmetric stack
    // reads as a toy from any angle.
    builder.add_cone(1.10, 0.95, 1.30, 8, LEAF_PINE_DARK);
    builder.add_cone_at([0.07, 1.85, -0.04], 0.95, 1.10, 8, LEAF_PINE);
    builder.add_cone(2.55, 0.85, 0.88, 8, LEAF_PINE_DARK);
    builder.add_cone_at([-0.06, 3.20, 0.04], 0.75, 0.66, 7, LEAF_PINE);
    builder.add_cone(3.80, 0.55, 0.44, 7, LEAF_PINE_LIGHT);
    builder.add_cone(4.20, 0.30, 0.22, 6, LEAF_PINE_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_pine_tree_medium_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Trunk: substantial bare trunk to ~2.2m, then foliage to ~6.5m.
    builder.add_box([0.0, 0.12, 0.0], [0.26, 0.12, 0.26], BARK_DARK);
    builder.add_box([0.0, 0.50, 0.0], [0.21, 0.26, 0.21], BARK_MID);
    builder.add_box([0.0, 1.05, 0.0], [0.18, 0.29, 0.18], BARK_DARK);
    builder.add_box([0.0, 1.60, 0.0], [0.16, 0.26, 0.16], BARK_MID);
    builder.add_box([0.0, 2.05, 0.0], [0.14, 0.19, 0.14], BARK_DARK);
    // Foliage cones stack to 6.5m. Wider base layers; tighter top. Mid
    // layers nudged off-axis for a natural, non-lathe silhouette.
    builder.add_cone(1.85, 1.30, 1.85, 9, LEAF_PINE_DARK);
    builder.add_cone_at([0.10, 2.85, -0.06], 1.20, 1.55, 9, LEAF_PINE);
    builder.add_cone(3.80, 1.10, 1.25, 8, LEAF_PINE_DARK);
    builder.add_cone_at([-0.09, 4.65, 0.06], 0.95, 0.98, 8, LEAF_PINE);
    builder.add_cone(5.40, 0.80, 0.72, 7, LEAF_PINE_LIGHT);
    builder.add_cone_at([0.05, 6.05, -0.03], 0.55, 0.46, 7, LEAF_PINE_LIGHT);
    builder.add_cone(6.40, 0.20, 0.18, 6, LEAF_PINE_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_pine_tree_large_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Old-growth pine: wide flared base, prominent bare trunk to ~3.5m.
    builder.add_box([0.0, 0.16, 0.0], [0.36, 0.16, 0.36], BARK_DARK);
    builder.add_box([0.0, 0.60, 0.0], [0.29, 0.30, 0.29], BARK_MID);
    builder.add_box([0.0, 1.25, 0.0], [0.25, 0.35, 0.25], BARK_DARK);
    builder.add_box([0.0, 1.90, 0.0], [0.22, 0.32, 0.22], BARK_MID);
    builder.add_box([0.0, 2.55, 0.0], [0.20, 0.32, 0.20], BARK_DARK);
    builder.add_box([0.0, 3.15, 0.0], [0.18, 0.28, 0.18], BARK_MID);
    // Seven foliage layers stack to 9m for a dense canopy silhouette,
    // with mid layers drifting off-axis so the old pine reads weathered
    // rather than lathe-turned.
    builder.add_cone(2.60, 1.60, 2.40, 10, LEAF_PINE_DARK);
    builder.add_cone_at([0.12, 3.85, 0.07], 1.50, 2.10, 10, LEAF_PINE);
    builder.add_cone(5.00, 1.35, 1.75, 9, LEAF_PINE_DARK);
    builder.add_cone_at([-0.10, 6.05, -0.06], 1.20, 1.40, 9, LEAF_PINE);
    builder.add_cone(7.00, 1.05, 1.05, 8, LEAF_PINE_DARK);
    builder.add_cone_at([0.06, 7.85, 0.04], 0.85, 0.72, 8, LEAF_PINE_LIGHT);
    builder.add_cone(8.55, 0.55, 0.44, 7, LEAF_PINE_LIGHT);
    builder.add_cone(8.90, 0.20, 0.20, 6, LEAF_PINE_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_birch_tree_small_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Birch trunks are slender; bands give the papery look at any size.
    // Segments stack with cumulative y so the trunk reads as one continuous
    // pole rather than separated discs. Extra-tall top segment so the trunk
    // climbs into the canopy and doesn't read as detached from the green
    // mass above.
    stack_birch_trunk(
        &mut builder,
        [0.045, 0.020],
        &[
            (0.155, 0.24, BIRCH_BARK),
            (0.158, 0.08, BIRCH_BARK_BAND),
            (0.150, 0.40, BIRCH_BARK),
            (0.153, 0.07, BIRCH_BARK_BAND),
            (0.145, 0.52, BIRCH_BARK),
            (0.148, 0.07, BIRCH_BARK_BAND),
            (0.140, 0.60, BIRCH_BARK),
        ],
    );
    // Stub branches bridge the trunk into the side clusters so the canopy
    // reads as carried by limbs rather than balanced on a pole.
    builder.add_box_oriented(
        [0.26, 2.06, -0.03],
        [0.20, 0.025, 0.025],
        0.21,
        0.50,
        BIRCH_BARK_BAND,
    );
    builder.add_box_oriented(
        [-0.25, 2.02, 0.09],
        [0.20, 0.025, 0.025],
        -2.82,
        0.50,
        BIRCH_BARK_BAND,
    );
    // Canopy clusters above the trunk reach ~3.6m. `add_octa_rock`'s bottom
    // vertex sits at `cy - 0.82 * sy`, so canopy centers must be low enough
    // that the lowest bottom vertex dips into the trunk top (~y=1.98) and
    // visually reads as one continuous tree. Side clusters hang lower and
    // wider than the crown so the canopy occupies real vertical depth.
    builder.add_octa_rock([0.09, 2.55, 0.04], [1.10, 0.85, 1.05], LEAF_BIRCH);
    builder.add_octa_rock([-0.58, 2.22, 0.20], [0.70, 0.58, 0.62], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.58, 2.26, -0.14], [0.66, 0.55, 0.60], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.20, 2.90, 0.28], [0.58, 0.46, 0.54], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([-0.30, 2.85, -0.30], [0.52, 0.42, 0.48], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([0.10, 3.20, -0.02], [0.36, 0.36, 0.36], LEAF_BIRCH_LIGHT);
    builder.build()
}

/// Stacks birch trunk segments end-to-end so the resulting cylinder reads as
/// continuous, no horizontal gaps between bark and band sections. Each entry
/// is `(half_width, total_height, color)`. `lean` drifts the segment centres
/// sideways by `[dx, dz]` per metre of height; the per-segment steps are small
/// enough to read as a gentle lean rather than stairs, and a birch that grows
/// dead-plumb reads as a lamp post.
fn stack_birch_trunk(
    builder: &mut LowPolyMeshBuilder,
    lean: [f32; 2],
    segments: &[(f32, f32, MeshColor)],
) {
    let mut y = 0.0;
    for &(half_width, height, color) in segments {
        let half_height = height * 0.5;
        let center_y = y + half_height;
        builder.add_box(
            [lean[0] * center_y, center_y, lean[1] * center_y],
            [half_width, half_height, half_width],
            color,
        );
        y += height;
    }
}

pub(crate) fn low_poly_birch_tree_medium_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Extended trunk with more bands. Bands stay thin so they read as
    // papery markings rather than separate sections.
    stack_birch_trunk(
        &mut builder,
        [0.050, -0.025],
        &[
            (0.200, 0.36, BIRCH_BARK),
            (0.202, 0.10, BIRCH_BARK_BAND),
            (0.195, 0.52, BIRCH_BARK),
            (0.197, 0.09, BIRCH_BARK_BAND),
            (0.190, 0.70, BIRCH_BARK),
            (0.192, 0.09, BIRCH_BARK_BAND),
            (0.185, 0.56, BIRCH_BARK),
            (0.187, 0.09, BIRCH_BARK_BAND),
            (0.180, 0.66, BIRCH_BARK),
            (0.182, 0.08, BIRCH_BARK_BAND),
            (0.170, 0.40, BIRCH_BARK),
        ],
    );
    // Stub branches reaching into the lowered side clusters.
    builder.add_box_oriented(
        [0.36, 3.40, -0.04],
        [0.26, 0.030, 0.030],
        0.18,
        0.45,
        BIRCH_BARK_BAND,
    );
    builder.add_box_oriented(
        [-0.32, 3.32, 0.11],
        [0.26, 0.030, 0.030],
        -2.85,
        0.45,
        BIRCH_BARK_BAND,
    );
    // Layered canopy of overlapping octa-rocks centered around 4.0m. The
    // dark side clusters hang noticeably below the crown so the canopy
    // reads deep instead of "blob on a pole".
    builder.add_octa_rock([0.16, 4.05, -0.06], [1.55, 1.05, 1.45], LEAF_BIRCH);
    builder.add_octa_rock([-0.78, 3.50, 0.24], [1.00, 0.78, 0.92], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.82, 3.56, -0.16], [0.95, 0.74, 0.88], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.12, 3.30, 0.58], [0.62, 0.50, 0.58], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.26, 4.45, 0.40], [0.82, 0.66, 0.74], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([-0.44, 4.36, -0.42], [0.74, 0.58, 0.66], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([0.14, 4.85, -0.04], [0.48, 0.42, 0.48], LEAF_BIRCH_LIGHT);
    builder.build()
}

pub(crate) fn low_poly_birch_tree_large_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Mature birch: thicker trunk, more bands carrying the look up to ~5m.
    stack_birch_trunk(
        &mut builder,
        [0.055, 0.022],
        &[
            (0.260, 0.44, BIRCH_BARK),
            (0.262, 0.12, BIRCH_BARK_BAND),
            (0.255, 0.62, BIRCH_BARK),
            (0.257, 0.10, BIRCH_BARK_BAND),
            (0.250, 0.78, BIRCH_BARK),
            (0.252, 0.10, BIRCH_BARK_BAND),
            (0.245, 0.62, BIRCH_BARK),
            (0.247, 0.10, BIRCH_BARK_BAND),
            (0.240, 0.80, BIRCH_BARK),
            (0.242, 0.10, BIRCH_BARK_BAND),
            (0.235, 0.62, BIRCH_BARK),
            (0.237, 0.09, BIRCH_BARK_BAND),
            (0.225, 0.46, BIRCH_BARK),
        ],
    );
    // Limbs into the low side clusters; the old birch earns a third,
    // lower branch for a craggy profile.
    builder.add_box_oriented(
        [0.50, 4.92, -0.05],
        [0.34, 0.035, 0.035],
        0.20,
        0.42,
        BIRCH_BARK_BAND,
    );
    builder.add_box_oriented(
        [-0.46, 4.82, 0.15],
        [0.34, 0.035, 0.035],
        -2.80,
        0.45,
        BIRCH_BARK_BAND,
    );
    builder.add_box_oriented(
        [0.32, 4.46, 0.42],
        [0.26, 0.030, 0.030],
        0.95,
        0.50,
        BIRCH_BARK_BAND,
    );
    // Bigger, denser canopy: side clusters dropped well below the crown,
    // plus low stragglers, so the head reads deep and weather-torn.
    builder.add_octa_rock([0.22, 5.75, 0.06], [2.10, 1.40, 1.95], LEAF_BIRCH);
    builder.add_octa_rock([-1.10, 5.08, 0.34], [1.30, 1.00, 1.16], LEAF_BIRCH_DARK);
    builder.add_octa_rock([1.14, 5.18, -0.22], [1.22, 0.95, 1.12], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.18, 4.74, 0.88], [0.78, 0.62, 0.72], LEAF_BIRCH_DARK);
    builder.add_octa_rock([-0.56, 4.66, -0.72], [0.72, 0.58, 0.66], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.34, 6.30, 0.58], [1.08, 0.84, 0.98], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([-0.62, 6.18, -0.58], [1.00, 0.80, 0.92], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([0.46, 6.40, -0.40], [0.78, 0.60, 0.72], LEAF_BIRCH_LIGHT);
    builder.add_octa_rock([-0.26, 5.95, 0.50], [0.74, 0.58, 0.68], LEAF_BIRCH_DARK);
    builder.add_octa_rock([0.16, 6.78, 0.05], [0.62, 0.54, 0.62], LEAF_BIRCH_LIGHT);
    builder.build()
}

// ---------------------------------------------------------------------------
// Distance LOD meshes
//
// Low-poly stand-ins swapped in past ~80 m via `VisibilityRange` hard switch (see
// the resource-node spawn path). Each preserves its full-detail counterpart's
// height, canopy extent, and colour palette so the hard switch reads as the same
// tree, but with ~1/3 the triangles (single trunk box, few low-segment cones /
// canopy blobs). At 80 m+ on screen the facet count is imperceptible; the win
// is the per-frame vertex throughput across a forest of distant trees.
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
