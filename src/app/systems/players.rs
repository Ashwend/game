use std::collections::{HashMap, HashSet};

use bevy::{ecs::change_detection::Ref, light::NotShadowCaster, prelude::*};

use crate::{
    app::PLAYER_VISUAL_CENTER_Y,
    items::{ArmorJoint, ArmorMesh, HeldMesh, ItemModel},
    protocol::ClientId,
    server::{
        Player, PlayerAction, PlayerEquipmentVisual, PlayerHeldItem, PlayerLifecycle, PlayerPose,
        PlayerSleeping,
    },
};

use super::super::{
    scene::{NetworkPlayer, PlayerPart, PlayerVisualAssets, player_visual_position, rig_layout},
    state::{ClientRuntime, swing_duration_seconds},
};
use super::items::{
    ArmorVisuals, HeldItemVisuals, armor_layers, carry_forearm_rotation, carry_upper_arm_rotation,
    held_item_hand_transform, held_item_layers, insert_held_layer_material, remote_swing_arm_pose,
};

const REMOTE_PLAYER_INTERPOLATION_SECONDS: f32 = 0.1;
const REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE: f32 = 6.0;

/// Player death animation tuning. Without a rigged skeleton we can't
/// do a true ragdoll, but the per-axis breakdown below gives a much
/// more "physical" collapse than a flat tilt:
///
///   1. Brief upward kick (`UPWARD_KICK_S`), the body lurches off
///      its feet at the moment of death.
///   2. Pivoted fall (`FALL_DURATION_S`), the rotation happens
///      around the feet pivot, with a small overshoot at impact so
///      the body bounces once it hits the ground.
///   3. Settle hold (`HOLD_DURATION_S`).
///   4. Fade alpha to 0 over `FADE_DURATION_S` and hide.
///
/// Random per-spawn yaw twist + roll keep the corpse from always
/// landing in the same exact pose; without it, a fight in one spot
/// stacks visually-identical bodies.
const DEATH_UPWARD_KICK_S: f32 = 0.12;
const DEATH_FALL_DURATION_S: f32 = 0.65;
const DEATH_HOLD_DURATION_S: f32 = 0.4;
const DEATH_FADE_DURATION_S: f32 = 0.9;
/// Fall arc, past 90° so the body face-plants into the floor
/// instead of standing rigid on its head.
const DEATH_FALL_ANGLE_RAD: f32 = std::f32::consts::FRAC_PI_2 + 0.12;
/// How far the body lifts during the initial death kick. Tiny, just
/// enough to see the corpse shudder off its feet.
const DEATH_UPWARD_KICK_M: f32 = 0.08;
/// Bounce magnitude at the moment of impact, in radians. Soft
/// overshoot that decays out before the fade phase starts.
const DEATH_IMPACT_BOUNCE_RAD: f32 = 0.10;

/// Per-entity death state. Stamped onto a `NetworkPlayer` the first
/// time we see its `PlayerLifecycle::Dead`. The tick system below
/// drives the tilt + fade off this; on respawn the component is
/// removed and the regular interpolation resumes.
#[derive(Component, Debug, Clone)]
pub(crate) struct DyingPlayer {
    elapsed: f32,
    /// Set once the fade has fully played out. The component is kept
    /// (not removed) so a later respawn can still find it and restore
    /// the avatar; `tick_dying_players_system` skips finished corpses so
    /// they stay frozen and hidden until then.
    finished: bool,
    /// True when the body was a logged-out sleeper at the moment of death.
    /// It's already lying on the ground, so the collapse animation is
    /// skipped, the corpse just holds its lying pose and fades out.
    from_sleep: bool,
    /// World-space rotation axis the body falls around. Picked once
    /// at death time so the corpse picks a direction and sticks with
    /// it. Horizontal, the rotation tips the body forward, not
    /// yaws it on the spot.
    fall_axis: Vec3,
    /// Small per-spawn roll (radians) so the body lands at a
    /// slightly tilted angle instead of square-flat, reads as a
    /// limp collapse rather than a folded mannequin.
    roll_axis: Vec3,
    roll_magnitude: f32,
    /// Snapshot of the world transform at death, used as the rest
    /// pose the tilt is layered on top of. The interpolator stops
    /// re-targeting once the entity is dying, so the dead avatar
    /// doesn't keep sliding to wherever the server-side controller
    /// would have walked.
    rest: Transform,
    /// Cloned material handle so this dying avatar can fade its
    /// alpha without dragging every other remote player along with
    /// it. Set once on death; the tick system mutates this handle's
    /// `base_color.alpha`.
    material: Handle<StandardMaterial>,
}

/// Marks a remote body as a logged-out "sleeping" body. Stamped while the
/// replicated `PlayerSleeping` is set; the body is laid into a static
/// lying-down pose and frozen (the interpolator is parked at its upright
/// target) until the owner reconnects and the flag clears.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct SleepingPlayer;

/// Roughly half the body's depth once it's lying on its back. The supine avatar
/// rests this far off the ground; we lift the pose by it so the mesh sits on top
/// of the floor instead of sinking its back through it.
const SLEEPING_GROUND_CLEARANCE: f32 = 0.1;

