//! Remote-player visuals: reconciles the visual `NetworkPlayer` entities
//! against the Lightyear-replicated player entities and mirrors the replicated
//! cosmetic state onto local components for the animators. Split by concern
//! into sibling submodules:
//!
//! - `death_anim`, the `DyingPlayer`/`SleepingPlayer` collapse + fade.
//! - `interpolation`, the snapshot-to-snapshot transform blend.
//! - `locomotion`, the procedural walk/swing/charge animators.
//! - `rig`, the part-entity skeleton builder and appearance swaps.
//!
//! This root keeps `apply_snapshot_system` plus the shared mirror components
//! it writes, and re-exports the submodule surface flat so call sites keep
//! saying `players::X`.

use std::collections::HashSet;

use bevy::{ecs::change_detection::Ref, prelude::*};

use crate::{
    items::{ArmorMesh, HeldMesh, HeldPieceSlot, ItemModel},
    protocol::ClientId,
    server::{
        Player, PlayerAction, PlayerChargeFraction, PlayerEquipmentVisual, PlayerHeldItem,
        PlayerLifecycle, PlayerPose, PlayerSleeping,
    },
};

use super::super::{
    scene::{NetworkPlayer, PlayerVisualAssets, player_visual_position},
    state::ClientRuntime,
};

mod death_anim;
mod interpolation;
mod locomotion;
mod rig;

pub(crate) use death_anim::{DyingPlayer, SleepingPlayer, tick_dying_players_system};
pub(crate) use interpolation::NetworkPlayerInterpolation;
pub(crate) use locomotion::{
    animate_remote_held_charge_system, animate_remote_players_system, remote_head_anchor_local,
};
pub(crate) use rig::{
    PlayerRig, apply_remote_player_appearance_system, reconcile_player_rigs_system,
};

use death_anim::{compute_fall_axes, lying_transform};
use locomotion::remote_equipment_from;

/// Persistent `client_id → Entity` map for remote players. Mirrors the live
/// entity set so the reconciliation system doesn't have to rebuild it from
/// a `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct RemotePlayerEntities(pub(crate) std::collections::HashMap<ClientId, Entity>);

/// Local mirror of the replicated pose's movement + look, written onto the
/// visual NetworkPlayer by `apply_snapshot_system` so the animators never
/// re-join the replicated entity.
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteLocomotion {
    pub(crate) speed: f32,
    pub(crate) grounded: bool,
    /// Replicated look pitch (radians, +up), so the animator can lean the peer's
    /// upper body / held item to match where they are aiming.
    pub(crate) pitch: f32,
    /// Replicated bow-draw fraction (0 rest, 1 full draw), so the animator can
    /// flex the peer's drawn bow. Zero for a non-bow or an undrawn bow.
    pub(crate) charge_fraction: f32,
}

/// Local mirror of the replicated `PlayerHeldItem`.
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteHeld(pub(crate) Option<HeldMesh>);

/// The rig slot a spawned remote held-item layer fills (bow limb/string/arrow,
/// crossbow string, or `Static`), so the per-frame draw animator can compose the
/// same per-piece transform the first-person viewmodel uses on top of the
/// whole-item grip. `Static` layers stay put (identity piece transform).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct RemoteHeldPiece(pub(crate) HeldPieceSlot);

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

/// Local mirror of the replicated `PlayerAction` (current swing).
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteAction {
    pub(crate) seq: u32,
    /// The swing archetype the peer is swinging (weapon's own model or a gather
    /// tool's archetype). Drives the third-person swing arc directly off the wire.
    pub(crate) model: ItemModel,
}

/// The mirror components `apply_snapshot_system` joins on a remote player's
/// visual entity, shared between the system's query and the per-tick
/// `update_remote_player_visual` helper.
type RemoteVisualQuery = (
    &'static Transform,
    &'static mut NetworkPlayerInterpolation,
    &'static mut RemoteLocomotion,
    &'static mut RemoteHeld,
    &'static mut RemoteEquipment,
    &'static mut RemoteAction,
    Option<&'static DyingPlayer>,
    Option<&'static SleepingPlayer>,
);

