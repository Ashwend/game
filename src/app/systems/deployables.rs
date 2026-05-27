//! Client-side deployable systems:
//!
//! - `update_placement_ghost_system` — raycasts the camera look ray
//!   against the ground, updates `DeployablePlacementState`, and
//!   spawns / despawns the ghost preview entity.
//! - `placement_input_system` — left-click sends the place command,
//!   held right-click rotates the ghost yaw, key `R` snaps to 90°.
//! - `apply_deployed_entities_system` — diffs the snapshot's
//!   `deployed_entities` list against the world: spawns new ones,
//!   despawns missing ones, updates kind/health if needed.
//!
//! Snapshot diffing follows the same pattern as
//! `apply_resource_nodes_system` / `apply_dropped_items_system` so the
//! lifecycle reads consistently across all networked entities.

use std::collections::{HashMap, HashSet};

use bevy::{light::NotShadowCaster, prelude::*, window::PrimaryWindow};

use crate::{
    analytics::{Analytics, Event},
    app::{
        scene::{
            DeployablePlacementGhost, DeployableVisualAssets, FurnaceMouthLight, MainCamera,
            NetworkDeployedEntity,
        },
        state::{ClientErrorToast, ClientRuntime, DeployablePlacementState, MenuState, Screen},
        systems::input::send_place_deployable_command,
    },
    items::{DeployableKind, DeployableProfile, ItemId, ItemModel, item_definition},
    protocol::{DeployedEntityId, DeployedEntityState, PlaceDeployableCommand, Vec3Net},
};

/// Maximum distance, in metres, between the player's feet and the
/// ghost. Matches `PLACEMENT_REACH_M` on the server so client preview
/// and server validation agree on what the player can reach.
const PLACEMENT_REACH_M: f32 = 5.0;
/// Yaw rotation rate while right mouse is held, in radians per second.
/// One full revolution takes ~2 seconds — fast enough to feel
/// responsive, slow enough to land on the angle the player wants.
const PLACEMENT_ROTATE_RATE_RAD_PER_SEC: f32 = std::f32::consts::PI;

/// Update the placement state from the active actionbar item + camera
/// look ray. Also spawns / despawns the single ghost preview entity.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_placement_ghost_system(
    mut commands: Commands,
    mut placement: ResMut<DeployablePlacementState>,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    assets: Option<Res<DeployableVisualAssets>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    ghosts: Query<(Entity, &DeployablePlacementGhost)>,
    deployed: Query<(&NetworkDeployedEntity, &Transform)>,
) {
    let Some(assets) = assets else {
        return;
    };

    let active = current_deployable(&runtime, &menu);
    let kind_changed = placement.item_id.as_ref().map(|id| id.as_ref())
        != active.as_ref().map(|(id, _, _)| id.as_ref());
    if kind_changed {
        // Reset yaw when the deployable type changes — otherwise the
        // first frame after a swap would inherit a yaw the player
        // dialled in for a different structure.
        placement.yaw = 0.0;
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

    let world_position = ground_under_aim(camera_transform);
    let valid = match world_position {
        Some(target) => {
            is_placement_valid(target, profile, runtime.local_player_position(), &deployed)
        }
        None => false,
    };
    placement.world_position = world_position;
    placement.valid = valid;

    refresh_ghost_entity(&mut commands, &ghosts, &assets, &placement, profile);
}

/// React to placement input: left-click commits, held right-mouse
/// rotates, R nudges by 90°.
#[allow(clippy::too_many_arguments)]
pub(crate) fn placement_input_system(
    time: Res<Time>,
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

    // Hold right mouse to rotate in place. We don't lock the world
    // position while doing this — the player still aims the cursor
    // wherever they want; only the yaw spins. This matches Rust's feel:
    // the ghost sits at the aim point and yaw is driven by hold-rotate.
    if mouse.pressed(MouseButton::Right) {
        placement.yaw =
            wrap_angle(placement.yaw + time.delta_secs() * PLACEMENT_ROTATE_RATE_RAD_PER_SEC);
    }
    if keys.just_pressed(KeyCode::KeyR) {
        placement.yaw = wrap_angle(placement.yaw + std::f32::consts::FRAC_PI_2);
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
        if let Some(kind) = kind_label {
            analytics.track(Event::DeployablePlaced { kind });
        }
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

/// Diff the snapshot's placed-structure list against the live entities.
/// Spawn missing ones, update kind/transform if the snapshot moved
/// them (admin nudge, future destroy/recreate), despawn any that left
/// the AoI ring. Toggles the furnace mouth light to match `active`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_deployed_entities_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    assets: Option<Res<DeployableVisualAssets>>,
    existing: Query<(Entity, &NetworkDeployedEntity)>,
    existing_lights: Query<(Entity, &ChildOf), With<FurnaceMouthLight>>,
) {
    let Some(assets) = assets else {
        return;
    };
    if runtime.snapshot.is_none() {
        for (entity, _) in &existing {
            commands.entity(entity).despawn();
        }
        return;
    }
    let deployed = collect_deployed_entities(&runtime);

    let mut existing_by_id: HashMap<DeployedEntityId, Entity> = existing
        .iter()
        .map(|(entity, marker)| (marker.id, entity))
        .collect();
    let mut snapshot_ids: HashSet<DeployedEntityId> = HashSet::new();

    // Map parent-entity → child-light-entity for the lights currently
    // in the world. We compare per-furnace-entity below and either
    // spawn (active && missing) or despawn (inactive && present) the
    // child light.
    let mut lights_by_parent: HashMap<Entity, Entity> = HashMap::new();
    for (light_entity, child_of) in &existing_lights {
        lights_by_parent.insert(child_of.parent(), light_entity);
    }

    for state in &deployed {
        snapshot_ids.insert(state.id);
        let transform = deployable_transform(state.position.into(), state.yaw);
        let parent_entity = if let Some(entity) = existing_by_id.remove(&state.id) {
            commands.entity(entity).insert(transform);
            entity
        } else {
            let (mesh, material) = deployable_visual(&assets, state.kind);
            commands
                .spawn((
                    Name::new(format!("Deployable {}", state.id)),
                    NetworkDeployedEntity {
                        id: state.id,
                        kind: state.kind,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    transform,
                    Visibility::Visible,
                ))
                .id()
        };

        sync_furnace_light(
            &mut commands,
            parent_entity,
            state.kind,
            state.active,
            lights_by_parent.remove(&parent_entity),
        );
    }

    for (id, entity) in existing_by_id {
        if !snapshot_ids.contains(&id) {
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

/// Source the per-tick deployable list from `runtime.snapshot`, which
/// Phase 6's `synthesize_runtime_snapshot_system` rebuilds from
/// Lightyear-replicated entities every frame.
fn collect_deployed_entities(runtime: &ClientRuntime) -> Vec<DeployedEntityState> {
    runtime
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.deployed_entities.clone())
        .unwrap_or_default()
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
    runtime: &ClientRuntime,
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
    let stack = runtime
        .local_player()?
        .inventory
        .as_ref()?
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
    fn wrap_angle_keeps_value_in_canonical_range() {
        assert!((wrap_angle(3.5 * std::f32::consts::PI) + 0.5 * std::f32::consts::PI).abs() < 1e-4);
        assert!(
            (wrap_angle(-0.5 * std::f32::consts::PI) + 0.5 * std::f32::consts::PI).abs() < 1e-4
        );
    }
}
