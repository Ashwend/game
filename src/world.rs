use serde::{Deserialize, Serialize};

use crate::{
    protocol::Vec3Net,
    resources::{
        BIRCH_TREE_NODE_ID, COAL_NODE_ID, DEAD_TREE_NODE_ID, IRON_NODE_ID, PINE_TREE_NODE_ID,
        SULFUR_NODE_ID,
    },
};

pub const DEFAULT_FLOOR_SIZE: f32 = 80.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MapType {
    #[default]
    Test,
    Procedural {
        seed: u64,
        #[serde(default)]
        size: ProceduralMapSize,
    },
}

impl MapType {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Test => "Test",
            Self::Procedural { .. } => "Procedural",
        }
    }

    pub fn world_data(&self) -> WorldData {
        match self {
            Self::Test => WorldData::test_world(),
            Self::Procedural { seed, size } => WorldData::procedural(*seed, *size),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProceduralMapSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl ProceduralMapSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
        }
    }

    pub fn floor_size(self) -> f32 {
        match self {
            Self::Small => 64.0,
            Self::Medium => 128.0,
            Self::Large => 256.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldData {
    pub floor_size: f32,
    pub blocks: Vec<WorldBlock>,
    #[serde(default)]
    pub resource_nodes: Vec<WorldResourceNodeSpawn>,
}

impl Default for WorldData {
    fn default() -> Self {
        Self::test_world()
    }
}

impl WorldData {
    pub fn procedural(seed: u64, size: ProceduralMapSize) -> Self {
        let _ = seed;
        Self::flat_floor(size.floor_size())
    }

    pub fn flat_floor(floor_size: f32) -> Self {
        Self {
            floor_size,
            blocks: Vec::new(),
            resource_nodes: Vec::new(),
        }
    }

    pub fn test_world() -> Self {
        Self {
            floor_size: DEFAULT_FLOOR_SIZE,
            blocks: vec![
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
            ],
            resource_nodes: vec![
                WorldResourceNodeSpawn::new(1, COAL_NODE_ID, Vec3Net::new(12.5, 0.0, -8.5), 0.2),
                WorldResourceNodeSpawn::new(2, COAL_NODE_ID, Vec3Net::new(14.3, 0.0, -10.1), 1.1),
                WorldResourceNodeSpawn::new(3, IRON_NODE_ID, Vec3Net::new(16.5, 0.0, -8.4), -0.4),
                WorldResourceNodeSpawn::new(4, IRON_NODE_ID, Vec3Net::new(18.4, 0.0, -10.3), 0.8),
                WorldResourceNodeSpawn::new(5, SULFUR_NODE_ID, Vec3Net::new(13.3, 0.0, -13.0), 0.5),
                WorldResourceNodeSpawn::new(
                    6,
                    SULFUR_NODE_ID,
                    Vec3Net::new(16.1, 0.0, -13.2),
                    -1.0,
                ),
                WorldResourceNodeSpawn::new(
                    20,
                    PINE_TREE_NODE_ID,
                    Vec3Net::new(-8.6, 0.0, 5.8),
                    0.1,
                ),
                WorldResourceNodeSpawn::new(
                    21,
                    BIRCH_TREE_NODE_ID,
                    Vec3Net::new(-6.2, 0.0, 7.2),
                    -0.6,
                ),
                WorldResourceNodeSpawn::new(
                    22,
                    DEAD_TREE_NODE_ID,
                    Vec3Net::new(-3.5, 0.0, 6.4),
                    0.9,
                ),
                WorldResourceNodeSpawn::new(
                    23,
                    PINE_TREE_NODE_ID,
                    Vec3Net::new(5.4, 0.0, 7.4),
                    -0.2,
                ),
                WorldResourceNodeSpawn::new(
                    24,
                    BIRCH_TREE_NODE_ID,
                    Vec3Net::new(8.0, 0.0, 6.1),
                    0.7,
                ),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldResourceNodeSpawn {
    pub id: u64,
    pub definition_id: String,
    pub position: Vec3Net,
    pub yaw: f32,
}

impl WorldResourceNodeSpawn {
    pub fn new(id: u64, definition_id: impl Into<String>, position: Vec3Net, yaw: f32) -> Self {
        Self {
            id,
            definition_id: definition_id.into(),
            position,
            yaw,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WorldBlock {
    pub center: Vec3Net,
    pub half_extents: Vec3Net,
}

impl WorldBlock {
    pub const fn new(center: Vec3Net, half_extents: Vec3Net) -> Self {
        Self {
            center,
            half_extents,
        }
    }

    pub fn min(self) -> Vec3Net {
        Vec3Net::new(
            self.center.x - self.half_extents.x,
            self.center.y - self.half_extents.y,
            self.center.z - self.half_extents.z,
        )
    }

    pub fn max(self) -> Vec3Net {
        Vec3Net::new(
            self.center.x + self.half_extents.x,
            self.center.y + self.half_extents.y,
            self.center.z + self.half_extents.z,
        )
    }

    pub fn size(self) -> Vec3Net {
        self.half_extents.scale(2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_are_above_floor() {
        let world = WorldData::test_world();
        assert!(world.floor_size > 0.0);
        for block in world.blocks {
            assert!(block.min().y >= 0.0);
            assert!(block.size().x > 0.0);
            assert!(block.size().y > 0.0);
            assert!(block.size().z > 0.0);
        }
    }

    #[test]
    fn test_world_includes_movement_test_shapes() {
        let world = WorldData::test_world();
        let low_steps = world
            .blocks
            .iter()
            .filter(|block| block.size().y <= 0.8)
            .count();
        let tall_walls = world
            .blocks
            .iter()
            .filter(|block| {
                let size = block.size();
                size.y >= 2.0 && (size.x >= 4.0 || size.z >= 4.0)
            })
            .count();

        assert!(world.blocks.len() >= 24);
        assert!(low_steps >= 8);
        assert!(tall_walls >= 5);
    }

    #[test]
    fn test_world_ore_nodes_do_not_overlap_blocks() {
        const ORE_RADIUS: f32 = 0.8;

        let world = WorldData::test_world();
        let ore_nodes = world
            .resource_nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.definition_id.as_str(),
                    COAL_NODE_ID | IRON_NODE_ID | SULFUR_NODE_ID
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(ore_nodes.len(), 6);
        for node in ore_nodes {
            for block in &world.blocks {
                let min = block.min();
                let max = block.max();
                assert!(
                    node.position.x < min.x - ORE_RADIUS
                        || node.position.x > max.x + ORE_RADIUS
                        || node.position.z < min.z - ORE_RADIUS
                        || node.position.z > max.z + ORE_RADIUS,
                    "ore node {} at ({:.1}, {:.1}) overlaps block centered at ({:.1}, {:.1})",
                    node.definition_id,
                    node.position.x,
                    node.position.z,
                    block.center.x,
                    block.center.z
                );
            }
        }
    }

    #[test]
    fn map_type_default_and_labels_are_stable() {
        assert_eq!(MapType::default(), MapType::Test);
        assert_eq!(MapType::Test.label(), "Test");
        assert_eq!(
            MapType::Procedural {
                seed: 42,
                size: ProceduralMapSize::Medium,
            }
            .label(),
            "Procedural"
        );
    }

    #[test]
    fn procedural_world_is_flat_floor_matching_size() {
        let world = MapType::Procedural {
            seed: 42,
            size: ProceduralMapSize::Large,
        }
        .world_data();

        assert_eq!(world.floor_size, ProceduralMapSize::Large.floor_size());
        assert!(world.blocks.is_empty());
    }
}
