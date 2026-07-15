//! Weapon-rig viewmodel math for the animatable held items: the bow's
//! draw/limb/string/arrow rig, the crossbow's cock/string/bolt rig, and the
//! bandage's unrolling tail. All of it composes per-piece local transforms on
//! top of the whole-item swing/carry transform owned by the `held` root, and
//! is shared with the third-person rig through `held_piece_local_transform`.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::{
    app::systems::items::swing_poses::{lerp, smoothstep},
    items::{HeldPieceSlot, ItemModel},
};

/// How long the bow's release flick plays out before the viewmodel settles back
/// to the carry rest, in seconds. Short and snappy: loose is a forward flick, not
/// a swing follow-through.
pub(crate) const BOW_RELEASE_SECONDS: f32 = 0.22;
/// How long the crossbow recoil kick decays over after a shot, in seconds. Punchy
/// and brief so the jolt reads as a hard report, not a wobble.
pub(crate) const CROSSBOW_RECOIL_SECONDS: f32 = 0.18;

/// The live ranged-pose inputs the held-item transform reads for a bow / crossbow,
/// computed from [`crate::app::state::RangedDrawState`] each frame. For a melee /
/// tool item every field is neutral (zero), so the melee swing path is byte
/// unchanged; a bow / crossbow drives its draw / reload / recoil pose off these.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RangedPoseInputs {
    /// Bow draw fraction in `[0, 1]` (0 rest, 1 full draw). Drives the draw pose.
    pub(crate) draw_fraction: f32,
    /// Whether a bow draw is currently being held (drives the draw pose vs the
    /// release/rest pose).
    pub(crate) drawing: bool,
    /// Bow release flick progress in `[0, 1]` (0 just released, 1 settled). Ignored
    /// while `drawing`.
    pub(crate) release_progress: f32,
    /// Crossbow reload fraction in `[0, 1]` (0 just fired, 1 ready). Drives the
    /// crank pose.
    pub(crate) reload_fraction: f32,
    /// Crossbow recoil in `[0, 1]` (1 just fired, 0 settled). Drives the fire kick.
    pub(crate) recoil: f32,
    /// Crossbow aim-down-sights fraction in `[0, 1]` (0 carry, 1 fully aimed).
    /// Slides the stock to the eye line and steadies the idle sway.
    pub(crate) aim: f32,
    /// Thrown-bomb charge wind-up in `[0, 1]` (0 carry rest, 1 fully wound up
    /// at the shoulder), from [`crate::app::state::ThrowChargeState::wind_up`].
    /// Zero for every other item; drives the bomb's hold-to-charge pose.
    pub(crate) throw_wind_up: f32,
    /// Consumable use charge in `[0, 1]` (0 carry rest, 1 the wrap is going on),
    /// from [`crate::app::state::ConsumeChargeState::use_fraction`]. Zero for every
    /// other item; drives the bandage's raise-and-wrap pose and unrolls its tail.
    pub(crate) use_fraction: f32,
    /// Settle progress in `[0, 1]` after a consumable use ended (`0` the instant
    /// it ended, `1` fully back at rest). Runs for BOTH outcomes: a completed
    /// bandage cinches off, an abandoned one drops back to the carry. Without it
    /// the item would snap from mid-wrap to rest in a single frame.
    pub(crate) use_settle: f32,
    /// The charge the last consumable use had reached when it ended. The settle
    /// blends *from* here, so an abandoned half-wrap drops back from halfway
    /// rather than from full.
    pub(crate) use_ended_at: f32,
    /// Seconds elapsed, seeding the draw tremble noise.
    pub(crate) time_seconds: f32,
}

/// Peak limb flex angle at full draw, radians (authored `BOW_LIMB_FLEX`). The
/// upper limb rotates `+flex*draw` about the flex axis, the lower `-flex*draw`,
/// so both tips curl BACK toward the archer (in-game +X, the string side) as
/// the draw ramps, the way a real stave bows under the string's pull. The
/// original signs bent the tips the other way, toward the target, which read
/// as the bow flexing backwards (owner report). Tuned down from the earlier
/// 0.62 (~35 deg per tip), which over-bent the stave into an
/// about-to-snap read (owner report); ~20 deg per tip is a clearly loaded
/// but healthy bend.
const BOW_LIMB_FLEX: f32 = 0.35;

