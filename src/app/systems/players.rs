use std::collections::HashSet;

use bevy::{ecs::change_detection::Ref, prelude::*};

use crate::{
    app::PLAYER_VISUAL_CENTER_Y,
    protocol::ClientId,
    server::{Player, PlayerLifecycle, PlayerPose, PlayerSleeping},
};

use super::super::{
    scene::{NetworkPlayer, PlayerVisualAssets, player_visual_position},
    state::ClientRuntime,
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

    for (player, pose, lifecycle, sleeping) in &replicated {
        if player.client_id == local_client_id {
            continue;
        }
        let is_dead = matches!(lifecycle, Some(PlayerLifecycle::Dead { .. }));
        let is_sleeping = matches!(sleeping, Some(PlayerSleeping(true)));
        // Keep the entity around even while dead so the tilt-and-fade
        // animation can finish playing. The tick system below
        // despawns the visual once the fade completes.
        visible_ids.insert(player.client_id);
        let tick = pose.last_changed().get() as u64;
        let feet = Vec3::from(pose.position);
        let target = Transform::from_translation(player_visual_position(feet))
            .with_rotation(Quat::from_rotation_y(pose.yaw));
        if let Some(entity) = entities.0.get(&player.client_id).copied() {
            if let Ok((current, mut interpolation, dying, asleep)) = players.get_mut(entity) {
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
                    commands.entity(entity).insert((
                        DyingPlayer {
                            elapsed: 0.0,
                            finished: false,
                            // A sleeper killed in place is already on the
                            // ground; skip the collapse and just fade.
                            from_sleep: is_sleeping,
                            fall_axis,
                            roll_axis,
                            roll_magnitude,
                            rest: *current,
                            material: cloned_material.clone(),
                        },
                        MeshMaterial3d(cloned_material),
                    ));
                }
                // Dead -> Alive this frame: the player respawned. True
                // whether the death animation was still playing or had
                // already faded out (we keep `DyingPlayer` around until
                // now precisely so this transition is always detectable).
                let just_respawned = !is_dead && dying.is_some();
                if just_respawned {
                    // Drop the dying state, restore the shared remote
                    // material and full visibility.
                    commands.entity(entity).remove::<DyingPlayer>().insert((
                        MeshMaterial3d(assets.remote_material.clone()),
                        Visibility::Visible,
                    ));
                }
                // Live players follow the interpolator. Dying
                // players' transforms are owned by the death-tick
                // system; the interpolator stays frozen at the rest
                // pose so a stray late-arriving movement update
                // can't slide a corpse.
                if !is_dead {
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
            let entity = commands
                .spawn((
                    Name::new(format!("Player {}", player.client_id)),
                    NetworkPlayer {
                        client_id: player.client_id,
                    },
                    NetworkPlayerInterpolation::new(tick, target),
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(assets.remote_material.clone()),
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
}
