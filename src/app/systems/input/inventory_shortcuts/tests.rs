use super::predict::predict_resource_node_pickup;
use super::send::send_gameplay_message;
use super::swing::{
    equipped_swing, equipped_tool_can_harvest_target, equipped_tool_profile,
    resource_target_anchor, resource_target_is_crude, resource_target_model,
    resource_target_surface, swing_spray_direction,
};
use super::*;
use crate::app::state::LocalPlayerState;
use crate::app::state::{ClientRuntime, ClientSettings, PickupTargetState, PredictionState};
use crate::items::{
    BASIC_HATCHET_ID, BASIC_PICKAXE_ID, IRON_MACE_ID, ItemModel, STONE_SPEAR_ID, ToolKind, WOOD_ID,
    WOODEN_CLUB_ID,
};
use crate::protocol::{ClientMessage, InventoryCommand, ItemStack, PlayerInventoryState, Vec3Net};
use crate::resource_nodes::{
    BRANCH_PILE_NODE_ID, COAL_NODE_ID, PINE_TREE_NODE_ID, SURFACE_STONE_NODE_ID,
};
use crate::server::{PlayerLifecycle, PlayerPrivate};
use bevy::input::ButtonInput;
use bevy::prelude::{KeyCode, Vec3};

fn local_player_holding(item_id: Option<&str>) -> LocalPlayerState {
    let mut inventory = PlayerInventoryState::empty();
    if let Some(id) = item_id {
        inventory.actionbar_slots[0] = Some(ItemStack::new(id, 1));
    }
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

fn target_for_node(node_id: u64, definition_id: &str) -> PickupTargetState {
    PickupTargetState {
        resource_node_id: Some(crate::protocol::ResourceNodeId(node_id)),
        resource_definition_id: Some(definition_id.to_owned()),
        world_position: Some(Vec3Net::new(1.0, 2.0, 3.0)),
        ..Default::default()
    }
}

#[test]
fn equipped_swing_resolves_tools_and_weapons_but_not_bare_or_inert_items() {
    // A tool resolves to its swing archetype (the same value the server derives
    // and stamps as the peer-visible swing model).
    let with_axe = local_player_holding(Some(BASIC_HATCHET_ID));
    assert_eq!(equipped_swing(&with_axe), Some(ItemModel::Hatchet));

    let with_pick = local_player_holding(Some(BASIC_PICKAXE_ID));
    assert_eq!(equipped_swing(&with_pick), Some(ItemModel::Pickaxe));

    // A weapon resolves to its own swing archetype, which is exactly its wire
    // impact identity now that the wire carries `ItemModel`.
    let with_club = local_player_holding(Some(WOODEN_CLUB_ID));
    assert_eq!(equipped_swing(&with_club), Some(ItemModel::Club));
    let with_spear = local_player_holding(Some(STONE_SPEAR_ID));
    assert_eq!(equipped_swing(&with_spear), Some(ItemModel::Spear));
    let with_mace = local_player_holding(Some(IRON_MACE_ID));
    assert_eq!(equipped_swing(&with_mace), Some(ItemModel::Mace));

    // Bare hands -> no swing.
    let empty = local_player_holding(None);
    assert_eq!(equipped_swing(&empty), None);

    // A non-tool, non-weapon item (wood) -> no swing.
    let with_wood = local_player_holding(Some(WOOD_ID));
    assert_eq!(equipped_swing(&with_wood), None);
}

#[test]
fn ranged_weapons_route_to_the_draw_loop_never_the_melee_swing() {
    use super::ranged::held_ranged;
    use crate::items::{ARROW_ID, CROSSBOW_ID, WOODEN_BOW_ID};

    // A held bow / crossbow resolves through `held_ranged` (the draw loop's gate)
    // and NOT through `equipped_swing` (a ranged weapon has no Tool/WeaponProfile),
    // which is exactly the branch decision the input system makes each frame: the
    // ranged path runs, the melee swing machine never starts.
    for id in [WOODEN_BOW_ID, CROSSBOW_ID] {
        let holding = local_player_holding(Some(id));
        assert_eq!(
            equipped_swing(&holding),
            None,
            "{id} must not start a melee swing"
        );
        let (weapon_id, profile, has_ammo) =
            held_ranged(&holding).expect("resolves the ranged profile");
        assert_eq!(
            &*weapon_id, id,
            "held_ranged carries the weapon id for analytics"
        );
        assert_eq!(profile.ammo_item, ARROW_ID);
        assert!(!has_ammo, "no arrows in the bag yet");
    }

    // With arrows in the bag the ammo gate opens.
    let mut with_ammo = local_player_holding(Some(WOODEN_BOW_ID));
    if let Some(private) = with_ammo.private.as_mut() {
        private.inventory.inventory_slots[0] = Some(ItemStack::new(ARROW_ID, 8));
    }
    let (_, _, has_ammo) = held_ranged(&with_ammo).expect("still holding the bow");
    assert!(has_ammo);

    // A melee weapon / tool resolves no ranged profile, so the melee path runs.
    let with_club = local_player_holding(Some(WOODEN_CLUB_ID));
    assert!(held_ranged(&with_club).is_none());
}

#[test]
fn equipped_tool_profile_falls_back_to_hands() {
    // Empty hand falls back to the synthesized HANDS_TOOL.
    let empty = local_player_holding(None);
    assert_eq!(equipped_tool_profile(&empty).kind, ToolKind::Hands);

    // A real tool returns its own profile.
    let with_pick = local_player_holding(Some(BASIC_PICKAXE_ID));
    assert_eq!(equipped_tool_profile(&with_pick).kind, ToolKind::Pickaxe);
}

#[test]
fn predict_resource_node_pickup_full_drain_into_empty_bag_predicts_and_hides() {
    let local = local_player_holding(None);
    let mut prediction = PredictionState::default();
    let mut target = target_for_node(7, BRANCH_PILE_NODE_ID);
    target.resource_storage = vec![ItemStack::new(WOOD_ID, 3)];

    let seq = predict_resource_node_pickup(
        &mut prediction,
        &local,
        crate::protocol::ResourceNodeId(7),
        &target,
    );
    assert_ne!(
        seq, 0,
        "a node draining fully into an empty bag is predicted"
    );
    assert!(
        prediction.is_node_hidden(crate::protocol::ResourceNodeId(7)),
        "a full drain hides the node visual"
    );

    // The whole node lands in the bag immediately.
    let effective = prediction.rebuild_effective(&PlayerInventoryState::empty());
    let total: u16 = effective
        .inventory_slots
        .iter()
        .chain(effective.actionbar_slots.iter())
        .filter_map(|slot| slot.as_ref().map(|s| s.quantity))
        .sum();
    assert_eq!(total, 3);
}

#[test]
fn predict_resource_node_pickup_empty_storage_is_not_predicted() {
    // A node with nothing left to give predicts nothing and stays
    // visible (the server would no-op too).
    let local = local_player_holding(None);
    let mut prediction = PredictionState::default();
    let target = target_for_node(9, BRANCH_PILE_NODE_ID);

    let seq = predict_resource_node_pickup(
        &mut prediction,
        &local,
        crate::protocol::ResourceNodeId(9),
        &target,
    );
    assert_eq!(seq, 0);
    assert!(!prediction.is_node_hidden(crate::protocol::ResourceNodeId(9)));
    assert!(prediction.is_idle());
}

#[test]
fn resource_target_is_crude_only_for_hand_harvestable_nodes() {
    // Branch piles + surface stones are crude (Hands).
    let branch = target_for_node(1, BRANCH_PILE_NODE_ID);
    assert!(resource_target_is_crude(&branch));
    let stone = target_for_node(2, SURFACE_STONE_NODE_ID);
    assert!(resource_target_is_crude(&stone));

    // Ore + trees are not crude.
    let ore = target_for_node(3, COAL_NODE_ID);
    assert!(!resource_target_is_crude(&ore));
    let tree = target_for_node(4, PINE_TREE_NODE_ID);
    assert!(!resource_target_is_crude(&tree));

    // Missing / unknown definition -> false.
    assert!(!resource_target_is_crude(&PickupTargetState::default()));
    let bogus = target_for_node(5, "not_a_real_node");
    assert!(!resource_target_is_crude(&bogus));
}

#[test]
fn harvest_check_matches_tool_to_node_requirement() {
    // Pickaxe vs ore vein -> allowed.
    let pick = local_player_holding(Some(BASIC_PICKAXE_ID));
    let ore = target_for_node(1, COAL_NODE_ID);
    assert!(equipped_tool_can_harvest_target(&pick, &ore));

    // Hatchet vs ore vein -> rejected (wrong tool).
    let axe = local_player_holding(Some(BASIC_HATCHET_ID));
    assert!(!equipped_tool_can_harvest_target(&axe, &ore));

    // Hatchet vs tree -> allowed.
    let tree = target_for_node(2, PINE_TREE_NODE_ID);
    assert!(equipped_tool_can_harvest_target(&axe, &tree));

    // Crude nodes are E-pickup-only: a swing (even bare hands) is
    // rejected so the player learns the quick-pickup key.
    let empty = local_player_holding(None);
    let branch = target_for_node(3, BRANCH_PILE_NODE_ID);
    assert!(!equipped_tool_can_harvest_target(&empty, &branch));
    // A real tool can't swing-harvest a crude node either.
    assert!(!equipped_tool_can_harvest_target(&pick, &branch));

    // Bare hands vs ore -> rejected.
    assert!(!equipped_tool_can_harvest_target(&empty, &ore));

    // No definition id on the target -> rejected.
    assert!(!equipped_tool_can_harvest_target(
        &pick,
        &PickupTargetState::default()
    ));
}

#[test]
fn resource_target_anchor_requires_matching_node_id() {
    let target = target_for_node(42, COAL_NODE_ID);
    let anchor = resource_target_anchor(&target, crate::protocol::ResourceNodeId(42))
        .expect("matching id resolves an anchor");
    assert_eq!(anchor, Vec3::new(1.0, 2.0, 3.0));

    // Mismatched id -> None even though a world position exists.
    assert!(resource_target_anchor(&target, crate::protocol::ResourceNodeId(7)).is_none());

    // No world position -> None.
    let mut no_pos = target_for_node(42, COAL_NODE_ID);
    no_pos.world_position = None;
    assert!(resource_target_anchor(&no_pos, crate::protocol::ResourceNodeId(42)).is_none());
}

#[test]
fn resource_target_model_resolves_definition_model() {
    let tree = target_for_node(1, PINE_TREE_NODE_ID);
    assert!(resource_target_model(&tree).is_some());
    // Unknown / missing definition -> None.
    assert!(resource_target_model(&PickupTargetState::default()).is_none());
}

#[test]
fn resource_target_surface_resolves_only_for_known_definition() {
    let ore = target_for_node(1, COAL_NODE_ID);
    assert!(resource_target_surface(&ore).is_some());
    assert!(resource_target_surface(&PickupTargetState::default()).is_none());
}

#[test]
fn swing_spray_direction_defaults_up_without_local_view() {
    // A default runtime has no predicted local player, so the spray
    // falls back to straight up.
    let runtime = ClientRuntime::default();
    let dir = swing_spray_direction(&runtime, Vec3::new(5.0, 0.0, 5.0));
    assert_eq!(dir, Vec3::Y);
}

#[test]
fn actionbar_key_pressed_out_of_range_slot_is_false() {
    let keys = ButtonInput::<KeyCode>::default();
    let settings = ClientSettings::default();
    // Slot index past the actionbar count never maps to an action.
    assert!(!actionbar_key_pressed(
        &keys,
        &settings,
        ACTIONBAR_SLOT_COUNT + 5
    ));
}

#[test]
fn send_inventory_command_reports_not_connected() {
    let mut runtime = ClientRuntime::default();
    let mut sink: Vec<String> = Vec::new();
    send_inventory_command(
        &mut runtime,
        &mut sink,
        InventoryCommand::SelectActionbarSlot { slot: 0 },
    );
    assert_eq!(sink.len(), 1);
    assert!(sink[0].contains("not connected"));
    assert!(sink[0].starts_with("inventory command failed"));
}

#[test]
fn send_furnace_command_reports_not_connected() {
    let mut runtime = ClientRuntime::default();
    let mut sink: Vec<String> = Vec::new();
    send_furnace_command(
        &mut runtime,
        &mut sink,
        crate::protocol::FurnaceCommand::Open {
            id: crate::protocol::DeployedEntityId(1),
        },
    );
    assert_eq!(sink.len(), 1);
    assert!(sink[0].contains("not connected"));
}

#[test]
fn send_gameplay_message_pushes_to_both_runtime_and_sink() {
    let mut runtime = ClientRuntime::default();
    let mut sink: Vec<String> = Vec::new();
    send_gameplay_message(
        &mut runtime,
        &mut sink,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
        "test label",
    );
    assert_eq!(sink.len(), 1);
    assert!(sink[0].starts_with("test label failed: not connected"));
    // The runtime also records the error message.
    assert!(!runtime.messages.is_empty());
}

#[test]
fn dead_lifecycle_matches_dying_check_used_by_the_swing_gate() {
    // The swing gate treats a Dead lifecycle as "can't swing"; verify
    // the lifecycle shape we rely on.
    let mut player = local_player_holding(Some(BASIC_HATCHET_ID));
    player.lifecycle = Some(PlayerLifecycle::Dead {
        since_tick: 0,
        killer: None,
    });
    assert!(matches!(
        player.lifecycle,
        Some(PlayerLifecycle::Dead { .. })
    ));
    // Alive (or none) does not.
    let alive = local_player_holding(Some(BASIC_HATCHET_ID));
    assert!(!matches!(
        alive.lifecycle,
        Some(PlayerLifecycle::Dead { .. })
    ));
}

#[test]
fn wheel_step_signs_vertical_scroll_both_ways() {
    // Plain (no-shift) scroll lands on the Y axis with a real sign.
    assert_eq!(wheel_step([(0.0, 1.0)].into_iter()), 1);
    assert_eq!(wheel_step([(0.0, -1.0)].into_iter()), -1);
    // Pixel-unit trackpad deltas accumulate; only the sign of the total matters.
    assert_eq!(wheel_step([(0.0, 3.5), (0.0, 2.1)].into_iter()), 1);
}

#[test]
fn wheel_step_reads_shift_scroll_off_the_x_axis_both_ways() {
    // macOS delivers Shift+scroll as horizontal: magnitude on X, Y == 0.0.
    // The old `event.y.signum()` mapped y==0.0 to +1 and locked the hotbar to
    // one direction; reading the X axis restores both directions.
    assert_eq!(wheel_step([(1.0, 0.0)].into_iter()), 1);
    assert_eq!(wheel_step([(-1.0, 0.0)].into_iter()), -1);
}

#[test]
fn wheel_step_is_zero_for_an_empty_or_null_frame() {
    // No events, and an exactly-zero delta, both mean "no step" (the case the
    // signum-of-zero bug used to mis-handle as +1).
    assert_eq!(wheel_step(std::iter::empty()), 0);
    assert_eq!(wheel_step([(0.0, 0.0)].into_iter()), 0);
}
