//! Client-side arrow rendering for every peer's projectiles and the local
//! player's own predicted shot.
//!
//! ## Two visual sources, one look
//!
//! Every arrow in flight is rendered from the `items/arrow/model.glb` mesh (the
//! same [`HeldMesh::Arrow`] layers the held-item table already loads), so a peer's
//! arrow and your own predicted arrow are visually identical. There are two ways an
//! arrow visual comes to exist:
//!
//! 1. **Replicated projectiles** (`server::Projectile` + `server::ProjectileTransform`),
//!    which cover every arrow in the AoI ring, including your own once the server
//!    has spawned it. These follow the event-driven reconciler rules from CLAUDE.md
//!    invariant 5: react to `Added<Projectile>` / `RemovedComponents<Projectile>`
//!    with a reverse `Entity -> id` map, never a full-query scan for change gating,
//!    and never `Ref::is_changed()` (it lies for Lightyear-touched components).
//!
//! 2. **Predicted own-arrows** ([`PredictedArrowEvent`]), spawned the instant the
//!    local player looses a shot so the arrow appears without waiting a round trip
//!    for the replicated projectile. A predicted arrow runs the same ballistic sim
//!    the server does (gravity + velocity integration) and is deduped away the
//!    moment the first replicated projectile owned by the local player appears
//!    within a short window (matched by owner + recency, since no id is on the wire
//!    at fire time). If none arrives within [`PREDICTION_TTL_SECONDS`] the server
//!    rejected the shot and the prediction is despawned.
//!
//! ## Extrapolation
//!
//! The server ships a `ProjectileTransform { position, velocity }` per tick (20 Hz).
//! Between ticks the client extrapolates each live arrow BALLISTICALLY from its
//! latest snapshot: it tracks how long the replicated position has been
//! unchanged and advances `position + velocity * age + gravity * age^2 / 2`
//! (the server's own integrator run forward), so a fast arrow reads as a
//! smooth streak rather than a 20 Hz teleport. A stuck arrow (near-zero
//! velocity) rests exactly at its replicated position, oriented along the
//! epsilon rest direction the server keeps for it.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    app::{
        audio::surface::SurfaceMaterial,
        state::{ClientRuntime, ImpactEffectKind, RemoteImpactEvent},
        systems::items::{HeldItemVisuals, held_item_layers, insert_held_layer_material},
    },
    game_balance::PROJECTILE_GRAVITY,
    items::{HeldMesh, ItemModel},
    protocol::ProjectileId,
    server::{Projectile, ProjectileTransform},
};

pub(crate) use crate::app::systems::input::PredictedArrowEvent;

mod predict;

#[cfg(test)]
mod tests;

pub(crate) use predict::{PredictionMatch, predicted_arrow_should_dedupe};

/// Below this speed (m/s) an arrow is treated as "stuck": it rests at its position
/// and keeps its last flight orientation rather than re-aiming along a near-zero
/// velocity (which would make a resting arrow spin to face nowhere).
const STUCK_SPEED_MPS: f32 = 0.5;

/// How long a predicted own-arrow lives without a matching replicated projectile
/// before it is assumed rejected and despawned, in seconds. Comfortably longer than
/// a round trip at any sane ping, short enough that a rejected shot's ghost arrow
/// clears quickly.
const PREDICTION_TTL_SECONDS: f32 = 1.0;

/// Persistent `ProjectileId -> visual Entity` map plus the reverse
/// `replicated Entity -> id` lookup, so the reconciler is event-driven (react to
/// `Added` / `Removed`) instead of scanning the full projectile query each frame.
/// Also owns the local player's in-flight predicted arrows.
#[derive(Resource, Default)]
pub(crate) struct ProjectileVisuals {
    /// Spawned arrow visual per replicated projectile id, plus the little state
    /// needed to detect the own-arrow moving -> stuck transition.
    visuals: HashMap<ProjectileId, ArrowVisual>,
    /// Reverse map `replicated Lightyear entity -> ProjectileId`, populated from
    /// `Added<Projectile>` and consumed from `RemovedComponents<Projectile>` so a
    /// despawn can find its id without scanning.
    replicated_to_id: HashMap<Entity, ProjectileId>,
    /// One catch-up scan flag, mirroring the resource-node reconciler: the
    /// `Added<T>` filter can miss entities that arrived while the system was
    /// early-returning (pre-connect), so the first real run seeds from the full
    /// query once, then event-driven Added/Removed handles the rest.
    applied_first_scan: bool,
    /// The local player's in-flight predicted arrows, each running its own ballistic
    /// sim until a replicated own-projectile dedupes it or its TTL expires.
    predictions: Vec<PredictedArrow>,
}