/// Per-tick values derived from one replicated remote player, shared by the
/// first-sight spawn and the per-tick update paths so the two can never
/// disagree about how the wire state maps onto the mirror components.
struct RemotePlayerSnapshot {
    client_id: ClientId,
    is_dead: bool,
    is_sleeping: bool,
    held_mesh: Option<HeldMesh>,
    equipment_visual: RemoteEquipment,
    action_seq: u32,
    action_model: ItemModel,
    charge_fraction: f32,
    horizontal_speed: f32,
    grounded: bool,
    pitch: f32,
    tick: u64,
    target: Transform,
}

/// Reconciles the set of visual `NetworkPlayer` entities against the
/// Lightyear-replicated `(Player, PlayerPose)` entities. Spawn,
/// despawn, and interpolation re-target all flow off the replicated
/// query, one visual entity per replicated entity.
#[expect(
    clippy::too_many_arguments,
    clippy::type_complexity,
    reason = "Bevy system params and query types"
)]
pub(crate) fn apply_snapshot_system(
    mut commands: Commands,
    time: Res<Time>,
    runtime: Res<ClientRuntime>,
    assets: Res<PlayerVisualAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut entities: ResMut<RemotePlayerEntities>,
    mut players: Query<RemoteVisualQuery, With<NetworkPlayer>>,
    replicated: Query<(
        &Player,
        Ref<PlayerPose>,
        Option<&PlayerLifecycle>,
        Option<&PlayerSleeping>,
        Option<&PlayerHeldItem>,
        Option<&PlayerEquipmentVisual>,
        Option<&PlayerAction>,
        Option<&PlayerChargeFraction>,
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

    for (player, pose, lifecycle, sleeping, held, equipment, action, charge) in &replicated {
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
        let charge_fraction = charge.map(|charge| charge.0).unwrap_or(0.0);
        let velocity = Vec3::from(pose.velocity);
        let horizontal_speed = velocity.with_y(0.0).length();
        // Keep the entity around even while dead so the tilt-and-fade
        // animation can finish playing. The death-tick system despawns
        // the visual once the fade completes.
        visible_ids.insert(player.client_id);
        let tick = pose.last_changed().get() as u64;
        let feet = Vec3::from(pose.position);
        let target = Transform::from_translation(player_visual_position(feet))
            .with_rotation(Quat::from_rotation_y(pose.yaw));
        let snapshot = RemotePlayerSnapshot {
            client_id: player.client_id,
            is_dead,
            is_sleeping,
            held_mesh,
            equipment_visual,
            action_seq,
            action_model,
            charge_fraction,
            horizontal_speed,
            grounded: pose.grounded,
            pitch: pose.pitch,
            tick,
            target,
        };
        if let Some(entity) = entities.0.get(&player.client_id).copied() {
            update_remote_player_visual(
                &mut commands,
                &time,
                &assets,
                &mut materials,
                &mut players,
                entity,
                &snapshot,
            );
        } else if !is_dead {
            // Don't spawn a fresh visual just to immediately mark it
            // dying, if we somehow see a player whose first sight is
            // already Dead (rare; would only happen on AoI cross-in
            // mid-death-anim), we let them stay invisible until the
            // server sends Alive again.
            let entity = spawn_remote_player_visual(&mut commands, &snapshot);
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

/// Per-tick update for an already-spawned remote visual: stamp/clear the dying
/// state on the lifecycle edges, mirror the cosmetic peer state onto the
/// animator components, track the sleep<->awake transition, and drive the
/// interpolator toward the replicated pose.
fn update_remote_player_visual(
    commands: &mut Commands,
    time: &Time,
    assets: &PlayerVisualAssets,
    materials: &mut Assets<StandardMaterial>,
    players: &mut Query<RemoteVisualQuery, With<NetworkPlayer>>,
    entity: Entity,
    snapshot: &RemotePlayerSnapshot,
) {
    let Ok((
        current,
        mut interpolation,
        mut loco,
        mut held_comp,
        mut equipment_comp,
        mut action_comp,
        dying,
        asleep,
    )) = players.get_mut(entity)
    else {
        return;
    };

    if snapshot.is_dead && dying.is_none() {
        // Just-died this frame, stamp the dying state so
        // the death-tick system takes over the transform.
        let (fall_axis, roll_axis, roll_magnitude) =
            compute_fall_axes(snapshot.client_id, *current);
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
            from_sleep: snapshot.is_sleeping,
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
    let just_respawned = !snapshot.is_dead && dying.is_some();
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
    if !snapshot.is_dead {
        // Feed the rig animators (read off the visual entity so they
        // don't need to re-join the replicated entity). Cheap small
        // writes; the animators read these by value each frame.
        loco.speed = snapshot.horizontal_speed;
        loco.grounded = snapshot.grounded;
        loco.pitch = snapshot.pitch;
        loco.charge_fraction = snapshot.charge_fraction;
        held_comp.0 = snapshot.held_mesh;
        // Worn-armor mirror: manual edge detection against the local
        // value (never `Ref::is_changed`, which lies for
        // Lightyear-touched components). Rig rendering off this is
        // Phase 4; today the write just keeps the local handle current.
        if *equipment_comp != snapshot.equipment_visual {
            *equipment_comp = snapshot.equipment_visual;
        }
        action_comp.seq = snapshot.action_seq;
        action_comp.model = snapshot.action_model;
        // Track the sleep<->awake transition so the marker, the
        // pose, and the interpolation stay in sync.
        let woke = asleep.is_some() && !snapshot.is_sleeping;
        if snapshot.is_sleeping && asleep.is_none() {
            commands.entity(entity).insert(SleepingPlayer);
        } else if woke {
            commands.entity(entity).remove::<SleepingPlayer>();
        }

        if snapshot.is_sleeping {
            // Logged-out body: hold a static lying-down pose and
            // freeze the interpolator at the upright target so the
            // body stands straight back up when it wakes.
            interpolation.snap_to(snapshot.tick, snapshot.target);
            commands
                .entity(entity)
                .insert(lying_transform(snapshot.target));
        } else {
            if just_respawned || woke {
                // A respawn or a wake is a teleport, not a walk.
                // Hard-snap instead of blending: when the new pose
                // lands within interpolation range of the old one,
                // `retarget` would otherwise slide the avatar across
                // for a few frames (the "flicker before disappearing
                // to spawn" the killer sees, and the slow rise from
                // a lying pose on wake).
                interpolation.snap_to(snapshot.tick, snapshot.target);
            } else {
                interpolation.retarget(snapshot.tick, current, snapshot.target);
            }
            let transform = interpolation.advance(time.delta_secs());
            commands.entity(entity).insert(transform);
        }
    }
}

/// First-time spawn of a remote visual root. The root is a transform-only
/// node: the visible body is the rig of child part entities built by
/// `reconcile_player_rigs_system` from the `Added<NetworkPlayer>` edge. The
/// cosmetic mirror components seed the animators with current state so an AoI
/// cross-in shows the right held item / locomotion immediately.
fn spawn_remote_player_visual(commands: &mut Commands, snapshot: &RemotePlayerSnapshot) -> Entity {
    // A body first seen while sleeping (AoI cross-in onto a logged-out
    // sleeper) spawns already lying down, so it never flashes upright
    // for a frame. The interpolator is still parked at the upright
    // target so it stands up cleanly on wake.
    let spawn_transform = if snapshot.is_sleeping {
        lying_transform(snapshot.target)
    } else {
        snapshot.target
    };
    let entity = commands
        .spawn((
            Name::new(format!("Player {}", snapshot.client_id)),
            NetworkPlayer {
                client_id: snapshot.client_id,
            },
            NetworkPlayerInterpolation::new(snapshot.tick, snapshot.target),
            RemoteLocomotion {
                speed: snapshot.horizontal_speed,
                grounded: snapshot.grounded,
                pitch: snapshot.pitch,
                charge_fraction: snapshot.charge_fraction,
            },
            RemoteHeld(snapshot.held_mesh),
            snapshot.equipment_visual,
            RemoteAction {
                seq: snapshot.action_seq,
                model: snapshot.action_model,
            },
            spawn_transform,
            Visibility::Visible,
        ))
        .id();
    if snapshot.is_sleeping {
        commands.entity(entity).insert(SleepingPlayer);
    }
    entity
}