/// The bow's model-local RIG geometry, expressed in the glb's frame (which is
/// already in-game coordinates, since the glb is post-export). All pivots /
/// anchors come straight from the authoring rig spec mapped through the export
/// (authoring (x,y,z) -> in-game (x, z, -y)), so authoring Z (limb axis) is
/// in-game Y and authoring Y (flex axis) is in-game -Z.
mod bow_rig {
    use bevy::prelude::*;

    /// Convert an authoring-space point `(x, y, z)` to the glb's in-game frame.
    pub(super) const fn from_authoring(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, z, -y)
    }

    /// The flex axis: authoring +Y maps to in-game -Z. Rotating "about authoring
    /// +Y by θ" is a rotation about this axis by θ.
    pub(super) fn flex_axis() -> Vec3 {
        Vec3::NEG_Z
    }
}

/// Local transform for one animatable held-item PIECE, composed on top of the
/// whole-item swing/carry transform. Returns [`Transform::IDENTITY`] for every
/// static piece (all melee / tool layers, the bow grip, the crossbow stock /
/// iron), so those items render exactly as before. Only the bow limbs / string
/// legs and the crossbow string carry a driven transform:
///
/// - Bow limbs flex about their authored pivots as the draw ramps (upper toward
///   the target, lower mirrored), and the string legs rotate about their limb
///   tips so their shared free (nock) end tracks the drawn nock point, forming a
///   deep V toward the archer at full draw.
/// - The crossbow string slides forward on release / back on the reload crank
///   (its nut translating along the down-range axis), each leg rotating about its
///   limb tip to track the nut.
pub(crate) fn held_piece_local_transform(
    model: ItemModel,
    slot: HeldPieceSlot,
    ranged: RangedPoseInputs,
) -> Transform {
    match (model, slot) {
        (ItemModel::Bow, HeldPieceSlot::BowLimbUpper) => bow_limb_transform(bow_draw(ranged), true),
        (ItemModel::Bow, HeldPieceSlot::BowLimbLower) => {
            bow_limb_transform(bow_draw(ranged), false)
        }
        (ItemModel::Bow, HeldPieceSlot::BowStringUpper) => {
            bow_string_transform(bow_draw(ranged), true)
        }
        (ItemModel::Bow, HeldPieceSlot::BowStringLower) => {
            bow_string_transform(bow_draw(ranged), false)
        }
        (ItemModel::Bow, HeldPieceSlot::BowArrow) => bow_arrow_transform(ranged),
        (ItemModel::Crossbow, HeldPieceSlot::CrossbowString) => {
            crossbow_string_transform(crossbow_cock(ranged))
        }
        (ItemModel::Crossbow, HeldPieceSlot::CrossbowBolt) => {
            crossbow_bolt_transform(crossbow_cock(ranged))
        }
        (ItemModel::Bandage, HeldPieceSlot::BandageTail) => {
            bandage_tail_transform(bandage_charge(ranged))
        }
        // Every static piece (and any slot that doesn't match its model) is the
        // whole-item transform alone.
        _ => Transform::IDENTITY,
    }
}

/// The bow's effective draw fraction for the rig, `0` at rest, `1` at full draw.
/// While drawing it is the live draw fraction; just after loose the release flick
/// relaxes the limbs back to rest, so the rig follows `1 - release_progress` so the
/// limbs spring forward as the string snaps off the cheek.
/// Where the bandage's loose tail is rooted, in the glb's in-game frame: the
/// bottom tangent of the roll. Authored at (0, 0, -ROLL_R) Blender Z-up, which
/// the +Y-up export maps to (0, -ROLL_R, 0) here.
///
/// This is the pivot the tail scales about, so it MUST match the tail's root
/// vertex in art/consumables/build_consumables.py. Move it there, move it here,
/// or the tail will telescope out of thin air instead of out of the roll.
const BANDAGE_TAIL_PIVOT: Vec3 = Vec3::new(0.0, -0.100, 0.0);
/// How much of the tail shows at rest. Not zero: a bandage with no tail at all
/// reads as a plain cylinder, so a stub always hangs out of the roll.
const BANDAGE_TAIL_REST_SCALE: f32 = 0.18;
/// How far the tail swings as it unrolls, in radians. A little lateral sway sells
/// the strip as cloth being pulled rather than a stick telescoping out.
const BANDAGE_TAIL_SWAY: f32 = 0.30;

