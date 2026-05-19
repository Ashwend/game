use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, STONE_DARK, STONE_EDGE, WOOD_LIGHT, WOOD_MID};

pub(crate) fn impact_wood_chip_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Splinter: a thin elongated box with a small darker cap to read at any angle.
    builder.add_box([0.0, 0.0, 0.0], [0.045, 0.012, 0.022], WOOD_LIGHT);
    builder.add_box([0.030, 0.0, 0.0], [0.015, 0.014, 0.018], WOOD_MID);
    builder.build()
}

pub(crate) fn impact_stone_shard_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Angular pebble: small octa rock plus a brighter cap face.
    builder.add_octa_rock([0.0, 0.0, 0.0], [0.05, 0.05, 0.05], STONE_DARK);
    builder.add_octa_rock([0.0, 0.022, 0.0], [0.028, 0.022, 0.028], STONE_EDGE);
    builder.build()
}