/// One locally-predicted own-arrow: a visual entity plus the ballistic state that
/// advances it each frame, deduped by owner + recency against the first replicated
/// own-projectile that appears.
struct PredictedArrow {
    entity: Entity,
    position: Vec3,
    velocity: Vec3,
    /// Seconds since the shot was fired, for the dedupe recency window and the TTL.
    age: f32,
}

/// A replicated projectile's arrow visual plus the state for the own-arrow impact
/// cue: the server's `ProjectileImpact` fan-out deliberately excludes the shooter
/// (their client owns their own feedback), so the owner detects their arrow's
/// moving -> stuck transition here and plays the world-impact cue locally.
struct ArrowVisual {
    entity: Entity,
    /// True when this projectile was fired by the local player.
    owned: bool,
    /// True for a thrown bomb: it renders the bomb mesh, TUMBLES with its
    /// travel instead of aiming along velocity, and skips the arrow's
    /// stuck-impact cue (its ending is the detonation, not a thud).
    bomb: bool,
    /// Accumulated tumble angle, in radians, advanced by the bomb's speed so
    /// it visibly rolls through flight and along the ground.
    tumble_angle: f32,
    /// Whether the arrow was in flight last frame, so the moving -> stuck edge
    /// fires the own-impact cue exactly once.
    was_moving: bool,
    /// The last replicated snapshot position, plus the seconds accumulated
    /// since it changed. Extrapolation must advance from the SNAPSHOT's age:
    /// the old code added `velocity * frame_dt` to the replicated position,
    /// which is a near-constant offset between 20 Hz diffs, so arrows visibly
    /// stepped from snapshot to snapshot (owner report: lag in flight).
    snapshot_position: Vec3,
    since_snapshot: f32,
}

/// Spawn a projectile visual from the shared held-item glb layers of `mesh`
/// (world-lit, so it is lit by the scene like every other world prop), parented
/// to nothing (a free world entity) at `transform`. Returns the root entity.
/// Arrows and thrown bombs share this: the mesh (and how the root is oriented
/// each frame) is the only difference.
fn spawn_projectile_visual(
    commands: &mut Commands,
    visuals: &HeldItemVisuals,
    mesh: HeldMesh,
    name: &'static str,
    transform: Transform,
    child_offset: Vec3,
) -> Entity {
    let mut root = commands.spawn((
        Name::new(name),
        transform,
        Visibility::Visible,
        InheritedVisibility::default(),
    ));
    root.with_children(|parent| {
        for item_layer in held_item_layers(visuals, mesh, false) {
            let mut layer = parent.spawn((
                Name::new("Projectile Layer"),
                Mesh3d(item_layer.mesh),
                // The glb's local frame matches the other held glbs (haft along
                // +Y). The root's rotation aims/tumbles it; the child just
                // carries the mesh so a multi-primitive glb overlays exactly.
                // `child_offset` moves the mesh under the root so the root can
                // sit at the physical pivot (the bomb sinks by its ball radius
                // so the root IS the ball center and the roll spins about it;
                // arrows pass zero).
                Transform::from_translation(child_offset),
                Visibility::Inherited,
                bevy::light::NotShadowCaster,
            ));
            insert_held_layer_material(&mut layer, item_layer.material);
        }
    });
    root.id()
}

