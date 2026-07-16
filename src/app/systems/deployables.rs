//! Client-side deployable systems.
//!
//! - `apply_deployed_entities_system`, reconciles the replicated
//!   deployable entities into local visuals. **Event-driven**: it reacts
//!   to `Added<Deployable>` / `RemovedComponents<Deployable>` plus
//!   value-compared `DeployableActive` changes instead of scanning the
//!   full replicated set every frame; with base building the deployable
//!   count is open-ended, so the full-scan version was the next
//!   "iterate N entities to discover nothing changed" bug shape
//!   (docs/profiling.md). The resource-node reconciler is the canonical
//!   pattern this follows.
//! - `maintain_world_grid_system`, rebuilds the client collision grid
//!   from the replicated resource-node and deployable sets when they
//!   change.
//!
//! Placement-ghost and input handling live in [`placement`].

pub(crate) mod charge_fuse;
mod placement;

pub(crate) use placement::{
    ChargeGhostMeshes, placement_input_system, prepare_charge_ghost_meshes_system,
    update_claim_boundary_system, update_placement_ghost_system,
};

use crate::building::{ClaimPlatform, DOOR_OPEN_ANGLE_RAD, DOOR_PANEL_WIDTH_M};

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    app::{
        scene::{DeployableVisualAssets, NetworkDeployedEntity, ToonMaterial, TorchFireAssets},
        state::{ClientRuntime, MenuState},
        systems::furnace_fire::{FurnaceFire, sync_furnace_fire},
        systems::torch_fire::{TorchFire, sync_torch_fire},
    },
    items::{DeployableKind, item_definition},
    protocol::{DeployedEntityId, Vec3Net},
    resource_nodes::resource_node_collider_at,
    server::{Deployable, DeployableActive, DeployableTransform, ResourceNode},
    world::WorldBlock,
};

/// Per-frame cap on fresh deployable visual spawns. Walking into a
/// large base (or the initial join burst) can pull hundreds of pieces
/// into the AoI in one tick; spawning every visual the same frame
/// stalls the command-buffer flush. Anything past the budget stays in
/// the pending queue and drains over the following frames. Updates to
/// existing visuals and despawns are uncapped.
const MAX_DEPLOYABLE_SPAWNS_PER_FRAME: usize = 16;

/// The budget while the world-entry loading splash is up: the scene is hidden
/// behind an opaque overlay, so spawn-burst frame hitches are invisible and a
/// bigger budget just shortens the load. The smooth budget above takes over
/// the moment the world is revealed.
const MAX_DEPLOYABLE_SPAWNS_PER_FRAME_LOADING: usize = 64;

/// A spawned deployable visual tracked by [`DeployedEntityVisuals`].
struct DeployableVisualEntry {
    /// Root visual entity. Owns the mesh (or, for doors, the animated
    /// panel child) and any furnace fire rig as children, so a single
    /// recursive despawn tears the whole thing down.
    entity: Entity,
    /// Kind at spawn / last upgrade. A fresh `Added` with the same id
    /// but a different kind is a hammer tier upgrade.
    kind: DeployableKind,
    /// Last applied `DeployableActive` value. Replicated change ticks
    /// fire even when the value is identical (see CLAUDE.md
    /// "Replicated state"), so flips are detected by comparing against
    /// this, never by `Changed` alone.
    active: bool,
    /// The replicated mirror entity currently backing this id. A tier
    /// upgrade respawns the mirror server-side (remove + add with the
    /// same id); tracking the backing entity lets the removal pass
    /// recognise the stale half of that pair and leave the visual
    /// alone.
    replicated: Entity,
    /// World position, kept as the anchor for flip sounds.
    position: Vec3,
}

/// Spawn data captured from `Added<Deployable>` and held until the
/// per-frame spawn budget admits it, same shape as the resource-node
/// reconciler's pending queue.
struct PendingDeployableSpawn {
    id: DeployedEntityId,
    replicated: Entity,
    kind: DeployableKind,
    position: Vec3Net,
    yaw: f32,
    active: bool,
}

/// Persistent client-side index of deployable visuals. Mirrors the
/// live replicated set so reconciliation reacts to events instead of
/// rebuilding maps from a `Query` every frame.
#[derive(Resource, Default)]
pub(crate) struct DeployedEntityVisuals {
    entries: HashMap<DeployedEntityId, DeployableVisualEntry>,
    /// Reverse lookup `Lightyear-replicated entity → id`. Populated on
    /// `Added`, consumed on `RemovedComponents`.
    replicated_to_id: HashMap<Entity, DeployedEntityId>,
    /// Arrivals waiting on [`MAX_DEPLOYABLE_SPAWNS_PER_FRAME`]. Drained
    /// nearest-to-player first (see the drain step), so this is an
    /// unordered bag: order within it carries no meaning.
    pending_spawns: Vec<PendingDeployableSpawn>,
    /// `true` once a reconciliation pass has run while connected. Gates
    /// the first-run catch-up scan: the `Added` filter compares against
    /// the system's `last_run` tick and misses entities that arrived
    /// during early-returning frames (menu, connecting).
    applied_first_snapshot: bool,
}

impl DeployedEntityVisuals {
    /// Replicated deployables still waiting on the per-frame spawn budget.
    /// The loading splash surfaces this as world-entry progress.
    pub(crate) fn pending_spawn_count(&self) -> usize {
        self.pending_spawns.len()
    }

    /// True once at least one reconciliation pass has run this session and
    /// the budgeted spawn queue is empty. Feeds the world-entry readiness
    /// gate (`world_ready_for_play`), same contract as
    /// `ResourceNodeEntities::is_caught_up`.
    pub(crate) fn is_caught_up(&self) -> bool {
        self.applied_first_snapshot && self.pending_spawns.is_empty()
    }
}

