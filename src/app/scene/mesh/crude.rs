//! Placeholder meshes for the crude (hand-harvestable) resource nodes —
//! surface stones, branch piles, and hay tufts.
//!
//! Stones and sticks are a single low-poly primitive (one rock lump, one box).
//! The hay tuft is a clump of the same tapered straws the streamed detail grass
//! uses ([`GrassBladeMesh`]) — just taller and bunched — so the harvestable
//! plant reads as the same kind of grass, only a denser, taller pocket. Kept
//! deliberately low-poly so the world can support a dense scatter without the
//! tris/draw-call cost of a fuller model. The yaw applied at spawn rotates each
//! instance around Y so the silhouette varies even though the mesh doesn't.

use std::f32::consts::TAU;

use bevy::prelude::*;

use super::builder::{
    GrassBlade, GrassBladeMesh, LowPolyMeshBuilder, MeshColor, grass_blade_colors,
};
use crate::world::splitmix64;

const STONE_BASE: MeshColor = [0.55, 0.55, 0.52, 1.0];
const BRANCH_DARK: MeshColor = [0.34, 0.21, 0.10, 1.0];

/// Fixed seed for the hay-tuft blade scatter (deterministic, world-independent).
const HAY_GRASS_SEED: u64 = 0xA17E_4A55_0F23_9C71;
/// Straws per hay tuft and the radius of the clump's footprint (m).
const HAY_GRASS_BLADES: u32 = 34;
const HAY_GRASS_RADIUS: f32 = 0.34;
/// RGB brightness multiplier so the harvestable tuft reads brighter than the
/// surrounding detail grass.
const HAY_GRASS_BRIGHTNESS: f32 = 1.35;

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

/// A harvestable hay tuft: a bunched pocket of the same tapered straws as the
/// detail grass, but taller + brighter (so it reads as a distinct "tall grass"
/// plant that stands out), clumped into a small disc. Rendered with the same
/// swaying [`GrassMaterial`](crate::app::scene::GrassMaterial) as the detail
/// grass (assigned at node spawn), so it bends in unison with the ground cover.
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
        // Well above the detail grass (0.16–0.36 m) so the tuft stands out.
        let height = 0.55 + next_unit(&mut rng) * 0.35;
        let half_width = 0.018 + next_unit(&mut rng) * 0.014;
        // Taller straws lean a touch more, fanning the tuft outward.
        let lean = 0.04 + next_unit(&mut rng) * 0.10;
        let lean_dir = next_unit(&mut rng) * TAU;
        let bend = Vec2::new(lean_dir.cos() * lean, lean_dir.sin() * lean);
        let shade = 0.72 + next_unit(&mut rng) * 0.28;
        let warm = next_unit(&mut rng) * 2.0 - 1.0;
        let (base_color, tip_color) = grass_blade_colors(shade, warm);

        builder.push_blade(&GrassBlade {
            base,
            yaw,
            height,
            half_width,
            bend,
            // Lift the green so the tuft pops against the darker ground cover.
            base_color: brighten(base_color, HAY_GRASS_BRIGHTNESS),
            tip_color: brighten(tip_color, HAY_GRASS_BRIGHTNESS),
            // `1.0` keeps the wind shader's distance dither from ever discarding
            // a hay blade (`uv.x < fade` is never true) — a harvestable node
            // shouldn't thin with distance; it just despawns at the AoI edge.
            dither: 1.0,
        });
    }
    builder.build()
}

/// Scale a blade colour's RGB toward brighter (alpha — the sway weight — is left
/// untouched), clamped to white.
fn brighten(color: [f32; 4], factor: f32) -> [f32; 4] {
    [
        (color[0] * factor).min(1.0),
        (color[1] * factor).min(1.0),
        (color[2] * factor).min(1.0),
        color[3],
    ]
}

/// Next pseudo-random `f32` in `[0, 1)` from a splitmix64 stream.
fn next_unit(state: &mut u64) -> f32 {
    *state = splitmix64(*state);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}
