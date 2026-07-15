use super::*;

use super::placement::{
    GhostIntent, current_ghost_intent, deployable_kind_label, ground_under_aim, wrap_angle,
    yaw_facing_player,
};

use crate::app::EYE_HEIGHT;
use crate::app::state::{BuildingPlanState, MenuState, Screen};

#[test]
fn ground_hit_returns_none_for_horizon_aim() {
    let transform = GlobalTransform::from(Transform::from_xyz(0.0, EYE_HEIGHT, 0.0));
    assert!(ground_under_aim(&transform).is_none());
}

#[test]
fn ground_hit_returns_point_when_looking_down() {
    // 45° downward look from eye height, should hit ground at the
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
    let yaw = yaw_facing_player(Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)).expect("distinct positions");
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
    assert!((wrap_angle(-0.5 * std::f32::consts::PI) + 0.5 * std::f32::consts::PI).abs() < 1e-4);
}

#[test]
fn ground_under_aim_rejects_upward_and_far_rays() {
    // Looking up at the sky never hits the y=0 plane.
    let up = GlobalTransform::from(
        Transform::from_xyz(0.0, EYE_HEIGHT, 0.0).looking_at(Vec3::new(0.0, 10.0, -2.0), Vec3::Y),
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
        id: crate::protocol::DeployedEntityId(1),
        item_id: intern_item_id(WORKBENCH_T1_ID),
        kind: DeployableKind::Workbench { tier: 1 },
        max_health: 500,
        owner: None,
        placed_at_tick: 0,
    };
    let transform = DeployableTransform {
        position: Vec3Net::new(2.0, 0.0, -3.0),
        yaw: 0.0,
    };
    let profile = item_definition(&meta.item_id).unwrap().deployable.unwrap();
    let blocks = deployable_colliders(&meta, &transform, false);
    assert_eq!(blocks.len(), 1, "classic deployables are one box");
    let block = blocks[0];
    // Center is raised by the collider half-height off the ground.
    assert!((block.center.y - profile.collider_half_height).abs() < 1e-4);
    assert!((block.center.x - 2.0).abs() < 1e-4);

    // Unknown item id -> no collider rather than a panic.
    let unknown = Deployable {
        id: crate::protocol::DeployedEntityId(2),
        item_id: intern_item_id("not_a_real_item"),
        kind: DeployableKind::Workbench { tier: 1 },
        max_health: 1,
        owner: None,
        placed_at_tick: 0,
    };
    assert!(deployable_colliders(&unknown, &transform, false).is_empty());
}

#[test]
fn door_colliders_follow_the_swing_and_doorways_stay_passable() {
    use crate::items::{HEWN_LOG_DOOR_ID, intern_item_id};
    let door = Deployable {
        id: crate::protocol::DeployedEntityId(3),
        item_id: intern_item_id(HEWN_LOG_DOOR_ID),
        kind: DeployableKind::Door {
            variant: crate::items::DoorVariant::HewnLog,
        },
        max_health: 1,
        owner: Some(crate::protocol::AccountId(1)),
        placed_at_tick: 0,
    };
    let transform = DeployableTransform {
        position: Vec3Net::new(0.0, 0.5, 0.0),
        yaw: 0.0,
    };
    let closed = deployable_colliders(&door, &transform, false);
    assert_eq!(closed.len(), 1);
    assert!(
        closed[0].center.z.abs() < 1e-6,
        "closed panel fills the opening plane"
    );
    // An open door keeps a collider, moved to the swung panel's pose:
    // clear of the opening (the -X hinge side) and out along +Z.
    let open = deployable_colliders(&door, &transform, true);
    assert_eq!(open.len(), 1);
    assert!(
        open[0].max().x < -0.4,
        "open panel leaves the opening passable"
    );
    assert!(open[0].center.z > 0.3, "open panel sits on the swing side");

    let doorway = Deployable {
        id: crate::protocol::DeployedEntityId(4),
        item_id: intern_item_id(crate::building::BUILDING_DOORWAY_ITEM_ID),
        kind: DeployableKind::Building {
            piece: crate::building::BuildingPiece::Doorway,
            tier: crate::building::BuildingTier::Sticks,
        },
        max_health: 1,
        owner: Some(crate::protocol::AccountId(1)),
        placed_at_tick: 0,
    };
    // Two jambs + header, the opening itself stays clear.
    assert_eq!(deployable_colliders(&doorway, &transform, false).len(), 3);
}

#[test]
fn deployable_transform_applies_position_and_yaw() {
    let yaw = std::f32::consts::FRAC_PI_2;
    let transform = deployable_transform(Vec3::new(1.0, 2.0, 3.0), yaw);
    assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
    let expected = Quat::from_rotation_y(yaw);
    assert!(transform.rotation.dot(expected).abs() > 1.0 - 1e-5);
}

