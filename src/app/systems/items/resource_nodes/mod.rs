use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::{
    app::{
        audio::PlaySound,
        scene::{ImpactEffectAssets, NetworkResourceNode, ResourceVisualAssets, ToonMaterial},
        state::{ClientRuntime, MenuState, PredictionState, WorldStreamState},
    },
    protocol::{ResourceNodeId, Vec3Net},
    resources::resource_node_definition,
    server::{ResourceNode, ResourceNodeStorage},
};

mod hay_sway;
mod pop_in;
mod spawn;
mod stages;

#[cfg(test)]
mod tests;

pub(crate) use hay_sway::sway_hay_grass_system;
pub(crate) use pop_in::{resource_node_transform_at, tick_resource_node_pop_in_system};
pub(crate) use spawn::{insert_resource_node_material, resource_node_visual, tree_foliage_visual};
pub(crate) use stages::apply_resource_node_stage_system;

use spawn::spawn_resource_node_entity;
use stages::initial_node_stage;

/// Per-frame cap on resource-node *spawns*. Crossing a chunk boundary
/// can pull tens of trees and ores into view in one snapshot tick. Doing
/// every fresh `commands.spawn(...)` in the same frame produces a
/// command-buffer / GPU-upload stall the player sees as a hitch, the
/// "feels like 40 FPS even at 500 FPS" pattern. Anything past the budget
/// is left untouched in `previous_progress` so the *next* frame still
/// classifies it as fresh and runs its pop-in animation. The snapshot
/// stays valid until the next server tick (~50 ms), giving plenty of
/// frames to drain a backlog. Existing-entity transform updates and
/// despawns are uncapped, only first-time spawns are budgeted.
const MAX_RESOURCE_NODE_SPAWNS_PER_FRAME: usize = 8;

/// The budget while the world-entry loading splash is up: the scene is hidden
/// behind an opaque overlay, so spawn-burst frame hitches are invisible and
/// the only thing the budget buys is a longer load. Draining aggressively
/// here shortens the "Placing N objects" wait; the smooth budget above takes
/// over the moment the world is revealed.
const MAX_RESOURCE_NODE_SPAWNS_PER_FRAME_LOADING: usize = 64;

/// Post-connect grace window during which freshly-spawned nodes appear
/// *without* the pop-in chip-burst, so the initial world materialises
/// quietly instead of as a field of bursts. Comfortably covers the
/// initial AoI delivery + drain (the ~1800-node fill takes a few seconds
/// at [`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME`]). No node can regrow this
/// soon (regrow is 5-15 min, see the chunk manager), so the grace never
/// swallows a genuine runtime pop-in.
const INITIAL_LOAD_QUIET_SECS: f32 = 5.0;

/// Past this distance (m from the player) a freshly-spawned node just
/// *appears* instead of playing the pop-in chip-burst. Distant pop-in is
/// the "things popping in on the horizon" the outward fill would
/// otherwise show; gating by distance keeps the satisfying grow only
/// where the player can read it (a nearby regrow), never at the AoI edge.
const POP_IN_VISIBLE_DISTANCE_M: f32 = 40.0;

/// Component attached to a freshly-spawned resource node while it
/// animates into view. The base transform is captured at spawn time so
/// the tick system can interpolate without re-reading the snapshot.
#[derive(Component, Debug, Clone)]
pub(crate) struct ResourceNodePopIn {
    pub(super) elapsed: f32,
    pub(super) base_transform: Transform,
}

/// How many frames a disappeared node sits in
/// `pending_depletion_check` before we conclude the
/// `ResourceNodeDepleted` message isn't coming and silent-despawn.
/// 3 frames at 60 FPS ≈ 50 ms, which is one full Lightyear server tick,
/// plenty of slack for the depleted message (reliable channel) to land
/// after the entity-despawn diff (replication channel).
const DEPLETION_GRACE_FRAMES: u8 = 3;

