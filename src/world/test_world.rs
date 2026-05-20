//! Static layout of the `Test` map: a perimeter-walled 80m floor, a cluster
//! of player-controller test obstacles, several ore veins, and four corner
//! forest groves plus some lone landmarks. Kept here (rather than inline in
//! `WorldData`) so the long data list doesn't bloat the world module and so
//! it stays easy to re-order without rewriting unrelated code.

use crate::{
    protocol::Vec3Net,
    resources::{
        BIRCH_TREE_LARGE_NODE_ID, BIRCH_TREE_NODE_ID, BIRCH_TREE_SMALL_NODE_ID, COAL_NODE_ID,
        IRON_NODE_ID, PINE_TREE_LARGE_NODE_ID, PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID,
        SULFUR_NODE_ID,
    },
};

use super::{DEFAULT_FLOOR_SIZE, WorldBlock, WorldData, WorldResourceNodeSpawn};

impl WorldData {
    pub fn test_world() -> Self {
        Self {
            floor_size: DEFAULT_FLOOR_SIZE,
            blocks: test_world_blocks(),
            resource_nodes: test_world_resource_nodes(),
        }
    }
}

fn test_world_blocks() -> Vec<WorldBlock> {
    vec![
        // Perimeter stone walls — keep the playable area bounded so
        // the player can't wander off the edge of the test floor.
        // Walls are 4m tall (well above eye level) and sit just inside
        // the 80m floor edge.
        WorldBlock::stone(Vec3Net::new(0.0, 2.0, 39.0), Vec3Net::new(39.0, 2.0, 0.5)),
        WorldBlock::stone(Vec3Net::new(0.0, 2.0, -39.0), Vec3Net::new(39.0, 2.0, 0.5)),
        WorldBlock::stone(Vec3Net::new(39.0, 2.0, 0.0), Vec3Net::new(0.5, 2.0, 39.0)),
        WorldBlock::stone(Vec3Net::new(-39.0, 2.0, 0.0), Vec3Net::new(0.5, 2.0, 39.0)),
        // Player controller test shapes.
        WorldBlock::new(Vec3Net::new(-4.0, 0.5, -4.0), Vec3Net::new(1.3, 0.5, 1.3)),
        WorldBlock::new(Vec3Net::new(3.6, 0.5, -2.4), Vec3Net::new(1.0, 0.5, 1.0)),
        WorldBlock::new(Vec3Net::new(0.0, 0.25, -6.0), Vec3Net::new(2.0, 0.25, 0.8)),
        WorldBlock::new(Vec3Net::new(5.2, 1.0, 4.2), Vec3Net::new(1.1, 1.0, 1.1)),
        WorldBlock::new(Vec3Net::new(-6.0, 0.75, 3.2), Vec3Net::new(1.5, 0.75, 1.3)),
        WorldBlock::new(Vec3Net::new(-2.3, 0.2, 2.8), Vec3Net::new(0.8, 0.2, 0.8)),
        WorldBlock::new(Vec3Net::new(0.0, 0.45, 3.8), Vec3Net::new(0.8, 0.45, 0.8)),
        WorldBlock::new(Vec3Net::new(2.2, 0.75, 3.8), Vec3Net::new(0.8, 0.75, 0.8)),
        WorldBlock::new(Vec3Net::new(-7.0, 1.4, -1.0), Vec3Net::new(0.75, 1.4, 0.75)),
        WorldBlock::new(Vec3Net::new(7.0, 0.35, -6.0), Vec3Net::new(1.6, 0.35, 1.0)),
        WorldBlock::new(Vec3Net::new(-1.6, 0.18, -2.7), Vec3Net::new(0.7, 0.18, 0.5)),
        WorldBlock::new(Vec3Net::new(0.0, 0.28, -3.4), Vec3Net::new(0.8, 0.28, 0.5)),
        WorldBlock::new(Vec3Net::new(1.7, 0.38, -4.1), Vec3Net::new(0.8, 0.38, 0.5)),
        WorldBlock::new(Vec3Net::new(-8.9, 1.2, -1.2), Vec3Net::new(0.25, 1.2, 5.4)),
        WorldBlock::new(Vec3Net::new(-6.4, 1.2, -1.2), Vec3Net::new(0.25, 1.2, 5.4)),
        WorldBlock::new(
            Vec3Net::new(-7.65, 0.15, -5.4),
            Vec3Net::new(0.8, 0.15, 0.35),
        ),
        WorldBlock::new(
            Vec3Net::new(-7.65, 0.35, -3.3),
            Vec3Net::new(0.65, 0.35, 0.35),
        ),
        WorldBlock::new(
            Vec3Net::new(-7.65, 0.55, -1.2),
            Vec3Net::new(0.55, 0.55, 0.35),
        ),
        WorldBlock::new(Vec3Net::new(4.0, 0.35, -9.0), Vec3Net::new(1.8, 0.35, 1.0)),
        WorldBlock::new(Vec3Net::new(4.0, 0.35, -13.0), Vec3Net::new(1.8, 0.35, 1.0)),
        WorldBlock::new(
            Vec3Net::new(4.0, 1.25, -16.0),
            Vec3Net::new(2.3, 1.25, 0.25),
        ),
        WorldBlock::new(
            Vec3Net::new(0.0, 1.25, -11.2),
            Vec3Net::new(4.6, 1.25, 0.25),
        ),
        WorldBlock::new(Vec3Net::new(7.5, 1.3, 0.0), Vec3Net::new(0.25, 1.3, 5.0)),
        WorldBlock::new(Vec3Net::new(10.5, 1.3, 0.0), Vec3Net::new(0.25, 1.3, 5.0)),
        WorldBlock::new(Vec3Net::new(9.0, 0.3, -3.6), Vec3Net::new(0.9, 0.3, 0.45)),
        WorldBlock::new(Vec3Net::new(9.0, 0.6, -1.2), Vec3Net::new(0.7, 0.6, 0.45)),
        WorldBlock::new(Vec3Net::new(9.0, 0.9, 1.3), Vec3Net::new(0.55, 0.9, 0.45)),
    ]
}

