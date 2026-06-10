//! Placeholder meshes for the crude (hand-harvestable) resource nodes,
//! surface stones, branch piles, and hay tufts.
//!
//! Stones are a single low-poly rock lump; the branch pile is a handful of
//! thin crossed boxes. The hay tuft is a clump of the same tapered straws the
//! streamed detail grass uses ([`GrassBladeMesh`]), just taller and bunched,
//! so the harvestable plant reads as the same kind of grass, only a denser,
//! taller, drier pocket. Kept deliberately low-poly so the world can support
//! a dense scatter without the tris/draw-call cost of a fuller model. The yaw
//! applied at spawn rotates each instance around Y so the silhouette varies
//! even though the mesh doesn't.

use std::f32::consts::TAU;

use bevy::prelude::*;

use super::builder::{GrassBlade, GrassBladeMesh, LowPolyMeshBuilder, MeshColor};
use crate::world::splitmix64;

// Linear albedos; see the palette note in `builder.rs` for the calibration
// anchor (the ground sits at linear ~(0.027, 0.095, 0.040)).
const STONE_BASE: MeshColor = [0.170, 0.165, 0.148, 1.0];
const BRANCH_DARK: MeshColor = [0.095, 0.045, 0.016, 1.0];
const BRANCH_MID: MeshColor = [0.140, 0.075, 0.030, 1.0];
/// Weathered, bark-stripped gray stick.
const BRANCH_GRAY: MeshColor = [0.105, 0.088, 0.062, 1.0];

/// Fixed seed for the hay-tuft blade scatter (deterministic, world-independent).
const HAY_GRASS_SEED: u64 = 0xA17E_4A55_0F23_9C71;
/// Straws per hay tuft and the radius of the clump's footprint (m).
const HAY_GRASS_BLADES: u32 = 34;
const HAY_GRASS_RADIUS: f32 = 0.34;

/// Hay blade root/tip colours (linear). The root stays green like the ground
/// cover; the tip dries to straw gold, which both separates the harvestable
/// tuft from the cosmetic detail grass and telegraphs "fiber".
const HAY_BLADE_BASE: [f32; 3] = [0.050, 0.085, 0.024];
const HAY_BLADE_TIP: [f32; 3] = [0.300, 0.215, 0.058];

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

/// A harvestable hay tuft: a bunched pocket of the same tapered straws as the
/// detail grass, but taller and drying to straw-gold tips (so it reads as a
/// distinct "tall grass" plant rather than blending into the ground cover),
/// clumped into a small disc. Rendered with the same swaying
/// [`GrassMaterial`](crate::app::scene::GrassMaterial) as the detail grass
/// (assigned at node spawn), so it bends in unison with the ground cover.
pub(crate) fn low_poly_hay_grass_mesh() -> Mesh {
    let mut rng = splitmix64(HAY_GRASS_SEED);
    let mut builder = GrassBladeMesh::default();
    for _ in 0..HAY_GRASS_BLADES {
        // Uniform scatter within the clump disc (sqrt keeps it from bunching up
        // at the centre).
        let radius = HAY_GRASS_RADIUS * next_unit(&mut rng).sqrt();
        let theta = next_unit(&mut rng) * TAU;
        let base = Vec2::new(radius * theta.cos(), radius * theta.sin());

        let yaw = next_unit(&mut rng) * TAU;
        // Well above the detail grass so the tuft stands out.
        let height = 0.55 + next_unit(&mut rng) * 0.35;
        let half_width = 0.012 + next_unit(&mut rng) * 0.009;
        // Hay stands up more than detail grass: a modest tilt fans the tuft out.
        let tilt = 0.15 + next_unit(&mut rng) * 0.20;
        let lean_dir = next_unit(&mut rng) * TAU;
        let lean_mag = height * tilt;
        let lean = Vec2::new(lean_dir.cos() * lean_mag, lean_dir.sin() * lean_mag);
        let flex = 0.10 + next_unit(&mut rng) * 0.12;
        let shade = 0.70 + next_unit(&mut rng) * 0.30;
        // How far this blade has dried: tall blades go golden, short ones
        // stay greener, so the tuft grades from a green skirt to straw tops.
        let dryness = ((height - 0.55) / 0.35) * (0.6 + next_unit(&mut rng) * 0.4);
        let (base_color, tip_color) = hay_blade_colors(shade, dryness);

        builder.push_blade(&GrassBlade {
            base,
            yaw,
            height,
            half_width,
            lean,
            flex,
            base_color,
            tip_color,
            // `1.0` keeps the wind shader's distance dither from ever discarding
            // a hay blade (`uv.x < fade` is never true), a harvestable node
            // shouldn't thin with distance; it just despawns at the AoI edge.
            dither: 1.0,
        });
    }
    builder.build()
}

/// Root/tip colours for one hay blade. `shade` darkens the whole blade
/// (per-blade variety); `dryness` in `[0, 1]` blends the tip from ground-
/// cover green toward straw gold. Alpha carries the sway weight (0 root,
/// 1 tip), matching the wind shader's convention.
fn hay_blade_colors(shade: f32, dryness: f32) -> ([f32; 4], [f32; 4]) {
    let green_tip = [0.060, 0.130, 0.045];
    let mix = |index: usize| green_tip[index] + (HAY_BLADE_TIP[index] - green_tip[index]) * dryness;
    (
        [
            HAY_BLADE_BASE[0] * shade,
            HAY_BLADE_BASE[1] * shade,
            HAY_BLADE_BASE[2] * shade,
            0.0,
        ],
        [mix(0) * shade, mix(1) * shade, mix(2) * shade, 1.0],
    )
}

/// Next pseudo-random `f32` in `[0, 1)` from a splitmix64 stream.
fn next_unit(state: &mut u64) -> f32 {
    *state = splitmix64(*state);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}
