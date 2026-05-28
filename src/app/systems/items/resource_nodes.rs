use std::collections::{HashMap, HashSet};

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        audio::PlaySound,
        scene::{ImpactEffectAssets, NetworkResourceNode, ResourceVisualAssets},
        state::{ClientRuntime, ImpactEffectKind},
        systems::effects::spawn_impact_burst,
    },
    protocol::{ResourceNodeId, Vec3Net},
    resources::{ResourceNodeModel, resource_node_definition},
    server::{ResourceNode, ResourceNodeStorage},
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

/// Persistent `id → Entity` lookup. Mirrors the live replicated set so
/// the per-frame reconciliation doesn't have to rebuild it from a
/// `Query`.
#[derive(Resource, Default)]
pub(crate) struct ResourceNodeEntities {
    pub(crate) entities: HashMap<ResourceNodeId, Entity>,
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
}

type ResourceEntityQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static NetworkResourceNode,
        &'static Mesh3d,
        &'static MeshMaterial3d<StandardMaterial>,
        &'static Transform,
    ),
>;

/// Reconcile the local `NetworkResourceNode` visuals against the
/// Lightyear-replicated `(ResourceNode, ResourceNodeStorage)` entities.
/// Spawn missing ones (rate-limited to
/// [`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME`]), refresh transforms,
/// despawn any that left the AoI ring. Despawns gated by the runtime's
/// `depleted_node_ids` set fire the death-effect; everything else is
/// a silent AoI-leave despawn.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_resource_nodes_system(
    mut commands: Commands,
    mut runtime: ResMut<ClientRuntime>,
    assets: Res<ResourceVisualAssets>,
    impact_assets: Res<ImpactEffectAssets>,
    mut play: MessageWriter<PlaySound>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut camera_kick: ResMut<crate::app::systems::CameraImpactKick>,
    mut entities: ResMut<ResourceNodeEntities>,
    resource_entities: ResourceEntityQuery,
    popping_in: Query<(), With<ResourceNodePopIn>>,
    replicated_nodes: Query<(&ResourceNode, &ResourceNodeStorage)>,
) {
    if runtime.client_id.is_none() {
        clear_all_tracked_nodes(&mut commands, &mut entities);
        return;
    }

    let entities = &mut *entities;
    let pop_in_enabled = entities.applied_first_snapshot;

    let player_position = runtime
        .local_view()
        .map(|view| Vec3::from(view.position) + Vec3::Y * crate::app::EYE_HEIGHT);

    // Snapshot the depleted-id set for this frame: nodes the server told us
    // are *actually* gone (gathered out, picked up). Anything that drops
    // from the replicated set but isn't here just left the player's AoI
    // and should despawn silently — no death animation, no camera kick.
    let depleted_this_frame: HashSet<ResourceNodeId> = runtime.depleted_node_ids.clone();
    let mut spawn_budget = MAX_RESOURCE_NODE_SPAWNS_PER_FRAME;
    let mut visible_ids: HashSet<ResourceNodeId> = HashSet::new();
    for (node, _storage) in &replicated_nodes {
        visible_ids.insert(node.id);
        // Replicated entity is back — if it was pending depletion,
        // clear that pending entry (e.g. AoI bounce, or regrow reusing
        // the id).
        entities.pending_depletion_check.remove(&node.id);
        let Some(definition) = resource_node_definition(&node.definition_id) else {
            continue;
        };

        let arrived_fresh = !entities.entities.contains_key(&node.id) && pop_in_enabled;

        let target_transform =
            resource_node_transform_at(node.position, node.yaw, definition.model);

        if let Some(entity) = entities.entities.get(&node.id).copied() {
            // An entity that's mid-pop-in owns its own transform — the
            // pop-in tick system is the only writer until the animation
            // completes. Replicated updates can still nudge other
            // components, but this prevents a one-frame jump on the
            // first frame.
            if !popping_in.contains(entity) {
                commands.entity(entity).insert(target_transform);
            }
            continue;
        }

        if spawn_budget == 0 {
            // Defer to a later frame. The replicated entity stays put,
            // so a subsequent invocation picks it up; the cleanup
            // pass below only despawns ids that left the replicated
            // set.
            continue;
        }
        spawn_budget -= 1;

        spawn_resource_node_entity(
            &mut commands,
            &assets,
            &impact_assets,
            entities,
            node.id,
            node.position,
            definition.model,
            target_transform,
            arrived_fresh,
        );
    }

    let mut consumed = resolve_pending_depletions(
        &mut commands,
        &impact_assets,
        &mut play,
        &mut materials,
        &mut camera_kick,
        &resource_entities,
        entities,
        &depleted_this_frame,
        player_position,
    );
    consumed.extend(queue_missing_nodes(
        &mut commands,
        &impact_assets,
        &mut play,
        &mut materials,
        &mut camera_kick,
        &resource_entities,
        entities,
        &visible_ids,
        &depleted_this_frame,
        player_position,
    ));

    entities.applied_first_snapshot = true;

    // Clear the consumed depletion ids so they don't fire twice if the
    // same id is somehow re-emitted later (it won't, but keeping the
    // set tight makes the invariant local rather than global).
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
    if let Ok((resource, mesh, material, transform)) = resource_entities.get(entity) {
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
            material.0.clone(),
            player_position,
        );
    }
    commands.entity(entity).despawn();
}