/// Visual entity whose replicated counterpart vanished but for which
/// we haven't yet seen a matching `ResourceNodeDepleted` server
/// message. The depleted message and the Lightyear entity-despawn ship
/// on different channels and can arrive in either order, keeping the
/// visual alive for a few frames lets the death animation fire even
/// when the despawn lands first.
#[derive(Debug, Clone, Copy)]
struct PendingDepletion {
    entity: Entity,
    frames_waited: u8,
}

/// Spawn data captured from an `Added<ResourceNode>` event and held in
/// [`ResourceNodeEntities::pending_spawns`] until the per-frame spawn
/// budget admits it. Carrying the data here (rather than re-querying
/// the replicated entity each frame) lets the system iterate the queue
/// instead of the whole `replicated_nodes` query.
struct PendingSpawn {
    id: ResourceNodeId,
    definition_id: String,
    position: Vec3Net,
    yaw: f32,
    /// Replicated bare-dead-tree flag (server-authoritative, see
    /// `ResourceNode::dead`); the spawn renders a snag mesh when set.
    dead: bool,
    /// Visual depletion stage at enqueue time (always 0 for anything but
    /// a part-mined ore/vein). If the storage changes while the spawn is
    /// still queued, the stage system refreshes this in place.
    stage: u8,
}

/// Persistent `id → Entity` lookup. Mirrors the live replicated set so
/// the per-frame reconciliation doesn't have to rebuild it from a
/// `Query`.
#[derive(Resource, Default)]
pub(crate) struct ResourceNodeEntities {
    pub(crate) entities: HashMap<ResourceNodeId, Entity>,
    /// Reverse lookup `Lightyear-replicated entity → ResourceNodeId`.
    /// Populated from `Added<ResourceNode>`, consumed from
    /// `RemovedComponents<ResourceNode>` so the system can find which
    /// node id was on a despawned entity without scanning.
    replicated_to_id: HashMap<Entity, ResourceNodeId>,
    /// Unordered set of `Added<ResourceNode>` arrivals waiting on the
    /// per-frame spawn budget ([`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME`]).
    /// Persisting across frames keeps the spawn rate-limit working
    /// without re-iterating the replicated query each frame. Drained
    /// nearest-to-player first (see the drain step), so this is a bag,
    /// not a FIFO: order within it carries no meaning.
    pending_spawns: Vec<PendingSpawn>,
    /// Current visual depletion stage per spawned ore/vein mirror (0 for
    /// every other model). The stage system compares freshly computed
    /// stages against this to detect real threshold crossings, replicated
    /// storage diffs that don't cross a threshold are no-ops here.
    stages: HashMap<ResourceNodeId, u8>,
    /// Disappeared visuals waiting for a possible
    /// `ResourceNodeDepleted` message before deciding silent-despawn
    /// vs. death-effect. See [`PendingDepletion`] / the
    /// [`DEPLETION_GRACE_FRAMES`] grace window.
    pending_depletion_check: HashMap<ResourceNodeId, PendingDepletion>,
    /// `true` once at least one reconciliation pass has fired. Gates the
    /// one-time catch-up scan (the `Added` filter can't see entities that
    /// arrived while this system was early-returning; see the note in the
    /// system body). The pop-in quieting for the initial world load is
    /// handled separately by [`Self::connected_at_secs`].
    applied_first_snapshot: bool,
    /// Wall-clock seconds (Bevy `Time::elapsed_secs`) at which the local
    /// player last connected, stamped once per session and cleared on
    /// disconnect. Freshly-spawned nodes stay silent (no pop-in) for
    /// [`INITIAL_LOAD_QUIET_SECS`] after this, so the whole initial world
    /// load appears quietly instead of as a field of chip-bursts.
    connected_at_secs: Option<f32>,
    /// Node ids whose visual we've hidden for an unconfirmed predicted
    /// crude pickup (see [`PredictionState::is_node_hidden`]). Mirrors the
    /// prediction overlay's `hidden_nodes`; the local copy lets the reconcile
    /// detect the hide→un-hide / hide→despawn transitions without a per-frame
    /// scan of the full replicated set. Tiny in practice (in-flight pickups).
    suppressed: HashSet<ResourceNodeId>,
}

