use std::collections::HashSet;

use bevy::{ecs::change_detection::Ref, prelude::*};

use crate::{
    app::PLAYER_VISUAL_CENTER_Y,
    protocol::ClientId,
    server::{Player, PlayerLifecycle, PlayerPublic},
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
///   1. Brief upward kick (`UPWARD_KICK_S`) — the body lurches off
///      its feet at the moment of death.
///   2. Pivoted fall (`FALL_DURATION_S`) — the rotation happens
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
/// Fall arc — past 90° so the body face-plants into the floor
/// instead of standing rigid on its head.
const DEATH_FALL_ANGLE_RAD: f32 = std::f32::consts::FRAC_PI_2 + 0.12;
/// How far the body lifts during the initial death kick. Tiny — just
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
    /// World-space rotation axis the body falls around. Picked once
    /// at death time so the corpse picks a direction and sticks with
    /// it. Horizontal — the rotation tips the body forward, not
    /// yaws it on the spot.
    fall_axis: Vec3,
    /// Small per-spawn roll (radians) so the body lands at a
    /// slightly tilted angle instead of square-flat — reads as a
    /// limp collapse rather than a folded mannequin.
    roll_axis: Vec3,
    roll_magnitude: f32,
    /// Snapshot of the world transform at death — used as the rest
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

/// Persistent `client_id → Entity` map for remote players. Mirrors the live
/// entity set so the reconciliation system doesn't have to rebuild it from
/// a `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct RemotePlayerEntities(pub(crate) std::collections::HashMap<ClientId, Entity>);

/// Reconciles the set of visual `NetworkPlayer` entities against the
/// Lightyear-replicated `(Player, PlayerPublic)` entities. Spawn,
/// despawn, and interpolation re-target all flow off the replicated
/// query — one visual entity per replicated entity.
#[allow(clippy::too_many_arguments)]
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
        ),
        With<NetworkPlayer>,
    >,
    replicated: Query<(&Player, Ref<PlayerPublic>, Option<&PlayerLifecycle>)>,
) {
    let Some(local_client_id) = runtime.client_id else {
        // Not connected — tear down any remote visuals from a prior
        // session.
        for (_, entity) in entities.0.drain() {
            commands.entity(entity).despawn();
        }
        return;
    };

    // `last_changed().get()` advances every time the replicated
    // `PlayerPublic` is mutated, so the interpolator only re-targets
    // when there's a real update (Phase 5's tick-as-change-tick trick).
    let mut visible_ids = HashSet::new();
    let entities = &mut *entities;

    for (player, public, lifecycle) in &replicated {
        if player.client_id == local_client_id {
            continue;
        }
        let is_dead = matches!(lifecycle, Some(PlayerLifecycle::Dead { .. }));
        // Keep the entity around even while dead so the tilt-and-fade
        // animation can finish playing. The tick system below
        // despawns the visual once the fade completes.
        visible_ids.insert(player.client_id);
        let tick = public.last_changed().get() as u64;
        let feet = Vec3::from(public.position);
        let target = Transform::from_translation(player_visual_position(feet))
            .with_rotation(Quat::from_rotation_y(public.yaw));
        if let Some(entity) = entities.0.get(&player.client_id).copied() {
            if let Ok((current, mut interpolation, dying)) = players.get_mut(entity) {
                if is_dead && dying.is_none() {
                    // Just-died this frame — stamp the dying state so
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
                            fall_axis,
                            roll_axis,
                            roll_magnitude,
                            rest: *current,
                            material: cloned_material.clone(),
                        },
                        MeshMaterial3d(cloned_material),
                    ));
                } else if !is_dead && dying.is_some() {
                    // Respawned — drop the dying state, restore the
                    // shared remote material and full visibility.
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
                    interpolation.retarget(tick, current, target);
                    let transform = interpolation.advance(time.delta_secs());
                    commands.entity(entity).insert(transform);
                }
            }
        } else if !is_dead {
            // Don't spawn a fresh visual just to immediately mark it
            // dying — if we somehow see a player whose first sight is
            // already Dead (rare; would only happen on AoI cross-in
            // mid-death-anim), we let them stay invisible until the
            // server sends Alive again.
            let entity = commands
                .spawn((
                    Name::new(format!("Player {}", player.client_id)),
                    NetworkPlayer {
                        client_id: player.client_id,
                    },
                    NetworkPlayerInterpolation::new(tick, target),
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(assets.remote_material.clone()),
                    target,
                    Visibility::Visible,
                ))
                .id();
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
///   - `roll_axis`: the same axis crossed with the fall direction —
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
///    feet — reads as "they just took a fatal hit".
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
/// The entity itself isn't despawned — it might respawn (the
/// lifecycle goes Alive → DyingPlayer removed → MeshMaterial3d
/// restored upstream), and recreating the mesh + material pair every
/// kill would churn allocations.
pub(crate) fn tick_dying_players_system(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut players: Query<(Entity, &mut Transform, &mut DyingPlayer, &mut Visibility)>,
) {
    let dt = time.delta_secs().max(0.0);
    for (entity, mut transform, mut dying, mut visibility) in &mut players {
        dying.elapsed += dt;

        // --- Phase 1+2: kick + fall, blended ---
        let kick_t = (dying.elapsed / DEATH_UPWARD_KICK_S).clamp(0.0, 1.0);
        // Half-sine pulse so the kick rises and settles inside its
        // own window without a hard step.
        let kick_pulse = (kick_t * std::f32::consts::PI).sin();
        let kick_y = DEATH_UPWARD_KICK_M * kick_pulse;

        let fall_t = (dying.elapsed / DEATH_FALL_DURATION_S).clamp(0.0, 1.0);
        // Ease-in cubic so the body hangs briefly before
        // accelerating downward — reads like "weight gives out".
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
        let fall_angle = DEATH_FALL_ANGLE_RAD * fall_eased - DEATH_IMPACT_BOUNCE_RAD * bounce_pulse;

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
        let fade_t = ((dying.elapsed - fade_start) / DEATH_FADE_DURATION_S).clamp(0.0, 1.0);
        let alpha = (1.0 - fade_t).clamp(0.0, 1.0);
        if let Some(material) = materials.get_mut(&dying.material) {
            material.base_color = material.base_color.with_alpha(alpha);
        }

        if fade_t >= 1.0 {
            // Fully gone. Drop visibility but leave the entity in
            // place so apply_snapshot can re-show it on respawn
            // without re-spawning a fresh `NetworkPlayer`.
            *visibility = Visibility::Hidden;
            commands.entity(entity).remove::<DyingPlayer>();
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
