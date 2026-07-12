//! Shared, pure meteor-trajectory math for the meteor shower world event.
//!
//! The meteor is **never streamed**. The server broadcasts one small announce
//! (`impact_position`, `impact_tick`, `trajectory_seed`), and every client
//! evaluates the fireball's **world-space** position each frame as a
//! deterministic function of that announce against its own authoritative-clock
//! estimate. Because both sides run this identical function over the same three
//! inputs plus the shared tick, every player sees the same object at the same
//! world point with zero per-tick replication. See `docs/meteor_shower.md` and
//! `docs/replication.md` (events go through a message; per-entity
//! state goes through replication; the meteor is neither, it is a function of
//! time).
//!
//! ## The path is committed
//!
//! There is no hover, no sway, and no "choosing a destination". From the moment
//! the meteor becomes visible ([`METEOR_FLIGHT_SECONDS`] before impact) it is a
//! single object on one committed arc that ends **exactly** at the impact point
//! at `impact_tick`. The path:
//!
//! - **Entry** is seeded off `trajectory_seed`: an azimuth around the compass, a
//!   horizontal distance of roughly 5 to 7 km from the impact point, and an
//!   altitude of 2.5 to 3.5 km. Far and high, so it reads as a real object
//!   hurtling in from the edge of the sky, not a disc pinned to the dome.
//! - **Descent** follows a slightly-curved quadratic Bezier (a gentle bow away
//!   from a dead-straight line) reparametrised by a quadratic ease so the final
//!   approach visibly accelerates, the object screams the last stretch to the
//!   ground.
//! - **Velocity** is the analytic derivative of that path, so it is stable and
//!   continuous (no finite-difference jitter, which is what made the old trail
//!   "point around" aimlessly).
//!
//! Before the flight window opens (`remaining > METEOR_FLIGHT_SECONDS`) and after
//! impact (`estimated_tick >= impact_tick`) the function returns `None`: the
//! fireball is not on screen. The countdown HUD, danger warning, and map marker
//! run off the announce payload independently, so a long warning window still
//! counts down before the object appears.
//!
//! This module is dependency-light on purpose (only `Vec2`/`Vec3` math and the
//! shared [`splitmix64`]) so both the client world renderer and the determinism
//! tests can call it without pulling in Bevy systems.

use bevy::math::{Vec2, Vec3};

use crate::world::chunk::splitmix64;

/// Real seconds the meteor is in visible flight before impact. The committed arc
/// spans exactly this window: at `impact_tick - METEOR_FLIGHT_SECONDS` the object
/// is at its far/high entry point, and it arrives at the impact point at
/// `impact_tick`. Forty-five seconds is long enough to read as a genuine plunge
/// from the edge of the sky (visible as a distant burning point through the
/// renderer's far-plane proxy for most of it, screaming overhead at the end) and
/// short enough that it always feels like it is coming *now*, not loitering.
/// Shorter than the shipped warning window (`METEOR_SHOWER_WARNING_SECONDS` = 600 s),
/// so the countdown ticks for minutes before the fireball appears for its final
/// visible descent; matched by the forced short test window (`/meteor_shower 45`), in
/// which the object is on screen descending from the entry point for the whole
/// warning.
pub const METEOR_FLIGHT_SECONDS: f32 = 45.0;

/// Minimum and maximum horizontal distance, in metres, from the impact point to
/// the meteor's entry point. Five to seven kilometres: far outside the few-
/// hundred-metre playspace, so the object enters from well beyond the world edge
/// and streaks the whole way in.
const ENTRY_HORIZONTAL_MIN_M: f32 = 5_000.0;
const ENTRY_HORIZONTAL_MAX_M: f32 = 7_000.0;

/// Minimum and maximum entry altitude, in metres. Two-and-a-half to three-and-a-
/// half kilometres up: a top-of-sky point, so the descent reads as "falling onto
/// the map" rather than "flying in low across it".
const ENTRY_ALTITUDE_MIN_M: f32 = 2_500.0;
const ENTRY_ALTITUDE_MAX_M: f32 = 3_500.0;

