use crate::{protocol::Vec3Net, world::WorldBlock};

use super::{GROUND_EPSILON, PLAYER_HEIGHT, PLAYER_RADIUS, grid::BlockGrid};

const COLLISION_SKIN: f32 = 0.001;

#[derive(Debug, Clone, Copy)]
pub(super) enum Axis {
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct MoveResult {
    pub(super) collided: bool,
    pub(super) landed: bool,
}

pub(super) fn move_with_collisions(
    position: &mut Vec3Net,
    velocity: &mut Vec3Net,
    grid: &BlockGrid,
    axis: Axis,
    delta: f32,
) -> MoveResult {
    if delta == 0.0 {
        return MoveResult::default();
    }

    let mut attempted = *position;
    match axis {
        Axis::X => attempted.x += delta,
        Axis::Y => attempted.y += delta,
        Axis::Z => attempted.z += delta,
    }

    let mut result = MoveResult::default();
    let mut resolved_axis_position = None;
    if matches!(axis, Axis::Y) {
        // The analytic floor: flat world plane, raised over a live MeteorShower
        // crater so its mound is solid ground rather than a visual shell.
        let floor = grid.floor_height(attempted.x, attempted.z);
        if attempted.y < floor {
            result.collided = true;
            result.landed = delta < 0.0;
            resolved_axis_position = Some(floor);
        }
    }

    // Inline the loop into each match arm so the candidate iterator stays a
    // concrete type, no `Box<dyn Iterator>` allocation per substep.
    let mut visit = |index: usize| {
        let block = grid.block(index);
        if let Some(candidate) = swept_axis_collision(*position, attempted, block, axis, delta) {
            result.collided = true;
            result.landed |= matches!(axis, Axis::Y) && delta < 0.0;
            resolved_axis_position = Some(nearest_axis_resolution(
                resolved_axis_position,
                candidate,
                delta,
            ));
        }
    };
    match axis {
        Axis::X => grid
            .candidates_for_swept(*position, delta, 0.0)
            .for_each(&mut visit),
        Axis::Y => grid.candidates_for_vertical(*position).for_each(&mut visit),
        Axis::Z => grid
            .candidates_for_swept(*position, 0.0, delta)
            .for_each(&mut visit),
    }

    *position = attempted;
    if let Some(axis_position) = resolved_axis_position {
        match axis {
            Axis::X => {
                position.x = axis_position;
                velocity.x = 0.0;
            }
            Axis::Y => {
                position.y = axis_position;
                velocity.y = 0.0;
            }
            Axis::Z => {
                position.z = axis_position;
                velocity.z = 0.0;
            }
        }
    }

    result
}

fn nearest_axis_resolution(current: Option<f32>, candidate: f32, delta: f32) -> f32 {
    let Some(current) = current else {
        return candidate;
    };

    if delta > 0.0 {
        current.min(candidate)
    } else {
        current.max(candidate)
    }
}

fn swept_axis_collision(
    start: Vec3Net,
    attempted: Vec3Net,
    block: WorldBlock,
    axis: Axis,
    delta: f32,
) -> Option<f32> {
    if !player_overlaps_block_on_other_axes(start, block, axis) {
        return None;
    }

    let min = block.min();
    let max = block.max();
    let face = match axis {
        Axis::X if delta > 0.0 => min.x - PLAYER_RADIUS,
        Axis::X => max.x + PLAYER_RADIUS,
        Axis::Y if delta > 0.0 => min.y - PLAYER_HEIGHT,
        Axis::Y => max.y,
        Axis::Z if delta > 0.0 => min.z - PLAYER_RADIUS,
        Axis::Z => max.z + PLAYER_RADIUS,
    };

    let start_coord = axis_coordinate(start, axis);
    let attempted_coord = axis_coordinate(attempted, axis);
    if if delta > 0.0 {
        start_coord <= face && attempted_coord > face
    } else {
        start_coord >= face && attempted_coord < face
    } {
        Some(face)
    } else {
        None
    }
}

fn axis_coordinate(position: Vec3Net, axis: Axis) -> f32 {
    match axis {
        Axis::X => position.x,
        Axis::Y => position.y,
        Axis::Z => position.z,
    }
}

fn player_overlaps_block_on_other_axes(position: Vec3Net, block: WorldBlock, axis: Axis) -> bool {
    match axis {
        Axis::X => {
            player_vertically_overlaps_block(position, block)
                && player_overlaps_z_block(position, block, COLLISION_SKIN)
        }
        Axis::Y => {
            player_overlaps_x_block(position, block, COLLISION_SKIN)
                && player_overlaps_z_block(position, block, COLLISION_SKIN)
        }
        Axis::Z => {
            player_vertically_overlaps_block(position, block)
                && player_overlaps_x_block(position, block, COLLISION_SKIN)
        }
    }
}

pub(super) fn player_overlaps_world(position: Vec3Net, grid: &BlockGrid) -> bool {
    grid.candidates_for_player(position)
        .any(|index| player_overlaps_block(position, grid.block(index)))
}

pub(super) fn support_height_between(
    position: Vec3Net,
    grid: &BlockGrid,
    min_y: f32,
    max_y: f32,
) -> Option<f32> {
    // The analytic floor (flat plane, or the crater mound surface) counts as
    // support exactly like a block top at that height would.
    let floor = grid.floor_height(position.x, position.z);
    let mut support = (min_y <= floor && max_y >= floor).then_some(floor);

    for index in grid.candidates_for_player(position) {
        let block = grid.block(index);
        let top = block.max().y;
        if top < min_y || top > max_y {
            continue;
        }
        if !player_horizontally_overlaps_block(position, block) {
            continue;
        }

        support = Some(support.map_or(top, |current| current.max(top)));
    }

    support
}

fn player_overlaps_block(position: Vec3Net, block: WorldBlock) -> bool {
    player_horizontally_overlaps_block(position, block)
        && player_vertically_overlaps_block(position, block)
}

fn player_horizontally_overlaps_block(position: Vec3Net, block: WorldBlock) -> bool {
    player_overlaps_x_block(position, block, 0.0) && player_overlaps_z_block(position, block, 0.0)
}

fn player_overlaps_x_block(position: Vec3Net, block: WorldBlock, skin: f32) -> bool {
    let min = block.min();
    let max = block.max();
    position.x + PLAYER_RADIUS > min.x + skin && position.x - PLAYER_RADIUS < max.x - skin
}

fn player_overlaps_z_block(position: Vec3Net, block: WorldBlock, skin: f32) -> bool {
    let min = block.min();
    let max = block.max();
    position.z + PLAYER_RADIUS > min.z + skin && position.z - PLAYER_RADIUS < max.z - skin
}

fn player_vertically_overlaps_block(position: Vec3Net, block: WorldBlock) -> bool {
    let min = block.min();
    let max = block.max();
    position.y + PLAYER_HEIGHT > min.y && position.y < max.y
}

pub(super) fn is_supported(position: Vec3Net, grid: &BlockGrid) -> bool {
    support_height_between(
        position,
        grid,
        position.y - GROUND_EPSILON,
        position.y + GROUND_EPSILON,
    )
    .is_some()
}

/// The topmost block whose top surface is right under the player's feet,
/// if any. Used by the footstep system to look up the material of the
/// surface the player is standing on. Returns `None` when the player is
/// standing on the world floor (i.e. no block directly under them) or is
/// airborne, callers can substitute their own fallback in either case.
pub fn block_under_feet(position: Vec3Net, grid: &BlockGrid) -> Option<WorldBlock> {
    let mut best: Option<WorldBlock> = None;
    for index in grid.candidates_for_player(position) {
        let block = grid.block(index);
        let top = block.max().y;
        if top > position.y + GROUND_EPSILON || top < position.y - GROUND_EPSILON {
            continue;
        }
        if !player_horizontally_overlaps_block(position, block) {
            continue;
        }
        best = Some(match best {
            Some(prev) if prev.max().y >= top => prev,
            _ => block,
        });
    }
    best
}