/// Reconcile the local `NetworkDeployedEntity` visuals against the
/// Lightyear-replicated `(Deployable, DeployableTransform,
/// DeployableActive)` entities. Spawns arrivals (budgeted), despawns
/// departures, swaps meshes in place on tier upgrades, and applies
/// `active` flips (furnace fire rig, door panel swing + audio).
/// Steady-state frames (no arrivals, departures, flips, or queued
/// spawns) do essentially no work.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn apply_deployed_entities_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Option<Res<DeployableVisualAssets>>,
    torch_assets: Option<Res<TorchFireAssets>>,
    mut visuals: ResMut<DeployedEntityVisuals>,
    // World-entry loading trio, bundled to stay under Bevy's 16-param
    // system limit: the clock + arrival tracker feed the readiness gate's
    // stream condition, the menu selects the loading spawn budget.
    world_load: (
        Res<Time>,
        ResMut<crate::app::state::WorldStreamState>,
        Res<MenuState>,
    ),
    existing_fires: Query<(Entity, &ChildOf), With<FurnaceFire>>,
    existing_torch_fires: Query<(Entity, &ChildOf), With<TorchFire>>,
    mut panels: Query<(&mut DoorPanel, &ChildOf)>,
    all_deployables: Query<(Entity, &Deployable, &DeployableTransform, &DeployableActive)>,
    added: Query<(Entity, &Deployable, &DeployableTransform, &DeployableActive), Added<Deployable>>,
    changed_active: Query<(Entity, &Deployable, &DeployableActive), Changed<DeployableActive>>,
    mut removed: RemovedComponents<Deployable>,
    mut play_sound: MessageWriter<crate::app::audio::PlaySound>,
    mut remote_impacts: MessageWriter<crate::app::state::RemoteImpactEvent>,
) {
    let (time, mut stream, menu) = world_load;
    let Some(assets) = assets else {
        return;
    };
    let torch_assets = torch_assets.as_deref();
    let visuals = &mut *visuals;
    if runtime.client_id.is_none() {
        // Not connected, tear down any visuals from a prior session.
        // Despawning the root also removes panel/fire children.
        for (_, entry) in visuals.entries.drain() {
            commands.entity(entry.entity).despawn();
        }
        visuals.replicated_to_id.clear();
        visuals.pending_spawns.clear();
        visuals.applied_first_snapshot = false;
        stream.reset();
        // Drain stale removal events so a reconnect doesn't replay them.
        removed.read().count();
        return;
    }
    stream.note_connected(time.elapsed_secs());

    // Replicated arrivals this frame, reported to the world-entry stream
    // tracker so the loading gate can wait for the server to finish the
    // initial send.
    let mut arrivals = 0usize;

    // First-run catch-up: seed the reverse map and the spawn queue from
    // the full query once. See the resource-node reconciler for why
    // `Added` can't cover entities that arrived while this system was
    // early-returning.
    if !visuals.applied_first_snapshot {
        for (replicated_entity, meta, transform, active) in &all_deployables {
            arrivals += 1;
            visuals.replicated_to_id.insert(replicated_entity, meta.id);
            if visuals.entries.contains_key(&meta.id) {
                continue;
            }
            visuals.pending_spawns.push(PendingDeployableSpawn {
                id: meta.id,
                replicated: replicated_entity,
                kind: meta.kind,
                position: transform.position,
                yaw: transform.yaw,
                active: active.0,
            });
        }
        visuals.applied_first_snapshot = true;
    }

    // 1. Arrivals, processed *before* departures so a tier upgrade's
    //    remove + add pair (same id, fresh mirror entity, see the
    //    server's `sync_deployable_entities`) resolves as an in-place
    //    mesh swap regardless of intra-frame event ordering.
    for (replicated_entity, meta, transform, active) in &added {
        if visuals.replicated_to_id.contains_key(&replicated_entity) {
            // Catch-up above already seeded this entity.
            continue;
        }
        arrivals += 1;
        visuals.replicated_to_id.insert(replicated_entity, meta.id);

        if let Some(mut entry) = visuals.entries.remove(&meta.id) {
            // Same id, new mirror entity: a hammer tier upgrade. Doors
            // can't be mesh-swapped (their visual is a panel child),
            // so a door<->non-door transition falls back to respawn;
            // it can't happen through the upgrade path today, this is
            // purely defensive.
            let door_transition = matches!(entry.kind, DeployableKind::Door { .. })
                != matches!(meta.kind, DeployableKind::Door { .. });
            if door_transition {
                commands.entity(entry.entity).despawn();
                visuals.pending_spawns.push(PendingDeployableSpawn {
                    id: meta.id,
                    replicated: replicated_entity,
                    kind: meta.kind,
                    position: transform.position,
                    yaw: transform.yaw,
                    active: active.0,
                });
                continue;
            }
            entry.replicated = replicated_entity;
            if entry.kind != meta.kind {
                // Swap the mesh in place and celebrate with a material
                // burst (chips/shards + impact audio ride the same
                // remote-impact pipeline as gather hits).
                let (mesh, material) = deployable_visual(&assets, meta.kind);
                let mut ec = commands.entity(entry.entity);
                ec.insert((NetworkDeployedEntity { id: meta.id }, Mesh3d(mesh)));
                insert_deployable_material(&mut ec, material);
                emit_upgrade_burst(&mut remote_impacts, meta.kind, entry.position);
                entry.kind = meta.kind;
            }
            if entry.active != active.0 {
                entry.active = active.0;
                apply_active_flip(
                    &mut commands,
                    &entry,
                    &existing_fires,
                    &existing_torch_fires,
                    torch_assets,
                    &mut panels,
                    &mut play_sound,
                );
            }
            visuals.entries.insert(meta.id, entry);
        } else if let Some(pending) = visuals
            .pending_spawns
            .iter_mut()
            .find(|spawn| spawn.id == meta.id)
        {
            // Removed and re-added while still queued (an upgrade
            // before the budget admitted the spawn): refresh in place.
            pending.replicated = replicated_entity;
            pending.kind = meta.kind;
            pending.position = transform.position;
            pending.yaw = transform.yaw;
            pending.active = active.0;
        } else {
            visuals.pending_spawns.push(PendingDeployableSpawn {
                id: meta.id,
                replicated: replicated_entity,
                kind: meta.kind,
                position: transform.position,
                yaw: transform.yaw,
                active: active.0,
            });
        }
    }

    stream.note_arrivals(time.elapsed_secs(), arrivals);

    // 2. Departures. AoI leave, destruction, or the stale half of an
    //    upgrade respawn (which arrivals above already retargeted).
    for replicated_entity in removed.read() {
        let Some(id) = visuals.replicated_to_id.remove(&replicated_entity) else {
            continue;
        };
        if let Some(entry) = visuals.entries.get(&id) {
            if entry.replicated != replicated_entity {
                // Upgrade respawn: the visual is already backed by the
                // new mirror entity.
                continue;
            }
            commands.entity(entry.entity).despawn();
            visuals.entries.remove(&id);
        } else {
            // Never spawned: drop the queued spawn if it is still
            // backed by the removed entity (an entry refreshed by the
            // arrivals pass survives).
            visuals
                .pending_spawns
                .retain(|spawn| spawn.replicated != replicated_entity);
        }
    }

    // 3. Active flips (furnace lit/cold, door open/close). `Changed`
    //    fires on every replication touch even when the value is
    //    identical, so the stored value is the actual edge detector.
    for (replicated_entity, meta, active) in &changed_active {
        if let Some(entry) = visuals.entries.get_mut(&meta.id) {
            if entry.replicated != replicated_entity || entry.active == active.0 {
                continue;
            }
            entry.active = active.0;
            apply_active_flip(
                &mut commands,
                entry,
                &existing_fires,
                &existing_torch_fires,
                torch_assets,
                &mut panels,
                &mut play_sound,
            );
        } else if let Some(pending) = visuals
            .pending_spawns
            .iter_mut()
            .find(|spawn| spawn.replicated == replicated_entity)
        {
            // Still queued: record the new state silently, the spawn
            // applies it directly.
            pending.active = active.0;
        }
    }

    // 4. Drain the spawn queue up to the per-frame budget. Usually
    //    empty in steady state, so this loop is zero iterations.
    // Perimeter walls render nudged inward so their outer face is flush
    // with the foundation edge; gather the platform set once (cheap, this
    // only runs on the structural change that queued a spawn). `maintain_
    // wall_visual_insets_system` keeps them correct as neighbours change.
    let platforms = (!visuals.pending_spawns.is_empty())
        .then(|| collect_building_platforms(all_deployables.iter().map(|(_, m, t, _)| (m, t))));
    // Drain nearest-to-player first so a base you spawn next to assembles
    // from the pieces around you outward, instead of in Lightyear's arrival
    // order (which leaves a hole in the wall right in front of you while a
    // far corner streams in). `select_nth_unstable_by` partitions the
    // nearest `budget` to the front in average O(n) with no allocation, and
    // the whole block is skipped in steady state.
    let player_position = runtime
        .local_view()
        .map(|view| Vec3::from(view.position) + Vec3::Y * crate::app::EYE_HEIGHT);
    let budget = if menu.world_entry_splash_active() {
        MAX_DEPLOYABLE_SPAWNS_PER_FRAME_LOADING
    } else {
        MAX_DEPLOYABLE_SPAWNS_PER_FRAME
    };
    let take = budget.min(visuals.pending_spawns.len());
    if let Some(player) = player_position
        && take > 0
        && take < visuals.pending_spawns.len()
    {
        visuals
            .pending_spawns
            .select_nth_unstable_by(take - 1, |a, b| {
                Vec3::from(a.position)
                    .distance_squared(player)
                    .total_cmp(&Vec3::from(b.position).distance_squared(player))
            });
    }
    for spawn in visuals.pending_spawns.drain(..take).collect::<Vec<_>>() {
        let position = Vec3::from(spawn.position);
        let visual_position =
            wall_inset_visual_position(spawn.kind, spawn.position, spawn.yaw, platforms.as_deref());
        let visual_transform = deployable_visual_transform(visual_position, spawn.yaw, spawn.kind);
        let parent = if let DeployableKind::Door { variant } = spawn.kind {
            // Doors spawn an animated panel child instead of a root
            // mesh: the root sits at the doorway centre (replicated
            // transform); the panel hangs off the hinge and swings via
            // `animate_door_panels_system`. The panel spawns at its
            // resolved pose so a base streaming in doesn't open with a
            // chorus of swinging doors.
            let parent = commands
                .spawn((
                    Name::new(format!("Deployable {}", spawn.id)),
                    NetworkDeployedEntity { id: spawn.id },
                    visual_transform,
                    Visibility::Visible,
                ))
                .id();
            let initial_angle = if spawn.active {
                -DOOR_OPEN_ANGLE_RAD
            } else {
                0.0
            };
            // The hinge sits on the -X jamb of the opening the panel spans:
            // the doorway width for doors, the window opening for the
            // shutter (whose mesh carries its own sill-height offset).
            let hinge_x = match variant {
                crate::items::DoorVariant::Shutter => {
                    -(crate::building::WINDOW_OPENING_WIDTH_M / 2.0)
                }
                _ => -(DOOR_PANEL_WIDTH_M / 2.0),
            };
            commands.spawn((
                Name::new("Door Panel"),
                DoorPanel {
                    angle: initial_angle,
                    open: spawn.active,
                },
                Mesh3d(assets.door_panel_mesh(variant)),
                MeshMaterial3d(assets.door_material(variant)),
                Transform::from_translation(Vec3::new(hinge_x, 0.0, 0.0))
                    .with_rotation(Quat::from_rotation_y(initial_angle)),
                Visibility::Visible,
                ChildOf(parent),
            ));
            parent
        } else {
            let (mesh, material) = deployable_visual(&assets, spawn.kind);
            let mut ec = commands.spawn((
                Name::new(format!("Deployable {}", spawn.id)),
                NetworkDeployedEntity { id: spawn.id },
                Mesh3d(mesh),
                visual_transform,
                Visibility::Visible,
            ));
            insert_deployable_material(&mut ec, material);
            ec.id()
        };
        if spawn.active {
            // A lit furnace streaming into the AoI gets its fire rig
            // immediately. `sync_furnace_fire` only plays audio on the
            // lit→cold edge, so the join burst stays silent.
            sync_furnace_fire(
                &mut commands,
                parent,
                spawn.kind,
                true,
                None,
                position,
                &mut play_sound,
            );
            // Same for a lit torch streaming in: attach its flame + light rig.
            if let Some(torch_assets) = torch_assets {
                sync_torch_fire(&mut commands, parent, spawn.kind, true, None, torch_assets);
            }
        }
        // A placed charge is ALWAYS armed while it exists (fuse is server-only),
        // so attach its fuse rig (sparks + hiss) unconditionally on spawn, not
        // gated on `active`. The rig tears down with the charge when it fizzles
        // or detonates (children despawn recursively with the root).
        if let DeployableKind::Explosive { kind } = spawn.kind {
            charge_fuse::spawn_charge_fuse_rig(&mut commands, parent, kind);
        }
        visuals.entries.insert(
            spawn.id,
            DeployableVisualEntry {
                entity: parent,
                kind: spawn.kind,
                active: spawn.active,
                replicated: spawn.replicated,
                position,
            },
        );
    }
}

