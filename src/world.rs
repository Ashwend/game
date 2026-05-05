use crate::protocol::Vec3Net;

pub const FLOOR_SIZE: f32 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq)]
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

pub const TEST_WORLD_BLOCKS: [WorldBlock; 10] = [
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
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_are_above_floor() {
        for block in TEST_WORLD_BLOCKS {
            assert!(block.min().y >= 0.0);
            assert!(block.size().x > 0.0);
            assert!(block.size().y > 0.0);
            assert!(block.size().z > 0.0);
        }
    }
}
