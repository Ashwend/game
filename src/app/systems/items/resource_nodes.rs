use std::collections::{HashMap, HashSet};

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        audio::PlaySound,
        scene::{ImpactEffectAssets, NetworkResourceNode, ResourceVisualAssets},
        state::{ClientRuntime, ImpactEffectKind},
        systems::effects::spawn_impact_burst,
    },
    protocol::{ResourceNodeId, ResourceNodeState},
    resources::{ResourceNodeModel, resource_node_definition},
};

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

/// Persistent `id → Entity` lookup plus the previous tick's respawn
/// progress for each tracked node. The progress map lets the snapshot
/// system detect transitions (depleted → regenerating, regenerating →
/// ready) without persisting any extra state on the entity itself.
#[derive(Resource, Default)]
pub(crate) struct ResourceNodeEntities {
    pub(crate) entities: HashMap<ResourceNodeId, Entity>,
    /// `id → respawn_progress as of the last applied snapshot`.
    /// `None` means the node was ready to gather; `Some` means it was
    /// regenerating.
    previous_progress: HashMap<ResourceNodeId, Option<f32>>,
    /// `true` once at least one snapshot has been applied. Suppresses
    /// the fresh-node pop-in animation for the initial wave of world
    /// geometry — we don't want 30 trees and ores to all pop up the
    /// moment the player connects.
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
) {
    if runtime.snapshot.is_none() {
        clear_all_tracked_nodes(&mut commands, &mut entities);
        return;
    }

    let snapshot_ids: HashSet<ResourceNodeId> = runtime
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.resource_nodes.iter().map(|node| node.id).collect())
        .unwrap_or_default();
    let entities = &mut *entities;
    let pop_in_enabled = entities.applied_first_snapshot;

    let player_position = runtime
        .local_view()
        .map(|view| Vec3::from(view.position) + Vec3::Y * crate::app::EYE_HEIGHT);

    // Snapshot the depleted-id set for this frame: nodes the server told us
    // are *actually* gone (gathered out, picked up). Anything that drops
    // from the snapshot but isn't in this set just left the player's AoI
    // and should despawn silently — no death animation, no camera kick.
    let depleted_this_frame: HashSet<ResourceNodeId> = runtime.depleted_node_ids.clone();

    let snapshot_resource_nodes = runtime
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.resource_nodes.clone())
        .unwrap_or_default();
    for node in &snapshot_resource_nodes {
        let Some(definition) = resource_node_definition(&node.definition_id) else {
            continue;
        };

        let transition = entities.classify(node, pop_in_enabled);

        if transition.just_entered_regen {
            despawn_with_death_effect(
                &mut commands,
                &impact_assets,
                &mut play,
                &mut materials,
                &mut camera_kick,
                &resource_entities,
                player_position,
                entities.entities.remove(&node.id),
            );
        }

        // Regenerating nodes have no on-screen presence — skip any
        // entity creation or transform updates this frame.
        if node.respawn_progress.is_some() {
            continue;
        }

        let target_transform = resource_node_transform(node, definition.model);

        if let Some(entity) = entities.entities.get(&node.id).copied() {
            // An entity that's mid-pop-in owns its own transform — the
            // pop-in tick system is the only writer until the animation
            // completes. Snapshot reads can still nudge other components,
            // but this prevents a one-frame jump on the first frame.
            if !popping_in.contains(entity) {
                commands.entity(entity).insert(target_transform);
            }
            continue;
        }

        spawn_resource_node_entity(
            &mut commands,
            &assets,
            &impact_assets,
            entities,
            node,
            definition.model,
            target_transform,
            transition.should_pop_in,
        );
    }

    let consumed = despawn_nodes_missing_from_snapshot(
        &mut commands,
        &impact_assets,
        &mut play,
        &mut materials,
        &mut camera_kick,
        &resource_entities,
        entities,
        &snapshot_ids,
        &depleted_this_frame,
        player_position,
    );

    entities.commit_progress(&snapshot_resource_nodes, &snapshot_ids);

    // Clear the consumed depletion ids so they don't fire twice if the
    // same id is somehow re-emitted later (it won't, but keeping the
    // set tight makes the invariant local rather than global).
    for id in consumed {
        runtime.depleted_node_ids.remove(&id);
    }
}

/// First-pass cleanup when the snapshot disappears (disconnect, world swap).
/// Resets the "have we ever applied a snapshot?" flag so the next batch of
/// nodes doesn't all pop in at once like a re-entry animation.
fn clear_all_tracked_nodes(commands: &mut Commands, entities: &mut ResourceNodeEntities) {
    for (_, entity) in entities.entities.drain() {
        commands.entity(entity).despawn();
    }
    entities.previous_progress.clear();
    entities.applied_first_snapshot = false;
}

#[derive(Debug, Clone, Copy)]
struct NodeTransition {
    just_entered_regen: bool,
    should_pop_in: bool,
}

impl ResourceNodeEntities {
    /// Classifies what this tick's snapshot means for `node` compared to the
    /// previously-applied snapshot. Returned flags drive the death-effect
    /// despawn and the fresh-spawn pop-in animation in the system loop.
    fn classify(&self, node: &ResourceNodeState, pop_in_enabled: bool) -> NodeTransition {
        let was_tracked = self.previous_progress.contains_key(&node.id);
        let previous_progress = self
            .previous_progress
            .get(&node.id)
            .copied()
            .unwrap_or(None);
        let just_entered_regen = previous_progress.is_none() && node.respawn_progress.is_some();
        let just_finished_regen = previous_progress.is_some() && node.respawn_progress.is_none();
        let arrived_fresh = !was_tracked && pop_in_enabled;
        NodeTransition {
            just_entered_regen,
            should_pop_in: just_finished_regen || arrived_fresh,
        }
    }