fn player_holding(item_id: &str) -> crate::app::state::LocalPlayerState {
    use crate::app::state::LocalPlayerState;
    use crate::protocol::{ItemStack, PlayerInventoryState};
    use crate::server::PlayerPrivate;

    let mut inventory = PlayerInventoryState::empty();
    inventory.actionbar_slots[0] = Some(ItemStack::new(item_id, 1));
    LocalPlayerState {
        entity: None,
        private: Some(PlayerPrivate {
            inventory,
            crafting: Default::default(),
            open_furnace: None,
            open_loot_bag: None,
            open_workbench: None,
            last_processed_input: 0,
            applied_action_seq: 0,
            run_speed_multiplier: 1.0,
        }),
        lifecycle: None,
    }
}

#[test]
fn ghost_intent_suppressed_by_modal_states() {
    use crate::items::WORKBENCH_T1_ID;

    let player = player_holding(WORKBENCH_T1_ID);
    let plan = BuildingPlanState::default();

    // In-game with a deployable selected -> Some.
    let in_game = MenuState {
        screen: Screen::InGame,
        ..Default::default()
    };
    assert!(matches!(
        current_ghost_intent(&player, &in_game, &plan),
        Some(GhostIntent::Deployable(_, _))
    ));

    // Inventory overlay open -> suppressed.
    let inv_open = MenuState {
        screen: Screen::InGame,
        inventory_open: true,
        ..Default::default()
    };
    assert!(current_ghost_intent(&player, &inv_open, &plan).is_none());

    // Not in game -> suppressed.
    let menu = MenuState {
        screen: Screen::MainMenu,
        ..Default::default()
    };
    assert!(current_ghost_intent(&player, &menu, &plan).is_none());
}

#[test]
fn ghost_intent_none_for_non_deployable_item() {
    let player = player_holding(crate::items::WOOD_ID);
    let in_game = MenuState {
        screen: Screen::InGame,
        ..Default::default()
    };
    assert!(current_ghost_intent(&player, &in_game, &BuildingPlanState::default()).is_none());
}

#[test]
fn ghost_intent_routes_plan_and_door_to_their_flows() {
    use crate::building::BuildingPiece;
    use crate::items::{BUILDING_PLAN_ID, HEWN_LOG_DOOR_ID};

    let in_game = MenuState {
        screen: Screen::InGame,
        ..Default::default()
    };
    let plan = BuildingPlanState {
        selected_piece: BuildingPiece::Doorway,
    };
    assert_eq!(
        current_ghost_intent(&player_holding(BUILDING_PLAN_ID), &in_game, &plan),
        Some(GhostIntent::Building(BuildingPiece::Doorway))
    );
    assert_eq!(
        current_ghost_intent(&player_holding(HEWN_LOG_DOOR_ID), &in_game, &plan),
        Some(GhostIntent::Door(crate::items::DoorVariant::HewnLog))
    );
}

#[test]
fn deployable_set_fingerprint_distinguishes_membership() {
    use crate::items::{WORKBENCH_T1_ID, intern_item_id};
    let make = |id: u64| {
        (
            Deployable {
                id: crate::protocol::DeployedEntityId(id),
                item_id: intern_item_id(WORKBENCH_T1_ID),
                kind: DeployableKind::Workbench { tier: 1 },
                max_health: 1,
                owner: None,
                placed_at_tick: 0,
            },
            DeployableTransform {
                position: Vec3Net::new(0.0, 0.0, 0.0),
                yaw: 0.0,
            },
            DeployableActive(false),
        )
    };
    let one = make(1);
    let two = make(2);

    let empty = deployable_set_fingerprint(std::iter::empty());
    let single = deployable_set_fingerprint([(&one.0, &one.1, &one.2)]);
    let pair = deployable_set_fingerprint([(&one.0, &one.1, &one.2), (&two.0, &two.1, &two.2)]);

    assert_ne!(empty, single);
    assert_ne!(single, pair);
    // Stable across recomputation.
    assert_eq!(
        single,
        deployable_set_fingerprint([(&one.0, &one.1, &one.2)])
    );
}

#[test]
fn deployable_set_fingerprint_tracks_door_open_state() {
    use crate::items::{HEWN_LOG_DOOR_ID, intern_item_id};
    let door = Deployable {
        id: crate::protocol::DeployedEntityId(9),
        item_id: intern_item_id(HEWN_LOG_DOOR_ID),
        kind: DeployableKind::Door {
            variant: crate::items::DoorVariant::HewnLog,
        },
        max_health: 1,
        owner: None,
        placed_at_tick: 0,
    };
    let transform = DeployableTransform {
        position: Vec3Net::new(0.0, 0.0, 0.0),
        yaw: 0.0,
    };
    let closed = deployable_set_fingerprint([(&door, &transform, &DeployableActive(false))]);
    let open = deployable_set_fingerprint([(&door, &transform, &DeployableActive(true))]);
    // Opening a door changes the collider set, so the fingerprint must
    // move or the grid rebuild would be skipped.
    assert_ne!(closed, open);
}

