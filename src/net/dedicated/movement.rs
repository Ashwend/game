use bevy::{ecs::system::SystemParam, prelude::*};
use lightyear::prelude::{Predicted, input::native::ActionState};

use crate::{
    controller::PlayerController,
    protocol::{PlayerInput, SERVER_TICK_RATE_HZ},
    world::WorldData,
};

use super::protocol::{
    NetworkController, NetworkGrounded, NetworkHealth, NetworkInput, NetworkInputSequence,
    NetworkLook, NetworkPosition, NetworkVelocity, NetworkWorld,
};

type NetworkMovementData = (
    &'static mut NetworkController,
    &'static mut NetworkInputSequence,
    &'static ActionState<NetworkInput>,
    &'static mut NetworkPosition,
    &'static mut NetworkVelocity,
    &'static mut NetworkLook,
    &'static mut NetworkHealth,
    &'static mut NetworkGrounded,
);

#[derive(SystemParam)]
pub(super) struct NetworkMovementParams<'w, 's> {
    players: Query<'w, 's, NetworkMovementData, Without<Predicted>>,
}

pub(super) fn authoritative_movement_system(
    world: Res<NetworkWorld>,
    mut params: NetworkMovementParams,
) {
    for (
        mut controller,
        mut sequence,
        input,
        mut position,
        mut velocity,
        mut look,
        mut health,
        mut grounded,
    ) in &mut params.players
    {
        apply_network_input(
            &mut controller.0,
            &mut sequence,
            &input.0,
            &world.0,
            1.0 / SERVER_TICK_RATE_HZ,
        );
        write_controller_state(
            &controller.0,
            &mut position,
            &mut velocity,
            &mut look,
            &mut health,
            &mut grounded,
        );
    }
}

fn apply_network_input(
    controller: &mut PlayerController,
    sequence: &mut NetworkInputSequence,
    input: &NetworkInput,
    world: &WorldData,
    delta_seconds: f32,
) {
    sequence.0 += 1;
    controller.apply_input(PlayerInput {
        sequence: sequence.0,
        delta_seconds,
        direction: input.direction,
        sprint: input.sprint,
        jump: input.jump,
        yaw: input.yaw,
        pitch: input.pitch,
    });
    controller.simulate(delta_seconds, world);
}

fn write_controller_state(
    controller: &PlayerController,
    position: &mut NetworkPosition,
    velocity: &mut NetworkVelocity,
    look: &mut NetworkLook,
    health: &mut NetworkHealth,
    grounded: &mut NetworkGrounded,
) {
    position.0 = controller.position;
    velocity.0 = controller.velocity;
    look.yaw = controller.yaw;
    look.pitch = controller.pitch;
    health.0 = controller.health;
    grounded.0 = controller.grounded;
}
