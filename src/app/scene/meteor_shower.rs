//! World-space meteor shower impact visuals: per meteor, a shallow dug-in
//! crater with a fading painted burn skirt, strewn with LIVE particle fires
//! that burn for the first minute or two after the strike, then die out and
//! leave only the crater for the rest of the window. A shower lands 4 to 5
//! size-varied meteors, so several crater rigs (keyed by each meteor's
//! `impact_tick`) can be live at once, each uniformly scaled by its meteor's
//! `size` (the shared crater profile is a pure homothety of size, so the
//! scaled mesh still matches the movement floor point for point).
//!
//! Entirely client-side and derived from the event state
//! (`runtime.meteor_showers`): there is no replicated crater entity and no
//! save bump. Each site appears the instant the local clock passes its
//! meteor's `impact_tick` and is removed when that meteor clears (crater
//! despawn window, matching the server). Because the announce is resent to
//! late joiners while the event is alive, a player who connects during a
//! crater phase gets the announce and this system draws the sites for them
//! too (fires only if the burn window is still open on their clock).
//!
//! The site has three parts, each deliberately transient except the crater:
//!
//! - **The crater** ([`build_crater_mesh`]): one vertex-coloured mesh. The
//!   terrain plane cannot be cut, so the "dug into the ground" read comes from
//!   a raised irregular rim lip around a floor at grade; beyond the rim a flat
//!   burn skirt overlays the grass, char-dark near the bowl and fading to
//!   nothing at the outer edge (the decal ramp gets stronger toward the
//!   impact). Persists the whole crater window.
//! - **Fires**: `FIRE_CLUSTER_COUNT` emitter points, a few down in the bowl and
//!   the rest scattered over the burn skirt, each shedding furnace-style rising
//!   flame puffs + embers (shared `FurnaceFireAssets` +
//!   `tick_furnace_particles_system` integrator) and carrying a flickering
//!   shadowless `PointLight`. They burn at full blaze for
//!   `METEOR_SHOWER_SITE_FIRE_SECONDS`, ramp down over the fade tail, then despawn
//!   (see [`animate_meteor_shower_site_fire_system`]).
//! - **The one-time rock blast**, fired only when the impact JUST happened: a
//!   fountain of fixed-size matte grey/brown boulder chunks (wide size spread)
//!   launched up and out under realistic gravity (`ImpactChip` physics: arc,
//!   land, bounce, settle for a beat, then despawn), a brief ground fireball
//!   flash, and a momentary flash light. No glowing tumbling chunks (spinning
//!   emissive cubes strobed); nothing from the blast persists.
//!
//! This module also owns the strike cues: the impact boom is PRE-ARMED
//! [`IMPACT_BOOM_LEAD_S`] before the strike so the file's baked lead-in ends on
//! the impact frame, the flyby bed starts at a fixed [`FLYBY_LEAD_S`] so its
//! silent tail lands on the strike (and fades rather than cuts if it must stop
//! early), and the strike fires a distance-scaled camera kick.
//!
//! Kept cheap: the crater is one static mesh; each fire cluster is one emitter
//! plus one clustered shadowless light, and its particles are the same
//! capped-lifetime additive puffs the furnace sheds; the blast debris
//! self-despawns.

use bevy::{
    asset::RenderAssetUsages,
    audio::{AudioSink, AudioSinkPlayback, Volume},
    light::NotShadowCaster,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::{
    app::{
        audio::{PlaySound, ScheduledSounds, SoundId, SoundLibrary, spawn_managed_sound},
        scene::FurnaceFireAssets,
        state::{ClientRuntime, ClientSettings, MenuState},
        systems::{CameraImpactKick, FurnaceParticle, effects::ImpactChip, furnace_flicker},
    },
    game_balance::{
        METEOR_SHOWER_IMPACT_RADIUS_M, METEOR_SHOWER_SITE_FIRE_FADE_SECONDS,
        METEOR_SHOWER_SITE_FIRE_SECONDS,
    },
    util::hash::hashed_unit,
    world::{CRATER_BOWL_RADIUS_M, CRATER_RIM_END_M, CRATER_SKIRT_RADIUS_M, crater_surface_height},
};

/// Seconds into `world/meteor-impact.wav` at which the boom itself lands. The
/// file opens with a rising-roar lead-in and its RMS envelope peaks at ~3.05 s
/// (first crosses 60% at ~2.85 s). The cue is started this far BEFORE the
/// visual impact so the roar swells through the final descent and the BOOM
/// lands on the strike frame; starting the file at impact put the boom seconds
/// late, which read as a mistimed clap.
const IMPACT_BOOM_LEAD_S: f32 = 2.95;

/// Seconds before impact the flyby crossing bed is started: the moment the
/// fireball becomes visible (`METEOR_FLIGHT_SECONDS`), so sight and sound
/// arrive together for every meteor, exactly the `/meteor-here` feel the
/// owner tuned against (its short warning always started the bed at spawn).
/// The 18.1 s `world/meteor-flyby.wav` therefore still has tail left when the
/// strike lands; that reads as the pass's rolling echo under the boom, the
/// same overlap `/meteor-here` has always played. (The previous fixed 17.5 s
/// lead predated the short flight window and left the audio trailing the
/// visual by several seconds on scheduled events; owner report.)
const FLYBY_LEAD_S: f32 = crate::world::METEOR_FLIGHT_SECONDS;

/// Per-second fade applied to the flyby bed when it must be torn down while
/// still audible (menu opened, event replaced, late-join edge). ~0.25 s to
/// silence, then the entity despawns: never a mid-waveform cut.
const FLYBY_FADE_OUT_PER_S: f32 = 4.0;

/// Proximity model for the meteor's crossing rumble + slight camera shake. Both
/// swell as the fireball's true world position nears the listener: full inside
/// `METEOR_RUMBLE_FULL_M`, tapering with distance to a floor of
/// `METEOR_RUMBLE_FAR_FLOOR` at `METEOR_RUMBLE_RANGE_M` and beyond. The floor
/// is deliberate: a meteor is a huge object, so its crossing stays AUDIBLE from
/// anywhere in the world, just quiet; only the volume is distance-based, capped
/// at the tuned near level (owner requirement). Kept in one place so the
/// audible rumble and the felt shake ramp together off the same distance.
const METEOR_RUMBLE_RANGE_M: f32 = 2_500.0;
const METEOR_RUMBLE_FULL_M: f32 = 120.0;
const METEOR_RUMBLE_FAR_FLOOR: f32 = 0.07;

/// Peak linear volume scale for the crossing rumble loop at closest approach. The
/// loop's manifest base gain is already low; this scales it further so even an
/// overhead pass sits under the impact thump.
const METEOR_RUMBLE_PEAK_VOLUME: f32 = 1.0;

/// Marks one meteor's impact-site visual rig (crater mesh + fire emitters), so
/// it can be found and despawned as a unit when its meteor ends. Keyed by the
/// meteor's `impact_tick` so a multi-meteor shower keeps one rig per landed
/// meteor. The one-time rock blast is thrown as free-standing `ImpactChip`
/// debris that self-despawns, so it is not parented here.
#[derive(Component)]
pub(crate) struct MeteorShowerCrater {
    /// The owning meteor's impact tick (its identity within the event).
    impact_tick: u64,
}

/// Marker + emitter state for one scattered fire at an impact site. A child of
/// its crater rig carrying the fire's `PointLight`; while the site's burn window
/// is open it sheds furnace-style flame puffs and embers each frame (see
/// [`animate_meteor_shower_site_fire_system`]), then despawns when the fire dies.
#[derive(Component)]
pub(crate) struct MeteorShowerSiteFire {
    /// The owning meteor's impact tick, so the burn-out envelope reads the
    /// right meteor's age in a multi-meteor shower.
    impact_tick: u64,
    /// Seconds until the next flame-puff emission.
    flame_cooldown: f32,
    /// Seconds until the next rising-ember emission.
    spark_cooldown: f32,
    /// Free-running phase offset so the fires flicker out of sync with each
    /// other instead of pulsing in lockstep.
    phase: f32,
    /// Per-fire size multiplier on particle scale/loft and light output, so the
    /// site mixes small licks and real blazes (already folded with the meteor's
    /// size at spawn).
    scale: f32,
    /// World-space distance from the (rig-scaled) emitter entity down to just
    /// above the ground, where the flames are born. Precomputed at spawn
    /// because the emitter's lift is scaled by the rig's size transform.
    anchor_drop: f32,
}

/// Per-meteor impact-cue flags so the pre-armed boom and the strike camera
/// kick each fire exactly once per meteor.
#[derive(Default, Clone, Copy)]
struct MeteorCueFlags {
    /// The boom cue has been scheduled (pre-armed [`IMPACT_BOOM_LEAD_S`] before
    /// the strike so the file's baked lead-in ends ON the strike).
    impact_played: bool,
    /// The one-off strike camera kick has fired (at the impact frame itself).
    impact_shake_fired: bool,
}

/// Impact-cue bookkeeping across the shower's meteors, keyed by each meteor's
/// `impact_tick`; entries for dead meteors are pruned so a long session never
/// grows the map. The crossing rumble is a separate one-shot the renderer owns
/// (see [`meteor_shower_rumble_system`]); there is no separate approach roar
/// (the flyby bed carries the whole approach).
#[derive(Default)]
pub(crate) struct MeteorShowerCueState {
    per_meteor: std::collections::HashMap<u64, MeteorCueFlags>,
}

/// How many distinct fire emitter points strew the impact site. Each is a
/// furnace-style particle fire plus its own shadowless `PointLight`, so the site
/// reads as many separate burning patches (not one painted ring) that genuinely
/// glow and light the ground at night.
const FIRE_CLUSTER_COUNT: u32 = 12;

/// Global budget of LIVE site-fire point lights across every burning impact
/// site. One site never exceeds it (`FIRE_CLUSTER_COUNT` at size 1.0), but a
/// multi-meteor shower can have 4-5 sites blazing at once, which is up to
/// ~40-60 concurrent shadowless lights, a real forward-pass cost for glow the
/// player can't see from across the map. Each frame the nearest fires to the
/// camera keep their animated lights; the rest keep shedding flame particles
/// (which read fine at distance) with their light zeroed. Matches the old
/// single-site worst case, so night scenes cost what they did before the
/// multi-meteor rework.
const FIRE_LIGHT_BUDGET: usize = 12;

// The crater's GEOMETRY (radii, heights, surface profile) lives in
// `crate::world::meteor_shower` (`CRATER_*_M`, `crater_surface_height`): the
// movement collider's analytic floor and the server's shard placement sample
// the SAME surface this mesh draws, so what you see is what you stand on.
// Only the tessellation is a render concern:
/// Radial rings and angular segments of the crater meshes. Coarse enough to
/// stay cheap, fine enough that the seeded jitter reads as a broken, natural
/// rim. Ring 0 is the centre point; ring [`CRATER_BOWL_LAST_RING`] (at
/// `CRATER_RIM_END_M`) is shared by the OPAQUE bowl mesh and the translucent
/// burn-skirt mesh with identical seeded jitter, so the two join seamlessly.
const CRATER_RINGS: usize = 10;
const CRATER_SEGMENTS: usize = 48;
const CRATER_BOWL_LAST_RING: usize = 6;

/// How many rock/stone debris chunks the one-time impact blast flings outward.
/// Much larger than the explosive-charge burst: this is a meteor strike, and the
/// rock-and-stone blast is meant to be the star of the impact moment.
const IMPACT_DEBRIS_COUNT: u32 = 110;

/// If the crater rig first appears more than this many seconds after the strike
/// (a late joiner, or a client that had the menu backdrop up), the one-time rock
/// blast is skipped: the fountain only makes sense at the impact moment.
const IMPACT_BLAST_WINDOW_S: f32 = 2.0;

/// Radius (metres) of each fire cluster's shadowless `PointLight`. Wide enough to
/// pool warm light on the surrounding scorched ground at night.
const FIRE_LIGHT_RANGE_M: f32 = 18.0;

/// Base lumen output of each fire cluster's light, so the burning patches visibly
/// cast light on the night ground and the site reads as fire, not an inert dark
/// stain. The per-frame flicker and the burn-out fade both scale this down.
const FIRE_LIGHT_INTENSITY: f32 = 26_000.0;

/// Seconds between flame-puff emissions per fire cluster. Each emission sheds
/// several puffs; an open ground fire is a bigger body of flame than a furnace
/// mouth, so the puffs themselves are scaled up rather than the rate.
const SITE_FLAME_INTERVAL: f32 = 0.05;

/// Flame puffs shed per emission per cluster. Three small puffs per emission
/// keep the knot dense now that each puff is only ~0.2-0.4 m.
const SITE_FLAMES_PER_EMISSION: u32 = 3;

/// Seconds between rising-ember emissions per fire cluster, far sparser than the
/// flame so embers read as occasional flecks lofting off the blaze.
const SITE_SPARK_INTERVAL: f32 = 0.13;

/// Spawn and tear down the per-meteor crater rigs from the event state: one
/// rig per live, already-impacted meteor, keyed by `impact_tick`. Runs in
/// `ClientSystemSet::Sky`; a no-op on the title backdrop (no world) and
/// whenever no meteor has impacted yet.
pub(crate) fn update_meteor_shower_ground_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<(Entity, &MeteorShowerCrater)>,
) {
    // Which meteors should show a crater? Only impacted, still-live ones, and
    // never on the title backdrop.
    let now = runtime.server_tick();
    let wanted: Vec<&crate::app::state::MeteorShowerEvent> = if menu.screen.uses_menu_backdrop() {
        Vec::new()
    } else {
        runtime
            .meteor_showers
            .iter()
            .filter(|event| event.has_impacted(now))
            .collect()
    };

    // Tear down rigs whose meteor is gone (event ended or backdrop opened).
    let mut live_rigs: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for (entity, crater) in existing.iter() {
        if wanted
            .iter()
            .any(|event| event.impact_tick == crater.impact_tick)
        {
            live_rigs.insert(crater.impact_tick);
        } else {
            info!(
                "meteor_shower: crater rig {} despawned (meteor ended or backdrop)",
                crater.impact_tick
            );
            commands.entity(entity).despawn();
        }
    }

    // Spawn a rig for each impacted meteor that lacks one. Usually the first
    // such frame IS the impact moment, but a late joiner (or a client
    // returning from the backdrop) spawns it mid-window: the site age gates
    // the one-time blast and the fires so they never replay stale.
    for event in wanted {
        if live_rigs.contains(&event.impact_tick) {
            continue;
        }
        let position = event.impact_position;
        let age_seconds = ((runtime.server_tick_precise() - event.impact_tick as f64)
            / f64::from(crate::protocol::SERVER_TICK_RATE_HZ)) as f32;
        info!(
            "meteor_shower: crater rig {} spawned at ({:.1}, {:.1}), size {:.2}, site age \
             {age_seconds:.1}s",
            event.impact_tick, position.x, position.z, event.size
        );
        spawn_crater(
            &mut commands,
            &mut meshes,
            &mut materials,
            Vec3::new(position.x, position.y, position.z),
            age_seconds,
            event.impact_tick,
            event.size,
        );
    }
}

