//! Client-only torch fire visuals.
//!
//! When a torch's replicated `DeployableActive` (lit) flag is on,
//! `apply_deployed_entities_system` attaches a fire rig as a child of the torch
//! structure and tears it down when the torch burns out. The rig carries:
//!
//! - a shadowless `PointLight` with a small range, the cheapest dynamic light
//!   Bevy's clustered forward+ renderer supports ("illuminate the world, as
//!   inexpensive as possible");
//! - a sparse particle flame, emitted only while the camera is near, and
//! - a single camera-facing emissive quad (a "bright rectangle") that stands in
//!   for the particle flame at distance.
//!
//! The distance LOD is the whole point: far torches pay for one cheap quad and
//! a clustered light, not a per-frame particle stream. Only lit torches in the
//! AoI (a handful) carry a rig, so the per-frame walk here stays cheap.

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::scene::{MainCamera, TorchFireAssets},
    items::DeployableKind,
    util::hash::hashed_unit,
};

/// Base lumen output of a torch, before the per-frame flicker scales it.
/// Dimmer than the furnace (a torch is a single flame, not a forge bed).
const TORCH_LIGHT_BASE_INTENSITY: f32 = 2_300.0;
/// Light radius in metres. Small + shadowless so clustered shading culls it
/// cheaply when the lit area isn't on screen.
const TORCH_LIGHT_RANGE_M: f32 = 6.0;
/// Camera distance within which the particle flame shows; beyond it the cheap
/// billboard quad stands in for the flame instead.
const TORCH_PARTICLE_NEAR_M: f32 = 16.0;
/// Seconds between flame-puff emissions. A thin tongue, far sparser than the
/// furnace's forge bed.
const FLAME_INTERVAL: f32 = 0.05;
/// Local height of the flame + light above the torch's base, just above the
/// charred head top (the authored model's head rim sits at ~0.52).
const FLAME_ANCHOR_Y: f32 = 0.55;

/// Marker + emitter state for a torch's fire rig (child of the torch entity;
/// carries the `PointLight` and the billboard child).
#[derive(Component)]
pub(crate) struct TorchFire {
    /// Seconds until the next flame-puff emission.
    flame_cooldown: f32,
    /// Free-running flicker phase so neighbouring torches don't pulse in sync.
    phase: f32,
}

/// Marker on the billboard quad child, the distance-LOD "bright rectangle".
#[derive(Component)]
pub(crate) struct TorchBillboard;

/// One rising flame puff. Lofts up, shrinks, then despawns.
#[derive(Component)]
pub(crate) struct TorchFlameParticle {
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    initial_scale: f32,
}

