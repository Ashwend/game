//! Procedural locomotion + swing animation for remote bodies, plus the
//! charging held-item animator. Only the part LOCAL rotations are animated
//! here, so they compose with the interpolated/collapsing root transform.

use bevy::prelude::*;

use crate::{
    app::{
        scene::{PLAYER_HEAD_TOP_LOCAL_Y, PlayerPart},
        state::swing_duration_seconds,
        systems::items::{
            HeldGripSockets, RangedPoseInputs, carry_forearm_rotation, carry_upper_arm_rotation,
            held_item_hand_transform, held_piece_local_transform, remote_swing_arm_pose,
        },
    },
    items::{HeldMesh, ItemModel},
    server::PlayerEquipmentVisual,
};

use super::{
    DyingPlayer, PlayerRig, RemoteAction, RemoteEquipment, RemoteHeld, RemoteHeldPiece,
    RemoteLocomotion, SleepingPlayer, interpolation::REMOTE_PLAYER_INTERPOLATION_SECONDS,
    rig::RemoteSwing,
};

/// Horizontal speed (m/s) below which a remote body reads as idle.
const LOCOMOTION_MOVE_THRESHOLD: f32 = 0.5;
/// Speed at which the walk cycle reaches full walk amplitude.
const LOCOMOTION_WALK_SPEED: f32 = 3.0;
/// Speed at which the run cycle reaches full amplitude.
const LOCOMOTION_RUN_SPEED: f32 = 6.0;
/// Thigh swing (radians) at a full walk / full run.
const LEG_SWING_WALK: f32 = 0.42;
const LEG_SWING_RUN: f32 = 0.85;
/// Arms counter-swing the legs at this fraction of the leg amplitude.
const ARM_SWING_FRACTION: f32 = 0.7;
/// Knee bend amplitude (radians).
const KNEE_BEND: f32 = 0.5;
/// Stride cadence (radians/sec) = base + scale * speed.
const STRIDE_CADENCE_BASE: f32 = 3.4;
const STRIDE_CADENCE_SCALE: f32 = 1.5;
/// Constant slight elbow bend so arms aren't ramrod-straight at rest.
const ELBOW_REST_BEND: f32 = 0.15;

/// Fraction of a peer's look pitch that leans the visible torso (head + chest +
/// both arms). The held arm makes up the remaining fraction on its own shoulder
/// so the item in hand still tracks the FULL aim, while the spine only bends this
/// much: a standing peer glancing straight up/down leans convincingly without
/// folding in half. `torso + held-arm compensation == 1.0`, so the bow/tool ends
/// pointed exactly where the peer is looking.
const REMOTE_TORSO_PITCH_FRACTION: f32 = 0.6;

/// Hip-line pivot (root-local Y) the torso pitch rotates about, so the pelvis
/// (baked into the `Body` mesh) stays glued to the root-parented legs instead of
/// swinging forward off the origin.
const REMOTE_TORSO_PITCH_PIVOT_Y: f32 = -0.13;

/// Local transform for the `Body` part that leans the upper torso by
/// `torso_pitch` (radians, +up = lean back to look up, matching the FP camera's
/// `from_euler(YXZ, yaw, pitch, 0)` sign) and twists it by `torso_twist` (the
/// swing wind-up), rotating about the hip line so the pelvis holds its place over
/// the legs. Returned as an explicit (rotation, translation) pair because pitch
/// about an off-origin pivot needs a compensating translation, unlike the
/// rotation-only rest pose.
fn remote_body_pose(torso_pitch: f32, torso_twist: f32) -> (Quat, Vec3) {
    let rotation = Quat::from_rotation_x(torso_pitch) * Quat::from_rotation_y(torso_twist);
    let pivot = Vec3::new(0.0, REMOTE_TORSO_PITCH_PIVOT_Y, 0.0);
    // Keep the pivot point fixed: world(pivot) = translation + rotation*pivot = pivot.
    let translation = pivot - rotation * pivot;
    (rotation, translation)
}

