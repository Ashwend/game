use bevy::{
    ecs::entity::{EntityMapper, MapEntities},
    math::Curve,
    prelude::*,
};
use lightyear::prelude::{input::native::InputPlugin, *};
use serde::{Deserialize, Serialize};

use crate::{
    controller::PlayerController,
    protocol::{MAX_HEALTH, SteamId, Vec3Net},
    world::WorldData,
};

#[derive(Clone)]
pub(super) struct LightyearProtocolPlugin;

impl Plugin for LightyearProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputPlugin::<NetworkInput>::default());

        app.register_component::<NetworkPlayerId>();
        app.register_component::<NetworkPlayerName>();
        app.register_component::<NetworkSteamId>();
        app.register_component::<NetworkAdmin>();

        app.register_component::<NetworkPosition>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<NetworkVelocity>().add_prediction();
        app.register_component::<NetworkLook>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<NetworkHealth>().add_prediction();
        app.register_component::<NetworkGrounded>().add_prediction();
    }
}

#[derive(Resource, Clone)]
pub(super) struct NetworkWorld(pub(super) WorldData);

#[derive(Component)]
pub(super) struct NetworkController(pub(super) PlayerController);

#[derive(Component, Default)]
pub(super) struct NetworkInputSequence(pub(super) u64);

#[derive(Bundle)]
pub(super) struct NetworkPlayerBundle {
    id: NetworkPlayerId,
    steam_id: NetworkSteamId,
    name: NetworkPlayerName,
    admin: NetworkAdmin,
    position: NetworkPosition,
    velocity: NetworkVelocity,
    look: NetworkLook,
    health: NetworkHealth,
    grounded: NetworkGrounded,
}

impl NetworkPlayerBundle {
    pub(super) fn new(id: PeerId, position: Vec3Net) -> Self {
        let steam_id = id.to_bits();
        Self {
            id: NetworkPlayerId(id),
            steam_id: NetworkSteamId(steam_id),
            name: NetworkPlayerName(clean_network_name(steam_id)),
            admin: NetworkAdmin(false),
            position: NetworkPosition(position),
            velocity: NetworkVelocity(Vec3Net::ZERO),
            look: NetworkLook::default(),
            health: NetworkHealth(MAX_HEALTH),
            grounded: NetworkGrounded(true),
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct NetworkPlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkSteamId(pub SteamId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkPlayerName(pub String);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkAdmin(pub bool);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkPosition(pub Vec3Net);

impl Ease for NetworkPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            NetworkPosition(lerp_vec3(start.0, end.0, t))
        })
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkVelocity(pub Vec3Net);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkLook {
    pub yaw: f32,
    pub pitch: f32,
}

impl Ease for NetworkLook {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| Self {
            yaw: lerp_f32(start.yaw, end.yaw, t),
            pitch: lerp_f32(start.pitch, end.pitch, t),
        })
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct NetworkHealth(pub f32);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct NetworkGrounded(pub bool);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct NetworkInput {
    pub direction: Vec3Net,
    pub sprint: bool,
    pub jump: bool,
    pub yaw: f32,
    pub pitch: f32,
}

impl MapEntities for NetworkInput {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
}

fn lerp_vec3(start: Vec3Net, end: Vec3Net, t: f32) -> Vec3Net {
    Vec3Net::new(
        lerp_f32(start.x, end.x, t),
        lerp_f32(start.y, end.y, t),
        lerp_f32(start.z, end.z, t),
    )
}

fn lerp_f32(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn clean_network_name(steam_id: SteamId) -> String {
    format!("Player {steam_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_position_interpolates_linearly() {
        let halfway = lerp_vec3(Vec3Net::ZERO, Vec3Net::new(2.0, 4.0, 6.0), 0.5);
        assert_eq!(halfway, Vec3Net::new(1.0, 2.0, 3.0));
    }
}