/// React to a real `DeployableActive` flip on a spawned visual:
/// furnaces toggle their fire rig, doors retarget their panel swing and
/// play the open/close cue. First sight of an entity never lands here
/// (spawns apply the initial state directly), so a base streaming in
/// stays silent, the behaviour the old per-frame scan guaranteed via
/// its seed-silently `Local` map.
fn apply_active_flip(
    commands: &mut Commands,
    entry: &DeployableVisualEntry,
    fires: &Query<(Entity, &ChildOf), With<FurnaceFire>>,
    torch_fires: &Query<(Entity, &ChildOf), With<TorchFire>>,
    torch_assets: Option<&TorchFireAssets>,
    panels: &mut Query<(&mut DoorPanel, &ChildOf)>,
    play_sound: &mut MessageWriter<crate::app::audio::PlaySound>,
) {
    match entry.kind {
        DeployableKind::Furnace { .. } => {
            // The fire-rig lookup walks only live rigs (lit furnaces in
            // the AoI, a handful at most) and only on flips.
            let existing_fire = fires
                .iter()
                .find(|(_, child_of)| child_of.parent() == entry.entity)
                .map(|(fire_entity, _)| fire_entity);
            sync_furnace_fire(
                commands,
                entry.entity,
                entry.kind,
                entry.active,
                existing_fire,
                entry.position,
                play_sound,
            );
        }
        DeployableKind::Torch { .. } => {
            // A torch burning out (active → false) tears its rig down; a
            // relit one (future) rebuilds it. Same cheap per-flip lookup.
            if let Some(torch_assets) = torch_assets {
                let existing_fire = torch_fires
                    .iter()
                    .find(|(_, child_of)| child_of.parent() == entry.entity)
                    .map(|(fire_entity, _)| fire_entity);
                sync_torch_fire(
                    commands,
                    entry.entity,
                    entry.kind,
                    entry.active,
                    existing_fire,
                    torch_assets,
                );
            }
        }
        DeployableKind::Door { .. } => {
            for (mut panel, child_of) in panels.iter_mut() {
                if child_of.parent() == entry.entity {
                    panel.open = entry.active;
                }
            }
            let id = if entry.active {
                crate::app::audio::SoundId::DoorOpen
            } else {
                crate::app::audio::SoundId::DoorClose
            };
            play_sound.write(crate::app::audio::PlaySound::at(id, entry.position));
        }
        _ => {}
    }
}

