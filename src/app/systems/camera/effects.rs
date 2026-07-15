use bevy::prelude::*;

use crate::{
    controller::{RUN_SPEED, WALK_SPEED},
    items::ItemModel,
};

// Per-archetype swing-side camera kick, keyed on the swing `ItemModel`. Each
// weapon punches the camera by its weight class so the swinger feels the heft of
// what they are swinging; the four weapons stake out points across the
// hatchet..pickaxe spectrum. The Hatchet and Pickaxe values are the shipped tool
// kicks, left byte-identical so tool feel is unchanged.

// Hatchet chop: firm and lingering to sell the committed strike, still short of
// the pickaxe slam. (Unchanged shipped value.)
const AXE_KICK_PITCH: f32 = 0.026;
const AXE_KICK_DOWN: f32 = 0.016;
const AXE_KICK_DURATION: f32 = 0.14;
// Pickaxe overhead: the heaviest gather swing. (Unchanged shipped value.)
const PICKAXE_KICK_PITCH: f32 = 0.038;
const PICKAXE_KICK_DOWN: f32 = 0.024;
const PICKAXE_KICK_DURATION: f32 = 0.18;
// Bag / bare-hand / deployable punch: lightest of all, the camera barely
// shifts. (Unchanged shipped value.)
const HANDS_KICK_PITCH: f32 = 0.005;
const HANDS_KICK_DOWN: f32 = 0.002;
const HANDS_KICK_DURATION: f32 = 0.06;
// Wooden club: the LIGHTEST weapon kick, a short quick chop just above the bag
// punch and below the hatchet.
const CLUB_KICK_PITCH: f32 = 0.014;
const CLUB_KICK_DOWN: f32 = 0.008;
const CLUB_KICK_DURATION: f32 = 0.10;
// Stone spear: a committed forward thrust, a touch firmer than the hatchet but
// not overhead-heavy; the linger sells the lunge.
const SPEAR_KICK_PITCH: f32 = 0.030;
const SPEAR_KICK_DOWN: f32 = 0.018;
const SPEAR_KICK_DURATION: f32 = 0.17;
// Iron sword: a balanced arc, roughly hatchet-plus, between the spear and the
// mace.
const SWORD_KICK_PITCH: f32 = 0.032;
const SWORD_KICK_DOWN: f32 = 0.020;
const SWORD_KICK_DURATION: f32 = 0.16;
// Iron mace: clearly the HEAVIEST kick in the game, a big overhead slam that
// out-punches even the pickaxe and lingers longest.
const MACE_KICK_PITCH: f32 = 0.050;
const MACE_KICK_DOWN: f32 = 0.034;
const MACE_KICK_DURATION: f32 = 0.24;
// Wooden bow release: a medium recoil, the string's snap felt through the grip.
// Sits around the hatchet/sword band, punchy but not a slam: a bow shot should
// feel like a real loose, well above the old bare-hands placeholder.
const BOW_KICK_PITCH: f32 = 0.022;
const BOW_KICK_DOWN: f32 = 0.014;
const BOW_KICK_DURATION: f32 = 0.12;
// Crossbow fire: a HEAVY recoil, near the mace. The 55-damage bolt leaves the
// rail with a hard snap, so the camera lurches: the crossbow is the game's
// hardest-hitting shot and the kick sells it.
const CROSSBOW_KICK_PITCH: f32 = 0.045;
const CROSSBOW_KICK_DOWN: f32 = 0.030;
const CROSSBOW_KICK_DURATION: f32 = 0.20;

// "I just got hit by a player" reaction. Larger and more downward-biased
// than any swing-side kick, the camera jolts down rather than up, so the
// recipient can tell at a glance whether the wobble was their own swing or
// an incoming hit. The profile scales with the attacker's swing archetype so a
// mace blow rocks the camera hardest and a club jab the least.
const HIT_RECEIVED_AXE_PITCH: f32 = 0.015;
const HIT_RECEIVED_AXE_DOWN: f32 = 0.045;
const HIT_RECEIVED_AXE_DURATION: f32 = 0.16;
const HIT_RECEIVED_PICKAXE_PITCH: f32 = 0.025;
const HIT_RECEIVED_PICKAXE_DOWN: f32 = 0.075;
const HIT_RECEIVED_PICKAXE_DURATION: f32 = 0.22;
// Club incoming: the lightest hit reaction (a quick knock), below the hatchet.
const HIT_RECEIVED_CLUB_PITCH: f32 = 0.012;
const HIT_RECEIVED_CLUB_DOWN: f32 = 0.036;
const HIT_RECEIVED_CLUB_DURATION: f32 = 0.14;
// Spear incoming: a sharp puncture jolt, between the hatchet and the pickaxe.
const HIT_RECEIVED_SPEAR_PITCH: f32 = 0.019;
const HIT_RECEIVED_SPEAR_DOWN: f32 = 0.058;
const HIT_RECEIVED_SPEAR_DURATION: f32 = 0.18;
// Sword incoming: a solid cut, just under the pickaxe.
const HIT_RECEIVED_SWORD_PITCH: f32 = 0.022;
const HIT_RECEIVED_SWORD_DOWN: f32 = 0.066;
const HIT_RECEIVED_SWORD_DURATION: f32 = 0.20;
// Mace incoming: the HARDEST hit reaction, a bone-rocking overhead that
// out-jolts every other source.
const HIT_RECEIVED_MACE_PITCH: f32 = 0.034;
const HIT_RECEIVED_MACE_DOWN: f32 = 0.100;
const HIT_RECEIVED_MACE_DURATION: f32 = 0.26;

// Explosion shake: a placed charge detonating nearby rocks the camera scaled by
// proximity. At ground zero it is the hardest kick in the game (a breach should
// feel bigger than any melee blow); it falls off linearly to nothing at
// `EXPLOSION_SHAKE_RANGE_M` so a distant breach is felt as a faint tremor and a
// far one not at all. The base values below are the ground-zero kick; the
// distance falloff in `explosion_shake_falloff` scales them down.
const EXPLOSION_KICK_PITCH: f32 = 0.060;
const EXPLOSION_KICK_DOWN: f32 = 0.045;
const EXPLOSION_KICK_DURATION: f32 = 0.32;
/// Beyond this horizontal distance (metres) from the blast the shake is zero.
/// Well inside the `EXPLOSION_CUE_RANGE_M` (120 m) at which the cue is even sent,
/// so the audio/VFX carry far but the camera only shakes when the blast is close
/// enough to matter (strong inside ~10 m, gone by here).
const EXPLOSION_SHAKE_RANGE_M: f32 = 30.0;
/// Distance (metres) within which the shake is at full ground-zero strength
/// before it starts falling off. Keeps a charge that goes off right next to you
/// at full intensity rather than already fading.
const EXPLOSION_SHAKE_FULL_M: f32 = 10.0;