/// The bandage's effective charge for the viewmodel: the live charge while a use
/// is being held, otherwise the charge it ended at, decaying over the settle.
///
/// Shared by the whole-item pose and the tail, so the roll and the strip coming
/// off it can never disagree about how far along the wrap is.
fn bandage_charge(ranged: RangedPoseInputs) -> f32 {
    let live = ranged.use_fraction.clamp(0.0, 1.0);
    if live > 0.0 {
        return live;
    }
    // Not charging: fall back from wherever the last use ended, over the settle.
    // `use_settle` is 1 when idle, so this is 0 for a player who is just carrying
    // a bandage around.
    ranged.use_ended_at.clamp(0.0, 1.0) * (1.0 - smoothstep(ranged.use_settle.clamp(0.0, 1.0)))
}

/// The bandage's loose tail: it UNROLLS out of the roll as the use charges.
///
/// The tail is authored at full extension and rooted at the roll's bottom
/// tangent, so a non-uniform scale along its length axis (in-game Z, since the
/// glb runs the strip along authoring +Y) about that root pulls it back into the
/// roll at rest and pays it out as the charge builds. Only Z scales, so the strip
/// keeps its width instead of fattening, the same trick the bow string uses.
///
/// A small rotation about the roll's own axis (in-game X, the cylinder axis) sways
/// the strip as it comes off, which is what makes it read as cloth being drawn out
/// rather than a rod extending.
fn bandage_tail_transform(charge: f32) -> Transform {
    let c = charge.clamp(0.0, 1.0);
    let eased = smoothstep(c);
    let extend = lerp(BANDAGE_TAIL_REST_SCALE, 1.0, eased);
    // Sway peaks mid-unroll and settles as the strip comes taut.
    let sway = (c * PI).sin() * BANDAGE_TAIL_SWAY;
    pivot_transform(
        BANDAGE_TAIL_PIVOT,
        Quat::from_rotation_x(sway),
        Vec3::new(1.0, 1.0, extend),
    )
}

fn bow_draw(ranged: RangedPoseInputs) -> f32 {
    if ranged.drawing {
        ranged.draw_fraction.clamp(0.0, 1.0)
    } else {
        // Right after loose (release_progress 0) the limbs are still bent from the
        // shot and spring forward as the flick settles (progress -> 1 => draw -> 0).
        (1.0 - ranged.release_progress).clamp(0.0, 1.0)
    }
}

/// The bow's NOCK point in the glb (in-game) frame at a given draw. At rest the
/// nock sits at the authored (0.16, 0, 0). At full draw it pulls straight back
/// toward the archer along the bow's own archer axis (in-game +X) with a small
/// drop toward the anchor (-Y), staying entirely in the bow's string plane. See
/// [`BOW_VIEWMODEL_FULL_NOCK`]. This is the client VIEWMODEL geometry only; the
/// server's shot direction is unaffected.
fn bow_nock_point(draw: f32) -> Vec3 {
    let d = draw.clamp(0.0, 1.0);
    // Rest nock is the authored (0.16, 0, 0); lerp to the full-draw nock, pulled
    // straight back toward the archer (+X) in the glb frame.
    let rest = bow_rig::from_authoring(0.16, 0.0, 0.0);
    rest.lerp(BOW_VIEWMODEL_FULL_NOCK, d)
}

/// Full-draw nock in the glb (in-game) frame for the FIRST-PERSON viewmodel.
///
/// The pull is a straight draw ALONG THE BOW'S OWN ARCHER AXIS (+X, the string
/// side) with a small drop toward the anchor (-Y): the whole displacement stays
/// in the bow's string plane, so wherever the draw pose yaws / rolls / trembles
/// the rig, the string stays visibly welded to the stave and pulls back with it.
/// The earlier value added a lateral (+Z) out-of-plane component to fake a
/// camera-facing V; that made the string appear to pull toward the PLAYER
/// independently of the bow's rotation (owner report: the line wasn't anchored
/// to the bow). The whole-item draw yaw turns the string plane slightly toward
/// the camera so the drawn V still reads from the side. Client VIEWMODEL
/// geometry only; the server's shot direction is unaffected.
const BOW_VIEWMODEL_FULL_NOCK: Vec3 = Vec3::new(0.40, -0.04, 0.0);

