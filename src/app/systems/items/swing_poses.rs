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

// Hatchet: a two-handed diagonal chop that draws up and out to the RIGHT and
// swings in across the body. The head winds up high toward the right shoulder
// and *hangs* at the apex (ease-out anticipation), the wind-up owns the larger
// share of the swing so the load reads as deliberate weight rather than a quick
// jab. It then accelerates down and across to the lower-left, driving forward
// *through* the target (the strike eases in, so the head is moving hardest at
// the moment of contact). That long-load → fast diagonal strike is what sells
// the mass; an even, eased-out arc reads as a limp wrist-flick. After contact
// the blade bites and dwells low, then the weight is hauled back up to rest
// more slowly than it came down. Both hands are baked into the mesh, so they
// ride the haft through the whole arc. Impact lands at phase 0.58, keep
// `AXE_IMPACT_FRACTION` in `gather.rs` matched to this so the chop sound and
// camera kick fire exactly as the head crosses through contact.
pub(crate) fn hatchet_swing_pose(phase: f32) -> ToolSwingPose {
    if phase <= 0.40 {
        // Wind-up: draw the head up and out to the right, decelerating into a
        // hang at the apex (ease-out). It lifts, slides right, rolls the blade
        // over toward the shoulder, and pulls back, the anticipation beat that
        // loads a diagonal cut. Negative yaw cocks the head to the right (same
        // sign convention as the pickaxe draw-up).
        let t = ease_out(phase / 0.40);
        return ToolSwingPose {
            pitch: lerp(-0.30, 0.70, t),
            yaw: lerp(0.10, -0.30, t),
            roll: lerp(0.08, 0.40, t),
            forward: lerp(0.0, -0.16, t),
            right: lerp(0.02, 0.18, t),
            up: lerp(0.0, 0.30, t),
        };
    }

    if phase <= 0.58 {
        // Strike: accelerate down and sweep across the body to the lower-left
        // (ease-in) so the head is travelling fastest exactly at impact. Right
        // crosses from + to -, yaw carries the cut across, and forward drives
        // past rest with full committed bodyweight.
        let t = ease_in((phase - 0.40) / 0.18);
        return ToolSwingPose {
            pitch: lerp(0.70, -1.12, t),
            yaw: lerp(-0.30, 0.32, t),
            roll: lerp(0.40, -0.08, t),
            forward: lerp(-0.16, 0.30, t),
            right: lerp(0.18, -0.16, t),
            up: lerp(0.30, -0.22, t),
        };
    }

    if phase <= 0.72 {
        // Bite + dwell: the head holds buried low at the end of the cut with a
        // small settle back off the contact. The brief hold sells the mass of
        // the strike before the recovery lifts it out.
        let t = smoothstep((phase - 0.58) / 0.14);
        return ToolSwingPose {
            pitch: lerp(-1.12, -0.94, t),
            yaw: lerp(0.32, 0.30, t),
            roll: lerp(-0.08, -0.04, t),
            forward: lerp(0.30, 0.18, t),
            right: lerp(-0.16, -0.07, t),
            up: lerp(-0.22, -0.14, t),
        };
    }

    // Recovery: haul the heavy head back up to rest. Slower than the strike,
    // you don't snap a buried axe straight back out.
    let t = smoothstep((phase - 0.72) / 0.28);
    ToolSwingPose {
        pitch: lerp(-0.94, -0.30, t),
        yaw: lerp(0.30, 0.10, t),
        roll: lerp(-0.04, 0.08, t),
        forward: lerp(0.18, 0.0, t),
        right: lerp(-0.07, 0.02, t),
        up: lerp(-0.14, 0.0, t),
    }
}