/// Fraction of the ground-zero explosion kick the meteor's continuous crossing
/// shake reaches at its peak (closest approach / lowest altitude). Kept below
/// the impact's own kick so the approach reads as a building tremor and the
/// strike as the payoff, but strong enough to be unmistakably felt.
const METEOR_SHAKE_PEAK_FRACTION: f32 = 0.45;
/// Re-arm window for the per-frame meteor rumble kick. Short but longer than a
/// frame so the continuously re-armed pulse sustains smoothly into a steady
/// tremor instead of stuttering.
const METEOR_SHAKE_DURATION: f32 = 0.18;

// Meteor impact shake: one hard kick at the strike, felt from much further out
// than a satchel charge (a meteor rocks the whole region, not a doorway).
// Ground-zero magnitude sits above the explosion kick and the falloff reaches
// out to `METEOR_IMPACT_SHAKE_RANGE_M`, so a viewer watching from a few hundred
// metres still feels a distinct thud while someone across the map feels nothing.
const METEOR_IMPACT_KICK_PITCH: f32 = 0.085;
const METEOR_IMPACT_KICK_DOWN: f32 = 0.065;
const METEOR_IMPACT_KICK_DURATION: f32 = 0.55;
/// Beyond this horizontal distance (metres) from the strike the impact shake is
/// zero.
const METEOR_IMPACT_SHAKE_RANGE_M: f32 = 700.0;
/// Distance (metres) within which the impact shake is at full strength.
const METEOR_IMPACT_SHAKE_FULL_M: f32 = 80.0;

// Head bob: walk-speed cadence is ~2 footsteps/sec, which is one full sine
// cycle per second (a step is half a cycle). BOB_FREQ_CYCLES_PER_METER *
// walk_speed ≈ 1.0 cycle/sec keeps the bob in step with the player's gait.
const BOB_FREQ_CYCLES_PER_METER: f32 = 0.192;
// Peak bob displacement at walk speed. Running scales up linearly until
// `BOB_AMP_SPEED_CAP_FRACTION` of walk speed, then plateaus so very fast
// motion doesn't shake the camera apart.
const BOB_BASE_AMP_METERS: f32 = 0.012;
const BOB_AMP_SPEED_CAP_FRACTION: f32 = 1.5;
const BOB_AMP_LERP_RATE: f32 = 12.0;

// Run FOV: full +RUN_FOV_BOOST_DEG when horizontal speed reaches
// RUN_SPEED, linear ramp from WALK_SPEED upward. The boost is small on
// purpose, enough to register peripherally without warping the geometry.
pub(super) const BASE_FOV_DEG: f32 = 65.0;
pub(super) const RUN_FOV_BOOST_DEG: f32 = 5.0;
const FOV_LERP_RATE: f32 = 8.0;

// Ranged draw FOV pinch: at full bow draw the main camera's FOV narrows by this
// many degrees, scaled linearly by the draw fraction. A second additive offset
// stacked beside the run boost (the boost widens, the pinch tightens), riding the
// same lerp rate so it eases in as the draw ramps and eases back out on release /
// cancel / swap (the draw fraction going to zero is the restore; no separate
// restore path can be forgotten). Small on purpose: focus, not a scope zoom.
pub(super) const RANGED_FOV_PINCH_DEG: f32 = 4.0;

// Crossbow aim-down-sights pinch: at full ADS (right mouse held with a ready
// crossbow) the FOV narrows by this many degrees, scaled by the aim fraction.
// Deliberately stronger than the bow's draw pinch: the ADS hold exists to give
// an experienced shooter a better read on where the bolt lands, so it is a
// real (if modest) zoom rather than a focus cue. Rides the same lerp/decay
// path as the draw pinch, so releasing the aim restores through the one decay.
pub(super) const CROSSBOW_ADS_PINCH_DEG: f32 = 10.0;

// Landing dip: half-sine pulse on touchdown. Triggered when the player goes
// from airborne to grounded with a downward velocity below the minimum
// trigger, scaled toward the max amplitude at terminal fall speed.
const LANDING_DIP_TRIGGER_SPEED: f32 = 2.0;
const LANDING_DIP_MAX_FALL_SPEED: f32 = 22.0;
const LANDING_DIP_MAX_METERS: f32 = 0.085;
const LANDING_DIP_DURATION: f32 = 0.22;

#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct CameraMotionEffects {
    bob_phase: f32,
    bob_amp_smooth: f32,
    /// Player-chosen base vertical FOV in degrees. Refreshed from settings
    /// every frame by [`super::follow::camera_follow_system`]; the run boost
    /// (`fov_offset_deg`) stacks on top of this.
    base_fov_deg: f32,
    fov_offset_deg: f32,
    /// Smoothed ranged-draw pinch in degrees, subtracted from the FOV while a bow
    /// draw is held (see [`RANGED_FOV_PINCH_DEG`]). Advanced by
    /// [`Self::advance_ranged_pinch`] toward `pinch * draw_fraction`; decays back
    /// to zero when the draw ends, which is the restore path.
    ranged_pinch_deg: f32,
    was_grounded: bool,
    prev_fall_speed: f32,
    dip_elapsed: f32,
    dip_amplitude: f32,
}

impl Default for CameraMotionEffects {
    fn default() -> Self {
        Self {
            bob_phase: 0.0,
            bob_amp_smooth: 0.0,
            base_fov_deg: BASE_FOV_DEG,
            fov_offset_deg: 0.0,
            ranged_pinch_deg: 0.0,
            // Default to "grounded" so the first frame after a session
            // start doesn't trigger a phantom landing dip.
            was_grounded: true,
            prev_fall_speed: 0.0,
            dip_elapsed: 0.0,
            dip_amplitude: 0.0,
        }
    }
}

