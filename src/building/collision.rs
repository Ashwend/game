//! Multi-box collider generation: the per-piece local solid boxes and their
//! rotation into world-space `WorldBlock`s, plus the door panel collider
//! that follows the hinge. This is the single source of truth for building
//! collision: the server's spawn-safety grid, the placement overlap test,
//! and the client movement grid all build from these boxes.

use crate::{protocol::Vec3Net, world::WorldBlock};

use super::{
    BuildingPiece, CEILING_THICKNESS_M, DOOR_OPEN_ANGLE_RAD, DOOR_PANEL_HEIGHT_M,
    DOOR_PANEL_THICKNESS_M, DOOR_PANEL_WIDTH_M, DOORWAY_OPENING_HEIGHT_M, DOORWAY_OPENING_WIDTH_M,
    FOUNDATION_HEIGHT_M, FOUNDATION_SIZE_M, STAIR_RISE_M, STAIR_STEP_COUNT, WALL_HEIGHT_M,
    WALL_THICKNESS_M, WINDOW_OPENING_HEIGHT_M, WINDOW_OPENING_WIDTH_M, WINDOW_SILL_HEIGHT_M,
    rotate_offset,
};

/// Axis-aligned solid boxes for a piece at `position`/`yaw`, in world
/// space. This is the single source of truth for building collision: the
/// server's spawn-safety grid, the placement overlap test, and the client
/// movement grid all build from these. Pieces with openings (window,
/// doorway) return one box per solid segment so the hole is genuinely
/// passable.
pub fn building_collider_blocks(
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
) -> Vec<WorldBlock> {
    piece_local_boxes(piece)
        .iter()
        .map(|(center, half)| {
            let (cx, cz) = rotate_offset(yaw, center.0, center.2);
            let (hx, hz) = rotate_half_extents(yaw, half.0, half.2);
            let mut bottom = position.y + center.1 - half.1;
            let top = position.y + center.1 + half.1;
            // A raised foundation's collider reaches down to the ground:
            // the skirt under the slab is solid, so nothing can be placed
            // (or walked) underneath it and an XZ-overlapping foundation
            // at a different height still collides.
            if matches!(piece, BuildingPiece::Foundation) {
                bottom = bottom.min(0.0);
            }
            WorldBlock::new(
                Vec3Net::new(position.x + cx, (bottom + top) / 2.0, position.z + cz),
                Vec3Net::new(hx, (top - bottom) / 2.0, hz),
            )
        })
        .collect()
}

/// Collider for a door panel seated in a doorway at `position`/`yaw`.
/// The box follows the hinge state so collision, interact targeting, and
/// hit detection all happen where the panel visibly is: closed, it fills
/// the opening plane; open, it is the AABB around the panel swung
/// [`DOOR_OPEN_ANGLE_RAD`] about the hinge (the local -X edge of the
/// opening, the same pivot and angle the client's panel mesh animates
/// with). The swung panel isn't axis-aligned, so its AABB is slightly
/// larger than the panel itself, close enough for the AABB-only
/// collision pipeline while leaving the opening genuinely passable.
pub fn door_collider_blocks(position: Vec3Net, yaw: f32, open: bool) -> Vec<WorldBlock> {
    let half_h = DOOR_PANEL_HEIGHT_M / 2.0;
    let ((cx_local, cz_local), (hx_local, hz_local)) = if open {
        open_door_panel_local_box()
    } else {
        (
            (0.0, 0.0),
            (DOORWAY_OPENING_WIDTH_M / 2.0, DOOR_PANEL_THICKNESS_M / 2.0),
        )
    };
    let (cx, cz) = rotate_offset(yaw, cx_local, cz_local);
    let (hx, hz) = rotate_half_extents(yaw, hx_local, hz_local);
    vec![WorldBlock::new(
        Vec3Net::new(position.x + cx, position.y + half_h, position.z + cz),
        Vec3Net::new(hx, half_h, hz),
    )]
}

/// XZ centre and half-extents (door-local space) of the AABB around the
/// open panel: the `DOOR_PANEL_WIDTH_M` x `DOOR_PANEL_THICKNESS_M`
/// rectangle rotated `-DOOR_OPEN_ANGLE_RAD` about the hinge at local
/// `(-DOOR_PANEL_WIDTH_M / 2, 0)`, matching `animate_door_panels_system`.
fn open_door_panel_local_box() -> ((f32, f32), (f32, f32)) {
    let hinge_x = -DOOR_PANEL_WIDTH_M / 2.0;
    let angle = -DOOR_OPEN_ANGLE_RAD;
    // Local +X (the panel span) and +Z (the panel thickness) after the
    // hinge rotation about Y.
    let span = (angle.cos(), -angle.sin());
    let thickness = (angle.sin(), angle.cos());
    let half_t = DOOR_PANEL_THICKNESS_M / 2.0;
    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for reach in [0.0, DOOR_PANEL_WIDTH_M] {
        for side in [-half_t, half_t] {
            let x = hinge_x + span.0 * reach + thickness.0 * side;
            let z = span.1 * reach + thickness.1 * side;
            min = (min.0.min(x), min.1.min(z));
            max = (max.0.max(x), max.1.max(z));
        }
    }
    (
        ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0),
        ((max.0 - min.0) / 2.0, (max.1 - min.1) / 2.0),
    )
}

