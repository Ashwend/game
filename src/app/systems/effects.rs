use bevy::prelude::*;

use super::super::{
    scene::ImpactEffectAssets,
    state::{GatherInputState, ImpactEffectKind, PendingImpactEffect},
};

const IMPACT_GRAVITY: f32 = 5.4;

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct ImpactChip {
    velocity: Vec3,
    spin_axis: Vec3,
    spin_speed: f32,
    lifetime: f32,
    age: f32,
    initial_scale: f32,
}

pub(crate) fn spawn_impact_effects_system(
    mut commands: Commands,
    assets: Res<ImpactEffectAssets>,
    mut gather_input: ResMut<GatherInputState>,
) {
    let Some(impact) = gather_input.take_pending_impact() else {
        return;
    };
    spawn_chips(&mut commands, &assets, &impact);
}

fn spawn_chips(commands: &mut Commands, assets: &ImpactEffectAssets, impact: &PendingImpactEffect) {
    let (mesh, material, count, base_speed, lifetime, scale) = match impact.kind {
        ImpactEffectKind::WoodChips => (
            assets.wood_chip_mesh.clone(),
            assets.wood_chip_material.clone(),
            6,
            2.6,
            0.45,
            1.0,
        ),
        ImpactEffectKind::StoneShards => (
            assets.stone_shard_mesh.clone(),
            assets.stone_shard_material.clone(),
            7,
            3.0,
            0.55,
            1.0,
        ),
    };

    let outward = impact.spray_direction.normalize_or_zero();
    let outward = if outward.length_squared() < f32::EPSILON {
        Vec3::Y
    } else {
        outward
    };
    let tangent = outward.any_orthonormal_vector();
    let bitangent = outward.cross(tangent).normalize_or_zero();

    for index in 0..count {
        let seed = impact
            .seed
            .wrapping_mul(2654435761)
            .wrapping_add(index * 374761393);
        let r1 = hashed_unit(seed);
        let r2 = hashed_unit(seed.wrapping_add(0xDEADBEEF));
        let r3 = hashed_unit(seed.wrapping_add(0xC0FFEE));

        let angle = (index as f32 / count as f32) * std::f32::consts::TAU + r1 * 0.6;
        let radial = tangent * angle.cos() + bitangent * angle.sin();
        let upward = 0.85 + r2 * 0.6;
        let outward_strength = 0.6 + r3 * 0.4;
        let velocity = (radial * outward_strength + outward * 0.4 + Vec3::Y * upward) * base_speed;

        let spin_axis = Vec3::new(r1 * 2.0 - 1.0, r2 * 2.0 - 1.0, r3 * 2.0 - 1.0)
            .normalize_or_zero()
            .max(Vec3::new(0.001, 1.0, 0.001));
        let spin_speed = 10.0 + r1 * 16.0;

        let initial_scale = scale * (0.85 + r2 * 0.4);
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            r1 * std::f32::consts::TAU,
            r2 * std::f32::consts::TAU,
            r3 * std::f32::consts::TAU,
        );

        commands.spawn((
            Name::new("Impact Chip"),
            ImpactChip {
                velocity,
                spin_axis,
                spin_speed,
                lifetime,
                age: 0.0,
                initial_scale,
            },
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(impact.anchor)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(initial_scale)),
            Visibility::Visible,
        ));
    }
}

pub(crate) fn tick_impact_chips_system(
    mut commands: Commands,
    time: Res<Time>,
    mut chips: Query<(Entity, &mut Transform, &mut ImpactChip)>,
) {
    let dt = time.delta_secs().max(0.0);
    if dt == 0.0 {
        return;
    }

    for (entity, mut transform, mut chip) in &mut chips {
        if advance_chip(&mut transform, &mut chip, dt) == ChipStep::Expired {
            commands.entity(entity).despawn();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChipStep {
    Alive,
    Expired,
}

fn advance_chip(transform: &mut Transform, chip: &mut ImpactChip, dt: f32) -> ChipStep {
    chip.age += dt;
    if chip.age >= chip.lifetime {
        return ChipStep::Expired;
    }

    chip.velocity.y -= IMPACT_GRAVITY * dt;
    transform.translation += chip.velocity * dt;
    let rotation = Quat::from_axis_angle(chip.spin_axis, chip.spin_speed * dt);
    transform.rotation = rotation * transform.rotation;

    let life_t = (chip.age / chip.lifetime).clamp(0.0, 1.0);
    // Hold size most of the way, then shrink off the last 35% for a clean
    // pop-out rather than a gradual fade.
    let shrink_t = ((life_t - 0.65) / 0.35).max(0.0);
    let scale = chip.initial_scale * (1.0 - shrink_t).max(0.0);
    transform.scale = Vec3::splat(scale);
    ChipStep::Alive
}

fn hashed_unit(seed: u32) -> f32 {
    // Cheap deterministic [0, 1) value derived from an integer seed. Keeps the
    // chip spread reproducible per-swing without dragging in an RNG crate.
    let mut x = seed.wrapping_add(0x9E3779B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EBCA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2AE35);
    x ^= x >> 16;
    (x & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_unit_stays_in_unit_interval_and_varies() {
        for seed in 0..200u32 {
            let value = hashed_unit(seed);
            assert!((0.0..1.0).contains(&value));
        }
        assert_ne!(hashed_unit(1), hashed_unit(2));
        assert_ne!(hashed_unit(100), hashed_unit(101));
    }

    #[test]
    fn impact_chip_falls_and_shrinks_during_its_lifetime() {
        let mut transform = Transform::from_xyz(0.0, 1.0, 0.0);
        let mut chip = ImpactChip {
            velocity: Vec3::new(0.0, 2.0, 0.0),
            spin_axis: Vec3::Y,
            spin_speed: 5.0,
            lifetime: 0.40,
            age: 0.0,
            initial_scale: 1.0,
        };

        // Mid-life — still alive, gravity has pulled velocity down.
        assert_eq!(
            advance_chip(&mut transform, &mut chip, 0.10),
            ChipStep::Alive
        );
        assert!(chip.velocity.y < 2.0);
        assert!(transform.translation.y > 1.0);
        assert!(transform.scale.x > 0.99); // still in hold range

        // Past the shrink threshold — scale should have shrunk noticeably.
        assert_eq!(
            advance_chip(&mut transform, &mut chip, 0.25),
            ChipStep::Alive
        );
        assert!(transform.scale.x < 0.5);

        // Crossing the lifetime expires the chip.
        assert_eq!(
            advance_chip(&mut transform, &mut chip, 0.20),
            ChipStep::Expired
        );
    }
}
