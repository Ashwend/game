//! Day/night visuals.
//!
//! The procedural [`Atmosphere`](bevy::pbr::Atmosphere) on the camera (set up
//! in `assets.rs`) now renders the sky itself, physically-based scattering,
//! the visible sun disc, and image-based ambient/reflection light via
//! [`AtmosphereEnvironmentMapLight`]. This module's job shrank to:
//!
//! - driving the sun + moon [`DirectionalLight`] direction/intensity from world
//!   time (the atmosphere reads the sun light to know where the sun is and
//!   tints it as it passes through the air),
//! - supplementing the night with a **fixed** ambient floor + a dim moon light
//!   so the player can still navigate after dark (intentionally not a user
//!   setting, night visibility is a gameplay-fair constant),
//! - and keeping a [`DistanceFog`] curtain that hides the far perimeter walls
//!   and dissolves the ground into the horizon before the camera's far plane
//!   (300 m) clips them (the atmosphere's own aerial perspective is negligible
//!   over our few-hundred-metre view distance).
//!
//! The math driver is [`ClientRuntime::world_time`]. The server owns it and the
//! client integrates between snapshots, by the time these systems run the
//! value is the live mirror.
//!
//! ## Tuning knobs
//!
//! Night brightness and the day/night balance live in the `const`s at the top
//! of this file (grouped so the look can be dialed in without touching logic).
//! Daytime ambient strength is the `AtmosphereEnvironmentMapLight` intensity on
//! the camera (`ATMOSPHERE_AMBIENT_INTENSITY` in `assets.rs`).

use bevy::{
    light::{CascadeShadowConfigBuilder, NotShadowCaster, SunDisk},
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
};

use crate::{
    app::state::{ClientRuntime, MenuState},
    world_time::{SECONDS_PER_DAY, WorldTime},
};

use super::components::MainCamera;

/// Apparent radius of the sky dome the moon visual rides on. The camera's far
/// plane is 300 m, so the moon stays comfortably inside it. The moon material
/// disables fog, so this distance only sets its apparent size, not its haze.
const SKY_DISTANCE: f32 = 140.0;

/// Visible radius of the moon disc. The sun is drawn by the atmosphere's
/// built-in [`SunDisk`]; the moon has no atmospheric equivalent, so it stays a
/// hand-placed emissive sphere.
const MOON_DISC_RADIUS: f32 = 4.2;

/// Tilt of the solar/lunar plane off the world's east-west axis, giving the
/// sun an oblique, more cinematic track across the sky.
const CELESTIAL_TILT_DEGREES: f32 = 18.0;

/// Peak daylight illuminance (lux) for the sun directional light. Kept at a
/// daylight-calibrated value (≈ `AMBIENT_DAYLIGHT`) rather than physical
/// `RAW_SUNLIGHT` + a manual camera `Exposure`: the atmosphere still renders the
/// sky and filters/tints the light toward the horizon, but this value keeps the
/// scene at a consistent brightness across the whole day under the renderer's
/// default exposure, which suits a stylised game with a fixed, gameplay-fair
/// night far better than raw sunlight (which really wants auto-exposure).
const SUN_PEAK_ILLUMINANCE: f32 = 11_000.0;

/// Peak moonlight illuminance. Real moonlight is ~0.05 lux; we cheat up hugely
/// so the player can navigate at night. Tuning knob for night brightness.
const MOON_PEAK_ILLUMINANCE: f32 = 1_300.0;

/// `GlobalAmbientLight` brightness at deep night. During the day this fades to
/// zero and the atmosphere environment map supplies ambient instead; at night
/// the atmosphere sky is dark, so this fixed floor is what lets the player
/// navigate. Raise it for brighter nights, lower it for moodier ones.
const NIGHT_AMBIENT_FLOOR: f32 = 90.0;

/// Cool blue-grey tint of the night ambient floor. Sells "moonlit" without
/// reading as overcast daytime.
const NIGHT_AMBIENT_COLOR: Vec3 = Vec3::new(0.40, 0.50, 0.72);

/// Sun direction shifts from "below horizon" through dawn into "up". This is
/// the half-width of the band (in sun-height units) over which day fades to
/// night, anchored at the horizon so dawn/dusk linger a beat.
const HORIZON_FADE_BAND: f32 = 0.18;

