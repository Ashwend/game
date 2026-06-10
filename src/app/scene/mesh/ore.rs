use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

#[derive(Clone, Copy)]
pub(crate) struct OreNodeStyle {
    pub(crate) base_color: MeshColor,
    pub(crate) accent_color: MeshColor,
    pub(crate) chunk_color: MeshColor,
    pub(crate) chunk_highlight: MeshColor,
}

// Linear albedos (see the palette note in `builder.rs`). Each ore tints the
// *whole* base rock toward its mineral, not just the embedded chunks: the
// chunks alone are too small to identify a node past ~8m, the mass of the
// mound is what reads at gameplay distance.

/// Charcoal-dark rock with near-black seams.
pub(crate) const COAL_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.052, 0.054, 0.058, 1.0],
    accent_color: [0.034, 0.036, 0.040, 1.0],
    chunk_color: [0.010, 0.010, 0.012, 1.0],
    chunk_highlight: [0.085, 0.088, 0.098, 1.0],
};

/// Rust-stained warm brown rock with vivid oxide chunks.
pub(crate) const IRON_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.200, 0.150, 0.105, 1.0],
    accent_color: [0.128, 0.092, 0.060, 1.0],
    chunk_color: [0.340, 0.085, 0.028, 1.0],
    chunk_highlight: [0.500, 0.160, 0.050, 1.0],
};

/// Ochre-cast gray rock with bright yellow crystal pockets.
pub(crate) const SULFUR_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.165, 0.148, 0.105, 1.0],
    accent_color: [0.105, 0.094, 0.066, 1.0],
    chunk_color: [0.850, 0.560, 0.030, 1.0],
    chunk_highlight: [0.950, 0.760, 0.110, 1.0],
};

/// Plain rock vein, same silhouette as the ore variants, but the
/// "chunks" embedded in the top are just darker rock instead of a
/// metallic/coal/sulfur colour. Reads as "weathered exposed stone".
pub(crate) const STONE_VEIN: OreNodeStyle = OreNodeStyle {
    base_color: [0.260, 0.250, 0.225, 1.0],
    accent_color: [0.155, 0.148, 0.130, 1.0],
    chunk_color: [0.120, 0.112, 0.100, 1.0],
    chunk_highlight: [0.360, 0.345, 0.305, 1.0],
};

/// Number of visual depletion stages an ore/vein node steps through while
/// being mined: 0 = untouched, 1 = worn down, 2 = nearly mined out. The
/// fully-empty node despawns with the shatter effect, so there's no
/// "stage 3" mesh.
pub(crate) const ORE_NODE_STAGE_COUNT: usize = 3;

/// All visual depletion stages for one ore style, index = stage. Each
/// stage drops the mound's silhouette (peak 0.58 → 0.39 → 0.22) and the
/// embedded chunk count (5 → 3 → 1) while scattering broken rubble at the
/// base, so a half-mined vein is readable at a glance from gameplay
/// distance.
pub(crate) fn low_poly_ore_node_stage_meshes(style: OreNodeStyle) -> [Mesh; ORE_NODE_STAGE_COUNT] {
    [
        ore_stage_full(style),
        ore_stage_worn(style),
        ore_stage_gutted(style),
    ]
}

fn ore_stage_full(style: OreNodeStyle) -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Layered base rock mound, bigger central mass plus smaller flanking stones.
    builder.add_rock_lump([0.00, 0.00, 0.00], [1.00, 1.00, 1.00], style.base_color);
    builder.add_rock_lump([-0.32, 0.00, 0.18], [0.62, 0.74, 0.58], style.accent_color);
    builder.add_rock_lump([0.38, 0.00, -0.12], [0.54, 0.62, 0.52], style.accent_color);
    builder.add_rock_lump([0.04, 0.00, -0.38], [0.46, 0.52, 0.44], style.base_color);
    // Embedded ore chunks placed at varied heights/angles on top of the rocks.
    // Each chunk is positioned over one of the mound's high spots, the
    // central peak (y≈0.58) or one of the three flanking stones (peaks
    // around y=0.30–0.43), with the centre tuned to sink under the local
    // surface so the visible portion pokes out by a similar amount on
    // every chunk. Without that, chunks placed over a slope used to look
    // like they were floating in mid-air. Five large chunks rather than
    // many small ones: the deposit has to be readable at the ~10m the
    // player actually decides "worth mining?" from.
    add_ore_chunks(
        &mut builder,
        style,
        &[
            // Main outcrop on the central peak.
            ([0.04, 0.40, -0.04], [0.24, 0.25, 0.22]),
            // Chunk on the upper front-right shoulder of the central mound.
            ([0.22, 0.27, 0.11], [0.17, 0.18, 0.17]),
            // Sitting on the back-left flanking stone's peak (~(-0.30, 0.43, 0.17)).
            ([-0.29, 0.27, 0.18], [0.18, 0.19, 0.18]),
            // Sitting on the front-right flanking stone's peak (~(0.39, 0.36, -0.13)).
            ([0.35, 0.19, -0.10], [0.15, 0.16, 0.15]),
            // Sitting on the back flanking stone's peak (~(0.05, 0.30, -0.39)).
            ([0.05, 0.16, -0.33], [0.14, 0.15, 0.14]),
        ],
    );
    builder.build()
}

