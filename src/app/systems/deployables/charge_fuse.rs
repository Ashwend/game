//! Client-only fuse VFX/SFX for a placed explosive charge.
//!
//! Every `DeployableKind::Explosive` charge is ALWAYS armed while it exists (the
//! fuse countdown is server-only; the client treats any live charge as ticking),
//! so the reconciler attaches a fuse rig to a charge the instant it spawns and
//! the rig tears down with the charge when it fizzles or detonates (the child
//! despawns recursively with the charge root).
//!
//! The rig carries:
//!
//! - a small bright spark emitter at the charge's kind-specific fuse-tip anchor
//!   (the torch-flame particle template: hashed velocity/lifetime quads on the
//!   shared spark mesh/material), throttled to the near-camera LOD so a base full
//!   of charges does not spew particles from across the map, and
//! - a looping/retriggering fuse hiss (a spatial `PlaySound::at`, re-fired on a
//!   fixed interval while the camera is near) so a defender can hear a live
//!   charge before they see it.

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        audio::{PlaySound, SoundId},
        scene::MainCamera,
    },
    items::ExplosiveKind,
    util::hash::hashed_unit,
};

/// Camera distance within which a charge emits fuse sparks + hiss. Beyond it the
/// charge still renders but pays for no particle stream or audio, the same
/// near-LOD the torch flame uses.
const CHARGE_FUSE_NEAR_M: f32 = 20.0;
/// Seconds between spark emissions while near. A little sparser than the torch
/// flame: a fuse spits, it does not pour.
const SPARK_INTERVAL: f32 = 0.06;
/// Seconds between fuse-hiss re-fires while near. The hiss is a short sample
/// re-triggered on this cadence so it reads as a continuous hiss without a real
/// looping voice; tuned so consecutive fires overlap slightly into a steady sizzle.
const HISS_INTERVAL: f32 = 0.45;

/// The fuse-tip spark anchor for each charge kind, in the charge visual
/// ROOT's space (metres, Y up), from the P6a model report: the rope-fuse tip
/// on the keg / satchel / bomb.
///
/// Placed charges (keg, satchel) put the glb at the root, so this is plain
/// mesh space. The thrown bomb's visual root is the ball CENTER (the mesh is
/// sunk by `POWDER_BOMB_BALL_RADIUS_M` so the roll spins about the ball), so
/// its mesh-space tip (0.036, 0.415, 0) drops by the same radius here.
///
/// Pure function so the anchor-per-kind mapping is unit-testable without a
/// running app.
pub(crate) fn fuse_tip_anchor(kind: ExplosiveKind) -> Vec3 {
    match kind {
        ExplosiveKind::PowderKeg => Vec3::new(0.049, 0.755, 0.000),
        ExplosiveKind::SatchelCharge => Vec3::new(0.021, 0.485, 0.000),
        ExplosiveKind::PowderBomb => Vec3::new(
            0.036,
            0.415 - crate::game_balance::POWDER_BOMB_BALL_RADIUS_M,
            0.000,
        ),
    }
}

/// Marker + emitter state for a charge's fuse rig (child of the charge's visual
/// entity). Sits at the fuse-tip anchor and sheds sparks + hiss while near.
#[derive(Component)]
pub(crate) struct ChargeFuse {
    /// Seconds until the next spark emission.
    spark_cooldown: f32,
    /// Seconds until the next hiss re-fire.
    hiss_cooldown: f32,
    /// Free-running phase so neighbouring charges don't sparkle/pulse in sync.
    phase: f32,
}

/// One rising fuse spark. Lofts up, shrinks, then despawns. Reuses the torch
/// flame particle shape (a shared component would couple the two modules for no
/// gain; this is a handful of fields).
#[derive(Component)]
pub(crate) struct ChargeSparkParticle {
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    initial_scale: f32,
}

/// Shared spark mesh + material for the fuse rig. Built once in `setup_scene`
/// (mirrors `FurnaceFireAssets`); a bright additive ember cube.
#[derive(Resource, Clone)]
pub(crate) struct ChargeFuseAssets {
    pub(crate) spark_mesh: Handle<Mesh>,
    pub(crate) spark_material: Handle<StandardMaterial>,
}

/// Attach a charge's fuse rig (its spark emitter) under `parent_entity`. Called
/// once per charge from the deployable reconciler at spawn; the rig tears down
/// automatically when the charge root despawns (fizzle / detonation).
pub(crate) fn spawn_charge_fuse_rig(
    commands: &mut Commands,
    parent_entity: Entity,
    kind: ExplosiveKind,
) {
    let anchor = fuse_tip_anchor(kind);
    let phase = hashed_unit(parent_entity.to_bits() as u32) * std::f32::consts::TAU;
    commands.entity(parent_entity).with_children(|parent| {
        parent.spawn((
            Name::new("Charge Fuse"),
            ChargeFuse {
                // Hold off one interval so the rig's GlobalTransform propagates
                // before the first spark emits.
                spark_cooldown: SPARK_INTERVAL,
                hiss_cooldown: HISS_INTERVAL,
                phase,
            },
            Transform::from_translation(anchor),
            Visibility::Visible,
        ));
    });
}