/// The site fires' burn-out envelope at `age_seconds` after impact: full blaze
/// until the fade window opens, a linear ramp to zero across
/// [`METEOR_SHOWER_SITE_FIRE_FADE_SECONDS`], and dead once
/// [`METEOR_SHOWER_SITE_FIRE_SECONDS`] is up. Pure so the curve is unit-testable.
fn site_fire_intensity(age_seconds: f32) -> f32 {
    if !age_seconds.is_finite() || age_seconds < 0.0 {
        return 0.0;
    }
    let fade_start = METEOR_SHOWER_SITE_FIRE_SECONDS - METEOR_SHOWER_SITE_FIRE_FADE_SECONDS;
    if age_seconds <= fade_start {
        return 1.0;
    }
    (1.0 - (age_seconds - fade_start) / METEOR_SHOWER_SITE_FIRE_FADE_SECONDS).clamp(0.0, 1.0)
}

/// Per-frame work for the impact site's scattered fires: flicker each fire's
/// light and shed furnace-style flame puffs + rising embers, all scaled by the
/// burn-out envelope so the blaze thins, shrinks, and dims across the fade tail
/// instead of cutting out. Despawns each fire (light included) once the envelope
/// hits zero, leaving only the crater for the rest of the window. The
/// particles ride the shared `tick_furnace_particles_system` integrator. Runs in
/// `ClientSystemSet::Sky`; a no-op whenever no site fires are alive.
pub(crate) fn animate_meteor_shower_site_fire_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    assets: Option<Res<FurnaceFireAssets>>,
    camera: super::sky::CameraTransformQuery,
    mut fires: Query<(
        Entity,
        &GlobalTransform,
        &mut MeteorShowerSiteFire,
        &mut PointLight,
    )>,
) {
    if fires.is_empty() {
        return;
    }
    let Some(assets) = assets else {
        return;
    };
    let dt = time.delta_secs().max(0.0);
    let t = time.elapsed_secs();
    let now = runtime.server_tick_precise();

    // Light budget: rank every live fire by distance to the camera and let
    // only the nearest FIRE_LIGHT_BUDGET keep a lit PointLight this frame
    // (see the constant for why). Fires past the budget still animate and
    // shed particles; only their light is zeroed. With no camera (menu
    // backdrop teardown frame) everything is treated as over-budget.
    let lit: std::collections::HashSet<Entity> = camera
        .single()
        .map(|camera_transform| {
            let eye = camera_transform.translation;
            let mut ranked: Vec<(f32, Entity)> = fires
                .iter()
                .map(|(entity, global, _, _)| (global.translation().distance_squared(eye), entity))
                .collect();
            ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
            ranked
                .into_iter()
                .take(FIRE_LIGHT_BUDGET)
                .map(|(_, entity)| entity)
                .collect()
        })
        .unwrap_or_default();

    // Per-meteor burn-out envelope off each site's own age. If a fire's meteor
    // vanished from under it (the rig teardown despawn is still queued this
    // frame), treat it as burnt out rather than freezing at full blaze.
    let intensity_for = |impact_tick: u64| -> f32 {
        runtime
            .meteor_showers
            .iter()
            .find(|event| event.impact_tick == impact_tick)
            .map(|event| {
                let age_seconds = ((now - event.impact_tick as f64)
                    / f64::from(crate::protocol::SERVER_TICK_RATE_HZ))
                    as f32;
                site_fire_intensity(age_seconds)
            })
            .unwrap_or(0.0)
    };

    for (entity, global, mut fire, mut light) in &mut fires {
        let intensity = intensity_for(fire.impact_tick);
        if intensity <= 0.0 {
            // Burnt out: remove the fire and its light, leave the crater.
            // `try_despawn`, not `despawn`: when the event ends (or the menu
            // backdrop opens) the crater rig teardown recursively despawns
            // these same fire children in this very frame, and whichever
            // command applies second would hit a dead entity and log a WARN
            // per fire (12 of them, every teardown).
            commands.entity(entity).try_despawn();
            continue;
        }
        let flicker = furnace_flicker(t, fire.phase);
        light.intensity = if lit.contains(&entity) {
            FIRE_LIGHT_INTENSITY * fire.scale * intensity * (0.7 + 0.55 * flicker)
        } else {
            // Over the global light budget this frame: flames only, no light.
            0.0
        };

        // The emitter entity sits lifted so its light pools on the ground around
        // it; the flames themselves are born at ground level under it. The lift
        // is scaled by the rig's size transform, so the drop is precomputed at
        // spawn.
        let anchor = global.translation() - Vec3::Y * fire.anchor_drop;
        let base_seed = t.to_bits() ^ fire.phase.to_bits();

        fire.flame_cooldown -= dt;
        if fire.flame_cooldown <= 0.0 {
            fire.flame_cooldown += SITE_FLAME_INTERVAL;
            for i in 0..SITE_FLAMES_PER_EMISSION {
                let seed = base_seed
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(i.wrapping_mul(2_246_822_519));
                // Emission also thins with the envelope, so a dying fire sheds
                // fewer puffs, not just smaller ones.
                if hashed_unit(seed ^ 0x00FA_DE00) > intensity {
                    continue;
                }
                spawn_site_flame(&mut commands, &assets, anchor, seed, fire.scale, intensity);
            }
        }

        fire.spark_cooldown -= dt;
        if fire.spark_cooldown <= 0.0 {
            fire.spark_cooldown += SITE_SPARK_INTERVAL;
            let seed = base_seed ^ 0x1234_5678;
            if hashed_unit(seed) <= intensity {
                spawn_site_spark(&mut commands, &assets, anchor, seed, fire.scale);
            }
        }
    }
}

