use std::collections::HashSet;

use bevy::prelude::*;

use crate::protocol::{ClientId, Vec3Net};

use super::super::{
    scene::{NetworkPlayer, PlayerVisualAssets, player_visual_position},
    state::ClientRuntime,
};

const REMOTE_PLAYER_INTERPOLATION_SECONDS: f32 = 0.1;
const REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE: f32 = 6.0;

/// Persistent `client_id → Entity` map for remote players. Mirrors the live
/// entity set so the snapshot-apply system doesn't have to rebuild it from a
/// `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct RemotePlayerEntities(pub(crate) std::collections::HashMap<ClientId, Entity>);

pub(crate) fn apply_snapshot_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    assets: Res<PlayerVisualAssets>,
    mut entities: ResMut<RemotePlayerEntities>,
    mut players: Query<(&Transform, &mut NetworkPlayerInterpolation), With<NetworkPlayer>>,
) {
    if runtime.snapshot.is_none() {
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    }

    let local_client_id = runtime.client_id;
    let players_in = collect_remote_players(&runtime);
    let snapshot_remote_ids: HashSet<ClientId> = players_in
        .iter()
        .filter(|p| Some(p.client_id) != local_client_id)
        .map(|p| p.client_id)
        .collect();
    let entities = &mut *entities;

    for player in &players_in {
        if Some(player.client_id) == local_client_id {
            continue;
        }
        let feet = Vec3::from(player.position);
        let target = Transform::from_translation(player_visual_position(feet))
            .with_rotation(Quat::from_rotation_y(player.yaw));
        if let Some(entity) = entities.0.get(&player.client_id).copied() {
            if let Ok((current, mut interpolation)) = players.get_mut(entity) {
                interpolation.retarget(player.tick, current, target);
                let transform = interpolation.advance(time.delta_secs());
                commands.entity(entity).insert(transform);
            }
        } else {
            let entity = commands
                .spawn((
                    Name::new(format!("Player {}", player.client_id)),
                    NetworkPlayer {
                        client_id: player.client_id,
                    },
                    NetworkPlayerInterpolation::new(player.tick, target),
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(assets.remote_material.clone()),
                    target,
                    Visibility::Visible,
                ))
                .id();
            entities.0.insert(player.client_id, entity);
        }
    }

    entities.0.retain(|id, entity| {
        if snapshot_remote_ids.contains(id) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
}

/// Minimal view of a remote player needed by `apply_snapshot_system`.
/// Keeps the feature-gated input source decoupled from the full
/// `PlayerState` wire shape — only the visual-driver fields are
/// included, plus a per-id `tick` so the interpolator knows when to
/// retarget.
struct RemotePlayerSample {
    client_id: ClientId,
    position: Vec3Net,
    yaw: f32,
    tick: u64,
}

fn collect_remote_players(runtime: &ClientRuntime) -> Vec<RemotePlayerSample> {
    runtime
        .snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .players
                .iter()
                .map(|player| RemotePlayerSample {
                    client_id: player.client_id,
                    position: player.position,
                    yaw: player.yaw,
                    tick: snapshot.tick,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct NetworkPlayerInterpolation {
    snapshot_tick: u64,
    from: Transform,
    to: Transform,
    elapsed: f32,
}

impl NetworkPlayerInterpolation {
    fn new(snapshot_tick: u64, transform: Transform) -> Self {
        Self {
            snapshot_tick,
            from: transform,
            to: transform,
            elapsed: REMOTE_PLAYER_INTERPOLATION_SECONDS,
        }
    }

    fn retarget(&mut self, snapshot_tick: u64, current: &Transform, target: Transform) {
        if snapshot_tick <= self.snapshot_tick {
            return;
        }

        let distance = current.translation.distance(target.translation);
        self.from = if distance > REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE {
            target
        } else {
            *current
        };
        self.to = target;
        self.elapsed = 0.0;
        self.snapshot_tick = snapshot_tick;
    }

    fn advance(&mut self, delta_seconds: f32) -> Transform {
        self.elapsed += delta_seconds.max(0.0);
        let alpha = (self.elapsed / REMOTE_PLAYER_INTERPOLATION_SECONDS).clamp(0.0, 1.0);
        Transform::from_translation(self.from.translation.lerp(self.to.translation, alpha))
            .with_rotation(self.from.rotation.slerp(self.to.rotation, alpha))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MAX_HEALTH, PlayerState, SteamId, Vec3Net, WorldSnapshot};

    fn player(client_id: ClientId, steam_id: SteamId, position: Vec3Net, yaw: f32) -> PlayerState {
        PlayerState {
            client_id,
            steam_id,
            name: format!("Player {client_id}"),
            position,
            velocity: Vec3Net::ZERO,
            yaw,
            pitch: 0.0,
            health: MAX_HEALTH,
            grounded: true,
            last_processed_input: 0,
            is_admin: false,
            chat_bubble: None,
            inventory: None,
            crafting: None,
            open_furnace: None,
        }
    }

    fn app_with_snapshot(snapshot: Option<WorldSnapshot>, client_id: Option<ClientId>) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(ClientRuntime {
            client_id,
            snapshot,
            ..Default::default()
        });
        app.insert_resource(PlayerVisualAssets {
            mesh: Handle::default(),
            remote_material: Handle::default(),
        });
        app.insert_resource(RemotePlayerEntities::default());
        app.add_systems(Update, apply_snapshot_system);
        app
    }

    // `runtime.snapshot` is the locally-synthesized mirror of replicated
    // state under Phase 6, but unit tests can still hand-build a snapshot
    // and drive the consumer directly — the synth system only runs in
    // the full app's Update schedule and is not included in this test rig.
    #[test]
    fn apply_snapshot_spawns_updates_and_removes_remote_players() {
        let mut app = app_with_snapshot(
            Some(WorldSnapshot {
                tick: 1,
                players: vec![
                    player(1, 1, Vec3Net::ZERO, 0.0),
                    player(2, 2, Vec3Net::new(2.0, 0.0, 0.0), 1.0),
                ],
                dropped_items: Vec::new(),
                resource_nodes: Vec::new(),
                deployed_entities: Vec::new(),
            }),
            Some(1),
        );

        app.update();

        let players = {
            let mut query = app.world_mut().query::<(&NetworkPlayer, &Transform)>();
            query
                .iter(app.world())
                .map(|(player, transform)| (player.client_id, transform.translation))
                .collect::<Vec<_>>()
        };
        assert_eq!(players.len(), 1);
        assert_eq!(players[0].0, 2);
        assert!(players[0].1.x > 1.9);

        app.world_mut().resource_mut::<ClientRuntime>().snapshot = Some(WorldSnapshot {
            tick: 2,
            players: vec![player(2, 2, Vec3Net::new(4.0, 0.0, 0.0), 0.5)],
            dropped_items: Vec::new(),
            resource_nodes: Vec::new(),
            deployed_entities: Vec::new(),
        });
        app.update();

        let players = {
            let mut query = app.world_mut().query::<(&NetworkPlayer, &Transform)>();
            query
                .iter(app.world())
                .map(|(player, transform)| (player.client_id, transform.translation))
                .collect::<Vec<_>>()
        };
        assert_eq!(players.len(), 1);
        // Position is now interpolating between the old (2,0,0) and new (4,0,0) target.
        // Exact value depends on dt, but it must sit within that range and the entity
        // must persist.
        assert!(players[0].1.x >= 1.9 && players[0].1.x <= 4.1);

        app.world_mut().resource_mut::<ClientRuntime>().snapshot = None;
        app.update();

        let players = {
            let mut query = app.world_mut().query::<(&NetworkPlayer, &Transform)>();
            query
                .iter(app.world())
                .map(|(player, transform)| (player.client_id, transform.translation))
                .collect::<Vec<_>>()
        };
        assert!(players.is_empty());
    }

    #[test]
    fn remote_player_interpolation_blends_between_snapshot_targets() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(4.0, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let mut interpolation = NetworkPlayerInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let halfway = interpolation.advance(REMOTE_PLAYER_INTERPOLATION_SECONDS * 0.5);

        assert!((halfway.translation.x - 2.0).abs() < 0.001);
        assert!(halfway.rotation.angle_between(current.rotation) > 0.1);
        assert!(halfway.rotation.angle_between(target.rotation) > 0.1);
    }

    #[test]
    fn remote_player_interpolation_snaps_extreme_corrections() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE + 1.0, 0.0, 0.0);
        let mut interpolation = NetworkPlayerInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let corrected = interpolation.advance(0.0);

        assert_eq!(corrected.translation, target.translation);
    }
}