/// Root-local position of the top of a remote peer's head AFTER the look-pitch
/// lean, so a head-anchored overlay (nametag, health bar, chat bubble) tracks the
/// leaning head instead of floating off it. The head is baked into the `Body`
/// mesh at local `(0, PLAYER_HEAD_TOP_LOCAL_Y, 0)`, and `Body` sits at the root
/// origin, so applying the same hip-pivot lean the animator uses gives the head
/// top in the root frame. Twist is passed as 0: a point on the local Y axis is
/// invariant under the swing's yaw twist, so only the pitch lean moves the head.
/// Callers project it through the root's `GlobalTransform` (which carries the
/// yaw + world position) to reach world space.
pub(crate) fn remote_head_anchor_local(pitch: f32) -> Vec3 {
    let torso_pitch = pitch * REMOTE_TORSO_PITCH_FRACTION;
    let (rotation, translation) = remote_body_pose(torso_pitch, 0.0);
    translation + rotation * Vec3::new(0.0, PLAYER_HEAD_TOP_LOCAL_Y, 0.0)
}

/// Copy a replicated `PlayerEquipmentVisual` into the local `RemoteEquipment`
/// mirror. A plain field copy; kept as a named helper so both the spawn and the
/// per-frame edge-detected update read the same mapping.
pub(super) fn remote_equipment_from(visual: PlayerEquipmentVisual) -> RemoteEquipment {
    RemoteEquipment {
        head: visual.head,
        chest: visual.chest,
        legs: visual.legs,
        feet: visual.feet,
    }
}

