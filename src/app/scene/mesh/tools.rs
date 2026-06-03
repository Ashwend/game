use bevy::prelude::*;

use super::builder::{
    IRON_BAND, IRON_EDGE, IRON_HEAD, IRON_HEAD_DARK, LEATHER_WRAP, LowPolyMeshBuilder, STONE_DARK,
    STONE_EDGE, STONE_LIGHT, WOOD_DARK, WOOD_LIGHT, WOOD_MID,
};

pub(crate) fn low_poly_hatchet_mesh() -> Mesh {
    // Built in the same orientation convention as the pickaxe: the head
    // extends along mesh +X (which becomes world -Z, i.e. forward in the
    // first-person view, after the model's Y rotation). The mesh-Z axis is
    // the blade's thickness, kept thin so the blade reads as a blade rather
    // than a block from the side profile.
    let mut builder = LowPolyMeshBuilder::default();

    // Handle shaft (tapered look via two stacked boxes).
    builder.add_box([0.0, -0.06, 0.0], [0.024, 0.28, 0.024], WOOD_LIGHT);
    builder.add_box([0.0, -0.30, 0.0], [0.028, 0.06, 0.028], WOOD_MID);
    // Pommel knob.
    builder.add_box([0.0, -0.38, 0.0], [0.036, 0.030, 0.034], WOOD_DARK);
    // Leather grip wraps near the bottom of the shaft.
    builder.add_box([0.0, -0.20, 0.0], [0.031, 0.022, 0.031], LEATHER_WRAP);
    builder.add_box([0.0, -0.10, 0.0], [0.031, 0.014, 0.031], LEATHER_WRAP);
    // Iron band binding the head to the handle.
    builder.add_box([0.0, 0.17, 0.0], [0.054, 0.020, 0.038], IRON_BAND);
    // Wooden head saddle that the stone bit wraps around.
    builder.add_box([0.0, 0.22, 0.0], [0.050, 0.044, 0.040], WOOD_DARK);

    // Stone bit body, flared trapezoid in the mesh-XY plane. The half-depth
    // is small so the blade is a true blade in profile rather than a block.
    builder.add_quad_prism(
        [[0.04, 0.10], [0.22, 0.07], [0.32, 0.32], [0.04, 0.32]],
        0.020,
        STONE_LIGHT,
    );
    // Bright cutting edge along the leading curve of the bit. Sits slightly
    // proud of the body so the highlight catches the light during the swing.
    builder.add_tri_prism(
        [[0.22, 0.08], [0.36, 0.20], [0.30, 0.30]],
        0.013,
        STONE_EDGE,
    );
    // Beard, small downward hook at the front-bottom of the bit.
    builder.add_tri_prism(
        [[0.04, 0.10], [0.22, 0.05], [0.20, 0.10]],
        0.013,
        STONE_DARK,
    );
    // Upper horn, small triangular peak at the front-top, balances the beard.
    builder.add_tri_prism(
        [[0.04, 0.32], [0.22, 0.36], [0.28, 0.32]],
        0.013,
        STONE_DARK,
    );
    // Poll, short counterweight behind the eye (mesh -X), i.e. the back of
    // the head in the held view.
    builder.add_box([-0.07, 0.22, 0.0], [0.046, 0.036, 0.036], STONE_DARK);

    builder.build()
}

