//! Procedural low-poly meshes for the base-building system: the four
//! building pieces at every tier, the hewn log door panel, the door
//! placement ghost (panel + swing-arc indicator), the sleeping bag, and
//! the held hammer / building-plan viewmodels.
//!
//! Piece geometry comes from [`crate::building::piece_local_boxes`], the
//! same boxes the collision grid is built from, so what the player sees is
//! exactly what blocks movement; each tier then layers cheap decorative
//! accents (stick posts, plank seams, stone coursing) over those boxes.
//! All colours are linear albedos in the prop range documented in
//! [`super::builder`].

use bevy::prelude::*;

use crate::building::{
    BuildingPiece, BuildingTier, CEILING_THICKNESS_M, DOOR_OPEN_ANGLE_RAD, DOOR_PANEL_HEIGHT_M,
    DOOR_PANEL_THICKNESS_M, DOOR_PANEL_WIDTH_M, DOORWAY_OPENING_WIDTH_M, FOUNDATION_SIZE_M,
    STAIR_RISE_M, piece_local_boxes,
};

use super::builder::{LowPolyMeshBuilder, MeshColor, scale_rgb};

// Tier palettes (linear albedo, see builder.rs for the calibration notes).
const STICKS_BASE: MeshColor = [0.150, 0.085, 0.034, 1.0];
const STICKS_DARK: MeshColor = [0.085, 0.046, 0.018, 1.0];
const WOOD_PLANK: MeshColor = [0.215, 0.105, 0.040, 1.0];
const WOOD_SEAM: MeshColor = [0.110, 0.052, 0.020, 1.0];
const STONE_BASE: MeshColor = [0.185, 0.192, 0.178, 1.0];
const STONE_MORTAR: MeshColor = [0.072, 0.078, 0.072, 1.0];
const DOOR_LOG: MeshColor = [0.170, 0.082, 0.030, 1.0];
const DOOR_BRACE: MeshColor = [0.080, 0.040, 0.016, 1.0];
const BAG_FABRIC: MeshColor = [0.062, 0.105, 0.058, 1.0];
const BAG_LINING: MeshColor = [0.240, 0.205, 0.140, 1.0];
const HAFT_WOOD: MeshColor = [0.230, 0.105, 0.038, 1.0];
const HAMMER_HEAD: MeshColor = [0.165, 0.090, 0.038, 1.0];
const IRON_BAND: MeshColor = [0.300, 0.310, 0.330, 1.0];
const PARCHMENT: MeshColor = [0.430, 0.350, 0.215, 1.0];
const PARCHMENT_EDGE: MeshColor = [0.300, 0.235, 0.135, 1.0];
const TWINE_TIE: MeshColor = [0.260, 0.205, 0.105, 1.0];

/// Cheap deterministic shade multiplier so repeated elements (sticks,
/// stone blocks) don't read as copy-paste. Golden-ratio fractional walk,
/// no RNG so the mesh is identical every build.
fn shade(index: usize, spread: f32) -> f32 {
    let t = ((index as f32) * 0.618_034).fract();
    1.0 - spread / 2.0 + t * spread
}

/// World mesh for a building piece at a tier. Geometry derives from the
/// same `piece_local_boxes` segments the collision grid uses, so the
/// silhouette always matches what blocks movement; each tier then renders
/// those segments in its own construction style:
///
/// - **Sticks**: an open lattice of lashed sticks with real gaps, rails
///   top/bottom and a diagonal brace, you can see daylight through it.
/// - **Wood**: solid plank construction with seam lines (the original
///   look).
/// - **Stone**: mortar-backed block coursing with staggered joints and
///   per-block shade variation.
pub(crate) fn building_piece_mesh(piece: BuildingPiece, tier: BuildingTier) -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();

    match piece {
        BuildingPiece::Foundation => {
            foundation_mesh(&mut builder, tier);
            return builder.build();
        }
        BuildingPiece::Ceiling => {
            ceiling_mesh(&mut builder, tier);
            return builder.build();
        }
        BuildingPiece::Stairs => {
            stairs_mesh(&mut builder, tier);
            return builder.build();
        }
        _ => {}
    }

    for (index, (center, half)) in piece_local_boxes(piece).iter().enumerate() {
        let center = [center.0, center.1, center.2];
        let mut half = [half.0, half.1, half.2];
        // Wood renders the segments as plain solid boxes, so the
        // sill/header bands (the boxes after the two jambs) would meet
        // the jambs in exactly coplanar, exactly abutting quads: a
        // T-junction that tears into hairline cracks. Tuck the bands
        // behind the jambs instead: recessed a hair in Z, widened to
        // overlap inside them, so the joint has no visible seam at all.
        if tier == BuildingTier::HewnWood
            && matches!(piece, BuildingPiece::WindowWall | BuildingPiece::Doorway)
            && index >= 2
        {
            half = [half[0] + 0.04, half[1], half[2] - 0.001];
        }
        match tier {
            BuildingTier::Sticks => stick_lattice_segment(&mut builder, center, half),
            BuildingTier::HewnWood => builder.add_box(center, half, WOOD_PLANK),
            BuildingTier::Stone => stone_segment(&mut builder, center, half),
        }
    }
    if tier == BuildingTier::HewnWood {
        // Horizontal plank seams every ~0.6 m, split around openings.
        let proud = crate::building::WALL_THICKNESS_M / 2.0 + 0.012;
        for i in 1..5 {
            let y = i as f32 * 0.6;
            add_horizontal_strip(&mut builder, piece, y, proud, WOOD_SEAM);
        }
    }

    builder.build()
}

