use std::collections::HashMap;

use bevy::prelude::*;

use super::super::{
    scene::{NetworkPlayer, PlayerVisualAssets, player_visual_position},
    state::ClientRuntime,
};

pub(crate) fn apply_snapshot_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Res<PlayerVisualAssets>,
    players: Query<(Entity, &NetworkPlayer)>,
) {
    let Some(snapshot) = &runtime.snapshot else {
        for (entity, _) in &players {
            commands.entity(entity).despawn();
        }
        return;
    };

    let existing = players
        .iter()
        .map(|(entity, player)| (player.client_id, entity))
        .collect::<HashMap<_, _>>();

    let remote_players = snapshot
        .players
        .iter()
        .filter(|player| Some(player.client_id) != runtime.client_id)
        .collect::<Vec<_>>();

    for player in &remote_players {
        let target = Vec3::new(player.position.x, player.position.y, player.position.z);
        let rotation = Quat::from_rotation_y(player.yaw);
        if let Some(entity) = existing.get(&player.client_id) {
            commands
                .entity(*entity)
                .insert((Transform::from_translation(player_visual_position(target))
                    .with_rotation(rotation),));
        } else {
            commands.spawn((
                Name::new(format!("Player {}", player.client_id)),
                NetworkPlayer {
                    client_id: player.client_id,
                },
                Mesh3d(assets.mesh.clone()),
                MeshMaterial3d(assets.remote_material.clone()),
                Transform::from_translation(player_visual_position(target)).with_rotation(rotation),
                Visibility::Visible,
            ));
        }
    }

    for (entity, network_player) in &players {
        if Some(network_player.client_id) == runtime.client_id
            || !remote_players
                .iter()
                .any(|player| player.client_id == network_player.client_id)
        {
            commands.entity(entity).despawn();
        }
    }
}
