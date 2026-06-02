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

// Hatchet: a heavy, committed chop. The head winds up high over the shoulder
// and *hangs* at the apex (ease-out anticipation), the wind-up takes the
// larger share of the swing, so the load reads as deliberate weight rather
// than a quick jab. It then accelerates down and forward *through* the target
// (the strike eases in, so the head is moving hardest at the moment of
// contact). That long-load → fast-strike contrast is what sells the mass; an
// even, eased-out arc reads as a limp wrist-flick. After contact the head
// bites and dwells at the bottom (the blade buried in the cut), then the
// weight is hauled back up to rest more slowly than it came down. Roll is held
// near rest so the handle stays aligned with the motion. Impact lands at phase
// 0.58, keep `AXE_IMPACT_FRACTION` in `gather.rs` matched to this so the chop
// sound and camera kick fire exactly as the head bottoms out.
pub(crate) fn hatchet_swing_pose(phase: f32) -> ToolSwingPose {
    if phase <= 0.40 {
        // Wind-up: cock the head high and back over the shoulder, decelerating
        // into a hang at the apex (ease-out). The hang at the top is the
        // anticipation beat that loads the swing with weight, and it owns the
        // first 40% of the swing so the chop feels deliberate. Yaw winds
        // inward and the head pulls back toward the shoulder.
        let t = ease_out(phase / 0.40);
        return ToolSwingPose {
            pitch: lerp(-0.32, 0.82, t),
            yaw: lerp(0.22, -0.14, t),
            roll: lerp(0.08, 0.06, t),
            forward: lerp(0.0, -0.18, t),
            right: lerp(0.0, 0.07, t),
            up: lerp(0.0, 0.28, t),
        };
    }

    if phase <= 0.58 {
        // Strike: accelerate down and forward through the target (ease-in) so
        // the head is travelling fastest exactly at impact. Forward drives
        // past rest, full bodyweight committed into the cut, and a diagonal
        // yaw sweep finishes the chop across the body.
        let t = ease_in((phase - 0.40) / 0.18);
        return ToolSwingPose {
            pitch: lerp(0.82, -1.22, t),
            yaw: lerp(-0.14, 0.24, t),
            roll: lerp(0.06, 0.08, t),
            forward: lerp(-0.18, 0.30, t),
            right: lerp(0.07, -0.08, t),
            up: lerp(0.28, -0.24, t),
        };
    }

    if phase <= 0.72 {
        // Bite + dwell: the head holds buried at the bottom of the arc with a
        // small settle back off the contact. The brief hold sells the mass of
        // the strike before the recovery lifts it out.
        let t = smoothstep((phase - 0.58) / 0.14);
        return ToolSwingPose {
            pitch: lerp(-1.22, -1.02, t),
            yaw: lerp(0.24, 0.24, t),
            roll: lerp(0.08, 0.08, t),
            forward: lerp(0.30, 0.18, t),
            right: lerp(-0.08, -0.03, t),
            up: lerp(-0.24, -0.16, t),
        };
    }

    // Recovery: haul the heavy head back up to rest. Slower than the strike,
    // you don't snap a buried axe straight back out.
    let t = smoothstep((phase - 0.72) / 0.28);
    ToolSwingPose {
        pitch: lerp(-1.02, -0.32, t),
        yaw: lerp(0.24, 0.22, t),
        roll: lerp(0.08, 0.08, t),
        forward: lerp(0.18, 0.0, t),
        right: lerp(-0.03, 0.0, t),
        up: lerp(-0.16, 0.0, t),
    }
}

// Pickaxe: a heavy two-step swing, deliberate draw-up that loads the head
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
        // Strike, short, snap-fast smash that drives back through the
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
        // Dwell at the bottom, pick buried in the stone, slight settle.
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

    // Long, smooth drag back to rest, the heavy head doesn't snap up.
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

/// Accelerating ease, slowest at the start, fastest at the end. Used for the
/// strike of a heavy swing so the tool is travelling hardest at the moment of
/// impact, which reads as force rather than a soft, evenly-paced arc.
pub(crate) fn ease_in(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t
}

/// Decelerating ease, fastest at the start, settling at the end. Used for the
/// wind-up so the head snaps back and then hangs at the apex; that hang is the
/// anticipation beat that gives a swing its weight.
pub(crate) fn ease_out(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

pub(crate) fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hatchet_swing_pose_drives_a_committed_chop() {
        let ready = hatchet_swing_pose(0.0);
        let apex = hatchet_swing_pose(0.40);
        let impact = hatchet_swing_pose(0.58);
        let dwell = hatchet_swing_pose(0.66);

        // Wind-up loads the head high and back over the shoulder, a real
        // cock-back, not a wrist flick: it lifts well clear of rest and pulls
        // back toward the shoulder rather than reaching forward.
        assert!(apex.pitch > ready.pitch + 0.8);
        assert!(apex.up > ready.up + 0.15);
        assert!(apex.forward < ready.forward - 0.10);

        // Strike drives the head deep, forward through the target, and down
        // with a diagonal yaw finish, bodyweight committed, not flicked.
        assert!(impact.pitch < apex.pitch - 1.4);
        assert!(impact.forward > apex.forward + 0.30);
        assert!(impact.up < ready.up - 0.15);
        assert!(impact.yaw > apex.yaw + 0.20);

        // The head bites and holds at the bottom rather than snapping straight
        // back, just after contact it still sits below rest and forward.
        assert!(dwell.up < ready.up - 0.10);
        assert!(dwell.forward > 0.10);

        // Handle stays aligned with the swing, roll never drifts far from
        // rest, so the haft isn't spinning around its own axis.
        assert!((apex.roll - ready.roll).abs() < 0.05);
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
        // but is still well below the apex, the load still feels weighty.
        assert!(mid_windup.pitch > ready.pitch + 0.50);
        assert!(mid_windup.pitch < ready.pitch + 1.20);
        assert!(mid_windup.up > ready.up + 0.10);
        assert!(mid_windup.up < ready.up + 0.30);

        // Apex lifts the head high and well back.
        assert!(apex.up > ready.up + 0.25);
        assert!(apex.pitch > ready.pitch + 1.4);

        // The wind-up loads up and to the right, the swing reads as a
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