impl CameraMotionEffects {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn advance(
        &mut self,
        dt: f32,
        horizontal_speed: f32,
        grounded: bool,
        fall_speed: f32,
    ) {
        let dt = dt.max(0.0);

        let bob_amp_target = if grounded {
            let fraction = (horizontal_speed / WALK_SPEED).min(BOB_AMP_SPEED_CAP_FRACTION);
            fraction * BOB_BASE_AMP_METERS
        } else {
            0.0
        };
        let bob_lerp = (BOB_AMP_LERP_RATE * dt).clamp(0.0, 1.0);
        self.bob_amp_smooth += (bob_amp_target - self.bob_amp_smooth) * bob_lerp;
        if grounded {
            self.bob_phase +=
                horizontal_speed * BOB_FREQ_CYCLES_PER_METER * std::f32::consts::TAU * dt;
            // Keep phase bounded so very long sessions don't lose precision.
            if self.bob_phase > std::f32::consts::TAU * 64.0 {
                self.bob_phase -= std::f32::consts::TAU * 64.0;
            }
        }

        let speed_above_walk = (horizontal_speed - WALK_SPEED).max(0.0);
        let speed_fraction =
            (speed_above_walk / (RUN_SPEED - WALK_SPEED).max(f32::EPSILON)).clamp(0.0, 1.0);
        let fov_target = RUN_FOV_BOOST_DEG * speed_fraction;
        let fov_lerp = (FOV_LERP_RATE * dt).clamp(0.0, 1.0);
        self.fov_offset_deg += (fov_target - self.fov_offset_deg) * fov_lerp;

        // Landing detection: airborne → grounded transition with enough
        // downward speed to be felt. Use the *previous* frame's fall speed
        // because grounding zeroes vy in the simulator.
        if !self.was_grounded && grounded && self.prev_fall_speed >= LANDING_DIP_TRIGGER_SPEED {
            let intensity = ((self.prev_fall_speed - LANDING_DIP_TRIGGER_SPEED)
                / (LANDING_DIP_MAX_FALL_SPEED - LANDING_DIP_TRIGGER_SPEED))
                .clamp(0.0, 1.0);
            self.dip_amplitude = LANDING_DIP_MAX_METERS * (0.35 + 0.65 * intensity); // small but felt even on light landings
            self.dip_elapsed = 0.0;
        }
        if self.dip_amplitude > 0.0 {
            self.dip_elapsed += dt;
            if self.dip_elapsed >= LANDING_DIP_DURATION {
                self.dip_amplitude = 0.0;
                self.dip_elapsed = 0.0;
            }
        }

        // Cache for the next tick's landing detection.
        self.prev_fall_speed = if grounded { 0.0 } else { fall_speed };
        self.was_grounded = grounded;
    }

    pub(super) fn bob_offset_y(&self) -> f32 {
        self.bob_phase.sin() * self.bob_amp_smooth
    }

    pub(super) fn landing_dip_y(&self) -> f32 {
        if self.dip_amplitude <= 0.0 {
            return 0.0;
        }
        let t = (self.dip_elapsed / LANDING_DIP_DURATION).clamp(0.0, 1.0);
        let pulse = (t * std::f32::consts::PI).sin();
        self.dip_amplitude * pulse
    }

    /// Set the player's chosen base FOV (degrees), clamped to the supported
    /// range. The run boost is added on top in [`Self::fov_radians`].
    pub(super) fn set_base_fov_deg(&mut self, degrees: f32) {
        use crate::app::state::{MAX_FOV_DEG, MIN_FOV_DEG};
        self.base_fov_deg = if degrees.is_finite() {
            degrees.clamp(MIN_FOV_DEG, MAX_FOV_DEG)
        } else {
            BASE_FOV_DEG
        };
    }

    /// Ease the ranged pinch toward `RANGED_FOV_PINCH_DEG * draw_fraction +
    /// CROSSBOW_ADS_PINCH_DEG * aim_fraction` (the same lerp shape as the run
    /// boost). A bow drives `draw_fraction`, a crossbow ADS drives
    /// `aim_fraction`; the two never overlap (different weapons) so the sum is
    /// just "whichever is active". Both are `0` whenever nothing is held, so
    /// release / cancel / item swap all restore through this one decay; there
    /// is no separate restore call to forget. A non-finite fraction is treated
    /// as zero so a corrupted input can never wedge the FOV.
    pub(super) fn advance_ranged_pinch(&mut self, dt: f32, draw_fraction: f32, aim_fraction: f32) {
        let sanitize = |fraction: f32| {
            if fraction.is_finite() {
                fraction.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let target = RANGED_FOV_PINCH_DEG * sanitize(draw_fraction)
            + CROSSBOW_ADS_PINCH_DEG * sanitize(aim_fraction);
        let lerp = (FOV_LERP_RATE * dt.max(0.0)).clamp(0.0, 1.0);
        self.ranged_pinch_deg += (target - self.ranged_pinch_deg) * lerp;
    }

    /// The current smoothed ranged-draw pinch, in degrees. Consumed by the
    /// viewmodel-camera FOV sync so the held bow tightens proportionally with the
    /// world view (the viewmodel camera's FOV is otherwise fixed at spawn).
    pub(crate) fn ranged_pinch_deg(&self) -> f32 {
        self.ranged_pinch_deg
    }

    /// The player-chosen base FOV in degrees (post-clamp). Together with
    /// [`Self::ranged_pinch_deg`] this lets the viewmodel sync compute the pinch as
    /// a proportion of the world FOV.
    pub(crate) fn base_fov_deg(&self) -> f32 {
        self.base_fov_deg
    }

    pub(super) fn fov_radians(&self) -> f32 {
        (self.base_fov_deg + self.fov_offset_deg - self.ranged_pinch_deg).to_radians()
    }
}

/// Dev combat-feel scales applied to every camera impact kick. Neutral
/// (both `1.0`) reproduces the shipped kick exactly, so a release build (Dev
/// tab hidden) is unaffected. Synced onto [`CameraImpactKick`] every frame from
/// `settings.dev.combat` by `camera_follow_system` so every `trigger` call site
/// (swing hits, node deaths, tree fells, incoming-hit reactions) picks up the
/// tuning without threading `ClientSettings` into each hot system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct KickScales {
    pub(crate) magnitude: f32,
    pub(crate) duration: f32,
}

impl Default for KickScales {
    fn default() -> Self {
        Self {
            magnitude: 1.0,
            duration: 1.0,
        }
    }
}

impl KickScales {
    /// Sanitize the raw slider values into safe multipliers: magnitude clamps to
    /// `>= 0` (0 disables the kick), duration to a positive finite value (a `0` or
    /// NaN falls back to neutral so a kick can never get a zero-length window that
    /// would divide-by-zero in `advance`).
    fn sanitized(self) -> Self {
        let magnitude = if self.magnitude.is_finite() {
            self.magnitude.max(0.0)
        } else {
            1.0
        };
        let duration = if self.duration.is_finite() && self.duration > 0.0 {
            self.duration
        } else {
            1.0
        };
        Self {
            magnitude,
            duration,
        }
    }
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub(crate) struct CameraImpactKick {
    pitch_magnitude: f32,
    down_magnitude: f32,
    duration: f32,
    elapsed: f32,
    /// Live dev combat-feel scales, refreshed each frame by
    /// `camera_follow_system`. Neutral by default so untouched sessions and
    /// release builds kick exactly as shipped.
    scales: KickScales,
}

impl CameraImpactKick {
    /// Refresh the dev combat-feel scales (called every frame from settings).
    /// Sanitized on the way in so a slammed slider can never produce an invalid
    /// kick window.
    pub(crate) fn set_scales(&mut self, scales: KickScales) {
        self.scales = scales.sanitized();
    }