/// World-space transform that lays an upright body flat on its back, face to the
/// sky, held statically for a sleeper. The mesh's local frame is +Y up and -Z
/// forward (the visor/nose), so pitching it backward 90 degrees about its own
/// right axis (local X) drops the face toward the sky while keeping the body's
/// compass facing, giving a consistent supine pose for every observer. (The
/// death collapse, by contrast, tilts in a per-client random direction for a
/// limp face-plant.)
fn lying_transform(upright: Transform) -> Transform {
    let tilt = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    let rotation = upright.rotation * tilt;
    // Rotate around the feet pivot, not the visual centre, so the body lies on
    // the ground instead of pivoting through it, then lift it by its
    // half-depth so the back rests on top of the floor rather than clipping in.
    let pivot_offset = Vec3::Y * PLAYER_VISUAL_CENTER_Y;
    let rotated_offset = rotation * pivot_offset;
    Transform::from_translation(
        upright.translation - pivot_offset + rotated_offset + Vec3::Y * SLEEPING_GROUND_CLEARANCE,
    )
    .with_rotation(rotation)
}

/// Persistent `client_id → Entity` map for remote players. Mirrors the live
/// entity set so the reconciliation system doesn't have to rebuild it from
/// a `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct RemotePlayerEntities(pub(crate) std::collections::HashMap<ClientId, Entity>);

/// Reconciles the set of visual `NetworkPlayer` entities against the
/// Lightyear-replicated `(Player, PlayerPose)` entities. Spawn,
/// despawn, and interpolation re-target all flow off the replicated
/// query, one visual entity per replicated entity.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn apply_snapshot_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    assets: Res<PlayerVisualAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut entities: ResMut<RemotePlayerEntities>,
    mut players: Query<
        (
            &Transform,
            &mut NetworkPlayerInterpolation,
            &mut RemoteLocomotion,
            &mut RemoteHeld,
            &mut RemoteEquipment,
            &mut RemoteAction,
            Option<&DyingPlayer>,
            Option<&SleepingPlayer>,
        ),
        With<NetworkPlayer>,
    >,
    replicated: Query<(
        &Player,
        Ref<PlayerPose>,
        Option<&PlayerLifecycle>,
        Option<&PlayerSleeping>,
        Option<&PlayerHeldItem>,
        Option<&PlayerEquipmentVisual>,
        Option<&PlayerAction>,
    )>,
) {
    let Some(local_client_id) = runtime.client_id else {
        // Not connected, tear down any remote visuals from a prior
        // session.
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    };

    // `last_changed().get()` advances every time the replicated
    // `PlayerPose` is mutated, so the interpolator only re-targets
    // when there's a real update (Phase 5's tick-as-change-tick trick).
    let mut visible_ids = HashSet::new();
    let entities = &mut *entities;

    for (player, pose, lifecycle, sleeping, held, equipment, action) in &replicated {
        if player.client_id == local_client_id {
            continue;
        }
        let is_dead = matches!(lifecycle, Some(PlayerLifecycle::Dead { .. }));
        let is_sleeping = matches!(sleeping, Some(PlayerSleeping(true)));
        // Cosmetic peer state for the rig animators: held mesh, worn armor,
        // current swing, and horizontal speed / grounded derived from the
        // replicated pose.
        let held_mesh = held.and_then(|held| held.0);
        let equipment_visual = equipment
            .copied()
            .map(remote_equipment_from)
            .unwrap_or_default();
        let (action_seq, action_model) = action
            .map(|action| (action.seq, action.model))
            .unwrap_or((0, ItemModel::Bag));
        let velocity = Vec3::from(pose.velocity);
        let horizontal_speed = velocity.with_y(0.0).length();
        // Keep the entity around even while dead so the tilt-and-fade
        // animation can finish playing. The tick system below
        // despawns the visual once the fade completes.
        visible_ids.insert(player.client_id);
        let tick = pose.last_changed().get() as u64;
        let feet = Vec3::from(pose.position);
        let target = Transform::from_translation(player_visual_position(feet))
            .with_rotation(Quat::from_rotation_y(pose.yaw));
        if let Some(entity) = entities.0.get(&player.client_id).copied() {
            if let Ok((
                current,
                mut interpolation,
                mut loco,
                mut held_comp,
                mut equipment_comp,
                mut action_comp,
                dying,
                asleep,
            )) = players.get_mut(entity)
            {
                if is_dead && dying.is_none() {
                    // Just-died this frame, stamp the dying state so
                    // the tick system below takes over the transform.
                    let (fall_axis, roll_axis, roll_magnitude) =
                        compute_fall_axes(player.client_id, *current);
                    // Clone the source material into a fresh handle
                    // so this dying avatar can fade its alpha without
                    // dragging every other remote player along. Read
                    // the source first, then drop the borrow before
                    // calling `add()`.
                    let source = materials
                        .get(&assets.remote_material)
                        .cloned()
                        .unwrap_or_default();
                    let cloned_material = materials.add(StandardMaterial {
                        alpha_mode: AlphaMode::Blend,
                        ..source
                    });
                    // The root carries no mesh (the rig parts do); the
                    // corpse-material repoint onto the parts happens in
                    // `apply_remote_player_appearance_system`, which reads this
                    // cloned handle off `DyingPlayer`.
                    commands.entity(entity).insert(DyingPlayer {
                        elapsed: 0.0,
                        finished: false,
                        // A sleeper killed in place is already on the
                        // ground; skip the collapse and just fade.
                        from_sleep: is_sleeping,
                        fall_axis,
                        roll_axis,
                        roll_magnitude,
                        rest: *current,
                        material: cloned_material,
                    });
                }
                // Dead -> Alive this frame: the player respawned. True
                // whether the death animation was still playing or had
                // already faded out (we keep `DyingPlayer` around until
                // now precisely so this transition is always detectable).
                let just_respawned = !is_dead && dying.is_some();
                if just_respawned {
                    // Drop the dying state and restore full visibility. The
                    // appearance system repoints the rig parts back to the
                    // shared (opaque) material once `DyingPlayer` is gone.
                    commands
                        .entity(entity)
                        .remove::<DyingPlayer>()
                        .insert(Visibility::Visible);
                }
                // Live players follow the interpolator. Dying
                // players' transforms are owned by the death-tick
                // system; the interpolator stays frozen at the rest
                // pose so a stray late-arriving movement update
                // can't slide a corpse.
                if !is_dead {
                    // Feed the rig animators (read off the visual entity so they
                    // don't need to re-join the replicated entity). Cheap small
                    // writes; the animators read these by value each frame.
                    loco.speed = horizontal_speed;
                    loco.grounded = pose.grounded;
                    held_comp.0 = held_mesh;
                    // Worn-armor mirror: manual edge detection against the local
                    // value (never `Ref::is_changed`, which lies for
                    // Lightyear-touched components). Rig rendering off this is
                    // Phase 4; today the write just keeps the local handle current.
                    if *equipment_comp != equipment_visual {
                        *equipment_comp = equipment_visual;
                    }
                    action_comp.seq = action_seq;
                    action_comp.model = action_model;
                    // Track the sleep<->awake transition so the marker, the
                    // pose, and the interpolation stay in sync.
                    let woke = asleep.is_some() && !is_sleeping;
                    if is_sleeping && asleep.is_none() {
                        commands.entity(entity).insert(SleepingPlayer);
                    } else if woke {
                        commands.entity(entity).remove::<SleepingPlayer>();
                    }

                    if is_sleeping {
                        // Logged-out body: hold a static lying-down pose and
                        // freeze the interpolator at the upright target so the
                        // body stands straight back up when it wakes.
                        interpolation.snap_to(tick, target);
                        commands.entity(entity).insert(lying_transform(target));
                    } else {
                        if just_respawned || woke {
                            // A respawn or a wake is a teleport, not a walk.
                            // Hard-snap instead of blending: when the new pose
                            // lands within interpolation range of the old one,
                            // `retarget` would otherwise slide the avatar across
                            // for a few frames (the "flicker before disappearing
                            // to spawn" the killer sees, and the slow rise from
                            // a lying pose on wake).
                            interpolation.snap_to(tick, target);
                        } else {
                            interpolation.retarget(tick, current, target);
                        }
                        let transform = interpolation.advance(time.delta_secs());
                        commands.entity(entity).insert(transform);
                    }
                }
            }
        } else if !is_dead {
            // Don't spawn a fresh visual just to immediately mark it
            // dying, if we somehow see a player whose first sight is
            // already Dead (rare; would only happen on AoI cross-in
            // mid-death-anim), we let them stay invisible until the
            // server sends Alive again.
            // A body first seen while sleeping (AoI cross-in onto a logged-out
            // sleeper) spawns already lying down, so it never flashes upright
            // for a frame. The interpolator is still parked at the upright
            // target so it stands up cleanly on wake.
            let spawn_transform = if is_sleeping {
                lying_transform(target)
            } else {
                target
            };
            // The root is a transform-only node: the visible body is the rig of
            // child part entities built by `reconcile_player_rigs_system` from
            // the `Added<NetworkPlayer>` edge. The cosmetic mirror components
            // seed the animators with current state so an AoI cross-in shows
            // the right held item / locomotion immediately.
            let entity = commands
                .spawn((
                    Name::new(format!("Player {}", player.client_id)),
                    NetworkPlayer {
                        client_id: player.client_id,
                    },
                    NetworkPlayerInterpolation::new(tick, target),
                    RemoteLocomotion {
                        speed: horizontal_speed,
                        grounded: pose.grounded,
                    },
                    RemoteHeld(held_mesh),
                    equipment_visual,
                    RemoteAction {
                        seq: action_seq,
                        model: action_model,
                    },
                    spawn_transform,
                    Visibility::Visible,
                ))
                .id();
            if is_sleeping {
                commands.entity(entity).insert(SleepingPlayer);
            }
            entities.0.insert(player.client_id, entity);
        }
    }

    entities.0.retain(|id, entity| {
        if visible_ids.contains(id) {
            true
        } else {
            commands.entity(*entity).despawn();
            false
        }
    });
}