/// Fill one solid wall segment with an open stick lattice: top/bottom
/// rails, slightly leaning vertical sticks with visible gaps, and a
/// diagonal brace on segments wide enough to carry one. The collider
/// stays the solid segment box; the gaps are purely visual, exactly the
/// "barely holding together" read the sticks tier wants.
fn stick_lattice_segment(builder: &mut LowPolyMeshBuilder, center: [f32; 3], half: [f32; 3]) {
    let [cx, cy, cz] = center;
    let [hx, hy, hz] = half;
    let rail_half = [hx, 0.045, hz + 0.012];

    // Rails pin the lattice top and bottom (plus a waist rail on tall
    // segments so full-height walls don't read as loose pickets).
    builder.add_box([cx, cy + hy - 0.05, cz], rail_half, STICKS_DARK);
    builder.add_box([cx, cy - hy + 0.05, cz], rail_half, STICKS_DARK);
    if hy > 0.9 {
        builder.add_box([cx, cy, cz], rail_half, scale_rgb(STICKS_DARK, 1.15));
    }

    // Vertical sticks, deliberately sparse: ~45% of the area stays open.
    let spacing = 0.30;
    let count = ((2.0 * hx - 0.12) / spacing).floor().max(1.0) as usize;
    let start_x = cx - (count as f32 - 1.0) * spacing / 2.0;
    for index in 0..count {
        let x = start_x + index as f32 * spacing;
        // Small alternating lean so the bundle looks hand-lashed, not
        // machined. Pitch tips the +X end of the (vertical) box.
        let lean = ((index % 5) as f32 - 2.0) * 0.018;
        builder.add_box_oriented(
            [x, cy, cz],
            [0.034, hy - 0.015, 0.034],
            0.0,
            lean,
            scale_rgb(STICKS_BASE, shade(index, 0.45)),
        );
    }

    // Diagonal brace across segments wide enough to need one.
    if hx > 0.55 && hy > 0.55 {
        let length = (hx * hx + hy * hy).sqrt() - 0.08;
        let pitch = (hy / hx).atan();
        builder.add_box_oriented(
            [cx, cy, cz],
            [length, 0.038, 0.038],
            0.0,
            pitch,
            scale_rgb(STICKS_DARK, 1.25),
        );
    }
}

