//! Client-only explosion feedback VFX, driven by [`ExplosionEvent`] (raised from
//! the `ServerMessage::Explosion` cue in the network tick).
//!
//! An explosion's authoritative outcome (player damage, structure destruction)
//! already lands through the replicated mirrors; this is purely the cosmetic
//! feedback stack the feel spec calls for:
//!
//! - a burst of **debris shards** (the shared impact-shard mesh on tinted
//!   materials, dark smoke-grey mixed with ember-orange), thrown radially,
//! - a brief bright **flash** (a small emissive sphere that lives 2 to 3 frames),
//!   and
//! - a short-lived **smoke puff cluster** that rises and fades.
//!
//! The camera shake and the audio (thump + distance-delayed rumble) are raised
//! alongside this in the network tick; this module owns only the spawned VFX
//! entities and their tick-down.

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{items::ExplosiveKind, util::hash::hashed_unit};

use super::effects::ImpactChip;

/// A detonation happened at `position`; spawn its VFX. Raised from the network
/// tick on a `ServerMessage::Explosion` cue and consumed by
/// [`spawn_explosion_effects_system`]. Carries the kind so bigger charges can
/// throw a bigger burst.
#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct ExplosionEvent {
    pub(crate) position: Vec3,
    pub(crate) kind: ExplosiveKind,
    /// Seed for the hashed particle spread, from the blast position so the burst
    /// is deterministic per event (two clients render the same shape).
    pub(crate) seed: u32,
}

/// Number of debris shards a charge of each kind throws. Bigger charges throw a
/// bigger burst; client-feel counts, tuned here beside the system that spawns
/// them (not gameplay balance).
const fn shard_count(kind: ExplosiveKind) -> u32 {
    match kind {
        ExplosiveKind::PowderBomb => 14,
        ExplosiveKind::PowderKeg => 22,
        ExplosiveKind::SatchelCharge => 28,
    }
}

/// How many smoke puffs rise from the blast, per kind.
const fn smoke_count(kind: ExplosiveKind) -> u32 {
    match kind {
        ExplosiveKind::PowderBomb => 6,
        ExplosiveKind::PowderKeg => 9,
        ExplosiveKind::SatchelCharge => 11,
    }
}

/// The charge's authoritative blast radius (metres), scaling the fireball,
/// shockwave ring, and flash so a satchel visibly outclasses a bomb. Falls back
/// to a sane default if the registry row is ever missing.
fn blast_radius_m(kind: ExplosiveKind) -> f32 {
    crate::items::item_definition(kind.item_id())
        .and_then(|def| def.explosive)
        .map(|e| e.radius_m)
        .unwrap_or(3.5)
}

/// Lifetime of the bright detonation flash sphere, in seconds. A hard, brief
/// pop of light at ground zero.
const FLASH_SECONDS: f32 = 0.06;
/// Peak radius of the flash sphere as a fraction of the blast radius.
const FLASH_RADIUS_FRACTION: f32 = 0.30;
/// Lifetime of the expanding fireball, in seconds: a fast roil that hands the
/// scene over to the smoke.
const FIREBALL_SECONDS: f32 = 0.34;
/// Peak fireball radius as a fraction of the blast radius.
const FIREBALL_RADIUS_FRACTION: f32 = 0.45;
/// Lifetime of the ground shockwave ring, in seconds.
const RING_SECONDS: f32 = 0.38;
/// Peak ring radius as a fraction of the blast radius (slightly past it: the
/// pressure wave outruns the fire).
const RING_RADIUS_FRACTION: f32 = 1.15;

