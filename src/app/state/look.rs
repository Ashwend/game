use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct LookState {
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) sensitivity: Vec2,
}

impl Default for LookState {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.04,
            sensitivity: Vec2::new(0.0024, 0.0020),
        }
    }
}