pub(crate) fn low_poly_pickaxe_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Longer, slightly heavier handle than the hatchet.
    builder.add_box([0.0, -0.08, 0.0], [0.026, 0.32, 0.026], WOOD_LIGHT);
    builder.add_box([0.0, -0.36, 0.0], [0.030, 0.060, 0.030], WOOD_MID);
    // Pommel knob.
    builder.add_box([0.0, -0.44, 0.0], [0.040, 0.030, 0.038], WOOD_DARK);
    // Leather grip wraps.
    builder.add_box([0.0, -0.24, 0.0], [0.033, 0.022, 0.033], LEATHER_WRAP);
    builder.add_box([0.0, -0.12, 0.0], [0.033, 0.014, 0.033], LEATHER_WRAP);
    builder.add_box([0.0, 0.00, 0.0], [0.033, 0.014, 0.033], LEATHER_WRAP);
    // Iron band binding the head to the haft.
    builder.add_box([0.0, 0.20, 0.0], [0.040, 0.020, 0.054], IRON_BAND);
    // Wooden head saddle that the stone head is set into.
    builder.add_box([0.0, 0.24, 0.0], [0.038, 0.040, 0.058], WOOD_DARK);

    // Stone head, central eye block sitting on the saddle. Wider than
    // the saddle so the head reads as a distinct stone piece capping the
    // handle rather than a continuation of the wood.
    builder.add_box([0.0, 0.27, 0.0], [0.054, 0.030, 0.054], STONE_DARK);
    // Top crown, bright stone that catches light along the upper face.
    builder.add_box([0.0, 0.298, 0.0], [0.048, 0.012, 0.044], STONE_LIGHT);

    // Forward pick, long tapered stone prong to a sharp point. The
    // profile narrows in both height and depth so the spike reads as a
    // real pick rather than a wedge.
    builder.add_quad_prism(
        [[0.054, 0.300], [0.22, 0.268], [0.22, 0.252], [0.054, 0.232]],
        0.030,
        STONE_LIGHT,
    );
    builder.add_tri_prism(
        [[0.22, 0.268], [0.30, 0.262], [0.22, 0.252]],
        0.017,
        STONE_EDGE,
    );

    // Back tail, short blunt chisel counterweight opposite the pick.
    // Kept stubby so the silhouette reads as asymmetric (pick + tail)
    // rather than a double-headed rock hammer.
    builder.add_quad_prism(
        [
            [-0.054, 0.300],
            [-0.15, 0.286],
            [-0.15, 0.234],
            [-0.054, 0.232],
        ],
        0.034,
        STONE_LIGHT,
    );
    builder.add_tri_prism(
        [[-0.15, 0.286], [-0.19, 0.262], [-0.15, 0.234]],
        0.024,
        STONE_EDGE,
    );

    builder.build()
}

// Tier-2 iron tools are rendered as two layers sharing one swing transform: a
// matte BODY (the hewn handle, drawn with the standard tool material) and a
// shiny HEAD (the forged iron parts, drawn with a metallic material).
// Splitting the mesh is what lets only the iron catch the light while the wood
// stays matte, Bevy binds one material per mesh. Both layers are built in the
// same mesh-local frame so they overlay exactly.

/// Iron hatchet, matte body layer: hewn handle, pommel, and leather wraps.
/// Drawn with the shared matte tool material.
pub(crate) fn low_poly_iron_hatchet_body_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_box([0.0, -0.06, 0.0], [0.026, 0.30, 0.026], WOOD_MID);
    builder.add_box([0.0, -0.32, 0.0], [0.030, 0.060, 0.030], WOOD_DARK);
    // Pommel cap.
    builder.add_box([0.0, -0.40, 0.0], [0.038, 0.030, 0.036], WOOD_DARK);
    // Leather grip wraps.
    builder.add_box([0.0, -0.20, 0.0], [0.033, 0.022, 0.033], LEATHER_WRAP);
    builder.add_box([0.0, -0.10, 0.0], [0.033, 0.014, 0.033], LEATHER_WRAP);
    builder.build()
}