// Pickaxe: a heavy two-handed overhead swing, a deliberate near-vertical draw
// up over the head, an explosive downward smash that drives back through the
// centre, a long dwell at the bottom (the pick buried in stone), then a slow
// drag back to rest. Kept close to vertical (only a slight rightward lean) so
// it reads as a straight overhead pick rather than a side chop, the contrast
// with the hatchet's diagonal "in from the right" cut. Impact lands at phase
// 0.68. The wind-up uses a smoothstep curve so the head moves immediately
// rather than crawling off, but still decelerates into the apex for a load.
pub(crate) fn pickaxe_swing_pose(phase: f32) -> ToolSwingPose {
    if phase <= 0.60 {
        // Draw-up. Smoothstep gives an immediate-but-controlled rise, lifting
        // the head high and nearly straight overhead with only a slight lean,
        // so the smash reads as a vertical pick rather than a diagonal chop.
        let t = smoothstep((phase / 0.60).clamp(0.0, 1.0));
        return ToolSwingPose {
            pitch: lerp(-0.32, 1.18, t),
            yaw: lerp(0.10, -0.06, t),
            roll: lerp(0.04, -0.06, t),
            forward: lerp(0.0, -0.20, t),
            right: lerp(0.0, 0.05, t),
            up: lerp(0.0, 0.36, t),
        };
    }

    if phase <= 0.68 {
        // Strike, short, snap-fast smash that drives straight back down
        // through the centre.
        let t = smoothstep((phase - 0.60) / 0.08);
        return ToolSwingPose {
            pitch: lerp(1.18, -1.90, t),
            yaw: lerp(-0.06, 0.03, t),
            roll: lerp(-0.06, 0.04, t),
            forward: lerp(-0.20, 0.38, t),
            right: lerp(0.05, -0.02, t),
            up: lerp(0.36, -0.32, t),
        };
    }

    if phase <= 0.85 {
        // Dwell at the bottom, pick buried in the stone, slight settle.
        let t = smoothstep((phase - 0.68) / 0.17);
        return ToolSwingPose {
            pitch: lerp(-1.90, -1.72, t),
            yaw: lerp(0.03, 0.05, t),
            roll: lerp(0.04, 0.02, t),
            forward: lerp(0.38, 0.28, t),
            right: lerp(-0.02, -0.01, t),
            up: lerp(-0.32, -0.26, t),
        };
    }

    // Long, smooth drag back to rest, the heavy head doesn't snap up.
    let t = smoothstep((phase - 0.85) / 0.15);
    ToolSwingPose {
        pitch: lerp(-1.72, -0.32, t),
        yaw: lerp(0.05, 0.10, t),
        roll: lerp(0.02, 0.04, t),
        forward: lerp(0.28, 0.0, t),
        right: lerp(-0.01, 0.0, t),
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
    fn hatchet_swing_pose_draws_up_right_and_chops_across() {
        let ready = hatchet_swing_pose(0.0);
        let apex = hatchet_swing_pose(0.40);
        let impact = hatchet_swing_pose(0.58);
        let dwell = hatchet_swing_pose(0.66);

        // Wind-up draws the head up and out to the RIGHT, cocked toward the
        // shoulder rather than straight overhead: it lifts, slides right, and
        // pulls back instead of reaching forward.
        assert!(apex.pitch > ready.pitch + 0.7);
        assert!(apex.up > ready.up + 0.15);
        assert!(apex.right > ready.right + 0.10, "draws out to the right");
        assert!(apex.forward < ready.forward - 0.08);

        // Strike chops down and sweeps across the body to the lower-left,
        // driving forward through the target, the diagonal "in from the
        // right" cut.
        assert!(impact.pitch < apex.pitch - 1.4);
        assert!(
            impact.right < apex.right - 0.20,
            "sweeps left across the body"
        );
        assert!(impact.yaw > apex.yaw + 0.40, "yaw carries the cut across");
        assert!(impact.forward > apex.forward + 0.30);
        assert!(impact.up < ready.up - 0.15);

        // The blade bites and dwells low and forward rather than snapping back.
        assert!(dwell.up < ready.up - 0.10);
        assert!(dwell.forward > 0.10);
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

        // The wind-up stays close to vertical, only a slight rightward lean,
        // so it reads as a straight overhead pick rather than a side chop.
        assert!(
            apex.right >= 0.0 && apex.right < 0.10,
            "stays near vertical"
        );
        assert!(
            apex.roll <= ready.roll,
            "head stays roughly upright, not cocked hard to the side"
        );

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
