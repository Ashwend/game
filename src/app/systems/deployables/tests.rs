use super::*;

use super::placement::{
    current_deployable, deployable_kind_label, ground_under_aim, wrap_angle, yaw_facing_player,
};

use crate::app::EYE_HEIGHT;
use crate::app::state::{MenuState, Screen};

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
            applied_action_seq: 0,
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
            applied_action_seq: 0,
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
