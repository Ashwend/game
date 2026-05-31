//! Client-only furnace fire visuals.
//!
//! When a furnace's replicated `DeployableActive` flag flips on,
//! `apply_deployed_entities_system` (see `deployables.rs`) attaches a fire rig
//! — a flickering ember `PointLight` — as a child of the furnace structure, and
//! tears it down when the furnace goes cold. This module owns the rig, the
//! per-frame light flicker, and the particles it sheds while lit: a dense bed
//! of small rising flame puffs that build into a soft glowing flame through
//! additive blending, plus the occasional higher-flying ember.
//!
//! Kept out of the deployable reconciler so the particle/animation concern
//! doesn't bloat the entity-diffing system. A fire rig only exists for lit
//! furnaces inside the AoI (a handful at most), so the per-frame iteration
//! here is negligible — unlike the resource-node path this never approaches
//! AoI scale, which is why a plain query walk is fine.

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{app::scene::FurnaceFireAssets, items::DeployableKind, util::hash::hashed_unit};

/// Base lumen output of the furnace mouth light, before the per-frame flicker
/// multiplies it up and down.
const FURNACE_LIGHT_BASE_INTENSITY: f32 = 5_500.0;
/// Seconds between flame-puff emissions. Each emission drops several puffs, so
/// the flame stays dense without one giant particle.
const FLAME_INTERVAL: f32 = 0.03;
/// How many flame puffs each emission drops.
const FLAME_PER_EMISSION: u32 = 3;
/// Seconds between ember emissions — far sparser than the flame, so embers read
/// as occasional flecks rising off the fire.
const SPARK_INTERVAL: f32 = 0.11;

/// Marker + emitter state for a furnace's fire rig. Lives on a child entity of
/// the furnace structure so it inherits the mouth offset and yaw, carries the
/// `PointLight`, and sheds particles while alive.
#[derive(Component)]
pub(crate) struct FurnaceFire {
    /// Seconds until the next flame-puff emission.
    flame_cooldown: f32,
    /// Seconds until the next ember emission.
    spark_cooldown: f32,
    /// Free-running phase offset so each furnace flickers out of sync with its
    /// neighbours instead of pulsing in lockstep.
    phase: f32,
}

/// A single fire particle — a flame puff or an ember. Lofts up, drifts,
/// shrinks, then despawns. Both kinds share the same integration; only their
/// spawn parameters and material differ.
#[derive(Component)]
pub(crate) struct FurnaceParticle {
    velocity: Vec3,
    /// Downward pull in m/s². Low for buoyant flame puffs, higher for embers
    /// that arc and fall.
    gravity: f32,
    /// Per-second horizontal velocity decay so outward drift bleeds off.
    drag: f32,
    age: f32,
    lifetime: f32,
    initial_scale: f32,
}

/// Spawn or tear down a furnace's fire rig to match its replicated `active`
/// flag. Called once per furnace from the deployable reconciler, which already
/// holds the local visual parent and the replicated `DeployableActive` value.
/// `existing_fire` is the rig child currently parented to `parent_entity`, if
/// any.
pub(crate) fn sync_furnace_fire(
    commands: &mut Commands,
    parent_entity: Entity,
    kind: DeployableKind,
    active: bool,
    existing_fire: Option<Entity>,
) {
    let is_furnace = matches!(kind, DeployableKind::Furnace { .. });
    match (is_furnace && active, existing_fire) {
        (true, None) => {
            // Phase the flicker off the parent's id so neighbouring furnaces
            // never breathe in unison.
            let phase = hashed_unit(parent_entity.to_bits() as u32) * std::f32::consts::TAU;
            commands.entity(parent_entity).with_children(|parent| {
                parent.spawn((
                    Name::new("Furnace Fire"),
                    FurnaceFire {
                        // Hold off one interval so the rig's GlobalTransform
                        // propagates before the first particle — otherwise the
                        // first puff would emit from the world origin.
                        flame_cooldown: FLAME_INTERVAL,
                        spark_cooldown: SPARK_INTERVAL,
                        phase,
                    },
                    PointLight {
                        // Saturated ember glow — warm enough to read as fire,
                        // dim enough not to wash out the scene at night when
                        // several furnaces might be lit.
                        color: Color::srgb(1.0, 0.62, 0.28),
                        intensity: FURNACE_LIGHT_BASE_INTENSITY,
                        range: 4.5,
                        radius: 0.10,
                        shadows_enabled: false,
                        ..default()
                    },
                    // Local offset matches the furnace mouth: low in the cavity
                    // where the coal bed sits, a little in front of the loading
                    // lip. The parent yaw rotates this so the fire always sits
                    // at the mouth.
                    Transform::from_xyz(0.0, 0.30, 0.36),
                    Visibility::Visible,
                ));
            });
        }
        (false, Some(fire_entity)) => {
            commands.entity(fire_entity).despawn();
        }
        // Already in the right state — leave it.
        _ => {}
    }
}