/// Knuth golden-ratio mix constant for the fingerprint helpers, the
/// XOR-of-ids accumulator gets multiplied by this to spread sequential
/// id values across the `u64`.
const FINGERPRINT_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// Per-frame maintainer for `ClientRuntime::world_grid`. Watches the
/// world version (Welcome bumps it), the replicated resource-node set,
/// and the replicated deployable set; rebuilds the grid when any of
/// them changes.
///
/// **Event-gated**, the previous fingerprint-based "idle frames cost a
/// fingerprint compare and nothing else" was a lie at scale: with 1811
/// resource nodes, the fingerprint scan was a 1-2 ms iteration *every
/// frame*, calling `resource_node_collider_at` (string-keyed HashMap
/// lookup) for each node just to detect "nothing changed". The probe
/// below short-circuits before that scan when `Added`/`Removed` events
/// confirm the entity set is unchanged.
/// During the initial world stream, coalesce collision-grid rebuilds whose only
/// trigger is newly-streamed entities to at most one per this interval, so the
/// per-frame O(all-entities) rebuild does not freeze the loading splash.
const GRID_STREAM_REBUILD_THROTTLE_SECS: f32 = 0.5;

#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn maintain_world_grid_system(
    mut runtime: ResMut<ClientRuntime>,
    resource_nodes: Query<&ResourceNode>,
    deployables: Query<(&Deployable, &DeployableTransform, &DeployableActive)>,
    added_nodes: Query<(), Added<ResourceNode>>,
    added_deps: Query<(), Added<Deployable>>,
    // A door's collider moves between the closed plane and the swung
    // pose, so an `active` flip must retrigger the rebuild. Lightyear's
    // receive path can mark this changed without a real value change
    // (see CLAUDE.md), which only costs a fingerprint compare, the
    // open-door bits folded into the fingerprint below stop a spurious
    // rebuild.
    changed_active: Query<(), (Changed<DeployableActive>, With<Deployable>)>,
    mut removed_nodes: RemovedComponents<ResourceNode>,
    mut removed_deps: RemovedComponents<Deployable>,
    mut last_fingerprint: Local<Option<(u64, u64, u64)>>,
    mut last_world_version: Local<u64>,
    // Separate fingerprint for the grass-displacer subset (ground-resting
    // footprints only), so a door open/close or a wall/ceiling placement,
    // which moves `last_fingerprint` but not the grass set, never re-pushes
    // the displacer field or re-filters the detail grass.
    mut last_grass_fingerprint: Local<Option<u64>>,
    time: Res<Time>,
    stream: Res<crate::app::state::WorldStreamState>,
    // Wall-clock of the last full grid rebuild, to throttle rebuilds during the
    // initial world stream (see the guard below).
    mut last_rebuild_secs: Local<f32>,
) {
    let world_version = runtime.world_version;
    // Cheap probe: skip the O(N) fingerprint scan when the entity sets
    // and world version are unchanged. `.count()` drains all events
    // so the cursor doesn't carry stale counts across frames.
    let world_changed = world_version != *last_world_version || last_fingerprint.is_none();
    let removed_count = removed_nodes.read().count() + removed_deps.read().count();
    let added_any = !added_nodes.is_empty() || !added_deps.is_empty();
    if !world_changed && !added_any && removed_count == 0 && changed_active.is_empty() {
        return;
    }

    // During the initial world stream the AoI floods in over several seconds and
    // `Added` fires on nearly every frame, so the O(all-entities) rebuild below
    // would run every frame and freeze the loading splash. While the stream is
    // still arriving, coalesce rebuilds whose ONLY trigger is newly-streamed
    // entities to one per `GRID_STREAM_REBUILD_THROTTLE_SECS`. World-version
    // changes, removals, and door active-flips still rebuild immediately so live
    // collision stays correct; the grid (with its ground blocks) is still built
    // within the throttle window, so the settling player keeps ground collision.
    let now = time.elapsed_secs();
    if !stream.initial_stream_settled(now)
        && !world_changed
        && removed_count == 0
        && changed_active.is_empty()
        && now - *last_rebuild_secs < GRID_STREAM_REBUILD_THROTTLE_SECS
    {
        return;
    }

    // At least one change is plausible. Compute the actual fingerprint
    // and bail if it matches (e.g. Added fired for an entity whose id
    // already contributed to the prior fingerprint, shouldn't happen
    // but cheap to verify).
    let resource_node_version = resource_node_set_fingerprint(resource_nodes.iter());
    let deployable_version = deployable_set_fingerprint(deployables.iter());
    let current = (world_version, resource_node_version, deployable_version);
    *last_world_version = world_version;

    if *last_fingerprint == Some(current) {
        return;
    }
    // A real rebuild is about to happen; stamp it for the stream throttle above.
    *last_rebuild_secs = now;

    let resource_colliders: Vec<WorldBlock> = resource_nodes
        .iter()
        .filter_map(|node| resource_node_collider_at(&node.definition_id, node.position))
        .collect();
    let deployable_collider_blocks: Vec<WorldBlock> = deployables
        .iter()
        .flat_map(|(meta, transform, active)| deployable_colliders(meta, transform, active.0))
        .collect();
    // Detail grass is carved only by ground-resting footprints (foundations
    // + classic ground deployables); walls, ceilings, doorways, stairs, and
    // doors sit on a platform's edges/cells (elevated or vertical), and a
    // door's box even moves when it swings, so carving grass around them
    // reads as the grass jumping. Re-push the displacer field only when
    // that subset actually changes, so an open/close or a wall placement
    // costs nothing on the grass side.
    let grass_fingerprint = grass_displacer_fingerprint(deployables.iter());
    if *last_grass_fingerprint != Some(grass_fingerprint) {
        let grass_displacer_blocks: Vec<WorldBlock> = deployables
            .iter()
            .filter(|(meta, _, _)| deployable_displaces_grass(meta.kind))
            .flat_map(|(meta, transform, active)| deployable_colliders(meta, transform, active.0))
            .collect();
        runtime.set_grass_displacers(grass_displacer_blocks);
        *last_grass_fingerprint = Some(grass_fingerprint);
    }
    // The collision grid still gets every footprint (walls and doors block
    // movement even though they don't touch grass).
    runtime.rebuild_world_grid(resource_colliders, deployable_collider_blocks);
    *last_fingerprint = Some(current);
}