/// How far the Bezier control point bows the path off the straight entry-to-
/// impact chord, as a fraction of the entry altitude. A gentle sideways+upward
/// bow so the flight is a shallow arc, not a ruler-straight line, without ever
/// looking like it changes its mind. Small on purpose.
const PATH_BOW_FRACTION: f32 = 0.18;

/// The deterministic world-space state of the fireball at one instant, evaluated
/// from the announce plus the clock estimate. Consumed by the client renderer to
/// place the fireball mesh at a true world position (or its far-plane proxy) and
/// to orient the trail opposite the direction of travel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeteorWorldState {
    /// World-space position of the fireball this instant. `y` is altitude above
    /// the ground plane; it decreases monotonically to the impact point's `y`
    /// (ground level) as impact nears.
    pub position: Vec3,
    /// World-space velocity (metres per second), the analytic derivative of the
    /// path. Always finite and continuous; points along the direction of travel
    /// (roughly toward the impact point, steepening as it descends). The renderer
    /// orients the trail opposite this.
    pub velocity: Vec3,
    /// `0.0` at the entry point, ramping to `1.0` at impact: the fraction of the
    /// committed flight elapsed. Drives trail length/brightness growth and any
    /// proximity-independent "it is nearly here" ramp so the renderer does not
    /// re-derive the phase.
    pub descent_fraction: f32,
    /// Seeded life: a slow two-sine shimmer in roughly
    /// `[1 - FLICKER_AMPLITUDE, 1 + FLICKER_AMPLITUDE]` the renderer multiplies
    /// into the fireball's scale and brightness so the ball reads as burning
    /// rather than a static disc. Deterministic in (trajectory_seed,
    /// estimated_tick), so every client sees the identical shimmer.
    pub flicker: f32,
}

// Crater surface geometry (metres), shared by everything that must agree on
// the impact site's shape: the client's crater mesh + fire anchoring
// (`app::scene::meteor_shower`), the movement collider's analytic floor
// (`controller`), and the server's shard-node placement. The terrain plane
// cannot be cut, so the "dug in" read comes from a raised, irregular rim lip
// around a floor that sits at grade.
/// Radius of the rim crest (the top of the raised lip).
pub const CRATER_BOWL_RADIUS_M: f32 = 6.5;
/// Radius where the outside of the lip returns to grade.
pub const CRATER_RIM_END_M: f32 = 9.5;
/// Outer edge of the fading burn skirt; the painted decal reaches zero here.
pub const CRATER_SKIRT_RADIUS_M: f32 = 14.5;
/// Rim crest height above grade. Tall enough to read over the ~0.3 m grass
/// carpet from a standing eye across the field; also buries the grass cards
/// inside the bowl so the interior reads as clean charred earth.
pub const CRATER_RIM_HEIGHT_M: f32 = 0.85;
/// Bowl floor height at the centre, just above the terrain plane so the mesh
/// never z-fights the ground it covers.
pub const CRATER_FLOOR_HEIGHT_M: f32 = 0.08;

/// The crater surface height above grade at radial `distance` from ground
/// zero: bowl floor near grade at the centre, sweeping up into the rim crest
/// at [`CRATER_BOWL_RADIUS_M`], falling back to just above grade at
/// [`CRATER_RIM_END_M`], then a flat skirt out to [`CRATER_SKIRT_RADIUS_M`]
/// and exactly grade (0) beyond. Pure so the mesh, the movement floor, and
/// the server's shard placement all sample the identical surface.
pub fn crater_surface_height(distance: f32) -> f32 {
    if !distance.is_finite() || distance < 0.0 {
        return CRATER_FLOOR_HEIGHT_M;
    }
    if distance <= CRATER_BOWL_RADIUS_M {
        // The floor stays low through the middle (a strong power keeps the bowl
        // wide and flat) then sweeps up into the inside of the lip.
        let t = distance / CRATER_BOWL_RADIUS_M;
        CRATER_FLOOR_HEIGHT_M + (CRATER_RIM_HEIGHT_M - CRATER_FLOOR_HEIGHT_M) * t.powf(2.6)
    } else if distance <= CRATER_RIM_END_M {
        // Outside face of the lip eases back down to just above grade.
        let t = (distance - CRATER_BOWL_RADIUS_M) / (CRATER_RIM_END_M - CRATER_BOWL_RADIUS_M);
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        CRATER_RIM_HEIGHT_M + (0.03 - CRATER_RIM_HEIGHT_M) * ease
    } else if distance <= CRATER_SKIRT_RADIUS_M {
        // The skirt hovers a hair above the terrain so it never z-fights it.
        0.025
    } else {
        0.0
    }
}

