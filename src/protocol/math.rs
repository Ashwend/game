//! Wire-friendly vector/quaternion types. Plain `f32` fields so they serialise
//! compactly and convert to/from Bevy's `Vec3`.

use bevy::prelude::{Reflect, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Vec3Net {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3Net {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length_squared(self) -> f32 {
        self.x
            .mul_add(self.x, self.y.mul_add(self.y, self.z * self.z))
    }

    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq <= f32::EPSILON {
            return Self::ZERO;
        }

        let inv_len = len_sq.sqrt().recip();
        Self::new(self.x * inv_len, self.y * inv_len, self.z * inv_len)
    }

    pub fn scale(self, value: f32) -> Self {
        Self::new(self.x * value, self.y * value, self.z * value)
    }

    pub fn plus(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    pub fn minus(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x
            .mul_add(other.x, self.y.mul_add(other.y, self.z * other.z))
    }

    /// Squared distance between `self` and `other` on the XZ ground plane,
    /// ignoring Y. Gameplay reach checks ("within arm's length on the ground")
    /// use this so a vertical offset between a player and a node, deployable,
    /// furnace, or loot bag never changes the interact range.
    pub fn horizontal_distance_squared(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dz = self.z - other.z;
        dx.mul_add(dx, dz * dz)
    }

    /// True when `other` is within `range` of `self` on the XZ plane. Compares
    /// squared values so no `sqrt` is taken.
    pub fn within_horizontal_range(self, other: Self, range: f32) -> bool {
        self.horizontal_distance_squared(other) <= range * range
    }
}

impl From<Vec3Net> for Vec3 {
    fn from(value: Vec3Net) -> Self {
        Vec3::new(value.x, value.y, value.z)
    }
}

impl From<Vec3> for Vec3Net {
    fn from(value: Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Reflect)]
pub struct QuatNet {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl QuatNet {
    pub const IDENTITY: Self = Self::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}

impl Default for QuatNet {
    fn default() -> Self {
        Self::IDENTITY
    }
}
