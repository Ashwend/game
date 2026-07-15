//! Transform interpolation for remote player visuals: blends each avatar
//! between replicated pose snapshots so movement glides at render rate instead
//! of stepping at the network tick rate.

use bevy::prelude::*;

pub(super) const REMOTE_PLAYER_INTERPOLATION_SECONDS: f32 = 0.1;
const REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE: f32 = 6.0;

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct NetworkPlayerInterpolation {
    snapshot_tick: u64,
    from: Transform,
    to: Transform,
    elapsed: f32,
}

impl NetworkPlayerInterpolation {
    pub(super) fn new(snapshot_tick: u64, transform: Transform) -> Self {
        Self {
            snapshot_tick,
            from: transform,
            to: transform,
            elapsed: REMOTE_PLAYER_INTERPOLATION_SECONDS,
        }
    }

    /// Hard-snap the interpolation onto `target` with no blend, and mark
    /// it fully advanced so `advance` returns `target` immediately. Used
    /// for respawn, which teleports the avatar: blending would slide it
    /// across the world (or briefly hold it at the old death spot) for a
    /// few frames.
    pub(super) fn snap_to(&mut self, snapshot_tick: u64, target: Transform) {
        self.from = target;
        self.to = target;
        self.elapsed = REMOTE_PLAYER_INTERPOLATION_SECONDS;
        self.snapshot_tick = snapshot_tick;
    }

    pub(super) fn retarget(&mut self, snapshot_tick: u64, current: &Transform, target: Transform) {
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

    pub(super) fn advance(&mut self, delta_seconds: f32) -> Transform {
        self.elapsed += delta_seconds.max(0.0);
        let alpha = (self.elapsed / REMOTE_PLAYER_INTERPOLATION_SECONDS).clamp(0.0, 1.0);
        Transform::from_translation(self.from.translation.lerp(self.to.translation, alpha))
            .with_rotation(self.from.rotation.slerp(self.to.rotation, alpha))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The system-level spawn/update/despawn cycle flows off the
    // replicated `(Player, PlayerPublic)` query. Exercising it as a
    // unit test would need the Lightyear replication plugin set up,
    // which is what the integration tests in `src/net/tests.rs`
    // already cover. The interpolation math below is the piece that
    // stays unit-testable in isolation.

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
    fn remote_player_interpolation_snaps_on_respawn_regardless_of_distance() {
        // A respawn teleport must jump even when the new spawn is well
        // within the blend's snap distance, otherwise the revived avatar
        // slides from the death spot (the killer-side flicker).
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let mut interpolation = NetworkPlayerInterpolation::new(1, current);
        let nearby_spawn = Transform::from_xyz(2.0, 0.0, 0.0);
        assert!(nearby_spawn.translation.length() < REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE);

        interpolation.snap_to(2, nearby_spawn);
        let result = interpolation.advance(REMOTE_PLAYER_INTERPOLATION_SECONDS * 0.1);
        assert_eq!(result.translation, nearby_spawn.translation);
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

    #[test]
    fn remote_player_interpolation_ignores_stale_ticks() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let mut interpolation = NetworkPlayerInterpolation::new(5, current);
        let target = Transform::from_xyz(3.0, 0.0, 0.0);

        // A tick <= the stored tick is a stale duplicate and must not move
        // the blend off the current pose.
        interpolation.retarget(5, &current, target);
        let after = interpolation.advance(REMOTE_PLAYER_INTERPOLATION_SECONDS);
        assert_eq!(after.translation, current.translation);
    }
}