/// Real-time cadence at which the directional light's transform is allowed to
/// change at the default `1×` multiplier (~15 Hz). Light colour/illuminance
/// still update every frame; only the transform (which drives the shadow
/// projection) is throttled to avoid per-frame shadow shimmer.
const SHADOW_UPDATE_BASE_INTERVAL_SECS: f32 = 1.0 / 15.0;

/// Lower bound on the shadow update interval (~60 Hz).
const SHADOW_UPDATE_MIN_INTERVAL_SECS: f32 = 1.0 / 60.0;

/// Time of day the menu backdrop's sky is pinned to. The gameplay day/night
/// clock ([`ClientRuntime::world_time`]) only ticks in-game and keeps the
/// server's last time after you leave a session, so reading it on the title
/// screen makes the backdrop look as if time passed while you were away. The
/// menu renders this fixed time instead, so the title screen is identical on
/// every visit. Pinned, not ticked: the menu never cycles.
///
/// Early morning, the look that suits the backdrop best. Nudged to 7:30 (just
/// past the 7am [`DEFAULT_START_SECONDS`] the game launches with) so the sun is a
/// touch higher than the very low 7am angle, easing the side-light across the
/// field without losing the morning mood. The remaining directionality reads as a
/// soft gradient, not a hard split, because the grass/prop cel shader floors deep
/// shadow against the real shade rather than crushing it to near-black.
const MENU_BACKDROP_SECONDS: f32 = 7.5 * 3600.0;

#[derive(Component)]
pub(crate) struct SunLight;

#[derive(Component)]
pub(crate) struct MoonLight;

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

type MoonVisualQuery<'w, 's> = Query<
    'w,
    's,
    &'static mut Transform,
    (
        With<MoonVisual>,
        Without<SunLight>,
        Without<MoonLight>,
        Without<MainCamera>,
    ),
>;

type FogQuery<'w, 's> = Query<'w, 's, &'static mut DistanceFog, With<MainCamera>>;

