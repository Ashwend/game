//! World-space meteor shower impact visuals: a shallow dug-in crater with a fading
//! painted burn skirt, strewn with LIVE particle fires that burn for the first
//! minute or two after the strike, then die out and leave only the crater for
//! the rest of the window.
//!
//! Entirely client-side and derived from the event state (`runtime.meteor_shower`):
//! there is no replicated crater entity and no save bump. The site appears the
//! instant the local clock passes `impact_tick` and is removed when the event
//! clears (crater despawn window, matching the server). Because the announce is
//! resent to late joiners while the event is alive, a player who connects during
//! the crater phase gets the announce and this system draws the site for them
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

/// Seconds before impact the flyby crossing bed is started. The 18.1 s
/// `world/meteor-flyby.wav` decays to silence over its final ~4 s, so starting
/// it at this fixed lead lets the file die out naturally just before the
/// strike; the impact cue's own rising lead-in carries the final seconds. The
/// old start rule (whenever the proximity curve first went non-zero) left the
/// file mid-waveform at impact, and the hard despawn cut it with a loud click.
const FLYBY_LEAD_S: f32 = 17.5;

/// Per-second fade applied to the flyby bed when it must be torn down while
/// still audible (menu opened, event replaced, late-join edge). ~0.25 s to
/// silence, then the entity despawns: never a mid-waveform cut.
const FLYBY_FADE_OUT_PER_S: f32 = 4.0;

/// Proximity model for the meteor's crossing rumble + slight camera shake. Both
/// swell as the fireball's true world position nears the listener: silent/still
/// beyond `METEOR_RUMBLE_RANGE_M`, full inside `METEOR_RUMBLE_FULL_M`. Kept in
/// one place so the audible rumble and the felt shake ramp together off the same
/// distance.
const METEOR_RUMBLE_RANGE_M: f32 = 2_500.0;
const METEOR_RUMBLE_FULL_M: f32 = 120.0;

/// Peak linear volume scale for the crossing rumble loop at closest approach. The
/// loop's manifest base gain is already low; this scales it further so even an
/// overhead pass sits under the impact thump.
const METEOR_RUMBLE_PEAK_VOLUME: f32 = 1.0;

/// Marks the whole impact-site visual rig (crater mesh + fire emitters), so it
/// can be found and despawned as a unit when the event ends. The one-time rock
/// blast is thrown as free-standing `ImpactChip` debris that self-despawns, so
/// it is not parented here.
#[derive(Component)]
pub(crate) struct MeteorShowerCrater;

/// Marker + emitter state for one scattered fire at the impact site. A child of
/// the crater rig carrying the fire's `PointLight`; while the site's burn window
/// is open it sheds furnace-style flame puffs and embers each frame (see
/// [`animate_meteor_shower_site_fire_system`]), then despawns when the fire dies.
#[derive(Component)]
pub(crate) struct MeteorShowerSiteFire {
    /// Seconds until the next flame-puff emission.
    flame_cooldown: f32,
    /// Seconds until the next rising-ember emission.
    spark_cooldown: f32,
    /// Free-running phase offset so the fires flicker out of sync with each
    /// other instead of pulsing in lockstep.
    phase: f32,
    /// Per-fire size multiplier on particle scale/loft and light output, so the
    /// site mixes small licks and real blazes.
    scale: f32,
}

/// Per-event impact-cue bookkeeping so the pre-armed boom and the strike camera
/// kick each fire exactly once. Keyed on the event's `impact_tick` so a new
/// event resets the flags. The crossing rumble is a separate one-shot the
/// renderer owns (see [`meteor_shower_rumble_system`]); there is no longer a
/// separate approach roar (the flyby bed carries the whole approach).
#[derive(Default)]
pub(crate) struct MeteorShowerCueState {
    /// The `impact_tick` of the event these flags belong to (0 = no event yet).
    event_tick: u64,
    /// The boom cue has been scheduled (pre-armed [`IMPACT_BOOM_LEAD_S`] before
    /// the strike so the file's baked lead-in ends ON the strike).
    impact_played: bool,
    /// The one-off strike camera kick has fired (at the impact frame itself).
    impact_shake_fired: bool,
}

/// How many distinct fire emitter points strew the impact site. Each is a
/// furnace-style particle fire plus its own shadowless `PointLight`, so the site
/// reads as many separate burning patches (not one painted ring) that genuinely
/// glow and light the ground at night.
const FIRE_CLUSTER_COUNT: u32 = 12;

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

