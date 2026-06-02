use std::collections::{HashMap, HashSet};

use bevy::{ecs::change_detection::Ref, light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        scene::{ItemVisualAssets, NetworkDroppedItem},
        state::{ClientRuntime, PredictionState},
    },
    protocol::{DroppedItemId, QuatNet},
    server::{DroppedItem, DroppedItemTransform},
};

const DROPPED_ITEM_INTERPOLATION_SECONDS: f32 = 0.1;
const DROPPED_ITEM_INTERPOLATION_SNAP_DISTANCE: f32 = 6.0;
/// Per-frame cap on fresh dropped-item spawns. A chest spill or player
/// death can produce a burst of dozens of drops in a single snapshot; if
/// every visual entity is created the same frame the snapshot lands the
/// command-buffer flush stalls the main thread. Updates to existing
/// drops and despawns are uncapped, only first-time spawns are
/// budgeted. The remainder appears the next frame; at 50 ms snapshot
/// cadence and 16-per-frame, even a 200-item burst drains in under a
/// snapshot interval.
const MAX_DROPPED_ITEM_SPAWNS_PER_FRAME: usize = 16;
/// Beyond this many metres from the camera, dropped items skip the
/// per-frame lerp/slerp blend and just snap to their target transform.
/// At that distance the sub-frame interpolation is invisible (the item
/// is a tiny bag clutched at horizon-edge), but skipping the math saves
/// a `Quat::slerp` per item per frame, meaningful when a player has
/// dropped a large pile in the same chunk.
const DROPPED_ITEM_INTERPOLATION_MAX_DISTANCE_M: f32 = 40.0;

/// Persistent `id → Entity` lookup for dropped items. Maintained
/// incrementally as items spawn and despawn so the snapshot-apply system
/// doesn't have to rebuild a `HashMap` from a `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct DroppedItemEntities(pub(crate) HashMap<DroppedItemId, Entity>);