/// Mesh + material handles for the explosion VFX. Built once in `setup_scene`.
#[derive(Resource, Clone)]
pub(crate) struct ExplosionEffectAssets {
    /// The shared angular debris shard (reuses the impact stone-shard silhouette).
    pub(crate) shard_mesh: Handle<Mesh>,
    /// Dark smoke-grey debris material.
    pub(crate) shard_grey_material: Handle<StandardMaterial>,
    /// Ember-orange debris material, mixed in for the hot look.
    pub(crate) shard_ember_material: Handle<StandardMaterial>,
    /// Bright additive flash sphere at ground zero.
    pub(crate) flash_mesh: Handle<Mesh>,
    pub(crate) flash_material: Handle<StandardMaterial>,
    /// Soft dark smoke puff (an ico sphere on a translucent grey material).
    pub(crate) smoke_mesh: Handle<Mesh>,
    pub(crate) smoke_material: Handle<StandardMaterial>,
    /// Roiling additive fireball sphere (hot orange, dimmer than the flash so
    /// it reads as fire, not light).
    pub(crate) fireball_material: Handle<StandardMaterial>,
    /// Flat shockwave ring (a thin torus) racing out along the ground.
    pub(crate) ring_mesh: Handle<Mesh>,
    pub(crate) ring_material: Handle<StandardMaterial>,
}

/// The brief detonation flash: a bright emissive sphere that scales up then
/// despawns after a few frames.
#[derive(Component)]
pub(crate) struct ExplosionFlash {
    age: f32,
    /// Peak radius, in metres (scaled by the charge's blast radius).
    peak_radius: f32,
}

/// The expanding fireball: grows fast to its peak, then collapses as the smoke
/// takes over.
#[derive(Component)]
pub(crate) struct ExplosionFireball {
    age: f32,
    peak_radius: f32,
}

/// The ground shockwave ring: a flat torus racing outward, gone in a blink.
#[derive(Component)]
pub(crate) struct ExplosionRing {
    age: f32,
    peak_radius: f32,
}

/// A rising smoke puff that lofts, grows, and fades to nothing.
#[derive(Component)]
pub(crate) struct ExplosionSmoke {
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    initial_scale: f32,
    final_scale: f32,
}

/// Consume [`ExplosionEvent`]s and spawn the flash, debris shards, and smoke
/// cluster for each.
pub(crate) fn spawn_explosion_effects_system(
    mut commands: Commands,
    assets: Option<Res<ExplosionEffectAssets>>,
    mut events: MessageReader<ExplosionEvent>,
) {
    let Some(assets) = assets else {
        // Drain so a late asset load does not replay a backlog.
        events.read().count();
        return;
    };
    for event in events.read() {
        spawn_explosion_burst(
            &mut commands,
            &assets,
            event.position,
            event.kind,
            event.seed,
        );
    }
}

