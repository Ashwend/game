use serde::{Deserialize, Serialize};

use crate::protocol::Vec3Net;

pub const DEFAULT_FLOOR_SIZE: f32 = 80.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MapType {
    #[default]
    Test,
    Procedural {
        seed: u64,
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
            Self::Procedural { .. } => WorldData::test_world(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldData {
    pub floor_size: f32,
    pub blocks: Vec<WorldBlock>,
}

impl Default for WorldData {
    fn default() -> Self {
        Self::test_world()
    }
}

impl WorldData {
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
            ],
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
    fn map_type_default_and_labels_are_stable() {
        assert_eq!(MapType::default(), MapType::Test);
        assert_eq!(MapType::Test.label(), "Test");
        assert_eq!(MapType::Procedural { seed: 42 }.label(), "Procedural");
    }
}
