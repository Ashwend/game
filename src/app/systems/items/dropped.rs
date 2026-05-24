use std::collections::{HashMap, HashSet};

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        scene::{ItemVisualAssets, NetworkDroppedItem},
        state::ClientRuntime,
    },
    protocol::{DroppedItemId, DroppedWorldItem, QuatNet},
};

const DROPPED_ITEM_INTERPOLATION_SECONDS: f32 = 0.1;
const DROPPED_ITEM_INTERPOLATION_SNAP_DISTANCE: f32 = 6.0;
/// Per-frame cap on fresh dropped-item spawns. A chest spill or player
/// death can produce a burst of dozens of drops in a single snapshot; if
/// every visual entity is created the same frame the snapshot lands the
/// command-buffer flush stalls the main thread. Updates to existing
/// drops and despawns are uncapped — only first-time spawns are
/// budgeted. The remainder appears the next frame; at 50 ms snapshot
/// cadence and 16-per-frame, even a 200-item burst drains in under a
/// snapshot interval.
const MAX_DROPPED_ITEM_SPAWNS_PER_FRAME: usize = 16;
/// Beyond this many metres from the camera, dropped items skip the
/// per-frame lerp/slerp blend and just snap to their target transform.
/// At that distance the sub-frame interpolation is invisible (the item
/// is a tiny bag clutched at horizon-edge), but skipping the math saves
/// a `Quat::slerp` per item per frame — meaningful when a player has
/// dropped a large pile in the same chunk.
const DROPPED_ITEM_INTERPOLATION_MAX_DISTANCE_M: f32 = 40.0;

/// Persistent `id → Entity` lookup for dropped items. Maintained
/// incrementally as items spawn and despawn so the snapshot-apply system
/// doesn't have to rebuild a `HashMap` from a `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct DroppedItemEntities(pub(crate) HashMap<DroppedItemId, Entity>);

pub(crate) fn apply_dropped_items_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    assets: Res<ItemVisualAssets>,
    mut entities: ResMut<DroppedItemEntities>,
    mut dropped_entities: Query<
        (&Transform, &mut DroppedItemInterpolation),
        With<NetworkDroppedItem>,
    >,
) {
    let Some(snapshot) = &runtime.snapshot else {
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    };

    // Single pass over the snapshot — spawn missing items, update existing
    // ones. We never rebuild the `HashMap` from a Query iteration; the
    // resource map already mirrors the live entity set.
    let snapshot_ids: HashSet<DroppedItemId> =
        snapshot.dropped_items.iter().map(|item| item.id).collect();
    let entities = &mut *entities;
    // Camera anchor for the per-item distance gate. Reuse the same eye
    // position the rest of the client uses (player position + EYE_HEIGHT)
    // so the gate's threshold is in metres along the line of sight.
    let camera_pos = runtime
        .local_view()
        .map(|view| Vec3::from(view.position) + Vec3::Y * crate::app::EYE_HEIGHT);
    let interp_threshold_sq =
        DROPPED_ITEM_INTERPOLATION_MAX_DISTANCE_M * DROPPED_ITEM_INTERPOLATION_MAX_DISTANCE_M;

    let mut spawn_budget = MAX_DROPPED_ITEM_SPAWNS_PER_FRAME;
    for item in &snapshot.dropped_items {
        let target = dropped_item_transform(item);
        if let Some(entity) = entities.0.get(&item.id).copied() {
            if let Ok((current, mut interpolation)) = dropped_entities.get_mut(entity) {
                interpolation.retarget(snapshot.tick, current, target);
                // Far items skip the per-frame blend — at horizon edge a
                // sub-100 ms lerp is invisible and we'd burn a slerp per
                // item per frame. Near items keep the smoothing so a
                // bouncing drop reads as a continuous arc.
                let far_away = camera_pos
                    .map(|camera| target.translation.distance_squared(camera) > interp_threshold_sq)
                    .unwrap_or(false);
                let transform = if far_away {
                    interpolation.snap_to_target()
                } else {
                    interpolation.advance(time.delta_secs())
                };
                commands.entity(entity).insert(transform);
            }
        } else {
            if spawn_budget == 0 {
                // Defer to a later frame. The snapshot stays valid
                // until the next server tick (~50 ms), and the cleanup
                // pass below only despawns ids that left the snapshot —
                // not ids we simply haven't spawned yet — so the item
                // is picked up by a subsequent invocation.
                continue;
            }
            spawn_budget -= 1;
            let entity = commands
                .spawn((
                    Name::new(format!("Dropped Item {}", item.id)),
                    NetworkDroppedItem { id: item.id },
                    DroppedItemInterpolation::new(snapshot.tick, target),
                    Mesh3d(assets.dropped_mesh.clone()),
                    MeshMaterial3d(assets.dropped_material.clone()),
                    target,
                    Visibility::Visible,
                    // Dropped items are tiny ground clutter — a sun-cast
                    // shadow on a 25 cm bag adds almost no visual signal
                    // but every drop in the world pays the shadow-pass
                    // draw call. Skip it entirely; the lighting on the
                    // bag itself still tracks the sun.
                    NotShadowCaster,
                ))
                .id();
            entities.0.insert(item.id, entity);
        }
    }

    entities.0.retain(|id, entity| {
        if snapshot_ids.contains(id) {
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

pub(super) fn dropped_item_transform(item: &DroppedWorldItem) -> Transform {
    Transform::from_xyz(item.position.x, item.position.y, item.position.z)
        .with_rotation(dropped_item_rotation(item.rotation, item.yaw))
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
}