#[test]
fn only_ground_footprints_displace_grass() {
    use crate::building::{BuildingPiece, BuildingTier};
    let building = |piece| DeployableKind::Building {
        piece,
        tier: BuildingTier::Sticks,
    };
    // The foundation slab rests in the grass.
    assert!(deployable_displaces_grass(building(
        BuildingPiece::Foundation
    )));
    // Every other building piece is elevated or vertical: no grass carve.
    for piece in [
        BuildingPiece::Wall,
        BuildingPiece::WindowWall,
        BuildingPiece::Doorway,
        BuildingPiece::Ceiling,
        BuildingPiece::Stairs,
    ] {
        assert!(
            !deployable_displaces_grass(building(piece)),
            "{piece:?} must not carve grass"
        );
    }
    // A door swings, so it never carves grass.
    assert!(!deployable_displaces_grass(DeployableKind::Door {
        variant: crate::items::DoorVariant::HewnLog,
    }));
    // Classic ground deployables do carve.
    assert!(deployable_displaces_grass(DeployableKind::Workbench {
        tier: 1
    }));
    assert!(deployable_displaces_grass(DeployableKind::SleepingBag));
    assert!(deployable_displaces_grass(DeployableKind::ToolCupboard));
    assert!(deployable_displaces_grass(DeployableKind::StorageBox {
        tier: 1
    }));
}

#[test]
fn grass_displacer_fingerprint_ignores_doors_walls_and_swing() {
    use crate::building::{BuildingPiece, BuildingTier};
    use crate::items::{WORKBENCH_T1_ID, intern_item_id};
    let tf = DeployableTransform {
        position: Vec3Net::ZERO,
        yaw: 0.0,
    };
    let dep = |id, kind| Deployable {
        id,
        item_id: intern_item_id(WORKBENCH_T1_ID),
        kind,
        max_health: 1,
        owner: None,
        placed_at_tick: 0,
    };
    let foundation = dep(
        crate::protocol::DeployedEntityId(1),
        DeployableKind::Building {
            piece: BuildingPiece::Foundation,
            tier: BuildingTier::Sticks,
        },
    );
    let wall = dep(
        crate::protocol::DeployedEntityId(2),
        DeployableKind::Building {
            piece: BuildingPiece::Wall,
            tier: BuildingTier::Sticks,
        },
    );
    let door = dep(
        crate::protocol::DeployedEntityId(3),
        DeployableKind::Door {
            variant: crate::items::DoorVariant::HewnLog,
        },
    );
    let closed = DeployableActive(false);
    let open = DeployableActive(true);

    let only_foundation = grass_displacer_fingerprint([(&foundation, &tf, &closed)]);
    // A wall and a (closed) door contribute nothing to the grass set.
    let with_wall_and_door = grass_displacer_fingerprint([
        (&foundation, &tf, &closed),
        (&wall, &tf, &closed),
        (&door, &tf, &closed),
    ]);
    assert_eq!(
        only_foundation, with_wall_and_door,
        "walls and doors do not carve grass"
    );
    // Swinging the door open must NOT move the grass fingerprint (it would
    // for the world-grid fingerprint), so the grass is never re-filtered.
    let with_door_open = grass_displacer_fingerprint([
        (&foundation, &tf, &closed),
        (&wall, &tf, &closed),
        (&door, &tf, &open),
    ]);
    assert_eq!(
        with_wall_and_door, with_door_open,
        "a door swing must not re-carve grass"
    );
}

#[test]
fn resource_node_set_fingerprint_skips_colliderless_clutter() {
    // Crude clutter (hay grass) contributes no collider, so it doesn't
    // move the fingerprint, only collidable nodes (trees/ore) do.
    let hay = ResourceNode {
        id: crate::protocol::ResourceNodeId(1),
        definition_id: crate::resource_nodes::HAY_GRASS_NODE_ID.to_owned(),
        position: Vec3Net::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        dead: false,
    };
    let tree = ResourceNode {
        id: crate::protocol::ResourceNodeId(2),
        definition_id: crate::resource_nodes::PINE_TREE_NODE_ID.to_owned(),
        position: Vec3Net::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        dead: false,
    };

    let empty = resource_node_set_fingerprint(std::iter::empty());
    let only_hay = resource_node_set_fingerprint([&hay]);
    // Hay alone contributes nothing -> same as empty.
    assert_eq!(empty, only_hay);

    let with_tree = resource_node_set_fingerprint([&hay, &tree]);
    assert_ne!(empty, with_tree);
}

#[test]
fn caught_up_requires_a_first_pass_and_an_empty_spawn_queue() {
    // Same contract as the resource-node reconciler: never report
    // caught-up before the first connected pass, and any queued spawn
    // holds the world-entry gate.
    let mut visuals = DeployedEntityVisuals::default();
    assert!(
        !visuals.is_caught_up(),
        "no reconciliation pass has run yet"
    );

    visuals.applied_first_snapshot = true;
    assert!(visuals.is_caught_up());

    visuals.pending_spawns.push(PendingDeployableSpawn {
        id: crate::protocol::DeployedEntityId(1),
        replicated: Entity::PLACEHOLDER,
        kind: crate::items::DeployableKind::Furnace { tier: 0 },
        position: crate::protocol::Vec3Net::new(0.0, 0.0, 0.0),
        yaw: 0.0,
        active: false,
    });
    assert!(!visuals.is_caught_up(), "a queued spawn holds the gate");
    assert_eq!(visuals.pending_spawn_count(), 1);
}