/// The rotation one bow limb applies at a given draw: about the flex axis, by
/// `+flex*draw` (upper) or `-flex*draw` (lower), which curls each tip back
/// toward the archer (+X, the string side). Shared by [`bow_limb_transform`]
/// (the whole-limb pivot transform) and [`flexed_limb_tip`] (where the tip lands
/// after the flex) so the string tracks exactly where the limb bent to.
fn bow_limb_flex_rotation(draw: f32, upper: bool) -> Quat {
    let angle = if upper {
        BOW_LIMB_FLEX * draw
    } else {
        -BOW_LIMB_FLEX * draw
    };
    Quat::from_axis_angle(bow_rig::flex_axis(), angle)
}

/// The limb pivot in the glb frame (authoring (-0.1079, 0, +/-0.085)): upper +z,
/// lower -z along the limb axis. The tip flexes about this as the draw ramps.
fn bow_limb_pivot(upper: bool) -> Vec3 {
    bow_rig::from_authoring(-0.1079, 0.0, if upper { 0.085 } else { -0.085 })
}

/// The transform for one bow limb: a rotation about its authored pivot, by
/// `-flex*draw` (upper) or `+flex*draw` (lower) about the flex axis.
fn bow_limb_transform(draw: f32, upper: bool) -> Transform {
    pivot_rotation(bow_limb_pivot(upper), bow_limb_flex_rotation(draw, upper))
}

/// Where a limb TIP lands after the draw's flex: the authored rest tip rotated
/// about the limb pivot by the same flex the limb piece applies. Anchoring each
/// string leg here (rather than the static rest tip) keeps the string connected to
/// the bent limb, so the limb flex actually reads: as the stave bows in under load
/// the string ends ride inward with the tips instead of floating off them.
fn flexed_limb_tip(draw: f32, upper: bool) -> Vec3 {
    let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
    let pivot = bow_limb_pivot(upper);
    pivot + bow_limb_flex_rotation(draw, upper) * (rest_tip - pivot)
}

/// The transform for one bow string leg. The leg is authored running from its LIMB
/// TIP (its pinned/anchored end) to the shared nock (its free end). At full draw we
/// want it to run from the FLEXED tip (where the limb bent to under load) to the
/// DRAWN nock, so the string stays welded to the bent limb at one end and to the
/// pulled arrow nock at the other, forming a deep V toward the archer.
///
/// The rest leg runs straight from the tip to the rest nock along model -Y, so its
/// LENGTH axis is Y and its slim square cross-section lives in the X-Z plane. We
/// stretch it by the length ratio along Y ONLY (a non-uniform scale, cross-section
/// left at 1.0) so the cord lengthens without fattening into a plank, rotate the
/// stretched leg from its rest direction onto the drawn direction, then translate
/// it so its anchored end lands on the flexed tip and its free end reaches the
/// drawn nock exactly.
fn bow_string_transform(draw: f32, upper: bool) -> Transform {
    // Authored rest geometry: the leg runs from the rest tip (pinned) to the rest
    // nock (free). Its length axis is the rest tip -> rest nock direction.
    let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
    let rest_nock = bow_nock_point(0.0);
    // Target geometry: from the flexed tip to the drawn nock, so the string tracks
    // the bent limb (the flex reads) and the pulled arrow nock (the V deepens).
    let flexed_tip = flexed_limb_tip(draw, upper);
    let drawn_nock = bow_nock_point(draw);

    let rest_vec = rest_nock - rest_tip;
    let drawn_vec = drawn_nock - flexed_tip;
    let rest_len = rest_vec.length();
    // Stretch along the rest leg's own length axis only, so the cord stays slim at
    // any draw depth (a uniform scale would fatten the cross-section into a plank).
    let stretch = if rest_len > 1e-6 {
        drawn_vec.length() / rest_len
    } else {
        1.0
    };
    let from = rest_vec.normalize_or_zero();
    let to = drawn_vec.normalize_or_zero();
    let rotation = Quat::from_rotation_arc(from, to);
    let scale = Vec3::new(1.0, stretch, 1.0);
    // Compose: p |-> flexed_tip + rotation * (scale * (p - rest_tip)). This maps the
    // authored rest_tip onto the flexed tip and, since scale*|rest_vec| = |drawn_vec|
    // and rotation carries `from` onto `to`, the authored rest_nock onto the drawn
    // nock. The translation term is what pivot_transform emits, offset so the pivot
    // (rest_tip) relocates to flexed_tip rather than staying fixed.
    Transform {
        translation: flexed_tip - rotation * (scale * rest_tip),
        rotation,
        scale,
    }
}