/// Per-frame work for every lit furnace: jitter the mouth light and shed flame
/// puffs + embers on their own cadences. Iterates only the live fire rigs (lit
/// furnaces in AoI), so it stays cheap.
pub(crate) fn animate_furnace_fire_system(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<FurnaceFireAssets>>,
    mut fires: Query<(&GlobalTransform, &mut FurnaceFire, &mut PointLight)>,
) {
    let Some(assets) = assets else {
        return;
    };
    let dt = time.delta_secs().max(0.0);
    let t = time.elapsed_secs();

    for (global, mut fire, mut light) in &mut fires {
        let flicker = furnace_flicker(t, fire.phase);
        light.intensity = FURNACE_LIGHT_BASE_INTENSITY * (0.7 + 0.55 * flicker);

        let anchor = global.translation();
        let base_seed = t.to_bits() ^ fire.phase.to_bits();

        fire.flame_cooldown -= dt;
        if fire.flame_cooldown <= 0.0 {
            fire.flame_cooldown += FLAME_INTERVAL;
            for i in 0..FLAME_PER_EMISSION {
                let seed = base_seed
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(i.wrapping_mul(2_246_822_519));
                spawn_flame(&mut commands, &assets, anchor, seed);
            }
        }

        fire.spark_cooldown -= dt;
        if fire.spark_cooldown <= 0.0 {
            fire.spark_cooldown += SPARK_INTERVAL;
            spawn_spark(&mut commands, &assets, anchor, base_seed ^ 0x1234_5678);
        }
    }
}

/// Integrate live fire particles: loft, drift, shrink, and despawn at end of
/// life. Shared by flame puffs and embers.
pub(crate) fn tick_furnace_particles_system(
    mut commands: Commands,
    time: Res<Time>,
    mut particles: Query<(Entity, &mut Transform, &mut FurnaceParticle)>,
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
        particle.velocity.y -= particle.gravity * dt;
        let drag = (1.0 - particle.drag * dt).max(0.0);
        particle.velocity.x *= drag;
        particle.velocity.z *= drag;
        transform.translation += particle.velocity * dt;

        // Shrink to nothing over life so the particle twinkles out instead of
        // popping.
        let life_t = (particle.age / particle.lifetime).clamp(0.0, 1.0);
        transform.scale = Vec3::splat((particle.initial_scale * (1.0 - life_t)).max(0.0));
    }
}

/// Sum a few detuned sine waves into a `[0, 1]` flicker that never reads as a
/// clean pulse.
fn furnace_flicker(t: f32, phase: f32) -> f32 {
    let a = (t * 11.0 + phase).sin();
    let b = (t * 19.0 + phase * 1.7).sin();
    let c = (t * 31.0 + phase * 2.3).sin();
    (0.5 + 0.30 * a + 0.14 * b + 0.06 * c).clamp(0.0, 1.0)
}

/// A buoyant flame puff: born low across a small disc, rising a short way and
/// fading fast. Many overlapping puffs blend additively into the body of the
/// flame.
fn spawn_flame(commands: &mut Commands, assets: &FurnaceFireAssets, anchor: Vec3, seed: u32) {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x9E37_79B9);
    let r3 = hashed_unit(seed ^ 0x85EB_CA6B);

    let angle = r1 * std::f32::consts::TAU;
    let radius = r2 * 0.11;
    let offset = Vec3::new(
        angle.cos() * radius,
        (r3 - 0.5) * 0.04,
        angle.sin() * radius,
    );
    // Mostly straight up, with a slight outward lean so the column tapers as
    // it climbs rather than rising as a straight tube.
    let outward = Vec3::new(angle.cos(), 0.0, angle.sin()) * (r1 * 0.12);
    let rise = 0.55 + r3 * 0.7;
    let velocity = Vec3::Y * rise + outward;
    let initial_scale = 0.8 + r2 * 0.9;
    let lifetime = 0.30 + r1 * 0.28;

    commands.spawn((
        Name::new("Furnace Flame"),
        FurnaceParticle {
            velocity,
            gravity: 0.3,
            drag: 0.8,
            age: 0.0,
            lifetime,
            initial_scale,
        },
        Mesh3d(assets.flame_mesh.clone()),
        MeshMaterial3d(assets.flame_material.clone()),
        Transform::from_translation(anchor + offset).with_scale(Vec3::splat(initial_scale)),
        Visibility::Visible,
        NotShadowCaster,
    ));
}

/// A single rising ember — higher, longer-lived, and heavier than a flame puff
/// so it arcs up off the fire and cools as it falls.
fn spawn_spark(commands: &mut Commands, assets: &FurnaceFireAssets, anchor: Vec3, seed: u32) {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x27D4_EB2F);
    let r3 = hashed_unit(seed ^ 0x1656_67B1);

    let offset = Vec3::new((r1 - 0.5) * 0.14, (r2 - 0.5) * 0.05, (r3 - 0.5) * 0.14);
    let drift = Vec3::new((r2 - 0.5) * 0.5, 0.0, (r1 - 0.5) * 0.5);
    let rise = 1.0 + r3 * 1.1;
    let velocity = drift + Vec3::Y * rise;
    let initial_scale = 0.6 + r2 * 0.7;
    let lifetime = 0.5 + r1 * 0.5;

    commands.spawn((
        Name::new("Furnace Spark"),
        FurnaceParticle {
            velocity,
            gravity: 1.6,
            drag: 1.6,
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
    fn flicker_stays_in_unit_range() {
        // Sample across time + phase; the summed sines must never leave [0, 1].
        for i in 0..200 {
            let t = i as f32 * 0.05;
            let phase = (i as f32 * 0.37) % std::f32::consts::TAU;
            let f = furnace_flicker(t, phase);
            assert!((0.0..=1.0).contains(&f), "flicker {f} out of range");
        }
    }

    #[test]
    fn flicker_actually_varies_over_time() {
        let phase = 1.3;
        let a = furnace_flicker(0.0, phase);
        let b = furnace_flicker(0.5, phase);
        assert!(
            (a - b).abs() > 1e-3,
            "flicker should change as time advances"
        );
    }
}