    /// Trigger the swing-side camera kick for a swing of archetype `model`. The
    /// magnitude/duration scale by weight class: the club is the lightest weapon,
    /// the mace clearly the heaviest; the Hatchet/Pickaxe/Bag values are the
    /// shipped tool kicks, unchanged.
    pub(crate) fn trigger(&mut self, model: ItemModel) {
        let (pitch, down, duration) = match model {
            ItemModel::Bag | ItemModel::Deployable => {
                (HANDS_KICK_PITCH, HANDS_KICK_DOWN, HANDS_KICK_DURATION)
            }
            ItemModel::Hatchet => (AXE_KICK_PITCH, AXE_KICK_DOWN, AXE_KICK_DURATION),
            ItemModel::Pickaxe => (PICKAXE_KICK_PITCH, PICKAXE_KICK_DOWN, PICKAXE_KICK_DURATION),
            ItemModel::Club => (CLUB_KICK_PITCH, CLUB_KICK_DOWN, CLUB_KICK_DURATION),
            ItemModel::Spear => (SPEAR_KICK_PITCH, SPEAR_KICK_DOWN, SPEAR_KICK_DURATION),
            ItemModel::Sword => (SWORD_KICK_PITCH, SWORD_KICK_DOWN, SWORD_KICK_DURATION),
            ItemModel::Mace => (MACE_KICK_PITCH, MACE_KICK_DOWN, MACE_KICK_DURATION),
            // Ranged weapons recoil ON FIRE (fired straight from the ranged fire
            // path, not the swing path): the bow's medium string-snap, the
            // crossbow's near-mace lurch. A firm shot should feel like a real
            // loose, so these sit well above the old bare-hands placeholder.
            ItemModel::Bow => (BOW_KICK_PITCH, BOW_KICK_DOWN, BOW_KICK_DURATION),
            ItemModel::Crossbow => (
                CROSSBOW_KICK_PITCH,
                CROSSBOW_KICK_DOWN,
                CROSSBOW_KICK_DURATION,
            ),
            // The thrown bomb has no swing / fire kick (the blast's proximity
            // shake is its feedback); the light toss barely nudges the camera.
            // The bandage has no kick at all: it is never swung, and jolting the
            // camera when someone finishes binding a wound would read as damage.
            ItemModel::ThrownBomb | ItemModel::Bandage => {
                (HANDS_KICK_PITCH, HANDS_KICK_DOWN, HANDS_KICK_DURATION)
            }
        };
        self.apply_kick(pitch, down, duration);
    }

    /// Trigger the "I just got hit by a player" kick. Distinct profile from the
    /// swing-side kick, sharper, more downward-biased, so the recipient can tell
    /// at a glance whether the wobble was their own swing or an incoming hit.
    /// `attacker_model` scales the response by weight class: a mace blow rocks the
    /// camera hardest, a club jab the least.
    pub(crate) fn trigger_from_hit(&mut self, attacker_model: ItemModel) {
        let (pitch, down, duration) = match attacker_model {
            ItemModel::Mace => (
                HIT_RECEIVED_MACE_PITCH,
                HIT_RECEIVED_MACE_DOWN,
                HIT_RECEIVED_MACE_DURATION,
            ),
            ItemModel::Pickaxe => (
                HIT_RECEIVED_PICKAXE_PITCH,
                HIT_RECEIVED_PICKAXE_DOWN,
                HIT_RECEIVED_PICKAXE_DURATION,
            ),
            ItemModel::Sword => (
                HIT_RECEIVED_SWORD_PITCH,
                HIT_RECEIVED_SWORD_DOWN,
                HIT_RECEIVED_SWORD_DURATION,
            ),
            ItemModel::Spear => (
                HIT_RECEIVED_SPEAR_PITCH,
                HIT_RECEIVED_SPEAR_DOWN,
                HIT_RECEIVED_SPEAR_DURATION,
            ),
            ItemModel::Club => (
                HIT_RECEIVED_CLUB_PITCH,
                HIT_RECEIVED_CLUB_DOWN,
                HIT_RECEIVED_CLUB_DURATION,
            ),
            // Hatchet, the ranged weapons, and the non-combat archetypes share
            // the lighter axe profile. Bag/Deployable shouldn't reach here (the
            // server rejects bare-handed PvP), and the projectile-hit reaction is
            // driven off `ProjectileImpact` rather than this melee path, but the
            // lighter profile keeps the kick proportionate if a ranged model ever
            // surfaces here.
            ItemModel::Hatchet
            | ItemModel::Bow
            | ItemModel::Crossbow
            | ItemModel::Bag
            | ItemModel::Deployable
            | ItemModel::ThrownBomb
            // The bandage cannot be an attacker's model (it deals no damage), so
            // it never really reaches here; it takes the light profile for the
            // same defensive reason the others do.
            | ItemModel::Bandage => (
                HIT_RECEIVED_AXE_PITCH,
                HIT_RECEIVED_AXE_DOWN,
                HIT_RECEIVED_AXE_DURATION,
            ),
        };
        self.apply_kick(pitch, down, duration);
    }

    /// Trigger the proximity-scaled explosion shake for a blast at `distance`
    /// metres from the local player. Strong inside `EXPLOSION_SHAKE_FULL_M`,
    /// linear falloff to nothing at `EXPLOSION_SHAKE_RANGE_M`, so a charge going
    /// off in your face rocks the camera and a distant breach is a faint tremor
    /// or nothing. A blast beyond the range arms no kick at all (the pure falloff
    /// returns 0). The Dev combat-feel scales apply through `apply_kick` like every
    /// other source.
    pub(crate) fn trigger_from_explosion(&mut self, distance: f32) {
        let falloff = explosion_shake_falloff(distance);
        if falloff <= 0.0 {
            return;
        }
        self.apply_kick(
            EXPLOSION_KICK_PITCH * falloff,
            EXPLOSION_KICK_DOWN * falloff,
            // Duration eases down with distance too so a far tremor is a quick
            // blip and a close blast a longer rock, but never below a floor so it
            // still reads as a discrete shake.
            EXPLOSION_KICK_DURATION * (0.5 + 0.5 * falloff),
        );
    }