impl ResourceNodeEntities {
    /// Replicated nodes still waiting on the per-frame spawn budget. The
    /// loading splash surfaces this as world-entry progress.
    pub(crate) fn pending_spawn_count(&self) -> usize {
        self.pending_spawns.len()
    }

    /// True once at least one reconciliation pass has run this session and
    /// the budgeted spawn queue is empty. Feeds the world-entry readiness
    /// gate (`world_ready_for_play`): the splash holds until the initial
    /// node backlog has fully materialised. Momentarily true between
    /// replication packets mid-stream; the splash's settle window absorbs
    /// that flicker.
    pub(crate) fn is_caught_up(&self) -> bool {
        self.applied_first_snapshot && self.pending_spawns.is_empty()
    }
}

type ResourceEntityQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static NetworkResourceNode,
        &'static Mesh3d,
        // Trees + ore/vein nodes carry a cel-shaded `ToonMaterial`; crude nodes
        // (branch piles, surface stones, hay) carry a `StandardMaterial` and so
        // match `None` here. Only the tree-felling death effect reads it (to clone
        // for the fade); ore/crude deaths ignore it, so `None` is harmless.
        Option<&'static MeshMaterial3d<ToonMaterial>>,
        &'static Transform,
    ),
>;

/// Reconcile the local `NetworkResourceNode` visuals against the
/// Lightyear-replicated `(ResourceNode, ResourceNodeStorage)` entities.
/// Spawn missing ones (rate-limited to
/// [`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME`]) and despawn any that left
/// the AoI ring. Despawns gated by the runtime's `depleted_node_ids`
/// set fire the death-effect; everything else is a silent AoI-leave.
///
/// **Event-driven** since [the pickup-target investigation]: the
/// previous design iterated all ~1811 replicated nodes every frame
/// just to detect "nothing changed", a 1.4 ms median / 4 ms slow-
/// frame cost that showed up as the second bimodal peak in the frame
/// histogram. This version reads `Added<ResourceNode>` and
/// `RemovedComponents<ResourceNode>` so steady-state frames (no
/// arrivals, no departures, no pending work) do essentially no work.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_resource_nodes_system(
    mut commands: Commands,
    mut runtime: ResMut<ClientRuntime>,
    time: Res<Time>,
    assets: Res<ResourceVisualAssets>,
    impact_assets: Res<ImpactEffectAssets>,
    mut play: MessageWriter<PlaySound>,
    mut materials: ResMut<Assets<ToonMaterial>>,
    mut camera_kick: ResMut<crate::app::systems::CameraImpactKick>,
    mut entities: ResMut<ResourceNodeEntities>,
    mut stream: ResMut<WorldStreamState>,
    menu: Res<MenuState>,
    prediction: Res<PredictionState>,
    resource_entities: ResourceEntityQuery,
    all_nodes: Query<(Entity, &ResourceNode, Option<&ResourceNodeStorage>)>,
    added_nodes: Query<(Entity, &ResourceNode, Option<&ResourceNodeStorage>), Added<ResourceNode>>,
    mut removed_nodes: RemovedComponents<ResourceNode>,
) {
    if runtime.client_id.is_none() {
        clear_all_tracked_nodes(&mut commands, &mut entities);
        stream.reset();
        return;
    }

    let entities = &mut *entities;
    stream.note_connected(time.elapsed_secs());

    // Stamp the connect time once so the initial world load can
    // materialise quietly (see the pop-in gate in the drain step below).
    // Cleared on disconnect by `clear_all_tracked_nodes`.
    if entities.connected_at_secs.is_none() {
        entities.connected_at_secs = Some(time.elapsed_secs());
    }

    // First-run catch-up. The `Added<T>` filter compares against the
    // system's `last_run` tick, which keeps advancing every frame the
    // system early-returns (i.e. while `client_id` is None during the
    // main menu). By the time we connect and Lightyear's replicated
    // entities arrive, `Added` won't fire for them on the first real
    // run because their add tick is older than `last_run`. So on the
    // first real run (`!applied_first_snapshot`), iterate the full
    // query once to seed the spawn queue and the reverse map. After
    // that, event-driven Added/Removed handles everything.
    // Replicated arrivals this frame, reported to the world-entry stream
    // tracker so the loading gate can wait for the server to finish the
    // initial send. Every row counts (even an AoI-bounce that needs no
    // spawn): what matters is that the wire is still delivering.
    let mut arrivals = 0usize;

    if !entities.applied_first_snapshot {
        for (replicated_entity, node, storage) in &all_nodes {
            arrivals += 1;
            entities.replicated_to_id.insert(replicated_entity, node.id);
            if entities.entities.contains_key(&node.id) {
                continue;
            }
            entities.pending_spawns.push(PendingSpawn {
                id: node.id,
                definition_id: node.definition_id.clone(),
                position: node.position,
                yaw: node.yaw,
                dead: node.dead,
                stage: initial_node_stage(&node.definition_id, storage),
            });
        }
    }

    let player_position = runtime
        .local_view()
        .map(|view| Vec3::from(view.position) + Vec3::Y * crate::app::EYE_HEIGHT);

    // 1. Departures. A Lightyear-replicated entity disappeared (AoI
    //    leave, depletion, server-side despawn). Look the id up via
    //    the reverse map and either fire the death effect (if the
    //    server already told us it was depleted) or queue grace.
    for replicated_entity in removed_nodes.read() {
        let Some(id) = entities.replicated_to_id.remove(&replicated_entity) else {
            // Either we never tracked this entity (unlikely, we
            // populate `replicated_to_id` on every `Added`) or it
            // was cleaned up by `clear_all_tracked_nodes`. Either
            // way nothing to do.
            continue;
        };

        // If still queued for spawn, the mirror was never created,
        // just drop the queue entry. The grace-period machinery
        // doesn't apply: there's no death animation to attach to
        // something that never appeared.
        let was_queued = entities.pending_spawns.iter().any(|s| s.id == id);
        if was_queued {
            entities.pending_spawns.retain(|s| s.id != id);
            continue;
        }

        let Some(mirror) = entities.entities.get(&id).copied() else {
            continue;
        };

        // Predicted crude pickup already played the depletion effect and
        // hid this node's visual. The server's confirming despawn (or an
        // AoI-leave before the command resolved) just finalises it: drop
        // the hidden mirror silently, no second death effect, no grace
        // window. Clearing `suppressed` here also keeps the reject-path
        // un-hide below from firing on an already-despawned node.
        if entities.suppressed.remove(&id) {
            commands.entity(mirror).despawn();
            entities.entities.remove(&id);
            entities.stages.remove(&id);
            runtime.depleted_node_ids.remove(&id);
            continue;
        }

        if runtime.depleted_node_ids.remove(&id) {
            // Server told us this was a depletion, death effect
            // fires immediately. No grace window needed.
            despawn_with_death_effect(
                &mut commands,
                &assets,
                &impact_assets,
                &mut play,
                &mut materials,
                &mut camera_kick,
                &resource_entities,
                player_position,
                Some(mirror),
            );
            entities.entities.remove(&id);
            entities.stages.remove(&id);
        } else {
            // No depletion message yet. Queue for grace, if the
            // message arrives within [`DEPLETION_GRACE_FRAMES`],
            // `resolve_pending_depletions` will fire the death
            // effect; otherwise the entity silent-despawns.
            entities.pending_depletion_check.insert(
                id,
                PendingDepletion {
                    entity: mirror,
                    frames_waited: 0,
                },
            );
        }
    }

    // 2. Arrivals. `Added<ResourceNode>` fires once per replicated
    //    entity, the frame after Lightyear spawns it. Record the
    //    reverse map and either cancel a pending depletion (AoI
    //    bounce / regrow reusing the id) or queue a spawn.
    for (replicated_entity, node, storage) in &added_nodes {
        // Skip entities we already know about, either the catch-up
        // above seeded them on the first run, or a previous Added
        // already enqueued them.
        if entities.replicated_to_id.contains_key(&replicated_entity) {
            continue;
        }
        arrivals += 1;
        entities.replicated_to_id.insert(replicated_entity, node.id);

        if entities.pending_depletion_check.remove(&node.id).is_some() {
            // AoI bounce, the mirror is still alive from before, so
            // no spawn needed and no pop-in (the visual stayed put).
            continue;
        }
        if entities.entities.contains_key(&node.id) {
            // Defensive: id is already tracked. Shouldn't happen but
            // skip rather than double-spawn.
            continue;
        }

        entities.pending_spawns.push(PendingSpawn {
            id: node.id,
            definition_id: node.definition_id.clone(),
            position: node.position,
            yaw: node.yaw,
            dead: node.dead,
            stage: initial_node_stage(&node.definition_id, storage),
        });
    }

    stream.note_arrivals(time.elapsed_secs(), arrivals);

    // 3. Drain the spawn queue up to the per-frame budget, NEAREST TO THE
    //    PLAYER FIRST. The initial ~1811-node fill takes ~226 frames at
    //    budget 8/frame to fully populate; ordering by distance means the
    //    ground around the player materialises first instead of filling in
    //    Lightyear's arrival order (which scatters across the whole AoI,
    //    leaving a blank foreground while distant nodes appear).
    //    `select_nth_unstable_by` partitions the nearest `budget` to the
    //    front in average O(n) with no allocation, so even the first-frame
    //    ~1800-node pass costs a fraction of a millisecond, and the whole
    //    block is skipped once the queue drains (steady state). Squared
    //    distance avoids the sqrt; the eye-height offset baked into
    //    `player_position` is negligible at these ranges.
    let initial_load_done = entities
        .connected_at_secs
        .is_some_and(|t0| time.elapsed_secs() - t0 >= INITIAL_LOAD_QUIET_SECS);
    let pop_in_dist_sq = POP_IN_VISIBLE_DISTANCE_M * POP_IN_VISIBLE_DISTANCE_M;
    let to_spawn: Vec<PendingSpawn> = {
        let pending = &mut entities.pending_spawns;
        let budget = if menu.world_entry_splash_active() {
            MAX_RESOURCE_NODE_SPAWNS_PER_FRAME_LOADING
        } else {
            MAX_RESOURCE_NODE_SPAWNS_PER_FRAME
        };
        let take = budget.min(pending.len());
        if let Some(player) = player_position
            && take > 0
            && take < pending.len()
        {
            pending.select_nth_unstable_by(take - 1, |a, b| {
                Vec3::from(a.position)
                    .distance_squared(player)
                    .total_cmp(&Vec3::from(b.position).distance_squared(player))
            });
        }
        pending.drain(..take).collect()
    };
    for spawn in to_spawn {
        let Some(definition) = resource_node_definition(&spawn.definition_id) else {
            continue;
        };
        // Pop-in (chip-burst + grow) only for a genuine near runtime event:
        // never during the initial world load (the quiet grace window), and
        // never for a distant spawn (a far AoI arrival or an out-of-sight
        // regrow). A nearby regrow still pops.
        let should_pop_in = initial_load_done
            && player_position.is_some_and(|player| {
                Vec3::from(spawn.position).distance_squared(player) <= pop_in_dist_sq
            });
        let target_transform =
            resource_node_transform_at(spawn.id, spawn.position, spawn.yaw, definition.model);
        spawn_resource_node_entity(
            &mut commands,
            &assets,
            &impact_assets,
            entities,
            spawn.id,
            spawn.position,
            definition.model,
            spawn.dead,
            spawn.stage,
            target_transform,
            should_pop_in,
        );
    }

    // 4. Grace-period bookkeeping. Empty in steady state, when it
    //    is empty the function returns immediately without iterating.
    let consumed = resolve_pending_depletions(
        &mut commands,
        &assets,
        &impact_assets,
        &mut play,
        &mut materials,
        &mut camera_kick,
        &resource_entities,
        entities,
        &runtime.depleted_node_ids,
        player_position,
    );

    // 5. Predicted crude-pickup suppression. Runs after departures so a
    //    confirmed despawn has already cleared its `suppressed` entry,
    //    leaving only genuine new-hides and reject un-hides here. Both
    //    sets are tiny (in-flight pickups), so this never iterates the
    //    full replicated node set.
    reconcile_predicted_pickups(
        &mut commands,
        &assets,
        &impact_assets,
        &mut play,
        &mut materials,
        &mut camera_kick,
        &resource_entities,
        entities,
        &prediction,
        player_position,
    );

    entities.applied_first_snapshot = true;

    for id in consumed {
        runtime.depleted_node_ids.remove(&id);
    }
}