/// The nocked arrow's transform: a rigid translate that keeps its authored nock
/// end welded to the string nock, so the ready arrow slides back with the draw
/// and its exposed tip becomes the full-draw aim reference. Right after loose
/// the piece collapses to the nock point (the real arrow is flying down-range)
/// and grows back in over the tail of the release flick, reading as the archer
/// nocking the next arrow.
fn bow_arrow_transform(ranged: RangedPoseInputs) -> Transform {
    let rest_nock = bow_nock_point(0.0);
    let nock = bow_nock_point(bow_draw(ranged));
    let regrow = if ranged.drawing {
        1.0
    } else {
        // Gone for the first 60% of the release window, then a quick grow-in.
        ((ranged.release_progress - 0.6) / 0.4).clamp(0.0, 1.0)
    };
    let scale = Vec3::splat(regrow);
    // p |-> nock + scale*(p - rest_nock): the authored nock end lands exactly on
    // the drawn nock at any draw, and the collapse shrinks about the nock rather
    // than the bow grip.
    Transform {
        translation: nock - scale * rest_nock,
        rotation: Quat::IDENTITY,
        scale,
    }
}

/// The crossbow string's effective cock fraction, `1` = cocked (pulled back, the
/// ready state), `0` = released (forward). At ready it sits fully cocked. A fresh
/// shot snaps it forward (recoil 1 => cock 0); the reload crank draws it back
/// (reload_fraction 0 -> 1 => cock 0 -> 1). Recoil dominates the instant of the
/// shot, then the reload owns the crank back to cocked.
fn crossbow_cock(ranged: RangedPoseInputs) -> f32 {
    // Just fired: the recoil term forces the string forward (cock toward 0). As the
    // reload cranks, cock rises with the reload fraction back to 1 (cocked/ready).
    // When idle (no recoil, no reload), the crossbow sits ready and cocked.
    let released_by_recoil = ranged.recoil.clamp(0.0, 1.0);
    let cocked_by_reload = ranged.reload_fraction.clamp(0.0, 1.0);
    // If a reload is in progress, the string tracks the crank (0 just fired -> 1
    // ready). Otherwise it is at rest cocked, briefly knocked forward by recoil.
    if cocked_by_reload > 0.0 {
        cocked_by_reload
    } else {
        1.0 - released_by_recoil
    }
}

/// The crossbow string's transform. The nut translates along the down-range axis
/// (authoring Z -> in-game Y): cocked (cock 1) it sits back near the trigger
/// (z_nut 0.115), released (cock 0) it sits forward at the prod (z_nut 0.260). The
/// whole string primitive is modelled at the cocked rest, so we translate it by
/// the delta from cocked to the current nut position along the down-range axis.
fn crossbow_string_transform(cock: f32) -> Transform {
    // z_nut = lerp(0.260 released, 0.115 cocked). The string glb is authored at
    // the cocked nut (z 0.115), so the offset is (z_nut(cock) - 0.115) along
    // authoring Z, which maps to in-game +Y.
    let z_nut = lerp(0.260, 0.115, cock.clamp(0.0, 1.0));
    let delta_authoring_z = z_nut - 0.115;
    // Authoring +Z -> in-game +Y.
    let translation = Vec3::Y * delta_authoring_z;
    Transform::from_translation(translation)
}

/// The loaded bolt's transform: glued to the string (the same nut-following
/// slide), visible only while the crossbow is at or near cocked. On fire the
/// cock snaps to 0 and the bolt collapses (the real projectile is flying); as
/// the reload crank finishes (the last ~15% of the cock) it scales back in,
/// reading as the next bolt being seated against the latched string.
fn crossbow_bolt_transform(cock: f32) -> Transform {
    let seated = ((cock.clamp(0.0, 1.0) - 0.85) / 0.15).clamp(0.0, 1.0);
    let mut transform = crossbow_string_transform(cock);
    transform.scale = Vec3::splat(seated);
    transform
}

