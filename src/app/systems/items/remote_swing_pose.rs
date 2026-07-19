//! Third-person swing poses for the rigged remote body.
//!
//! The first-person curves in [`super::swing_poses`] are authored for the
//! camera-parented viewmodel (their `forward`/`right`/`up` are view-space
//! offsets), so they can't drive a body rig. This module maps a swing `phase`
//! (0..1) to right-arm joint rotations instead, keeping the same
//! wind-up-heavy / ease-in-strike philosophy and the same per-tool impact
//! phases so a remote swing reads with the same weight as the swinger's own.
//!
//! IMPORTANT: these are **deltas applied on top of the arm's current rest
//! pose**, not absolute angles. When a tool is held the rest pose is the bent
//! *carry* pose (see `held::carry_*_rotation`); empty-handed it's the relaxed
//! locomotion pose. At phase 0 and 1 every delta is 0, so a swing winds up and
//! strikes *from* the carry and settles back into it with no jump. The animator
//! composes the shoulder delta in the body frame (raise/lower the whole arm)
//! and the elbow delta in the forearm's local frame (flex), see
//! `animate_remote_players_system`.
//!
//! The swing *duration* and *impact fraction* are shared with the first-person
//! path via `swing_duration_seconds` (see `crate::combat` / `gather`), so the
//! visuals stay in lockstep across both views.

use crate::items::ItemModel;

use super::swing_poses::{ease_in, ease_out, lerp, smoothstep};

/// Right-arm swing deltas for one frame, in radians, relative to the arm's rest
/// pose (all zero = rest, i.e. the carry pose when a tool is held).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RemoteArmPose {
    /// Shoulder pitch delta (body frame): `> 0` raises the whole arm forward and
    /// overhead (wind-up); `< 0` drives it down through the strike.
    pub(crate) shoulder_pitch: f32,
    /// Shoulder yaw delta (body frame): sweeps the arm across the body (the
    /// hatchet's diagonal cut).
    pub(crate) shoulder_yaw: f32,
    /// Shoulder roll delta (body frame): cocks the arm out on the wind-up.
    pub(crate) shoulder_roll: f32,
    /// Elbow flex delta (forearm local frame): `> 0` folds the elbow tighter so
    /// the tool draws back over the shoulder (wind-up); `< 0` extends it through
    /// the strike.
    pub(crate) forearm_pitch: f32,
    /// Small torso twist about root Y for follow-through.
    pub(crate) torso_twist: f32,
}

/// Map a swing archetype ([`ItemModel`]) + phase to the right-arm delta pose.
/// Phases match the first-person impact fractions so the body's strike crosses
/// through contact at the same instant the swinger feels it:
///
/// - Club and sword read as the hatchet's diagonal chop.
/// - The spear is a genuine forward thrust (shoulder-forward extension with a
///   body lean), distinct from the short bag jab.
/// - Bag/deployable-in-hand and bare hands read as the short jab.
pub(crate) fn remote_swing_arm_pose(model: ItemModel, phase: f32) -> RemoteArmPose {
    let phase = phase.clamp(0.0, 1.0);
    match model {
        ItemModel::Pickaxe => pickaxe_arm_pose(phase),
        // The thrown bomb's overhand lob reads as an overhand chop from PvP range,
        // so it reuses the hatchet arm archetype (keeps the toss "light", no new
        // third-person pose to maintain). The club also reuses the hatchet chop.
        ItemModel::Hatchet | ItemModel::Club | ItemModel::ThrownBomb => hatchet_arm_pose(phase),
        // The sword now has its OWN wide horizontal slash (its first-person pose is
        // a level arc, not a chop), so peers read a sweep across the body rather
        // than the hatchet's overhead diagonal. The sickle's reap is the same
        // level yaw-sweep from a peer's distance, so it reuses the sword arm
        // (the same way the club reuses the hatchet's).
        ItemModel::Sword | ItemModel::Sickle => sword_arm_pose(phase),
        ItemModel::Spear => spear_arm_pose(phase),
        // The bow's swing "phase" is driven by the draw window: a peer reads a
        // draw-to-anchor, a brief full-draw hold, and a forward loose flick.
        ItemModel::Bow => bow_arm_pose(phase),
        // The crossbow reads as a level shoulder brace with a recoil kick on fire,
        // then a dip-and-crank reload cycle.
        ItemModel::Crossbow => crossbow_arm_pose(phase),
        // Bare hands / a held bundle read as the short straight jab. The bandage
        // rides here only as a fallback: it never swings, so this pose is not
        // what a peer sees them do. The bandage's real third-person read is the
        // use charge, driven off the replicated charge fraction in
        // `players::animate_remote_held_charge_system`, not off a swing phase.
        ItemModel::Bag | ItemModel::Deployable | ItemModel::Bandage => hands_arm_pose(phase),
    }
}