/// Per-frame work for every charge fuse in view: shed sparks while near and
/// re-fire the hiss on its cadence.
pub(crate) fn animate_charge_fuse_system(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<ChargeFuseAssets>>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    mut play_sound: MessageWriter<PlaySound>,
    mut fuses: Query<(&GlobalTransform, &mut ChargeFuse)>,
) {
    let Some(assets) = assets else {
        return;
    };
    let dt = time.delta_secs().max(0.0);
    let t = time.elapsed_secs();
    let camera_pos = camera.single().map(GlobalTransform::translation).ok();

    for (global, mut fuse) in &mut fuses {
        let anchor = global.translation();
        let near = camera_pos.is_none_or(|cam| anchor.distance(cam) <= CHARGE_FUSE_NEAR_M);
        if !near {
            continue;
        }
        // Sparks.
        fuse.spark_cooldown -= dt;
        if fuse.spark_cooldown <= 0.0 {
            fuse.spark_cooldown += SPARK_INTERVAL;
            let seed = t.to_bits() ^ fuse.phase.to_bits();
            spawn_charge_spark(&mut commands, &assets, anchor, seed);
        }
        // Hiss: re-fire the short sample on its cadence so it reads as a steady
        // sizzle. Spatial at the fuse tip so a defender can locate the charge.
        fuse.hiss_cooldown -= dt;
        if fuse.hiss_cooldown <= 0.0 {
            fuse.hiss_cooldown += HISS_INTERVAL;
            play_sound.write(PlaySound::at(SoundId::FuseHiss, anchor));
        }
    }
}

/// Integrate live fuse sparks: loft, shrink, despawn at end of life. Mirrors the
/// torch flame tick.
pub(crate) fn tick_charge_spark_particles_system(
    mut commands: Commands,
    time: Res<Time>,
    mut particles: Query<(Entity, &mut Transform, &mut ChargeSparkParticle)>,
) {
    let dt = time.delta_secs().max(0.0);
    if dt == 0.0 {
        return;
    }
    for (entity, mut transform, mut particle) in &mut particles {
        particle.age += dt;
        if particle.age >= particle.lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        // Sparks decelerate as they rise and fall away.
        particle.velocity.y -= 1.2 * dt;
        transform.translation += particle.velocity * dt;
        let life_t = (particle.age / particle.lifetime).clamp(0.0, 1.0);
        transform.scale = Vec3::splat((particle.initial_scale * (1.0 - life_t)).max(0.0));
    }
}

/// A small bright spark born at the fuse tip. Hashed offset/velocity/lifetime so
/// each spark differs. World-space so it stays put as the charge sits.
fn spawn_charge_spark(commands: &mut Commands, assets: &ChargeFuseAssets, anchor: Vec3, seed: u32) {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x27D4_EB2F);
    let r3 = hashed_unit(seed ^ 0x1656_67B1);

    let offset = Vec3::new((r1 - 0.5) * 0.04, (r3 - 0.5) * 0.02, (r2 - 0.5) * 0.04);
    let drift = Vec3::new((r2 - 0.5) * 0.35, 0.0, (r1 - 0.5) * 0.35);
    let rise = 0.35 + r3 * 0.45;
    let velocity = drift + Vec3::Y * rise;
    let initial_scale = 0.5 + r2 * 0.6;
    let lifetime = 0.22 + r1 * 0.24;

    commands.spawn((
        Name::new("Charge Spark"),
        ChargeSparkParticle {
            velocity,
            age: 0.0,
            lifetime,
            initial_scale,
        },
        Mesh3d(assets.spark_mesh.clone()),
        MeshMaterial3d(assets.spark_material.clone()),
        Transform::from_translation(anchor + offset).with_scale(Vec3::splat(initial_scale)),
        Visibility::Visible,
        NotShadowCaster,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_charge_kind_has_a_distinct_fuse_anchor_above_the_base() {
        // Each kind's fuse tip is above the model base (positive Y) so sparks
        // rise off the top, and no two kinds share the exact same anchor (each is
        // authored to its own model). Pins the P6a-reported coordinates.
        let anchors = [
            fuse_tip_anchor(ExplosiveKind::PowderKeg),
            fuse_tip_anchor(ExplosiveKind::SatchelCharge),
            fuse_tip_anchor(ExplosiveKind::PowderBomb),
        ];
        for a in anchors {
            assert!(a.y > 0.0, "a fuse tip sits above the base");
        }
        for (i, a) in anchors.iter().enumerate() {
            for (j, b) in anchors.iter().enumerate() {
                if i != j {
                    assert!(a.distance(*b) > 1e-4, "anchors {i} and {j} must differ");
                }
            }
        }
    }
}