/// First-pass cleanup when the session ends (disconnect, world swap).
/// Resets the "have we ever applied a reconciliation pass?" flag so the
/// next batch of nodes doesn't all pop in at once like a re-entry
/// animation.
fn clear_all_tracked_nodes(commands: &mut Commands, entities: &mut ResourceNodeEntities) {
    for (_, entity) in entities.entities.drain() {
        commands.entity(entity).despawn();
    }
    for (_, entry) in entities.pending_depletion_check.drain() {
        commands.entity(entry.entity).despawn();
    }
    entities.replicated_to_id.clear();
    entities.pending_spawns.clear();
    entities.suppressed.clear();
    entities.stages.clear();
    entities.applied_first_snapshot = false;
    entities.connected_at_secs = None;
}

#[allow(clippy::too_many_arguments)]
fn despawn_with_death_effect(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<ToonMaterial>,
    camera_kick: &mut crate::app::systems::CameraImpactKick,
    resource_entities: &ResourceEntityQuery,
    player_position: Option<Vec3>,
    entity: Option<Entity>,
) {
    let Some(entity) = entity else {
        return;
    };
    fire_node_death_effect(
        commands,
        assets,
        impact_assets,
        play,
        materials,
        camera_kick,
        resource_entities,
        player_position,
        entity,
    );
    commands.entity(entity).despawn();
}

