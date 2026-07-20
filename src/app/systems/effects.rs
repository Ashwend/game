use bevy::{light::NotShadowCaster, prelude::*};

use super::super::{
    scene::ImpactEffectAssets,
    state::{GatherInputState, ImpactEffectKind, RemoteImpactEvent},
};
use crate::util::hash::hashed_unit;

const IMPACT_GRAVITY: f32 = 5.4;
// Approximate ground level. The world floor is a flat plane at Y=0, so
// clamping chips here lets them settle on the surface instead of falling
// through it.
const CHIP_GROUND_Y: f32 = 0.02;
// Vertical bounce restitution, chips kiss the ground rather than launching.
const CHIP_BOUNCE: f32 = 0.18;
// Horizontal friction applied per second while a chip is on the ground.
const CHIP_GROUND_FRICTION: f32 = 6.0;
// A chip "near the ground" still gets friction so its outward velocity
// bleeds off between tiny bounces, rather than skittering forever.
const CHIP_GROUND_CONTACT_BAND: f32 = 0.04;
// Fraction of a chip's spin that survives each ground bounce. Spin behaves as
// physics, not a looping animation: impacts and ground friction bleed the
// tumble off with the motion, so a chip that has come to rest lies still
// instead of spinning in place.
const CHIP_BOUNCE_SPIN_KEEP: f32 = 0.6;

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct ImpactChip {
    velocity: Vec3,
    spin_axis: Vec3,
    spin_speed: f32,
    lifetime: f32,
    age: f32,
    initial_scale: f32,
    /// Multiplier on the global `IMPACT_GRAVITY`. Use values > 1 for heavier
    /// debris (e.g. rock crumbling at your feet) and 1.0 for regular chips.
    gravity_scale: f32,
    /// When set, the chip keeps its spawn scale for its whole life and simply
    /// despawns at the end, instead of shrinking out over the final stretch.
    /// Every flung debris chip (rock chunks, gather chips, the MeteorShower
    /// rock blast) sets this: solid matter holds its size and only tumbles
    /// under physics, it never animates scale. Only the blood pool decal still
    /// shrinks out (its shrink IS its fade; it is a flat ground stain, not a
    /// flying particle).
    fixed_scale: bool,
}

impl ImpactChip {
    /// Construct a physics chip (fresh, `age = 0`). The single public constructor
    /// so other effect spawners (e.g. the explosion debris burst) can reuse the
    /// same integrator without touching the private fields. `gravity_scale`
    /// multiplies the shared `IMPACT_GRAVITY` (use `> 1.0` for heavier debris).
    pub(crate) fn new(
        velocity: Vec3,
        spin_axis: Vec3,
        spin_speed: f32,
        lifetime: f32,
        initial_scale: f32,
        gravity_scale: f32,
    ) -> Self {
        Self {
            velocity,
            spin_axis,
            spin_speed,
            lifetime,
            age: 0.0,
            initial_scale,
            gravity_scale,
            fixed_scale: false,
        }
    }

    /// Keep the spawn scale for the chip's whole life (no end-of-life shrink).
    pub(crate) fn with_fixed_scale(mut self) -> Self {
        self.fixed_scale = true;
        self
    }
}

pub(crate) fn spawn_impact_effects_system(
    mut commands: Commands,
    assets: Res<ImpactEffectAssets>,
    mut gather_input: ResMut<GatherInputState>,
    mut remote_impacts: MessageReader<RemoteImpactEvent>,
) {
    if let Some(impact) = gather_input.take_pending_impact() {
        spawn_impact_burst(
            &mut commands,
            &assets,
            impact.kind,
            impact.anchor,
            impact.spray_direction,
            impact.seed,
            1.0,
        );
        if impact.kind == ImpactEffectKind::FleshHit {
            spawn_blood_splatter(&mut commands, &assets, impact.anchor, impact.seed);
        }
    }
    for event in remote_impacts.read() {
        // Remote impacts have no view of which way the swinger was facing.
        // An upward spray reads as a clean "hit landed here" without needing
        // that information, and keeps the burst symmetric.
        spawn_impact_burst(
            &mut commands,
            &assets,
            event.effect_kind,
            event.anchor,
            Vec3::Y,
            event.seed,
            1.0,
        );
        if event.effect_kind == ImpactEffectKind::FleshHit {
            spawn_blood_splatter(&mut commands, &assets, event.anchor, event.seed);
        }
    }
}

