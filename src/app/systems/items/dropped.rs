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

    for item in &snapshot.dropped_items {
        let target = dropped_item_transform(item);
        if let Some(entity) = entities.0.get(&item.id).copied() {
            if let Ok((current, mut interpolation)) = dropped_entities.get_mut(entity) {
                interpolation.retarget(snapshot.tick, current, target);
                let transform = interpolation.advance(time.delta_secs());
                commands.entity(entity).insert(transform);
            }
        } else {
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