/// Orient an arrow to fly along `velocity` at `position`. The direction is used
/// down to near-zero speeds: a stuck arrow's replicated velocity is a tiny
/// epsilon along its final flight direction (`PROJECTILE_REST_DIR_EPSILON`,
/// server-side), so even a client that first sees the arrow already at rest
/// aims the shaft into the impact instead of leaving it pointing up. Only a
/// true zero vector falls back to `fallback_rotation`.
fn arrow_transform(position: Vec3, velocity: Vec3, fallback_rotation: Quat) -> Transform {
    let speed = velocity.length();
    let rotation = if speed > 1e-5 {
        // The arrow glb points along +Y (the held-item haft convention), so aim +Y
        // down the velocity direction.
        Quat::from_rotation_arc(Vec3::Y, velocity / speed)
    } else {
        fallback_rotation
    };
    Transform::from_translation(position).with_rotation(rotation)
}

/// Reconcile arrow visuals against the replicated projectile set and advance both
/// replicated and predicted arrows each frame.
///
/// Event-driven per CLAUDE.md invariant 5: spawns react to `Added<Projectile>`,
/// despawns to `RemovedComponents<Projectile>`, and the per-frame transform update
/// iterates only the (small) live projectile set to extrapolate, never a full scan
/// for change gating. Predicted own-arrows advance under gravity and are deduped by
/// the first replicated own-projectile within the recency window.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn apply_projectiles_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    visuals: Res<HeldItemVisuals>,
    mut state: ResMut<ProjectileVisuals>,
    mut own_impacts: MessageWriter<RemoteImpactEvent>,
    projectiles: Query<(Entity, &Projectile, &ProjectileTransform)>,
    added: Query<(Entity, &Projectile, &ProjectileTransform), Added<Projectile>>,
    mut removed: RemovedComponents<Projectile>,
    mut visual_transforms: Query<&mut Transform>,
) {
    if runtime.client_id.is_none() {
        clear_all(&mut commands, &mut state);
        return;
    }
    let dt = time.delta_secs().max(0.0);
    let state = &mut *state;
    let local_id = runtime.client_id;

    // First-run catch-up: the `Added<T>` filter compares against the system's
    // last_run tick, which advances every frame the system early-returns (pre
    // connect), so replicated projectiles that arrived before the first real run
    // won't fire `Added`. Seed once from the full query, then Added/Removed drives
    // the rest.
    if !state.applied_first_scan {
        for (entity, projectile, transform) in &projectiles {
            state.replicated_to_id.insert(entity, projectile.id);
            spawn_replicated(
                &mut commands,
                &visuals,
                state,
                projectile,
                transform,
                local_id,
            );
        }
        state.applied_first_scan = true;
    }

    // 1. Departures: a replicated projectile despawned (hit, AoI leave, TTL). Drop
    //    its visual.
    for entity in removed.read() {
        if let Some(id) = state.replicated_to_id.remove(&entity)
            && let Some(visual) = state.visuals.remove(&id)
        {
            commands.entity(visual.entity).despawn();
        }
    }

    // 2. Arrivals: a new replicated projectile. Record the reverse map, spawn its
    //    visual, and try to dedupe a matching predicted own-arrow.
    for (entity, projectile, transform) in &added {
        if state.replicated_to_id.contains_key(&entity) {
            continue;
        }
        state.replicated_to_id.insert(entity, projectile.id);
        spawn_replicated(
            &mut commands,
            &visuals,
            state,
            projectile,
            transform,
            local_id,
        );

        // If this projectile is the local player's, retire the oldest matching
        // prediction (owner + recency): the authoritative arrow has taken over.
        let ages: Vec<f32> = state.predictions.iter().map(|a| a.age).collect();
        if let PredictionMatch::Retire(index) =
            predicted_arrow_should_dedupe(projectile.owner, local_id, &ages)
        {
            let arrow = state.predictions.swap_remove(index);
            commands.entity(arrow.entity).despawn();
        }
    }

    // 3. Extrapolate every live replicated arrow ballistically from its latest
    //    snapshot (position + velocity * age + the gravity term, the server's own
    //    integrator run forward by the snapshot's age), oriented along velocity.
    //    Iterates only the small live set, not a change-gated full scan; writes the
    //    visual's Transform directly.
    //
    //    The moving -> stuck edge on an OWN arrow additionally fires the world-impact
    //    cue locally: the server's `ProjectileImpact` fan-out excludes the shooter on
    //    purpose (the owner's client owns their own feedback), so without this the
    //    shooter's arrow would thud into a wall silently. The event mirrors what the
    //    peer path emits for a `ProjectileSurface::World` hit in `network.rs`.
    for (entity, projectile, transform) in &projectiles {
        let Some(id) = state.replicated_to_id.get(&entity) else {
            continue;
        };
        let Some(visual) = state.visuals.get_mut(id) else {
            continue;
        };
        let velocity = Vec3::from(transform.velocity);
        let moving = velocity.length() > STUCK_SPEED_MPS;
        let replicated = Vec3::from(transform.position);
        if replicated == visual.snapshot_position {
            visual.since_snapshot += dt;
        } else {
            visual.snapshot_position = replicated;
            visual.since_snapshot = 0.0;
        }
        if visual.owned && visual.was_moving && !moving && !visual.bomb {
            // The arrow's own-thud cue; a bomb's ending is its detonation, so
            // its coming to rest stays silent (the fuse hiss carries it).
            own_impacts.write(RemoteImpactEvent {
                anchor: Vec3::from(transform.position),
                model: projectile.model,
                surface: SurfaceMaterial::Stone,
                effect_kind: ImpactEffectKind::WoodChips,
                seed: projectile.id.0 as u32,
                is_player_hit: false,
            });
        }
        visual.was_moving = moving;

        let Ok(mut visual_transform) = visual_transforms.get_mut(visual.entity) else {
            continue;
        };
        // Ballistic extrapolation by the snapshot's age, capped so a stalled
        // diff (AoI hiccup) can never fly the visual far off the authoritative
        // arc. A stuck arrow rests exactly at its replicated position.
        let position = if moving {
            let age = visual.since_snapshot.min(0.25);
            replicated + velocity * age + Vec3::Y * (0.5 * PROJECTILE_GRAVITY * age * age)
        } else {
            replicated
        };
        if visual.bomb {
            // Tumble instead of aiming: roll about the axis perpendicular to
            // the travel (the physical rolling axis). The visual root is the
            // BALL CENTER (the server sweeps a sphere of the ball radius and
            // the mesh child is sunk by it), so spinning at the physical
            // rolling rate `speed / radius` about the root reads as the ball
            // smoothly rolling on its own surface, fuse cap swinging with it.
            let speed = velocity.length();
            if speed > 1e-4 {
                visual.tumble_angle += speed * dt / crate::game_balance::POWDER_BOMB_BALL_RADIUS_M;
                let axis = Vec3::Y
                    .cross(velocity / speed)
                    .try_normalize()
                    .unwrap_or(Vec3::X);
                *visual_transform = Transform::from_translation(position)
                    .with_rotation(Quat::from_axis_angle(axis, visual.tumble_angle));
            } else {
                // Rolled to rest: ease the ball upright so the burning fuse
                // tip (and its spark rig) surfaces instead of pointing into
                // the dirt. Exponential slerp = fast settle, no pop.
                let ease = 1.0 - (-dt / 0.12).exp();
                let rotation = visual_transform.rotation.slerp(Quat::IDENTITY, ease);
                *visual_transform = Transform::from_translation(position).with_rotation(rotation);
            }
        } else {
            *visual_transform = arrow_transform(position, velocity, visual_transform.rotation);
        }
    }

    // 4. Advance predicted own-arrows under gravity and expire stale ones.
    advance_predictions(&mut commands, &mut visual_transforms, state, dt);
}

