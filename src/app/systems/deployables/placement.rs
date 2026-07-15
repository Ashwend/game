//! Client-side placement preview + input for everything placeable:
//!
//! - Classic deployables (workbench, furnace, sleeping bag): free ground
//!   placement, right-mouse-hold rotates, exactly the original flow.
//! - Building plan pieces: the ghost snaps to the building grid
//!   (foundations onto the 3 m neighbour grid of existing foundations,
//!   wall-like pieces onto foundation edge sockets) and left click sends
//!   `PlaceBuilding`. The piece comes from the plan's wheel selection.
//! - Hewn log doors: the ghost (panel + swing-arc indicator) snaps to the
//!   nearest free doorway; right-click flips hinge/swing (a half-turn);
//!   left click opens the set-code prompt, and only confirming that sends
//!   `DoorCommand::Place`.
//!
//! The server re-derives every snap and re-validates every cost; this
//! module exists so the preview the player sees is the pose the server
//! will accept.
//!
//! Split by concern: this root file owns the ghost update systems and
//! placement input; the low-level snap/occupancy geometry lives in
//! [`snapping`] and the claim-boundary ring VFX in [`claim_ring`].

use bevy::{
    input::mouse::AccumulatedMouseMotion, light::NotShadowCaster, prelude::*, window::PrimaryWindow,
};

use crate::{
    analytics::{Analytics, Event},
    app::{
        scene::{DeployablePlacementGhost, DeployableVisualAssets, MainCamera},
        state::{
            BuildingCostReadout, BuildingPlanState, ClientErrorToast, ClientRuntime, CurrentUser,
            DeployablePlacementState, LocalPlayerState, MenuState, Screen, TextPrompt,
            TextPromptKind, WheelMenuState,
        },
        systems::input::send_place_deployable_command,
    },
    building::{
        BuildingPiece, BuildingTier, ClaimPlatform, StabilitySupport, building_collider_blocks,
        candidate_stability_pct, claim_cells_overlap_blocks, claim_footprint_cells,
        door_collider_blocks, placement_cost, platform_top_offset, snap_yaw_quarter_turn,
    },
    game_balance::{
        BUILDING_MIN_PLACEMENT_STABILITY_PCT, BUILDING_PRIVILEGE_MARGIN_CELLS,
        FOUNDATION_RAISE_MAX_M,
    },
    inventory::count_items_in_inventory,
    items::{
        BUILDING_PLAN_ID, DeployableKind, DeployableProfile, DoorVariant, ItemId, item_definition,
    },
    protocol::{AccountId, PlaceBuildingCommand, PlaceDeployableCommand, Vec3Net},
    server::{Deployable, DeployableAuth, DeployableStability, DeployableTransform},
    world::WorldBlock,
};

use super::deployable_visual_transform;

mod claim_ring;
mod snapping;

pub(crate) use claim_ring::update_claim_boundary_system;

use snapping::{
    any_replicated_overlap, foundation_cell_occupied, nearest_ceiling_cell,
    nearest_foundation_neighbor, nearest_free_doorway, nearest_stairs_cell, nearest_wall_hit,
    nearest_wall_socket, wall_socket_occupied,
};

/// Ghost-ready variants of the placed-charge body meshes (keg / satchel /
/// bomb), keyed by the source mesh's handle. The charge glbs follow the
/// ember-glow COLOR_0 convention where vertex ALPHA is a glow mask (0 on a
/// non-glowing body), which is fine for their cel material, but the ghost's
/// translucent `StandardMaterial` multiplies vertex alpha into its own: alpha
/// 0.38 * 0 = an invisible ghost. [`prepare_charge_ghost_meshes_system`]
/// clones each charge mesh once loaded, saturates COLOR_0 alpha to 1, and the
/// ghost binds the clone instead.
#[derive(Resource, Default)]
pub(crate) struct ChargeGhostMeshes {
    by_source: std::collections::HashMap<AssetId<Mesh>, Handle<Mesh>>,
}

/// Build the alpha-saturated ghost clone for each charge body mesh once its
/// glb finishes loading. Cheap steady-state: three map hits per frame once
/// populated.
pub(crate) fn prepare_charge_ghost_meshes_system(
    assets: Option<Res<DeployableVisualAssets>>,
    mut ghost_meshes: ResMut<ChargeGhostMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Some(assets) = assets else {
        return;
    };
    for source in [
        &assets.charge_keg_mesh,
        &assets.charge_satchel_mesh,
        &assets.charge_bomb_mesh,
    ] {
        if ghost_meshes.by_source.contains_key(&source.id()) {
            continue;
        }
        let Some(mesh) = meshes.get(source) else {
            continue;
        };
        let mut clone = mesh.clone();
        saturate_vertex_color_alpha(&mut clone);
        let handle = meshes.add(clone);
        ghost_meshes.by_source.insert(source.id(), handle);
    }
}

/// Force every COLOR_0 alpha to 1 so a translucent ghost material's own alpha
/// is the only transparency source. No-op for meshes without vertex colors.
fn saturate_vertex_color_alpha(mesh: &mut Mesh) {
    use bevy::mesh::VertexAttributeValues;
    match mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR) {
        Some(VertexAttributeValues::Float32x4(colors)) => {
            for color in colors {
                color[3] = 1.0;
            }
        }
        Some(VertexAttributeValues::Unorm8x4(colors)) => {
            for color in colors {
                color[3] = u8::MAX;
            }
        }
        Some(VertexAttributeValues::Unorm16x4(colors)) => {
            for color in colors {
                color[3] = u16::MAX;
            }
        }
        _ => {}
    }
}

