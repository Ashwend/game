use bevy::prelude::*;
use lightyear::{
    connection::client::Connected,
    prelude::{server::ClientOf, *},
};

use crate::{controller::PlayerController, protocol::Vec3Net};

use super::protocol::{NetworkController, NetworkInputSequence, NetworkPlayerBundle};

pub(super) fn handle_connected_client(
    trigger: On<Add, Connected>,
    clients: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(remote_id) = clients.get(trigger.entity) else {
        return;
    };
    let client_id = remote_id.0;
    commands.spawn((
        NetworkPlayerBundle::new(client_id, Vec3Net::ZERO),
        NetworkController(PlayerController::spawn()),
        NetworkInputSequence::default(),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: trigger.entity,
            lifetime: Default::default(),
        },
    ));
}