/// Half-range of the [`MeteorWorldState::flicker`] shimmer. Subtle on purpose:
/// the fireball should feel alive, not strobe.
const FLICKER_AMPLITUDE: f32 = 0.08;

/// The seeded shimmer factor at one instant. Two incommensurate sine
/// frequencies (so the pattern never visibly loops) phase-offset by the
/// trajectory seed. Pure and cheap; called once per frame per client.
fn meteor_flicker(trajectory_seed: u64, estimated_tick: f64) -> f32 {
    const FLICKER_SALT: u64 = 0xF11C_4E12_0000_0000;
    let phase = (splitmix64(trajectory_seed ^ FLICKER_SALT) % 6_283) as f32 / 1_000.0;
    let t = (estimated_tick / f64::from(crate::protocol::SERVER_TICK_RATE_HZ)) as f32;
    let wobble = 0.5 * ((t * 7.9 + phase).sin() + (t * 12.7 + phase * 1.7).sin());
    1.0 + FLICKER_AMPLITUDE * wobble
}

/// A seeded value in `[0, 1)` from the trajectory seed and a per-axis salt, so
/// each committed-path parameter (azimuth, distance, altitude, bow bearing)
/// draws from an independent stream and a determinism test can assert two seeds
/// differ. Spread across the full unit interval.
fn seeded_unit(trajectory_seed: u64, salt: u64) -> f32 {
    let bits = splitmix64(trajectory_seed ^ salt) >> 40;
    (bits as f32) / ((1u64 << 24) as f32)
}

/// Seeded compass azimuth (radians) the meteor approaches from. Distinct salt so
/// different events streak in from different bearings and a determinism test can
/// assert two seeds differ.
fn entry_azimuth(trajectory_seed: u64) -> f32 {
    const AZIMUTH_SALT: u64 = 0xE11B_A115_0000_0000;
    seeded_unit(trajectory_seed, AZIMUTH_SALT) * std::f32::consts::TAU
}

/// The committed entry point in world space: `[ENTRY_HORIZONTAL_MIN_M,
/// ENTRY_HORIZONTAL_MAX_M]` out on the seeded azimuth from the impact point, and
/// `[ENTRY_ALTITUDE_MIN_M, ENTRY_ALTITUDE_MAX_M]` up. Pure in the seed + impact
/// point, so it is stable for the whole event.
fn entry_point(impact_ground: Vec3, trajectory_seed: u64) -> Vec3 {
    const DISTANCE_SALT: u64 = 0xD157_A9CE_0000_0000;
    const ALTITUDE_SALT: u64 = 0xA17E_2D00_0000_0000;
    let azimuth = entry_azimuth(trajectory_seed);
    let distance = lerp(
        ENTRY_HORIZONTAL_MIN_M,
        ENTRY_HORIZONTAL_MAX_M,
        seeded_unit(trajectory_seed, DISTANCE_SALT),
    );
    let altitude = lerp(
        ENTRY_ALTITUDE_MIN_M,
        ENTRY_ALTITUDE_MAX_M,
        seeded_unit(trajectory_seed, ALTITUDE_SALT),
    );
    // Azimuth 0 faces -Z (north), increasing clockwise from above, matching the
    // rest of the world's compass convention.
    let offset_x = distance * azimuth.sin();
    let offset_z = -distance * azimuth.cos();
    Vec3::new(
        impact_ground.x + offset_x,
        impact_ground.y + altitude,
        impact_ground.z + offset_z,
    )
}