/// Pick the fall + roll axes the death animation will rotate around.
/// Hashes the client id so a corpse always falls the same way for
/// every observer (deterministic across clients) but two adjacent
/// deaths don't collapse onto the same orientation.
///
/// Returns:
///   - `fall_axis`: horizontal axis the body tips forward over.
///   - `roll_axis`: the same axis crossed with the fall direction,
///     the body's "long axis" once it lands. Small magnitude roll
///     around this axis gives the corpse a sideways lean.
///   - `roll_magnitude`: signed roll in radians.
fn compute_fall_axes(client_id: ClientId, current: Transform) -> (Vec3, Vec3, f32) {
    let seed = client_id.wrapping_mul(0x9E3779B97F4A7C15);
    let upper = (seed >> 32) as u32;
    let lower = (seed & 0xFFFF_FFFF) as u32;
    let angle = (upper as f32 / u32::MAX as f32) * std::f32::consts::TAU;
    let roll = ((lower as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.45;

    let local_forward = Vec3::new(angle.cos(), 0.0, angle.sin());
    // Rotate the random vector into the avatar's facing so the corpse
    // doesn't fall perpendicular to the way they were running.
    let direction = current.rotation * local_forward;
    let horizontal = Vec3::new(direction.x, 0.0, direction.z).normalize_or_zero();
    let fall_axis = horizontal.cross(Vec3::Y).normalize_or_zero();
    let fall_axis = if fall_axis.length_squared() < f32::EPSILON {
        Vec3::X
    } else {
        fall_axis
    };
    // Roll axis = the direction of the fall (perpendicular to
    // fall_axis, horizontal) so the body twists around the long
    // axis it ends up lying along.
    let roll_axis = horizontal;
    let roll_axis = if roll_axis.length_squared() < f32::EPSILON {
        Vec3::Z
    } else {
        roll_axis
    };
    (fall_axis, roll_axis, roll)
}

/// Advance every dying player's animation. Multi-phase sequence so
/// the body collapses through a recognisable arc:
///
/// 1. **Kick** (`DEATH_UPWARD_KICK_S`): tiny vertical lurch off the
///    feet, reads as "they just took a fatal hit".
/// 2. **Fall** (`DEATH_FALL_DURATION_S`): pivot around the feet from
///    upright to flat. Ease-in so the body accelerates as gravity
///    takes over.
/// 3. **Bounce**: small overshoot at impact that decays back to the
///    rest pose. Without it the body slams into the ground stiff;
///    with it the corpse settles like flesh.
/// 4. **Hold** (`DEATH_HOLD_DURATION_S`): pause so the player sees
///    where the body landed before it disappears.
/// 5. **Fade** (`DEATH_FADE_DURATION_S`): alpha 1 → 0, then hide.
///
/// The entity itself isn't despawned, it might respawn (the
/// lifecycle goes Alive → DyingPlayer removed → MeshMaterial3d
/// restored upstream), and recreating the mesh + material pair every
/// kill would churn allocations.
pub(crate) fn tick_dying_players_system(
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut players: Query<(&mut Transform, &mut DyingPlayer, &mut Visibility)>,
) {
    let dt = time.delta_secs().max(0.0);
    for (mut transform, mut dying, mut visibility) in &mut players {
        if dying.finished {
            // Fully faded already. Hold it hidden and frozen; the
            // component stays so `apply_snapshot_system` can restore the
            // avatar if the player respawns, or despawn it on AoI exit.
            continue;
        }
        dying.elapsed += dt;

        let fade_t = if dying.from_sleep {
            // Killed in its sleep: the body is already lying on the ground, so
            // there's no collapse to play. Hold the resting (lying) pose and
            // fade out from the moment of death.
            transform.translation = dying.rest.translation;
            transform.rotation = dying.rest.rotation;
            (dying.elapsed / DEATH_FADE_DURATION_S).clamp(0.0, 1.0)
        } else {
            // --- Phase 1+2: kick + fall, blended ---
            let kick_t = (dying.elapsed / DEATH_UPWARD_KICK_S).clamp(0.0, 1.0);
            // Half-sine pulse so the kick rises and settles inside its
            // own window without a hard step.
            let kick_pulse = (kick_t * std::f32::consts::PI).sin();
            let kick_y = DEATH_UPWARD_KICK_M * kick_pulse;

            let fall_t = (dying.elapsed / DEATH_FALL_DURATION_S).clamp(0.0, 1.0);
            // Ease-in cubic so the body hangs briefly before
            // accelerating downward, reads like "weight gives out".
            let fall_eased = fall_t * fall_t * fall_t;

            // Bounce: a damped sine pulse layered on top of the fall
            // angle once the body's mostly down. Peaks just past
            // impact and decays smoothly.
            let bounce_t = (dying.elapsed - DEATH_FALL_DURATION_S * 0.85).max(0.0) / 0.35;
            let bounce_pulse = if bounce_t > 0.0 && bounce_t < 1.0 {
                (bounce_t * std::f32::consts::PI).sin() * (1.0 - bounce_t)
            } else {
                0.0
            };
            let fall_angle =
                DEATH_FALL_ANGLE_RAD * fall_eased - DEATH_IMPACT_BOUNCE_RAD * bounce_pulse;

            // Roll ramps in alongside the fall so the body twists onto
            // its side while it tips over rather than snapping into the
            // tilt mid-fall.
            let roll_angle = dying.roll_magnitude * fall_eased;

            let tilt = Quat::from_axis_angle(dying.fall_axis, fall_angle);
            let roll = Quat::from_axis_angle(dying.roll_axis, roll_angle);
            let new_rotation = roll * tilt * dying.rest.rotation;

            // Rotate around the feet pivot, not the visual centre. The
            // visual entity sits at `rest.translation`, which is
            // `PLAYER_VISUAL_CENTER_Y` above the feet. To pivot at the
            // feet we offset by that amount, rotate, then offset back.
            let pivot_offset = Vec3::Y * PLAYER_VISUAL_CENTER_Y;
            let rotated_offset = roll * tilt * pivot_offset;
            transform.rotation = new_rotation;
            transform.translation =
                dying.rest.translation - pivot_offset + rotated_offset + Vec3::Y * kick_y;

            // --- Phase 5: fade alpha ---
            let fade_start = DEATH_FALL_DURATION_S + DEATH_HOLD_DURATION_S;
            ((dying.elapsed - fade_start) / DEATH_FADE_DURATION_S).clamp(0.0, 1.0)
        };
        let alpha = (1.0 - fade_t).clamp(0.0, 1.0);
        if let Some(material) = materials.get_mut(&dying.material) {
            material.base_color = material.base_color.with_alpha(alpha);
        }

        if fade_t >= 1.0 {
            // Fully gone. Hide it and mark the corpse finished, but keep
            // the `DyingPlayer` component so `apply_snapshot_system` can
            // re-show the avatar on respawn (without re-spawning a fresh
            // `NetworkPlayer`). Removing it here would strand a
            // late-respawning player invisible.
            *visibility = Visibility::Hidden;
            dying.finished = true;
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct NetworkPlayerInterpolation {
    snapshot_tick: u64,
    from: Transform,
    to: Transform,
    elapsed: f32,
}

impl NetworkPlayerInterpolation {
    fn new(snapshot_tick: u64, transform: Transform) -> Self {
        Self {
            snapshot_tick,
            from: transform,
            to: transform,
            elapsed: REMOTE_PLAYER_INTERPOLATION_SECONDS,
        }
    }

    /// Hard-snap the interpolation onto `target` with no blend, and mark
    /// it fully advanced so `advance` returns `target` immediately. Used
    /// for respawn, which teleports the avatar: blending would slide it
    /// across the world (or briefly hold it at the old death spot) for a
    /// few frames.
    fn snap_to(&mut self, snapshot_tick: u64, target: Transform) {
        self.from = target;
        self.to = target;
        self.elapsed = REMOTE_PLAYER_INTERPOLATION_SECONDS;
        self.snapshot_tick = snapshot_tick;
    }

    fn retarget(&mut self, snapshot_tick: u64, current: &Transform, target: Transform) {
        if snapshot_tick <= self.snapshot_tick {
            return;
        }

        let distance = current.translation.distance(target.translation);
        self.from = if distance > REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE {
            target
        } else {
            *current
        };
        self.to = target;
        self.elapsed = 0.0;
        self.snapshot_tick = snapshot_tick;
    }

    fn advance(&mut self, delta_seconds: f32) -> Transform {
        self.elapsed += delta_seconds.max(0.0);
        let alpha = (self.elapsed / REMOTE_PLAYER_INTERPOLATION_SECONDS).clamp(0.0, 1.0);
        Transform::from_translation(self.from.translation.lerp(self.to.translation, alpha))
            .with_rotation(self.from.rotation.slerp(self.to.rotation, alpha))
    }
}

// ---------------------------------------------------------------------------
// Rigged remote body: a hierarchy of part child entities under the root,
// animated procedurally from the replicated pose + held item + swing action.
// The root stays the interpolation target; only the part LOCAL rotations are
// animated here, so they compose with the interpolated/collapsing root.
// ---------------------------------------------------------------------------

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

/// Local mirror of the replicated pose's movement, written onto the visual
/// NetworkPlayer by `apply_snapshot_system` so the locomotion animator never
/// re-joins the replicated entity.
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteLocomotion {
    pub(crate) speed: f32,
    pub(crate) grounded: bool,
}

/// Local mirror of the replicated `PlayerHeldItem`.
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteHeld(pub(crate) Option<HeldMesh>);

/// Local mirror of the replicated `PlayerEquipmentVisual`: the four worn-armor
/// mesh selectors for a remote body. Copied off the replicated component by
/// `apply_snapshot_system` with manual edge detection (never `Ref::is_changed`,
/// which lies for Lightyear-touched components). No rig rendering consumes it
/// yet, that is Phase 4; landing the mirror now keeps the wire path exercised
/// and gives the future rig-attachment system a local handle to read.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RemoteEquipment {
    pub(crate) head: Option<ArmorMesh>,
    pub(crate) chest: Option<ArmorMesh>,
    pub(crate) legs: Option<ArmorMesh>,
    pub(crate) feet: Option<ArmorMesh>,
}

/// Copy a replicated `PlayerEquipmentVisual` into the local `RemoteEquipment`
/// mirror. A plain field copy; kept as a named helper so both the spawn and the
/// per-frame edge-detected update read the same mapping.
fn remote_equipment_from(visual: PlayerEquipmentVisual) -> RemoteEquipment {
    RemoteEquipment {
        head: visual.head,
        chest: visual.chest,
        legs: visual.legs,
        feet: visual.feet,
    }
}

/// Local mirror of the replicated `PlayerAction` (current swing).
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteAction {
    pub(crate) seq: u32,
    /// The swing archetype the peer is swinging (weapon's own model or a gather
    /// tool's archetype). Drives the third-person swing arc directly off the wire.
    pub(crate) model: ItemModel,
}

/// An in-progress third-person swing on a remote body.
#[derive(Debug, Clone, Copy)]
struct RemoteSwing {
    /// Swing animation archetype: drives the third-person arc and duration. Read
    /// straight from the peer's replicated `PlayerAction.model` (a weapon's own
    /// archetype or a gather tool's), so peers animate the right swing directly
    /// off the wire.
    model: ItemModel,
    elapsed: f32,
    duration: f32,
}

/// Handles to a remote player's rig part entities plus its animation state.
/// Lives on the root NetworkPlayer entity, attached by
/// `reconcile_player_rigs_system`.
#[derive(Component)]
pub(crate) struct PlayerRig {
    body: Entity,
    upper_arm_l: Entity,
    upper_arm_r: Entity,
    forearm_l: Entity,
    forearm_r: Entity,
    hand_anchor: Entity,
    thigh_l: Entity,
    thigh_r: Entity,
    shin_l: Entity,
    shin_r: Entity,
    /// Held-tool layer entities parented to the hand anchor (despawned + rebuilt
    /// on a held-item change).
    held_layers: Vec<Entity>,
    /// Last-seen replicated held mesh, for change detection (NOT `is_changed`,
    /// which lies for Lightyear-touched components).
    last_held: Option<HeldMesh>,
    /// Worn-armor layer entities parented to the rig joints (despawned + rebuilt
    /// on an equipment change), across all four slots and both L/R mirrors.
    armor_layers: Vec<Entity>,
    /// Last-seen replicated worn armor, for the same manual edge detection as
    /// `last_held` (NEVER `is_changed`, which lies for Lightyear-touched
    /// components).
    last_equipment: RemoteEquipment,
    /// Last-seen swing seq, for edge detection.
    last_swing_seq: u32,
    swing: Option<RemoteSwing>,
    /// Accumulated walk-cycle phase.
    stride_phase: f32,
    /// True once the rig parts have been repointed to the per-corpse fade
    /// material (so the repoint runs once per death, not every frame).
    corpse_faded: bool,
}

/// The rig joint entity(ies) a worn-armor [`ArmorJoint`] attaches under, per the
/// P4a ART CONTRACT: helmets and chest shells parent to the Body (there is no
/// Head rig part, the head is baked into the Body mesh); the chest's symmetric
/// shoulder aux, the leg shells, and the feet shells attach to BOTH the left and
/// right joint. Returned as a small fixed-capacity list the caller iterates, so
/// a `*Both` joint fans one authored (X-symmetric) mesh out to two child
/// entities with no mirroring transform.
fn armor_joint_entities(rig: &PlayerRig, joint: ArmorJoint) -> Vec<Entity> {
    match joint {
        ArmorJoint::Body => vec![rig.body],
        ArmorJoint::UpperArmsBoth => vec![rig.upper_arm_l, rig.upper_arm_r],
        ArmorJoint::ThighsBoth => vec![rig.thigh_l, rig.thigh_r],
        ArmorJoint::ShinsBoth => vec![rig.shin_l, rig.shin_r],
    }
}

impl PlayerRig {
    /// Mesh-bearing parts (everything but the empty hand anchor), for the
    /// corpse-material repoint.
    fn mesh_parts(&self) -> [Entity; 9] {
        [
            self.body,
            self.upper_arm_l,
            self.upper_arm_r,
            self.forearm_l,
            self.forearm_r,
            self.thigh_l,
            self.thigh_r,
            self.shin_l,
            self.shin_r,
        ]
    }
}

/// Builds the part hierarchy for a freshly-spawned remote player off the
/// `Added<NetworkPlayer>` edge: the root carries no mesh, so this hangs the
/// body/limbs off it and records the part entities in `PlayerRig`. Despawn is
/// automatic, removing the root recursively removes the parts.
pub(crate) fn reconcile_player_rigs_system(
    mut commands: Commands,
    assets: Res<PlayerVisualAssets>,
    new_players: Query<Entity, Added<NetworkPlayer>>,
) {
    for root in &new_players {
        let mut parts: HashMap<PlayerPart, Entity> = HashMap::new();
        for spec in rig_layout() {
            let parent = match spec.parent {
                Some(part) => parts[&part],
                None => root,
            };
            let mut entity =
                commands.spawn((spec.part, spec.rest, Visibility::Inherited, ChildOf(parent)));
            if let Some(kind) = spec.mesh {
                entity.insert((
                    Mesh3d(assets.rig.handle(kind)),
                    MeshMaterial3d(assets.remote_material.clone()),
                ));
            }
            parts.insert(spec.part, entity.id());
        }
        commands.entity(root).insert(PlayerRig {
            body: parts[&PlayerPart::Body],
            upper_arm_l: parts[&PlayerPart::UpperArmL],
            upper_arm_r: parts[&PlayerPart::UpperArmR],
            forearm_l: parts[&PlayerPart::ForearmL],
            forearm_r: parts[&PlayerPart::ForearmR],
            hand_anchor: parts[&PlayerPart::HandAnchor],
            thigh_l: parts[&PlayerPart::ThighL],
            thigh_r: parts[&PlayerPart::ThighR],
            shin_l: parts[&PlayerPart::ShinL],
            shin_r: parts[&PlayerPart::ShinR],
            held_layers: Vec::new(),
            last_held: None,
            armor_layers: Vec::new(),
            last_equipment: RemoteEquipment::default(),
            last_swing_seq: 0,
            swing: None,
            stride_phase: 0.0,
            corpse_faded: false,
        });
    }
}

/// Structural appearance updates: swap the hand-held tool when the replicated
/// held item changes, rebuild the worn-armor layers when the replicated
/// equipment changes, and repoint the rig parts to the per-corpse fade material
/// on death (and back on respawn). Runs only on real changes, so the steady
/// state costs nothing.
pub(crate) fn apply_remote_player_appearance_system(
    mut commands: Commands,
    held_visuals: Res<HeldItemVisuals>,
    armor_visuals: Res<ArmorVisuals>,
    player_assets: Res<PlayerVisualAssets>,
    mut rigs: Query<(
        &mut PlayerRig,
        &RemoteHeld,
        &RemoteEquipment,
        Option<&DyingPlayer>,
    )>,
) {
    for (mut rig, held, equipment, dying) in &mut rigs {
        // Held-item swap.
        if held.0 != rig.last_held {
            rig.last_held = held.0;
            for entity in std::mem::take(&mut rig.held_layers) {
                commands.entity(entity).despawn();
            }
            if let Some(mesh) = held.0 {
                let grip = held_item_hand_transform(mesh);
                let anchor = rig.hand_anchor;
                for held_layer in held_item_layers(&held_visuals, mesh, false) {
                    let mut layer = commands.spawn((
                        Name::new("Held Item (remote)"),
                        Mesh3d(held_layer.mesh),
                        grip,
                        Visibility::Inherited,
                        // Shadow would be noise at this scale; it rides the
                        // swinging arm anyway.
                        NotShadowCaster,
                        ChildOf(anchor),
                    ));
                    insert_held_layer_material(&mut layer, held_layer.material);
                    rig.held_layers.push(layer.id());
                }
            }
        }

        // Worn-armor swap: same manual edge detection as the held item (NEVER
        // `is_changed`, which lies for Lightyear-touched components). On any
        // change to the four worn selectors, tear down every armor layer and
        // rebuild them from the current set, parenting each shell to the joint(s)
        // the ART CONTRACT dictates. A shell is authored pivot-local for identity
        // attach, so the child transform is `IDENTITY`; the `*Both` joints attach
        // the same (X-symmetric) mesh at both the left and right joint.
        if *equipment != rig.last_equipment {
            rig.last_equipment = *equipment;
            for entity in std::mem::take(&mut rig.armor_layers) {
                commands.entity(entity).despawn();
            }
            let worn = [
                equipment.head,
                equipment.chest,
                equipment.legs,
                equipment.feet,
            ];
            for mesh in worn.into_iter().flatten() {
                for layer in armor_layers(&armor_visuals, mesh) {
                    for joint in armor_joint_entities(&rig, layer.joint) {
                        let entity = commands
                            .spawn((
                                Name::new("Armor (remote)"),
                                Mesh3d(layer.mesh.clone()),
                                MeshMaterial3d(layer.material.clone()),
                                // Shells are authored pivot-local for identity
                                // attach at their joint.
                                Transform::IDENTITY,
                                Visibility::Inherited,
                                // The rig itself is a shadow caster; the armor
                                // shells sit flush over the body parts, so their
                                // own shadow would only fight the body's. Match
                                // the held-layer choice and skip the shadow pass.
                                NotShadowCaster,
                                ChildOf(joint),
                            ))
                            .id();
                        rig.armor_layers.push(entity);
                    }
                }
            }
        }

        // Corpse fade material: the parts share the live opaque material, so a
        // death fade needs them repointed onto the per-corpse Blend clone, then
        // back to the shared one on respawn.
        match (dying, rig.corpse_faded) {
            (Some(dying), false) => {
                let material = dying.material.clone();
                for part in rig.mesh_parts() {
                    commands
                        .entity(part)
                        .insert(MeshMaterial3d(material.clone()));
                }
                rig.corpse_faded = true;
            }
            (None, true) => {
                for part in rig.mesh_parts() {
                    commands
                        .entity(part)
                        .insert(MeshMaterial3d(player_assets.remote_material.clone()));
                }
                rig.corpse_faded = false;
            }
            _ => {}
        }
    }
}

/// Procedural locomotion + swing animation for remote bodies. Reads the mirror
/// components written by `apply_snapshot_system` and writes each part's local
/// rotation. Dying bodies freeze (the death tick owns their root transform);
/// sleeping bodies relax to a straight pose.
#[allow(clippy::type_complexity)]
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
        // root transform); don't keep walking it.
        if dying.is_some() {
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
            continue;
        }

        // Swing edge detection (seq, never `is_changed`).
        let mut swing = rig.swing;
        if action.seq > rig.last_swing_seq {
            rig.last_swing_seq = action.seq;
            // The wire `model` is the swing archetype directly (a weapon's own
            // Club/Spear/Sword/Mace, a gather tool's Hatchet/Pickaxe), so a peer
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
                carry_upper_arm_rotation() * Quat::from_rotation_x(bob),
                carry_forearm_rotation(),
            )
        } else {
            (
                Quat::from_rotation_x(arm_r),
                Quat::from_rotation_x(-ELBOW_REST_BEND - arm_r.max(0.0) * 0.3),
            )
        };
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
                    set_rot(&mut parts, upper_arm_r, shoulder_delta * rest_right_arm);
                    set_rot(
                        &mut parts,
                        forearm_r,
                        rest_right_elbow * Quat::from_rotation_x(pose.forearm_pitch),
                    );
                    Some(active)
                }
            }
            None => {
                set_rot(&mut parts, upper_arm_r, rest_right_arm);
                set_rot(&mut parts, forearm_r, rest_right_elbow);
                None
            }
        };
        rig.swing = next_swing;

        // Upper body twists into a swing.
        set_rot(&mut parts, body, Quat::from_rotation_y(torso_twist));
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

    // The system-level spawn/update/despawn cycle now flows off the
    // replicated `(Player, PlayerPublic)` query. Exercising it as a
    // unit test would need the Lightyear replication plugin set up,
    // which is what the integration tests in `src/net/tests.rs`
    // already cover. The interpolation math below is the only piece
    // that stays unit-testable in isolation.

    #[test]
    fn remote_player_interpolation_blends_between_snapshot_targets() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(4.0, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let mut interpolation = NetworkPlayerInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let halfway = interpolation.advance(REMOTE_PLAYER_INTERPOLATION_SECONDS * 0.5);

        assert!((halfway.translation.x - 2.0).abs() < 0.001);
        assert!(halfway.rotation.angle_between(current.rotation) > 0.1);
        assert!(halfway.rotation.angle_between(target.rotation) > 0.1);
    }

    #[test]
    fn remote_player_interpolation_snaps_on_respawn_regardless_of_distance() {
        // A respawn teleport must jump even when the new spawn is well
        // within the blend's snap distance, otherwise the revived avatar
        // slides from the death spot (the killer-side flicker).
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let mut interpolation = NetworkPlayerInterpolation::new(1, current);
        let nearby_spawn = Transform::from_xyz(2.0, 0.0, 0.0);
        assert!(nearby_spawn.translation.length() < REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE);

        interpolation.snap_to(2, nearby_spawn);
        let result = interpolation.advance(REMOTE_PLAYER_INTERPOLATION_SECONDS * 0.1);
        assert_eq!(result.translation, nearby_spawn.translation);
    }

    #[test]
    fn remote_player_interpolation_snaps_extreme_corrections() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let target = Transform::from_xyz(REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE + 1.0, 0.0, 0.0);
        let mut interpolation = NetworkPlayerInterpolation::new(1, current);

        interpolation.retarget(2, &current, target);
        let corrected = interpolation.advance(0.0);

        assert_eq!(corrected.translation, target.translation);
    }

    #[test]
    fn remote_player_interpolation_ignores_stale_ticks() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let mut interpolation = NetworkPlayerInterpolation::new(5, current);
        let target = Transform::from_xyz(3.0, 0.0, 0.0);

        // A tick <= the stored tick is a stale duplicate and must not move
        // the blend off the current pose.
        interpolation.retarget(5, &current, target);
        let after = interpolation.advance(REMOTE_PLAYER_INTERPOLATION_SECONDS);
        assert_eq!(after.translation, current.translation);
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

    #[test]
    fn compute_fall_axes_yields_horizontal_unit_axes() {
        let (fall_axis, roll_axis, roll) = compute_fall_axes(42, Transform::IDENTITY);

        // Both axes are horizontal (no vertical tip) and unit length.
        assert!(fall_axis.y.abs() < 1e-5);
        assert!((fall_axis.length() - 1.0).abs() < 1e-4);
        assert!(roll_axis.y.abs() < 1e-5);
        assert!((roll_axis.length() - 1.0).abs() < 1e-4);

        // Fall axis is perpendicular to roll axis (fall_axis = roll x Y).
        assert!(fall_axis.dot(roll_axis).abs() < 1e-4);

        // The roll magnitude stays within the documented +/-0.45 band.
        assert!(roll.abs() <= 0.45 + 1e-4);
    }

    #[test]
    fn compute_fall_axes_is_deterministic_per_client_id() {
        // The same id always produces the same collapse so every observer
        // sees the corpse fall identically.
        let a = compute_fall_axes(7, Transform::IDENTITY);
        let b = compute_fall_axes(7, Transform::IDENTITY);
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
        assert_eq!(a.2, b.2);

        // Different ids (very likely) produce a different roll magnitude.
        let other = compute_fall_axes(8, Transform::IDENTITY);
        assert!((a.2 - other.2).abs() > f32::EPSILON || a.0 != other.0);
    }

    /// Build a `PlayerRig` with ten distinct placeholder joint entities so the
    /// pure attachment helpers can be exercised without a running app. Only the
    /// joint entity fields matter here.
    fn test_rig() -> PlayerRig {
        let mut next = 0u32;
        let mut fresh = || {
            let entity = Entity::from_raw_u32(next).expect("valid raw entity index");
            next += 1;
            entity
        };
        PlayerRig {
            body: fresh(),
            upper_arm_l: fresh(),
            upper_arm_r: fresh(),
            forearm_l: fresh(),
            forearm_r: fresh(),
            hand_anchor: fresh(),
            thigh_l: fresh(),
            thigh_r: fresh(),
            shin_l: fresh(),
            shin_r: fresh(),
            held_layers: Vec::new(),
            last_held: None,
            armor_layers: Vec::new(),
            last_equipment: RemoteEquipment::default(),
            last_swing_seq: 0,
            swing: None,
            stride_phase: 0.0,
            corpse_faded: false,
        }
    }

    #[test]
    fn armor_joints_map_to_the_contract_rig_parts() {
        // The ART CONTRACT joint mapping, resolved to actual rig entities: helmets
        // and chest shells go on the Body (one part, there is no Head rig part);
        // the shoulder aux, legs, and feet mirror across both L/R joints.
        let rig = test_rig();
        assert_eq!(armor_joint_entities(&rig, ArmorJoint::Body), vec![rig.body]);
        assert_eq!(
            armor_joint_entities(&rig, ArmorJoint::UpperArmsBoth),
            vec![rig.upper_arm_l, rig.upper_arm_r]
        );
        assert_eq!(
            armor_joint_entities(&rig, ArmorJoint::ThighsBoth),
            vec![rig.thigh_l, rig.thigh_r]
        );
        assert_eq!(
            armor_joint_entities(&rig, ArmorJoint::ShinsBoth),
            vec![rig.shin_l, rig.shin_r]
        );
    }

    #[test]
    fn a_full_chest_piece_resolves_to_three_attachment_targets() {
        // The chest piece is the only one that fans out to three child entities:
        // one torso shell on the Body plus a shoulder aux on each upper arm. This
        // is the rig-entity half of the pure layout test in `items::visual`.
        let rig = test_rig();
        let visual = ArmorMesh::IronCuirass.visual();
        let targets: Vec<Entity> = visual
            .layers()
            .flat_map(|layer| armor_joint_entities(&rig, layer.joint))
            .collect();
        assert_eq!(targets, vec![rig.body, rig.upper_arm_l, rig.upper_arm_r]);
    }

    #[test]
    fn remote_equipment_edge_detection_fires_only_on_a_change() {
        // The appearance system rebuilds armor when the mirror differs from the
        // last-seen value (the `last_held` pattern, never `is_changed`). Pin that
        // the `PartialEq` on `RemoteEquipment` distinguishes a real equip from an
        // identical re-send, so a steady state never churns the layer entities.
        let bare = RemoteEquipment::default();
        let helmed = RemoteEquipment {
            head: Some(ArmorMesh::LamellarHelm),
            ..RemoteEquipment::default()
        };
        // Same value: no rebuild.
        assert_eq!(bare, RemoteEquipment::default());
        // A new worn piece: rebuild.
        assert_ne!(bare, helmed);
        // Swapping one slot's mesh is still a change.
        let iron_helmed = RemoteEquipment {
            head: Some(ArmorMesh::IronHelm),
            ..RemoteEquipment::default()
        };
        assert_ne!(helmed, iron_helmed);
    }
}