/// A rotation `rotation` about a pivot point `pivot` (both in the piece's local
/// frame): translate the pivot to the origin, rotate, translate back.
fn pivot_rotation(pivot: Vec3, rotation: Quat) -> Transform {
    pivot_transform(pivot, rotation, Vec3::ONE)
}

/// A per-axis scale (in the piece's local frame) then rotation about a pivot point:
/// translate the pivot to the origin, scale, rotate, translate back. Bevy applies
/// scale, then rotation, then translation, so applied to a point `p` this yields
/// `pivot + rotation*(scale*(p - pivot))`, and the translation that reproduces the
/// pivot-anchored transform is `pivot - rotation*(scale*pivot)`. A `Vec3::splat(s)`
/// scale is the uniform case; the string legs pass a Y-only stretch so the cord
/// lengthens without fattening its cross-section.
fn pivot_transform(pivot: Vec3, rotation: Quat, scale: Vec3) -> Transform {
    Transform {
        translation: pivot - rotation * (scale * pivot),
        rotation,
        scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranged(draw: f32, drawing: bool) -> RangedPoseInputs {
        RangedPoseInputs {
            draw_fraction: draw,
            drawing,
            ..Default::default()
        }
    }

    #[test]
    fn static_pieces_have_an_identity_local_transform() {
        // Every melee / tool layer, plus the bow grip and the crossbow stock /
        // iron, is a static piece: its per-piece transform is identity, so the
        // whole-item transform is the entire transform (single-layer items are
        // byte-unchanged).
        for (model, slot) in [
            (ItemModel::Sword, HeldPieceSlot::Static),
            (ItemModel::Hatchet, HeldPieceSlot::Static),
            (ItemModel::Bow, HeldPieceSlot::Static),
            (ItemModel::Crossbow, HeldPieceSlot::Static),
            // A mismatched slot (a bow limb slot on a non-bow) also falls through to
            // identity, so a stale tag can never corrupt a static item.
            (ItemModel::Sword, HeldPieceSlot::BowLimbUpper),
        ] {
            let t = held_piece_local_transform(model, slot, RangedPoseInputs::default());
            assert_eq!(t.translation, Vec3::ZERO, "{model:?}/{slot:?} no translate");
            assert!(
                t.rotation.angle_between(Quat::IDENTITY) < 1e-6,
                "{model:?}/{slot:?} no rotate"
            );
        }
    }

    #[test]
    fn bow_arrow_rides_the_string_nock_and_collapses_after_loose() {
        // At rest the nocked arrow sits exactly as authored (a ready arrow
        // always shows on the carried bow).
        let rest =
            held_piece_local_transform(ItemModel::Bow, HeldPieceSlot::BowArrow, ranged(0.0, true));
        assert!(
            rest.translation.length() < 1e-6,
            "authored at the rest nock"
        );
        assert!(
            (rest.scale - Vec3::ONE).length() < 1e-6,
            "full size at rest"
        );

        // At full draw it slides rigidly with the drawn nock (straight back
        // toward the archer in the bow's string plane), so its exposed tip
        // reads as the aim reference.
        let full =
            held_piece_local_transform(ItemModel::Bow, HeldPieceSlot::BowArrow, ranged(1.0, true));
        let expected = BOW_VIEWMODEL_FULL_NOCK - bow_rig::from_authoring(0.16, 0.0, 0.0);
        assert!(
            (full.translation - expected).length() < 1e-5,
            "the arrow slides back exactly with the string nock"
        );
        assert!(
            full.rotation.angle_between(Quat::IDENTITY) < 1e-6,
            "a rigid slide, never a re-aim"
        );

        // Right after loose the piece collapses (the real arrow is flying
        // down-range), then grows back in over the tail of the release flick.
        let just_loosed = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowArrow,
            RangedPoseInputs {
                drawing: false,
                release_progress: 0.1,
                ..Default::default()
            },
        );
        assert!(
            just_loosed.scale.length() < 1e-6,
            "the nocked arrow is gone right after loose"
        );
        let renocked = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowArrow,
            RangedPoseInputs {
                drawing: false,
                release_progress: 1.0,
                ..Default::default()
            },
        );
        assert!(
            (renocked.scale - Vec3::ONE).length() < 1e-6,
            "a settled release shows the next arrow nocked"
        );
    }

    #[test]
    fn crossbow_bolt_shows_only_while_cocked_and_rides_the_string() {
        // Idle (no recoil, no reload) is cocked: the bolt sits full size at its
        // authored spot, glued to the latched string.
        let idle = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs::default(),
        );
        assert!(
            (idle.scale - Vec3::ONE).length() < 1e-6,
            "cocked shows the bolt"
        );
        assert!(
            idle.translation.length() < 1e-6,
            "authored at the cocked nut"
        );

        // Just fired (string snapped forward): the bolt is gone, the real
        // projectile is flying.
        let fired = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs {
                recoil: 1.0,
                ..Default::default()
            },
        );
        assert!(fired.scale.length() < 1e-6, "no bolt right after the shot");

        // Mid-reload: still no bolt until the crank is nearly done.
        let cranking = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs {
                reload_fraction: 0.5,
                ..Default::default()
            },
        );
        assert!(cranking.scale.length() < 1e-6, "no bolt mid-crank");

        // Reload complete: the next bolt is seated.
        let ready = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs {
                reload_fraction: 1.0,
                ..Default::default()
            },
        );
        assert!(
            (ready.scale - Vec3::ONE).length() < 1e-6,
            "a finished reload seats the next bolt"
        );
    }

    #[test]
    fn bow_limbs_flex_back_toward_the_archer_as_the_draw_ramps() {
        // At rest both limbs are unflexed (identity); at full draw each rotates by
        // the authored flex about its pivot, in OPPOSITE directions (upper +flex,
        // lower -flex), so the bow bends symmetrically with both tips curling
        // BACK toward the archer (the string side).
        let rest_upper = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowLimbUpper,
            ranged(0.0, true),
        );
        assert!(
            rest_upper.rotation.angle_between(Quat::IDENTITY) < 1e-6,
            "an undrawn bow limb is unflexed"
        );

        let full_upper = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowLimbUpper,
            ranged(1.0, true),
        );
        let full_lower = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowLimbLower,
            ranged(1.0, true),
        );
        // Both limbs rotate by the full flex magnitude at full draw.
        assert!(
            (full_upper.rotation.angle_between(Quat::IDENTITY) - BOW_LIMB_FLEX).abs() < 1e-4,
            "the upper limb flexes by the authored angle at full draw"
        );
        assert!(
            (full_lower.rotation.angle_between(Quat::IDENTITY) - BOW_LIMB_FLEX).abs() < 1e-4,
            "the lower limb flexes by the authored angle at full draw"
        );
        // They flex in opposite directions (mirror), so the two rotations are not
        // equal.
        assert!(
            full_upper.rotation.angle_between(full_lower.rotation) > BOW_LIMB_FLEX,
            "the limbs flex in opposite directions (mirror)"
        );
        // Direction check (owner report: the stave used to bend the WRONG way,
        // toward the target): at full draw each tip must land further along
        // +X (toward the archer / string side) than its authored rest tip.
        for upper in [true, false] {
            let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
            let flexed = flexed_limb_tip(1.0, upper);
            assert!(
                flexed.x > rest_tip.x + 0.05,
                "the limb tip curls back toward the archer (upper={upper})"
            );
        }
    }

    #[test]
    fn bow_string_legs_anchor_on_the_flexed_limb_tips() {
        // The string tracks the BENT limb, not the rest stave: each leg's anchored
        // (tip) end must land on the flexed limb tip at full draw, so the limb flex
        // reads (the string ends ride inward with the tips instead of floating off
        // the un-bent rest tips). Applying the leg transform to its authored rest tip
        // must reproduce the flexed tip.
        for upper in [true, false] {
            let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
            let flexed = flexed_limb_tip(1.0, upper);
            // The flex actually moves the tip (a bent limb, not a straight one).
            assert!(
                flexed.distance(rest_tip) > 0.02,
                "the limb tip visibly flexes inward under load (upper={upper})"
            );
            let leg = held_piece_local_transform(
                ItemModel::Bow,
                if upper {
                    HeldPieceSlot::BowStringUpper
                } else {
                    HeldPieceSlot::BowStringLower
                },
                ranged(1.0, true),
            );
            let anchored_end = leg.transform_point(rest_tip);
            assert!(
                anchored_end.distance(flexed) < 1e-4,
                "the string leg's anchored end welds to the flexed limb tip (upper={upper})"
            );
        }
    }

    #[test]
    fn bow_string_forms_a_deep_v_toward_the_archer_at_full_draw() {
        // The two string legs are pinned at their limb tips and meet at the shared
        // nock. At full draw the viewmodel pulls the nock back toward the archer
        // (along +X, which maps to view +Z toward the camera) AND down toward the
        // anchor (along -Y, which maps to view -Y), so applying each leg's transform
        // to the rest nock must land BOTH legs' free ends at the SAME drawn nock,
        // that drawn nock must be clearly displaced from the rest nock (a deep pull
        // toward the eye), and the two legs must splay into a genuine V (not a
        // straight line). The pull stays in the bow's flat laterally (Z = 0); the
        // +X brings the nock toward the camera and the -Y drops it toward the cheek.
        let rest_nock = bow_nock_point(0.0);
        let drawn_nock = bow_nock_point(1.0);

        let upper = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowStringUpper,
            ranged(1.0, true),
        );
        let lower = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowStringLower,
            ranged(1.0, true),
        );
        // Each leg is pinned at its limb tip and rotated + length-scaled so its
        // free (nock) end REACHES the drawn nock exactly (the string stays
        // connected to the arrow nock through the draw). Apply each leg's transform
        // to the rest nock (its authored free end) and confirm it lands on the
        // drawn nock.
        let upper_free = upper.transform_point(rest_nock);
        let lower_free = lower.transform_point(rest_nock);
        assert!(
            upper_free.distance(drawn_nock) < 1e-4,
            "the upper string leg reaches the drawn nock"
        );
        assert!(
            lower_free.distance(drawn_nock) < 1e-4,
            "the lower string leg reaches the drawn nock"
        );
        // Both free ends meet at the same point (the V apex), forming the string V.
        assert!(
            upper_free.distance(lower_free) < 1e-4,
            "the two string legs meet at a single nock"
        );
        // The drawn nock is pulled a real distance toward the archer (a readable V,
        // not a shallow twitch), and the whole pull stays IN THE BOW'S STRING
        // PLANE (no lateral Z component): an out-of-plane pull made the string
        // appear to reach for the player independently of the bow's rotation.
        assert!(
            drawn_nock.distance(rest_nock) > 0.2,
            "the nock pulls clearly toward the archer at full draw"
        );
        assert!(
            drawn_nock.x > rest_nock.x + 0.15,
            "the drawn nock pulls back toward the archer along the bow's +X"
        );
        assert!(
            drawn_nock.z.abs() < 1e-6,
            "the pull stays in the bow's string plane, anchored to the stave"
        );
        // The two legs are genuinely splayed (not collinear): from the shared apex
        // (drawn nock) the direction to the upper limb tip and to the lower limb tip
        // are well apart, so the string reads as a V rather than a single line. The
        // legs anchor at the FLEXED tips (where the limbs bent to under load), not
        // the rest tips, so the splay is measured from those.
        let upper_tip = flexed_limb_tip(1.0, true);
        let lower_tip = flexed_limb_tip(1.0, false);
        let to_upper = (upper_tip - drawn_nock).normalize_or_zero();
        let to_lower = (lower_tip - drawn_nock).normalize_or_zero();
        let cos = to_upper.dot(to_lower);
        assert!(
            cos > -0.9 && cos < 0.6,
            "the two string legs splay into a V (not a straight line); cos = {cos}"
        );
    }

    #[test]
    fn crossbow_string_snaps_forward_on_release_and_sits_back_when_cocked() {
        // Cocked (ready, no recoil / reload): the string nut sits back near the
        // trigger. On a fresh shot (recoil 1) it snaps forward toward the prod. The
        // down-range axis is in-game +Y (authoring +Z), so "forward" is a larger y.
        let cocked = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowString,
            RangedPoseInputs::default(),
        );
        let fired = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowString,
            RangedPoseInputs {
                recoil: 1.0,
                ..Default::default()
            },
        );
        assert!(
            fired.translation.y > cocked.translation.y + 0.1,
            "the string snaps forward toward the prod on release"
        );
        // Mid-reload the nut is drawn back from the released position toward cocked.
        let mid_reload = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowString,
            RangedPoseInputs {
                reload_fraction: 0.5,
                ..Default::default()
            },
        );
        assert!(
            mid_reload.translation.y < fired.translation.y,
            "the reload crank draws the string back from the fired position"
        );
    }
}