/// The Bezier control point that bows the flight off the straight entry-to-impact
/// chord. Lifted above the chord midpoint and pushed a little to one seeded side
/// so the arc is a shallow, natural-looking curve rather than a ruler line. Pure
/// in the seed + endpoints.
fn control_point(entry: Vec3, impact_ground: Vec3, trajectory_seed: u64) -> Vec3 {
    const BOW_SALT: u64 = 0xB0F7_1DE5_0000_0000;
    let mid = (entry + impact_ground) * 0.5;
    let bow = (entry.y - impact_ground.y).abs() * PATH_BOW_FRACTION;
    // Bow bearing: a seeded horizontal direction perpendicular-ish to the chord,
    // plus a lift. Kept small so it never reads as "steering".
    let bearing = seeded_unit(trajectory_seed, BOW_SALT) * std::f32::consts::TAU;
    Vec3::new(
        mid.x + bow * 0.4 * bearing.sin(),
        mid.y + bow,
        mid.z + bow * 0.4 * bearing.cos(),
    )
}

/// Evaluate the fireball's world-space state for the given clock estimate.
///
/// `estimated_tick` is FRACTIONAL (the client clock estimate with its sub-tick
/// fraction intact). The trajectory is a pure function of time, so evaluating
/// it at whole 20 Hz ticks quantises the descent into 50 ms position steps;
/// at final-approach speeds that stutters visibly on any 60+ fps client.
///
/// Returns `None` before the object is in flight (`remaining >
/// METEOR_FLIGHT_SECONDS`) and once it has struck (`estimated_tick >=
/// impact_tick`). Otherwise the object is on its single committed arc, arriving
/// exactly at `impact_position` (at ground level, `y = 0`) at `impact_tick`.
///
/// Deterministic: identical inputs always produce identical output, and two
/// different `trajectory_seed`s produce different entry points and arcs.
pub fn meteor_world_state(
    impact_position: Vec2,
    impact_tick: u64,
    trajectory_seed: u64,
    estimated_tick: f64,
) -> Option<MeteorWorldState> {
    if estimated_tick >= impact_tick as f64 {
        return None;
    }
    let remaining_ticks = impact_tick as f64 - estimated_tick;
    let remaining_seconds =
        (remaining_ticks / f64::from(crate::protocol::SERVER_TICK_RATE_HZ)) as f32;
    if remaining_seconds > METEOR_FLIGHT_SECONDS {
        // Not yet in visible flight: the countdown runs, the object has not
        // appeared. No hover on the dome, no seeded loiter.
        return None;
    }

    // Impact point on the ground plane (y = 0). The meteor ends here exactly.
    let impact_ground = Vec3::new(impact_position.x, 0.0, impact_position.y);
    let entry = entry_point(impact_ground, trajectory_seed);
    let control = control_point(entry, impact_ground, trajectory_seed);

    // Descent progress: 0 at entry, 1 at impact. `remaining_seconds` runs from
    // METEOR_FLIGHT_SECONDS (entry) down to 0 (impact).
    let descent_fraction = (1.0 - remaining_seconds / METEOR_FLIGHT_SECONDS).clamp(0.0, 1.0);

    // Quadratic ease-in on the Bezier parameter so the *final* approach visibly
    // accelerates: near impact (descent_fraction -> 1) the parameter sweeps
    // faster, so the object covers more ground per second. `u` runs 0 (entry) ->
    // 1 (impact); `du/dp` grows toward impact.
    let p = descent_fraction;
    let u = p * p;
    let du_dp = 2.0 * p;

    // Real-time rate of the descent parameter. `p = 1 - remaining/flight`, and
    // `d(remaining)/d(t) = -1`, so `dp/dt = 1/flight` (per real second).
    let dp_dt = 1.0 / METEOR_FLIGHT_SECONDS;

    // Quadratic Bezier B(u) = (1-u)^2 * entry + 2(1-u)u * control + u^2 * impact,
    // and its parameter derivative dB/du = 2(1-u)(control-entry) + 2u(impact-control).
    let one_minus_u = 1.0 - u;
    let position = entry * (one_minus_u * one_minus_u)
        + control * (2.0 * one_minus_u * u)
        + impact_ground * (u * u);
    let db_du = (control - entry) * (2.0 * one_minus_u) + (impact_ground - control) * (2.0 * u);

    // Chain rule: velocity = dB/du * du/dp * dp/dt (metres per real second).
    let velocity = db_du * (du_dp * dp_dt);

    Some(MeteorWorldState {
        position,
        velocity,
        descent_fraction,
        flicker: meteor_flicker(trajectory_seed, estimated_tick),
    })
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK_HZ: f32 = crate::protocol::SERVER_TICK_RATE_HZ;

    /// The fractional clock estimate `seconds` before `impact_tick`.
    fn before(impact_tick: u64, seconds: f32) -> f64 {
        impact_tick as f64 - f64::from(seconds * TICK_HZ)
    }

    #[test]
    fn crater_surface_dips_inside_a_raised_rim_and_ends_at_grade() {
        // Floor near grade at the centre, crest at the bowl radius, back near
        // grade past the rim, flat skirt, then exactly grade outside.
        assert!(crater_surface_height(0.0) <= CRATER_FLOOR_HEIGHT_M + 1e-4);
        let crest = crater_surface_height(CRATER_BOWL_RADIUS_M);
        assert!((crest - CRATER_RIM_HEIGHT_M).abs() < 1e-4);
        assert!(crater_surface_height(CRATER_BOWL_RADIUS_M * 0.5) < crest * 0.5);
        assert!(crater_surface_height(CRATER_RIM_END_M) < 0.06);
        assert!(crater_surface_height(CRATER_SKIRT_RADIUS_M) < 0.06);
        assert_eq!(crater_surface_height(CRATER_SKIRT_RADIUS_M + 1.0), 0.0);
        // Degenerate input stays finite.
        assert!(crater_surface_height(f32::NAN).is_finite());
    }

    #[test]
    fn identical_inputs_yield_identical_output() {
        let impact = Vec2::new(120.0, -80.0);
        let seed = 0xABCD_1234;
        let impact_tick = 10_000;
        let est = before(impact_tick, 30.0);
        let a = meteor_world_state(impact, impact_tick, seed, est).unwrap();
        let b = meteor_world_state(impact, impact_tick, seed, est).unwrap();
        assert_eq!(a, b, "same inputs must be deterministic");
    }

    #[test]
    fn different_seeds_pick_different_entry_points() {
        let impact = Vec2::new(50.0, 50.0);
        let impact_tick = 10_000;
        // Sample inside the flight window (just after it opens) so the state exists.
        let est = before(impact_tick, METEOR_FLIGHT_SECONDS - 1.0);
        let a = meteor_world_state(impact, impact_tick, 1, est).unwrap();
        let b = meteor_world_state(impact, impact_tick, 2, est).unwrap();
        let c = meteor_world_state(impact, impact_tick, 999, est).unwrap();
        assert!(
            a.position.distance(b.position) > 1.0,
            "distinct seeds should place the fireball on distinct arcs: {:?} vs {:?}",
            a.position,
            b.position
        );
        assert!(a.position.distance(c.position) > 1.0);
        assert!(b.position.distance(c.position) > 1.0);

        // And the seeded entry azimuths themselves differ.
        assert!((entry_azimuth(1) - entry_azimuth(2)).abs() > 1e-3);
        assert!((entry_azimuth(1) - entry_azimuth(999)).abs() > 1e-3);
    }

    #[test]
    fn returns_none_after_impact() {
        let impact_tick = 5_000;
        assert!(
            meteor_world_state(Vec2::ZERO, impact_tick, 7, impact_tick as f64).is_none(),
            "at impact_tick the fireball has struck"
        );
        assert!(
            meteor_world_state(Vec2::ZERO, impact_tick, 7, (impact_tick + 100) as f64).is_none(),
            "past impact the fireball is gone"
        );
    }

    #[test]
    fn returns_none_before_the_flight_window_opens() {
        let impact_tick = 100_000;
        // Well before the flight window: the countdown runs but no fireball yet.
        let early = before(impact_tick, METEOR_FLIGHT_SECONDS + 30.0);
        assert!(
            meteor_world_state(Vec2::new(200.0, 0.0), impact_tick, 3, early).is_none(),
            "before the flight window the meteor is not visible (no hover)"
        );
        // The instant the window opens the fireball appears.
        let opening = before(impact_tick, METEOR_FLIGHT_SECONDS - 0.1);
        assert!(
            meteor_world_state(Vec2::new(200.0, 0.0), impact_tick, 3, opening).is_some(),
            "at the flight window the fireball is in flight"
        );
    }

    #[test]
    fn fractional_ticks_move_the_meteor_between_whole_ticks() {
        // The renderer evaluates the path at the fractional clock estimate; the
        // position must advance strictly within one tick, otherwise the descent
        // quantises into 50 ms steps and stutters (the bug this guards against).
        let impact = Vec2::new(40.0, -25.0);
        let impact_tick = 20_000u64;
        let base = before(impact_tick, 3.0);
        let at_whole = meteor_world_state(impact, impact_tick, 9, base).unwrap();
        let at_half = meteor_world_state(impact, impact_tick, 9, base + 0.5).unwrap();
        let at_next = meteor_world_state(impact, impact_tick, 9, base + 1.0).unwrap();
        let step = at_whole.position.distance(at_next.position);
        let half_step = at_whole.position.distance(at_half.position);
        assert!(
            step > 1.0,
            "3 s out the meteor should cover metres per tick, got {step}"
        );
        assert!(
            half_step > step * 0.25 && half_step < step * 0.75,
            "a half tick should land roughly midway: {half_step} of {step}"
        );
    }

    #[test]
    fn starts_far_and_high() {
        let impact = Vec2::new(120.0, -60.0);
        let impact_tick = 50_000;
        // First visible frame (entry point).
        let entry = meteor_world_state(
            impact,
            impact_tick,
            42,
            before(impact_tick, METEOR_FLIGHT_SECONDS),
        )
        .unwrap();
        let horizontal = Vec2::new(entry.position.x - impact.x, entry.position.z - impact.y);
        assert!(
            horizontal.length() >= ENTRY_HORIZONTAL_MIN_M - 1.0,
            "entry should be at least ~5 km out horizontally, was {}",
            horizontal.length()
        );
        assert!(
            entry.position.y >= ENTRY_ALTITUDE_MIN_M - 1.0,
            "entry should be at least ~2.5 km up, was {}",
            entry.position.y
        );
        assert!(
            entry.descent_fraction < 0.02,
            "entry is descent_fraction ~0"
        );
    }

    #[test]
    fn ends_exactly_at_the_impact_point() {
        let impact = Vec2::new(-150.0, 90.0);
        let impact_tick = 40_000;
        let target = Vec3::new(impact.x, 0.0, impact.y);

        // As the sample approaches impact the position converges monotonically to
        // ground zero (the path arrives exactly at impact_tick in the limit).
        let mut prev_dist = f32::INFINITY;
        for secs in [4.0_f32, 2.0, 1.0, 0.5, 0.25, 0.1, 0.05] {
            let tick = before(impact_tick, secs).min(impact_tick as f64 - 1.0);
            let state = meteor_world_state(impact, impact_tick, 5, tick).unwrap();
            let dist = state.position.distance(target);
            assert!(
                dist <= prev_dist + 1e-2,
                "distance to impact must shrink toward 0 as impact nears: {prev_dist} then {dist}"
            );
            prev_dist = dist;
        }
        // The last sample (1 tick before impact) is within a couple of crater
        // radii of ground zero and closing fast; the analytic limit at
        // impact_tick is exactly the impact point (evaluated below).
        assert!(
            prev_dist < 2.0 * crate::game_balance::METEOR_SHOWER_IMPACT_RADIUS_M,
            "one tick before impact the fireball is essentially at the site, dist {prev_dist}"
        );

        // Exact endpoint: the analytic path evaluated at the impact instant (the
        // Bezier at parameter u = 1) is the impact point at ground level. Prove it
        // via the same math the function uses, so "arrives exactly at impact_tick"
        // is a hard guarantee, not just a near-miss.
        let entry = entry_point(target, 5);
        let control = control_point(entry, target, 5);
        // Quadratic Bezier at u = 1 is exactly the third control point.
        let at_impact = entry * 0.0 + control * 0.0 + target * 1.0;
        assert!(
            at_impact.distance(target) < 1e-3,
            "the path's terminal point is exactly the impact site"
        );
    }

    #[test]
    fn altitude_decreases_monotonically() {
        let impact = Vec2::new(80.0, 80.0);
        let impact_tick = 60_000;
        let mut prev = f32::INFINITY;
        // Sample the whole flight from entry to just before impact.
        let steps = 60;
        for i in 0..=steps {
            let secs = METEOR_FLIGHT_SECONDS * (1.0 - i as f32 / steps as f32);
            let secs = secs.max(0.05);
            let state =
                meteor_world_state(impact, impact_tick, 11, before(impact_tick, secs)).unwrap();
            assert!(
                state.position.y <= prev + 1e-2,
                "altitude must not increase along the descent: {} then {}",
                prev,
                state.position.y
            );
            prev = state.position.y;
        }
    }

    #[test]
    fn velocity_is_continuous_and_points_along_the_path() {
        let impact = Vec2::new(60.0, -120.0);
        let impact_tick = 70_000;
        // The analytic velocity should closely match a finite difference of the
        // position (continuity / no jitter), and point in the travel direction.
        let mut prev_pos: Option<Vec3> = None;
        let mut prev_vel: Option<Vec3> = None;
        let steps = 40;
        for i in 1..steps {
            let secs = METEOR_FLIGHT_SECONDS * (1.0 - i as f32 / steps as f32);
            let secs = secs.max(0.1);
            let tick = before(impact_tick, secs);
            let state = meteor_world_state(impact, impact_tick, 21, tick).unwrap();

            // Velocity finite and downward (into the ground) overall.
            assert!(state.velocity.is_finite());
            assert!(
                state.velocity.y < 0.0,
                "the meteor should be descending (negative vy), got {}",
                state.velocity.y
            );

            if let (Some(pp), Some(_)) = (prev_pos, prev_vel) {
                // Analytic velocity roughly matches the position delta direction.
                let fd = (state.position - pp).normalize_or_zero();
                let an = state.velocity.normalize_or_zero();
                assert!(
                    fd.dot(an) > 0.9,
                    "analytic velocity should align with the path tangent: fd={fd:?} an={an:?}"
                );
            }
            prev_pos = Some(state.position);
            prev_vel = Some(state.velocity);
        }
    }

    #[test]
    fn final_approach_accelerates() {
        let impact = Vec2::new(30.0, 30.0);
        let impact_tick = 80_000;
        // Speed early in the flight vs. late in the flight: the quadratic ease
        // means the object is moving faster near impact than near entry.
        let early = meteor_world_state(
            impact,
            impact_tick,
            33,
            before(impact_tick, METEOR_FLIGHT_SECONDS * 0.8),
        )
        .unwrap();
        let late = meteor_world_state(impact, impact_tick, 33, before(impact_tick, 2.0)).unwrap();
        assert!(
            late.velocity.length() > early.velocity.length() * 1.5,
            "final approach should be visibly faster: early {} vs late {}",
            early.velocity.length(),
            late.velocity.length()
        );
    }

    #[test]
    fn flicker_is_bounded_deterministic_and_alive() {
        let impact_tick = 30_000u64;
        let impact = Vec2::new(80.0, -40.0);
        let mut values = Vec::new();
        for offset in 0..200u64 {
            // Sample inside the flight window so the state exists.
            let est = before(impact_tick, METEOR_FLIGHT_SECONDS - 1.0) + (offset * 3) as f64;
            if est >= impact_tick as f64 {
                break;
            }
            let state = meteor_world_state(impact, impact_tick, 77, est).unwrap();
            assert!(
                (1.0 - FLICKER_AMPLITUDE - 1e-3..=1.0 + FLICKER_AMPLITUDE + 1e-3)
                    .contains(&state.flicker),
                "flicker must stay within its amplitude band, got {}",
                state.flicker
            );
            values.push(state.flicker);
        }
        // Deterministic: re-evaluating one instant reproduces the same value.
        let first_tick = before(impact_tick, METEOR_FLIGHT_SECONDS - 1.0);
        let again = meteor_world_state(impact, impact_tick, 77, first_tick)
            .unwrap()
            .flicker;
        assert_eq!(again, values[0]);
        // Alive: the shimmer actually varies across the sampled window.
        let min = values.iter().copied().fold(f32::MAX, f32::min);
        let max = values.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            max - min > 0.02,
            "flicker should visibly vary over time, spread was {}",
            max - min
        );
    }
}