/// Spawn, position, and tear down the crater visual from the event state. Runs
/// in `ClientSystemSet::Sky`; a no-op on the title backdrop (no world) and
/// whenever no event has impacted yet.
pub(crate) fn update_meteor_shower_ground_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<(Entity, &MeteorShowerCrater)>,
    mut transforms: Query<&mut Transform, With<MeteorShowerCrater>>,
) {
    // Should the crater be shown? Only when an event exists, it has impacted, and
    // we are not on the title backdrop.
    let crater_event = (!menu.screen.uses_menu_backdrop())
        .then(|| runtime.meteor_shower)
        .flatten()
        .filter(|event| event.has_impacted(runtime.server_tick()));

    match (crater_event, existing.iter().next()) {
        (Some(event), None) => {
            // First frame the site appears on this client. Usually that IS the
            // impact moment, but a late joiner (or a client returning from the
            // backdrop) spawns it mid-window: the site age gates the one-time
            // blast and the fires so they never replay stale.
            let position = event.impact_position;
            let age_seconds = ((runtime.server_tick_precise() - event.impact_tick as f64)
                / f64::from(crate::protocol::SERVER_TICK_RATE_HZ))
                as f32;
            info!(
                "meteor_shower: crater rig spawned at ({:.1}, {:.1}), site age {age_seconds:.1}s",
                position.x, position.z
            );
            spawn_crater(
                &mut commands,
                &mut meshes,
                &mut materials,
                Vec3::new(position.x, position.y, position.z),
                age_seconds,
            );
        }
        (Some(event), Some(_)) => {
            // Live crater: keep it anchored (the impact point never moves, but a
            // reconnect or world reload could re-seed it, so reassert).
            if let Ok(mut transform) = transforms.single_mut() {
                let position = event.impact_position;
                transform.translation = Vec3::new(position.x, position.y, position.z);
            }
        }
        (None, Some((entity, _))) => {
            // Event ended (or backdrop opened): tear the rig down.
            info!("meteor_shower: crater rig despawned (event ended or backdrop)");
            commands.entity(entity).despawn();
        }
        (None, None) => {}
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
    // Burn-out envelope off the site age. If the event vanished from under the
    // fires (the rig teardown despawn is still queued this frame), treat them as
    // burnt out rather than freezing at full blaze.
    let intensity = runtime
        .meteor_shower
        .map(|event| {
            let age_seconds = ((runtime.server_tick_precise() - event.impact_tick as f64)
                / f64::from(crate::protocol::SERVER_TICK_RATE_HZ))
                as f32;
            site_fire_intensity(age_seconds)
        })
        .unwrap_or(0.0);
    let Some(assets) = assets else {
        return;
    };
    let dt = time.delta_secs().max(0.0);
    let t = time.elapsed_secs();

    for (entity, global, mut fire, mut light) in &mut fires {
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
        light.intensity = FIRE_LIGHT_INTENSITY * fire.scale * intensity * (0.7 + 0.55 * flicker);

        // The emitter entity sits lifted so its light pools on the ground around
        // it; the flames themselves are born at ground level under it.
        let anchor = global.translation() - Vec3::Y * 0.95;
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
/// disc and rising a metre or two before fading. Rides the furnace-particle
/// integrator (loft, drag, shrink to nothing, despawn).
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
    // night). Shrinks with the burn-out envelope so a dying fire is smaller.
    let initial_scale = (3.0 + r2 * 2.5) * fire_scale * (0.55 + 0.45 * intensity);
    let lifetime = 0.45 + r1 * 0.4;

    commands.spawn((
        Name::new("MeteorShower Fire Flame"),
        FurnaceParticle::new(velocity, 0.3, 0.8, lifetime, initial_scale),
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
    let initial_scale = (2.5 + r2 * 2.5) * fire_scale;
    let lifetime = 0.7 + r1 * 0.7;

    commands.spawn((
        Name::new("MeteorShower Fire Spark"),
        FurnaceParticle::new(velocity, 2.2, 1.2, lifetime, initial_scale),
        Mesh3d(assets.spark_mesh.clone()),
        MeshMaterial3d(assets.spark_material.clone()),
        Transform::from_translation(anchor + offset).with_scale(Vec3::splat(initial_scale)),
        Visibility::Visible,
        NotShadowCaster,
    ));
}

/// Fire the meteor shower strike cues from the event state, each exactly once per
/// event:
///
/// - **Boom** (spatial at the crater): started [`IMPACT_BOOM_LEAD_S`] BEFORE
///   the strike so the file's baked rising lead-in plays through the final
///   descent and its detonation peak lands on the visual impact frame.
/// - **Camera kick**: at the impact frame itself, distance-scaled via
///   `CameraImpactKick::trigger_meteor_impact` (felt from hundreds of metres,
///   the payoff the crossing tremor builds to).
///
/// A late joiner who connects after the strike gets neither (the crater is
/// stale news). The approach is carried wholly by the crossing bed (see
/// [`meteor_shower_rumble_system`]). Gated on `!uses_menu_backdrop`.
pub(crate) fn meteor_shower_audio_system(
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut cue: Local<MeteorShowerCueState>,
    mut scheduled: ResMut<ScheduledSounds>,
    mut kick: ResMut<CameraImpactKick>,
) {
    if menu.screen.uses_menu_backdrop() {
        return;
    }
    let Some(event) = runtime.meteor_shower else {
        return;
    };
    // Reset the per-event flags when a new (or resent-but-different) event lands.
    if cue.event_tick != event.impact_tick {
        cue.event_tick = event.impact_tick;
        cue.impact_played = false;
        cue.impact_shake_fired = false;
    }

    let now = runtime.server_tick();
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

    // Boom: pre-armed so the file's lead-in ends exactly at the strike. Only
    // when we are genuinely witnessing the strike window (not a joiner arriving
    // mid-crater, whose clock is already well past impact).
    if !cue.impact_played && seconds_to_impact <= IMPACT_BOOM_LEAD_S {
        cue.impact_played = true;
        if seconds_to_impact >= -1.0 {
            scheduled.push(0.0, PlaySound::at(SoundId::MeteorShowerImpact, impact));
        }
    }

    // Strike camera kick: the frame the clock crosses impact, distance-scaled.
    if !cue.impact_shake_fired && event.has_impacted(now) {
        cue.impact_shake_fired = true;
        if seconds_to_impact >= -1.0 {
            kick.trigger_meteor_impact(impact_distance);
        }
    }
}

/// Proximity intensity in `[0, 1]` for the meteor's crossing rumble + shake,
/// given the listener's horizontal distance to the fireball's true world
/// position. Full inside [`METEOR_RUMBLE_FULL_M`], linear taper to zero at
/// [`METEOR_RUMBLE_RANGE_M`], zero beyond. Pure so the curve is unit-testable.
fn meteor_crossing_intensity(distance: f32) -> f32 {
    if !distance.is_finite() || distance <= METEOR_RUMBLE_FULL_M {
        return 1.0;
    }
    if distance >= METEOR_RUMBLE_RANGE_M {
        return 0.0;
    }
    let span = (METEOR_RUMBLE_RANGE_M - METEOR_RUMBLE_FULL_M).max(f32::EPSILON);
    (1.0 - (distance - METEOR_RUMBLE_FULL_M) / span).clamp(0.0, 1.0)
}

/// Distance from the listener to the fireball's current true world position for a
/// live, in-flight event, or `None` when there is no fireball to hear (no event,
/// not yet in flight, already struck, backdrop up, or no local player yet).
fn meteor_listener_distance(runtime: &ClientRuntime, menu: &MenuState) -> Option<f32> {
    if menu.screen.uses_menu_backdrop() {
        return None;
    }
    let event = runtime.meteor_shower?;
    let state = crate::world::meteor_world_state(
        bevy::math::Vec2::new(event.impact_position.x, event.impact_position.z),
        event.impact_tick,
        event.trajectory_seed,
        runtime.server_tick_precise(),
    )?;
    let view = runtime.local_view()?;
    let listener = Vec3::new(view.position.x, view.position.y, view.position.z);
    Some(listener.distance(state.position))
}

/// The live crossing-bed entity + which event it belongs to, so the bed is
/// spawned exactly once per event and torn down cleanly. `started` guards a
/// respawn: because the bed is a one-shot (not a loop) it may self-despawn when
/// the file ends, and we must NOT start it again mid-event. `last_volume` and
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
/// sound wrong. It is started at a FIXED lead of [`FLYBY_LEAD_S`] before impact
/// so the file's baked decay-to-silence tail lands on the strike and the bed
/// simply dies out on its own; its volume is driven each frame off the
/// proximity curve (inaudible kilometres out, swelling as it nears). If it ever
/// has to stop while still audible (menu opened, event replaced) it fades over
/// ~0.25 s instead of being cut mid-waveform, which used to produce a loud
/// click at impact. Zero wire cost: the distance is computed client-side from
/// the announce payload. Runs in `ClientSystemSet::Sky`.
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

    // Is there an in-flight fireball to hear, and how loud?
    let live = runtime
        .meteor_shower
        .filter(|_| !menu.screen.uses_menu_backdrop());
    let distance = meteor_listener_distance(&runtime, &menu);

    match (live, distance) {
        (Some(event), Some(distance)) => {
            // Reset the bed for a new event: fade out any prior entity's leftovers
            // is moot here (a replaced event is rare); drop it and clear the
            // once-per-event start guard.
            if loop_state.event_tick != event.impact_tick {
                if let Some(entity) = loop_state.entity.take() {
                    // The bed is a one-shot that may have already self-despawned
                    // at the file's end; despawning it again must stay silent.
                    commands.entity(entity).try_despawn();
                }
                loop_state.event_tick = event.impact_tick;
                loop_state.started = false;
            }
            let intensity = meteor_crossing_intensity(distance);
            let seconds_to_impact = event.seconds_to_impact(runtime.server_tick());
            // Start the one-shot at the fixed lead so the file's silent tail ends
            // at the strike (no teardown cut, no click). Never restart it:
            // `started` stays true for the whole event even after the file ends.
            if !loop_state.started && seconds_to_impact <= FLYBY_LEAD_S {
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
                // Gain-scale the live bed by proximity. `sinks.get_mut` fails
                // silently once the one-shot self-despawns at the file's end,
                // which is fine (nothing left to scale).
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
        _ => {
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

/// Drive the slight continuous camera shake while the meteor crosses the sky,
/// ramping with the same proximity curve as the rumble and hard-capped small (a
/// fraction of the explosion kick, per the owner's "not too much"). Cuts off at
/// impact: the impact's own shake fires separately from the explosion path, so
/// this only covers the crossing. Runs in `ClientSystemSet::Sky`, before the
/// camera consumes the kick.
pub(crate) fn meteor_shower_camera_shake_system(
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut kick: ResMut<CameraImpactKick>,
) {
    let Some(distance) = meteor_listener_distance(&runtime, &menu) else {
        return;
    };
    let intensity = meteor_crossing_intensity(distance);
    if intensity > 0.0 {
        kick.trigger_meteor_rumble(intensity);
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
    let mut h = crater_surface_height(r);
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
        positions.push([0.0, crater_surface_height(0.0), 0.0]);
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

/// Build the impact-site rig at `position`: the dug-in crater (raised rim bowl
/// and fading burn skirt, one vertex-coloured mesh) plus the scattered
/// particle-fire emitters (when the burn window is still open at
/// `age_seconds`). No persistent rubble and no static flame geometry:
/// everything fiery is live particles that burn out. Also throws the one-time
/// rock-and-stone blast (fixed-size physics debris + a bright flash), but only
/// when the impact just happened, never replayed for a late joiner. The rig
/// despawns with the crater window (the debris via its own `ImpactChip`
/// lifetime, the fires via [`animate_meteor_shower_site_fire_system`]).
fn spawn_crater(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    position: Vec3,
    age_seconds: f32,
) {
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
            MeteorShowerCrater,
            Transform::from_translation(position),
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
            // open; a late joiner past it gets scorch only.
            if age_seconds < METEOR_SHOWER_SITE_FIRE_SECONDS {
                for c in 0..FIRE_CLUSTER_COUNT {
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
                    // the crater surface height at their radius.
                    let dist = if c % 3 == 0 {
                        0.8 + r3 * 3.0
                    } else {
                        CRATER_RIM_END_M
                            + (CRATER_SKIRT_RADIUS_M - CRATER_RIM_END_M - 1.0) * r3.sqrt()
                    };
                    let theta = r1 * std::f32::consts::TAU;
                    let ground = crater_surface_height(dist);
                    let cluster = Vec3::new(dist * theta.cos(), ground, dist * theta.sin());

                    parent.spawn((
                        Name::new("MeteorShower Site Fire"),
                        MeteorShowerSiteFire {
                            // Hold off one interval so the rig's GlobalTransform
                            // propagates before the first particle, otherwise the
                            // first puff would emit from the world origin.
                            flame_cooldown: SITE_FLAME_INTERVAL,
                            spark_cooldown: SITE_SPARK_INTERVAL,
                            phase: hashed_unit(s ^ 0x00F1_4E55) * std::f32::consts::TAU,
                            // Per-fire size spread so the site mixes small licks
                            // and real blazes rather than a field of clones.
                            scale: 0.75 + r2 * 0.65,
                        },
                        PointLight {
                            color: Color::srgb(1.0, 0.45, 0.12),
                            intensity: FIRE_LIGHT_INTENSITY * (0.7 + r2 * 0.6),
                            range: FIRE_LIGHT_RANGE_M,
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
        spawn_impact_rock_blast(commands, meshes, materials, position, seed);
    }
}

/// Throw the meteor's rock/stone debris burst + a bright flash, once, at the
/// impact moment. Reuses the [`ImpactChip`] integrator (like the explosive
/// debris burst) but larger and rock-tinted, with more chunks: a meteor strike,
/// not a satchel charge.
fn spawn_impact_rock_blast(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    position: Vec3,
    seed: u32,
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

    for i in 0..IMPACT_DEBRIS_COUNT {
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
        let angle = (i as f32 / IMPACT_DEBRIS_COUNT as f32) * std::f32::consts::TAU + r1 * 0.9;
        let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
        let up = 1.4 + r2 * 2.2;
        // A hard, fast throw so chunks fountain up and out before arcing back:
        // this is a meteor strike, the rubble is hurled far.
        let speed = 11.0 + r3 * 12.0;
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
        // Fixed-size chunks under realistic gravity (1.8 x the shared base
        // lands near 9.8 m/s²): they launch, arc, land, bounce, and tumble out
        // under ground friction, their spin bleeding off with it. Lifetimes
        // are long enough to watch the whole fall and the chunk resting on the
        // ground for a beat before it pops out. Sizes are capped modest (the
        // largest ~0.8 m across, a stone you could just about lift) and the
        // squared draw skews the spread toward the SMALLER variants, so the
        // fountain reads as smashed-up rubble, not flying boulders.
        let chip_scale = 0.2 + r3 * r3 * 0.7;
        let rock_mesh = rock_meshes[(s >> 7) as usize % rock_meshes.len()].clone();
        commands.spawn((
            Name::new("MeteorShower Debris"),
            ImpactChip::new(
                velocity,
                spin_axis,
                4.0 + r1 * 8.0,
                5.0 + r2 * 2.5,
                chip_scale,
                1.8,
            )
            .with_fixed_scale(),
            Mesh3d(rock_mesh),
            MeshMaterial3d(material),
            Transform::from_translation(position + Vec3::Y * (0.5 + r4 * 0.8))
                .with_scale(Vec3::splat(chip_scale)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }

    // (The old dense ember-spark spray is gone: dozens of fast-spinning bright
    // cubes strobed against the sky. The blast is carried by the solid rock
    // fountain plus the flash + flash light below.)

    // A brief fireball flash at ground zero: a low, wide unlit HDR ember dome that
    // the chip integrator lets sit (zero velocity/gravity) then shrinks out. Kept
    // SHORT so it punches the impact but clears fast, and its emissive is
    // red-biased so it reads as a fireball, not a bleached white bulb that reads
    // as a central rock mass (the round 2 failure). The flung debris is the star.
    let flash_radius = 5.0;
    let flash_mesh = meshes.add(Sphere::new(1.0).mesh().ico(2).unwrap());
    let flash_material = materials.add(StandardMaterial {
        // A broad area, so it must stay DEEP to keep its hue under AgX (a bright
        // value here is exactly what bleached round 2 to a white central mass).
        // This is the deep orange ground fireball. unlit -> base_color emits.
        base_color: Color::linear_rgb(1.6, 0.22, 0.0),
        unlit: true,
        fog_enabled: false,
        ..default()
    });
    commands.spawn((
        Name::new("MeteorShower Impact Flash"),
        // Squashed low so it is a ground fireball, not a dome. Lives ~0.8s so the
        // fireball is still up while the ejecta fountains through it, then clears.
        ImpactChip::new(Vec3::ZERO, Vec3::Y, 0.0, 0.8, flash_radius, 0.0),
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
            intensity: FIRE_LIGHT_INTENSITY * 12.0,
            range: METEOR_SHOWER_IMPACT_RADIUS_M * 4.0,
            radius: 2.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_translation(position + Vec3::Y * 4.0),
        Visibility::Visible,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::CRATER_RIM_HEIGHT_M;

    #[test]
    fn crossing_intensity_is_full_close_and_zero_far() {
        assert_eq!(meteor_crossing_intensity(0.0), 1.0);
        assert_eq!(meteor_crossing_intensity(METEOR_RUMBLE_FULL_M), 1.0);
        assert_eq!(meteor_crossing_intensity(METEOR_RUMBLE_FULL_M * 0.5), 1.0);
        assert_eq!(meteor_crossing_intensity(METEOR_RUMBLE_RANGE_M), 0.0);
        assert_eq!(
            meteor_crossing_intensity(METEOR_RUMBLE_RANGE_M + 1_000.0),
            0.0
        );
        // Kilometres out (like the entry point) is silent.
        assert_eq!(meteor_crossing_intensity(6_000.0), 0.0);
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