fn resource_node_set_fingerprint<'a>(iter: impl IntoIterator<Item = &'a ResourceNode>) -> u64 {
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for node in iter {
        // Skip ids that contribute no collider so the fingerprint stays
        // tight to the actual collision set, crude clutter (surface
        // stones, branch piles, hay grass) doesn't move the grid.
        if resource_node_collider_at(&node.definition_id, node.position).is_none() {
            continue;
        }
        hash ^= node.id.0;
        count += 1;
    }
    hash.wrapping_mul(FINGERPRINT_MIX).wrapping_add(count)
}

/// Whether a placed structure carves detail grass out from under it. Only
/// ground-resting footprints qualify: a foundation slab sits in the grass,
/// as do the classic deployables (furnace, workbench, bag, box, cupboard,
/// torch). Walls, window walls, doorways, ceilings, and stairs mount on a
/// foundation's edges/cells (elevated or vertical), and a door swings, so
/// carving grass around them looks wrong.
fn deployable_displaces_grass(kind: DeployableKind) -> bool {
    match kind {
        DeployableKind::Building { piece, .. } => {
            matches!(piece, crate::building::BuildingPiece::Foundation)
        }
        DeployableKind::Door { .. } => false,
        _ => true,
    }
}

/// Fingerprint of only the grass-displacing deployables (see
/// [`deployable_displaces_grass`]). Excludes the elevated/vertical building
/// pieces, doors, and a door's open state, so the detail-grass field is
/// re-carved only when a ground footprint is genuinely added or removed.
fn grass_displacer_fingerprint<'a>(
    iter: impl IntoIterator<
        Item = (
            &'a Deployable,
            &'a DeployableTransform,
            &'a DeployableActive,
        ),
    >,
) -> u64 {
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for (meta, _, _) in iter {
        if !deployable_displaces_grass(meta.kind) {
            continue;
        }
        hash ^= meta.id.0 ^ 0xD9E3_F1A7_5B6C_8024;
        count += 1;
    }
    hash.wrapping_mul(FINGERPRINT_MIX).wrapping_add(count)
}

