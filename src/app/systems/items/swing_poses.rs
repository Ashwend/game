use std::f32::consts::PI;

/// Pose offsets for the held-tool animation. `pitch`/`yaw`/`roll` are
/// Euler rotations; `forward`/`right`/`up` are additive view-space offsets.
/// Kept in its own module because the curves are large and tweak-prone, and
/// you don't usually want to scroll past them to touch the held-item system.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolSwingPose {
    pub(crate) pitch: f32,
    pub(crate) yaw: f32,
    pub(crate) roll: f32,
    pub(crate) forward: f32,
    pub(crate) right: f32,
    pub(crate) up: f32,
}

pub(crate) fn bag_idle_pose(phase: f32) -> ToolSwingPose {
    let swing = (phase * PI).sin();
    let windup = (0.5 - phase).max(0.0) * 0.28;
    ToolSwingPose {
        pitch: -0.35 + windup - swing * 0.9,
        yaw: 0.25 + swing * 0.12,
        roll: 0.18 - swing * 0.18,
        forward: swing * 0.06,
        right: 0.0,
        up: -swing * 0.05,
    }
}

// Hatchet: a quick, pitch-driven chop. The head lifts up and back over the
// shoulder (no handle twist), then snaps forward and down with a slight
// rightward kick for a natural diagonal finish. The pitch arc is intentionally
// modest — a wrist-flick chop rather than a full-body swing — and roll is
// held nearly constant so the handle stays aligned with the motion. Impact
// lands at phase 0.50.
pub(crate) fn hatchet_swing_pose(phase: f32) -> ToolSwingPose {
    if phase <= 0.32 {
        // Wind-up: lift the head up and tilt it back. Yaw eases toward
        // centre — no sideways throw, no handle roll.
        let t = smoothstep(phase / 0.32);
        return ToolSwingPose {
            pitch: lerp(-0.32, 0.42, t),
            yaw: lerp(0.22, -0.04, t),
            roll: lerp(0.08, 0.06, t),
            forward: lerp(0.0, -0.08, t),
            right: lerp(0.0, 0.02, t),
            up: lerp(0.0, 0.14, t),
        };
    }

    if phase <= 0.50 {
        // Strike: snap forward and down. Small yaw sweep gives the chop a
        // slight diagonal finish without twisting the handle.
        let t = smoothstep((phase - 0.32) / 0.18);
        return ToolSwingPose {
            pitch: lerp(0.42, -0.78, t),
            yaw: lerp(-0.04, 0.18, t),
            roll: lerp(0.06, 0.08, t),
            forward: lerp(-0.08, 0.16, t),
            right: lerp(0.02, -0.05, t),
            up: lerp(0.14, -0.12, t),
        };
    }

    if phase <= 0.62 {
        // Brief follow-through — head holds at the bottom of the arc.
        let t = smoothstep((phase - 0.50) / 0.12);
        return ToolSwingPose {
            pitch: lerp(-0.78, -0.66, t),
            yaw: lerp(0.18, 0.20, t),
            roll: lerp(0.08, 0.08, t),
            forward: lerp(0.16, 0.12, t),
            right: lerp(-0.05, -0.03, t),
            up: lerp(-0.12, -0.08, t),
        };
    }

    // Smooth drag back to rest.
    let t = smoothstep((phase - 0.62) / 0.38);
    ToolSwingPose {
        pitch: lerp(-0.66, -0.32, t),
        yaw: lerp(0.20, 0.22, t),
        roll: lerp(0.08, 0.08, t),
        forward: lerp(0.12, 0.0, t),
        right: lerp(-0.03, 0.0, t),
        up: lerp(-0.08, 0.0, t),
    }
}