/// Spawn the node depletion effect (chip burst / tree-fall + sound + camera
/// kick) for `entity` *without* despawning it. Pulled out of
/// [`despawn_with_death_effect`] so the predicted-pickup path can play the
/// effect while merely *hiding* the mirror, the visual still has to survive
/// in case the server rejects the pickup and we un-hide it.
#[allow(clippy::too_many_arguments)]
fn fire_node_death_effect(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<ToonMaterial>,
    camera_kick: &mut crate::app::systems::CameraImpactKick,
    resource_entities: &ResourceEntityQuery,
    player_position: Option<Vec3>,
    entity: Entity,
) {
    if let Ok((resource, mesh, material, transform)) = resource_entities.get(entity) {
        // Only the tree-felling path uses the material (trees carry a cel-shaded
        // `ToonMaterial`); crude/ore deaths ignore it, so a default handle is fine
        // for the crude nodes that match `None` (they carry a `StandardMaterial`).
        let material = material.map(|m| m.0.clone()).unwrap_or_default();
        // The node entity carries the trunk mesh; the alpha-masked canopy lives on
        // a child. Re-derive it from the model so the felling tree falls + fades
        // with its foliage instead of dropping a bare trunk. `None` for non-trees
        // and for dead snags (which are bare, so they fell as just the trunk).
        let canopy = if resource.dead {
            None
        } else {
            tree_foliage_visual(assets, resource.model)
        };
        crate::app::systems::node_death::spawn_node_death(
            commands,
            impact_assets,
            play,
            materials,
            camera_kick,
            resource.id,
            resource.model,
            *transform,
            mesh.0.clone(),
            material,
            canopy,
            player_position,
        );
    }
}