/// Test-world resource-node spawns: ore clusters near the controller test
/// area plus three veins around the map, and tree groves at each corner of
/// the floor showcasing every size variant.
fn test_world_resource_nodes() -> Vec<WorldResourceNodeSpawn> {
    vec![
        // Ore cluster near the player controller test area.
        WorldResourceNodeSpawn::new(1, COAL_NODE_ID, Vec3Net::new(12.5, 0.0, -8.5), 0.2),
        WorldResourceNodeSpawn::new(2, COAL_NODE_ID, Vec3Net::new(14.3, 0.0, -10.1), 1.1),
        WorldResourceNodeSpawn::new(3, IRON_NODE_ID, Vec3Net::new(16.5, 0.0, -8.4), -0.4),
        WorldResourceNodeSpawn::new(4, IRON_NODE_ID, Vec3Net::new(18.4, 0.0, -10.3), 0.8),
        WorldResourceNodeSpawn::new(5, SULFUR_NODE_ID, Vec3Net::new(13.3, 0.0, -13.0), 0.5),
        WorldResourceNodeSpawn::new(6, SULFUR_NODE_ID, Vec3Net::new(16.1, 0.0, -13.2), -1.0),
        // North-west ore vein.
        WorldResourceNodeSpawn::new(7, COAL_NODE_ID, Vec3Net::new(-22.0, 0.0, 21.0), 0.4),
        WorldResourceNodeSpawn::new(8, IRON_NODE_ID, Vec3Net::new(-25.5, 0.0, 23.0), -0.7),
        WorldResourceNodeSpawn::new(9, SULFUR_NODE_ID, Vec3Net::new(-20.5, 0.0, 25.5), 1.4),
        // South-east ore vein.
        WorldResourceNodeSpawn::new(10, IRON_NODE_ID, Vec3Net::new(25.0, 0.0, -22.0), 0.3),
        WorldResourceNodeSpawn::new(11, COAL_NODE_ID, Vec3Net::new(28.5, 0.0, -24.0), -1.1),
        WorldResourceNodeSpawn::new(12, SULFUR_NODE_ID, Vec3Net::new(23.5, 0.0, -27.0), 0.6),
        // Scattered ores around the player loop area.
        WorldResourceNodeSpawn::new(13, IRON_NODE_ID, Vec3Net::new(-15.0, 0.0, -8.0), 0.9),
        WorldResourceNodeSpawn::new(14, COAL_NODE_ID, Vec3Net::new(-18.0, 0.0, -22.0), -0.5),
        WorldResourceNodeSpawn::new(15, SULFUR_NODE_ID, Vec3Net::new(20.0, 0.0, 22.0), 1.2),
        // North-east forest grove — mixed variants.
        WorldResourceNodeSpawn::new(
            20,
            PINE_TREE_LARGE_NODE_ID,
            Vec3Net::new(28.0, 0.0, 28.0),
            0.1,
        ),
        WorldResourceNodeSpawn::new(21, PINE_TREE_NODE_ID, Vec3Net::new(22.0, 0.0, 31.0), 0.8),
        WorldResourceNodeSpawn::new(22, PINE_TREE_NODE_ID, Vec3Net::new(18.5, 0.0, 18.5), -0.4),
        WorldResourceNodeSpawn::new(
            23,
            PINE_TREE_SMALL_NODE_ID,
            Vec3Net::new(26.0, 0.0, 21.5),
            1.3,
        ),
        WorldResourceNodeSpawn::new(
            24,
            BIRCH_TREE_LARGE_NODE_ID,
            Vec3Net::new(16.5, 0.0, 26.0),
            0.5,
        ),
        WorldResourceNodeSpawn::new(25, BIRCH_TREE_NODE_ID, Vec3Net::new(31.0, 0.0, 17.5), -1.2),
        WorldResourceNodeSpawn::new(
            26,
            BIRCH_TREE_SMALL_NODE_ID,
            Vec3Net::new(20.5, 0.0, 24.5),
            0.3,
        ),
        // North-west forest grove — denser, more pines.
        WorldResourceNodeSpawn::new(
            30,
            PINE_TREE_LARGE_NODE_ID,
            Vec3Net::new(-26.0, 0.0, 18.5),
            0.7,
        ),
        WorldResourceNodeSpawn::new(31, PINE_TREE_NODE_ID, Vec3Net::new(-19.0, 0.0, 28.5), -0.3),
        WorldResourceNodeSpawn::new(
            32,
            PINE_TREE_SMALL_NODE_ID,
            Vec3Net::new(-15.5, 0.0, 18.5),
            1.1,
        ),
        WorldResourceNodeSpawn::new(33, PINE_TREE_NODE_ID, Vec3Net::new(-30.0, 0.0, 30.0), 0.2),
        WorldResourceNodeSpawn::new(
            34,
            BIRCH_TREE_LARGE_NODE_ID,
            Vec3Net::new(-22.5, 0.0, 30.0),
            -0.8,
        ),
        WorldResourceNodeSpawn::new(35, BIRCH_TREE_NODE_ID, Vec3Net::new(-30.5, 0.0, 21.5), 0.5),
        WorldResourceNodeSpawn::new(36, BIRCH_TREE_NODE_ID, Vec3Net::new(-16.5, 0.0, 24.0), -1.0),
        WorldResourceNodeSpawn::new(
            37,
            BIRCH_TREE_SMALL_NODE_ID,
            Vec3Net::new(-32.0, 0.0, 33.0),
            1.4,
        ),
        // South-east forest grove.
        WorldResourceNodeSpawn::new(40, PINE_TREE_NODE_ID, Vec3Net::new(24.5, 0.0, -28.0), 0.6),
        WorldResourceNodeSpawn::new(
            41,
            PINE_TREE_SMALL_NODE_ID,
            Vec3Net::new(18.5, 0.0, -22.5),
            -0.9,
        ),
        WorldResourceNodeSpawn::new(
            42,
            BIRCH_TREE_LARGE_NODE_ID,
            Vec3Net::new(30.5, 0.0, -30.5),
            1.0,
        ),
        WorldResourceNodeSpawn::new(43, BIRCH_TREE_NODE_ID, Vec3Net::new(16.5, 0.0, -24.5), -0.2),
        // South-west forest grove.
        WorldResourceNodeSpawn::new(
            50,
            PINE_TREE_LARGE_NODE_ID,
            Vec3Net::new(-28.0, 0.0, -28.5),
            -0.5,
        ),
        WorldResourceNodeSpawn::new(51, PINE_TREE_NODE_ID, Vec3Net::new(-22.0, 0.0, -22.0), 1.1),
        WorldResourceNodeSpawn::new(
            52,
            PINE_TREE_SMALL_NODE_ID,
            Vec3Net::new(-32.5, 0.0, -22.0),
            0.3,
        ),
        WorldResourceNodeSpawn::new(
            53,
            BIRCH_TREE_NODE_ID,
            Vec3Net::new(-18.5, 0.0, -30.0),
            -0.7,
        ),
        WorldResourceNodeSpawn::new(
            54,
            BIRCH_TREE_SMALL_NODE_ID,
            Vec3Net::new(-32.5, 0.0, -32.0),
            1.5,
        ),
        // Trees near the player spawn — close enough to chop in the first
        // minute of play, far enough not to crowd the controller test area.
        WorldResourceNodeSpawn::new(60, PINE_TREE_NODE_ID, Vec3Net::new(-9.0, 0.0, 8.5), -0.3),
        WorldResourceNodeSpawn::new(61, BIRCH_TREE_NODE_ID, Vec3Net::new(-1.0, 0.0, 9.5), -0.9),
        WorldResourceNodeSpawn::new(
            62,
            PINE_TREE_SMALL_NODE_ID,
            Vec3Net::new(-6.0, 0.0, 14.0),
            0.4,
        ),
        WorldResourceNodeSpawn::new(
            63,
            BIRCH_TREE_SMALL_NODE_ID,
            Vec3Net::new(3.0, 0.0, 14.0),
            0.7,
        ),
        WorldResourceNodeSpawn::new(65, PINE_TREE_NODE_ID, Vec3Net::new(11.5, 0.0, 16.0), -0.2),
        // Lone landmarks in the open middle bands of the map.
        WorldResourceNodeSpawn::new(
            71,
            PINE_TREE_LARGE_NODE_ID,
            Vec3Net::new(-12.5, 0.0, -16.0),
            0.6,
        ),
        WorldResourceNodeSpawn::new(
            72,
            BIRCH_TREE_LARGE_NODE_ID,
            Vec3Net::new(14.5, 0.0, 4.0),
            -1.2,
        ),
    ]
}