/// Hatchet/hammer: a diagonal chop off the carry pose. Wind up (raise the arm
/// and fold the elbow so the head draws back over the shoulder, ease-out hang),
/// then accelerate down and across the body extending the elbow (ease-in) so
/// the head moves hardest at impact (phase 0.58), bite + dwell, then recover.
fn hatchet_arm_pose(phase: f32) -> RemoteArmPose {
    if phase <= 0.40 {
        let t = ease_out(phase / 0.40);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.0, 1.55, t),
            shoulder_yaw: lerp(0.0, 0.25, t),
            shoulder_roll: lerp(0.0, 0.30, t),
            forearm_pitch: lerp(0.0, 0.55, t),
            torso_twist: lerp(0.0, 0.22, t),
        };
    }
    if phase <= 0.58 {
        let t = ease_in((phase - 0.40) / 0.18);
        return RemoteArmPose {
            shoulder_pitch: lerp(1.55, -0.75, t),
            shoulder_yaw: lerp(0.25, -0.32, t),
            shoulder_roll: lerp(0.30, -0.08, t),
            forearm_pitch: lerp(0.55, -0.65, t),
            torso_twist: lerp(0.22, -0.16, t),
        };
    }
    if phase <= 0.72 {
        let t = smoothstep((phase - 0.58) / 0.14);
        return RemoteArmPose {
            shoulder_pitch: lerp(-0.75, -0.55, t),
            shoulder_yaw: lerp(-0.32, -0.26, t),
            shoulder_roll: lerp(-0.08, -0.05, t),
            forearm_pitch: lerp(-0.65, -0.45, t),
            torso_twist: lerp(-0.16, -0.10, t),
        };
    }
    let t = smoothstep((phase - 0.72) / 0.28);
    RemoteArmPose {
        shoulder_pitch: lerp(-0.55, 0.0, t),
        shoulder_yaw: lerp(-0.26, 0.0, t),
        shoulder_roll: lerp(-0.05, 0.0, t),
        forearm_pitch: lerp(-0.45, 0.0, t),
        torso_twist: lerp(-0.10, 0.0, t),
    }
}

/// Pickaxe: a heavy near-vertical overhead smash off the carry pose. Draw the
/// arm high overhead and fold the elbow, snap it straight down through the
/// centre (impact 0.68), dwell, then a slow recover. Yaw/roll stay ~0 so it
/// reads as a straight overhead pick rather than a side chop.
fn pickaxe_arm_pose(phase: f32) -> RemoteArmPose {
    if phase <= 0.60 {
        let t = smoothstep((phase / 0.60).clamp(0.0, 1.0));
        return RemoteArmPose {
            shoulder_pitch: lerp(0.0, 1.95, t),
            shoulder_yaw: 0.0,
            shoulder_roll: lerp(0.0, -0.05, t),
            forearm_pitch: lerp(0.0, 0.60, t),
            torso_twist: lerp(0.0, 0.10, t),
        };
    }
    if phase <= 0.68 {
        let t = ease_in((phase - 0.60) / 0.08);
        return RemoteArmPose {
            shoulder_pitch: lerp(1.95, -0.95, t),
            shoulder_yaw: 0.0,
            shoulder_roll: lerp(-0.05, 0.0, t),
            forearm_pitch: lerp(0.60, -0.50, t),
            torso_twist: lerp(0.10, -0.06, t),
        };
    }
    if phase <= 0.85 {
        let t = smoothstep((phase - 0.68) / 0.17);
        return RemoteArmPose {
            shoulder_pitch: lerp(-0.95, -0.75, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(-0.50, -0.35, t),
            torso_twist: lerp(-0.06, -0.03, t),
        };
    }
    let t = smoothstep((phase - 0.85) / 0.15);
    RemoteArmPose {
        shoulder_pitch: lerp(-0.75, 0.0, t),
        shoulder_yaw: 0.0,
        shoulder_roll: 0.0,
        forearm_pitch: lerp(-0.35, 0.0, t),
        torso_twist: lerp(-0.03, 0.0, t),
    }
}

/// Bare hands: a short straight jab off the (straight) rest pose. Cock the
/// elbow, then punch the arm forward and extend (impact 0.55), then recover.
fn hands_arm_pose(phase: f32) -> RemoteArmPose {
    if phase <= 0.45 {
        let t = ease_out(phase / 0.45);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.0, 0.35, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(0.0, 0.70, t),
            torso_twist: lerp(0.0, 0.12, t),
        };
    }
    if phase <= 0.55 {
        let t = ease_in((phase - 0.45) / 0.10);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.35, 0.95, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(0.70, -0.30, t),
            torso_twist: lerp(0.12, -0.10, t),
        };
    }
    let t = smoothstep((phase - 0.55) / 0.45);
    RemoteArmPose {
        shoulder_pitch: lerp(0.95, 0.0, t),
        shoulder_yaw: 0.0,
        shoulder_roll: 0.0,
        forearm_pitch: lerp(-0.30, 0.0, t),
        torso_twist: lerp(-0.10, 0.0, t),
    }
}