/// Walk the `pending_depletion_check` map: any id that has since shown
/// up in `depleted_this_frame` fires the death-effect immediately; any
/// id past the grace window silent-despawns. Returns consumed
/// depletion ids.
#[allow(clippy::too_many_arguments)]
fn resolve_pending_depletions(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<ToonMaterial>,
    camera_kick: &mut crate::app::systems::CameraImpactKick,
    resource_entities: &ResourceEntityQuery,
    entities: &mut ResourceNodeEntities,
    depleted_this_frame: &HashSet<ResourceNodeId>,
    player_position: Option<Vec3>,
) -> Vec<ResourceNodeId> {
    let pending_ids: Vec<ResourceNodeId> =
        entities.pending_depletion_check.keys().copied().collect();
    let mut consumed = Vec::new();
    for id in pending_ids {
        let depleted = depleted_this_frame.contains(&id);
        if depleted {
            let entry = entities
                .pending_depletion_check
                .remove(&id)
                .expect("just iterated this key");
            entities.entities.remove(&id);
            entities.stages.remove(&id);
            consumed.push(id);
            despawn_with_death_effect(
                commands,
                assets,
                impact_assets,
                play,
                materials,
                camera_kick,
                resource_entities,
                player_position,
                Some(entry.entity),
            );
            continue;
        }
        let entry = entities
            .pending_depletion_check
            .get_mut(&id)
            .expect("just iterated this key");
        entry.frames_waited += 1;
        if entry.frames_waited >= DEPLETION_GRACE_FRAMES {
            let entry = entities
                .pending_depletion_check
                .remove(&id)
                .expect("just iterated this key");
            entities.entities.remove(&id);
            entities.stages.remove(&id);
            // AoI-leave: silent despawn. The depleted message never
            // arrived, so this was a chunk-boundary leave rather than
            // a real depletion.
            commands.entity(entry.entity).despawn();
        }
    }
    consumed
}