/// Spawn (or refresh) a replicated projectile's arrow visual.
fn spawn_replicated(
    commands: &mut Commands,
    visuals: &HeldItemVisuals,
    state: &mut ProjectileVisuals,
    projectile: &Projectile,
    transform: &ProjectileTransform,
    local_id: Option<crate::protocol::ClientId>,
) {
    if state.visuals.contains_key(&projectile.id) {
        return;
    }
    let position = Vec3::from(transform.position);
    let velocity = Vec3::from(transform.velocity);
    let bomb = matches!(projectile.model, ItemModel::ThrownBomb);
    let entity = if bomb {
        // The lit bomb: the powder-bomb glb tumbling with its travel, with the
        // placed-charge fuse rig (sparks + hiss) riding its fuse cap so a
        // flying bomb visibly and audibly burns.
        let entity = spawn_projectile_visual(
            commands,
            visuals,
            HeldMesh::PowderBomb,
            "Thrown Bomb",
            Transform::from_translation(position),
            // The replicated position is the ball CENTER (server sphere sim);
            // the glb's origin is the ball's bottom, so sink the mesh by the
            // ball radius to line them up.
            Vec3::new(0.0, -crate::game_balance::POWDER_BOMB_BALL_RADIUS_M, 0.0),
        );
        crate::app::systems::deployables::charge_fuse::spawn_charge_fuse_rig(
            commands,
            entity,
            crate::items::ExplosiveKind::PowderBomb,
        );
        entity
    } else {
        spawn_projectile_visual(
            commands,
            visuals,
            HeldMesh::Arrow,
            "Arrow",
            arrow_transform(position, velocity, Quat::IDENTITY),
            Vec3::ZERO,
        )
    };
    state.visuals.insert(
        projectile.id,
        ArrowVisual {
            entity,
            owned: Some(projectile.owner) == local_id,
            bomb,
            tumble_angle: 0.0,
            // Spawned already at rest (an arrow that stuck before entering the
            // AoI) must not fire the impact cue; only a real moving -> stuck
            // transition does.
            was_moving: velocity.length() > STUCK_SPEED_MPS,
            snapshot_position: position,
            since_snapshot: 0.0,
        },
    );
}