/// Spear: a committed forward thrust off the carry pose, contact at phase 0.55
/// (matching `SPEAR_IMPACT_FRACTION` so peers see contact when the swinger feels
/// it). Unlike the arc archetypes this reads as a straight lunge: the arm draws
/// back and coils the elbow slightly (wind-up), then drives the whole arm forward
/// and extends the elbow hard along the aim axis (the point punches out), with a
/// forward torso lean behind it, then recovers. Shoulder yaw/roll stay ~0 and the
/// shoulder pitch dips only a little, so the motion is dominated by elbow
/// extension + torso lean (a thrust), never a chop or overhead.
fn spear_arm_pose(phase: f32) -> RemoteArmPose {
    // Wind-up: retract. The elbow folds (drawing the spear back over the grip)
    // and the arm cocks slightly down/back, the torso winds away from the target.
    if phase <= 0.38 {
        let t = ease_out(phase / 0.38);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.0, -0.18, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(0.0, 0.85, t),
            torso_twist: lerp(0.0, 0.20, t),
        };
    }
    // Thrust: drive the arm forward and snap the elbow straight (negative flex =
    // full extension) so the point lunges along the aim axis, contact at 0.55.
    // The torso leans in behind the point (twist swings past zero to negative).
    if phase <= 0.55 {
        let t = ease_in((phase - 0.38) / 0.17);
        return RemoteArmPose {
            shoulder_pitch: lerp(-0.18, 0.14, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(0.85, -0.75, t),
            torso_twist: lerp(0.20, -0.24, t),
        };
    }
    // Brief dwell at full extension (the point held out at the target).
    if phase <= 0.68 {
        let t = smoothstep((phase - 0.55) / 0.13);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.14, 0.10, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(-0.75, -0.55, t),
            torso_twist: lerp(-0.24, -0.16, t),
        };
    }
    // Recover: retract the arm and unwind the torso back to the carry rest.
    let t = smoothstep((phase - 0.68) / 0.32);
    RemoteArmPose {
        shoulder_pitch: lerp(0.10, 0.0, t),
        shoulder_yaw: 0.0,
        shoulder_roll: 0.0,
        forearm_pitch: lerp(-0.55, 0.0, t),
        torso_twist: lerp(-0.16, 0.0, t),
    }
}