fn deployable_set_fingerprint<'a>(
    iter: impl IntoIterator<
        Item = (
            &'a Deployable,
            &'a DeployableTransform,
            &'a DeployableActive,
        ),
    >,
) -> u64 {
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for (meta, _, active) in iter {
        // XOR ^ 0xD9E3_F1A7_5B6C_8024 ensures the deployable id space
        // (separate counter from resource nodes server-side) doesn't
        // accidentally cancel against a resource node id with the same
        // numeric value when the two fingerprints are tupled together.
        hash ^= meta.id.0 ^ 0xD9E3_F1A7_5B6C_8024;
        // Open doors contribute a different bit pattern than closed ones
        // so an open/close flip changes the fingerprint (the panel's
        // collider genuinely moved) without a kind lookup.
        if matches!(meta.kind, DeployableKind::Door { .. }) && active.0 {
            hash ^= meta.id.0.rotate_left(17) ^ 0x5A5A_5A5A_5A5A_5A5A;
        }
        count += 1;
    }
    hash.wrapping_mul(FINGERPRINT_MIX).wrapping_add(count)
}

/// Build the solid AABBs for a placed structure from its replicated
/// components. Classic deployables resolve a single square-footprint box
/// from their item profile; building blocks and doors use the building
/// module's box layouts (openings stay passable, a door's box follows
/// its open/closed pose). Empty if the item id no longer resolves (e.g.
/// a server using a newer item table than this client knows about, skip
/// the collider rather than crash, the renderer will still draw it).
pub(crate) fn deployable_colliders(
    meta: &Deployable,
    transform: &DeployableTransform,
    active: bool,
) -> Vec<WorldBlock> {
    match meta.kind {
        DeployableKind::Building { piece, .. } => {
            crate::building::building_collider_blocks(piece, transform.position, transform.yaw)
        }
        DeployableKind::Door { variant } => match variant {
            crate::items::DoorVariant::Shutter => {
                crate::building::shutter_collider_blocks(transform.position, transform.yaw, active)
            }
            _ => crate::building::door_collider_blocks(transform.position, transform.yaw, active),
        },
        _ => {
            let Some(profile) = item_definition(&meta.item_id).and_then(|def| def.deployable)
            else {
                return Vec::new();
            };
            let center = Vec3Net::new(
                transform.position.x,
                transform.position.y + profile.collider_half_height,
                transform.position.z,
            );
            let half = Vec3Net::new(
                profile.collider_half_width,
                profile.collider_half_height,
                profile.collider_half_width,
            );
            vec![WorldBlock::new(center, half)]
        }
    }
}

/// Material burst + impact audio for a just-upgraded building piece, two
/// pops up the piece's height so a 3 m wall doesn't celebrate only at its
/// feet. Rides the remote-impact pipeline (visual chips in `effects.rs`,
/// audio in `audio::impact`), so the feedback matches the new material:
/// wood chips for the wood tier, stone shards for stone.
fn emit_upgrade_burst(
    remote_impacts: &mut MessageWriter<crate::app::state::RemoteImpactEvent>,
    kind: DeployableKind,
    base: Vec3,
) {
    use crate::app::audio::surface::SurfaceMaterial;
    let (model, surface) = match kind {
        DeployableKind::Building { tier, .. } => match tier {
            crate::building::BuildingTier::Stone => {
                (crate::items::ItemModel::Pickaxe, SurfaceMaterial::Stone)
            }
            _ => (crate::items::ItemModel::Hatchet, SurfaceMaterial::Wood),
        },
        _ => return,
    };
    let heights = match kind {
        DeployableKind::Building {
            piece: crate::building::BuildingPiece::Foundation,
            ..
        } => [0.3, 0.5],
        // The ceiling slab is thin: keep both bursts hugging it.
        DeployableKind::Building {
            piece: crate::building::BuildingPiece::Ceiling,
            ..
        } => [0.1, 0.3],
        // Stairs: one burst low on the flight, one near the landing.
        DeployableKind::Building {
            piece: crate::building::BuildingPiece::Stairs,
            ..
        } => [0.6, 2.4],
        _ => [0.8, 2.2],
    };
    for (index, height) in heights.into_iter().enumerate() {
        remote_impacts.write(crate::app::state::RemoteImpactEvent {
            anchor: base + Vec3::new(0.0, height, 0.0),
            model,
            surface,
            effect_kind: crate::app::state::ImpactEffectKind::for_surface(surface),
            seed: (base.x.to_bits() ^ base.z.to_bits()).wrapping_add(index as u32 * 7919),
            is_player_hit: false,
        });
    }
}

