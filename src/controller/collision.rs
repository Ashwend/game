use crate::{
    protocol::Vec3Net,
    world::{TEST_WORLD_BLOCKS, WorldBlock},
};

use super::{GROUND_EPSILON, PLAYER_HEIGHT, PLAYER_RADIUS};

#[derive(Debug, Clone, Copy)]
pub(super) enum Axis {
    X,
    Y,
    Z,
}

pub(super) fn move_with_collisions(
    position: &mut Vec3Net,
    velocity: &mut Vec3Net,
    axis: Axis,
    delta: f32,
) -> bool {
    if delta == 0.0 {
        return false;
    }

    match axis {
        Axis::X => position.x += delta,
        Axis::Y => position.y += delta,
        Axis::Z => position.z += delta,
    }

    let mut landed = false;
    if matches!(axis, Axis::Y) && position.y < 0.0 {
        position.y = 0.0;
        velocity.y = 0.0;
        landed = delta < 0.0;
    }

    for block in TEST_WORLD_BLOCKS {
        if !player_overlaps_block(*position, block) {
            continue;
        }

        let min = block.min();
        let max = block.max();
        match axis {
            Axis::X if delta > 0.0 => {
                position.x = min.x - PLAYER_RADIUS;
                velocity.x = 0.0;
            }
            Axis::X => {
                position.x = max.x + PLAYER_RADIUS;
                velocity.x = 0.0;
            }
            Axis::Y if delta > 0.0 => {
                position.y = min.y - PLAYER_HEIGHT;
                velocity.y = 0.0;
            }
            Axis::Y => {
                position.y = max.y;
                velocity.y = 0.0;
                landed = true;
            }
            Axis::Z if delta > 0.0 => {
                position.z = min.z - PLAYER_RADIUS;
                velocity.z = 0.0;
            }
            Axis::Z => {
                position.z = max.z + PLAYER_RADIUS;
                velocity.z = 0.0;
            }
        }
    }

    landed
}

fn player_overlaps_block(position: Vec3Net, block: WorldBlock) -> bool {
    let min = block.min();
    let max = block.max();
    position.x + PLAYER_RADIUS > min.x
        && position.x - PLAYER_RADIUS < max.x
        && position.y + PLAYER_HEIGHT > min.y
        && position.y < max.y
        && position.z + PLAYER_RADIUS > min.z
        && position.z - PLAYER_RADIUS < max.z
}

pub(super) fn is_supported(position: Vec3Net) -> bool {
    if position.y <= GROUND_EPSILON {
        return true;
    }

    TEST_WORLD_BLOCKS.iter().any(|block| {
        let min = block.min();
        let max = block.max();
        (position.y - max.y).abs() <= GROUND_EPSILON
            && position.x + PLAYER_RADIUS > min.x
            && position.x - PLAYER_RADIUS < max.x
            && position.z + PLAYER_RADIUS > min.z
            && position.z - PLAYER_RADIUS < max.z
    })
}