/// Spawn the directional lights and the moon disc visual. Called from
/// `setup_scene` after the camera exists.
pub(crate) fn setup_sky(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    // Sun: shadow-casting directional light. Colour stays neutral white, the
    // atmosphere filters it through the air, warming it toward the horizon, so
    // tinting it here too would double-count. `SunDisk` makes the atmosphere
    // draw the visible solar disc (which bloom then makes glow).
    commands.spawn((
        Name::new("Sun"),
        SunLight,
        DirectionalLight {
            illuminance: SUN_PEAK_ILLUMINANCE * 0.5,
            color: Color::WHITE,
            shadows_enabled: true,
            shadow_depth_bias: 0.10,
            shadow_normal_bias: 1.8,
            ..default()
        },
        SunDisk::EARTH,
        // Default cascade config goes out to 150 m, sized for AAA open worlds.
        // Our playspace is ~80 m across so trimming to 100 m gives every shadow
        // texel ~33% more on-screen resolution with no visible difference.
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            maximum_distance: 100.0,
            first_cascade_far_bound: 8.0,
            ..default()
        }
        .build(),
        Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Moon: dim cool directional light. Shadows off, a second shadow map is
    // expensive and moonlit shadows in stylised low-poly art read as noise.
    // This is the documented way to do nighttime with the atmosphere: a dim
    // directional light for the moon while the sun sits below the horizon.
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

    // Three ico subdivisions (320 tris) instead of Bevy's 720-tri default:
    // the silhouette stays round under bloom and the disc is a single
    // unlit emissive blob, so the extra subdivision levels buy nothing.
    let moon_visual_mesh = meshes.add(
        Sphere::new(MOON_DISC_RADIUS)
            .mesh()
            .ico(3)
            .expect("valid subdivisions"),
    );
    let moon_visual_material = materials.add(StandardMaterial {
        // A faint warm tint on the surface lit colour reads as moon regolith
        // without needing a texture. The emissive is bright enough that the disc
        // reads as a silver moon against the atmosphere's dark night sky, with a
        // little headroom to bloom.
        base_color: Color::srgb(0.95, 0.95, 0.92),
        emissive: Color::srgb(3.4, 3.7, 4.6).to_linear(),
        unlit: true,
        fog_enabled: false,
        ..default()
    });

    commands.spawn((
        Name::new("Moon Visual"),
        MoonVisual,
        Mesh3d(moon_visual_mesh),
        MeshMaterial3d(moon_visual_material),
        Transform::IDENTITY,
        NotShadowCaster,
    ));
}

/// Distance fog the gameplay camera should carry. Colour and falloff are
/// updated per-frame; this is just the initial component so the camera owns the
/// slot before the lighting system runs.
pub(crate) fn initial_distance_fog() -> DistanceFog {
    DistanceFog {
        color: Color::srgb(0.55, 0.65, 0.78),
        directional_light_color: Color::srgba(1.0, 0.92, 0.78, 0.5),
        directional_light_exponent: 30.0,
        // Squared falloff, not plain exponential: both hit 5% contrast at
        // the visibility distance, but the squared curve stays nearly clear
        // through the 0-40m gameplay range (plain exponential already mixed
        // ~30% fog into a prop at 17m, washing everything toward pastel) and
        // ramps up near the horizon where the haze belongs.
        falloff: FogFalloff::from_visibility_squared(190.0),
    }
}

/// Drives every per-frame day/night change: sun/moon light direction, colour,
/// and intensity; the night ambient floor; the fog curtain; and the moon disc
/// position. Runs in `ClientSystemSet::Sky`, after the network tick (so
/// `runtime.world_time` is fresh) and after the camera follow (so the moon
/// anchors to the latest camera pose).
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_sky_system(
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut shadow_throttle: Local<f32>,
    mut ambient: ResMut<GlobalAmbientLight>,
    camera: CameraTransformQuery,
    mut sun_light: SunLightQuery,
    mut moon_light: MoonLightQuery,
    mut moon_visual: MoonVisualQuery,
    mut fog: FogQuery,
) {
    // Only gameplay reads the live day/night clock. The menu backdrop renders a
    // fixed time of day so its sky never drifts between launches or after
    // returning from a session. See `MENU_BACKDROP_SECONDS`.
    let world_time = if menu.screen.uses_menu_backdrop() {
        WorldTime {
            seconds_of_day: MENU_BACKDROP_SECONDS,
            multiplier: 0.0,
        }
    } else {
        runtime.world_time
    };

    let lighting = compute_lighting(&world_time);

    // Real-time throttle for the directional lights' *transform*. The shadow
    // projection is the part the eye reads as "shimmery" when updated every
    // frame; throttling its writes gives windows of fully stable shadows. The
    // interval scales with the world-time multiplier (see
    // `shadow_update_interval`) so each tick represents a roughly constant
    // angle. Colour and illuminance still update every frame.
    let interval = shadow_update_interval(world_time.multiplier);
    *shadow_throttle += time.delta_secs();
    let advance_shadows = *shadow_throttle >= interval;
    if advance_shadows {
        *shadow_throttle -= interval;
        if *shadow_throttle > interval {
            *shadow_throttle = 0.0;
        }
    }

    if let Ok((mut light, mut transform)) = sun_light.single_mut() {
        // Neutral white; the atmosphere applies the warm horizon tint.
        light.color = Color::WHITE;
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

    if let Ok(mut fog) = fog.single_mut() {
        fog.color = vec3_to_color(lighting.fog_color);
        // Squared falloff to keep the near field clear; see
        // `initial_distance_fog`.
        fog.falloff = FogFalloff::from_visibility_squared(lighting.fog_distance);
    }

    // Moon visual follows the camera at a fixed dome radius so it feels
    // infinitely distant regardless of where the player walks.
    if let Ok(camera_transform) = camera.single()
        && let Ok(mut transform) = moon_visual.single_mut()
    {
        transform.translation =
            camera_transform.translation + lighting.moon_direction * SKY_DISTANCE;
    }
}

#[derive(Debug, Clone, Copy)]
struct LightingFrame {
    /// Continuous sun direction. Drives the directional-light transform (when
    /// the shadow throttle fires) and, via that light, the atmosphere.
    sun_direction: Vec3,
    /// Continuous moon direction (always the antipode of the sun).
    moon_direction: Vec3,
    sun_illuminance: f32,
    moon_illuminance: f32,
    moon_color: Color,
    ambient_color: Vec3,
    ambient_brightness: f32,
    fog_color: Vec3,
    fog_distance: f32,
}

fn compute_lighting(time: &WorldTime) -> LightingFrame {
    let fraction = (time.seconds_of_day / SECONDS_PER_DAY).rem_euclid(1.0);
    let sun_direction = celestial_direction(fraction);
    let moon_direction = -sun_direction;

    let sun_height = sun_direction.y;
    let moon_height = moon_direction.y;

    // Smooth day/night factor in [0, 1], anchored just below the horizon so the
    // dawn/dusk transition lingers instead of flicking.
    let day_strength = smoothstep(-HORIZON_FADE_BAND, HORIZON_FADE_BAND, sun_height);

    // Sun direct illuminance: 0 below the horizon, ramping to peak well above.
    // The pow keeps the sun gentle and shadows long in the first hour.
    let sun_elevation = sun_height.max(0.0).clamp(0.0, 1.0);
    let sun_illuminance = SUN_PEAK_ILLUMINANCE * sun_elevation.powf(0.55);

    // Moonlight only takes over once the sun has set.
    let moon_elevation = moon_height.max(0.0).clamp(0.0, 1.0);
    let moon_illuminance = MOON_PEAK_ILLUMINANCE * moon_elevation.powf(0.6) * (1.0 - day_strength);
    let moon_color = Color::srgb(0.70, 0.78, 1.00);

    // Ambient floor: zero during the day (the atmosphere environment map does
    // the daytime ambient), ramping up to the fixed night floor after dark.
    let ambient_brightness = NIGHT_AMBIENT_FLOOR * (1.0 - day_strength);
    let ambient_color = NIGHT_AMBIENT_COLOR;

    // Fog curtain: matches a desaturated horizon tone, tightening at night so
    // the player can't see across the world in the dark. Sized so distant
    // chunks fade fully into the sky (squared fog is opaque by ~260 m by day,
    // sooner at dusk/night) well before the 300 m far plane clips them, so no
    // half-faded geometry is ever hard-cut at the frustum edge. NOTE: the day
    // value is the practical view distance; pushing it much past the AoI
    // streaming ring (View Distance tier, ~130-190 m on Medium) would reveal
    // the streaming edge, and past ~260 m would need a larger far plane.
    let day_fog = Vec3::new(0.55, 0.65, 0.78);
    let night_fog = Vec3::new(0.05, 0.07, 0.13);
    let fog_color = lerp_vec3(night_fog, day_fog, day_strength);
    let fog_distance = lerp(105.0, 190.0, day_strength);

    LightingFrame {
        sun_direction,
        moon_direction,
        sun_illuminance,
        moon_illuminance,
        moon_color,
        ambient_color,
        ambient_brightness,
        fog_color,
        fog_distance,
    }
}

/// Build the unit-length sun direction (origin → sun) for the given day
/// fraction in `[0, 1)`. The plane is tilted off east-west so the sun doesn't
/// march along a straight axis-aligned path.
fn celestial_direction(day_fraction: f32) -> Vec3 {
    // Theta = 0 at sunrise (east horizon), π/2 at noon (overhead), π at sunset,
    // 3π/2 below the horizon.
    let theta = (day_fraction - 0.25) * std::f32::consts::TAU;
    let east_west = theta.cos();
    let up = theta.sin();

    let tilt = CELESTIAL_TILT_DEGREES.to_radians();
    let tilt_cos = tilt.cos();
    let tilt_sin = tilt.sin();

    // Rotate the east-up plane around the world's east axis by the tilt.
    Vec3::new(east_west, up * tilt_cos, up * tilt_sin).normalize_or_zero()
}

fn directional_light_transform(direction_to_source: Vec3) -> Transform {
    // The directional light's local -Z is the direction light *travels*. We
    // want light to travel from the celestial body toward the ground, so we
    // place the entity at `+direction` and orient it back at the origin. The
    // translation is meaningless for an infinite directional light; only the
    // rotation matters.
    let position = direction_to_source.normalize_or_zero() * 100.0;
    if position.length_squared() < 1.0 {
        return Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y);
    }
    Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y)
}