/// Reconcile predicted crude-pickup suppression against the prediction
/// overlay's `hidden_nodes`. Two transitions, both over the tiny in-flight
/// set (never the full node list, so this stays within the event-driven
/// budget the rest of the system was tuned to):
///
/// * **New hide**, id is predicted-hidden but not yet suppressed: play the
///   depletion effect once and hide the mirror, so the node vanishes the
///   instant the player presses E (the dropped-item pickup feel, extended to
///   the much-more-numerous resource nodes).
/// * **Un-hide**, id is suppressed but no longer predicted-hidden while its
///   mirror still exists: the server *rejected* the pickup. (A confirmed
///   despawn instead clears `suppressed` and removes the entity in the
///   departures pass, which runs first, so reaching here with a live mirror
///   means a revert.) Make the node visible again.
#[allow(clippy::too_many_arguments)]
fn reconcile_predicted_pickups(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<ToonMaterial>,
    camera_kick: &mut crate::app::systems::CameraImpactKick,
    resource_entities: &ResourceEntityQuery,
    entities: &mut ResourceNodeEntities,
    prediction: &PredictionState,
    player_position: Option<Vec3>,
) {
    for id in prediction.hidden_node_ids() {
        if entities.suppressed.contains(&id) {
            continue;
        }
        let Some(mirror) = entities.entities.get(&id).copied() else {
            // Not spawned locally yet (still queued / outside AoI). If it
            // appears while still predicted-hidden, a later pass hides it.
            continue;
        };
        fire_node_death_effect(
            commands,
            assets,
            impact_assets,
            play,
            materials,
            camera_kick,
            resource_entities,
            player_position,
            mirror,
        );
        commands.entity(mirror).insert(Visibility::Hidden);
        entities.suppressed.insert(id);
    }

    let stale: Vec<ResourceNodeId> = entities
        .suppressed
        .iter()
        .copied()
        .filter(|id| !prediction.is_node_hidden(*id))
        .collect();
    for id in stale {
        entities.suppressed.remove(&id);
        if let Some(mirror) = entities.entities.get(&id).copied() {
            commands.entity(mirror).insert(Visibility::Visible);
        }
    }
}