/// A lingering blood pool on the ground beneath a PvP hit. A couple of flat
/// discs scattered around the impact point, laid on the floor, that sit for a
/// few seconds and then shrink out. Reuses [`ImpactChip`] with zero
/// velocity/gravity/spin and a long lifetime so it stays put and self-despawns.
fn spawn_blood_splatter(
    commands: &mut Commands,
    assets: &ImpactEffectAssets,
    anchor: Vec3,
    seed: u32,
) {
    const SPLAT_LIFETIME: f32 = 6.0;
    const SPLAT_COUNT: u32 = 3;
    // Drop straight down to the (flat) world floor beneath the hit. A small lift
    // off the ground avoids z-fighting with the terrain.
    let ground_y = CHIP_GROUND_Y + 0.005;

    for index in 0..SPLAT_COUNT {
        let seed = seed
            .wrapping_mul(2654435761)
            .wrapping_add(index.wrapping_mul(0x9E37_79B1));
        let r1 = hashed_unit(seed);
        let r2 = hashed_unit(seed.wrapping_add(0xABCD));
        let r3 = hashed_unit(seed.wrapping_add(0x1234));

        let offset = Vec3::new((r1 * 2.0 - 1.0) * 0.20, 0.0, (r2 * 2.0 - 1.0) * 0.20);
        let radius = 0.14 + r3 * 0.12;
        // Circle mesh faces +Z; lay it flat (face +Y) and spin it in-plane for
        // variety so the blobs aren't identical.
        let rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
            * Quat::from_rotation_z(r1 * std::f32::consts::TAU);
        let position = Vec3::new(anchor.x, ground_y, anchor.z) + offset;

        commands.spawn((
            Name::new("Blood Splatter"),
            ImpactChip {
                velocity: Vec3::ZERO,
                spin_axis: Vec3::Y,
                spin_speed: 0.0,
                lifetime: SPLAT_LIFETIME,
                age: 0.0,
                initial_scale: radius,
                gravity_scale: 0.0,
                fixed_scale: false,
            },
            Mesh3d(assets.blood_splatter_mesh.clone()),
            MeshMaterial3d(assets.blood_material.clone()),
            Transform::from_translation(position)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(radius)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }
}

/// Spawn a radial shatter burst, the "rock cracked apart" effect we play
/// when an ore node is depleted. Chunks tumble outward at near-ground
/// level in every horizontal direction, then fall under heavy gravity.
///
/// `magnitude` scales the burst: `1.0` is the full depletion shatter;
/// smaller values (the depletion-stage crossings use ~0.5) shed fewer,
/// slightly smaller chunks over a tighter radius, the mound crumbling
/// down a size rather than exploding.
pub(crate) fn spawn_ore_shatter_burst(
    commands: &mut Commands,
    assets: &ImpactEffectAssets,
    anchor: Vec3,
    seed: u32,
    magnitude: f32,
) {
    let magnitude = magnitude.clamp(0.1, 1.0);
    let count: u32 = ((20.0 * magnitude).round() as u32).max(6);
    // Square-root scaling keeps a half-magnitude burst from looking
    // limp: chunk travel shrinks slower than chunk count.
    let speed = 2.6 * magnitude.sqrt();
    let lifetime = 0.45;
    let chunk_scale = 1.30 * (0.72 + 0.28 * magnitude);
    // Heavy gravity, chunks of rock are dense, they don't drift. Combined
    // with the very low upward kick below, this gives a "crumbling" feel
    // where pieces tumble outward and fall straight to the ground rather
    // than blasting up like an explosion.
    let gravity_scale = 2.8;

    for index in 0..count {
        let seed = seed
            .wrapping_mul(2654435761)
            .wrapping_add(index.wrapping_mul(374761393));
        let r1 = hashed_unit(seed);
        let r2 = hashed_unit(seed.wrapping_add(0xDEADBEEF));
        let r3 = hashed_unit(seed.wrapping_add(0xC0FFEE));

        // Spray outward at near-ground level with only a hint of upward
        // bias. Most of the energy goes into horizontal spread.
        let theta = (index as f32 / count as f32) * std::f32::consts::TAU + r1 * 0.45;
        let horizontal = Vec3::new(theta.cos(), 0.0, theta.sin());
        let horizontal_speed = 0.95 + r2 * 0.55;
        let upward = 0.08 + r3 * 0.30;
        let velocity = (horizontal * horizontal_speed + Vec3::Y * upward) * speed;

        let spin_axis = Vec3::new(r1 * 2.0 - 1.0, r2 * 2.0 - 1.0, r3 * 2.0 - 1.0)
            .normalize_or_zero()
            .max(Vec3::new(0.001, 1.0, 0.001));

        // Fixed quantized size with the matching mass feel: big chunks leave
        // slower and tumble slower, small ones skitter.
        let variant = debris_variant(chunk_scale, r2);
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            r1 * std::f32::consts::TAU,
            r2 * std::f32::consts::TAU,
            r3 * std::f32::consts::TAU,
        );

        commands.spawn((
            Name::new("Ore Shatter Chunk"),
            // Fixed scale: broken rock is solid matter, it tumbles under
            // physics but never grows or shrinks (a chunk visibly animating
            // in size reads as a rendering glitch, seen mixed into the
            // meteor-strike ejecta next to the fixed-size boulder blast).
            ImpactChip {
                velocity: velocity * variant.speed,
                spin_axis,
                spin_speed: (10.0 + r1 * 12.0) * variant.spin,
                lifetime,
                age: 0.0,
                initial_scale: variant.scale,
                gravity_scale,
                fixed_scale: true,
            },
            Mesh3d(assets.stone_chunk_mesh(seed)),
            MeshMaterial3d(assets.stone_shard_material.clone()),
            Transform::from_translation(anchor)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(variant.scale)),
            Visibility::Visible,
            // Sub-cm flying debris. A shadow would be a single fuzzy
            // texel at best and just adds work to the cascade pass.
            NotShadowCaster,
        ));
    }
}