/// Maximum distance, in metres, between the player's feet and the ghost. Reads
/// the one balance constant the server also validates against so client preview
/// and server authority cannot drift.
const PLACEMENT_REACH_M: f32 = crate::game_balance::DEPLOYABLE_PLACEMENT_REACH_M;

/// True when the XZ distance from the player's `feet` to `point` is within
/// [`PLACEMENT_REACH_M`]. The horizontal-only reach gate used by every
/// placement preview (building, door, torch, foundation aim, free placement).
/// Accepts both `Vec3` and the replicated `Vec3Net` via `Into`.
fn within_reach(point: impl Into<Vec3>, feet: Vec3) -> bool {
    let point = point.into();
    let dx = point.x - feet.x;
    let dz = point.z - feet.z;
    (dx * dx + dz * dz).sqrt() <= PLACEMENT_REACH_M
}
/// Radians of ghost yaw per pixel of horizontal mouse motion while
/// right-mouse is held. ~157 px sweeps a quarter turn, slow enough to
/// land precisely on the angle the player wants while fine-tuning.
const PLACEMENT_ROTATE_RAD_PER_PIXEL: f32 = 0.01;

/// What the placement preview is currently showing.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum GhostIntent {
    /// A classic deployable item with its profile kind.
    Deployable(ItemId, DeployableProfile),
    /// A building-plan piece (always previewed at the sticks tier).
    Building(BuildingPiece),
    /// A door (wood or iron) of the held variant, snapping to doorways.
    Door(DoorVariant),
    /// A torch: free-view placement that mounts on the ground or the side of
    /// a wall (no socket snapping).
    Torch(ItemId, DeployableProfile),
}