    /// Drive the slight continuous camera shake while the meteor shower
    /// crosses the sky. Unlike the discrete explosion kick this is re-armed every
    /// frame with a caller-computed proximity `intensity` in `[0, 1]` (see
    /// `METEOR_SHAKE_*` on the sky renderer), so the shake sustains and ramps as
    /// the fireball nears rather than firing one pulse. The peak is a small
    /// fraction of the ground-zero explosion kick even at closest approach (the
    /// owner asked for "not too much"): `METEOR_SHAKE_PEAK_FRACTION` of the
    /// explosion values, hard-capped here so it can never rival the real impact's
    /// own shake (which fires from `trigger_from_explosion` at the strike). A
    /// non-positive intensity arms nothing.
    pub(crate) fn trigger_meteor_rumble(&mut self, intensity: f32) {
        let intensity = intensity.clamp(0.0, 1.0);
        if intensity <= 0.0 {
            return;
        }
        // A short, always-refreshing window so the per-frame re-arm reads as one
        // continuous tremor rather than a machine-gun of separate pulses.
        self.apply_kick(
            EXPLOSION_KICK_PITCH * METEOR_SHAKE_PEAK_FRACTION * intensity,
            EXPLOSION_KICK_DOWN * METEOR_SHAKE_PEAK_FRACTION * intensity,
            METEOR_SHAKE_DURATION,
        );
    }

    /// Trigger the one-off meteor-strike kick for an impact `distance` metres
    /// from the local player. Harder and much further-reaching than a placed
    /// charge (see `METEOR_IMPACT_*`): the payoff the crossing tremor builds to.
    /// Beyond the range it arms nothing.
    pub(crate) fn trigger_meteor_impact(&mut self, distance: f32) {
        let falloff = meteor_impact_shake_falloff(distance);
        if falloff <= 0.0 {
            return;
        }
        self.apply_kick(
            METEOR_IMPACT_KICK_PITCH * falloff,
            METEOR_IMPACT_KICK_DOWN * falloff,
            METEOR_IMPACT_KICK_DURATION * (0.5 + 0.5 * falloff),
        );
    }

    /// Arm a kick from its base (pitch, down, duration), applying the live dev
    /// combat-feel scales. Magnitude scales both the pitch punch and the drop;
    /// duration scales the linger. If a previous kick is still decaying, take the
    /// stronger of the two magnitudes so rapid hits accumulate rather than stomp
    /// on each other. At neutral scales (both `1.0`) this is byte-identical to the
    /// old inline assignment.
    fn apply_kick(&mut self, pitch: f32, down: f32, duration: f32) {
        let scaled_pitch = pitch * self.scales.magnitude;
        let scaled_down = down * self.scales.magnitude;
        let scaled_duration = duration * self.scales.duration;
        self.pitch_magnitude = self.pitch_magnitude.max(scaled_pitch);
        self.down_magnitude = self.down_magnitude.max(scaled_down);
        self.duration = scaled_duration;
        self.elapsed = 0.0;
    }