    /// Refresh the progress map after a tick, dropping ids that left the
    /// snapshot so it can't grow without bound across long sessions.
    fn commit_progress(
        &mut self,
        nodes: &[ResourceNodeState],
        snapshot_ids: &HashSet<ResourceNodeId>,
    ) {
        self.previous_progress
            .retain(|id, _| snapshot_ids.contains(id));
        for node in nodes {
            self.previous_progress
                .insert(node.id, node.respawn_progress);
        }
        self.applied_first_snapshot = true;
    }
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
    node: &ResourceNodeState,
    model: ResourceNodeModel,
    target_transform: Transform,
    should_pop_in: bool,
) {
    let (mesh, material) = resource_node_visual(assets, node, model);
    let mut spawn_command = commands.spawn((
        Name::new(format!("Resource Node {}", node.id)),
        NetworkResourceNode { id: node.id, model },
        Mesh3d(mesh),
        MeshMaterial3d(material),
        target_transform,
        Visibility::Visible,
    ));
    // Crude clutter (branch piles, surface stones, hay grass) spawns
    // densely — up to ~30 per chunk in Forest/Rocky — and each casts a
    // negligible-size shadow under its own footprint. Skipping the
    // shadow pass for these gets us a meaningful per-frame win in
    // populated areas without losing readable silhouettes (trees and
    // veins still cast).
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
    entities.entities.insert(node.id, entity);

    if should_pop_in {
        spawn_pop_in_chip_burst(commands, impact_assets, node, model);
    }
}

/// A short upward chip burst sells the "fresh from the ground" moment.
/// Trees throw wood chips, ores throw stone shards, crude nodes throw
/// their own small per-kind burst — same palette as gather impacts so
/// the visual language stays consistent.
fn spawn_pop_in_chip_burst(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    node: &ResourceNodeState,
    model: ResourceNodeModel,
) {
    let burst_anchor = Vec3::from(node.position) + Vec3::Y * 0.18;
    let kind = ImpactEffectKind::for_resource_model(model);
    let seed = (node.id as u32).wrapping_mul(0x9E37_79B1);
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

/// Sweeps the tracked-entities map for ids that no longer appear in the
/// snapshot. Two paths:
///
/// - The id is in `depleted_this_frame` → the server told us the node
///   was actually gathered/picked-up, so play the full death effect
///   (tree fell, ore shatter, crude pickup burst).
/// - The id is *only* missing from the snapshot → it just left the
///   player's AoI ring. Silent despawn: no particles, no camera kick,
///   no sound. Otherwise every chunk-boundary crossing would animate
///   the death of every node dropping out of view (the boundary-
///   crossing "spasm" that this branch fixes).
///
/// Returns the ids whose depletion was consumed so the caller can clear
/// them from the runtime's pending set.
#[allow(clippy::too_many_arguments)]
fn despawn_nodes_missing_from_snapshot(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<StandardMaterial>,
    camera_kick: &mut crate::app::systems::CameraImpactKick,
    resource_entities: &ResourceEntityQuery,
    entities: &mut ResourceNodeEntities,
    snapshot_ids: &HashSet<ResourceNodeId>,
    depleted_this_frame: &HashSet<ResourceNodeId>,
    player_position: Option<Vec3>,
) -> Vec<ResourceNodeId> {
    let to_remove: Vec<ResourceNodeId> = entities
        .entities
        .iter()
        .filter(|(id, _)| !snapshot_ids.contains(id))
        .map(|(id, _)| *id)
        .collect();
    let mut consumed = Vec::new();
    for id in to_remove {
        let entity = entities.entities.remove(&id);
        if depleted_this_frame.contains(&id) {
            consumed.push(id);
            despawn_with_death_effect(
                commands,
                impact_assets,
                play,
                materials,
                camera_kick,
                resource_entities,
                player_position,
                entity,
            );
        } else if let Some(entity) = entity {
            // AoI-leave: silent despawn. Just drop the entity.
            commands.entity(entity).despawn();
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

pub(crate) fn resource_node_transform(
    node: &ResourceNodeState,
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
    Transform::from_xyz(
        node.position.x,
        node.position.y + height_offset,
        node.position.z,
    )
    .with_rotation(Quat::from_rotation_y(node.yaw))
    .with_scale(scale)
}

pub(crate) fn resource_node_visual(
    assets: &ResourceVisualAssets,
    _node: &ResourceNodeState,
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

    fn node_at(id: &str, position: Vec3Net) -> ResourceNodeState {
        ResourceNodeState {
            id: 1,
            definition_id: id.to_owned(),
            position,
            yaw: 0.0,
            storage: Vec::new(),
            respawn_progress: None,
        }
    }

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
            let node = node_at(ore_id, Vec3Net::new(2.0, 0.0, -3.0));
            let definition = crate::resources::resource_node_definition(ore_id).unwrap();
            let transform = resource_node_transform(&node, definition.model);
            assert_eq!(
                transform.translation.y, node.position.y,
                "{ore_id} mesh must sit at the spawn y (no floating offset)"
            );
        }
    }
}