/// Fill one solid wall segment with stone-block coursing: a mortar-dark
/// backing box at the collider size, then staggered rows of slightly
/// proud blocks with per-block shade variation.
fn stone_segment(builder: &mut LowPolyMeshBuilder, center: [f32; 3], half: [f32; 3]) {
    let [cx, cy, cz] = center;
    let [hx, hy, hz] = half;
    builder.add_box(center, half, STONE_MORTAR);

    let row_height = 0.44;
    let block_width = 0.56;
    let gap = 0.03;
    let proud = hz + 0.014;
    // Row-end blocks clamp to slightly *inside* the mortar box: a block
    // face flush with the mortar's side face is two overlapping coplanar
    // quads facing the same way, which z-fights (visible tearing on wall
    // ends and window jambs). The recessed sliver reads as a mortar seam.
    let end_inset = 0.012;
    let left_limit = cx - hx + end_inset;
    let right_limit = cx + hx - end_inset;
    let rows = ((2.0 * hy) / row_height).ceil().max(1.0) as usize;
    for row in 0..rows {
        let row_bottom = cy - hy + row as f32 * row_height;
        let row_top = (row_bottom + row_height).min(cy + hy);
        if row_top - row_bottom < 0.09 {
            // A sliver row would need its half-height padded past the
            // mortar top, poking blocks out of the segment; skip it.
            continue;
        }
        let row_cy = (row_bottom + row_top) / 2.0;
        let row_hh = (row_top - row_bottom) / 2.0 - gap / 2.0;
        // Stagger every other course by half a block.
        let offset = if row % 2 == 0 { 0.0 } else { block_width / 2.0 };
        let mut left = cx - hx + offset - block_width;
        let mut column = 0;
        while left < right_limit {
            let right = (left + block_width - gap).min(right_limit);
            let clipped_left = left.max(left_limit);
            if right - clipped_left > 0.06 {
                let block_cx = (clipped_left + right) / 2.0;
                let block_hw = (right - clipped_left) / 2.0;
                builder.add_box(
                    [block_cx, row_cy, cz],
                    [block_hw, row_hh, proud],
                    scale_rgb(STONE_BASE, shade(row * 31 + column, 0.34)),
                );
            }
            left += block_width;
            column += 1;
        }
    }
}

/// Foundation mesh per tier: a lashed log platform on cross beams
/// (sticks), a plank deck (wood), or a mortar slab with flagstones and a
/// coursed skirt (stone).
///
/// Every tier also carries under-structure reaching `skirt_depth` below
/// the piece origin (stilt posts for sticks, a recessed plinth for wood
/// and stone). Foundations can be placed raised
/// (`game_balance::FOUNDATION_RAISE_MAX_M`), and the same mesh serves
/// every height, so the under-structure is simply buried when the slab
/// sits at ground level.
fn foundation_mesh(builder: &mut LowPolyMeshBuilder, tier: BuildingTier) {
    let half = FOUNDATION_SIZE_M / 2.0;
    let height = crate::building::FOUNDATION_HEIGHT_M;
    let skirt_depth = crate::game_balance::FOUNDATION_RAISE_MAX_M + 0.05;
    match tier {
        BuildingTier::Sticks => {
            // Two support beams along Z carrying a deck of round-ish logs
            // laid along X with visible gaps.
            for x in [-half + 0.5, half - 0.5] {
                builder.add_box([x, 0.14, 0.0], [0.12, 0.14, half - 0.05], STICKS_DARK);
            }
            // Stilt legs under each beam so a raised platform stands on
            // posts instead of floating.
            for x in [-half + 0.5, half - 0.5] {
                for z in [-(half - 0.45), 0.0, half - 0.45] {
                    builder.add_box(
                        [x, -skirt_depth / 2.0 + 0.05, z],
                        [0.07, skirt_depth / 2.0 + 0.05, 0.07],
                        scale_rgb(STICKS_DARK, 0.9),
                    );
                }
            }
            let spacing = 0.25;
            let count = ((2.0 * half) / spacing).floor() as usize;
            let start_z = -(count as f32 - 1.0) * spacing / 2.0;
            for index in 0..count {
                let z = start_z + index as f32 * spacing;
                builder.add_box_oriented(
                    [0.0, height - 0.10, z],
                    [half - 0.02, 0.095, 0.10],
                    0.0,
                    ((index % 3) as f32 - 1.0) * 0.006,
                    scale_rgb(STICKS_BASE, shade(index, 0.4)),
                );
            }
            // Corner lashing posts, running all the way down to the
            // ground (the visible stilts on a raised platform).
            for (x, z) in [
                (-half + 0.1, -half + 0.1),
                (half - 0.1, -half + 0.1),
                (-half + 0.1, half - 0.1),
                (half - 0.1, half - 0.1),
            ] {
                builder.add_box(
                    [x, (height - skirt_depth) / 2.0, z],
                    [0.05, (height + skirt_depth) / 2.0, 0.05],
                    STICKS_DARK,
                );
            }
        }
        BuildingTier::HewnWood => {
            builder.add_box(
                [0.0, height / 2.0, 0.0],
                [half, height / 2.0, half],
                scale_rgb(WOOD_PLANK, 0.92),
            );
            // Recessed plinth from the slab underside to the ground, a
            // shade darker so the raised under-structure reads as shadow.
            builder.add_box(
                [0.0, (0.02 - skirt_depth) / 2.0, 0.0],
                [half - 0.03, (skirt_depth + 0.02) / 2.0, half - 0.03],
                scale_rgb(WOOD_PLANK, 0.55),
            );
            // Deck plank seams along X + perimeter rim.
            for i in 1..6 {
                let z = -half + i as f32 * (2.0 * half / 6.0);
                builder.add_box([0.0, height + 0.012, z], [half, 0.016, 0.018], WOOD_SEAM);
            }
            for (cx, cz, hx, hz) in [
                (0.0, half - 0.06, half, 0.06),
                (0.0, -(half - 0.06), half, 0.06),
                (half - 0.06, 0.0, 0.06, half),
                (-(half - 0.06), 0.0, 0.06, half),
            ] {
                builder.add_box([cx, height + 0.012, cz], [hx, 0.018, hz], WOOD_SEAM);
            }
        }
        BuildingTier::Stone => {
            builder.add_box(
                [0.0, height / 2.0, 0.0],
                [half, height / 2.0, half],
                STONE_MORTAR,
            );
            // Recessed mortar plinth down to the ground for raised slabs.
            builder.add_box(
                [0.0, (0.02 - skirt_depth) / 2.0, 0.0],
                [half - 0.025, (skirt_depth + 0.02) / 2.0, half - 0.025],
                scale_rgb(STONE_MORTAR, 0.8),
            );
            // Flagstone top: a slightly proud grid of varied slabs.
            let cells = 4;
            let cell = 2.0 * half / cells as f32;
            for ix in 0..cells {
                for iz in 0..cells {
                    let x = -half + cell * (ix as f32 + 0.5);
                    let z = -half + cell * (iz as f32 + 0.5);
                    builder.add_box(
                        [x, height + 0.010, z],
                        [cell / 2.0 - 0.025, 0.014, cell / 2.0 - 0.025],
                        scale_rgb(STONE_BASE, shade(ix * 7 + iz, 0.3)),
                    );
                }
            }
            // Block skirt around the visible sides: one course of proud,
            // shade-varied blocks per face.
            let block = 0.70;
            let count = ((2.0 * half) / block).floor() as usize;
            let start = -(count as f32 - 1.0) * block / 2.0;
            for index in 0..count {
                let along = start + index as f32 * block;
                let tint = scale_rgb(STONE_BASE, shade(index + 3, 0.3));
                let half_block = block / 2.0 - 0.02;
                // ±Z faces (blocks span X), then ±X faces (blocks span Z).
                builder.add_box(
                    [along, height / 2.0, half + 0.010],
                    [half_block, height / 2.0 - 0.03, 0.012],
                    tint,
                );
                builder.add_box(
                    [along, height / 2.0, -half - 0.010],
                    [half_block, height / 2.0 - 0.03, 0.012],
                    tint,
                );
                builder.add_box(
                    [half + 0.010, height / 2.0, along],
                    [0.012, height / 2.0 - 0.03, half_block],
                    tint,
                );
                builder.add_box(
                    [-half - 0.010, height / 2.0, along],
                    [0.012, height / 2.0 - 0.03, half_block],
                    tint,
                );
            }
        }
    }
}