/// Spawn the full VFX stack for one blast.
pub(crate) fn spawn_explosion_burst(
    commands: &mut Commands,
    assets: &ExplosionEffectAssets,
    center: Vec3,
    kind: ExplosiveKind,
    seed: u32,
) {
    let radius = blast_radius_m(kind);

    // Flash: one bright sphere at the centre, a couple of frames.
    commands.spawn((
        Name::new("Explosion Flash"),
        ExplosionFlash {
            age: 0.0,
            peak_radius: radius * FLASH_RADIUS_FRACTION,
        },
        Mesh3d(assets.flash_mesh.clone()),
        MeshMaterial3d(assets.flash_material.clone()),
        Transform::from_translation(center).with_scale(Vec3::splat(0.2)),
        Visibility::Visible,
        NotShadowCaster,
    ));

    // Fireball: a hot additive sphere that roils out to near half the blast
    // radius and collapses into the smoke. The flash pops inside it, the
    // debris flies through it; together they read as a real detonation rather
    // than a sparkle.
    commands.spawn((
        Name::new("Explosion Fireball"),
        ExplosionFireball {
            age: 0.0,
            peak_radius: radius * FIREBALL_RADIUS_FRACTION,
        },
        Mesh3d(assets.flash_mesh.clone()),
        MeshMaterial3d(assets.fireball_material.clone()),
        Transform::from_translation(center + Vec3::Y * 0.25).with_scale(Vec3::splat(0.15)),
        Visibility::Visible,
        NotShadowCaster,
    ));

    // Shockwave ring: a flat torus racing outward along the ground, slightly
    // past the blast radius, gone in a blink. Sells the pressure wave.
    commands.spawn((
        Name::new("Explosion Ring"),
        ExplosionRing {
            age: 0.0,
            peak_radius: radius * RING_RADIUS_FRACTION,
        },
        Mesh3d(assets.ring_mesh.clone()),
        MeshMaterial3d(assets.ring_material.clone()),
        Transform::from_translation(center + Vec3::Y * 0.12).with_scale(Vec3::splat(0.2)),
        Visibility::Visible,
        NotShadowCaster,
    ));

    // Debris shards thrown radially, roughly half grey half ember.
    let shards = shard_count(kind);
    for i in 0..shards {
        let s = seed
            .wrapping_mul(2_654_435_761)
            .wrapping_add(i.wrapping_mul(374_761_393));
        let r1 = hashed_unit(s);
        let r2 = hashed_unit(s ^ 0xDEAD_BEEF);
        let r3 = hashed_unit(s ^ 0x00C0_FFEE);
        // Spread across the full sphere biased upward.
        let angle = (i as f32 / shards as f32) * std::f32::consts::TAU + r1 * 0.7;
        let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
        let up = 0.4 + r2 * 0.9;
        let speed = 3.5 + r3 * 3.5;
        let velocity = (radial * (0.7 + r1 * 0.6) + Vec3::Y * up).normalize_or_zero() * speed;
        let spin_axis = Vec3::new(r1 * 2.0 - 1.0, r2 * 2.0 - 1.0, r3 * 2.0 - 1.0)
            .normalize_or_zero()
            .max(Vec3::new(0.001, 1.0, 0.001));
        let material = if i % 2 == 0 {
            assets.shard_grey_material.clone()
        } else {
            assets.shard_ember_material.clone()
        };
        commands.spawn((
            Name::new("Explosion Debris"),
            ImpactChip::new(
                velocity,
                spin_axis,
                12.0 + r1 * 18.0,
                0.55 + r2 * 0.5,
                0.9 + r3 * 0.7,
                1.4,
            ),
            Mesh3d(assets.shard_mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(center + Vec3::Y * 0.2)
                .with_scale(Vec3::splat(0.9 + r3 * 0.7)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }

    // Smoke puffs: a small cluster that rises and expands.
    let puffs = smoke_count(kind);
    for i in 0..puffs {
        let s = seed
            .wrapping_add(0x5152_5354)
            .wrapping_mul(2_246_822_519)
            .wrapping_add(i.wrapping_mul(3_266_489_917));
        let r1 = hashed_unit(s);
        let r2 = hashed_unit(s ^ 0x1357_9BDF);
        let r3 = hashed_unit(s ^ 0x2468_ACE0);
        let offset = Vec3::new((r1 - 0.5) * 0.9, r2 * 0.4, (r3 - 0.5) * 0.9);
        let drift = Vec3::new((r1 - 0.5) * 0.5, 0.6 + r2 * 0.6, (r3 - 0.5) * 0.5);
        let initial_scale = 0.5 + r2 * 0.4;
        let final_scale = initial_scale * (2.4 + r1 * 1.4);
        commands.spawn((
            Name::new("Explosion Smoke"),
            ExplosionSmoke {
                velocity: drift,
                age: 0.0,
                lifetime: 0.8 + r3 * 0.7,
                initial_scale,
                final_scale,
            },
            Mesh3d(assets.smoke_mesh.clone()),
            MeshMaterial3d(assets.smoke_material.clone()),
            Transform::from_translation(center + Vec3::Y * 0.3 + offset)
                .with_scale(Vec3::splat(initial_scale)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }
}

/// The shockwave-ring query, disjoint from the flash/fireball queries (each
/// component lives on its own entity, but the `&mut Transform` access needs
/// the `Without` proofs). Named to keep the system signature readable.
type ExplosionRingQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Transform, &'static mut ExplosionRing),
    (Without<ExplosionFlash>, Without<ExplosionFireball>),
>;

/// Grow-then-pop the flash sphere over a few frames, then despawn it. Also
/// advances the fireball (fast roil out, collapse) and the shockwave ring
/// (race out flat, vanish); all three are simple scale animations on shared
/// additive materials, so one system owns the whole "burst core" family.
pub(crate) fn tick_explosion_flash_system(
    mut commands: Commands,
    time: Res<Time>,
    mut flashes: Query<(Entity, &mut Transform, &mut ExplosionFlash)>,
    mut fireballs: Query<(Entity, &mut Transform, &mut ExplosionFireball), Without<ExplosionFlash>>,
    mut rings: ExplosionRingQuery,
) {
    let dt = time.delta_secs().max(0.0);
    for (entity, mut transform, mut flash) in &mut flashes {
        flash.age += dt;
        let t = (flash.age / FLASH_SECONDS).clamp(0.0, 1.0);
        if flash.age >= FLASH_SECONDS {
            commands.entity(entity).despawn();
            continue;
        }
        // Fast expand to full radius then it despawns; a hard bright pop.
        transform.scale = Vec3::splat(flash.peak_radius * (0.2 + 0.8 * t));
    }
    for (entity, mut transform, mut fireball) in &mut fireballs {
        fireball.age += dt;
        if fireball.age >= FIREBALL_SECONDS {
            commands.entity(entity).despawn();
            continue;
        }
        let t = (fireball.age / FIREBALL_SECONDS).clamp(0.0, 1.0);
        // Roil out fast (ease-out) for the first ~70% of its life, then
        // collapse as the smoke takes over.
        let grow = 1.0 - (1.0 - (t / 0.7).min(1.0)).powi(2);
        let collapse = ((t - 0.7) / 0.3).clamp(0.0, 1.0);
        let scale = fireball.peak_radius * grow * (1.0 - collapse * collapse);
        transform.scale = Vec3::splat(scale.max(0.01));
    }
    for (entity, mut transform, mut ring) in &mut rings {
        ring.age += dt;
        if ring.age >= RING_SECONDS {
            commands.entity(entity).despawn();
            continue;
        }
        let t = (ring.age / RING_SECONDS).clamp(0.0, 1.0);
        // Race out with an ease-out (pressure waves start fastest), staying
        // flat: the torus scales in X/Z, barely in Y.
        let eased = 1.0 - (1.0 - t) * (1.0 - t);
        let r = ring.peak_radius * (0.1 + 0.9 * eased);
        transform.scale = Vec3::new(r, 1.0, r);
    }
}

/// Loft, expand, and fade the smoke puffs, then despawn at end of life.
pub(crate) fn tick_explosion_smoke_system(
    mut commands: Commands,
    time: Res<Time>,
    mut smoke: Query<(Entity, &mut Transform, &mut ExplosionSmoke)>,
) {
    let dt = time.delta_secs().max(0.0);
    if dt == 0.0 {
        return;
    }
    for (entity, mut transform, mut puff) in &mut smoke {
        puff.age += dt;
        if puff.age >= puff.lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        let t = (puff.age / puff.lifetime).clamp(0.0, 1.0);
        // Rising and slowing.
        puff.velocity *= 1.0 - (0.9 * dt).min(1.0);
        transform.translation += puff.velocity * dt;
        // Expand from initial to final over life.
        let scale = puff.initial_scale + (puff.final_scale - puff.initial_scale) * t;
        transform.scale = Vec3::splat(scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigger_charges_throw_more_debris_and_smoke() {
        // The burst scales with the charge: the satchel throws the biggest
        // burst, a powder bomb the smallest, monotonically across the kinds.
        assert!(shard_count(ExplosiveKind::PowderBomb) < shard_count(ExplosiveKind::PowderKeg));
        assert!(shard_count(ExplosiveKind::PowderKeg) < shard_count(ExplosiveKind::SatchelCharge));
        assert!(
            smoke_count(ExplosiveKind::PowderBomb) <= smoke_count(ExplosiveKind::SatchelCharge)
        );
        // Every kind throws at least some debris and smoke, so no blast is silent
        // visually.
        for kind in ExplosiveKind::ALL {
            assert!(shard_count(*kind) > 0);
            assert!(smoke_count(*kind) > 0);
        }
    }
}
