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

/// Bandage: the hold-to-use wrap. `charge` 0 is the carry rest, 1 is the wrap
/// fully going on; `settle` eases back to the carry after the use ends (whether
/// it completed or was abandoned), blending from `ended_at`, the charge it had
/// reached.
///
/// The read has to be legible in first person from the roll alone, so the motion
/// is: lift the roll up and across to the *left* of the view (toward the off-hand
/// the player is notionally binding), rolling it over so the coil face turns
/// toward the camera and you can see it is a bandage, and pull it in close. The
/// tail unrolling out of it is the other half of the story and is animated
/// separately, per-piece, in `bandage_tail_transform`.
///
/// A strain tremble ramps in near full, the same idiom the bow draw and the bomb
/// wind-up use, so the last stretch of the charge feels like effort and the player
/// can feel how close they are without reading the HUD arc.
pub(crate) fn bandage_use_pose(
    charge: f32,
    settle: f32,
    ended_at: f32,
    time_seconds: f32,
) -> ToolSwingPose {
    // While a use is live, `charge` drives it. Once it ends, blend from the charge
    // it reached back to the carry over the settle. `settle` is 1 (fully at rest)
    // whenever no use is running and none recently ended, so the idle carry is the
    // natural resting value of this whole function.
    let live = charge.clamp(0.0, 1.0);
    let s = settle.clamp(0.0, 1.0);
    let effective = if live > 0.0 {
        live
    } else {
        // Ease-out on the way back so the roll drops away softly.
        ended_at.clamp(0.0, 1.0) * (1.0 - smooth(s))
    };

    // Ease-in-out: the lift starts unhurried, the wrap presses home in the middle,
    // and the last stretch holds steady against the wound.
    let e = smooth(effective);

    // Strain tremble in the last 45% of the charge, ramping quadratically. Only
    // while actually charging: a settling bandage should not shake.
    let strain = ((live - 0.55) / 0.45).clamp(0.0, 1.0);
    let tremble = 0.014 * strain * strain;

    ToolSwingPose {
        // Tip the roll's face up toward the camera so the coil reads.
        pitch: -0.30 + e * 0.62 + (time_seconds * 37.0).sin() * tremble,
        // Swing it across to the left (the arm being bound), and turn the coil to
        // face the viewer as it comes.
        yaw: 0.25 - e * 0.78 + (time_seconds * 41.0 + 1.7).sin() * tremble,
        roll: 0.18 + e * 0.55 + (time_seconds * 29.0 + 0.5).sin() * tremble * 0.6,
        // Draw it in close to the body: the wrap happens against you, not out at
        // arm's length.
        forward: e * 0.10,
        right: -e * 0.14,
        up: e * 0.13,
    }
}

/// Smoothstep on `[0, 1]`. Local to the pose curves.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Thrown powder bomb: a short overhand lob. `phase` 0 is the carry rest; the
/// hand draws the bomb back and up (wind-up), snaps forward and up through the
/// release (at ~0.45, matching `THROW_BOMB_IMPACT_FRACTION` where the bomb
/// leaves the hand and the release cue fires), then eases back to rest
/// (recovery). Light and committed: a real toss, not a swing arc, and cheap
/// (a couple of eased segments), so it stays in the "keep it light" spirit.
pub(crate) fn throw_lob_pose(phase: f32) -> ToolSwingPose {
    // Wind-up: pull the bomb back toward the shoulder, cocking up and right.
    if phase <= 0.45 {
        let t = ease_out(phase / 0.45);
        return ToolSwingPose {
            pitch: lerp(-0.30, 0.55, t),
            yaw: lerp(0.10, -0.18, t),
            roll: lerp(0.05, 0.22, t),
            forward: lerp(0.0, -0.14, t),
            right: lerp(0.0, 0.10, t),
            up: lerp(0.0, 0.22, t),
        };
    }
    // Release + follow-through: snap the hand forward and down as the bomb leaves,
    // then settle back toward rest (ease-in on the throw, so the hand is fastest
    // at the release point).
    let t = ease_in((phase - 0.45) / 0.55);
    ToolSwingPose {
        pitch: lerp(0.55, -0.40, t),
        yaw: lerp(-0.18, 0.16, t),
        roll: lerp(0.22, 0.0, t),
        forward: lerp(-0.14, 0.16, t),
        right: lerp(0.10, 0.0, t),
        up: lerp(0.22, -0.02, t),
    }
}

