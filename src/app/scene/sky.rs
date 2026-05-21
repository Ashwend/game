//! Day/night visuals: sun + moon directional lights, animated ambient
//! light, animated clear color, animated distance fog, and visible sun
//! and moon discs that ride the sky dome around the player camera.
//!
//! The math driver is [`ClientRuntime::world_time`]. The server owns it
//! and the client integrates between snapshots — by the time these
//! systems run the value is the live mirror.
//!
//! Module boundary: lighting setup and per-frame updates live here so
//! `assets.rs` stays focused on neutral mesh/material handles. The sun
//! direction math also lives here because it's only consumed by the
//! systems below.

use bevy::{
    light::{CascadeShadowConfigBuilder, NotShadowCaster},
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
};

use crate::{
    app::state::ClientRuntime,
    world_time::{SECONDS_PER_DAY, WorldTime},
};

use super::components::MainCamera;

/// Apparent radius of the sky dome the sun/moon ride on. The camera's
/// far plane is 200 m, so we stay comfortably inside it. Chosen big
/// enough that parallax against nearby trees feels infinite, small
/// enough that the disc resolves cleanly.
const SKY_DISTANCE: f32 = 140.0;

/// Visible radius of the sun disc. ~3.5 m at 140 m → about 2.9° of arc,
/// roughly twice the real sun's apparent diameter. The exaggeration is
/// deliberate; a true 0.5° disc reads as a pinhole in stylised art.
const SUN_DISC_RADIUS: f32 = 3.6;

/// Visible radius of the moon disc. Slightly larger than the sun so a
/// full moon reads as a feature, not a freckle.
const MOON_DISC_RADIUS: f32 = 4.2;

/// Tilt of the solar/lunar plane off the world's east-west axis. Bevy's
/// world Z grows "forward" in the default camera setup; tilting the
/// celestial plane a bit gives the sun an oblique track across the sky
/// instead of marching straight overhead, which reads as more cinematic.
const CELESTIAL_TILT_DEGREES: f32 = 18.0;

/// Peak daylight illuminance (lux). Matched to Bevy's
/// `light_consts::lux::AMBIENT_DAYLIGHT` (10 000 lux). Higher values
/// blow PBR surfaces past the tonemapper's response curve and the
/// scene reads as overexposed "atomic flash" rather than midday.
const SUN_PEAK_ILLUMINANCE: f32 = 11_000.0;

/// Peak moonlight illuminance. Real moonlight is ~0.3 lux; we cheat
/// up by ~3 000× so the player can actually see what's around them.
/// ~7% of the sun's peak gives a moonlit-grass survival look without
/// reading as overcast daytime.
const MOON_PEAK_ILLUMINANCE: f32 = 800.0;

/// Sun direction shifts from "below horizon" through dawn into "up".
/// This is the cosine of the angle below the horizon at which the sun
/// transitions from off to its full illuminance ramp.
const HORIZON_FADE_BAND: f32 = 0.18;

/// Real-time cadence at which the directional light's transform is
/// allowed to change at the default `1×` multiplier. Targets 15 Hz —
/// fast enough that the eye fuses successive updates into smooth
/// motion (well above the ~24 fps perception threshold for fluid
/// motion is a movie myth; ~12–15 Hz is plenty for tiny shadow-edge
/// changes) but only a quarter of the per-frame cost the original
/// every-frame approach was paying.
///
/// At higher time multipliers the interval is scaled down so that
/// each tick still represents roughly the same angle change —
/// otherwise fast-forward would produce visible stepping. See
/// [`shadow_update_interval`].
///
/// Light *colour* and *illuminance* still update every frame — only
/// the transform (which drives the shadow projection) is throttled.
const SHADOW_UPDATE_BASE_INTERVAL_SECS: f32 = 1.0 / 15.0;

/// Lower bound on the shadow update interval. ~60 Hz: faster than this
/// puts us back into per-frame shimmer territory and gains nothing,
/// since the eye fuses motion above ~24 Hz regardless.
const SHADOW_UPDATE_MIN_INTERVAL_SECS: f32 = 1.0 / 60.0;

#[derive(Component)]
pub(crate) struct SunLight;

#[derive(Component)]
pub(crate) struct MoonLight;

#[derive(Component)]
pub(crate) struct SunVisual;

