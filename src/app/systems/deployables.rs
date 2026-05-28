//! Client-side deployable systems:
//!
//! - `update_placement_ghost_system` — raycasts the camera look ray
//!   against the ground, updates `DeployablePlacementState`, and
//!   spawns / despawns the ghost preview entity.
//! - `placement_input_system` — left-click sends the place command;
//!   holding right-click freezes the spot + camera and turns horizontal
//!   mouse motion into ghost rotation, key `R` snaps to 90°. Until the
//!   player rotates, the ghost auto-faces them (front toward the player).
//! - `apply_deployed_entities_system` — diffs the snapshot's
//!   `deployed_entities` list against the world: spawns new ones,
//!   despawns missing ones, updates kind/health if needed.
//!
//! Snapshot diffing follows the same pattern as
//! `apply_resource_nodes_system` / `apply_dropped_items_system` so the
//! lifecycle reads consistently across all networked entities.

use std::collections::{HashMap, HashSet};

use bevy::{
    input::mouse::AccumulatedMouseMotion, light::NotShadowCaster, prelude::*, window::PrimaryWindow,
};

use crate::{
    analytics::{Analytics, Event},
    app::{
        scene::{
            DeployablePlacementGhost, DeployableVisualAssets, FurnaceMouthLight, MainCamera,
            NetworkDeployedEntity,
        },
        state::{
            ClientErrorToast, ClientRuntime, DeployablePlacementState, LocalPlayerState, MenuState,
            Screen,
        },
        systems::input::send_place_deployable_command,
    },
    items::{DeployableKind, DeployableProfile, ItemId, ItemModel, item_definition},
    protocol::{DeployedEntityId, PlaceDeployableCommand, Vec3Net},
    resources::resource_node_collider_at,
    server::{Deployable, DeployableActive, DeployableHealth, DeployableTransform, ResourceNode},
    world::WorldBlock,
};

/// Maximum distance, in metres, between the player's feet and the
/// ghost. Matches `PLACEMENT_REACH_M` on the server so client preview
/// and server validation agree on what the player can reach.
const PLACEMENT_REACH_M: f32 = 5.0;
/// Radians of ghost yaw per pixel of horizontal mouse motion while
/// right-mouse is held. ~157 px sweeps a quarter turn — slow enough to
/// land precisely on the angle the player wants while fine-tuning.
const PLACEMENT_ROTATE_RAD_PER_PIXEL: f32 = 0.01;

/// Update the placement state from the active actionbar item + camera
/// look ray. Also spawns / despawns the single ghost preview entity.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_placement_ghost_system(
    mut commands: Commands,
    mut placement: ResMut<DeployablePlacementState>,
    mouse: Res<ButtonInput<MouseButton>>,
    runtime: Res<ClientRuntime>,
    local_player: Res<LocalPlayerState>,
    menu: Res<MenuState>,
    assets: Option<Res<DeployableVisualAssets>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    ghosts: Query<(Entity, &DeployablePlacementGhost)>,
    deployed: Query<(&NetworkDeployedEntity, &Transform)>,
) {
    let Some(assets) = assets else {
        return;
    };

    let active = current_deployable(&local_player, &menu);
    let kind_changed = placement.item_id.as_ref().map(|id| id.as_ref())
        != active.as_ref().map(|(id, _, _)| id.as_ref());
    if kind_changed {
        // Hand control back to auto-facing when the deployable type
        // changes — otherwise the first frame after a swap would inherit
        // a yaw the player dialled in for a different structure.
        placement.manual_yaw = false;
    }
    placement.item_id = active.as_ref().map(|(id, _, _)| id.clone());

    let Some((_, profile, _)) = active else {
        placement.world_position = None;
        placement.valid = false;
        despawn_ghost(&mut commands, &ghosts);
        return;
    };

    let Ok((_camera, camera_transform)) = camera.single() else {
        despawn_ghost(&mut commands, &ghosts);
        return;
    };

    // While right-mouse rotates the ghost, freeze the spot: the camera is
    // also locked this frame, so re-aiming would otherwise let a tiny
    // residual look-ray shift drift the position the player just settled
    // on. Otherwise the ghost tracks the look ray as usual.
    let rotating = mouse.pressed(MouseButton::Right) && placement.world_position.is_some();
    let world_position = if rotating {
        placement.world_position
    } else {
        ground_under_aim(camera_transform)
    };
    let player_feet = runtime.local_player_position();
    let valid = match world_position {
        Some(target) => is_placement_valid(target, profile, player_feet, &deployed),
        None => false,
    };
    placement.world_position = world_position;
    placement.valid = valid;

    // Until the player takes manual control, keep the front of the ghost
    // turned toward them so a freshly selected deployable reads the right
    // way round without any input.
    if !placement.manual_yaw
        && let (Some(target), Some(feet)) = (world_position, player_feet)
        && let Some(yaw) = yaw_facing_player(target, feet)
    {
        placement.yaw = yaw;
    }

    refresh_ghost_entity(&mut commands, &ghosts, &assets, &placement, profile);
}

