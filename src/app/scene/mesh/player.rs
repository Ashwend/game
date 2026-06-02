use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

/// Local-space Y offset (relative to the network player entity's transform
/// origin, which is `PLAYER_VISUAL_CENTER_Y` above the feet) where the head
/// top sits. Used by the nametag overlay to anchor the floating label.
pub(crate) const PLAYER_HEAD_TOP_LOCAL_Y: f32 = 0.86;

const TORSO: MeshColor = [0.92, 0.52, 0.16, 1.0];
const TORSO_DARK: MeshColor = [0.62, 0.32, 0.08, 1.0];
const HEAD_SKIN: MeshColor = [0.95, 0.78, 0.60, 1.0];
const HAIR: MeshColor = [0.18, 0.12, 0.08, 1.0];
const LIMB: MeshColor = [0.30, 0.32, 0.40, 1.0];
const LIMB_DARK: MeshColor = [0.18, 0.20, 0.26, 1.0];
const ACCENT: MeshColor = [0.20, 0.18, 0.16, 1.0];

/// Builds a low-poly humanoid mesh for remote players. Single mesh so the
/// entire body inherits one parent transform, the snapshot apply system
/// only has to interpolate the root.
///
/// Local frame: y=0 is `PLAYER_VISUAL_CENTER_Y` above feet, -Z is forward
/// (matching Bevy's default forward direction). Total height ~1.76 m so a
/// head-top anchor at +0.86 sits just above the visor strip.
pub(crate) fn low_poly_player_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();

    // Legs: two stout cuboids descending to the ground (local y = -0.9 is
    // feet level). Center each so the bottom sits flush with the ground.
    for x in [-0.13, 0.13] {
        builder.add_box([x, -0.55, 0.0], [0.10, 0.35, 0.11], LIMB);
        // Boots, a darker stub at the very bottom for visual contrast.
        builder.add_box([x, -0.85, 0.02], [0.11, 0.05, 0.13], LIMB_DARK);
    }

    // Torso: chunky chest with a slightly recessed waist for a hint of shape.
    builder.add_box([0.0, 0.10, 0.0], [0.30, 0.34, 0.17], TORSO);
    // Belt, darker band across the waist seam.
    builder.add_box([0.0, -0.24, 0.0], [0.31, 0.04, 0.18], TORSO_DARK);

    // Arms: hanging at the sides, slightly forward so the silhouette reads
    // from any angle.
    for x in [-0.40, 0.40] {
        builder.add_box([x, 0.08, 0.0], [0.09, 0.30, 0.09], TORSO);
        builder.add_box([x, -0.27, 0.02], [0.10, 0.07, 0.10], HEAD_SKIN);
    }

    // Head: cube with a strip of "hair" on top and a thin dark visor across
    // the front so the facing direction reads at a glance.
    builder.add_box([0.0, 0.62, 0.0], [0.17, 0.17, 0.16], HEAD_SKIN);
    builder.add_box([0.0, 0.78, 0.0], [0.18, 0.04, 0.17], HAIR);
    // Visor, front-facing dark strip. -Z is forward in Bevy.
    builder.add_box([0.0, 0.64, -0.16], [0.16, 0.04, 0.01], ACCENT);
    // Tiny nose nub for additional facing cue.
    builder.add_box([0.0, 0.58, -0.17], [0.025, 0.03, 0.02], ACCENT);

    builder.build()
}