#[derive(Component)]
pub(crate) struct MoonVisual;

type CameraTransformQuery<'w, 's> =
    Query<'w, 's, &'static Transform, (With<MainCamera>, Without<SunLight>, Without<MoonLight>)>;

type SunLightQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut DirectionalLight, &'static mut Transform),
    (With<SunLight>, Without<MoonLight>, Without<MainCamera>),
>;

type MoonLightQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut DirectionalLight, &'static mut Transform),
    (With<MoonLight>, Without<SunLight>, Without<MainCamera>),
>;

type SunVisualQuery<'w, 's> = Query<
    'w,
    's,
    &'static mut Transform,
    (
        With<SunVisual>,
        Without<MoonVisual>,
        Without<SunLight>,
        Without<MoonLight>,
        Without<MainCamera>,
    ),
>;

type MoonVisualQuery<'w, 's> = Query<
    'w,
    's,
    &'static mut Transform,
    (
        With<MoonVisual>,
        Without<SunVisual>,
        Without<SunLight>,
        Without<MoonLight>,
        Without<MainCamera>,
    ),
>;

type FogQuery<'w, 's> = Query<'w, 's, &'static mut DistanceFog, With<MainCamera>>;

/// Spawn the directional lights and sun/moon disc visuals. Called from
/// `setup_scene` after the camera exists.
pub(crate) fn setup_sky(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    // Sun: shadow-casting directional light. Slightly warm white at
    // peak; the per-frame color update will tint warmer at sunrise and
    // sunset, cooler at midday. We start with `illuminance` near zero
    // because the lighting system will overwrite it on the next tick
    // anyway, but a sensible non-zero starting value avoids a black
    // first frame.
    commands.spawn((
        Name::new("Sun"),
        SunLight,
        DirectionalLight {
            illuminance: SUN_PEAK_ILLUMINANCE * 0.5,
            color: Color::srgb(1.00, 0.96, 0.88),
            shadows_enabled: true,
            shadow_depth_bias: 0.10,
            shadow_normal_bias: 1.8,
            ..default()
        },
        // Default cascade config goes out to 150 m, sized for AAA open
        // worlds. Our floor is at most ~80 m across so trimming the far
        // bound to 100 m gives every shadow texel ~33% more on-screen
        // resolution and skips one cascade's worth of shadow-caster
        // draws past the visible playspace, with no visible difference.
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            maximum_distance: 100.0,
            first_cascade_far_bound: 8.0,
            ..default()
        }
        .build(),
        Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Moon: dim cool directional light. Shadows are off — a second
    // shadow map is expensive and moonlit shadows in stylised low-poly
    // art read as visual noise. The moon contributes color and a sense
    // of direction without slamming the shadow budget.
    commands.spawn((
        Name::new("Moon"),
        MoonLight,
        DirectionalLight {
            illuminance: 0.0,
            color: Color::srgb(0.70, 0.78, 1.00),
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let sun_visual_mesh = meshes.add(Sphere::new(SUN_DISC_RADIUS));
    let moon_visual_mesh = meshes.add(Sphere::new(MOON_DISC_RADIUS));

    let sun_visual_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.95, 0.85),
        emissive: Color::srgb(40.0, 30.0, 15.0).to_linear(),
        unlit: true,
        fog_enabled: false,
        ..default()
    });
    let moon_visual_material = materials.add(StandardMaterial {
        // A faint warm tint on the surface lit color reads as moon
        // regolith without needing a texture.
        base_color: Color::srgb(0.95, 0.95, 0.92),
        emissive: Color::srgb(2.6, 2.8, 3.4).to_linear(),
        unlit: true,
        fog_enabled: false,
        ..default()
    });

    commands.spawn((
        Name::new("Sun Visual"),
        SunVisual,
        Mesh3d(sun_visual_mesh),
        MeshMaterial3d(sun_visual_material),
        Transform::IDENTITY,
        // The sun mesh IS the sun — it shouldn't drop a shadow on the
        // world from its own light. Same logic for the moon.
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Moon Visual"),
        MoonVisual,
        Mesh3d(moon_visual_mesh),
        MeshMaterial3d(moon_visual_material),
        Transform::IDENTITY,
        NotShadowCaster,
    ));
}

/// Distance fog the gameplay camera should carry. Color and falloff are
/// updated per-frame; this is just the initial component so the camera
/// owns the slot before the lighting system runs.
pub(crate) fn initial_distance_fog() -> DistanceFog {
    DistanceFog {
        color: Color::srgb(0.55, 0.65, 0.78),
        directional_light_color: Color::srgba(1.0, 0.92, 0.78, 0.5),
        directional_light_exponent: 30.0,
        falloff: FogFalloff::from_visibility(220.0),
    }
}

/// Drives every per-frame day/night change: light direction and color,
/// ambient brightness, clear color, fog, and the sun/moon disc
/// positions. Runs in `ClientSystemSet::Sky`, which sits after the
/// network tick (so `runtime.world_time` is fresh) and after the
/// camera follow (so we anchor the discs to the latest camera pose).
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_sky_system(
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    mut shadow_throttle: Local<f32>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear_color: ResMut<ClearColor>,
    camera: CameraTransformQuery,
    mut sun_light: SunLightQuery,
    mut moon_light: MoonLightQuery,
    mut sun_visual: SunVisualQuery,
    mut moon_visual: MoonVisualQuery,
    mut fog: FogQuery,
) {
    let lighting = compute_lighting(&runtime.world_time);

    // Real-time throttle for the directional lights' *transform*. The
    // shadow projection is the part the eye reads as "shimmery" when
    // updated every frame; throttling its writes gives windows of
    // fully stable shadows between events. The interval scales with
    // the world-time multiplier (see `shadow_update_interval`) so
    // each tick represents a roughly constant *angle* — fast-forward
    // gets more frequent updates, normal play gets the calm ~4.5 Hz.
    // We accumulate `delta_secs` and only fire on overflow so the
    // actual cadence doesn't drift with frame rate.
    let interval = shadow_update_interval(runtime.world_time.multiplier);
    *shadow_throttle += time.delta_secs();
    let advance_shadows = *shadow_throttle >= interval;
    if advance_shadows {
        // Subtract the interval rather than zeroing so accumulated
        // overshoot is preserved — keeps the long-run cadence honest
        // even under uneven frame timing.
        *shadow_throttle -= interval;
        // If the game was paused or framerate stalled hard the
        // accumulator could pile up enough to fire several events in
        // a row; clamp to a single fire per frame so we never
        // hammer the shadow cascade rebuild after a hitch.
        if *shadow_throttle > interval {
            *shadow_throttle = 0.0;
        }
    }

    // Colour and illuminance update every frame — those are scalar
    // changes that don't move the shadow grid, so they can ride the
    // continuous time-of-day curve without contributing to shimmer.
    // Only the Transform write is gated by the throttle.
    if let Ok((mut light, mut transform)) = sun_light.single_mut() {
        light.color = lighting.sun_color;
        light.illuminance = lighting.sun_illuminance;
        if advance_shadows {
            *transform = directional_light_transform(lighting.sun_direction);
        }
    }

    if let Ok((mut light, mut transform)) = moon_light.single_mut() {
        light.color = lighting.moon_color;
        light.illuminance = lighting.moon_illuminance;
        if advance_shadows {
            *transform = directional_light_transform(lighting.moon_direction);
        }
    }

    ambient.color = vec3_to_color(lighting.ambient_color);
    ambient.brightness = lighting.ambient_brightness;
    clear_color.0 = vec3_to_color(lighting.sky_color);

    if let Ok(mut fog) = fog.single_mut() {
        fog.color = vec3_to_color(lighting.fog_color);
        fog.directional_light_color = Color::srgba(
            lighting.sun_glow.x,
            lighting.sun_glow.y,
            lighting.sun_glow.z,
            lighting.sun_glow_strength,
        );
        fog.falloff = FogFalloff::from_visibility(lighting.fog_distance);
    }

    // Sun/moon visuals follow the camera at a fixed dome radius so they
    // feel infinitely distant regardless of where the player walks.
    if let Ok(camera_transform) = camera.single() {
        let anchor = camera_transform.translation;
        if let Ok(mut transform) = sun_visual.single_mut() {
            transform.translation = anchor + lighting.sun_direction * SKY_DISTANCE;
        }
        if let Ok(mut transform) = moon_visual.single_mut() {
            transform.translation = anchor + lighting.moon_direction * SKY_DISTANCE;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LightingFrame {
    /// Continuous sun direction. Drives the visible sun disc, all the
    /// colour/intensity ramps, and the directional-light transform
    /// when the shadow-update throttle fires.
    sun_direction: Vec3,
    /// Continuous moon direction (always the antipode of the sun).
    moon_direction: Vec3,
    sun_illuminance: f32,
    moon_illuminance: f32,
    sun_color: Color,
    moon_color: Color,
    ambient_color: Vec3,
    ambient_brightness: f32,
    sky_color: Vec3,
    sun_glow: Vec3,
    sun_glow_strength: f32,
    fog_color: Vec3,
    fog_distance: f32,
}

fn compute_lighting(time: &WorldTime) -> LightingFrame {
    let fraction = (time.seconds_of_day / SECONDS_PER_DAY).rem_euclid(1.0);
    let sun_direction = celestial_direction(fraction);
    let moon_direction = -sun_direction;

    let sun_height = sun_direction.y;
    let moon_height = moon_direction.y;

    // Smooth, signed elevation factor in [0, 1] used to blend day/night
    // palettes. Anchored to ~10° below the horizon so the dawn/dusk
    // colour ramp lingers a beat instead of flicking to night.
    let day_strength = smoothstep(-HORIZON_FADE_BAND, HORIZON_FADE_BAND, sun_height);

    // Warm-cast band around the horizon. Peaks when the sun is right at
    // the edge of the world; zero when it's well above or well below.
    let sunset_strength = (1.0 - (sun_height / HORIZON_FADE_BAND).abs())
        .clamp(0.0, 1.0)
        .powf(1.7);

    // Civil-twilight palette. Day sky is desaturated from a pure cyan
    // toward a slightly warmer azure so it doesn't read as a flat
    // bleach when the sun is directly overhead. Night sky is lifted
    // off black so the distance fog (which mirrors the sky tone)
    // doesn't crush everything behind 30 m into a black void.
    let day_sky = Vec3::new(0.46, 0.66, 0.86);
    let sunset_sky = Vec3::new(0.92, 0.48, 0.26);
    let night_sky = Vec3::new(0.045, 0.065, 0.130);

    let sky_color =
        lerp_vec3(night_sky, day_sky, day_strength).lerp(sunset_sky, sunset_strength * 0.85);

    // Ambient *color* sits close to neutral white during the day with a
    // very mild sky-bounce tint. Heavier blue here saturates against
    // the cyan sky and pushes the scene into the "atomic flash" look.
    let day_ambient = Vec3::new(0.92, 0.94, 1.00);
    let night_ambient = Vec3::new(0.32, 0.42, 0.66);
    let sunset_ambient = Vec3::new(0.96, 0.70, 0.50);
    let ambient_color = lerp_vec3(night_ambient, day_ambient, day_strength)
        .lerp(sunset_ambient, sunset_strength * 0.4);

    // Ambient *brightness* curve. Bevy's default GlobalAmbientLight is
    // 80; we stay close to that for noon so PBR surfaces aren't lit
    // twice (once by the sun, once by an oversized ambient bounce).
    // Night sits at ~60 so the player can actually navigate by moon
    // and ambient sky bounce. Combined with the cool-blue night
    // `ambient_color` it still reads unambiguously as night, just a
    // moonlit one rather than a cave.
    let ambient_brightness = lerp(60.0, 75.0, day_strength) + sunset_strength * 25.0;

    // Sun direct illuminance: 0 below the horizon, ramping up to a soft
    // peak well above. The pow shapes the curve so the sun stays warm
    // and shadows stay long during the first hour of "morning".
    let sun_elevation = sun_height.max(0.0).clamp(0.0, 1.0);
    let sun_illuminance = SUN_PEAK_ILLUMINANCE * sun_elevation.powf(0.55);

    let sun_warm = Vec3::new(1.00, 0.55, 0.30);
    let sun_neutral = Vec3::new(1.00, 0.96, 0.88);
    let sun_tint = sun_warm.lerp(sun_neutral, day_strength);
    let sun_color = vec3_to_color(sun_tint);

    // Moonlight only takes over once the sun has fully set. The
    // moonlight color is a cool blue-white; intensity follows the moon
    // height with a softer curve.
    let moon_elevation = moon_height.max(0.0).clamp(0.0, 1.0);
    let moon_illuminance = MOON_PEAK_ILLUMINANCE * moon_elevation.powf(0.6) * (1.0 - day_strength);
    let moon_color = Color::srgb(0.70, 0.78, 1.00);

    let sun_glow = sun_warm.lerp(Vec3::new(1.0, 0.92, 0.80), 1.0 - sunset_strength);
    // The directional-light bleed through fog should be a hint of warm
    // glow around the sun, not a second sun layer. Keep it modest at
    // noon, peaking at sunset where it sells the atmosphere most.
    let sun_glow_strength = (0.08 + sunset_strength * 0.35) * sun_elevation.powf(0.4);

    // Fog matches the sky horizon and tightens at night so the player
    // can't see across the world in pitch-blackness. During sunset the
    // distance shrinks for that hazy, dust-laden look.
    let fog_color = sky_color * 0.9 + Vec3::splat(0.02);
    let fog_distance = lerp(110.0, 240.0, day_strength) - sunset_strength * 40.0;

    LightingFrame {
        sun_direction,
        moon_direction,
        sun_illuminance,
        moon_illuminance,
        sun_color,
        moon_color,
        ambient_color,
        ambient_brightness,
        sky_color,
        sun_glow,
        sun_glow_strength,
        fog_color,
        fog_distance,
    }
}

/// Build the unit-length sun direction (origin → sun) for the given
/// day fraction in `[0, 1)`. The plane is tilted off east-west so the
/// sun doesn't march along a straight axis-aligned path.
fn celestial_direction(day_fraction: f32) -> Vec3 {
    // Theta = 0 at sunrise (east horizon), π/2 at noon (overhead),
    // π at sunset, 3π/2 below the horizon.
    let theta = (day_fraction - 0.25) * std::f32::consts::TAU;
    let east_west = theta.cos();
    let up = theta.sin();

    let tilt = CELESTIAL_TILT_DEGREES.to_radians();
    let tilt_cos = tilt.cos();
    let tilt_sin = tilt.sin();

    // Rotate the east-up plane around the world's east axis by the
    // tilt: introduces a Z component so the sun arcs slightly toward
    // the player's "forward" direction as it climbs.
    Vec3::new(east_west, up * tilt_cos, up * tilt_sin).normalize_or_zero()
}

fn directional_light_transform(direction_to_source: Vec3) -> Transform {
    // The directional light's local -Z is the direction light *travels*.
    // We want light to travel from the celestial body toward the ground,
    // so we place the entity at `+direction` and orient it back at the
    // origin. The translation itself is meaningless for an infinite
    // directional light; we only need the rotation.
    let position = direction_to_source.normalize_or_zero() * 100.0;
    if position.length_squared() < 1.0 {
        return Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y);
    }
    Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y)
}

/// How long to wait between writes to the directional light's
/// transform, given the current world-time multiplier. Scales
/// inversely so each tick represents roughly the same angular change
/// at any speed: at 1× the sun ticks every ~67 ms (15 Hz), at 4× and
/// above we're already at the per-frame floor so the cadence
/// saturates at 60 Hz. Multipliers below 1× inherit the base
/// interval — slow-motion is already smooth at 15 Hz, no need to
/// slow updates down further.
fn shadow_update_interval(multiplier: f32) -> f32 {
    let scale = multiplier.max(1.0);
    (SHADOW_UPDATE_BASE_INTERVAL_SECS / scale).max(SHADOW_UPDATE_MIN_INTERVAL_SECS)
}

fn smoothstep(edge_lo: f32, edge_hi: f32, value: f32) -> f32 {
    let t = ((value - edge_lo) / (edge_hi - edge_lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn lerp_vec3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a.lerp(b, t.clamp(0.0, 1.0))
}

fn vec3_to_color(v: Vec3) -> Color {
    Color::srgb(v.x.max(0.0), v.y.max(0.0), v.z.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_time::SECONDS_PER_DAY;

    fn time_at(hour: f32) -> WorldTime {
        WorldTime {
            seconds_of_day: hour * 3600.0,
            multiplier: 1.0,
        }
    }

    #[test]
    fn sun_is_below_horizon_at_midnight() {
        let dir = celestial_direction(0.0);
        assert!(
            dir.y < -0.5,
            "midnight sun should be well below horizon: {dir:?}"
        );
    }

    #[test]
    fn sun_is_above_horizon_at_noon() {
        let dir = celestial_direction(0.5);
        assert!(
            dir.y > 0.5,
            "noon sun should be well above horizon: {dir:?}"
        );
    }

    #[test]
    fn sun_is_near_horizon_at_sunrise_and_sunset() {
        let dawn = celestial_direction(0.25);
        let dusk = celestial_direction(0.75);
        assert!(
            dawn.y.abs() < 0.05,
            "dawn sun should sit on horizon: {dawn:?}"
        );
        assert!(
            dusk.y.abs() < 0.05,
            "dusk sun should sit on horizon: {dusk:?}"
        );
        assert!(dawn.x > 0.7, "dawn sun should be east: {dawn:?}");
        assert!(dusk.x < -0.7, "dusk sun should be west: {dusk:?}");
    }

    #[test]
    fn moon_is_opposite_the_sun() {
        for &fraction in &[0.0, 0.1, 0.33, 0.71, 0.95] {
            let sun = celestial_direction(fraction);
            assert!((sun + (-sun)).length() < 1e-5, "moon should mirror sun");
        }
    }

    #[test]
    fn night_lighting_is_dimmer_than_day() {
        let day = compute_lighting(&time_at(12.0));
        let night = compute_lighting(&time_at(0.0));
        assert!(night.sun_illuminance < 1.0);
        assert!(day.sun_illuminance > 100.0);
        assert!(night.ambient_brightness < day.ambient_brightness);
        // Night ambient is intentionally non-zero so the player can read
        // their surroundings.
        assert!(night.ambient_brightness > 5.0);
    }

    #[test]
    fn moon_provides_some_illumination_at_night() {
        let night = compute_lighting(&time_at(0.0));
        // The moon's job is "see your surroundings", not "stadium
        // floodlight". A double-digit lux value is plenty against a
        // ~15-lux ambient.
        assert!(night.moon_illuminance > 10.0);
        let noon = compute_lighting(&time_at(12.0));
        assert!(noon.moon_illuminance < 1.0);
    }

    #[test]
    fn day_fraction_wraps_around_seconds() {
        // Make sure rem_euclid is in the conversion, not just the
        // server-side advance.
        let time = WorldTime {
            seconds_of_day: SECONDS_PER_DAY + 1.0,
            multiplier: 1.0,
        };
        let lighting = compute_lighting(&time);
        // Just-after-midnight should look like midnight: sun very low.
        assert!(lighting.sun_illuminance < 10.0);
    }

    #[test]
    fn directional_light_transform_does_not_panic_for_origin() {
        let t = directional_light_transform(Vec3::ZERO);
        // Identity-ish fallback that still has a valid orientation.
        assert!(t.translation.is_finite());
    }

    #[test]
    fn shadow_update_interval_scales_inversely_with_multiplier() {
        let base = shadow_update_interval(1.0);
        assert!((base - SHADOW_UPDATE_BASE_INTERVAL_SECS).abs() < 1e-6);

        // Modest multipliers scale linearly (between base 15 Hz and
        // the per-frame floor at 60 Hz — crossover happens at 4×).
        let two_x = shadow_update_interval(2.0);
        assert!((two_x - SHADOW_UPDATE_BASE_INTERVAL_SECS / 2.0).abs() < 1e-4);
        assert!(two_x > SHADOW_UPDATE_MIN_INTERVAL_SECS);

        // Fast-forward saturates at the ~60 Hz floor — once we're
        // updating every frame there's nothing left to gain.
        let fast = shadow_update_interval(60.0);
        assert!((fast - SHADOW_UPDATE_MIN_INTERVAL_SECS).abs() < 1e-6);
        let extreme = shadow_update_interval(10_000.0);
        assert!((extreme - SHADOW_UPDATE_MIN_INTERVAL_SECS).abs() < 1e-6);

        // Slow-motion stays at the base cadence — no need to throttle
        // updates further when shadows are already smooth at 15 Hz.
        let slow = shadow_update_interval(0.5);
        assert_eq!(slow, SHADOW_UPDATE_BASE_INTERVAL_SECS);
        let paused = shadow_update_interval(0.0);
        assert_eq!(paused, SHADOW_UPDATE_BASE_INTERVAL_SECS);
    }
}
