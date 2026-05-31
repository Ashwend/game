use std::collections::{HashMap, HashSet, VecDeque};

use bevy::{camera::visibility::VisibilityRange, light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        audio::PlaySound,
        scene::{
            GrassMaterialHandle, ImpactEffectAssets, NetworkResourceNode, ResourceVisualAssets,
        },
        state::{ClientRuntime, ImpactEffectKind, PredictionState},
        systems::effects::spawn_impact_burst,
    },
    protocol::{ResourceNodeId, Vec3Net},
    resources::{ResourceNodeModel, resource_node_definition},
    server::ResourceNode,
};

/// Per-frame cap on resource-node *spawns*. Crossing a chunk boundary
/// can pull tens of trees and ores into view in one snapshot tick. Doing
/// every fresh `commands.spawn(...)` in the same frame produces a
/// command-buffer / GPU-upload stall the player sees as a hitch — the
/// "feels like 40 FPS even at 500 FPS" pattern. Anything past the budget
/// is left untouched in `previous_progress` so the *next* frame still
/// classifies it as fresh and runs its pop-in animation. The snapshot
/// stays valid until the next server tick (~50 ms), giving plenty of
/// frames to drain a backlog. Existing-entity transform updates and
/// despawns are uncapped — only first-time spawns are budgeted.
const MAX_RESOURCE_NODE_SPAWNS_PER_FRAME: usize = 8;

/// How long the "node emerges from the ground" animation runs. Short
/// enough to feel like a pop rather than a slow grow, long enough to
/// register as something happening.
const POP_IN_DURATION_SECS: f32 = 0.42;
/// How far below the floor the node starts on emerge. The mesh's bottom
/// sits at local y=0 so this pulls the rock/sapling fully into the
/// ground at t=0, then lifts back to flush.
const POP_IN_GROUND_OFFSET: f32 = 0.55;
/// Peak overshoot scale during the emergence pulse. The node briefly
/// pops slightly above its target size then settles, giving a "landed"
/// feel rather than a linear ramp.
const POP_IN_OVERSHOOT: f32 = 0.06;

/// Component attached to a freshly-spawned resource node while it
/// animates into view. The base transform is captured at spawn time so
/// the tick system can interpolate without re-reading the snapshot.
#[derive(Component, Debug, Clone)]
pub(crate) struct ResourceNodePopIn {
    elapsed: f32,
    base_transform: Transform,
}

/// How many frames a disappeared node sits in
/// `pending_depletion_check` before we conclude the
/// `ResourceNodeDepleted` message isn't coming and silent-despawn.
/// 3 frames at 60 FPS ≈ 50 ms, which is one full Lightyear server tick —
/// plenty of slack for the depleted message (reliable channel) to land
/// after the entity-despawn diff (replication channel).
const DEPLETION_GRACE_FRAMES: u8 = 3;

/// Visual entity whose replicated counterpart vanished but for which
/// we haven't yet seen a matching `ResourceNodeDepleted` server
/// message. The depleted message and the Lightyear entity-despawn ship
/// on different channels and can arrive in either order — keeping the
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
    /// FIFO queue of `Added<ResourceNode>` arrivals waiting on the
    /// per-frame spawn budget ([`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME`]).
    /// Persisting across frames keeps the spawn rate-limit working
    /// without re-iterating the replicated query each frame.
    pending_spawns: VecDeque<PendingSpawn>,
    /// Disappeared visuals waiting for a possible
    /// `ResourceNodeDepleted` message before deciding silent-despawn
    /// vs. death-effect. See [`PendingDepletion`] / the
    /// [`DEPLETION_GRACE_FRAMES`] grace window.
    pending_depletion_check: HashMap<ResourceNodeId, PendingDepletion>,
    /// `true` once at least one reconciliation pass has fired.
    /// Suppresses the fresh-node pop-in animation for the initial wave
    /// of world geometry — we don't want 30 trees and ores to all pop
    /// up the moment the player connects.
    applied_first_snapshot: bool,
    /// Node ids whose visual we've hidden for an unconfirmed predicted
    /// crude pickup (see [`PredictionState::is_node_hidden`]). Mirrors the
    /// prediction overlay's `hidden_nodes`; the local copy lets the reconcile
    /// detect the hide→un-hide / hide→despawn transitions without a per-frame
    /// scan of the full replicated set. Tiny in practice (in-flight pickups).
    suppressed: HashSet<ResourceNodeId>,
}

type ResourceEntityQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static NetworkResourceNode,
        &'static Mesh3d,
        // Optional: the hay-grass node uses `GrassMaterial` (the wind shader),
        // not `StandardMaterial`, so it has no entry here. Only the tree-felling
        // death effect actually reads the material; crude pickups (incl. hay
        // grass) ignore it, so `None` is fine for them.
        Option<&'static MeshMaterial3d<StandardMaterial>>,
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
/// just to detect "nothing changed" — a 1.4 ms median / 4 ms slow-
/// frame cost that showed up as the second bimodal peak in the frame
/// histogram. This version reads `Added<ResourceNode>` and
/// `RemovedComponents<ResourceNode>` so steady-state frames (no
/// arrivals, no departures, no pending work) do essentially no work.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_resource_nodes_system(
    mut commands: Commands,
    mut runtime: ResMut<ClientRuntime>,
    assets: Res<ResourceVisualAssets>,
    grass_material: Res<GrassMaterialHandle>,
    impact_assets: Res<ImpactEffectAssets>,
    mut play: MessageWriter<PlaySound>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut camera_kick: ResMut<crate::app::systems::CameraImpactKick>,
    mut entities: ResMut<ResourceNodeEntities>,
    prediction: Res<PredictionState>,
    resource_entities: ResourceEntityQuery,
    all_nodes: Query<(Entity, &ResourceNode)>,
    added_nodes: Query<(Entity, &ResourceNode), Added<ResourceNode>>,
    mut removed_nodes: RemovedComponents<ResourceNode>,
) {
    if runtime.client_id.is_none() {
        clear_all_tracked_nodes(&mut commands, &mut entities);
        return;
    }

    let entities = &mut *entities;

    // First-run catch-up. The `Added<T>` filter compares against the
    // system's `last_run` tick, which keeps advancing every frame the
    // system early-returns (i.e. while `client_id` is None during the
    // main menu). By the time we connect and Lightyear's replicated
    // entities arrive, `Added` won't fire for them on the first real
    // run because their add tick is older than `last_run`. So on the
    // first real run (`!applied_first_snapshot`), iterate the full
    // query once to seed the spawn queue and the reverse map. After
    // that, event-driven Added/Removed handles everything.
    if !entities.applied_first_snapshot {
        for (replicated_entity, node) in &all_nodes {
            entities.replicated_to_id.insert(replicated_entity, node.id);
            if entities.entities.contains_key(&node.id) {
                continue;
            }
            entities.pending_spawns.push_back(PendingSpawn {
                id: node.id,
                definition_id: node.definition_id.clone(),
                position: node.position,
                yaw: node.yaw,
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
            // Either we never tracked this entity (unlikely — we
            // populate `replicated_to_id` on every `Added`) or it
            // was cleaned up by `clear_all_tracked_nodes`. Either
            // way nothing to do.
            continue;
        };

        // If still queued for spawn, the mirror was never created —
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
        // the hidden mirror silently — no second death effect, no grace
        // window. Clearing `suppressed` here also keeps the reject-path
        // un-hide below from firing on an already-despawned node.
        if entities.suppressed.remove(&id) {
            commands.entity(mirror).despawn();
            entities.entities.remove(&id);
            runtime.depleted_node_ids.remove(&id);
            continue;
        }

        if runtime.depleted_node_ids.remove(&id) {
            // Server told us this was a depletion — death effect
            // fires immediately. No grace window needed.
            despawn_with_death_effect(
                &mut commands,
                &impact_assets,
                &mut play,
                &mut materials,
                &mut camera_kick,
                &resource_entities,
                player_position,
                Some(mirror),
            );
            entities.entities.remove(&id);
        } else {
            // No depletion message yet. Queue for grace — if the
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
    for (replicated_entity, node) in &added_nodes {
        // Skip entities we already know about — either the catch-up
        // above seeded them on the first run, or a previous Added
        // already enqueued them.
        if entities.replicated_to_id.contains_key(&replicated_entity) {
            continue;
        }
        entities.replicated_to_id.insert(replicated_entity, node.id);

        if entities.pending_depletion_check.remove(&node.id).is_some() {
            // AoI bounce — the mirror is still alive from before, so
            // no spawn needed and no pop-in (the visual stayed put).
            continue;
        }
        if entities.entities.contains_key(&node.id) {
            // Defensive: id is already tracked. Shouldn't happen but
            // skip rather than double-spawn.
            continue;
        }

        entities.pending_spawns.push_back(PendingSpawn {
            id: node.id,
            definition_id: node.definition_id.clone(),
            position: node.position,
            yaw: node.yaw,
        });
    }

    // 3. Drain the spawn queue up to the per-frame budget. The
    //    initial 1811-node fill takes ~226 frames at budget 8/frame
    //    to fully populate; thereafter the queue is usually empty
    //    and this loop is zero iterations.
    let pop_in_enabled = entities.applied_first_snapshot;
    let mut spawn_budget = MAX_RESOURCE_NODE_SPAWNS_PER_FRAME;
    while spawn_budget > 0 {
        let Some(spawn) = entities.pending_spawns.pop_front() else {
            break;
        };
        let Some(definition) = resource_node_definition(&spawn.definition_id) else {
            continue;
        };
        spawn_budget -= 1;
        let target_transform =
            resource_node_transform_at(spawn.position, spawn.yaw, definition.model);
        spawn_resource_node_entity(
            &mut commands,
            &assets,
            &grass_material,
            &impact_assets,
            entities,
            spawn.id,
            spawn.position,
            definition.model,
            target_transform,
            pop_in_enabled,
        );
    }

    // 4. Grace-period bookkeeping. Empty in steady state — when it
    //    is empty the function returns immediately without iterating.
    let consumed = resolve_pending_depletions(
        &mut commands,
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
    entities.applied_first_snapshot = false;
}

#[allow(clippy::too_many_arguments)]
fn despawn_with_death_effect(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<StandardMaterial>,
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
/// effect while merely *hiding* the mirror — the visual still has to survive
/// in case the server rejects the pickup and we un-hide it.
#[allow(clippy::too_many_arguments)]
fn fire_node_death_effect(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<StandardMaterial>,
    camera_kick: &mut crate::app::systems::CameraImpactKick,
    resource_entities: &ResourceEntityQuery,
    player_position: Option<Vec3>,
    entity: Entity,
) {
    if let Ok((resource, mesh, material, transform)) = resource_entities.get(entity) {
        // Only the tree-felling path uses the material (trees always carry a
        // `StandardMaterial`); crude/ore deaths ignore it, so a default handle
        // is fine for the materialless hay-grass node.
        let material = material.map(|m| m.0.clone()).unwrap_or_default();
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
            player_position,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_resource_node_entity(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    grass_material: &GrassMaterialHandle,
    impact_assets: &ImpactEffectAssets,
    entities: &mut ResourceNodeEntities,
    id: ResourceNodeId,
    position: Vec3Net,
    model: ResourceNodeModel,
    target_transform: Transform,
    should_pop_in: bool,
) {
    let (mesh, material) = resource_node_visual(assets, model);
    let lod_mesh = tree_lod_mesh(assets, model);
    let mut spawn_command = commands.spawn((
        Name::new(format!("Resource Node {id}")),
        NetworkResourceNode { id, model },
        Mesh3d(mesh),
        target_transform,
        Visibility::Visible,
    ));
    // Hay grass renders with the swaying grass shader (the *same* material as
    // the cosmetic detail grass, so they move in unison); every other node uses
    // its matte `StandardMaterial`.
    if model == ResourceNodeModel::HayGrass {
        spawn_command.insert(MeshMaterial3d(grass_material.0.clone()));
    } else {
        spawn_command.insert(MeshMaterial3d(material.clone()));
    }
    // Crude clutter (branch piles, surface stones, hay grass) spawns
    // densely — Plains chunks alone carry ~28 grass tufts plus stones
    // and sticks — and each casts a negligible-size shadow under its
    // own footprint. Skipping the shadow pass for these gets us a
    // meaningful per-frame win in populated areas without losing
    // readable silhouettes (trees and veins still cast).
    if model.is_crude() {
        spawn_command.insert(NotShadowCaster);
    }
    if should_pop_in {
        spawn_command.insert(ResourceNodePopIn {
            elapsed: 0.0,
            base_transform: target_transform,
        });
    }
    // Trees get a distance LOD: this (full-detail) mesh switches off past
    // `TREE_LOD_DISTANCE`; a child carrying the low-poly mesh switches on at the
    // same distance. Bevy's `VisibilityRange` does this GPU-side off the
    // existing visibility pass — no per-frame CPU cost. It's a hard step, not a
    // dither crossfade (see `TREE_LOD_DISTANCE` for why). Cuts main-pass vertex
    // throughput across a forest of distant trees (shadow distance is bounded
    // separately by the Shadows graphics setting).
    if lod_mesh.is_some() {
        spawn_command.insert(tree_lod_high_range());
    }
    let entity = spawn_command.id();
    entities.entities.insert(id, entity);

    if let Some(lod_mesh) = lod_mesh {
        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                Name::new(format!("Resource Node {id} LOD")),
                Mesh3d(lod_mesh),
                MeshMaterial3d(material),
                tree_lod_low_range(),
                Transform::default(),
                Visibility::Visible,
            ));
        });
    }

    if should_pop_in {
        spawn_pop_in_chip_burst(commands, impact_assets, id, position, model);
    }
}

/// Distance (m from camera) at which a tree switches from its full-detail mesh
/// to the low-poly LOD.
///
/// We deliberately use a **hard switch, not a dither crossfade**. Bevy's
/// `VisibilityRange` crossfade is a screen-space stochastic dither in the
/// fragment shader, and it misbehaves against this camera's post-process stack
/// (HDR + bloom + the fullscreen atmosphere pass + FXAA): inside the fade band
/// the dithered fragments get discarded, so a tree at ~LOD distance randomly
/// renders as nothing but its shadow. See the upstream reports
/// (<https://github.com/bevyengine/bevy/issues/17643>,
/// <https://github.com/bevyengine/bevy/pull/16286>). Zero-width margins below
/// disable the dither entirely; the trade-off is a small LOD pop at this
/// distance instead of a fade.
const TREE_LOD_DISTANCE: f32 = 80.0;

/// The low-poly LOD mesh for a tree model, or `None` for non-tree nodes.
fn tree_lod_mesh(assets: &ResourceVisualAssets, model: ResourceNodeModel) -> Option<Handle<Mesh>> {
    Some(match model {
        ResourceNodeModel::PineTreeSmall => assets.pine_tree_small_lod_mesh.clone(),
        ResourceNodeModel::PineTreeMedium => assets.pine_tree_medium_lod_mesh.clone(),
        ResourceNodeModel::PineTreeLarge => assets.pine_tree_large_lod_mesh.clone(),
        ResourceNodeModel::BirchTreeSmall => assets.birch_tree_small_lod_mesh.clone(),
        ResourceNodeModel::BirchTreeMedium => assets.birch_tree_medium_lod_mesh.clone(),
        ResourceNodeModel::BirchTreeLarge => assets.birch_tree_large_lod_mesh.clone(),
        _ => return None,
    })
}

/// `VisibilityRange` for the full-detail mesh: visible up close, hard cutoff at
/// `TREE_LOD_DISTANCE`. Zero-width margins = a step switch, no dither crossfade
/// (see [`TREE_LOD_DISTANCE`] for why we avoid the crossfade).
fn tree_lod_high_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: TREE_LOD_DISTANCE..TREE_LOD_DISTANCE,
        use_aabb: false,
    }
}

/// `VisibilityRange` for the low-poly mesh: hard switch-in at
/// `TREE_LOD_DISTANCE`, then visible out to the (well-beyond-far-plane) cutoff.
/// The switch-in distance is identical to the high mesh's cutoff so exactly one
/// LOD is visible at any distance — no gap, no overlap.
fn tree_lod_low_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: TREE_LOD_DISTANCE..TREE_LOD_DISTANCE,
        end_margin: 10_000.0..10_000.0,
        use_aabb: false,
    }
}