/// How long to wait between writes to the directional light's transform, given
/// the current world-time multiplier. Scales inversely so each tick represents
/// roughly the same angular change at any speed; saturates at the ~60 Hz floor
/// for fast-forward and inherits the base interval for slow-motion.
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
    fn sun_illuminance_is_dim_at_night_and_bright_at_noon() {
        let day = compute_lighting(&time_at(12.0));
        let night = compute_lighting(&time_at(0.0));
        assert!(night.sun_illuminance < 1.0);
        assert!(day.sun_illuminance > 100.0);
    }

    #[test]
    fn night_has_an_ambient_floor_and_day_relies_on_the_atmosphere() {
        let day = compute_lighting(&time_at(12.0));
        let night = compute_lighting(&time_at(0.0));
        // Daytime ambient comes from the atmosphere environment map, so the
        // GlobalAmbientLight floor fades to ~zero.
        assert!(day.ambient_brightness < 1.0);
        // Night keeps a non-zero floor so the player can navigate.
        assert!(night.ambient_brightness > 5.0);
    }

    #[test]
    fn moon_provides_some_illumination_at_night() {
        let night = compute_lighting(&time_at(0.0));
        assert!(night.moon_illuminance > 10.0);
        let noon = compute_lighting(&time_at(12.0));
        assert!(noon.moon_illuminance < 1.0);
    }

    #[test]
    fn day_fraction_wraps_around_seconds() {
        let time = WorldTime {
            seconds_of_day: SECONDS_PER_DAY + 1.0,
            multiplier: 1.0,
        };
        let lighting = compute_lighting(&time);
        // Just-after-midnight should look like midnight: sun very low.
        assert!(lighting.sun_illuminance < 10.0);
    }

    #[test]
    fn fog_tightens_at_night() {
        let day = compute_lighting(&time_at(12.0));
        let night = compute_lighting(&time_at(0.0));
        assert!(night.fog_distance < day.fog_distance);
    }

    #[test]
    fn menu_backdrop_time_is_a_lit_morning() {
        // The title screen pins the sky to this fixed early-morning time instead
        // of the live gameplay clock. Guard that it reads as daylight (sun above
        // the horizon), not an accidental midnight.
        let lighting = compute_lighting(&WorldTime {
            seconds_of_day: MENU_BACKDROP_SECONDS,
            multiplier: 0.0,
        });
        assert!(
            lighting.sun_direction.y > 0.1,
            "menu sun should be above the horizon, got y={}",
            lighting.sun_direction.y
        );
        assert!(lighting.sun_illuminance > 100.0, "menu sun should be up");
    }

    #[test]
    fn directional_light_transform_does_not_panic_for_origin() {
        let t = directional_light_transform(Vec3::ZERO);
        assert!(t.translation.is_finite());
    }

    #[test]
    fn shadow_update_interval_scales_inversely_with_multiplier() {
        let base = shadow_update_interval(1.0);
        assert!((base - SHADOW_UPDATE_BASE_INTERVAL_SECS).abs() < 1e-6);

        let two_x = shadow_update_interval(2.0);
        assert!((two_x - SHADOW_UPDATE_BASE_INTERVAL_SECS / 2.0).abs() < 1e-4);
        assert!(two_x > SHADOW_UPDATE_MIN_INTERVAL_SECS);

        let fast = shadow_update_interval(60.0);
        assert!((fast - SHADOW_UPDATE_MIN_INTERVAL_SECS).abs() < 1e-6);
        let extreme = shadow_update_interval(10_000.0);
        assert!((extreme - SHADOW_UPDATE_MIN_INTERVAL_SECS).abs() < 1e-6);

        let slow = shadow_update_interval(0.5);
        assert_eq!(slow, SHADOW_UPDATE_BASE_INTERVAL_SECS);
        let paused = shadow_update_interval(0.0);
        assert_eq!(paused, SHADOW_UPDATE_BASE_INTERVAL_SECS);
    }
}