/// Procedural locomotion + swing animation for remote bodies. Reads the mirror
/// components written by `apply_snapshot_system` and writes each part's local
/// rotation. Dying bodies freeze (the death tick owns their root transform);
/// sleeping bodies relax to a straight pose.
#[expect(clippy::type_complexity, reason = "Bevy system query type")]
pub(crate) fn animate_remote_players_system(
    time: Res<Time>,
    mut rigs: Query<(
        &mut PlayerRig,
        &RemoteLocomotion,
        &RemoteAction,
        &RemoteHeld,
        Option<&DyingPlayer>,
        Option<&SleepingPlayer>,
    )>,
    mut parts: Query<&mut Transform, With<PlayerPart>>,
) {
    use std::f32::consts::PI;
    let dt = time.delta_secs().max(0.0);
    for (mut rig, loco, action, held, dying, sleeping) in &mut rigs {
        // A collapsing corpse keeps the pose it died in (the death tick owns the
        // root transform); don't keep walking it. The look-pitch lean is a
        // living-peer behavior, though: clear the Body's local pitch + hip-pivot
        // translation so the corpse collapses with an upright torso instead of
        // freezing an arched back (a dead player has no aim to track).
        if dying.is_some() {
            if let Ok(mut transform) = parts.get_mut(rig.body) {
                transform.rotation = Quat::IDENTITY;
                transform.translation = Vec3::ZERO;
            }
            continue;
        }
        let holding = held.0.is_some();

        // Copy the part handles out so we never hold a `Mut<PlayerRig>` borrow
        // across the part-Transform writes.
        let body = rig.body;
        let upper_arm_l = rig.upper_arm_l;
        let upper_arm_r = rig.upper_arm_r;
        let forearm_l = rig.forearm_l;
        let forearm_r = rig.forearm_r;
        let hand_anchor = rig.hand_anchor;
        let thigh_l = rig.thigh_l;
        let thigh_r = rig.thigh_r;
        let shin_l = rig.shin_l;
        let shin_r = rig.shin_r;

        // A logged-out sleeper lies straight; relax every joint and drop any
        // in-progress swing.
        if sleeping.is_some() {
            rig.swing = None;
            for part in [
                body,
                upper_arm_l,
                upper_arm_r,
                forearm_l,
                forearm_r,
                thigh_l,
                thigh_r,
                shin_l,
                shin_r,
            ] {
                set_rot(&mut parts, part, Quat::IDENTITY);
            }
            // The torso pitch also moves the body's translation (hip-pivot
            // compensation); clear it so a sleeper relaxes to the rest origin.
            if let Ok(mut transform) = parts.get_mut(body) {
                transform.translation = Vec3::ZERO;
            }
            continue;
        }

        // Ease the look pitch toward the replicated value, which steps at the
        // network tick rate, so the lean glides like the interpolated root yaw
        // instead of stair-stepping. Seed it on first sight so a peer already
        // looking up snaps to that aim rather than ramping up from level.
        let target_pitch = loco.pitch;
        if rig.pitch_seeded {
            let ease = 1.0 - (-dt / REMOTE_PLAYER_INTERPOLATION_SECONDS).exp();
            rig.smoothed_pitch += (target_pitch - rig.smoothed_pitch) * ease;
        } else {
            rig.smoothed_pitch = target_pitch;
            rig.pitch_seeded = true;
        }

        // Look pitch leans the upper body: the visible torso bends
        // `REMOTE_TORSO_PITCH_FRACTION` of the way, and (when holding) the held
        // arm makes up the rest on its own shoulder so the item in hand tracks
        // the FULL aim. Both are rotations about the same world axis (the peer's
        // right), so torso + arm sum back to the exact look pitch.
        let look_pitch = rig.smoothed_pitch;
        let torso_pitch = look_pitch * REMOTE_TORSO_PITCH_FRACTION;
        let arm_aim_pitch = if holding {
            look_pitch * (1.0 - REMOTE_TORSO_PITCH_FRACTION)
        } else {
            0.0
        };

        // Swing edge detection (seq, never `is_changed`).
        let mut swing = rig.swing;
        if action.seq > rig.last_swing_seq {
            rig.last_swing_seq = action.seq;
            // The wire `model` is the swing archetype directly (a weapon's own
            // Club/Spear/Sword, a gather tool's Hatchet/Pickaxe), so a peer
            // animates the right swing straight off the replicated action, no need
            // to infer it from the held mesh.
            let model = action.model;
            swing = Some(RemoteSwing {
                model,
                elapsed: 0.0,
                duration: swing_duration_seconds(model).max(0.05),
            });
        }

        // Locomotion: walk/run amplitude ramps with speed; cadence too.
        let speed = loco.speed;
        let walk_blend = smooth01((speed / LOCOMOTION_WALK_SPEED).clamp(0.0, 1.0));
        let leg_amp = locomotion_leg_amplitude(speed);
        let arm_amp = leg_amp * ARM_SWING_FRACTION;
        rig.stride_phase += dt * (STRIDE_CADENCE_BASE + speed * STRIDE_CADENCE_SCALE);
        let phase = rig.stride_phase;

        // Legs swing in anti-phase; knees bend on the lift.
        let leg_l = phase.sin() * leg_amp;
        let leg_r = (phase + PI).sin() * leg_amp;
        let knee_l = -(0.5 - 0.5 * (phase - 0.7).cos()) * KNEE_BEND * walk_blend;
        let knee_r = -(0.5 - 0.5 * (phase + PI - 0.7).cos()) * KNEE_BEND * walk_blend;
        set_rot(&mut parts, thigh_l, Quat::from_rotation_x(leg_l));
        set_rot(&mut parts, thigh_r, Quat::from_rotation_x(leg_r));
        set_rot(&mut parts, shin_l, Quat::from_rotation_x(knee_l));
        set_rot(&mut parts, shin_r, Quat::from_rotation_x(knee_r));

        // Arms counter-swing the legs (left arm with right leg), with a faint
        // idle breathing sway so a standing body isn't dead-still.
        let idle = (phase * 0.35).sin() * 0.05 * (1.0 - walk_blend);
        let arm_l = (phase + PI).sin() * arm_amp + idle;
        let arm_r = phase.sin() * arm_amp + idle;
        set_rot(&mut parts, upper_arm_l, Quat::from_rotation_x(arm_l));
        set_rot(
            &mut parts,
            forearm_l,
            Quat::from_rotation_x(-ELBOW_REST_BEND - arm_l.max(0.0) * 0.3),
        );

        // Right arm rest pose: when a tool is held it adopts the bent CARRY pose
        // (the tool seats in this hand, so the held mesh's grip is derived from
        // the same carry rotation in `held_item_hand_transform`); otherwise it
        // does the normal empty-handed counter-swing. A small bob keeps the
        // carry alive while walking. A swing overrides this for its duration.
        let mut torso_twist = 0.0;
        let (rest_right_arm, rest_right_elbow) = if holding {
            let bob = (phase * 0.5).sin() * 0.04 * walk_blend;
            (
                // Aim pitch (body-frame X, pre-multiplied) raises the whole arm so
                // the held item points at the peer's look; it sums with the torso
                // lean about the same axis to the full pitch. The swing delta
                // below composes on top of this aimed rest.
                Quat::from_rotation_x(arm_aim_pitch)
                    * carry_upper_arm_rotation()
                    * Quat::from_rotation_x(bob),
                carry_forearm_rotation(),
            )
        } else {
            (
                Quat::from_rotation_x(arm_r),
                Quat::from_rotation_x(-ELBOW_REST_BEND - arm_r.max(0.0) * 0.3),
            )
        };
        // The held item hangs off the hand anchor; it stays at its identity rest
        // rotation except for the spear-thrust lock computed inside the swing.
        let mut hand_anchor_rot = Quat::IDENTITY;
        let next_swing = match swing {
            Some(mut active) => {
                active.elapsed += dt;
                if active.elapsed >= active.duration {
                    set_rot(&mut parts, upper_arm_r, rest_right_arm);
                    set_rot(&mut parts, forearm_r, rest_right_elbow);
                    None
                } else {
                    let phase01 = (active.elapsed / active.duration).clamp(0.0, 1.0);
                    let pose = remote_swing_arm_pose(active.model, phase01);
                    torso_twist = pose.torso_twist;
                    // The pose is a DELTA on the rest pose (the bent carry pose
                    // when holding a tool, the straight pose otherwise): the
                    // shoulder delta in the body frame (pre-multiplied) raises /
                    // drives the whole arm, the elbow delta in the forearm's
                    // local frame (post-multiplied) flexes it. So the chop winds
                    // up and strikes from the carry and settles back into it.
                    let shoulder_delta = Quat::from_rotation_x(pose.shoulder_pitch)
                        * Quat::from_rotation_y(pose.shoulder_yaw)
                        * Quat::from_rotation_z(pose.shoulder_roll);
                    let swung_arm = shoulder_delta * rest_right_arm;
                    let swung_elbow = rest_right_elbow * Quat::from_rotation_x(pose.forearm_pitch);
                    set_rot(&mut parts, upper_arm_r, swung_arm);
                    set_rot(&mut parts, forearm_r, swung_elbow);
                    // Spear thrust lock: on a rotation-only rig the elbow
                    // fold/extend sweeps the long shaft through a big arc, so
                    // from a peer's view the thrust read as a swing no matter
                    // how the arm curves were shaped. Counter-rotate the hand
                    // anchor so the spear KEEPS its carry orientation for the
                    // whole swing: the shaft stays couched and level while the
                    // hand translation drives it straight forward, which is
                    // exactly a stab. Other archetypes want the tool to swing
                    // with the arm, so they keep the identity anchor.
                    if active.model == ItemModel::Spear {
                        let rest_chain = rest_right_arm * rest_right_elbow;
                        hand_anchor_rot = (swung_arm * swung_elbow).inverse() * rest_chain;
                    }
                    Some(active)
                }
            }
            None => {
                set_rot(&mut parts, upper_arm_r, rest_right_arm);
                set_rot(&mut parts, forearm_r, rest_right_elbow);
                None
            }
        };
        set_rot(&mut parts, hand_anchor, hand_anchor_rot);
        rig.swing = next_swing;

        // Upper body leans to the peer's look pitch and twists into a swing,
        // pivoting at the hips so the pelvis stays over the legs.
        let (body_rot, body_translation) = remote_body_pose(torso_pitch, torso_twist);
        if let Ok(mut transform) = parts.get_mut(body) {
            transform.rotation = body_rot;
            transform.translation = body_translation;
        }
    }
}