/// Ceiling mesh per tier: a lashed pole deck on carrier beams (sticks),
/// a plank slab with seams (wood), or a mortar slab with flagstones
/// (stone). Base sits on the wall tops; the upper face is the next
/// storey's floor.
fn ceiling_mesh(builder: &mut LowPolyMeshBuilder, tier: BuildingTier) {
    let half = FOUNDATION_SIZE_M / 2.0;
    let thickness = CEILING_THICKNESS_M;
    match tier {
        BuildingTier::Sticks => {
            // Carrier beams under the deck edges.
            for x in [-half + 0.45, half - 0.45] {
                builder.add_box([x, 0.055, 0.0], [0.09, 0.055, half - 0.04], STICKS_DARK);
            }
            // Pole deck along X with visible gaps, like the sticks
            // foundation but thinner.
            let spacing = 0.25;
            let count = ((2.0 * half) / spacing).floor() as usize;
            let start_z = -(count as f32 - 1.0) * spacing / 2.0;
            for index in 0..count {
                let z = start_z + index as f32 * spacing;
                builder.add_box_oriented(
                    [0.0, thickness - 0.075, z],
                    [half - 0.02, 0.072, 0.085],
                    0.0,
                    ((index % 3) as f32 - 1.0) * 0.006,
                    scale_rgb(STICKS_BASE, shade(index, 0.4)),
                );
            }
        }
        BuildingTier::HewnWood => {
            builder.add_box(
                [0.0, thickness / 2.0, 0.0],
                [half, thickness / 2.0, half],
                scale_rgb(WOOD_PLANK, 0.92),
            );
            // Plank seams on the walkable top.
            for i in 1..6 {
                let z = -half + i as f32 * (2.0 * half / 6.0);
                builder.add_box([0.0, thickness + 0.010, z], [half, 0.014, 0.018], WOOD_SEAM);
            }
        }
        BuildingTier::Stone => {
            builder.add_box(
                [0.0, thickness / 2.0, 0.0],
                [half, thickness / 2.0, half],
                STONE_MORTAR,
            );
            // Flagstone top, matching the stone foundation's surface so
            // stacked storeys read as one material.
            let cells = 4;
            let cell = 2.0 * half / cells as f32;
            for ix in 0..cells {
                for iz in 0..cells {
                    let x = -half + cell * (ix as f32 + 0.5);
                    let z = -half + cell * (iz as f32 + 0.5);
                    builder.add_box(
                        [x, thickness + 0.008, z],
                        [cell / 2.0 - 0.025, 0.012, cell / 2.0 - 0.025],
                        scale_rgb(STONE_BASE, shade(ix * 7 + iz, 0.3)),
                    );
                }
            }
        }
    }
}