    pub(super) fn advance(&mut self, dt: f32) -> (f32, f32) {
        if self.duration <= 0.0 {
            return (0.0, 0.0);
        }
        self.elapsed += dt.max(0.0);
        if self.elapsed >= self.duration {
            self.pitch_magnitude = 0.0;
            self.down_magnitude = 0.0;
            self.duration = 0.0;
            self.elapsed = 0.0;
            return (0.0, 0.0);
        }
        // Half-sine pulse: ramps in fast, settles smoothly.
        let t = self.elapsed / self.duration;
        let pulse = (t * std::f32::consts::PI).sin();
        (self.pitch_magnitude * pulse, self.down_magnitude * pulse)
    }
}

/// Proximity falloff `[0, 1]` for the explosion camera shake: full (`1.0`)
/// within `EXPLOSION_SHAKE_FULL_M`, linear ramp down to `0.0` at
/// `EXPLOSION_SHAKE_RANGE_M`, and `0.0` beyond. Pure so the falloff shape is
/// unit-testable without a camera. A non-finite or negative distance clamps to
/// full (a degenerate "right on top of it").
pub(crate) fn explosion_shake_falloff(distance: f32) -> f32 {
    linear_shake_falloff(distance, EXPLOSION_SHAKE_FULL_M, EXPLOSION_SHAKE_RANGE_M)
}

/// Proximity falloff for the meteor-impact kick: full inside
/// [`METEOR_IMPACT_SHAKE_FULL_M`], linear to zero at
/// [`METEOR_IMPACT_SHAKE_RANGE_M`]. Pure so the curve is unit-testable.
pub(crate) fn meteor_impact_shake_falloff(distance: f32) -> f32 {
    linear_shake_falloff(
        distance,
        METEOR_IMPACT_SHAKE_FULL_M,
        METEOR_IMPACT_SHAKE_RANGE_M,
    )
}

/// Shared linear proximity ramp: `1.0` inside `full`, `0.0` at and beyond
/// `range`, linear between. Non-finite distances degrade to full.
fn linear_shake_falloff(distance: f32, full: f32, range: f32) -> f32 {
    if !distance.is_finite() || distance <= full {
        return 1.0;
    }
    if distance >= range {
        return 0.0;
    }
    let span = (range - full).max(f32::EPSILON);
    (1.0 - (distance - full) / span).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explosion_shake_falloff_is_full_close_zero_far_and_ramps_between() {
        // Full inside the full-strength radius (and at a degenerate 0 distance).
        assert_eq!(explosion_shake_falloff(0.0), 1.0);
        assert_eq!(explosion_shake_falloff(EXPLOSION_SHAKE_FULL_M), 1.0);
        // Zero at and beyond the outer range.
        assert_eq!(explosion_shake_falloff(EXPLOSION_SHAKE_RANGE_M), 0.0);
        assert_eq!(explosion_shake_falloff(EXPLOSION_SHAKE_RANGE_M + 50.0), 0.0);
        // Strictly decreasing across the ramp, and bounded in [0, 1].
        let mid = (EXPLOSION_SHAKE_FULL_M + EXPLOSION_SHAKE_RANGE_M) / 2.0;
        let f_mid = explosion_shake_falloff(mid);
        assert!((0.0..=1.0).contains(&f_mid));
        assert!(f_mid < 1.0 && f_mid > 0.0, "midpoint is a partial shake");
        let closer = explosion_shake_falloff(mid - 5.0);
        assert!(closer > f_mid, "closer to the blast shakes harder");
        // A non-finite distance degrades to full rather than NaN-propagating.
        assert_eq!(explosion_shake_falloff(f32::NAN), 1.0);
    }

    #[test]
    fn meteor_impact_shake_reaches_far_but_not_map_wide() {
        // Full at the (survivable) close range, still clearly felt at a few
        // hundred metres (the typical spectating distance), zero across the map.
        assert_eq!(meteor_impact_shake_falloff(METEOR_IMPACT_SHAKE_FULL_M), 1.0);
        let spectator = meteor_impact_shake_falloff(300.0);
        assert!(
            spectator > 0.3 && spectator < 1.0,
            "a 300 m spectator feels a partial thud, got {spectator}"
        );
        assert_eq!(
            meteor_impact_shake_falloff(METEOR_IMPACT_SHAKE_RANGE_M),
            0.0
        );

        // The trigger arms a real kick in range and nothing beyond it.
        let mut close = CameraImpactKick::default();
        close.trigger_meteor_impact(100.0);
        let (pitch, drop) = close.advance(METEOR_IMPACT_KICK_DURATION * 0.25);
        assert!(pitch > 0.0 && drop > 0.0, "a near strike rocks the camera");
        let mut far = CameraImpactKick::default();
        far.trigger_meteor_impact(METEOR_IMPACT_SHAKE_RANGE_M + 100.0);
        assert_eq!(far.advance(0.05), (0.0, 0.0));
    }

    #[test]
    fn explosion_trigger_only_kicks_within_range() {
        // A ground-zero blast arms a real kick; a blast beyond the shake range
        // arms nothing (advance immediately reads neutral).
        let mut close = CameraImpactKick::default();
        close.trigger_from_explosion(2.0);
        let (pitch, drop) = close.advance(EXPLOSION_KICK_DURATION * 0.25);
        assert!(pitch > 0.0 && drop > 0.0, "a close blast rocks the camera");

        let mut far = CameraImpactKick::default();
        far.trigger_from_explosion(EXPLOSION_SHAKE_RANGE_M + 10.0);
        assert_eq!(
            far.advance(0.05),
            (0.0, 0.0),
            "a blast beyond the shake range arms no kick"
        );
    }

    #[test]
    fn camera_kick_pulses_and_decays_after_trigger() {
        let mut kick = CameraImpactKick::default();
        assert_eq!(kick.advance(0.1), (0.0, 0.0));

        kick.trigger(ItemModel::Pickaxe);
        let (mid_pitch, mid_drop) = kick.advance(PICKAXE_KICK_DURATION * 0.5);
        assert!(mid_pitch > 0.0);
        assert!(mid_drop > 0.0);

        let (after_pitch, after_drop) = kick.advance(PICKAXE_KICK_DURATION);
        assert_eq!((after_pitch, after_drop), (0.0, 0.0));
    }

    #[test]
    fn motion_effects_reset_to_neutral_state() {
        let mut motion = CameraMotionEffects {
            bob_phase: 1.0,
            bob_amp_smooth: 0.05,
            base_fov_deg: BASE_FOV_DEG,
            fov_offset_deg: 3.0,
            ranged_pinch_deg: 2.0,
            was_grounded: false,
            prev_fall_speed: 10.0,
            dip_elapsed: 0.1,
            dip_amplitude: 0.04,
        };

        motion.reset();

        assert_eq!(motion.bob_amp_smooth, 0.0);
        assert_eq!(motion.fov_offset_deg, 0.0);
        assert_eq!(motion.ranged_pinch_deg, 0.0);
        assert_eq!(motion.dip_amplitude, 0.0);
        assert!(
            motion.was_grounded,
            "reset should default to grounded so it cannot trigger a phantom dip"
        );
        assert_eq!(motion.bob_offset_y(), 0.0);
        assert_eq!(motion.landing_dip_y(), 0.0);
    }

    #[test]
    fn head_bob_amplitude_scales_with_horizontal_speed_while_grounded() {
        let mut motion = CameraMotionEffects::default();
        // Several short steps so the smoothed amplitude has time to ramp up.
        for _ in 0..40 {
            motion.advance(1.0 / 60.0, WALK_SPEED, true, 0.0);
        }
        let walking_amp = motion.bob_amp_smooth;
        assert!(walking_amp > 0.0);

        let mut running = CameraMotionEffects::default();
        for _ in 0..40 {
            running.advance(1.0 / 60.0, RUN_SPEED, true, 0.0);
        }
        assert!(running.bob_amp_smooth > walking_amp);
    }

    #[test]
    fn head_bob_disengages_in_the_air() {
        let mut motion = CameraMotionEffects::default();
        for _ in 0..40 {
            motion.advance(1.0 / 60.0, WALK_SPEED, true, 0.0);
        }
        let grounded_amp = motion.bob_amp_smooth;
        assert!(grounded_amp > 0.0);

        for _ in 0..40 {
            motion.advance(1.0 / 60.0, WALK_SPEED, false, 0.0);
        }
        assert!(motion.bob_amp_smooth < grounded_amp * 0.1);
    }

    #[test]
    fn base_fov_drives_resting_fov_and_run_boost_stacks() {
        let mut motion = CameraMotionEffects::default();
        motion.set_base_fov_deg(80.0);
        // At rest the FOV is exactly the player's chosen base.
        assert!((motion.fov_radians() - 80.0_f32.to_radians()).abs() < 1e-5);

        // The run boost adds on top of the chosen base, not the hardcoded one.
        for _ in 0..120 {
            motion.advance(1.0 / 60.0, RUN_SPEED, true, 0.0);
        }
        assert!(motion.fov_radians() > 80.0_f32.to_radians());
        assert!(motion.fov_radians() <= (80.0 + RUN_FOV_BOOST_DEG).to_radians() + 1e-4);
    }

    #[test]
    fn base_fov_is_clamped_to_supported_range() {
        let mut motion = CameraMotionEffects::default();
        motion.set_base_fov_deg(10_000.0);
        assert!((motion.fov_radians() - 100.0_f32.to_radians()).abs() < 1e-5);
        motion.set_base_fov_deg(0.0);
        assert!((motion.fov_radians() - 50.0_f32.to_radians()).abs() < 1e-5);
        motion.set_base_fov_deg(f32::NAN);
        assert!((motion.fov_radians() - BASE_FOV_DEG.to_radians()).abs() < 1e-5);
    }

    #[test]
    fn run_fov_offset_ramps_up_with_speed() {
        let mut motion = CameraMotionEffects::default();
        for _ in 0..120 {
            motion.advance(1.0 / 60.0, RUN_SPEED, true, 0.0);
        }
        assert!(motion.fov_offset_deg > RUN_FOV_BOOST_DEG * 0.85);

        for _ in 0..120 {
            motion.advance(1.0 / 60.0, WALK_SPEED, true, 0.0);
        }
        assert!(motion.fov_offset_deg < 0.05);
    }

    #[test]
    fn landing_dip_triggers_on_fast_touchdown_and_decays() {
        let mut motion = CameraMotionEffects::default();
        // Airborne with a hard downward velocity.
        motion.advance(1.0 / 60.0, 0.0, false, 12.0);
        // Touchdown.
        motion.advance(1.0 / 60.0, 0.0, true, 0.0);
        let initial_dip = motion.landing_dip_y();
        assert!(initial_dip > 0.0);

        for _ in 0..30 {
            motion.advance(1.0 / 60.0, 0.0, true, 0.0);
        }
        assert_eq!(motion.landing_dip_y(), 0.0);
    }

    #[test]
    fn landing_dip_ignores_gentle_touchdowns() {
        let mut motion = CameraMotionEffects::default();
        motion.advance(1.0 / 60.0, 0.0, false, 0.5);
        motion.advance(1.0 / 60.0, 0.0, true, 0.0);
        assert_eq!(motion.landing_dip_y(), 0.0);
    }

    #[test]
    fn pickaxe_kick_is_heavier_than_axe_kick() {
        let mut axe_kick = CameraImpactKick::default();
        axe_kick.trigger(ItemModel::Hatchet);
        let (axe_peak, _) = axe_kick.advance(AXE_KICK_DURATION * 0.5);

        let mut pickaxe_kick = CameraImpactKick::default();
        pickaxe_kick.trigger(ItemModel::Pickaxe);
        let (pickaxe_peak, _) = pickaxe_kick.advance(PICKAXE_KICK_DURATION * 0.5);

        assert!(pickaxe_peak > axe_peak);
    }

    /// Peak (pitch, drop) magnitude of a fresh kick for `model`, sampled at the
    /// crest of the half-sine pulse. A tiny epsilon window past the exact midpoint
    /// avoids landing on the crest of a longer/shorter window.
    fn kick_peak(model: ItemModel, sample_dt: f32) -> (f32, f32) {
        let mut kick = CameraImpactKick::default();
        kick.trigger(model);
        kick.advance(sample_dt)
    }

    #[test]
    fn every_item_model_arms_a_kick_profile() {
        // Completeness: `trigger` and `trigger_from_hit` are total over the whole
        // `ItemModel` enum. Every archetype arms a non-zero (or, for the
        // deliberately tiny bag punch, at least well-defined) kick without a
        // panic. A new variant is a compile error in both matches until covered.
        for &model in ItemModel::ALL {
            let mut swing = CameraImpactKick::default();
            swing.trigger(model);
            // A very small step lands inside the pulse for every archetype.
            let (pitch, drop) = swing.advance(0.02);
            assert!(pitch >= 0.0 && drop >= 0.0, "{model:?} swing kick sane");

            let mut hit = CameraImpactKick::default();
            hit.trigger_from_hit(model);
            let (hp, hd) = hit.advance(0.02);
            assert!(hp >= 0.0 && hd >= 0.0, "{model:?} hit kick sane");
        }
    }

    #[test]
    fn tool_kicks_are_unchanged_by_the_rekey() {
        // Anchor: the Hatchet and Pickaxe swing kicks (and the bag punch) must
        // still fire exactly the shipped tool values, byte-for-byte, sampled at a
        // fixed point in the pulse. Zero behaviour change for tools is the
        // package's hard invariant.
        let dt = 0.05;
        assert_eq!(
            kick_peak(ItemModel::Hatchet, dt),
            {
                // Reproduce the shipped hatchet kick inline from the constants.
                let t = dt / AXE_KICK_DURATION;
                let pulse = (t * std::f32::consts::PI).sin();
                (AXE_KICK_PITCH * pulse, AXE_KICK_DOWN * pulse)
            },
            "hatchet swing kick unchanged"
        );
        assert_eq!(
            kick_peak(ItemModel::Pickaxe, dt),
            {
                let t = dt / PICKAXE_KICK_DURATION;
                let pulse = (t * std::f32::consts::PI).sin();
                (PICKAXE_KICK_PITCH * pulse, PICKAXE_KICK_DOWN * pulse)
            },
            "pickaxe swing kick unchanged"
        );
        assert_eq!(
            kick_peak(ItemModel::Bag, dt),
            {
                let t = dt / HANDS_KICK_DURATION;
                let pulse = (t * std::f32::consts::PI).sin();
                (HANDS_KICK_PITCH * pulse, HANDS_KICK_DOWN * pulse)
            },
            "bag / bare-hand swing kick unchanged"
        );
    }

    #[test]
    fn mace_is_the_heaviest_and_club_the_lightest_weapon_kick() {
        // Sample each weapon's peak at the crest of its own pulse (halfway
        // through its own duration) so the comparison is magnitude, not timing.
        let club = kick_peak(ItemModel::Club, CLUB_KICK_DURATION * 0.5).0;
        let spear = kick_peak(ItemModel::Spear, SPEAR_KICK_DURATION * 0.5).0;
        let sword = kick_peak(ItemModel::Sword, SWORD_KICK_DURATION * 0.5).0;
        let mace = kick_peak(ItemModel::Mace, MACE_KICK_DURATION * 0.5).0;
        let pickaxe = kick_peak(ItemModel::Pickaxe, PICKAXE_KICK_DURATION * 0.5).0;

        // The mace out-punches every weapon and even the pickaxe (the previous
        // heaviest swing in the game).
        for other in [club, spear, sword, pickaxe] {
            assert!(mace > other, "mace out-kicks {other}");
        }
        // The club is the lightest of the four weapons.
        for other in [spear, sword, mace] {
            assert!(club < other, "club under-kicks {other}");
        }
    }

    #[test]
    fn ranged_shots_arm_a_real_recoil_and_crossbow_out_kicks_the_bow() {
        // The ranged fire recoil must be a genuine kick, not the old bare-hands
        // placeholder: both bow and crossbow out-punch the hands profile, and the
        // crossbow (the heavy hitter) out-kicks the bow.
        let hands = kick_peak(ItemModel::Bag, HANDS_KICK_DURATION * 0.5).0;
        let bow = kick_peak(ItemModel::Bow, BOW_KICK_DURATION * 0.5).0;
        let crossbow = kick_peak(ItemModel::Crossbow, CROSSBOW_KICK_DURATION * 0.5).0;

        assert!(
            bow > hands,
            "the bow recoil is more than a bare-hands nudge"
        );
        assert!(
            crossbow > hands,
            "the crossbow recoil is more than a bare-hands nudge"
        );
        assert!(
            crossbow > bow,
            "the crossbow (the heavy hitter) out-kicks the bow"
        );
        // The crossbow recoil lands in the heavyweight band (at or above the
        // sword, the game's balanced weapon kick).
        let sword = kick_peak(ItemModel::Sword, SWORD_KICK_DURATION * 0.5).0;
        assert!(
            crossbow >= sword,
            "the crossbow recoil is a heavyweight, near the mace"
        );
    }

    #[test]
    fn default_kick_scales_are_neutral() {
        // The default kick must apply neutral scales, so a release build (no Dev
        // panel) kicks exactly as shipped.
        assert_eq!(KickScales::default().magnitude, 1.0);
        assert_eq!(KickScales::default().duration, 1.0);
    }

    #[test]
    fn kick_magnitude_scale_scales_the_peak() {
        // Half magnitude halves the kick peak; double doubles it. Sampled at the
        // same relative point in the (unscaled-duration) window.
        let mut neutral = CameraImpactKick::default();
        neutral.trigger(ItemModel::Hatchet);
        let (neutral_peak, neutral_drop) = neutral.advance(AXE_KICK_DURATION * 0.5);

        let mut half = CameraImpactKick::default();
        half.set_scales(KickScales {
            magnitude: 0.5,
            duration: 1.0,
        });
        half.trigger(ItemModel::Hatchet);
        let (half_peak, half_drop) = half.advance(AXE_KICK_DURATION * 0.5);
        assert!((half_peak - neutral_peak * 0.5).abs() < 1e-6);
        assert!((half_drop - neutral_drop * 0.5).abs() < 1e-6);

        let mut double = CameraImpactKick::default();
        double.set_scales(KickScales {
            magnitude: 2.0,
            duration: 1.0,
        });
        double.trigger(ItemModel::Hatchet);
        let (double_peak, _) = double.advance(AXE_KICK_DURATION * 0.5);
        assert!((double_peak - neutral_peak * 2.0).abs() < 1e-6);
    }

    #[test]
    fn kick_magnitude_scale_zero_disables_the_kick() {
        let mut kick = CameraImpactKick::default();
        kick.set_scales(KickScales {
            magnitude: 0.0,
            duration: 1.0,
        });
        kick.trigger(ItemModel::Pickaxe);
        assert_eq!(kick.advance(PICKAXE_KICK_DURATION * 0.5), (0.0, 0.0));
    }

    #[test]
    fn kick_duration_scale_extends_the_window() {
        // At 2x duration the kick is still pulsing past the base window's end,
        // where the neutral kick has already fully decayed to zero.
        let mut neutral = CameraImpactKick::default();
        neutral.trigger(ItemModel::Hatchet);
        // One base duration in, the neutral kick has ended.
        assert_eq!(neutral.advance(AXE_KICK_DURATION), (0.0, 0.0));

        let mut stretched = CameraImpactKick::default();
        stretched.set_scales(KickScales {
            magnitude: 1.0,
            duration: 2.0,
        });
        stretched.trigger(ItemModel::Hatchet);
        // Just short of the base duration, the stretched kick is still active.
        let (pitch, _) = stretched.advance(AXE_KICK_DURATION * 0.9);
        assert!(pitch > 0.0);
    }

    #[test]
    fn ranged_pinch_ramps_toward_full_draw_and_narrows_the_fov() {
        let mut motion = CameraMotionEffects::default();
        let resting = motion.fov_radians();

        // Hold a full draw: the pinch eases toward RANGED_FOV_PINCH_DEG.
        for _ in 0..120 {
            motion.advance_ranged_pinch(1.0 / 60.0, 1.0, 0.0);
        }
        assert!(
            motion.ranged_pinch_deg() > RANGED_FOV_PINCH_DEG * 0.9,
            "pinch converges near the full-draw target, got {}",
            motion.ranged_pinch_deg()
        );
        // The pinch NARROWS the FOV (subtracts), landing near base - pinch.
        assert!(motion.fov_radians() < resting);
        let expected = (BASE_FOV_DEG - RANGED_FOV_PINCH_DEG).to_radians();
        assert!(
            (motion.fov_radians() - expected).abs() < 0.5_f32.to_radians(),
            "full-draw FOV sits near base minus the pinch"
        );
    }

    #[test]
    fn ranged_pinch_scales_with_draw_fraction() {
        // A half draw converges to half the pinch, so the tightening tracks the
        // draw ramp rather than snapping at full.
        let mut motion = CameraMotionEffects::default();
        for _ in 0..120 {
            motion.advance_ranged_pinch(1.0 / 60.0, 0.5, 0.0);
        }
        assert!(
            (motion.ranged_pinch_deg() - RANGED_FOV_PINCH_DEG * 0.5).abs() < 0.2,
            "half draw pinches about half the full amount, got {}",
            motion.ranged_pinch_deg()
        );
    }

    #[test]
    fn ranged_pinch_restores_after_the_draw_ends() {
        // Release / cancel / swap all drive the fraction to 0; the pinch must decay
        // back and the FOV return to its resting value.
        let mut motion = CameraMotionEffects::default();
        let resting = motion.fov_radians();
        for _ in 0..120 {
            motion.advance_ranged_pinch(1.0 / 60.0, 1.0, 0.0);
        }
        assert!(motion.fov_radians() < resting, "drawn FOV is pinched");

        for _ in 0..240 {
            motion.advance_ranged_pinch(1.0 / 60.0, 0.0, 0.0);
        }
        assert!(
            motion.ranged_pinch_deg() < 0.02,
            "the pinch decays to (near) zero after the draw ends"
        );
        assert!(
            (motion.fov_radians() - resting).abs() < 0.05_f32.to_radians(),
            "the FOV restores to its resting value"
        );
    }

    #[test]
    fn ranged_pinch_guards_non_finite_fractions() {
        // A NaN draw fraction must read as zero, never poisoning the smoothed FOV.
        let mut motion = CameraMotionEffects::default();
        for _ in 0..60 {
            motion.advance_ranged_pinch(1.0 / 60.0, f32::NAN, f32::NAN);
        }
        assert_eq!(motion.ranged_pinch_deg(), 0.0);
        assert!(motion.fov_radians().is_finite());
    }

    #[test]
    fn ranged_pinch_stacks_with_the_run_boost() {
        // The pinch is a second additive offset beside the run boost: running at
        // full draw lands at base + boost - pinch, not one stomping the other.
        let mut motion = CameraMotionEffects::default();
        for _ in 0..240 {
            motion.advance(1.0 / 60.0, RUN_SPEED, true, 0.0);
            motion.advance_ranged_pinch(1.0 / 60.0, 1.0, 0.0);
        }
        let expected = (BASE_FOV_DEG + RUN_FOV_BOOST_DEG - RANGED_FOV_PINCH_DEG).to_radians();
        assert!(
            (motion.fov_radians() - expected).abs() < 0.5_f32.to_radians(),
            "boost and pinch stack additively"
        );
    }

    #[test]
    fn kick_scales_sanitize_invalid_values_to_neutral() {
        // A non-finite magnitude / duration or a zero duration must fall back to
        // neutral so a slammed slider can never produce a divide-by-zero window.
        let mut kick = CameraImpactKick::default();
        kick.set_scales(KickScales {
            magnitude: f32::NAN,
            duration: 0.0,
        });
        kick.trigger(ItemModel::Hatchet);
        // Neutral magnitude and duration reproduce the shipped kick.
        let mut neutral = CameraImpactKick::default();
        neutral.trigger(ItemModel::Hatchet);
        assert_eq!(
            kick.advance(AXE_KICK_DURATION * 0.5),
            neutral.advance(AXE_KICK_DURATION * 0.5)
        );
    }
}