/// A short upward chip burst sells the "fresh from the ground" moment.
/// Trees throw wood chips, ores throw stone shards, crude nodes throw
/// their own small per-kind burst — same palette as gather impacts so
/// the visual language stays consistent.
fn spawn_pop_in_chip_burst(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    id: ResourceNodeId,
    position: Vec3Net,
    model: ResourceNodeModel,
) {
    let burst_anchor = Vec3::from(position) + Vec3::Y * 0.18;
    let kind = ImpactEffectKind::for_resource_model(model);
    let seed = (id as u32).wrapping_mul(0x9E37_79B1);
    spawn_impact_burst(
        commands,
        impact_assets,
        kind,
        burst_anchor,
        Vec3::Y,
        seed,
        0.65,
    );
}

/// Walk the `pending_depletion_check` map: any id that has since shown
/// up in `depleted_this_frame` fires the death-effect immediately; any
/// id past the grace window silent-despawns. Returns consumed
/// depletion ids.
#[allow(clippy::too_many_arguments)]
fn resolve_pending_depletions(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<StandardMaterial>,
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
            consumed.push(id);
            despawn_with_death_effect(
                commands,
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
/// * **New hide** — id is predicted-hidden but not yet suppressed: play the
///   depletion effect once and hide the mirror, so the node vanishes the
///   instant the player presses E (the dropped-item pickup feel, extended to
///   the much-more-numerous resource nodes).
/// * **Un-hide** — id is suppressed but no longer predicted-hidden while its
///   mirror still exists: the server *rejected* the pickup. (A confirmed
///   despawn instead clears `suppressed` and removes the entity in the
///   departures pass, which runs first — so reaching here with a live mirror
///   means a revert.) Make the node visible again.
#[allow(clippy::too_many_arguments)]
fn reconcile_predicted_pickups(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<StandardMaterial>,
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

/// Drives the "emerge from the ground" animation attached to freshly
/// (re)spawned resource nodes. Removes the component once the curve
/// settles, after which the entity returns to snapshot-driven transforms.
pub(crate) fn tick_resource_node_pop_in_system(
    mut commands: Commands,
    time: Res<Time>,
    mut popping_in: Query<(Entity, &mut Transform, &mut ResourceNodePopIn)>,
) {
    let dt = time.delta_secs().clamp(0.0, 0.1);
    if dt == 0.0 {
        return;
    }
    for (entity, mut transform, mut pop_in) in &mut popping_in {
        pop_in.elapsed += dt;
        let finished = pop_in.elapsed >= POP_IN_DURATION_SECS;
        *transform = pop_in_transform(pop_in.base_transform, pop_in.elapsed);
        if finished {
            commands.entity(entity).remove::<ResourceNodePopIn>();
        }
    }
}

/// Pure math behind the pop-in transform. Pulled out of the system so
/// it can be exercised without spinning up a Bevy world.
fn pop_in_transform(base: Transform, elapsed: f32) -> Transform {
    let raw = (elapsed / POP_IN_DURATION_SECS).clamp(0.0, 1.0);
    if raw >= 1.0 {
        return base;
    }
    let ease = ease_out_cubic(raw);
    // Lift from below the floor to flush, with a brief overshoot beyond
    // unit scale that settles back to 1.0 — reads as the node "thudding"
    // into place rather than easing to a stop.
    let height = -POP_IN_GROUND_OFFSET * (1.0 - ease);
    let overshoot = if raw < 0.7 {
        POP_IN_OVERSHOOT * (raw / 0.7)
    } else {
        POP_IN_OVERSHOOT * (1.0 - (raw - 0.7) / 0.3)
    };
    let scale_factor = ease + overshoot * (raw * (1.0 - raw) * 4.0);
    let mut next = base;
    next.translation.y = base.translation.y + height;
    next.scale = base.scale * scale_factor.max(0.0);
    next
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

pub(crate) fn resource_node_transform_at(
    position: Vec3Net,
    yaw: f32,
    model: ResourceNodeModel,
) -> Transform {
    // Trees bake their full size into the mesh and sit on the ground at
    // unit scale, which keeps each variant a single canonical mesh that
    // can be GPU-instanced. Ore nodes keep their per-instance scale
    // jitter for shape variety. Both the tree trunks and the ore rock
    // lumps have their lowest vertices at local y=0, so no height offset
    // is needed — adding one would float the geometry above the floor.
    let (height_offset, scale) = match model {
        ResourceNodeModel::CoalOre => (0.0, Vec3::new(1.0, 1.0, 1.0)),
        ResourceNodeModel::IronOre => (0.0, Vec3::new(1.1, 1.05, 0.95)),
        ResourceNodeModel::SulfurOre => (0.0, Vec3::new(0.96, 0.92, 1.06)),
        // Stone veins are wider/flatter than ore mounds — they read as
        // an outcrop rather than a focused deposit.
        ResourceNodeModel::StoneVein => (0.0, Vec3::new(1.18, 0.86, 1.08)),
        ResourceNodeModel::PineTreeSmall
        | ResourceNodeModel::PineTreeMedium
        | ResourceNodeModel::PineTreeLarge
        | ResourceNodeModel::BirchTreeSmall
        | ResourceNodeModel::BirchTreeMedium
        | ResourceNodeModel::BirchTreeLarge => (0.0, Vec3::ONE),
        ResourceNodeModel::SurfaceStone => (0.0, Vec3::new(0.9, 0.9, 0.9)),
        ResourceNodeModel::BranchPile => (0.0, Vec3::ONE),
        ResourceNodeModel::HayGrass => (0.0, Vec3::ONE),
    };
    Transform::from_xyz(position.x, position.y + height_offset, position.z)
        .with_rotation(Quat::from_rotation_y(yaw))
        .with_scale(scale)
}

pub(crate) fn resource_node_visual(
    assets: &ResourceVisualAssets,
    model: ResourceNodeModel,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    match model {
        ResourceNodeModel::CoalOre => (assets.coal_node_mesh.clone(), assets.coal_material.clone()),
        ResourceNodeModel::IronOre => (assets.iron_node_mesh.clone(), assets.iron_material.clone()),
        ResourceNodeModel::SulfurOre => (
            assets.sulfur_node_mesh.clone(),
            assets.sulfur_material.clone(),
        ),
        ResourceNodeModel::StoneVein => (
            assets.stone_vein_mesh.clone(),
            assets.stone_vein_material.clone(),
        ),
        ResourceNodeModel::PineTreeSmall => (
            assets.pine_tree_small_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::PineTreeMedium => (
            assets.pine_tree_medium_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::PineTreeLarge => (
            assets.pine_tree_large_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeSmall => (
            assets.birch_tree_small_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeMedium => (
            assets.birch_tree_medium_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeLarge => (
            assets.birch_tree_large_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::SurfaceStone => (
            assets.surface_stone_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BranchPile => (
            assets.branch_pile_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::HayGrass => (
            assets.hay_grass_mesh.clone(),
            assets.vertex_material.clone(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Vec3Net;
    use crate::resources::{COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID};

    #[test]
    fn pop_in_starts_below_floor_and_settles_to_base_transform() {
        let base = Transform::from_xyz(3.0, 0.0, -2.0).with_scale(Vec3::ONE);

        // At t=0 the node is fully buried — the very first frame the
        // animation runs the entity should be at the deepest point.
        let at_start = pop_in_transform(base, 0.0);
        assert!(
            at_start.translation.y < base.translation.y - 0.4,
            "pop-in should start well below the floor, got {at_start:?}"
        );
        assert!(at_start.scale.length() <= base.scale.length() + 1e-3);

        // Mid-curve the node is on its way up and slightly above unit
        // scale (the overshoot pulse), but still below its final y.
        let mid = pop_in_transform(base, POP_IN_DURATION_SECS * 0.6);
        assert!(mid.translation.y > at_start.translation.y);
        assert!(mid.translation.y < base.translation.y);

        // Past the window the result snaps exactly back to the base
        // transform so subsequent snapshot updates take over cleanly.
        let after = pop_in_transform(base, POP_IN_DURATION_SECS + 1.0);
        assert_eq!(after.translation, base.translation);
        assert_eq!(after.scale, base.scale);
    }

    #[test]
    fn ore_transform_matches_spawn_y_so_rock_sits_on_ground() {
        // The ore meshes have their lowest vertex at local y=0, so the
        // transform must not raise them above the floor.
        for ore_id in [COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID] {
            let position = Vec3Net::new(2.0, 0.0, -3.0);
            let definition = crate::resources::resource_node_definition(ore_id).unwrap();
            let transform = resource_node_transform_at(position, 0.0, definition.model);
            assert_eq!(
                transform.translation.y, position.y,
                "{ore_id} mesh must sit at the spawn y (no floating offset)"
            );
        }
    }

    #[test]
    fn ease_out_cubic_spans_zero_to_one_monotonically() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-6);
        // Eased value leads a linear ramp in the middle (ease-out).
        assert!(ease_out_cubic(0.5) > 0.5);
        // Clamped below 0 and above 1.
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert!((ease_out_cubic(2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pop_in_overshoots_above_unit_scale_mid_curve() {
        let base = Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::ONE);
        // Just past the overshoot peak (raw ~0.7) the node briefly scales
        // beyond its base size before settling.
        let mid = pop_in_transform(base, POP_IN_DURATION_SECS * 0.65);
        assert!(mid.scale.length() > base.scale.length());
    }

    #[test]
    fn tree_transform_keeps_unit_scale_on_the_ground() {
        let position = Vec3Net::new(1.0, 0.0, 2.0);
        let transform = resource_node_transform_at(position, 0.5, ResourceNodeModel::PineTreeLarge);
        assert_eq!(transform.scale, Vec3::ONE);
        assert_eq!(transform.translation.y, position.y);
        // Yaw is applied as a rotation about Y.
        let expected = Quat::from_rotation_y(0.5);
        assert!(transform.rotation.dot(expected).abs() > 1.0 - 1e-5);
    }

    #[test]
    fn ore_models_carry_per_model_scale_jitter() {
        let position = Vec3Net::new(0.0, 0.0, 0.0);
        let iron = resource_node_transform_at(position, 0.0, ResourceNodeModel::IronOre);
        let coal = resource_node_transform_at(position, 0.0, ResourceNodeModel::CoalOre);
        // Iron has a distinct non-uniform scale; coal stays at unit scale.
        assert_ne!(iron.scale, coal.scale);
        assert_eq!(coal.scale, Vec3::ONE);
    }
}
