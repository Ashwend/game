//! Client-side deployable placement:
//!
//! - `update_placement_ghost_system` — raycasts the camera look ray
//!   against the ground, updates `DeployablePlacementState`, and
//!   spawns / despawns the ghost preview entity.
//! - `placement_input_system` — left-click sends the place command;
//!   holding right-click freezes the spot + camera and turns horizontal
//!   mouse motion into ghost rotation, key `R` snaps to 90°. Until the
//!   player rotates, the ghost auto-faces them (front toward the player).

use bevy::{
    input::mouse::AccumulatedMouseMotion, light::NotShadowCaster, prelude::*, window::PrimaryWindow,
};

use crate::{
    analytics::{Analytics, Event},
    app::{
        scene::{
            DeployablePlacementGhost, DeployableVisualAssets, MainCamera, NetworkDeployedEntity,
        },
        state::{
            ClientErrorToast, ClientRuntime, DeployablePlacementState, LocalPlayerState, MenuState,
            Screen,
        },
        systems::input::send_place_deployable_command,
    },
    items::{DeployableKind, DeployableProfile, ItemId, ItemModel, item_definition},
    protocol::{PlaceDeployableCommand, Vec3Net},
};

use super::deployable_transform;

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

pub(super) fn deployable_kind_label(item_id: &ItemId) -> Option<String> {
    let definition = item_definition(item_id)?;
    let profile = definition.deployable?;
    Some(match profile.kind {
        DeployableKind::Workbench { .. } => "workbench".to_owned(),
        DeployableKind::Furnace { .. } => "furnace".to_owned(),
    })
}

pub(super) fn current_deployable(
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

pub(super) fn ground_under_aim(camera_transform: &GlobalTransform) -> Option<Vec3> {
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

/// Yaw that turns the deployable's local +Z front toward the player.
/// `Quat::from_rotation_y(yaw) * Vec3::Z == (sin yaw, 0, cos yaw)`, so the
/// yaw whose forward points along `player - object` is `atan2(dx, dz)`.
/// Returns `None` when the player stands on the spot (direction undefined),
/// leaving the previous yaw untouched.
pub(super) fn yaw_facing_player(object: Vec3, player: Vec3) -> Option<f32> {
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

pub(super) fn wrap_angle(angle: f32) -> f32 {
    use std::f32::consts::TAU;
    let mut a = angle % TAU;
    if a > std::f32::consts::PI {
        a -= TAU;
    } else if a < -std::f32::consts::PI {
        a += TAU;
    }
    a
}
