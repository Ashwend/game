//! Procedural low-poly meshes for the base-building system: the door
//! placement ghost (panel + swing-arc indicator), the sleeping bag, and the
//! held hammer / building-plan viewmodels.
//!
//! The rendered building pieces and door panels are authored Blender glbs
//! (`art/building/build_pieces.py` + `build_door.py`), built from the same
//! box layout as [`crate::building::piece_local_boxes`] so the silhouette
//! matches what blocks movement; only the ghost + small viewmodels stay
//! procedural here. All colours are linear albedos in the prop range
//! documented in [`super::builder`].

use bevy::prelude::*;

use crate::building::{
    DOOR_OPEN_ANGLE_RAD, DOOR_PANEL_HEIGHT_M, DOOR_PANEL_THICKNESS_M, DOOR_PANEL_WIDTH_M,
};

use super::builder::{LowPolyMeshBuilder, MeshColor, scale_rgb};

// Palettes (linear albedo, see builder.rs for the calibration notes).
const DOOR_LOG: MeshColor = [0.170, 0.082, 0.030, 1.0];
const DOOR_BRACE: MeshColor = [0.080, 0.040, 0.016, 1.0];
const HAFT_WOOD: MeshColor = [0.230, 0.105, 0.038, 1.0];
const HAMMER_HEAD: MeshColor = [0.165, 0.090, 0.038, 1.0];
const IRON_BAND: MeshColor = [0.300, 0.310, 0.330, 1.0];
const PARCHMENT: MeshColor = [0.430, 0.350, 0.215, 1.0];
const PARCHMENT_EDGE: MeshColor = [0.300, 0.235, 0.135, 1.0];
const TWINE_TIE: MeshColor = [0.260, 0.205, 0.105, 1.0];

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

/// Held construction hammer, built in the shared held-item reference
/// frame the authored tools use (pommel at y ≈ -0.514, head at the top,
/// vertical haft along Y; see docs/playbooks/art-pipeline.md). The swing pose and
/// grip transform assume that frame, so matching it is what makes the
/// hammer sit in the hand like the hatchet does, head up, striking face
/// forward, instead of floating as a tiny crossbar.
pub(crate) fn held_hammer_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Haft: pommel at -0.514 up to the head seat at +0.26.
    builder.add_box([0.0, -0.127, 0.0], [0.023, 0.387, 0.023], HAFT_WOOD);
    // Pommel knob + twine grip wrap at the lower hand position.
    builder.add_box(
        [0.0, -0.505, 0.0],
        [0.032, 0.018, 0.032],
        scale_rgb(HAFT_WOOD, 0.8),
    );
    builder.add_box([0.0, -0.30, 0.0], [0.027, 0.075, 0.027], TWINE_TIE);
    // Head: a heavy block across the top with its long (striking) axis
    // along Z so the faces point forward/backward in hand.
    builder.add_box([0.0, 0.305, 0.0], [0.058, 0.058, 0.135], HAMMER_HEAD);
    // Iron hoops shrunk-fit near both striking faces.
    builder.add_box([0.0, 0.305, 0.105], [0.062, 0.062, 0.016], IRON_BAND);
    builder.add_box([0.0, 0.305, -0.105], [0.062, 0.062, 0.016], IRON_BAND);
    builder.build()
}

/// Held building plan: a rolled parchment scroll with a twine tie, sized
/// to the same reference frame (held mid-shaft, leaning into view).
pub(crate) fn held_building_plan_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // The roll: an octagon-ish tube faked with two crossed boxes,
    // spanning from just above the grip to head height.
    builder.add_box([0.0, -0.05, 0.0], [0.043, 0.26, 0.043], PARCHMENT);
    builder.add_box(
        [0.0, -0.05, 0.0],
        [0.032, 0.265, 0.032],
        scale_rgb(PARCHMENT, 1.08),
    );
    // Slightly unrolled flap.
    builder.add_box([0.072, -0.03, 0.0], [0.036, 0.225, 0.007], PARCHMENT_EDGE);
    // Twine ties near both ends.
    builder.add_box([0.0, 0.13, 0.0], [0.05, 0.016, 0.05], TWINE_TIE);
    builder.add_box([0.0, -0.21, 0.0], [0.05, 0.016, 0.05], TWINE_TIE);
    builder.build()
}