/// Half-extents swap between X and Z on quarter turns.
fn rotate_half_extents(yaw: f32, hx: f32, hz: f32) -> (f32, f32) {
    let steps = ((yaw / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4);
    if steps % 2 == 0 { (hx, hz) } else { (hz, hx) }
}

/// One solid segment of a piece in local space: `(center (x, y, z),
/// half-extents (x, y, z))` with the piece's base at the local origin and
/// width along local X.
pub type LocalBox = ((f32, f32, f32), (f32, f32, f32));

/// Local-space solid boxes per piece. Shared by the collider builder
/// above and the client's composite-mesh builder so the ghost/visual
/// exactly matches what blocks movement.
pub fn piece_local_boxes(piece: BuildingPiece) -> &'static [LocalBox] {
    const F_HALF: f32 = FOUNDATION_SIZE_M / 2.0;
    const F_HH: f32 = FOUNDATION_HEIGHT_M / 2.0;
    const W_HALF: f32 = FOUNDATION_SIZE_M / 2.0;
    const W_HH: f32 = WALL_HEIGHT_M / 2.0;
    const T_H: f32 = WALL_THICKNESS_M / 2.0;

    const FOUNDATION: &[LocalBox] = &[((0.0, F_HH, 0.0), (F_HALF, F_HH, F_HALF))];

    const WALL: &[LocalBox] = &[((0.0, W_HH, 0.0), (W_HALF, W_HH, T_H))];

    // Window wall: left jamb, right jamb, sill band, header band.
    const WIN_HW: f32 = WINDOW_OPENING_WIDTH_M / 2.0;
    const WIN_JAMB_HW: f32 = (W_HALF - WIN_HW) / 2.0;
    const WIN_JAMB_CX: f32 = WIN_HW + WIN_JAMB_HW;
    const WIN_SILL_HH: f32 = WINDOW_SILL_HEIGHT_M / 2.0;
    const WIN_TOP: f32 = WINDOW_SILL_HEIGHT_M + WINDOW_OPENING_HEIGHT_M;
    const WIN_HEADER_HH: f32 = (WALL_HEIGHT_M - WIN_TOP) / 2.0;
    const WINDOW_WALL: &[LocalBox] = &[
        ((-WIN_JAMB_CX, W_HH, 0.0), (WIN_JAMB_HW, W_HH, T_H)),
        ((WIN_JAMB_CX, W_HH, 0.0), (WIN_JAMB_HW, W_HH, T_H)),
        ((0.0, WIN_SILL_HH, 0.0), (WIN_HW, WIN_SILL_HH, T_H)),
        (
            (0.0, WIN_TOP + WIN_HEADER_HH, 0.0),
            (WIN_HW, WIN_HEADER_HH, T_H),
        ),
    ];

    // Doorway: left jamb, right jamb, header beam over the opening.
    const DOOR_HW: f32 = DOORWAY_OPENING_WIDTH_M / 2.0;
    const DOOR_JAMB_HW: f32 = (W_HALF - DOOR_HW) / 2.0;
    const DOOR_JAMB_CX: f32 = DOOR_HW + DOOR_JAMB_HW;
    const DOOR_HEADER_HH: f32 = (WALL_HEIGHT_M - DOORWAY_OPENING_HEIGHT_M) / 2.0;
    const DOORWAY: &[LocalBox] = &[
        ((-DOOR_JAMB_CX, W_HH, 0.0), (DOOR_JAMB_HW, W_HH, T_H)),
        ((DOOR_JAMB_CX, W_HH, 0.0), (DOOR_JAMB_HW, W_HH, T_H)),
        (
            (0.0, DOORWAY_OPENING_HEIGHT_M + DOOR_HEADER_HH, 0.0),
            (DOOR_HW, DOOR_HEADER_HH, T_H),
        ),
    ];

    // Ceiling: one slab spanning the cell, base on the wall tops.
    const C_HH: f32 = CEILING_THICKNESS_M / 2.0;
    const CEILING: &[LocalBox] = &[((0.0, C_HH, 0.0), (F_HALF, C_HH, F_HALF))];

    const STAIRS: &[LocalBox] = &stair_step_boxes();

    match piece {
        BuildingPiece::Foundation => FOUNDATION,
        BuildingPiece::Wall => WALL,
        BuildingPiece::WindowWall => WINDOW_WALL,
        BuildingPiece::Doorway => DOORWAY,
        BuildingPiece::Ceiling => CEILING,
        BuildingPiece::Stairs => STAIRS,
    }
}