/// Which material kind a deployable visual carries. The vertex-colour props
/// (workbench, furnace, storage, torch, tool cupboard, sleeping bag) are
/// cel-shaded ([`ToonMaterial`]); building pieces + doors keep their textured
/// `StandardMaterial` for now (see docs/toon-shading.md roadmap). The spawn sites
/// attach whichever component type this names, since `MeshMaterial3d<A>` and
/// `MeshMaterial3d<B>` are distinct components.
enum DeployableMaterial {
    Standard(Handle<StandardMaterial>),
    Toon(Handle<ToonMaterial>),
}

/// Attach the right material component for a deployable visual. Centralised so
/// the initial-spawn and tier-upgrade sites stay in sync.
fn insert_deployable_material(entity: &mut EntityCommands, material: DeployableMaterial) {
    match material {
        DeployableMaterial::Standard(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
        DeployableMaterial::Toon(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
    }
}

fn deployable_visual(
    assets: &DeployableVisualAssets,
    kind: DeployableKind,
) -> (Handle<Mesh>, DeployableMaterial) {
    let mesh = match kind {
        DeployableKind::Workbench { tier } => assets.workbench_mesh(tier),
        DeployableKind::Furnace { .. } => assets.furnace_mesh.clone(),
        DeployableKind::Building { piece, tier } => assets.building_mesh(piece, tier),
        // Doors get an animated panel child instead (see the spawn site);
        // the root mesh handle is unused but keeps this function total.
        DeployableKind::Door { variant } => assets.door_panel_mesh(variant),
        DeployableKind::SleepingBag => assets.sleeping_bag_mesh.clone(),
        DeployableKind::StorageBox { tier } => assets.storage_box_mesh(tier),
        DeployableKind::Torch { .. } => assets.torch_mesh.clone(),
        DeployableKind::ToolCupboard => assets.tool_cupboard_mesh.clone(),
        DeployableKind::RuinCache => assets.ruin_cache_mesh.clone(),
        // Placed charge: the authored charge body glb (primitive 0).
        DeployableKind::Explosive { kind } => charge_body_mesh(assets, kind),
    };
    // Building pieces carry their tier's textured `StandardMaterial` (twig /
    // timber / stone) and doors their variant material, both still PBR for now.
    // The free-standing deployables are cel-shaded: wood props share the toon wood
    // material, the furnace the toon stone material, and the sleeping bag the
    // toon fabric material.
    let material = match kind {
        DeployableKind::Building { tier, .. } => {
            DeployableMaterial::Standard(assets.building_material(tier))
        }
        DeployableKind::Door { variant } => {
            DeployableMaterial::Standard(assets.door_material(variant))
        }
        // The furnace reads as weathered stone.
        DeployableKind::Furnace { .. } => {
            DeployableMaterial::Toon(assets.toon_stone_material.clone())
        }
        // The salvage chest is charred wood under iron bands, so it rides the
        // wood plank line-art with its near-black COLOR_0 identity.
        DeployableKind::Workbench { .. }
        | DeployableKind::StorageBox { .. }
        | DeployableKind::ToolCupboard
        | DeployableKind::RuinCache
        | DeployableKind::Torch { .. } => {
            DeployableMaterial::Toon(assets.toon_wood_material.clone())
        }
        // A placed charge's body material by kind: the keg's staved barrel reads
        // as wood, the cloth-bodied bomb + satchel as fabric.
        DeployableKind::Explosive { kind } => {
            DeployableMaterial::Toon(charge_body_material(assets, kind))
        }
        DeployableKind::SleepingBag => {
            DeployableMaterial::Toon(assets.toon_fabric_material.clone())
        }
    };
    (mesh, material)
}

/// The body-primitive (primitive 0) mesh for a placed charge of `kind`. Shared
/// with the placement ghost so the preview matches the placed prop.
pub(super) fn charge_body_mesh(
    assets: &DeployableVisualAssets,
    kind: crate::items::ExplosiveKind,
) -> Handle<Mesh> {
    use crate::items::ExplosiveKind;
    match kind {
        ExplosiveKind::PowderKeg => assets.charge_keg_mesh.clone(),
        ExplosiveKind::SatchelCharge => assets.charge_satchel_mesh.clone(),
        ExplosiveKind::PowderBomb => assets.charge_bomb_mesh.clone(),
    }
}

/// The body-primitive cel material for a placed charge of `kind`.
fn charge_body_material(
    assets: &DeployableVisualAssets,
    kind: crate::items::ExplosiveKind,
) -> Handle<ToonMaterial> {
    use crate::items::ExplosiveKind;
    match kind {
        ExplosiveKind::PowderKeg => assets.toon_wood_material.clone(),
        ExplosiveKind::PowderBomb | ExplosiveKind::SatchelCharge => {
            assets.charge_cloth_material.clone()
        }
    }
}

pub(super) fn deployable_transform(position: Vec3, yaw: f32) -> Transform {
    Transform::from_translation(position).with_rotation(Quat::from_rotation_y(yaw))
}

/// How far a wall-mounted torch leans out from the wall, in radians (~36°). The
/// haft rocks up and away so the flame clears the masonry.
pub(super) const TORCH_WALL_TILT_RAD: f32 = 0.62;

/// Transform for a placed deployable, honouring the torch wall-mount tilt: a
/// wall torch leans up and out from the wall about its base along the stored
/// yaw (the outward direction); everything else stands upright. Shared by the
/// placed-entity spawn and the placement ghost so the preview matches.
pub(super) fn deployable_visual_transform(
    position: Vec3,
    yaw: f32,
    kind: DeployableKind,
) -> Transform {
    if matches!(kind, DeployableKind::Torch { wall: true }) {
        // yaw points away from the wall; the X-tilt rocks the torch's local
        // +Y (its shaft) toward that outward (+Z) direction.
        Transform::from_translation(position)
            .with_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_x(TORCH_WALL_TILT_RAD))
    } else {
        deployable_transform(position, yaw)
    }
}

/// Collect every placed foundation/ceiling as a [`ClaimPlatform`] (cell
/// centre + walkable top). Shared input for the perimeter-wall visual
/// inset: a wall is flush-mounted toward whichever side carries a platform
/// at its base height.
fn collect_building_platforms<'a>(
    deployables: impl Iterator<Item = (&'a Deployable, &'a DeployableTransform)>,
) -> Vec<ClaimPlatform> {
    deployables
        .filter_map(|(meta, transform)| {
            let DeployableKind::Building { piece, .. } = meta.kind else {
                return None;
            };
            let top = crate::building::platform_top_offset(piece)?;
            Some(ClaimPlatform {
                position: transform.position,
                top: transform.position.y + top,
            })
        })
        .collect()
}