/// Stairs mesh per tier, built over the same step boxes the collider
/// uses so every visible tread is exactly the surface the controller
/// climbs.
fn stairs_mesh(builder: &mut LowPolyMeshBuilder, tier: BuildingTier) {
    let steps = piece_local_boxes(BuildingPiece::Stairs);
    match tier {
        BuildingTier::Sticks => {
            // Open flight: each tread is a pair of lashed poles resting
            // at the step's top, carried by diagonal side stringers. The
            // collider stays the solid steps; the daylight between poles
            // is the sticks-tier look.
            for (index, (center, half)) in steps.iter().enumerate() {
                let top = center.1 + half.1;
                for (slot, offset) in [(-0.085_f32), 0.085].into_iter().enumerate() {
                    builder.add_box(
                        [0.0, top - 0.052, center.2 + offset],
                        [half.0 - 0.04, 0.052, 0.055],
                        scale_rgb(STICKS_BASE, shade(index * 2 + slot, 0.45)),
                    );
                }
            }
            let run = FOUNDATION_SIZE_M;
            let length = (run * run + STAIR_RISE_M * STAIR_RISE_M).sqrt() / 2.0 - 0.06;
            let pitch = (STAIR_RISE_M / run).atan();
            for x in [
                -(FOUNDATION_SIZE_M / 2.0 - 0.07),
                FOUNDATION_SIZE_M / 2.0 - 0.07,
            ] {
                builder.add_box_oriented(
                    [x, STAIR_RISE_M / 2.0 - 0.11, 0.0],
                    [length, 0.058, 0.058],
                    -std::f32::consts::FRAC_PI_2,
                    pitch,
                    scale_rgb(STICKS_DARK, 1.15),
                );
            }
        }
        BuildingTier::HewnWood => {
            for (index, (center, half)) in steps.iter().enumerate() {
                builder.add_box(
                    [center.0, center.1, center.2],
                    [half.0, half.1, half.2],
                    scale_rgb(WOOD_PLANK, 0.92 * shade(index, 0.10)),
                );
                // Proud tread plank with a slight nose overhang.
                let top = center.1 + half.1;
                builder.add_box(
                    [0.0, top + 0.008, center.2],
                    [half.0 - 0.02, 0.012, half.2 + 0.012],
                    scale_rgb(WOOD_PLANK, 1.12),
                );
            }
        }
        BuildingTier::Stone => {
            for (index, (center, half)) in steps.iter().enumerate() {
                builder.add_box(
                    [center.0, center.1, center.2],
                    [half.0, half.1, half.2],
                    STONE_MORTAR,
                );
                // Proud stone tread slab, shade-varied per step.
                let top = center.1 + half.1;
                builder.add_box(
                    [0.0, top + 0.008, center.2],
                    [half.0 - 0.025, 0.012, half.2 - 0.012],
                    scale_rgb(STONE_BASE, shade(index, 0.3)),
                );
            }
        }
    }
}