/// One buoyant flame puff for a site fire: the furnace flame scaled up an order
/// of magnitude (an open ground blaze, not a furnace mouth), born across a small
/// disc and rising a metre or two before despawning. Rides the furnace-particle
/// integrator in fixed-scale mode (loft, drag, despawn; no size animation).
fn spawn_site_flame(
    commands: &mut Commands,
    assets: &FurnaceFireAssets,
    anchor: Vec3,
    seed: u32,
    fire_scale: f32,
    intensity: f32,
) {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x9E37_79B9);
    let r3 = hashed_unit(seed ^ 0x85EB_CA6B);

    let angle = r1 * std::f32::consts::TAU;
    let radius = r2 * 0.55 * fire_scale;
    let offset = Vec3::new(angle.cos() * radius, r3 * 0.12, angle.sin() * radius);
    // Mostly straight up with a slight outward lean so the body of flame tapers
    // as it climbs instead of rising as a straight tube.
    let outward = Vec3::new(angle.cos(), 0.0, angle.sin()) * (r1 * 0.35);
    let rise = (0.9 + r3 * 1.0) * fire_scale;
    let velocity = Vec3::Y * rise + outward;
    // The shared flame mesh is a 0.07 m sphere, so the puff lands around
    // 0.2-0.4 m: a knot of licks about a metre tall, NOT a glowing dome (the
    // first-round 0.5-0.9 m puffs merged into giant marshmallow blobs at
    // night). Four FIXED size variants held for the puff's whole life (no
    // shrink-out): the constant size churn of dozens of scaling puffs is what
    // read as flickering particle sizes on the crater. The burn-out envelope
    // still shrinks NEW puffs as the fire dies.
    let initial_scale = crate::app::systems::quantized_chip_scale(
        3.4 * fire_scale.min(1.1) * (0.55 + 0.45 * intensity),
        r2,
    );
    let lifetime = 0.45 + r1 * 0.4;

    commands.spawn((
        Name::new("MeteorShower Fire Flame"),
        FurnaceParticle::new(velocity, 0.3, 0.8, lifetime, initial_scale).with_fixed_scale(),
        Mesh3d(assets.flame_mesh.clone()),
        MeshMaterial3d(assets.flame_material.clone()),
        Transform::from_translation(anchor + offset).with_scale(Vec3::splat(initial_scale)),
        Visibility::Visible,
        NotShadowCaster,
    ));
}

/// A single rising ember off a site fire: higher, longer-lived, and heavier than
/// a flame puff so it lofts up out of the blaze, cools, and falls.
fn spawn_site_spark(
    commands: &mut Commands,
    assets: &FurnaceFireAssets,
    anchor: Vec3,
    seed: u32,
    fire_scale: f32,
) {
    let r1 = hashed_unit(seed);
    let r2 = hashed_unit(seed ^ 0x27D4_EB2F);
    let r3 = hashed_unit(seed ^ 0x1656_67B1);

    let offset = Vec3::new((r1 - 0.5) * 0.8, 0.1, (r3 - 0.5) * 0.8) * fire_scale;
    let drift = Vec3::new((r2 - 0.5) * 1.6, 0.0, (r1 - 0.5) * 1.6);
    let rise = (2.2 + r3 * 2.0) * fire_scale;
    let velocity = drift + Vec3::Y * rise;
    // Four fixed size variants, held for the spark's whole life (see the flame
    // above for why nothing here animates scale any more).
    let initial_scale = crate::app::systems::quantized_chip_scale(2.6 * fire_scale.min(1.1), r2);
    let lifetime = 0.7 + r1 * 0.7;

    commands.spawn((
        Name::new("MeteorShower Fire Spark"),
        FurnaceParticle::new(velocity, 2.2, 1.2, lifetime, initial_scale).with_fixed_scale(),
        Mesh3d(assets.spark_mesh.clone()),
        MeshMaterial3d(assets.spark_material.clone()),
        Transform::from_translation(anchor + offset).with_scale(Vec3::splat(initial_scale)),
        Visibility::Visible,
        NotShadowCaster,
    ));
}

/// The boom cue's gain offset for a meteor of `size`: the amplitude ratio in
/// decibels (`20 log10(size)`), so a 0.4 meteor thumps ~8 dB softer than the
/// headliner, floored so a tiny dev meteor stays audible. Pure so it is
/// unit-testable.
fn meteor_size_gain_db(size: f32) -> f32 {
    (20.0 * size.clamp(0.05, 1.0).log10()).max(-14.0)
}

/// Inside this listener distance the boom plays through the tuned SPATIAL path
/// (positional, at the crater) exactly as before: this is the loudness cap the
/// far model tapers down from.
const BOOM_SPATIAL_RANGE_M: f32 = 300.0;

/// Floor on the far boom's distance attenuation. Even a strike on the far side
/// of the world lands as a soft distant thump: a meteor impact is a
/// world-scale event and must be audible to everyone (owner requirement); only
/// the volume is distance-based.
const BOOM_FAR_FLOOR_DB: f32 = -34.0;

/// Attenuation slope of the far boom, in dB per decade of distance past the
/// spatial handoff. Steeper than the free-field inverse-distance law's 20:
/// the extra ~8 dB/decade stands in for air absorption and ground effect,
/// which real kilometre-scale blasts lose on top of geometric spreading (the
/// plain 20 left cross-map strikes too loud; owner report).
const BOOM_FAR_DB_PER_DECADE: f32 = -28.0;

/// Beyond this listener distance the boom plays the MUFFLED variant
/// (`SoundId::MeteorShowerImpactFar`, a double 450 Hz low-pass of the same
/// file): air absorption strips the crack long before it strips the thump, so
/// a far strike must sound dull, not merely quiet. Between the spatial
/// handoff and here the crisp file plays (quieter, slightly lagged).
const BOOM_MUFFLED_RANGE_M: f32 = 800.0;

/// Extra attenuation when the listener is SHELTERED toward the strike (a
/// solid collider within [`SHELTER_SCAN_RANGE_M`] blocks the bearing from
/// their ear to the impact, e.g. standing inside a walled base). Sheltered
/// listeners also always get the muffled variant, whatever the distance.
const BOOM_SHELTER_EXTRA_DB: f32 = -5.0;

/// How far from the listener the shelter check scans for occluding colliders.
/// Deliberately LOCAL: only structures around the listener are in their AoI
/// (the strike's own surroundings are not replicated to a far listener), and
/// "I am indoors / behind my wall" is also the perceptually meaningful case.
const SHELTER_SCAN_RANGE_M: f32 = 35.0;

/// Listener ear height above the feet for the shelter ray.
const SHELTER_EYE_HEIGHT_M: f32 = 1.6;

/// The far boom's distance gain: 0 dB at the spatial handoff range, then
/// [`BOOM_FAR_DB_PER_DECADE`] per decade, floored at [`BOOM_FAR_FLOOR_DB`].
/// Pure so the taper is unit-testable.
fn meteor_boom_distance_gain_db(distance: f32) -> f32 {
    if !distance.is_finite() || distance <= BOOM_SPATIAL_RANGE_M {
        return 0.0;
    }
    (BOOM_FAR_DB_PER_DECADE * (distance / BOOM_SPATIAL_RANGE_M).log10()).max(BOOM_FAR_FLOOR_DB)
}

/// The `ScheduledSounds` delay that lands the file's detonation peak (at
/// [`IMPACT_BOOM_LEAD_S`] into playback) EXACTLY at the visual impact frame,
/// regardless of which frame the trigger fired on (the old fixed-lead push
/// drifted by however far past the lead the triggering frame ran, which read
/// as the boom slightly missing the explosion; owner report). Clamped at zero
/// for a witness who arrives too late to pre-arm.
///
/// Deliberately NO speed-of-sound travel lag: an earlier pass delayed far
/// booms by `distance / 343 m/s` for physical realism, but a lagged boom
/// splits the impact into two events, the trees fell and the crater blast
/// fires at the strike, then the "actual impact" boom lands seconds later
/// (owner report: the felling read as happening BEFORE the impact). Distance
/// is expressed through gain + the muffled variant instead; the timing is
/// always the impact frame itself.
fn meteor_boom_push_delay(seconds_to_impact: f32) -> f32 {
    (seconds_to_impact - IMPACT_BOOM_LEAD_S).max(0.0)
}

/// Segment-vs-AABB slab test: does the ray from `origin` along the unit `dir`
/// hit `block` within `max_t` metres? Pure so the shelter check is
/// unit-testable.
fn segment_hits_block(
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
    block: &crate::world::WorldBlock,
) -> bool {
    let min = block.min();
    let max = block.max();
    let (mut t_enter, mut t_exit) = (0.0_f32, max_t);
    for axis in 0..3 {
        let (o, d, lo, hi) = match axis {
            0 => (origin.x, dir.x, min.x, max.x),
            1 => (origin.y, dir.y, min.y, max.y),
            _ => (origin.z, dir.z, min.z, max.z),
        };
        if d.abs() < 1e-6 {
            if o < lo || o > hi {
                return false;
            }
            continue;
        }
        let (mut near, mut far) = ((lo - o) / d, (hi - o) / d);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t_enter = t_enter.max(near);
        t_exit = t_exit.min(far);
        if t_enter > t_exit {
            return false;
        }
    }
    true
}

