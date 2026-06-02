use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

#[derive(Clone, Copy)]
pub(crate) struct OreNodeStyle {
    pub(crate) base_color: MeshColor,
    pub(crate) accent_color: MeshColor,
    pub(crate) chunk_color: MeshColor,
    pub(crate) chunk_highlight: MeshColor,
}

pub(crate) const COAL_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.26, 0.27, 0.28, 1.0],
    accent_color: [0.18, 0.19, 0.20, 1.0],
    chunk_color: [0.05, 0.05, 0.06, 1.0],
    chunk_highlight: [0.12, 0.12, 0.13, 1.0],
};

pub(crate) const IRON_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.52, 0.50, 0.46, 1.0],
    accent_color: [0.40, 0.38, 0.34, 1.0],
    chunk_color: [0.62, 0.30, 0.18, 1.0],
    chunk_highlight: [0.78, 0.42, 0.24, 1.0],
};

pub(crate) const SULFUR_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.48, 0.46, 0.42, 1.0],
    accent_color: [0.36, 0.34, 0.30, 1.0],
    chunk_color: [0.96, 0.80, 0.18, 1.0],
    chunk_highlight: [1.00, 0.92, 0.36, 1.0],
};

/// Plain rock vein, same silhouette as the ore variants, but the
/// "chunks" embedded in the top are just darker rock instead of a
/// metallic/coal/sulfur colour. Reads as "weathered exposed stone".
pub(crate) const STONE_VEIN: OreNodeStyle = OreNodeStyle {
    base_color: [0.58, 0.56, 0.52, 1.0],
    accent_color: [0.46, 0.44, 0.41, 1.0],
    chunk_color: [0.42, 0.40, 0.38, 1.0],
    chunk_highlight: [0.66, 0.64, 0.60, 1.0],
};

pub(crate) fn low_poly_ore_node_mesh(style: OreNodeStyle) -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Layered base rock mound, bigger central mass plus smaller flanking stones.
    builder.add_rock_lump([0.00, 0.00, 0.00], [1.00, 1.00, 1.00], style.base_color);
    builder.add_rock_lump([-0.32, 0.00, 0.18], [0.62, 0.74, 0.58], style.accent_color);
    builder.add_rock_lump([0.38, 0.00, -0.12], [0.54, 0.62, 0.52], style.accent_color);
    builder.add_rock_lump([0.04, 0.00, -0.38], [0.46, 0.52, 0.44], style.base_color);
    // Embedded ore chunks placed at varied heights/angles on top of the rocks.
    add_ore_chunks(&mut builder, style);
    builder.build()
}

fn add_ore_chunks(builder: &mut LowPolyMeshBuilder, style: OreNodeStyle) {
    // Each chunk is positioned over one of the mound's high spots, the
    // central peak (y≈0.58) or one of the three flanking stones (peaks
    // around y=0.30–0.43), with the centre tuned to sink under the local
    // surface so the visible portion pokes out by a similar amount on
    // every chunk. Without that, chunks placed over a slope used to look
    // like they were floating in mid-air.
    let placements: &[([f32; 3], [f32; 3])] = &[
        // Main outcrop on the central peak.
        ([0.04, 0.42, -0.04], [0.17, 0.18, 0.16]),
        // Smaller chunk on the upper front-right shoulder of the central mound.
        ([0.20, 0.30, 0.10], [0.12, 0.13, 0.12]),
        // Sitting on the back-left flanking stone's peak (~(-0.30, 0.43, 0.17)).
        ([-0.28, 0.30, 0.18], [0.13, 0.14, 0.13]),
        // Sitting on the front-right flanking stone's peak (~(0.39, 0.36, -0.13)).
        ([0.34, 0.22, -0.10], [0.11, 0.12, 0.11]),
        // Sitting on the back flanking stone's peak (~(0.05, 0.30, -0.39)).
        ([0.05, 0.18, -0.34], [0.10, 0.11, 0.10]),
        // Filler on the left slope between the central mound and stone 2.
        ([-0.18, 0.22, 0.04], [0.10, 0.11, 0.10]),
    ];
    for (centre, scale) in placements {
        builder.add_octa_rock(*centre, *scale, style.chunk_color);
        builder.add_octa_rock(
            [centre[0], centre[1] + scale[1] * 0.55, centre[2]],
            [scale[0] * 0.45, scale[1] * 0.35, scale[2] * 0.45],
            style.chunk_highlight,
        );
    }
}