/// A thin horizontal accent strip across the wall at height `y`, split
/// into segments that avoid the opening when one exists at that height.
fn add_horizontal_strip(
    builder: &mut LowPolyMeshBuilder,
    piece: BuildingPiece,
    y: f32,
    proud: f32,
    accent: MeshColor,
) {
    let half_w = FOUNDATION_SIZE_M / 2.0;
    let opening = match piece {
        BuildingPiece::Wall
        | BuildingPiece::Foundation
        | BuildingPiece::Ceiling
        | BuildingPiece::Stairs => None,
        BuildingPiece::WindowWall => {
            let bottom = crate::building::WINDOW_SILL_HEIGHT_M;
            let top = bottom + crate::building::WINDOW_OPENING_HEIGHT_M;
            (y > bottom && y < top).then_some(crate::building::WINDOW_OPENING_WIDTH_M / 2.0)
        }
        BuildingPiece::Doorway => {
            (y < crate::building::DOORWAY_OPENING_HEIGHT_M).then_some(DOORWAY_OPENING_WIDTH_M / 2.0)
        }
    };
    // Strip ends pull slightly inside the wall ends and opening jambs:
    // a strip end cap flush with the wall's end face is two coplanar
    // quads facing the same way, which z-fights (the tearing previously
    // visible on wood wall ends), same trick as the stone tier's
    // `end_inset`.
    let end_inset = 0.012;
    match opening {
        None => builder.add_box([0.0, y, 0.0], [half_w - end_inset, 0.022, proud], accent),
        Some(opening_half) => {
            let seg_hw = (half_w - end_inset - opening_half - end_inset) / 2.0;
            let seg_cx = opening_half + end_inset + seg_hw;
            builder.add_box([-seg_cx, y, 0.0], [seg_hw, 0.022, proud], accent);
            builder.add_box([seg_cx, y, 0.0], [seg_hw, 0.022, proud], accent);
        }
    }
}

/// Door panel with its hinge edge at the local origin, spanning +X.
/// Spawned as a child of the (invisible) door root at the hinge offset,
/// so opening the door is a pure yaw rotation of this mesh.
pub(crate) fn door_panel_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    let hw = DOOR_PANEL_WIDTH_M / 2.0;
    let hh = DOOR_PANEL_HEIGHT_M / 2.0;
    let ht = DOOR_PANEL_THICKNESS_M / 2.0;
    // Panel body, centred at +hw so the hinge edge sits on the origin.
    builder.add_box([hw, hh, 0.0], [hw, hh, ht], DOOR_LOG);
    // Vertical log seams.
    for i in 1..4 {
        let x = i as f32 * (DOOR_PANEL_WIDTH_M / 4.0);
        builder.add_box([x, hh, 0.0], [0.016, hh, ht + 0.008], DOOR_BRACE);
    }
    // Cross braces front and back.
    for i in 0..2 {
        let y = 0.45 + i as f32 * 1.2;
        builder.add_box([hw, y, 0.0], [hw - 0.06, 0.07, ht + 0.014], DOOR_BRACE);
    }
    // Handle nub on the swing-edge side.
    builder.add_box(
        [DOOR_PANEL_WIDTH_M - 0.12, 1.05, 0.0],
        [0.045, 0.045, ht + 0.05],
        IRON_BAND,
    );
    builder.build()
}

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

/// Sleeping bag: a low fabric roll with a folded-back lining and a small
/// pillow bump. Base centred on the origin, spanning local X (head at +X).
pub(crate) fn sleeping_bag_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Main roll, slightly tapered toward the foot via two overlapping
    // boxes. The foot box's base floats a hair above the main roll's so
    // their downward faces aren't coplanar.
    builder.add_box([0.10, 0.10, 0.0], [0.85, 0.10, 0.40], BAG_FABRIC);
    builder.add_box(
        [-0.65, 0.089, 0.0],
        [0.32, 0.085, 0.34],
        scale_rgb(BAG_FABRIC, 0.85),
    );
    // Folded-back lining near the head end, proud of the roll's top
    // face. Its old top sat exactly on the roll top (two coplanar
    // same-facing quads), which z-fought as flicker around the pillow.
    builder.add_box([0.62, 0.19, 0.0], [0.30, 0.035, 0.36], BAG_LINING);
    // Pillow bump, seated into the lining and rising clear of it.
    builder.add_box(
        [0.78, 0.225, 0.0],
        [0.18, 0.05, 0.22],
        scale_rgb(BAG_LINING, 1.1),
    );
    builder.build()
}

/// Held construction hammer, built in the shared held-item reference
/// frame the authored tools use (pommel at y ≈ -0.514, head at the top,
/// vertical haft along Y; see docs/icon-to-model.md). The swing pose and
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