/// The stairs flight as solid filled steps, ascending along local +Z
/// from the cell's -Z edge to the +Z edge. Each box runs from the base
/// to that step's tread height, so the underside is solid (no crawling
/// through a staircase) and every tread is a real walkable AABB.
const fn stair_step_boxes() -> [LocalBox; STAIR_STEP_COUNT] {
    const RISE: f32 = STAIR_RISE_M / STAIR_STEP_COUNT as f32;
    const DEPTH: f32 = FOUNDATION_SIZE_M / STAIR_STEP_COUNT as f32;
    let mut boxes = [((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)); STAIR_STEP_COUNT];
    let mut index = 0;
    while index < STAIR_STEP_COUNT {
        let tread_top = RISE * (index as f32 + 1.0);
        let center_z = -FOUNDATION_SIZE_M / 2.0 + DEPTH * (index as f32 + 0.5);
        boxes[index] = (
            (0.0, tread_top / 2.0, center_z),
            (FOUNDATION_SIZE_M / 2.0, tread_top / 2.0, DEPTH / 2.0),
        );
        index += 1;
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doorway_boxes_leave_the_opening_clear() {
        let blocks = building_collider_blocks(BuildingPiece::Doorway, Vec3Net::ZERO, 0.0);
        assert_eq!(blocks.len(), 3);
        // Nothing solid in the middle of the opening at chest height.
        for block in &blocks {
            let min = block.min();
            let max = block.max();
            let inside_x = min.x < 0.0 && max.x > 0.0;
            let inside_y = min.y < 1.0 && max.y > 1.0;
            assert!(!(inside_x && inside_y), "opening obstructed by {block:?}");
        }
    }

    #[test]
    fn window_wall_boxes_leave_the_window_clear() {
        let blocks = building_collider_blocks(BuildingPiece::WindowWall, Vec3Net::ZERO, 0.0);
        assert_eq!(blocks.len(), 4);
        let probe_y = WINDOW_SILL_HEIGHT_M + WINDOW_OPENING_HEIGHT_M / 2.0;
        for block in &blocks {
            let min = block.min();
            let max = block.max();
            let inside_x = min.x < 0.0 && max.x > 0.0;
            let inside_y = min.y < probe_y && max.y > probe_y;
            assert!(!(inside_x && inside_y), "window obstructed by {block:?}");
        }
    }

    #[test]
    fn quarter_turned_walls_swap_their_half_extents() {
        let blocks = building_collider_blocks(
            BuildingPiece::Wall,
            Vec3Net::ZERO,
            std::f32::consts::FRAC_PI_2,
        );
        let block = blocks[0];
        let half = block.half_extents;
        assert!((half.x - WALL_THICKNESS_M / 2.0).abs() < 1e-6);
        assert!((half.z - FOUNDATION_SIZE_M / 2.0).abs() < 1e-6);
    }

    #[test]
    fn stair_steps_stay_under_the_controller_auto_step() {
        let boxes = piece_local_boxes(BuildingPiece::Stairs);
        assert_eq!(boxes.len(), STAIR_STEP_COUNT);
        let mut previous_top = 0.0;
        for (center, half) in boxes {
            let top = center.1 + half.1;
            let rise = top - previous_top;
            // Controller STEP_HEIGHT is 0.45; every tread must be
            // climbable without jumping.
            assert!(rise > 0.0 && rise <= 0.45, "unwalkable step rise {rise}");
            previous_top = top;
        }
        assert!((previous_top - STAIR_RISE_M).abs() < 1e-4);
    }

    #[test]
    fn open_door_collider_follows_the_swung_panel() {
        let closed = door_collider_blocks(Vec3Net::ZERO, 0.0, false);
        assert_eq!(closed.len(), 1);
        assert!(
            closed[0].center.z.abs() < 1e-6,
            "closed panel sits in the opening plane"
        );

        let open = door_collider_blocks(Vec3Net::ZERO, 0.0, true);
        assert_eq!(open.len(), 1);
        let panel = open[0];
        // The panel swings toward local +Z about the hinge on the -X
        // side: its box must clear the opening and sit on the swing side.
        assert!(
            panel.max().x < -0.4,
            "open panel must leave the opening passable, max x {}",
            panel.max().x
        );
        assert!(panel.center.z > 0.3, "open panel sits on the swing side");
        assert!((panel.max().y - DOOR_PANEL_HEIGHT_M).abs() < 1e-5);
        assert!(panel.min().y.abs() < 1e-5);

        // A quarter-turned door swings its panel along the rotated axes:
        // rotate_offset maps local (x, z) to (z, -x) at 90 degrees.
        let turned = door_collider_blocks(Vec3Net::ZERO, std::f32::consts::FRAC_PI_2, true);
        assert!(turned[0].center.x > 0.3);
        assert!(turned[0].center.z > 0.3);
    }
}