/// True when a solid collider near the LISTENER blocks the horizontal bearing
/// from their ear toward the strike: standing inside a walled base (or behind
/// one) toward the impact muffles and softens the boom. Scans only replicated
/// deployables within [`SHELTER_SCAN_RANGE_M`] (the strike's own surroundings
/// are outside a far listener's AoI, and local cover is the perceptually
/// meaningful case anyway); open doors don't block (their swung collider
/// clears the opening).
fn listener_sheltered_toward(
    runtime: &ClientRuntime,
    impact: Vec3,
    occluders: &Query<(
        &crate::server::Deployable,
        &crate::server::DeployableTransform,
        &crate::server::DeployableActive,
    )>,
) -> bool {
    let Some(view) = runtime.local_view() else {
        return false;
    };
    let eye = Vec3::new(
        view.position.x,
        view.position.y + SHELTER_EYE_HEIGHT_M,
        view.position.z,
    );
    let mut bearing = impact - eye;
    bearing.y = 0.0;
    let Some(dir) = bearing.try_normalize() else {
        return false;
    };
    let cull_sq = (SHELTER_SCAN_RANGE_M + 6.0) * (SHELTER_SCAN_RANGE_M + 6.0);
    occluders.iter().any(|(meta, transform, active)| {
        let dx = transform.position.x - eye.x;
        let dz = transform.position.z - eye.z;
        if dx * dx + dz * dz > cull_sq {
            return false;
        }
        crate::app::systems::deployable_colliders(meta, transform, active.0)
            .iter()
            .any(|block| segment_hits_block(eye, dir, SHELTER_SCAN_RANGE_M, block))
    })
}

/// Fire the meteor shower strike cues from the event state, each exactly once
/// PER METEOR (a shower staggers several strikes):
///
/// - **Boom** (spatial at the crater): started [`IMPACT_BOOM_LEAD_S`] BEFORE
///   each strike so the file's baked rising lead-in plays through the final
///   descent and its detonation peak lands on the visual impact frame. The
///   gain scales with the meteor's size.
/// - **Camera kick**: at each impact frame itself, distance-scaled via
///   `CameraImpactKick::trigger_meteor_impact` (felt from hundreds of metres,
///   the payoff the crossing tremor builds to). A small meteor kicks like a
///   proportionally more distant strike.
///
/// A late joiner who connects after a strike gets neither for it (the crater
/// is stale news). The approach is carried wholly by the crossing bed (see
/// [`meteor_shower_rumble_system`]). Gated on `!uses_menu_backdrop`.
pub(crate) fn meteor_shower_audio_system(
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut cue: Local<MeteorShowerCueState>,
    mut scheduled: ResMut<ScheduledSounds>,
    mut kick: ResMut<CameraImpactKick>,
    occluders: Query<(
        &crate::server::Deployable,
        &crate::server::DeployableTransform,
        &crate::server::DeployableActive,
    )>,
) {
    if menu.screen.uses_menu_backdrop() {
        return;
    }
    if runtime.meteor_showers.is_empty() {
        if !cue.per_meteor.is_empty() {
            cue.per_meteor.clear();
        }
        return;
    }
    // Prune flags for meteors that have cleaned up.
    cue.per_meteor.retain(|impact_tick, _| {
        runtime
            .meteor_showers
            .iter()
            .any(|event| event.impact_tick == *impact_tick)
    });

    let now = runtime.server_tick();
    for event in &runtime.meteor_showers {
        let flags = cue.per_meteor.entry(event.impact_tick).or_default();
        let seconds_to_impact = event.seconds_to_impact(now);
        let impact = Vec3::new(
            event.impact_position.x,
            event.impact_position.y,
            event.impact_position.z,
        );
        // Horizontal listener distance to ground zero, for the strike kick falloff.
        let impact_distance = runtime
            .local_view()
            .map(|view| {
                let dx = view.position.x - impact.x;
                let dz = view.position.z - impact.z;
                (dx * dx + dz * dz).sqrt()
            })
            .unwrap_or(f32::INFINITY);

        // Boom: pre-armed so the file's detonation peak lands exactly on the
        // visual impact frame (`meteor_boom_push_delay`; no sound-travel lag,
        // see its doc). Only when we are genuinely witnessing the strike
        // window (not a joiner arriving mid-crater, whose clock is already
        // well past impact). Gain scales with the meteor's size so a small
        // strike lands a smaller thump.
        //
        // Near (inside BOOM_SPATIAL_RANGE_M) keeps the tuned spatial path
        // unchanged; that loudness is the cap. Far strikes play NON-spatially
        // (Bevy's spatial rolloff would silence them entirely) with the
        // steepened distance gain; past the muffle handoff, or whenever the
        // listener is sheltered toward the strike, the low-passed FAR variant
        // plays instead, so distance and cover genuinely dull the sound
        // rather than just quieting it.
        if !flags.impact_played && seconds_to_impact <= IMPACT_BOOM_LEAD_S {
            flags.impact_played = true;
            if seconds_to_impact >= -1.0 {
                let size_db = meteor_size_gain_db(event.size);
                let delay = meteor_boom_push_delay(seconds_to_impact);
                if impact_distance <= BOOM_SPATIAL_RANGE_M {
                    scheduled.push(
                        delay,
                        PlaySound::at(SoundId::MeteorShowerImpact, impact)
                            .with_gain_offset_db(size_db),
                    );
                } else {
                    let sheltered = listener_sheltered_toward(&runtime, impact, &occluders);
                    let muffled = sheltered || impact_distance >= BOOM_MUFFLED_RANGE_M;
                    let id = if muffled {
                        SoundId::MeteorShowerImpactFar
                    } else {
                        SoundId::MeteorShowerImpact
                    };
                    let shelter_db = if sheltered {
                        BOOM_SHELTER_EXTRA_DB
                    } else {
                        0.0
                    };
                    scheduled.push(
                        delay,
                        PlaySound::non_spatial(id).with_gain_offset_db(
                            size_db + meteor_boom_distance_gain_db(impact_distance) + shelter_db,
                        ),
                    );
                }
            }
        }

        // Strike camera kick: the frame the clock crosses impact, distance-scaled.
        // Dividing the distance by size makes a small meteor kick like a farther
        // strike, reusing the tuned falloff instead of a second knob.
        if !flags.impact_shake_fired && event.has_impacted(now) {
            flags.impact_shake_fired = true;
            if seconds_to_impact >= -1.0 {
                kick.trigger_meteor_impact(impact_distance / event.size.clamp(0.05, 1.0));
            }
        }
    }
}

/// Proximity intensity for the meteor's crossing rumble + shake, given the
/// listener's horizontal distance to the fireball's true world position. Full
/// (1.0) inside [`METEOR_RUMBLE_FULL_M`], linear taper down to
/// [`METEOR_RUMBLE_FAR_FLOOR`] at [`METEOR_RUMBLE_RANGE_M`], and HELD at the
/// floor beyond: a crossing meteor is audible from anywhere in the world, just
/// distance-quiet, never cut to silence mid-flight. Pure so the curve is
/// unit-testable.
fn meteor_crossing_intensity(distance: f32) -> f32 {
    if !distance.is_finite() || distance <= METEOR_RUMBLE_FULL_M {
        return 1.0;
    }
    if distance >= METEOR_RUMBLE_RANGE_M {
        return METEOR_RUMBLE_FAR_FLOOR;
    }
    let span = (METEOR_RUMBLE_RANGE_M - METEOR_RUMBLE_FULL_M).max(f32::EPSILON);
    let t = (1.0 - (distance - METEOR_RUMBLE_FULL_M) / span).clamp(0.0, 1.0);
    METEOR_RUMBLE_FAR_FLOOR + (1.0 - METEOR_RUMBLE_FAR_FLOOR) * t
}

/// The in-flight meteor currently driving the crossing rumble + shake: the
/// LOUDEST one, where loudness is the proximity curve weighted by the meteor's
/// size (a big rock roars from farther out than a small one).
#[derive(Debug, Clone, Copy, PartialEq)]
struct MeteorRumbleDriver {
    /// The driving meteor's impact tick (its identity within the event).
    impact_tick: u64,
    /// Real seconds until the driving meteor strikes.
    seconds_to_impact: f32,
    /// Size-weighted proximity intensity in `[0, 1]` (already folds the
    /// meteor's size into the crossing curve).
    intensity: f32,
}

/// Find the loudest live, in-flight fireball for the listener, or `None` when
/// there is nothing to hear (no event, none in its flight window, all already
/// struck, backdrop up, or no local player yet).
fn meteor_rumble_driver(runtime: &ClientRuntime, menu: &MenuState) -> Option<MeteorRumbleDriver> {
    if menu.screen.uses_menu_backdrop() {
        return None;
    }
    let view = runtime.local_view()?;
    let listener = Vec3::new(view.position.x, view.position.y, view.position.z);
    let now_precise = runtime.server_tick_precise();
    let now = runtime.server_tick();

    let mut best: Option<MeteorRumbleDriver> = None;
    for event in &runtime.meteor_showers {
        let Some(state) = crate::world::meteor_world_state(
            bevy::math::Vec2::new(event.impact_position.x, event.impact_position.z),
            event.impact_tick,
            event.trajectory_seed,
            now_precise,
        ) else {
            continue;
        };
        let distance = listener.distance(state.position);
        let intensity = meteor_crossing_intensity(distance) * event.size.clamp(0.0, 1.0);
        let candidate = MeteorRumbleDriver {
            impact_tick: event.impact_tick,
            seconds_to_impact: event.seconds_to_impact(now),
            intensity,
        };
        // Loudest wins; earliest impact breaks ties so the bed keys stably.
        let better = match best {
            None => true,
            Some(current) => {
                candidate.intensity > current.intensity
                    || (candidate.intensity == current.intensity
                        && candidate.impact_tick < current.impact_tick)
            }
        };
        if better {
            best = Some(candidate);
        }
    }
    best
}