/// React to placement input: left-click commits, held right-mouse
/// freezes the spot and turns mouse motion into rotation, R nudges by 90°.
#[allow(clippy::too_many_arguments)]
pub(crate) fn placement_input_system(
    mouse_motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut placement: ResMut<DeployablePlacementState>,
    mut runtime: ResMut<ClientRuntime>,
    mut error_toasts: MessageWriter<ClientErrorToast>,
    menu: Res<MenuState>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    analytics: Res<Analytics>,
) {
    if !gameplay_accepts_input(&menu, &primary_window) {
        return;
    }
    let Some(item_id) = placement.item_id.clone() else {
        return;
    };

    // Hold right mouse to take manual control: the camera is frozen (see
    // `mouse_look_system`) and the spot is frozen (see
    // `update_placement_ghost_system`), so horizontal mouse motion only
    // turns the ghost. This lets the player settle on a spot, grab it,
    // and slowly rotate it into place without nudging the location.
    if mouse.pressed(MouseButton::Right) {
        let dx = mouse_motion.delta.x;
        if dx != 0.0 {
            placement.yaw = wrap_angle(placement.yaw + dx * PLACEMENT_ROTATE_RAD_PER_PIXEL);
            placement.manual_yaw = true;
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        placement.yaw = wrap_angle(placement.yaw + std::f32::consts::FRAC_PI_2);
        placement.manual_yaw = true;
    }

    if mouse.just_pressed(MouseButton::Left) {
        let Some(position) = placement.world_position else {
            return;
        };
        if !placement.valid {
            return;
        }
        let kind_label = deployable_kind_label(&item_id);
        send_place_deployable_command(
            &mut runtime,
            &mut error_toasts,
            PlaceDeployableCommand {
                item_id,
                position: Vec3Net::from(position),
                yaw: placement.yaw,
            },
        );
        // Drop manual control so the next ghost (same deployable type,
        // another in the stack) starts by auto-facing the player again.
        placement.manual_yaw = false;
        if let Some(kind) = kind_label {
            analytics.track(Event::DeployablePlaced { kind });
        }
        // Keep `manual_yaw` set: the ghost stays visible until the
        // actionbar count replicates down, so resetting here would snap
        // it back to auto-facing for a frame or two before it vanishes.
        // It resets when the deployable type changes (see the ghost
        // system), which also leaves repeat placements at the same angle.
    }
}

fn deployable_kind_label(item_id: &ItemId) -> Option<String> {
    let definition = item_definition(item_id)?;
    let profile = definition.deployable?;
    Some(match profile.kind {
        DeployableKind::Workbench { .. } => "workbench".to_owned(),
        DeployableKind::Furnace { .. } => "furnace".to_owned(),
    })
}

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
    existing: Query<(Entity, &NetworkDeployedEntity)>,
    existing_lights: Query<(Entity, &ChildOf), With<FurnaceMouthLight>>,
    replicated: Query<(
        &Deployable,
        &DeployableTransform,
        &DeployableHealth,
        &DeployableActive,
    )>,
) {
    let Some(assets) = assets else {
        return;
    };
    if runtime.client_id.is_none() {
        // Not connected — tear down any visuals from a prior session.
        for (entity, _) in &existing {
            commands.entity(entity).despawn();
        }
        return;
    }

    let mut existing_by_id: HashMap<DeployedEntityId, Entity> = existing
        .iter()
        .map(|(entity, marker)| (marker.id, entity))
        .collect();
    let mut visible_ids: HashSet<DeployedEntityId> = HashSet::new();

    // Map parent-entity → child-light-entity for the lights currently
    // in the world. We compare per-furnace-entity below and either
    // spawn (active && missing) or despawn (inactive && present) the
    // child light.
    let mut lights_by_parent: HashMap<Entity, Entity> = HashMap::new();
    for (light_entity, child_of) in &existing_lights {
        lights_by_parent.insert(child_of.parent(), light_entity);
    }

    for (meta, transform, _health, active) in &replicated {
        visible_ids.insert(meta.id);
        let visual_transform = deployable_transform(transform.position.into(), transform.yaw);
        let parent_entity = if let Some(entity) = existing_by_id.remove(&meta.id) {
            commands.entity(entity).insert(visual_transform);
            entity
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

        sync_furnace_light(
            &mut commands,
            parent_entity,
            meta.kind,
            active.0,
            lights_by_parent.remove(&parent_entity),
        );
    }

    for (id, entity) in existing_by_id {
        if !visible_ids.contains(&id) {
            commands.entity(entity).despawn();
        }
    }
    // Any light whose parent disappeared (out of AoI / destroyed)
    // would normally despawn alongside its parent via Bevy's hierarchy
    // cleanup, but a despawn-recursive isn't guaranteed everywhere —
    // sweep here just in case.
    for (_, light_entity) in lights_by_parent {
        commands.entity(light_entity).despawn();
    }
}

/// Knuth golden-ratio mix constant for the fingerprint helpers — the
/// XOR-of-ids accumulator gets multiplied by this to spread sequential
/// id values across the `u64`.
const FINGERPRINT_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// Per-frame maintainer for `ClientRuntime::world_grid`. Watches the
/// world version (Welcome bumps it), the replicated resource-node set,
/// and the replicated deployable set; rebuilds the grid when any of
/// them changes. The local `Option` cache means idle frames cost a
/// fingerprint compare and nothing else.
pub(crate) fn maintain_world_grid_system(
    mut runtime: ResMut<ClientRuntime>,
    resource_nodes: Query<&ResourceNode>,
    deployables: Query<(&Deployable, &DeployableTransform)>,
    mut last_fingerprint: Local<Option<(u64, u64, u64)>>,
) {
    let world_version = runtime.world_version;
    let resource_node_version = resource_node_set_fingerprint(resource_nodes.iter());
    let deployable_version = deployable_set_fingerprint(deployables.iter());
    let current = (world_version, resource_node_version, deployable_version);

    if *last_fingerprint == Some(current) {
        return;
    }

    let resource_colliders: Vec<WorldBlock> = resource_nodes
        .iter()
        .filter_map(|node| resource_node_collider_at(&node.definition_id, node.position))
        .collect();
    let deployable_colliders: Vec<WorldBlock> = deployables
        .iter()
        .filter_map(|(meta, transform)| deployable_collider(meta, transform))
        .collect();
    runtime.rebuild_world_grid(resource_colliders, deployable_colliders);
    *last_fingerprint = Some(current);
}

fn resource_node_set_fingerprint<'a>(iter: impl IntoIterator<Item = &'a ResourceNode>) -> u64 {
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for node in iter {
        // Skip ids that contribute no collider so the fingerprint stays
        // tight to the actual collision set — crude clutter (surface
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
    iter: impl IntoIterator<Item = (&'a Deployable, &'a DeployableTransform)>,
) -> u64 {
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for (meta, _) in iter {
        // XOR ^ 0xD9E3_F1A7_5B6C_8024 ensures the deployable id space
        // (separate counter from resource nodes server-side) doesn't
        // accidentally cancel against a resource node id with the same
        // numeric value when the two fingerprints are tupled together.
        hash ^= meta.id ^ 0xD9E3_F1A7_5B6C_8024;
        count += 1;
    }
    hash.wrapping_mul(FINGERPRINT_MIX).wrapping_add(count)
}

/// Build the AABB collider for a placed structure from its replicated
/// components. Returns `None` if the item id no longer resolves (e.g.
/// a server using a newer item table than this client knows about — in
/// which case skip the collider rather than crash, the renderer will
/// still draw the structure).
pub(crate) fn deployable_collider(
    meta: &Deployable,
    transform: &DeployableTransform,
) -> Option<WorldBlock> {
    let profile = item_definition(&meta.item_id)?.deployable?;
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
    Some(WorldBlock::new(center, half))
}

/// Spawn / despawn the warm point light that simulates the fire inside
/// the furnace mouth. The light is a child of the structure entity so
/// it inherits the parent's transform and yaw automatically.
fn sync_furnace_light(
    commands: &mut Commands,
    parent_entity: Entity,
    kind: DeployableKind,
    active: bool,
    existing_light: Option<Entity>,
) {
    let is_furnace = matches!(kind, DeployableKind::Furnace { .. });
    match (is_furnace && active, existing_light) {
        (true, None) => {
            // Local offset matches the furnace mouth: just in front of
            // the loading lip, halfway up the cavity. The parent's yaw
            // rotates the offset so the light always shines where the
            // mouth faces.
            commands.entity(parent_entity).with_children(|parent| {
                parent.spawn((
                    Name::new("Furnace Mouth Light"),
                    FurnaceMouthLight,
                    PointLight {
                        // Saturated ember glow — warm enough to read as
                        // fire, dim enough not to wash out the scene
                        // at night when several furnaces might be lit.
                        color: Color::srgb(1.0, 0.62, 0.28),
                        intensity: 5_500.0,
                        range: 4.5,
                        radius: 0.10,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(0.0, 0.55, 0.42),
                ));
            });
        }
        (false, Some(light_entity)) => {
            commands.entity(light_entity).despawn();
        }
        // Already in the right state — leave it.
        _ => {}
    }
}

fn current_deployable(
    local_player: &LocalPlayerState,
    menu: &MenuState,
) -> Option<(ItemId, DeployableProfile, ItemModel)> {
    if menu.screen != Screen::InGame || menu.pause_open {
        return None;
    }
    // Any modal-open state (inventory, crafting, chat) suppresses the
    // ghost so we don't draw it while the player can't actually click
    // to place. `menu.inventory_open`/`crafting_open` cover those.
    if menu.inventory_open || menu.crafting_open || menu.chat_open {
        return None;
    }
    let stack = local_player
        .private
        .as_ref()?
        .inventory
        .active_actionbar_stack()?;
    let definition = item_definition(&stack.item_id)?;
    let profile = definition.deployable?;
    Some((stack.item_id.clone(), profile, definition.model))
}

fn ground_under_aim(camera_transform: &GlobalTransform) -> Option<Vec3> {
    let origin = camera_transform.translation();
    let forward = camera_transform.forward().as_vec3();
    // Clamp slightly steeper than vertical so the ghost doesn't latch
    // onto a horizon-far point when the player looks straight ahead —
    // pure horizontal aim is unsolvable for a ground hit.
    if forward.y.abs() < 1e-3 {
        return None;
    }
    // Only consider forward hits. Looking up at the sky gives a positive
    // ray.y; we need negative-y (looking down) to actually hit a ground
    // plane at y=0.
    if forward.y >= 0.0 {
        return None;
    }
    let t = -origin.y / forward.y;
    if t <= 0.0 || t > 50.0 {
        return None;
    }
    let hit = origin + forward * t;
    Some(Vec3::new(hit.x, 0.0, hit.z))
}

fn is_placement_valid(
    target: Vec3,
    profile: DeployableProfile,
    player_feet: Option<Vec3>,
    deployed: &Query<(&NetworkDeployedEntity, &Transform)>,
) -> bool {
    let Some(player_feet) = player_feet else {
        return false;
    };
    let dx = target.x - player_feet.x;
    let dz = target.z - player_feet.z;
    if (dx * dx + dz * dz).sqrt() > PLACEMENT_REACH_M {
        return false;
    }
    // Cheap AABB-vs-AABB overlap test against already-placed structures
    // using a shared default half-extent — we don't know the placed
    // entity's profile here (its kind would need a definition lookup),
    // so use a conservative footprint that matches both Workbench and
    // Furnace tier-1 widths.
    let candidate_min = Vec2::new(
        target.x - profile.collider_half_width,
        target.z - profile.collider_half_width,
    );
    let candidate_max = Vec2::new(
        target.x + profile.collider_half_width,
        target.z + profile.collider_half_width,
    );
    for (_, transform) in deployed.iter() {
        let p = transform.translation;
        // Use the same conservative half-width on both sides — being
        // generous on overlap here matches the server's actual
        // per-profile check, which uses the persisted entity's profile.
        let other_min = Vec2::new(p.x - 0.55, p.z - 0.55);
        let other_max = Vec2::new(p.x + 0.55, p.z + 0.55);
        if candidate_min.x < other_max.x
            && candidate_max.x > other_min.x
            && candidate_min.y < other_max.y
            && candidate_max.y > other_min.y
        {
            return false;
        }
    }
    true
}

fn deployable_visual(
    assets: &DeployableVisualAssets,
    kind: DeployableKind,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    let mesh = match kind {
        DeployableKind::Workbench { .. } => assets.workbench_mesh.clone(),
        DeployableKind::Furnace { .. } => assets.furnace_mesh.clone(),
    };
    (mesh, assets.material.clone())
}

fn deployable_transform(position: Vec3, yaw: f32) -> Transform {
    Transform::from_translation(position).with_rotation(Quat::from_rotation_y(yaw))
}

/// Yaw that turns the deployable's local +Z front toward the player.
/// `Quat::from_rotation_y(yaw) * Vec3::Z == (sin yaw, 0, cos yaw)`, so the
/// yaw whose forward points along `player - object` is `atan2(dx, dz)`.
/// Returns `None` when the player stands on the spot (direction undefined),
/// leaving the previous yaw untouched.
fn yaw_facing_player(object: Vec3, player: Vec3) -> Option<f32> {
    let dx = player.x - object.x;
    let dz = player.z - object.z;
    if dx * dx + dz * dz < 1e-4 {
        return None;
    }
    Some(dx.atan2(dz))
}

fn refresh_ghost_entity(
    commands: &mut Commands,
    ghosts: &Query<(Entity, &DeployablePlacementGhost)>,
    assets: &DeployableVisualAssets,
    placement: &DeployablePlacementState,
    profile: DeployableProfile,
) {
    let Some(position) = placement.world_position else {
        despawn_ghost(commands, ghosts);
        return;
    };
    let item_id = placement.item_id.as_ref();
    let kind = item_id
        .and_then(|id| item_definition(id))
        .and_then(|def| def.deployable)
        .map(|profile| profile.kind);
    let Some(kind) = kind else {
        despawn_ghost(commands, ghosts);
        return;
    };
    let mesh = match kind {
        DeployableKind::Workbench { .. } => assets.workbench_mesh.clone(),
        DeployableKind::Furnace { .. } => assets.furnace_mesh.clone(),
    };
    let material = if placement.valid {
        assets.ghost_valid_material.clone()
    } else {
        assets.ghost_invalid_material.clone()
    };
    let transform = deployable_transform(position, placement.yaw);

    if let Some((entity, _)) = ghosts.iter().next() {
        commands.entity(entity).insert((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            transform,
            Visibility::Visible,
        ));
        return;
    }
    commands.spawn((
        Name::new("Placement Ghost"),
        DeployablePlacementGhost,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        transform,
        Visibility::Visible,
        // Casting a shadow off a translucent placement preview reads as
        // a hard floor blob — disable it so the ghost looks unbaked.
        NotShadowCaster,
    ));
    let _ = profile; // silence unused-var if expansions never read it.
}

fn despawn_ghost(commands: &mut Commands, ghosts: &Query<(Entity, &DeployablePlacementGhost)>) {
    for (entity, _) in ghosts.iter() {
        commands.entity(entity).despawn();
    }
}

fn gameplay_accepts_input(
    menu: &MenuState,
    primary_window: &Query<&Window, With<PrimaryWindow>>,
) -> bool {
    if menu.screen != Screen::InGame || menu.pause_open {
        return false;
    }
    if menu.inventory_open || menu.crafting_open || menu.chat_open {
        return false;
    }
    primary_window
        .single()
        .map(|window| window.focused)
        .unwrap_or(false)
}

fn wrap_angle(angle: f32) -> f32 {
    use std::f32::consts::TAU;
    let mut a = angle % TAU;
    if a > std::f32::consts::PI {
        a -= TAU;
    } else if a < -std::f32::consts::PI {
        a += TAU;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::EYE_HEIGHT;

    #[test]
    fn ground_hit_returns_none_for_horizon_aim() {
        let transform = GlobalTransform::from(Transform::from_xyz(0.0, EYE_HEIGHT, 0.0));
        assert!(ground_under_aim(&transform).is_none());
    }

    #[test]
    fn ground_hit_returns_point_when_looking_down() {
        // 45° downward look from eye height — should hit ground at the
        // same horizontal distance as the eye height.
        let transform = GlobalTransform::from(
            Transform::from_xyz(0.0, EYE_HEIGHT, 0.0).looking_at(Vec3::new(2.0, 0.0, 0.0), Vec3::Y),
        );
        let hit = ground_under_aim(&transform).expect("downward look should hit");
        assert!((hit.x - 2.0).abs() < 0.1);
        assert!(hit.y.abs() < 1e-3);
    }

    #[test]
    fn yaw_facing_player_points_local_front_at_player() {
        // Object at origin, player off to +X: the local +Z front should
        // rotate to point along +X, i.e. forward == (sin yaw, 0, cos yaw)
        // ~= (1, 0, 0), so yaw ~= +pi/2.
        let yaw =
            yaw_facing_player(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)).expect("distinct positions");
        let forward = Quat::from_rotation_y(yaw) * Vec3::Z;
        assert!((forward.x - 1.0).abs() < 1e-4);
        assert!(forward.z.abs() < 1e-4);
    }

    #[test]
    fn yaw_facing_player_is_none_when_coincident() {
        assert!(yaw_facing_player(Vec3::new(5.0, 0.0, 5.0), Vec3::new(5.0, 1.7, 5.0)).is_none());
    }

    #[test]
    fn wrap_angle_keeps_value_in_canonical_range() {
        assert!((wrap_angle(3.5 * std::f32::consts::PI) + 0.5 * std::f32::consts::PI).abs() < 1e-4);
        assert!(
            (wrap_angle(-0.5 * std::f32::consts::PI) + 0.5 * std::f32::consts::PI).abs() < 1e-4
        );
    }

    #[test]
    fn ground_under_aim_rejects_upward_and_far_rays() {
        // Looking up at the sky never hits the y=0 plane.
        let up = GlobalTransform::from(
            Transform::from_xyz(0.0, EYE_HEIGHT, 0.0)
                .looking_at(Vec3::new(0.0, 10.0, -2.0), Vec3::Y),
        );
        assert!(ground_under_aim(&up).is_none());

        // A very shallow downward look hits far past the 50m clamp.
        let shallow = GlobalTransform::from(
            Transform::from_xyz(0.0, EYE_HEIGHT, 0.0)
                .looking_at(Vec3::new(0.0, EYE_HEIGHT - 0.001, -1000.0), Vec3::Y),
        );
        assert!(ground_under_aim(&shallow).is_none());
    }

    #[test]
    fn deployable_kind_label_resolves_known_deployables() {
        use crate::items::{CRUDE_FURNACE_ID, WORKBENCH_T1_ID, intern_item_id};
        assert_eq!(
            deployable_kind_label(&intern_item_id(WORKBENCH_T1_ID)).as_deref(),
            Some("workbench")
        );
        assert_eq!(
            deployable_kind_label(&intern_item_id(CRUDE_FURNACE_ID)).as_deref(),
            Some("furnace")
        );
        // A non-deployable item resolves to no label.
        assert!(deployable_kind_label(&intern_item_id(crate::items::WOOD_ID)).is_none());
    }

    #[test]
    fn deployable_collider_uses_profile_extents_and_lifts_center() {
        use crate::items::{WORKBENCH_T1_ID, intern_item_id};
        let meta = Deployable {
            id: 1,
            item_id: intern_item_id(WORKBENCH_T1_ID),
            kind: DeployableKind::Workbench { tier: 1 },
            max_health: 500,
        };
        let transform = DeployableTransform {
            position: Vec3Net::new(2.0, 0.0, -3.0),
            yaw: 0.0,
        };
        let profile = item_definition(&meta.item_id).unwrap().deployable.unwrap();
        let block = deployable_collider(&meta, &transform).expect("known item resolves a collider");
        // Center is raised by the collider half-height off the ground.
        assert!((block.center.y - profile.collider_half_height).abs() < 1e-4);
        assert!((block.center.x - 2.0).abs() < 1e-4);

        // Unknown item id -> no collider rather than a panic.
        let unknown = Deployable {
            id: 2,
            item_id: intern_item_id("not_a_real_item"),
            kind: DeployableKind::Workbench { tier: 1 },
            max_health: 1,
        };
        assert!(deployable_collider(&unknown, &transform).is_none());
    }

    #[test]
    fn deployable_transform_applies_position_and_yaw() {
        let yaw = std::f32::consts::FRAC_PI_2;
        let transform = deployable_transform(Vec3::new(1.0, 2.0, 3.0), yaw);
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
        let expected = Quat::from_rotation_y(yaw);
        assert!(transform.rotation.dot(expected).abs() > 1.0 - 1e-5);
    }

    #[test]
    fn current_deployable_suppressed_by_modal_states() {
        use crate::app::state::LocalPlayerState;
        use crate::items::WORKBENCH_T1_ID;
        use crate::protocol::{ItemStack, PlayerInventoryState};
        use crate::server::PlayerPrivate;

        let mut inventory = PlayerInventoryState::empty();
        inventory.actionbar_slots[0] = Some(ItemStack::new(WORKBENCH_T1_ID, 1));
        let player = LocalPlayerState {
            entity: None,
            public: None,
            private: Some(PlayerPrivate {
                inventory,
                crafting: Default::default(),
                open_furnace: None,
                open_loot_bag: None,
                last_processed_input: 0,
            }),
            lifecycle: None,
        };

        // In-game with a deployable selected -> Some.
        let in_game = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };
        assert!(current_deployable(&player, &in_game).is_some());

        // Inventory overlay open -> suppressed.
        let inv_open = MenuState {
            screen: Screen::InGame,
            inventory_open: true,
            ..Default::default()
        };
        assert!(current_deployable(&player, &inv_open).is_none());

        // Not in game -> suppressed.
        let menu = MenuState {
            screen: Screen::MainMenu,
            ..Default::default()
        };
        assert!(current_deployable(&player, &menu).is_none());
    }

    #[test]
    fn current_deployable_none_for_non_deployable_item() {
        use crate::app::state::LocalPlayerState;
        use crate::protocol::{ItemStack, PlayerInventoryState};
        use crate::server::PlayerPrivate;

        let mut inventory = PlayerInventoryState::empty();
        inventory.actionbar_slots[0] = Some(ItemStack::new(crate::items::WOOD_ID, 1));
        let player = LocalPlayerState {
            entity: None,
            public: None,
            private: Some(PlayerPrivate {
                inventory,
                crafting: Default::default(),
                open_furnace: None,
                open_loot_bag: None,
                last_processed_input: 0,
            }),
            lifecycle: None,
        };
        let in_game = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };
        assert!(current_deployable(&player, &in_game).is_none());
    }

    #[test]
    fn deployable_set_fingerprint_distinguishes_membership() {
        use crate::items::{WORKBENCH_T1_ID, intern_item_id};
        let make = |id: u64| {
            (
                Deployable {
                    id,
                    item_id: intern_item_id(WORKBENCH_T1_ID),
                    kind: DeployableKind::Workbench { tier: 1 },
                    max_health: 1,
                },
                DeployableTransform {
                    position: Vec3Net::new(0.0, 0.0, 0.0),
                    yaw: 0.0,
                },
            )
        };
        let one = make(1);
        let two = make(2);

        let empty = deployable_set_fingerprint(std::iter::empty());
        let single = deployable_set_fingerprint([(&one.0, &one.1)]);
        let pair = deployable_set_fingerprint([(&one.0, &one.1), (&two.0, &two.1)]);

        assert_ne!(empty, single);
        assert_ne!(single, pair);
        // Stable across recomputation.
        assert_eq!(single, deployable_set_fingerprint([(&one.0, &one.1)]));
    }

    #[test]
    fn resource_node_set_fingerprint_skips_colliderless_clutter() {
        // Crude clutter (hay grass) contributes no collider, so it doesn't
        // move the fingerprint — only collidable nodes (trees/ore) do.
        let hay = ResourceNode {
            id: 1,
            definition_id: crate::resources::HAY_GRASS_NODE_ID.to_owned(),
            position: Vec3Net::new(0.0, 0.0, 0.0),
            yaw: 0.0,
        };
        let tree = ResourceNode {
            id: 2,
            definition_id: crate::resources::PINE_TREE_NODE_ID.to_owned(),
            position: Vec3Net::new(0.0, 0.0, 0.0),
            yaw: 0.0,
        };

        let empty = resource_node_set_fingerprint(std::iter::empty());
        let only_hay = resource_node_set_fingerprint([&hay]);
        // Hay alone contributes nothing -> same as empty.
        assert_eq!(empty, only_hay);

        let with_tree = resource_node_set_fingerprint([&hay, &tree]);
        assert_ne!(empty, with_tree);
    }
}