/// Stage 1: the mound has been visibly bitten into. The central mass is
/// shrunk and lowered, the back flanking stone is mined away entirely
/// (its footprint replaced by rubble), and only three ore chunks remain,
/// re-seated onto the new, lower peaks.
fn ore_stage_worn(style: OreNodeStyle) -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_rock_lump([0.00, 0.00, 0.00], [0.84, 0.68, 0.84], style.base_color);
    builder.add_rock_lump([-0.30, 0.00, 0.17], [0.55, 0.60, 0.52], style.accent_color);
    builder.add_rock_lump([0.36, 0.00, -0.11], [0.44, 0.40, 0.42], style.accent_color);
    add_ore_chunks(
        &mut builder,
        style,
        &[
            // Smaller outcrop on the lowered central peak (~y=0.39).
            ([0.03, 0.26, -0.03], [0.19, 0.20, 0.18]),
            // On the back-left flank's peak (~y=0.35).
            ([-0.27, 0.22, 0.17], [0.15, 0.16, 0.15]),
            // Low on the front-right stub (~y=0.23).
            ([0.33, 0.12, -0.09], [0.12, 0.13, 0.12]),
        ],
    );
    // Broken rock scattered where the mined-away mass used to be, plus a
    // couple of spilled ore pebbles. Sells "someone has been working this".
    add_rubble(
        &mut builder,
        style,
        &[
            ([0.10, 0.02, 0.44], [0.09, 0.06, 0.08]),
            ([-0.06, 0.02, -0.46], [0.10, 0.05, 0.09]),
            ([0.52, 0.02, 0.12], [0.08, 0.05, 0.08]),
        ],
        &[([0.30, 0.02, 0.30], [0.05, 0.04, 0.05])],
    );
    builder.build()
}

/// Stage 2: nearly mined out. A low cratered core and one stub flank are
/// all that's left standing, one last half-buried chunk marks the
/// remaining yield, and the rubble field has spread wider.
fn ore_stage_gutted(style: OreNodeStyle) -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_rock_lump([0.02, 0.00, -0.02], [0.70, 0.38, 0.70], style.base_color);
    builder.add_rock_lump([-0.26, 0.00, 0.16], [0.40, 0.30, 0.38], style.accent_color);
    add_ore_chunks(
        &mut builder,
        style,
        // Half-buried low on the core (~peak y=0.22).
        &[([0.05, 0.08, 0.02], [0.13, 0.13, 0.12])],
    );
    add_rubble(
        &mut builder,
        style,
        &[
            ([0.14, 0.02, 0.46], [0.10, 0.06, 0.09]),
            ([-0.08, 0.02, -0.48], [0.11, 0.06, 0.10]),
            ([0.55, 0.02, 0.14], [0.09, 0.05, 0.08]),
            ([-0.48, 0.02, -0.16], [0.08, 0.05, 0.08]),
            ([0.34, 0.02, -0.36], [0.07, 0.04, 0.07]),
        ],
        &[
            ([0.28, 0.02, 0.32], [0.05, 0.04, 0.05]),
            ([-0.36, 0.02, 0.38], [0.04, 0.03, 0.04]),
        ],
    );
    builder.build()
}

fn add_ore_chunks(
    builder: &mut LowPolyMeshBuilder,
    style: OreNodeStyle,
    placements: &[([f32; 3], [f32; 3])],
) {
    for (centre, scale) in placements {
        builder.add_octa_rock(*centre, *scale, style.chunk_color);
        builder.add_octa_rock(
            [centre[0], centre[1] + scale[1] * 0.55, centre[2]],
            [scale[0] * 0.45, scale[1] * 0.35, scale[2] * 0.45],
            style.chunk_highlight,
        );
    }
}

/// Ground-level debris for the partially-mined stages: `rock` pieces use
/// the accent rock colour (freshly broken faces), `ore` pieces use the
/// chunk colour so a little spilled mineral reads near the base.
fn add_rubble(
    builder: &mut LowPolyMeshBuilder,
    style: OreNodeStyle,
    rock: &[([f32; 3], [f32; 3])],
    ore: &[([f32; 3], [f32; 3])],
) {
    for (centre, scale) in rock {
        builder.add_octa_rock(*centre, *scale, style.accent_color);
    }
    for (centre, scale) in ore {
        builder.add_octa_rock(*centre, *scale, style.chunk_color);
    }
}
