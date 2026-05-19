use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

#[derive(Clone, Copy)]
pub(crate) struct OreNodeStyle {
    pub(crate) base_color: MeshColor,
    pub(crate) accent_color: MeshColor,
    pub(crate) chunk_color: MeshColor,
    pub(crate) chunk_highlight: MeshColor,
    pub(crate) chunk_shape: OreChunkShape,
}

#[derive(Clone, Copy)]
pub(crate) enum OreChunkShape {
    Boulder,
    Crystal,
}

pub(crate) const COAL_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.26, 0.27, 0.28, 1.0],
    accent_color: [0.18, 0.19, 0.20, 1.0],
    chunk_color: [0.05, 0.05, 0.06, 1.0],
    chunk_highlight: [0.12, 0.12, 0.13, 1.0],
    chunk_shape: OreChunkShape::Boulder,
};

pub(crate) const IRON_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.52, 0.50, 0.46, 1.0],
    accent_color: [0.40, 0.38, 0.34, 1.0],
    chunk_color: [0.62, 0.30, 0.18, 1.0],
    chunk_highlight: [0.78, 0.42, 0.24, 1.0],
    chunk_shape: OreChunkShape::Boulder,
};

pub(crate) const SULFUR_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.48, 0.46, 0.42, 1.0],
    accent_color: [0.36, 0.34, 0.30, 1.0],
    chunk_color: [0.96, 0.80, 0.18, 1.0],
    chunk_highlight: [1.00, 0.92, 0.36, 1.0],
    chunk_shape: OreChunkShape::Crystal,
};

pub(crate) fn low_poly_ore_node_mesh(style: OreNodeStyle) -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Layered base rock mound — bigger central mass plus smaller flanking stones.
    builder.add_rock_lump([0.00, 0.00, 0.00], [1.00, 1.00, 1.00], style.base_color);
    builder.add_rock_lump([-0.32, 0.00, 0.18], [0.62, 0.74, 0.58], style.accent_color);
    builder.add_rock_lump([0.38, 0.00, -0.12], [0.54, 0.62, 0.52], style.accent_color);
    builder.add_rock_lump([0.04, 0.00, -0.38], [0.46, 0.52, 0.44], style.base_color);
    // Embedded ore chunks placed at varied heights/angles on top of the rocks.
    add_ore_chunks(&mut builder, style);
    builder.build()
}

fn add_ore_chunks(builder: &mut LowPolyMeshBuilder, style: OreNodeStyle) {
    let placements: &[([f32; 3], [f32; 3])] = &[
        ([0.06, 0.46, 0.08], [0.16, 0.18, 0.16]),
        ([-0.22, 0.32, -0.06], [0.12, 0.13, 0.11]),
        ([0.28, 0.30, 0.16], [0.13, 0.14, 0.12]),
        ([-0.18, 0.20, 0.34], [0.10, 0.12, 0.10]),
        ([0.22, 0.18, -0.30], [0.11, 0.13, 0.11]),
        ([-0.04, 0.10, 0.38], [0.09, 0.10, 0.09]),
    ];
    for (centre, scale) in placements {
        match style.chunk_shape {
            OreChunkShape::Boulder => {
                builder.add_octa_rock(*centre, *scale, style.chunk_color);
                builder.add_octa_rock(
                    [centre[0], centre[1] + scale[1] * 0.55, centre[2]],
                    [scale[0] * 0.45, scale[1] * 0.35, scale[2] * 0.45],
                    style.chunk_highlight,
                );
            }
            OreChunkShape::Crystal => {
                builder.add_crystal_cluster(
                    *centre,
                    *scale,
                    style.chunk_color,
                    style.chunk_highlight,
                );
            }
        }
    }
}