/// The live crossing-bed entity + which meteor drives it (`event_tick` is the
/// driving meteor's impact tick), so the bed is spawned exactly once per
/// driving meteor and torn down cleanly. `started` guards a respawn: because
/// the bed is a one-shot (not a loop) it may self-despawn when the file ends,
/// and we must NOT start it again for the same driver. `last_volume` and
/// `fade` drive the click-free teardown (ramp to silence, then despawn).
#[derive(Default)]
pub(crate) struct MeteorRumbleLoop {
    entity: Option<Entity>,
    event_tick: u64,
    started: bool,
    /// The most recent live volume written to the sink, the fade's start level.
    last_volume: f32,
    /// Teardown ramp in `[0, 1]`; `1.0` while live, counted down to zero (then
    /// the entity despawns) whenever the bed must stop while still audible.
    fade: f32,
}

/// Play, gain-scale, and tear down the meteor's non-spatial crossing bed
/// (`world/meteor-flyby.wav`). The bed is a ONE-SHOT, not a loop: the file has
/// its own approach-then-pass shape, so a loop would restart it mid-descent and
/// sound wrong. It is keyed to the LOUDEST (nearest, size-weighted) in-flight
/// meteor of the shower: started at a FIXED lead of [`FLYBY_LEAD_S`] before
/// that meteor's impact so the file's baked decay-to-silence tail lands on the
/// strike and the bed simply dies out on its own; its volume is driven each
/// frame off the size-weighted proximity curve (inaudible kilometres out,
/// swelling as it nears). When the driving meteor lands and a later sibling is
/// still crossing, the bed re-keys to the sibling and plays its crossing too.
/// If it ever has to stop while still audible (menu opened, event replaced) it
/// fades over ~0.25 s instead of being cut mid-waveform, which used to produce
/// a loud click at impact. Zero wire cost: the distance is computed
/// client-side from the announce payload. Runs in `ClientSystemSet::Sky`.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn meteor_shower_rumble_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    settings: Res<ClientSettings>,
    time: Res<Time>,
    library: Option<Res<SoundLibrary>>,
    mut loop_state: Local<MeteorRumbleLoop>,
    mut sinks: Query<&mut AudioSink>,
) {
    let Some(library) = library else {
        return;
    };

    // Is there an in-flight fireball to hear, and how loud? The loudest
    // (nearest, size-weighted) meteor drives the bed.
    match meteor_rumble_driver(&runtime, &menu) {
        Some(driver) => {
            // Reset the bed when the driving meteor changes (a new event, or
            // the previous driver landed and a sibling takes over).
            if loop_state.event_tick != driver.impact_tick {
                if let Some(entity) = loop_state.entity.take() {
                    // The bed is a one-shot that may have already self-despawned
                    // at the file's end; despawning it again must stay silent.
                    commands.entity(entity).try_despawn();
                }
                loop_state.event_tick = driver.impact_tick;
                loop_state.started = false;
            }
            let intensity = driver.intensity;
            // Start the one-shot at the fixed lead so the file's silent tail ends
            // at the strike (no teardown cut, no click). Never restart it for the
            // same driver: `started` stays true even after the file ends.
            if !loop_state.started && driver.seconds_to_impact <= FLYBY_LEAD_S {
                loop_state.entity = spawn_managed_sound(
                    &mut commands,
                    &library,
                    &settings,
                    SoundId::MeteorShowerRumble,
                    None,
                    0.0,
                    intensity.clamp(0.0, 1.0),
                    false, // one-shot: do NOT loop the flyby bed
                );
                loop_state.started = true;
                loop_state.fade = 1.0;
            } else if let Some(entity) = loop_state.entity {
                // Gain-scale the live bed by size-weighted proximity.
                // `sinks.get_mut` fails silently once the one-shot
                // self-despawns at the file's end, which is fine (nothing left
                // to scale).
                if let Ok(mut sink) = sinks.get_mut(entity) {
                    // Manifest base gain lands the reference level; scale it by
                    // proximity so far is a whisper, overhead a roar.
                    let base = sink_base_volume(&settings, &library);
                    let volume = base * intensity * METEOR_RUMBLE_PEAK_VOLUME;
                    loop_state.last_volume = volume;
                    sink.set_volume(Volume::Linear(volume));
                }
            }
        }
        None => {
            // No live fireball. If a bed is still playing (menu opened mid-flight,
            // late-join edge), ramp it to silence and only then despawn: an
            // instant despawn cuts the waveform mid-sample and clicks.
            if let Some(entity) = loop_state.entity {
                loop_state.fade =
                    (loop_state.fade - time.delta_secs() * FLYBY_FADE_OUT_PER_S).max(0.0);
                if loop_state.fade <= 0.0 {
                    // Same one-shot self-despawn race as the replaced-event arm.
                    commands.entity(entity).try_despawn();
                    loop_state.entity = None;
                } else if let Ok(mut sink) = sinks.get_mut(entity) {
                    sink.set_volume(Volume::Linear(loop_state.last_volume * loop_state.fade));
                } else {
                    // The one-shot already self-despawned at the file's end.
                    loop_state.entity = None;
                }
            }
            if loop_state.entity.is_none() {
                loop_state.event_tick = 0;
                loop_state.started = false;
            }
        }
    }
}

/// The reference linear volume for the rumble loop at full proximity, from the
/// manifest base gain and the user's SFX slider. Read each frame so the slider
/// stays live.
fn sink_base_volume(settings: &ClientSettings, library: &SoundLibrary) -> f32 {
    use crate::app::audio::category_volume;
    let defaults = library.defaults_for(SoundId::MeteorShowerRumble);
    category_volume(defaults.category, settings, defaults.base_gain_db, 0.0)
        .to_linear()
        .max(0.0)
}

/// Drive the slight continuous camera shake while a meteor crosses the sky,
/// ramping with the same size-weighted proximity curve as the rumble (the
/// loudest of the shower's in-flight meteors) and hard-capped small (a
/// fraction of the explosion kick, per the owner's "not too much"). Cuts off
/// at each impact: the impact's own shake fires separately from the explosion
/// path, so this only covers the crossings. Runs in `ClientSystemSet::Sky`,
/// before the camera consumes the kick.
pub(crate) fn meteor_shower_camera_shake_system(
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut kick: ResMut<CameraImpactKick>,
) {
    let Some(driver) = meteor_rumble_driver(&runtime, &menu) else {
        return;
    };
    if driver.intensity > 0.0 {
        kick.trigger_meteor_rumble(driver.intensity);
    }
}

/// A deterministic per-site seed from the impact position, so the scattered
/// fires, scorch blotches, and debris blast render identically on every client
/// (and reproducibly between sessions) without any wire cost.
fn site_seed(position: Vec3) -> u32 {
    position.x.to_bits() ^ position.z.to_bits().rotate_left(16) ^ 0x00E3_1B01
}

/// The crater's vertex colour at radial distance `r`: near-black char in the
/// bowl, easing to a scorched earthy brown at the rim (fully opaque), then
/// holding that tone while the ALPHA ramps to zero across the skirt, so the
/// burn reads as a painted decal over the grass that gets stronger toward the
/// impact.
fn crater_color(r: f32) -> [f32; 4] {
    // Both tints deliberately DARK (linear-space albedo): full daylight,
    // fog, and AgX all lighten a broad ground area, and a paler earth read
    // as a dust mound rather than burnt ground.
    const CHAR: [f32; 3] = [0.030, 0.024, 0.019];
    const EARTH: [f32; 3] = [0.055, 0.040, 0.030];
    let mix = |t: f32| -> [f32; 3] {
        [
            CHAR[0] + (EARTH[0] - CHAR[0]) * t,
            CHAR[1] + (EARTH[1] - CHAR[1]) * t,
            CHAR[2] + (EARTH[2] - CHAR[2]) * t,
        ]
    };
    if r <= CRATER_RIM_END_M {
        // Char-dominant through the bowl, easing to scorched earth only out
        // at the rim; solid painted burn.
        let t = (r / CRATER_RIM_END_M).clamp(0.0, 1.0);
        let rgb = mix(t * t * t);
        [rgb[0], rgb[1], rgb[2], 1.0]
    } else {
        // Skirt: earthy scorch fading out over the grass.
        let t =
            ((r - CRATER_RIM_END_M) / (CRATER_SKIRT_RADIUS_M - CRATER_RIM_END_M)).clamp(0.0, 1.0);
        let rgb = mix(1.0);
        [rgb[0], rgb[1], rgb[2], (1.0 - t).powf(1.4)]
    }
}

/// The crater's ring radii: denser through the bowl/rim (where the profile
/// curves), then two wide skirt rings out to the fade edge. Ring
/// [`CRATER_BOWL_LAST_RING`] sits exactly at `CRATER_RIM_END_M`, the seam
/// between the opaque bowl mesh and the translucent skirt mesh.
fn crater_ring_radii() -> [f32; CRATER_RINGS] {
    let mut ring_radii = [0.0f32; CRATER_RINGS];
    let ring_fractions = [0.0, 0.22, 0.45, 0.68, 1.0];
    for (i, f) in ring_fractions.iter().enumerate() {
        ring_radii[i] = CRATER_BOWL_RADIUS_M * f;
    }
    ring_radii[5] = CRATER_BOWL_RADIUS_M + (CRATER_RIM_END_M - CRATER_BOWL_RADIUS_M) * 0.45;
    ring_radii[6] = CRATER_RIM_END_M;
    ring_radii[7] = CRATER_RIM_END_M + (CRATER_SKIRT_RADIUS_M - CRATER_RIM_END_M) * 0.35;
    ring_radii[8] = CRATER_RIM_END_M + (CRATER_SKIRT_RADIUS_M - CRATER_RIM_END_M) * 0.70;
    ring_radii[9] = CRATER_SKIRT_RADIUS_M;
    ring_radii
}