/// Spawn or tear down a torch's fire rig to match its replicated lit flag.
/// Called once per torch from the deployable reconciler. `existing_fire` is the
/// rig child currently parented to `parent_entity`, if any.
pub(crate) fn sync_torch_fire(
    commands: &mut Commands,
    parent_entity: Entity,
    kind: DeployableKind,
    active: bool,
    existing_fire: Option<Entity>,
    assets: &TorchFireAssets,
) {
    let is_torch = matches!(kind, DeployableKind::Torch { .. });
    match (is_torch && active, existing_fire) {
        (true, None) => {
            let phase = hashed_unit(parent_entity.to_bits() as u32) * std::f32::consts::TAU;
            commands.entity(parent_entity).with_children(|parent| {
                parent
                    .spawn((
                        Name::new("Torch Fire"),
                        TorchFire {
                            // Hold off one interval so the rig's GlobalTransform
                            // propagates before the first particle emits.
                            flame_cooldown: FLAME_INTERVAL,
                            phase,
                        },
                        PointLight {
                            color: Color::srgb(1.0, 0.66, 0.30),
                            intensity: TORCH_LIGHT_BASE_INTENSITY,
                            range: TORCH_LIGHT_RANGE_M,
                            radius: 0.06,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(0.0, FLAME_ANCHOR_Y, 0.0),
                        Visibility::Visible,
                    ))
                    .with_children(|fire| {
                        // The far-distance billboard, hidden until the LOD
                        // system turns it on at range.
                        fire.spawn((
                            Name::new("Torch Billboard"),
                            TorchBillboard,
                            Mesh3d(assets.billboard_mesh.clone()),
                            MeshMaterial3d(assets.billboard_material.clone()),
                            Transform::default(),
                            Visibility::Hidden,
                            NotShadowCaster,
                        ));
                    });
            });
        }
        (false, Some(fire_entity)) => {
            // Despawns the rig and its billboard child (recursive by default).
            commands.entity(fire_entity).despawn();
        }
        // Already in the right state.
        _ => {}
    }
}

/// Per-frame work for every lit torch in view: flicker the light, swap the
/// particle flame for the billboard by camera distance, aim the billboard at
/// the camera, and shed flame puffs while near.
pub(crate) fn animate_torch_fire_system(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<TorchFireAssets>>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    mut fires: Query<(&GlobalTransform, &mut TorchFire, &mut PointLight, &Children)>,
    mut billboards: Query<(&mut Transform, &mut Visibility), With<TorchBillboard>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let dt = time.delta_secs().max(0.0);
    let t = time.elapsed_secs();
    let camera_pos = camera.single().map(GlobalTransform::translation).ok();

    for (global, mut fire, mut light, children) in &mut fires {
        let anchor = global.translation();
        let flicker = torch_flicker(t, fire.phase);
        light.intensity = TORCH_LIGHT_BASE_INTENSITY * (0.65 + 0.5 * flicker);

        let near = camera_pos.is_none_or(|cam| anchor.distance(cam) <= TORCH_PARTICLE_NEAR_M);

        // Billboard child: visible only at distance, yawed to face the camera.
        // The quad's world facing must ignore the torch's (possibly tilted)
        // rotation, so undo the rig's world rotation before applying the yaw.
        for &child in children {
            if let Ok((mut billboard_transform, mut visibility)) = billboards.get_mut(child) {
                *visibility = if near {
                    Visibility::Hidden
                } else {
                    Visibility::Visible
                };
                if let Some(cam) = camera_pos {
                    let to_camera = cam - anchor;
                    let yaw = to_camera.x.atan2(to_camera.z);
                    billboard_transform.rotation =
                        global.rotation().inverse() * Quat::from_rotation_y(yaw);
                }
            }
        }

        if near {
            fire.flame_cooldown -= dt;
            if fire.flame_cooldown <= 0.0 {
                fire.flame_cooldown += FLAME_INTERVAL;
                let seed = t.to_bits() ^ fire.phase.to_bits();
                spawn_torch_flame(&mut commands, &assets, anchor, seed);
            }
        }
    }
}

/// Integrate live torch flame puffs: loft, shrink, despawn at end of life.
pub(crate) fn tick_torch_particles_system(
    mut commands: Commands,
    time: Res<Time>,
    mut particles: Query<(Entity, &mut Transform, &mut TorchFlameParticle)>,
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
        // Gentle deceleration so the puff slows as it rises, then fades.
        particle.velocity.y -= 0.25 * dt;
        transform.translation += particle.velocity * dt;
        let life_t = (particle.age / particle.lifetime).clamp(0.0, 1.0);
        transform.scale = Vec3::splat((particle.initial_scale * (1.0 - life_t)).max(0.0));
    }
}

/// A buoyant flame puff born just above the torch head.
fn spawn_torch_flame(commands: &mut Commands, assets: &TorchFireAssets, anchor: Vec3, seed: u32) {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x9E37_79B9);
    let r3 = hashed_unit(seed ^ 0x85EB_CA6B);

    let offset = Vec3::new((r1 - 0.5) * 0.05, (r3 - 0.5) * 0.02, (r2 - 0.5) * 0.05);
    let drift = Vec3::new((r1 - 0.5) * 0.10, 0.0, (r2 - 0.5) * 0.10);
    let rise = 0.45 + r3 * 0.40;
    let velocity = drift + Vec3::Y * rise;
    let initial_scale = 0.7 + r2 * 0.6;
    let lifetime = 0.26 + r1 * 0.22;

    commands.spawn((
        Name::new("Torch Flame"),
        TorchFlameParticle {
            velocity,
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

/// Two detuned sines into a `[0, 1]` flicker that never reads as a clean pulse.
fn torch_flicker(t: f32, phase: f32) -> f32 {
    let a = (t * 13.0 + phase).sin();
    let b = (t * 23.0 + phase * 1.7).sin();
    (0.5 + 0.32 * a + 0.18 * b).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicker_stays_in_unit_range() {
        for i in 0..200 {
            let t = i as f32 * 0.05;
            let phase = (i as f32 * 0.37) % std::f32::consts::TAU;
            let f = torch_flicker(t, phase);
            assert!((0.0..=1.0).contains(&f), "flicker {f} out of range");
        }
    }
}