/// Sword: a wide horizontal slash off the carry pose (the third-person mirror of
/// the first-person `sword_swing_pose` arc). Contact at phase 0.60, matching
/// `SWORD_IMPACT_FRACTION`. Unlike the hatchet's overhead diagonal chop, this is
/// dominated by a shoulder-YAW sweep across the body with the shoulder pitch
/// staying shallow (a level cut), so peers read a horizontal slash. The wind-up
/// cocks the arm out to the right (positive yaw + a little roll), then the strike
/// sweeps hard to the left (yaw drives strongly negative) with a torso twist
/// behind it, then recovers.
fn sword_arm_pose(phase: f32) -> RemoteArmPose {
    // Wind-up: cock the arm out to the right and raise it a touch, loading the
    // horizontal sweep (ease-out hang at the cocked apex).
    if phase <= 0.42 {
        let t = ease_out(phase / 0.42);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.0, 0.35, t),
            shoulder_yaw: lerp(0.0, 0.55, t),
            shoulder_roll: lerp(0.0, 0.20, t),
            forearm_pitch: lerp(0.0, 0.30, t),
            torso_twist: lerp(0.0, 0.28, t),
        };
    }
    // Strike: sweep the whole arm across the body to the left (yaw crosses from
    // + to strongly -), staying near level (shallow pitch), driving through
    // contact at 0.60. The elbow extends into the cut and the torso unwinds.
    if phase <= 0.60 {
        let t = ease_in((phase - 0.42) / 0.18);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.35, -0.10, t),
            shoulder_yaw: lerp(0.55, -0.70, t),
            shoulder_roll: lerp(0.20, -0.10, t),
            forearm_pitch: lerp(0.30, -0.40, t),
            torso_twist: lerp(0.28, -0.26, t),
        };
    }
    // End of the arc: the blade holds out to the left, settling before recovery.
    if phase <= 0.74 {
        let t = smoothstep((phase - 0.60) / 0.14);
        return RemoteArmPose {
            shoulder_pitch: lerp(-0.10, -0.06, t),
            shoulder_yaw: lerp(-0.70, -0.58, t),
            shoulder_roll: lerp(-0.10, -0.06, t),
            forearm_pitch: lerp(-0.40, -0.28, t),
            torso_twist: lerp(-0.26, -0.16, t),
        };
    }
    // Recover to the carry rest.
    let t = smoothstep((phase - 0.74) / 0.26);
    RemoteArmPose {
        shoulder_pitch: lerp(-0.06, 0.0, t),
        shoulder_yaw: lerp(-0.58, 0.0, t),
        shoulder_roll: lerp(-0.06, 0.0, t),
        forearm_pitch: lerp(-0.28, 0.0, t),
        torso_twist: lerp(-0.16, 0.0, t),
    }
}

/// Bow: a draw-to-anchor, a brief full-draw hold, then a forward loose flick, off
/// the carry pose. The "swing" phase stands in for the draw window when a peer's
/// ranged action surfaces one. The draw hand (the swinging right arm here) folds
/// the elbow back to the anchor (near the cheek) while the shoulder lifts toward
/// eye line, holds, then flicks the arm forward as the string looses. Reads as a
/// steady pull rather than a chop: shoulder pitch rises modestly and the elbow
/// does the drawing, with no sideways sweep.
fn bow_arm_pose(phase: f32) -> RemoteArmPose {
    // Draw: raise the bow-arm shoulder toward eye line and fold the elbow back to
    // the anchor (the string hand pulling to the cheek), decelerating into the
    // full-draw hold.
    if phase <= 0.55 {
        let t = ease_out(phase / 0.55);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.0, 0.55, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(0.0, 0.95, t),
            torso_twist: lerp(0.0, 0.10, t),
        };
    }
    // Full-draw hold: a brief anchored beat at maximum tension.
    if phase <= 0.68 {
        let t = smoothstep((phase - 0.55) / 0.13);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.55, 0.52, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(0.95, 0.90, t),
            torso_twist: lerp(0.10, 0.10, t),
        };
    }
    // Loose: the string releases and the draw hand flicks forward (the elbow
    // snaps open past the anchor), then the arm settles back to the carry rest.
    let t = ease_in((phase - 0.68) / 0.32);
    RemoteArmPose {
        shoulder_pitch: lerp(0.52, 0.0, t),
        shoulder_yaw: 0.0,
        shoulder_roll: 0.0,
        forearm_pitch: lerp(0.90, 0.0, t),
        torso_twist: lerp(0.10, 0.0, t),
    }
}