/// One jittered crater surface vertex: `(position, true_radius)`. Seeded by the
/// GLOBAL ring index + segment so the seam ring comes out byte-identical in
/// both crater meshes. Radial jitter breaks the circular silhouette; height
/// jitter (only where the profile is raised) roughs up the lip. The outermost
/// ring stays exactly at the skirt radius and grade so the fade edge is clean.
fn crater_vertex(seed: u32, ring: usize, segment: usize, radius: f32) -> ([f32; 3], f32) {
    let s = seed
        .wrapping_add((ring as u32).wrapping_mul(0x9E37_79B1))
        .wrapping_mul(2_654_435_761)
        .wrapping_add((segment as u32).wrapping_mul(0x85EB_CA6B));
    let j1 = hashed_unit(s);
    let j2 = hashed_unit(s ^ 0x00C0_FFEE);
    let outer = ring == CRATER_RINGS - 1;
    let r_jitter = if outer {
        0.0
    } else {
        (j1 - 0.5) * 0.14 * radius
    };
    let r = (radius + r_jitter).max(0.05);
    // The mesh is built in unit-size local space; the rig's uniform size
    // transform scales it into world space (the profile is a homothety of
    // size, so the scaled mesh matches the analytic floor exactly).
    let mut h = crater_surface_height(r, 1.0);
    if !outer {
        h += (j2 - 0.5) * (h * 0.5).min(0.12);
    }
    let theta = (segment as f32 / CRATER_SEGMENTS as f32) * std::f32::consts::TAU;
    ([r * theta.cos(), h, r * theta.sin()], r)
}