#[allow(clippy::too_many_arguments)]
fn spawn_resource_node_entity(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    impact_assets: &ImpactEffectAssets,
    entities: &mut ResourceNodeEntities,
    id: ResourceNodeId,
    position: Vec3Net,
    model: ResourceNodeModel,
    target_transform: Transform,
    should_pop_in: bool,
) {
    let (mesh, material) = resource_node_visual(assets, model);
    let mut spawn_command = commands.spawn((
        Name::new(format!("Resource Node {id}")),
        NetworkResourceNode { id, model },
        Mesh3d(mesh),
        MeshMaterial3d(material),
        target_transform,
        Visibility::Visible,
    ));
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
    let entity = spawn_command.id();
    entities.entities.insert(id, entity);

    if should_pop_in {
        spawn_pop_in_chip_burst(commands, impact_assets, id, position, model);
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

/// Sweeps the tracked-entities map for ids that just disappeared from
/// the replicated set. Two paths:
///
/// - The id is already in `depleted_this_frame` → the server's
///   `ResourceNodeDepleted` message beat the Lightyear despawn diff
///   to the client. Fire the death effect immediately.
/// - Otherwise → queue the id in `pending_depletion_check` so the
///   death effect can still fire if the depleted message arrives
///   within [`DEPLETION_GRACE_FRAMES`] frames. After that we treat it
///   as an AoI-leave.
#[allow(clippy::too_many_arguments)]
fn queue_missing_nodes(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<StandardMaterial>,
    camera_kick: &mut crate::app::systems::CameraImpactKick,
    resource_entities: &ResourceEntityQuery,
    entities: &mut ResourceNodeEntities,
    visible_ids: &HashSet<ResourceNodeId>,
    depleted_this_frame: &HashSet<ResourceNodeId>,
    player_position: Option<Vec3>,
) -> Vec<ResourceNodeId> {
    let to_handle: Vec<(ResourceNodeId, Entity)> = entities
        .entities
        .iter()
        .filter(|(id, _)| {
            !visible_ids.contains(id) && !entities.pending_depletion_check.contains_key(id)
        })
        .map(|(id, entity)| (*id, *entity))
        .collect();
    let mut consumed = Vec::new();
    for (id, entity) in to_handle {
        if depleted_this_frame.contains(&id) {
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
                Some(entity),
            );
        } else {
            // Hold the visual entity for a few frames in case the
            // depleted message is still in flight.
            entities.pending_depletion_check.insert(
                id,
                PendingDepletion {
                    entity,
                    frames_waited: 0,
                },
            );
        }
    }
    consumed
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
}