/// Spawn a predicted own-arrow the instant the local player fires, so the arrow
/// appears without a round trip. Runs on [`PredictedArrowEvent`].
pub(crate) fn spawn_predicted_arrows_system(
    mut commands: Commands,
    visuals: Res<HeldItemVisuals>,
    mut state: ResMut<ProjectileVisuals>,
    mut events: MessageReader<PredictedArrowEvent>,
) {
    for event in events.read() {
        let entity = spawn_projectile_visual(
            &mut commands,
            &visuals,
            HeldMesh::Arrow,
            "Arrow",
            arrow_transform(event.origin, event.velocity, Quat::IDENTITY),
            Vec3::ZERO,
        );
        state.predictions.push(PredictedArrow {
            entity,
            position: event.origin,
            velocity: event.velocity,
            age: 0.0,
        });
    }
}

/// Advance each predicted arrow's ballistic sim (semi-implicit Euler under gravity,
/// matching the server's `advance_kinematics`) and despawn any whose TTL elapsed
/// without a matching replicated projectile (the server rejected the shot).
fn advance_predictions(
    commands: &mut Commands,
    visual_transforms: &mut Query<&mut Transform>,
    state: &mut ProjectileVisuals,
    dt: f32,
) {
    let mut index = 0;
    while index < state.predictions.len() {
        let expired = {
            let arrow = &mut state.predictions[index];
            arrow.age += dt;
            // Gravity onto velocity, then position by the new velocity (semi-implicit
            // Euler, same integrator as the server).
            arrow.velocity.y += PROJECTILE_GRAVITY * dt;
            arrow.position += arrow.velocity * dt;
            if let Ok(mut transform) = visual_transforms.get_mut(arrow.entity) {
                *transform = arrow_transform(arrow.position, arrow.velocity, transform.rotation);
            }
            arrow.age >= PREDICTION_TTL_SECONDS
        };
        if expired {
            let arrow = state.predictions.swap_remove(index);
            commands.entity(arrow.entity).despawn();
        } else {
            index += 1;
        }
    }
}

/// Tear down every arrow visual (session end / disconnect).
fn clear_all(commands: &mut Commands, state: &mut ProjectileVisuals) {
    for (_, visual) in state.visuals.drain() {
        commands.entity(visual.entity).despawn();
    }
    for arrow in state.predictions.drain(..) {
        commands.entity(arrow.entity).despawn();
    }
    state.replicated_to_id.clear();
    state.applied_first_scan = false;
}
