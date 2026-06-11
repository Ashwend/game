//! Client-side deployable systems.
//!
//! - `apply_deployed_entities_system`, diffs the replicated deployable
//!   entities against the world: spawns new ones, despawns missing ones,
//!   updates kind/health if needed.
//! - `maintain_world_grid_system`, rebuilds the client collision grid
//!   from the replicated resource-node and deployable sets when they
//!   change.
//!
//! Diffing follows the same pattern as `apply_resource_nodes_system` /
//! `apply_dropped_items_system` so the lifecycle reads consistently
//! across all networked entities. Placement-ghost and input handling
//! live in [`placement`].

mod placement;

pub(crate) use placement::{placement_input_system, update_placement_ghost_system};

use crate::building::{DOOR_OPEN_ANGLE_RAD, DOOR_PANEL_WIDTH_M};

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::{
    app::{
        scene::{DeployableVisualAssets, NetworkDeployedEntity},
        state::ClientRuntime,
        systems::furnace_fire::{FurnaceFire, sync_furnace_fire},
    },
    items::{DeployableKind, item_definition},
    protocol::{DeployedEntityId, Vec3Net},
    resources::resource_node_collider_at,
    server::{Deployable, DeployableActive, DeployableHealth, DeployableTransform, ResourceNode},
    world::WorldBlock,
};