/// Held bomb wind-up: the charge pose for the hold-to-power throw. Tracks
/// [`throw_lob_pose`]'s wind-up segment (so a release that primes the toss at
/// its release beat continues seamlessly from wherever the charge held), eased
/// so the pull-back starts eager and settles into the shoulder, with the bow
/// draw's strain tremble ramping in near full charge.
pub(crate) fn throw_charge_pose(wind_up: f32, time_seconds: f32) -> ToolSwingPose {
    let w = wind_up.clamp(0.0, 1.0);
    // Ease-out: most of the pull-back happens early, the last stretch settles.
    let eased = 1.0 - (1.0 - w) * (1.0 - w);
    let base = throw_lob_pose(0.45 * eased);
    // Strain tremble near full charge, the bow-draw idiom: zero through the
    // first 40%, quadratic ramp to a sub-degree shake at full.
    let strain = ((w - 0.4) / 0.6).clamp(0.0, 1.0);
    let tremble = 0.010 * strain * strain;
    ToolSwingPose {
        pitch: base.pitch + (time_seconds * 37.0).sin() * tremble,
        yaw: base.yaw + (time_seconds * 41.0 + 1.7).sin() * tremble,
        roll: base.roll + (time_seconds * 29.0 + 0.5).sin() * tremble * 0.6,
        ..base
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

// Wooden club: a short, quick chop. A brief wind-up cocks the club up and back
// over the shoulder, then a fast snap down and slightly across drives through
// contact at phase 0.45 (early, this is the fastest melee weapon), a short bite,
// then a quick recovery back to rest. Deliberately compact: no long overhead
// load like the mace, no reach like the spear, just a snappy one-hander. Keep
// `CLUB_IMPACT_FRACTION` in `gather.rs` matched to the 0.45 contact.
pub(crate) fn club_swing_pose(phase: f32) -> ToolSwingPose {
    if phase <= 0.30 {
        // Wind-up: cock the club up and back over the shoulder (ease-out hang),
        // a short load, this is a quick weapon, not a heavy one.
        let t = ease_out(phase / 0.30);
        return ToolSwingPose {
            pitch: lerp(-0.30, 0.55, t),
            yaw: lerp(0.10, -0.18, t),
            roll: lerp(0.08, 0.28, t),
            forward: lerp(0.0, -0.10, t),
            right: lerp(0.0, 0.12, t),
            up: lerp(0.0, 0.22, t),
        };
    }
    if phase <= 0.45 {
        // Strike: snap down and slightly across, driving forward through contact
        // (ease-in), so the head is moving fastest at impact (phase 0.45).
        let t = ease_in((phase - 0.30) / 0.15);
        return ToolSwingPose {
            pitch: lerp(0.55, -0.85, t),
            yaw: lerp(-0.18, 0.18, t),
            roll: lerp(0.28, -0.04, t),
            forward: lerp(-0.10, 0.22, t),
            right: lerp(0.12, -0.08, t),
            up: lerp(0.22, -0.14, t),
        };
    }
    if phase <= 0.58 {
        // Short bite: the head holds low just past contact before recovery.
        let t = smoothstep((phase - 0.45) / 0.13);
        return ToolSwingPose {
            pitch: lerp(-0.85, -0.72, t),
            yaw: lerp(0.18, 0.14, t),
            roll: lerp(-0.04, -0.02, t),
            forward: lerp(0.22, 0.14, t),
            right: lerp(-0.08, -0.04, t),
            up: lerp(-0.14, -0.10, t),
        };
    }
    // Quick recovery back to rest.
    let t = smoothstep((phase - 0.58) / 0.42);
    ToolSwingPose {
        pitch: lerp(-0.72, -0.30, t),
        yaw: lerp(0.14, 0.10, t),
        roll: lerp(-0.02, 0.08, t),
        forward: lerp(0.14, 0.0, t),
        right: lerp(-0.04, 0.0, t),
        up: lerp(-0.10, 0.0, t),
    }
}

// Stone spear: a committed two-handed forward THRUST along the aim axis, not
// an arc. The CARRY is a low two-handed guard, the point angled clearly DOWN
// ahead of the feet (owner feedback: held with both hands from the lower
// part). The wind-up chambers the shaft back toward the hip, then it lunges
// hard forward AND UP from below, the tip climbing from the low guard to the
// crosshair as the `forward` offset drives extension, levelling out exactly at
// contact (phase 0.55). It holds the extension a beat, then retracts back to
// the low guard. Keep `SPEAR_IMPACT_FRACTION` matched to the 0.55 contact.
pub(crate) fn spear_swing_pose(phase: f32) -> ToolSwingPose {
    // The waist carry: the rig sits LOW in the frame (hands braced at the hip)
    // with the tip angled UP toward the centre, the shaft rising out of the
    // lower-right corner. The previous steep tip-down carry sat high in the
    // frame and read as the spear held with both arms stretched overhead
    // (owner report), and a level couch foreshortened the shaft into nearly
    // nothing on screen; the low grip + rising tip is what reads as a
    // two-handed hold from the waist.
    const REST_PITCH: f32 = 0.30;
    const REST_UP: f32 = -0.11;
    if phase <= 0.35 {
        // Wind-up: chamber the shaft straight back along the couch line
        // (ease-out), both hands loading the thrust from the hip.
        let t = ease_out(phase / 0.35);
        return ToolSwingPose {
            pitch: lerp(REST_PITCH, -0.30, t),
            yaw: lerp(0.10, 0.06, t),
            roll: lerp(0.08, 0.04, t),
            forward: lerp(0.0, -0.26, t),
            right: lerp(0.0, 0.03, t),
            up: lerp(REST_UP, -0.14, t),
        };
    }
    if phase <= 0.55 {
        // Thrust: drive forward and UP from the hip chamber (ease-in) to full
        // extension at contact, the tip climbing to the crosshair (pitch levels
        // out at strike). Drifts toward the screen centre (negative right) so
        // the extended stab reads driven at the aim point, not parked at the
        // right edge.
        let t = ease_in((phase - 0.35) / 0.20);
        return ToolSwingPose {
            pitch: lerp(-0.30, -0.04, t),
            yaw: lerp(0.06, 0.0, t),
            roll: lerp(0.04, 0.0, t),
            forward: lerp(-0.26, 0.52, t),
            right: lerp(0.03, -0.16, t),
            up: lerp(-0.14, 0.10, t),
        };
    }
    if phase <= 0.70 {
        // Hold the extension a beat, the spear stays buried at full reach (and
        // still centred, still risen) before it is drawn back.
        let t = smoothstep((phase - 0.55) / 0.15);
        return ToolSwingPose {
            pitch: lerp(-0.04, -0.08, t),
            yaw: 0.0,
            roll: 0.0,
            forward: lerp(0.52, 0.44, t),
            right: lerp(-0.16, -0.10, t),
            up: lerp(0.10, 0.06, t),
        };
    }
    // Retract back down to the waist couch.
    let t = smoothstep((phase - 0.70) / 0.30);
    ToolSwingPose {
        pitch: lerp(-0.08, REST_PITCH, t),
        yaw: lerp(0.0, 0.10, t),
        roll: lerp(0.0, 0.08, t),
        forward: lerp(0.44, 0.0, t),
        right: lerp(-0.10, 0.0, t),
        up: lerp(0.06, REST_UP, t),
    }
}

// Iron sword: a fast swing-around SLASH, the whippiest swing in the game,
// authored as a KEYFRAME SPLINE rather than eased segments. The blade draws
// smoothly up over the right shoulder, swings around and down the right side,
// whips across the BOTTOM of the screen past centre and clean off the left
// edge of the frame, hangs a beat, and flows back to guard. Interpolating one
// Catmull-Rom spline through the keys (the way DCC-authored game slash
// animations are keyed and sampled) keeps position AND velocity continuous
// across the whole swing: no stop-start at the top of the draw, no corner at
// the bottom of the arc, none of the segment-boundary jank the eased version
// had (owner report). Pacing lives in the key SPACING: the cross-screen keys
// sit close together in phase, so the blade is fastest through the cut.
// Contact (`SWORD_IMPACT_FRACTION` = 0.34) is the moment the blade crosses
// the crosshair mid-sweep, not the end of the travel. The slash trail
// (`slash_trail`) samples this same pose, so the wind draws the same arc.
const SWORD_SLASH_KEYS: [(f32, ToolSwingPose); 7] = [
    // guard
    (0.00, pose(-0.30, 0.10, 0.08, 0.0, 0.0, 0.0)),
    // drawn up over the right shoulder, edge rolled
    (0.16, pose(0.34, -0.50, 0.55, -0.12, 0.26, 0.16)),
    // swinging around: down the right side of the frame
    (0.27, pose(-0.30, -0.08, 0.16, 0.08, 0.18, -0.16)),
    // crossing the crosshair, the fastest stretch of the arc
    (0.36, pose(-0.50, 0.55, -0.28, 0.26, -0.35, -0.25)),
    // exited: off past the left edge of the frame
    (0.46, pose(-0.52, 1.25, -0.45, 0.30, -0.95, -0.26)),
    // short hang out left so the exit reads
    (0.58, pose(-0.46, 1.02, -0.30, 0.18, -0.72, -0.19)),
    // flow back to guard
    (1.00, pose(-0.30, 0.10, 0.08, 0.0, 0.0, 0.0)),
];

/// Shorthand const constructor so the keyframe table reads as rows.
const fn pose(pitch: f32, yaw: f32, roll: f32, forward: f32, right: f32, up: f32) -> ToolSwingPose {
    ToolSwingPose {
        pitch,
        yaw,
        roll,
        forward,
        right,
        up,
    }
}

/// Centripetal-free Catmull-Rom over one scalar channel: interpolates p1..p2
/// with tangents from the neighbours, C1-continuous across segments.
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

pub(crate) fn sword_swing_pose(phase: f32) -> ToolSwingPose {
    sample_keys(&SWORD_SLASH_KEYS, phase)
}

/// Sample a Catmull-Rom keyframe table at `phase`: the shared spline sampler
/// behind the keyed slashes (sword, sickle). Clamped ends (the first/last key
/// doubles as the missing neighbour), C1-continuous across segments.
fn sample_keys(keys: &[(f32, ToolSwingPose)], phase: f32) -> ToolSwingPose {
    let phase = phase.clamp(0.0, 1.0);
    // Find the segment [i, i+1] containing `phase`.
    let mut i = 0;
    while i + 2 < keys.len() && phase > keys[i + 1].0 {
        i += 1;
    }
    let (t0, ref a) = keys[i];
    let (t1, ref b) = keys[i + 1];
    let t = if t1 > t0 {
        (phase - t0) / (t1 - t0)
    } else {
        0.0
    };
    // Clamped ends: duplicate the first/last key as the missing neighbour.
    let prev = if i == 0 { a } else { &keys[i - 1].1 };
    let next = if i + 2 < keys.len() {
        &keys[i + 2].1
    } else {
        b
    };
    let ch = |f: fn(&ToolSwingPose) -> f32| catmull_rom(f(prev), f(a), f(b), f(next), t);
    ToolSwingPose {
        pitch: ch(|p| p.pitch),
        yaw: ch(|p| p.yaw),
        roll: ch(|p| p.roll),
        forward: ch(|p| p.forward),
        right: ch(|p| p.right),
        up: ch(|p| p.up),
    }
}

// Sickle: a low horizontal REAPING cut, keyed as a Catmull-Rom spline like the
// sword. Where the sword whips shoulder-high around the frame, the sickle's
// whole arc stays DOWN at grass height. Authoring constraint: the crescent's
// plane CONTAINS the haft, so the face reads only while the haft stays
// roughly upright in view (yaw the blade toward the view axis, or lay the
// haft along it, and the mesh collapses to a paper-thin line). The sweep is
// therefore a windshield-wiper arc: the haft stays near vertical, ROLL tips
// it right-then-left through the cut, and the lateral `right` travel carries
// it across the bottom of the frame, with only a whisper of yaw. Contact
// (`SICKLE_IMPACT_FRACTION` = 0.42) is the crescent crossing under the
// crosshair mid-sweep, face-on the whole way.
const SICKLE_REAP_KEYS: [(f32, ToolSwingPose); 7] = [
    // carry rest: a near-upright haft with a whisper of image-plane lean
    // (positive roll = haft top toward screen LEFT), the owner's reference
    // framing: the pale handle sits at the lower right and the forged hook
    // arcs up and INWARD with its point hanging down. The hook shape is in
    // the mesh now, so the carry needs no big compensating lean.
    (0.00, pose(-0.15, 0.10, 0.15, 0.0, -0.02, -0.04)),
    // drawn out low to the right, the hook rolled up and cocked (the roll
    // swings from the rest lean through upright, loading the wiper)
    (0.22, pose(-0.35, -0.15, 0.50, -0.10, 0.34, -0.08)),
    // sweep begins: coming in low along the right, tipping through upright
    (0.34, pose(-0.50, 0.05, 0.10, 0.10, 0.14, -0.22)),
    // crossing under the crosshair, the fastest stretch of the reap
    (0.42, pose(-0.55, 0.22, -0.35, 0.22, -0.16, -0.28)),
    // exited: off past the left edge, hook swept through, still low
    (0.54, pose(-0.50, 0.40, -0.90, 0.18, -0.78, -0.24)),
    // short hang out left so the cut reads
    (0.68, pose(-0.44, 0.32, -0.65, 0.12, -0.55, -0.18)),
    // settle back to the carry
    (1.00, pose(-0.15, 0.10, 0.15, 0.0, -0.02, -0.04)),
];

pub(crate) fn sickle_swing_pose(phase: f32) -> ToolSwingPose {
    sample_keys(&SICKLE_REAP_KEYS, phase)
}

// Iron mace: a big, slow overhead with a pronounced wind-up and follow-through,
// the heaviest swing in the game. A long, deliberate draw hauls the head high
// overhead and well back (ease-out, so it decelerates into a long hang at the
// apex, the anticipation that sells the mass), then an explosive downward smash
// drives forward through the centre at contact (phase 0.70, late, the payoff of
// the load). The head buries low and DWELLS there, then is dragged slowly back
// up to rest, slower than it came down. The huge wind-up and the late, heavy
// contact are the mace's whole identity. Keep `MACE_IMPACT_FRACTION` matched to
// the 0.70 contact.
pub(crate) fn mace_swing_pose(phase: f32) -> ToolSwingPose {
    if phase <= 0.55 {
        // Long, deliberate overhead draw. Smoothstep gives an immediate but
        // controlled rise that decelerates into a long hang at the apex, the
        // pronounced wind-up. The head goes high and well back, near vertical.
        let t = smoothstep((phase / 0.55).clamp(0.0, 1.0));
        return ToolSwingPose {
            pitch: lerp(-0.30, 1.30, t),
            yaw: lerp(0.10, -0.05, t),
            roll: lerp(0.08, -0.04, t),
            forward: lerp(0.0, -0.24, t),
            right: lerp(0.0, 0.06, t),
            up: lerp(0.0, 0.42, t),
        };
    }
    if phase <= 0.70 {
        // Smash: an explosive downward drive through the centre (ease-in), so the
        // head is moving hardest at the late contact (phase 0.70).
        let t = ease_in((phase - 0.55) / 0.15);
        return ToolSwingPose {
            pitch: lerp(1.30, -2.00, t),
            yaw: lerp(-0.05, 0.04, t),
            roll: lerp(-0.04, 0.04, t),
            forward: lerp(-0.24, 0.42, t),
            right: lerp(0.06, -0.03, t),
            up: lerp(0.42, -0.36, t),
        };
    }
    if phase <= 0.86 {
        // Follow-through + dwell: the head buries low and forward and holds there,
        // a heavy settle that reads as the weapon's full mass landing.
        let t = smoothstep((phase - 0.70) / 0.16);
        return ToolSwingPose {
            pitch: lerp(-2.00, -1.80, t),
            yaw: lerp(0.04, 0.05, t),
            roll: lerp(0.04, 0.02, t),
            forward: lerp(0.42, 0.30, t),
            right: lerp(-0.03, -0.01, t),
            up: lerp(-0.36, -0.30, t),
        };
    }
    // Slow drag back to rest, slower than the smash: you don't snap a mace up.
    let t = smoothstep((phase - 0.86) / 0.14);
    ToolSwingPose {
        pitch: lerp(-1.80, -0.30, t),
        yaw: lerp(0.05, 0.10, t),
        roll: lerp(0.02, 0.08, t),
        forward: lerp(0.30, 0.0, t),
        right: lerp(-0.01, 0.0, t),
        up: lerp(-0.30, 0.0, t),
    }
}

// Wooden bow: a hold-to-draw pose, not a swing. `draw_fraction` (0 = at rest just
// after nocking, 1 = full draw) pulls the bow up and across the body: the grip
// hand rises toward eye line and the whole weapon rolls so the string can be
// drawn back past the cheek. As full draw nears, a fine tremble builds (sub-degree
// noise that scales with the fraction) so holding a committed shot reads as
// muscular strain rather than a locked mannequin. The pose is continuous in
// `draw_fraction`, so the input layer can drive it straight off
// `RangedDrawState::draw_fraction()` every frame. `time_seconds` seeds the
// tremble; passing a fixed value gives a deterministic pose for tests.
pub(crate) fn bow_draw_pose(draw_fraction: f32, time_seconds: f32) -> ToolSwingPose {
    let f = draw_fraction.clamp(0.0, 1.0);
    // Ease the gross motion so the bow settles into full draw rather than snapping.
    let t = smoothstep(f);
    // Anchor-hand read: the bow lifts (up), rolls the limbs over (roll), and the
    // draw hand pulls the whole rig back toward the face (negative forward), while
    // the aim stays roughly level (small pitch) so the arrow points where you look.
    let base = ToolSwingPose {
        pitch: lerp(-0.18, -0.02, t),
        // With the drawn string anchored ON the centre line (see `right`
        // below) the arrow needs almost no convergence angle at all: it
        // points straight down the camera axis, exactly where the shot
        // actually goes. The earlier 0.26 over-rotated and read as shooting
        // at the left of the screen (owner report, twice, from both
        // directions).
        yaw: lerp(0.08, 0.05, t),
        // The drawn stave takes a modest CANT (roll): archers tilt the bow
        // aiming instinctively, and on screen the tilt breaks the flat
        // side-profile "C aiming left" read while the string stays welded to
        // the stave.
        roll: lerp(0.06, 0.18, t),
        // Push the bow away from the camera only moderately as it draws (positive
        // = further down -Z, added on top of the base forward offset): enough that
        // the limb span reads in frame, but well short of the old deep push, so
        // the draw reads as the string coming IN toward the eye rather than the
        // whole bow being shoved out across the screen.
        forward: lerp(0.02, 0.34, t),
        // Pull the drawn bow IN toward the centre line, the eye close behind
        // the nocked arrow (the classic first-person archery anchor: you
        // sight down the ARROW), while keeping the rig clearly right of the
        // crosshair so it frames the aim instead of covering it (dead-centre
        // and -0.14 both read as too far in, owner reports).
        right: lerp(0.02, -0.10, t),
        // Lift the rig toward the EYE LINE as it draws, so the nocked arrow's
        // foreshortened line sits at crosshair height and the shot reads as
        // aimed forward rather than lobbed from the hip.
        up: lerp(0.0, 0.16, t),
    };
    // Tremble ramps in near full draw: exactly zero through the first 40% of the
    // draw (an early draw is genuinely steady), then a quadratic ramp to a
    // sub-degree shake at full, so a maxed hold reads as muscular strain. Two
    // incommensurate sine frequencies keep it from reading as a clean oscillation.
    let strain = ((f - 0.4) / 0.6).clamp(0.0, 1.0);
    let tremble = 0.010 * strain * strain;
    let shake_p = (time_seconds * 37.0).sin() * tremble;
    let shake_y = (time_seconds * 41.0 + 1.7).sin() * tremble;
    let shake_r = (time_seconds * 29.0 + 0.5).sin() * tremble * 0.6;
    ToolSwingPose {
        pitch: base.pitch + shake_p,
        yaw: base.yaw + shake_y,
        roll: base.roll + shake_r,
        ..base
    }
}

// Bow release: the snap-forward on loose that settles back to rest. `progress`
// runs 0 (just released, still near full draw) to 1 (returned to the carry rest).
// It jumps the rig forward off the cheek fast (the string releasing) then eases
// back, so loose reads as a recoil-free forward flick rather than a swing.
pub(crate) fn bow_release_pose(progress: f32) -> ToolSwingPose {
    let p = progress.clamp(0.0, 1.0);
    // A quick forward flick early, decaying back to the carry rest. `punch` peaks
    // just after release and falls to zero by the end.
    let punch = (1.0 - p) * (1.0 - p);
    ToolSwingPose {
        pitch: lerp(-0.02, -0.18, p),
        // Starts at the full-draw yaw (so loose doesn't snap the rig sideways)
        // and settles to the carry's inward turn, keeping the ready bow
        // pointed toward a forward shot rather than off to the left.
        yaw: lerp(0.05, 0.08, p),
        roll: lerp(0.34, 0.10, p),
        // Snap forward off the cheek (positive forward) right at release, settling
        // back to the small rest offset.
        forward: lerp(0.14, 0.02, p) + punch * 0.06,
        // Starts at the drawn near-centre anchor so loose doesn't jump the bow
        // sideways, then settles back out to the carry rest.
        right: lerp(-0.10, 0.02, p),
        up: lerp(0.16, 0.0, p),
    }
}

// Crossbow: a shouldered idle with a punchy recoil kick on fire and a reload crank
// cycle. There is no draw hold. `recoil` (0 = settled, 1 = just fired) drives a
// short back-and-up jolt; `reload_fraction` (0 = just fired, 1 = ready) drives the
// crank: the stock dips and rolls as the windlass is cranked, then rises back to
// the shouldered ready pose. Recoil and reload overlap early (the kick lands as the
// crank begins) and the reload owns the rest of the cooldown. `aim` (0 = carry,
// 1 = full ADS) levels the ready lift so the sighted stock points straight down
// the crosshair; the whole-item centring for the ADS lives in the held-item
// offset blend, this pose only trues up the sight line.
pub(crate) fn crossbow_pose(recoil: f32, reload_fraction: f32, aim: f32) -> ToolSwingPose {
    let r = recoil.clamp(0.0, 1.0);
    let rl = reload_fraction.clamp(0.0, 1.0);
    let a = aim.clamp(0.0, 1.0);
    // Shouldered ready pose: the whole-item crossbow rotation already lays the
    // stock running forward into the screen (muzzle down-range), so the ready pose
    // only needs a tiny UP pitch to lift the far prod to the crosshair and read as
    // aimed rather than drooping. Level, braced. This is the rest the recoil and
    // reload perturb. In this frame the crossbow points along view -Z, so a
    // POSITIVE pitch lifts the muzzle UP and a NEGATIVE pitch dips it DOWN. At
    // full ADS the lift flattens almost level so the stock runs straight along
    // the sight line instead of angling up across it.
    let ready = ToolSwingPose {
        pitch: lerp(0.10, 0.01, a),
        yaw: 0.0,
        roll: 0.0,
        forward: 0.0,
        right: 0.0,
        up: 0.0,
    };
    // Recoil: a sharp back-and-up jolt right on fire, decaying to nothing. The
    // muzzle kicks UP (positive pitch) and the whole rig shoves back toward the
    // shoulder (negative forward = pulled toward the camera).
    let kick = r * r;
    // Reload crank: the muzzle DIPS DOWN (a strong negative pitch that overwhelms
    // the small ready lift) hardest at the middle of the cycle, as if the bow is
    // tipped nose-down and the windlass cranked up from below, then rises back to
    // the shouldered ready by the end. A sine bump peaks at reload_fraction 0.5.
    // The strong negative pitch + a downward drop + a roll read as the crossbow
    // being cranked up from below, clearly distinct from the level ready pose.
    let crank = (rl * std::f32::consts::PI).sin();
    ToolSwingPose {
        pitch: ready.pitch + kick * 0.28 - crank * 1.00,
        yaw: ready.yaw - crank * 0.10,
        roll: ready.roll + crank * 0.35,
        forward: ready.forward - kick * 0.12 + crank * 0.06,
        right: ready.right,
        up: ready.up + kick * 0.12 - crank * 0.30,
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
    fn throw_lob_pose_winds_up_then_flicks_forward_on_release() {
        let rest = throw_lob_pose(0.0);
        let cocked = throw_lob_pose(0.45); // the release point (impact fraction)
        let settled = throw_lob_pose(1.0);

        // Wind-up cocks the bomb back and up (pitch + up rise, forward pulls back)
        // so the toss loads before the flick.
        assert!(cocked.pitch > rest.pitch + 0.5, "cocks back");
        assert!(cocked.up > rest.up + 0.10, "raises the bomb");
        assert!(
            cocked.forward < rest.forward - 0.08,
            "draws back before the throw"
        );

        // The follow-through drives forward past the wind-up (the release flick)
        // then eases back near rest.
        assert!(
            settled.forward > cocked.forward + 0.20,
            "flicks forward on release"
        );
        assert!(
            settled.pitch < cocked.pitch - 0.5,
            "snaps the hand down through the throw"
        );
        // Rest and the settled follow-through are close (the pose returns toward
        // the carry rest), and every field stays finite across the whole arc.
        for i in 0..=20 {
            let p = throw_lob_pose(i as f32 / 20.0);
            for v in [p.pitch, p.yaw, p.roll, p.forward, p.right, p.up] {
                assert!(v.is_finite(), "throw pose stays finite");
            }
        }
    }

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
    fn club_swing_pose_is_a_short_quick_chop() {
        let ready = club_swing_pose(0.0);
        let apex = club_swing_pose(0.30);
        let impact = club_swing_pose(0.45);

        // A brief wind-up cocks the club up and back over the shoulder.
        assert!(apex.pitch > ready.pitch + 0.5, "cocks up on the wind-up");
        assert!(apex.up > ready.up + 0.10, "lifts on the wind-up");
        // The strike snaps down and drives forward through the early contact.
        assert!(
            impact.pitch < apex.pitch - 1.0,
            "snaps down into the strike"
        );
        assert!(impact.forward > apex.forward + 0.20, "drives forward");
        assert!(impact.up < ready.up, "the head ends low at contact");
    }

    #[test]
    fn spear_swing_pose_is_a_rising_two_handed_thrust() {
        let ready = spear_swing_pose(0.0);
        let chamber = spear_swing_pose(0.35);
        let impact = spear_swing_pose(0.55);

        // The thrust is defined by forward travel, not a swing arc: the point
        // chambers back AND DOWN toward the hip on the wind-up (the two-handed
        // load), then lunges hard forward to full extension.
        assert!(
            chamber.forward < ready.forward - 0.15,
            "the spear chambers backward before the lunge"
        );
        assert!(
            chamber.up < ready.up,
            "the chamber drops toward the hip (a low two-handed load)"
        );
        assert!(
            impact.forward > chamber.forward + 0.6,
            "the thrust drives a long way forward"
        );
        assert!(
            impact.forward > 0.4,
            "contact is at full forward extension along the aim axis"
        );
        // The stab rises from below: the tip climbs from the low chamber up
        // past the guard height at full extension.
        assert!(
            impact.up > chamber.up + 0.15,
            "the thrust drives upward out of the low chamber"
        );
        // The CARRY is a waist-level two-handed couch: the whole rig sits LOW
        // in the frame (hands braced at the hip) with the shaft near level,
        // only a slight tip-down. The earlier steep tip-down guard sat high
        // and read as arms stretched overhead (owner report). The thrust still
        // LEVELS OUT at contact so the strike runs straight along the aim axis
        // (owner: the striking part is good).
        assert!(
            ready.up < -0.04,
            "the carry sits low in the frame, held from the waist"
        );
        assert!(
            ready.pitch > 0.05 && ready.pitch < 0.40,
            "the carried tip angles UP out of the low grip, never the old steep tip-down guard"
        );
        assert!(
            impact.pitch.abs() < 0.15,
            "the strike levels out along the aim at contact"
        );
    }

    #[test]
    fn sword_swing_pose_is_a_smooth_swing_around_full_cross_cut() {
        let ready = sword_swing_pose(0.0);
        let apex = sword_swing_pose(0.16);
        let mid = sword_swing_pose(0.27);
        let exit = sword_swing_pose(0.46);

        // Wind-up flicks the blade UP over the right shoulder with the edge
        // rolled, loading the cut. Short: the raise is quick.
        assert!(apex.right > ready.right + 0.15, "draws out to the right");
        assert!(apex.up > ready.up + 0.12, "cocks UP over the shoulder");
        assert!(apex.yaw < ready.yaw - 0.4, "cocks the yaw for the sweep");
        assert!(
            apex.roll > ready.roll + 0.3,
            "rolls the edge over on the load"
        );
        // The strike is a SWING-AROUND: mid-strike the blade has already come
        // down the right side (low) while the lateral sweep has barely begun,
        // so the cross-screen travel happens along the BOTTOM of the frame.
        assert!(
            mid.up < -0.10,
            "mid-strike the blade is already low, got up {}",
            mid.up
        );
        assert!(
            mid.right > 0.0,
            "mid-strike the sweep has barely begun, got right {}",
            mid.right
        );
        // The cut carries past centre and clean OFF the left edge, low, with
        // forward drive; it never parks at the centre of the frame.
        assert!(
            exit.right < -0.8,
            "the cut exits past the left edge of the frame, got right {}",
            exit.right
        );
        assert!(exit.yaw > apex.yaw + 1.2, "yaw whips the cut across");
        assert!(
            exit.roll < apex.roll - 0.6,
            "the edge rolls through the cut"
        );
        assert!(
            (exit.pitch - ready.pitch).abs() < 0.35,
            "the cut stays near guard height: a slash, not an overhead chop"
        );
        assert!(
            exit.forward > apex.forward + 0.25,
            "drives forward through the cut"
        );
        assert!(
            exit.up < ready.up - 0.15,
            "the blade exits clearly low: across the bottom of the screen"
        );
        // Contact happens as the blade crosses the crosshair, mid-sweep.
        let contact = sword_swing_pose(0.34);
        assert!(
            contact.right.abs() < 0.30,
            "at the impact fraction the blade is crossing the centre, got {}",
            contact.right
        );
    }

    #[test]
    fn sword_swing_pose_is_continuous_with_no_velocity_spikes() {
        // The spline must be smooth end to end: dense-sample every channel and
        // assert no single step teleports (C0) and no step is wildly larger
        // than its neighbours (the segment-boundary jank the eased version
        // had). 200 samples => the fastest legitimate stretch (the cut, ~1.2
        // units of `yaw` travel over ~0.1 of phase) steps ~0.03 per sample.
        const N: usize = 200;
        let sample = |i: usize| sword_swing_pose(i as f32 / N as f32);
        let mut prev = sample(0);
        let mut max_step = 0.0f32;
        for i in 1..=N {
            let cur = sample(i);
            for (a, b) in [
                (prev.pitch, cur.pitch),
                (prev.yaw, cur.yaw),
                (prev.roll, cur.roll),
                (prev.forward, cur.forward),
                (prev.right, cur.right),
                (prev.up, cur.up),
            ] {
                max_step = max_step.max((b - a).abs());
            }
            prev = cur;
        }
        assert!(
            max_step < 0.06,
            "no channel may jump between adjacent samples, got {max_step}"
        );
        // And the swing starts/ends exactly at the same guard pose.
        let start = sword_swing_pose(0.0);
        let end = sword_swing_pose(1.0);
        assert!(
            (start.pitch - end.pitch).abs() < 1e-6 && (start.right - end.right).abs() < 1e-6,
            "the spline returns to the exact guard pose"
        );
    }

    #[test]
    fn sickle_swing_pose_is_a_low_horizontal_reap() {
        let ready = sickle_swing_pose(0.0);
        let cocked = sickle_swing_pose(0.22);
        let contact = sickle_swing_pose(0.42);
        let exit = sickle_swing_pose(0.54);

        // The wind-up draws out to the right at carry height, never up over
        // the shoulder: no overhead load at all.
        assert!(cocked.right > ready.right + 0.2, "draws out to the right");
        assert!(cocked.up < ready.up + 0.05, "no overhead load");
        assert!(
            cocked.roll > ready.roll + 0.3,
            "the wiper cocks to the right"
        );
        // The cut is roll+travel driven and LOW: at contact the crescent is
        // crossing under the crosshair at grass height, and the haft stays
        // near upright (small pitch/yaw travel) so the crescent's face reads
        // through the whole sweep (the blade plane contains the haft; yawing
        // it toward the view axis collapses it to a line).
        assert!(contact.up < -0.2, "the cut skims the ground");
        assert!(
            contact.right.abs() < 0.30,
            "contact happens crossing the centre, got {}",
            contact.right
        );
        assert!(exit.right < -0.7, "the reap exits past the left edge");
        assert!(
            exit.roll < cocked.roll - 1.0,
            "roll carries the wiper arc across"
        );
        assert!(exit.up < -0.15, "the exit stays low");
        for phase in [0.22, 0.34, 0.42, 0.54, 0.68] {
            let p = sickle_swing_pose(phase);
            assert!(
                p.yaw.abs() < 0.6 && (p.pitch - ready.pitch).abs() < 0.45,
                "the haft stays near upright through the reap (phase {phase})"
            );
        }
    }

    #[test]
    fn sickle_swing_pose_is_continuous_with_no_velocity_spikes() {
        // Same dense-sample smoothness bar the sword spline is held to.
        const N: usize = 200;
        let sample = |i: usize| sickle_swing_pose(i as f32 / N as f32);
        let mut prev = sample(0);
        let mut max_step = 0.0f32;
        for i in 1..=N {
            let cur = sample(i);
            for (a, b) in [
                (prev.pitch, cur.pitch),
                (prev.yaw, cur.yaw),
                (prev.roll, cur.roll),
                (prev.forward, cur.forward),
                (prev.right, cur.right),
                (prev.up, cur.up),
            ] {
                max_step = max_step.max((b - a).abs());
            }
            prev = cur;
        }
        assert!(
            max_step < 0.06,
            "no channel may jump between adjacent samples, got {max_step}"
        );
        let start = sickle_swing_pose(0.0);
        let end = sickle_swing_pose(1.0);
        assert!(
            (start.pitch - end.pitch).abs() < 1e-6 && (start.right - end.right).abs() < 1e-6,
            "the spline returns to the exact carry pose"
        );
    }

    #[test]
    fn mace_swing_pose_is_a_big_slow_overhead_with_a_late_contact() {
        let ready = mace_swing_pose(0.0);
        let apex = mace_swing_pose(0.55);
        let impact = mace_swing_pose(0.70);
        let follow = mace_swing_pose(0.80);

        // A pronounced overhead wind-up hauls the head high and well back, higher
        // than the pickaxe's already-heavy apex.
        assert!(apex.pitch > ready.pitch + 1.4, "big overhead wind-up");
        assert!(apex.up > ready.up + 0.35, "the head goes high overhead");
        assert!(
            apex.up > pickaxe_swing_pose(0.60).up,
            "the mace winds up higher than the pickaxe"
        );
        // The smash drives down through contact at the LATE fraction (0.70).
        assert!(impact.pitch < apex.pitch - 2.5, "explosive downward smash");
        assert!(
            impact.forward > apex.forward + 0.4,
            "drives forward at contact"
        );
        // A pronounced follow-through: the head buries low and dwells past contact
        // rather than snapping back.
        assert!(
            follow.up < ready.up - 0.15,
            "the head dwells low after contact"
        );
        assert!(
            follow.forward > 0.2,
            "the follow-through stays committed forward"
        );
    }

    #[test]
    fn bow_draw_pose_pulls_inward_right_as_it_ramps() {
        // At a fixed time (no tremble contribution difference), the full draw must
        // be clearly displaced from the rest draw: the rig lifts, rolls, and is
        // pulled back toward the cheek.
        let t = 0.0;
        let rest = bow_draw_pose(0.0, t);
        let full = bow_draw_pose(1.0, t);

        // The bow is pushed away from the camera moderately as it draws so its
        // limb span fits in frame. Positive forward = further down -Z (away).
        assert!(
            full.forward > rest.forward + 0.2,
            "the bow is pushed out enough that the limb span and drawn V fit in frame"
        );
        // The draw turns a little inward (a small convergence angle, never the
        // big left-turn that read as shooting at the left of the screen):
        // with the drawn string anchored on the centre line, the arrow points
        // essentially straight down the camera axis.
        assert!(
            full.yaw.abs() < 0.1,
            "the drawn yaw is a whisper so the shot reads dead ahead"
        );
        assert!(
            full.right < rest.right - 0.08,
            "the draw pulls the bow in toward the centre line, eye behind the arrow"
        );
        assert!(
            full.up > rest.up + 0.1,
            "the draw lifts the arrow line to the eye"
        );
        // The aim stays roughly level (a small pitch change) so the arrow points
        // where the player looks; the draw is not an overhead arc.
        assert!(
            (full.pitch - rest.pitch).abs() < 0.25,
            "the bow stays near level through the draw"
        );
    }

    #[test]
    fn bow_draw_pose_trembles_near_full_but_is_steady_early() {
        // Near full draw the pose must differ frame to frame (the tension tremble);
        // at rest / low draw it must be steady (no tremble). Sample two different
        // times and compare.
        let low_a = bow_draw_pose(0.05, 0.0);
        let low_b = bow_draw_pose(0.05, 1.0);
        assert_eq!(
            low_a.pitch, low_b.pitch,
            "an early draw is steady (no tremble)"
        );

        let full_a = bow_draw_pose(1.0, 0.20);
        let full_b = bow_draw_pose(1.0, 0.40);
        assert!(
            full_a.pitch != full_b.pitch || full_a.yaw != full_b.yaw || full_a.roll != full_b.roll,
            "a full draw trembles frame to frame"
        );
    }

    #[test]
    fn bow_release_snaps_forward_then_settles_to_rest() {
        // Just after release the rig flicks forward (off the cheek); by the end it
        // returns to the carry rest, matching the draw pose's rest at fraction 0.
        let early = bow_release_pose(0.0);
        let settled = bow_release_pose(1.0);
        assert!(
            early.forward > settled.forward,
            "release snaps forward before settling back"
        );
        // The settled release pose lines up with the resting draw pose (continuity
        // between loose and carry).
        let rest_draw = bow_draw_pose(0.0, 0.0);
        assert!(
            (settled.up - rest_draw.up).abs() < 0.05
                && (settled.forward - rest_draw.forward).abs() < 0.05,
            "the release settles back to the carry rest"
        );
    }

    #[test]
    fn crossbow_pose_kicks_on_fire_and_cranks_through_the_reload() {
        // A fresh fire (recoil 1) jolts back-and-up from the settled ready pose.
        let ready = crossbow_pose(0.0, 0.0, 0.0);
        let fired = crossbow_pose(1.0, 0.0, 0.0);
        assert!(fired.up > ready.up + 0.05, "recoil kicks the muzzle up");
        assert!(
            fired.pitch > ready.pitch + 0.05,
            "recoil pitches the stock back"
        );

        // The reload crank dips the stock hardest at mid-cycle, returning to ready
        // by the end.
        let mid_reload = crossbow_pose(0.0, 0.5, 0.0);
        let done_reload = crossbow_pose(0.0, 1.0, 0.0);
        assert!(
            mid_reload.pitch < ready.pitch - 0.10,
            "the crank dips the stock mid-reload"
        );
        assert!(
            mid_reload.roll > ready.roll + 0.10,
            "the crank rolls the stock as the windlass turns"
        );
        assert!(
            (done_reload.pitch - ready.pitch).abs() < 1e-6,
            "a finished reload returns to the shouldered ready pose"
        );
    }

    #[test]
    fn crossbow_ads_levels_the_sight_line() {
        // Holding the aim flattens the carry pose's up-lift toward level, so the
        // sighted stock runs straight along the crosshair instead of angling up
        // across it. The whole-item centring lives in the held-item offset
        // blend; the pose's job is only the sight-line pitch.
        let carry = crossbow_pose(0.0, 0.0, 0.0);
        let aimed = crossbow_pose(0.0, 0.0, 1.0);
        assert!(
            aimed.pitch < carry.pitch - 0.05,
            "full ADS flattens the muzzle lift toward level"
        );
        assert!(
            aimed.pitch > 0.0,
            "the aimed stock keeps a hair of lift, never drooping"
        );
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