/// Animate the *charging* held items on a peer's rig: the drawn bow and the
/// bandage being wrapped.
///
/// Both are multi-primitive glbs whose layers are tagged with a
/// [`RemoteHeldPiece`] slot, and both are driven by the one replicated
/// [`RemoteLocomotion::charge_fraction`] the server computes. This composes the
/// SAME per-piece transform the first-person viewmodel uses on top of the
/// whole-item grip, so peers see exactly the motion the owner sees: the bow's
/// limbs bend and its string pulls into a deep V, and the bandage's tail unrolls
/// out of its roll.
///
/// Seeing this matters tactically, which is the whole reason the fraction is
/// replicated at all: a drawn bow tells you an arrow is coming, and someone
/// mid-bandage is slowed, committed, and worth rushing.
///
/// Every other held item keeps the static grip it was spawned with in
/// `apply_remote_player_appearance_system` (its per-piece transform is identity),
/// and a dead body's item is left frozen wherever it was.
pub(crate) fn animate_remote_held_charge_system(
    time: Res<Time>,
    grip_sockets: Res<HeldGripSockets>,
    rigs: Query<(
        &PlayerRig,
        &RemoteHeld,
        &RemoteLocomotion,
        Option<&DyingPlayer>,
    )>,
    mut layers: Query<(&RemoteHeldPiece, &mut Transform)>,
) {
    let t = time.elapsed_secs();
    for (rig, held, loco, dying) in &rigs {
        if dying.is_some() {
            continue;
        }
        // Only the bow and the bandage carry a replicated charge.
        let Some(model) = held.0.and_then(|mesh| match mesh {
            HeldMesh::WoodenBow => Some(ItemModel::Bow),
            HeldMesh::Bandage => Some(ItemModel::Bandage),
            _ => None,
        }) else {
            continue;
        };
        let Some(mesh) = held.0 else { continue };

        let grip = held_item_hand_transform(mesh, grip_sockets.get(mesh));
        let charge = loco.charge_fraction.clamp(0.0, 1.0);
        let active = charge > 1e-3;
        let pose = RangedPoseInputs {
            draw_fraction: charge,
            drawing: active,
            // Not drawing => fully settled at rest (the release flick is a
            // first-person nicety we skip for peers); the pose falls back to the
            // rest bow.
            release_progress: if active { 0.0 } else { 1.0 },
            reload_fraction: 1.0,
            recoil: 0.0,
            aim: 0.0,
            throw_wind_up: 0.0,
            use_fraction: charge,
            // Peers get no settle animation: the server stops reporting a charge
            // the instant the use ends, so `use_settle: 1.0` (fully at rest) makes
            // the tail roll straight back up. Reproducing the owner's ease-out
            // would need the *end* event replicated too, which is not worth a
            // component for a quarter-second of polish on someone else's hands.
            use_settle: 1.0,
            use_ended_at: 0.0,
            time_seconds: t,
        };
        for &layer in &rig.held_layers {
            if let Ok((piece, mut transform)) = layers.get_mut(layer) {
                *transform = grip * held_piece_local_transform(model, piece.0, pose);
            }
        }
    }
}

