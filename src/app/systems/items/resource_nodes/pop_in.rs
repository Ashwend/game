use bevy::prelude::*;

use crate::{protocol::Vec3Net, resources::ResourceNodeModel};

use super::ResourceNodePopIn;

/// How long the "node emerges from the ground" animation runs. Short
/// enough to feel like a pop rather than a slow grow, long enough to
/// register as something happening.
pub(super) const POP_IN_DURATION_SECS: f32 = 0.42;
/// How far below the floor the node starts on emerge. The mesh's bottom
/// sits at local y=0 so this pulls the rock/sapling fully into the
/// ground at t=0, then lifts back to flush.
const POP_IN_GROUND_OFFSET: f32 = 0.55;
/// Peak overshoot scale during the emergence pulse. The node briefly
/// pops slightly above its target size then settles, giving a "landed"
/// feel rather than a linear ramp.
const POP_IN_OVERSHOOT: f32 = 0.06;

/// Drives the "emerge from the ground" animation attached to freshly
/// (re)spawned resource nodes. Removes the component once the curve
/// settles, after which the entity returns to snapshot-driven transforms.
pub(crate) fn tick_resource_node_pop_in_system(
    mut commands: Commands,
    time: Res<Time>,
    mut popping_in: Query<(Entity, &mut Transform, &mut ResourceNodePopIn)>,
) {
    let dt = time.delta_secs().clamp(0.0, 0.1);
    if dt == 0.0 {
        return;
    }
    for (entity, mut transform, mut pop_in) in &mut popping_in {
        pop_in.elapsed += dt;
        let finished = pop_in.elapsed >= POP_IN_DURATION_SECS;
        *transform = pop_in_transform(pop_in.base_transform, pop_in.elapsed);
        if finished {
            commands.entity(entity).remove::<ResourceNodePopIn>();
        }
    }
}

/// Pure math behind the pop-in transform. Pulled out of the system so
/// it can be exercised without spinning up a Bevy world.
pub(super) fn pop_in_transform(base: Transform, elapsed: f32) -> Transform {
    let raw = (elapsed / POP_IN_DURATION_SECS).clamp(0.0, 1.0);
    if raw >= 1.0 {
        return base;
    }
    let ease = ease_out_cubic(raw);
    // Lift from below the floor to flush, with a brief overshoot beyond
    // unit scale that settles back to 1.0 — reads as the node "thudding"
    // into place rather than easing to a stop.
    let height = -POP_IN_GROUND_OFFSET * (1.0 - ease);
    let overshoot = if raw < 0.7 {
        POP_IN_OVERSHOOT * (raw / 0.7)
    } else {
        POP_IN_OVERSHOOT * (1.0 - (raw - 0.7) / 0.3)
    };
    let scale_factor = ease + overshoot * (raw * (1.0 - raw) * 4.0);
    let mut next = base;
    next.translation.y = base.translation.y + height;
    next.scale = base.scale * scale_factor.max(0.0);
    next
}

pub(super) fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

pub(crate) fn resource_node_transform_at(
    position: Vec3Net,
    yaw: f32,
    model: ResourceNodeModel,
) -> Transform {
    // Trees bake their full size into the mesh and sit on the ground at
    // unit scale, which keeps each variant a single canonical mesh that
    // can be GPU-instanced. Ore nodes keep their per-instance scale
    // jitter for shape variety. Both the tree trunks and the ore rock
    // lumps have their lowest vertices at local y=0, so no height offset
    // is needed — adding one would float the geometry above the floor.
    let (height_offset, scale) = match model {
        ResourceNodeModel::CoalOre => (0.0, Vec3::new(1.0, 1.0, 1.0)),
        ResourceNodeModel::IronOre => (0.0, Vec3::new(1.1, 1.05, 0.95)),
        ResourceNodeModel::SulfurOre => (0.0, Vec3::new(0.96, 0.92, 1.06)),
        // Stone veins are wider/flatter than ore mounds — they read as
        // an outcrop rather than a focused deposit.
        ResourceNodeModel::StoneVein => (0.0, Vec3::new(1.18, 0.86, 1.08)),
        ResourceNodeModel::PineTreeSmall
        | ResourceNodeModel::PineTreeMedium
        | ResourceNodeModel::PineTreeLarge
        | ResourceNodeModel::BirchTreeSmall
        | ResourceNodeModel::BirchTreeMedium
        | ResourceNodeModel::BirchTreeLarge => (0.0, Vec3::ONE),
        ResourceNodeModel::SurfaceStone => (0.0, Vec3::new(0.9, 0.9, 0.9)),
        ResourceNodeModel::BranchPile => (0.0, Vec3::ONE),
        ResourceNodeModel::HayGrass => (0.0, Vec3::ONE),
    };
    Transform::from_xyz(position.x, position.y + height_offset, position.z)
        .with_rotation(Quat::from_rotation_y(yaw))
        .with_scale(scale)
}