// Pickaxe: a heavy two-step swing — deliberate draw-up that loads the head
// up over the right shoulder, an explosive downward smash that drives back
// through the centre, a long dwell at the bottom (the pick buried in stone),
// then a slow drag back to rest. Impact lands at phase 0.68. The wind-up
// uses a smoothstep curve so the head moves immediately rather than crawling
// off, but still decelerates into the apex for a satisfying load.
pub(crate) fn pickaxe_swing_pose(phase: f32) -> ToolSwingPose {
    if phase <= 0.60 {
        // Draw-up. Smoothstep gives an immediate-but-controlled rise, and the
        // head loads up and to the right rather than purely overhead, so the
        // motion reads as multi-axis instead of a single hinge.
        let t = smoothstep((phase / 0.60).clamp(0.0, 1.0));
        return ToolSwingPose {
            pitch: lerp(-0.32, 1.18, t),
            yaw: lerp(0.10, -0.18, t),
            roll: lerp(0.04, -0.18, t),
            forward: lerp(0.0, -0.20, t),
            right: lerp(0.0, 0.13, t),
            up: lerp(0.0, 0.34, t),
        };
    }

    if phase <= 0.68 {
        // Strike — short, snap-fast smash that drives back through the
        // centre, neutralising the diagonal load.
        let t = smoothstep((phase - 0.60) / 0.08);
        return ToolSwingPose {
            pitch: lerp(1.18, -1.90, t),
            yaw: lerp(-0.18, 0.05, t),
            roll: lerp(-0.18, 0.06, t),
            forward: lerp(-0.20, 0.38, t),
            right: lerp(0.13, -0.04, t),
            up: lerp(0.34, -0.32, t),
        };
    }

    if phase <= 0.85 {
        // Dwell at the bottom — pick buried in the stone, slight settle.
        let t = smoothstep((phase - 0.68) / 0.17);
        return ToolSwingPose {
            pitch: lerp(-1.90, -1.72, t),
            yaw: lerp(0.05, 0.06, t),
            roll: lerp(0.06, 0.02, t),
            forward: lerp(0.38, 0.28, t),
            right: lerp(-0.04, -0.02, t),
            up: lerp(-0.32, -0.26, t),
        };
    }

    // Long, smooth drag back to rest — the heavy head doesn't snap up.
    let t = smoothstep((phase - 0.85) / 0.15);
    ToolSwingPose {
        pitch: lerp(-1.72, -0.32, t),
        yaw: lerp(0.06, 0.10, t),
        roll: lerp(0.02, 0.04, t),
        forward: lerp(0.28, 0.0, t),
        right: lerp(-0.02, 0.0, t),
        up: lerp(-0.26, 0.0, t),
    }
}

pub(crate) fn smoothstep(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hatchet_swing_pose_chops_with_a_stable_handle() {
        let ready = hatchet_swing_pose(0.0);
        let windup = hatchet_swing_pose(0.32);
        let impact = hatchet_swing_pose(0.50);

        // Wind-up loads the head up and back, not sideways. The arc stays
        // modest — a wrist-flick chop rather than a full overhead swing.
        assert!(windup.pitch > ready.pitch + 0.6);
        assert!(windup.up > ready.up + 0.10);

        // Strike drops the head forward and down with a small diagonal yaw.
        assert!(impact.pitch < windup.pitch - 1.0);
        assert!(impact.forward > windup.forward + 0.20);
        assert!(impact.up < windup.up - 0.20);
        assert!(impact.yaw > windup.yaw + 0.10);

        // Handle stays aligned with the swing — roll never drifts far from
        // rest, so the haft isn't spinning around its own axis.
        assert!((windup.roll - ready.roll).abs() < 0.05);
        assert!((impact.roll - ready.roll).abs() < 0.05);
    }

    #[test]
    fn pickaxe_swing_pose_drives_a_heavy_overhead_strike() {
        let ready = pickaxe_swing_pose(0.0);
        let mid_windup = pickaxe_swing_pose(0.30);
        let apex = pickaxe_swing_pose(0.60);
        let impact = pickaxe_swing_pose(0.68);
        let dwell = pickaxe_swing_pose(0.78);

        // Mid-windup has clearly moved (the head doesn't crawl off the start),
        // but is still well below the apex — the load still feels weighty.
        assert!(mid_windup.pitch > ready.pitch + 0.50);
        assert!(mid_windup.pitch < ready.pitch + 1.20);
        assert!(mid_windup.up > ready.up + 0.10);
        assert!(mid_windup.up < ready.up + 0.30);

        // Apex lifts the head high and well back.
        assert!(apex.up > ready.up + 0.25);
        assert!(apex.pitch > ready.pitch + 1.4);

        // The wind-up loads up and to the right — the swing reads as a
        // multi-axis motion rather than a single overhead hinge.
        assert!(apex.right > 0.08);
        assert!(apex.roll < ready.roll - 0.10);

        // Strike drives back through the centre and slams forward+down.
        assert!(impact.pitch < apex.pitch - 2.8);
        assert!(impact.up < ready.up - 0.20);
        assert!(impact.forward > apex.forward + 0.45);
        assert!(impact.right.abs() < 0.08);
        assert!((impact.roll - ready.roll).abs() < 0.05);

        // Bottom dwell holds near the impact pose rather than snapping back.
        assert!(dwell.up < ready.up - 0.15);
        assert!(dwell.forward > 0.20);
    }
}