/// Update the placement state from the active actionbar item + camera
/// look ray. Also spawns / despawns the single ghost preview entity.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn update_placement_ghost_system(
    mut commands: Commands,
    mut placement: ResMut<DeployablePlacementState>,
    plan: Res<BuildingPlanState>,
    mouse: Res<ButtonInput<MouseButton>>,
    runtime: Res<ClientRuntime>,
    local_player: Res<LocalPlayerState>,
    menu: Res<MenuState>,
    assets: Option<Res<DeployableVisualAssets>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    ghosts: Query<(Entity, &DeployablePlacementGhost)>,
    replicated: Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
    claim_query: Query<(&Deployable, &DeployableTransform, &DeployableAuth)>,
    user: Option<Res<CurrentUser>>,
    ghost_meshes: Res<ChargeGhostMeshes>,
) {
    let Some(assets) = assets else {
        return;
    };

    let intent = current_ghost_intent(&local_player, &menu, &plan);
    // Track the held item id (not the intent) for kind-change detection:
    // swapping between any two placeables resets manual rotation + flip.
    let active_item_id = intent.as_ref().and_then(|_| current_item_id(&local_player));
    let kind_changed = placement.item_id.as_ref().map(|id| id.as_ref())
        != active_item_id.as_ref().map(|id| id.as_ref());
    if kind_changed {
        // Hand control back to auto-facing when the deployable type
        // changes, otherwise the first frame after a swap would inherit
        // a yaw the player dialled in for a different structure.
        placement.manual_yaw = false;
        placement.door_flip = false;
    }
    placement.item_id = active_item_id;

    let Some(intent) = intent else {
        placement.world_position = None;
        placement.valid = false;
        placement.door_target = None;
        placement.building_cost = None;
        despawn_ghost(&mut commands, &ghosts);
        return;
    };

    let Ok((camera_component, camera_transform)) = camera.single() else {
        placement.building_cost = None;
        despawn_ghost(&mut commands, &ghosts);
        return;
    };

    let player_feet = runtime.local_player_position();
    // Refresh the ruin-footprint cache when the world changes; the gate
    // after the pose update mirrors the server's ruin placement rejection.
    if placement.ruin_footprints_key != runtime.world_map_seed_dims {
        placement.ruin_footprints_key = runtime.world_map_seed_dims;
        placement.ruin_footprints = match runtime.world_map_seed_dims {
            Some((seed, dims)) => {
                crate::world::ruin_footprints(&crate::world::ruin_layout(seed, dims))
            }
            None => Vec::new(),
        };
    }
    let ghost_kind = match intent {
        GhostIntent::Deployable(_, profile) => {
            update_free_placement(
                &mut placement,
                profile,
                &mouse,
                camera_transform,
                player_feet,
                &replicated,
            );
            // Deployables consume the held item itself, not raw materials, so
            // they carry no separate cost readout.
            placement.building_cost = None;
            profile.kind
        }
        GhostIntent::Building(piece) => {
            update_building_placement(
                &mut placement,
                piece,
                camera_transform,
                player_feet,
                &replicated,
            );
            placement.building_cost = building_cost_readout(
                piece,
                placement.world_position,
                &local_player,
                camera_component,
                camera_transform,
            );
            DeployableKind::Building {
                piece,
                tier: BuildingTier::Sticks,
            }
        }
        GhostIntent::Door(variant) => {
            update_door_placement(&mut placement, camera_transform, player_feet, &replicated);
            placement.building_cost = None;
            DeployableKind::Door { variant }
        }
        GhostIntent::Torch(_, _) => {
            update_torch_placement(&mut placement, camera_transform, player_feet, &replicated);
            placement.building_cost = None;
            // Fold the mount into the kind so the ghost tilts on a wall.
            DeployableKind::Torch {
                wall: placement.wall_mounted,
            }
        }
    };

    // Ruin exclusion: player construction is banned inside a ruin footprint
    // plus the placement margin (the shared salvage chests must stay
    // unwallable and uncampable), so force the ghost red there, exactly
    // matching the server gate. Explosive charges stay exempt: raid tools
    // work anywhere.
    if placement.valid
        && !matches!(ghost_kind, DeployableKind::Explosive { .. })
        && let Some(pos) = placement.world_position
        && crate::world::point_near_any_footprint(
            &placement.ruin_footprints,
            pos.x,
            pos.z,
            crate::game_balance::RUIN_PLACEMENT_EXCLUSION_MARGIN_M,
        )
    {
        placement.valid = false;
    }

    // Building privilege: if the snapped placement falls inside a Tool
    // Cupboard claim the local player isn't authorized for, force the ghost
    // red (the server would reject it). Covers every placement kind,
    // including a first-tier building piece. Footprint-aware: the ghost's
    // full collider footprint is tested, so a piece whose model reaches into
    // the claim turns red even when its snap centre is just outside, exactly
    // matching the server gate.
    if placement.valid
        && let Some(pos) = placement.world_position
        && let Some(account) = user.as_ref().map(|user| user.0.account_id)
    {
        let footprint = ghost_collider_blocks(
            ghost_kind,
            placement.item_id.as_ref(),
            Vec3Net::new(pos.x, pos.y, pos.z),
            placement.yaw,
        );
        if placement_blocked_by_claim(&footprint, &claim_query, account) {
            placement.valid = false;
        }
    }

    // For a wall ghost, gather the platform set so the preview can flush its
    // outer face against the foundation edge exactly like the placed piece.
    let wall_platforms: Vec<ClaimPlatform> = if matches!(ghost_kind, DeployableKind::Building { piece, .. } if piece.is_wall_like())
    {
        replicated
            .iter()
            .filter_map(|(meta, transform, _)| {
                let DeployableKind::Building { piece, .. } = meta.kind else {
                    return None;
                };
                let top = platform_top_offset(piece)?;
                Some(ClaimPlatform {
                    position: transform.position,
                    top: transform.position.y + top,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    refresh_ghost_entity(
        &mut commands,
        &ghosts,
        &assets,
        &ghost_meshes,
        &placement,
        ghost_kind,
        &wall_platforms,
    );
}

/// The world-space collider boxes a ghost of `kind` would occupy at
/// `position`/`yaw`. Mirrors [`super::deployable_colliders`] (the same
/// per-kind box layouts the placed entity uses), so the claim footprint
/// test the ghost runs matches what the server will check on the real
/// piece. Empty when a deployable item id no longer resolves a profile.
fn ghost_collider_blocks(
    kind: DeployableKind,
    item_id: Option<&ItemId>,
    position: Vec3Net,
    yaw: f32,
) -> Vec<WorldBlock> {
    match kind {
        DeployableKind::Building { piece, .. } => building_collider_blocks(piece, position, yaw),
        DeployableKind::Door { .. } => door_collider_blocks(position, yaw, false),
        _ => {
            let Some(profile) = item_id
                .and_then(|id| item_definition(id))
                .and_then(|def| def.deployable)
            else {
                return Vec::new();
            };
            let center = Vec3Net::new(
                position.x,
                position.y + profile.collider_half_height,
                position.z,
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

/// True when a placement's collider `blocks` overlap a Tool Cupboard claim
/// the local `account` isn't authorized for. Mirrors the server's
/// permissive footprint gate (authorized by *any* covering claim means
/// allowed) using the same shared geometry, so the ghost matches the
/// server's verdict.
fn placement_blocked_by_claim(
    blocks: &[WorldBlock],
    claim_query: &Query<(&Deployable, &DeployableTransform, &DeployableAuth)>,
    account: AccountId,
) -> bool {
    let platforms: Vec<ClaimPlatform> = claim_query
        .iter()
        .filter_map(|(meta, transform, _)| {
            let DeployableKind::Building { piece, .. } = meta.kind else {
                return None;
            };
            let top = platform_top_offset(piece)?;
            Some(ClaimPlatform {
                position: transform.position,
                top: transform.position.y + top,
            })
        })
        .collect();

    let mut covered = false;
    for (meta, transform, auth) in claim_query {
        if !matches!(meta.kind, DeployableKind::ToolCupboard) {
            continue;
        }
        let cells = claim_footprint_cells(
            &platforms,
            transform.position,
            BUILDING_PRIVILEGE_MARGIN_CELLS,
        );
        if !claim_cells_overlap_blocks(&cells, blocks) {
            continue;
        }
        covered = true;
        if auth.0.contains(&account) {
            return false;
        }
    }
    covered
}

/// The original free-ground flow for classic deployables: ghost follows
/// the look ray, right-mouse freezes the spot for rotation, auto-faces
/// the player until manually rotated.
fn update_free_placement(
    placement: &mut DeployablePlacementState,
    profile: DeployableProfile,
    mouse: &ButtonInput<MouseButton>,
    camera_transform: &GlobalTransform,
    player_feet: Option<Vec3>,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) {
    // While right-mouse rotates the ghost, freeze the spot: the camera is
    // also locked this frame, so re-aiming would otherwise let a tiny
    // residual look-ray shift drift the position the player just settled
    // on. Otherwise the ghost tracks the look ray as usual.
    let rotating = mouse.pressed(MouseButton::Right) && placement.world_position.is_some();
    let world_position = if rotating {
        placement.world_position
    } else {
        surface_under_aim(camera_transform, replicated)
    };
    let valid = match world_position {
        Some(target) => is_free_placement_valid(target, profile, player_feet, replicated),
        None => false,
    };
    placement.world_position = world_position;
    placement.valid = valid;
    placement.door_target = None;

    // Until the player takes manual control, keep the front of the ghost
    // turned toward them so a freshly selected deployable reads the right
    // way round without any input.
    if !placement.manual_yaw
        && let (Some(target), Some(feet)) = (world_position, player_feet)
        && let Some(yaw) = yaw_facing_player(target, feet)
    {
        placement.yaw = yaw;
    }
}

/// Building-plan ghost: snap to the building grid. Foundations prefer an
/// existing foundation's neighbour socket and otherwise sit free on the
/// ground (quarter-turn yaw); wall-like pieces only ever sit on a
/// platform edge socket; ceilings cap a walled storey; stairs stand on a
/// platform cell.
fn update_building_placement(
    placement: &mut DeployablePlacementState,
    piece: BuildingPiece,
    camera_transform: &GlobalTransform,
    player_feet: Option<Vec3>,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) {
    placement.door_target = None;
    let ground_aim = ground_under_aim(camera_transform);

    let (pose, socketed_valid) = match piece {
        BuildingPiece::Wall | BuildingPiece::WindowWall | BuildingPiece::Doorway => {
            match nearest_wall_socket(camera_transform, replicated) {
                Some(socket) => {
                    let occupied = wall_socket_occupied(socket.position, socket.yaw, replicated);
                    (Some((socket.position, socket.yaw)), !occupied)
                }
                None => (None, false),
            }
        }
        BuildingPiece::Foundation => {
            let Some(aim) = foundation_aim(camera_transform, player_feet) else {
                placement.world_position = None;
                placement.valid = false;
                return;
            };
            let aim_net = Vec3Net::new(aim.x, aim.y, aim.z);
            match nearest_foundation_neighbor(aim_net, replicated) {
                Some(socket) => {
                    // A snapped extension can still overlap a foundation
                    // sitting on a *different* (offset) grid; check the
                    // boxes like the server does, not just the cell, or
                    // the ghost shows green for a doomed placement.
                    let blocks = crate::building::building_collider_blocks(
                        piece,
                        socket.position,
                        socket.yaw,
                    );
                    let clear = !foundation_cell_occupied(socket.position, replicated)
                        && !any_replicated_overlap(&blocks, replicated, false);
                    (Some((socket.position, socket.yaw)), clear)
                }
                None => {
                    // Free foundation: quarter-turn grid placement at the
                    // aim-driven height (see `foundation_aim`), one face
                    // kept toward the player (like the furnace/workbench
                    // ghosts) until R takes manual control. Overlap with
                    // existing structures is the server's final word; the
                    // preview checks the same boxes.
                    if !placement.manual_yaw
                        && let Some(feet) = player_feet
                        && let Some(facing) = yaw_facing_player(aim, feet)
                    {
                        placement.yaw = facing;
                    }
                    // Snap to the quarter-turn grid like every other building
                    // piece (module invariant). The facing above chooses which
                    // cardinal side fronts the player; snapping then locks it to
                    // 90°. Left un-snapped, the slab renders at an arbitrary
                    // angle while its wall sockets (always quarter-snapped) align
                    // to the world axes, so every wall lands rotated relative to
                    // the foundation it sits on. The server snaps too
                    // (`snap_foundation`); snapping here keeps the ghost honest.
                    let yaw = snap_yaw_quarter_turn(placement.yaw);
                    let blocks = crate::building::building_collider_blocks(piece, aim_net, yaw);
                    let clear = !any_replicated_overlap(&blocks, replicated, false);
                    (Some((aim_net, yaw)), clear)
                }
            }
        }
        BuildingPiece::Ceiling => match nearest_ceiling_cell(camera_transform, replicated) {
            Some((position, yaw)) => {
                let blocks = crate::building::building_collider_blocks(piece, position, yaw);
                // A duplicate ceiling or stairs rising through the cell
                // shows up as a box overlap, same as the server's check.
                // Wall-plane pairs are skipped: a stacked wall shares the
                // slab's height band at the edge by construction.
                let clear = !any_replicated_overlap(&blocks, replicated, true);
                (Some((position, yaw)), clear)
            }
            None => (None, false),
        },
        BuildingPiece::Stairs => {
            match nearest_stairs_cell(camera_transform, replicated) {
                Some(base) => {
                    let yaw = snap_yaw_quarter_turn(placement.yaw);
                    let blocks = crate::building::building_collider_blocks(piece, base, yaw);
                    // Stairs legitimately clip the wall/door plane at the
                    // cell edges; collide with platforms, ceilings, and
                    // other stairs only.
                    let clear = !any_replicated_overlap(&blocks, replicated, true);
                    (Some((base, yaw)), clear)
                }
                None => (None, false),
            }
        }
    };

    let Some((position, yaw)) = pose else {
        // No snap target near the aim: park the ghost at the ground aim
        // (when there is one) so the player sees what they're holding,
        // but red.
        placement.world_position = ground_aim;
        placement.yaw = snap_yaw_quarter_turn(placement.yaw);
        placement.valid = false;
        return;
    };

    let in_reach = player_feet.is_some_and(|feet| within_reach(position, feet));
    // Mirror of the server's stability gate, predicted from replicated
    // per-piece stabilities, so the ghost goes red where the server
    // would refuse ("too far up / too far out").
    let supports: Vec<StabilitySupport> = replicated
        .iter()
        .filter_map(|(meta, transform, stability)| {
            let DeployableKind::Building { piece, .. } = meta.kind else {
                return None;
            };
            Some((
                piece,
                transform.position,
                transform.yaw,
                u32::from(stability.0),
            ))
        })
        .collect();
    let stable = candidate_stability_pct(piece, position, yaw, &supports)
        >= BUILDING_MIN_PLACEMENT_STABILITY_PCT;
    placement.world_position = Some(Vec3::new(position.x, position.y, position.z));
    placement.yaw = yaw;
    placement.valid = socketed_valid && in_reach && stable;
}

/// Build the cost readout for a building-piece ghost: the placement material,
/// how much it costs, how much the player currently holds, and the screen
/// anchor (the projected base of the ghost) the in-game overlay pins the label
/// to. `None` when the ghost has no world position or its base projects
/// off-screen. The affordability check is the same `count` vs `placement_cost`
/// the server runs, so the green/red the player sees matches what it accepts.
fn building_cost_readout(
    piece: BuildingPiece,
    world_position: Option<Vec3>,
    local_player: &LocalPlayerState,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<BuildingCostReadout> {
    let world = world_position?;
    let (cost_item, required) = placement_cost(piece);
    let have = local_player
        .private
        .as_ref()
        .map(|private| count_items_in_inventory(&private.inventory, cost_item))
        .unwrap_or(0);
    let material = item_definition(cost_item)
        .map(|definition| definition.name)
        .unwrap_or("materials");
    let anchor = camera.world_to_viewport(camera_transform, world).ok()?;
    Some(BuildingCostReadout {
        material,
        required,
        have,
        anchor,
    })
}

/// Door ghost: latch onto the nearest free doorway around the aim point.
/// The flip toggle is a half-turn, mirroring hinge + swing together,
/// which the arc indicator baked into the ghost mesh makes visible.
fn update_door_placement(
    placement: &mut DeployablePlacementState,
    camera_transform: &GlobalTransform,
    player_feet: Option<Vec3>,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) {
    let aim = ground_under_aim(camera_transform);
    let aim_net = aim.map(|aim| Vec3Net::new(aim.x, 0.0, aim.z));

    let target = aim_net.and_then(|aim| nearest_free_doorway(aim, replicated));
    match target {
        Some((doorway_id, position, doorway_yaw)) => {
            let yaw = snap_yaw_quarter_turn(
                doorway_yaw
                    + if placement.door_flip {
                        std::f32::consts::PI
                    } else {
                        0.0
                    },
            );
            let in_reach = player_feet.is_some_and(|feet| within_reach(position, feet));
            placement.world_position = Some(Vec3::new(position.x, position.y, position.z));
            placement.yaw = yaw;
            placement.valid = in_reach;
            placement.door_target = Some(doorway_id);
        }
        None => {
            placement.world_position = aim;
            placement.valid = false;
            placement.door_target = None;
        }
    }
}

/// Torch ghost: free-view placement with no socket snapping. The look ray is
/// tested against wall-like building pieces and the ground/platform; whichever
/// is nearer wins. A wall hit mounts the torch (tilted out along the wall's
/// outward normal); otherwise it stands upright on the surface.
fn update_torch_placement(
    placement: &mut DeployablePlacementState,
    camera_transform: &GlobalTransform,
    player_feet: Option<Vec3>,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) {
    let origin = camera_transform.translation();
    let forward = camera_transform.forward().as_vec3();
    placement.door_target = None;

    let wall = nearest_wall_hit(origin, forward, replicated);
    let ground = surface_under_aim(camera_transform, replicated)
        .map(|point| ((point - origin).dot(forward), point));

    let pick_wall = match (&wall, &ground) {
        (Some((wall_t, _, _)), Some((ground_t, _))) => wall_t < ground_t,
        (Some(_), None) => true,
        _ => false,
    };

    if pick_wall {
        let (_, point, normal) = wall.expect("pick_wall implies a wall hit");
        // Nudge the base a hair off the wall so it doesn't z-fight the masonry.
        let position = point + normal * 0.04;
        placement.world_position = Some(position);
        // yaw points away from the wall (the outward normal's heading).
        placement.yaw = normal.x.atan2(normal.z);
        placement.wall_mounted = true;
        placement.valid = torch_in_reach(position, player_feet);
        return;
    }

    if let Some((_, point)) = ground {
        placement.world_position = Some(point);
        placement.wall_mounted = false;
        // A torch shaft is radially symmetric, so just face the player for a
        // tidy default; the exact floor yaw doesn't matter.
        if let Some(feet) = player_feet
            && let Some(yaw) = yaw_facing_player(point, feet)
        {
            placement.yaw = yaw;
        }
        placement.valid = torch_in_reach(point, player_feet);
        return;
    }

    placement.world_position = None;
    placement.valid = false;
    placement.wall_mounted = false;
}

fn torch_in_reach(position: Vec3, player_feet: Option<Vec3>) -> bool {
    player_feet.is_some_and(|feet| within_reach(position, feet))
}

/// React to placement input: left-click commits, held right-mouse
/// freezes the spot and turns mouse motion into rotation (classic
/// deployables), right-click flips the door ghost, R nudges by 90°.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn placement_input_system(
    mouse_motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut placement: ResMut<DeployablePlacementState>,
    plan: Res<BuildingPlanState>,
    wheel: Res<WheelMenuState>,
    mut runtime: ResMut<ClientRuntime>,
    mut error_toasts: MessageWriter<ClientErrorToast>,
    mut menu: ResMut<MenuState>,
    local_player: Res<LocalPlayerState>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    analytics: Res<Analytics>,
) {
    if !gameplay_accepts_input(&menu, &primary_window) || wheel.blocks_input() {
        return;
    }
    let intent = current_ghost_intent(&local_player, &menu, &plan);
    let Some(intent) = intent else {
        return;
    };

    match &intent {
        GhostIntent::Deployable(_, _) => {
            // Hold right mouse to take manual control: the camera is
            // frozen (see `mouse_look_system`) and the spot is frozen
            // (see the ghost system), so horizontal mouse motion only
            // turns the ghost.
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
        }
        GhostIntent::Building(_) => {
            // Free foundations rotate on the quarter-turn grid; snapped
            // poses own their orientation, so R is only felt off-grid.
            if keys.just_pressed(KeyCode::KeyR) {
                placement.yaw = snap_yaw_quarter_turn(placement.yaw + std::f32::consts::FRAC_PI_2);
                placement.manual_yaw = true;
            }
        }
        GhostIntent::Door(_) => {
            // Right-click flips hinge + swing side (a half-turn of the
            // ghost; the swing-arc indicator shows the result).
            if mouse.just_pressed(MouseButton::Right) {
                placement.door_flip = !placement.door_flip;
            }
        }
        // The torch pose is fully driven by the aim (wall normal or ground),
        // and the haft is radially symmetric, so there's nothing to rotate.
        GhostIntent::Torch(_, _) => {}
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(position) = placement.world_position else {
        return;
    };
    if !placement.valid {
        return;
    }

    match intent {
        GhostIntent::Deployable(item_id, _) => {
            let kind_label = deployable_kind_label(&item_id);
            send_place_deployable_command(
                &mut runtime,
                &mut error_toasts,
                PlaceDeployableCommand {
                    item_id,
                    position: Vec3Net::from(position),
                    yaw: placement.yaw,
                    wall_mounted: false,
                },
            );
            placement.manual_yaw = false;
            if let Some(kind) = kind_label {
                analytics.track(Event::DeployablePlaced { kind });
            }
        }
        GhostIntent::Building(piece) => {
            send_place_building_command(
                &mut runtime,
                &mut error_toasts,
                PlaceBuildingCommand {
                    piece,
                    position: Vec3Net::from(position),
                    yaw: placement.yaw,
                },
            );
            analytics.track(Event::DeployablePlaced {
                kind: piece.label().to_lowercase(),
            });
        }
        GhostIntent::Door(variant) => {
            let Some(doorway_id) = placement.door_target else {
                return;
            };
            // The door only ships once the player confirms a lock code;
            // cancelling the prompt places nothing.
            menu.text_prompt = Some(TextPrompt::new(TextPromptKind::DoorSetCode {
                doorway_id,
                variant,
                flip: placement.door_flip,
            }));
        }
        GhostIntent::Torch(item_id, _) => {
            send_place_deployable_command(
                &mut runtime,
                &mut error_toasts,
                PlaceDeployableCommand {
                    item_id,
                    position: Vec3Net::from(position),
                    yaw: placement.yaw,
                    wall_mounted: placement.wall_mounted,
                },
            );
            analytics.track(Event::DeployablePlaced {
                kind: "torch".to_owned(),
            });
        }
    }
}

fn send_place_building_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut MessageWriter<ClientErrorToast>,
    command: PlaceBuildingCommand,
) {
    use crate::app::state::ErrorToastSink;
    let Some(session) = runtime.session.as_mut() else {
        error_toasts.push_error("place failed: not connected".to_owned());
        return;
    };
    if let Err(error) = session.send(crate::protocol::ClientMessage::PlaceBuilding(command)) {
        error_toasts.push_error(format!("place failed: {error}"));
    }
}

pub(super) fn deployable_kind_label(item_id: &ItemId) -> Option<String> {
    let definition = item_definition(item_id)?;
    let profile = definition.deployable?;
    Some(match profile.kind {
        DeployableKind::Workbench { .. } => "workbench".to_owned(),
        DeployableKind::Furnace { .. } => "furnace".to_owned(),
        DeployableKind::Building { piece, .. } => piece.label().to_lowercase(),
        DeployableKind::Door { .. } => "door".to_owned(),
        DeployableKind::SleepingBag => "sleeping_bag".to_owned(),
        DeployableKind::StorageBox { tier } => {
            if tier >= 2 {
                "storage_box_large".to_owned()
            } else {
                "storage_box_small".to_owned()
            }
        }
        DeployableKind::Torch { .. } => "torch".to_owned(),
        DeployableKind::ToolCupboard => "tool_cupboard".to_owned(),
        // Not player-placeable, but the match must stay total.
        DeployableKind::RuinCache => "ruin_cache".to_owned(),
        // Placed charges; the item id is the label so the ghost/model lookup
        // keys off the specific charge. Full charge VFX/model lands with the
        // explosive VFX package.
        DeployableKind::Explosive { .. } => definition.id.to_owned(),
    })
}

fn current_item_id(local_player: &LocalPlayerState) -> Option<ItemId> {
    local_player
        .private
        .as_ref()?
        .inventory
        .active_actionbar_stack()
        .map(|stack| stack.item_id.clone())
}

/// What the active actionbar item wants the ghost to preview. The same
/// modal-suppression rules as the original `current_deployable` apply.
pub(super) fn current_ghost_intent(
    local_player: &LocalPlayerState,
    menu: &MenuState,
    plan: &BuildingPlanState,
) -> Option<GhostIntent> {
    if menu.screen != Screen::InGame || menu.pause_open {
        return None;
    }
    // Any modal-open state (inventory, crafting, chat, text prompt)
    // suppresses the ghost so we don't draw it while the player can't
    // actually click to place.
    if menu.inventory_open || menu.crafting_open || menu.chat_open || menu.text_prompt.is_some() {
        return None;
    }
    let stack = local_player
        .private
        .as_ref()?
        .inventory
        .active_actionbar_stack()?;
    if stack.item_id.as_ref() == BUILDING_PLAN_ID {
        return Some(GhostIntent::Building(plan.selected_piece));
    }
    let definition = item_definition(&stack.item_id)?;
    let profile = definition.deployable?;
    if let DeployableKind::Door { variant } = profile.kind {
        return Some(GhostIntent::Door(variant));
    }
    if matches!(profile.kind, DeployableKind::Torch { .. }) {
        return Some(GhostIntent::Torch(stack.item_id.clone(), profile));
    }
    // Hidden building items can't be held, but keep the gate total: only
    // free-placeable kinds ride the classic flow.
    if matches!(profile.kind, DeployableKind::Building { .. }) {
        return None;
    }
    Some(GhostIntent::Deployable(stack.item_id.clone(), profile))
}

/// Aim point for a free foundation, with aim-driven height. Looking at
/// the ground inside reach places at ground level, exactly the old
/// behaviour. Raising the aim past the reach ring keeps the ghost on the
/// ring and lifts it continuously with the look ray, up to
/// [`FOUNDATION_RAISE_MAX_M`], which is how stilted platforms are
/// placed: look where you want the slab, then look up to raise it.
fn foundation_aim(camera_transform: &GlobalTransform, player_feet: Option<Vec3>) -> Option<Vec3> {
    let origin = camera_transform.translation();
    let forward = camera_transform.forward().as_vec3();
    let feet = player_feet?;
    if forward.y < -1e-3 {
        let t = -origin.y / forward.y;
        if t > 0.0 && t <= 60.0 {
            let hit = origin + forward * t;
            if within_reach(hit, feet) {
                return Some(Vec3::new(hit.x, 0.0, hit.z));
            }
        }
    }
    // The ground hit is out of reach (or the player is looking level or
    // up): clamp the aim to the reach ring and take the ray's height
    // there. As the look ray crosses the ring higher and higher, the
    // slab rises with it; the band cap keeps it honest.
    let horizontal = (forward.x * forward.x + forward.z * forward.z).sqrt();
    if horizontal < 1e-3 {
        return None;
    }
    // A hair inside the ring so the reach check never flickers on f32
    // rounding.
    let t = (PLACEMENT_REACH_M - 0.01) / horizontal;
    let point = origin + forward * t;
    let y = point.y.clamp(0.0, FOUNDATION_RAISE_MAX_M);
    Some(Vec3::new(point.x, y, point.z))
}

/// Where the look ray lands for a free deployable: the ground plane or
/// the walkable top of a building platform (foundation or ceiling),
/// whichever the ray reaches first. This is what lets furnaces, beds,
/// and boxes stand on foundations and upstairs floors, not just dirt.
fn surface_under_aim(
    camera_transform: &GlobalTransform,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> Option<Vec3> {
    let origin = camera_transform.translation();
    let forward = camera_transform.forward().as_vec3();
    if forward.y >= -1e-3 {
        return None;
    }

    let mut best: Option<(f32, Vec3)> = None;
    let ground_t = -origin.y / forward.y;
    if ground_t > 0.0 && ground_t <= 50.0 {
        let hit = origin + forward * ground_t;
        best = Some((ground_t, Vec3::new(hit.x, 0.0, hit.z)));
    }
    let half = crate::building::FOUNDATION_SIZE_M / 2.0;
    for (meta, transform, _) in replicated.iter() {
        let DeployableKind::Building { piece, .. } = meta.kind else {
            continue;
        };
        let Some(top_offset) = crate::building::platform_top_offset(piece) else {
            continue;
        };
        let top = transform.position.y + top_offset;
        // Only surfaces below the camera count: hitting a slab from
        // underneath would place the ghost on the floor above the
        // player's head, through the ceiling.
        if origin.y <= top {
            continue;
        }
        let t = (top - origin.y) / forward.y;
        if t <= 0.0 || t > 50.0 {
            continue;
        }
        let hit = origin + forward * t;
        if (hit.x - transform.position.x).abs() <= half
            && (hit.z - transform.position.z).abs() <= half
            && best.is_none_or(|(best_t, _)| t < best_t)
        {
            best = Some((t, Vec3::new(hit.x, top, hit.z)));
        }
    }
    best.map(|(_, point)| point)
}

pub(super) fn ground_under_aim(camera_transform: &GlobalTransform) -> Option<Vec3> {
    let origin = camera_transform.translation();
    let forward = camera_transform.forward().as_vec3();
    // Clamp slightly steeper than vertical so the ghost doesn't latch
    // onto a horizon-far point when the player looks straight ahead,
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

fn is_free_placement_valid(
    target: Vec3,
    profile: DeployableProfile,
    player_feet: Option<Vec3>,
    replicated: &Query<(&Deployable, &DeployableTransform, &DeployableStability)>,
) -> bool {
    let Some(player_feet) = player_feet else {
        return false;
    };
    if !within_reach(target, player_feet) {
        return false;
    }
    // Real 3D box check against every replicated deployable, buildings
    // included; the millimetre epsilon means standing exactly on a
    // platform top (touching faces) is not a collision. This replaced
    // an XZ-only heuristic that could never have allowed placing on a
    // foundation (the foundation itself always "overlapped" in XZ).
    let candidate = crate::world::WorldBlock::new(
        Vec3Net::new(target.x, target.y + profile.collider_half_height, target.z),
        Vec3Net::new(
            profile.collider_half_width,
            profile.collider_half_height,
            profile.collider_half_width,
        ),
    );
    !any_replicated_overlap(&[candidate], replicated, false)
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
    ghost_meshes: &ChargeGhostMeshes,
    placement: &DeployablePlacementState,
    kind: DeployableKind,
    wall_platforms: &[ClaimPlatform],
) {
    let Some(position) = placement.world_position else {
        despawn_ghost(commands, ghosts);
        return;
    };
    // A perimeter wall renders nudged inward so its outer face is flush with
    // the foundation edge (matching the placed piece); the collider/snap
    // position stays on the edge.
    let position = match kind {
        DeployableKind::Building { piece, .. } if piece.is_wall_like() => {
            let offset = crate::building::wall_face_inset_offset(
                Vec3Net::new(position.x, position.y, position.z),
                placement.yaw,
                wall_platforms,
            );
            position + Vec3::new(offset.x, offset.y, offset.z)
        }
        _ => position,
    };
    let mesh = match kind {
        DeployableKind::Workbench { tier } => assets.workbench_mesh(tier),
        DeployableKind::Furnace { .. } => assets.furnace_mesh.clone(),
        DeployableKind::Building { piece, tier } => assets.building_mesh(piece, tier),
        DeployableKind::Door { .. } => assets.door_ghost_mesh.clone(),
        DeployableKind::SleepingBag => assets.sleeping_bag_mesh.clone(),
        DeployableKind::StorageBox { tier } => assets.storage_box_mesh(tier),
        DeployableKind::Torch { .. } => assets.torch_mesh.clone(),
        DeployableKind::ToolCupboard => assets.tool_cupboard_mesh.clone(),
        // Never player-placeable, so no ghost is ever previewed for it; the
        // arm exists only to keep the match total.
        DeployableKind::RuinCache => assets.ruin_cache_mesh.clone(),
        // A charge's placement ghost is its real body mesh (primitive 0), so the
        // translucent preview matches the placed prop. It binds the
        // alpha-saturated ghost CLONE: the charge glbs carry COLOR_0 alpha 0
        // (the ember-glow mask convention), which would multiply the ghost
        // material fully invisible. Falls back to the source mesh for the
        // frame or two before the clone is built.
        DeployableKind::Explosive { kind } => {
            let source = super::charge_body_mesh(assets, kind);
            ghost_meshes
                .by_source
                .get(&source.id())
                .cloned()
                .unwrap_or(source)
        }
    };
    // The small charges run the punchier ghost variant (higher alpha, hotter
    // emissive) so a knee-high keg/satchel preview reads through grass; every
    // bigger structure keeps the subtle shared tint.
    let material = match (kind, placement.valid) {
        (DeployableKind::Explosive { .. }, true) => assets.ghost_valid_charge_material.clone(),
        (DeployableKind::Explosive { .. }, false) => assets.ghost_invalid_charge_material.clone(),
        (_, true) => assets.ghost_valid_material.clone(),
        (_, false) => assets.ghost_invalid_material.clone(),
    };
    let transform = deployable_visual_transform(position, placement.yaw, kind);

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
        // a hard floor blob, disable it so the ghost looks unbaked.
        NotShadowCaster,
    ));
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
    if menu.inventory_open || menu.crafting_open || menu.chat_open || menu.text_prompt.is_some() {
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
