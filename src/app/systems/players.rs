use std::collections::HashSet;

use bevy::{ecs::change_detection::Ref, prelude::*};

use crate::{
    protocol::ClientId,
    server::{Player, PlayerPublic},
};

use super::super::{
    scene::{NetworkPlayer, PlayerVisualAssets, player_visual_position},
    state::ClientRuntime,
};

const REMOTE_PLAYER_INTERPOLATION_SECONDS: f32 = 0.1;
const REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE: f32 = 6.0;

/// Persistent `client_id → Entity` map for remote players. Mirrors the live
/// entity set so the reconciliation system doesn't have to rebuild it from
/// a `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct RemotePlayerEntities(pub(crate) std::collections::HashMap<ClientId, Entity>);

/// Reconciles the set of visual `NetworkPlayer` entities against the
/// Lightyear-replicated `(Player, PlayerPublic)` entities. Spawn,
/// despawn, and interpolation re-target all flow off the replicated
/// query — one visual entity per replicated entity.
pub(crate) fn apply_snapshot_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    assets: Res<PlayerVisualAssets>,
    mut entities: ResMut<RemotePlayerEntities>,
    mut players: Query<(&Transform, &mut NetworkPlayerInterpolation), With<NetworkPlayer>>,
    replicated: Query<(&Player, Ref<PlayerPublic>)>,
) {
    let Some(local_client_id) = runtime.client_id else {
        // Not connected — tear down any remote visuals from a prior
        // session.
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    };

    // `last_changed().get()` advances every time the replicated
    // `PlayerPublic` is mutated, so the interpolator only re-targets
    // when there's a real update (Phase 5's tick-as-change-tick trick).
    let mut visible_ids = HashSet::new();
    let entities = &mut *entities;

    for (player, public) in &replicated {
        if player.client_id == local_client_id {
            continue;
        }
        visible_ids.insert(player.client_id);
        let tick = public.last_changed().get() as u64;
        let feet = Vec3::from(public.position);
        let target = Transform::from_translation(player_visual_position(feet))
            .with_rotation(Quat::from_rotation_y(public.yaw));
        if let Some(entity) = entities.0.get(&player.client_id).copied() {
            if let Ok((current, mut interpolation)) = players.get_mut(entity) {
                interpolation.retarget(tick, current, target);
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
                    NetworkPlayerInterpolation::new(tick, target),
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
        if visible_ids.contains(id) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
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

    // The system-level spawn/update/despawn cycle now flows off the
    // replicated `(Player, PlayerPublic)` query. Exercising it as a
    // unit test would need the Lightyear replication plugin set up,
    // which is what the integration tests in `src/net/tests.rs`
    // already cover. The interpolation math below is the only piece
    // that stays unit-testable in isolation.

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