/// Spawn a burst of impact chips at `anchor`. `intensity` scales the chip
/// count, velocity, lifetime, and size, pass `1.0` for the regular per-hit
/// burst and a larger value (e.g. `3.0`) for "kill" effects when a resource
/// node is depleted.
pub(crate) fn spawn_impact_burst(
    commands: &mut Commands,
    assets: &ImpactEffectAssets,
    kind: ImpactEffectKind,
    anchor: Vec3,
    spray_direction: Vec3,
    seed: u32,
    intensity: f32,
) {
    // Which seeded mesh family a burst kind throws (picked per chip below, so
    // one burst mixes silhouettes).
    enum ChipMeshFamily {
        Wood,
        Stone,
        Droplet,
    }
    let (family, material, base_count, base_speed, base_lifetime, base_scale, gravity_scale) =
        match kind {
            ImpactEffectKind::WoodChips => (
                ChipMeshFamily::Wood,
                assets.wood_chip_material.clone(),
                6.0,
                2.4,
                0.60,
                1.0,
                1.6,
            ),
            ImpactEffectKind::StoneShards => (
                ChipMeshFamily::Stone,
                assets.stone_shard_material.clone(),
                7.0,
                2.6,
                0.70,
                1.0,
                2.0,
            ),
            // Crude pickups (branches, surface stones, grass) are
            // hand-harvested, the burst should feel like flicking up
            // a handful of debris rather than a tool strike. Fewer
            // chips, smaller, shorter-lived. Grass also gets a green
            // material so the tuft visibly bursts into leaves rather
            // than gray pebbles.
            ImpactEffectKind::Sticks => (
                ChipMeshFamily::Wood,
                assets.wood_chip_material.clone(),
                3.0,
                1.6,
                0.40,
                0.55,
                1.6,
            ),
            ImpactEffectKind::Pebbles => (
                ChipMeshFamily::Stone,
                assets.stone_shard_material.clone(),
                3.0,
                1.7,
                0.42,
                0.55,
                2.0,
            ),
            ImpactEffectKind::GrassBlades => (
                ChipMeshFamily::Stone,
                assets.grass_blade_material.clone(),
                3.0,
                1.4,
                0.35,
                0.45,
                1.4,
            ),
            // PvP blood spray: small round red droplets flung out fast and
            // short-lived, with heavy gravity so they arc down like spatter
            // rather than hang like dust. A lingering ground pool is spawned
            // separately (see `spawn_blood_splatter`). The droplet mesh keeps it
            // from reading as red rock chips.
            ImpactEffectKind::FleshHit => (
                ChipMeshFamily::Droplet,
                assets.blood_material.clone(),
                8.0,
                2.8,
                0.45,
                0.5,
                2.8,
            ),
        };

    let intensity = intensity.max(0.0);
    let count = (base_count * intensity).round().max(1.0) as u32;
    let speed = base_speed * (1.0 + (intensity - 1.0).max(0.0) * 0.55);
    let lifetime = base_lifetime * (1.0 + (intensity - 1.0).max(0.0) * 0.45);
    let chip_scale = base_scale * (1.0 + (intensity - 1.0).max(0.0) * 0.20);

    let outward = spray_direction.normalize_or_zero();
    let outward = if outward.length_squared() < f32::EPSILON {
        Vec3::Y
    } else {
        outward
    };
    let tangent = outward.any_orthonormal_vector();
    let bitangent = outward.cross(tangent).normalize_or_zero();

    for index in 0..count {
        let seed = seed
            .wrapping_mul(2654435761)
            .wrapping_add(index.wrapping_mul(374761393));
        let r1 = hashed_unit(seed);
        let r2 = hashed_unit(seed.wrapping_add(0xDEADBEEF));
        let r3 = hashed_unit(seed.wrapping_add(0xC0FFEE));

        // Most of the energy goes into a horizontal spread, only a small
        // upward "puff" so chips clear the surface, then gravity pulls them
        // straight down and friction rolls them out.
        let angle = (index as f32 / count as f32) * std::f32::consts::TAU + r1 * 0.6;
        let radial = tangent * angle.cos() + bitangent * angle.sin();
        let upward = 0.25 + r2 * 0.35;
        let outward_strength = 0.85 + r3 * 0.50;
        let velocity = (radial * outward_strength + outward * 0.45 + Vec3::Y * upward) * speed;

        let spin_axis = Vec3::new(r1 * 2.0 - 1.0, r2 * 2.0 - 1.0, r3 * 2.0 - 1.0)
            .normalize_or_zero()
            .max(Vec3::new(0.001, 1.0, 0.001));

        // Fixed quantized size with the matching mass feel (heavy = slower
        // launch, lazier tumble), and a per-chip mesh variant so the handful
        // mixes silhouettes.
        let variant = debris_variant(chip_scale, r2);
        let mesh = match family {
            ChipMeshFamily::Wood => assets.wood_chip_mesh(seed),
            ChipMeshFamily::Stone => assets.stone_chunk_mesh(seed),
            ChipMeshFamily::Droplet => assets.blood_droplet_mesh.clone(),
        };
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            r1 * std::f32::consts::TAU,
            r2 * std::f32::consts::TAU,
            r3 * std::f32::consts::TAU,
        );

        commands.spawn((
            Name::new("Impact Chip"),
            // Fixed scale for the same reason as the ore shatter and meteor
            // rubble: flung chips are physical debris, so they keep their
            // spawn size and only rotate; the short lifetimes mean they pop
            // out at rest rather than shrinking mid-flight.
            ImpactChip {
                velocity: velocity * variant.speed,
                spin_axis,
                spin_speed: (8.0 + r1 * 11.0) * variant.spin,
                lifetime,
                age: 0.0,
                initial_scale: variant.scale,
                gravity_scale,
                fixed_scale: true,
            },
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(anchor)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(variant.scale)),
            Visibility::Visible,
            NotShadowCaster,
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

/// One drawn debris variant: a fixed size plus the physics feel of its mass.
/// All four sizes fall under the same gravity like real matter, but the
/// heavier chunks leave the impact slower and tumble slower (more mass, more
/// angular inertia) while the small chips fly and spin quickest, so a chunk's
/// size always reads as proportional to how it moves (owner rule). Sizes are
/// fixed from spawn and never animate.
pub(crate) struct DebrisVariant {
    pub(crate) scale: f32,
    /// Launch-velocity multiplier for this mass class.
    pub(crate) speed: f32,
    /// Tumble-rate multiplier for this mass class.
    pub(crate) spin: f32,
}

/// Quantize a debris chip onto one of four FIXED size variants around `base`,
/// with the matching mass feel. The top variant doubles as a hard size cap
/// (owner call: continuous rolls sometimes read as huge boulders).
pub(crate) fn debris_variant(base: f32, unit: f32) -> DebrisVariant {
    const SCALE: [f32; 4] = [0.7, 0.85, 1.0, 1.2];
    const SPEED: [f32; 4] = [1.22, 1.08, 1.0, 0.85];
    const SPIN: [f32; 4] = [1.40, 1.15, 1.0, 0.72];
    let index = (((unit.clamp(0.0, 0.999)) * SCALE.len() as f32) as usize).min(SCALE.len() - 1);
    DebrisVariant {
        scale: base * SCALE[index],
        speed: SPEED[index],
        spin: SPIN[index],
    }
}

/// Size-only view of [`debris_variant`], for particles whose motion is not a
/// ballistic launch (fire, sparks, smoke).
pub(crate) fn quantized_chip_scale(base: f32, unit: f32) -> f32 {
    debris_variant(base, unit).scale
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

    chip.velocity.y -= IMPACT_GRAVITY * chip.gravity_scale * dt;
    transform.translation += chip.velocity * dt;

    // Ground interaction, once a chip reaches the floor it stops sinking,
    // bounces a little, and slides outward under friction so it reads as
    // "tumbling along the ground until it stops" instead of vanishing in
    // mid-air or punching through the floor.
    if transform.translation.y < CHIP_GROUND_Y {
        transform.translation.y = CHIP_GROUND_Y;
        if chip.velocity.y < 0.0 {
            chip.velocity.y = -chip.velocity.y * CHIP_BOUNCE;
            // Each impact also bleeds rotational energy.
            chip.spin_speed *= CHIP_BOUNCE_SPIN_KEEP;
        }
    }
    if transform.translation.y <= CHIP_GROUND_Y + CHIP_GROUND_CONTACT_BAND {
        // Friction applies whenever the chip is on or just above the ground,
        // so its horizontal energy decays continuously through small bounces.
        // The tumble decays with it: the chip rolls slower as it slides slower
        // and lies STILL once it has stopped, rather than spinning in place.
        let friction = (1.0 - CHIP_GROUND_FRICTION * dt).max(0.0);
        chip.velocity.x *= friction;
        chip.velocity.z *= friction;
        chip.spin_speed *= friction;
        // Once it has essentially stopped sliding, it has landed: kill the
        // residual motion outright so a resting chunk lies dead still (no
        // slow-motion spin on the spot).
        let horizontal =
            (chip.velocity.x * chip.velocity.x + chip.velocity.z * chip.velocity.z).sqrt();
        if horizontal < 0.25 && chip.velocity.y.abs() < 0.5 {
            chip.velocity = Vec3::ZERO;
            chip.spin_speed = 0.0;
        }
    }

    let rotation = Quat::from_axis_angle(chip.spin_axis, chip.spin_speed * dt);
    transform.rotation = rotation * transform.rotation;

    if !chip.fixed_scale {
        let life_t = (chip.age / chip.lifetime).clamp(0.0, 1.0);
        // Hold size most of the way, then shrink off the last 35% for a clean
        // pop-out rather than a gradual fade.
        let shrink_t = ((life_t - 0.65) / 0.35).max(0.0);
        let scale = chip.initial_scale * (1.0 - shrink_t).max(0.0);
        transform.scale = Vec3::splat(scale);
    }
    ChipStep::Alive
}

#[cfg(test)]
mod tests {
    use super::*;

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
            gravity_scale: 1.0,
            fixed_scale: false,
        };

        // Mid-life, still alive, gravity has pulled velocity down.
        assert_eq!(
            advance_chip(&mut transform, &mut chip, 0.10),
            ChipStep::Alive
        );
        assert!(chip.velocity.y < 2.0);
        assert!(transform.translation.y > 1.0);
        assert!(transform.scale.x > 0.99); // still in hold range

        // Past the shrink threshold, scale should have shrunk noticeably.
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

    #[test]
    fn impact_chip_settles_on_the_ground_with_friction() {
        // Start near the floor with a strong downward + horizontal velocity.
        // After enough integration steps the chip should be resting on the
        // ground (Y clamped) with most of its horizontal energy bled off.
        let mut transform = Transform::from_xyz(0.0, 0.20, 0.0);
        let mut chip = ImpactChip {
            velocity: Vec3::new(3.0, -4.0, 0.0),
            spin_axis: Vec3::Y,
            spin_speed: 0.0,
            lifetime: 2.0,
            age: 0.0,
            initial_scale: 1.0,
            gravity_scale: 1.0,
            fixed_scale: false,
        };

        for _ in 0..40 {
            let _ = advance_chip(&mut transform, &mut chip, 1.0 / 60.0);
        }

        assert!(transform.translation.y >= CHIP_GROUND_Y - 1e-4);
        assert!(transform.translation.y < CHIP_GROUND_Y + 0.05);
        assert!(
            transform.translation.x > 0.5,
            "chip should have rolled outward"
        );
        assert!(
            chip.velocity.x.abs() < 1.0,
            "horizontal friction should bleed energy"
        );
        assert!(chip.velocity.y.abs() < 0.5, "vertical motion should settle");
    }

    #[test]
    fn heavier_gravity_scale_pulls_chip_down_faster() {
        let mut light_transform = Transform::from_xyz(0.0, 1.0, 0.0);
        let mut light = ImpactChip {
            velocity: Vec3::new(0.0, 1.0, 0.0),
            spin_axis: Vec3::Y,
            spin_speed: 0.0,
            lifetime: 1.0,
            age: 0.0,
            initial_scale: 1.0,
            gravity_scale: 1.0,
            fixed_scale: false,
        };
        let mut heavy_transform = Transform::from_xyz(0.0, 1.0, 0.0);
        let mut heavy = ImpactChip {
            gravity_scale: 3.0,
            ..light
        };

        // Step both for the same duration. The heavier chip should be
        // noticeably lower than the light one.
        advance_chip(&mut light_transform, &mut light, 0.20);
        advance_chip(&mut heavy_transform, &mut heavy, 0.20);

        assert!(heavy_transform.translation.y < light_transform.translation.y - 0.10);
        assert!(heavy.velocity.y < light.velocity.y - 0.5);
    }

    #[test]
    fn chip_expires_the_instant_its_age_reaches_lifetime() {
        let mut transform = Transform::from_xyz(0.0, 5.0, 0.0);
        let mut chip = ImpactChip {
            velocity: Vec3::ZERO,
            spin_axis: Vec3::Y,
            spin_speed: 0.0,
            lifetime: 0.20,
            age: 0.19,
            initial_scale: 1.0,
            gravity_scale: 1.0,
            fixed_scale: false,
        };
        // One step that crosses the lifetime expires the chip immediately.
        assert_eq!(
            advance_chip(&mut transform, &mut chip, 0.02),
            ChipStep::Expired
        );
    }

    #[test]
    fn chip_bounces_off_the_floor_rather_than_sinking_through() {
        // Start below the floor with downward velocity, the chip should be
        // clamped to ground level and its vertical velocity reflected up.
        let mut transform = Transform::from_xyz(0.0, -0.5, 0.0);
        let mut chip = ImpactChip {
            velocity: Vec3::new(0.0, -3.0, 0.0),
            spin_axis: Vec3::Y,
            spin_speed: 0.0,
            lifetime: 1.0,
            age: 0.0,
            initial_scale: 1.0,
            gravity_scale: 1.0,
            fixed_scale: false,
        };
        assert_eq!(
            advance_chip(&mut transform, &mut chip, 1.0 / 60.0),
            ChipStep::Alive
        );
        assert!((transform.translation.y - CHIP_GROUND_Y).abs() < 1e-5);
        // Velocity reflected upward (with restitution), so now positive.
        assert!(chip.velocity.y > 0.0);
        assert!(chip.velocity.y < 3.0, "bounce should lose energy");
    }

    #[test]
    fn chip_spin_bleeds_off_with_its_slide_and_settles_still() {
        // Spin is physics, not a looping animation: bounces and ground
        // friction drain it alongside the horizontal slide, so a chip that has
        // come to rest lies still instead of spinning in place.
        let mut transform = Transform::from_xyz(0.0, 0.20, 0.0);
        let mut chip = ImpactChip {
            velocity: Vec3::new(3.0, -4.0, 0.0),
            spin_axis: Vec3::Y,
            spin_speed: 20.0,
            lifetime: 3.0,
            age: 0.0,
            initial_scale: 1.0,
            gravity_scale: 1.0,
            fixed_scale: true,
        };
        // One airborne step first: spin must NOT decay in flight.
        let _ = advance_chip(&mut transform, &mut chip, 1.0 / 60.0);
        assert!(chip.spin_speed > 19.9, "airborne tumble keeps its energy");
        for _ in 0..60 {
            let _ = advance_chip(&mut transform, &mut chip, 1.0 / 60.0);
        }
        assert!(
            chip.velocity.x.abs() < 0.5,
            "the slide should have bled off"
        );
        assert!(
            chip.spin_speed < 1.0,
            "a settled chip stops tumbling, spin still {}",
            chip.spin_speed
        );
    }
}