/// Build one of the crater's two meshes over the GLOBAL ring range
/// `first_ring..=last_ring`: the SOLID bowl+rim body (`0..=CRATER_BOWL_LAST_RING`,
/// rendered opaque so the crater is unmistakably solid ground) or the
/// translucent burn skirt (`CRATER_BOWL_LAST_RING..=CRATER_RINGS-1`, vertex
/// alpha fading the decal into the grass). [`crater_vertex`]'s global-ring
/// seeding makes the shared seam ring identical in both, so they join without
/// a gap. Indexed, so the smooth normals shade the mound like ground rather
/// than a faceted prop.
fn build_crater_mesh(seed: u32, first_ring: usize, last_ring: usize) -> Mesh {
    let ring_radii = crater_ring_radii();
    let has_center = first_ring == 0;
    let ring_lo = if has_center { 1 } else { first_ring };

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();

    if has_center {
        positions.push([0.0, crater_surface_height(0.0, 1.0), 0.0]);
        colors.push(crater_color(0.0));
        uvs.push([0.5, 0.5]);
    }
    for (ring, &radius) in ring_radii
        .iter()
        .enumerate()
        .take(last_ring + 1)
        .skip(ring_lo)
    {
        for segment in 0..CRATER_SEGMENTS {
            let (position, r) = crater_vertex(seed, ring, segment, radius);
            uvs.push([
                0.5 + position[0] / (2.0 * CRATER_SKIRT_RADIUS_M),
                0.5 + position[2] / (2.0 * CRATER_SKIRT_RADIUS_M),
            ]);
            positions.push(position);
            colors.push(crater_color(r));
        }
    }

    // Indices: an optional fan from the centre to the first ring, then quads
    // between rings. With `x = r cos, z = r sin`, increasing-theta order winds
    // clockwise seen from +Y, so triangles list the NEXT segment before the
    // current one to face up.
    let center_offset = usize::from(has_center);
    let ring_start = |ring: usize| center_offset + (ring - ring_lo) * CRATER_SEGMENTS;
    let mut indices: Vec<u32> = Vec::new();
    if has_center {
        for segment in 0..CRATER_SEGMENTS {
            let next = (segment + 1) % CRATER_SEGMENTS;
            indices.extend([
                0,
                (ring_start(1) + next) as u32,
                (ring_start(1) + segment) as u32,
            ]);
        }
    }
    for ring in ring_lo..last_ring {
        let inner = ring_start(ring);
        let outer = ring_start(ring + 1);
        for segment in 0..CRATER_SEGMENTS {
            let next = (segment + 1) % CRATER_SEGMENTS;
            let (a, b) = ((inner + segment) as u32, (inner + next) as u32);
            let (c, d) = ((outer + segment) as u32, (outer + next) as u32);
            indices.extend([a, d, c, a, b, d]);
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
    .with_computed_smooth_normals()
}

/// Build one meteor's impact-site rig at `position`: the dug-in crater (raised
/// rim bowl and fading burn skirt, one vertex-coloured mesh) plus the
/// scattered particle-fire emitters (when the burn window is still open at
/// `age_seconds`). The whole rig is uniformly scaled by the meteor's `size`;
/// because the shared crater profile is a pure homothety of size, the scaled
/// mesh still matches the movement grid's analytic floor and the server's
/// node seating exactly. No persistent rubble and no static flame geometry:
/// everything fiery is live particles that burn out. Also throws the one-time
/// rock-and-stone blast (fixed-size physics debris + a bright flash, both
/// intensity-scaled by size), but only when the impact just happened, never
/// replayed for a late joiner. The rig despawns with the crater window (the
/// debris via its own `ImpactChip` lifetime, the fires via
/// [`animate_meteor_shower_site_fire_system`]).
fn spawn_crater(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    position: Vec3,
    age_seconds: f32,
    impact_tick: u64,
    size: f32,
) {
    let size = if size.is_finite() && size > 0.0 {
        size
    } else {
        1.0
    };
    let seed = site_seed(position);

    // The crater in two meshes sharing one seam ring: the SOLID bowl+rim body
    // (opaque, so the crater reads as real dug ground with nothing showing
    // through it) and the translucent burn skirt whose vertex alpha melts the
    // painted decal into the grass. Both lit + rough so they shade with the
    // day/night sun like the terrain; white base colour because the vertex
    // COLOR_0 gradient carries the char-to-earth ramp.
    let bowl_mesh = meshes.add(build_crater_mesh(seed, 0, CRATER_BOWL_LAST_RING));
    let bowl_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        ..default()
    });
    let skirt_mesh = meshes.add(build_crater_mesh(
        seed,
        CRATER_BOWL_LAST_RING,
        CRATER_RINGS - 1,
    ));
    let skirt_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands
        .spawn((
            Name::new("MeteorShower Impact Site"),
            MeteorShowerCrater { impact_tick },
            // Uniform size scale: the unit-space crater mesh and fire anchors
            // scale into the meteor's own footprint (homothety contract).
            Transform::from_translation(position).with_scale(Vec3::splat(size)),
            Visibility::Visible,
        ))
        .with_children(|parent| {
            // The crater: solid raised-rim bowl plus the painted burn skirt
            // fading out over the grass, both at the site origin.
            parent.spawn((
                Name::new("MeteorShower Crater Bowl"),
                Mesh3d(bowl_mesh),
                MeshMaterial3d(bowl_material),
                Transform::IDENTITY,
                NotShadowCaster,
            ));
            parent.spawn((
                Name::new("MeteorShower Crater Skirt"),
                Mesh3d(skirt_mesh),
                MeshMaterial3d(skirt_material),
                Transform::IDENTITY,
                NotShadowCaster,
            ));

            // Fire emitters: many separate burning points strewn across and
            // around the radius, each a furnace-style particle fire (flame puffs
            // + rising embers, shed by `animate_meteor_shower_site_fire_system`) with
            // its own shadowless PointLight so the patches genuinely glow and
            // light the night ground. Only spawned while the burn window is
            // open; a late joiner past it gets scorch only. A small meteor
            // burns with proportionally fewer, smaller, dimmer fires.
            let fire_count = ((FIRE_CLUSTER_COUNT as f32 * size).round() as u32).max(3);
            if age_seconds < METEOR_SHOWER_SITE_FIRE_SECONDS {
                for c in 0..fire_count {
                    let s = seed
                        .wrapping_add(0x5152_5354)
                        .wrapping_mul(2_246_822_519)
                        .wrapping_add(c.wrapping_mul(3_266_489_917));
                    let r1 = hashed_unit(s);
                    let r2 = hashed_unit(s ^ 0x1357_9BDF);
                    let r3 = hashed_unit(s ^ 0x2468_ACE0);
                    // Scatter the fires over the dark crater ground so the char
                    // under them keeps the additive puffs from washing to pale
                    // pink over bright sunlit grass: a few burn down inside the
                    // bowl, the rest on the burn skirt outside the rim (never ON
                    // the lip, where they would sit half-buried). Anchored to
                    // the crater surface height at their radius. All in the
                    // rig's unit-size local space; the size transform scales
                    // the anchor positions into place.
                    let dist = if c % 3 == 0 {
                        0.8 + r3 * 3.0
                    } else {
                        CRATER_RIM_END_M
                            + (CRATER_SKIRT_RADIUS_M - CRATER_RIM_END_M - 1.0) * r3.sqrt()
                    };
                    let theta = r1 * std::f32::consts::TAU;
                    let ground = crater_surface_height(dist, 1.0);
                    let cluster = Vec3::new(dist * theta.cos(), ground, dist * theta.sin());

                    parent.spawn((
                        Name::new("MeteorShower Site Fire"),
                        MeteorShowerSiteFire {
                            impact_tick,
                            // Hold off one interval so the rig's GlobalTransform
                            // propagates before the first particle, otherwise the
                            // first puff would emit from the world origin.
                            flame_cooldown: SITE_FLAME_INTERVAL,
                            spark_cooldown: SITE_SPARK_INTERVAL,
                            phase: hashed_unit(s ^ 0x00F1_4E55) * std::f32::consts::TAU,
                            // Per-fire size spread so the site mixes small licks
                            // and real blazes rather than a field of clones,
                            // eased down with the meteor's size (particles are
                            // world-space, so the rig scale does not touch them).
                            scale: (0.75 + r2 * 0.65) * (0.5 + 0.5 * size),
                            // The local 1.0 lift below lands at `size` world
                            // metres above ground once the rig scale applies;
                            // flames are born just above the ground under it.
                            anchor_drop: size - 0.05,
                        },
                        PointLight {
                            color: Color::srgb(1.0, 0.45, 0.12),
                            intensity: FIRE_LIGHT_INTENSITY * (0.7 + r2 * 0.6) * size,
                            range: FIRE_LIGHT_RANGE_M * (0.5 + 0.5 * size),
                            radius: 0.3,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_translation(cluster + Vec3::new(0.0, 1.0, 0.0)),
                        Visibility::Visible,
                    ));
                }
            }
        });

    // One-time rock-and-stone blast: fling grey/brown debris chunks outward and
    // upward, plus a bright fire flash, at the impact moment. These are
    // free-standing `ImpactChip` physics entities (not parented to the site) that
    // arc, fall, and self-despawn via `tick_impact_chips_system`. Only when the
    // impact JUST happened: a late joiner gets the burning/burnt site, not a
    // replayed explosion.
    if age_seconds < IMPACT_BLAST_WINDOW_S {
        spawn_impact_rock_blast(commands, meshes, materials, position, seed, size);
    }
}

/// Throw the meteor's rock/stone debris burst + a bright flash, once, at the
/// impact moment. Reuses the [`ImpactChip`] integrator (like the explosive
/// debris burst) but larger and rock-tinted, with more chunks: a meteor strike,
/// not a satchel charge. `size` scales the burst's intensity: fewer, smaller,
/// slower chunks and a smaller flash for a small meteor.
fn spawn_impact_rock_blast(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    position: Vec3,
    seed: u32,
    size: f32,
) {
    // A small pool of lumpy faceted rock variants (the meteor core's
    // `irregular_rock_mesh` off different seeds), so the flung chunks read as
    // real curved, irregular stone with shape variety, not one repeated
    // cuboid. Two earthy tints (grey stone, brown dirt-clod).
    let rock_meshes: Vec<Handle<Mesh>> = (0..5u32)
        .map(|i| {
            meshes.add(super::meteor_sky::irregular_rock_mesh(
                0.45,
                seed.wrapping_add(i.wrapping_mul(0x9E37_79B1)),
            ))
        })
        .collect();
    let rock_grey = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.29, 0.27),
        perceptual_roughness: 1.0,
        ..default()
    });
    let rock_brown = materials.add(StandardMaterial {
        base_color: Color::srgb(0.26, 0.18, 0.11),
        perceptual_roughness: 1.0,
        ..default()
    });

    // Fewer chunks for a small strike (still a real fountain at the size floor).
    let debris_count = ((IMPACT_DEBRIS_COUNT as f32 * size).round() as u32).max(20);
    for i in 0..debris_count {
        let s = seed
            .wrapping_mul(2_654_435_761)
            .wrapping_add(i.wrapping_mul(374_761_393));
        let r1 = hashed_unit(s);
        let r2 = hashed_unit(s ^ 0xDEAD_BEEF);
        let r3 = hashed_unit(s ^ 0x00C0_FFEE);
        let r4 = hashed_unit(s ^ 0x1234_5678);
        // Spread across the full ring. A strong UPWARD-and-out component so the
        // boulders genuinely LAUNCH into the air and arc, reading as ejecta in
        // flight at the impact moment (the round 2 debris fell too fast to be
        // seen mid-air). Up-bias comparable to the outward push.
        let angle = (i as f32 / debris_count as f32) * std::f32::consts::TAU + r1 * 0.9;
        let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
        let up = 1.4 + r2 * 2.2;
        // A hard throw so chunks fountain up and out before arcing back, but
        // capped below the old speeds: rubble that rocketed 20 m/s toward the
        // lens filled the frame and read as giant boulders. The per-variant
        // mass multiplier below slows the big chunks further.
        let speed = (8.5 + r3 * 8.5) * (0.7 + 0.3 * size);
        let velocity = (radial * (1.1 + r1 * 1.2) + Vec3::Y * up).normalize_or_zero() * speed;
        let spin_axis = Vec3::new(r1 * 2.0 - 1.0, r2 * 2.0 - 1.0, r3 * 2.0 - 1.0)
            .normalize_or_zero()
            .max(Vec3::new(0.001, 1.0, 0.001));
        // Matte grey/brown stone only: the old bright unlit "molten ember"
        // chunks strobed as they tumbled (a spinning emissive cube reads as a
        // flickering shape). The flash + flash light carry the hot moment.
        let material = if i % 3 == 0 {
            rock_brown.clone()
        } else {
            rock_grey.clone()
        };
        // Fixed-size chunks under heavy gravity (2.2 x the shared base): they
        // launch, arc, land, bounce, and tumble out under ground friction,
        // their spin bleeding off with it. Lifetimes are long enough to watch
        // the whole fall and the chunk resting on the ground for a beat before
        // it pops out. Sizes are capped modest (the largest ~0.3 m across at
        // size 1.0, a stone you could lift one-handed), so the fountain reads
        // as smashed-up rubble, not flying boulders. A small meteor smashes
        // smaller rubble.
        // Four fixed rubble sizes, capped well below "boulder", each with its
        // mass feel: the biggest chunks are hurled slowest and tumble lazily,
        // the small ones fly and spin quickest, so every chunk's motion reads
        // proportional to its size (owner rule).
        let variant = crate::app::systems::debris_variant(0.28 * (0.6 + 0.4 * size), r3);
        let rock_mesh = rock_meshes[(s >> 7) as usize % rock_meshes.len()].clone();
        commands.spawn((
            Name::new("MeteorShower Debris"),
            ImpactChip::new(
                velocity * variant.speed,
                spin_axis,
                (4.0 + r1 * 6.0) * variant.spin,
                5.0 + r2 * 2.5,
                variant.scale,
                2.2,
            )
            .with_fixed_scale(),
            Mesh3d(rock_mesh),
            MeshMaterial3d(material),
            Transform::from_translation(position + Vec3::Y * (0.5 + r4 * 0.8))
                .with_scale(Vec3::splat(variant.scale)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }

    // (The old dense ember-spark spray is gone: dozens of fast-spinning bright
    // cubes strobed against the sky. The blast is carried by the solid rock
    // fountain plus the flash + flash light below.)

    // A brief fireball flash at ground zero: a low, wide additive dome at a FIXED
    // size that fades its own material instance to black through the shared
    // meteor-flash ticker, then despawns. The old opaque dome shrank out over its
    // life, which read as a huge scale-animating particle (owner report); a pure
    // color fade keeps the punch with no size animation. Its emissive stays
    // red-biased so it reads as a fireball, not a bleached white bulb (the round
    // 2 failure). The flung debris is the star. Kept a touch smaller than the old
    // dome since it now opens at full size.
    let flash_radius = 4.0 * size;
    let flash_mesh = meshes.add(Sphere::new(1.0).mesh().ico(2).unwrap());
    let flash_color = Vec3::new(1.6, 0.22, 0.0);
    let flash_material = materials.add(StandardMaterial {
        // DEEP color to keep its hue under AgX (a bright value here is exactly
        // what bleached round 2 to a white central mass). unlit -> base_color
        // emits; additive so the fade-to-black reads as the fireball clearing.
        base_color: Color::linear_rgb(flash_color.x, flash_color.y, flash_color.z),
        unlit: true,
        fog_enabled: false,
        alpha_mode: AlphaMode::Add,
        ..default()
    });
    commands.spawn((
        Name::new("MeteorShower Impact Flash"),
        // Squashed low so it is a ground fireball, not a dome. Lives ~0.7s so the
        // fireball is still up while the ejecta fountains through it, then clears.
        super::meteor_sky::MeteorAirburstFlash::new(0.7, flash_color),
        Mesh3d(flash_mesh),
        MeshMaterial3d(flash_material),
        Transform::from_translation(position + Vec3::Y * flash_radius * 0.35)
            .with_scale(Vec3::new(flash_radius, flash_radius * 0.6, flash_radius)),
        Visibility::Visible,
        NotShadowCaster,
    ));

    // A bright momentary flash light so the impact instant floods the night scene
    // with fire-light (the blast lights the world for a beat). It rides the same
    // short-lived flash chip lifetime via a separate zero-motion chip carrying a
    // PointLight.
    commands.spawn((
        Name::new("MeteorShower Impact Flash Light"),
        ImpactChip::new(Vec3::ZERO, Vec3::Y, 0.0, 0.4, 1.0, 0.0),
        PointLight {
            color: Color::srgb(1.0, 0.55, 0.2),
            intensity: FIRE_LIGHT_INTENSITY * 12.0 * size,
            range: METEOR_SHOWER_IMPACT_RADIUS_M * 4.0 * size,
            radius: 2.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_translation(position + Vec3::Y * 4.0 * size.max(0.4)),
        Visibility::Visible,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::CRATER_RIM_HEIGHT_M;

    #[test]
    fn crossing_intensity_is_full_close_and_floored_far() {
        assert_eq!(meteor_crossing_intensity(0.0), 1.0);
        assert_eq!(meteor_crossing_intensity(METEOR_RUMBLE_FULL_M), 1.0);
        assert_eq!(meteor_crossing_intensity(METEOR_RUMBLE_FULL_M * 0.5), 1.0);
        // A crossing meteor never goes silent, no matter the distance: the
        // taper bottoms out at the world-audible floor and holds it.
        assert_eq!(
            meteor_crossing_intensity(METEOR_RUMBLE_RANGE_M),
            METEOR_RUMBLE_FAR_FLOOR
        );
        assert_eq!(
            meteor_crossing_intensity(METEOR_RUMBLE_RANGE_M + 1_000.0),
            METEOR_RUMBLE_FAR_FLOOR
        );
        assert_eq!(meteor_crossing_intensity(6_000.0), METEOR_RUMBLE_FAR_FLOOR);
    }

    #[test]
    fn boom_distance_gain_is_flat_near_and_floored_far() {
        // Inside the spatial handoff range the tuned spatial path plays at
        // full gain (the cap the far model tapers from).
        assert_eq!(meteor_boom_distance_gain_db(0.0), 0.0);
        assert_eq!(meteor_boom_distance_gain_db(BOOM_SPATIAL_RANGE_M), 0.0);
        // Steepened law past the handoff: 10x the range is one decade.
        let ten_x = meteor_boom_distance_gain_db(BOOM_SPATIAL_RANGE_M * 10.0);
        assert!(
            (ten_x - BOOM_FAR_DB_PER_DECADE).abs() < 1e-3,
            "10x range is one decade of attenuation, got {ten_x}"
        );
        // Monotonic and floored: the far side of any world still thumps.
        assert!(meteor_boom_distance_gain_db(800.0) > meteor_boom_distance_gain_db(1_600.0));
        assert_eq!(meteor_boom_distance_gain_db(1.0e9), BOOM_FAR_FLOOR_DB);
        assert_eq!(meteor_boom_distance_gain_db(f32::NAN), 0.0);
    }

    #[test]
    fn boom_peak_lands_exactly_at_the_impact_frame() {
        // The scheduled delay puts the file's detonation peak
        // (IMPACT_BOOM_LEAD_S into playback) exactly at the visual impact,
        // with NO distance-dependent travel lag (a lagged boom split the
        // strike into "trees fell" and then a late "impact", owner report):
        // distance shapes only gain and the muffled variant. The normal
        // trigger fires the frame the countdown crosses the lead, so
        // seconds_to_impact sits a frame under IMPACT_BOOM_LEAD_S and the
        // peak error is that sub-frame sliver, never a distance term.
        for frame in [0.0_f32, 1.0 / 144.0, 1.0 / 30.0] {
            let seconds_to_impact = IMPACT_BOOM_LEAD_S - frame;
            let delay = meteor_boom_push_delay(seconds_to_impact);
            let peak_after_impact = delay + IMPACT_BOOM_LEAD_S - seconds_to_impact;
            assert!(
                (peak_after_impact - frame).abs() < 1e-4,
                "peak error must be only the trigger frame's sliver, got {peak_after_impact}"
            );
        }
        // A LATE witness (first saw the strike inside the lead window)
        // clamps at zero rather than scheduling into the past: the peak
        // lands late by exactly the missed lead, never early.
        let late = meteor_boom_push_delay(1.0);
        assert_eq!(late, 0.0);
        assert_eq!(late + IMPACT_BOOM_LEAD_S - 1.0, IMPACT_BOOM_LEAD_S - 1.0);
    }

    #[test]
    fn shelter_segment_test_hits_blocks_on_the_bearing_only() {
        use crate::protocol::Vec3Net;
        use crate::world::WorldBlock;
        let eye = Vec3::new(0.0, 1.6, 0.0);
        let wall = WorldBlock::new(Vec3Net::new(5.0, 1.5, 0.0), Vec3Net::new(0.15, 1.5, 1.5));
        // Straight through the wall: sheltered.
        assert!(segment_hits_block(
            eye,
            Vec3::X,
            SHELTER_SCAN_RANGE_M,
            &wall
        ));
        // Behind the listener, or off to the side: clear.
        assert!(!segment_hits_block(
            eye,
            -Vec3::X,
            SHELTER_SCAN_RANGE_M,
            &wall
        ));
        assert!(!segment_hits_block(
            eye,
            Vec3::Z,
            SHELTER_SCAN_RANGE_M,
            &wall
        ));
        // Beyond the scan range: clear (local cover only).
        let far_wall = WorldBlock::new(Vec3Net::new(100.0, 1.5, 0.0), Vec3Net::new(0.15, 1.5, 1.5));
        assert!(!segment_hits_block(
            eye,
            Vec3::X,
            SHELTER_SCAN_RANGE_M,
            &far_wall
        ));
        // A knee-high fence under the ear line does not shelter.
        let fence = WorldBlock::new(Vec3Net::new(5.0, 0.4, 0.0), Vec3Net::new(0.15, 0.4, 1.5));
        assert!(!segment_hits_block(
            eye,
            Vec3::X,
            SHELTER_SCAN_RANGE_M,
            &fence
        ));
    }

    #[test]
    fn crossing_intensity_ramps_monotonically_between_the_bounds() {
        let mid = (METEOR_RUMBLE_FULL_M + METEOR_RUMBLE_RANGE_M) * 0.5;
        let near = meteor_crossing_intensity(METEOR_RUMBLE_FULL_M + 100.0);
        let middle = meteor_crossing_intensity(mid);
        let far = meteor_crossing_intensity(METEOR_RUMBLE_RANGE_M - 100.0);
        assert!(
            near > middle && middle > far,
            "intensity should fall off with distance: {near} > {middle} > {far}"
        );
        assert!((0.0..=1.0).contains(&middle));
    }

    #[test]
    fn crossing_intensity_handles_non_finite() {
        // A degenerate distance clamps to full rather than NaN-propagating.
        assert_eq!(meteor_crossing_intensity(f32::NAN), 1.0);
    }

    #[test]
    fn size_gain_is_zero_for_the_headliner_and_softer_for_small_meteors() {
        assert!(meteor_size_gain_db(1.0).abs() < 1e-4);
        let small = meteor_size_gain_db(0.4);
        assert!(
            small < -6.0 && small > -14.0,
            "0.4 lands ~-8 dB, got {small}"
        );
        // Monotonic in size, floored for degenerate sizes.
        assert!(meteor_size_gain_db(0.8) > meteor_size_gain_db(0.4));
        assert!(meteor_size_gain_db(0.0) >= -14.0);
        assert!(meteor_size_gain_db(f32::NAN).is_finite());
    }

    #[test]
    fn site_fire_burns_full_then_fades_then_dies() {
        let fade_start = METEOR_SHOWER_SITE_FIRE_SECONDS - METEOR_SHOWER_SITE_FIRE_FADE_SECONDS;
        // Full blaze from impact until the fade window opens.
        assert_eq!(site_fire_intensity(0.0), 1.0);
        assert_eq!(site_fire_intensity(fade_start), 1.0);
        // Ramps down monotonically across the fade tail.
        let early = site_fire_intensity(fade_start + METEOR_SHOWER_SITE_FIRE_FADE_SECONDS * 0.25);
        let late = site_fire_intensity(fade_start + METEOR_SHOWER_SITE_FIRE_FADE_SECONDS * 0.75);
        assert!(early > late && (0.0..1.0).contains(&late));
        // Dead at the end of the burn window and forever after.
        assert_eq!(site_fire_intensity(METEOR_SHOWER_SITE_FIRE_SECONDS), 0.0);
        assert_eq!(
            site_fire_intensity(METEOR_SHOWER_SITE_FIRE_SECONDS + 500.0),
            0.0
        );
    }

    #[test]
    fn site_fire_intensity_handles_degenerate_ages() {
        // Pre-impact (negative age) and non-finite ages read as no fire rather
        // than NaN-propagating into light intensity.
        assert_eq!(site_fire_intensity(-5.0), 0.0);
        assert_eq!(site_fire_intensity(f32::NAN), 0.0);
    }

    #[test]
    fn crater_color_is_solid_char_at_core_and_fades_out_across_the_skirt() {
        let core = crater_color(0.0);
        let rim = crater_color(CRATER_RIM_END_M);
        let mid_skirt = crater_color((CRATER_RIM_END_M + CRATER_SKIRT_RADIUS_M) * 0.5);
        let edge = crater_color(CRATER_SKIRT_RADIUS_M);
        // Fully-solid burn through the bowl and rim, darkest at the core.
        assert_eq!(core[3], 1.0);
        assert_eq!(rim[3], 1.0);
        assert!(core[0] < rim[0], "core is darker char than the rim earth");
        // The decal weakens outward and reaches zero at the skirt edge.
        assert!(mid_skirt[3] < rim[3] && mid_skirt[3] > 0.0);
        assert!(edge[3] < 1e-4);
    }

    /// Vertex positions of `mesh` (panics if absent).
    fn mesh_positions(mesh: &Mesh) -> Vec<[f32; 3]> {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|values| values.as_float3())
            .expect("mesh has positions")
            .to_vec()
    }

    #[test]
    fn crater_meshes_are_well_formed_and_share_the_seam_ring() {
        let seed = 0xE31B_01AA;
        let bowl = build_crater_mesh(seed, 0, CRATER_BOWL_LAST_RING);
        let skirt = build_crater_mesh(seed, CRATER_BOWL_LAST_RING, CRATER_RINGS - 1);
        let bowl_positions = mesh_positions(&bowl);
        let skirt_positions = mesh_positions(&skirt);
        assert_eq!(
            bowl_positions.len(),
            1 + CRATER_BOWL_LAST_RING * CRATER_SEGMENTS
        );
        assert_eq!(
            skirt_positions.len(),
            (CRATER_RINGS - CRATER_BOWL_LAST_RING) * CRATER_SEGMENTS
        );
        // Every vertex sits between grade and the jittered rim crest.
        for p in bowl_positions.iter().chain(skirt_positions.iter()) {
            assert!(
                p[1] >= 0.0 && p[1] <= CRATER_RIM_HEIGHT_M + 0.15,
                "y {}",
                p[1]
            );
        }
        // The seam: the bowl's OUTER ring and the skirt's INNER ring are the
        // same jittered vertices, so the two meshes join without a gap.
        let bowl_seam = &bowl_positions[bowl_positions.len() - CRATER_SEGMENTS..];
        let skirt_seam = &skirt_positions[..CRATER_SEGMENTS];
        assert_eq!(bowl_seam, skirt_seam, "seam rings must match exactly");
        // Indices reference valid vertices in both meshes.
        for (mesh, positions) in [(&bowl, &bowl_positions), (&skirt, &skirt_positions)] {
            let indices: Vec<u32> = mesh
                .indices()
                .expect("indexed")
                .iter()
                .map(|i| i as u32)
                .collect();
            assert!(!indices.is_empty());
            assert!(indices.iter().all(|&i| (i as usize) < positions.len()));
        }
    }
}