/// World position to *render* a building piece at: a perimeter wall is
/// nudged inward by [`crate::building::wall_face_inset_offset`] so its
/// outer face is flush with the foundation edge; every other piece (and an
/// interior or unsupported wall) renders at its canonical collider
/// position. `platforms` is `None` when no inset is needed this frame.
fn wall_inset_visual_position(
    kind: DeployableKind,
    position: Vec3Net,
    yaw: f32,
    platforms: Option<&[ClaimPlatform]>,
) -> Vec3 {
    let base = Vec3::from(position);
    let DeployableKind::Building { piece, .. } = kind else {
        return base;
    };
    if !piece.is_wall_like() {
        return base;
    }
    let Some(platforms) = platforms else {
        return base;
    };
    base + Vec3::from(crate::building::wall_face_inset_offset(
        position, yaw, platforms,
    ))
}

/// Keep perimeter walls' rendered models flush with the foundation edge as
/// neighbours change. The visual nudge depends on which sides carry a
/// platform, so a wall placed on a lone foundation edge (perimeter, inset)
/// can later turn interior (centred) when a floor is added on its open
/// side, and a base streaming in may replicate a wall a frame before the
/// foundation backing it. Recompute every wall's visual transform whenever
/// the building set changes.
///
/// Event-gated (only does work on a structural change), per the
/// client-reconciler rule in CLAUDE.md. The canonical collider position is
/// never touched: this moves only the rendered model.
pub(crate) fn maintain_wall_visual_insets_system(
    visuals: Res<DeployedEntityVisuals>,
    all_deployables: Query<(&Deployable, &DeployableTransform)>,
    added: Query<(), Added<Deployable>>,
    mut removed: RemovedComponents<Deployable>,
    mut transforms: Query<&mut Transform, With<NetworkDeployedEntity>>,
) {
    let changed = !added.is_empty() | (removed.read().count() > 0);
    if !changed {
        return;
    }
    let platforms = collect_building_platforms(all_deployables.iter());
    for (meta, transform) in &all_deployables {
        let DeployableKind::Building { piece, .. } = meta.kind else {
            continue;
        };
        if !piece.is_wall_like() {
            continue;
        }
        let Some(entry) = visuals.entries.get(&meta.id) else {
            continue;
        };
        let Ok(mut visual) = transforms.get_mut(entry.entity) else {
            continue;
        };
        let visual_position = Vec3::from(transform.position)
            + Vec3::from(crate::building::wall_face_inset_offset(
                transform.position,
                transform.yaw,
                &platforms,
            ));
        *visual = deployable_visual_transform(visual_position, transform.yaw, meta.kind);
    }
}

/// Swinging panel child of a door visual. `angle` is the panel's current
/// hinge rotation in radians (0 = closed); `open` is the replicated
/// target state, written by the reconciler on real `DeployableActive`
/// flips so the animation never has to read the replicated set. The
/// door swing audio rides the same flip edge in [`apply_active_flip`].
#[derive(Component)]
pub(crate) struct DoorPanel {
    pub(crate) angle: f32,
    pub(crate) open: bool,
}

/// How fast the door panel sweeps, in rad/s. ~0.35 s for the full swing,
/// quick enough to feel responsive, slow enough to read as a real door.
const DOOR_SWING_SPEED_RAD_PER_SEC: f32 = 5.0;

/// Ease each door panel toward its target pose. Iterates only the panel
/// entities themselves (a handful per base); settled panels skip the
/// transform write, so an idle frame costs a float compare per door.
pub(crate) fn animate_door_panels_system(
    time: Res<Time>,
    mut panels: Query<(&mut Transform, &mut DoorPanel)>,
) {
    let step = DOOR_SWING_SPEED_RAD_PER_SEC * time.delta_secs();
    for (mut transform, mut panel) in panels.iter_mut() {
        // Negative yaw swings the +X panel toward local +Z, matching the
        // swing-arc indicator baked into the placement ghost.
        let target = if panel.open {
            -DOOR_OPEN_ANGLE_RAD
        } else {
            0.0
        };
        let delta = target - panel.angle;
        if delta.abs() < 1e-3 {
            continue;
        }
        panel.angle += delta.clamp(-step, step);
        transform.rotation = Quat::from_rotation_y(panel.angle);
    }
}

#[cfg(test)]
mod tests;
