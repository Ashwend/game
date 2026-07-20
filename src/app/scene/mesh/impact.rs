use bevy::prelude::*;

use crate::util::hash::hashed_unit;

use super::builder::{LowPolyMeshBuilder, STONE_DARK, STONE_EDGE, WOOD_LIGHT, WOOD_MID};

/// Number of pre-built seeded shape variants per chip family. Spawns pick one
/// by hashing the chip seed, so a burst mixes silhouettes instead of spinning
/// one repeated shape.
pub(crate) const IMPACT_CHIP_MESH_VARIANTS: u32 = 4;

/// Volumetric wood splinter (seeded): an elongated shard with a near-square
/// cross-section and an angled snapped-off end. Real split wood has depth on
/// every axis; the old single thin plank strobed like a flat card as it
/// tumbled (owner report: "very flat and rotates all around").
pub(crate) fn impact_wood_chip_mesh(seed: u32) -> Mesh {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x9E37_79B9);
    let r3 = hashed_unit(seed ^ 0x85EB_CA6B);
    let mut builder = LowPolyMeshBuilder::default();
    let length = 0.038 + r1 * 0.020;
    let thick = 0.015 + r2 * 0.006;
    // Main shaft: slightly rectangular, never plank-flat.
    builder.add_box_oriented(
        [0.0, 0.0, 0.0],
        [length, thick, thick * (0.8 + r3 * 0.4)],
        0.0,
        0.0,
        WOOD_LIGHT,
    );
    // Snapped end: a shorter angled chunk biting off one end, so the shard
    // silhouette is a broken splinter rather than a machined block.
    builder.add_box_oriented(
        [length * 0.72, thick * 0.35, 0.0],
        [length * 0.42, thick * 0.85, thick * 0.7],
        0.30 + r3 * 0.5,
        0.25 + r1 * 0.4,
        WOOD_MID,
    );
    builder.build()
}

/// Volumetric stone chunk (seeded): an irregular faceted lump with a smaller
/// knuckle fused into one side, so it reads as a solid broken chunk from any
/// tumble angle (the old fixed octa + flat top cap read as a spinning shard).
pub(crate) fn impact_stone_chunk_mesh(seed: u32) -> Mesh {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x9E37_79B9);
    let r3 = hashed_unit(seed ^ 0x85EB_CA6B);
    let mut builder = LowPolyMeshBuilder::default();
    let sx = 0.042 + r1 * 0.022;
    let sy = 0.036 + r2 * 0.020;
    let sz = 0.040 + r3 * 0.022;
    builder.add_octa_rock([0.0, 0.0, 0.0], [sx, sy, sz], STONE_DARK);
    // The knuckle sits off-axis in all three dimensions, breaking the
    // symmetric silhouette that made the old shard read as a flat plate.
    builder.add_octa_rock(
        [sx * 0.55, sy * (r3 - 0.35) * 0.8, -sz * 0.4],
        [sx * 0.55, sy * 0.5, sz * 0.5],
        STONE_EDGE,
    );
    builder.build()
}