/// Smoothstep on a 0..1 value.
fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Thigh swing amplitude (radians) for a given horizontal speed: zero below the
/// idle threshold, ramping to `LEG_SWING_WALK` at walk speed and on toward
/// `LEG_SWING_RUN` at run speed.
fn locomotion_leg_amplitude(speed: f32) -> f32 {
    if speed <= LOCOMOTION_MOVE_THRESHOLD {
        return 0.0;
    }
    let walk_blend = smooth01((speed / LOCOMOTION_WALK_SPEED).clamp(0.0, 1.0));
    let run_t = ((speed - LOCOMOTION_WALK_SPEED) / (LOCOMOTION_RUN_SPEED - LOCOMOTION_WALK_SPEED))
        .clamp(0.0, 1.0);
    LEG_SWING_WALK * walk_blend + (LEG_SWING_RUN - LEG_SWING_WALK) * run_t
}

/// Write a part's local rotation, tolerating a missing entity (e.g. mid-despawn).
fn set_rot(parts: &mut Query<&mut Transform, With<PlayerPart>>, entity: Entity, rotation: Quat) {
    if let Ok(mut transform) = parts.get_mut(entity) {
        transform.rotation = rotation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_body_pose_leans_to_look_pitch_about_the_hip() {
        // Rest: identity rotation, no translation offset.
        let (rot, tr) = remote_body_pose(0.0, 0.0);
        assert!(rot.angle_between(Quat::IDENTITY) < 1e-5);
        assert!(
            tr.length() < 1e-6,
            "rest torso must sit at the origin: {tr:?}"
        );

        // Looking up (+pitch): the body's forward (-Z) tilts UP (+Y), matching the
        // first-person camera's `from_euler(YXZ, yaw, pitch, 0)` sign.
        let (up_rot, up_tr) = remote_body_pose(0.5, 0.0);
        let forward = up_rot * Vec3::NEG_Z;
        assert!(
            forward.y > 0.0,
            "positive pitch should look up: {forward:?}"
        );

        // Hip pivot is preserved: the pivot point maps back to itself, so the
        // pelvis holds its place over the legs instead of swinging off the origin.
        let pivot = Vec3::new(0.0, REMOTE_TORSO_PITCH_PIVOT_Y, 0.0);
        let mapped = up_tr + up_rot * pivot;
        assert!(
            (mapped - pivot).length() < 1e-5,
            "hip pivot moved: {mapped:?}"
        );
    }

    #[test]
    fn remote_head_anchor_tracks_the_leaned_head() {
        // At rest the anchor sits straight up the Y axis at the head top, matching
        // the pre-lean overlay position (so nothing shifts when nobody is aiming).
        let rest = remote_head_anchor_local(0.0);
        assert!((rest - Vec3::new(0.0, PLAYER_HEAD_TOP_LOCAL_Y, 0.0)).length() < 1e-5);

        // Looking up (+pitch) leans the torso back, so the head top swings toward
        // the archer's back (+Z) and drops a little; the anchor must follow it off
        // the Y axis, otherwise the nametag detaches from the head.
        let up = remote_head_anchor_local(1.2);
        assert!(
            up.z > 0.01,
            "head should swing back (+Z) when looking up: {up:?}"
        );
        assert!(
            up.y < PLAYER_HEAD_TOP_LOCAL_Y,
            "head top drops as it leans: {up:?}"
        );
    }

    #[test]
    fn torso_lean_and_arm_aim_sum_to_the_full_look_pitch() {
        // The held item must point exactly where the peer looks: the torso bends a
        // fraction and the held arm makes up the rest on the SAME axis (the peer's
        // right), so the two X-rotations compose to the full look pitch.
        let look = 0.9_f32;
        let torso = look * REMOTE_TORSO_PITCH_FRACTION;
        let arm = look * (1.0 - REMOTE_TORSO_PITCH_FRACTION);
        let composed = Quat::from_rotation_x(torso) * Quat::from_rotation_x(arm);
        let full = Quat::from_rotation_x(look);
        assert!(
            composed.angle_between(full) < 1e-5,
            "torso + arm aim must reconstruct the full look pitch"
        );
    }

    #[test]
    fn locomotion_amplitude_is_zero_when_idle_and_ramps_with_speed() {
        // Idle and sub-threshold creep produce no leg swing.
        assert_eq!(locomotion_leg_amplitude(0.0), 0.0);
        assert_eq!(locomotion_leg_amplitude(LOCOMOTION_MOVE_THRESHOLD), 0.0);

        // A walk reaches roughly the walk amplitude; a run exceeds it.
        let walk = locomotion_leg_amplitude(LOCOMOTION_WALK_SPEED);
        let run = locomotion_leg_amplitude(LOCOMOTION_RUN_SPEED);
        assert!((walk - LEG_SWING_WALK).abs() < 1e-3);
        assert!((run - LEG_SWING_RUN).abs() < 1e-3);

        // Monotonic non-decreasing across the range.
        let mut last = -1.0;
        for step in 0..=20 {
            let amp = locomotion_leg_amplitude(step as f32 * 0.4);
            assert!(amp + 1e-4 >= last, "amplitude should not decrease");
            last = amp;
        }
    }
}