/// Crossbow: a shoulder brace with a recoil kick on the shot, then a dip-and-crank
/// reload, off the carry pose. The "swing" phase reads the whole fire-then-reload
/// beat when one surfaces: a sharp back-and-up jolt right on the shot (early
/// phase), then the stock dips and the crank hand works the windlass (the elbow
/// pumping) through the middle, returning to the braced ready by the end. No
/// sideways sweep: it is a level brace, not an arc.
fn crossbow_arm_pose(phase: f32) -> RemoteArmPose {
    // Recoil: a sharp jolt back and up right on the shot, decaying fast.
    if phase <= 0.20 {
        let t = ease_out(phase / 0.20);
        return RemoteArmPose {
            shoulder_pitch: lerp(0.0, 0.30, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: lerp(0.0, -0.25, t),
            torso_twist: lerp(0.0, -0.08, t),
        };
    }
    // Reload dip + crank: the stock dips (shoulder pitches down below rest) and the
    // crank hand pumps the windlass (the elbow folds and extends), hardest at the
    // middle of the cycle.
    if phase <= 0.75 {
        let t = smoothstep((phase - 0.20) / 0.55);
        let crank = (t * std::f32::consts::PI).sin();
        return RemoteArmPose {
            shoulder_pitch: lerp(0.30, -0.35, t),
            shoulder_yaw: 0.0,
            shoulder_roll: 0.0,
            forearm_pitch: 0.70 * crank,
            torso_twist: -0.10 * crank,
        };
    }
    // Return to the braced ready pose.
    let t = smoothstep((phase - 0.75) / 0.25);
    RemoteArmPose {
        shoulder_pitch: lerp(-0.35, 0.0, t),
        shoulder_yaw: 0.0,
        shoulder_roll: 0.0,
        forearm_pitch: 0.0,
        torso_twist: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_at_phase_zero_and_one() {
        // Deltas are zero at the endpoints, so a swing starts and ends exactly
        // at the arm's rest (carry) pose with no jump. Covers every archetype,
        // including the four weapon models.
        for model in [
            ItemModel::Hatchet,
            ItemModel::Pickaxe,
            ItemModel::Bag,
            ItemModel::Club,
            ItemModel::Spear,
            ItemModel::Sword,
        ] {
            let start = remote_swing_arm_pose(model, 0.0);
            let end = remote_swing_arm_pose(model, 1.0);
            assert_eq!(start, RemoteArmPose::default(), "starts at rest");
            assert!(end.shoulder_pitch.abs() < 1e-5, "ends at rest (shoulder)");
            assert!(end.forearm_pitch.abs() < 1e-5, "ends at rest (elbow)");
        }
    }

    #[test]
    fn hatchet_winds_up_then_strikes_down_through_impact() {
        let apex = remote_swing_arm_pose(ItemModel::Hatchet, 0.40);
        let impact = remote_swing_arm_pose(ItemModel::Hatchet, 0.58);
        // Wind-up raises the arm above the carry rest and folds the elbow.
        assert!(apex.shoulder_pitch > 1.0, "arm raised on wind-up");
        assert!(apex.forearm_pitch > 0.3, "elbow folds tighter on wind-up");
        // The strike drives the arm below the carry rest (negative delta), sweeps
        // across (yaw negative), and extends the elbow (negative delta).
        assert!(impact.shoulder_pitch < 0.0, "arm drives down past rest");
        assert!(impact.shoulder_yaw < apex.shoulder_yaw, "sweeps across");
        assert!(
            impact.forearm_pitch < 0.0,
            "elbow extends through the strike"
        );
    }

    #[test]
    fn pickaxe_is_vertical_and_raises_highest() {
        let apex = remote_swing_arm_pose(ItemModel::Pickaxe, 0.60);
        let impact = remote_swing_arm_pose(ItemModel::Pickaxe, 0.68);
        // Highest raise of the three, and stays vertical (no sideways yaw).
        assert!(apex.shoulder_pitch > 1.5, "pickaxe raises highest");
        assert!(apex.shoulder_yaw.abs() < 1e-5, "stays vertical");
        assert!(
            impact.shoulder_pitch < apex.shoulder_pitch - 1.5,
            "smashes down"
        );
    }

    #[test]
    fn club_reuses_its_nearest_existing_arc() {
        // Club reads as the hatchet chop. The spear and sword are their OWN
        // arcs now (asserted separately below).
        for phase in [0.0, 0.2, 0.4, 0.58, 0.68, 0.8] {
            assert_eq!(
                remote_swing_arm_pose(ItemModel::Club, phase),
                remote_swing_arm_pose(ItemModel::Hatchet, phase),
                "club reuses the hatchet arc"
            );
        }
    }

    #[test]
    fn sword_remote_pose_is_a_horizontal_slash_not_a_chop() {
        // The sword now has its own wide horizontal slash, distinct from the
        // hatchet's overhead diagonal: the sweep is dominated by shoulder YAW
        // crossing the body (positive on the wind-up to strongly negative through
        // contact), with the shoulder PITCH staying shallow (a level cut, not an
        // overhead smash). Contact matches SWORD_IMPACT_FRACTION (0.60).
        let cocked = remote_swing_arm_pose(ItemModel::Sword, 0.42);
        let impact = remote_swing_arm_pose(ItemModel::Sword, 0.60);

        // Wind-up cocks the arm out to the right (positive yaw).
        assert!(cocked.shoulder_yaw > 0.3, "cocks out to the right");
        // The strike sweeps hard across to the left (yaw drives strongly negative).
        assert!(
            impact.shoulder_yaw < cocked.shoulder_yaw - 1.0,
            "the slash sweeps across the body"
        );
        // It stays near level: the pitch travel is small compared with the yaw
        // sweep, unlike the hatchet's big overhead pitch.
        assert!(
            impact.shoulder_pitch.abs() < 0.3,
            "the slash stays roughly level (horizontal), not an overhead chop"
        );
        // And it is genuinely distinct from the hatchet chop it used to reuse.
        assert_ne!(
            remote_swing_arm_pose(ItemModel::Sword, 0.5),
            remote_swing_arm_pose(ItemModel::Hatchet, 0.5),
            "the sword slash is no longer the hatchet chop"
        );
    }

    #[test]
    fn bow_and_crossbow_remote_poses_rest_at_the_endpoints() {
        // The ranged arm poses are no longer the placeholder jab: each is its own
        // draw / fire cycle. They still start and end at the carry rest (all deltas
        // zero at phase 0 and 1) so a surfaced ranged action blends cleanly.
        for model in [ItemModel::Bow, ItemModel::Crossbow] {
            let start = remote_swing_arm_pose(model, 0.0);
            let end = remote_swing_arm_pose(model, 1.0);
            assert_eq!(start, RemoteArmPose::default(), "{model:?} starts at rest");
            assert!(
                end.shoulder_pitch.abs() < 1e-5 && end.forearm_pitch.abs() < 1e-5,
                "{model:?} ends at rest"
            );
            // And each is distinct from the old placeholder jab (the bag pose).
            assert_ne!(
                remote_swing_arm_pose(model, 0.5),
                remote_swing_arm_pose(ItemModel::Bag, 0.5),
                "{model:?} is no longer the placeholder jab"
            );
        }
    }

    #[test]
    fn spear_remote_pose_is_a_thrust_not_an_arc() {
        // Mirror of the first-person `spear_swing_pose_is_a_forward_thrust_not_an_arc`
        // test, adapted to the rotation-only body rig: a thrust is dominated by
        // elbow EXTENSION (forearm_pitch driving strongly negative through
        // contact) with the shoulder staying near-neutral and no sideways sweep,
        // the opposite of the hatchet's chop (big shoulder pitch + yaw sweep).
        let ready = remote_swing_arm_pose(ItemModel::Spear, 0.0);
        let chamber = remote_swing_arm_pose(ItemModel::Spear, 0.38);
        // Contact matches SPEAR_IMPACT_FRACTION (0.55).
        let impact = remote_swing_arm_pose(ItemModel::Spear, 0.55);

        // Wind-up coils the elbow (draws the spear back), then the thrust snaps it
        // straight past rest into full extension (negative flex).
        assert!(
            chamber.forearm_pitch > ready.forearm_pitch + 0.3,
            "the elbow folds to chamber the thrust"
        );
        assert!(
            impact.forearm_pitch < chamber.forearm_pitch - 1.0,
            "the elbow extends hard through the thrust"
        );
        assert!(
            impact.forearm_pitch < -0.4,
            "contact is at full forward extension"
        );

        // A thrust does not sweep: the shoulder yaw/roll stay flat across the whole
        // motion, unlike the hatchet chop which sweeps the shoulder across the body.
        for phase in [0.0, 0.2, 0.38, 0.55, 0.7, 0.9] {
            let pose = remote_swing_arm_pose(ItemModel::Spear, phase);
            assert_eq!(pose.shoulder_yaw, 0.0, "spear does not sweep sideways");
            assert_eq!(pose.shoulder_roll, 0.0, "spear does not roll the shoulder");
        }

        // And it is genuinely distinct from the short bag jab it used to reuse.
        assert_ne!(
            remote_swing_arm_pose(ItemModel::Spear, 0.5),
            remote_swing_arm_pose(ItemModel::Bag, 0.5),
            "the spear thrust is no longer the bag jab"
        );
    }
}
