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

use crate::items::ToolKind;

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

/// Map a tool + swing phase to the right-arm delta pose. Phases match the
/// first-person impact fractions (hatchet/hammer 0.58, pickaxe 0.68, hands
/// 0.55) so the body's strike crosses through contact at the same instant the
/// swinger feels it.
pub(crate) fn remote_swing_arm_pose(tool: ToolKind, phase: f32) -> RemoteArmPose {
    let phase = phase.clamp(0.0, 1.0);
    match tool {
        ToolKind::Pickaxe => pickaxe_arm_pose(phase),
        ToolKind::Axe | ToolKind::Hammer => hatchet_arm_pose(phase),
        ToolKind::Hands => hands_arm_pose(phase),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_at_phase_zero_and_one() {
        // Deltas are zero at the endpoints, so a swing starts and ends exactly
        // at the arm's rest (carry) pose with no jump.
        for tool in [ToolKind::Axe, ToolKind::Pickaxe, ToolKind::Hands] {
            let start = remote_swing_arm_pose(tool, 0.0);
            let end = remote_swing_arm_pose(tool, 1.0);
            assert_eq!(start, RemoteArmPose::default(), "starts at rest");
            assert!(end.shoulder_pitch.abs() < 1e-5, "ends at rest (shoulder)");
            assert!(end.forearm_pitch.abs() < 1e-5, "ends at rest (elbow)");
        }
    }

    #[test]
    fn hatchet_winds_up_then_strikes_down_through_impact() {
        let apex = remote_swing_arm_pose(ToolKind::Axe, 0.40);
        let impact = remote_swing_arm_pose(ToolKind::Axe, 0.58);
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
        let apex = remote_swing_arm_pose(ToolKind::Pickaxe, 0.60);
        let impact = remote_swing_arm_pose(ToolKind::Pickaxe, 0.68);
        // Highest raise of the three, and stays vertical (no sideways yaw).
        assert!(apex.shoulder_pitch > 1.5, "pickaxe raises highest");
        assert!(apex.shoulder_yaw.abs() < 1e-5, "stays vertical");
        assert!(
            impact.shoulder_pitch < apex.shoulder_pitch - 1.5,
            "smashes down"
        );
    }

    #[test]
    fn hammer_uses_the_hatchet_curve() {
        for phase in [0.0, 0.2, 0.4, 0.58, 0.8] {
            assert_eq!(
                remote_swing_arm_pose(ToolKind::Hammer, phase),
                remote_swing_arm_pose(ToolKind::Axe, phase),
            );
        }
    }
}