/// Reconcile the local `NetworkDroppedItem` visuals against the
/// Lightyear-replicated `(DroppedItem, DroppedItemTransform)` entities.
/// Spawn missing ones (rate-limited to
/// [`MAX_DROPPED_ITEM_SPAWNS_PER_FRAME`]), retarget interpolation on
/// real transform updates (`Ref::last_changed()` as the per-id tick),
/// despawn any that left the AoI ring.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_dropped_items_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    prediction: Res<PredictionState>,
    assets: Res<ItemVisualAssets>,
    mut entities: ResMut<DroppedItemEntities>,
    mut dropped_entities: Query<
        (&Transform, &mut DroppedItemInterpolation),
        With<NetworkDroppedItem>,
    >,
    replicated: Query<(&DroppedItem, Ref<DroppedItemTransform>)>,
) {
    if runtime.client_id.is_none() {
        // Not connected, tear down any visuals from a prior session.
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    }

    let entities = &mut *entities;
    // Camera anchor for the per-item distance gate. Reuse the same eye
    // position the rest of the client uses (player position + EYE_HEIGHT)
    // so the gate's threshold is in metres along the line of sight.
    let camera_pos = runtime
        .local_view()
        .map(|view| Vec3::from(view.position) + Vec3::Y * crate::app::EYE_HEIGHT);
    let interp_threshold_sq =
        DROPPED_ITEM_INTERPOLATION_MAX_DISTANCE_M * DROPPED_ITEM_INTERPOLATION_MAX_DISTANCE_M;

    let mut visible_ids: HashSet<DroppedItemId> = HashSet::new();
    let mut spawn_budget = MAX_DROPPED_ITEM_SPAWNS_PER_FRAME;
    for (drop, transform) in &replicated {
        // Suppressed by an unconfirmed predicted pickup: leave it out of
        // `visible_ids` so the cleanup pass below despawns any existing
        // visual, making the item vanish instantly. If the server rejects
        // the pickup, `applied_action_seq` advances, the id un-hides, and
        // the item respawns from the still-replicated entity next frame.
        if prediction.is_dropped_hidden(drop.id) {
            continue;
        }
        visible_ids.insert(drop.id);
        let tick = transform.last_changed().get() as u64;
        let target = dropped_item_transform_from(&transform);
        if let Some(entity) = entities.0.get(&drop.id).copied() {
            if let Ok((current, mut interpolation)) = dropped_entities.get_mut(entity) {
                interpolation.retarget(tick, current, target);
                // Far items skip the per-frame blend, at horizon edge a
                // sub-100 ms lerp is invisible and we'd burn a slerp per
                // item per frame. Near items keep the smoothing so a
                // bouncing drop reads as a continuous arc.
                let far_away = camera_pos
                    .map(|camera| target.translation.distance_squared(camera) > interp_threshold_sq)
                    .unwrap_or(false);
                let visual_transform = if far_away {
                    interpolation.snap_to_target()
                } else {
                    interpolation.advance(time.delta_secs())
                };
                commands.entity(entity).insert(visual_transform);
            }
        } else {
            if spawn_budget == 0 {
                // Defer to a later frame. The replicated entity still
                // exists, so a subsequent invocation picks it up; the
                // cleanup pass below only despawns ids that left the
                // replicated set.
                continue;
            }
            spawn_budget -= 1;
            let entity = commands
                .spawn((
                    Name::new(format!("Dropped Item {}", drop.id)),
                    NetworkDroppedItem { id: drop.id },
                    DroppedItemInterpolation::new(tick, target),
                    Mesh3d(assets.dropped_mesh.clone()),
                    MeshMaterial3d(assets.dropped_material.clone()),
                    target,
                    Visibility::Visible,
                    // Dropped items are tiny ground clutter, a sun-cast
                    // shadow on a 25 cm bag adds almost no visual signal
                    // but every drop in the world pays the shadow-pass
                    // draw call. Skip it entirely; the lighting on the
                    // bag itself still tracks the sun.
                    NotShadowCaster,
                ))
                .id();
            entities.0.insert(drop.id, entity);
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
pub(crate) struct DroppedItemInterpolation {
    snapshot_tick: u64,
    from: Transform,
    to: Transform,
    elapsed: f32,
}

impl DroppedItemInterpolation {
    fn new(snapshot_tick: u64, transform: Transform) -> Self {
        Self {
            snapshot_tick,
            from: transform,
            to: transform,
            elapsed: DROPPED_ITEM_INTERPOLATION_SECONDS,
        }
    }

    fn retarget(&mut self, snapshot_tick: u64, current: &Transform, target: Transform) {
        if snapshot_tick <= self.snapshot_tick {
            return;
        }

        let distance = current.translation.distance(target.translation);
        self.from = if distance > DROPPED_ITEM_INTERPOLATION_SNAP_DISTANCE {
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
        let alpha = (self.elapsed / DROPPED_ITEM_INTERPOLATION_SECONDS).clamp(0.0, 1.0);
        Transform::from_translation(self.from.translation.lerp(self.to.translation, alpha))
            .with_rotation(self.from.rotation.slerp(self.to.rotation, alpha))
    }

    /// Skip the per-frame blend and finish at the retargeted pose. Used
    /// by the distance gate so far-away items don't burn the
    /// lerp/slerp pair every frame. Marks the interpolation as
    /// completed so a later `advance` call doesn't replay the curve.
    fn snap_to_target(&mut self) -> Transform {
        self.elapsed = DROPPED_ITEM_INTERPOLATION_SECONDS;
        self.from = self.to;
        self.to
    }
}

fn dropped_item_transform_from(transform: &DroppedItemTransform) -> Transform {
    Transform::from_xyz(
        transform.position.x,
        transform.position.y,
        transform.position.z,
    )
    .with_rotation(dropped_item_rotation(transform.rotation, transform.yaw))
}

fn dropped_item_rotation(rotation: QuatNet, fallback_yaw: f32) -> Quat {
    let len_sq = rotation.x.mul_add(
        rotation.x,
        rotation.y.mul_add(
            rotation.y,
            rotation.z.mul_add(rotation.z, rotation.w * rotation.w),
        ),
    );
    if len_sq.is_finite() && len_sq > f32::EPSILON {
        Quat::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w).normalize()
    } else {
        Quat::from_rotation_y(fallback_yaw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_item_interpolation_blends_between_snapshot_targets() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(4.0, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let mut interpolation = DroppedItemInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let halfway = interpolation.advance(DROPPED_ITEM_INTERPOLATION_SECONDS * 0.5);

        assert!((halfway.translation.x - 2.0).abs() < 0.001);
        assert!(halfway.rotation.angle_between(current.rotation) > 0.1);
        assert!(halfway.rotation.angle_between(target.rotation) > 0.1);
    }

    #[test]
    fn dropped_item_interpolation_snaps_extreme_corrections() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(DROPPED_ITEM_INTERPOLATION_SNAP_DISTANCE + 1.0, 0.0, 0.0);
        let mut interpolation = DroppedItemInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let corrected = interpolation.advance(0.0);

        assert_eq!(corrected.translation, target.translation);
    }

    #[test]
    fn retarget_ignores_stale_or_equal_snapshot_ticks() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let mut interpolation = DroppedItemInterpolation::new(5, current);
        let target = Transform::from_xyz(2.0, 0.0, 0.0);

        // A tick at or below the stored tick is a stale duplicate, it must
        // not retarget the blend.
        interpolation.retarget(5, &current, target);
        let after_equal = interpolation.advance(DROPPED_ITEM_INTERPOLATION_SECONDS);
        assert_eq!(after_equal.translation, current.translation);

        interpolation.retarget(3, &current, target);
        let after_stale = interpolation.advance(DROPPED_ITEM_INTERPOLATION_SECONDS);
        assert_eq!(after_stale.translation, current.translation);
    }

    #[test]
    fn snap_to_target_finishes_immediately_and_stops_replaying() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(3.0, 0.0, 0.0);
        let mut interpolation = DroppedItemInterpolation::new(1, current);
        interpolation.retarget(2, &current, target);

        let snapped = interpolation.snap_to_target();
        assert_eq!(snapped.translation, target.translation);

        // After a snap, a subsequent advance must not slide back through the
        // curve, the interpolation is marked complete.
        let next = interpolation.advance(DROPPED_ITEM_INTERPOLATION_SECONDS * 0.5);
        assert_eq!(next.translation, target.translation);
    }

    #[test]
    fn rotation_uses_quaternion_when_finite_and_nonzero() {
        // A valid normalised quaternion is used directly (renormalised).
        let q = QuatNet {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        let rot = dropped_item_rotation(q, 1.23);
        assert!(rot.dot(Quat::IDENTITY).abs() > 1.0 - 1e-5);
    }

    #[test]
    fn rotation_falls_back_to_yaw_for_degenerate_quaternion() {
        // An all-zero quaternion has zero length, so we fall back to the
        // yaw-only rotation rather than producing a NaN quaternion.
        let zero = QuatNet {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 0.0,
        };
        let yaw = std::f32::consts::FRAC_PI_2;
        let rot = dropped_item_rotation(zero, yaw);
        let expected = Quat::from_rotation_y(yaw);
        assert!(rot.dot(expected).abs() > 1.0 - 1e-5);
    }

    #[test]
    fn transform_from_dropped_carries_position() {
        let dt = DroppedItemTransform {
            position: crate::protocol::Vec3Net::new(1.0, 2.0, 3.0),
            rotation: QuatNet {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            yaw: 0.0,
        };
        let transform = dropped_item_transform_from(&dt);
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
    }
}