/// Iron hatchet, shiny head layer: forged bit, honed edge, beard, horn, poll,
/// and the iron langets/band that bind the head to the haft. Drawn with the
/// metallic iron material.
pub(crate) fn low_poly_iron_hatchet_head_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Iron langet sleeve + band binding the head onto the haft.
    builder.add_box([0.0, 0.14, 0.0], [0.038, 0.070, 0.030], IRON_HEAD_DARK);
    builder.add_box([0.0, 0.18, 0.0], [0.056, 0.022, 0.040], IRON_BAND);

    // Forged bit body, a clean flared trapezoid.
    builder.add_quad_prism(
        [[0.03, 0.10], [0.24, 0.06], [0.34, 0.33], [0.03, 0.33]],
        0.018,
        IRON_HEAD,
    );
    // Honed cutting edge along the leading curve, proud of the body.
    builder.add_tri_prism([[0.24, 0.06], [0.40, 0.20], [0.32, 0.32]], 0.011, IRON_EDGE);
    // Beard, downward hook at the front-bottom.
    builder.add_tri_prism(
        [[0.03, 0.10], [0.24, 0.04], [0.20, 0.10]],
        0.011,
        IRON_HEAD_DARK,
    );
    // Upper horn, balances the beard at the front-top.
    builder.add_tri_prism(
        [[0.03, 0.33], [0.22, 0.37], [0.30, 0.33]],
        0.011,
        IRON_HEAD_DARK,
    );
    // Poll, short counterweight behind the eye.
    builder.add_box([-0.07, 0.215, 0.0], [0.048, 0.048, 0.034], IRON_HEAD_DARK);
    builder.build()
}

/// Iron pickaxe, matte body layer: hewn handle, pommel, and leather wraps.
pub(crate) fn low_poly_iron_pickaxe_body_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_box([0.0, -0.08, 0.0], [0.028, 0.34, 0.028], WOOD_MID);
    builder.add_box([0.0, -0.38, 0.0], [0.032, 0.060, 0.032], WOOD_DARK);
    // Pommel cap.
    builder.add_box([0.0, -0.46, 0.0], [0.042, 0.030, 0.040], WOOD_DARK);
    // Leather grip wraps.
    builder.add_box([0.0, -0.24, 0.0], [0.035, 0.022, 0.035], LEATHER_WRAP);
    builder.add_box([0.0, -0.12, 0.0], [0.035, 0.014, 0.035], LEATHER_WRAP);
    builder.add_box([0.0, 0.00, 0.0], [0.035, 0.014, 0.035], LEATHER_WRAP);
    builder.build()
}

/// Iron pickaxe, shiny head layer: forward pick, stubby back tail, eye block,
/// band, and bright crown. Asymmetric so it never reads as a double-headed
/// hammer. Drawn with the metallic iron material.
pub(crate) fn low_poly_iron_pickaxe_head_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Iron eye block clamping the head to the haft, plus a binding band.
    builder.add_box([0.0, 0.24, 0.0], [0.052, 0.050, 0.052], IRON_HEAD_DARK);
    builder.add_box([0.0, 0.235, 0.0], [0.060, 0.020, 0.046], IRON_BAND);
    // Bright crown catching light along the top of the head.
    builder.add_box([0.0, 0.292, 0.0], [0.050, 0.014, 0.046], IRON_EDGE);

    // Forward pick, long tapered iron spike to a sharp point.
    builder.add_quad_prism(
        [[0.052, 0.300], [0.24, 0.270], [0.24, 0.250], [0.052, 0.225]],
        0.028,
        IRON_HEAD,
    );
    builder.add_tri_prism(
        [[0.24, 0.270], [0.34, 0.260], [0.24, 0.250]],
        0.015,
        IRON_EDGE,
    );

    // Back tail, short blunt chisel counterweight opposite the pick.
    builder.add_quad_prism(
        [
            [-0.052, 0.300],
            [-0.17, 0.286],
            [-0.17, 0.232],
            [-0.052, 0.225],
        ],
        0.032,
        IRON_HEAD,
    );
    builder.add_tri_prism(
        [[-0.17, 0.286], [-0.215, 0.260], [-0.17, 0.232]],
        0.020,
        IRON_EDGE,
    );
    builder.build()
}