/// Reconcile the local `NetworkDeployedEntity` visuals against the
/// Lightyear-replicated `(Deployable, DeployableTransform,
/// DeployableActive)` entities. Spawn missing ones, refresh transforms,
/// despawn any that left the AoI ring. Toggles the furnace mouth light
/// to match `active`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_deployed_entities_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Option<Res<DeployableVisualAssets>>,
    existing: Query<(Entity, &NetworkDeployedEntity, &Transform)>,
    existing_fires: Query<(Entity, &ChildOf), With<FurnaceFire>>,
    replicated: Query<(
        &Deployable,
        &DeployableTransform,
        &DeployableHealth,
        &DeployableActive,
    )>,
    mut play_sound: MessageWriter<crate::app::audio::PlaySound>,
    mut remote_impacts: MessageWriter<crate::app::state::RemoteImpactEvent>,
) {
    let Some(assets) = assets else {
        return;
    };
    if runtime.client_id.is_none() {
        // Not connected, tear down any visuals from a prior session.
        for (entity, _, _) in &existing {
            commands.entity(entity).despawn();
        }
        return;
    }

    let mut existing_by_id: HashMap<DeployedEntityId, (Entity, Transform, DeployableKind)> =
        existing
            .iter()
            .map(|(entity, marker, transform)| (marker.id, (entity, *transform, marker.kind)))
            .collect();
    let mut visible_ids: HashSet<DeployedEntityId> = HashSet::new();

    // Map parent-entity → child-fire-rig-entity for the furnace fires
    // currently in the world. We compare per-furnace-entity below and either
    // spawn (active && missing) or despawn (inactive && present) the rig.
    let mut fires_by_parent: HashMap<Entity, Entity> = HashMap::new();
    for (fire_entity, child_of) in &existing_fires {
        fires_by_parent.insert(child_of.parent(), fire_entity);
    }

    for (meta, transform, _health, active) in &replicated {
        visible_ids.insert(meta.id);
        let visual_transform = deployable_transform(transform.position.into(), transform.yaw);
        let parent_entity = if let Some((entity, current, current_kind)) =
            existing_by_id.remove(&meta.id)
        {
            // Only write the Transform when it actually moved. The replicated
            // transform stops changing once a deployable is placed, so an
            // unconditional insert would trip change detection (and the
            // renderer) every frame for a static structure, the spurious
            // change-detection pattern docs/profiling.md warns about.
            if current != visual_transform {
                commands.entity(entity).insert(visual_transform);
            }
            // A kind change is a hammer tier upgrade: the server respawns
            // the mirror entity but the same id maps onto this visual, so
            // swap the mesh in place and celebrate with a material burst
            // (chips/shards + the matching impact audio ride the same
            // remote-impact pipeline as gather hits).
            if current_kind != meta.kind {
                let (mesh, material) = deployable_visual(&assets, meta.kind);
                commands.entity(entity).insert((
                    NetworkDeployedEntity {
                        id: meta.id,
                        kind: meta.kind,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                ));
                emit_upgrade_burst(&mut remote_impacts, meta.kind, visual_transform.translation);
            }
            entity
        } else if matches!(meta.kind, DeployableKind::Door) {
            // Doors spawn an animated panel child instead of a root mesh:
            // the root sits at the doorway centre (replicated transform);
            // the panel hangs off the hinge and swings via
            // `animate_door_panels_system` when `DeployableActive` flips.
            let parent = commands
                .spawn((
                    Name::new(format!("Deployable {}", meta.id)),
                    NetworkDeployedEntity {
                        id: meta.id,
                        kind: meta.kind,
                    },
                    visual_transform,
                    Visibility::Visible,
                ))
                .id();
            commands.spawn((
                Name::new("Door Panel"),
                DoorPanel { angle: 0.0 },
                Mesh3d(assets.door_panel_mesh.clone()),
                MeshMaterial3d(assets.material.clone()),
                Transform::from_translation(Vec3::new(-(DOOR_PANEL_WIDTH_M / 2.0), 0.0, 0.0)),
                Visibility::Visible,
                ChildOf(parent),
            ));
            parent
        } else {
            let (mesh, material) = deployable_visual(&assets, meta.kind);
            commands
                .spawn((
                    Name::new(format!("Deployable {}", meta.id)),
                    NetworkDeployedEntity {
                        id: meta.id,
                        kind: meta.kind,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    visual_transform,
                    Visibility::Visible,
                ))
                .id()
        };

        sync_furnace_fire(
            &mut commands,
            parent_entity,
            meta.kind,
            active.0,
            fires_by_parent.remove(&parent_entity),
            visual_transform.translation,
            &mut play_sound,
        );
    }

    for (id, (entity, _, _)) in existing_by_id {
        if !visible_ids.contains(&id) {
            commands.entity(entity).despawn();
        }
    }
    // Any fire rig whose parent disappeared (out of AoI / destroyed)
    // would normally despawn alongside its parent via Bevy's hierarchy
    // cleanup, but a despawn-recursive isn't guaranteed everywhere,
    // sweep here just in case.
    for (_, fire_entity) in fires_by_parent {
        commands.entity(fire_entity).despawn();
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn maintain_world_grid_system(
    mut runtime: ResMut<ClientRuntime>,
    resource_nodes: Query<&ResourceNode>,
    deployables: Query<(&Deployable, &DeployableTransform, &DeployableActive)>,
    added_nodes: Query<(), Added<ResourceNode>>,
    added_deps: Query<(), Added<Deployable>>,
    // Doors drop their collider while open, so an `active` flip must
    // retrigger the rebuild. Lightyear's receive path can mark this
    // changed without a real value change (see CLAUDE.md), which only
    // costs a fingerprint compare, the open-door bits folded into the
    // fingerprint below stop a spurious rebuild.
    changed_active: Query<(), (Changed<DeployableActive>, With<Deployable>)>,
    mut removed_nodes: RemovedComponents<ResourceNode>,
    mut removed_deps: RemovedComponents<Deployable>,
    mut last_fingerprint: Local<Option<(u64, u64, u64)>>,
    mut last_world_version: Local<u64>,
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

    let resource_colliders: Vec<WorldBlock> = resource_nodes
        .iter()
        .filter_map(|node| resource_node_collider_at(&node.definition_id, node.position))
        .collect();
    let deployable_collider_blocks: Vec<WorldBlock> = deployables
        .iter()
        .flat_map(|(meta, transform, active)| deployable_colliders(meta, transform, active.0))
        .collect();
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
        hash ^= node.id;
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
        hash ^= meta.id ^ 0xD9E3_F1A7_5B6C_8024;
        // Open doors contribute a different bit pattern than closed ones
        // so an open/close flip changes the fingerprint (its collider
        // set genuinely changed) without a kind lookup.
        if matches!(meta.kind, DeployableKind::Door) && active.0 {
            hash ^= meta.id.rotate_left(17) ^ 0x5A5A_5A5A_5A5A_5A5A;
        }
        count += 1;
    }
    hash.wrapping_mul(FINGERPRINT_MIX).wrapping_add(count)
}

/// Build the solid AABBs for a placed structure from its replicated
/// components. Classic deployables resolve a single square-footprint box
/// from their item profile; building blocks and doors use the building
/// module's box layouts (openings stay passable, open doors contribute
/// nothing). Empty if the item id no longer resolves (e.g. a server
/// using a newer item table than this client knows about, skip the
/// collider rather than crash, the renderer will still draw it).
pub(crate) fn deployable_colliders(
    meta: &Deployable,
    transform: &DeployableTransform,
    active: bool,
) -> Vec<WorldBlock> {
    match meta.kind {
        DeployableKind::Building { piece, .. } => {
            crate::building::building_collider_blocks(piece, transform.position, transform.yaw)
        }
        DeployableKind::Door => {
            crate::building::door_collider_blocks(transform.position, transform.yaw, active)
        }
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
    let (tool, surface) = match kind {
        DeployableKind::Building { tier, .. } => match tier {
            crate::building::BuildingTier::Stone => {
                (crate::items::ToolKind::Pickaxe, SurfaceMaterial::Stone)
            }
            _ => (crate::items::ToolKind::Axe, SurfaceMaterial::Wood),
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
            tool,
            surface,
            effect_kind: crate::app::state::ImpactEffectKind::for_surface(surface),
            seed: (base.x.to_bits() ^ base.z.to_bits()).wrapping_add(index as u32 * 7919),
            is_player_hit: false,
        });
    }
}

fn deployable_visual(
    assets: &DeployableVisualAssets,
    kind: DeployableKind,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    let mesh = match kind {
        DeployableKind::Workbench { .. } => assets.workbench_mesh.clone(),
        DeployableKind::Furnace { .. } => assets.furnace_mesh.clone(),
        DeployableKind::Building { piece, tier } => assets.building_mesh(piece, tier),
        // Doors get an animated panel child instead (see the spawn site);
        // the root mesh handle is unused but keeps this function total.
        DeployableKind::Door => assets.door_panel_mesh.clone(),
        DeployableKind::SleepingBag => assets.sleeping_bag_mesh.clone(),
        DeployableKind::StorageBox { tier } => assets.storage_box_mesh(tier),
    };
    (mesh, assets.material.clone())
}

pub(super) fn deployable_transform(position: Vec3, yaw: f32) -> Transform {
    Transform::from_translation(position).with_rotation(Quat::from_rotation_y(yaw))
}

/// Swinging panel child of a door visual. `angle` is the panel's current
/// hinge rotation in radians (0 = closed); the animation system eases it
/// toward the replicated open state every frame.
#[derive(Component)]
pub(crate) struct DoorPanel {
    pub(crate) angle: f32,
}

/// How fast the door panel sweeps, in rad/s. ~0.35 s for the full swing,
/// quick enough to feel responsive, slow enough to read as a real door.
const DOOR_SWING_SPEED_RAD_PER_SEC: f32 = 5.0;

/// Ease each door panel toward its replicated open/closed pose. Doors are
/// rare (a handful per base), so iterating the panel set per frame is
/// negligible; the early-out below keeps the no-door case free.
pub(crate) fn animate_door_panels_system(
    time: Res<Time>,
    replicated: Query<(&Deployable, &DeployableActive)>,
    parents: Query<&NetworkDeployedEntity>,
    mut panels: Query<(&mut Transform, &mut DoorPanel, &ChildOf)>,
) {
    if panels.is_empty() {
        return;
    }
    // id → open, doors only. Built per frame; the door count is tiny.
    let open_by_id: HashMap<DeployedEntityId, bool> = replicated
        .iter()
        .filter(|(meta, _)| matches!(meta.kind, DeployableKind::Door))
        .map(|(meta, active)| (meta.id, active.0))
        .collect();

    let step = DOOR_SWING_SPEED_RAD_PER_SEC * time.delta_secs();
    for (mut transform, mut panel, child_of) in panels.iter_mut() {
        let Ok(marker) = parents.get(child_of.parent()) else {
            continue;
        };
        let open = open_by_id.get(&marker.id).copied().unwrap_or(false);
        // Negative yaw swings the +X panel toward local +Z, matching the
        // swing-arc indicator baked into the placement ghost.
        let target = if open { -DOOR_OPEN_ANGLE_RAD } else { 0.0 };
        let delta = target - panel.angle;
        if delta.abs() < 1e-3 {
            continue;
        }
        panel.angle += delta.clamp(-step, step);
        transform.rotation = Quat::from_rotation_y(panel.angle);
    }
}

/// Play the door swing sound when a door's replicated open state actually
/// flips. Tracks last-seen values in a `Local` instead of using a
/// `Changed<DeployableActive>` filter: Lightyear's receive path bumps the
/// change tick on every replication tick even when the value is identical
/// (see CLAUDE.md "Replicated state"), so a filter would fire constantly.
/// First sight of a door (join, AoI entry) seeds silently, no chorus of
/// clicks when a base streams in.
pub(crate) fn door_swing_audio_system(
    replicated: Query<(&Deployable, &DeployableTransform, &DeployableActive)>,
    mut last_open: Local<HashMap<DeployedEntityId, bool>>,
    mut play_sound: MessageWriter<crate::app::audio::PlaySound>,
) {
    for (meta, transform, active) in replicated.iter() {
        if !matches!(meta.kind, DeployableKind::Door) {
            continue;
        }
        match last_open.insert(meta.id, active.0) {
            None => {}
            Some(previous) if previous == active.0 => {}
            Some(_) => {
                let id = if active.0 {
                    crate::app::audio::SoundId::DoorOpen
                } else {
                    crate::app::audio::SoundId::DoorClose
                };
                let at = Vec3::new(
                    transform.position.x,
                    transform.position.y,
                    transform.position.z,
                );
                play_sound.write(crate::app::audio::PlaySound::at(id, at));
            }
        }
    }
}

#[cfg(test)]
mod tests;
